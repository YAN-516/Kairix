//! Multi-core Module.
//!
//! This is a leader for the multicore operation
//!
//! You can use this function to use the multicore operation
//!
//! Boot other calls after the multicore
//! If you use this function call, you should call it after arch::init(..);
//! This function will allocate the stack and map it for itself.
//!
//! ```rust
//! boot_core(hart_id, sp_top);
//! ```
//!
//! Here will have more functionality about multicore in the future.
//!

use crate::pub_use_arch;
use core::sync::atomic::{AtomicUsize, Ordering};

super::define_arch_mods!();
pub_use_arch!(
    boot_core,
    enable_ipi,
    acknowledge_ipi,
    send_ipi,
    send_ipi_mask,
    wait_for_tlb_shootdown
);

const MAX_TLB_SHOOTDOWN_CPUS: usize = 64;
const IPI_REASON_TLB_SHOOTDOWN: usize = 1 << 0;
const IPI_REASON_RESCHEDULE: usize = 1 << 1;
const IPI_REASON_TIMER_RECOVERY: usize = 1 << 2;
static TLB_SHOOTDOWN_GENERATION: AtomicUsize = AtomicUsize::new(0);
static ICACHE_REQUIRED_GENERATION: AtomicUsize = AtomicUsize::new(0);
static TLB_SHOOTDOWN_ACKS: [AtomicUsize; MAX_TLB_SHOOTDOWN_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_TLB_SHOOTDOWN_CPUS];
static TLB_WAIT_GENERATIONS: [AtomicUsize; MAX_TLB_SHOOTDOWN_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_TLB_SHOOTDOWN_CPUS];
static TLB_WAIT_TARGET_MASKS: [AtomicUsize; MAX_TLB_SHOOTDOWN_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_TLB_SHOOTDOWN_CPUS];
static TLB_WAIT_PENDING_MASKS: [AtomicUsize; MAX_TLB_SHOOTDOWN_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_TLB_SHOOTDOWN_CPUS];
static TLB_OPERATION_PHASES: [AtomicUsize; MAX_TLB_SHOOTDOWN_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_TLB_SHOOTDOWN_CPUS];
static TLB_OPERATION_GENERATIONS: [AtomicUsize; MAX_TLB_SHOOTDOWN_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_TLB_SHOOTDOWN_CPUS];
static TLB_OPERATION_TARGET_MASKS: [AtomicUsize; MAX_TLB_SHOOTDOWN_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_TLB_SHOOTDOWN_CPUS];
static USER_TLB_ACTIVE_MASK: AtomicUsize = AtomicUsize::new(0);
static USER_TLB_ACTIVE_TOKENS: [AtomicUsize; MAX_TLB_SHOOTDOWN_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_TLB_SHOOTDOWN_CPUS];
static TLB_SHOOTDOWN_CALLS: AtomicUsize = AtomicUsize::new(0);
static PENDING_IPI_REASONS: [AtomicUsize; MAX_TLB_SHOOTDOWN_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_TLB_SHOOTDOWN_CPUS];
static RESCHEDULE_IPI_SENT: AtomicUsize = AtomicUsize::new(0);
static RESCHEDULE_IPI_RECEIVED: AtomicUsize = AtomicUsize::new(0);
static TIMER_RECOVERY_IPI_SENT: [AtomicUsize; MAX_TLB_SHOOTDOWN_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_TLB_SHOOTDOWN_CPUS];
static TIMER_RECOVERY_IPI_RECEIVED: [AtomicUsize; MAX_TLB_SHOOTDOWN_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_TLB_SHOOTDOWN_CPUS];
static TRAP_ENTRIES: [AtomicUsize; MAX_TLB_SHOOTDOWN_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_TLB_SHOOTDOWN_CPUS];
static TRAP_STAGES: [AtomicUsize; MAX_TLB_SHOOTDOWN_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_TLB_SHOOTDOWN_CPUS];
static TRAP_CAUSES: [AtomicUsize; MAX_TLB_SHOOTDOWN_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_TLB_SHOOTDOWN_CPUS];
static TRAP_PCS: [AtomicUsize; MAX_TLB_SHOOTDOWN_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_TLB_SHOOTDOWN_CPUS];
static TRAP_VALUES: [AtomicUsize; MAX_TLB_SHOOTDOWN_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_TLB_SHOOTDOWN_CPUS];
static TRAP_FROM_USER: [AtomicUsize; MAX_TLB_SHOOTDOWN_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_TLB_SHOOTDOWN_CPUS];

