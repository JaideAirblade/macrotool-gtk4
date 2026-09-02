//! Pixel Trigger Engine — polls screen pixels and fires action keys on match.
//!
//! Direct port of the Go `internal/engine/pixel.go`.

use crate::config::{self, PixelTrigger};
use crate::engine::macro_engine::EngineHandle;
use crate::platform;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

pub struct PixelEngine {
    triggers: Arc<Mutex<Vec<PixelTrigger>>>,
    stop_flag: Arc<AtomicBool>,
    worker_started: AtomicBool,
    handle: Mutex<Option<EngineHandle>>,
}

impl PixelEngine {
    pub fn new() -> Self {
        PixelEngine {
            triggers: Arc::new(Mutex::new(Vec::new())),
            stop_flag: Arc::new(AtomicBool::new(false)),
            worker_started: AtomicBool::new(false),
            handle: Mutex::new(None),
        }
    }

    pub fn set_handle(&self, handle: EngineHandle) {
        *self.handle.lock() = Some(handle);
    }

    /// Initialize triggers from config. The worker thread reads the shared
    /// trigger list every tick, so edits take effect live without a restart.
    pub fn setup(&self, triggers: Vec<PixelTrigger>) {
        *self.triggers.lock() = triggers;
        self.start();
    }

    /// Start the background polling thread (spawn-once).
    pub fn start(&self) {
        if self
            .worker_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return; // worker already running
        }
        self.stop_flag.store(false, Ordering::Release);
        let triggers = self.triggers.clone();
        let stop = self.stop_flag.clone();
        let handle = self.handle.lock().clone();
        thread::Builder::new()
            .name("pixel-poll".into())
            .spawn(move || {
                poll_loop(triggers, stop, handle);
            })
            .ok();
    }

    /// Stop the polling thread (shutdown only — the worker exits on its own).
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::Release);
    }

    pub fn is_running(&self) -> bool {
        self.worker_started.load(Ordering::Acquire) && !self.stop_flag.load(Ordering::Acquire)
    }
}

fn requires_screen_capture(triggers: &[PixelTrigger]) -> bool {
    !triggers.is_empty()
}

