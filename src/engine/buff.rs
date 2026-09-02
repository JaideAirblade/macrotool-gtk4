//! Buff Engine — priority queue of buff timers that fire action keys on expiry.
//!
//! Uses a min-heap of expiry times. A worker thread wakes up at
//! the next expiry, fires the action key, and removes the entry.

use crate::config::BuffTimer;
use crate::engine::macro_engine::EngineHandle;
use crate::platform;
use parking_lot::Mutex;
use std::collections::{BinaryHeap, HashMap};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone)]
struct BuffEntry {
    gen: u64,
    expire: Instant,
    start: Instant,
    duration: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExpiryAction {
    FireActionKey { play_sound: bool },
    SoundOnly,
}

#[derive(Clone)]
struct BuffHeapItem {
    expire: Instant,
    gen: u64,
    name: String,
    buff: BuffTimer,
    action: ExpiryAction,
}

// Min-heap by expire time (reverse Ord for BinaryHeap which is max-heap)
impl PartialEq for BuffHeapItem {
    fn eq(&self, other: &Self) -> bool {
        self.expire == other.expire
    }
}
impl Eq for BuffHeapItem {}
impl PartialOrd for BuffHeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for BuffHeapItem {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.expire.cmp(&self.expire) // reverse for min-heap
    }
}

/// Shared buff state — accessible from worker thread and public API.
struct BuffShared {
    entries: HashMap<String, BuffEntry>,
    heap: BinaryHeap<BuffHeapItem>,
}

pub struct BuffEngine {
    shared: Arc<Mutex<BuffShared>>,
    gen: AtomicU64,
    stop_flag: Arc<AtomicBool>,
    worker_started: AtomicBool,
    handle: Mutex<Option<EngineHandle>>, // set by hub after construction
}

impl BuffEngine {
    pub fn new() -> Self {
        BuffEngine {
            shared: Arc::new(Mutex::new(BuffShared {
                entries: HashMap::new(),
                heap: BinaryHeap::new(),
            })),
            gen: AtomicU64::new(0),
            stop_flag: Arc::new(AtomicBool::new(false)),
            worker_started: AtomicBool::new(false),
            handle: Mutex::new(None),
        }
    }

    /// Set the engine handle (for firing action keys). Called by the hub.
    pub fn set_handle(&self, handle: EngineHandle) {
        *self.handle.lock() = Some(handle);
    }

    /// Activate a normal buff timer. Expiry presses its configured action key.
    pub fn activate(&self, buff: BuffTimer) {
        let play_sound = buff.sound_on_expiry;
        self.schedule(buff, ExpiryAction::FireActionKey { play_sound });
    }

    /// Start a manual cooldown reminder. It uses the buff's configured duration,
    /// shows the usual countdown, and plays a sound on expiry without pressing
    /// the buff's action key.
    pub fn start_reminder(&self, mut buff: BuffTimer) {
        // A manual click always starts a fresh countdown, even when the buff's
        // automatic trigger is configured to ignore or extend refreshes.
        buff.on_refresh = "reset".into();
        self.schedule(buff, ExpiryAction::SoundOnly);
    }

    fn schedule(&self, buff: BuffTimer, action: ExpiryAction) {
        let name = buff.name.clone();
        let new_duration = Duration::from_millis(buff.duration.max(0) as u64);

        let mut shared = self.shared.lock();
        let gen = self.gen.fetch_add(1, Ordering::AcqRel);
        let now = Instant::now();

        if let Some(existing) = shared.entries.get(&name) {
            let on_refresh = buff.on_refresh.as_str();
            if on_refresh == "ignore" {
                return;
            }
            let final_duration = if on_refresh == "extend" {
                let elapsed = existing.start.elapsed();
                let remaining = existing.duration.saturating_sub(elapsed);
                remaining + Duration::from_millis(buff.extend_ms.max(0) as u64)
            } else {
                new_duration
            };
            let expire = now + final_duration;
            shared.entries.insert(
                name.clone(),
                BuffEntry {
                    gen,
                    expire,
                    start: now,
                    duration: final_duration,
                },
            );
            shared.heap.push(BuffHeapItem {
                expire,
                gen,
                name: name.clone(),
                buff,
                action,
            });
        } else {
            let expire = now + new_duration;
            shared.entries.insert(
                name.clone(),
                BuffEntry {
                    gen,
                    expire,
                    start: now,
                    duration: new_duration,
                },
            );
            shared.heap.push(BuffHeapItem {
                expire,
                gen,
                name: name.clone(),
                buff,
                action,
            });
        }
        drop(shared);

        self.ensure_worker();
    }

    pub fn clear_all(&self) {
        let mut shared = self.shared.lock();
        shared.entries.clear();
        shared.heap.clear();
    }

    pub fn get_active_timers(&self) -> HashMap<String, f64> {
        let shared = self.shared.lock();
        let now = Instant::now();
        shared
            .entries
            .iter()
            .map(|(name, e)| {
                let remaining = e.expire.saturating_duration_since(now).as_secs_f64() * 1000.0;
                (name.clone(), remaining)
            })
            .collect()
    }

    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::Release);
    }

    fn ensure_worker(&self) {
        if self
            .worker_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let shared = self.shared.clone();
        let stop = self.stop_flag.clone();
        let handle = self.handle.lock().clone();
        thread::Builder::new()
            .name("buff-worker".into())
            .spawn(move || {
                buff_worker(shared, stop, handle);
            })
            .ok();
    }
}

