use std::array;

use crate::TimerEntry;

const NUM_LEVELS: usize = 6;

const BITS_PER_LEVEL: usize = 6;

const SLOTS_PER_LEVEL: usize = 64;

const SLOT_MASK: u64 = (1 << BITS_PER_LEVEL) - 1;

const MAX_DURATION: u64 = (1 << NUM_LEVELS * BITS_PER_LEVEL) - 1;

/// Determines the timer wheel level for `when` relative to `elapsed`.
///
/// Timers exceeding `MAX_DURATION` (~2.17 years) are clamped to the highest level.
fn level_for(elapsed: u64, when: u64) -> usize {
    let masked = (elapsed ^ when) | SLOT_MASK;

    if masked >= MAX_DURATION {
        return NUM_LEVELS - 1;
    }

    masked.ilog2() as usize / BITS_PER_LEVEL
}

/// Converts a deadline timestamp `when` and a wheel level to its slot index (0..63).
fn slot_for(when: u64, level: usize) -> usize {
    ((when >> (level * BITS_PER_LEVEL)) & SLOT_MASK) as usize
}

/// Returns the time duration (in ticks / ms) represented by a single slot at the given level.
fn slot_range(level: usize) -> u64 {
    1 << (BITS_PER_LEVEL * level)
}

/// Returns the total time span (in ticks / ms) covered by a full rotation of the given level.
fn level_range(level: usize) -> u64 {
    1 << (BITS_PER_LEVEL * (level + 1))
}

/// Represents a single level in the hierarchical timer wheel.
struct Level {
    /// Zero-based index of this level (0..NUM_LEVELS).
    level: usize,
    /// Timer buckets for each of the 64 slots in this level.
    slots: [Vec<TimerEntry>; SLOTS_PER_LEVEL],
    /// Bitmask where bit `i` is 1 if `slots[i]` contains at least one timer.
    occupied: u64,
}

/// Represents a single 64-slot ring in the hierarchical timer wheel.
///
/// Each level covers a distinct time resolution (`slot_range(level)`), tracking
/// non-empty slots with an `occupied` 64-bit mask for fast lookup.
impl Level {
    /// Creates a new [Level] at the given index with empty slots.
    fn new(level: usize) -> Level {
        Level {
            level: level,
            slots: array::from_fn(|_| Vec::new()),
            occupied: 0,
        }
    }

    /// Inserts a [TimerEntry] into its corresponding slot.
    fn insert(&mut self, entry: TimerEntry) {
        let slot_idx = slot_for(entry.deadline_tick, self.level);

        self.slots[slot_idx].push(entry);
        self.occupied |= 1 << slot_idx;
    }

    /// Removes and returns all [TimerEntry] instances stored in the given slot.
    fn remove_slot(&mut self, slot_idx: usize) -> Vec<TimerEntry> {
        if self.slots[slot_idx].is_empty() {
            return Vec::new();
        }
        self.occupied &= !(1 << slot_idx);
        std::mem::take(&mut self.slots[slot_idx])
    }

    /// Finds the next occupied slot index at or after the given time, wrapping around if necessary.
    ///
    /// Returns `None` if this level contains no timers (`occupied == 0`).
    fn next_occupied_slot(&self, now: u64) -> Option<usize> {
        if self.occupied == 0 {
            return None;
        }

        let now_slot = ((now / slot_range(self.level)) % SLOTS_PER_LEVEL as u64) as usize;
        let rotated = self.occupied.rotate_right(now_slot as u32);
        let zeros = rotated.trailing_zeros() as usize;
        let slot = (now_slot + zeros) % SLOTS_PER_LEVEL;

        Some(slot)
    }
}

/// Represents the hierarchical timer wheel for the runtime.
///
/// Manages timer placement across multiple levels and handles cascading [TimerEntry] instances
/// as time advances.
struct TimerWheel {
    /// Current monotonic clock tick (in milliseconds).
    elapsed: u64,
    /// Hierarchical wheel levels, where each higher level covers a coarser time span.
    levels: [Level; NUM_LEVELS],
    /// Timers that have expired and are ready to be processed or woken.
    pending: Vec<TimerEntry>,
}

