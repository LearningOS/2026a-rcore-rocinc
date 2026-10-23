use crate::{Pages, Replacer};
use alloc::collections::BTreeSet;
use alloc::vec::Vec;

/// Page fault frequency: grows the resident set while faults are frequent,
/// and shrinks it when more than `interval` ticks pass between two faults.
///
/// 参考：操作系统课程“缺页率置换算法”，缺页间隔大于 T 时换出两次缺页之间
/// 未被访问的页面，否则只加入缺失页；每次缺页后清除所有访问位。
pub struct PageFaultFrequency {
    interval: usize,
    last_fault: usize,
    resident: BTreeSet<usize>,
}

impl PageFaultFrequency {
    pub fn new(interval: usize) -> Self {
        Self {
            interval,
            last_fault: 0,
            resident: BTreeSet::new(),
        }
    }
}

impl Replacer for PageFaultFrequency {
    fn insert(&mut self, vpn: usize, now: usize, pages: &mut dyn Pages) -> Vec<usize> {
        let shrink = now - self.last_fault > self.interval;
        let mut evicted = Vec::new();
        self.resident.remove(&vpn);
        self.resident.retain(|&v| match pages.flags(v) {
            None => false,
            Some(flags) if !flags.accessed && shrink => {
                evicted.push(v);
                false
            }
            Some(flags) => {
                if flags.accessed {
                    pages.clear_accessed(v);
                }
                true
            }
        });
        self.resident.insert(vpn);
        self.last_fault = now;
        evicted
    }
}
