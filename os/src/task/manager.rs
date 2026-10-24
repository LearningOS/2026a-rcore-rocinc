//!Implementation of [`TaskManager`]
use super::TaskControlBlock;
use crate::sync::UPSafeCell;
use alloc::boxed::Box;
use alloc::sync::Arc;
use lazy_static::*;
use sched::Scheduler;

/// Scheduler chosen at build time by `SCHED`, stride by default
const SCHED: &str = match option_env!("SCHED") {
    Some(name) => name,
    None => "stride",
};

///A ready queue of `TaskControlBlock` shared by all harts
pub struct TaskManager {
    scheduler: Box<dyn Scheduler<Arc<TaskControlBlock>> + Send>,
}

impl TaskManager {
    ///Creat an empty TaskManager
    pub fn new() -> Self {
        Self {
            scheduler: sched::by_name(SCHED).expect("unknown SCHED"),
        }
    }
    /// Add process back to ready queue
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        let mut inner = task.inner_exclusive_access();
        self.scheduler.push(task.clone(), &mut inner.sched);
    }
    /// Take a process out of the ready queue
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        let task = self.scheduler.pop()?;
        self.scheduler
            .pick(&mut task.inner_exclusive_access().sched);
        Some(task)
    }
    /// A timer interrupt hit the running `task`
    pub fn tick(&mut self, task: &TaskControlBlock) {
        self.scheduler
            .tick(&mut task.inner_exclusive_access().sched);
    }
}

lazy_static! {
    /// TASK_MANAGER instance through lazy_static!
    pub static ref TASK_MANAGER: UPSafeCell<TaskManager> =
        unsafe { UPSafeCell::new(TaskManager::new()) };
}

/// Add process to ready queue
pub fn add_task(task: Arc<TaskControlBlock>) {
    TASK_MANAGER.exclusive_access().add(task);
}

/// Take a process out of the ready queue
pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    TASK_MANAGER.exclusive_access().fetch()
}

/// Update the scheduling state of the running `task` on a timer interrupt
pub fn tick_task(task: &TaskControlBlock) {
    TASK_MANAGER.exclusive_access().tick(task);
}
