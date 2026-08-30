//! Linux/Wayland platform layer — evdev input, uinput injection, grim capture.
//!
//! Requires:
//!   - membership in the `input` group (or root) to read /dev/input/event*
//!   - membership in the `uinput` group (or root) to write to /dev/uinput
//!   - `grim` installed for screen capture (wlroots-based compositors)
//!
//! The implementation intentionally does not depend on a specific Wayland
//! compositor protocol for focus detection; it uses /proc-based process
//! matching and lets the user enable `allowBackground` if focus detection is
//! not perfect.

#![allow(dead_code)]

use super::{HookCallback, HookHandle, WindowHandle, MOUSE_KEY_NAMES};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::fs::read_dir;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

// ── Key name mapping (engine names → Linux evdev key codes) ──────────────

static KEY_MAP: Lazy<HashMap<&'static str, u16>> = Lazy::new(|| {
    [
        ("1", 2u16),
        ("2", 3),
        ("3", 4),
        ("4", 5),
        ("5", 6),
        ("6", 7),
        ("7", 8),
        ("8", 9),
        ("9", 10),
        ("0", 11),
        ("a", 30),
        ("b", 48),
        ("c", 46),
        ("d", 32),
        ("e", 18),
        ("f", 33),
        ("g", 34),
        ("h", 35),
        ("i", 23),
        ("j", 36),
        ("k", 37),
        ("l", 38),
        ("m", 50),
        ("n", 49),
        ("o", 24),
        ("p", 25),
        ("q", 16),
        ("r", 19),
        ("s", 31),
        ("t", 20),
        ("u", 22),
        ("v", 47),
        ("w", 17),
        ("x", 45),
        ("y", 21),
        ("z", 44),
        ("minus", 12),       // -
        ("equal", 13),       // =
        ("lbracket", 26),    // [
        ("rbracket", 27),    // ]
        ("backslash", 43),   // \
        ("semicolon", 39),   // ;
        ("apostrophe", 40),  // '
        ("grave", 41),       // `
        ("comma", 51),       // ,
        ("period", 52),      // .
        ("slash", 53),       // /
        ("f1", 59),
        ("f2", 60),
        ("f3", 61),
        ("f4", 62),
        ("f5", 63),
        ("f6", 64),
        ("f7", 65),
        ("f8", 66),
        ("f9", 67),
        ("f10", 68),
        ("f11", 87),
        ("f12", 88),
        ("tab", 15),
        ("enter", 28),
        ("escape", 1),
        ("backspace", 14),
        ("space", 57),
        ("delete", 111),
        ("insert", 110),
        ("home", 102),
        ("end", 107),
        ("pgup", 104),
        ("pgdn", 109),
        ("up", 103),
        ("down", 108),
        ("left", 105),
        ("right", 106),
        ("capslock", 58),
        ("scrolllock", 70),
        ("numlock", 69),
        ("printscreen", 99),
        ("pause", 119),
        ("shift", 42),       // left shift
        ("ctrl", 29),        // left ctrl
        ("alt", 56),         // left alt
        ("win", 125),        // left meta
        ("lbutton", 272),    // BTN_LEFT
        ("rbutton", 273),    // BTN_RIGHT
        ("mbutton", 274),    // BTN_MIDDLE
        ("xbutton1", 275),   // BTN_SIDE
        ("xbutton2", 276),   // BTN_EXTRA
        ("numpad0", 82),
        ("numpad1", 79),
        ("numpad2", 80),
        ("numpad3", 81),
        ("numpad4", 75),
        ("numpad5", 76),
        ("numpad6", 77),
        ("numpad7", 71),
        ("numpad8", 72),
        ("numpad9", 73),
        ("numpadenter", 96),
        ("numpadadd", 78),
        ("numpadsub", 74),
        ("numpadmult", 55),
        ("numpaddiv", 98),
        ("numpaddot", 83),   // numpad .
    ]
    .into_iter()
    .collect()
});

static CODE_TO_NAME: Lazy<HashMap<u16, String>> =
    Lazy::new(|| KEY_MAP.iter().map(|(&k, &v)| (v, k.to_string())).collect());

static MOUSE_KEYS: Lazy<HashMap<&'static str, bool>> = Lazy::new(|| {
    MOUSE_KEY_NAMES
        .into_iter()
        .map(|k| (k, true))
        .collect()
});

pub fn name_to_vk(name: &str) -> u16 {
    KEY_MAP
        .get(&name.to_lowercase().as_str())
        .copied()
        .unwrap_or(0)
}

pub fn vk_to_name(vk: u16) -> String {
    CODE_TO_NAME.get(&vk).cloned().unwrap_or_default()
}

pub fn is_mouse_key(name: &str) -> bool {
    MOUSE_KEYS.contains_key(&name.to_lowercase().as_str())
}

pub fn get_all_key_names() -> Vec<String> {
    let mut names: Vec<String> = KEY_MAP.keys().map(|s| s.to_string()).collect();
    names.sort();
    names
}

// ── Virtual input device (uinput) ────────────────────────────────────────

fn open_uinput_device() -> Option<evdev::uinput::VirtualDevice> {
    use evdev::{AttributeSet, KeyCode, RelativeAxisCode};

    // Register EVERY possible key code (1..=255) with uinput, not just the
    // ones in KEY_MAP. When we grab a keyboard and re-emit keys via uinput,
    // any key the kernel sends (backslash, media keys, etc.) must be accepted
    // by the virtual device or it silently drops them — making keys like
    // backslash appear "blocked" to the user.
    let mut keys = AttributeSet::new();
    for code in 1u16..=255 {
        keys.insert(KeyCode::new(code));
    }
    // Mouse buttons (BTN_* codes live in the 0x110-0x11f range).
    keys.insert(KeyCode::BTN_LEFT);
    keys.insert(KeyCode::BTN_RIGHT);
    keys.insert(KeyCode::BTN_MIDDLE);
    keys.insert(KeyCode::BTN_SIDE);
    keys.insert(KeyCode::BTN_EXTRA);

    let mut rel = AttributeSet::new();
    rel.insert(RelativeAxisCode::REL_X);
    rel.insert(RelativeAxisCode::REL_Y);
    rel.insert(RelativeAxisCode::REL_WHEEL);
    rel.insert(RelativeAxisCode::REL_HWHEEL);

    let builder = evdev::uinput::VirtualDevice::builder().ok()?;
    let builder = builder.name("Macrotool Virtual Input");
    let builder = builder.with_keys(&keys).ok()?;
    let builder = builder.with_relative_axes(&rel).ok()?;
    builder.build().ok()
}

fn emit_key(dev: &mut evdev::uinput::VirtualDevice, code: u16, value: i32) {
    let ev = evdev::InputEvent::new(evdev::EventType::KEY.0, code, value);
    let syn = evdev::InputEvent::new(evdev::EventType::SYNCHRONIZATION.0, 0, 0);
    if let Err(e) = dev.emit(&[ev, syn]) {
        log::warn!("[linux] uinput emit key {} val={} failed: {}", code, value, e);
    }
}

