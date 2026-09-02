//! Macro Engine — manages macro hotkeys and execution.
//!
//! Supports press / hold / toggle modes. Keys are injected via uinput if and
//! only if the configured game is the live focused window, as reported by the
//! detector's fresh `/proc` basename comparison. There is no background mode:
//! injecting into a window we cannot positively identify as the game is the
//! misfire this design removes.

use crate::config::{self, Macro};
use crate::engine::game::GameDetector;
use crate::engine::input;
use crate::platform;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Lightweight handle for engines to access shared services.
#[derive(Clone)]
pub struct EngineHandle {
    pub input: Arc<input::InputManager>,
    pub cfg: Arc<config::Manager>,
    pub detector: Arc<GameDetector>,
    pub buffs: Arc<crate::engine::buff::BuffEngine>,
}

/// Global reference to MacroEngine for engines that need to check running state.
/// Set once by the hub after construction.
static MACRO_ENGINE_REF: once_cell::sync::OnceCell<Arc<MacroEngine>> =
    once_cell::sync::OnceCell::new();

pub fn set_macro_engine(macros: Arc<MacroEngine>) {
    let _ = MACRO_ENGINE_REF.set(macros);
}

pub fn get_macro_engine() -> Option<&'static Arc<MacroEngine>> {
    MACRO_ENGINE_REF.get()
}

pub struct MacroEngine {
    profile: Mutex<HashMap<String, Macro>>,
    running: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    handle: EngineHandle,
    paused: Arc<AtomicBool>,
}

