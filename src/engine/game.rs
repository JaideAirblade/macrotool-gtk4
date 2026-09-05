//! Game Detector — answers two questions about the configured game:
//! is it the focused window, and is it running at all.
//!
//! Both answers are derived fresh from `/proc` on every tick by comparing
//! executable basenames (see `platform::focused_window_is_game` and
//! `platform::find_live_game_exe`). Nothing about the game process is
//! cached — no process id, no window handle. The old design cached both,
//! and any drift between that cache and reality
//! (Wine re-exec, a game that forks its renderer after the splash screen,
//! xwayland-satellite reassigning the surface) permanently wedged the gate
//! — the cached PID matched nothing, so no macro fired again until
//! macrotool restarted.

use crate::config;
use crate::platform;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub struct GameDetector {
    /// The configured game is the live focused window.
    game_in_focus: AtomicBool,
    /// A process running the configured game executable exists, whether or
    /// not it currently holds focus.
    game_present: AtomicBool,
    prev_active: Mutex<bool>,
    prev_foreground: Mutex<Option<(bool, bool)>>,
    active_callbacks: Mutex<Vec<Box<dyn Fn(bool) + Send + Sync>>>,
    foreground_callbacks: Mutex<Vec<Box<dyn Fn(bool, bool) + Send + Sync>>>,
}

impl GameDetector {
    pub fn new() -> Self {
        GameDetector {
            game_in_focus: AtomicBool::new(false),
            game_present: AtomicBool::new(false),
            prev_active: Mutex::new(false),
            prev_foreground: Mutex::new(None),
            active_callbacks: Mutex::new(Vec::new()),
            foreground_callbacks: Mutex::new(Vec::new()),
        }
    }

    pub fn register_active_callback(&self, cb: Box<dyn Fn(bool) + Send + Sync>) {
        self.active_callbacks.lock().push(cb);
    }

    /// Register a `(focused, present)` observer. Fired only on an actual
    /// state change, never on every tick.
    pub fn register_foreground_callback(&self, cb: Box<dyn Fn(bool, bool) + Send + Sync>) {
        self.foreground_callbacks.lock().push(cb);
    }

    /// Is the configured game the focused window right now?
    ///
    /// This is the only gate every injection path consults. It is a plain
    /// atomic load of the value the last detector tick computed.
    pub fn is_in_focus(&self) -> bool {
        self.game_in_focus.load(Ordering::Acquire)
    }

    /// Is a process running the configured game executable present?
    ///
    /// Informational only — used by the overlay to distinguish "game not
    /// running" from "game running but not focused". It never authorises an
    /// injection on its own.
    pub fn is_present(&self) -> bool {
        self.game_present.load(Ordering::Acquire)
    }

