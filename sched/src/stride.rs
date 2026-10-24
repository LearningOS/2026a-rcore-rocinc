use crate::{Entity, Scheduler};
use alloc::collections::VecDeque;
use core::cmp::Ordering;

/// With priorities >= 2, ready strides differ by at most `BIG_STRIDE / 2`.
pub const BIG_STRIDE: u64 = 1 << 40;

// 参考：rCore-Tutorial-Guide 第五章问答题中 STRIDE_MAX - STRIDE_MIN <= BigStride / 2 的结论
/// A stride that compares correctly after wrapping around.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stride(pub u64);

impl Stride {
    fn advance(&mut self, priority: usize) {
        let pass = (BIG_STRIDE / priority as u64).max(1);
        self.0 = self.0.wrapping_add(pass);
    }
}

impl Ord for Stride {
    fn cmp(&self, other: &Self) -> Ordering {
        // the real distance is below 2^63, so the wrapped difference keeps its sign
        (self.0.wrapping_sub(other.0) as i64).cmp(&0)
    }
}

impl PartialOrd for Stride {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Stride scheduling: run the ready task with the smallest stride.
pub struct StrideScheduler<T> {
    ready: VecDeque<(T, Stride)>,
}

impl<T> StrideScheduler<T> {
    pub const fn new() -> Self {
        Self {
            ready: VecDeque::new(),
        }
    }
}

impl<T> Scheduler<T> for StrideScheduler<T> {
    fn push(&mut self, task: T, entity: &mut Entity) {
        self.ready.push_back((task, entity.stride));
    }
    fn pop(&mut self) -> Option<T> {
        let (idx, _) = self
            .ready
            .iter()
            .enumerate()
            .min_by_key(|(_, (_, stride))| *stride)?;
        self.ready.remove(idx).map(|(task, _)| task)
    }
    fn pick(&mut self, entity: &mut Entity) {
        entity.stride.advance(entity.priority);
    }
}
