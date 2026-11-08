use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time;

pub struct Sleep {
    pub(crate) share_state: Arc<Mutex<TimerShareState>>,
}

impl Future for Sleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut share_state = self.share_state.lock().expect("failed to get share state");
        if share_state.fired {
            return Poll::Ready(());
        }

        match share_state.waker.as_ref() {
            Some(old_waker) if old_waker.will_wake(cx.waker()) => {}
            _ => {
                share_state.waker.replace(cx.waker().clone());
            }
        }
        Poll::Pending
    }
}

pub struct TimerEntry {
    // Moment when the timer should fire
    pub(crate) deadline: time::Instant,
    pub(crate) deadline_tick: u64,
    pub(crate) share_state: Arc<Mutex<TimerShareState>>,
}

impl Ord for TimerEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.deadline.cmp(&other.deadline)
    }
}

impl Eq for TimerEntry {}

impl PartialOrd for TimerEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        let ordering = self.deadline.partial_cmp(&other.deadline);
        if ordering.is_some_and(|ord| ord.is_ne()) {
            return ordering;
        }
        self.deadline_tick.partial_cmp(&other.deadline_tick)
    }
}

impl PartialEq for TimerEntry {
    fn eq(&self, other: &Self) -> bool {
        self.deadline.eq(&other.deadline) && self.deadline_tick.eq(&other.deadline_tick)
    }
}

pub struct TimerShareState {
    // Indicate if the timer has fired
    pub(crate) fired: bool,
    pub(crate) waker: Option<Waker>,
}
