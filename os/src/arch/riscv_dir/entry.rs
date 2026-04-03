use core::sync::atomic::{AtomicBool, Ordering};
use log::warn;

// static BSP_DONE: AtomicBool = AtomicBool::new(false);

use crate::arch::riscv_dir::BOOT_STACK;
use crate::config::{KERNEL_STACK_SIZE, PTES_PER_PAGE};
use polyhal::arch::consts::VIRT_ADDR_START;
use crate::sbi::*;
#[repr(C, align(4096))]
#[allow(missing_docs)]
pub struct BootPageTable([u64; PTES_PER_PAGE]);
#[allow(missing_docs)]
pub static mut BOOT_PAGE_TABLE: BootPageTable = {
    let mut arr: [u64; PTES_PER_PAGE] = [0; PTES_PER_PAGE];
    arr[2] = (0x80000 << 10) | 0xcf;
    arr[256] = (0x00000 << 10) | 0xcf;
    arr[258] = (0x80000 << 10) | 0xcf;
    BootPageTable(arr)
};

#[naked]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
unsafe extern "C" fn _start(id: usize) -> ! {
    unsafe {
        core::arch::naked_asm!(
            // 1. set boot stack
            // a0 = processor_id
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
            jalr    a2                      // call rust_main
        ",
            boot_stack_size = const KERNEL_STACK_SIZE,
            boot_stack = sym BOOT_STACK,
            page_table = sym BOOT_PAGE_TABLE,
            entry = sym rust_main,
            virt_ram_offset = const VIRT_ADDR_START,
        )
    }
}

pub(crate) fn rust_main(id: usize) {
    set_tp(id);
    println!("Hello from cpu {}!", id);
    // let is_bsp = !BSP_DONE.swap(true, Ordering::SeqCst);
    if id == 0 {
        let _ = unsafe { super::_main_for_arch(id, true) };
    } else {
        let _ = unsafe { super::_main_for_arch(id, false) };
    }

    loop {}
}