fn emit_rel(dev: &mut evdev::uinput::VirtualDevice, axis: u16, value: i32) {
    let ev = evdev::InputEvent::new(evdev::EventType::RELATIVE.0, axis, value);
    let syn = evdev::InputEvent::new(evdev::EventType::SYNCHRONIZATION.0, 0, 0);
    let _ = dev.emit(&[ev, syn]);
}

fn send_linux_key(code: u16, up: bool) {
    // Track what the virtual device holds down so stuck keys can always be
    // released later (device death, macro abort, pause, shutdown).
    {
        let mut injected = INJECTED_DOWN.lock();
        if up {
            injected.remove(&code);
        } else {
            injected.insert(code);
        }
    }
    let mut guard = UINPUT.lock();
    if let Some(ref mut d) = *guard {
        let value = if up { 0 } else { 1 };
        emit_key(d, code, value);
    }
}

fn send_linux_mouse_button(code: u16, up: bool) {
    send_linux_key(code, up); // buttons are key events
}

fn send_linux_mouse_move(dx: i32, dy: i32) {
    let mut guard = UINPUT.lock();
    if let Some(ref mut d) = *guard {
        if dx != 0 {
            emit_rel(d, evdev::RelativeAxisCode::REL_X.0, dx);
        }
        if dy != 0 {
            emit_rel(d, evdev::RelativeAxisCode::REL_Y.0, dy);
        }
    }
}

// ── Global state ─────────────────────────────────────────────────────────

static UINPUT: Lazy<Mutex<Option<evdev::uinput::VirtualDevice>>> =
    Lazy::new(|| Mutex::new(open_uinput_device()));

static KB_HOOK_CB: Lazy<Mutex<Option<HookCallback>>> = Lazy::new(|| Mutex::new(None));
static MS_HOOK_CB: Lazy<Mutex<Option<HookCallback>>> = Lazy::new(|| Mutex::new(None));

static HOOK_STOP: AtomicBool = AtomicBool::new(false);

/// True while any registered macro hotkey is a keyboard (non-mouse) key.
/// When false, keyboard devices are read passively for state tracking only
/// and NEVER grabbed — physical typing cannot get stuck inside macrotool's
/// re-emit pipeline because macrotool is not in the path at all.
static KEY_GRAB_NEEDED: AtomicBool = AtomicBool::new(false);

/// Codes the virtual device currently holds down (injected, not yet released).
static INJECTED_DOWN: Lazy<Mutex<HashSet<u16>>> =
    Lazy::new(|| Mutex::new(HashSet::new()));

/// evdev device paths that have a live reader thread.
static ACTIVE_DEVICES: Lazy<Mutex<HashSet<PathBuf>>> =
    Lazy::new(|| Mutex::new(HashSet::new()));

/// Path of the device that owns the keyboard hotkey callback.
static PRIMARY_KB_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

/// How long an injected tap is held down. A down/up pair microseconds apart
/// is missed or mis-latched by games that poll key state once per frame.
const TAP_HOLD: Duration = Duration::from_millis(10);

/// Reconfigure whether keyboards must be grabbed (a profile with keyboard
/// hotkeys needs grabbing; a mouse-only profile must not touch keyboards).
pub fn set_keyboard_grab_needed(needed: bool) {
    let changed = KEY_GRAB_NEEDED.swap(needed, Ordering::AcqRel) != needed;
    if changed {
        log::info!("[linux] keyboard grab needed = {}", needed);
    }
}

/// Release every key the virtual device still holds down. Idempotent.
pub fn release_all_injected() {
    let codes: Vec<u16> = INJECTED_DOWN.lock().drain().collect();
    if codes.is_empty() {
        return;
    }
    log::warn!("[linux] releasing {} stuck injected key(s)", codes.len());
    let mut guard = UINPUT.lock();
    if let Some(ref mut d) = *guard {
        for code in codes {
            emit_key(d, code, 0);
        }
    }
}

/// Release a single key by engine name (no-op for unknown names).
pub fn release_key(key: &str) {
    let code = name_to_vk(key);
    if code != 0 {
        send_linux_key(code, true);
    }
}

static CURSOR_X: AtomicI32 = AtomicI32::new(0);
static CURSOR_Y: AtomicI32 = AtomicI32::new(0);

static SCREEN_W: AtomicI32 = AtomicI32::new(1920);
static SCREEN_H: AtomicI32 = AtomicI32::new(1080);

static KEY_STATE: Lazy<Mutex<HashMap<u16, bool>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static LOCK_STATE: Lazy<Mutex<HashMap<u16, bool>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn is_keyboard_device(dev: &evdev::Device) -> bool {
    let ev = dev.supported_events();
    if !ev.contains(evdev::EventType::KEY) {
        return false;
    }
    // Exclude pure mice/joysticks by requiring at least one letter key.
    let keys = dev.supported_keys();
    keys.map(|k| k.contains(evdev::KeyCode::KEY_A)).unwrap_or(false)
}

fn is_mouse_device(dev: &evdev::Device) -> bool {
    let ev = dev.supported_events();
    ev.contains(evdev::EventType::RELATIVE) || ev.contains(evdev::EventType::ABSOLUTE)
}

fn is_virtual_device(dev: &evdev::Device, dev_file: &str) -> bool {
    // Name-based check (cheap, catches most uinput tools).
    let name = dev.name().unwrap_or_default().to_string();
    if name.contains("Macrotool")
        || name.to_lowercase().contains("virtual")
        || name.to_lowercase().contains("uinput")
    {
        return true;
    }
    // Kernel-accurate check: uinput-created devices live under
    // /sys/devices/virtual/input/. Reading them is pointless (they only
    // carry events another tool injected, which the compositor already
    // receives) and grabbing one would wedge real input.
    if let Ok(link) = std::fs::read_link(format!("/sys/class/input/{}", dev_file)) {
        return link.to_string_lossy().contains("/devices/virtual/");
    }
    false
}

fn open_input_devices() -> Vec<(evdev::Device, PathBuf)> {
    let mut out = Vec::new();
    let dir = Path::new("/dev/input");
    if let Ok(entries) = read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let s = name.to_string_lossy();
            if s.starts_with("event") {
                if let Ok(dev) = evdev::Device::open(entry.path()) {
                    // Skip virtual (uinput-created) devices — our own, plus
                    // ydotoold and friends. Reading them back would create a
                    // feedback loop (we inject → we read → we re-inject), and
                    // a virtual device must never become the primary
                    // keyboard: it never emits events, so keyboard hotkeys
                    // routed to it would silently never trigger.
                    if is_virtual_device(&dev, &s) {
                        log::debug!(
                            "[linux] skipping virtual device: {} ({})",
                            s,
                            dev.name().unwrap_or_default()
                        );
                        continue;
                    }
                    out.push((dev, entry.path()));
                }
            }
        }
    }
    out
}

