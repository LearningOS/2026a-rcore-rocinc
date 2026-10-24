//! Banker's algorithm bookkeeping for deadlock detection

use alloc::vec;
use alloc::vec::Vec;

/// Per-process resource matrices, rows are threads and columns are resources
#[derive(Default)]
pub struct Banker {
    avail: Vec<usize>,
    alloc: Vec<Vec<usize>>,
    need: Vec<Vec<usize>>,
}

impl Banker {
    /// Create an empty banker
    pub fn new() -> Self {
        Self::default()
    }

    /// Register resource `id` with `count` units
    pub fn add_resource(&mut self, id: usize, count: usize) {
        if self.avail.len() <= id {
            self.avail.resize(id + 1, 0);
            for row in self.alloc.iter_mut().chain(self.need.iter_mut()) {
                row.resize(id + 1, 0);
            }
        }
        self.avail[id] = count;
        for row in self.alloc.iter_mut().chain(self.need.iter_mut()) {
            row[id] = 0;
        }
    }

    fn grow(&mut self, tid: usize) {
        while self.alloc.len() <= tid {
            self.alloc.push(vec![0; self.avail.len()]);
            self.need.push(vec![0; self.avail.len()]);
        }
    }

    /// A new thread takes `tid`, which may be recycled from an exited one
    pub fn reset_thread(&mut self, tid: usize) {
        self.grow(tid);
        self.alloc[tid].fill(0);
        self.need[tid].fill(0);
    }

    /// Record a pending request; with `check` set, refuse it if the state becomes unsafe
    pub fn request(&mut self, tid: usize, id: usize, check: bool) -> bool {
        self.grow(tid);
        self.need[tid][id] += 1;
        if check && !self.is_safe() {
            self.need[tid][id] -= 1;
            return false;
        }
        true
    }

    /// The pending request has been granted
    pub fn acquire(&mut self, tid: usize, id: usize) {
        self.need[tid][id] -= 1;
        self.alloc[tid][id] += 1;
        self.avail[id] -= 1;
    }

    /// One unit is returned, possibly by a thread other than the holder
    pub fn release(&mut self, tid: usize, id: usize) {
        self.grow(tid);
        self.alloc[tid][id] = self.alloc[tid][id].saturating_sub(1);
        self.avail[id] += 1;
    }

    fn is_safe(&self) -> bool {
        let mut work = self.avail.clone();
        let mut done = vec![false; self.alloc.len()];
        loop {
            let mut progress = false;
            for t in 0..done.len() {
                if !done[t] && self.need[t].iter().zip(&work).all(|(n, w)| n <= w) {
                    for (w, a) in work.iter_mut().zip(&self.alloc[t]) {
                        *w += a;
                    }
                    done[t] = true;
                    progress = true;
                }
            }
            if !progress {
                return done.iter().all(|&d| d);
            }
        }
    }
}
