//! Disk block manager layer
//!
//! Disk block manager  knows the disk location of each layout area, and the allocation and reclamation of disk blocks need to be completed through it.
//!
//! NOTICE: from this level, all data structures are in memory.
use super::{
    block_cache_sync_all, dirty_block_caches, get_block_cache, Bitmap, BlockDevice, DiskInode,
    DiskInodeType, Inode, SuperBlock,
};
use crate::BLOCK_SZ;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::convert::TryInto;
use spin::Mutex;

/// The log header block, followed by the logged copies of blocks
const LOG_START: usize = 1;
/// The max number of blocks modified by an operation
const LOG_SIZE: usize = 32;
const LOG_BLOCKS: u32 = 1 + LOG_SIZE as u32;

/// EasyFileSystem struct
pub struct EasyFileSystem {
    /// The block device
    pub block_device: Arc<dyn BlockDevice>,
    /// The bitmap of inode blocks
    pub inode_bitmap: Bitmap,
    /// The bitmap of data blocks
    pub data_bitmap: Bitmap,
    /// The start block id of inode area
    inode_area_start_block: u32,
    /// The start block id of data area
    data_area_start_block: u32,
    /// Number of `Inode`s in memory for each inode
    pub(crate) open_count: BTreeMap<u32, usize>,
    /// Inodes without directory entries, freed when no longer open
    pub(crate) unlinked: BTreeSet<u32>,
}

type DataBlock = [u8; BLOCK_SZ];

impl EasyFileSystem {
    /// Create a new EasyFileSystem
    pub fn create(
        block_device: Arc<dyn BlockDevice>,
        total_blocks: u32,
        inode_bitmap_blocks: u32,
    ) -> Arc<Mutex<Self>> {
        // calculate block size of areas & create bitmaps
        let inode_bitmap = Bitmap::new(1 + LOG_BLOCKS as usize, inode_bitmap_blocks as usize);
        let inode_num = inode_bitmap.maximum();
        let inode_area_blocks =
            ((inode_num * core::mem::size_of::<DiskInode>() + BLOCK_SZ - 1) / BLOCK_SZ) as u32;
        let inode_total_blocks = inode_bitmap_blocks + inode_area_blocks;
        let data_total_blocks = total_blocks - 1 - LOG_BLOCKS - inode_total_blocks;
        let data_bitmap_blocks = (data_total_blocks + 4096) / 4097;
        let data_area_blocks = data_total_blocks - data_bitmap_blocks;
        let data_bitmap = Bitmap::new(
            (1 + LOG_BLOCKS + inode_total_blocks) as usize,
            data_bitmap_blocks as usize,
        );
        let mut efs = Self {
            block_device: Arc::clone(&block_device),
            inode_bitmap,
            data_bitmap,
            inode_area_start_block: 1 + LOG_BLOCKS + inode_bitmap_blocks,
            data_area_start_block: 1 + LOG_BLOCKS + inode_total_blocks + data_bitmap_blocks,
            open_count: BTreeMap::new(),
            unlinked: BTreeSet::new(),
        };
        // clear all blocks, including the log header
        for i in 0..total_blocks {
            block_device.write_block(i as usize, &[0; BLOCK_SZ]);
        }
        // initialize SuperBlock
        get_block_cache(0, Arc::clone(&block_device)).lock().modify(
            0,
            |super_block: &mut SuperBlock| {
                super_block.initialize(
                    total_blocks,
                    inode_bitmap_blocks,
                    inode_area_blocks,
                    data_bitmap_blocks,
                    data_area_blocks,
                );
            },
        );
        // write back immediately
        // create a inode for root node "/"
        assert_eq!(efs.alloc_inode(), 0);
        let (root_inode_block_id, root_inode_offset) = efs.get_disk_inode_pos(0);
        get_block_cache(root_inode_block_id as usize, Arc::clone(&block_device))
            .lock()
            .modify(root_inode_offset, |disk_inode: &mut DiskInode| {
                disk_inode.initialize(DiskInodeType::Directory);
            });
        block_cache_sync_all();
        Arc::new(Mutex::new(efs))
    }
    /// Open an existing EasyFileSystem
    pub fn open(block_device: Arc<dyn BlockDevice>) -> Arc<Mutex<Self>> {
        Self::recover(&block_device);
        // read SuperBlock
        get_block_cache(0, Arc::clone(&block_device))
            .lock()
            .read(0, |super_block: &SuperBlock| {
                assert!(super_block.is_valid(), "Error loading EFS!");
                let inode_total_blocks =
                    super_block.inode_bitmap_blocks + super_block.inode_area_blocks;
                let efs = Self {
                    block_device,
                    inode_bitmap: Bitmap::new(
                        1 + LOG_BLOCKS as usize,
                        super_block.inode_bitmap_blocks as usize,
                    ),
                    data_bitmap: Bitmap::new(
                        (1 + LOG_BLOCKS + inode_total_blocks) as usize,
                        super_block.data_bitmap_blocks as usize,
                    ),
                    inode_area_start_block: 1 + LOG_BLOCKS + super_block.inode_bitmap_blocks,
                    data_area_start_block: 1
                        + LOG_BLOCKS
                        + inode_total_blocks
                        + super_block.data_bitmap_blocks,
                    open_count: BTreeMap::new(),
                    unlinked: BTreeSet::new(),
                };
                Arc::new(Mutex::new(efs))
            })
    }
    /// Get the root inode
    pub fn root_inode(efs: &Arc<Mutex<Self>>) -> Inode {
        Inode::new(0, efs, &mut efs.lock())
    }
    /// Get inode block position (the block id and offset in this block) according to the inode id
    pub fn get_disk_inode_pos(&self, inode_id: u32) -> (u32, usize) {
        let inode_size = core::mem::size_of::<DiskInode>();
        let inodes_per_block = (BLOCK_SZ / inode_size) as u32;
        let block_id = self.inode_area_start_block + inode_id / inodes_per_block;
        (
            block_id,
            (inode_id % inodes_per_block) as usize * inode_size,
        )
    }
    /// Get data block position according to the data block id
    pub fn get_data_block_id(&self, data_block_id: u32) -> u32 {
        self.data_area_start_block + data_block_id
    }
    /// allocate a new inode, return its inode_id
    pub fn alloc_inode(&mut self) -> u32 {
        self.inode_bitmap.alloc(&self.block_device).unwrap() as u32
    }

