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
fn try_poll(wheel: &mut TimerWheel, now: Option<u64>) {
    let now = now.unwrap_or_else(|| to_tick(Instant::now()));

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

/// Shutdown timer wheel and force wakes up all the [TimerEntry].
fn shutdown(wheel: &mut TimerWheel) {
    try_poll(wheel, Some(u64::MAX));
}

/// Initializes the timer wheel and processes incoming [TimerEntry] messages from the receivers.
fn run_timer_wheel(recv: Receiver<TimerEntry>) {
    let mut wheel = TimerWheel::new();

    loop {
        // Clear all expired timers from the timer wheel before polling from new entries.
        try_poll(&mut wheel, None);

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
                        // Channel disconnected, trigger shutdown and flush remaining timers
                        shutdown(&mut wheel);
                        return;
                    }
                };
            }
            None => {
                // No pending sleep timer, waiting for new task from channel indefinitely to save CPU cycles.
                if let Ok(new_entry) = recv.recv() {
                    wheel.insert(new_entry);
                } else {
                    shutdown(&mut wheel);
                    return;
                }
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

#[cfg(test)]
mod timer_test {
    use std::ops::{Add, Sub};

    use super::*;

    #[test]
    fn to_tick_handle_earlier_instant() {
        let past_instant = Instant::now().sub(Duration::from_secs(60));
        let tick = to_tick(past_instant);

        assert_eq!(tick, 0);
    }

    #[test]
    fn try_poll_process_all_expired_timer() {
        let mut wheel = TimerWheel::new();
        let expired_shared = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: None,
        }));
        let expired_entry = TimerEntry {
            deadline_tick: to_tick(Instant::now().sub(Duration::from_hours(1))),
            share_state: expired_shared.clone(),
        };
        let future_shared = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: None,
        }));
        let future_entry = TimerEntry {
            deadline_tick: to_tick(Instant::now().add(Duration::from_hours(1))),
            share_state: future_shared.clone(),
        };
        wheel.insert(expired_entry);
        wheel.insert(future_entry);

        try_poll(&mut wheel, None);

        assert!(expired_shared.lock().unwrap().fired);
        assert!(!future_shared.lock().unwrap().fired);
    }

    #[test]
    fn shutdown_clear_all_timer() {
        let mut wheel = TimerWheel::new();
        let shared1 = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: None,
        }));
        let entry1 = TimerEntry {
            deadline_tick: to_tick(Instant::now().add(Duration::from_hours(1))),
            share_state: shared1.clone(),
        };
        let shared2 = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: None,
        }));
        let entry2 = TimerEntry {
            deadline_tick: to_tick(Instant::now().add(Duration::from_millis(10))),
            share_state: shared2.clone(),
        };
        wheel.insert(entry1);
        wheel.insert(entry2);

        shutdown(&mut wheel);

        assert!(wheel.poll(to_tick(Instant::now())).is_none());
        assert!(shared1.lock().unwrap().fired);
        assert!(shared2.lock().unwrap().fired);
    }
}
