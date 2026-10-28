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
        let slot_idx = slot_for(entry.when, self.level);

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
    fn level_insert_boundary() {
        let shared = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: None,
        }));

        for i in 0..NUM_LEVELS {
            let mut level = Level::new(i);
            let first_entry = TimerEntry {
                deadline: Instant::now(),
                when: 0,
                share_state: shared.clone(),
            };
            let last_entry = TimerEntry {
                deadline: Instant::now(),
                when: 63 * slot_range(i),
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
            when: 64 * slot_range(0),
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
            when: 1 * slot_range(0),
            share_state: shared.clone(),
        };
        let second_entry = TimerEntry {
            deadline: Instant::now(),
            when: 1 * slot_range(0),
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
            when: 1 * slot_range(0),
            share_state: shared.clone(),
        };
        let second_entry = TimerEntry {
            deadline: Instant::now(),
            when: 5 * slot_range(0),
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
            when: 1 * slot_range(0),
            share_state: shared.clone(),
        };
        let second_entry = TimerEntry {
            deadline: Instant::now(),
            when: 1 * slot_range(0),
            share_state: shared.clone(),
        };
        let third_entry = TimerEntry {
            deadline: Instant::now(),
            when: 63 * slot_range(0),
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
}
