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
/// Suppression logic:
/// 1. Must not be paused, engine must be active, game PID must be set.
/// 2. If our own window is foreground → don't suppress (user is configuring).
/// 3. If the game window is foreground → suppress (hotkey is for the game).
/// 4. If the game is alive but not foreground:
///    - For background macros → suppress (they fire via global uinput injection).
///    - For non-background macros → don't suppress (user is on another app,
///      we shouldn't eat their keypress). The macro engine's send path will
///      also drop the key if the game isn't foreground, so suppressing here
///      would just waste the input.
fn should_suppress_hotkey(state: &HookSharedState, hk: &HotkeyInfo) -> bool {
    if state.paused.load(Ordering::Acquire) {
        return false;
    }
    if !state.engine_active.load(Ordering::Acquire) {
        return false;
    }

    let game_pid = state.game_pid.load(Ordering::Acquire);
    if game_pid == 0 {
        return false;
    }

    let fg = platform::get_foreground_window();
    let (_, fg_pid) = platform::get_window_thread_process_id(fg);

    // Our own window foreground → don't suppress
    if fg_pid == state.own_pid && fg_pid != 0 {
        return false;
    }

    // Game is foreground → suppress
    if fg_pid == game_pid {
        return true;
    }

    // On Wayland we often cannot detect the foreground window (fg_pid == 0).
    // In that case, fall back to "game alive = suppress allowed" so macros
    // still fire. Without this, the suppression gate is permanently closed
    // on Wayland and no macro ever fires.
    if fg_pid == 0 {
        let game_alive = state.game_alive.load(Ordering::Acquire);
        return game_alive;
    }

    // Game is alive but not foreground.
    // Background macros: suppress (they inject globally via uinput).
    // Non-background macros: don't suppress (user is on another app,
    //   eating their keypress would just block them for no benefit —
    //   the send path drops the key when the game isn't foreground).
    let game_alive = state.game_alive.load(Ordering::Acquire);
    if game_alive && hk.is_background() {
        return true;
    }

    false
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

    if !should_suppress_hotkey(state, &hk) {
        return false; // gate says don't suppress — pass through
    }

    let transition = if is_down {
        hk.on_key_down()
    } else {
        hk.on_key_up()
    };
    log::debug!(
        "[input] hotkey {} down={} transition={:?}",
        key_name,
        is_down,
        transition
    );
    state.dispatch(&key_name, transition);
    // Suppress the key only when we actually dispatched a transition
    transition != Transition::None
}
