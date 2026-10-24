use super::{get_block_cache, BlockDevice, BLOCK_SZ};
use alloc::sync::Arc;
/// A bitmap block
type BitmapBlock = [u64; 64];
/// Number of bits in a block
const BLOCK_BITS: usize = BLOCK_SZ * 8;
/// A bitmap
pub struct Bitmap {
    start_block_id: usize,
    blocks: usize,
}

/// Decompose bits into (block_pos, bits64_pos, inner_pos)
fn decomposition(mut bit: usize) -> (usize, usize, usize) {
    let block_pos = bit / BLOCK_BITS;
    bit %= BLOCK_BITS;
    (block_pos, bit / 64, bit % 64)
}

impl Bitmap {
    /// A new bitmap from start block id and number of blocks
    pub fn new(start_block_id: usize, blocks: usize) -> Self {
        Self {
            start_block_id,
            blocks,
        }
    }
    /// Allocate a new block from a block device
    pub fn alloc(&self, block_device: &Arc<dyn BlockDevice>) -> Option<usize> {
        for block_id in 0..self.blocks {
            let block_cache =
                get_block_cache(block_id + self.start_block_id, Arc::clone(block_device));
            let mut cache = block_cache.lock();
            // only the block that changes is modified, keeping transactions small
            let Some((bits64_pos, inner_pos)) = cache.read(0, |bitmap_block: &BitmapBlock| {
                bitmap_block
                    .iter()
                    .enumerate()
                    .find(|(_, bits64)| **bits64 != u64::MAX)
                    .map(|(bits64_pos, bits64)| (bits64_pos, bits64.trailing_ones() as usize))
            }) else {
                continue;
            };
            cache.modify(0, |bitmap_block: &mut BitmapBlock| {
                bitmap_block[bits64_pos] |= 1u64 << inner_pos;
            });
            return Some(block_id * BLOCK_BITS + bits64_pos * 64 + inner_pos);
        }
        None
    }
    /// Deallocate a block
    pub fn dealloc(&self, block_device: &Arc<dyn BlockDevice>, bit: usize) {
        let (block_pos, bits64_pos, inner_pos) = decomposition(bit);
        get_block_cache(block_pos + self.start_block_id, Arc::clone(block_device))
            .lock()
            .modify(0, |bitmap_block: &mut BitmapBlock| {
                assert!(bitmap_block[bits64_pos] & (1u64 << inner_pos) > 0);
                bitmap_block[bits64_pos] -= 1u64 << inner_pos;
            });
    }
    /// Count the allocated bits
    #[cfg(test)]
    pub fn allocated(&self, block_device: &Arc<dyn BlockDevice>) -> usize {
        (0..self.blocks)
            .map(|i| {
                get_block_cache(self.start_block_id + i, Arc::clone(block_device))
                    .lock()
                    .read(0, |bitmap_block: &BitmapBlock| {
                        bitmap_block
                            .iter()
                            .map(|b| b.count_ones() as usize)
                            .sum::<usize>()
                    })
            })
            .sum()
    }
    /// Get the max number of allocatable blocks
    pub fn maximum(&self) -> usize {
        self.blocks * BLOCK_BITS
    }
}
