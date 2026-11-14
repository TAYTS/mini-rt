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

impl Drop for Sleep {
    fn drop(&mut self) {
        let mut state = self.share_state.lock().unwrap();
        state.fired = true;
        state.waker = None;
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

#[cfg(test)]
mod entrytest {
    use super::*;
    use std::task::Waker;

    #[test]
    fn test_drop_sleep() {
        let shared = Arc::new(Mutex::new(TimerShareState {
            fired: false,
            waker: Some(Waker::noop().clone()),
        }));

        let sleep = Sleep {
            share_state: shared.clone(),
        };

        drop(sleep);

        let guard = shared.lock().unwrap();
        assert!(guard.fired);
        assert!(guard.waker.is_none());
    }
}
