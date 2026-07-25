//! Engine Hub — central coordinator that owns all engine instances.
//!
//! Wires together: InputManager, GameDetector, MacroEngine, BuffEngine,
//! PixelEngine. Manages the full lifecycle: start, reload profile, stop.
//! Also runs the toggle-key polling loop.

use crate::config;
use crate::engine::buff::BuffEngine;
use crate::engine::game::GameDetector;
use crate::engine::input::InputManager;
use crate::engine::macro_engine::{self, EngineHandle, MacroEngine};
use crate::engine::pixel::PixelEngine;
use crate::platform;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct EngineHub {
    pub cfg: Arc<config::Manager>,
    pub input: Arc<InputManager>,
    pub detector: Arc<GameDetector>,
    pub macros: Arc<MacroEngine>,
    pub buffs: Arc<BuffEngine>,
    pub pixels: Arc<PixelEngine>,
    pub macro_enabled: Arc<AtomicBool>,
}

impl EngineHub {
    pub fn new(cfg: Arc<config::Manager>) -> Self {
        let input = Arc::new(InputManager::new());
        let detector = Arc::new(GameDetector::new());
        let buffs = Arc::new(BuffEngine::new());
        let pixels = Arc::new(PixelEngine::new());

        let handle = EngineHandle {
            input: input.clone(),
            cfg: cfg.clone(),
            detector: detector.clone(),
            buffs: buffs.clone(),
        };

        let macros = Arc::new(MacroEngine::new(handle.clone()));
        buffs.set_handle(handle.clone());
        pixels.set_handle(handle.clone());

        // Register macro engine globally so pixel engine can check running state
        macro_engine::set_macro_engine(macros.clone());

        EngineHub {
            cfg,
            input,
            detector,
            macros,
            buffs,
            pixels,
            macro_enabled: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Start all engines and background loops.
    pub fn start(&self) {
        // Start input hooks
        if let Err(e) = self.input.start() {
            log::error!("[hub] failed to start input hooks: {}", e);
        }
        self.input.set_engine_active(true);

        // Start game detector
        let cfg = self.cfg.clone();
        self.detector.start(cfg.clone());

        // Wire game-active callback: sync focus flag to input hooks so they only
        // suppress macro hotkeys when the game is actually focused.
        let input_mgr = self.input.clone();
        let input_mgr_fg = self.input.clone();
        let detector_for_fg = self.detector.clone();
        self.detector
            .register_active_callback(Box::new(move |active| {
                log::info!("[hub] game active changed: {}", active);
                if !active {
                    input_mgr.reset_all_states();
                }
            }));
        detector_for_fg.register_foreground_callback(Box::new(move |focused, alive, pid| {
            log::info!(
                "[hub] game foreground changed: focused={} alive={} pid={}",
                focused,
                alive,
                pid
            );
            input_mgr_fg.set_game_alive(alive);
            input_mgr_fg.set_game_pid(pid);
        }));

        // Initial profile load
        self.reload_profile();

        // ── Background threads ──

        // Debounced-save ticker
        let cfg_save = self.cfg.clone();
        std::thread::Builder::new()
            .name("save-ticker".into())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(100));
                cfg_save.check_debounced_save();
            })
            .ok();

        // Toggle-key polling loop (global enable/disable)
        let cfg_toggle = self.cfg.clone();
        let macros_toggle = self.macros.clone();
        let enabled_flag = self.macro_enabled.clone();
        std::thread::Builder::new()
            .name("toggle-key".into())
            .spawn(move || {
                let mut was_down = false;
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(50));

                    let key = cfg_toggle.settings().toggle_key;
                    let vk = platform::name_to_vk(&key);
                    if vk == 0 {
                        continue;
                    }

                    // Handle lock keys (ScrollLock, CapsLock, NumLock) by toggle state
                    let key_lower = key.to_lowercase();
                    let is_lock_key = key_lower == "scrolllock"
                        || key_lower == "capslock"
                        || key_lower == "numlock";

                    if is_lock_key {
                        let state = (platform::get_key_state_toggled(vk as i32) & 0x0001) != 0;
                        if state != was_down {
                            was_down = state;
                            let old = enabled_flag.load(Ordering::Acquire);
                            enabled_flag.store(!old, Ordering::Release);
                            macros_toggle.set_paused(!old);
                            log::info!("[hub] toggle key {} → macros {}", key, !old);
                        }
                    } else {
                        let down = platform::get_async_key_state(vk as i32);
                        if down && !was_down {
                            was_down = true;
                            let old = enabled_flag.load(Ordering::Acquire);
                            enabled_flag.store(!old, Ordering::Release);
                            macros_toggle.set_paused(!old);
                            log::info!("[hub] toggle key {} → macros {}", key, !old);
                        } else if !down {
                            was_down = false;
                        }
                    }
                }
            })
            .ok();
    }

    /// Reload the active profile into all engines.
    pub fn reload_profile(&self) {
        let macros = self.cfg.get_macros();
        let triggers = self.cfg.get_pixel_triggers();
        self.macros.setup(macros);
        self.pixels.setup(triggers);
    }
}

impl Drop for EngineHub {
    fn drop(&mut self) {
        self.macros.cleanup();
        self.pixels.stop();
        self.buffs.stop();
        self.input.stop();
    }
}