fn device_reader_loop(mut dev: evdev::Device, path: PathBuf, is_primary: bool) {
    let dev_name = dev.name().unwrap_or_default().to_string();
    let is_keyboard = is_keyboard_device(&dev);
    log::info!(
        "[linux] reader start {} ({}) primary={} keyboard={}",
        path.display(),
        dev_name,
        is_primary,
        is_keyboard
    );
    ACTIVE_DEVICES.lock().insert(path.clone());
    if is_primary {
        *PRIMARY_KB_PATH.lock() = Some(path.clone());
    }

    // is_primary=true: this device owns the hotkey callback. The device may
    // be a hybrid (keyboard with mouse buttons) — key events route to
    // Keyboard, mouse-button events route to Mouse.
    // is_primary=false: shadow keyboard or plain mouse.
    //   - Shadow keyboards: re-emit every key (no callback) while
    //     KEY_GRAB_NEEDED is set; otherwise passive state tracking only.
    //   - Mice: never grabbed; mouse-button events still fire the mouse
    //     callback (that is how rbutton / xbutton2 macros trigger).
    let mut consecutive_errors = 0u32;
    let mut grabbed = false;
    // Keys this device currently holds down (per-device, so a dying device
    // only synthesizes release events for keys IT reported).
    let mut local_held: HashSet<u16> = HashSet::new();

    loop {
        if HOOK_STOP.load(Ordering::Acquire) {
            break;
        }

        // Grab state follows KEY_GRAB_NEEDED live, so a mouse-only profile
        // never intercepts physical typing and a profile edit that adds a
        // keyboard hotkey starts grabbing without a restart.
        let want_grab = is_keyboard && KEY_GRAB_NEEDED.load(Ordering::Acquire);
        if want_grab != grabbed {
            let r = if want_grab { dev.grab() } else { dev.ungrab() };
            match r {
                Ok(()) => {
                    grabbed = want_grab;
                    log::info!(
                        "[linux] {} {} ({})",
                        if want_grab { "grabbed" } else { "ungrabbed" },
                        path.display(),
                        dev_name
                    );
                }
                Err(e) => {
                    log::warn!(
                        "[linux] grab toggle failed on {}: {} — will retry",
                        path.display(),
                        e
                    );
                    thread::sleep(Duration::from_millis(50));
                }
            }
        }

        match dev.fetch_events() {
            Ok(it) => {
                consecutive_errors = 0;
                for ev in it {
                    if let evdev::EventSummary::Key(_, key, value) = ev.destructure() {
                        let code = key.code() as u16;
                        if value != 0 {
                            local_held.insert(code);
                        } else {
                            local_held.remove(&code);
                        }
                    }
                    handle_linux_event(ev, is_primary, grabbed);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // Idle poll on a non-blocking fd — nothing to do.
            }
            Err(e) => {
                // ENODEV = device unplugged / re-enumerated. Bail out now;
                // the exit cleanup synthesizes the missing key-ups.
                if e.raw_os_error() == Some(libc::ENODEV) {
                    log::warn!(
                        "[linux] device {} detached (ENODEV)",
                        path.display()
                    );
                    break;
                }
                consecutive_errors += 1;
                if consecutive_errors == 1 {
                    log::warn!("[linux] read error on {}: {}", path.display(), e);
                }
                if consecutive_errors >= 50 {
                    log::error!(
                        "[linux] reader for {} failed {}x — exiting thread",
                        path.display(),
                        consecutive_errors
                    );
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        }

        // Device vanished from /dev/input (unplug / dongle re-enumeration).
        if !path.exists() {
            log::warn!("[linux] device {} disappeared", path.display());
            break;
        }
    }

    // ── Thread exit cleanup ────────────────────────────────────────────
    // The device is gone or wedged. While grabbed, its key-ups can never
    // arrive through us, so synthesize them: fire the hook callbacks with
    // is_down=false (updates the hotkey state machine + physical_down so
    // hold macros stop cleanly) and release everything the virtual device
    // still holds. This is the anti-stuck-key safety net.
    if grabbed {
        let downs: Vec<(u16, String)> = local_held
            .iter()
            .filter_map(|code| CODE_TO_NAME.get(code).map(|n| (*code, n.clone())))
            .collect();
        for (_, name) in &downs {
            let cb = if is_mouse_key(name) {
                MS_HOOK_CB.lock().clone()
            } else {
                KB_HOOK_CB.lock().clone()
            };
            if let Some(cb) = cb {
                let _ = cb(name, false);
            }
        }
        if !downs.is_empty() {
            log::warn!(
                "[linux] synthesized key-up for {} key(s) after {} died",
                downs.len(),
                path.display()
            );
        }
        release_all_injected();
        KEY_STATE.lock().clear();
        let _ = dev.ungrab();
    }
    ACTIVE_DEVICES.lock().remove(&path);
    if is_primary {
        *PRIMARY_KB_PATH.lock() = None;
    }
}

#[derive(Debug, PartialEq, Eq)]
enum EventRoute {
    Keyboard,
    Mouse,
    Passthrough,
}

fn grabbed_event_route(is_primary: bool, is_mouse: bool) -> EventRoute {
    if is_primary {
        if is_mouse {
            EventRoute::Mouse
        } else {
            EventRoute::Keyboard
        }
    } else {
        EventRoute::Passthrough
    }
}

fn handle_linux_event(ev: evdev::InputEvent, is_primary: bool, grabbed: bool) {
    match ev.destructure() {
        evdev::EventSummary::Key(_, key, value) => {
            let code = key.code() as u16;
            let down = value != 0;
            KEY_STATE.lock().insert(code, down);
            let is_lock = matches!(code, 58 | 69 | 70); // caps, num, scroll
            if is_lock && down {
                let mut ls = LOCK_STATE.lock();
                let cur = ls.get(&code).copied().unwrap_or(false);
                ls.insert(code, !cur);
            }

            // Ungrabbed: events flow to the compositor directly, so we can
            // never suppress — but we still observe them. Mouse buttons fire
            // the mouse callback (that's how rbutton/xbutton2 hotkeys work;
            // mice are never grabbed). Keyboard keys on an ungrabbed keyboard
            // are pure state tracking (mouse-only profile: macrotool must not
            // be in the typing path at all).
            if !grabbed {
                let key_name = vk_to_name(code);
                if !key_name.is_empty() && is_mouse_key(&key_name) {
                    if let Some(cb) = MS_HOOK_CB.lock().clone() {
                        let _ = cb(&key_name, down);
                    }
                }
                return;
            }

            let key_name = vk_to_name(code);

            if key_name.is_empty() {
                send_linux_key(code, !down);
                return;
            }

            let is_mouse = is_mouse_key(&key_name);
            // Shadow keyboards re-emit events without invoking callbacks to
            // avoid duplicate hotkey transitions. The primary hybrid keyboard
            // may also carry mouse buttons, which must use the mouse callback
            // rather than being silently treated as ordinary keyboard input.
            if grabbed_event_route(is_primary, is_mouse) == EventRoute::Passthrough {
                send_linux_key(code, !down);
                return;
            }

            let cb = match grabbed_event_route(is_primary, is_mouse) {
                EventRoute::Mouse => MS_HOOK_CB.lock().clone(),
                EventRoute::Keyboard => KB_HOOK_CB.lock().clone(),
                EventRoute::Passthrough => None,
            };

            let suppress = match cb {
                Some(cb) => cb(&key_name, down),
                None => false,
            };

            if suppress {
                log::debug!("[linux] SUPPRESSED key={} code={} down={}", key_name, code, down);
            }

            // Re-emit policy for grabbed devices:
            //   - Keyboard-key presses are owned by the macro when
            //     suppressed (correct — no double-fire).
            //   - Keyboard-key releases: do NOT re-emit. The press was
            //     suppressed, so the OS has no matching down; re-emitting
            //     an unmatched release confuses the kernel/Wayland stack.
            //   - Mouse-button presses: suppressed when the macro owns
            //     them (correct).
            //   - Mouse-button releases: ALWAYS re-emit, even when
            //     suppressed. Without this, holding a mouse-button hotkey
            //     (Rbutton, xbutton1/2) and releasing it leaves the OS
            //     thinking the button is still down forever — the desktop
            //     and any app polling the physical button state see a
            //     stuck button. The macro's uinput-injected key sequence
            //     is independent of this path, so the release doesn't
            //     leak the hotkey into the game.
            let must_emit_release = !down && is_mouse;
            if !suppress || must_emit_release {
                send_linux_key(code, !down);
            }
        }
        evdev::EventSummary::RelativeAxis(_, axis, value) => {
            if axis == evdev::RelativeAxisCode::REL_X || axis == evdev::RelativeAxisCode::REL_Y {
                let dx = if axis == evdev::RelativeAxisCode::REL_X {
                    value
                } else {
                    0
                };
                let dy = if axis == evdev::RelativeAxisCode::REL_Y {
                    value
                } else {
                    0
                };
                if dx != 0 || dy != 0 {
                    let mut x = CURSOR_X.load(Ordering::Relaxed) + dx;
                    let mut y = CURSOR_Y.load(Ordering::Relaxed) + dy;
                    let sw = SCREEN_W.load(Ordering::Relaxed);
                    let sh = SCREEN_H.load(Ordering::Relaxed);
                    x = x.clamp(0, sw.max(1) - 1);
                    y = y.clamp(0, sh.max(1) - 1);
                    CURSOR_X.store(x, Ordering::Relaxed);
                    CURSOR_Y.store(y, Ordering::Relaxed);
                }
            }
            // Never re-emit mouse movement.
        }
        evdev::EventSummary::AbsoluteAxis(_, _, _value) => {
            // Touchpad — track only, never re-emit.
        }
        _ => {}
    }
}

// ── Public API ───────────────────────────────────────────────────────────

pub fn set_console_visibility(_visible: bool) {
    // Linux: no-op; this app is a Tauri GUI process.
}

pub fn current_process_id() -> u32 {
    std::process::id()
}

pub fn get_foreground_window() -> WindowHandle {
    // Hot path: atomic load of the cached foreground PID. The cache is
    // refreshed by `refresh_foreground_cache()` below, which the game
    // detector calls every 150ms. Reading the cache is ~nanoseconds, so
    // the per-key-event cost in `should_suppress_hotkey` is negligible.
    // Without this, the previous implementation spawned a `niri msg`
    // subprocess on every keypress, which pegged CPU and tanked game
    // framerate (100+ fork+exec per second on a held movement key).
    let cached = CACHED_FOCUSED_PID.load(Ordering::Acquire);
    WindowHandle(cached as u64)
}

/// Refresh the cached foreground PID by polling Niri (preferred) and X11
/// (fallback). Intended to be called from a low-frequency poll thread — the
/// game detector calls it every 150ms. A slow Niri IPC round-trip here does
/// NOT block the input hot path.
pub fn refresh_foreground_cache() {
    // Niri path first — it's the dominant case on the user's UwU host
    // and the only one that works on pure-Wayland sessions.
    if let Some(pid) = niri_foreground_pid() {
        if pid != CACHED_FOCUSED_PID.load(Ordering::Acquire) {
            log::debug!("[linux] focus -> {} (niri)", pid);
        }
        CACHED_FOCUSED_PID.store(pid, Ordering::Release);
        return;
    }
    // X11 fallback for XWayland sessions (GDK_BACKEND=x11).
    if let Some(pid) = x11_foreground_pid() {
        if pid != CACHED_FOCUSED_PID.load(Ordering::Acquire) {
            log::debug!("[linux] focus -> {} (x11)", pid);
        }
        CACHED_FOCUSED_PID.store(pid, Ordering::Release);
        return;
    }
    // Neither source reachable. Set the cache to 0 so the suppression
    // gate's "unknown focus → deny" branch fires. We don't keep the
    // previous value — a stale PID was the original Niri bug.
    let prev = CACHED_FOCUSED_PID.swap(0, Ordering::AcqRel);
    if prev != 0 {
        log::debug!("[linux] focus -> 0 (unknown)");
    }
}

/// Cached foreground window PID (0 = unknown / no source). Updated by
/// `refresh_foreground_cache()` and read by `get_foreground_window() at
/// every key event. Atomic load makes the hot path lock-free.
static CACHED_FOCUSED_PID: AtomicU32 = AtomicU32::new(0);

/// Resolve the focused window PID via Niri's IPC socket.
///
/// Niri stores its socket at `${NIRI_SOCKET:-/run/user/<uid>/niri.wayland-1.<pid>.sock}`.
/// We shell out to `niri msg -j focused-window` and parse the JSON
/// `{"pid": <u32>, ...}` field. Returns `None` if Niri isn't running, the
/// socket isn't reachable, the command fails, or no window is focused.
///
/// Important: returns `None` (not a stale PID) on any failure so the caller
/// can fail-closed rather than fire macros into the wrong window.
///
/// EXPENSIVE: forks a process. Call from a poll thread (the game detector
/// polls every 150ms), NOT from the key-event hot path.
fn niri_foreground_pid() -> Option<u32> {
    let socket = niri_socket_path()?;
    if !socket.exists() {
        return None;
    }

    // Synchronous run — called from the game-detector poll thread every
    // 150ms, NOT from the key-event hot path. If Niri hangs (e.g.
    // compositing stall, busy GPU, IPC socket wedged), the spawn-wait
    // MUST time out so the detector thread keeps ticking. Otherwise the
    // cached foreground PID goes stale forever and macros stop firing.
    //
    // 100 ms cap is well under the 150 ms poll interval so each tick still
    // completes even when Niri hangs, and the thread keeps moving.
    let mut child = Command::new("niri")
        .args(["msg", "-j", "focused-window"])
        .env("NIRI_SOCKET", &socket)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .ok()?;
    let start = std::time::Instant::now();
    let timeout = Duration::from_millis(100);

    // Drain stdout/stderr in the background while waiting, so a chatty
    // Niri can't block on a full pipe. We don't need the data — we only
    // care whether `niri msg` exited cleanly and in time.
    let mut stdout_handle = child.stdout.take();
    let mut stderr_handle = child.stderr.take();
    let drain_thread = std::thread::spawn(move || {
        use std::io::Read;
        fn drain<R: Read>(mut s: R) -> Vec<u8> {
            let mut out = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                match s.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => out.extend_from_slice(&buf[..n]),
                    Err(_) => break,
                }
            }
            out
        }
        let stdout = stdout_handle.take().map(drain);
        let stderr = stderr_handle.take().map(drain);
        (stdout.unwrap_or_default(), stderr.unwrap_or_default())
    });

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // The drain thread has been reading stdout/stderr in the
                // background; wait for it so we get the full output
                // before checking status / parsing JSON.
                let (stdout_bytes, stderr_bytes) =
                    drain_thread.join().unwrap_or_default();
                let _ = child.wait();
                if !status.success() {
                    log::trace!(
                        "[linux] niri msg -j focused-window failed: status={:?} stderr={}",
                        status.code(),
                        String::from_utf8_lossy(&stderr_bytes).trim()
                    );
                    return None;
                }
                return parse_niri_focused_window(&stdout_bytes);
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    log::warn!(
                        "[linux] niri msg timed out after {:?} — killing the stuck \
                         subprocess and continuing with the cached focus. Focus \
                         tracking may be stale until Niri recovers.",
                        timeout
                    );
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = drain_thread.join();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = drain_thread.join();
                return None;
            }
        }
    }
}

