//! The panic handler and backtrace

use crate::console::print_in_panic;
use crate::sbi::shutdown;
use crate::task::current_kstack_top;
use core::arch::asm;
use core::panic::PanicInfo;

#[panic_handler]
/// panic handler
fn panic(info: &PanicInfo) -> ! {
    if let Some(location) = info.location() {
        print_in_panic(format_args!(
            "[kernel] Panicked at {}:{} {}\n",
            location.file(),
            location.line(),
            info.message().unwrap()
        ));
    } else {
        print_in_panic(format_args!(
            "[kernel] Panicked: {}\n",
            info.message().unwrap()
        ));
    }
    // unsafe {
    //     backtrace();
    // }
    shutdown()
}
/// backtrace function
#[allow(unused)]
unsafe fn backtrace() {
    let mut fp: usize;
    let stop = current_kstack_top();
    asm!("mv {}, s0", out(reg) fp);
    println!("---START BACKTRACE---");
    for i in 0..10 {
        if fp == stop {
            break;
        }
        println!("#{}:ra={:#x}", i, *((fp - 8) as *const usize));
        fp = *((fp - 16) as *const usize);
    }
    println!("---END   BACKTRACE---");
}
