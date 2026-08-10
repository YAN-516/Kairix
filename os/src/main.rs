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
use polyhal::utils::addr::PhysPageNum;
use trap::_set_sum_bit;
use trap::handle_page_fault;
#[cfg(board = "visionfive2")]
#[path = "boards/visionfive2.rs"]
mod board;
#[cfg(board = "2k1000")]
#[path = "boards/2k1000.rs"]
mod board;
#[cfg(not(any(board = "visionfive2", board = "2k1000")))]
#[path = "boards/qemu.rs"]
mod board;
use crate::mm::vm_set::VMSpace;
use crate::timer::set_next_trigger;
use crate::vm_set::PageFaultError;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
/// Interrupt accounting for `/proc/interrupts`.
pub mod interrupts;
pub mod lang_items;
mod logging;
pub(crate) mod ltp;
pub mod mm;
mod net;
///
#[cfg(target_arch = "riscv64")]
pub mod sbi;
/// Security policy modules.
pub mod security;
mod socket;
pub mod ssh;

///
#[cfg(target_arch = "loongarch64")]
pub mod sbi_la;

pub mod sync;
pub mod syscall;
#[allow(missing_docs)]
pub mod task;

pub mod timer;

#[cfg(target_arch = "riscv64")]
fn trap_from_user(ctx: &polyhal_trap::trapframe::TrapFrame) -> bool {
    ctx.from_user()
}

#[cfg(target_arch = "riscv64")]
fn user_general_registers(ctx: &TrapFrame) -> [usize; 32] {
    ctx.x
}

#[cfg(target_arch = "loongarch64")]
fn user_general_registers(ctx: &TrapFrame) -> [usize; 32] {
    ctx.regs
}

