use std::future::poll_fn;
use std::sync::Arc;
use std::sync::Mutex;
use std::task::Poll;

use mini_rt::{ShareState, block_on, spawn};

fn main() {
    let _ = block_on(async {
        let shared_data: Arc<Mutex<ShareState<String>>> = Arc::new(Mutex::new(ShareState {
            result: None,
            waker: None,
        }));
        let clone1 = shared_data.clone();
        let clone2 = shared_data.clone();

        let fut1 = spawn(async move {
            let mut count = 0;
            while count < 5 {
                poll_fn(|cx| {
                    let mut guard = clone1.lock().unwrap();

                    if guard.result.as_deref().is_some_and(|res| res == "pong")
                        || guard.result.as_deref().is_none()
                    {
                        guard.result.replace("ping".to_string());
                        if let Some(waker) = guard.waker.take() {
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
                println!("ping");
            }
        });
        let fut2 = spawn(async move {
            let mut count = 0;
            while count < 5 {
                poll_fn(|cx| {
                    let mut guard = clone2.lock().unwrap();

                    if guard.result.as_deref().is_some_and(|res| res == "ping")
                        || guard.result.as_deref().is_none()
                    {
                        guard.result.replace("pong".to_string());

                        if let Some(waker) = guard.waker.take() {
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
                println!("pong");
            }
        });

        let _ = fut1.await;
        let _ = fut2.await;
    });

    println!("done")
}
