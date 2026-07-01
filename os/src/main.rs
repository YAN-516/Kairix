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
//! The operating system also starts in this module. Architecture-specific boot
//! code enters here and initializes the kernel facilities. (See the source for
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
use core::time::Duration;
extern crate alloc;
// extern crate flat_device_tree;
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
use polyhal::utils::addr::PhysPageNum;
use trap::_set_sum_bit;
use trap::handle_page_fault;
#[path = "boards/qemu.rs"]
mod board;
use crate::mm::vm_set::VMSpace;
use crate::timer::set_next_trigger;
use crate::vm_set::PageFaultError;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
pub use polyhal::println;
#[allow(missing_docs)]
pub mod arch;
mod config;
#[allow(missing_docs)]
pub mod devices;
mod drivers;
mod embedded;
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
pub mod tls;

pub mod timer;

#[cfg(target_arch = "riscv64")]
fn trap_from_user(ctx: &polyhal_trap::trapframe::TrapFrame) -> bool {
    ctx.from_user()
}

#[cfg(target_arch = "loongarch64")]
fn trap_from_user(ctx: &polyhal_trap::trapframe::TrapFrame) -> bool {
    ctx.prmd & 0b11 == 0b11
}
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
use syscall::{syscall, SYSCALL_EXECVE};
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

#[allow(unused)]
fn processor_start(id: usize) {
    let nums = crate::config::MAX_CPU_NUM;
    for i in 0..nums {
        if i == id {
            continue;
        }
        #[cfg(target_arch = "riscv64")]
        crate::sbi::hart_start(i, 0);
        warn!("[kernel] start to wake up cpu {}... ", i);
    }
}

struct TrapReturnState {
    process_missing: bool,
    task_exit_code: Option<i32>,
    process_exit_code: Option<i32>,
    has_pending_signal: bool,
}