fn log_unexpected_syscall_context_change(
    syscall_id: usize,
    before: &[usize; 32],
    after: &[usize; 32],
) {
    // Successful exec and rt_sigreturn deliberately replace the complete
    // context. Every other Linux syscall may change only its return register.
    if matches!(syscall_id, 139 | 221 | 281) {
        return;
    }
    #[cfg(target_arch = "riscv64")]
    const RETURN_REGISTER: usize = 10;
    #[cfg(target_arch = "loongarch64")]
    const RETURN_REGISTER: usize = 4;

    let mut changed_mask = 0u32;
    let mut first = None;
    for index in 1..32 {
        if index != RETURN_REGISTER && before[index] != after[index] {
            changed_mask |= 1u32 << index;
            if first.is_none() {
                first = Some((index, before[index], after[index]));
            }
        }
    }
    if let Some((index, old, new)) = first {
        let (pid, tid, owner_cpu) = current_task()
            .map(|task| {
                let tid = task.inner_exclusive_access().global_tid;
                (task.process_id(), tid, task.on_cpu_index())
            })
            .unwrap_or((0, 0, None));
        error!(
            "[USER_CONTEXT_INVARIANT] cpu={} owner_cpu={:?} pid={} tid={} syscall={} changed_mask={:#x} first_reg={} old={:#x} new={:#x}",
            polyhal::arch::hart_id(),
            owner_cpu,
            pid,
            tid,
            syscall_id,
            changed_mask,
            index,
            old,
            new,
        );
    }
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
use crate::syscall::signal::deliver_signal;
use drivers::block::*;
use polyhal_trap::trap::init_trap;
use polyhal_trap::trap::*;
use polyhal_trap::trapframe::*;
use syscall::{SYSCALL_EXECVE, syscall};
use task::*;

/// Temporarily admit hardware interrupts while retaining the kernel's
/// non-preemptible execution model. Nested kernel timer traps only perform
/// accounting and timer re-arming; they do not switch this continuation out.
struct InterruptibleKernelSection {
    admitted_interrupts: bool,
}

static KERNEL_PROGRESS_IRQ_ACTIVE: [AtomicBool; config::MAX_CPU_NUM] =
    [const { AtomicBool::new(false) }; config::MAX_CPU_NUM];
static KERNEL_PROGRESS_IRQ_SAVED_MASK: [AtomicUsize; config::MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; config::MAX_CPU_NUM];

/// Restrict the current CPU to the lock-free timer/IPI paths. The caller must
/// keep global interrupts disabled until all per-CPU state has been published.
fn restrict_kernel_progress_interrupts() {
    let cpu = polyhal::arch::hart_id();
    assert!(cpu < config::MAX_CPU_NUM);

    #[cfg(target_arch = "riscv64")]
    let saved_mask = riscv::register::sie::read().bits();
    #[cfg(target_arch = "loongarch64")]
    let saved_mask = loongArch64::register::ecfg::read().lie().bits();
    KERNEL_PROGRESS_IRQ_SAVED_MASK[cpu].store(saved_mask, Ordering::Relaxed);

    #[cfg(target_arch = "riscv64")]
    unsafe {
        riscv::register::sie::clear_sext();
        riscv::register::sie::set_ssoft();
        riscv::register::sie::set_stimer();
    }
    #[cfg(target_arch = "loongarch64")]
    loongArch64::register::ecfg::set_lie(
        loongArch64::register::ecfg::LineBasedInterrupt::TIMER
            | loongArch64::register::ecfg::LineBasedInterrupt::IPI,
    );
    KERNEL_PROGRESS_IRQ_ACTIVE[cpu].store(true, Ordering::Release);
}

/// Stop the restricted interrupt window on this physical CPU and restore its
/// own mask. Returns whether a writeback continuation owned the window.
pub(crate) fn suspend_kernel_progress_interrupts() -> bool {
    IRQ::int_disable();
    let cpu = polyhal::arch::hart_id();
    if cpu >= config::MAX_CPU_NUM || !KERNEL_PROGRESS_IRQ_ACTIVE[cpu].swap(false, Ordering::AcqRel)
    {
        return false;
    }
    let saved_mask = KERNEL_PROGRESS_IRQ_SAVED_MASK[cpu].load(Ordering::Relaxed);
    #[cfg(target_arch = "riscv64")]
    unsafe {
        if saved_mask & (1 << 1) != 0 {
            riscv::register::sie::set_ssoft();
        } else {
            riscv::register::sie::clear_ssoft();
        }
        if saved_mask & (1 << 5) != 0 {
            riscv::register::sie::set_stimer();
        } else {
            riscv::register::sie::clear_stimer();
        }
        if saved_mask & (1 << 9) != 0 {
            riscv::register::sie::set_sext();
        } else {
            riscv::register::sie::clear_sext();
        }
    }
    #[cfg(target_arch = "loongarch64")]
    loongArch64::register::ecfg::set_lie(
        loongArch64::register::ecfg::LineBasedInterrupt::from_bits_truncate(saved_mask),
    );
    true
}

/// Recreate a suspended writeback interrupt window using the mask belonging
/// to the CPU on which the continuation has just resumed.
pub(crate) fn resume_kernel_progress_interrupts() {
    debug_assert!(!IRQ::int_enabled());
    restrict_kernel_progress_interrupts();
}

impl InterruptibleKernelSection {
    fn enter() -> Self {
        let admitted_interrupts = !IRQ::int_enabled();
        if admitted_interrupts {
            // Only the two lock-free/re-entrant kernel interrupt paths are
            // admitted here. In particular, RISC-V external interrupts and
            // LoongArch hardware interrupt lines still have no nested-kernel
            // dispatcher in this kernel and must remain masked.
            restrict_kernel_progress_interrupts();
            IRQ::int_enable();
        }
        Self {
            admitted_interrupts,
        }
    }
}

impl Drop for InterruptibleKernelSection {
    fn drop(&mut self) {
        if self.admitted_interrupts {
            assert!(suspend_kernel_progress_interrupts());
        }
    }
}

/// 主核初始化完成标志，用于同步从核启动
static INIT_COMPLETED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static TIMER_MAINTENANCE_PENDING: AtomicBool = AtomicBool::new(false);
static TIMER_MAINTENANCE_RUNNING: AtomicBool = AtomicBool::new(false);
static TIMER_TICK_COUNT: AtomicUsize = AtomicUsize::new(0);
static LAST_TIMER_MAINTENANCE_NS: AtomicUsize = AtomicUsize::new(0);
static LAST_MEMORY_DEBUG_BUCKET: AtomicUsize = AtomicUsize::new(0);
static LAST_WRITEBACK_BUCKET: AtomicUsize = AtomicUsize::new(0);

struct TimerMaintenanceGuard;

impl Drop for TimerMaintenanceGuard {
    fn drop(&mut self) {
        TIMER_MAINTENANCE_RUNNING.store(false, Ordering::Release);
    }
}

/// Request one global wall-clock timer-maintenance tick at most every 10ms.
///
/// Both hardware timer interrupts and the IRQ-disabled idle scheduler use this
/// helper, so SMP CPUs do not multiply the maintenance clock.
pub(crate) fn request_timer_maintenance() {
    const TIMER_MAINTENANCE_INTERVAL_NS: usize = 10_000_000;

    let now_ns = polyhal::timer::current_time().as_nanos() as usize;
    let mut previous = LAST_TIMER_MAINTENANCE_NS.load(Ordering::Acquire);
    loop {
        if now_ns.saturating_sub(previous) < TIMER_MAINTENANCE_INTERVAL_NS {
            return;
        }
        match LAST_TIMER_MAINTENANCE_NS.compare_exchange_weak(
            previous,
            now_ns,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                TIMER_TICK_COUNT.fetch_add(1, Ordering::Release);
                TIMER_MAINTENANCE_PENDING.store(true, Ordering::Release);
                return;
            }
            Err(observed) => previous = observed,
        }
    }
}