fn poll_loop(
    triggers: Arc<Mutex<Vec<PixelTrigger>>>,
    stop: Arc<AtomicBool>,
    handle: Option<EngineHandle>,
) {
    platform::set_thread_priority_above_normal();

    let mut last_fired: HashMap<String, Instant> = HashMap::new();

    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }

        // Re-read settings + trigger list every tick so config edits
        // (rate, added/removed triggers) apply without a restart.
        let rate = handle
            .as_ref()
            .map(|h| h.cfg.settings().pixel_check_rate)
            .unwrap_or(250)
            .clamp(10, 1000);
        let triggers = triggers.lock().clone();
        if !requires_screen_capture(&triggers) {
            thread::sleep(Duration::from_millis(rate as u64));
            continue;
        }

        let (cur_w, cur_h) = platform::get_screen_resolution();
        let now = Instant::now();

        for trigger in &triggers {
            if !trigger.enabled {
                continue;
            }
            // Per-trigger cooldown from config (was previously a hardcoded
            // 500ms for all triggers, ignoring the UI's Cooldown field).
            let cooldown = Duration::from_millis(trigger.cooldown.max(0) as u64);
            if let Some(last) = last_fired.get(&trigger.name) {
                if now.duration_since(*last) < cooldown {
                    continue;
                }
            }

            // Anchor check
            if let Some(ref anchor) = trigger.anchor {
                if !anchor.pixels.is_empty() {
                    let anchor_px =
                        scale_pixels(&anchor.pixels, &trigger.capture_res, cur_w, cur_h);
                    if !platform::check_pixels_batched(&anchor_px, &anchor.match_mode) {
                        continue;
                    }
                }
            }

            // Blocker check
            if let Some(ref blocker) = trigger.blocker {
                if !blocker.pixels.is_empty() {
                    let blocker_px =
                        scale_pixels(&blocker.pixels, &trigger.capture_res, cur_w, cur_h);
                    if platform::check_pixels_batched(&blocker_px, &blocker.match_mode) {
                        continue;
                    }
                }
            }

            // Pixel region check. `inverse` flips the result: fire when the
            // pixels do NOT match (was previously parsed but never applied).
            let scaled = scale_pixels(&trigger.pixels, &trigger.capture_res, cur_w, cur_h);
            let matched = platform::check_pixels_batched(&scaled, &trigger.match_mode)
                != trigger.inverse;

            if matched {
                // trigger_mode "macro": only fire while a macro is running.
                // macro_hotkey narrows it to one specific macro (empty = any).
                // Previously both fields were parsed but never enforced.
                if trigger.trigger_mode == "macro" {
                    let me = crate::engine::macro_engine::get_macro_engine();
                    let running = if trigger.macro_hotkey.is_empty() {
                        me.map(|m| m.any_running()).unwrap_or(false)
                    } else {
                        me.map(|m| m.is_running(&trigger.macro_hotkey))
                            .unwrap_or(false)
                    };
                    if !running {
                        continue;
                    }
                }

                // Fire only while the game is actually the focused window.
                // Pixel triggers must never fire onto the desktop, and the
                // removed `allowBackground` setting was precisely the escape
                // hatch that let them.
                let game_in_focus = handle
                    .as_ref()
                    .map(|h| h.detector.is_in_focus())
                    .unwrap_or(false);

                if !game_in_focus {
                    continue;
                }

                if !trigger.action_key.is_empty() {
                    if let Some(ref h) = handle {
                        h.input.acquire_sending();
                        platform::send_key(&trigger.action_key);
                        h.input.release_sending();
                        // Activate buffs for this key
                        check_buffs(&h.cfg, &h.buffs, &trigger.action_key);
                    } else {
                        platform::send_key(&trigger.action_key);
                    }
                }
                last_fired.insert(trigger.name.clone(), now);
            }
        }

        thread::sleep(Duration::from_millis(rate as u64));
    }
}

/// Check buffs for key-activated buffs matching the given key.
fn check_buffs(
    cfg: &Arc<config::Manager>,
    buffs: &Arc<crate::engine::buff::BuffEngine>,
    key: &str,
) {
    let key_lower = key.to_lowercase();
    for b in cfg.get_buff_timers().iter() {
        if !b.enabled || b.trigger_type != "keys" {
            continue;
        }
        if b.watch_keys.iter().any(|wk| wk.to_lowercase() == key_lower) {
            buffs.activate(b.clone());
        }
    }
}

/// Convert config pixels into platform tuples and scale them.
fn scale_pixels(
    pixels: &[config::Pixel],
    capture_res: &Option<config::Resolution>,
    cur_w: i32,
    cur_h: i32,
) -> Vec<(i32, i32, u32, i32)> {
    let result: Vec<(i32, i32, u32, i32)> = pixels
        .iter()
        .map(|p| (p.x, p.y, parse_color(&p.color), p.variation))
        .collect();
    let cr = match capture_res {
        Some(r) if r.w > 0 && r.h > 0 && (r.w != cur_w || r.h != cur_h) => r,
        _ => return result,
    };

    let rx = cur_w as f64 / cr.w as f64;
    let ry = cur_h as f64 / cr.h as f64;
    result
        .iter()
        .map(|(x, y, c, v)| {
            (
                (*x as f64 * rx + 0.5) as i32,
                (*y as f64 * ry + 0.5) as i32,
                *c,
                *v,
            )
        })
        .collect()
}

fn parse_color(s: &str) -> u32 {
    let s = s
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .trim_start_matches('#');
    u32::from_str_radix(s, 16).unwrap_or(0xFFFFFFFF)
}

#[cfg(test)]
mod tests {
    use super::requires_screen_capture;
    use crate::config::PixelTrigger;

    #[test]
    fn empty_trigger_list_does_not_capture_the_screen() {
        let triggers: Vec<PixelTrigger> = Vec::new();
        assert!(!requires_screen_capture(&triggers));
    }
}