/// Enable the platform IPI channel used by TLB shootdowns and scheduler kicks.
pub fn enable_tlb_shootdown_ipi() {
    enable_ipi();
}

fn send_ipi_reason(cpu: usize, reason: usize) -> bool {
    if cpu >= MAX_TLB_SHOOTDOWN_CPUS {
        return false;
    }
    PENDING_IPI_REASONS[cpu].fetch_or(reason, Ordering::Release);
    // SBI/MMIO interrupt injection is not itself a Rust atomic operation.
    // Order the software reason publication before the hardware doorbell so
    // the target cannot clear SSIP, observe zero, and strand a late-visible
    // reason without another interrupt edge.
    core::sync::atomic::fence(Ordering::SeqCst);
    send_ipi(cpu)
}

fn send_tlb_shootdown_ipi_mask(target_mask: usize) -> bool {
    let mut mask = target_mask;
    while mask != 0 {
        let cpu = mask.trailing_zeros() as usize;
        let bit = 1usize << cpu;
        if cpu >= MAX_TLB_SHOOTDOWN_CPUS {
            return false;
        }
        PENDING_IPI_REASONS[cpu].fetch_or(IPI_REASON_TLB_SHOOTDOWN, Ordering::Release);
        mask &= !bit;
    }
    core::sync::atomic::fence(Ordering::SeqCst);
    send_ipi_mask(target_mask)
}

/// Wake an idle remote scheduler after publishing work to its ready queue.
pub fn send_reschedule_ipi(cpu: usize) -> bool {
    let sent = send_ipi_reason(cpu, IPI_REASON_RESCHEDULE);
    if sent {
        RESCHEDULE_IPI_SENT.fetch_add(1, Ordering::Relaxed);
    }
    sent
}

/// Ask a hart to repair its local one-shot timer through the independent IPI
/// channel. This is used only after another CPU proves that the published timer
/// deadline is overdue and the target's timer heartbeat has stopped.
pub fn send_timer_recovery_ipi(cpu: usize) -> bool {
    let sent = send_ipi_reason(cpu, IPI_REASON_TIMER_RECOVERY);
    if sent && cpu < MAX_TLB_SHOOTDOWN_CPUS {
        TIMER_RECOVERY_IPI_SENT[cpu].fetch_add(1, Ordering::Relaxed);
    }
    sent
}

/// Per-target timer recovery submissions and completions.
pub fn timer_recovery_ipi_stats(cpu: usize) -> (usize, usize) {
    if cpu >= MAX_TLB_SHOOTDOWN_CPUS {
        return (0, 0);
    }
    (
        TIMER_RECOVERY_IPI_SENT[cpu].load(Ordering::Acquire),
        TIMER_RECOVERY_IPI_RECEIVED[cpu].load(Ordering::Acquire),
    )
}

/// Number of scheduler IPIs successfully submitted and received.
pub fn reschedule_ipi_stats() -> (usize, usize) {
    (
        RESCHEDULE_IPI_SENT.load(Ordering::Relaxed),
        RESCHEDULE_IPI_RECEIVED.load(Ordering::Relaxed),
    )
}

/// Number of synchronous address-space invalidations requested since reset.
pub fn tlb_shootdown_calls() -> usize {
    TLB_SHOOTDOWN_CALLS.load(Ordering::Relaxed)
}

