//! Memory management implementation
//!
//! SV39 page-based virtual-memory architecture for RV64 systems, and
//! everything about memory management, like frame allocator, page table,
//! map area and memory set, is implemented here.
//!
//! Every task or process has a memory_set to control its virtual memory.

mod address;
mod frame_allocator;
mod heap_allocator;
mod memory_set;
mod page_table;

use crate::sbi::remote_sfence_vma;
use crate::sync::hart_id;
use address::VPNRange;
pub use address::{PhysAddr, PhysPageNum, StepByOne, VirtAddr, VirtPageNum};
use core::sync::atomic::{AtomicUsize, Ordering};
pub use frame_allocator::{frame_alloc, frame_dealloc, FrameTracker};
pub use memory_set::remap_test;
pub use memory_set::{kernel_token, MapPermission, MemorySet, KERNEL_SPACE};
pub use page_table::PTEFlags;
pub use page_table::{
    translated_byte_buffer, translated_ref, translated_refmut, translated_str, PageTable,
    PageTableEntry, UserBuffer, UserBufferIterator,
};

/// initiate heap allocator, frame allocator and kernel space
pub fn init() {
    heap_allocator::init_heap();
    frame_allocator::init_frame_allocator();
    activate_kernel_space();
}
/// harts that have entered the kernel, to which remote TLB flushes are sent
static ONLINE_HARTS: AtomicUsize = AtomicUsize::new(0);

/// switch the current hart to kernel space
pub fn activate_kernel_space() {
    KERNEL_SPACE.exclusive_access().activate();
    ONLINE_HARTS.fetch_or(1 << hart_id(), Ordering::Release);
}

/// flush the TLB entries of [start, start + size) on all harts in the kernel;
/// a hart joining later flushes its TLB before running any task
pub fn flush_tlb(start: usize, size: usize) {
    remote_sfence_vma(ONLINE_HARTS.load(Ordering::Acquire), start, size);
}
