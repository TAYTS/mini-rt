use std::{
    cell::RefCell,
    cmp::Reverse,
    collections::{BinaryHeap, VecDeque},
    pin::Pin,
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{RecvTimeoutError, SyncSender, sync_channel},
    },
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
    thread::{self, Thread},
    time,
};

thread_local! {
    static RUN_QUEUE: RefCell<RunQueue> = RefCell::new(RunQueue::new());
}

static TIMER_SENDER: OnceLock<SyncSender<TimerEntry>> = OnceLock::new();

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

struct ShareState<T> {
    result: Option<T>,
    waker: Option<Waker>,
}

pub struct JoinHandle<T> {
    share_state: Arc<Mutex<ShareState<T>>>,
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

struct TimerShareState {
    // Indicate if the timer has fired
    fired: bool,
    waker: Option<Waker>,
}

pub struct Sleep {
    share_state: Arc<Mutex<TimerShareState>>,
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

struct TimerEntry {
    // Moment when the timer should fire
    deadline: time::Instant,
    share_state: Arc<Mutex<TimerShareState>>,
}

impl Ord for TimerEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.deadline.cmp(&other.deadline)
    }
}

impl Eq for TimerEntry {}

impl PartialOrd for TimerEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.deadline.partial_cmp(&other.deadline)
    }
}

impl PartialEq for TimerEntry {
    fn eq(&self, other: &Self) -> bool {
        self.deadline.eq(&other.deadline)
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
        share_state: share_state,
    };
    sender.send(timer_entry).expect("failed to queue timer");
    sleep_fut
}