/// Reset the synchronous invalidation counter.
pub fn reset_tlb_shootdown_calls() {
    TLB_SHOOTDOWN_CALLS.store(0, Ordering::Relaxed);
}

/// Return the subset of `target_mask` that has not acknowledged `generation`.
pub(crate) fn tlb_shootdown_pending_mask(generation: usize, target_mask: usize) -> usize {
    let mut pending = 0usize;
    let mut mask = target_mask;
    while mask != 0 {
        let cpu = mask.trailing_zeros() as usize;
        let bit = 1usize << cpu;
        if TLB_SHOOTDOWN_ACKS[cpu].load(Ordering::Acquire) < generation {
            pending |= bit;
        }
        mask &= !bit;
    }
    pending
}

/// Lock-free state of one CPU's synchronous shootdown wait.
#[derive(Debug, Clone, Copy)]
pub struct TlbShootdownWaitState {
    pub operation_phase: usize,
    pub operation_generation: usize,
    pub operation_target_mask: usize,
    pub generation: usize,
    pub target_mask: usize,
    pub pending_mask: usize,
    pub acknowledged_generation: usize,
    pub latest_generation: usize,
}

/// Return shootdown progress for watchdog diagnostics without taking a lock.
pub fn tlb_shootdown_wait_state(cpu: usize) -> TlbShootdownWaitState {
    if cpu >= MAX_TLB_SHOOTDOWN_CPUS {
        return TlbShootdownWaitState {
            operation_phase: 0,
            operation_generation: 0,
            operation_target_mask: 0,
            generation: 0,
            target_mask: 0,
            pending_mask: 0,
            acknowledged_generation: 0,
            latest_generation: TLB_SHOOTDOWN_GENERATION.load(Ordering::Acquire),
        };
    }
    TlbShootdownWaitState {
        operation_phase: TLB_OPERATION_PHASES[cpu].load(Ordering::Acquire),
        operation_generation: TLB_OPERATION_GENERATIONS[cpu].load(Ordering::Acquire),
        operation_target_mask: TLB_OPERATION_TARGET_MASKS[cpu].load(Ordering::Acquire),
        generation: TLB_WAIT_GENERATIONS[cpu].load(Ordering::Acquire),
        target_mask: TLB_WAIT_TARGET_MASKS[cpu].load(Ordering::Acquire),
        pending_mask: TLB_WAIT_PENDING_MASKS[cpu].load(Ordering::Acquire),
        acknowledged_generation: TLB_SHOOTDOWN_ACKS[cpu].load(Ordering::Acquire),
        latest_generation: TLB_SHOOTDOWN_GENERATION.load(Ordering::Acquire),
    }
}

fn record_tlb_operation(phase: usize, generation: usize, target_mask: usize) {
    let cpu = crate::arch::hart_id();
    if cpu < MAX_TLB_SHOOTDOWN_CPUS {
        TLB_OPERATION_GENERATIONS[cpu].store(generation, Ordering::Relaxed);
        TLB_OPERATION_TARGET_MASKS[cpu].store(target_mask, Ordering::Relaxed);
        TLB_OPERATION_PHASES[cpu].store(phase, Ordering::Release);
    }
}

/// Lock-free architecture trap progress for a stalled-CPU observer.
#[derive(Debug, Clone, Copy)]
pub struct TrapProgress {
    pub entries: usize,
    /// 1=architecture callback, 2=IPI-local work, 3=OS callback, 4=handled.
    pub stage: usize,
    pub cause: usize,
    /// Saved instruction pointer at trap entry.
    pub pc: usize,
    /// Architecture fault detail (`stval` on RISC-V, `badv` on LoongArch).
    pub value: usize,
    pub from_user: bool,
}

