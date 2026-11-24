use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

/// Fixed capacity, worker-owned local queue that pops task in LIFO order and overflows into [Inject].
pub(crate) struct Local<T> {
    queue: Arc<Mutex<VecDeque<T>>>,
    capacity: usize,
}

/// Shared handle to a worker's [Local] queue that allows sibling workers to steal tasks in FIFO order.
pub(crate) struct Steal<T> {
    queue: Arc<Mutex<VecDeque<T>>>,
    capacity: usize,
}

impl<T> Clone for Steal<T> {
    fn clone(&self) -> Self {
        Self {
            queue: self.queue.clone(),
            capacity: self.capacity,
        }
    }
}

/// Creates a worker-owned [Local] queue and [Steal] queue to allow other worker threads to steal tasks.
///
/// # Panics
///
/// Panics if `capacity` is 0.
fn worker_queue_with_capacity<T>(capacity: usize) -> (Local<T>, Steal<T>) {
    assert!(capacity > 0, "capacity must be greater than 0");
    let queue = Arc::new(Mutex::new(VecDeque::with_capacity(capacity)));

    let local_queue = Local {
        queue: queue.clone(),
        capacity,
    };
    let steal_queue = Steal { queue, capacity };
    (local_queue, steal_queue)
}

/// Global injection queue for tasks submitted from outside worker threads or overflowed from [Local] queues.
pub(crate) struct Inject<T> {
    queue: Mutex<VecDeque<T>>,
}

impl<T> Inject<T> {
    pub(crate) fn new() -> Self {
        Inject {
            queue: Mutex::new(VecDeque::new()),
        }
    }

    pub(crate) fn push(&self, item: T) {
        let mut guard = self
            .queue
            .lock()
            .expect("failed to acquire global injection queue lock");

        guard.push_back(item);
    }

    pub(crate) fn push_batch(&self, iter: impl IntoIterator<Item = T>) {
        let mut guard = self
            .queue
            .lock()
            .expect("failed to acquire global injection queue lock");

        for item in iter {
            guard.push_back(item);
        }
    }

    pub(crate) fn pop(&self) -> Option<T> {
        let mut guard = self
            .queue
            .lock()
            .expect("failed to acquire global injection queue lock");
        guard.pop_front()
    }

    pub(crate) fn is_empty(&self) -> bool {
        let guard = self
            .queue
            .lock()
            .expect("failed to acquire global injection queue lock");
        guard.is_empty()
    }
}

#[cfg(test)]
mod queue_test {
    use std::thread;

    use super::*;

    #[test]
    fn empty_inject_queue() {
        let inject = Inject::<i32>::new();

        assert!(inject.is_empty());
        assert!(inject.pop().is_none());

        inject.push_batch([]);

        assert!(inject.is_empty());
        assert!(inject.pop().is_none());
    }

    #[test]
    fn inject_queue_push_pop() {
        let inject = Inject::<i32>::new();
        inject.push(1);
        inject.push_batch([2, 3]);

        assert!(!inject.is_empty());
        assert_eq!(inject.pop().unwrap(), 1);
        assert_eq!(inject.pop().unwrap(), 2);
        assert_eq!(inject.pop().unwrap(), 3);
        assert!(inject.is_empty());
        assert!(inject.pop().is_none());

        inject.push_batch([3, 2]);
        inject.push(1);

        assert!(!inject.is_empty());
        assert_eq!(inject.pop().unwrap(), 3);
        assert_eq!(inject.pop().unwrap(), 2);
        assert_eq!(inject.pop().unwrap(), 1);
        assert!(inject.is_empty());
        assert!(inject.pop().is_none());
    }

    #[test]
    fn inject_queue_concurrent_push_pop() {
        let inject = Arc::new(Inject::<Arc<i32>>::new());

        let inject_1 = inject.clone();
        let inject_2 = inject.clone();

        let handle1 = thread::spawn(move || {
            for _ in 0..5 {
                inject_1.push(Arc::new(1));
            }
        });
        let handle2 = thread::spawn(move || {
            for _ in 0..5 {
                inject_2.push_batch([Arc::new(1), Arc::new(1)]);
            }
        });

        handle1.join().unwrap();
        handle2.join().unwrap();
        let mut count = 0;

        while inject.pop().is_some() {
            count += 1;
        }
        assert_eq!(count, 15);
    }

    #[test]
    fn create_worker_queue() {
        let (local_queue, steal_queue) = worker_queue_with_capacity::<i32>(10);
        let cloned_steal = steal_queue.clone();

        assert!(Arc::ptr_eq(&local_queue.queue, &steal_queue.queue));
        assert!(Arc::ptr_eq(&local_queue.queue, &cloned_steal.queue));

        assert_eq!(local_queue.capacity, 10);
        let queue = local_queue.queue.lock().unwrap();
        assert!(queue.capacity() >= 10);
        assert!(queue.is_empty());
        drop(queue);

        assert_eq!(steal_queue.capacity, 10);
        let queue = steal_queue.queue.lock().unwrap();
        assert!(queue.capacity() >= 10);
        assert!(queue.is_empty());
        drop(queue);
    }

    #[test]
    #[should_panic(expected = "capacity must be greater than 0")]
    fn create_worker_queue_reject_zero_capacity() {
        let _ = worker_queue_with_capacity::<i32>(0);
    }
}
