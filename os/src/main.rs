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
use alloc::sync::Arc;
use alloc::vec::Vec;

#[macro_use]
extern crate bitflags;
use crate::syscall::signal::handle_signals;
use crate::syscall::signal::sys_rt_sigreturn;
use core::arch::naked_asm;
use log::*;
use mm::vm_set;
use polyhal::VirtAddr;
use polyhal::consts::VIRT_ADDR_START;
use polyhal::pagetable::TLB;
use polyhal::utils::addr::PhysPageNum;
use trap::_set_sum_bit;
use trap::handle_page_fault;
#[path = "boards/qemu.rs"]
mod board;
use crate::mm::vm_set::VMSpace;
use crate::timer::set_next_trigger;
use crate::vm_set::PageFaultError;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use core::time::Duration;
// #[macro_use]
// mod console;
pub use polyhal::println;
#[allow(missing_docs)]
pub mod arch;
mod config;
#[allow(missing_docs)]
pub mod devices;
mod drivers;
/// error code
pub mod error;
///
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

// #[cfg(target_arch = "riscv64")]
pub mod timer;
pub mod trap;
use crate::task::init_processors;
// use config::KERNEL_STACK_SIZE};

#[cfg(target_arch = "loongarch64")]
use crate::virtio_blk::_init_virtio_pci;
#[allow(missing_docs)]
use core::arch::global_asm;
use mm::frame_allocator;
use mm::heap_allocator;
use polyhal::common::{self, *};
use polyhal::irq::IRQ;

#[cfg(target_arch = "loongarch64")]
use polyhal_boot::*;

use crate::signal::Signal;
use crate::syscall::futex::check_futex_timeouts;
use crate::syscall::signal::deliver_signal;
use drivers::block::*;
use polyhal_trap::trap::init_trap;
use polyhal_trap::trap::*;
use polyhal_trap::trapframe::*;
use syscall::syscall;
use task::*;

/// 主核初始化完成标志，用于同步从核启动
static INIT_COMPLETED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// 设置初始化完成标志（主核调用）
pub fn set_init_completed() {
    INIT_COMPLETED.store(true, core::sync::atomic::Ordering::SeqCst);
}

/// 等待主核完成初始化（从核调用）
fn wait_for_init() {
    while !INIT_COMPLETED.load(core::sync::atomic::Ordering::SeqCst) {
        core::hint::spin_loop();
    }
}

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

/// 定时器中断中标记的页缓存回刷请求，在返回用户态前由当前任务执行
static SYNC_PENDING: AtomicBool = AtomicBool::new(false);