pub fn record_trap_entry(cause: usize, from_user: bool, pc: usize, value: usize) {
    let cpu = crate::arch::hart_id();
    if cpu < MAX_TLB_SHOOTDOWN_CPUS {
        TRAP_CAUSES[cpu].store(cause, Ordering::Relaxed);
        TRAP_PCS[cpu].store(pc, Ordering::Relaxed);
        TRAP_VALUES[cpu].store(value, Ordering::Relaxed);
        TRAP_FROM_USER[cpu].store(from_user as usize, Ordering::Relaxed);
        TRAP_ENTRIES[cpu].fetch_add(1, Ordering::Relaxed);
        TRAP_STAGES[cpu].store(1, Ordering::Release);
    }
}

pub fn record_trap_stage(stage: usize) {
    let cpu = crate::arch::hart_id();
    if cpu < MAX_TLB_SHOOTDOWN_CPUS {
        TRAP_STAGES[cpu].store(stage, Ordering::Release);
    }
}

pub fn trap_progress(cpu: usize) -> TrapProgress {
    if cpu >= MAX_TLB_SHOOTDOWN_CPUS {
        return TrapProgress {
            entries: 0,
            stage: 0,
            cause: 0,
            pc: 0,
            value: 0,
            from_user: false,
        };
    }
    TrapProgress {
        entries: TRAP_ENTRIES[cpu].load(Ordering::Acquire),
        stage: TRAP_STAGES[cpu].load(Ordering::Acquire),
        cause: TRAP_CAUSES[cpu].load(Ordering::Relaxed),
        pc: TRAP_PCS[cpu].load(Ordering::Relaxed),
        value: TRAP_VALUES[cpu].load(Ordering::Relaxed),
        from_user: TRAP_FROM_USER[cpu].load(Ordering::Relaxed) != 0,
    }
}

pub(crate) fn begin_tlb_shootdown_wait(generation: usize, target_mask: usize) {
    let cpu = crate::arch::hart_id();
    if cpu < MAX_TLB_SHOOTDOWN_CPUS {
        TLB_WAIT_TARGET_MASKS[cpu].store(target_mask, Ordering::Relaxed);
        TLB_WAIT_PENDING_MASKS[cpu].store(target_mask, Ordering::Relaxed);
        TLB_WAIT_GENERATIONS[cpu].store(generation, Ordering::Release);
    }
}

pub(crate) fn update_tlb_shootdown_wait(pending_mask: usize) {
    let cpu = crate::arch::hart_id();
    if cpu < MAX_TLB_SHOOTDOWN_CPUS {
        TLB_WAIT_PENDING_MASKS[cpu].store(pending_mask, Ordering::Release);
    }
}

pub(crate) fn end_tlb_shootdown_wait() {
    let cpu = crate::arch::hart_id();
    if cpu < MAX_TLB_SHOOTDOWN_CPUS {
        TLB_WAIT_PENDING_MASKS[cpu].store(0, Ordering::Relaxed);
        TLB_WAIT_TARGET_MASKS[cpu].store(0, Ordering::Relaxed);
        TLB_WAIT_GENERATIONS[cpu].store(0, Ordering::Release);
    }
}

/// While waiting for remote acknowledgements, also consume any newer
/// generation that selected this CPU before trap entry cleared its active bit.
/// This breaks cross-CPU wait chains without depending on nested IPI delivery.
pub(crate) fn service_local_tlb_shootdown_generation() {
    let generation = TLB_SHOOTDOWN_GENERATION.load(Ordering::Acquire);
    acknowledge_current_cpu_generation(generation);
}

fn acknowledge_current_cpu_generation(generation: usize) {
    let cpu = crate::arch::hart_id();
    if cpu >= MAX_TLB_SHOOTDOWN_CPUS {
        return;
    }
    let acknowledged = TLB_SHOOTDOWN_ACKS[cpu].load(Ordering::Acquire);
    if acknowledged < generation {
        crate::pagetable::TLB::flush_all();
        if acknowledged < ICACHE_REQUIRED_GENERATION.load(Ordering::Acquire) {
            crate::instruction::synchronize_instruction_cache();
        }
        // Concurrent invalidations may already have acknowledged a newer
        // generation for this CPU. ACK publication is monotonic: writing an
        // older value back would make a newer sender wait forever.
        TLB_SHOOTDOWN_ACKS[cpu].fetch_max(generation, Ordering::Release);
    }
}

