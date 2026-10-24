#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{get_time, set_priority, spawn, waitpid, yield_};

static TESTS: &[&str] = &[
    "ch5_stride0\0",
    "ch5_stride1\0",
    "ch5_stride2\0",
    "ch5_stride3\0",
    "ch5_stride4\0",
    "ch5_stride5\0",
];

/// Six CPU-bound children with priority 5..10 run for 4 s each, while the
/// parent (priority 4) keeps yielding and measures how soon it gets the CPU back.
#[no_mangle]
fn main() -> i32 {
    let mut pids = [0; 6];
    for (pid, test) in pids.iter_mut().zip(TESTS) {
        *pid = spawn(test);
        assert!(*pid > 0);
    }
    set_priority(4);
    let start = get_time();
    let (mut yields, mut max_gap, mut last) = (0, 0, start);
    while last - start < 4000 {
        yield_();
        let now = get_time();
        max_gap = max_gap.max(now - last);
        last = now;
        yields += 1;
    }
    for &pid in pids.iter() {
        let mut code = 0;
        assert_eq!(waitpid(pid as usize, &mut code), pid);
    }
    println!("interactive: yields = {}, max gap = {} ms", yields, max_gap);
    println!("Test sched OK!");
    0
}