impl MacroEngine {
    pub fn new(handle: EngineHandle) -> Self {
        MacroEngine {
            profile: Mutex::new(HashMap::new()),
            running: Arc::new(Mutex::new(HashMap::new())),
            handle,
            paused: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Release);
        self.handle.input.set_paused(paused);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    pub fn is_running(&self, hotkey: &str) -> bool {
        self.running
            .lock()
            .get(&hotkey.to_lowercase())
            .map(|f| f.load(Ordering::Acquire))
            .unwrap_or(false)
    }

    pub fn any_running(&self) -> bool {
        self.running
            .lock()
            .values()
            .any(|f| f.load(Ordering::Acquire))
    }

    /// Initialize hotkeys from config.
    pub fn setup(&self, macros: Vec<Macro>) {
        self.cleanup();

        let mut profile = self.profile.lock();
        let mut running = self.running.lock();

        // Do any registered hotkeys use keyboard keys? If every hotkey is a
        // mouse button (rbutton/xbutton2/...), keyboards must NOT be grabbed:
        // macrotool then stays completely out of the physical typing path and
        // cannot stick movement keys like a/s/w.
        let kb_needed = macros
            .iter()
            .any(|m| !m.hotkey.is_empty() && !platform::is_mouse_key(&m.hotkey));
        platform::set_keyboard_grab_needed(kb_needed);

        for m in macros {
            let hk = m.hotkey.to_lowercase();
            if hk.is_empty() {
                continue;
            }
            let mode = input::parse_mode(&m.mode);
            self.handle.input.register_hotkey(&hk, mode);
            self.handle.input.set_enabled(&hk, m.enabled);
            profile.insert(hk.clone(), m);
            running.insert(hk, Arc::new(AtomicBool::new(false)));
        }
        drop(profile);
        drop(running);

        // Register event handler — closures capture clones, not references
        let handle = self.handle.clone();
        let paused = self.paused.clone();
        let running_map = self.running.clone();

        self.handle.input.on_event(Box::new(move |key, transition| {
            let profile_map = handle.cfg.get_macros();
            let hk = key.to_lowercase();
            let m = match profile_map
                .iter()
                .find(|mac| mac.hotkey.to_lowercase() == hk)
            {
                Some(m) => m.clone(),
                None => return,
            };

            if paused.load(Ordering::Acquire) || !m.enabled {
                return;
            }

            match transition {
                input::Transition::Start => {
                    start_macro(handle.clone(), hk.clone(), m, running_map.clone());
                }
                input::Transition::Stop => {
                    // Press-mode macros run to completion once started —
                    // releasing the hotkey must NOT truncate the key sequence
                    // mid-flight (previously a quick tap could abort before
                    // the first key even sent). Stop only applies to
                    // hold/toggle modes.
                    if m.mode != "press" {
                        stop_macro(&handle, &hk, &running_map);
                    }
                }
                input::Transition::Toggle => {
                    // Toggle ON: state machine flipped Idle→Holding.
                    // Just start the macro. The turn-off is handled by
                    // Transition::Stop (Holding→Idle), which makes hold_loop
                    // exit via the detector.is_in_focus() check.
                    start_macro(handle.clone(), hk.clone(), m, running_map.clone());
                }
                input::Transition::None => {}
            }
        }));
    }

    pub fn cleanup(&self) {
        self.handle.input.clear_hotkeys();
        self.handle.input.clear_handlers();
        for flag in self.running.lock().values() {
            flag.store(false, Ordering::Release); // false = stop
        }
        self.running.lock().clear();
        self.profile.lock().clear();
    }
}

fn start_macro(
    handle: EngineHandle,
    hk: String,
    m: Macro,
    running_map: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
) {
    // Get the running flag from the map — serves dual purpose:
    // true = running, false = stop requested
    let running_flag = running_map.lock().get(&hk).cloned();
    let running_flag = match running_flag {
        Some(f) => f,
        None => return, // hotkey not registered
    };
    running_flag.store(true, Ordering::Release);
    log::debug!("[macro] starting macro {} mode={}", hk, m.mode);

    if m.mode == "press" {
        let h = handle.clone();
        let flag_clone = running_flag.clone();
        thread::spawn(move || {
            h.input.acquire_sending();
            send_key_sequence(&m.keys, m.inter_key_delay, &flag_clone, &h);
            h.input.release_sending();
            // Clear running flag when done
            flag_clone.store(false, Ordering::Release);
        });
    } else {
        // Hold/toggle mode
        let h = handle.clone();
        let flag_clone = running_flag.clone();
        thread::spawn(move || {
            hold_loop(hk, m, flag_clone, h);
            // Clear running flag when the loop exits
            // (stop_macro may have already set it to false, that's fine)
        });
    }
}

fn stop_macro(
    _handle: &EngineHandle,
    hk: &str,
    running_map: &Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
) {
    // Signal the hold_loop to stop immediately by setting the running flag to false.
    // The loop checks this flag every iteration (before sleeping).
    if let Some(flag) = running_map.lock().get(hk) {
        flag.store(false, Ordering::Release);
    }
}

fn configured_hold_duration(seconds: i32) -> Option<Duration> {
    (seconds > 0).then(|| Duration::from_secs(seconds as u64))
}

fn hold_loop(hk: String, m: Macro, stop_flag: Arc<AtomicBool>, handle: EngineHandle) {
    platform::set_macro_thread_affinity();
    platform::set_thread_priority_above_normal();

    let keys = m.keys.clone();
    if keys.is_empty() {
        stop_flag.store(false, Ordering::Release);
        return;
    }

    let mut interval = m.delay;
    if interval < 1 {
        interval = 1;
    }
    let ikd = m.inter_key_delay;
    let start = Instant::now();
    let max_duration = configured_hold_duration(m.max_hold_duration);

    loop {
        // The stop_flag IS the running flag: true = running, false = stop requested.
        // stop_macro() sets it to false to signal immediate stop.
        if !stop_flag.load(Ordering::Acquire) {
            break;
        }

        // For hold mode: also check if the key/button is still physically held
        if m.mode == "hold" && !handle.input.is_hotkey_physically_down(&hk) {
            break;
        }

        // For toggle mode: also check the hotkey state machine
        if m.mode == "toggle" && !handle.input.is_active(&hk) {
            break;
        }

        // Check if the macro is still enabled in the current profile. The user can
        // disable a macro from the UI; that should stop it immediately rather than
        // at the end of the interval.
        if !m.enabled || handle.input.is_paused() {
            break;
        }

        // Check engine-active. Every macro stops when the engine is switched
        // off — the removed `background` flag used to exempt itself from this
        // check and keep firing into whatever window happened to be focused.
        if !handle
            .input
            .shared_state()
            .engine_active
            .load(Ordering::Acquire)
        {
            break;
        }

        // Send keys
        handle.input.acquire_sending();
        send_key_sequence(&keys, ikd, &stop_flag, &handle);
        handle.input.release_sending();

        if let Some(max_duration) = max_duration {
            if start.elapsed() > max_duration {
                break;
            }
        }

        // Sleep for the interval
        if interval <= 50 {
            platform::precise_sleep(interval as f64);
        } else {
            thread::sleep(Duration::from_millis(interval as u64));
        }
    }

    // Always clear the running flag when the loop exits so any
    // callers see a consistent stopped state.
    stop_flag.store(false, Ordering::Release);

    // Safety net: if the loop exited between an injected key-down and its
    // key-up (stop requested mid-sequence), release every key this macro
    // could still hold down. release_key is idempotent.
    for key in &keys {
        platform::release_key(key);
    }
}

/// Send a sequence of keys.
///
/// Delivery is unconditional in form (always uinput) and conditional in
/// permission: a key is injected only while the configured game is the live
/// focused window. Focus is re-checked per key rather than once per sequence,
/// so alt-tabbing mid-sequence stops the remaining keys instead of spraying
/// them into whatever the user switched to.
fn send_key_sequence(
    keys: &[String],
    ikd: i32,
    stop_flag: &AtomicBool,
    handle: &EngineHandle,
) {
    let detector = &handle.detector;

    for (i, key) in keys.iter().enumerate() {
        // stop_flag is the running flag: false = stop requested
        if !stop_flag.load(Ordering::Acquire) {
            return;
        }
        if i > 0 && ikd > 0 {
            platform::precise_sleep(ikd as f64);
        }

        // Inject only while the game is CURRENTLY focused. The detector
        // re-derives this from /proc, so a lost tick simply drops one key;
        // the held hotkey retries on the next interval.
        if detector.is_in_focus() {
            log::debug!("[macro] sending key {} (game focused)", key);
            platform::send_key(key);
        } else {
            log::debug!("[macro] dropping key {} (game not focused)", key);
        }

        // Activate buff timers for this key
        check_buffs(&handle.cfg, &handle.buffs, key);
    }
}

/// Check if any buff timers watch this key and activate them.
fn check_buffs(cfg: &config::Manager, buffs: &crate::engine::buff::BuffEngine, key: &str) {
    let key_lower = key.to_lowercase();
    let buff_timers = cfg.get_buff_timers();
    for b in &buff_timers {
        if !b.enabled || b.trigger_type != "keys" {
            continue;
        }
        if b.watch_keys.iter().any(|wk| wk.to_lowercase() == key_lower) {
            buffs.activate(b.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::configured_hold_duration;
    use std::time::Duration;

    #[test]
    fn zero_hold_duration_means_no_automatic_timeout() {
        assert_eq!(configured_hold_duration(0), None);
    }

    #[test]
    fn explicit_hold_duration_remains_supported() {
        assert_eq!(configured_hold_duration(90), Some(Duration::from_secs(90)));
    }
}
