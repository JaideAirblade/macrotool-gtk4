//! Game Detector — polls the foreground window and detects game process.
//!
//! Direct port of the Go `internal/engine/game.go`. Tracks game PID,
//! foreground state, alive state, and resolves the game window handle for
//! background input.

use crate::config;
use crate::platform;
use crate::platform::WindowHandle;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub struct GameDetector {
    window_active: AtomicBool,
    game_foreground: AtomicBool,
    game_alive: AtomicBool,
    game_pid: AtomicU32,
    cached_game_hwnd: Mutex<u64>, // stored as u64 (0 = invalid)
    prev_active: Mutex<bool>,
    prev_foreground: Mutex<Option<(bool, bool, u32)>>,
    active_callbacks: Mutex<Vec<Box<dyn Fn(bool) + Send + Sync>>>,
    foreground_callbacks: Mutex<Vec<Box<dyn Fn(bool, bool, u32) + Send + Sync>>>,
}

impl GameDetector {
    pub fn new() -> Self {
        GameDetector {
            window_active: AtomicBool::new(false),
            game_foreground: AtomicBool::new(false),
            game_alive: AtomicBool::new(false),
            game_pid: AtomicU32::new(0),
            cached_game_hwnd: Mutex::new(0),
            prev_active: Mutex::new(false),
            prev_foreground: Mutex::new(None),
            active_callbacks: Mutex::new(Vec::new()),
            foreground_callbacks: Mutex::new(Vec::new()),
        }
    }

    pub fn register_active_callback(&self, cb: Box<dyn Fn(bool) + Send + Sync>) {
        self.active_callbacks.lock().push(cb);
    }

    pub fn register_foreground_callback(&self, cb: Box<dyn Fn(bool, bool, u32) + Send + Sync>) {
        self.foreground_callbacks.lock().push(cb);
    }

    pub fn is_active(&self, cfg: &config::Manager) -> bool {
        let s = cfg.settings();
        if !s.only_in_game {
            return true;
        }
        if s.allow_background && self.game_alive.load(Ordering::Acquire) {
            return true;
        }
        self.window_active.load(Ordering::Acquire)
    }

    pub fn is_game_alive(&self) -> bool {
        self.game_alive.load(Ordering::Acquire)
    }

    pub fn game_pid(&self) -> u32 {
        self.game_pid.load(Ordering::Acquire)
    }

    /// Invalidate the cached game window handle. Called when focus is lost.
    pub fn invalidate_hwnd_cache(&self) {
        *self.cached_game_hwnd.lock() = 0;
    }

    /// Get the cached game window handle, validated with a PID check.
    pub fn get_cached_hwnd(&self) -> WindowHandle {
        let pid = self.game_pid();
        if pid == 0 {
            return platform::INVALID_WINDOW_HANDLE;
        }

        // Fast path: check cache
        let cached = *self.cached_game_hwnd.lock();
        if cached != 0 {
            let hwnd = WindowHandle(cached);
            if platform::is_window_valid(hwnd) {
                let (_, wpid) = platform::get_window_thread_process_id(hwnd);
                if wpid == pid {
                    return hwnd;
                }
            }
        }

        // Slow path: enumerate windows to find the largest visible window for this PID
        let hwnd = self.find_game_hwnd(pid);
        *self.cached_game_hwnd.lock() = hwnd.0;
        hwnd
    }

    /// Collect all top-level windows for a given PID. Returns the largest visible
    /// window, or the first visible window if no size info is available.
    fn find_game_hwnd(&self, pid: u32) -> WindowHandle {
        let mut best = 0u64;
        let mut best_area = 0i64;
        let mut fallback = 0u64;

        let mut visit = |hwnd: WindowHandle| {
            if fallback == 0 {
                fallback = hwnd.0;
            }
            if let Some(rect) = platform::get_window_rect(hwnd) {
                let w = (rect.right - rect.left) as i64;
                let h = (rect.bottom - rect.top) as i64;
                if w > 0 && h > 0 {
                    let area = w * h;
                    if area > best_area {
                        best_area = area;
                        best = hwnd.0;
                    }
                }
            }
        };

        platform::enum_windows_for_pid(pid, &mut visit);

        if best != 0 {
            WindowHandle(best)
        } else {
            WindowHandle(fallback)
        }
    }

    /// Start the detection loop. Polls foreground window every 150ms.
    pub fn start(self: &Arc<Self>, cfg: Arc<config::Manager>) {
        let detector = self.clone();
        thread::Builder::new()
            .name("game-detect".into())
            .spawn(move || loop {
                detector.check_window(&cfg);
                thread::sleep(Duration::from_millis(150));
            })
            .expect("spawn game-detect thread");
    }

