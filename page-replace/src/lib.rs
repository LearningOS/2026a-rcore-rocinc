//! Page replacement algorithms that do not depend on any kernel.
//!
//! Pages are identified by virtual page numbers. The kernel exposes the
//! accessed/dirty bits of resident pages through [`Pages`], and a [`Replacer`]
//! returns the pages the kernel should swap out.
#![no_std]

extern crate alloc;

mod clock;
mod pff;
mod working_set;

use alloc::vec::Vec;

pub use clock::Clock;
pub use pff::PageFaultFrequency;
pub use working_set::WorkingSet;

/// Accessed and dirty bits of a resident page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Flags {
    pub accessed: bool,
    pub dirty: bool,
}

/// The resident pages of one address space.
pub trait Pages {
    /// Flags of `vpn`, or `None` if it is no longer resident.
    fn flags(&mut self, vpn: usize) -> Option<Flags>;
    fn clear_accessed(&mut self, vpn: usize);
    /// Save the page to swap and clear its dirty bit.
    fn write_back(&mut self, vpn: usize);
}

/// A page replacement policy for one address space.
pub trait Replacer {
    /// `vpn` became resident at virtual time `now`; returns pages to evict.
    fn insert(&mut self, vpn: usize, now: usize, pages: &mut dyn Pages) -> Vec<usize>;
    /// Called on every clock tick of the owner; returns pages to evict.
    fn tick(&mut self, _now: usize, _pages: &mut dyn Pages) -> Vec<usize> {
        Vec::new()
    }
}
