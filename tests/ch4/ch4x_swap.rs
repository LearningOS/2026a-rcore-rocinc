#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{get_time, mmap, munmap};

const PAGE: usize = 4096;
const START: usize = 0x10000000;
const PAGES: usize = 64;
const EXTRA: usize = 2;

fn page(i: usize) -> *mut usize {
    (START + i * PAGE) as *mut usize
}

/// Keeps writing pages `first..first + n` for `ms` milliseconds.
fn work(first: usize, n: usize, ms: isize) {
    let end = get_time() + ms;
    while get_time() < end {
        for i in first..first + n {
            unsafe { *page(i) = i + first };
        }
    }
}

/// A loop over more pages than the clock frames, then two phases with
/// different working sets, so that every policy swaps pages out and in.
/// The extra pages fault right before and after the phases, letting PFF
/// drop the pages unused in between.
#[no_mangle]
fn main() -> i32 {
    assert_eq!(mmap(START, (PAGES + EXTRA) * PAGE, 3), 0);
    for round in 1..4 {
        for i in 0..PAGES {
            unsafe { *page(i) = i * round };
        }
        for i in 0..PAGES {
            assert_eq!(unsafe { *page(i) }, i * round);
        }
    }
    unsafe { *page(PAGES) = 1 };
    work(0, 8, 100);
    work(8, 8, 200);
    unsafe { *page(PAGES + 1) = 2 };
    for i in 0..PAGES {
        let expect = match i {
            0..=7 => i,
            8..=15 => i + 8,
            _ => i * 3,
        };
        assert_eq!(unsafe { *page(i) }, expect);
    }
    assert_eq!(unsafe { *page(PAGES) + *page(PAGES + 1) }, 3);
    assert_eq!(munmap(START, (PAGES + EXTRA) * PAGE), 0);
    println!("Test swap OK!");
    0
}
