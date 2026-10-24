use crate::{Entity, Scheduler};
use alloc::collections::VecDeque;

/// Earliest deadline first. `priority` is the relative deadline in ticks;
/// a task gets a new deadline when it becomes ready after the old one passed.
pub struct Edf<T> {
    ready: VecDeque<(T, usize)>,
    now: usize,
}

impl<T> Edf<T> {
    pub const fn new() -> Self {
        Self {
            ready: VecDeque::new(),
            now: 0,
        }
    }
}

impl<T> Scheduler<T> for Edf<T> {
    fn push(&mut self, task: T, entity: &mut Entity) {
        if entity.deadline <= self.now {
            entity.deadline = self.now.saturating_add(entity.priority);
        }
        self.ready.push_back((task, entity.deadline));
    }
    fn pop(&mut self) -> Option<T> {
        let min = self.ready.iter().map(|(_, d)| *d).min()?;
        let idx = self.ready.iter().position(|(_, d)| *d == min)?;
        self.ready.remove(idx).map(|(task, _)| task)
    }
    fn tick(&mut self, _entity: &mut Entity) {
        self.now += 1;
    }
}