    /// Check the foreground window against configured game path.
    pub fn check_window(&self, cfg: &config::Manager) {
        // Refresh the foreground-PID cache once per detector tick. The
        // hot path (suppression gate, macro send) only reads the cache,
        // so polling cost is bounded here. Without this refresh the cache
        // would be frozen at 0 forever and the gate would deny every
        // macro press.
        platform::refresh_foreground_cache();

        let active_game = cfg.active_game();
        let game_path = cfg.game_path(&active_game).unwrap_or_default();

        // No game selected
        if active_game.is_empty() || game_path.is_empty() {
            let old = self.window_active.swap(false, Ordering::AcqRel);
            self.game_foreground.store(false, Ordering::Release);
            self.game_pid.store(0, Ordering::Release);
            if old {
                self.notify(false);
            }
            self.notify_foreground(false);
            return;
        }

        let fg = platform::get_foreground_window();
        let (_tid, fg_pid) = platform::get_window_thread_process_id(fg);
        let own_pid = platform::current_process_id();

        // Our own window counts as "active" for macro firing but is NOT the game
        // being foreground, so input hooks should not consume global hotkeys.
        if fg_pid == own_pid {
            let old = self.window_active.swap(true, Ordering::AcqRel);
            self.game_foreground.store(false, Ordering::Release);
            if !old {
                self.notify(true);
            }
            self.notify_foreground(false);
            return;
        }

        // Check foreground process path
        let proc_path = platform::query_process_path(fg_pid);
        let matched = proc_path
            .as_ref()
            .map(|p| paths_match(&game_path, p))
            .unwrap_or(false);

        if matched {
            self.game_pid.store(fg_pid, Ordering::Release);
            self.game_alive.store(true, Ordering::Release);
            let old = self.window_active.swap(true, Ordering::AcqRel);
            let old_fg = self.game_foreground.swap(true, Ordering::AcqRel);
            if !old {
                self.notify(true);
            }
            if !old_fg {
                self.notify_foreground(true);
            }
        } else {
            // Check if game process is still alive (even if not foreground)
            let game_pid = self.game_pid();
            let alive = if game_pid != 0 {
                platform::is_process_alive(game_pid)
            } else {
                false
            };
            self.game_alive.store(alive, Ordering::Release);
            if !alive {
                self.game_pid.store(0, Ordering::Release);
            }

            // Invalidate window handle cache when focus is lost
            self.invalidate_hwnd_cache();

            let old = self.window_active.swap(false, Ordering::AcqRel);
            let old_fg = self.game_foreground.swap(false, Ordering::AcqRel);
            if old {
                self.notify(false);
            }
            if old_fg {
                self.notify_foreground(false);
            }
        }
    }

    fn notify(&self, active: bool) {
        let prev = {
            let mut p = self.prev_active.lock();
            let old = *p;
            *p = active;
            old
        };
        if prev == active {
            return;
        }
        let callbacks = self.active_callbacks.lock();
        for cb in callbacks.iter() {
            cb(active);
        }
    }

    fn notify_foreground(&self, focused: bool) {
        let alive = self.game_alive.load(Ordering::Acquire);
        let pid = self.game_pid.load(Ordering::Acquire);
        // Only dispatch on an actual state change — previously the no-game
        // and own-window paths called this every 150ms tick, spamming the
        // hub callback (and its log line) 6-7 times a second forever.
        {
            let mut prev = self.prev_foreground.lock();
            let cur = (focused, alive, pid);
            if *prev == Some(cur) {
                return;
            }
            *prev = Some(cur);
        }
        let callbacks = self.foreground_callbacks.lock();
        for cb in callbacks.iter() {
            cb(focused, alive, pid);
        }
    }
}

/// Compare two file paths case-insensitively. On Wine/Proton the X11
/// window PID points to the wine process, whose cmdline has a Windows-style
/// path (Z:\...\Client.exe) that differs from the configured Linux path
/// (/home/.../Client.exe). So we compare both the full path AND just the
/// filename (last path component), which is the most reliable signal.
fn paths_match(configured: &str, actual: &str) -> bool {
    let normalize = |s: &str| {
        s.replace('\\', "/")
            .to_lowercase()
            .trim_end_matches('\0')
            .to_string()
    };
    let c = normalize(configured);
    let a = normalize(actual);
    if a == c || a.ends_with(&c) || c.ends_with(&a) {
        return true;
    }
    // Fallback: compare just the filename (last path component).
    let c_file = c.rsplit('/').next().unwrap_or(&c);
    let a_file = a.rsplit('/').next().unwrap_or(&a);
    !c_file.is_empty() && c_file == a_file
}
