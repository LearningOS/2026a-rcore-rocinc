//! Crash tests: a disk loses all writes after a given number of them,
//! and the file system must be consistent after recovery.
use super::{BlockDevice, DiskInode, EasyFileSystem, Inode, BLOCK_SZ};
use alloc::collections::BTreeSet;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

const TOTAL_BLOCKS: u32 = 2048;

/// The block cache is global, so the tests run one at a time
static SERIAL: Mutex<()> = Mutex::new(());

struct MemDisk {
    /// what survives a power loss
    blocks: Mutex<Vec<[u8; BLOCK_SZ]>>,
    /// what the running system sees
    volatile: Mutex<Vec<[u8; BLOCK_SZ]>>,
    /// writes left before the crash
    budget: Mutex<usize>,
    writes: Mutex<usize>,
}

impl MemDisk {
    fn new(blocks: Vec<[u8; BLOCK_SZ]>, budget: usize) -> Arc<Self> {
        Arc::new(Self {
            volatile: Mutex::new(blocks.clone()),
            blocks: Mutex::new(blocks),
            budget: Mutex::new(budget),
            writes: Mutex::new(0),
        })
    }
    /// What remains on the disk after power is lost
    fn reboot(&self, budget: usize) -> Arc<Self> {
        Self::new(self.blocks.lock().clone(), budget)
    }
}

impl BlockDevice for MemDisk {
    fn read_block(&self, block_id: usize, buf: &mut [u8]) {
        buf.copy_from_slice(&self.volatile.lock()[block_id]);
    }
    fn write_block(&self, block_id: usize, buf: &[u8]) {
        *self.writes.lock() += 1;
        self.volatile.lock()[block_id].copy_from_slice(buf);
        let mut budget = self.budget.lock();
        if *budget > 0 {
            *budget -= 1;
            self.blocks.lock()[block_id].copy_from_slice(buf);
        }
    }
}

fn size_of(inode: &Inode) -> usize {
    let mut buf = [0u8; BLOCK_SZ];
    let mut size = 0;
    loop {
        let len = inode.read_at(size, &mut buf);
        if len == 0 {
            return size;
        }
        size += len;
    }
}

fn content(inode: &Inode) -> Vec<u8> {
    let mut v = vec![0u8; size_of(inode)];
    inode.read_at(0, &mut v);
    v
}

/// Every allocated inode and data block belongs to a file, and the other way around
fn assert_consistent(efs: &Arc<Mutex<EasyFileSystem>>) {
    let root = EasyFileSystem::root_inode(efs);
    let mut inodes = BTreeSet::from([0]);
    let mut blocks = DiskInode::total_blocks(size_of(&root) as u32) as usize;
    for name in root.ls() {
        let inode = root.find(&name).unwrap();
        if inodes.insert(inode.inode_id()) {
            blocks += DiskInode::total_blocks(size_of(&inode) as u32) as usize;
        }
    }
    let fs = efs.lock();
    assert_eq!(fs.inode_bitmap.allocated(&fs.block_device), inodes.len());
    assert_eq!(fs.data_bitmap.allocated(&fs.block_device), blocks);
}

/// Run `op` on a file system prepared by `setup`, losing power after each possible
/// number of writes, and check the state recovered is either before or after `op`.
/// Returns the number of writes of `op`.
fn crash_test(
    setup: impl Fn(&Inode),
    op: impl Fn(&Inode),
    before: impl Fn(&Inode) -> bool,
    after: impl Fn(&Inode) -> bool,
) -> usize {
    let _serial = SERIAL.lock();
    let image = {
        let disk = MemDisk::new(vec![[0; BLOCK_SZ]; TOTAL_BLOCKS as usize], usize::MAX);
        let efs = EasyFileSystem::create(disk.clone(), TOTAL_BLOCKS, 1);
        setup(&EasyFileSystem::root_inode(&efs));
        drop(efs);
        let blocks = disk.blocks.lock().clone();
        blocks
    };
    let total = {
        let disk = MemDisk::new(image.clone(), usize::MAX);
        let efs = EasyFileSystem::open(disk.clone());
        op(&EasyFileSystem::root_inode(&efs));
        let writes = *disk.writes.lock();
        writes
    };
    for k in 0..=total {
        let disk = MemDisk::new(image.clone(), k);
        let efs = EasyFileSystem::open(disk.clone());
        op(&EasyFileSystem::root_inode(&efs));
        drop(efs);
        // also lose power once more while recovering
        for j in [0, 1, 2] {
            let efs = EasyFileSystem::open(disk.reboot(usize::MAX).reboot(j));
            drop(efs);
        }
        let efs = EasyFileSystem::open(disk.reboot(usize::MAX));
        assert_consistent(&efs);
        let root = EasyFileSystem::root_inode(&efs);
        assert!(
            after(&root) || k < total && before(&root),
            "crash after {} writes",
            k
        );
    }
    total
}

