use page_replace::{Clock, Flags, PageFaultFrequency, Pages, Replacer, WorkingSet};
use std::collections::BTreeMap;

/// Resident pages with A/D bits, as an MMU would maintain them.
#[derive(Default)]
struct Mmu {
    pages: BTreeMap<usize, Flags>,
    written_back: Vec<usize>,
}

impl Mmu {
    fn touch(&mut self, vpn: usize, write: bool) {
        let flags = self.pages.get_mut(&vpn).unwrap();
        flags.accessed = true;
        flags.dirty |= write;
    }
}

impl Pages for Mmu {
    fn flags(&mut self, vpn: usize) -> Option<Flags> {
        self.pages.get(&vpn).copied()
    }
    fn clear_accessed(&mut self, vpn: usize) {
        self.pages.get_mut(&vpn).unwrap().accessed = false;
    }
    fn write_back(&mut self, vpn: usize) {
        self.pages.get_mut(&vpn).unwrap().dirty = false;
        self.written_back.push(vpn);
    }
}

/// Access `vpn` like a process would: fault it in if needed, then touch it.
/// Returns whether a page fault happened.
fn access(r: &mut dyn Replacer, mmu: &mut Mmu, vpn: usize, write: bool, now: usize) -> bool {
    let fault = !mmu.pages.contains_key(&vpn);
    if fault {
        mmu.pages.insert(
            vpn,
            Flags {
                accessed: false,
                dirty: false,
            },
        );
        for victim in r.insert(vpn, now, mmu) {
            assert_ne!(victim, vpn);
            mmu.pages.remove(&victim).unwrap();
        }
    }
    mmu.touch(vpn, write);
    fault
}

fn tick(r: &mut dyn Replacer, mmu: &mut Mmu, now: usize) {
    for victim in r.tick(now, mmu) {
        mmu.pages.remove(&victim).unwrap();
    }
}

fn resident(mmu: &Mmu) -> Vec<usize> {
    mmu.pages.keys().copied().collect()
}

#[test]
fn clock_keeps_frame_limit() {
    let (mut clock, mut mmu) = (Clock::new(3), Mmu::default());
    for vpn in 0..20 {
        access(&mut clock, &mut mmu, vpn % 7, vpn % 2 == 0, 0);
        assert!(mmu.pages.len() <= 3);
    }
}

#[test]
fn clock_prefers_clean_unused_pages() {
    let (mut clock, mut mmu) = (Clock::new(3), Mmu::default());
    access(&mut clock, &mut mmu, 1, true, 0);
    access(&mut clock, &mut mmu, 2, false, 0);
    access(&mut clock, &mut mmu, 3, true, 0);
    // first sweep clears all A bits; second sweep writes back page 1,
    // then finds page 2 clean and unused
    assert!(access(&mut clock, &mut mmu, 4, false, 0));
    assert_eq!(resident(&mmu), [1, 3, 4]);
    assert_eq!(mmu.written_back, [1]);
}

#[test]
fn clock_gives_referenced_pages_a_second_chance() {
    let (mut clock, mut mmu) = (Clock::new(3), Mmu::default());
    for vpn in 1..=3 {
        access(&mut clock, &mut mmu, vpn, false, 0);
    }
    access(&mut clock, &mut mmu, 4, false, 0); // evicts 1, all A bits now clear
    assert_eq!(resident(&mmu), [2, 3, 4]);
    access(&mut clock, &mut mmu, 2, false, 0); // hit, sets A of page 2
    access(&mut clock, &mut mmu, 5, false, 0);
    assert_eq!(resident(&mmu), [2, 4, 5]);
}

#[test]
fn clock_skips_unmapped_pages() {
    let (mut clock, mut mmu) = (Clock::new(2), Mmu::default());
    access(&mut clock, &mut mmu, 1, false, 0);
    access(&mut clock, &mut mmu, 2, false, 0);
    mmu.pages.remove(&1); // munmap
    access(&mut clock, &mut mmu, 3, false, 0);
    assert_eq!(resident(&mmu), [2, 3]);
}

#[test]
fn working_set_drops_pages_outside_window() {
    let (mut ws, mut mmu) = (WorkingSet::new(2), Mmu::default());
    for vpn in 0..4 {
        access(&mut ws, &mut mmu, vpn, false, 0);
    }
    for now in 1..=4 {
        access(&mut ws, &mut mmu, 0, false, now);
        tick(&mut ws, &mut mmu, now);
    }
    // pages 1..4 were last used at tick 1
    assert_eq!(resident(&mmu), [0]);
}

#[test]
fn working_set_follows_phase_change() {
    let (mut ws, mut mmu) = (WorkingSet::new(3), Mmu::default());
    let mut faults = 0;
    for now in 0..20 {
        let phase = if now < 10 { 0..4 } else { 10..14 };
        for vpn in phase {
            faults += access(&mut ws, &mut mmu, vpn, true, now) as usize;
        }
        tick(&mut ws, &mut mmu, now + 1);
    }
    assert_eq!(faults, 8);
    assert_eq!(resident(&mmu), [10, 11, 12, 13]);
}

#[test]
fn pff_grows_when_faults_are_frequent() {
    let (mut pff, mut mmu) = (PageFaultFrequency::new(2), Mmu::default());
    for vpn in 0..8 {
        access(&mut pff, &mut mmu, vpn, false, vpn / 4);
    }
    assert_eq!(mmu.pages.len(), 8);
}

#[test]
fn pff_shrinks_after_a_quiet_period() {
    let (mut pff, mut mmu) = (PageFaultFrequency::new(2), Mmu::default());
    for vpn in 0..6 {
        access(&mut pff, &mut mmu, vpn, false, 0);
    }
    // the fault of page 5 cleared the A bits of 0..5; afterwards 0, 1 and 5 are used
    for now in 1..5 {
        access(&mut pff, &mut mmu, 0, false, now);
        access(&mut pff, &mut mmu, 1, false, now);
    }
    access(&mut pff, &mut mmu, 9, false, 5);
    assert_eq!(resident(&mmu), [0, 1, 5, 9]);
}
