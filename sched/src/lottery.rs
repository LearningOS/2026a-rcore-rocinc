use crate::{Entity, Scheduler};
use alloc::vec::Vec;

/// Lottery scheduling: each ready task holds `priority` tickets.
pub struct Lottery<T> {
    ready: Vec<(T, usize)>,
    seed: u64,
}

impl<T> Lottery<T> {
    pub const fn new() -> Self {
        Self {
            ready: Vec::new(),
            seed: 0x2545_f491_4f6c_dd1d,
        }
    }
    /// xorshift64
    fn random(&mut self) -> u64 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        self.seed
    }
}

impl<T> Scheduler<T> for Lottery<T> {
    fn push(&mut self, task: T, entity: &mut Entity) {
        self.ready.push((task, entity.priority));
    }
    fn pop(&mut self) -> Option<T> {
        let total = self
            .ready
            .iter()
            .fold(0u64, |sum, (_, t)| sum.saturating_add(*t as u64));
        if total == 0 {
            return None;
        }
        let mut winner = self.random() % total;
        let idx = self
            .ready
            .iter()
            .position(|(_, t)| match winner.checked_sub(*t as u64) {
                Some(rest) => {
                    winner = rest;
                    false
                }
                None => true,
            })?;
        Some(self.ready.swap_remove(idx).0)
    }
}
