//! Implementation of process [`ProcessControlBlock`] and task(thread) [`TaskControlBlock`] management mechanism
//!
//! Here is the entry for task scheduling required by other modules
//! (such as syscall or clock interrupt).
//! By suspending or exiting the current task, you can
//! modify the task state, manage the task queue through TASK_MANAGER (in task/manager.rs) ,
//! and switch the control flow through PROCESSOR (in task/processor.rs) .
//!
//! Be careful when you see [`__switch`]. Control flow around this function
//! might not be what you expect.

mod context;
mod id;
mod manager;
mod process;
mod processor;
mod signal;
mod switch;
#[allow(clippy::module_inception)]
mod task;

use self::id::TaskUserRes;
use crate::fs::{open_file, OpenFlags};
use crate::timer::remove_timer;
use alloc::{sync::Arc, vec::Vec};
use lazy_static::*;
use manager::fetch_task;
use process::ProcessControlBlock;
use switch::__switch;

pub use context::TaskContext;
pub use id::{kstack_alloc, pid_alloc, KernelStack, PidHandle, IDLE_PID};
pub use manager::{add_task, pid2process, remove_from_pid2process, tick_task, wakeup_task};
pub use processor::{
    current_kstack_top, current_process, current_task, current_trap_cx, current_trap_cx_user_va,
    current_user_token, run_tasks, schedule, take_current_task,
};
pub use signal::SignalFlags;
pub use task::{TaskControlBlock, TaskStatus};

fn task_cx_ptr_of(task: &TaskControlBlock) -> *mut TaskContext {
    &mut task.inner_exclusive_access().task_cx as *mut TaskContext
}

/// Make current task suspended and switch to the next task
pub fn suspend_current_and_run_next() {
    // There must be an application running.
    let task = take_current_task().unwrap();
    let task_cx_ptr = task_cx_ptr_of(&task);
    // jump to scheduling cycle, which pushes it back to ready queue
    schedule(task_cx_ptr, task, TaskStatus::Ready);
}

/// Make current task blocked and switch to the next task.
pub fn block_current_and_run_next() {
    let task = take_current_task().unwrap();
    let task_cx_ptr = task_cx_ptr_of(&task);
    schedule(task_cx_ptr, task, TaskStatus::Blocked);
}

use crate::board::QEMUExit;

/// Exit the current 'Running' task and run the next task in task list.
pub fn exit_current_and_run_next(exit_code: i32) {
    trace!(
        "kernel: pid[{}] exit_current_and_run_next",
        current_task().unwrap().process.upgrade().unwrap().getpid()
    );
    // take from Processor
    let task = take_current_task().unwrap();
    let mut task_inner = task.inner_exclusive_access();
    let process = task.process.upgrade().unwrap();
    let tid = task_inner.res.as_ref().unwrap().tid;
    // record exit code
    task_inner.exit_code = Some(exit_code);
    let res = task_inner.res.take();
    // here we do not remove the thread since we are still using the kstack
    // it will be deallocated when sys_waittid is called
    drop(task_inner);
    // releasing the user resource takes the process, which is locked before its threads
    drop(res);

    // however, if this is the main thread of current process
    // the process should terminate at once
    if tid == 0 {
        let pid = process.getpid();
        if pid == IDLE_PID {
            println!(
                "[kernel] Idle process exit with exit_code {} ...",
                exit_code
            );
            if exit_code != 0 {
                crate::board::QEMU_EXIT_HANDLE.exit_failure();
            } else {
                crate::board::QEMU_EXIT_HANDLE.exit_success();
            }
        }
        remove_from_pid2process(pid);
        let mut process_inner = process.inner_exclusive_access();
        // record exit code of main process
        process_inner.exit_code = exit_code;
        let children = core::mem::take(&mut process_inner.children);
        let others: Vec<_> = process_inner.tasks.iter().flatten().cloned().collect();
        drop(process_inner);

        // the other threads must stop before the address space goes away
        for other in others.iter().filter(|other| !Arc::ptr_eq(other, &task)) {
            // Ready threads are dropped when fetched, and blocked ones with the PCB.
            // Mention that we do not need to consider Mutex/Semaphore since they
            // are limited in a single process.
            other.inner_exclusive_access().killed = true;
            remove_timer(Arc::clone(other));
        }
        for other in others.iter().filter(|other| !Arc::ptr_eq(other, &task)) {
            // one running on another hart exits itself on its next trap
            while other.inner_exclusive_access().task_status == TaskStatus::Running {
                core::hint::spin_loop();
            }
        }

        // initproc locks its children in waitpid, so we do not hold ourselves meanwhile
        {
            let mut initproc_inner = INITPROC.inner_exclusive_access();
            for child in children {
                child.inner_exclusive_access().parent = Some(Arc::downgrade(&INITPROC));
                initproc_inner.children.push(child);
            }
        }

        // deallocate user res (including tid/trap_cx/ustack) of all threads
        // it has to be done before we dealloc the whole memory_set
        // otherwise they will be deallocated twice
        let mut recycle_res = Vec::<TaskUserRes>::new();
        for other in others.iter() {
            if let Some(res) = other.inner_exclusive_access().res.take() {
                recycle_res.push(res);
            }
        }
        // dealloc_tid and dealloc_user_res require access to PCB inner, so we
        // need to collect those user res first, then release process_inner
        // for now to avoid deadlock/double borrow problem.
        recycle_res.clear();

        let mut process_inner = process.inner_exclusive_access();
        // deallocate other data in user space i.e. program code/data section
        process_inner.memory_set.recycle_data_pages();
        // drop file descriptors
        process_inner.fd_table.clear();
        // threads blocked on them are released with the wait queues
        process_inner.mutex_list.clear();
        process_inner.semaphore_list.clear();
        process_inner.condvar_list.clear();
        // remove all tasks
        process_inner.tasks.clear();
        // mark this process as a zombie process
        process_inner.is_zombie = true;
    }
    drop(process);
    // we do not have to save task context
    let mut _unused = TaskContext::zero_init();
    schedule(&mut _unused as *mut _, task, TaskStatus::Exited);
}

lazy_static! {
    /// Creation of initial process
    ///
    /// the name "initproc" may be changed to any other app name like "usertests",
    /// but we have user_shell, so we don't need to change it.
    pub static ref INITPROC: Arc<ProcessControlBlock> = {
        let inode = open_file("ch8b_initproc", OpenFlags::RDONLY).unwrap();
        ProcessControlBlock::new(&inode.inode())
    };
}

///Add init process to the manager
pub fn add_initproc() {
    let _initproc = INITPROC.clone();
}

/// Check if the current task has any signal to handle
pub fn check_signals_of_current() -> Option<(i32, &'static str)> {
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    process_inner.signals.check_error()
}

/// Add signal to the current task
pub fn current_add_signal(signal: SignalFlags) {
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    process_inner.signals |= signal;
}
