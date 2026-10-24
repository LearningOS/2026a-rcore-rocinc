#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{exit, fork, mmap, sleep, thread_create, waitpid, waittid};

const START: usize = 0x10000000;
const PAGE: usize = 4096;
const PAGES: usize = 256;
const THREADS: usize = 4;

fn word(page: usize, t: usize) -> *mut usize {
    (START + page * PAGE + t * 8) as *mut usize
}

fn spinner(_: usize) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// every thread writes its own word of every page, so they fault on the same pages at once
fn writer(t: usize) -> ! {
    for p in 0..PAGES {
        unsafe { word(p, t).write_volatile(p * THREADS + t) };
    }
    exit(0)
}

#[no_mangle]
fn main() -> i32 {
    // the main thread exits while the other threads keep running
    let pid = fork();
    if pid == 0 {
        for _ in 0..THREADS {
            thread_create(spinner as usize, 0);
        }
        sleep(100);
        exit(3);
    }
    let mut code = -1;
    assert_eq!(waitpid(pid as usize, &mut code), pid);
    assert_eq!(code, 3);
    println!("threads: running threads stop with the process");

    // threads write pages shared copy-on-write with a child process
    assert_eq!(mmap(START, PAGES * PAGE, 3), 0);
    for p in 0..PAGES {
        for t in 0..THREADS {
            unsafe { word(p, t).write_volatile(usize::MAX) };
        }
    }
    let child = fork();
    if child == 0 {
        sleep(500);
        let ok = (0..PAGES)
            .all(|p| (0..THREADS).all(|t| unsafe { word(p, t).read_volatile() } == usize::MAX));
        exit(if ok { 0 } else { 1 });
    }
    let mut tids = [0; THREADS];
    for (t, tid) in tids.iter_mut().enumerate() {
        *tid = thread_create(writer as usize, t) as usize;
    }
    for tid in tids {
        assert_eq!(waittid(tid), 0);
    }
    for p in 0..PAGES {
        for t in 0..THREADS {
            assert_eq!(unsafe { word(p, t).read_volatile() }, p * THREADS + t);
        }
    }
    assert_eq!(waitpid(child as usize, &mut code), child);
    assert_eq!(code, 0);
    println!("threads: copy-on-write faults from several threads");
    println!("Test threads OK!");
    0
}
