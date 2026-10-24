//!Implementation of [`Processor`] and Intersection of control flow
//!
//! Here, the continuous operation of user apps in CPU is maintained,
//! the current running state of CPU is recorded,
//! and the replacement and transfer of control flow of different applications are executed.

use super::__switch;
use super::{add_task, fetch_task, TaskStatus};
use super::{TaskContext, TaskControlBlock};
use crate::config::MAX_HARTS;
use crate::sync::{hart_id, UPRefMut, UPSafeCell};
use crate::trap::TrapContext;
use alloc::sync::Arc;
use core::arch::asm;
use lazy_static::*;

/// Processor management structure
pub struct Processor {
    ///The task currently executing on the current processor
    current: Option<Arc<TaskControlBlock>>,

    ///The basic control flow of each core, helping to select and switch process
    idle_task_cx: TaskContext,

    /// The task that just switched out and the status it leaves with.
    /// Others may only see it after its context is saved and its kernel stack is free.
    prev: Option<(Arc<TaskControlBlock>, TaskStatus)>,
}

impl Processor {
    ///Create an empty Processor
    pub fn new() -> Self {
        Self {
            current: None,
            idle_task_cx: TaskContext::zero_init(),
            prev: None,
        }
    }

    ///Get mutable reference to `idle_task_cx`
    fn get_idle_task_cx_ptr(&mut self) -> *mut TaskContext {
        &mut self.idle_task_cx as *mut _
    }

    ///Get current task in moving semanteme
    pub fn take_current(&mut self) -> Option<Arc<TaskControlBlock>> {
        self.current.take()
    }

    ///Get current task in cloning semanteme
    pub fn current(&self) -> Option<Arc<TaskControlBlock>> {
        self.current.as_ref().map(Arc::clone)
    }
}

lazy_static! {
    static ref PROCESSORS: [UPSafeCell<Processor>; MAX_HARTS] =
        core::array::from_fn(|_| unsafe { UPSafeCell::new(Processor::new()) });
}

#[track_caller]
fn processor() -> UPRefMut<'static, Processor> {
    PROCESSORS[hart_id()].exclusive_access()
}

///The main part of process execution and scheduling
///Loop `fetch_task` to get the process that needs to run, and switch the process through `__switch`
pub fn run_tasks() {
    loop {
        let prev = processor().prev.take();
        if let Some((task, status)) = prev {
            task.inner_exclusive_access().task_status = status;
            if status == TaskStatus::Ready {
                add_task(task);
            }
        }
        if let Some(task) = fetch_task() {
            let mut processor = processor();
            let idle_task_cx_ptr = processor.get_idle_task_cx_ptr();
            // access coming task TCB exclusively
            let mut task_inner = task.inner_exclusive_access();
            let next_task_cx_ptr = &task_inner.task_cx as *const TaskContext;
            task_inner.task_status = TaskStatus::Running;
            // release coming task_inner manually
            drop(task_inner);
            // release coming task TCB manually
            processor.current = Some(task);
            // release processor manually
            drop(processor);
            // 参考：RISC-V 特权级规范 Supervisor Memory-Management Fence 一节，其他 hart 修改页表后，
            // 本 hart 需执行 sfence.vma 才能看到新的映射
            // the kernel stack may be remapped since this hart last flushed its TLB
            unsafe {
                asm!("sfence.vma");
                __switch(idle_task_cx_ptr, next_task_cx_ptr);
            }
        }
    }
}

/// Get current task through take, leaving a None in its place
pub fn take_current_task() -> Option<Arc<TaskControlBlock>> {
    processor().take_current()
}

/// Get a copy of the current task
pub fn current_task() -> Option<Arc<TaskControlBlock>> {
    processor().current()
}

/// Get the current user token(addr of page table)
pub fn current_user_token() -> usize {
    let task = current_task().unwrap();
    task.get_user_token()
}

///Get the mutable reference to trap context of current task
pub fn current_trap_cx() -> &'static mut TrapContext {
    current_task()
        .unwrap()
        .inner_exclusive_access()
        .get_trap_cx()
}

///Return to idle control flow for new scheduling,
///where `task` gets `status` after its context is saved
pub fn schedule(
    switched_task_cx_ptr: *mut TaskContext,
    task: Arc<TaskControlBlock>,
    status: TaskStatus,
) {
    let mut processor = processor();
    let idle_task_cx_ptr = processor.get_idle_task_cx_ptr();
    processor.prev = Some((task, status));
    drop(processor);
    unsafe {
        __switch(switched_task_cx_ptr, idle_task_cx_ptr);
    }
}
