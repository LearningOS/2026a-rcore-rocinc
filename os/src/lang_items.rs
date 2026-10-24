//! The panic handler

use crate::console::print_in_panic;
use crate::sbi::shutdown;
use core::panic::PanicInfo;

#[panic_handler]
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
    shutdown()
}