/// Parse the JSON returned by `niri msg -j focused-window`. Extracted so
/// the timeout-wrapped call site above stays readable.
fn parse_niri_focused_window(stdout: &[u8]) -> Option<u32> {
    let json = serde_json::from_slice::<serde_json::Value>(stdout).ok()?;
    json.get("pid")
        .and_then(|p| p.as_u64())
        .filter(|&p| p != 0)
        .map(|p| p as u32)
}

/// Locate the Niri IPC socket. Honour `$NIRI_SOCKET` first (set by the
/// compositor when launching child processes), then fall back to scanning
/// `/run/user/<uid>/` for a `niri.wayland-1.*.sock` file.
///
/// Skips stale sockets whose target is missing (broken symlinks left over
/// from a previous Niri instance whose PID no longer exists). Without this
/// filter, a stale socket name with a higher readdir() order would shadow
/// the new, working socket and silently break focus tracking forever.
fn niri_socket_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NIRI_SOCKET") {
        if !p.is_empty() {
            let path = PathBuf::from(p);
            // Probe the env-pinned socket first; if it's a broken symlink
            // or otherwise unreachable, fall through to the directory scan
            // so we don't permanently lock onto a dead path.
            if path.exists() {
                return Some(path);
            }
        }
    }
    // Best-effort scan of /run/user/$UID/. We don't fail loudly if the
    // directory isn't readable (different UID, sandbox); the caller will
    // simply not detect Niri and move on to X11.
    let uid = unsafe { libc::getuid() };
    let dir = PathBuf::from(format!("/run/user/{}", uid));
    let entries = std::fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let s = name.to_string_lossy();
        if s.starts_with("niri.wayland-1.") && s.ends_with(".sock") {
            // exists() follows broken symlinks correctly — if Niri was
            // restarted under a new PID and left the old symlink dangling,
            // .exists() returns false here and we skip past it.
            if entry.path().exists() {
                return Some(entry.path());
            }
        }
    }
    None
}