pub(crate) fn service_deferred_timer_maintenance() {
    crate::task::processor::record_scheduler_phase(130, None);
    if !TIMER_MAINTENANCE_PENDING.swap(false, Ordering::AcqRel) {
        return;
    }
    crate::task::processor::record_scheduler_phase(131, None);
    if TIMER_MAINTENANCE_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        TIMER_MAINTENANCE_PENDING.store(true, Ordering::Release);
        return;
    }
    let _guard = TimerMaintenanceGuard;
    let tick = TIMER_TICK_COUNT.load(Ordering::Acquire);
    crate::task::processor::record_scheduler_phase(132, None);

    const MEMORY_DEBUG_INTERVAL: usize = 500;
    let memory_debug_bucket = tick / MEMORY_DEBUG_INTERVAL;
    let should_print_memory_debug =
        memory_debug_bucket > LAST_MEMORY_DEBUG_BUCKET.swap(memory_debug_bucket, Ordering::AcqRel);
    if log::log_enabled!(log::Level::Debug) && should_print_memory_debug {
        mm::heap_allocator::print_heap_stats();
        mm::frame_allocator::print_frame_stats();
        if let Some(stats) = crate::fs::page::pagecache::PAGE_CACHE.try_snapshot() {
            let swap = mm::swap::stats();
            debug!(
                "[MEMDEBUG] page_cache: pages={} dirty={} disk_pages={} disk_dirty={} tmpfs={} tmpfs_swapped={} fat32={} ext4={} unknown={} lru_order={} lru_gen={} writeback_queue={} swap_used={} swap_free={} swap_total={}",
                stats.pages,
                stats.dirty_pages,
                stats.disk_pages,
                stats.dirty_disk_pages,
                stats.tmpfs_pages,
                stats.swapped_tmpfs_pages,
                stats.fat32_pages,
                stats.ext4_pages,
                stats.unknown_pages,
                stats.lru_order_entries,
                stats.lru_gen_entries,
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
        let page_cache_atomic = crate::fs::page::pagecache::atomic_stats();
        let tmpfs_inode = crate::fs::tmpfs::inode::tmpfs_inode_stats();
        debug!(
            "[MEMDEBUG] page_cache_atomic: pages={} tmpfs={} fat32={} ext4={} unknown={} insert_count={} remove_count={} tmpfs_inode_current={} tmpfs_xattrs={} tmpfs_xattr_bytes={}",
            page_cache_atomic.pages,
            page_cache_atomic.tmpfs_pages,
            page_cache_atomic.fat32_pages,
            page_cache_atomic.ext4_pages,
            page_cache_atomic.unknown_pages,
            page_cache_atomic.insert_count,
            page_cache_atomic.remove_count,
            tmpfs_inode.current,
            tmpfs_inode.xattrs,
            tmpfs_inode.xattr_bytes
        );
    }
    crate::task::processor::record_scheduler_phase(133, None);

    let now_us = polyhal::timer::current_time().as_micros();
    let now_ticks = crate::timer::get_time();
    let mut expired_processes = Vec::new();
    let mut to_remove = Vec::new();
    let Some(mut timer_procs) = crate::task::manager::TIMER_PROCS.try_lock() else {
        TIMER_MAINTENANCE_PENDING.store(true, Ordering::Release);
        return;
    };
    crate::task::processor::record_scheduler_phase(134, None);
    for (pid, process) in timer_procs.iter() {
        crate::task::processor::record_scheduler_phase(135, None);
        let process = Arc::clone(process);
        let Some(mut inner) = process.try_inner_exclusive_access() else {
            continue;
        };
        let (alarm_expired, itimer_expired, still_active) = {
            if inner.is_zombie {
                inner.alarm_deadline_us = None;
                inner.itimer_real_deadline = None;
                inner.itimer_real_interval = None;
                to_remove.push(*pid);
                continue;
            }
            let alarm = inner.alarm_deadline_us.map_or(false, |d| now_us >= d);
            let itimer = inner.itimer_real_deadline.map_or(false, |d| now_ticks >= d);
            if alarm {
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
            if itimer {
                if let Some(interval) = inner.itimer_real_interval {
                    let new_deadline = inner.itimer_real_deadline.unwrap_or(0) + interval;
                    inner.itimer_real_deadline = Some(new_deadline);
                } else {
                    inner.itimer_real_deadline = None;
                }
            }
            let still = inner.alarm_deadline_us.is_some() || inner.itimer_real_deadline.is_some();
            (alarm, itimer, still)
        };
        drop(inner);
        if alarm_expired || itimer_expired {
            expired_processes.push((process.clone(), alarm_expired, itimer_expired));
        }
        if !still_active {
            to_remove.push(*pid);
        }
    }
    crate::task::processor::record_scheduler_phase(136, None);
    for pid in to_remove {
        timer_procs.remove(&pid);
    }
    drop(timer_procs);
    crate::task::processor::record_scheduler_phase(137, None);

    for (process, alarm_expired, itimer_expired) in expired_processes {
        error!(
            "timer: SIGALRM fired for pid={}, alarm={}, itimer={}",
            process.getpid(),
            alarm_expired,
            itimer_expired
        );
        deliver_signal(&process, Signal::SigAlrm);
    }
    crate::task::processor::record_scheduler_phase(138, None);

    const WRITEBACK_INTERVAL_TICKS: usize = 10;
    let writeback_bucket = tick / WRITEBACK_INTERVAL_TICKS;
    if writeback_bucket > LAST_WRITEBACK_BUCKET.swap(writeback_bucket, Ordering::AcqRel) {
        crate::task::processor::record_scheduler_phase(139, None);
        crate::mm::reclaim::poll_background_reclaim();
        crate::task::processor::record_scheduler_phase(140, None);
    }
}

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
    #[cfg(board = "visionfive2")]
    {
        #[cfg(vf2_harts = "1")]
        const APPLICATION_HARTS: [usize; 1] = [1];
        #[cfg(vf2_harts = "4")]
        const APPLICATION_HARTS: [usize; 4] = [1, 2, 3, 4];

        for hart_id in APPLICATION_HARTS {
            if hart_id == id {
                continue;
            }
            let result = crate::sbi::hart_start(hart_id, 0);
            if result.is_ok() {
                info!("[kernel] VisionFive 2 hart {} start requested", hart_id);
            } else {
                error!(
                    "[kernel] VisionFive 2 hart {} start failed: {:?}",
                    hart_id, result
                );
            }
        }
        return;
    }

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

fn read_mapped_user_bytes(
    page_table: &polyhal::PageTable,
    start: usize,
    output: &mut [u8],
) -> usize {
    let mut copied = 0usize;
    while copied < output.len() {
        let va = start.saturating_add(copied);
        let vpn = VirtAddr::from(va).floor();
        let Some(pte) = page_table.find_pte(vpn) else {
            break;
        };
        if !pte.is_valid() || pte.is_table() {
            break;
        }
        let page_offset = va % polyhal::PageTable::PAGE_SIZE;
        let copy_len =
            (output.len() - copied).min(polyhal::PageTable::PAGE_SIZE.saturating_sub(page_offset));
        if copy_len == 0 {
            break;
        }
        output[copied..copied + copy_len]
            .copy_from_slice(&pte.ppn().get_bytes_array()[page_offset..page_offset + copy_len]);
        copied += copy_len;
    }
    copied
}

fn print_user_crash_mapping(pc: usize) {
    let page_table = polyhal::PageTable::current();
    let vpn = VirtAddr::from(pc).floor();
    let mapping = page_table.find_pte(vpn).map(|pte| {
        let ppn = pte.ppn();
        let sample = (pte.is_valid() && !pte.is_table()).then(|| {
            let page_offset = pc % polyhal::PageTable::PAGE_SIZE;
            let sample_len = 4usize.min(polyhal::PageTable::PAGE_SIZE - page_offset);
            let bytes = ppn.get_bytes_array();
            let mut sample = 0u32;
            for (index, byte) in bytes[page_offset..page_offset + sample_len]
                .iter()
                .enumerate()
            {
                sample |= u32::from(*byte) << (index * 8);
            }
            (sample, sample_len)
        });
        (pte.0, ppn.0, pte.flags(), sample)
    });
    error!(
        "[USER_CRASH_MAP] pc={:#x} vpn={:#x} mapping={:?}",
        pc, vpn.0, mapping,
    );
    let code_start = pc.saturating_sub(16);
    let mut code_window = [0u8; 48];
    let code_len = read_mapped_user_bytes(&page_table, code_start, &mut code_window);
    error!(
        "[USER_CRASH_CODE_WINDOW] pc={:#x} start={:#x} len={} bytes={:02x?}",
        pc,
        code_start,
        code_len,
        &code_window[..code_len],
    );
    crate::mm::print_user_crash_vma(pc);
}

fn print_user_crash_registers(ctx: &TrapFrame, pc: usize) {
    #[cfg(target_arch = "riscv64")]
    let regs = &ctx.x;
    #[cfg(target_arch = "loongarch64")]
    let regs = &ctx.regs;
    error!(
        "[USER_CRASH_REGS] pc={:#x} r1_8={:x?} r9_16={:x?} r17_24={:x?} r25_31={:x?}",
        pc,
        &regs[1..9],
        &regs[9..17],
        &regs[17..25],
        &regs[25..32],
    );
    let sp = ctx[TrapFrameArgs::SP];
    let page_table = polyhal::PageTable::current();
    let mut stack_bytes = [0u8; 8 * core::mem::size_of::<usize>()];
    let stack_len = read_mapped_user_bytes(&page_table, sp, &mut stack_bytes);
    let mut stack_words = [0usize; 8];
    let word_count = stack_len / core::mem::size_of::<usize>();
    for (index, chunk) in stack_bytes
        .chunks_exact(core::mem::size_of::<usize>())
        .take(word_count)
        .enumerate()
    {
        let mut bytes = [0u8; core::mem::size_of::<usize>()];
        bytes.copy_from_slice(chunk);
        stack_words[index] = usize::from_ne_bytes(bytes);
    }
    error!(
        "[USER_CRASH_STACK_WINDOW] pc={:#x} sp={:#x} bytes={} words={:x?}",
        pc,
        sp,
        stack_len,
        &stack_words[..word_count],
    );
}

fn print_user_crash_signal_state(
    task: &Arc<crate::task::TaskControlBlock>,
    process: &Arc<crate::task::ProcessControlBlock>,
    signal: Signal,
) {
    let task_state = task.try_inner_exclusive_access().map(|inner| {
        (
            inner.pending_signals.bits(),
            inner.blocked_signals.bits(),
            inner.need_signal_handle,
        )
    });
    let process_state = process.try_inner_exclusive_access().map(|inner| {
        let action = inner.signals_handler.lock().get(signal);
        (
            inner.pending_signals.bits(),
            inner.blocked_signals.bits(),
            inner.need_signal_handle,
            action.sa_handler,
            action.sa_flags,
            action.sa_restorer,
        )
    });
    error!(
        "[USER_CRASH_SIGNAL] pid={} signal={} task={:?} process={:?}",
        process.getpid(),
        signal.as_i32(),
        task_state,
        process_state,
    );
}

fn try_trap_return_state(task: &crate::task::TaskControlBlock) -> Option<TrapReturnState> {
    if task.exec_exit_requested() {
        return Some(TrapReturnState {
            process_missing: false,
            task_exit_code: Some(0),
            process_exit_code: None,
            has_pending_signal: false,
        });
    }
    let Some(process) = task.process.upgrade() else {
        return Some(TrapReturnState {
            process_missing: true,
            task_exit_code: None,
            process_exit_code: None,
            has_pending_signal: false,
        });
    };
    let (task_status, task_exit_code, task_pending, task_blocked, task_needs_signal) = {
        let t_inner = task.try_inner_exclusive_access()?;
        (
            t_inner.task_status,
            t_inner.exit_code,
            t_inner.pending_signals,
            t_inner.blocked_signals,
            t_inner.need_signal_handle,
        )
    };
    if task_status == crate::task::TaskStatus::Zombie {
        return Some(TrapReturnState {
            process_missing: false,
            task_exit_code: Some(task_exit_code.unwrap_or(0)),
            process_exit_code: None,
            has_pending_signal: false,
        });
    }
    let (proc_is_zombie, proc_exit_code, proc_pending, proc_needs_signal) = {
        let p_inner = process.try_inner_exclusive_access()?;
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
    Some(TrapReturnState {
        process_missing: false,
        task_exit_code: None,
        process_exit_code: proc_is_zombie.then_some(proc_exit_code),
        has_pending_signal,
    })
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
    // Preserve the origin before any syscall, signal, or exception handler can
    // rewrite the saved privilege status. Every direct trap return to user
    // mode must pass through prepare_user_return(); task_entry() only covers a
    // task's initial entry and cannot repair later syscall/fault returns.
    let trapped_from_user = trap_from_user(ctx);
    if trapped_from_user {
        if let Some(task) = current_task() {
            task.note_user_trap();
        }
        crate::task::processor::publish_current_user_context_nolock(
            ctx.pc(),
            ctx[TrapFrameArgs::RA],
            ctx[TrapFrameArgs::SP],
        );
        crate::task::processor::record_current_task_kernel_phase(10);
        if let Some(task) = current_task() {
            let cpu = polyhal::arch::hart_id();
            if !task.is_on_cpu_at(cpu) {
                error!(
                    "[USER_TASK_CPU_INVARIANT] trap_cpu={} owner_cpu={:?} pid={} tid={} trap_ctx={:#x} pc={:#x}",
                    cpu,
                    task.on_cpu_index(),
                    task.process_id(),
                    task.inner_exclusive_access().global_tid,
                    ctx as *mut TrapFrame as usize,
                    ctx.pc(),
                );
            }
        }
        // Stop advertising this CPU as a consumer of user TLB entries before
        // taking any kernel lock or mutating a shared address space.
        polyhal::multicore::mark_current_cpu_kernel_entry();
    }
    _set_sum_bit();
    if matches!(trap_type, TrapType::Timer) && trapped_from_user {
        if let Some(task) = current_task() {
            task.record_user_context(ctx);
        }
    }
    // Fast syscall path skips this defensive orphan check; the scheduler already
    // filters tasks whose PCB has disappeared.
    if trapped_from_user && !matches!(trap_type, TrapType::SysCall | TrapType::Breakpoint) {
        if let Some(task) = current_task() {
            if task.process.upgrade().is_none() {
                drop(task);
                crate::task::exit_current_and_run_next(0);
            }
        }
    }
    match trap_type {
        TrapType::Handled => {
            if trapped_from_user && current_task().is_some() {
                crate::task::prepare_user_return(ctx);
            }
            return;
        }
        TrapType::Reschedule => {
            // The ready-queue publication precedes the IPI. If the target left
            // idle and entered user mode before consuming it, preempt here so
            // the kick cannot degrade into an ineffective trap-and-return.
            preempt_current_and_run_next();
        }
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
            let registers_before = user_general_registers(ctx);
            ctx.syscall_ok();
            let args = ctx.args();
            // get system call return value
            let syscall_id = ctx[TrapFrameArgs::SYSCALL];
            // if syscall_id == 260 || syscall_id == 95 {
            //     println!("!!!SYSCALL{}!!! pid={}", syscall_id, current_task().unwrap().process.upgrade().unwrap().getpid());
            // }

            crate::task::processor::record_current_task_kernel_phase(11);
            let result = syscall(syscall_id, [
                args[0], args[1], args[2], args[3], args[4], args[5],
            ]);
            crate::task::processor::record_current_task_kernel_phase(12);
            let registers_after = user_general_registers(ctx);
            log_unexpected_syscall_context_change(syscall_id, &registers_before, &registers_after);
            match result {
                // Successful execve has replaced the trap context; keep a0/a1 as argc/argv.
                Ok(_val) if matches!(syscall_id, SYSCALL_EXECVE | 281) => {}
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
                let kernel_token = crate::mm::vm_set::kernel_page_table_token();
                let kernel_translate = (kernel_token != 0)
                    .then(|| polyhal::PageTable::from_token(kernel_token).translate_va(fault_va))
                    .flatten();
                log::error!(
                    "[KERNEL_PAGE_FAULT_DETAIL] cpu={} current_token={:#x} kernel_token={:#x} fault_va={:#x} current_translate={:?} kernel_translate={:?} ext4_flush={:?} block_io={:?}",
                    polyhal::arch::hart_id(),
                    current_page_table.token(),
                    kernel_token,
                    _paddr,
                    current_translate,
                    kernel_translate,
                    crate::fs::lwext4::file::ext4_flush_stats(),
                    crate::drivers::block::virtio_blk::virtio_block_io_stats(),
                );
                panic!(
                    "[kernel] page fault in kernel mode: trap_type={:?}, bad addr={:#x}, current_root_ppn={:#x}, current_translate={:?}, pte_info={:?}, ctx={:#x?}",
                    trap_type, _paddr, current_root, current_translate, pte_info, ctx
                );
            }
            // info!("trap type {:?}", trap_type);
            match handle_page_fault(trap_type) {
                Some(PageFaultError::Normal) => {}
                Some(PageFaultError::BeyondFileSize) => {
                    let _pid = current_task()
                        .and_then(|task| task.process.upgrade())
                        .map(|process| process.getpid())
                        .unwrap_or(usize::MAX);
                    if let Some(task) = current_task() {
                        if let Some(process) = task.process.upgrade() {
                            error!(
                                "[USER_CRASH] signal=SIGBUS cpu={} pid={} pc={:#x} ra={:#x} sp={:#x} fault_addr={:#x} syscall={:?} syscall_stage={}",
                                polyhal::arch::hart_id(),
                                process.getpid(),
                                ctx.pc(),
                                ctx[TrapFrameArgs::RA],
                                ctx[TrapFrameArgs::SP],
                                _paddr,
                                task.active_syscall(),
                                task.active_syscall_stage(),
                            );
                            print_user_crash_mapping(ctx.pc());
                            print_user_crash_registers(ctx, ctx.pc());
                            print_user_crash_signal_state(&task, &process, Signal::SigBus);
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
                                exit_current_and_run_next(128 + Signal::SigBus.as_i32());
                            }
                        }
                    }
                }
                _ => {
                    let _pid = current_task()
                        .and_then(|task| task.process.upgrade())
                        .map(|process| process.getpid())
                        .unwrap_or(usize::MAX);

                    error!(
                        "[kernel] in application, bad addr = {:#x}, ctx: {:#x?} sending SIGSEGV.",
                        _paddr, ctx
                    );
                    if let Some(task) = current_task() {
                        if let Some(process) = task.process.upgrade() {
                            error!(
                                "[USER_CRASH] signal=SIGSEGV cpu={} pid={} pc={:#x} ra={:#x} sp={:#x} fault_addr={:#x} syscall={:?} syscall_stage={}",
                                polyhal::arch::hart_id(),
                                process.getpid(),
                                ctx.pc(),
                                ctx[TrapFrameArgs::RA],
                                ctx[TrapFrameArgs::SP],
                                _paddr,
                                task.active_syscall(),
                                task.active_syscall_stage(),
                            );
                            print_user_crash_mapping(ctx.pc());
                            print_user_crash_registers(ctx, ctx.pc());
                            print_user_crash_signal_state(&task, &process, Signal::SigSegv);
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
                                exit_current_and_run_next(128 + Signal::SigSegv.as_i32());
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
        TrapType::IllegalInstruction(detail) => {
            if let Some(task) = current_task() {
                if let Some(process) = task.process.upgrade() {
                    #[cfg(target_arch = "riscv64")]
                    let pc = ctx.sepc;
                    #[cfg(target_arch = "loongarch64")]
                    let pc = ctx.era;
                    let page_table = polyhal::PageTable::current();
                    let mut mapped_bytes = [0u8; 4];
                    let mapped_len = read_mapped_user_bytes(&page_table, pc, &mut mapped_bytes);
                    let mapped_instruction = u32::from_le_bytes(mapped_bytes) as usize;
                    #[cfg(target_arch = "riscv64")]
                    let status = unsafe { *(&ctx.sstatus as *const _ as *const usize) };
                    #[cfg(target_arch = "loongarch64")]
                    let status = ctx.prmd;
                    crate::trap::record_user_sigill(
                        process.getpid(),
                        pc,
                        detail,
                        mapped_instruction,
                        mapped_len,
                        status,
                    );
                    error!(
                        "[USER_SIGILL] cpu={} pid={} pc={:#x} detail={:#x}",
                        polyhal::arch::hart_id(),
                        process.getpid(),
                        pc,
                        detail,
                    );
                    error!(
                        "[USER_CRASH] signal=SIGILL cpu={} pid={} pc={:#x} ra={:#x} sp={:#x} instruction={:#x} syscall={:?} syscall_stage={}",
                        polyhal::arch::hart_id(),
                        process.getpid(),
                        pc,
                        ctx[TrapFrameArgs::RA],
                        ctx[TrapFrameArgs::SP],
                        detail,
                        task.active_syscall(),
                        task.active_syscall_stage(),
                    );
                    print_user_crash_mapping(pc);
                    print_user_crash_registers(ctx, pc);
                    print_user_crash_signal_state(&task, &process, Signal::SigIll);
                    let mut t_inner = task.inner_exclusive_access();
                    t_inner.blocked_signals.remove(Signal::SigIll);
                    drop(t_inner);
                    let mut p_inner = process.inner_exclusive_access();
                    p_inner.blocked_signals.remove(Signal::SigIll);
                    drop(p_inner);
                    deliver_signal(&process, Signal::SigIll);
                }
            }
        }
        TrapType::FloatingPointException(_) => {
            if let Some(task) = current_task() {
                if let Some(process) = task.process.upgrade() {
                    error!(
                        "[USER_CRASH] signal=SIGFPE cpu={} pid={} pc={:#x} ra={:#x} sp={:#x} syscall={:?} syscall_stage={}",
                        polyhal::arch::hart_id(),
                        process.getpid(),
                        ctx.pc(),
                        ctx[TrapFrameArgs::RA],
                        ctx[TrapFrameArgs::SP],
                        task.active_syscall(),
                        task.active_syscall_stage(),
                    );
                    print_user_crash_mapping(ctx.pc());
                    print_user_crash_registers(ctx, ctx.pc());
                    print_user_crash_signal_state(&task, &process, Signal::SigFpe);
                    let mut t_inner = task.inner_exclusive_access();
                    t_inner.blocked_signals.remove(Signal::SigFpe);
                    drop(t_inner);
                    let mut p_inner = process.inner_exclusive_access();
                    p_inner.blocked_signals.remove(Signal::SigFpe);
                    drop(p_inner);
                    deliver_signal(&process, Signal::SigFpe);
                }
            }
        }
        TrapType::Timer => {
            crate::interrupts::record_timer_interrupt();
            // The idle-loop watchdog cannot observe a CPU that remains inside
            // one syscall. Track execution time on the TCB so migrations do
            // not reset the evidence needed to distinguish a syscall stall
            // from a scheduler-idle stall.
            const SYSCALL_STALL_TICKS: usize = 500;
            const SYSCALL_LONG_STALL_INTERVAL: usize = 5_000;
            if trapped_from_user {
                if let Some(task) = current_task() {
                    if let Some((syscall_id, syscall_ticks)) = task.tick_active_syscall() {
                        if syscall_ticks == SYSCALL_STALL_TICKS
                            || syscall_ticks % SYSCALL_LONG_STALL_INTERVAL == 0
                        {
                            let pid = task.process_id();
                            log::error!(
                                "[SYSCALL_STALL_VISIBLE] cpu={} pid={} syscall={} ticks={} ready_queued={} on_cpu={} context={:?}",
                                polyhal::arch::hart_id(),
                                pid,
                                syscall_id,
                                syscall_ticks,
                                task.is_ready_queued(),
                                task.is_on_cpu(),
                                task.user_context_snapshot(),
                            );
                            warn!(
                                "[SYSCALL_STALL] cpu={} pid={} syscall={} ticks={} ready_queued={} on_cpu={} context={:?}",
                                polyhal::arch::hart_id(),
                                pid,
                                syscall_id,
                                syscall_ticks,
                                task.is_ready_queued(),
                                task.is_on_cpu(),
                                task.user_context_snapshot(),
                            );
                        }
                    }
                }
            }
            request_timer_maintenance();
            crate::interrupts::program_next_timer(Duration::from_millis(10));
            // set_next_trigger();
            // Timeout-table scans may acquire global futex/POSIX-timer locks
            // and wake tasks.  They run at the scheduler safe point after this
            // preemption instead of extending a hard timer trap with IRQs off.
            // User execution is preemptible. A timer admitted by an explicitly
            // interruptible kernel section must only re-arm/account here: an
            // asynchronous switch at an arbitrary Rust/C instruction would
            // violate the kernel's continuation and lock invariants.
            if trapped_from_user {
                preempt_current_and_run_next();
            }
        }
        _ => {
            warn!("unsuspended trap type: {:?}", trap_type);
            if !trap_from_user(ctx) || current_task().is_none() {
                panic!(
                    "[kernel] unexpected trap without runnable task: trap_type={:?}",
                    trap_type
                );
            }
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

    // Kernel-origin nested interrupts (notably a shootdown IPI admitted while
    // a page-fault handler owns the PCB lock) must not execute the user-return
    // signal/zombie path. Doing so recursively acquires the same PCB lock.
    if !trapped_from_user {
        return;
    }

    let (current_task_for_return, mut return_state) = loop {
        let task = current_task();
        let Some(current) = task.as_ref() else {
            break (None, None);
        };
        if let Some(state) = try_trap_return_state(current) {
            break (task, Some(state));
        }
        drop(task);
        // A different CPU may be mutating this process's VM. Never spin on its
        // PCB in the return path; yield and retry before exposing user mode.
        suspend_current_and_run_next();
    };
    // 返回用户态前处理 pending 的异步信号。无 pending 时只读取一次 task/process 状态。
    if let Some(state) = return_state.as_ref() {
        if state.has_pending_signal {
            handle_signals(ctx);
            return_state = current_task_for_return.as_ref().map(|task| {
                loop {
                    if let Some(state) = try_trap_return_state(task) {
                        break state;
                    }
                    suspend_current_and_run_next();
                }
            });
        }
    }

    // 如果 pending 了页缓存回刷/内存回收，在 syscall 返回路径中做少量延迟写回。
    if matches!(trap_type, TrapType::SysCall) {
        let reclaim_requested = crate::mm::reclaim::take_background_reclaim_request();
        let writeback_requested = crate::fs::writeback::take_writeback_request();
        if reclaim_requested || writeback_requested || crate::mm::reclaim::below_low_watermark() {
            // Syscall traps arrive with hardware interrupts masked. Writeback
            // may block on filesystem locks and synchronously poll VirtIO for
            // many milliseconds, so keeping the trap's IRQ state here strands
            // timer and recovery IPIs. Admit interrupts for this bounded task-
            // context work; schedule() preserves this state across cooperative
            // continuation yields and restores the scheduler's IRQ-off state.
            {
                let _interruptible = InterruptibleKernelSection::enter();
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
                            crate::fs::writeback::queue_file_lazy(file);
                        }
                    }
                }
                crate::fs::writeback::drain_some(crate::mm::reclaim::writeback_budget());
                crate::mm::reclaim::trim_clean_page_cache_to_limit();
                if crate::mm::reclaim::below_high_watermark()
                    || (crate::fs::writeback::has_pending_writeback()
                        && crate::mm::reclaim::page_cache_needs_writeback())
                {
                    crate::mm::reclaim::request_background_reclaim();
                }
            }
        }
    }

    // 如果当前进程已被标记为 zombie（如收到默认终止信号），直接退出当前任务
    drop(current_task_for_return);
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

    // syscall/page-fault/signal handling returns directly through the
    // architecture trap vector rather than re-entering task_entry(). Restore
    // the user privilege/interrupt baseline and the per-CPU timer interrupt
    // mask at this common boundary so one damaged frame cannot leave a CPU
    // executing user code without preemption forever.
    if trapped_from_user && current_task().is_some() {
        crate::task::prepare_user_return(ctx);
    }
}

#[unsafe(no_mangle)]
///
pub extern "C" fn _secondary_for_arch(hart_id: usize) -> ! {
    if hart_id >= crate::config::MAX_CPU_NUM {
        log::error!(
            "cpu {} exceeds MAX_CPU_NUM={}, parking",
            hart_id,
            crate::config::MAX_CPU_NUM
        );
        loop {
            core::hint::spin_loop();
        }
    }
    // 初始化从核
    if hart_id != 0 {
        polyhal::println!("cpu {} waiting for init...", hart_id);
        wait_for_init();
        polyhal::println!("cpu {} init completed, starting scheduler", hart_id);
    }
    polyhal::println!("Secondary CPU {} starting", hart_id);

    // 初始化从核的 trap 处理
    polyhal::println!("cpu {} init trap", hart_id);
    init_trap();
    polyhal::println!("cpu {} set_next_trigger", hart_id);
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
    fn dealloc(&self, ppn: PhysPageNum, allocation_site: &'static core::panic::Location<'static>) {
        mm::frame_dealloc_with_site(ppn, allocation_site)
    }
}

#[polyhal::arch_entry]
fn main(id: usize, first: bool) -> bool {
    if first {
        // Install the logger before CPU validation so genuine early-boot
        // failures can still be reported through `log::error!`.
        logging::init();
    }
    if id >= crate::config::MAX_CPU_NUM {
        log::error!(
            "cpu {} exceeds MAX_CPU_NUM={}, parking",
            id,
            crate::config::MAX_CPU_NUM
        );
        loop {
            core::hint::spin_loop();
        }
    }
    if first {
        unsafe extern "C" {
            safe fn _skernel();
            safe fn ekernel();
        }

        let kernel_start_va = _skernel as usize;
        let kernel_end_va = ekernel as usize;
        let kernel_start_pa = kernel_start_va - VIRT_ADDR_START;
        let kernel_end_pa = kernel_end_va - VIRT_ADDR_START;

        polyhal::println!("Kairix kernel booting");
        polyhal::println!(
            "kernel image virt {:#x}..{:#x}, phys {:#x}..{:#x}",
            kernel_start_va,
            kernel_end_va,
            kernel_start_pa,
            kernel_end_pa
        );

        polyhal::println!("init logging");
        polyhal::println!("logging initialized");
        info!("[kernel] Hello, world!");
        polyhal::println!("init heap_allocator");
        heap_allocator::init_heap();
        polyhal::println!("init frame_allocator");
        frame_allocator::init_frame_allocator();
        heap_allocator::enable_heap_growth();
        common::init(&PageAllocImpl);
        init_trap();
        polyhal::println!("init mm");
        mm::init();
        #[cfg(all(board = "visionfive2", vf2_sd_smoke))]
        crate::drivers::block::vf2_sd::smoke_test_read_headers();
        // mm::remap_test();

        // IRQ::int_enable();
        // if IRQ::int_enabled(){
        //     println!("int enabled");
        // }
        init_processors();

        net::init();
        polyhal::println!("cpu {} init processors", id);

        // #[cfg(target_arch = "loongarch64")]
        // init_virtio_pci();

        polyhal::println!("init fs");
        fs::init();
        embedded::install_runtime_files();
        polyhal::println!("init swap");
        mm::swap::init();
        // println!("LIST APPS");
        // fs::list_apps();
        polyhal::println!("ADD INITPROC");
        task::add_initproc();
        polyhal::println!("processor_start");

        set_init_completed();
        processor_start(id);
    } else {
        polyhal::println!("cpu {} init processors", id);
        //mm::start_kvm();
        init_trap();
    }
    // println!("cpu {} enable_timer_interrupt", id);
    // trap::enable_timer_interrupt();
    polyhal::println!("cpu {} set_next_trigger", id);
    set_next_trigger();
    polyhal::println!("cpu {} run_tasks", id);
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