fn data(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 7 + i / 512) as u8).collect()
}

#[test]
fn crash_create() {
    let n = crash_test(
        |_| {},
        |root| {
            root.create("a");
        },
        |root| root.find("a").is_none(),
        |root| root.find("a").is_some_and(|a| size_of(&a) == 0),
    );
    std::println!("create: {} writes", n);
}

#[test]
fn crash_write() {
    // 5 blocks are written in one transaction, growing into indirect1
    let old = data(26 * BLOCK_SZ);
    let new = data(31 * BLOCK_SZ);
    let n = crash_test(
        |root| {
            root.create("a").unwrap().write_at(0, &old);
        },
        |root| {
            root.find("a")
                .unwrap()
                .write_at(26 * BLOCK_SZ, &new[26 * BLOCK_SZ..]);
        },
        |root| content(&root.find("a").unwrap()) == old,
        |root| content(&root.find("a").unwrap()) == new,
    );
    std::println!("write: {} writes", n);
}

#[test]
fn crash_link_unlink() {
    let file = data(40 * BLOCK_SZ);
    let n = crash_test(
        |root| {
            root.create("a").unwrap().write_at(0, &file);
            assert!(root.link("a", "b"));
        },
        |root| {
            assert!(root.unlink("a"));
            assert!(root.unlink("b"));
        },
        // either no entry was removed or only "a"
        |root| {
            root.find("b").is_some_and(|b| {
                content(&b) == file && root.nlink(b.inode_id()) == root.ls().len() as u32
            })
        },
        |root| root.ls().is_empty(),
    );
    std::println!("link + unlink: {} writes", n);
}

#[test]
fn unlink_open_file() {
    let _serial = SERIAL.lock();
    let disk = MemDisk::new(vec![[0; BLOCK_SZ]; TOTAL_BLOCKS as usize], usize::MAX);
    let efs = EasyFileSystem::create(disk, TOTAL_BLOCKS, 1);
    let root = EasyFileSystem::root_inode(&efs);
    let file = data(3 * BLOCK_SZ);
    let a = root.create("a").unwrap();
    a.write_at(0, &file);
    assert!(root.unlink("a"));
    assert!(!root.unlink("a"));
    // still readable until closed
    assert_eq!(content(&a), file);
    let fs = efs.lock();
    assert_eq!(fs.inode_bitmap.allocated(&fs.block_device), 2);
    drop(fs);
    drop(a);
    assert_consistent(&efs);
}

#[test]
fn write_amplification() {
    let _serial = SERIAL.lock();
    let disk = MemDisk::new(vec![[0; BLOCK_SZ]; TOTAL_BLOCKS as usize], usize::MAX);
    let efs = EasyFileSystem::create(disk.clone(), TOTAL_BLOCKS, 1);
    let a = EasyFileSystem::root_inode(&efs).create("a").unwrap();
    let before = *disk.writes.lock();
    a.write_at(0, &data(128 * BLOCK_SZ));
    let writes = *disk.writes.lock() - before;
    std::println!("writing 128 blocks: {} device writes", writes);
    // 16 transactions, each writing its blocks twice and the header twice
    assert_eq!((writes - 16 * 2) % 2, 0);
}