/// Query X11 _NET_ACTIVE_WINDOW → _NET_WM_PID to find the PID of the
/// currently focused window. Returns None if X11 is unavailable or the
/// focused window has no PID (rare — means a WM-internal window).
fn x11_foreground_pid() -> Option<u32> {
    use x11::xlib;
    use std::ffi::CString;
    use std::os::raw::{c_int, c_ulong, c_uchar};

    // Install our silent error handler so BadWindow errors from stale window
    // IDs don't kill the process.
    install_x11_error_handler();

    unsafe {
        let display = xlib::XOpenDisplay(std::ptr::null());
        if display.is_null() {
            log::debug!("[linux] XOpenDisplay failed — no X11");
            return None;
        }
        let _guard = DisplayGuard(display);

        let root = xlib::XDefaultRootWindow(display);
        let atom_active = CString::new("_NET_ACTIVE_WINDOW").ok()?;
        let atom_active = xlib::XInternAtom(display, atom_active.as_ptr(), 0);
        let atom_pid = CString::new("_NET_WM_PID").ok()?;
        let atom_pid = xlib::XInternAtom(display, atom_pid.as_ptr(), 0);

        // Try _NET_ACTIVE_WINDOW first (EWMH-compliant WMs).
        let mut active_win: c_ulong = 0;
        let mut got_active = false;

        if atom_active != 0 {
            let mut actual_type: c_ulong = 0;
            let mut actual_format: c_int = 0;
            let mut nitems: c_ulong = 0;
            let mut bytes_after: c_ulong = 0;
            let mut data: *mut c_uchar = std::ptr::null_mut();
            let r = xlib::XGetWindowProperty(
                display, root, atom_active, 0, 1, 0, xlib::AnyPropertyType as c_ulong,
                &mut actual_type, &mut actual_format, &mut nitems, &mut bytes_after, &mut data,
            );
            if r == 0 && !data.is_null() && nitems >= 1 {
                active_win = *(data as *const c_ulong);
                xlib::XFree(data as *mut std::ffi::c_void);
                got_active = active_win != 0;
            } else if !data.is_null() {
                xlib::XFree(data as *mut std::ffi::c_void);
            }
        }

        // Fallback: XGetInputFocus — universally supported, returns the window
        // that currently has keyboard focus. Works even when the WM doesn't
        // implement EWMH (_NET_ACTIVE_WINDOW).
        if !got_active {
            let mut focus_win: c_ulong = 0;
            let mut revert: c_int = 0;
            if xlib::XGetInputFocus(display, &mut focus_win, &mut revert) != 0 && focus_win != 0 {
                active_win = focus_win;
                got_active = true;
            }
        }

        if !got_active {
            log::debug!("[linux] no active window from X11");
            return None;
        }

        // Read _NET_WM_PID from the active window.
        // Validate the window ID first — XWayland can return bogus IDs (e.g. 0x1)
        // that cause BadWindow errors when queried.
        if atom_pid != 0 && got_active && active_win != 0 {
            let mut actual_type: c_ulong = 0;
            let mut actual_format: c_int = 0;
            let mut nitems: c_ulong = 0;
            let mut bytes_after: c_ulong = 0;
            let mut data2: *mut c_uchar = std::ptr::null_mut();
            let r2 = xlib::XGetWindowProperty(
                display, active_win, atom_pid, 0, 1, 0, xlib::AnyPropertyType as c_ulong,
                &mut actual_type, &mut actual_format, &mut nitems, &mut bytes_after, &mut data2,
            );
            // Flush the X11 error queue — with our silent handler, errors
            // are swallowed but we still need to sync so the return value
            // reflects reality.
            xlib::XSync(display, 0);
            if r2 == 0 && !data2.is_null() && nitems >= 1 {
                let pid = *(data2 as *const c_ulong) as u32;
                xlib::XFree(data2 as *mut std::ffi::c_void);
                if pid != 0 {
                    log::debug!("[linux] X11 active win=0x{:x} pid={}", active_win, pid);
                    return Some(pid);
                }
            }
            if !data2.is_null() {
                xlib::XFree(data2 as *mut std::ffi::c_void);
            }
        }

        // Last resort: if _NET_WM_PID isn't set, we can't get the PID.
        log::debug!("[linux] X11 active win=0x{:x} but no _NET_WM_PID", active_win);
        None
    }
}

