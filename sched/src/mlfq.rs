use crate::{Entity, Scheduler};
use alloc::collections::VecDeque;

const LEVELS: usize = 4;
/// every `BOOST_TICKS` ticks all tasks go back to the top level
const BOOST_TICKS: usize = 100;

/// time allotment of `level` in ticks
const fn allotment(level: usize) -> usize {
    1 << level
}

// 参考：《Operating Systems: Three Easy Pieces》第 8 章 MLFQ 的规则（按配额降级、周期性提升）
/// Multi-level feedback queue: a task that uses up its allotment moves one
/// level down, tasks that give up the CPU early stay, and periodic boosts
/// raise every task to the top level.
pub struct Mlfq<T> {
    queues: [VecDeque<T>; LEVELS],
    ticks: usize,
    epoch: usize,
}

impl<T> Mlfq<T> {
    pub const fn new() -> Self {
        Self {
            queues: [const { VecDeque::new() }; LEVELS],
            ticks: 0,
            epoch: 0,
        }
    }
    fn sync(&self, entity: &mut Entity) {
        if entity.epoch != self.epoch {
            entity.epoch = self.epoch;
            entity.level = 0;
            entity.used = 0;
        }
    }
}

impl<T> Scheduler<T> for Mlfq<T> {
    fn push(&mut self, task: T, entity: &mut Entity) {
        self.sync(entity);
        self.queues[entity.level].push_back(task);
    }
    fn pop(&mut self) -> Option<T> {
        self.queues.iter_mut().find_map(|q| q.pop_front())
    }
    fn pick(&mut self, entity: &mut Entity) {
        self.sync(entity);
    }
    fn tick(&mut self, entity: &mut Entity) {
        self.ticks += 1;
        if self.ticks % BOOST_TICKS == 0 {
            self.epoch += 1;
            let (top, rest) = self.queues.split_first_mut().unwrap();
            for q in rest {
                top.append(q);
            }
        }
        self.sync(entity);
        entity.used += 1;
        if entity.used >= allotment(entity.level) && entity.level + 1 < LEVELS {
            entity.level += 1;
            entity.used = 0;
        }
    }
}
