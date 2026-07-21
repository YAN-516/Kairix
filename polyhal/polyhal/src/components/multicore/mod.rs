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
    enable_tlb_shootdown_ipi,
    acknowledge_tlb_shootdown_ipi,
    send_tlb_shootdown_ipi,
    wait_for_tlb_shootdown
);

const MAX_TLB_SHOOTDOWN_CPUS: usize = 64;
static TLB_SHOOTDOWN_GENERATION: AtomicUsize = AtomicUsize::new(0);
static ICACHE_REQUIRED_GENERATION: AtomicUsize = AtomicUsize::new(0);
static TLB_SHOOTDOWN_ACKS: [AtomicUsize; MAX_TLB_SHOOTDOWN_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_TLB_SHOOTDOWN_CPUS];
static USER_TLB_ACTIVE_MASK: AtomicUsize = AtomicUsize::new(0);
static USER_TLB_ACTIVE_TOKENS: [AtomicUsize; MAX_TLB_SHOOTDOWN_CPUS] =
    [const { AtomicUsize::new(0) }; MAX_TLB_SHOOTDOWN_CPUS];
static TLB_SHOOTDOWN_CALLS: AtomicUsize = AtomicUsize::new(0);

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
        TLB_SHOOTDOWN_ACKS[cpu].store(generation, Ordering::Release);
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
    if synchronize_instructions {
        ICACHE_REQUIRED_GENERATION.fetch_max(generation, Ordering::SeqCst);
    }
    crate::pagetable::TLB::flush_all();
    if synchronize_instructions {
        crate::instruction::synchronize_instruction_cache();
    }
    if current_cpu < MAX_TLB_SHOOTDOWN_CPUS {
        TLB_SHOOTDOWN_ACKS[current_cpu].store(generation, Ordering::Release);
    }
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
    if target_mask == 0 {
        return;
    }
    let mut mask = target_mask;
    while mask != 0 {
        let cpu = mask.trailing_zeros() as usize;
        assert!(
            send_tlb_shootdown_ipi(cpu),
            "failed to send TLB shootdown IPI to CPU {}",
            cpu
        );
        mask &= !(1usize << cpu);
    }
    wait_for_tlb_shootdown(generation, target_mask);
}

/// Handle a local TLB-shootdown IPI without entering OS locks or scheduling.
pub fn handle_tlb_shootdown_ipi() {
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
        TLB_SHOOTDOWN_ACKS[cpu].store(generation, Ordering::Release);
    }
}

/// Synchronize newly generated or loaded executable code throughout one user
/// address space. This shares the synchronous generation/IPI protocol with TLB
/// invalidation so CPUs in user mode are interrupted immediately, while CPUs
/// currently in the kernel acknowledge before their next user return.
pub fn synchronize_instruction_cache(token: usize) {
    invalidate_user_caches(token, true);
}
