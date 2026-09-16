use std::{
    collections::VecDeque,
    pin::Pin,
    sync::{Arc, Mutex},
};

pub struct Task {
    pub fut: Mutex<Option<Pin<Box<dyn Future<Output = ()>>>>>,
    pub task_sender: RunQueue,
}

#[derive(Clone)]
pub struct RunQueue {
    queue: Arc<Mutex<VecDeque<Arc<Task>>>>,
}

impl RunQueue {
    pub fn new() -> Self {
        let queue = Arc::new(Mutex::new(VecDeque::<Arc<Task>>::with_capacity(1000)));
        RunQueue { queue }
    }

    pub fn is_empty(&self) -> bool {
        self.queue.lock().is_ok_and(|q| q.is_empty())
    }

    pub fn pop(&self) -> Option<Arc<Task>> {
        if let Ok(mut queue) = self.queue.lock() {
            if !queue.is_empty() {
                return queue.pop_front().into();
            }
        }
        None
    }

    pub fn push(&self, task: Arc<Task>) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.push_back(task);
        }
    }
}
