#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{mmap, munmap, sys_get_time, trace_read, trace_write, TimeVal};

const PAGE: usize = 4096;

/// Maps 256 MiB, more than the physical memory of QEMU (128 MiB),
/// so it only works with lazy allocation.
#[no_mangle]
fn main() -> i32 {
    let start = 0x10000000;
    let len = 256 << 20;
    assert_eq!(mmap(start, len, 3), 0);
    assert_eq!(mmap(start + len - PAGE, PAGE, 3), -1);
    for i in (0..len).step_by(len / 16) {
        let p = (start + i) as *mut usize;
        unsafe {
            assert_eq!(*p, 0);
            *p = i;
        }
    }
    for i in (0..len).step_by(len / 16) {
        assert_eq!(unsafe { *((start + i) as *const usize) }, i);
    }
    // the kernel writes into a page that has never been touched
    let ts = (start + PAGE * 7 - 8) as *mut TimeVal;
    assert_eq!(sys_get_time(unsafe { &mut *ts }, 0), 0);
    assert!(unsafe { (*ts).sec + (*ts).usec } > 0);
    let p = (start + PAGE * 9) as *const u8;
    assert_eq!(trace_read(p), Some(0));
    assert_eq!(trace_write(p, 7), 0);
    assert_eq!(trace_read(p), Some(7));

    assert_eq!(mmap(start + len, PAGE, 1), 0);
    assert_eq!(trace_write((start + len) as *const u8, 1), -1);
    assert_eq!(munmap(start, len + PAGE), 0);
    assert_eq!(trace_read(p), None);
    assert_eq!(munmap(start, PAGE), -1);
    println!("Test lazy OK!");
    0
}
