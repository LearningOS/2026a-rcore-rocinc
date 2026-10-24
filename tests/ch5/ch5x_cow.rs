#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{exit, fork, get_time, mmap, sleep, sys_get_time, waitpid, TimeVal};

const PAGE: usize = 4096;
const START: usize = 0x10000000;
/// 20 MiB; seven copies would not fit in QEMU's 128 MiB without copy-on-write
const LEN: usize = 20 << 20;
const CHILDREN: usize = 6;

static mut EXIT_CODE: i32 = 0x5a5a;
static mut TIME: TimeVal = TimeVal { sec: 0, usec: 0 };

fn page(i: usize) -> *mut usize {
    (START + i * PAGE) as *mut usize
}

#[no_mangle]
fn main() -> i32 {
    assert_eq!(mmap(START, LEN, 3), 0);
    let pages = LEN / PAGE;
    for i in 0..pages {
        unsafe { *page(i) = i };
    }

    let start = get_time();
    for _ in 0..10 {
        let pid = fork();
        if pid == 0 {
            exit(0);
        }
        let mut code = -1;
        assert_eq!(waitpid(pid as usize, &mut code), pid);
    }
    println!("cow: 10 x (fork + exit + waitpid) took {} ms", get_time() - start);

    let mut pids = [0; CHILDREN];
    for (k, pid) in pids.iter_mut().enumerate() {
        *pid = fork();
        if *pid == 0 {
            for i in 0..pages {
                assert_eq!(unsafe { *page(i) }, i);
            }
            unsafe { *page(k) = usize::MAX - k };
            assert_eq!(unsafe { *page(k) }, usize::MAX - k);
            // stay alive until all children exist
            sleep(500);
            exit(k as i32);
        }
    }
    // the parent writes while the children still share the page
    unsafe { *page(pages - 1) = 0 };
    for (k, &pid) in pids.iter().enumerate() {
        let mut code = 0;
        assert_eq!(waitpid(pid as usize, &mut code), pid);
        assert_eq!(code, k as i32);
    }
    for i in 0..pages - 1 {
        assert_eq!(unsafe { *page(i) }, i);
    }
    println!("cow: {} children shared {} pages", CHILDREN, pages);

    // the kernel writes into pages shared with a living child
    let watcher = fork();
    if watcher == 0 {
        sleep(300);
        let ok = unsafe { EXIT_CODE == 0x5a5a && TIME.sec == 0 && TIME.usec == 0 };
        let mut t = TimeVal::new();
        assert_eq!(sys_get_time(&mut t, 0), 0);
        exit(if ok { 0 } else { 1 });
    }
    let quick = fork();
    if quick == 0 {
        exit(7);
    }
    unsafe {
        assert_eq!(waitpid(quick as usize, &mut *core::ptr::addr_of_mut!(EXIT_CODE)), quick);
        assert_eq!(EXIT_CODE, 7);
        assert_eq!(sys_get_time(&mut *core::ptr::addr_of_mut!(TIME), 0), 0);
        assert!(TIME.sec + TIME.usec > 0);
    }
    let mut code = -1;
    assert_eq!(waitpid(watcher as usize, &mut code), watcher);
    assert_eq!(code, 0, "kernel wrote into a shared page");
    println!("cow: kernel writes are private");

    // a read-only page stays read-only after fork
    assert_eq!(mmap(START + LEN, PAGE, 1), 0);
    let child = fork();
    if child == 0 {
        unsafe { *((START + LEN) as *mut usize) = 1 };
        exit(0);
    }
    assert_eq!(waitpid(child as usize, &mut code), child);
    assert_eq!(code, -2);
    println!("Test cow OK!");
    0
}