    /// Start the detection loop. Spawns up to two threads:
    /// 1. **Event thread** *(Niri only)* — subscribes to
    ///    `niri msg --json event-stream` and reacts to focus/window
    ///    changes the instant Niri publishes them. Only spawned when a
    ///    Niri socket is reachable at startup; on every other compositor
    ///    this thread is skipped and detection falls back to (2). That
    ///    way macrotool keeps working on Sway, Hyprland, KWin, GNOME,
    ///    the TDM greeter, tty, and SSH sessions.
    /// 2. **Poll thread** — once-a-second fallback that re-runs the full
    ///    detector (which includes the /proc comm+cmdline floor and the
    ///    compositor-agnostic `focused_pid_universal` lookup) to
    ///    recover from any events the event-stream missed (Niri hiccup,
    ///    restart, race at startup).
    pub fn start(self: &Arc<Self>, cfg: Arc<config::Manager>) {
        let detector1 = self.clone();
        let detector2 = self.clone();
        let cfg_for_poll = cfg.clone();
        let cfg_for_event = cfg.clone();

        // Niri event-stream thread: only when a Niri socket is actually
        // reachable. The previous implementation spawned this thread
        // unconditionally and ran it in a tight reconnect loop on
        // non-Niri compositors, which worked but was pure noise (and
        // forked `niri msg` 6x/minute in a busy-loop forever on every
        // host that did not happen to be running Niri).
        if Self::niri_socket_available() {
            thread::Builder::new()
                .name("game-detect-event".into())
                .spawn(move || {
                    let socket_env = std::env::var("NIRI_SOCKET").ok();
                    let cfg = cfg_for_event;
                    loop {
                        if detector1.run_event_stream_iteration(
                            socket_env.as_deref(),
                            cfg.clone(),
                        ) {
                            std::thread::sleep(Duration::from_millis(50));
                        } else {
                            std::thread::sleep(Duration::from_secs(2));
                        }
                    }
                })
                .expect("spawn game-detect-event thread");
        } else if crate::platform::umbriel_socket_available() {
            // Umbriel event-stream thread: same instant-tick job as the
            // Niri one, for hosts on the Umbriel compositor. `umbriel
            // subscribe windows` emits one JSON line per window-list
            // change (focus, open, close, move); each line busts the
            // Umbriel focus-throttle cache and drives a full detector
            // tick so focus transitions land in ~ms instead of waiting
            // for the 1Hz poll.
            let detector_u = self.clone();
            let cfg_for_umbriel = cfg.clone();
            thread::Builder::new()
                .name("game-detect-umbriel".into())
                .spawn(move || loop {
                    if detector_u.run_umbriel_event_iteration(cfg_for_umbriel.clone()) {
                        std::thread::sleep(Duration::from_millis(50));
                    } else {
                        std::thread::sleep(Duration::from_secs(2));
                    }
                })
                .expect("spawn game-detect-umbriel thread");
        } else {
            log::info!(
                "[detect] no Niri or Umbriel socket found at startup; relying on the \
                 1Hz poll thread + X11 route (compositor-agnostic path)."
            );
        }

        // Poll thread: once-a-second fallback. Runs the full detector
        // (cfg + /proc floor + compositor-agnostic lookup) so a missed
        // event doesn't strand us in a stale state. This thread is
        // ALWAYS spawned — it is what makes macrotool work on every
        // non-Niri compositor.
        thread::Builder::new()
            .name("game-detect-poll".into())
            .spawn(move || loop {
                detector2.check_window(&cfg_for_poll);
                std::thread::sleep(Duration::from_secs(1));
            })
            .expect("spawn game-detect-poll thread");
    }