/// Mark that this CPU has trapped out of user mode. If a shootdown raced with
/// trap entry, acknowledge it here so the sender never waits for an IPI that
/// cannot be delivered while the kernel keeps global interrupts masked.
pub fn mark_current_cpu_kernel_entry() {
    let cpu = crate::arch::hart_id();
    if cpu >= usize::BITS as usize {
        return;
    }
    USER_TLB_ACTIVE_MASK.fetch_and(!(1usize << cpu), Ordering::SeqCst);
    USER_TLB_ACTIVE_TOKENS[cpu].store(0, Ordering::Release);
    let generation = TLB_SHOOTDOWN_GENERATION.load(Ordering::Acquire);
    acknowledge_current_cpu_generation(generation);
}

/// Publish that this CPU is about to execute user code. A generation retry
/// closes the race where a shootdown starts between the local flush and active
/// mask publication.
pub fn prepare_current_cpu_user_return(token: usize) {
    let cpu = crate::arch::hart_id();
    assert!(
        cpu < MAX_TLB_SHOOTDOWN_CPUS.min(usize::BITS as usize),
        "TLB user CPU id {} exceeds mask capacity",
        cpu
    );
    loop {
        let generation = TLB_SHOOTDOWN_GENERATION.load(Ordering::Acquire);
        acknowledge_current_cpu_generation(generation);
        USER_TLB_ACTIVE_TOKENS[cpu].store(token, Ordering::Release);
        USER_TLB_ACTIVE_MASK.fetch_or(1usize << cpu, Ordering::SeqCst);
        if TLB_SHOOTDOWN_GENERATION.load(Ordering::Acquire) == generation {
            return;
        }
        USER_TLB_ACTIVE_MASK.fetch_and(!(1usize << cpu), Ordering::SeqCst);
        USER_TLB_ACTIVE_TOKENS[cpu].store(0, Ordering::Release);
    }
}

/// Flush the current CPU and synchronously invalidate every other CPU that can
/// currently execute user translations before a user-mapped frame is recycled.
pub fn shootdown_tlb_all(token: usize) {
    invalidate_user_caches(token, false);
}

fn invalidate_user_caches(token: usize, synchronize_instructions: bool) {
    TLB_SHOOTDOWN_CALLS.fetch_add(1, Ordering::Relaxed);
    let current_cpu = crate::arch::hart_id();
    let generation = TLB_SHOOTDOWN_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    record_tlb_operation(1, generation, 0);
    if synchronize_instructions {
        ICACHE_REQUIRED_GENERATION.fetch_max(generation, Ordering::SeqCst);
    }
    crate::pagetable::TLB::flush_all();
    if synchronize_instructions {
        crate::instruction::synchronize_instruction_cache();
    }
    if current_cpu < MAX_TLB_SHOOTDOWN_CPUS {
        TLB_SHOOTDOWN_ACKS[current_cpu].fetch_max(generation, Ordering::Release);
    }
    record_tlb_operation(2, generation, 0);
    // CPUs in the scheduler or inside the kernel cannot consume a stale user
    // translation. They acknowledge the current generation before returning
    // to user mode, so only CPUs actively executing user code need an IPI.
    let active_mask = USER_TLB_ACTIVE_MASK.load(Ordering::SeqCst);
    let mut target_mask = 0usize;
    let mut candidates = active_mask & !(1usize << current_cpu);
    while candidates != 0 {
        let cpu = candidates.trailing_zeros() as usize;
        let bit = 1usize << cpu;
        if USER_TLB_ACTIVE_TOKENS[cpu].load(Ordering::Acquire) == token {
            target_mask |= bit;
        }
        candidates &= !bit;
    }
    record_tlb_operation(3, generation, target_mask);
    if target_mask == 0 {
        record_tlb_operation(0, 0, 0);
        return;
    }

    // Keep remote invalidation in S-mode on every architecture. In particular,
    // an SBI RFENCE call can hold every target hart in firmware while the
    // caller waits synchronously. Under a many-threaded mmap/munmap workload
    // that made the affected RISC-V harts unable to consume either their timer
    // interrupt or a recovery SSIP. The generation protocol is safe under
    // concurrent callers because acknowledgements are monotonic and an IPI
    // handler flushes through the latest published generation.
    // Publish the synchronous wait before entering firmware. A concurrent
    // caller can now identify and service this CPU even if the SBI submission
    // itself is the point that stops making progress.
    begin_tlb_shootdown_wait(generation, target_mask);
    record_tlb_operation(4, generation, target_mask);
    assert!(
        send_tlb_shootdown_ipi_mask(target_mask),
        "failed to send TLB shootdown IPI mask {:#x}",
        target_mask
    );
    record_tlb_operation(5, generation, target_mask);
    record_tlb_operation(6, generation, target_mask);
    wait_for_tlb_shootdown(generation, target_mask);
    record_tlb_operation(0, 0, 0);
}

