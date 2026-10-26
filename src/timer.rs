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

#[cfg(test)]
mod timer_test {
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
}
