//! Interrupt accounting exposed through `/proc/interrupts`.

use crate::config::MAX_CPU_NUM;
use alloc::string::String;
use core::fmt::Write;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;

/// Keep the table bounded so interrupt handlers never allocate or take a lock.
const MAX_INTERRUPT_NUMBER: usize = 256;

#[cfg(target_arch = "riscv64")]
const TIMER_INTERRUPT_NUMBER: usize = 5;

#[cfg(target_arch = "loongarch64")]
const TIMER_INTERRUPT_NUMBER: usize = 11;

static TIMER_INTERRUPT_COUNT: AtomicUsize = AtomicUsize::new(0);
static TIMER_INTERRUPT_HEARTBEATS_NS: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static TIMER_IRQ_REPORTED_SCHEDULER_HEARTBEAT: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static TIMER_PROGRAM_COUNTS: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static TIMER_PROGRAM_CURRENT_TICKS: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static TIMER_PROGRAM_DEADLINE_TICKS: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static TIMER_PROGRAM_ERRORS: [AtomicUsize; MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static EXTERNAL_INTERRUPT_COUNTS: [AtomicUsize; MAX_INTERRUPT_NUMBER] =
    [const { AtomicUsize::new(0) }; MAX_INTERRUPT_NUMBER];

/// Lock-free per-CPU evidence from the last hardware timer programming call.
#[derive(Debug, Clone, Copy)]
pub struct TimerProgrammingStats {
    /// Hardware tick sampled by the CPU producing the stall snapshot. RISC-V
    /// time is system-wide, so it can be compared with every stored deadline.
    pub observed_ticks: usize,
    /// Number of programming calls issued on each CPU.
    pub counts: [usize; MAX_CPU_NUM],
    /// Hardware tick sampled immediately before the last programming call.
    pub current_ticks: [usize; MAX_CPU_NUM],
    /// Absolute hardware deadline supplied to the timer implementation.
    pub deadline_ticks: [usize; MAX_CPU_NUM],
    /// Last SBI/HAL error value (`0` means success).
    pub errors: [usize; MAX_CPU_NUM],
}

/// Program this CPU's one-shot timer and publish the exact result for a
/// different CPU's stall snapshot. This path is allocation- and lock-free.
#[inline]
pub fn program_next_timer(interval: Duration) {
    let cpu = polyhal::arch::hart_id();
    let (current, deadline, error) = polyhal::timer::set_next_timer(interval);
    if cpu >= MAX_CPU_NUM {
        return;
    }
    TIMER_PROGRAM_CURRENT_TICKS[cpu].store(current as usize, Ordering::Relaxed);
    TIMER_PROGRAM_DEADLINE_TICKS[cpu].store(deadline as usize, Ordering::Relaxed);
    TIMER_PROGRAM_ERRORS[cpu].store(error, Ordering::Relaxed);
    TIMER_PROGRAM_COUNTS[cpu].fetch_add(1, Ordering::Release);
    if error != 0 {
        log::error!(
            "[TIMER_PROGRAM_ERROR_VISIBLE] cpu={} current_ticks={} deadline_ticks={} error={:#x}",
            cpu,
            current,
            deadline,
            error,
        );
    }
}

/// Return the most recent timer programming evidence for every CPU.
pub fn timer_programming_stats() -> TimerProgrammingStats {
    TimerProgrammingStats {
        observed_ticks: polyhal::timer::get_ticks() as usize,
        counts: core::array::from_fn(|cpu| TIMER_PROGRAM_COUNTS[cpu].load(Ordering::Acquire)),
        current_ticks: core::array::from_fn(|cpu| {
            TIMER_PROGRAM_CURRENT_TICKS[cpu].load(Ordering::Relaxed)
        }),
        deadline_ticks: core::array::from_fn(|cpu| {
            TIMER_PROGRAM_DEADLINE_TICKS[cpu].load(Ordering::Relaxed)
        }),
        errors: core::array::from_fn(|cpu| TIMER_PROGRAM_ERRORS[cpu].load(Ordering::Relaxed)),
    }
}

/// Account for one handled timer interrupt.
#[inline]
pub fn record_timer_interrupt() {
    TIMER_INTERRUPT_COUNT.fetch_add(1, Ordering::Relaxed);
    let cpu = polyhal::arch::hart_id();
    if cpu < MAX_CPU_NUM {
        TIMER_INTERRUPT_HEARTBEATS_NS[cpu].store(
            polyhal::timer::current_time().as_nanos() as usize,
            Ordering::Release,
        );
    }
}

/// Last observed timer-interrupt time for every CPU.
pub fn timer_interrupt_heartbeats_ns() -> [usize; MAX_CPU_NUM] {
    core::array::from_fn(|cpu| TIMER_INTERRUPT_HEARTBEATS_NS[cpu].load(Ordering::Acquire))
}

/// Diagnose a CPU whose timer interrupts still arrive while its scheduler no
/// longer advances.  This path is allocation- and lock-free so it remains
/// usable when the scheduler is stuck inside a no-IRQ critical section.
pub fn diagnose_scheduler_stall_from_timer_interrupt() {
    const REPORT_AFTER_NS: usize = 1_000_000_000;

    let observer_cpu = polyhal::arch::hart_id();
    let now_ns = polyhal::timer::current_time().as_nanos() as usize;
    for cpu in 0..MAX_CPU_NUM {
        let (heartbeat_ns, phase, pid, irq_enabled, scheduler_sp, scheduler_stack_cpu) =
            crate::task::processor::scheduler_progress(cpu);
        if heartbeat_ns == 0 || now_ns.saturating_sub(heartbeat_ns) < REPORT_AFTER_NS {
            continue;
        }
        let reported = TIMER_IRQ_REPORTED_SCHEDULER_HEARTBEAT[cpu].load(Ordering::Acquire);
        if reported == heartbeat_ns
            || TIMER_IRQ_REPORTED_SCHEDULER_HEARTBEAT[cpu]
                .compare_exchange(reported, heartbeat_ns, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            continue;
        }
        log::error!(
            "[TIMER_IRQ_SCHED_STALL_VISIBLE] observer_cpu={} stalled_cpu={} now_ns={} scheduler_heartbeat_ns={} timer_interrupt_heartbeat_ns={} phase={} pid={} phase_irq_enabled={} scheduler_sp={:#x} scheduler_stack_cpu={}",
            observer_cpu,
            cpu,
            now_ns,
            heartbeat_ns,
            TIMER_INTERRUPT_HEARTBEATS_NS[cpu].load(Ordering::Acquire),
            phase,
            pid,
            irq_enabled,
            scheduler_sp,
            scheduler_stack_cpu,
        );
        log::warn!(
            "[TIMER_IRQ_SCHED_STALL] observer_cpu={} stalled_cpu={} now_ns={} scheduler_heartbeat_ns={} phase={} pid={} phase_irq_enabled={}",
            observer_cpu,
            cpu,
            now_ns,
            heartbeat_ns,
            phase,
            pid,
            irq_enabled,
        );
    }
}

/// Account for one handled external interrupt with the supplied IRQ number.
///
/// Unknown interrupt numbers outside the representable table are deliberately
/// ignored: accounting must never make an otherwise handled interrupt fail.
#[inline]
pub fn record_external_interrupt(interrupt_number: usize) {
    if let Some(counter) = EXTERNAL_INTERRUPT_COUNTS.get(interrupt_number) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

/// Render the current counters in the format required by `/proc/interrupts`.
pub fn render() -> String {
    let mut output = String::new();

    for (interrupt_number, external_count) in EXTERNAL_INTERRUPT_COUNTS.iter().enumerate() {
        let count = if interrupt_number == TIMER_INTERRUPT_NUMBER {
            TIMER_INTERRUPT_COUNT.load(Ordering::Relaxed)
        } else {
            external_count.load(Ordering::Relaxed)
        };

        // Keep the timer line visible even before the first tick.  Besides
        // matching procfs' representation of registered IRQs, this guarantees
        // that a successful early read is not indistinguishable from EOF.
        if count != 0 || interrupt_number == TIMER_INTERRUPT_NUMBER {
            // Writing to a String cannot fail.
            writeln!(&mut output, "{}:        {}", interrupt_number, count).unwrap();
        }
    }

    output
}
