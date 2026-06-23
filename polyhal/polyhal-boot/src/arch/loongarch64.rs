use core::arch::naked_asm;
use core::hint::spin_loop;
use core::sync::atomic::AtomicBool;
use loongArch64::register::euen;
use polyhal::percpu::set_local_thread_pointer;
use polyhal::{
    consts::QEMU_DTB_ADDR,
    ctor::{ph_init_iter, CtorType},
    hart_id,
    mem::{init_dtb_once, parse_system_info},
};

/// Signal that primary core has completed initialization
static INIT_DONE: AtomicBool = AtomicBool::new(false);

fn early_uart_puts(s: &str) {
    const UART_BASE: usize = 0x8000_0000_1fe2_0000;

    for byte in s.bytes() {
        if byte == b'\n' {
            early_uart_put_byte(b'\r');
        }
        early_uart_put_byte(byte);
    }

    fn early_uart_put_byte(byte: u8) {
        const UART_BASE: usize = 0x8000_0000_1fe2_0000;
        let thr = UART_BASE as *mut u8;
        let lsr = (UART_BASE + 5) as *const u8;

        for _ in 0..10_000 {
            if unsafe { lsr.read_volatile() } & 0x20 != 0 {
                break;
            }
        }
        unsafe {
            thr.write_volatile(byte);
        }
    }
}

macro_rules! init_dwm {
    () => {
        "
        ori         $t0, $zero, 0x1     # CSR_DMW1_PLV0
        lu52i.d     $t0, $t0, -2048     # UC, PLV0, 0x8000 xxxx xxxx xxxx
        csrwr       $t0, 0x180          # LOONGARCH_CSR_DMWIN0
        ori         $t0, $zero, 0x11    # CSR_DMW1_MAT | CSR_DMW1_PLV0
        lu52i.d     $t0, $t0, -1792     # CA, PLV0, 0x9000 xxxx xxxx xxxx
        csrwr       $t0, 0x181          # LOONGARCH_CSR_DMWIN1
        // ori         $t0, $zero, 0x13
        // lu52i.d     $t0, $t0, 0x0000          # 虚拟地址高位为 0x0
        // csrwr       $t0, 0x182                # LOONGARCH_CSR_DMWIN2
        "
    };
}

/// The earliest entry point for the primary CPU.
///
/// We can't use bl to jump to higher address, so we use jirl to jump to higher address.
#[naked]
#[no_mangle]
#[link_section = ".text.entry"]
unsafe extern "C" fn _start() -> ! {
    naked_asm!(
        init_dwm!(),
        
        "
        
        # Enable PG
        li.w        $t0, 0xb0       # PLV=0, IE=0, PG=1
        csrwr       $t0, 0x0        # LOONGARCH_CSR_CRMD
        li.w        $t0, 0x00       # PLV=0, PIE=0, PWE=0
        csrwr       $t0, 0x1        # LOONGARCH_CSR_PRMD
        li.w        $t0, 0x00       # FPE=0, SXE=0, ASXE=0, BTE=0
        csrwr       $t0, 0x2        # LOONGARCH_CSR_EUEN
        
        la.global   $sp, bstack_top
        csrrd       $a0, 0x20           # cpuid
        la.global   $t0, {entry}
        jirl        $zero,$t0,0
        ",
        entry = sym rust_tmp_main,
    )
}

/// The earliest entry point for the primary CPU.
///
/// We can't use bl to jump to higher address, so we use jirl to jump to higher address.
#[naked]
#[no_mangle]
unsafe extern "C" fn _secondary_start() -> ! {
    naked_asm!(
        init_dwm!(),
        "
        # Enable PG
        li.w        $t0, 0xb0       # PLV=0, IE=0, PG=1
        csrwr       $t0, 0x0        # LOONGARCH_CSR_CRMD
        li.w        $t0, 0x00       # PLV=0, PIE=0, PWE=0
        csrwr       $t0, 0x1        # LOONGARCH_CSR_PRMD
        li.w        $t0, 0x00       # FPE=0, SXE=0, ASXE=0, BTE=0
        csrwr       $t0, 0x2        # LOONGARCH_CSR_EUEN
        
        # Load Stack Pointer From Message Buffer
        li.w         $t0, {MBUF1}
        iocsrrd.d    $sp, $t0

        csrrd        $a0, 0x20                  # cpuid
        la.global    $t0, {entry}

        jirl         $zero, $t0, 0
        ",
        MBUF1 = const loongArch64::consts::LOONGARCH_CSR_MAIL_BUF1,
        entry = sym _rust_secondary_main,
    )
}

/// Rust temporary entry point
///
/// This function will be called after assembly boot stage.
pub fn rust_tmp_main(hart_id: usize) {
    early_uart_puts("Kairix: enter rust_tmp_main\n");
    super::clear_bss();
    early_uart_puts("Kairix: clear_bss done\n");
    if init_dtb_once(QEMU_DTB_ADDR).is_err() {
        early_uart_puts("Kairix: init_dtb_once failed, use 2K1000 fallback mem\n");
        unsafe {
            polyhal::mem::add_memory_region(0x0020_0000, 0x0f00_0000);
            polyhal::mem::add_memory_region(0x9000_0000, 0x1_0000_0000);
        }
    }
    early_uart_puts("Kairix: init_dtb_once done\n");
    set_local_thread_pointer(hart_id);
    early_uart_puts("Kairix: set tp done\n");

    // Initialize CPU Configuration.
    init_cpu();
    early_uart_puts("Kairix: init_cpu done\n");
    ph_init_iter(CtorType::Cpu).for_each(|x| (x.func)());
    early_uart_puts("Kairix: cpu ctors done\n");

    parse_system_info();
    early_uart_puts("Kairix: parse_system_info done\n");
    ph_init_iter(CtorType::Platform).for_each(|x| (x.func)());
    early_uart_puts("Kairix: platform ctors done\n");
    ph_init_iter(CtorType::HALDriver).for_each(|x| (x.func)());
    early_uart_puts("Kairix: hal driver ctors done\n");

    // Signal secondary cores that initialization is complete
    INIT_DONE.store(true, core::sync::atomic::Ordering::SeqCst);

    early_uart_puts("Kairix: call_real_main\n");
    super::call_real_main(hart_id);
}

/// Initialize CPU Configuration.
fn init_cpu() {
    // Enable floating point
    euen::set_fpe(true);

    // Initialzie Timer
    // timer::init_timer();
}

/// The entry point for the second core.
pub(crate) extern "C" fn _rust_secondary_main() {
    // Wait for primary core to complete initialization
    while !INIT_DONE.load(core::sync::atomic::Ordering::SeqCst) {
        spin_loop();
    }

    set_local_thread_pointer(hart_id());
    // Initialize CPU Configuration.
    init_cpu();
    ph_init_iter(CtorType::Cpu).for_each(|x| (x.func)());

    super::call_real_main(hart_id());
}
