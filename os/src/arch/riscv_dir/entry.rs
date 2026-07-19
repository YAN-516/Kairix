use core::sync::atomic::{AtomicBool, Ordering};
use log::warn;
use spin::Mutex;
static BSP_DONE: Mutex<bool> = Mutex::new(true);
static INIT_DONE: AtomicBool = AtomicBool::new(false);

// static BSP_DONE: AtomicBool = AtomicBool::new(false);

use crate::arch::riscv_dir::BOOT_STACK;
use crate::config::_PTES_PER_PAGE;
use crate::sbi::*;
use polyhal::PhysAddr;
use polyhal::arch::consts::VIRT_ADDR_START;
use polyhal::consts::*;
use polyhal::mem::{init_dtb_once, parse_system_info};

const SV39_ROOT_ENTRY_COUNT: usize = 512;
const SV39_ROOT_HALF_ENTRY_COUNT: usize = SV39_ROOT_ENTRY_COUNT / 2;
const GIGAPAGE_PPN_SHIFT: usize = 18;
const EARLY_LEAF_FLAGS: u64 = 0xcf; // V | R | W | X | A | D
const _: () = assert!(_PTES_PER_PAGE == SV39_ROOT_ENTRY_COUNT);

#[repr(C, align(4096))]
#[allow(missing_docs)]
pub struct BootPageTable([u64; _PTES_PER_PAGE]);
#[allow(missing_docs)]
pub static mut BOOT_PAGE_TABLE: BootPageTable = {
    let mut arr: [u64; _PTES_PER_PAGE] = [0; _PTES_PER_PAGE];
    let mut index = 0;
    while index < SV39_ROOT_HALF_ENTRY_COUNT {
        // A root-level Sv39 leaf maps one 1 GiB region. Map the lower
        // canonical half identically and mirror the same physical range into
        // the higher-half direct-map window. QEMU may place the DTB near the
        // top of RAM, so limiting this table to the first 1 GiB of RAM makes
        // early DTB parsing fault as soon as the guest is larger than 1 GiB.
        let ppn = (index as u64) << GIGAPAGE_PPN_SHIFT;
        let pte = (ppn << 10) | EARLY_LEAF_FLAGS;
        arr[index] = pte;
        arr[index + SV39_ROOT_HALF_ENTRY_COUNT] = pte;
        index += 1;
    }
    BootPageTable(arr)
};

#[naked]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
unsafe extern "C" fn _start(_id: usize, _dtb: usize) -> ! {
    unsafe {
        core::arch::naked_asm!(
            // 1. set boot stack
            // a0 = processor_id, a1 = device tree physical address
            // sp = boot_stack + (hartid + 1) * 64KB
            "
            .attribute arch, \"rv64gc\"
            mv      tp, a0
            addi    t0, a0, 1
            li      t1, {boot_stack_size}
            mul     t0, t0, t1                // t0 = (hart_id + 1) * boot_stack_size
            la      sp, {boot_stack}
            add     sp, sp, t0                // set boot stack
        ",
            // 2. enable sv39 page table
            // satp = (8 << 60) | PPN(page_table)
            "
            la      t0, {page_table}
            srli    t0, t0, 12
            li      t1, 8 << 60
            or      t0, t0, t1
            csrw    satp, t0
            sfence.vma
        ",
            // 3. enable float register
            "
            li   t0, (0b01 << 13)
            csrs sstatus, t0
        ",
            // 4. jump to rust_main
            // add virtual address offset to sp and pc
            "
            li      t2, {virt_ram_offset}
            or      sp, sp, t2
            la      a2, {entry}
            or      a2, a2, t2
            jalr    a2                      // call rust_main(hartid, dtb)
        ",
            boot_stack_size = const KERNEL_STACK_SIZE,
            boot_stack = sym BOOT_STACK,
            page_table = sym BOOT_PAGE_TABLE,
            entry = sym rust_main,
            virt_ram_offset = const VIRT_ADDR_START,
        )
    }
}

fn clear_bss() {
    unsafe extern "C" {
        safe fn sbss();
        safe fn _ebss();
    }
    unsafe {
        core::slice::from_raw_parts_mut(sbss as usize as *mut u8, _ebss as usize - sbss as usize)
            .fill(0);
    }
}

pub(crate) fn rust_main(id: usize, dtb: usize) {
    set_tp(id);
    let bsp_lock = BSP_DONE.lock();
    let is_first = *bsp_lock;
    // println!("{} {}", id, is_first);
    drop(bsp_lock);
    if is_first == true {
        let mut bsp_lock = BSP_DONE.lock();
        *bsp_lock = false;
        drop(bsp_lock);
        clear_bss();
        let _ = init_dtb_once(PhysAddr::from(dtb));
        parse_system_info();
        INIT_DONE.store(true, Ordering::SeqCst);
        polyhal::println!("Hello from cpu {}!", id);
        let _ = unsafe { super::_main_for_arch(id, true) };
    } else {
        while !INIT_DONE.load(Ordering::SeqCst) {
            core::hint::spin_loop();
        }
        polyhal::println!("Hello from cpu {}!", id);
        let _ = unsafe { super::_main_for_arch(id, false) };
    }

    loop {}
}
