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
            let when: u64 = 1 << (6 * i);

            assert_eq!(level_for(elapsed, when), i);
        }
    }

    #[test]
    fn level_for_max_duration() {
        let elapsed: u64 = 0;
        let when: u64 = (1 << (6 * 6)) - 1;

        assert_eq!(level_for(elapsed, when), 5);

        let elapsed: u64 = 0;
        let when: u64 = 1 << (6 * 6);

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
}
