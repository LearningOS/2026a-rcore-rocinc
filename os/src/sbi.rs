//! SBI call wrappers

#![allow(unused)]

use core::arch::asm;
/// set timer sbi call id
const SBI_SET_TIMER: usize = 0;
/// console putchar sbi call id
const SBI_CONSOLE_PUTCHAR: usize = 1;
/// console getchar sbi call id
const SBI_CONSOLE_GETCHAR: usize = 2;
/// shutdown sbi call id
const SBI_SYSTEM_RESET: usize = 0x53525354;
/// hart state management extension id
const SBI_HSM: usize = 0x48534D;
/// remote fence extension id
const SBI_RFENCE: usize = 0x52464E43;

/// general sbi call
#[inline(always)]
fn sbi_call(which: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
    let mut ret;
    unsafe {
        asm!(
            "ecall",     // sbi call
            inlateout("x10") arg0 => ret, // sbi call arg0 and return value
            in("x11") arg1, // sbi call arg1
            in("x12") arg2, // sbi call arg2
            in("x16") 0, // for sbi call id args need 2 reg (x16, x17)
            in("x17") which,// sbi call id
        );
    }
    ret
}

/// use sbi call to set timer
pub fn set_timer(timer: usize) {
    sbi_call(SBI_SET_TIMER, timer, 0, 0);
}

/// use sbi call to putchar in console (qemu uart handler)
pub fn console_putchar(c: usize) {
    sbi_call(SBI_CONSOLE_PUTCHAR, c, 0, 0);
}

/// use sbi call to getchar from console (qemu uart handler)
pub fn console_getchar() -> usize {
    let ch = sbi_call(SBI_CONSOLE_GETCHAR, 0, 0, 0);
    // Legacy SBI returns -1 when no input is available; readers expect zero.
    if ch == usize::MAX {
        0
    } else {
        ch
    }
}

/// start a stopped hart at `start_addr` in S-mode with a0 = hartid, return false if it fails
pub fn hart_start(hartid: usize, start_addr: usize) -> bool {
    let error: isize;
    unsafe {
        asm!(
            "ecall",
            in("a7") SBI_HSM,
            in("a6") 0,
            inlateout("a0") hartid => error,
            inlateout("a1") start_addr => _,
            in("a2") 0,
        );
    }
    error == 0
}

// 参考：RISC-V SBI 规范 RFENCE 扩展中 sbi_remote_sfence_vma 的参数，目标 hart 须处于 STARTED 状态
/// flush the TLB entries of [start, start + size) on the harts in `hart_mask`
pub fn remote_sfence_vma(hart_mask: usize, start: usize, size: usize) {
    let error: isize;
    unsafe {
        asm!(
            "ecall",
            in("a7") SBI_RFENCE,
            in("a6") 1,
            inlateout("a0") hart_mask => error,
            inlateout("a1") 0usize => _,
            in("a2") start,
            in("a3") size,
        );
    }
    assert_eq!(error, 0);
}

/// use sbi call to shutdown the kernel
pub fn shutdown() -> ! {
    // SBI System Reset: shutdown (type 0), no failure reason (reason 0).
    unsafe {
        asm!(
            "ecall",
            in("a7") SBI_SYSTEM_RESET,
            in("a6") 0,
            inlateout("a0") 0usize => _,
            inlateout("a1") 0usize => _,
        );
    }
    panic!("It should shutdown!");
}