fn handle_tlb_shootdown_ipi() {
    let generation = TLB_SHOOTDOWN_GENERATION.load(Ordering::Acquire);
    let cpu = crate::arch::hart_id();
    let acknowledged = if cpu < MAX_TLB_SHOOTDOWN_CPUS {
        TLB_SHOOTDOWN_ACKS[cpu].load(Ordering::Acquire)
    } else {
        generation
    };
    crate::pagetable::TLB::flush_all();
    if acknowledged < ICACHE_REQUIRED_GENERATION.load(Ordering::Acquire) {
        crate::instruction::synchronize_instruction_cache();
    }
    if cpu < MAX_TLB_SHOOTDOWN_CPUS {
        TLB_SHOOTDOWN_ACKS[cpu].fetch_max(generation, Ordering::Release);
    }
}

/// Acknowledge and drain all software reasons carried by one platform IPI.
///
/// This path must remain lock-free because an IPI can interrupt a no-IRQ
/// critical section. The return value tells the trap layer whether a scheduler
/// kick raced with the idle-to-user transition and therefore needs to preempt
/// user mode; kernel-origin IPIs still finish entirely in this lock-free path.
pub fn handle_ipi() -> bool {
    acknowledge_ipi();
    let cpu = crate::arch::hart_id();
    if cpu >= MAX_TLB_SHOOTDOWN_CPUS {
        return false;
    }

    let mut reschedule = false;
    loop {
        let reasons = PENDING_IPI_REASONS[cpu].swap(0, Ordering::AcqRel);
        if reasons == 0 {
            return reschedule;
        }
        if reasons & IPI_REASON_TLB_SHOOTDOWN != 0 {
            handle_tlb_shootdown_ipi();
        }
        if reasons & IPI_REASON_RESCHEDULE != 0 {
            RESCHEDULE_IPI_RECEIVED.fetch_add(1, Ordering::Relaxed);
            reschedule = true;
        }
        if reasons & IPI_REASON_TIMER_RECOVERY != 0 {
            crate::timer::enable_timer_interrupt();
            let _ = crate::timer::set_next_timer(core::time::Duration::from_millis(10));
            TIMER_RECOVERY_IPI_RECEIVED[cpu].fetch_add(1, Ordering::Release);
            // A user-mode target must enter the scheduler instead of returning
            // to the task whose timer preemption was already lost. Kernel-mode
            // IPIs remain lock-free; their trap path ignores this return value.
            reschedule = true;
        }
    }
}

/// Synchronize newly generated or loaded executable code throughout one user
/// address space. This shares the synchronous generation/IPI protocol with TLB
/// invalidation so CPUs in user mode are interrupted immediately, while CPUs
/// currently in the kernel acknowledge before their next user return.
pub fn synchronize_instruction_cache(token: usize) {
    invalidate_user_caches(token, true);
}
