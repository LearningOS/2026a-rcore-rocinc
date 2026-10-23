//! Implementation of [`MapArea`] and [`MemorySet`].

use super::swap::SWAP;
use super::{frame_alloc, FrameTracker};
use super::{PTEFlags, PageTable, PageTableEntry};
use super::{PhysAddr, PhysPageNum, VirtAddr, VirtPageNum};
use super::{StepByOne, VPNRange};
use crate::config::{
    CLOCK_FRAMES, KERNEL_STACK_SIZE, MEMORY_END, PAGE_SIZE, PFF_INTERVAL, TRAMPOLINE,
    TRAP_CONTEXT_BASE, USER_STACK_SIZE, WORKING_SET_WINDOW,
};
use crate::sync::UPSafeCell;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::arch::asm;
use lazy_static::*;
use page_replace::{Clock, Flags, PageFaultFrequency, Pages, Replacer, WorkingSet};
use riscv::register::satp;

/// Page policy chosen at build time by `PAGE`: unset means mmap allocates eagerly,
/// otherwise mmap pages are allocated on page fault, and `clock`/`ws`/`pff` also swap.
pub const PAGE_POLICY: Option<&str> = option_env!("PAGE");

fn replacer() -> Option<Box<dyn Replacer + Send>> {
    match PAGE_POLICY {
        Some("clock") => Some(Box::new(Clock::new(CLOCK_FRAMES))),
        Some("ws") => Some(Box::new(WorkingSet::new(WORKING_SET_WINDOW))),
        Some("pff") => Some(Box::new(PageFaultFrequency::new(PFF_INTERVAL))),
        _ => None,
    }
}

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
/// address space
pub struct MemorySet {
    page_table: PageTable,
    areas: Vec<MapArea>,
    lazy: BTreeMap<VirtPageNum, LazyPage>,
    replacer: Option<Box<dyn Replacer + Send>>,
    /// timer ticks this address space has run for
    ticks: usize,
    /// paging statistics
    pub stats: PageStats,
}

/// A page created by mmap under a lazy policy.
struct LazyPage {
    perm: MapPermission,
    frame: Option<FrameTracker>,
    /// swap slot holding the content while the page is not dirty
    slot: Option<usize>,
}

/// Paging statistics of an address space
#[derive(Debug, Default)]
pub struct PageStats {
    /// page faults handled
    pub faults: usize,
    /// pages read from swap
    pub swap_ins: usize,
    /// pages evicted
    pub swap_outs: usize,
}

/// Resident lazy pages, viewed by the replacer.
struct Resident<'a> {
    page_table: &'a mut PageTable,
    lazy: &'a mut BTreeMap<VirtPageNum, LazyPage>,
}

impl Pages for Resident<'_> {
    fn flags(&mut self, vpn: usize) -> Option<Flags> {
        let vpn = VirtPageNum(vpn);
        self.lazy.get(&vpn)?.frame.as_ref()?;
        let pte = self.page_table.translate(vpn)?;
        Some(Flags {
            accessed: pte.accessed(),
            dirty: pte.dirty(),
        })
    }
    fn clear_accessed(&mut self, vpn: usize) {
        self.page_table.clear_flags(VirtPageNum(vpn), PTEFlags::A);
    }
    fn write_back(&mut self, vpn: usize) {
        let vpn = VirtPageNum(vpn);
        let page = self.lazy.get_mut(&vpn).unwrap();
        let bytes = page.frame.as_ref().unwrap().ppn.get_bytes_array();
        page.slot = Some(SWAP.exclusive_access().write(page.slot, bytes));
        self.page_table.clear_flags(vpn, PTEFlags::D);
    }
}

