use core::arch::naked_asm;
use core::hint::spin_loop;
use core::sync::atomic::AtomicBool;
use loongArch64::register::euen;
use polyhal::percpu::set_local_thread_pointer;
use polyhal::{
    ctor::{ph_init_iter, CtorType},
    hart_id,
    mem::{add_memory_region, init_dtb_once, parse_system_info},
};

/// Signal that primary core has completed initialization
static INIT_DONE: AtomicBool = AtomicBool::new(false);

#[cfg(not(board = "2k1000"))]
const EARLY_UART_ADDR: usize = 0x8000_0000_1fe0_01e0;
#[cfg(board = "2k1000")]
const EARLY_UART_ADDR: usize = 0x8000_0000_1fe2_0000;

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

        # Early marker 'P': paging/DMW configured.
        li.d        $t1, {early_uart}
        addi.d      $t2, $t1, 5
1:
        ld.bu       $t3, $t2, 0
        andi        $t3, $t3, 0x20
        beqz        $t3, 1b
        ori         $t3, $zero, 80
        st.b        $t3, $t1, 0
        
        la.global   $sp, bstack_top
        csrrd       $a0, 0x20           # cpuid
        la.global   $t0, {entry}

        # Early marker 'J': about to jump to rust_tmp_main.
        li.d        $t1, {early_uart}
        addi.d      $t2, $t1, 5
2:
        ld.bu       $t3, $t2, 0
        andi        $t3, $t3, 0x20
        beqz        $t3, 2b
        ori         $t3, $zero, 74
        st.b        $t3, $t1, 0

        jirl        $zero,$t0,0
        ",
        early_uart = const EARLY_UART_ADDR,
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

#[cfg(not(board = "2k1000"))]
const BOOT_DTB_ADDR: polyhal::PhysAddr = polyhal::PhysAddr(0x0010_0000);
#[cfg(board = "2k1000")]
const BOOT_DTB_ADDR: polyhal::PhysAddr = polyhal::PhysAddr(0x0ecc_f480);

const FALLBACK_MEM_START: usize = 0x8000_0000;
const FALLBACK_MEM_END: usize = 0x1_0000_0000;

fn boot_putchar(ch: u8) {
    if ch == b'\n' {
        boot_putchar(b'\r');
    }

    let thr = EARLY_UART_ADDR as *mut u8;
    let lsr = (EARLY_UART_ADDR + 5) as *const u8;

    for _ in 0..100_000 {
        if unsafe { lsr.read_volatile() } & 0x20 != 0 {
            break;
        }
        core::hint::spin_loop();
    }

    unsafe {
        thr.write_volatile(ch);
    }

    for _ in 0..10_000 {
        core::hint::spin_loop();
    }
}

fn boot_dbg(msg: &str) {
    msg.as_bytes().iter().for_each(|&ch| boot_putchar(ch));
}

fn boot_dbg_hex(label: &str, value: usize) {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    boot_dbg(label);
    boot_dbg("0x");
    for shift in (0..usize::BITS).step_by(4).rev() {
        let digit = ((value >> shift) & 0xf) as usize;
        boot_putchar(HEX[digit]);
    }
    boot_dbg("\n");
}

/// Rust temporary entry point
///
/// This function will be called after assembly boot stage.
pub fn rust_tmp_main(hart_id: usize) {
    boot_dbg("\n[la64] rust_tmp_main enter\n");
    boot_dbg_hex("[la64] hart_id=", hart_id);
    boot_dbg_hex("[la64] dtb_phys=", BOOT_DTB_ADDR.0);

    boot_dbg("[la64] clear_bss begin\n");
    super::clear_bss();
    boot_dbg("[la64] clear_bss done\n");

    boot_dbg("[la64] init_dtb_once begin\n");
    match init_dtb_once(BOOT_DTB_ADDR) {
        Ok(()) => boot_dbg("[la64] init_dtb_once ok\n"),
        Err(_) => {
            boot_dbg("[la64] init_dtb_once failed\n");
            boot_dbg("[la64] add fallback memory begin\n");
            unsafe {
                add_memory_region(FALLBACK_MEM_START, FALLBACK_MEM_END);
            }
            boot_dbg("[la64] add fallback memory done\n");
        }
    }

    boot_dbg("[la64] set_local_thread_pointer begin\n");
    set_local_thread_pointer(hart_id);
    boot_dbg("[la64] set_local_thread_pointer done\n");

    // Initialize CPU Configuration.
    boot_dbg("[la64] init_cpu begin\n");
    init_cpu();
    boot_dbg("[la64] init_cpu done\n");

    boot_dbg("[la64] cpu ctors begin\n");
    ph_init_iter(CtorType::Cpu).for_each(|x| (x.func)());
    boot_dbg("[la64] cpu ctors done\n");

    boot_dbg("[la64] parse_system_info begin\n");
    parse_system_info();
    boot_dbg("[la64] parse_system_info done\n");

    boot_dbg("[la64] platform ctors begin\n");
    ph_init_iter(CtorType::Platform).for_each(|x| (x.func)());
    boot_dbg("[la64] platform ctors done\n");

    boot_dbg("[la64] hal driver ctors begin\n");
    ph_init_iter(CtorType::HALDriver).for_each(|x| (x.func)());
    boot_dbg("[la64] hal driver ctors done\n");

    // Signal secondary cores that initialization is complete
    INIT_DONE.store(true, core::sync::atomic::Ordering::SeqCst);

    boot_dbg("[la64] call_real_main begin\n");
    super::call_real_main(hart_id);
}

/// Initialize CPU Configuration.
fn init_cpu() {
    // Enable floating point
    euen::set_fpe(true);
    // Alpine's LoongArch64 userland uses the 128-bit LSX extension.
    euen::set_sxe(true);

    // Initialzie Timer
    // timer::init_timer();
}

/// The entry point for the second core.
pub(crate) extern "C" fn _rust_secondary_main() {
    boot_dbg("[la64-secondary] enter\n");
    // Wait for primary core to complete initialization
    while !INIT_DONE.load(core::sync::atomic::Ordering::SeqCst) {
        spin_loop();
    }
    boot_dbg("[la64-secondary] primary init done\n");

    set_local_thread_pointer(hart_id());
    // Initialize CPU Configuration.
    init_cpu();
    ph_init_iter(CtorType::Cpu).for_each(|x| (x.func)());

    boot_dbg("[la64-secondary] call_real_main begin\n");
    super::call_real_main(hart_id());
}