/// Background worker: wakes up at the next expiry, fires the action key,
/// and removes the entry. Sleeps in 100ms max intervals to stay responsive.
fn buff_worker(
    shared: Arc<Mutex<BuffShared>>,
    stop: Arc<AtomicBool>,
    handle: Option<EngineHandle>,
) {
    loop {
        if stop.load(Ordering::Acquire) {
            return;
        }

        let (to_fire, sleep_dur) = {
            let mut s = shared.lock();
            let now = Instant::now();

            // Pop expired entries
            let mut fired: Option<BuffHeapItem> = None;
            while let Some(item) = s.heap.peek() {
                if item.expire > now {
                    break;
                }
                let item = s.heap.pop().unwrap();
                if let Some(entry) = s.entries.get(&item.name) {
                    if entry.gen == item.gen {
                        s.entries.remove(&item.name);
                        fired = Some(item);
                        break;
                    }
                }
            }

            let sleep_dur = match s.heap.peek() {
                Some(item) => item
                    .expire
                    .saturating_duration_since(now)
                    .min(Duration::from_millis(100)),
                None => Duration::from_millis(100),
            };

            (fired, sleep_dur)
        };

        if let Some(item) = to_fire {
            match item.action {
                ExpiryAction::FireActionKey { play_sound } => {
                    fire_buff_action_key(&item, handle.as_ref());
                    if play_sound {
                        play_cooldown_ready_sound(&item.name);
                    }
                }
                ExpiryAction::SoundOnly => play_cooldown_ready_sound(&item.name),
            }
        }

        // Sleep until next expiry or timeout
        thread::sleep(sleep_dur);
    }
}

fn fire_buff_action_key(item: &BuffHeapItem, handle: Option<&EngineHandle>) -> bool {
    if item.buff.action_key.is_empty() {
        return false;
    }

    let mut fired = false;
    if let Some(h) = handle {
        h.input.acquire_sending();
        // Fire only while the game is the focused window. The previous
        // implementation fell back to a cached window handle when the game
        // was merely alive, which injected the buff key into whatever the
        // user had switched to.
        if h.detector.is_in_focus() {
            platform::send_key(&item.buff.action_key);
            fired = true;
        } else {
            log::debug!(
                "[buff] {} expired but the game is not focused — key {} dropped",
                item.name,
                item.buff.action_key
            );
        }
        h.input.release_sending();
    } else {
        // No handle — just send directly.
        platform::send_key(&item.buff.action_key);
        fired = true;
    }
    if fired {
        log::info!(
            "[buff] expired: {} → fired {}",
            item.name,
            item.buff.action_key
        );
    }
    fired
}

fn cooldown_sound_path() -> Option<PathBuf> {
    let relative = "sounds/freedesktop/stereo/complete.oga";
    let mut candidates = vec![
        PathBuf::from("/run/current-system/sw/share").join(relative),
        PathBuf::from("/usr/share").join(relative),
    ];
    if let Ok(data_dirs) = std::env::var("XDG_DATA_DIRS") {
        candidates.extend(
            data_dirs
                .split(':')
                .filter(|dir| !dir.is_empty())
                .map(|dir| PathBuf::from(dir).join(relative)),
        );
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn play_cooldown_ready_sound(name: &str) {
    let Some(sound) = cooldown_sound_path() else {
        log::warn!("[buff] cooldown ready for {name}, but no completion sound was found");
        return;
    };
    let player = if std::path::Path::new("/run/current-system/sw/bin/pw-play").is_file() {
        PathBuf::from("/run/current-system/sw/bin/pw-play")
    } else {
        PathBuf::from("pw-play")
    };
    let name = name.to_string();
    let _ = thread::Builder::new()
        .name("buff-reminder-sound".into())
        .spawn(move || match Command::new(player).arg(sound).status() {
            Ok(status) if status.success() => log::info!("[buff] cooldown ready: {name}"),
            Ok(status) => log::warn!("[buff] cooldown sound exited with {status}: {name}"),
            Err(error) => log::warn!("[buff] could not play cooldown sound for {name}: {error}"),
        });
}

#[cfg(test)]
mod tests {
    use super::{BuffEngine, ExpiryAction};
    use crate::config::BuffTimer;

    #[test]
    fn manual_reminder_queues_a_sound_without_pressing_the_action_key() {
        let engine = BuffEngine::new();
        let mut buff = BuffTimer::default();
        buff.name = "Cooldown reminder".into();
        buff.action_key = "f".into();
        buff.duration = 60_000;

        engine.start_reminder(buff);
        let queued = engine
            .shared
            .lock()
            .heap
            .peek()
            .cloned()
            .expect("queued reminder");
        assert_eq!(queued.action, ExpiryAction::SoundOnly);
        engine.stop();
    }

    #[test]
    fn watched_key_buff_can_play_a_sound_when_its_countdown_expires() {
        let engine = BuffEngine::new();
        let mut buff = BuffTimer::default();
        buff.name = "BlueBuff".into();
        buff.action_key = "v".into();
        buff.duration = 60_000;
        buff.sound_on_expiry = true;

        engine.activate(buff);
        let queued = engine
            .shared
            .lock()
            .heap
            .peek()
            .cloned()
            .expect("queued automatic buff");
        assert_eq!(
            queued.action,
            ExpiryAction::FireActionKey { play_sound: true }
        );
        engine.stop();
    }

    #[test]
    fn negative_buff_extension_does_not_wrap_to_an_enormous_duration() {
        let engine = BuffEngine::new();
        let mut buff = BuffTimer::default();
        buff.name = "Refreshable".into();
        buff.duration = 60_000;
        engine.activate(buff.clone());

        buff.on_refresh = "extend".into();
        buff.extend_ms = -1;
        engine.activate(buff);

        assert!(engine.get_active_timers()["Refreshable"] <= 60_000.0);
        engine.stop();
    }
}
