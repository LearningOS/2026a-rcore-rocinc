use crate::{Entity, Scheduler};
use alloc::collections::VecDeque;

/// Round robin in arrival order.
pub struct Fifo<T> {
    ready: VecDeque<T>,
}

impl<T> Fifo<T> {
    pub const fn new() -> Self {
        Self {
            ready: VecDeque::new(),
        }
    }
}

impl<T> Scheduler<T> for Fifo<T> {
    fn push(&mut self, task: T, _entity: &mut Entity) {
        self.ready.push_back(task);
    }
    fn pop(&mut self) -> Option<T> {
        self.ready.pop_front()
    }
}
