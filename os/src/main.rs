//! The main module and entrypoint
//!
//! Various facilities of the kernels are implemented as submodules. The most
//! important ones are:
//!
//! - [`trap`]: Handles all cases of switching from userspace to the kernel
//! - [`task`]: Task management
//! - [`syscall`]: System call handling and implementation
//! - [`mm`]: Address map using SV39
//! - [`sync`]: Wrap a static data structure inside it so that we are able to access it without any `unsafe`.
//! - [`fs`]: Separate user from file system with some structures
//!
//! The operating system also starts in this module. Kernel code starts
//! executing from `entry.asm`, after which [`rust_main()`] is called to
//! initialize various pieces of functionality. (See its source code for
//! details.)
//!
//! We then call [`task::run_tasks()`] and for the first time go to
//! userspace.

#![deny(missing_docs)]
#![deny(warnings)]
#![allow(unused_imports)]
#![no_std]
#![no_main]
#![feature(alloc_error_handler)]
#![feature(step_trait)]
#![feature(naked_functions)]
extern crate alloc;

#[macro_use]
extern crate bitflags;

use core::arch::naked_asm;
use log::*;
#[path = "boards/qemu.rs"]
mod board;
#[macro_use]
mod console;
#[allow(missing_docs)]
pub mod arch;
mod config;
#[allow(missing_docs)]
pub mod devices;
mod drivers;
pub mod fs;
pub mod lang_items;
mod logging;
pub mod mm;
mod net;
pub mod sbi;
mod socket;
pub mod sync;
pub mod syscall;
#[allow(missing_docs)]
pub mod task;

pub mod timer;
pub mod trap;
use crate::task::init_processors;
use config::{KERNEL_CORE_STACK_BASE, KERNEL_SPACE_OFFSET, KERNEL_STACK_SIZE};

#[allow(missing_docs)]
#[allow(missing_docs)]
use core::arch::global_asm;

//global_asm!(include_str!("entry.asm"));
/// clear BSS segment
fn clear_bss() {
    unsafe extern "C" {
        safe fn sbss();
        safe fn ebss();
    }
    unsafe {
        core::slice::from_raw_parts_mut(sbss as usize as *mut u8, ebss as usize - sbss as usize)
            .fill(0);
    }
}

/// the rust entry-point of os
// #[unsafe(no_mangle)]
// pub fn rust_main() -> ! {
//     unsafe extern "C" {
//         safe fn ekernel();
//     }

//     println!("ekernel virt = {:#x}", ekernel as u64);
//     println!(
//         "ekernel phys = {:#x}",
//         ekernel as u64 - KERNEL_SPACE_OFFSET as u64
//     );

//     println!("Hello from kernel!");
//     println!("Kernel loaded at 0x80200000");
//     clear_bss();
//     println!("init logging");
//     logging::init();
//     info!("[kernel] Hello, world!");
//     println!("init mm");
//     mm::init();
//     mm::remap_test();
//     trap::init();
//     trap::enable_timer_interrupt();
//     timer::set_next_trigger();
//     println!("LIST APPS");
//     fs::list_apps();
//     println!("ADD INITPROC");
//     task::add_initproc();
//     println!("run_tasks");

//     task::run_tasks();
//     panic!("Unreachable in rust_main!");
// }

#[allow(unused)]
fn processor_start(id: usize) {
    let nums = crate::config::MAX_CPU_NUM;
    for i in 0..nums {
        if i == id {
            continue;
        }
        crate::sbi::hart_start(i, 0);
        warn!("[kernel] start to wake up cpu {}... ", i);
    }
}

// /// the rust entry-point of os
// /// return true if need reboot (but not supported yet)
fn main(id: usize, first: bool) -> bool {
    if first {
        unsafe extern "C" {
            safe fn ekernel();
        }

        println!("ekernel virt = {:#x}", ekernel as u64);
        println!(
            "ekernel phys = {:#x}",
            ekernel as u64 - KERNEL_SPACE_OFFSET as u64
        );

        println!("Hello from kernel!");
        println!("Kernel loaded at 0x80200000");
        clear_bss();
        println!("init logging");
        logging::init();
        info!("[kernel] Hello, world!");
        println!("init mm");
        mm::init();
        mm::remap_test();
        trap::init();
        net::init();
        init_processors();
        println!("cpu {} init processors", id);
        fs::init();
        // println!("LIST APPS");
        // fs::list_apps();
        task::add_initproc();
        println!("ADD INITPROC");

        processor_start(id);
    } else {
        println!("cpu {} init processors", id);
        //mm::start_kvm();
        trap::init();
    }
    println!("cpu {} enable_timer_interrupt", id);
    trap::enable_timer_interrupt();
    println!("cpu {} set_next_trigger", id);
    timer::set_next_trigger();
    println!("cpu {} run_tasks", id);
    task::run_tasks();
    false
}

#[naked]
extern "C" fn pre_main(id: usize, first: bool) -> bool {
    unsafe {
        naked_asm!(
            "
            // mv      a0, tp
            // addi    a0, a0, 1
            // la      t0, {kernel_stacks_base}     // t0 = 栈数组基址
            // slli    t1, a0, 14                   // t1 = （id+1） * 16KB (用移位代替mul)
            // sub     sp, t0, t1                    // sp = 栈顶                
            
            j       {main}
            
            ",
            kernel_stacks_base = const KERNEL_CORE_STACK_BASE,    // 16KB
            main = sym main,
        )
    }
}

define_entry!(pre_main);