/// RAII guard to ensure XCloseDisplay is called.
struct DisplayGuard(*mut x11::xlib::Display);
impl Drop for DisplayGuard {
    fn drop(&mut self) {
        unsafe { x11::xlib::XCloseDisplay(self.0); }
    }
}

/// Custom X11 error handler that swallows errors instead of calling the
/// default handler (which prints to stderr and can abort the process on
/// BadWindow / BadAtom errors from stale window IDs).
extern "C" fn x11_silent_error_handler(
    _display: *mut x11::xlib::Display,
    _error: *mut x11::xlib::XErrorEvent,
) -> std::os::raw::c_int {
    // Return 0 to indicate "handled" — the default handler prints the error
    // and calls exit(1) on some error types, killing the whole app.
    0
}

/// Install the silent X11 error handler once. Must be called before any
/// XGetWindowProperty / XQueryTree calls.
static X11_HANDLER_INSTALLED: AtomicBool = AtomicBool::new(false);
fn install_x11_error_handler() {
    if !X11_HANDLER_INSTALLED.swap(true, Ordering::SeqCst) {
        unsafe {
            x11::xlib::XSetErrorHandler(Some(x11_silent_error_handler));
        }
    }
}

pub fn get_window_thread_process_id(hwnd: WindowHandle) -> (u32, u32) {
    (0, hwnd.0 as u32)
}

pub fn is_window_valid(_hwnd: WindowHandle) -> bool {
    true
}

pub fn is_window_visible(_hwnd: WindowHandle) -> bool {
    true
}

pub fn get_window_rect(_hwnd: WindowHandle) -> Option<RECT> {
    let w = SCREEN_W.load(Ordering::Acquire);
    let h = SCREEN_H.load(Ordering::Acquire);
    Some(RECT {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    })
}

pub fn get_screen_resolution() -> (i32, i32) {
    // Try to update from a quick grim capture.
    if let Some((w, h, _)) = capture_screen_bgra() {
        SCREEN_W.store(w, Ordering::Release);
        SCREEN_H.store(h, Ordering::Release);
    }
    (SCREEN_W.load(Ordering::Acquire), SCREEN_H.load(Ordering::Acquire))
}

pub fn enum_windows_for_pid<F>(pid: u32, f: &mut F)
where
    F: FnMut(WindowHandle),
{
    f(WindowHandle(pid as u64));
}

fn process_path_from_cmdline(data: &[u8]) -> Option<String> {
    let args: Vec<&[u8]> = data.split(|&b| b == 0).filter(|s| !s.is_empty()).collect();

    // Wine/Proton commonly puts its loader in argv[0] and the Windows game
    // executable in a later argument. Prefer that executable when present.
    args.iter()
        .map(|arg| String::from_utf8_lossy(arg))
        .find(|arg| arg.to_lowercase().ends_with(".exe"))
        .map(|arg| arg.into_owned())
        .or_else(|| {
            args.first()
                .map(|arg| String::from_utf8_lossy(arg).into_owned())
        })
}

pub fn query_process_path(pid: u32) -> Option<String> {
    // For native Linux apps, /proc/{pid}/exe is the real binary.
    // For Wine/Proton games, /proc/{pid}/exe points to the wine64 binary
    // (a Linux executable), NOT the Windows .exe. In that case the cmdline
    // contains the Windows path (e.g. "Z:\Games\SEBNS\bin64\Client.exe").
    // Strategy: read exe first. If it's NOT a .exe (i.e. it's a Linux binary
    // like wine64/preloader), fall back to cmdline which has the Windows exe.
    let exe_path = PathBuf::from(format!("/proc/{}/exe", pid));
    if let Ok(p) = std::fs::read_link(&exe_path) {
        let path_str = p.to_string_lossy().to_string();
        let lower = path_str.to_lowercase();
        if lower.ends_with(".exe") {
            // Native Windows exe (unlikely on Linux, but handle it).
            return Some(path_str);
        }
        // It's a Linux binary (wine64, wineserver, proton, etc.).
        // Don't return it — fall through to cmdline.
    }

    // Read /proc/{pid}/cmdline — null-separated args.
    let cmd = PathBuf::from(format!("/proc/{}/cmdline", pid));
    if let Ok(data) = std::fs::read(&cmd) {
        // cmdline is null-separated. The first arg is usually the executable
        // path. For Wine it's often "Z:\path\to\Client.exe".
        if let Some(path) = process_path_from_cmdline(&data) {
            return Some(path);
        }
    }

    // Special case for xwayland-satellite under Niri: Wine/Proton X11 apps
    // are fronted by an xwayland-satellite instance whose own cmdline has
    // no useful info, but whose child tree contains the actual game loader
    // (wine-preloader / proton / wine64) — and that child has the Windows
    // exe path in its cmdline. Walk the child tree to find it. This is the
    // bridge that lets the detector match under Niri + Heroic/Wine/Proton.
    if let Some(p) = query_child_wine_path(pid) {
        return Some(p);
    }

    // Absolute last resort: return the exe path even if it's not .exe.
    if let Ok(p) = std::fs::read_link(&exe_path) {
        return Some(p.to_string_lossy().to_string());
    }
    None
}

