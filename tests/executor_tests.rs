use mini_rt::{ShareState, block_on, spawn};
use ntest::timeout;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::thread;
use std::time::Duration;

struct NoopFuture;

impl Future for NoopFuture {
    type Output = bool;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Ready(true)
    }
}

struct CounterFuture {
    count: i32,
}

impl Future for CounterFuture {
    type Output = bool;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.count > 0 {
            self.count -= 1;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        Poll::Ready(true)
    }
}

struct StashWakerFuture {
    shared_data: Arc<Mutex<ShareState<bool>>>,
}

impl Future for StashWakerFuture {
    type Output = bool;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut data_guard = self.shared_data.lock().unwrap();

        if !data_guard.result.unwrap_or_default() {
            match data_guard.waker.as_ref() {
                Some(old_waker) if old_waker.will_wake(cx.waker()) => {}
                _ => {
                    data_guard.waker.replace(cx.waker().clone());
                }
            };
            return Poll::Pending;
        }

        Poll::Ready(true)
    }
}

#[test]
#[timeout(1000)]
fn test_immediate_return_future() {
    let fut = NoopFuture;

    assert!(block_on(fut));
}

#[test]
#[timeout(1000)]
fn test_yield_n_times() {
    let fut = CounterFuture { count: 5 };
    assert!(block_on(fut));
}

#[test]
#[timeout(1000)]
fn test_wake_from_other_thread() {
    let share_state: Arc<Mutex<ShareState<bool>>> = Arc::new(Mutex::new(ShareState {
        result: None,
        waker: None,
    }));
    let cloned = share_state.clone();

    let fut = StashWakerFuture {
        shared_data: share_state,
    };
    // Spawn future and let it store the waker
    let join_handle = spawn(fut);

    thread::spawn(move || {
        // Add delay to ensure the waker has set
        thread::sleep(Duration::from_millis(500));

        let mut data = cloned.lock().unwrap();
        let waker = data.waker.take().expect("Waker should have set");
        data.result.replace(true);
        waker.wake_by_ref();
    });

    // Ensure the spawned future has driven to completion
    assert!(block_on(async { join_handle.await }));
}
