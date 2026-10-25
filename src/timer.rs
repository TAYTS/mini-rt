const NUM_LEVELS: usize = 6;

const BITS_PER_LEVEL: usize = 6;

const SLOTS_PER_LEVEL: usize = 64;

const SLOT_MASK: usize = (1 << BITS_PER_LEVEL) - 1;

const MAX_DURATION: usize = (1 << NUM_LEVELS * BITS_PER_LEVEL) - 1;
