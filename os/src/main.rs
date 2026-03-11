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

use log::*;

#[path = "boards/qemu.rs"]
mod board;

#[macro_use]
mod console;
mod config;
mod drivers;
pub mod fs;
pub mod lang_items;
mod logging;
pub mod mm;
pub mod sbi;
pub mod sync;
pub mod syscall;
pub mod task;
pub mod timer;
pub mod trap;
#[allow(missing_docs)]
pub mod arch;
use config::KERNEL_SPACE_OFFSET;
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
#[unsafe(no_mangle)]
pub fn rust_main() -> ! {
    unsafe extern "C" {
        safe fn ekernel();
    }
    
    println!("ekernel virt = {:#x}", ekernel as u64);
    println!("ekernel phys = {:#x}", ekernel as u64 - KERNEL_SPACE_OFFSET as u64);
    

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
    trap::enable_timer_interrupt();
    timer::set_next_trigger();
    println!("LIST APPS");
    fs::list_apps();
    println!("ADD INITPROC");
    task::add_initproc();
    println!("run_tasks");
    task::run_tasks();
    panic!("Unreachable in rust_main!");
}
