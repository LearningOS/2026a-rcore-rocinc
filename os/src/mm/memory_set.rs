//! Implementation of [`MapArea`] and [`MemorySet`].
use super::{frame_alloc, FrameTracker};
use super::{PTEFlags, PageTable, PageTableEntry};
use super::{PhysAddr, PhysPageNum, VirtAddr, VirtPageNum};
use super::{StepByOne, VPNRange};
use crate::config::{MEMORY_END, MMIO, PAGE_SIZE, TRAMPOLINE, TRAP_CONTEXT_BASE, USER_STACK_SIZE};
use crate::sync::UPSafeCell;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::arch::asm;
use easy_fs::Inode;
use lazy_static::*;
use riscv::register::satp;

extern "C" {
    fn stext();
    fn etext();
    fn srodata();
    fn erodata();
    fn sdata();
    fn edata();
    fn sbss_with_stack();
    fn ebss();
    fn ekernel();
    fn strampoline();
}

lazy_static! {
    /// The kernel's initial memory mapping(kernel address space)
    pub static ref KERNEL_SPACE: Arc<UPSafeCell<MemorySet>> =
        Arc::new(unsafe { UPSafeCell::new(MemorySet::new_kernel()) });
}

/// the kernel token
pub fn kernel_token() -> usize {
    KERNEL_SPACE.exclusive_access().token()
}

/// address space
pub struct MemorySet {
    page_table: PageTable,
    areas: Vec<MapArea>,
}

