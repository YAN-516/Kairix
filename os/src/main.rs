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
#![cfg_attr(target_arch = "riscv64", feature(riscv_ext_intrinsics))]
// #![feature(riscv_ext_intrinsics)]

extern crate alloc;
// extern crate flat_device_tree; 

#[macro_use]
extern crate bitflags;
use polyhal::VirtAddr;
use trap::_set_sum_bit;
use core::arch::naked_asm;
use log::*;
use mm::vm_set;
use polyhal::consts::VIRT_ADDR_START;
use polyhal::utils::addr::PhysPageNum;
use trap::handle_page_fault;
use polyhal::pagetable::TLB;
#[path = "boards/qemu.rs"]
mod board;
use core::time::Duration;
use crate::mm::vm_set::VMSpace;
// #[macro_use]
// mod console;
pub use polyhal::println;
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
///
#[cfg(target_arch = "riscv64")]
pub mod sbi;
mod socket;

///
#[cfg(target_arch = "loongarch64")]
pub mod sbi_la;

pub mod sync;
pub mod syscall;
#[allow(missing_docs)]
pub mod task;

#[cfg(target_arch = "riscv64")]
pub mod timer;
pub mod trap;
use crate::task::init_processors;
// use config::KERNEL_STACK_SIZE};

#[allow(missing_docs)]
use core::arch::global_asm;
#[cfg(target_arch = "loongarch64")]
use crate::virtio_blk::_init_virtio_pci;
use mm::frame_allocator;
use mm::heap_allocator;
use polyhal::common::{self, *};
use polyhal::irq::IRQ;

#[cfg(target_arch = "loongarch64")]
use polyhal_boot::*;

use polyhal_trap::trap::init_trap;
use polyhal_trap::trap::*;
use polyhal_trap::trapframe::*;
use syscall::syscall;
use task::*;
use drivers::block::*;
//global_asm!(include_str!("entry.asm"));
/// clear BSS segment
fn clear_bss() {
    unsafe extern "C" {
        safe fn _sbss();
        safe fn _ebss();
    }
    unsafe {
        core::slice::from_raw_parts_mut(_sbss as usize as *mut u8, _ebss as usize - _sbss as usize)
            .fill(0);
    }
}

#[allow(unused)]
fn processor_start(id: usize) {
    let nums = crate::config::MAX_CPU_NUM;
    for i in 0..nums {
        if i == id {
            continue;
        }
        // crate::sbi::hart_start(i, 0);
        warn!("[kernel] start to wake up cpu {}... ", i);
    }
}

/// kernel interrupt
#[polyhal::arch_interrupt]
fn kernel_interrupt(ctx: &mut TrapFrame, trap_type: TrapType) {
    // error!("trap_type @ {:x?} {:#x?}", trap_type,  ctx);
    // unsafe {
    // let pgdl: usize;
    // core::arch::asm!("csrrd {}, 0x1B", out(reg) pgdl);
    // error!("PGDL = 0x{:016x}", pgdl);
    // }
    // info!("current_task id: {}", current_task().is_some());
    _set_sum_bit();
    match trap_type {
        TrapType::Breakpoint => return,
        TrapType::SysCall => {
            // jump to next instruction anyway
            ctx.syscall_ok();
            _set_sum_bit();
            let args = ctx.args();
            // get system call return value
            // info!("syscall: {}", ctx[TrapFrameArgs::SYSCALL]);

            let result = syscall(ctx[TrapFrameArgs::SYSCALL], [
                args[0], args[1], args[2], args[3], args[4], args[5],
            ]);
            // cx is changed during sys_exec, so we have to call it again
            ctx[TrapFrameArgs::RET] = result as usize;
            TLB::flush_all();
        }
        TrapType::StorePageFault(_paddr)
        | TrapType::LoadPageFault(_paddr)
        | TrapType::InstructionPageFault(_paddr) => {
            // info!(
            //     "[kernel] in application, bad addr = {:#x}, ctx: {:#x?} kernel killed it.",
            //     //scause.cause(),
            //     _paddr,
            //     ctx
            //     //current_trap_cx().sepc,
            // );
            // exit_current_and_run_next(-2);
            error!("trap type {:?}",trap_type);
            // {
            //     let process = current_task().unwrap().process.upgrade().unwrap();
            // let vm_set = &mut process.inner_exclusive_access().vm_set;
            // if let Some(pte) = vm_set.translate(VirtAddr::from(_paddr).floor()) {
            //     error!("pte flag {:?} {:#x}", pte.flags(), pte.ppn().0);
            // } else {
            //     error!("nothing");
            // }
            // }
            if !handle_page_fault(trap_type).is_some() {

                error!(
                    "[kernel] in application, bad addr = {:#x}, ctx: {:#x?} kernel killed it.",
                    //scause.cause(),
                    _paddr,
                    ctx //current_trap_cx().sepc,
                );
                loop {
                    
                }
                // exit_current_and_run_next(-2);
            }

            // current_add_signal(SignalFlags::SIGSEGV);
        }
        TrapType::IllegalInstruction(_) => {
            // current_add_signal(SignalFlags::SIGILL);
            exit_current_and_run_next(-2);
        }
        TrapType::Timer => {
            // error!("trap in main");
            polyhal::timer::set_next_timer(Duration::from_millis(1000)); // 10ms 后

            suspend_current_and_run_next();
        }
        _ => {
            warn!("unsuspended trap type: {:?}", trap_type);
            exit_current_and_run_next(-2);
        }
    }
    // handle signals (handle the sent signal)
    // println!("[K] trap_handler:: handle_signals");
    // handle_signals();

    // // check error signals (if error then exit)
    // if let Some((errno, msg)) = check_signals_error_of_current() {
    //     println!("[kernel] {}", msg);
    //     exit_current_and_run_next(errno);
    // }
    // if let Some((errno, msg)) = check_signals_of_current() {
    //     println!("[kernel] {}", msg);
    //     // panic!("end");
    //     exit_current_and_run_next(errno);
    // }
}