/// If `pid` is (or hosts) a Wine/Proton game process, return the Windows
/// exe path found in that process's cmdline. Used to identify the real game
/// under Niri's xwayland-satellite wrapper.
///
/// Walks one level of children looking for processes whose cmdline contains
/// a Windows-style path ending in `.exe`. Recursion is intentionally one
/// level deep — Wine launchers are direct children of xwayland-satellite,
/// and deeper walks add latency on every 150ms detector tick.
fn query_child_wine_path(pid: u32) -> Option<String> {
    let children = read_child_pids(pid)?;
    for child in children {
        let cmd_path = PathBuf::from(format!("/proc/{}/cmdline", child));
        if let Ok(data) = std::fs::read(&cmd_path) {
            if let Some(path) = process_path_from_cmdline(&data) {
                if path.to_lowercase().ends_with(".exe") {
                    return Some(path);
                }
            }
        }
        // Also try the child's exe in case the cmdline doesn't have the
        // Windows path (some Proton variants).
        let child_exe = PathBuf::from(format!("/proc/{}/exe", child));
        if let Ok(p) = std::fs::read_link(&child_exe) {
            let s = p.to_string_lossy().to_string();
            if s.to_lowercase().ends_with(".exe") {
                return Some(s);
            }
        }
    }
    None
}

/// Read /proc/<pid>/task/<tid>/children to enumerate direct children.
/// Returns PIDs (as u32).
fn read_child_pids(pid: u32) -> Option<Vec<u32>> {
    // The first task's children file lists all direct children.
    let path = PathBuf::from(format!("/proc/{}/task/{}/children", pid, pid));
    let data = std::fs::read_to_string(&path).ok()?;
    let mut out = Vec::new();
    for tok in data.split_whitespace() {
        if let Ok(p) = tok.parse::<u32>() {
            out.push(p);
        }
    }
    Some(out)
}

pub fn is_process_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{}", pid)).exists()
}

// ── Input injection ──────────────────────────────────────────────────────

pub fn send_key(key: &str) {
    if is_mouse_key(key) {
        send_mouse(key, false);
        thread::sleep(TAP_HOLD);
        send_mouse(key, true);
        return;
    }
    let code = name_to_vk(key);
    if code == 0 {
        log::warn!("[linux] send_key unknown {}", key);
        return;
    }
    send_linux_key(code, false);
    // Hold the tap long enough for games that poll key state once per
    // frame — a near-zero hold is missed or mis-latched by some engines.
    thread::sleep(TAP_HOLD);
    send_linux_key(code, true);
}

pub fn send_key_to_window(_hwnd: WindowHandle, key: &str) {
    // Wayland has no direct per-window injection; inject globally.
    send_key(key);
}

fn send_mouse(key: &str, up: bool) {
    let code = name_to_vk(key);
    if code == 0 {
        return;
    }
    send_linux_mouse_button(code, up);
}

pub fn get_async_key_state(vk: i32) -> bool {
    KEY_STATE.lock().get(&(vk as u16)).copied().unwrap_or(false)
}

pub fn get_key_state_toggled(vk: i32) -> u16 {
    let down = LOCK_STATE.lock().get(&(vk as u16)).copied().unwrap_or(false);
    if down { 1 } else { 0 }
}

// ── Pixel reading / screen capture ───────────────────────────────────────

pub fn get_cursor_pos() -> (i32, i32) {
    (CURSOR_X.load(Ordering::Acquire), CURSOR_Y.load(Ordering::Acquire))
}

pub fn get_pixel_color(x: i32, y: i32) -> u32 {
    let region = get_pixels_region(x, y, 1, 1);
    region.first().copied().unwrap_or(0)
}

pub fn get_pixels_region(x: i32, y: i32, w: i32, h: i32) -> Vec<u32> {
    let count = (w.max(0) * h.max(0)) as usize;
    let mut out = vec![0u32; count];
    if w <= 0 || h <= 0 {
        return out;
    }
    let geom = format!("{},{} {}x{}", x, y, w, h);
    match grim_capture(Some(&geom)) {
        Some((cw, ch, rgb)) if cw == w && ch == h => {
            for (i, px) in out.iter_mut().enumerate() {
                *px = rgb[i];
            }
        }
        _ => {}
    }
    out
}

pub fn capture_screen_bgra() -> Option<(i32, i32, Vec<u8>)> {
    let (w, h, rgb) = grim_capture(None)?;
    let mut bgra = vec![0u8; (w * h * 4) as usize];
    for i in 0..(w * h) as usize {
        bgra[i * 4] = (rgb[i] & 0xFF) as u8;
        bgra[i * 4 + 1] = ((rgb[i] >> 8) & 0xFF) as u8;
        bgra[i * 4 + 2] = ((rgb[i] >> 16) & 0xFF) as u8;
        bgra[i * 4 + 3] = 0xFF;
    }
    Some((w, h, bgra))
}

pub fn check_pixels_batched(
    pixels: &[(i32, i32, u32, i32)],
    match_mode: &str,
) -> bool {
    if pixels.is_empty() {
        return false;
    }
    let min_x = pixels.iter().map(|p| p.0).min().unwrap_or(0);
    let min_y = pixels.iter().map(|p| p.1).min().unwrap_or(0);
    let max_x = pixels.iter().map(|p| p.0).max().unwrap_or(0);
    let max_y = pixels.iter().map(|p| p.1).max().unwrap_or(0);
    let w = max_x - min_x + 1;
    let h = max_y - min_y + 1;
    let region = get_pixels_region(min_x, min_y, w, h);
    let mut matched = 0;
    for &(x, y, target, variation) in pixels {
        let dx = x - min_x;
        let dy = y - min_y;
        let idx = (dy * w + dx) as usize;
        if idx >= region.len() {
            continue;
        }
        let color = region[idx];
        let r = (color >> 16) & 0xFF;
        let g = (color >> 8) & 0xFF;
        let b = color & 0xFF;
        let tr = (target >> 16) & 0xFF;
        let tg = (target >> 8) & 0xFF;
        let tb = target & 0xFF;
        let ok = (r as i32 - tr as i32).unsigned_abs() <= variation as u32
            && (g as i32 - tg as i32).unsigned_abs() <= variation as u32
            && (b as i32 - tb as i32).unsigned_abs() <= variation as u32;
        if ok {
            matched += 1;
            if match_mode == "any" {
                return true;
            }
        }
    }
    if match_mode == "all" {
        matched == pixels.len()
    } else {
        matched > 0
    }
}

