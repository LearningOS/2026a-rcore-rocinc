//! Task schedulers that do not depend on any kernel.
//!
//! The kernel keeps an [`Entity`] in every task. It calls [`Scheduler::push`]
//! when a task becomes ready, [`Scheduler::pop`] and then [`Scheduler::pick`]
//! with the entity of the popped task to choose the next one, and
//! [`Scheduler::tick`] on every timer interrupt of a running task.
#![no_std]

extern crate alloc;

mod edf;
mod fifo;
mod lottery;
mod mlfq;
mod rt;
mod stride;

use alloc::boxed::Box;

pub use edf::Edf;
pub use fifo::Fifo;
pub use lottery::Lottery;
pub use mlfq::Mlfq;
pub use rt::FixedPriority;
pub use stride::{Stride, StrideScheduler, BIG_STRIDE};

/// Initial priority of a task.
pub const DEFAULT_PRIORITY: usize = 16;

/// Scheduling state of a task.
#[derive(Clone, Copy, Debug)]
pub struct Entity {
    /// Stride weight, lottery tickets, real-time priority,
    /// or EDF relative deadline in ticks, depending on the scheduler.
    pub priority: usize,
    stride: Stride,
    level: usize,
    /// ticks used at `level`
    used: usize,
    /// MLFQ boost epoch that `level` belongs to
    epoch: usize,
    deadline: usize,
}

impl Entity {
    /// An entity with [`DEFAULT_PRIORITY`] and stride 0.
    pub const fn new() -> Self {
        Self {
            priority: DEFAULT_PRIORITY,
            stride: Stride(0),
            level: 0,
            used: 0,
            epoch: 0,
            deadline: 0,
        }
    }
}

impl Default for Entity {
    fn default() -> Self {
        Self::new()
    }
}

/// A ready queue with a scheduling policy.
pub trait Scheduler<T> {
    /// `task` becomes ready.
    fn push(&mut self, task: T, entity: &mut Entity);
    /// Remove the task to run next.
    fn pop(&mut self) -> Option<T>;
    /// `entity` belongs to the task just returned by `pop`.
    fn pick(&mut self, _entity: &mut Entity) {}
    /// A timer interrupt hit the running task of `entity`.
    fn tick(&mut self, _entity: &mut Entity) {}
}

/// Create a scheduler by name: `fifo`, `stride`, `lottery`, `mlfq`, `rt` or `edf`.
pub fn by_name<T: Send + 'static>(name: &str) -> Option<Box<dyn Scheduler<T> + Send>> {
    Some(match name {
        "fifo" => Box::new(Fifo::new()),
        "stride" => Box::new(StrideScheduler::new()),
        "lottery" => Box::new(Lottery::new()),
        "mlfq" => Box::new(Mlfq::new()),
        "rt" => Box::new(FixedPriority::new()),
        "edf" => Box::new(Edf::new()),
        _ => return None,
    })
}
