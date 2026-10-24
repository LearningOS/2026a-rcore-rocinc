use crate::{Entity, Scheduler};
use alloc::collections::VecDeque;

/// Fixed-priority real-time scheduling: the ready task with the highest
/// priority runs, tasks with equal priority take turns.
pub struct FixedPriority<T> {
    ready: VecDeque<(T, usize)>,
}

impl<T> FixedPriority<T> {
    pub const fn new() -> Self {
        Self {
            ready: VecDeque::new(),
        }
    }
}

impl<T> Scheduler<T> for FixedPriority<T> {
    fn push(&mut self, task: T, entity: &mut Entity) {
        self.ready.push_back((task, entity.priority));
    }
    fn pop(&mut self) -> Option<T> {
        // the first one among the highest, so equal priorities stay FIFO
        let max = self.ready.iter().map(|(_, p)| *p).max()?;
        let idx = self.ready.iter().position(|(_, p)| *p == max)?;
        self.ready.remove(idx).map(|(task, _)| task)
    }
}
