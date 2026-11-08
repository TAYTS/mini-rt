mod entry;
mod wheel;

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::{
    Arc, Mutex, OnceLock,
    mpsc::{RecvTimeoutError, SyncSender, sync_channel},
};
use std::{thread, time};

use entry::{Sleep, TimerEntry, TimerShareState};

static TIMER_SENDER: OnceLock<SyncSender<TimerEntry>> = OnceLock::new();

pub fn sleep(duration: time::Duration) -> Sleep {
    let sender = TIMER_SENDER.get_or_init(|| {
        const MAX_QUEUE_SIZE: usize = 10_000;
        let (sender, recv) = sync_channel::<TimerEntry>(MAX_QUEUE_SIZE);

        // Spawn background thread, safe as this is only called once
        thread::spawn(move || {
            // Min heap based on the timer entry deadline
            let mut timer_heap: BinaryHeap<Reverse<TimerEntry>> = BinaryHeap::new();

            loop {
                // No pending sleep timer, waiting for new task from channel
                if timer_heap.is_empty() {
                    let new_entry = recv.recv().expect("failed to get timer");
                    timer_heap.push(Reverse(new_entry));
                }

                let head_timer = &timer_heap.peek().unwrap().0;
                // Wait until the head timer is completed else terminate with new timer
                match recv.recv_timeout(head_timer.deadline - time::Instant::now()) {
                    Ok(new_entry) => {
                        timer_heap.push(Reverse(new_entry));
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        // timer completed, trigger waker stored in the timer
                        let completed_timer = timer_heap.pop().unwrap().0;
                        let waker = {
                            let mut share_state = completed_timer.share_state.lock().unwrap();
                            share_state.fired = true;
                            share_state.waker.take()
                        };
                        if let Some(waker) = waker {
                            waker.wake();
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        // No more timer in the pipeline, terminate
                        return;
                    }
                }
            }
        });
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
        deadline,
        deadline_tick: 0,
        share_state,
    };
    sender.send(timer_entry).expect("failed to queue timer");
    sleep_fut
}
