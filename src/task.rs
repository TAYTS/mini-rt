use std::{
    cell::RefCell,
    collections::VecDeque,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
    thread::Thread,
};

thread_local! {
    pub(crate) static RUN_QUEUE: RefCell<RunQueue> = RefCell::new(RunQueue::new());
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

pub(crate) fn get_raw_waker(data: *const ()) -> RawWaker {
    RawWaker::new(data, &VTABLE)
}

#[derive(Clone)]
pub(crate) struct RunQueue {
    queue: Arc<Mutex<VecDeque<Arc<Task>>>>,
    executor_thread: Thread,
}

impl RunQueue {
    pub(crate) fn new() -> Self {
        let queue = Arc::new(Mutex::new(VecDeque::<Arc<Task>>::with_capacity(1000)));
        RunQueue {
            queue,
            executor_thread: std::thread::current(),
        }
    }

    pub(crate) fn pop(&self) -> Option<Arc<Task>> {
        if let Ok(mut queue) = self.queue.lock() {
            return queue.pop_front();
        }
        None
    }

    pub(crate) fn push(&self, task: Arc<Task>) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.push_back(task);
        }
        // Unconditionally wake up the executor thread when there is new task
        self.executor_thread.unpark();
    }
}

impl Default for RunQueue {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) struct ShareState<T> {
    pub(crate) result: Option<T>,
    pub(crate) waker: Option<Waker>,
}

pub struct JoinHandle<T> {
    pub(crate) share_state: Arc<Mutex<ShareState<T>>>,
}

impl<T: Send + 'static> Future for JoinHandle<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut shared_state = self.share_state.lock().unwrap();

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

pub(crate) struct Task {
    pub(crate) fut: Mutex<Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>>,
    pub(crate) task_sender: RunQueue,
}
