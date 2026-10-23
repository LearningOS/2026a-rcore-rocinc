//! A swap device kept in the kernel heap, standing in for a disk.

use crate::config::PAGE_SIZE;
use crate::sync::UPSafeCell;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use lazy_static::*;

pub struct Swap {
    slots: Vec<Box<[u8]>>,
    free: Vec<usize>,
}

lazy_static! {
    pub static ref SWAP: UPSafeCell<Swap> = unsafe {
        UPSafeCell::new(Swap {
            slots: Vec::new(),
            free: Vec::new(),
        })
    };
}

impl Swap {
    /// Write a page into `slot`, or into a new slot if `None`. Returns the slot used.
    pub fn write(&mut self, slot: Option<usize>, page: &[u8]) -> usize {
        let slot = slot.or_else(|| self.free.pop()).unwrap_or_else(|| {
            self.slots.push(vec![0; PAGE_SIZE].into_boxed_slice());
            self.slots.len() - 1
        });
        self.slots[slot].copy_from_slice(page);
        slot
    }
    pub fn read(&self, slot: usize, page: &mut [u8]) {
        page.copy_from_slice(&self.slots[slot]);
    }
    pub fn free(&mut self, slot: usize) {
        self.free.push(slot);
    }
}