impl TimerWheel {
    /// Creates a new empty [TimerWheel] with all levels initialized and `elapsed` set to 0.
    fn new() -> TimerWheel {
        return TimerWheel {
            elapsed: 0,
            levels: array::from_fn(|i| Level::new(i)),
            pending: Vec::new(),
        };
    }

    /// Inserts a [TimerEntry] into the wheel hierarchy.
    ///
    /// If the entry's deadline has already arrived or passed relative to `elapsed`,
    /// it is pushed directly to the `pending` queue so it can be fired immediately.
    fn insert(&mut self, entry: TimerEntry) {
        if entry.deadline_tick <= self.elapsed {
            self.pending.push(entry);
            return;
        }

        let level = level_for(self.elapsed, entry.deadline_tick);
        self.levels[level].insert(entry);
    }

    /// Retrieves the next [Expiration] across all wheel levels, or `None` if the wheel is empty.
    fn next_expiration(&self) -> Option<Expiration> {
        if !self.pending.is_empty() {
            return Some(Expiration {
                level: 0,
                slot: 0,
                deadline: self.elapsed,
            });
        }

        let now = self.elapsed;
        for i in 0..NUM_LEVELS {
            if let Some(slot) = self.levels[i].next_occupied_slot(now) {
                let level_range = level_range(i);
                let slot_range = slot_range(i);

                // Mask out the low bits to compute the start time on current level for `now`.
                let level_start = now & !(level_range - 1);
                let mut deadline = level_start + slot as u64 * slot_range;

                if deadline <= now {
                    // This only happened for the top level where it serves as pseudo buffer
                    // for entries that exceed max timer wheel duration. Will simply bump
                    // the deadline one more period until it reaches it true deadline.
                    deadline += level_range;
                }
                return Some(Expiration {
                    level: i,
                    slot: slot,
                    deadline: deadline,
                });
            }
        }
        None
    }
}

/// Represents a snapshot of [TimerWheel] expiration.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
struct Expiration {
    /// The wheel level containing the expiring slot (0..NUM_LEVELS).
    level: usize,
    /// The slot index within the level (0..SLOTS_PER_LEVEL).
    slot: usize,
    /// Monotonic timestamp (in ticks / ms) when this slot must be processed.
    deadline: u64,
}

#[cfg(test)]
mod timer_test {
    use std::{
        sync::{Arc, Mutex},
        time::Instant,
    };

    use crate::TimerShareState;

    use super::*;

    #[test]
    fn level_for_base_case() {
        let elapsed: u64 = 0;
        let when: u64 = 0;

        assert_eq!(level_for(elapsed, when), 0);
    }

    #[test]
    fn level_for_level_threshold() {
        for i in 0..NUM_LEVELS {
            let elapsed: u64 = 0;
            let when: u64 = 1 << (BITS_PER_LEVEL * i);

            assert_eq!(level_for(elapsed, when), i);
        }
    }

    #[test]
    fn level_for_max_duration() {
        let elapsed: u64 = 0;
        let when: u64 = (1 << (BITS_PER_LEVEL * 6)) - 1;

        assert_eq!(level_for(elapsed, when), 5);

        let elapsed: u64 = 0;
        let when: u64 = 1 << (BITS_PER_LEVEL * 6);

        assert_eq!(level_for(elapsed, when), 5);

        let elapsed: u64 = 0;
        let when: u64 = u64::MAX;

        assert_eq!(level_for(elapsed, when), 5);
    }

    #[test]
    fn level_for_slot_boundary() {
        let elapsed: u64 = 63;
        let when: u64 = 64;

        assert_eq!(level_for(elapsed, when), 1);

        let elapsed: u64 = 0;
        let when: u64 = 63;

        assert_eq!(level_for(elapsed, when), 0);

        let elapsed: u64 = 0;
        let when: u64 = 64;

        assert_eq!(level_for(elapsed, when), 1);
    }

    #[test]
    fn level_for_symmetry() {
        let elapsed: u64 = 63;
        let when: u64 = 64;

        assert_eq!(level_for(elapsed, when), 1);
        assert_eq!(level_for(when, elapsed), 1);
    }

    #[test]
    fn slot_for_base() {
        let when = 0;
        let level = 0;

        assert_eq!(slot_for(when, level), 0);
    }

