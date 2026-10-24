//! Implementation of [`TaskManager`]
//!
//! It is only used to manage processes and schedule process based on ready queue.
//! Other CPU process monitoring functions are in Processor.

use super::{ProcessControlBlock, TaskControlBlock, TaskStatus};
use crate::sync::UPSafeCell;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
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
    /// PID2PCB instance (map of pid to pcb)
    pub static ref PID2PCB: UPSafeCell<BTreeMap<usize, Arc<ProcessControlBlock>>> =
        unsafe { UPSafeCell::new(BTreeMap::new()) };
}

/// Add a task to ready queue
pub fn add_task(task: Arc<TaskControlBlock>) {
    //trace!("kernel: TaskManager::add_task");
    TASK_MANAGER.exclusive_access().add(task);
}

/// Wake up a task. If it has not finished blocking on its hart yet,
/// `run_tasks` makes it ready once it has.
pub fn wakeup_task(task: Arc<TaskControlBlock>) {
    trace!("kernel: TaskManager::wakeup_task");
    let mut task_inner = task.inner_exclusive_access();
    match task_inner.task_status {
        TaskStatus::Blocked => task_inner.task_status = TaskStatus::Ready,
        TaskStatus::Running => {
            task_inner.wakeup_pending = true;
            return;
        }
        _ => return,
    }
    drop(task_inner);
    add_task(task);
}

/// Update the scheduling state of the running `task` on a timer interrupt
pub fn tick_task(task: &TaskControlBlock) {
    TASK_MANAGER.exclusive_access().tick(task);
}

/// Fetch a task out of the ready queue
pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    //trace!("kernel: TaskManager::fetch_task");
    TASK_MANAGER.exclusive_access().fetch()
}

/// Get process by pid
pub fn pid2process(pid: usize) -> Option<Arc<ProcessControlBlock>> {
    let map = PID2PCB.exclusive_access();
    map.get(&pid).map(Arc::clone)
}

/// Insert item(pid, pcb) into PID2PCB map (called by do_fork AND ProcessControlBlock::new)
pub fn insert_into_pid2process(pid: usize, process: Arc<ProcessControlBlock>) {
    PID2PCB.exclusive_access().insert(pid, process);
}

/// Remove item(pid, _some_pcb) from PDI2PCB map (called by exit_current_and_run_next)
pub fn remove_from_pid2process(pid: usize) {
    let mut map = PID2PCB.exclusive_access();
    if map.remove(&pid).is_none() {
        panic!("cannot find pid {} in pid2task!", pid);
    }
}
