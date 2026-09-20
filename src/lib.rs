use std::{
    cell::RefCell,
    collections::VecDeque,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
    thread::Thread,
};

thread_local! {
    static RUN_QUEUE: RefCell<RunQueue> = RefCell::new(RunQueue::new());
}

unsafe fn clone(data: *const ()) -> RawWaker {
    unsafe { Arc::increment_strong_count(data as *const Task) };
    RawWaker::new(data, &VTABLE)
}

unsafe fn wake(data: *const ()) {
    let arc_data = unsafe { Arc::from_raw(data as *const Task) };
    let cloned = arc_data.clone();
    arc_data.task_sender.push(cloned);
}

unsafe fn wake_by_ref(data: *const ()) {
    let arc_data = unsafe { Arc::from_raw(data as *const Task) };
    let cloned = arc_data.clone();
    arc_data.task_sender.push(cloned);
    std::mem::forget(arc_data);
}

unsafe fn _drop(data: *const ()) {
    unsafe { Arc::decrement_strong_count(data as *const Task) };
}

static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, _drop);

fn get_raw_waker(data: *const ()) -> RawWaker {
    RawWaker::new(data, &VTABLE)
}

pub struct Task {
    pub fut: Mutex<Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>>,
    pub task_sender: RunQueue,
}

#[derive(Clone)]
pub struct RunQueue {
    queue: Arc<Mutex<VecDeque<Arc<Task>>>>,
    executor_thread: Thread,
}

impl RunQueue {
    pub fn new() -> Self {
        let queue = Arc::new(Mutex::new(VecDeque::<Arc<Task>>::with_capacity(1000)));
        RunQueue {
            queue,
            executor_thread: std::thread::current(),
        }
    }

    pub fn pop(&self) -> Option<Arc<Task>> {
        if let Ok(mut queue) = self.queue.lock() {
            return queue.pop_front();
        }
        None
    }

    pub fn push(&self, task: Arc<Task>) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.push_back(task);
        }
        // Unconditionally wake up the executor thread when there is new task
        self.executor_thread.unpark();
    }
}

pub struct ShareState<T> {
    pub result: Option<T>,
    pub waker: Option<Waker>,
}

pub struct JoinHandle<T> {
    shared_state: Arc<Mutex<ShareState<T>>>,
}

impl<T: Send + 'static> Future for JoinHandle<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut shared_state = self.shared_state.lock().unwrap();

        if shared_state.result.is_some() {
            let result = shared_state.result.take().unwrap();
            return Poll::Ready(result);
        }
        match &shared_state.waker {
            Some(old_waker) if old_waker.will_wake(cx.waker()) => {}
            _ => {
                shared_state.waker.replace(cx.waker().clone());
            }
        }
        Poll::Pending
    }
}

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
        shared_state: shared_state,
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
