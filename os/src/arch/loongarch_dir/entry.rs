use core::sync::atomic::{AtomicBool, Ordering};
use log::warn;
use polyhal::println;
use spin::Mutex;

static BSP_DONE: Mutex<bool> = Mutex::new(true);

use crate::arch::loongarch_dir::BOOT_STACK;
use crate::config::{KERNEL_STACK_SIZE};
use polyhal::arch::consts::VIRT_ADDR_START;
use crate::sbi_la::*;

macro_rules! init_dwm {
    () => {
        "
        # 设置直接映射窗口 DMWIN0: 映射 0x8000_xxxx_xxxx_xxxx 到物理地址
        ori         $t0, $zero, 0x1     # CSR_DMW0_PLV0
        lu52i.d     $t0, $t0, -2048     # UC, PLV0, 0x8000 xxxx xxxx xxxx
        csrwr       $t0, 0x180          # LOONGARCH_CSR_DMWIN0
        
        # 设置直接映射窗口 DMWIN1: 映射 0x9000_xxxx_xxxx_xxxx 到物理地址
        ori         $t0, $zero, 0x11    # CSR_DMW1_MAT | CSR_DMW1_PLV0
        lu52i.d     $t0, $t0, -1792     # CA, PLV0, 0x9000 xxxx xxxx xxxx
        csrwr       $t0, 0x181          # LOONGARCH_CSR_DMWIN1
        "
    };
}

#[naked]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
unsafe extern "C" fn _start(id: usize) -> ! {
    // core::arch::asm!(
    //     "li.w $a7, 1",  // SBI_EXT_DEBUG_CONSOLE_PUTCHAR
    //     "li.w $a0, 65", // 'A'
    //     "ecall",
    //     options(nomem, nostack)
    // );
    unsafe {
        core::arch::naked_asm!(
            "li.w $t0, 0x1fe001e0",    // UART 基址
            "li.w $t1, 0x48",           // 'H'
            "st.b $t1, $t0, 0",         // 输出 'H'
            "li.w $t1, 0x69",           // 'i'
            "st.b $t1, $t0, 0",         // 输出 'i'
            "li.w $t1, 0x0a",           // '\n'
            "st.b $t1, $t0, 0",         // 输出换行
            // 1. 初始化直接映射窗口 (DMW)
            init_dwm!(),
            
            // 2. 启用页表
            "
            li.w        $t0, 0xb0       # PLV=0, IE=0, PG=1
            csrwr       $t0, 0x0        # LOONGARCH_CSR_CRMD
            li.w        $t0, 0x00       # PLV=0, PIE=0, PWE=0
            csrwr       $t0, 0x1        # LOONGARCH_CSR_PRMD
            li.w        $t0, 0x00       # FPE=0, SXE=0, ASXE=0, BTE=0
            csrwr       $t0, 0x2        # LOONGARCH_CSR_EUEN
        ",
            
            // 3. 读取CPU ID
            "
            csrrd       $a0, 0x20       # cpuid
            move          $tp, $a0
        ",
            
            // 4. 设置启动栈
            // sp = boot_stack + (hartid + 1) * 64KB
            "
            addi.d        $t0, $a0, 1
            li.d        $t1, {boot_stack_size}
            mul.d       $t0, $t0, $t1                # t0 = (hart_id + 1) * boot_stack_size
            la.local    $sp, {boot_stack}
            add.d       $sp, $sp, $t0                # set boot stack
        ",
            
            // 5. 启用浮点寄存器
            "
            li.w        $t0, 0x11
            csrwr       $t0, 0x2        # LOONGARCH_CSR_EUEN
        ",
            
            // 6. 跳转到 rust_main
            "
            la.local    $t2, {entry}
            jirl        $ra, $t2, 0                      # call rust_main
        ",
            boot_stack_size = const KERNEL_STACK_SIZE,
            boot_stack = sym BOOT_STACK,
            entry = sym rust_main,
        )
    }
}

unsafe extern "C" {
    safe fn sbss();
    safe fn ebss();
}

fn clear_bss() {
    unsafe {
        core::slice::from_raw_parts_mut(
            sbss as *mut u8,
            ebss as usize - sbss as usize,
        ).fill(0);
    }
}

pub(crate) fn rust_main(id: usize) {
    clear_bss();
    set_tp(id);
    println!("[LoongArch64] Hello from cpu {}!", id);
    let bsp_lock = BSP_DONE.lock();
    let is_first = *bsp_lock;
    drop(bsp_lock);
    if is_first == true {
        let mut bsp_lock = BSP_DONE.lock();
        *bsp_lock = false;
        drop(bsp_lock);
        let _ = unsafe { super::_main_for_arch(id, true) };
    } else {
        let _ = unsafe { super::_main_for_arch(id, false) };
    }

    loop {}
}