    /// Cheap probe used by `start` to decide whether to spawn the Niri
    /// event-stream subscriber thread. Returns true when `$NIRI_SOCKET`
    /// is set and points at a live socket, OR when a
    /// `niri.wayland-1.*.sock` file exists under `/run/user/<uid>/`.
    fn niri_socket_available() -> bool {
        if let Ok(p) = std::env::var("NIRI_SOCKET") {
            if !p.is_empty() && std::path::Path::new(&p).exists() {
                return true;
            }
        }
        let uid = unsafe { libc::getuid() };
        let dir = std::path::PathBuf::from(format!("/run/user/{}", uid));
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return false;
        };
        entries
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("niri.wayland-1."))
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
            // or windows — workspace-only changes don't affect us. When
            // the event carries a WindowsChanged / WindowFocusChanged
            // payload we extract the focused window's pid INLINE (no
            // fork) so `focused_window_is_game` has a fresh pid without
            // round-tripping through `niri msg -j focused-window`.
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if t.contains("Window focus")
                || t.contains("Windows changed")
                || t.contains("Window opened")
                || t.contains("Window closed")
            {
                // Parse the JSON event and find the focused window's
                // pid. `WindowsChanged` is the dominant case (emitted on
                // every focus change, window create, and window close).
                // Its payload shape is
                //   {"WindowsChanged":{"windows":[
                //     {"id":N,"pid":<u32>,"is_focused":bool,...},
                //     ...
                //   ]}}
                // `WindowFocusChanged` is just `{"WindowFocusChanged":
                // {"id":N}}` — we resolve the pid from the same
                // WindowsChanged snapshot that Niri emits on focus
                // changes anyway, so we treat it identically.
                if let Some(pid) = parse_niri_event_focused_pid(&line) {
                    crate::platform::set_focused_pid_from_event(Some(pid));
                }
                // Drive a full detector tick on the spot. The detector
                // re-reads cfg, re-derives focus and presence from /proc,
                // and notifies the overlay — all on this thread, all
                // within a couple of ms.
                self.check_window(&cfg);
            }
        }
    }

    /// Subscribe to `umbriel subscribe windows` and drive an instant
    /// detector tick on every window-list change (focus, open, close,
    /// move). Returns true on a healthy iteration; false on fatal
    /// error so the caller backs off and retries. Mirrors
    /// `run_event_stream_iteration` (the Niri equivalent).
    fn run_umbriel_event_iteration(&self, cfg: Arc<crate::config::Manager>) -> bool {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};

        let socket = match crate::platform::umbriel_socket_path_string() {
            Some(s) => s,
            None => return false,
        };

        let mut child = match Command::new("umbriel")
            .args(["subscribe", "windows"])
            .env("UMBRIEL_SOCKET", &socket)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                log::warn!("[detect] umbriel subscribe spawn failed: {}", e);
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
            match reader.read_line(&mut line) {
                Ok(0) => {
                    log::warn!("[detect] umbriel event-stream EOF — reconnecting in 2s");
                    let _ = child.wait();
                    return false;
                }
                Ok(_) => {}
                Err(e) => {
                    log::warn!("[detect] umbriel event-stream read error: {}", e);
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
            }
            let t = line.trim();
            if t.is_empty() || !t.contains("\"event\"") {
                continue;
            }
            // Any windows-list change can move focus: bust the throttle
            // cache so the next query re-runs `umbriel windows --json`
            // immediately, then tick the full detector on the spot.
            crate::platform::umbriel_invalidate_focus_cache();
            self.check_window(&cfg);
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

    /// Recompute focus/presence from `/proc` and fire edge-triggered
    /// callbacks.
    ///
    /// Both facts are resolved fresh here — this function stores only the
    /// two booleans it just derived, and never a PID or window handle.
    pub fn check_window(&self, cfg: &config::Manager) {
        // Refresh the foreground-PID cache once per detector tick. The hot
        // path (suppression gate, macro send) only reads booleans derived
        // here, so the polling cost is bounded to this call site. Without
        // the refresh the cache would sit at 0 forever and the gate would
        // deny every macro press.
        platform::refresh_foreground_cache();

        let focused = platform::focused_window_is_game(cfg);
        // A focused game is by definition present, so only pay for the
        // /proc walk when the game is NOT the focused window.
        let present = focused || platform::find_live_game_exe(cfg).is_some();

        let was_focused = self.game_in_focus.swap(focused, Ordering::AcqRel);
        self.game_present.store(present, Ordering::Release);

        // `notify` reports "the engine may act", which under the PID-free
        // contract is exactly "the game is focused" — there is no longer a
        // background mode that widens it.
        if was_focused != focused {
            self.notify(focused);
            // One line per transition, on stderr, via the normal logger.
            // The previous implementation appended to
            // /tmp/macrotool-detector.log every 5 seconds forever, which is
            // pure noise in steady state and only ever useful while the
            // focus bug was being chased.
            log::info!(
                "[game] focus transition: game_in_focus={} game_present={}",
                focused,
                present
            );
        }
        self.notify_foreground(focused, present);
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

    fn notify_foreground(&self, focused: bool, present: bool) {
        // Only dispatch on an actual state change — the no-game and
        // own-window paths used to call this on every tick, spamming the hub
        // callback (and its log line) several times a second forever.
        {
            let mut prev = self.prev_foreground.lock();
            let cur = (focused, present);
            if *prev == Some(cur) {
                return;
            }
            *prev = Some(cur);
        }
        let callbacks = self.foreground_callbacks.lock();
        for cb in callbacks.iter() {
            cb(focused, present);
        }
    }
}

/// Parse the focused window's pid out of a single Niri event-stream JSON
/// line. Returns the first window with `is_focused:true` whose `pid` is
/// non-zero. Returns `None` for unrelated events, parse failures, or
/// focus-state-only events that carry no pid.
///
/// Cheap: runs synchronously on the event-stream subscriber thread, never
/// forks, never blocks on /proc. The poll thread re-derives everything via
/// `/proc` for the heavier "is this process the game" question.
fn parse_niri_event_focused_pid(line: &str) -> Option<u32> {
    let v: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return None,
    };
    let obj = v.as_object()?;
    // WindowsChanged is the primary focus-tracking event. Niri emits it
    // on every window create / close / focus transition, and the payload
    // carries the full pid per window. Find the window whose
    // `is_focused:true` and read its pid.
    if let Some(wc) = obj.get("WindowsChanged").and_then(|x| x.as_object()) {
        if let Some(windows) = wc.get("windows").and_then(|x| x.as_array()) {
            for w in windows {
                let focused = w
                    .get("is_focused")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false);
                if !focused {
                    continue;
                }
                if let Some(pid) = w.get("pid").and_then(|x| x.as_u64()) {
                    if pid != 0 && pid <= u32::MAX as u64 {
                        return Some(pid as u32);
                    }
                }
            }
        }
        // WindowsChanged with no focused window: clear the cache.
        return None;
    }
    None
}