fn trap_return_state(task: &crate::task::TaskControlBlock) -> TrapReturnState {
    let Some(process) = task.process.upgrade() else {
        return TrapReturnState {
            process_missing: true,
            task_exit_code: None,
            process_exit_code: None,
            has_pending_signal: false,
        };
    };
    let (task_status, task_exit_code, task_pending, task_blocked, task_needs_signal) = {
        let t_inner = task.inner_exclusive_access();
        (
            t_inner.task_status,
            t_inner.exit_code,
            t_inner.pending_signals,
            t_inner.blocked_signals,
            t_inner.need_signal_handle,
        )
    };
    if task_status == crate::task::TaskStatus::Zombie {
        return TrapReturnState {
            process_missing: false,
            task_exit_code: Some(task_exit_code.unwrap_or(0)),
            process_exit_code: None,
            has_pending_signal: false,
        };
    }
    let (proc_is_zombie, proc_exit_code, proc_pending, proc_needs_signal) = {
        let p_inner = process.inner_exclusive_access();
        (
            p_inner.is_zombie,
            p_inner.exit_code,
            p_inner.pending_signals,
            p_inner.need_signal_handle,
        )
    };
    let has_pending_signal = task_needs_signal
        || proc_needs_signal
        || ((task_pending.bits() | proc_pending.bits()) & !task_blocked.bits()) != 0;
    TrapReturnState {
        process_missing: false,
        task_exit_code: None,
        process_exit_code: proc_is_zombie.then_some(proc_exit_code),
        has_pending_signal,
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
    // Fast syscall path skips this defensive orphan check; the scheduler already
    // filters tasks whose PCB has disappeared.
    if !matches!(trap_type, TrapType::SysCall | TrapType::Breakpoint) {
        if let Some(task) = current_task() {
            if task.process.upgrade().is_none() {
                crate::task::exit_current_and_run_next(0);
            }
        }
    }
    match trap_type {
        TrapType::Breakpoint => {
            // jump to next instruction anyway
            ctx.syscall_ok();
            let args = ctx.args();
            // get system call return value
            let _syscall_id = ctx[TrapFrameArgs::SYSCALL];
            // if syscall_id == 260 || syscall_id == 95 {
            //     println!("!!!SYSCALL{}!!! pid={}", syscall_id, current_task().unwrap().process.upgrade().unwrap().getpid());
            // }

            let result = syscall(139, [args[0], args[1], args[2], args[3], args[4], args[5]]);
            match result {
                Ok(val) => ctx[TrapFrameArgs::RET] = val,
                Err(errno) => ctx[TrapFrameArgs::RET] = (-(errno.code() as isize)) as usize,
            }
        }
        TrapType::SysCall => {
            // jump to next instruction anyway
            ctx.syscall_ok();
            let args = ctx.args();
            // get system call return value
            let syscall_id = ctx[TrapFrameArgs::SYSCALL];
            // if syscall_id == 260 || syscall_id == 95 {
            //     println!("!!!SYSCALL{}!!! pid={}", syscall_id, current_task().unwrap().process.upgrade().unwrap().getpid());
            // }

            let result = syscall(syscall_id, [
                args[0], args[1], args[2], args[3], args[4], args[5],
            ]);
            match result {
                // Successful execve has replaced the trap context; keep a0/a1 as argc/argv.
                Ok(_val) if syscall_id == SYSCALL_EXECVE => {}
                Ok(val) => ctx[TrapFrameArgs::RET] = val,
                Err(errno) => ctx[TrapFrameArgs::RET] = (-(errno.code() as isize)) as usize,
            }
        }
        TrapType::StorePageFault(_paddr)
        | TrapType::LoadPageFault(_paddr)
        | TrapType::InstructionPageFault(_paddr) => {
            if !trap_from_user(ctx) {
                let current_page_table = polyhal::PageTable::current();
                let current_root = current_page_table.root().0;
                let fault_va = VirtAddr::from(_paddr);
                let raw_pte = current_page_table
                    .find_pte(fault_va.floor())
                    .map(|pte| *pte);
                let pte_info = raw_pte.map(|pte| {
                    (
                        pte.0,
                        pte.ppn().0,
                        pte.flags(),
                        pte.is_valid(),
                        pte.is_table(),
                        pte.readable(),
                        pte.writable(),
                        pte.executable(),
                    )
                });
                let current_translate = current_page_table.translate_va(fault_va);
                panic!(
                    "[kernel] page fault in kernel mode: trap_type={:?}, bad addr={:#x}, current_root_ppn={:#x}, current_translate={:?}, pte_info={:?}, ctx={:#x?}",
                    trap_type, _paddr, current_root, current_translate, pte_info, ctx
                );
            }
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
            if log::log_enabled!(log::Level::Debug) && tick % MEMORY_DEBUG_INTERVAL == 0 {
                mm::heap_allocator::print_heap_stats();
                mm::frame_allocator::print_frame_stats();
                if let Some(cache) = crate::fs::page::pagecache::PAGE_CACHE.try_lock() {
                    let stats = cache.stats();
                    let swap = mm::swap::stats();
                    debug!(
                        "[MEMDEBUG] page_cache: pages={} dirty={} disk_pages={} disk_dirty={} tmpfs={} tmpfs_swapped={} fat32={} ext4={} unknown={} writeback_queue={} swap_used={} swap_free={} swap_total={}",
                        stats.pages,
                        stats.dirty_pages,
                        stats.disk_pages,
                        stats.dirty_disk_pages,
                        stats.tmpfs_pages,
                        stats.swapped_tmpfs_pages,
                        stats.fat32_pages,
                        stats.ext4_pages,
                        stats.unknown_pages,
                        crate::fs::writeback::pending_count(),
                        swap.used_slots,
                        swap.free_slots,
                        swap.total_slots
                    );
                } else {
                    let swap = mm::swap::stats();
                    debug!(
                        "[MEMDEBUG] page_cache: lock busy writeback_queue={} swap_used={} swap_free={} swap_total={}",
                        crate::fs::writeback::pending_count(),
                        swap.used_slots,
                        swap.free_slots,
                        swap.total_slots
                    );
                }
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
                        let mut inner = process.inner_exclusive_access();
                        if inner.is_zombie {
                            inner.alarm_deadline_us = None;
                            inner.itimer_real_deadline = None;
                            inner.itimer_real_interval = None;
                            to_remove.push(*pid);
                            continue;
                        }
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
                if process.inner_exclusive_access().is_zombie {
                    crate::task::manager::TIMER_PROCS
                        .lock()
                        .remove(&process.getpid());
                    continue;
                }
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

            // 页缓存/内存压力检查：timer 只发起请求，实际写回放到 syscall 返回路径。
            const WRITEBACK_INTERVAL_TICKS: usize = 10;
            if tick % WRITEBACK_INTERVAL_TICKS == 0 {
                crate::mm::reclaim::poll_background_reclaim();
            }
            polyhal::timer::set_next_timer(Duration::from_millis(10));
            // set_next_trigger();

            check_futex_timeouts();
            suspend_current_and_run_next();
        }
        _ => {
            warn!("unsuspended trap type: {:?}", trap_type);
            exit_current_and_run_next(-(Signal::SigAbrt.as_i32()));
        }
    }
    // handle signals (handle the sent signal)
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

    let current_task_for_return = current_task();
    let mut return_state = current_task_for_return
        .as_ref()
        .map(|task| trap_return_state(task));
    // 返回用户态前处理 pending 的异步信号。无 pending 时只读取一次 task/process 状态。
    if let Some(state) = return_state.as_ref() {
        if state.has_pending_signal {
            handle_signals(ctx);
            return_state = current_task_for_return
                .as_ref()
                .map(|task| trap_return_state(task));
        }
    }

    // 如果 pending 了页缓存回刷/内存回收，在 syscall 返回路径中做少量延迟写回。
    if matches!(trap_type, TrapType::SysCall) {
        let reclaim_requested = crate::mm::reclaim::take_background_reclaim_request();
        let writeback_requested = crate::fs::writeback::take_writeback_request();
        if reclaim_requested || writeback_requested || crate::mm::reclaim::below_low_watermark() {
            if let Some(task) = current_task_for_return.as_ref() {
                if let Some(process) = task.process.upgrade() {
                    let mut files = Vec::new();
                    if let Some(inner) = process.inner_try_access() {
                        for fd in 0..inner.fd_table.len() {
                            if let Some(file) = inner.fd_table[fd].as_ref() {
                                files.push(file.clone());
                            }
                        }
                    }
                    for file in files {
                        crate::fs::writeback::queue_file(file);
                    }
                }
            }
            crate::fs::writeback::drain_some(crate::mm::reclaim::writeback_budget());
            crate::mm::reclaim::trim_clean_page_cache_to_limit();
            if crate::fs::writeback::has_pending_writeback()
                || crate::mm::reclaim::below_high_watermark()
            {
                crate::mm::reclaim::request_background_reclaim();
            }
        }
    }

    // 如果当前进程已被标记为 zombie（如收到默认终止信号），直接退出当前任务
    if let Some(state) = return_state {
        if state.process_missing {
            exit_current_and_run_next(0);
            return;
        }
        if let Some(exit_code) = state.task_exit_code {
            exit_current_and_run_next(exit_code);
            return;
        }
        if let Some(exit_code) = state.process_exit_code {
            exit_current_and_run_next(exit_code);
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
            safe fn _skernel();
            safe fn ekernel();
        }

        let kernel_start_va = _skernel as usize;
        let kernel_end_va = ekernel as usize;
        let kernel_start_pa = kernel_start_va - VIRT_ADDR_START;
        let kernel_end_pa = kernel_end_va - VIRT_ADDR_START;

        println!("Kairix kernel booting");
        println!(
            "kernel image virt {:#x}..{:#x}, phys {:#x}..{:#x}",
            kernel_start_va, kernel_end_va, kernel_start_pa, kernel_end_pa
        );

        println!("init logging");
        logging::init();
        println!("logging initialized");
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
        embedded::install_runtime_files();
        println!("init swap");
        mm::swap::init();
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
