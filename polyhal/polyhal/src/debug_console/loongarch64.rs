use ns16550a::Uart;
use spin::Mutex;

use super::DebugConsole;

#[cfg(not(board = "2k1000"))]
const UART_ADDR: usize = 0x01FE001E0 | crate::arch::consts::VIRT_ADDR_START;
// const UART_ADDR: usize = 0x800000001fe20000;

#[cfg(board = "2k1000")]

const UART_ADDR: usize = 0x800000001fe20000;
// 0x800000001fe20000ULL
static COM1: Mutex<Uart> = Mutex::new(Uart::new(UART_ADDR));

impl DebugConsole {
    /// Writes a byte to the console.
    #[inline]
    pub fn putchar(ch: u8) {
        if ch == b'\n' {
            Self::putchar(b'\r');
        }

        let uart = COM1.lock();
        while uart.put(ch).is_none() {
            core::hint::spin_loop();
        }
    }

    /// read a byte, return -1 if nothing exists.
    #[inline]
    pub fn getchar() -> Option<u8> {
        COM1.lock().get()
    }
}