    #[test]
    fn slot_for_different_level() {
        for i in 0..NUM_LEVELS {
            let when = 1 << (BITS_PER_LEVEL * i);
            let level = i;

            assert_eq!(slot_for(when, level), 1);
        }

        // upper bound for each level
        for i in 0..NUM_LEVELS {
            let when = 63 << (BITS_PER_LEVEL * i);
            let level = i;

            assert_eq!(slot_for(when, level), 63);
        }
    }

    #[test]
    fn slot_for_exceed_max_level() {
        let when = 1 << (BITS_PER_LEVEL * 6);
        let level = 5;

        assert_eq!(slot_for(when, level), 0);
    }

    #[test]
    fn slot_for_level_isolation() {
        // lower bits (bits 0..5) are discarded by shift
        assert_eq!(slot_for(64 + 63, 1), 1);

        // higher bits (bits 6+) are discarded by mask
        assert_eq!(slot_for(64 + 5, 0), 5);
    }

    #[test]
    fn slot_range_different_level() {
        let slot_res: [u64; NUM_LEVELS] = [1, 64, 4096, 262_144, 16_777_216, 1_073_741_824];
        for res in slot_res.iter().enumerate() {
            assert_eq!(slot_range(res.0), *res.1);
        }
    }

    #[test]
    fn level_range_different_level() {
        let level_res: [u64; NUM_LEVELS] =
            [64, 4096, 262_144, 16_777_216, 1_073_741_824, 68_719_476_736];
        for res in level_res.iter().enumerate() {
            assert_eq!(level_range(res.0), *res.1);
        }
    }

