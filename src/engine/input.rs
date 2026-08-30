//! Input Manager — global keyboard/mouse hooks with a clean state machine.
//!
//! All low-level input calls are delegated to the `platform` module. On
//! Linux/Wayland this means evdev device readers and uinput injection.

use crate::platform;
use crossbeam_channel::{unbounded, Receiver, Sender};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread;

// ── Hotkey state machine ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyState {
    Idle,
    Pressed,
    Holding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyMode {
    Press,
    Hold,
    Toggle,
}

pub fn parse_mode(s: &str) -> HotkeyMode {
    match s {
        "hold" => HotkeyMode::Hold,
        "toggle" => HotkeyMode::Toggle,
        _ => HotkeyMode::Press,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    None,
    Start,
    Stop,
    Toggle,
}

pub struct HotkeyInfo {
    state: AtomicI32,
    enabled: AtomicBool,
    background: AtomicBool,
    mode: HotkeyMode,
}

impl HotkeyInfo {
    pub fn new(mode: HotkeyMode) -> Self {
        HotkeyInfo {
            state: AtomicI32::new(0),
            enabled: AtomicBool::new(true),
            background: AtomicBool::new(false),
            mode,
        }
    }

    fn state(&self) -> HotkeyState {
        match self.state.load(Ordering::Acquire) {
            1 => HotkeyState::Pressed,
            2 => HotkeyState::Holding,
            _ => HotkeyState::Idle,
        }
    }

    pub fn set_state(&self, s: HotkeyState) {
        self.state.store(s as i32, Ordering::Release);
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state(), HotkeyState::Pressed | HotkeyState::Holding)
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    pub fn set_enabled(&self, en: bool) {
        self.enabled.store(en, Ordering::Release);
    }

    pub fn is_background(&self) -> bool {
        self.background.load(Ordering::Acquire)
    }

    pub fn set_background(&self, bg: bool) {
        self.background.store(bg, Ordering::Release);
    }

    pub fn on_key_down(&self) -> Transition {
        let current = self.state();
        match self.mode {
            HotkeyMode::Press => {
                if current == HotkeyState::Idle {
                    self.set_state(HotkeyState::Pressed);
                    Transition::Start
                } else {
                    Transition::None
                }
            }
            HotkeyMode::Hold => {
                if current == HotkeyState::Idle {
                    self.set_state(HotkeyState::Holding);
                    Transition::Start
                } else {
                    Transition::None
                }
            }
            HotkeyMode::Toggle => {
                if current == HotkeyState::Idle {
                    self.set_state(HotkeyState::Holding);
                    Transition::Toggle
                } else if current == HotkeyState::Holding {
                    self.set_state(HotkeyState::Idle);
                    Transition::Stop
                } else {
                    Transition::None
                }
            }
        }
    }

    pub fn on_key_up(&self) -> Transition {
        let current = self.state();
        match self.mode {
            HotkeyMode::Press => {
                if current == HotkeyState::Pressed {
                    self.set_state(HotkeyState::Idle);
                    Transition::Stop
                } else {
                    Transition::None
                }
            }
            HotkeyMode::Hold => {
                if current == HotkeyState::Holding {
                    self.set_state(HotkeyState::Idle);
                    Transition::Stop
                } else {
                    Transition::None
                }
            }
            HotkeyMode::Toggle => Transition::None,
        }
    }
}

// ── Shared hook state ────────────────────────────────────────────────────

pub type HandlerFn = Box<dyn Fn(&str, Transition) + Send + Sync>;

#[derive(Clone)]
pub struct HookSharedState {
    pub hotkeys: Arc<Mutex<HashMap<String, Arc<HotkeyInfo>>>>,
    pub handlers: Arc<Mutex<Vec<HandlerFn>>>,
    pub physical_down: Arc<Mutex<HashMap<String, bool>>>,
    pub engine_active: Arc<AtomicBool>,
    pub game_pid: Arc<AtomicU32>,
    pub game_alive: Arc<AtomicBool>,
    pub paused: Arc<AtomicBool>,
    pub sending: Arc<AtomicBool>,
    pub own_pid: u32,
}

impl HookSharedState {
    pub fn new() -> Self {
        HookSharedState {
            hotkeys: Arc::new(Mutex::new(HashMap::new())),
            handlers: Arc::new(Mutex::new(Vec::new())),
            physical_down: Arc::new(Mutex::new(HashMap::new())),
            engine_active: Arc::new(AtomicBool::new(false)),
            game_pid: Arc::new(AtomicU32::new(0)),
            game_alive: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
            sending: Arc::new(AtomicBool::new(false)),
            own_pid: platform::current_process_id(),
        }
    }

    fn dispatch(&self, key_name: &str, transition: Transition) {
        if transition == Transition::None {
            return;
        }
        let handlers = self.handlers.lock();
        for h in handlers.iter() {
            h(key_name, transition);
        }
    }
}

// ── Input Manager ───────────────────────────────────────────────────────

pub struct InputManager {
    state: HookSharedState,
    running: AtomicBool,
    stop_tx: Mutex<Option<Sender<()>>>,
}

impl InputManager {
    pub fn new() -> Self {
        InputManager {
            state: HookSharedState::new(),
            running: AtomicBool::new(false),
            stop_tx: Mutex::new(None),
        }
    }

    pub fn shared_state(&self) -> &HookSharedState {
        &self.state
    }

    pub fn set_game_pid(&self, pid: u32) {
        self.state.game_pid.store(pid, Ordering::Release);
    }

    pub fn set_game_alive(&self, alive: bool) {
        self.state.game_alive.store(alive, Ordering::Release);
    }

    pub fn set_paused(&self, paused: bool) {
        self.state.paused.store(paused, Ordering::Release);
    }

    pub fn is_paused(&self) -> bool {
        self.state.paused.load(Ordering::Acquire)
    }

    pub fn register_hotkey(&self, name: &str, mode: HotkeyMode) {
        self.state
            .hotkeys
            .lock()
            .insert(name.to_lowercase(), Arc::new(HotkeyInfo::new(mode)));
    }

    pub fn clear_hotkeys(&self) {
        self.state.hotkeys.lock().clear();
    }

    pub fn set_background(&self, name: &str, bg: bool) {
        let map = self.state.hotkeys.lock();
        if let Some(hk) = map.get(&name.to_lowercase()) {
            hk.set_background(bg);
        }
    }

    pub fn set_enabled(&self, name: &str, en: bool) {
        let map = self.state.hotkeys.lock();
        if let Some(hk) = map.get(&name.to_lowercase()) {
            hk.set_enabled(en);
        }
    }

    pub fn is_active(&self, name: &str) -> bool {
        self.state
            .hotkeys
            .lock()
            .get(&name.to_lowercase())
            .map(|hk| hk.is_active())
            .unwrap_or(false)
    }

    pub fn any_active(&self) -> bool {
        self.state.hotkeys.lock().values().any(|hk| hk.is_active())
    }

    pub fn on_event(&self, handler: HandlerFn) {
        self.state.handlers.lock().push(handler);
    }

    pub fn clear_handlers(&self) {
        self.state.handlers.lock().clear();
    }

    pub fn acquire_sending(&self) {
        SENDING_COUNTER.fetch_add(1, Ordering::AcqRel);
        self.state.sending.store(true, Ordering::Release);
    }

    pub fn release_sending(&self) {
        loop {
            let old = SENDING_COUNTER.load(Ordering::Acquire);
            if old <= 0 {
                return;
            }
            if SENDING_COUNTER
                .compare_exchange(old, old - 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                if old - 1 == 0 {
                    self.state.sending.store(false, Ordering::Release);
                }
                return;
            }
        }
    }

    pub fn is_sending(&self) -> bool {
        self.state.sending.load(Ordering::Acquire)
    }

    pub fn set_engine_active(&self, active: bool) {
        self.state.engine_active.store(active, Ordering::Release);
    }

    pub fn reset_all_states(&self) {
        for hk in self.state.hotkeys.lock().values() {
            hk.set_state(HotkeyState::Idle);
        }
    }

    pub fn is_hotkey_physically_down(&self, key_name: &str) -> bool {
        let name = key_name.to_lowercase();
        if let Some(&down) = self.state.physical_down.lock().get(&name) {
            return down;
        }
        let vk = platform::name_to_vk(&name);
        if vk == 0 {
            return false;
        }
        platform::get_async_key_state(vk as i32)
    }

    pub fn start(&self) -> Result<(), String> {
        self.start_local_hooks()
    }

    fn start_local_hooks(&self) -> Result<(), String> {
        if self.running.load(Ordering::Acquire) {
            return Ok(());
        }

        let (started_tx, started_rx) = unbounded::<Result<(), String>>();
        let (stop_tx, stop_rx) = unbounded::<()>();
        *self.stop_tx.lock() = Some(stop_tx);

        self.running.store(true, Ordering::Release);
        let shared = self.state.clone();

        thread::Builder::new()
            .name("input-hooks".into())
            .spawn(move || {
                hook_thread_fn(shared, started_tx, stop_rx);
            })
            .map_err(|e| format!("spawn failed: {}", e))?;

        started_rx
            .recv()
            .map_err(|e| format!("hook start channel: {}", e))??;

        Ok(())
    }

    pub fn stop(&self) {
        self.stop_local_hooks();
    }

    fn stop_local_hooks(&self) {
        if !self.running.swap(false, Ordering::AcqRel) {
            return;
        }
        if let Some(tx) = self.stop_tx.lock().take() {
            let _ = tx.send(());
        }
        self.reset_all_states();
    }
}

static SENDING_COUNTER: AtomicI32 = AtomicI32::new(0);

// ── Hook thread ─────────────────────────────────────────────────────────

fn hook_thread_fn(
    shared: HookSharedState,
    started: Sender<Result<(), String>>,
    stop_rx: Receiver<()>,
) {
    // Install keyboard hook
    let kb_shared = shared.clone();
    let kb_cb: platform::HookCallback = Arc::new(move |key_name: &str, is_down: bool| {
        handle_hotkey_key(&kb_shared, key_name, is_down)
    });
    let kb_hook = match platform::set_keyboard_hook(kb_cb) {
        Ok(h) => h,
        Err(e) => {
            let _ = started.send(Err(e));
            return;
        }
    };

    // Install mouse hook
    let ms_shared = shared.clone();
    let ms_cb: platform::HookCallback = Arc::new(move |key_name: &str, is_down: bool| {
        handle_hotkey_key(&ms_shared, key_name, is_down)
    });
    let ms_hook = match platform::set_mouse_hook(ms_cb) {
        Ok(h) => h,
        Err(e) => {
            platform::unhook(kb_hook);
            let _ = started.send(Err(e));
            return;
        }
    };

    let _ = started.send(Ok(()));

    // Platform-specific message/event loop.
    let _ = platform::run_hook_message_loop(stop_rx);

    platform::unhook(kb_hook);
    platform::unhook(ms_hook);
}

/// Decide whether the physical key/button event should be eaten and forwarded
/// to the macro engine.
///
/// Suppression logic (fail-closed — when in doubt, pass the key through):
/// 1. Paused or engine inactive → don't suppress.
/// 2. No game PID configured → don't suppress.
/// 3. Our own window is foreground → don't suppress (user is configuring).
/// 4. The game window is foreground → suppress.
/// 5. Some other app is foreground → don't suppress (the user is somewhere
///    else; eating their keypress is the bug we are guarding against).
/// 6. Foreground is UNKNOWN (PID 0 — Niri IPC down, X11 unavailable,
///    no compositor API) → don't suppress. Macros stay silent rather than
///    fire into the wrong window. The per-macro `background` flag remains
///    the explicit opt-in for cross-app behaviour.
// ── Test-only focus injection ───────────────────────────────────────────
//
// The suppression gate normally calls `platform::get_foreground_window()` to
// ask the OS which window is focused. Under unit tests there is no real
// compositor, so the platform returns PID 0 (unknown). To exercise the
// gate without mocking the platform, tests can stash a synthetic PID into
// `TEST_FOCUSED_PID` (non-zero) before calling `handle_hotkey_key`, and
// zero it again afterwards.
#[cfg(test)]
static TEST_FOCUSED_PID: AtomicU32 = AtomicU32::new(0);

/// One-shot warn so the emergency-stop explanation doesn't spam the log
/// on every mouse press after Ctrl+Shift+Esc is hit by accident.
static PAUSED_WARN_EMITTED: AtomicBool = AtomicBool::new(false);

fn should_suppress_hotkey(state: &HookSharedState, hk: &HotkeyInfo) -> bool {
    if state.paused.load(Ordering::Acquire) {
        if !PAUSED_WARN_EMITTED.swap(true, Ordering::AcqRel) {
            log::warn!(
                "[input] gate DENY: paused=true. Ctrl+Shift+Esc emergency-stop is engaged. \
                 Press the toggle key (ScrollLock per config) to unpause, or check the \
                 engine state via the overlay. No macros will fire until unpaused."
            );
        }
        return false;
    }
    // Reset the warn flag when not paused so the next accidental trigger
    // is logged again (helpful if the user paused, debugged, unpaused,
    // and re-paused by accident — they'd otherwise miss the second
    // warning).
    PAUSED_WARN_EMITTED.store(false, Ordering::Release);
    if !state.engine_active.load(Ordering::Acquire) {
        log::trace!("[input] gate deny: engine_active=false (toggle key off)");
        return false;
    }

    let game_pid = state.game_pid.load(Ordering::Acquire);
    if game_pid == 0 {
        log::trace!("[input] gate deny: game_pid=0 (no game configured/detected)");
        return false;
    }

    let (fg_pid, own_pid): (u32, u32) = {
        #[cfg(test)]
        {
            let injected = TEST_FOCUSED_PID.load(Ordering::Acquire);
            if injected != 0 {
                (injected, state.own_pid)
            } else {
                let fg = platform::get_foreground_window();
                let (_, p) = platform::get_window_thread_process_id(fg);
                (p, state.own_pid)
            }
        }
        #[cfg(not(test))]
        {
            let fg = platform::get_foreground_window();
            let (_, p) = platform::get_window_thread_process_id(fg);
            (p, state.own_pid)
        }
    };

    // Our own window foreground → don't suppress (user is configuring).
    // Skip this when fg_pid == 0 — the comparator might be returning its
    // own process ID for a moment, and we don't want that to override the
    // "unknown focus" deny below.
    if fg_pid != 0 && fg_pid == own_pid {
        return false;
    }

    // Game is foreground → suppress.
    if fg_pid == game_pid {
        return true;
    }

    // Focus is known and is not the game → never suppress. The user is
    // somewhere else (browser, terminal, Discord) and we must not eat
    // their keypresses.
    if fg_pid != 0 {
        return false;
    }

    // Focus UNKNOWN (fg_pid == 0): fail CLOSED. The historical fallback
    // returned `game_alive` here, which silently leaked macros into every
    // other app on compositors where the platform layer couldn't determine
    // focus (notably Niri without an explicit IPC call). Macros may stop
    // firing if our focus source dies — that is strictly safer than firing
    // them into the wrong window. The dedicated `allow_background` flag on
    // each macro remains the explicit way to opt into cross-app firing.
    log::debug!("[input] focus unknown (fg_pid=0); denying suppression");
    false
}

fn should_consume_hotkey_event(
    is_down: bool,
    hotkey: &HotkeyInfo,
    transition: Transition,
) -> bool {
    transition != Transition::None || (is_down && hotkey.is_active())
}

/// Returns true if the event was consumed (suppressed), false if it should
/// pass through to the OS. Only registered, enabled hotkeys that pass the
/// suppression gate are consumed; everything else passes through.
fn handle_hotkey_key(state: &HookSharedState, key_name: &str, is_down: bool) -> bool {
    let key_name = key_name.to_lowercase();
    state
        .physical_down
        .lock()
        .insert(key_name.clone(), is_down);

    // ── EMERGENCY STOP ──────────────────────────────────────────────
    // Ctrl+Shift+Escape always kills all macros, regardless of config.
    // This is hardcoded so it works even if the toggle key is broken.
    // We check physical_down for all three keys being pressed.
    {
        let pd = state.physical_down.lock();
        let ctrl = *pd.get("ctrl").unwrap_or(&false) || *pd.get("control").unwrap_or(&false);
        let shift = *pd.get("shift").unwrap_or(&false);
        let esc = *pd.get("escape").unwrap_or(&false);
        if ctrl && shift && esc && is_down {
            log::warn!("[input] EMERGENCY STOP: Ctrl+Shift+Esc pressed — killing all macros");
            state.paused.store(true, Ordering::Release);
            drop(pd);
            // Clear all hotkey states
            for hk in state.hotkeys.lock().values() {
                hk.set_state(HotkeyState::Idle);
            }
            return false; // let the Esc through too
        }
    }

    let hotkeys = state.hotkeys.lock();
    let hk = match hotkeys.get(&key_name) {
        Some(h) => h.clone(),
        None => return false, // not a registered hotkey — pass through
    };
    drop(hotkeys);

    if !hk.is_enabled() {
        return false; // hotkey disabled — pass through
    }

    // ── Anti-wedge: releases must ALWAYS advance the state machine ──
    // The suppression gate decides whether the physical event is eaten,
    // NOT whether the hotkey state machine runs. If we skipped on_key_up
    // whenever the gate closed (focus flicker, game exit, pause), the FSM
    // stayed stuck in Pressed/Holding forever and the next press looked
    // like autorepeat (Transition::None) — the macro could never trigger
    // again until the game restarted.
    let transition = if is_down {
        hk.on_key_down()
    } else {
        hk.on_key_up()
    };

    let gate_open = should_suppress_hotkey(state, &hk);

    // A press while the gate is closed belongs to the desktop, not the
    // game: revert the FSM so nothing wedges and never START a macro from
    // a desktop press (keys would be injected into the focused app).
    //
    // Anti-wedge: also reset the FSM on every gate-closed event when the
    // hotkey is mid-Press. Without this, a single stray press while the
    // game was unfocused (e.g. user alt-tabbed to a chat window for one
    // second and bumped the mouse button) leaves the FSM in Pressed/Holding
    // forever; the next press during the game then returns Transition::None
    // (autorepeat) and the macro never fires until macrotool restarts.
    if !gate_open {
        hk.set_state(HotkeyState::Idle);
        if matches!(transition, Transition::Start | Transition::Toggle) {
            return false;
        }
    }

    log::debug!(
        "[input] hotkey {} down={} transition={:?} gate={}",
        key_name,
        is_down,
        transition,
        gate_open
    );
    state.dispatch(&key_name, transition);

    // Gate open → the event belongs to us: consume it (and any autorepeat
    // while active) so the physical hotkey cannot leak into the game.
    // Gate closed → pass the physical event through untouched.
    if !gate_open {
        return false;
    }
    should_consume_hotkey_event(is_down, &hk, transition)
}

#[cfg(test)]
mod tests {
    use super::{
        handle_hotkey_key, should_consume_hotkey_event, HookSharedState, HotkeyInfo, HotkeyMode,
        Transition, TEST_FOCUSED_PID,
    };
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    /// Helper: every test that touches the suppression gate sets a synthetic
    /// foreground PID and resets it on drop, so concurrent tests can't
    /// observe each other's injected focus.
    struct FocusGuard(u32);
    impl FocusGuard {
        fn set(pid: u32) -> Self {
            TEST_FOCUSED_PID.store(pid, Ordering::Release);
            FocusGuard(pid)
        }
    }
    impl Drop for FocusGuard {
        fn drop(&mut self) {
            TEST_FOCUSED_PID.store(0, Ordering::Release);
        }
    }

    #[test]
    fn active_macro_hotkey_suppresses_key_repeat_without_redispatching() {
        let hotkey = HotkeyInfo::new(HotkeyMode::Press);
        assert_eq!(hotkey.on_key_down(), Transition::Start);

        let repeat = hotkey.on_key_down();

        assert_eq!(repeat, Transition::None);
        assert!(should_consume_hotkey_event(true, &hotkey, repeat));
    }

    fn armed_state() -> HookSharedState {
        let state = HookSharedState::new();
        state.hotkeys.lock().insert(
            "rbutton".to_string(),
            Arc::new(HotkeyInfo::new(HotkeyMode::Hold)),
        );
        state.engine_active.store(true, Ordering::Release);
        state.game_pid.store(42, Ordering::Release);
        state.game_alive.store(true, Ordering::Release);
        state
    }

    #[test]
    fn release_while_gate_closed_still_advances_state_machine() {
        let state = armed_state();
        let _focus = FocusGuard::set(42); // game focused → gate open

        // Gate open (game focused): press is consumed and activates the hotkey.
        assert!(handle_hotkey_key(&state, "rbutton", true));
        assert!(state.hotkeys.lock()["rbutton"].is_active());

        // Gate closes between press and release (focus lost).
        drop(_focus);
        let _focus_unknown = FocusGuard::set(0); // unknown focus → gate closed
        state.game_alive.store(false, Ordering::Release);

        // The release PASSES THROUGH (gate closed, not our event anymore)…
        assert!(!handle_hotkey_key(&state, "rbutton", false));
        // …but the state machine still advanced to Idle — no wedge.
        assert!(!state.hotkeys.lock()["rbutton"].is_active());

        // Next press with the gate open starts cleanly instead of being
        // swallowed as "autorepeat" — the old bug made this impossible.
        drop(_focus_unknown);
        let _focus = FocusGuard::set(42);
        state.game_alive.store(true, Ordering::Release);
        assert!(handle_hotkey_key(&state, "rbutton", true));
        assert!(state.hotkeys.lock()["rbutton"].is_active());
    }

    #[test]
    fn press_while_gate_closed_does_not_start_a_macro() {
        let state = armed_state();
        // Gate closed: user is on the desktop — focus is a different PID.
        let _focus = FocusGuard::set(9999);
        state.game_alive.store(false, Ordering::Release);

        assert!(!handle_hotkey_key(&state, "rbutton", true));
        assert!(
            !state.hotkeys.lock()["rbutton"].is_active(),
            "desktop press must not arm the hotkey"
        );
    }

    #[test]
    fn unknown_focus_fails_closed_even_when_game_is_alive() {
        // Regression: under Niri without an explicit IPC call, the platform
        // layer returned PID 0 for the foreground window. The old gate then
        // fell back to `game_alive`, so any registered hotkey press was
        // suppressed and the macro fired — even if the user was typing in
        // a browser. New contract: unknown focus → no suppression.
        let state = armed_state();
        let _focus = FocusGuard::set(0); // unknown focus
        state.game_alive.store(true, Ordering::Release);

        assert!(
            !handle_hotkey_key(&state, "rbutton", true),
            "press with unknown focus must pass through to the focused app"
        );
        assert!(
            !state.hotkeys.lock()["rbutton"].is_active(),
            "press with unknown focus must not arm the hotkey"
        );
    }

    #[test]
    fn own_window_focused_does_not_suppress() {
        // The user is interacting with macrotool's own UI: hotkeys must
        // pass through so the QML bindings can react to them. The old
        // check had `fg_pid != 0` baked in but skipped on the fallback
        // path; the new check is explicit and survives refactors.
        let state = armed_state();
        let _focus = FocusGuard::set(state.own_pid);

        assert!(!handle_hotkey_key(&state, "rbutton", true));
        assert!(!state.hotkeys.lock()["rbutton"].is_active());
    }
}
