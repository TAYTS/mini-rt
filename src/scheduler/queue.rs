use std::{collections::VecDeque, sync::Mutex};

/// Global injection queue for tasks submitted from outside worker threads.
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
    use std::{sync::Arc, thread};

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

        while let Some(_) = inject.pop() {
            count += 1;
        }
        assert_eq!(count, 15);
    }
}