    /// Deallocate an inode
    pub fn dealloc_inode(&mut self, inode_id: u32) {
        self.inode_bitmap
            .dealloc(&self.block_device, inode_id as usize)
    }
    /// Deallocate an inode and its data blocks
    pub(crate) fn free_inode(&mut self, inode_id: u32) {
        let (block_id, block_offset) = self.get_disk_inode_pos(inode_id);
        let data_blocks = get_block_cache(block_id as usize, Arc::clone(&self.block_device))
            .lock()
            .modify(block_offset, |disk_inode: &mut DiskInode| {
                disk_inode.clear_size(&self.block_device)
            });
        for data_block in data_blocks {
            self.dealloc_data(data_block);
        }
        self.dealloc_inode(inode_id);
    }
    /// allocate a zeroed data block, return its block position (block_id)
    pub fn alloc_data(&mut self) -> u32 {
        let block_id =
            self.data_bitmap.alloc(&self.block_device).unwrap() as u32 + self.data_area_start_block;
        // zeroed here instead of on deallocation, so that freeing a file modifies few blocks
        get_block_cache(block_id as usize, Arc::clone(&self.block_device))
            .lock()
            .modify(0, |data_block: &mut DataBlock| data_block.fill(0));
        block_id
    }
    /// deallocate a data block according to its block id
    pub fn dealloc_data(&mut self, block_id: u32) {
        self.data_bitmap.dealloc(
            &self.block_device,
            (block_id - self.data_area_start_block) as usize,
        )
    }
    /// Write the modified blocks to disk atomically: they are written to the log first,
    /// and copied to their home locations only after the log header records them.
    pub fn commit(&mut self) {
        let dirty = dirty_block_caches(&self.block_device);
        if dirty.is_empty() {
            return;
        }
        assert!(dirty.len() <= LOG_SIZE, "Too many blocks in a transaction!");
        let mut block_ids = Vec::new();
        for (i, cache) in dirty.iter().enumerate() {
            let cache = cache.lock();
            self.block_device
                .write_block(LOG_START + 1 + i, &cache.cache);
            block_ids.push(cache.block_id as u32);
        }
        // the transaction is committed once the header is written
        write_log_header(&self.block_device, &block_ids);
        for cache in dirty.iter() {
            cache.lock().sync();
        }
        write_log_header(&self.block_device, &[]);
    }
    /// Redo a committed transaction that was not completely copied home
    fn recover(block_device: &Arc<dyn BlockDevice>) {
        let block_ids = read_log_header(block_device);
        let mut buf = [0u8; BLOCK_SZ];
        for (i, &block_id) in block_ids.iter().enumerate() {
            block_device.read_block(LOG_START + 1 + i, &mut buf);
            block_device.write_block(block_id as usize, &buf);
        }
        if !block_ids.is_empty() {
            write_log_header(block_device, &[]);
        }
    }
}

/// The header holds the number of logged blocks and their home block ids
fn write_log_header(block_device: &Arc<dyn BlockDevice>, block_ids: &[u32]) {
    let mut buf = [0u8; BLOCK_SZ];
    let words = core::iter::once(block_ids.len() as u32).chain(block_ids.iter().copied());
    for (word, v) in buf.chunks_exact_mut(4).zip(words) {
        word.copy_from_slice(&v.to_le_bytes());
    }
    block_device.write_block(LOG_START, &buf);
}

fn read_log_header(block_device: &Arc<dyn BlockDevice>) -> Vec<u32> {
    let mut buf = [0u8; BLOCK_SZ];
    block_device.read_block(LOG_START, &mut buf);
    let mut words = buf
        .chunks_exact(4)
        .map(|word| u32::from_le_bytes(word.try_into().unwrap()));
    let count = words.next().unwrap() as usize;
    words.take(count).collect()
}
