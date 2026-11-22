use std::{
    sync::{Arc, Mutex},
    task::{Context, Waker},
};

use crate::task::{JoinHandle, RUN_QUEUE, ShareState, Task, get_raw_waker};

pub fn spawn<F>(fut: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send,
{
    let shared_state = Arc::new(Mutex::new(ShareState {
        result: None,
        waker: None,
    }));
    let cloned = shared_state.clone();

    RUN_QUEUE.with_borrow(|run_queue| {
        let boxed_fut = Box::pin(async move {
            let result = fut.await;
            let extracted_waker = {
                let mut cloned_guard = cloned.lock().unwrap();
                cloned_guard.result.replace(result);
                cloned_guard.waker.take()
            };
            if let Some(waker) = extracted_waker {
                waker.wake_by_ref();
            }
        });
        let task = Task {
            fut: Mutex::new(Some(boxed_fut)),
            task_sender: run_queue.clone(),
        };
        run_queue.push(Arc::new(task));
    });
    JoinHandle::<F::Output> {
        share_state: shared_state,
    }
}

pub fn block_on<F>(fut: F) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send,
{
    RUN_QUEUE.with_borrow(|run_queue| {
        let result_slot: Arc<Mutex<Option<F::Output>>> = Arc::new(Mutex::new(None));
        let cloned = result_slot.clone();

        let task = Task {
            fut: Mutex::new(Some(Box::pin(async move {
                let result = fut.await;
                cloned.lock().unwrap().replace(result);
            }))),
            task_sender: run_queue.clone(),
        };
        run_queue.push(Arc::new(task));

        loop {
            while let Some(task) = run_queue.pop() {
                let extracted_fut = { task.fut.lock().unwrap().take() };

                if let Some(mut fut) = extracted_fut {
                    let data_ptr = Arc::into_raw(task.clone());
                    let raw_waker = get_raw_waker(data_ptr as *const ());
                    let waker = unsafe { Waker::from_raw(raw_waker) };
                    let cx = &mut Context::from_waker(&waker);
                    if fut.as_mut().poll(cx).is_pending() {
                        task.fut.lock().unwrap().replace(fut);
                    }
                }
            }
            if let Some(result) = result_slot.lock().expect("failed to acquire result").take() {
                return result;
            }
            std::thread::park();
        }
    })
}
