#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    close, exit, fork, get_time, open, read, spawn, unlink, waitpid, write, OpenFlags,
};

const PAGE: usize = 4096;
/// 2 MiB of initialized data, loaded from the ELF file
const PAGES: usize = 512;
static mut BIG: [u8; PAGES * PAGE] = [0x5a; PAGES * PAGE];

const FLAG: &str = "ch6x_lazy_flag\0";
const TMP: &str = "ch6x_lazy_tmp\0";
const ROUNDS: usize = 10;

fn big() -> &'static mut [u8] {
    unsafe { &mut *core::ptr::addr_of_mut!(BIG) }
}

#[no_mangle]
fn main() -> i32 {
    // spawned by itself: touch one page only
    let fd = open(FLAG, OpenFlags::RDONLY);
    if fd >= 0 {
        close(fd as usize);
        return big()[PAGES / 2 * PAGE] as i32;
    }

    let fd = open(FLAG, OpenFlags::CREATE | OpenFlags::WRONLY);
    close(fd as usize);
    let start = get_time();
    for _ in 0..ROUNDS {
        let pid = spawn("ch6x_lazy\0");
        let mut code = 0;
        assert_eq!(waitpid(pid as usize, &mut code), pid);
        assert_eq!(code, 0x5a);
    }
    println!(
        "lazy: {} x (spawn + exit + waitpid) of a 2 MiB program took {} ms",
        ROUNDS,
        get_time() - start
    );
    assert_eq!(unlink(FLAG), 0);

    // the kernel reads pages not loaded yet
    let fd = open(TMP, OpenFlags::CREATE | OpenFlags::WRONLY) as usize;
    assert_eq!(write(fd, &big()[100 * PAGE..103 * PAGE]), 3 * PAGE as isize);
    close(fd);
    let fd = open(TMP, OpenFlags::RDONLY) as usize;
    // and writes into them, keeping the rest of the page from the file
    assert_eq!(read(fd, &mut big()[200 * PAGE..201 * PAGE + 10]), PAGE as isize + 10);
    close(fd);
    assert_eq!(unlink(TMP), 0);
    assert!(big()[200 * PAGE..202 * PAGE].iter().all(|&b| b == 0x5a));
    println!("lazy: kernel accesses load the pages");

    // after fork, the child loads the pages untouched by the parent from the file
    big()[0] = 1;
    let pid = fork();
    if pid == 0 {
        let ok = big()[0] == 1 && big()[300 * PAGE] == 0x5a && big()[PAGES * PAGE - 1] == 0x5a;
        big()[300 * PAGE] = 2;
        exit(ok as i32);
    }
    let mut code = 0;
    assert_eq!(waitpid(pid as usize, &mut code), pid);
    assert_eq!(code, 1);
    assert_eq!(big()[300 * PAGE], 0x5a);
    println!("Test lazy OK!");
    0
}