/// kernel interrupt
#[polyhal::arch_interrupt]
fn kernel_interrupt(ctx: &mut TrapFrame, trap_type: TrapType) {
    // info!("enter trap_handler");
    // error!("trap_type @ {:x?} {:#x?}", trap_type,  ctx);
    // unsafe {
    // let pgdl: usize;
    // core::arch::asm!("csrrd {}, 0x1B", out(reg) pgdl);
    // error!("PGDL = 0x{:016x}", pgdl);
    // }
    // info!("current_task id: {}", current_task().is_some());
    _set_sum_bit();
    // 如果当前任务的进程已被回收（孤儿线程），直接退出
    // info!("trap type {:?}", trap_type);
    if let Some(task) = current_task() {
        if task.process.upgrade().is_none() {
            crate::task::exit_current_and_run_next(0);
        }
    }
    match trap_type {
        TrapType::Breakpoint => {
            // jump to next instruction anyway
            ctx.syscall_ok();
            _set_sum_bit();
            let args = ctx.args();
            // get system call return value
            let _syscall_id = ctx[TrapFrameArgs::SYSCALL];
            // if syscall_id == 260 || syscall_id == 95 {
            //     println!("!!!SYSCALL{}!!! pid={}", syscall_id, current_task().unwrap().process.upgrade().unwrap().getpid());
            // }

            let result = syscall(139, [args[0], args[1], args[2], args[3], args[4], args[5]]);
            // cx is changed during sys_exec, so we have to call it again
            match result {
                Ok(val) => ctx[TrapFrameArgs::RET] = val,
                Err(errno) => ctx[TrapFrameArgs::RET] = (-(errno.code() as isize)) as usize,
            }
            TLB::flush_all();
        }
        TrapType::SysCall => {
            // jump to next instruction anyway
            ctx.syscall_ok();
            _set_sum_bit();
            let args = ctx.args();
            // get system call return value
            let syscall_id = ctx[TrapFrameArgs::SYSCALL];
            // if syscall_id == 260 || syscall_id == 95 {
            //     println!("!!!SYSCALL{}!!! pid={}", syscall_id, current_task().unwrap().process.upgrade().unwrap().getpid());
            // }

            let result = syscall(syscall_id, [
                args[0], args[1], args[2], args[3], args[4], args[5],
            ]);
            // cx is changed during sys_exec, so we have to call it again
            match result {
                Ok(val) => ctx[TrapFrameArgs::RET] = val,
                Err(errno) => ctx[TrapFrameArgs::RET] = (-(errno.code() as isize)) as usize,
            }
            TLB::flush_all();
        }
        TrapType::StorePageFault(_paddr)
        | TrapType::LoadPageFault(_paddr)
        | TrapType::InstructionPageFault(_paddr) => {
            // info!("trap type {:?}", trap_type);
            match handle_page_fault(trap_type) {
                Some(PageFaultError::Normal) => {}
                Some(PageFaultError::BeyondFileSize) => {
                    if let Some(task) = current_task() {
                        if let Some(process) = task.process.upgrade() {
                            // 同步信号（SIGSEGV）不能被阻塞，否则 longjmp 跳过
                            // sigreturn 后将导致无限死循环
                            let mut t_inner = task.inner_exclusive_access();
                            t_inner.blocked_signals.remove(Signal::SigBus);
                            drop(t_inner);
                            let mut p_inner = process.inner_exclusive_access();
                            p_inner.blocked_signals.remove(Signal::SigBus);
                            drop(p_inner);
                            deliver_signal(&process, Signal::SigBus);
                            if process.inner_exclusive_access().is_zombie {
                                exit_current_and_run_next(-(Signal::SigBus.as_i32()));
                            }
                        }
                    }
                }
                _ => {
                    error!(
                        "[kernel] in application, bad addr = {:#x}, ctx: {:#x?} sending SIGSEGV.",
                        _paddr, ctx
                    );
                    if let Some(task) = current_task() {
                        if let Some(process) = task.process.upgrade() {
                            // 同步信号（SIGSEGV）不能被阻塞，否则 longjmp 跳过
                            // sigreturn 后将导致无限死循环
                            let mut t_inner = task.inner_exclusive_access();
                            t_inner.blocked_signals.remove(Signal::SigSegv);
                            drop(t_inner);
                            let mut p_inner = process.inner_exclusive_access();
                            p_inner.blocked_signals.remove(Signal::SigSegv);
                            drop(p_inner);
                            deliver_signal(&process, Signal::SigSegv);
                            if process.inner_exclusive_access().is_zombie {
                                exit_current_and_run_next(-(Signal::SigSegv.as_i32()));
                            }
                        }
                    }
                }
            }
            // if !handle_page_fault(trap_type).is_some() {
            //     error!(
            //         "[kernel] in application, bad addr = {:#x}, ctx: {:#x?} sending SIGSEGV.",
            //         _paddr, ctx
            //     );
            //     if let Some(task) = current_task() {
            //         if let Some(process) = task.process.upgrade() {
            //             // 同步信号（SIGSEGV）不能被阻塞，否则 longjmp 跳过
            //             // sigreturn 后将导致无限死循环
            //             let mut t_inner = task.inner_exclusive_access();
            //             t_inner.blocked_signals.remove(Signal::SigSegv);
            //             drop(t_inner);
            //             let mut p_inner = process.inner_exclusive_access();
            //             p_inner.blocked_signals.remove(Signal::SigSegv);
            //             drop(p_inner);
            //             deliver_signal(&process, Signal::SigSegv);
            //             if process.inner_exclusive_access().is_zombie {
            //                 exit_current_and_run_next(-(Signal::SigSegv.as_i32()));
            //             }
            //         }
            //     }
            // }
        }
        TrapType::IllegalInstruction(_) => {
            if let Some(task) = current_task() {
                if let Some(process) = task.process.upgrade() {
                    let mut t_inner = task.inner_exclusive_access();
                    t_inner.blocked_signals.remove(Signal::SigIll);
                    drop(t_inner);
                    let mut p_inner = process.inner_exclusive_access();
                    p_inner.blocked_signals.remove(Signal::SigIll);
                    drop(p_inner);
                    deliver_signal(&process, Signal::SigIll);
                    if process.inner_exclusive_access().is_zombie {
                        exit_current_and_run_next(-(Signal::SigIll.as_i32()));
                    }
                }
            }
        }
        TrapType::Timer => {
            const MEMORY_DEBUG_INTERVAL: usize = 500; // 约每 5 秒打印一次（500 * 10ms）
            static TIMER_TICK_COUNT: AtomicUsize = AtomicUsize::new(0);
            let tick = TIMER_TICK_COUNT.fetch_add(1, Ordering::Relaxed);
            if tick % MEMORY_DEBUG_INTERVAL == 0 {
                mm::heap_allocator::print_heap_stats();
                mm::frame_allocator::print_frame_stats();
            }
            // 检查设置了 alarm/itimer 的进程（不再遍历所有进程）
            let now_us = polyhal::timer::current_time().as_micros();
            let now_ticks = crate::timer::get_time();
            let mut expired_processes = Vec::new();
            let mut to_remove = Vec::new();
            {
                let mut timer_procs = crate::task::manager::TIMER_PROCS.lock();
                for (pid, weak) in timer_procs.iter() {
                    let Some(process) = weak.upgrade() else {
                        to_remove.push(*pid);
                        continue;
                    };
                    let (alarm_expired, itimer_expired, still_active) = {
                        let inner = process.inner_exclusive_access();
                        let alarm = inner.alarm_deadline_us.map_or(false, |d| now_us >= d);
                        let itimer = inner.itimer_real_deadline.map_or(false, |d| now_ticks >= d);
                        let still = inner.alarm_deadline_us.is_some()
                            || inner.itimer_real_deadline.is_some();
                        (alarm, itimer, still)
                    };
                    if alarm_expired || itimer_expired {
                        expired_processes.push((process.clone(), alarm_expired, itimer_expired));
                    }
                    if !still_active {
                        to_remove.push(*pid);
                    }
                }
                for pid in to_remove {
                    timer_procs.remove(&pid);
                }
            }

            for (process, alarm_expired, itimer_expired) in expired_processes {
                if alarm_expired || itimer_expired {
                    error!(
                        "timer: SIGALRM fired for pid={}, alarm={}, itimer={}",
                        process.getpid(),
                        alarm_expired,
                        itimer_expired
                    );
                    deliver_signal(&process, Signal::SigAlrm);
                }
                let mut inner = process.inner_exclusive_access();
                if alarm_expired {
                    if let Some(interval) = inner.alarm_interval_us {
                        if interval > 0 {
                            let new_deadline = inner.alarm_deadline_us.unwrap_or(0) + interval;
                            inner.alarm_deadline_us = Some(new_deadline);
                        } else {
                            inner.alarm_deadline_us = None;
                        }
                    } else {
                        inner.alarm_deadline_us = None;
                    }
                }
                if itimer_expired {
                    if let Some(interval) = inner.itimer_real_interval {
                        let new_deadline = inner.itimer_real_deadline.unwrap_or(0) + interval;
                        inner.itimer_real_deadline = Some(new_deadline);
                    } else {
                        inner.itimer_real_deadline = None;
                    }
                }
                // 处理完后如果仍然没有活跃 timer，下次循环会被清理
            }

            // 页缓存脏页回刷：每 10 tick（约 1s）检查一次压力
            const WRITEBACK_INTERVAL_TICKS: usize = 10;
            if tick % WRITEBACK_INTERVAL_TICKS == 0 {
                if let Some(cache) = crate::fs::page::pagecache::PAGE_CACHE.try_lock() {
                    let dirty = cache.dirty_pages_count();
                    let threshold = crate::fs::page::pagecache::MAX_PAGE_CACHE_PAGES / 2;
                    drop(cache);
                    if dirty > threshold {
                        SYNC_PENDING.store(true, Ordering::Relaxed);
                    }
                }
            }

            polyhal::timer::set_next_timer(Duration::from_millis(100)); // 100ms 后

            check_futex_timeouts();
            suspend_current_and_run_next();
        }
        _ => {
            warn!("unsuspended trap type: {:?}", trap_type);
            exit_current_and_run_next(-(Signal::SigAbrt.as_i32()));
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

    // 返回用户态前处理 pending 的异步信号

    handle_signals(current_trap_cx());

    // 如果 pending 了页缓存回刷，在当前任务上下文中执行轻量 flush
    if SYNC_PENDING.load(Ordering::Relaxed) {
        if let Some(task) = current_task() {
            if let Some(process) = task.process.upgrade() {
                if let Some(inner) = process.inner_try_access() {
                    for fd in 0..inner.fd_table.len() {
                        if let Some(file) = inner.fd_table[fd].as_ref() {
                            file.flush();
                        }
                    }
                }
                SYNC_PENDING.store(false, Ordering::Relaxed);
            }
        }
    }

    // 如果当前进程已被标记为 zombie（如收到默认终止信号），直接退出当前任务
    if let Some(task) = current_task() {
        if let Some(process) = task.process.upgrade() {
            let inner = process.inner_exclusive_access();
            let is_zombie = inner.is_zombie;
            let exit_code = inner.exit_code;
            let pid = process.getpid();
            drop(inner);
            if is_zombie {
                error!(
                    "[DEBUG kernel_interrupt] pid={} is_zombie=true exit_code={}",
                    pid, exit_code
                );
                exit_current_and_run_next(exit_code);
            }
        } else {
            // 进程已被回收，当前线程为孤儿线程，直接退出
            exit_current_and_run_next(0);
        }
    }
}

#[unsafe(no_mangle)]
///
pub extern "C" fn _secondary_for_arch(hart_id: usize) -> ! {
    // 初始化从核
    if hart_id != 0 {
        println!("cpu {} waiting for init...", hart_id);
        wait_for_init();
        println!("cpu {} init completed, starting scheduler", hart_id);
    }
    println!("Secondary CPU {} starting", hart_id);

    // 初始化从核的 trap 处理
    println!("cpu {} init trap", hart_id);
    init_trap();
    println!("cpu {} set_next_trigger", hart_id);
    set_next_trigger();
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
    // println!("cpu {} enable_timer_interrupt", id);
    // trap::enable_timer_interrupt();
    println!("cpu {} set_next_trigger", id);
    set_next_trigger();
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
