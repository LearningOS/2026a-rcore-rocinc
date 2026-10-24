#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{exit, fork, get_time, waitpid};

const WORKERS: usize = 8;

fn work() -> usize {
    let mut x = 1usize;
    for i in 0..300_000_000usize {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(i);
    }
    x
}

/// The same CPU-bound work in 8 processes; compare the elapsed time under different SMP.
#[no_mangle]
fn main() -> i32 {
    let start = get_time();
    let mut pids = [0; WORKERS];
    for pid in pids.iter_mut() {
        *pid = fork();
        if *pid == 0 {
            exit((work() & 1) as i32);
        }
    }
    let mut first = -1;
    for &pid in pids.iter() {
        let mut code = 0;
        assert_eq!(waitpid(pid as usize, &mut code), pid);
        if first == -1 {
            first = code;
        }
        assert_eq!(code, first);
    }
    println!("smp: {} workers took {} ms", WORKERS, get_time() - start);
    println!("Test smp OK!");
    0
}
