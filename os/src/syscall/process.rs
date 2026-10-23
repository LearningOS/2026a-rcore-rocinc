//! Process management syscalls
use crate::config::PAGE_SIZE;
use crate::mm::{translated_byte_buffer, MapPermission, PageTable, VirtAddr};
use crate::task::{
    change_program_brk, current_user_token, exit_current_and_run_next, get_syscall_times,
    suspend_current_and_run_next, with_current_memory_set,
};
use crate::timer::get_time_us;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// get time with second and microsecond
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let us = get_time_us();
    let time = TimeVal {
        sec: us / 1_000_000,
        usec: us % 1_000_000,
    };
    let src = unsafe {
        core::slice::from_raw_parts(
            &time as *const TimeVal as *const u8,
            core::mem::size_of::<TimeVal>(),
        )
    };
    let len = src.len();
    let start = VirtAddr::from(ts as usize);
    with_current_memory_set(|ms| ms.fault_in(start, (ts as usize + len).into(), MapPermission::W));
    // TimeVal may cross a page boundary
    let mut offset = 0;
    for buf in translated_byte_buffer(current_user_token(), ts as *const u8, len) {
        buf.copy_from_slice(&src[offset..offset + buf.len()]);
        offset += buf.len();
    }
    0
}

// 参考：rCore-Tutorial-Guide 第四章练习中 sys_trace 的要求，地址用户不可见或不可读/不可写时返回 -1
/// trace memory access or syscall count of current task
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!("kernel: sys_trace");
    let access = match trace_request {
        0 => MapPermission::R,
        1 => MapPermission::W,
        2 => return get_syscall_times(id) as isize,
        _ => return -1,
    };
    let va = VirtAddr::from(id);
    if usize::from(va) != id {
        return -1;
    }
    with_current_memory_set(|ms| ms.handle_page_fault(va, access));
    let Some(pte) = PageTable::from_token(current_user_token()).translate(va.floor()) else {
        return -1;
    };
    let perm = MapPermission::from_bits_truncate(pte.flags().bits());
    if !pte.is_valid() || !perm.contains(MapPermission::U | access) {
        return -1;
    }
    let byte = &mut pte.ppn().get_bytes_array()[va.page_offset()];
    if trace_request == 0 {
        *byte as isize
    } else {
        *byte = data as u8;
        0
    }
}

// 参考：rCore-Tutorial-Guide 第四章练习中 mmap 的错误条件，以及“注意 prot 与 MapPermission 的区别”“你增加 PTE_U 了吗”的提示
/// map anonymous memory of `len` bytes at `start`
pub fn sys_mmap(start: usize, len: usize, prot: usize) -> isize {
    trace!("kernel: sys_mmap");
    if start % PAGE_SIZE != 0 || prot & !0x7 != 0 || prot & 0x7 == 0 {
        return -1;
    }
    // prot is R/W/X from bit 0, while MapPermission starts from bit 1
    let perm = MapPermission::from_bits_truncate((prot << 1) as u8) | MapPermission::U;
    if with_current_memory_set(|ms| ms.mmap(start.into(), (start + len).into(), perm)) {
        0
    } else {
        -1
    }
}

/// unmap the memory of `len` bytes at `start`
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel: sys_munmap");
    if start % PAGE_SIZE != 0 {
        return -1;
    }
    if with_current_memory_set(|ms| ms.munmap(start.into(), (start + len).into())) {
        0
    } else {
        -1
    }
}
/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}
