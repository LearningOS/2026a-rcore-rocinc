use crate::{Flags, Pages, Replacer};
use alloc::vec::Vec;

/// Enhanced clock with a fixed number of frames (local replacement).
///
/// 参考：操作系统课程“改进的 Clock 页面置换算法”，按 (访问位, 修改位) 扫描：
/// (0,0) 换出，(0,1) 写回后变为 (0,0)，(1,x) 清访问位。
pub struct Clock {
    frames: usize,
    ring: Vec<usize>,
    hand: usize,
}

impl Clock {
    pub fn new(frames: usize) -> Self {
        Self {
            frames,
            ring: Vec::new(),
            hand: 0,
        }
    }
}

impl Replacer for Clock {
    fn insert(&mut self, vpn: usize, _now: usize, pages: &mut dyn Pages) -> Vec<usize> {
        let mut evicted = Vec::new();
        if self.ring.contains(&vpn) {
            return evicted;
        }
        while self.ring.len() >= self.frames {
            if self.hand >= self.ring.len() {
                self.hand = 0;
            }
            let victim = self.ring[self.hand];
            match pages.flags(victim) {
                None => {
                    self.ring.remove(self.hand);
                }
                Some(Flags {
                    accessed: false,
                    dirty: false,
                }) => evicted.push(self.ring.remove(self.hand)),
                Some(Flags {
                    accessed: false,
                    dirty: true,
                }) => {
                    pages.write_back(victim);
                    self.hand += 1;
                }
                Some(_) => {
                    pages.clear_accessed(victim);
                    self.hand += 1;
                }
            }
        }
        // just behind the hand, so it is the last to be examined
        self.ring.insert(self.hand.min(self.ring.len()), vpn);
        self.hand += 1;
        evicted
    }
}
