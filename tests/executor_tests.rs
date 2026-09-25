use mini_rt::{block_on, spawn};
use ntest::timeout;
use std::future::poll_fn;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
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

struct ShareData {
    data: bool,
    waker: Option<Waker>,
}

struct StashWakerFuture {
    share_data: Arc<Mutex<ShareData>>,
}

impl Future for StashWakerFuture {
    type Output = bool;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut data_guard = self.share_data.lock().unwrap();

        if !data_guard.data {
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
    let share_state: Arc<Mutex<ShareData>> = Arc::new(Mutex::new(ShareData {
        data: false,
        waker: None,
    }));
    let cloned = share_state.clone();

    let fut = StashWakerFuture {
        share_data: share_state,
    };
    // Spawn future and let it store the waker
    let join_handle = spawn(fut);

    thread::spawn(move || {
        // Add delay to ensure the waker has set
        thread::sleep(Duration::from_millis(500));

        let mut data = cloned.lock().unwrap();
        let waker = data.waker.take().expect("Waker should have set");
        data.data = true;
        waker.wake_by_ref();
    });

    // Ensure the spawned future has driven to completion
    assert!(block_on(async { join_handle.await }));
}

#[test]
#[timeout(1000)]
fn test_ping_pong() {
    let _ = block_on(async {
        let shared_data: Arc<Mutex<ShareData>> = Arc::new(Mutex::new(ShareData {
            data: false,
            waker: None,
        }));
        let clone1 = shared_data.clone();
        let clone2 = shared_data.clone();

        let output: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::<String>::new()));
        let out_clone1 = output.clone();
        let out_clone2 = output.clone();

        let fut1 = spawn(async move {
            let mut count = 0;
            while count < 2 {
                poll_fn(|cx| {
                    let mut guard = clone1.lock().unwrap();

                    if !guard.data {
                        guard.data = true;
                        let waker = guard.waker.take();

                        drop(guard);

                        if let Some(waker) = waker {
                            waker.wake();
                        }
                        return Poll::Ready(());
                    } else {
                        guard.waker.replace(cx.waker().clone());
                    }

                    Poll::Pending
                })
                .await;
                count += 1;
                let mut guard = out_clone1.lock().unwrap();
                guard.push("ping".to_string());
            }
        });
        let fut2 = spawn(async move {
            let mut count = 0;
            while count < 2 {
                poll_fn(|cx| {
                    let mut guard = clone2.lock().unwrap();

                    if guard.data {
                        guard.data = false;
                        let waker = guard.waker.take();

                        drop(guard);

                        if let Some(waker) = waker {
                            waker.wake();
                        }
                        return Poll::Ready(());
                    } else {
                        guard.waker.replace(cx.waker().clone());
                    }
                    Poll::Pending
                })
                .await;
                count += 1;
                let mut guard = out_clone2.lock().unwrap();
                guard.push("pong".to_string());
            }
        });

        let _ = fut1.await;
        let _ = fut2.await;

        let output_guard = output.lock().unwrap();
        println!("{:?}", output_guard);
        assert_eq!(*output_guard, ["ping", "pong", "ping", "pong"]);
    });
}