#[cfg(test)]
mod parse_niri_event_tests {
    use super::parse_niri_event_focused_pid;

    /// The real Niri `WindowsChanged` payload shape — every window has
    /// `id`, `pid`, `is_focused`, etc. The focused window's pid must be
    /// returned even when other windows in the same event are not
    /// focused.
    #[test]
    fn extracts_focused_pid_from_a_real_windows_changed_payload() {
        let line = r#"{"WindowsChanged":{"windows":[{"id":42,"title":"Macrotool","app_id":"com.jaide.macrotool","pid":2744065,"workspace_id":1,"is_focused":true,"is_floating":false,"is_urgent":false},{"id":77,"title":"hermes","app_id":"com.mitchellh.ghostty","pid":1836845,"workspace_id":1,"is_focused":false,"is_floating":false,"is_urgent":false}]}}"#;
        assert_eq!(parse_niri_event_focused_pid(line), Some(2744065));
    }

    /// When a WindowsChanged event arrives but no window is focused
    /// (focus moved to the desktop, or the compositor's own surface),
    /// the helper must return None so the platform cache clears.
    #[test]
    fn returns_none_when_no_window_is_focused() {
        let line = r#"{"WindowsChanged":{"windows":[{"id":42,"title":"Macrotool","app_id":"com.jaide.macrotool","pid":2744065,"workspace_id":1,"is_focused":false,"is_floating":false,"is_urgent":false}]}}"#;
        assert_eq!(parse_niri_event_focused_pid(line), None);
    }

    /// Unrelated events (WorkspacesChanged, ConfigLoaded, KeyboardLayouts
    /// Changed) must return None — they don't carry a focused pid.
    #[test]
    fn returns_none_for_unrelated_events() {
        let line = r#"{"WorkspacesChanged":{"workspaces":[]}}"#;
        assert_eq!(parse_niri_event_focused_pid(line), None);

        let line = r#"{"ConfigLoaded":{"failed":false}}"#;
        assert_eq!(parse_niri_event_focused_pid(line), None);
    }

    /// Malformed JSON must return None, not panic. The subscriber thread
    /// runs continuously and a corrupt line must not kill the process.
    #[test]
    fn returns_none_for_malformed_json() {
        assert_eq!(parse_niri_event_focused_pid("not json at all"), None);
        assert_eq!(parse_niri_event_focused_pid(""), None);
        assert_eq!(
            parse_niri_event_focused_pid(r#"{"WindowsChanged":"oops"}"#),
            None
        );
    }

    /// The focused window must have a non-zero pid (the `pid:0` case is
    /// niri's sentinel for "no process"; treat it like no-focus).
    #[test]
    fn focused_window_with_zero_pid_is_ignored() {
        let line = r#"{"WindowsChanged":{"windows":[{"id":42,"title":"Compositor","app_id":null,"pid":0,"workspace_id":null,"is_focused":true,"is_floating":false,"is_urgent":false}]}}"#;
        assert_eq!(parse_niri_event_focused_pid(line), None);
    }
}
