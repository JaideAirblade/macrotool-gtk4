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

pub mod linux;
pub use linux::*;

use std::sync::Arc;

/// Opaque handle to a native top-level window. On Linux we treat it as the
/// focused PID because Wayland has no global HWND equivalent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowHandle(pub u64);

impl WindowHandle {
    pub fn is_null(self) -> bool {
        self.0 == 0
    }
}

/// Opaque handle returned by `set_keyboard_hook` / `set_mouse_hook`.
#[derive(Clone, Copy, Debug)]
pub struct HookHandle(pub i32);

/// Callback signature for global keyboard/mouse hooks.
/// Arguments: `(key_name, is_down)`; return value: `true` if the event should
/// be suppressed (eaten), `false` to let it pass through to the OS.
pub type HookCallback = Arc<dyn Fn(&str, bool) -> bool + Send + Sync>;

// Common mouse-key names recognised by the engine.
#[allow(dead_code)]
pub(crate) const MOUSE_KEY_NAMES: [&str; 5] = ["lbutton", "rbutton", "mbutton", "xbutton1", "xbutton2"];

pub const INVALID_WINDOW_HANDLE: WindowHandle = WindowHandle(0);
