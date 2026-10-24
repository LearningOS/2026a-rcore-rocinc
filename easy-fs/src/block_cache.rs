//! Block Cache Layer
//! Implements about the disk block cache functionality
use super::{BlockDevice, BLOCK_SZ};
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use lazy_static::*;
use spin::Mutex;
/// BlockCache is a cache for a block in disk.
pub struct BlockCache {
    pub(crate) cache: Vec<u8>,
    pub(crate) block_id: usize,
    block_device: Arc<dyn BlockDevice>,
    modified: bool,
}

impl BlockCache {
    /// Load a new BlockCache from disk.
    pub fn new(block_id: usize, block_device: Arc<dyn BlockDevice>) -> Self {
        // for alignment and move effciency
        let mut cache = vec![0u8; BLOCK_SZ];
        block_device.read_block(block_id, &mut cache);
        Self {
            cache,
            block_id,
            block_device,
            modified: false,
        }
    }
    /// Get the slice in the block cache according to the offset.
    fn addr_of_offset(&self, offset: usize) -> usize {
        &self.cache[offset] as *const _ as usize
    }
    /// Get a immutable reference to the data in the block cache according to the offset.
    pub fn get_ref<T>(&self, offset: usize) -> &T
    where
        T: Sized,
    {
        let type_size = core::mem::size_of::<T>();
        assert!(offset + type_size <= BLOCK_SZ);
        let addr = self.addr_of_offset(offset);
        unsafe { &*(addr as *const T) }
    }
    /// Get a mutable reference to the data in the block cache according to the offset.
    pub fn get_mut<T>(&mut self, offset: usize) -> &mut T
    where
        T: Sized,
    {
        let type_size = core::mem::size_of::<T>();
        assert!(offset + type_size <= BLOCK_SZ);
        self.modified = true;
        let addr = self.addr_of_offset(offset);
        unsafe { &mut *(addr as *mut T) }
    }
    /// Read the data from the block cache according to the offset.
    pub fn read<T, V>(&self, offset: usize, f: impl FnOnce(&T) -> V) -> V {
        f(self.get_ref(offset))
    }
    /// Write the data into the block cache according to the offset.
    pub fn modify<T, V>(&mut self, offset: usize, f: impl FnOnce(&mut T) -> V) -> V {
        f(self.get_mut(offset))
    }
    /// Sync(write) the block cache to disk.
    pub fn sync(&mut self) {
        if self.modified {
            self.modified = false;
            self.block_device.write_block(self.block_id, &self.cache);
        }
    }
}

impl Drop for BlockCache {
    fn drop(&mut self) {
        self.sync()
    }
}

/// More blocks than an operation modifies
const BLOCK_CACHE_SIZE: usize = 64;

/// Identify a device by its address, so that a cache never serves another device
fn device_id(block_device: &Arc<dyn BlockDevice>) -> usize {
    Arc::as_ptr(block_device) as *const () as usize
}

/// BlockCacheManager is a manager for BlockCache.
pub struct BlockCacheManager {
    /// ((device, block_id), block_cache)
    queue: VecDeque<((usize, usize), Arc<Mutex<BlockCache>>)>,
}

impl BlockCacheManager {
    /// Create a new BlockCacheManager with an empty queue (block_id, block_cache)
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }
    /// Get a block cache from the queue. according to the block_id.
    pub fn get_block_cache(
        &mut self,
        block_id: usize,
        block_device: Arc<dyn BlockDevice>,
    ) -> Arc<Mutex<BlockCache>> {
        let key = (device_id(&block_device), block_id);
        if let Some(pair) = self.queue.iter().find(|pair| pair.0 == key) {
            Arc::clone(&pair.1)
        } else {
            // substitute
            if self.queue.len() == BLOCK_CACHE_SIZE {
                // from front to tail
                if let Some((idx, _)) = self
                    .queue
                    .iter()
                    .enumerate()
                    // modified blocks wait for the journal to commit them
                    .find(|(_, pair)| Arc::strong_count(&pair.1) == 1 && !pair.1.lock().modified)
                {
                    self.queue.drain(idx..=idx);
                } else {
                    panic!("Run out of BlockCache!");
                }
            }
            // load block into mem and push back
            let block_cache = Arc::new(Mutex::new(BlockCache::new(
                block_id,
                Arc::clone(&block_device),
            )));
            self.queue.push_back((key, Arc::clone(&block_cache)));
            block_cache
        }
    }
}

lazy_static! {
    /// BLOCK_CACHE_MANAGER: Glocal instance of BlockCacheManager.
    pub static ref BLOCK_CACHE_MANAGER: Mutex<BlockCacheManager> =
        Mutex::new(BlockCacheManager::new());
}
/// Get a block cache from the queue. according to the block_id.
pub fn get_block_cache(
    block_id: usize,
    block_device: Arc<dyn BlockDevice>,
) -> Arc<Mutex<BlockCache>> {
    BLOCK_CACHE_MANAGER
        .lock()
        .get_block_cache(block_id, block_device)
}
/// Get the modified block caches of the given block device
pub fn dirty_block_caches(block_device: &Arc<dyn BlockDevice>) -> Vec<Arc<Mutex<BlockCache>>> {
    let id = device_id(block_device);
    BLOCK_CACHE_MANAGER
        .lock()
        .queue
        .iter()
        .filter(|(key, cache)| key.0 == id && cache.lock().modified)
        .map(|(_, cache)| Arc::clone(cache))
        .collect()
}
/// Sync(write) all the block cache to disk.
pub fn block_cache_sync_all() {
    let manager = BLOCK_CACHE_MANAGER.lock();
    for (_, cache) in manager.queue.iter() {
        cache.lock().sync();
    }
}
