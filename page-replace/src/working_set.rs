use crate::{Pages, Replacer};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// Working set: evicts pages not referenced during the last `window` ticks.
///
/// 参考：操作系统课程“工作集置换算法”，每个时钟中断采样访问位，
/// 记录页面最近一次被访问的时刻。
pub struct WorkingSet {
    window: usize,
    last_use: BTreeMap<usize, usize>,
}

impl WorkingSet {
    pub fn new(window: usize) -> Self {
        Self {
            window,
            last_use: BTreeMap::new(),
        }
    }
}

impl Replacer for WorkingSet {
    fn insert(&mut self, vpn: usize, now: usize, _pages: &mut dyn Pages) -> Vec<usize> {
        self.last_use.insert(vpn, now);
        Vec::new()
    }

    fn tick(&mut self, now: usize, pages: &mut dyn Pages) -> Vec<usize> {
        let mut evicted = Vec::new();
        self.last_use.retain(|&vpn, last| match pages.flags(vpn) {
            None => false,
            Some(flags) if flags.accessed => {
                pages.clear_accessed(vpn);
                *last = now;
                true
            }
            Some(_) if now - *last >= self.window => {
                evicted.push(vpn);
                false
            }
            Some(_) => true,
        });
        evicted
    }
}
