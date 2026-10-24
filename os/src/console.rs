//! SBI console driver, for text output
use crate::sbi::console_putchar;
use crate::sync::UPSafeCell;
use core::fmt::{self, Write};

struct Stdout;

impl Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            console_putchar(c as usize);
        }
        Ok(())
    }
}

/// keeps the output of different harts from interleaving
static STDOUT: UPSafeCell<Stdout> = unsafe { UPSafeCell::new(Stdout) };

pub fn print(args: fmt::Arguments) {
    STDOUT.exclusive_access().write_fmt(args).unwrap();
}

/// print for the panic handler, which may be called inside `print`
pub fn print_in_panic(args: fmt::Arguments) {
    if STDOUT.accessed_here() {
        Stdout.write_fmt(args).unwrap();
    } else {
        print(args);
    }
}

/// Print! to the host console using the format string and arguments.
#[macro_export]
macro_rules! print {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::print(format_args!($fmt $(, $($arg)+)?))
    }
}

/// Println! to the host console using the format string and arguments.
#[macro_export]
macro_rules! println {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::print(format_args!(concat!($fmt, "\n") $(, $($arg)+)?))
    }
}