#[unsafe(no_mangle)]
///
pub extern "C" fn _secondary_for_arch(hart_id: usize) -> ! {
    // 初始化从核
    println!("Secondary CPU {} starting", hart_id);

    // 初始化从核的 trap 处理
    init_trap();

    // 初始化从核的 per-CPU 数据
    // init_percpu(hart_id);

    // 进入调度器
    task::run_tasks();

    loop {}
}

///
pub struct PageAllocImpl;

impl PageAlloc for PageAllocImpl {
    #[inline]
    fn alloc(&self) -> Option<PhysPageNum> {
        mm::frame_alloc_hal()
    }

    #[inline]
    fn dealloc(&self, ppn: PhysPageNum) {
        mm::frame_dealloc(ppn)
    }
}

#[polyhal::arch_entry]

fn main(id: usize, first: bool) -> bool {
    if first {
        unsafe extern "C" {
            safe fn ekernel();
        }

        println!("ekernel virt = {:#x}", ekernel as u64);
        println!(
            "ekernel phys = {:#x}",
            ekernel as u64 - VIRT_ADDR_START as u64
        );

        println!("Hello from kernel!");
        println!("Kernel loaded at 0x80200000");
        clear_bss();
        println!("init logging");
        logging::init();
        println!("cargo build success");
        info!("[kernel] Hello, world!");
        println!("init heap_allocator");
        heap_allocator::init_heap();
        println!("init frame_allocator");
        frame_allocator::init_frame_allocator();
        common::init(&PageAllocImpl);
        init_trap();
        println!("init mm");
        mm::init();
        // mm::remap_test();

        // IRQ::int_enable();
        // if IRQ::int_enabled(){
        //     println!("int enabled");
        // }

        net::init();
        init_processors();
        println!("cpu {} init processors", id);

        // #[cfg(target_arch = "loongarch64")]
        // init_virtio_pci();
        
        println!("init fs");
        fs::init();
        // println!("LIST APPS");
        // fs::list_apps();
        println!("ADD INITPROC");
        task::add_initproc();
        println!("processor_start");

        processor_start(id);
    } else {
        println!("cpu {} init processors", id);
        //mm::start_kvm();
        init_trap();
    }
    println!("cpu {} enable_timer_interrupt", id);
    // trap::enable_timer_interrupt();
    println!("cpu {} set_next_trigger", id);
    // timer::set_next_trigger();
    println!("cpu {} run_tasks", id);
    task::run_tasks();
    false
}

// #[naked]
// extern "C" fn pre_main(id: usize, first: bool) -> bool {
//     unsafe {
//         naked_asm!(
//             "
//             // mv      a0, tp
//             // addi    a0, a0, 1
//             // la      t0, {kernel_stacks_base}     // t0 = 栈数组基址
//             // slli    t1, a0, 14                   // t1 = （id+1） * 16KB (用移位代替mul)
//             // sub     sp, t0, t1                    // sp = 栈顶

//             j       {main}

//             ",
//             kernel_stacks_base = const KERNEL_CORE_STACK_BASE,    // 16KB
//             main = sym main,
//         )
//     }
// }

// define_entry!(pre_main);