impl MemorySet {
    /// Create a new empty `MemorySet`.
    pub fn new_bare() -> Self {
        Self {
            page_table: PageTable::new(),
            areas: Vec::new(),
            lazy: BTreeMap::new(),
            replacer: replacer(),
            ticks: 0,
            stats: PageStats::default(),
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
        memory_set
    }
    /// Include sections in elf and trampoline and TrapContext and user stack,
    /// also returns user_sp_base and entry point.
    pub fn from_elf(elf_data: &[u8]) -> (Self, usize, usize) {
        let mut memory_set = Self::new_bare();
        // map trampoline
        memory_set.map_trampoline();
        // map program headers of elf, with U flag
        let elf = xmas_elf::ElfFile::new(elf_data).unwrap();
        let elf_header = elf.header;
        let magic = elf_header.pt1.magic;
        assert_eq!(magic, [0x7f, 0x45, 0x4c, 0x46], "invalid elf!");
        let ph_count = elf_header.pt2.ph_count();
        let mut max_end_vpn = VirtPageNum(0);
        for i in 0..ph_count {
            let ph = elf.program_header(i).unwrap();
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
                let map_area = MapArea::new(start_va, end_va, MapType::Framed, map_perm);
                max_end_vpn = map_area.vpn_range.get_end();
                memory_set.push(
                    map_area,
                    Some(&elf.input[ph.offset() as usize..(ph.offset() + ph.file_size()) as usize]),
                );
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
            elf.header.pt2.entry_point() as usize,
        )
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
    // 参考：rCore-Tutorial-Guide 第四章练习中 mmap/munmap 的定义，区间内已映射或存在未映射的页时失败
    /// map [start, end) for user, fail if any page is already mapped
    pub fn mmap(&mut self, start: VirtAddr, end: VirtAddr, perm: MapPermission) -> bool {
        let range = VPNRange::new(start.floor(), end.ceil());
        if range.into_iter().any(|vpn| {
            self.lazy.contains_key(&vpn) || self.translate(vpn).is_some_and(|pte| pte.is_valid())
        }) {
            return false;
        }
        if PAGE_POLICY.is_some() {
            for vpn in range {
                let page = LazyPage {
                    perm,
                    frame: None,
                    slot: None,
                };
                self.lazy.insert(vpn, page);
            }
        } else {
            self.insert_framed_area(start, end, perm);
        }
        true
    }
    /// unmap [start, end), fail if any page is not mapped for user
    pub fn munmap(&mut self, start: VirtAddr, end: VirtAddr) -> bool {
        let range = VPNRange::new(start.floor(), end.ceil());
        if !range.into_iter().all(|vpn| {
            self.lazy.contains_key(&vpn)
                || self
                    .translate(vpn)
                    .is_some_and(|pte| pte.is_valid() && pte.flags().contains(PTEFlags::U))
        }) {
            return false;
        }
        for vpn in range {
            if let Some(page) = self.lazy.remove(&vpn) {
                if page.frame.is_some() {
                    self.page_table.unmap(vpn);
                }
                if let Some(slot) = page.slot {
                    SWAP.exclusive_access().free(slot);
                }
            } else if let Some(area) = self
                .areas
                .iter_mut()
                .find(|area| area.data_frames.contains_key(&vpn))
            {
                area.unmap_one(&mut self.page_table, vpn);
            }
        }
        true
    }
    /// Make the lazy page at `va` resident if `access` is allowed.
    /// Returns false if the fault is not caused by lazy allocation or swap.
    pub fn handle_page_fault(&mut self, va: VirtAddr, access: MapPermission) -> bool {
        let vpn = va.floor();
        let Some(page) = self.lazy.get_mut(&vpn) else {
            return false;
        };
        if !page.perm.contains(access) {
            return false;
        }
        // 参考：RISC-V 特权级规范 Sv39 一节，A/D 位可由软件设置
        // the kernel accesses user pages by physical address, so set A/D for it
        let mut ad = PTEFlags::A;
        if access.contains(MapPermission::W) {
            ad |= PTEFlags::D;
        }
        if page.frame.is_some() {
            // with A/D already set, the fault is caused by something else, e.g. W without R
            if self.page_table.translate(vpn).unwrap().flags().contains(ad) {
                return false;
            }
            self.page_table.set_flags(vpn, ad);
            return true;
        }
        let Some(frame) = frame_alloc() else {
            return false;
        };
        if let Some(slot) = page.slot {
            SWAP.exclusive_access()
                .read(slot, frame.ppn.get_bytes_array());
            self.stats.swap_ins += 1;
        }
        let flags = PTEFlags::from_bits(page.perm.bits).unwrap() | ad;
        self.page_table.map(vpn, frame.ppn, flags);
        page.frame = Some(frame);
        self.stats.faults += 1;
        let now = self.ticks;
        self.replace(|replacer, pages| replacer.insert(vpn.0, now, pages));
        true
    }
    /// make the lazy pages of a user buffer resident before the kernel accesses it
    pub fn fault_in(&mut self, start: VirtAddr, end: VirtAddr, access: MapPermission) {
        for vpn in VPNRange::new(start.floor(), end.ceil()) {
            self.handle_page_fault(vpn.into(), access);
        }
    }
    /// called on each timer tick while this address space is running
    pub fn tick(&mut self) {
        self.ticks += 1;
        let now = self.ticks;
        self.replace(|replacer, pages| replacer.tick(now, pages));
    }
    fn replace(&mut self, f: impl FnOnce(&mut dyn Replacer, &mut dyn Pages) -> Vec<usize>) {
        let Some(replacer) = self.replacer.as_mut() else {
            return;
        };
        let mut resident = Resident {
            page_table: &mut self.page_table,
            lazy: &mut self.lazy,
        };
        let evicted = f(replacer.as_mut(), &mut resident);
        for vpn in evicted {
            self.swap_out(VirtPageNum(vpn));
        }
    }
    fn swap_out(&mut self, vpn: VirtPageNum) {
        let page = self.lazy.get_mut(&vpn).unwrap();
        let frame = page.frame.take().unwrap();
        // a clean page is either all zero or still the same as its swap slot
        if self.page_table.translate(vpn).unwrap().dirty() {
            let slot = SWAP
                .exclusive_access()
                .write(page.slot, frame.ppn.get_bytes_array());
            page.slot = Some(slot);
        }
        self.page_table.unmap(vpn);
        self.stats.swap_outs += 1;
    }
}

impl Drop for MemorySet {
    fn drop(&mut self) {
        for slot in self.lazy.values().filter_map(|page| page.slot) {
            SWAP.exclusive_access().free(slot);
        }
    }
}
/// map area structure, controls a contiguous piece of virtual memory
pub struct MapArea {
    vpn_range: VPNRange,
    data_frames: BTreeMap<VirtPageNum, FrameTracker>,
    map_type: MapType,
    map_perm: MapPermission,
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
                self.data_frames.insert(vpn, frame);
            }
        }
        let pte_flags = PTEFlags::from_bits(self.map_perm.bits).unwrap();
        page_table.map(vpn, ppn, pte_flags);
    }
    #[allow(unused)]
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
    #[allow(unused)]
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

/// Return (bottom, top) of a kernel stack in kernel space.
pub fn kernel_stack_position(app_id: usize) -> (usize, usize) {
    let top = TRAMPOLINE - app_id * (KERNEL_STACK_SIZE + PAGE_SIZE);
    let bottom = top - KERNEL_STACK_SIZE;
    (bottom, top)
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
