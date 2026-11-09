use std::{
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
};

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
    pub(crate) deadline_tick: u64,
    pub(crate) share_state: Arc<Mutex<TimerShareState>>,
}

pub struct TimerShareState {
    // Indicate if the timer has fired
    pub(crate) fired: bool,
    pub(crate) waker: Option<Waker>,
}
