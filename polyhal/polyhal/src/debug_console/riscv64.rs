use crate::debug_console::DebugConsole;

#[cfg(board = "visionfive2")]
mod visionfive2_uart {
    use crate::consts::VIRT_ADDR_START;

    const UART0_BASE: usize = VIRT_ADDR_START + 0x1000_0000;
    const UART_RBR: usize = 0;
    const UART_LSR: usize = 5;
    const UART_REG_SHIFT: usize = 2;
    const UART_LSR_DR: u8 = 1 << 0;

    #[inline]
    fn reg(offset: usize) -> *mut u8 {
        (UART0_BASE + (offset << UART_REG_SHIFT)) as *mut u8
    }

    #[inline]
    pub fn getchar() -> Option<u8> {
        unsafe {
            if reg(UART_LSR).read_volatile() & UART_LSR_DR == 0 {
                return None;
            }
            Some(reg(UART_RBR).read_volatile())
        }
    }
}

/// Debug console function.
impl DebugConsole {
    #[inline]
    #[cfg(board = "visionfive2")]
    #[allow(deprecated)]
    pub fn putchar(ch: u8) {
        sbi_rt::legacy::console_putchar(ch as _);
    }

    #[inline]
    #[cfg(not(board = "visionfive2"))]
    #[allow(deprecated)]
    pub fn putchar(ch: u8) {
        sbi_rt::legacy::console_putchar(ch as _);
    }

    #[inline]
    #[cfg(board = "visionfive2")]
    pub fn getchar() -> Option<u8> {
        visionfive2_uart::getchar()
    }

    #[inline]
    #[cfg(not(board = "visionfive2"))]
    #[allow(deprecated)]
    pub fn getchar() -> Option<u8> {
        let c = sbi_rt::legacy::console_getchar() as u8;
        match c == u8::MAX {
            true => None,
            _ => Some(c),
        }
    }
}