fn grim_capture(geometry: Option<&str>) -> Option<(i32, i32, Vec<u32>)> {
    let mut cmd = Command::new("grim");
    cmd.arg("-t").arg("png").arg("-");
    if let Some(g) = geometry {
        cmd.arg("-g").arg(g);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = cmd.output().ok()?;
    if !output.status.success() {
        log::warn!(
            "[linux] grim failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }
    decode_png(&output.stdout)
}

fn decode_png(data: &[u8]) -> Option<(i32, i32, Vec<u32>)> {
    use image::ImageReader;
    let cursor = std::io::Cursor::new(data);
    let img = ImageReader::new(cursor)
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width() as i32, rgba.height() as i32);
    let rgb: Vec<u32> = rgba
        .pixels()
        .map(|p| {
            let [r, g, b, _] = p.0;
            ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
        })
        .collect();
    Some((w, h, rgb))
}

// ── Timing ───────────────────────────────────────────────────────────────

pub fn precise_sleep(ms: f64) {
    let start = Instant::now();
    let target = Duration::from_secs_f64(ms / 1000.0);
    // Hybrid sleep: spin only for the last 2ms (accuracy), sleep before that
    // (CPU friendly). The old pure spin loop burned a full core per hold
    // macro and stole CPU from the evdev reader threads — a starved reader
    // re-emits key-down but misses key-up, which is how keys get stuck.
    let spin_threshold = Duration::from_millis(2);
    loop {
        let elapsed = start.elapsed();
        if elapsed >= target {
            break;
        }
        let remaining = target - elapsed;
        if remaining > spin_threshold {
            thread::sleep(remaining - spin_threshold);
        } else {
            std::hint::spin_loop();
        }
    }
}

pub fn set_thread_priority_above_normal() {
    // best-effort nice decrease
    let _ = unsafe { libc::nice(-1) };
}

pub fn set_macro_thread_affinity() {
    // no-op on Linux
}

// ── Global hooks ─────────────────────────────────────────────────────────

pub fn set_keyboard_hook(callback: HookCallback) -> Result<HookHandle, String> {
    *KB_HOOK_CB.lock() = Some(callback);
    Ok(HookHandle(1))
}

pub fn set_mouse_hook(callback: HookCallback) -> Result<HookHandle, String> {
    *MS_HOOK_CB.lock() = Some(callback);
    Ok(HookHandle(2))
}

pub fn unhook(_hook: HookHandle) {
    // Individual hook teardown is driven by run_hook_message_loop's stop
    // channel; nothing to do per-hook here.
}

/// Spawn reader threads for every currently-connected keyboard/mouse that
/// isn't already being read. First keyboard found becomes the primary (owns
/// the hotkey callback); later keyboards are shadows (re-emit only).
/// Returns the handles and whether a primary keyboard was seen.
fn spawn_device_readers() -> (Vec<std::thread::JoinHandle<()>>, bool) {
    let mut handles = Vec::new();
    let mut primary_keyboard_seen = false;
    let already_active = ACTIVE_DEVICES.lock().clone();

    for (dev, path) in open_input_devices() {
        let is_kb = is_keyboard_device(&dev);
        let is_mouse = is_mouse_device(&dev);
        if !is_kb && !is_mouse {
            continue;
        }
        if already_active.contains(&path) {
            continue; // reader thread already running for this node
        }
        let is_primary = is_kb && !primary_keyboard_seen;
        if is_primary {
            primary_keyboard_seen = true;
        }
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        match thread::Builder::new()
            .name(format!("evdev-{}", name))
            .spawn(move || device_reader_loop(dev, path, is_primary))
        {
            Ok(h) => handles.push(h),
            Err(e) => log::warn!("[linux] spawn reader for {} failed: {}", name, e),
        }
    }
    (handles, primary_keyboard_seen)
}

pub fn run_hook_message_loop(_stop_rx: crossbeam_channel::Receiver<()>) -> Result<(), String> {
    HOOK_STOP.store(false, Ordering::Release);

    // Ensure the uinput device exists.
    if UINPUT.lock().is_none() {
        *UINPUT.lock() = open_uinput_device();
    }

    // Make sure virtual device is fully registered before we start grabbing.
    std::thread::sleep(Duration::from_millis(100));

    // A single physical keyboard often exposes MULTIPLE evdev device nodes
    // (e.g. "SCYROX Xpunk 63 Keyboard" on event9/10/11/13 + the 8K dongle on
    // event14-22). If we grab only ONE, the keypress leaks to the compositor
    // through the ungrabbed devices. If we grab ALL and let each call the
    // hotkey callback, the first fires the transition (suppress=true) but the
    // second sees Transition::None (suppress=false) and re-emits — breaking
    // suppression and causing hold macros to stop instantly.
    //
    // Strategy: read ALL keyboard + mouse nodes. The FIRST keyboard is the
    // "primary" — it owns the hotkey callback. Other keyboards are "shadow"
    // devices. Keyboards are only actually GRABBED while KEY_GRAB_NEEDED is
    // set (some registered hotkey is a keyboard key); with a mouse-only
    // profile macrotool never touches physical typing at all. Mice are never
    // grabbed.
    let (mut handles, primary_seen) = spawn_device_readers();
    if !primary_seen {
        log::warn!("[linux] no keyboard device found — keyboard hotkeys disabled");
    }

    // Watch for device hotplug: 8K mice/keyboards re-enumerate their nodes
    // (event14-22 → event23+), leaving the old reader threads reading a dead
    // fd. Rescan every 2s and spawn readers for new nodes.
    let watcher_handles = thread::Builder::new()
        .name("evdev-rescan".into())
        .spawn(|| {
            loop {
                thread::sleep(Duration::from_secs(2));
                if HOOK_STOP.load(Ordering::Acquire) {
                    break;
                }
                let (new_handles, _) = spawn_device_readers();
                for h in new_handles {
                    // Detached: HOOK_STOP is the shutdown signal for all
                    // reader threads including these.
                    std::mem::forget(h);
                }
            }
        });

    // Wait until stopped.
    let _ = _stop_rx.recv();
    HOOK_STOP.store(true, Ordering::Release);
    if let Ok(h) = watcher_handles {
        let _ = h.join();
    }

    // Detach reader threads instead of joining them: they sit in blocking
    // fetch_events() and would only notice HOOK_STOP after their next event,
    // making app shutdown hang until the user touches a key. EVIOCGRAB is
    // released by the kernel when the process exits, so detaching is safe.
    std::mem::forget(handles);

    // Final safety net: never leave injected keys down at shutdown.
    release_all_injected();

    Ok(())
}

// ── Window rectangle type (matches the original Win32 RECT layout) ─────────────────

#[derive(Debug, Default, Clone, Copy)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[cfg(test)]
mod tests {
    use super::{grabbed_event_route, process_path_from_cmdline, EventRoute};

    #[test]
    fn hybrid_primary_mouse_button_uses_mouse_route() {
        assert_eq!(grabbed_event_route(true, true), EventRoute::Mouse);
    }

    #[test]
    fn wine_cmdline_prefers_the_game_executable_over_the_loader() {
        let cmdline = b"/nix/store/wine64-preloader\0Z:\\Games\\SEBNS\\Client.exe\0--flag\0";
        assert_eq!(
            process_path_from_cmdline(cmdline).as_deref(),
            Some("Z:\\Games\\SEBNS\\Client.exe")
        );
    }
}