    #[test]
    fn level_insert_boundary() {
        let shared = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: None,
        }));

        for i in 0..NUM_LEVELS {
            let mut level = Level::new(i);
            let first_entry = TimerEntry {
                deadline: Instant::now(),
                deadline_tick: 0,
                share_state: shared.clone(),
            };
            let last_entry = TimerEntry {
                deadline: Instant::now(),
                deadline_tick: 63 * slot_range(i),
                share_state: shared.clone(),
            };

            level.insert(first_entry);
            level.insert(last_entry);

            assert_eq!(level.occupied, 1 | 1 << (SLOTS_PER_LEVEL - 1));
            assert_eq!(level.slots[0].len(), 1);
            assert_eq!(level.slots[SLOTS_PER_LEVEL - 1].len(), 1);
        }
    }

    #[test]
    fn level_insert_wrap_around() {
        let shared = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: None,
        }));

        let mut level = Level::new(0);
        let entry = TimerEntry {
            deadline: Instant::now(),
            deadline_tick: 64 * slot_range(0),
            share_state: shared.clone(),
        };

        level.insert(entry);

        assert_eq!(level.occupied, 1);
        assert_eq!(level.slots[0].len(), 1);
    }

    #[test]
    fn level_insert_same_slot() {
        let shared = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: None,
        }));

        let mut level = Level::new(0);
        let first_entry = TimerEntry {
            deadline: Instant::now(),
            deadline_tick: 1 * slot_range(0),
            share_state: shared.clone(),
        };
        let second_entry = TimerEntry {
            deadline: Instant::now(),
            deadline_tick: 1 * slot_range(0),
            share_state: shared.clone(),
        };

        level.insert(first_entry);
        level.insert(second_entry);

        assert_eq!(level.occupied, 1 << 1);
        assert_eq!(level.slots[1].len(), 2);
    }

    #[test]
    fn level_insert_multiple() {
        let shared = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: None,
        }));

        let mut level = Level::new(0);
        let first_entry = TimerEntry {
            deadline: Instant::now(),
            deadline_tick: 1 * slot_range(0),
            share_state: shared.clone(),
        };
        let second_entry = TimerEntry {
            deadline: Instant::now(),
            deadline_tick: 5 * slot_range(0),
            share_state: shared.clone(),
        };

        level.insert(first_entry);
        level.insert(second_entry);

        assert_eq!(level.occupied, 1 << 1 | 1 << 5);
        assert_eq!(level.slots[1].len(), 1);
        assert_eq!(level.slots[5].len(), 1);
    }

    #[test]
    fn level_remove_slot() {
        let shared = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: None,
        }));

        let mut level = Level::new(0);
        let first_entry = TimerEntry {
            deadline: Instant::now(),
            deadline_tick: 1 * slot_range(0),
            share_state: shared.clone(),
        };
        let second_entry = TimerEntry {
            deadline: Instant::now(),
            deadline_tick: 1 * slot_range(0),
            share_state: shared.clone(),
        };
        let third_entry = TimerEntry {
            deadline: Instant::now(),
            deadline_tick: 63 * slot_range(0),
            share_state: shared.clone(),
        };

        level.insert(first_entry);
        level.insert(second_entry);
        level.insert(third_entry);

        assert_eq!(level.remove_slot(0).len(), 0);
        assert_eq!(level.remove_slot(1).len(), 2);
        assert_eq!(level.occupied, 1 << 63);
        assert_eq!(level.remove_slot(63).len(), 1);
        assert_eq!(level.occupied, 0);
    }

    #[test]
    fn next_occupied_slot_none_for_empty_slot() {
        let level = Level::new(0);

        assert!(level.next_occupied_slot(10).is_none());
    }

    #[test]
    fn next_occupied_slot_different_level() {
        let shared = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: None,
        }));

        for i in 0..NUM_LEVELS {
            let mut level = Level::new(i);
            let now = SLOTS_PER_LEVEL as u64 / 2 * slot_range(i);
            let past_entry = TimerEntry {
                deadline: Instant::now(),
                deadline_tick: 1 * slot_range(i),
                share_state: shared.clone(),
            };
            let future_entry = TimerEntry {
                deadline: Instant::now(),
                deadline_tick: (SLOTS_PER_LEVEL as u64 - 1) * slot_range(i),
                share_state: shared.clone(),
            };
            level.insert(past_entry);
            level.insert(future_entry);

            assert_eq!(
                level.next_occupied_slot(now),
                Some((SLOTS_PER_LEVEL as u64 - 1) as usize)
            );
        }
    }

    #[test]
    fn next_occupied_slot_wrap_around() {
        let shared = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: None,
        }));

        let mut level = Level::new(0);
        let now = 5 * slot_range(0);
        let entry = TimerEntry {
            deadline: Instant::now(),
            deadline_tick: 1 * slot_range(0),
            share_state: shared.clone(),
        };
        level.insert(entry);

        assert_eq!(level.next_occupied_slot(now), Some(1));
    }

    #[test]
    fn next_occupied_slot_immediate() {
        let shared = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: None,
        }));

        let mut level = Level::new(0);
        let now = 5 * slot_range(0);
        let entry = TimerEntry {
            deadline: Instant::now(),
            deadline_tick: 5 * slot_range(0),
            share_state: shared.clone(),
        };
        level.insert(entry);

        assert_eq!(level.next_occupied_slot(now), Some(5));
    }

    #[test]
    fn timer_wheel_insert() {
        let shared = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: None,
        }));
        let mut wheel = TimerWheel::new();
        for i in 0..NUM_LEVELS {
            let entry = TimerEntry {
                deadline: Instant::now(),
                deadline_tick: 5 * slot_range(i),
                share_state: shared.clone(),
            };
            wheel.insert(entry);

            assert_eq!(wheel.levels[i].level, i);
            assert_eq!(wheel.levels[i].occupied, 1 << 5);
            assert_eq!(wheel.levels[i].slots[5].len(), 1);
        }
    }

    #[test]
    fn timer_wheel_insert_arrived_or_passed_entry() {
        let shared = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: None,
        }));

        let mut wheel = TimerWheel::new();
        wheel.elapsed = 10;

        let arrived_entry = TimerEntry {
            deadline: Instant::now(),
            deadline_tick: 10 * slot_range(0),
            share_state: shared.clone(),
        };
        let passed_entry = TimerEntry {
            deadline: Instant::now(),
            deadline_tick: 5 * slot_range(0),
            share_state: shared.clone(),
        };

        wheel.insert(arrived_entry);
        wheel.insert(passed_entry);

        assert_eq!(wheel.pending.len(), 2);
        assert_eq!(wheel.levels[0].occupied, 0);
    }

    #[test]
    fn timer_wheel_insert_far_entry() {
        let shared = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: None,
        }));

        let mut wheel = TimerWheel::new();

        let far_entry = TimerEntry {
            deadline: Instant::now(),
            deadline_tick: (SLOTS_PER_LEVEL as u64 + 5) * slot_range(NUM_LEVELS - 1),
            share_state: shared.clone(),
        };

        wheel.insert(far_entry);

        assert_eq!(wheel.pending.len(), 0);
        assert_eq!(wheel.levels[NUM_LEVELS - 1].occupied, 1 << 5);
        assert_eq!(wheel.levels[NUM_LEVELS - 1].slots[5].len(), 1);
    }

    #[test]
    fn timer_wheel_next_expiration_none_for_empty_wheel() {
        let wheel = TimerWheel::new();

        assert!(wheel.next_expiration().is_none());
    }

    #[test]
    fn timer_wheel_next_expiration_return_closest_expiration_across_levels() {
        let shared = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: None,
        }));

        let mut wheel = TimerWheel::new();
        wheel.elapsed = 5;

        // Backward insert entries
        for i in (0..NUM_LEVELS).rev() {
            let entry = TimerEntry {
                deadline: Instant::now(),
                deadline_tick: 10 * slot_range(i),
                share_state: shared.clone(),
            };
            wheel.insert(entry);

            assert!(wheel.next_expiration().is_some());
            let expiration = wheel.next_expiration().unwrap();
            assert_eq!(expiration.level, i);
            assert_eq!(expiration.slot, 10);
            assert_eq!(expiration.deadline, 10 * slot_range(i));
        }

        let mut wheel = TimerWheel::new();
        wheel.elapsed = 5;

        // Forward insert entries
        for i in 0..NUM_LEVELS {
            let entry = TimerEntry {
                deadline: Instant::now(),
                deadline_tick: 10 * slot_range(i),
                share_state: shared.clone(),
            };
            wheel.insert(entry);

            assert!(wheel.next_expiration().is_some());
            let expiration = wheel.next_expiration().unwrap();
            assert_eq!(expiration.level, 0);
            assert_eq!(expiration.slot, 10);
            assert_eq!(expiration.deadline, 10 * slot_range(0));
        }
    }

    #[test]
    fn timer_wheel_next_expiration_return_closest_expiration_within_level() {
        let shared = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: None,
        }));

        let mut wheel = TimerWheel::new();
        wheel.elapsed = 5;
        let first_entry = TimerEntry {
            deadline: Instant::now(),
            deadline_tick: 10,
            share_state: shared.clone(),
        };
        let second_entry = TimerEntry {
            deadline: Instant::now(),
            deadline_tick: 20,
            share_state: shared.clone(),
        };
        wheel.insert(first_entry);
        wheel.insert(second_entry);

        assert!(wheel.next_expiration().is_some());
        let expiration = wheel.next_expiration().unwrap();
        assert_eq!(expiration.level, 0);
        assert_eq!(expiration.slot, 10);
        assert_eq!(expiration.deadline, 10);
    }

    #[test]
    fn timer_wheel_next_expiration_return_pending_entry() {
        let shared = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: None,
        }));

        let mut wheel = TimerWheel::new();
        wheel.elapsed = 10;
        let pending_entry = TimerEntry {
            deadline: Instant::now(),
            deadline_tick: 10,
            share_state: shared.clone(),
        };
        wheel.insert(pending_entry);

        assert!(wheel.next_expiration().is_some());
        let expiration = wheel.next_expiration().unwrap();
        assert_eq!(expiration.level, 0);
        assert_eq!(expiration.slot, 0);
        assert_eq!(expiration.deadline, 10);
    }

    #[test]
    fn timer_wheel_next_expiration_wrap_around() {
        let shared = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: None,
        }));

        let mut wheel = TimerWheel::new();
        wheel.elapsed = 10;
        let entry = TimerEntry {
            deadline: Instant::now(),
            deadline_tick: 64 * slot_range(NUM_LEVELS - 1),
            share_state: shared.clone(),
        };
        wheel.insert(entry);

        assert!(wheel.next_expiration().is_some());
        let expiration = wheel.next_expiration().unwrap();
        assert_eq!(expiration.level, NUM_LEVELS - 1);
        assert_eq!(expiration.slot, 0);
        assert_eq!(expiration.deadline, 64 * slot_range(NUM_LEVELS - 1));
    }
}
