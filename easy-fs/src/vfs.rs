use super::{
    get_block_cache, BlockDevice, DirEntry, DiskInode, DiskInodeType, EasyFileSystem, BLOCK_SZ,
    DIRENT_SZ,
};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, MutexGuard};
/// Virtual filesystem layer over easy-fs
pub struct Inode {
    inode_id: u32,
    block_id: usize,
    block_offset: usize,
    fs: Arc<Mutex<EasyFileSystem>>,
    block_device: Arc<dyn BlockDevice>,
}

impl Inode {
    /// Create a vfs inode, where `fs` is the locked `efs`
    pub(crate) fn new(
        inode_id: u32,
        efs: &Arc<Mutex<EasyFileSystem>>,
        fs: &mut EasyFileSystem,
    ) -> Self {
        *fs.open_count.entry(inode_id).or_insert(0) += 1;
        let (block_id, block_offset) = fs.get_disk_inode_pos(inode_id);
        Self {
            inode_id,
            block_id: block_id as usize,
            block_offset,
            fs: Arc::clone(efs),
            block_device: Arc::clone(&fs.block_device),
        }
    }
    /// Get the inode number
    pub fn inode_id(&self) -> u32 {
        self.inode_id
    }
    /// Whether this inode is a directory
    pub fn is_dir(&self) -> bool {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| disk_inode.is_dir())
    }
    /// Call a function over a disk inode to read it
    fn read_disk_inode<V>(&self, f: impl FnOnce(&DiskInode) -> V) -> V {
        get_block_cache(self.block_id, Arc::clone(&self.block_device))
            .lock()
            .read(self.block_offset, f)
    }
    /// Call a function over a disk inode to modify it
    fn modify_disk_inode<V>(&self, f: impl FnOnce(&mut DiskInode) -> V) -> V {
        get_block_cache(self.block_id, Arc::clone(&self.block_device))
            .lock()
            .modify(self.block_offset, f)
    }
    /// Get the directory entries under a disk inode with their indexes,
    /// where an unlinked entry is left empty
    fn dirents(&self, disk_inode: &DiskInode) -> Vec<(usize, DirEntry)> {
        // assert it is a directory
        assert!(disk_inode.is_dir());
        let file_count = (disk_inode.size as usize) / DIRENT_SZ;
        (0..file_count)
            .map(|i| {
                let mut dirent = DirEntry::empty();
                assert_eq!(
                    disk_inode.read_at(DIRENT_SZ * i, dirent.as_bytes_mut(), &self.block_device,),
                    DIRENT_SZ,
                );
                (i, dirent)
            })
            .filter(|(_, dirent)| !dirent.name().is_empty())
            .collect()
    }
    /// Find inode under a disk inode by name
    fn find_inode_id(&self, name: &str, disk_inode: &DiskInode) -> Option<u32> {
        self.dirents(disk_inode)
            .into_iter()
            .find(|(_, dirent)| dirent.name() == name)
            .map(|(_, dirent)| dirent.inode_id())
    }
    /// Write a directory entry into the first empty slot, or append it
    fn add_dirent(&self, name: &str, inode_id: u32, fs: &mut MutexGuard<EasyFileSystem>) {
        self.modify_disk_inode(|root_inode| {
            let file_count = (root_inode.size as usize) / DIRENT_SZ;
            let used: Vec<usize> = self.dirents(root_inode).iter().map(|(i, _)| *i).collect();
            let slot = (0..file_count)
                .find(|i| used.binary_search(i).is_err())
                .unwrap_or(file_count);
            // increase size
            self.increase_size(((slot + 1) * DIRENT_SZ) as u32, root_inode, fs);
            // write dirent
            let dirent = DirEntry::new(name, inode_id);
            root_inode.write_at(slot * DIRENT_SZ, dirent.as_bytes(), &self.block_device);
        });
    }
    /// Find inode under current inode by name
    pub fn find(&self, name: &str) -> Option<Arc<Inode>> {
        let mut fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| self.find_inode_id(name, disk_inode))
            .map(|inode_id| Arc::new(Self::new(inode_id, &self.fs, &mut fs)))
    }
    /// Increase the size of a disk inode
    fn increase_size(
        &self,
        new_size: u32,
        disk_inode: &mut DiskInode,
        fs: &mut MutexGuard<EasyFileSystem>,
    ) {
        if new_size < disk_inode.size {
            return;
        }
        let blocks_needed = disk_inode.blocks_num_needed(new_size);
        let mut v: Vec<u32> = Vec::new();
        for _ in 0..blocks_needed {
            v.push(fs.alloc_data());
        }
        disk_inode.increase_size(new_size, v, &self.block_device);
    }
    /// Create inode under current inode by name
    pub fn create(&self, name: &str) -> Option<Arc<Inode>> {
        let mut fs = self.fs.lock();
        let op = |root_inode: &DiskInode| {
            // assert it is a directory
            assert!(root_inode.is_dir());
            // has the file been created?
            self.find_inode_id(name, root_inode)
        };
        if self.read_disk_inode(op).is_some() {
            return None;
        }
        // create a new file
        // alloc a inode with an indirect block
        let new_inode_id = fs.alloc_inode();
        // initialize inode
        let (new_inode_block_id, new_inode_block_offset) = fs.get_disk_inode_pos(new_inode_id);
        get_block_cache(new_inode_block_id as usize, Arc::clone(&self.block_device))
            .lock()
            .modify(new_inode_block_offset, |new_inode: &mut DiskInode| {
                new_inode.initialize(DiskInodeType::File);
            });
        self.add_dirent(name, new_inode_id, &mut fs);

        fs.commit();
        // return inode
        Some(Arc::new(Self::new(new_inode_id, &self.fs, &mut fs)))
        // release efs lock automatically by compiler
    }
    /// Create a directory entry `new_name` for the file of `old_name` under current inode
    pub fn link(&self, old_name: &str, new_name: &str) -> bool {
        let mut fs = self.fs.lock();
        let (old, new) = self.read_disk_inode(|root_inode| {
            (
                self.find_inode_id(old_name, root_inode),
                self.find_inode_id(new_name, root_inode),
            )
        });
        let (Some(inode_id), None) = (old, new) else {
            return false;
        };
        self.add_dirent(new_name, inode_id, &mut fs);
        fs.commit();
        true
    }
    /// Remove the directory entry `name` under current inode.
    /// The file is freed when no entry points to it and it is not open.
    pub fn unlink(&self, name: &str) -> bool {
        let mut fs = self.fs.lock();
        let Some((slot, inode_id, nlink)) = self.read_disk_inode(|root_inode| {
            let dirents = self.dirents(root_inode);
            let (slot, dirent) = dirents.iter().find(|(_, dirent)| dirent.name() == name)?;
            let id = dirent.inode_id();
            let nlink = dirents.iter().filter(|(_, d)| d.inode_id() == id).count();
            Some((*slot, id, nlink))
        }) else {
            return false;
        };
        self.modify_disk_inode(|root_inode| {
            root_inode.write_at(
                slot * DIRENT_SZ,
                DirEntry::empty().as_bytes(),
                &self.block_device,
            );
        });
        if nlink == 1 {
            if fs.open_count.contains_key(&inode_id) {
                fs.unlinked.insert(inode_id);
            } else {
                fs.free_inode(inode_id);
            }
        }
        fs.commit();
        true
    }
    /// Count the directory entries under current inode that point to `inode_id`
    pub fn nlink(&self, inode_id: u32) -> u32 {
        let _fs = self.fs.lock();
        self.read_disk_inode(|root_inode| {
            self.dirents(root_inode)
                .iter()
                .filter(|(_, dirent)| dirent.inode_id() == inode_id)
                .count() as u32
        })
    }
    /// List inodes under current inode
    pub fn ls(&self) -> Vec<String> {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| {
            self.dirents(disk_inode)
                .iter()
                .map(|(_, dirent)| String::from(dirent.name()))
                .collect()
        })
    }
    /// Read data from current inode
    pub fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let _fs = self.fs.lock();
        self.read_disk_inode(|disk_inode| disk_inode.read_at(offset, buf, &self.block_device))
    }
    /// Write data to current inode
    pub fn write_at(&self, offset: usize, buf: &[u8]) -> usize {
        let mut fs = self.fs.lock();
        let mut size = 0;
        // each chunk is a transaction, which fits in the log
        for chunk in buf.chunks(8 * BLOCK_SZ) {
            let start = offset + size;
            size += self.modify_disk_inode(|disk_inode| {
                self.increase_size((start + chunk.len()) as u32, disk_inode, &mut fs);
                disk_inode.write_at(start, chunk, &self.block_device)
            });
            fs.commit();
        }
        size
    }
    /// Clear the data in current inode
    pub fn clear(&self) {
        let mut fs = self.fs.lock();
        self.modify_disk_inode(|disk_inode| {
            let size = disk_inode.size;
            let data_blocks_dealloc = disk_inode.clear_size(&self.block_device);
            assert!(data_blocks_dealloc.len() == DiskInode::total_blocks(size) as usize);
            for data_block in data_blocks_dealloc.into_iter() {
                fs.dealloc_data(data_block);
            }
        });
        fs.commit();
    }
}

impl Drop for Inode {
    fn drop(&mut self) {
        let mut fs = self.fs.lock();
        let count = fs.open_count.get_mut(&self.inode_id).unwrap();
        *count -= 1;
        if *count == 0 {
            fs.open_count.remove(&self.inode_id);
            if fs.unlinked.remove(&self.inode_id) {
                fs.free_inode(self.inode_id);
                fs.commit();
            }
        }
    }
}
