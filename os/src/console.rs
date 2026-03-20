//! SBI console driver, for text output
use crate::sbi::console_putchar;
use core::fmt::{self, Write};
use lazy_static::*;
use spin::Mutex;
lazy_static! {
    pub static ref CONSOLE_LOCK: Mutex<()> = Mutex::new(());
}
struct Stdout;

impl Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            console_putchar(c as usize);
        }
        Ok(())
    }
}

pub fn print(args: fmt::Arguments) {
    let _guard = CONSOLE_LOCK.lock();
    Stdout.write_fmt(args).unwrap();
    //CONSOLE_LOCK.unlock();
}

#[macro_export]
/// print string macro
macro_rules! print {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::print(format_args!($fmt $(, $($arg)+)?));
    }
}

#[macro_export]
/// println string macro
macro_rules! println {
    ($fmt: literal $(, $($arg: tt)+)?) => {
        $crate::console::print(format_args!(concat!($fmt, "\n") $(, $($arg)+)?));
    }
}