impl MemorySet {
    /// Create a new empty `MemorySet`.
    pub fn new_bare() -> Self {
        Self {
            page_table: PageTable::new(),
            areas: Vec::new(),
        }
    }
    /// Get the page table token
    pub fn token(&self) -> usize {
        self.page_table.token()
    }
    /// Assume that no conflicts.
    pub fn insert_framed_area(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
        permission: MapPermission,
    ) {
        self.push(
            MapArea::new(start_va, end_va, MapType::Framed, permission),
            None,
        );
    }
    /// remove a area
    pub fn remove_area_with_start_vpn(&mut self, start_vpn: VirtPageNum) {
        if let Some((idx, area)) = self
            .areas
            .iter_mut()
            .enumerate()
            .find(|(_, area)| area.vpn_range.get_start() == start_vpn)
        {
            area.unmap(&mut self.page_table);
            self.areas.remove(idx);
        }
    }
    /// Add a new MapArea into this MemorySet.
    /// Assuming that there are no conflicts in the virtual address
    /// space.
    fn push(&mut self, mut map_area: MapArea, data: Option<&[u8]>) {
        map_area.map(&mut self.page_table);
        if let Some(data) = data {
            map_area.copy_data(&mut self.page_table, data);
        }
        self.areas.push(map_area);
    }
    /// Mention that trampoline is not collected by areas.
    fn map_trampoline(&mut self) {
        self.page_table.map(
            VirtAddr::from(TRAMPOLINE).into(),
            PhysAddr::from(strampoline as usize).into(),
            PTEFlags::R | PTEFlags::X,
        );
    }
    /// Without kernel stacks.
    pub fn new_kernel() -> Self {
        let mut memory_set = Self::new_bare();
        // map trampoline
        memory_set.map_trampoline();
        // map kernel sections
        info!(".text [{:#x}, {:#x})", stext as usize, etext as usize);
        info!(".rodata [{:#x}, {:#x})", srodata as usize, erodata as usize);
        info!(".data [{:#x}, {:#x})", sdata as usize, edata as usize);
        info!(
            ".bss [{:#x}, {:#x})",
            sbss_with_stack as usize, ebss as usize
        );
        info!("mapping .text section");
        memory_set.push(
            MapArea::new(
                (stext as usize).into(),
                (etext as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::X,
            ),
            None,
        );
        info!("mapping .rodata section");
        memory_set.push(
            MapArea::new(
                (srodata as usize).into(),
                (erodata as usize).into(),
                MapType::Identical,
                MapPermission::R,
            ),
            None,
        );
        info!("mapping .data section");
        memory_set.push(
            MapArea::new(
                (sdata as usize).into(),
                (edata as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
            ),
            None,
        );
        info!("mapping .bss section");
        memory_set.push(
            MapArea::new(
                (sbss_with_stack as usize).into(),
                (ebss as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
            ),
            None,
        );
        info!("mapping physical memory");
        memory_set.push(
            MapArea::new(
                (ekernel as usize).into(),
                MEMORY_END.into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
            ),
            None,
        );
        info!("mapping memory-mapped registers");
        for pair in MMIO {
            memory_set.push(
                MapArea::new(
                    (*pair).0.into(),
                    ((*pair).0 + (*pair).1).into(),
                    MapType::Identical,
                    MapPermission::R | MapPermission::W,
                ),
                None,
            );
        }
        memory_set
    }
    /// Include sections in elf and trampoline and TrapContext and user stack,
    /// also returns user_sp_base and entry point.
    /// Sections in elf are loaded from `elf` page by page when first accessed.
    pub fn from_elf(elf: &Arc<Inode>) -> (Self, usize, usize) {
        let mut memory_set = Self::new_bare();
        // map trampoline
        memory_set.map_trampoline();
        // read the elf header, then the program headers following it
        let mut elf_data = vec![0u8; 64];
        elf.read_at(0, &mut elf_data);
        let ph_end = {
            let pt2 = xmas_elf::ElfFile::new(&elf_data).unwrap().header.pt2;
            pt2.ph_offset() as usize + pt2.ph_count() as usize * pt2.ph_entry_size() as usize
        };
        elf_data.resize(ph_end, 0);
        elf.read_at(0, &mut elf_data);
        let elf_file = xmas_elf::ElfFile::new(&elf_data).unwrap();
        let elf_header = elf_file.header;
        let magic = elf_header.pt1.magic;
        assert_eq!(magic, [0x7f, 0x45, 0x4c, 0x46], "invalid elf!");
        let ph_count = elf_header.pt2.ph_count();
        let mut max_end_vpn = VirtPageNum(0);
        for i in 0..ph_count {
            let ph = elf_file.program_header(i).unwrap();
            if ph.get_type().unwrap() == xmas_elf::program::Type::Load {
                let start_va: VirtAddr = (ph.virtual_addr() as usize).into();
                let end_va: VirtAddr = ((ph.virtual_addr() + ph.mem_size()) as usize).into();
                let mut map_perm = MapPermission::U;
                let ph_flags = ph.flags();
                if ph_flags.is_read() {
                    map_perm |= MapPermission::R;
                }
                if ph_flags.is_write() {
                    map_perm |= MapPermission::W;
                }
                if ph_flags.is_execute() {
                    map_perm |= MapPermission::X;
                }
                let mut map_area = MapArea::new(start_va, end_va, MapType::Framed, map_perm);
                map_area.file = Some(FileSegment {
                    inode: elf.clone(),
                    va: ph.virtual_addr() as usize,
                    offset: ph.offset() as usize,
                    size: ph.file_size() as usize,
                });
                max_end_vpn = map_area.vpn_range.get_end();
                memory_set.areas.push(map_area);
            }
        }
        // map user stack with U flags
        let max_end_va: VirtAddr = max_end_vpn.into();
        let mut user_stack_bottom: usize = max_end_va.into();
        // guard page
        user_stack_bottom += PAGE_SIZE;
        let user_stack_top = user_stack_bottom + USER_STACK_SIZE;
        memory_set.push(
            MapArea::new(
                user_stack_bottom.into(),
                user_stack_top.into(),
                MapType::Framed,
                MapPermission::R | MapPermission::W | MapPermission::U,
            ),
            None,
        );
        // used in sbrk
        memory_set.push(
            MapArea::new(
                user_stack_top.into(),
                user_stack_top.into(),
                MapType::Framed,
                MapPermission::R | MapPermission::W | MapPermission::U,
            ),
            None,
        );
        // map TrapContext
        memory_set.push(
            MapArea::new(
                TRAP_CONTEXT_BASE.into(),
                TRAMPOLINE.into(),
                MapType::Framed,
                MapPermission::R | MapPermission::W,
            ),
            None,
        );
        (
            memory_set,
            user_stack_top,
            elf_header.pt2.entry_point() as usize,
        )
    }
    /// Create a new address space that shares the user pages of `user_space` copy-on-write.
    pub fn from_existed_user(user_space: &mut Self) -> Self {
        let mut memory_set = Self::new_bare();
        // map trampoline
        memory_set.map_trampoline();
        for area in user_space.areas.iter() {
            let mut new_area = MapArea::from_another(area);
            if area.map_perm.contains(MapPermission::U) {
                // both sides map the frame read-only until one of them writes
                let flags = PTEFlags::from_bits(area.map_perm.bits).unwrap() - PTEFlags::W;
                for (&vpn, frame) in area.data_frames.iter() {
                    user_space.page_table.remap(vpn, frame.ppn, flags);
                    memory_set.page_table.map(vpn, frame.ppn, flags);
                    new_area.data_frames.insert(vpn, frame.clone());
                }
            } else {
                // the kernel writes the trap context directly, so copy it now
                for &vpn in area.data_frames.keys() {
                    new_area.map_one(&mut memory_set.page_table, vpn);
                    let src_ppn = user_space.translate(vpn).unwrap().ppn();
                    let dst_ppn = memory_set.translate(vpn).unwrap().ppn();
                    dst_ppn
                        .get_bytes_array()
                        .copy_from_slice(src_ppn.get_bytes_array());
                }
            }
            memory_set.areas.push(new_area);
        }
        memory_set
    }
    // 参考：RISC-V 特权级规范中 Store/AMO Page Fault 的触发条件，写 W=0 的有效页面时产生该异常，stval 为出错地址
    /// Give `vpn` a frame of its own if it is shared copy-on-write.
    /// Returns false if `vpn` is not a copy-on-write page.
    pub fn copy_on_write(&mut self, vpn: VirtPageNum) -> bool {
        let Some(area) = self
            .areas
            .iter_mut()
            .find(|area| area.data_frames.contains_key(&vpn))
        else {
            return false;
        };
        if self.page_table.translate(vpn).unwrap().writable()
            || !area.map_perm.contains(MapPermission::W)
        {
            return false;
        }
        let frame = area.data_frames.get_mut(&vpn).unwrap();
        // only holders can share it further, so a single owner may write in place
        if Arc::get_mut(frame).is_none() {
            let copy = frame_alloc().unwrap();
            copy.ppn
                .get_bytes_array()
                .copy_from_slice(frame.ppn.get_bytes_array());
            *frame = Arc::new(copy);
        }
        let flags = PTEFlags::from_bits(area.map_perm.bits).unwrap();
        self.page_table.remap(vpn, frame.ppn, flags);
        true
    }
    /// Load `vpn` from the file if it belongs to a section not loaded yet.
    fn load_page(&mut self, vpn: VirtPageNum) -> bool {
        let Some(area) = self.areas.iter_mut().find(|area| {
            area.file.is_some()
                && area.vpn_range.get_start() <= vpn
                && vpn < area.vpn_range.get_end()
                && !area.data_frames.contains_key(&vpn)
        }) else {
            return false;
        };
        area.map_one(&mut self.page_table, vpn);
        let file = area.file.as_ref().unwrap();
        let page_va = VirtAddr::from(vpn).0;
        let start = page_va.max(file.va);
        let end = (page_va + PAGE_SIZE).min(file.va + file.size);
        if start < end {
            let page = self
                .page_table
                .translate(vpn)
                .unwrap()
                .ppn()
                .get_bytes_array();
            file.inode.read_at(
                file.offset + start - file.va,
                &mut page[start - page_va..end - page_va],
            );
        }
        true
    }
    /// Handle a page fault at `vpn` for demand paging or copy-on-write.
    pub fn handle_page_fault(&mut self, vpn: VirtPageNum, write: bool) -> bool {
        self.load_page(vpn) || (write && self.copy_on_write(vpn))
    }
    /// Load the pages in [start, end) before the kernel reads there.
    pub fn prepare_read(&mut self, start: VirtAddr, end: VirtAddr) {
        for vpn in VPNRange::new(start.floor(), end.ceil()) {
            self.load_page(vpn);
        }
    }
    /// Load the pages and resolve copy-on-write in [start, end) before the kernel writes there.
    pub fn prepare_write(&mut self, start: VirtAddr, end: VirtAddr) {
        for vpn in VPNRange::new(start.floor(), end.ceil()) {
            self.load_page(vpn);
            self.copy_on_write(vpn);
        }
    }
    /// Read a string ending with `\0` at `ptr`, loading its pages first.
    pub fn read_str(&mut self, ptr: *const u8) -> String {
        let mut string = String::new();
        let mut va = ptr as usize;
        loop {
            self.load_page(VirtAddr::from(va).floor());
            let ch: u8 = *self.page_table.translate_va(va.into()).unwrap().get_mut();
            if ch == 0 {
                break;
            }
            string.push(ch as char);
            va += 1;
        }
        string
    }
    /// Change page table by writing satp CSR Register.
    pub fn activate(&self) {
        let satp = self.page_table.token();
        unsafe {
            satp::write(satp);
            asm!("sfence.vma");
        }
    }
    /// Translate a virtual page number to a page table entry
    pub fn translate(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        self.page_table.translate(vpn)
    }

    ///Remove all `MapArea`
    pub fn recycle_data_pages(&mut self) {
        self.areas.clear();
    }

    /// shrink the area to new_end
    #[allow(unused)]
    pub fn shrink_to(&mut self, start: VirtAddr, new_end: VirtAddr) -> bool {
        if let Some(area) = self
            .areas
            .iter_mut()
            .find(|area| area.vpn_range.get_start() == start.floor())
        {
            area.shrink_to(&mut self.page_table, new_end.ceil());
            true
        } else {
            false
        }
    }

    /// append the area to new_end
    #[allow(unused)]
    pub fn append_to(&mut self, start: VirtAddr, new_end: VirtAddr) -> bool {
        if let Some(area) = self
            .areas
            .iter_mut()
            .find(|area| area.vpn_range.get_start() == start.floor())
        {
            area.append_to(&mut self.page_table, new_end.ceil());
            true
        } else {
            false
        }
    }
    /// map [start, end) for user, fail if any page is already mapped
    pub fn mmap(&mut self, start: VirtAddr, end: VirtAddr, perm: MapPermission) -> bool {
        let range = VPNRange::new(start.floor(), end.ceil());
        // sections not loaded yet are taken as well
        if range.into_iter().any(|vpn| {
            self.translate(vpn).is_some_and(|pte| pte.is_valid())
                || self.areas.iter().any(|area| {
                    area.file.is_some()
                        && area.vpn_range.get_start() <= vpn
                        && vpn < area.vpn_range.get_end()
                })
        }) {
            return false;
        }
        self.insert_framed_area(start, end, perm);
        true
    }
    /// unmap [start, end), fail if any page is not mapped for user
    pub fn munmap(&mut self, start: VirtAddr, end: VirtAddr) -> bool {
        let range = VPNRange::new(start.floor(), end.ceil());
        if !range.into_iter().all(|vpn| {
            self.translate(vpn)
                .is_some_and(|pte| pte.is_valid() && pte.flags().contains(PTEFlags::U))
        }) {
            return false;
        }
        for vpn in range {
            if let Some(area) = self
                .areas
                .iter_mut()
                .find(|area| area.data_frames.contains_key(&vpn))
            {
                area.unmap_one(&mut self.page_table, vpn);
            }
        }
        true
    }
}
/// map area structure, controls a contiguous piece of virtual memory
pub struct MapArea {
    vpn_range: VPNRange,
    data_frames: BTreeMap<VirtPageNum, Arc<FrameTracker>>,
    map_type: MapType,
    map_perm: MapPermission,
    /// where the area is loaded from on demand
    file: Option<FileSegment>,
}

/// `size` bytes at `offset` of the file, which are mapped at `va`
#[derive(Clone)]
struct FileSegment {
    inode: Arc<Inode>,
    va: usize,
    offset: usize,
    size: usize,
}

impl MapArea {
    pub fn new(
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_type: MapType,
        map_perm: MapPermission,
    ) -> Self {
        let start_vpn: VirtPageNum = start_va.floor();
        let end_vpn: VirtPageNum = end_va.ceil();
        Self {
            vpn_range: VPNRange::new(start_vpn, end_vpn),
            data_frames: BTreeMap::new(),
            map_type,
            map_perm,
            file: None,
        }
    }
    pub fn from_another(another: &Self) -> Self {
        Self {
            vpn_range: VPNRange::new(another.vpn_range.get_start(), another.vpn_range.get_end()),
            data_frames: BTreeMap::new(),
            map_type: another.map_type,
            map_perm: another.map_perm,
            file: another.file.clone(),
        }
    }
    pub fn map_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        let ppn: PhysPageNum;
        match self.map_type {
            MapType::Identical => {
                ppn = PhysPageNum(vpn.0);
            }
            MapType::Framed => {
                let frame = frame_alloc().unwrap();
                ppn = frame.ppn;
                self.data_frames.insert(vpn, Arc::new(frame));
            }
        }
        let pte_flags = PTEFlags::from_bits(self.map_perm.bits).unwrap();
        page_table.map(vpn, ppn, pte_flags);
    }
    pub fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        if self.map_type == MapType::Framed {
            self.data_frames.remove(&vpn);
        }
        page_table.unmap(vpn);
    }
    pub fn map(&mut self, page_table: &mut PageTable) {
        for vpn in self.vpn_range {
            self.map_one(page_table, vpn);
        }
    }
    pub fn unmap(&mut self, page_table: &mut PageTable) {
        for vpn in self.vpn_range {
            self.unmap_one(page_table, vpn);
        }
    }
    #[allow(unused)]
    pub fn shrink_to(&mut self, page_table: &mut PageTable, new_end: VirtPageNum) {
        for vpn in VPNRange::new(new_end, self.vpn_range.get_end()) {
            self.unmap_one(page_table, vpn)
        }
        self.vpn_range = VPNRange::new(self.vpn_range.get_start(), new_end);
    }
    #[allow(unused)]
    pub fn append_to(&mut self, page_table: &mut PageTable, new_end: VirtPageNum) {
        for vpn in VPNRange::new(self.vpn_range.get_end(), new_end) {
            self.map_one(page_table, vpn)
        }
        self.vpn_range = VPNRange::new(self.vpn_range.get_start(), new_end);
    }
    /// data: start-aligned but maybe with shorter length
    /// assume that all frames were cleared before
    pub fn copy_data(&mut self, page_table: &mut PageTable, data: &[u8]) {
        assert_eq!(self.map_type, MapType::Framed);
        let mut start: usize = 0;
        let mut current_vpn = self.vpn_range.get_start();
        let len = data.len();
        loop {
            let src = &data[start..len.min(start + PAGE_SIZE)];
            let dst = &mut page_table
                .translate(current_vpn)
                .unwrap()
                .ppn()
                .get_bytes_array()[..src.len()];
            dst.copy_from_slice(src);
            start += PAGE_SIZE;
            if start >= len {
                break;
            }
            current_vpn.step();
        }
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
/// map type for memory set: identical or framed
pub enum MapType {
    Identical,
    Framed,
}

bitflags! {
    /// map permission corresponding to that in pte: `R W X U`
    pub struct MapPermission: u8 {
        ///Readable
        const R = 1 << 1;
        ///Writable
        const W = 1 << 2;
        ///Excutable
        const X = 1 << 3;
        ///Accessible in U mode
        const U = 1 << 4;
    }
}

/// remap test in kernel space
#[allow(unused)]
pub fn remap_test() {
    let mut kernel_space = KERNEL_SPACE.exclusive_access();
    let mid_text: VirtAddr = ((stext as usize + etext as usize) / 2).into();
    let mid_rodata: VirtAddr = ((srodata as usize + erodata as usize) / 2).into();
    let mid_data: VirtAddr = ((sdata as usize + edata as usize) / 2).into();
    assert!(!kernel_space
        .page_table
        .translate(mid_text.floor())
        .unwrap()
        .writable(),);
    assert!(!kernel_space
        .page_table
        .translate(mid_rodata.floor())
        .unwrap()
        .writable(),);
    assert!(!kernel_space
        .page_table
        .translate(mid_data.floor())
        .unwrap()
        .executable(),);
    println!("remap_test passed!");
}
