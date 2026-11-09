mod entry;
mod wheel;

use std::sync::{
    Arc, Mutex, OnceLock,
    mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel},
};
use std::time::{Duration, Instant};
use std::{thread, time};

use entry::{Sleep, TimerEntry, TimerShareState};
use wheel::TimerWheel;

/// Anchor start time of the runtime, used to calculate monotonic clock ticks.
static START_TIME: OnceLock<time::Instant> = OnceLock::new();

/// Channel used to pass [TimerEntry] to the background processing thread.
static TIMER_SENDER: OnceLock<SyncSender<TimerEntry>> = OnceLock::new();

/// Converts a [time::Instant] to monotonic clock ticks with millisecond resolution.
fn to_tick(instant: time::Instant) -> u64 {
    let start_time = START_TIME.get_or_init(|| time::Instant::now());
    let tick = instant.saturating_duration_since(*start_time);
    u64::try_from(tick.as_millis()).unwrap_or(u64::MAX)
}

/// Extracts all expired [TimerEntry] instances from the timer wheel and wakes their tasks.
fn try_poll(wheel: &mut TimerWheel) {
    let now = to_tick(Instant::now());

    while let Some(entry) = wheel.poll(now) {
        let waker = {
            let mut state = entry.share_state.lock().unwrap();
            state.fired = true;
            state.waker.take()
        };

        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// Initializes the timer wheel and processes incoming [TimerEntry] messages from the receivers.
fn run_timer_wheel(recv: Receiver<TimerEntry>) {
    let mut wheel = TimerWheel::new();

    loop {
        // Clear all the expired timer in timer wheel before polling new entry from the channel
        try_poll(&mut wheel);

        match wheel.next_expiration_time() {
            Some(expiration_time) => {
                let now = to_tick(Instant::now());
                let timeout = Duration::from_millis(expiration_time.saturating_sub(now));

                // Sleeps up to `timeout`, but wakes up early if a new timer arrives
                match recv.recv_timeout(timeout) {
                    Ok(new_entry) => wheel.insert(new_entry),
                    Err(RecvTimeoutError::Timeout) => {
                        // Move to next iteration to start polling expired timer.
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        // No more timer in the pipeline, terminate
                        return;
                    }
                };
            }
            None => {
                // No pending sleep timer, waiting for new task from channel idefinitely to save CPU cycles.
                let new_entry = recv.recv().expect("failed to get timer");
                wheel.insert(new_entry);
            }
        }
    }
}

pub fn sleep(duration: time::Duration) -> Sleep {
    let sender = TIMER_SENDER.get_or_init(|| {
        const MAX_QUEUE_SIZE: usize = 10_000;
        let (sender, recv) = sync_channel::<TimerEntry>(MAX_QUEUE_SIZE);

        // Spawn background thread, safe as this is only called once
        thread::spawn(move || run_timer_wheel(recv));
        sender
    });

    let share_state = Arc::new(Mutex::new(TimerShareState {
        fired: false,
        waker: None,
    }));
    let deadline = time::Instant::now()
        .checked_add(duration)
        .expect("invalid sleep duration");
    let sleep_fut = Sleep {
        share_state: share_state.clone(),
    };
    let timer_entry = TimerEntry {
        deadline_tick: to_tick(deadline),
        share_state,
    };
    sender.send(timer_entry).expect("failed to queue timer");
    sleep_fut
}
