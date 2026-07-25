//! Buff Engine — priority queue of buff timers that fire action keys on expiry.
//!
//! Uses a min-heap of expiry times. A background worker thread wakes up at
//! the next expiry, fires the action key, and removes the entry.

use crate::config::BuffTimer;
use crate::engine::macro_engine::EngineHandle;
use crate::platform;
use parking_lot::Mutex;
use std::collections::{BinaryHeap, HashMap};
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

#[derive(Clone)]
struct BuffHeapItem {
    expire: Instant,
    gen: u64,
    name: String,
    buff: BuffTimer,
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

    /// Activate or refresh a buff timer.
    pub fn activate(&self, buff: BuffTimer) {
        let name = buff.name.clone();
        let new_duration = Duration::from_millis(buff.duration.max(0) as u64);

        let mut shared = self.shared.lock();
        let gen = self.gen.fetch_add(1, Ordering::AcqRel);
        let now = Instant::now();

        // Check for existing entry
        if let Some(existing) = shared.entries.get(&name) {
            let on_refresh = buff.on_refresh.as_str();
            if on_refresh == "ignore" {
                return;
            }
            let final_duration = if on_refresh == "extend" {
                let elapsed = existing.start.elapsed();
                let remaining = existing.duration.saturating_sub(elapsed);
                remaining + Duration::from_millis(buff.extend_ms as u64)
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

        // Fire the expired buff's action key
        if let Some(item) = to_fire {
            if !item.buff.action_key.is_empty() {
                if let Some(ref h) = handle {
                    h.input.acquire_sending();
                    // Buffs always fire — use uinput if game is foreground, global injection
                    // if game is alive but backgrounded.
                    let game_pid = h.detector.game_pid();
                    let game_hwnd = if game_pid != 0 {
                        h.detector.get_cached_hwnd()
                    } else {
                        platform::INVALID_WINDOW_HANDLE
                    };

                    let game_foreground = is_game_foreground(game_pid);
                    if game_foreground {
                        platform::send_key(&item.buff.action_key);
                    } else if !game_hwnd.is_null() {
                        platform::send_key_to_window(game_hwnd, &item.buff.action_key);
                    }
                    h.input.release_sending();
                } else {
                    // No handle — just send directly
                    platform::send_key(&item.buff.action_key);
                }
            }
            log::info!(
                "[buff] expired: {} → fired {}",
                item.name,
                item.buff.action_key
            );
        }

        // Sleep until next expiry or timeout
        thread::sleep(sleep_dur);
    }
}

fn is_game_foreground(game_pid: u32) -> bool {
    if game_pid == 0 {
        return false;
    }
    let fg = platform::get_foreground_window();
    let (_, fg_pid) = platform::get_window_thread_process_id(fg);
    fg_pid == game_pid
}
