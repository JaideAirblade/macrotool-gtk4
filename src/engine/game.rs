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

    /// Start the detection loop. Spawns two threads:
    /// 1. **Event thread** — subscribes to `niri msg --json event-stream`
    ///    and reacts to focus/window changes the instant Niri publishes
    ///    them. No polling latency.
    /// 2. **Poll thread** — once-a-second fallback that re-runs the full
    ///    detector (which includes the Wine-cmdline scan) to recover
    ///    from any events the event-stream missed (Niri hiccup, restart,
    ///    race at startup).
    pub fn start(self: &Arc<Self>, cfg: Arc<config::Manager>) {
        let detector1 = self.clone();
        let detector2 = self.clone();
        let cfg_for_poll = cfg.clone();
        let cfg_for_event = cfg.clone();

        // Event-stream thread: instant focus tracking. Every time Niri
        // emits a focus/window/workspace event we refresh the foreground
        // cache, and (because the event carries the new pid directly) we
        // also run the matching check_window path inline so game_pid is
        // updated on the same instant the user clicked.
        thread::Builder::new()
            .name("game-detect-event".into())
            .spawn(move || {
                let socket_env = std::env::var("NIRI_SOCKET").ok();
                let cfg = cfg_for_event;
                loop {
                    if detector1.run_event_stream_iteration(socket_env.as_deref(), cfg.clone()) {
                        std::thread::sleep(Duration::from_millis(50));
                    } else {
                        std::thread::sleep(Duration::from_secs(2));
                    }
                }
            })
            .expect("spawn game-detect-event thread");

        // Poll thread: once-a-second fallback. Runs the full detector
        // (cfg + Wine-cmdline scan) so a missed event doesn't strand us
        // in a stale state.
        thread::Builder::new()
            .name("game-detect-poll".into())
            .spawn(move || loop {
                detector2.check_window(&cfg_for_poll);
                std::thread::sleep(Duration::from_secs(1));
            })
            .expect("spawn game-detect-poll thread");
    }

    /// Subscribe to `niri msg --json event-stream` and trigger an
    /// instant detector tick on every relevant event. Returns true on
    /// a healthy iteration; false on a fatal error so the caller can
    /// back off and retry.
    fn run_event_stream_iteration(
        &self,
        socket_env: Option<&str>,
        cfg: Arc<crate::config::Manager>,
    ) -> bool {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};

        let socket_path = match socket_env {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => match self.find_niri_socket() {
                Some(p) => p,
                None => return false,
            },
        };

        let mut child = match Command::new("niri")
            .args(["msg", "--json", "event-stream"])
            .env("NIRI_SOCKET", &socket_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                log::warn!("[detect] niri event-stream spawn failed: {}", e);
                return false;
            }
        };

        let stdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        };
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();

        loop {
            line.clear();
            let n = match reader.read_line(&mut line) {
                Ok(0) => {
                    // Niri closed the stream — it restarted or shut down.
                    log::warn!("[detect] niri event-stream EOF — reconnecting in 2s");
                    let _ = child.wait();
                    return false;
                }
                Ok(n) => n,
                Err(e) => {
                    log::warn!("[detect] niri event-stream read error: {}", e);
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
            };
            if n == 0 {
                continue;
            }
            // Cheap event filter. The full event-stream emits one JSON
            // object per line. We only react to lines that mention focus
            // or windows — workspace-only changes don't affect us.
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if t.contains("Window focus")
                || t.contains("Windows changed")
                || t.contains("Window opened")
                || t.contains("Window closed")
            {
                // Drive a full detector tick on the spot. The detector
                // reads cfg, runs the Wine-cmdline scan, updates
                // game_pid, and notifies the overlay — all on this
                // thread, all within a couple of ms.
                self.check_window(&cfg);
            }
        }
    }

    /// Locate the live Niri IPC socket. Honours `$NIRI_SOCKET` first;
    /// falls back to scanning `/run/user/$UID/` for a working socket
    /// (skipping stale symlinks whose target has been deleted).
    fn find_niri_socket(&self) -> Option<String> {
        if let Ok(p) = std::env::var("NIRI_SOCKET") {
            if !p.is_empty() && std::path::Path::new(&p).exists() {
                return Some(p);
            }
        }
        let uid = unsafe { libc::getuid() };
        let dir = std::path::PathBuf::from(format!("/run/user/{}", uid));
        let entries = std::fs::read_dir(&dir).ok()?;
        for entry in entries.flatten() {
            let s = entry.file_name();
            let n = s.to_string_lossy();
            if n.starts_with("niri.wayland-1.") && n.ends_with(".sock") {
                if entry.path().exists() {
                    return Some(entry.path().to_string_lossy().into_owned());
                }
            }
        }
        None
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
        let mut matched = proc_path
            .as_ref()
            .map(|p| paths_match(&game_path, p))
            .unwrap_or(false);

        // Niri + Heroic/Wine/Proton fallback: when the foreground PID is
        // xwayland-satellite, its own /proc cmdline has nothing useful
        // (X clients connect via socket, not fork). Walk every /proc entry
        // looking for a process whose cmdline ends in the configured
        // Windows .exe. If we find one, treat the foreground window as
        // the game — the user has the game focused, just via the
        // satellite wrapper.
        if !matched {
            let scan_result = scan_wine_process_for_game(&game_path);
            log::info!(
                "[game] scan_wine_process_for_game: game_path={:?} result={:?}",
                game_path,
                scan_result
            );
            if let Some(wine_path) = scan_result {
                log::info!(
                    "[game] foreground PID {} (xwayland-satellite?) matched via Wine child cmdline {}",
                    fg_pid,
                    wine_path
                );
                matched = true;
            }
        }

        // Periodic state log (every ~5s) so the user can see what the
        // detector is actually seeing. Cheap: a single timestamp check
        // per 150ms tick. Writes to /tmp/macrotool-detector.log so it's
        // visible even when macrotool's stderr is /dev/null (launched
        // from a graphical session).
        {
            use std::sync::atomic::{AtomicU64, Ordering};
            static LAST_LOG: AtomicU64 = AtomicU64::new(0);
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let last = LAST_LOG.load(Ordering::Acquire);
            if now_ms.saturating_sub(last) >= 5000 {
                LAST_LOG.store(now_ms, Ordering::Release);
                let line = format!(
                    "[game] ts={} fg_pid={} game_path={:?} proc_path={:?} matched={} own_pid={}\n",
                    now_ms,
                    fg_pid,
                    game_path,
                    proc_path,
                    matched,
                    own_pid
                );
                let _ = std::fs::write("/tmp/macrotool-detector.log",
                    std::fs::read_to_string("/tmp/macrotool-detector.log")
                        .unwrap_or_default() + &line);
            }
        }

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
/// path (Z:\...\\Client.exe) that differs from the configured Linux path
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

/// Scan every /proc entry looking for a Wine/Proton game process whose
/// cmdline ends in the configured Windows .exe. Used as a fallback when
/// the foreground window's own /proc lookup doesn't yield a useful path
/// (e.g. Niri's xwayland-satellite wrapper, where X clients connect via
/// Unix socket rather than forking from the satellite).
///
/// Returns the cmdline path of the matching process, or None if no Wine
/// process carrying the configured exe name is running.
///
/// Cost: one readdir on /proc per 150ms detector tick (~few hundred
/// entries, ~1ms total). Acceptable for a low-frequency poll thread.
fn scan_wine_process_for_game(configured: &str) -> Option<String> {
    let configured_basename = std::path::Path::new(configured)
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    // Path::file_name() on Unix treats backslashes as literal chars (not
    // separators), so for a Windows path like "..\bin64\Client.exe" we'd
    // get the whole string back as the "filename". Normalize by splitting
    // on both '/' and '\\' ourselves so a Windows exe path matches the
    // cmdline basename even on Linux.
    if configured_basename.is_empty() {
        return None;
    }
    let configured_basename = configured_basename
        .rsplit(|c| c == '/' || c == '\\')
        .next()
        .unwrap_or(&configured_basename)
        .to_string();

    let entries = std::fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let s = name.to_string_lossy();
        if !s.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let pid: u32 = match s.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        // Never the game if it's us.
        if pid == platform::current_process_id() {
            continue;
        }
        let cmd_path = std::path::PathBuf::from(format!("/proc/{}/cmdline", pid));
        let data = match std::fs::read(&cmd_path) {
            Ok(d) => d,
            Err(_) => continue,
        };
        // cmdline is null-separated args. Take the first arg.
        let first_arg = data
            .split(|&b| b == 0)
            .find(|a| !a.is_empty())
            .map(|a| String::from_utf8_lossy(a).to_string());
        let path = match first_arg {
            Some(p) => p,
            None => continue,
        };
        let lowercase = path.to_ascii_lowercase();
        let basename = lowercase
            .rsplit(|c: char| c == '/' || c == '\\')
            .next()
            .unwrap_or(&lowercase)
            .to_string();
        if basename == configured_basename {
            return Some(path);
        }
    }
    None
}
