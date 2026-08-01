use core::sync::atomic::{AtomicUsize, Ordering};
use polyhal::timer::current_time;

static CLONE_THREAD_CALLS: AtomicUsize = AtomicUsize::new(0);
static CLONE_THREAD_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static CLONE_THREAD_NS_MAX: AtomicUsize = AtomicUsize::new(0);
static CLONE_PROCESS_CALLS: AtomicUsize = AtomicUsize::new(0);
static CLONE_PROCESS_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static CLONE_PROCESS_NS_MAX: AtomicUsize = AtomicUsize::new(0);

static EXIT_CALLS: AtomicUsize = AtomicUsize::new(0);
static EXIT_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static EXIT_NS_MAX: AtomicUsize = AtomicUsize::new(0);

static KSTACK_ALLOC_CALLS: AtomicUsize = AtomicUsize::new(0);
static KSTACK_ALLOC_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static KSTACK_ALLOC_NS_MAX: AtomicUsize = AtomicUsize::new(0);
static TCB_NEW_CALLS: AtomicUsize = AtomicUsize::new(0);
static TCB_NEW_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static TCB_NEW_NS_MAX: AtomicUsize = AtomicUsize::new(0);
static TASK_USER_RES_NEW_CALLS: AtomicUsize = AtomicUsize::new(0);
static TASK_USER_RES_NEW_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static TASK_USER_RES_NEW_NS_MAX: AtomicUsize = AtomicUsize::new(0);

static FUTEX_WAIT_CALLS: AtomicUsize = AtomicUsize::new(0);
static FUTEX_WAIT_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static FUTEX_WAIT_NS_MAX: AtomicUsize = AtomicUsize::new(0);
static FUTEX_WAIT_BLOCK_CALLS: AtomicUsize = AtomicUsize::new(0);
static FUTEX_WAIT_SUSPEND_CALLS: AtomicUsize = AtomicUsize::new(0);
static FUTEX_WAKE_CALLS: AtomicUsize = AtomicUsize::new(0);
static FUTEX_WAKE_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static FUTEX_WAKE_NS_MAX: AtomicUsize = AtomicUsize::new(0);
static FUTEX_WAKE_WOKEN_TOTAL: AtomicUsize = AtomicUsize::new(0);
static FUTEX_WAKE_ONE_CALLS: AtomicUsize = AtomicUsize::new(0);
static FUTEX_WAKE_ONE_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static FUTEX_WAKE_ONE_NS_MAX: AtomicUsize = AtomicUsize::new(0);
static FUTEX_WAKE_ONE_WOKEN_TOTAL: AtomicUsize = AtomicUsize::new(0);

static BLOCK_CALLS: AtomicUsize = AtomicUsize::new(0);
static BLOCK_SCHEDULE_CALLS: AtomicUsize = AtomicUsize::new(0);
static BLOCK_FAST_RETURN_CALLS: AtomicUsize = AtomicUsize::new(0);
static SUSPEND_CALLS: AtomicUsize = AtomicUsize::new(0);
static SUSPEND_SCHEDULE_CALLS: AtomicUsize = AtomicUsize::new(0);
static PREEMPT_CALLS: AtomicUsize = AtomicUsize::new(0);
static PREEMPT_SCHEDULE_CALLS: AtomicUsize = AtomicUsize::new(0);

static FIRST_RUN_CALLS: AtomicUsize = AtomicUsize::new(0);
static FIRST_RUN_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static FIRST_RUN_NS_MAX: AtomicUsize = AtomicUsize::new(0);
static READY_QUEUE_PUSHES: AtomicUsize = AtomicUsize::new(0);
static READY_QUEUE_FETCHES: AtomicUsize = AtomicUsize::new(0);
static READY_QUEUE_MAX_LEN: AtomicUsize = AtomicUsize::new(0);

static PROC_SMAPS_READ_CALLS: AtomicUsize = AtomicUsize::new(0);
static PROC_SMAPS_RENDER_CALLS: AtomicUsize = AtomicUsize::new(0);
static PROC_SMAPS_RENDER_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static PROC_SMAPS_RENDER_NS_MAX: AtomicUsize = AtomicUsize::new(0);
static PROC_SMAPS_RENDER_AREAS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static PROC_SMAPS_RENDER_BYTES_TOTAL: AtomicUsize = AtomicUsize::new(0);

static MMAP_CALLS: AtomicUsize = AtomicUsize::new(0);
static MMAP_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static MMAP_NS_MAX: AtomicUsize = AtomicUsize::new(0);
static MUNMAP_CALLS: AtomicUsize = AtomicUsize::new(0);
static MUNMAP_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static MUNMAP_NS_MAX: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_CALLS: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_NS_MAX: AtomicUsize = AtomicUsize::new(0);

static MPROTECT_PHASE_CALLS: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_PHASE_ELAPSED_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_PHASE_ELAPSED_NS_MAX: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_INNER_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_INNER_NS_MAX: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_ACCOUNTED_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_UNACCOUNTED_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_UNACCOUNTED_NS_MAX: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_CONTEXT_SWITCH_CALLS: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_CONTEXT_SWITCHES_TOTAL: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_VM_LOCK_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_PREFLIGHT_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_VMA_UPDATE_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_PTE_WALK_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_TLB_NS_TOTAL: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_PREFIX_EXTENSIONS: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_VMA_SPLITS: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_VMA_MERGES: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_PTES_WALKED: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_PTES_PRESENT: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_PTES_CHANGED: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_TLB_NONE_CALLS: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_TLB_LOCAL_PAGE_CALLS: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_TLB_LOCAL_ALL_CALLS: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_TLB_REMOTE_CALLS: AtomicUsize = AtomicUsize::new(0);
static MPROTECT_TLB_ICACHE_CALLS: AtomicUsize = AtomicUsize::new(0);

static ANON_FAULT_CALLS: AtomicUsize = AtomicUsize::new(0);
static ANON_FAULT_HEAP_CALLS: AtomicUsize = AtomicUsize::new(0);
static ANON_FAULT_STACK_CALLS: AtomicUsize = AtomicUsize::new(0);
static ANON_FAULT_MMAP_CALLS: AtomicUsize = AtomicUsize::new(0);
static ANON_FAULT_SHARED_CALLS: AtomicUsize = AtomicUsize::new(0);
static ANON_FAULT_ELF_CALLS: AtomicUsize = AtomicUsize::new(0);
static ANON_FAULT_TOTAL_NS: AtomicUsize = AtomicUsize::new(0);
static ANON_FAULT_TOTAL_NS_MAX: AtomicUsize = AtomicUsize::new(0);
static ANON_FAULT_FRAME_ALLOC_NS: AtomicUsize = AtomicUsize::new(0);
static ANON_FAULT_FRAME_ALLOC_NS_MAX: AtomicUsize = AtomicUsize::new(0);
static ANON_FAULT_ZERO_NS: AtomicUsize = AtomicUsize::new(0);
static ANON_FAULT_ZERO_NS_MAX: AtomicUsize = AtomicUsize::new(0);
static ANON_FAULT_PUBLISH_NS: AtomicUsize = AtomicUsize::new(0);
static ANON_FAULT_PAGE_TABLE_NS: AtomicUsize = AtomicUsize::new(0);
static ANON_FAULT_PAGE_TABLE_NS_MAX: AtomicUsize = AtomicUsize::new(0);
static ANON_FAULT_ICACHE_NS: AtomicUsize = AtomicUsize::new(0);
static ANON_FAULT_TLB_NS: AtomicUsize = AtomicUsize::new(0);
static ANON_FAULT_TLB_NS_MAX: AtomicUsize = AtomicUsize::new(0);

static EXEC_FILE_MAPPINGS: AtomicUsize = AtomicUsize::new(0);
static EXEC_FILE_LAZY_BYTES: AtomicUsize = AtomicUsize::new(0);
static EXEC_FILE_LAZY_PAGES: AtomicUsize = AtomicUsize::new(0);
static FILE_FAULT_SHARED_PAGES: AtomicUsize = AtomicUsize::new(0);
static FILE_FAULT_PRIVATE_COPIES: AtomicUsize = AtomicUsize::new(0);
static FILE_FAULT_ZERO_PAGES: AtomicUsize = AtomicUsize::new(0);
static PAGE_TABLE_ACTIVATIONS: AtomicUsize = AtomicUsize::new(0);
static PAGE_TABLE_ACTIVATION_SKIPS: AtomicUsize = AtomicUsize::new(0);
static IDLE_WFI_CALLS: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy)]
pub struct PerfStatsSnapshot {
    pub clone_thread_calls: usize,
    pub clone_thread_ns_total: usize,
    pub clone_thread_ns_max: usize,
    pub clone_process_calls: usize,
    pub clone_process_ns_total: usize,
    pub clone_process_ns_max: usize,
    pub exit_calls: usize,
    pub exit_ns_total: usize,
    pub exit_ns_max: usize,
    pub kstack_alloc_calls: usize,
    pub kstack_alloc_ns_total: usize,
    pub kstack_alloc_ns_max: usize,
    pub tcb_new_calls: usize,
    pub tcb_new_ns_total: usize,
    pub tcb_new_ns_max: usize,
    pub task_user_res_new_calls: usize,
    pub task_user_res_new_ns_total: usize,
    pub task_user_res_new_ns_max: usize,
    pub futex_wait_calls: usize,
    pub futex_wait_ns_total: usize,
    pub futex_wait_ns_max: usize,
    pub futex_wait_block_calls: usize,
    pub futex_wait_suspend_calls: usize,
    pub futex_wake_calls: usize,
    pub futex_wake_ns_total: usize,
    pub futex_wake_ns_max: usize,
    pub futex_wake_woken_total: usize,
    pub futex_wake_one_calls: usize,
    pub futex_wake_one_ns_total: usize,
    pub futex_wake_one_ns_max: usize,
    pub futex_wake_one_woken_total: usize,
    pub block_calls: usize,
    pub block_schedule_calls: usize,
    pub block_fast_return_calls: usize,
    pub suspend_calls: usize,
    pub suspend_schedule_calls: usize,
    pub preempt_calls: usize,
    pub preempt_schedule_calls: usize,
    pub first_run_calls: usize,
    pub first_run_ns_total: usize,
    pub first_run_ns_max: usize,
    pub ready_queue_pushes: usize,
    pub ready_queue_fetches: usize,
    pub ready_queue_max_len: usize,
    pub proc_smaps_read_calls: usize,
    pub proc_smaps_render_calls: usize,
    pub proc_smaps_render_ns_total: usize,
    pub proc_smaps_render_ns_max: usize,
    pub proc_smaps_render_areas_total: usize,
    pub proc_smaps_render_bytes_total: usize,
    pub mmap_calls: usize,
    pub mmap_ns_total: usize,
    pub mmap_ns_max: usize,
    pub munmap_calls: usize,
    pub munmap_ns_total: usize,
    pub munmap_ns_max: usize,
    pub mprotect_calls: usize,
    pub mprotect_ns_total: usize,
    pub mprotect_ns_max: usize,
    pub exec_file_mappings: usize,
    pub exec_file_lazy_bytes: usize,
    pub exec_file_lazy_pages: usize,
    pub file_fault_shared_pages: usize,
    pub file_fault_private_copies: usize,
    pub file_fault_zero_pages: usize,
    pub page_table_activations: usize,
    pub page_table_activation_skips: usize,
    pub tlb_shootdown_calls: usize,
    pub idle_wfi_calls: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct MprotectPhaseStatsSnapshot {
    pub calls: usize,
    pub elapsed_ns_total: usize,
    pub elapsed_ns_max: usize,
    pub inner_ns_total: usize,
    pub inner_ns_max: usize,
    pub accounted_ns_total: usize,
    pub unaccounted_ns_total: usize,
    pub unaccounted_ns_max: usize,
    pub context_switch_calls: usize,
    pub context_switches_total: usize,
    pub vm_lock_ns_total: usize,
    pub preflight_ns_total: usize,
    pub vma_update_ns_total: usize,
    pub pte_walk_ns_total: usize,
    pub tlb_ns_total: usize,
    pub prefix_extensions: usize,
    pub vma_splits: usize,
    pub vma_merges: usize,
    pub ptes_walked: usize,
    pub ptes_present: usize,
    pub ptes_changed: usize,
    pub tlb_none_calls: usize,
    pub tlb_local_page_calls: usize,
    pub tlb_local_all_calls: usize,
    pub tlb_remote_calls: usize,
    pub tlb_icache_calls: usize,
}

pub struct MprotectPhaseSample {
    pub elapsed_ns: usize,
    pub inner_ns: usize,
    pub context_switches: usize,
    pub vm_lock_ns: usize,
    pub preflight_ns: usize,
    pub vma_update_ns: usize,
    pub pte_walk_ns: usize,
    pub tlb_ns: usize,
    pub prefix_extensions: usize,
    pub vma_splits: usize,
    pub vma_merges: usize,
    pub ptes_walked: usize,
    pub ptes_present: usize,
    pub ptes_changed: usize,
    pub tlb_kind: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub enum AnonFaultKind {
    Heap,
    Stack,
    Mmap,
    Shared,
    Elf,
}

#[derive(Debug, Clone, Copy)]
pub struct AnonFaultPhaseStatsSnapshot {
    pub calls: usize,
    pub heap_calls: usize,
    pub stack_calls: usize,
    pub mmap_calls: usize,
    pub shared_calls: usize,
    pub elf_calls: usize,
    pub total_ns: usize,
    pub total_ns_max: usize,
    pub frame_alloc_ns: usize,
    pub frame_alloc_ns_max: usize,
    pub zero_ns: usize,
    pub zero_ns_max: usize,
    pub publish_ns: usize,
    pub page_table_ns: usize,
    pub page_table_ns_max: usize,
    pub icache_ns: usize,
    pub tlb_ns: usize,
    pub tlb_ns_max: usize,
}

pub struct AnonFaultPhaseSample {
    pub kind: AnonFaultKind,
    pub total_ns: usize,
    pub frame_alloc_ns: usize,
    pub zero_ns: usize,
    pub publish_ns: usize,
    pub page_table_ns: usize,
    pub icache_ns: usize,
    pub tlb_ns: usize,
}

#[derive(Debug, Clone, Copy)]
pub enum PerfTimerKind {
    KstackAlloc,
    TcbNew,
    TaskUserResNew,
    FutexWait,
    FutexWake,
    FutexWakeOne,
    Mmap,
    Munmap,
    Mprotect,
}

pub struct PerfTimer {
    kind: PerfTimerKind,
    start_ns: usize,
}

impl Drop for PerfTimer {
    fn drop(&mut self) {
        let elapsed = elapsed_since(self.start_ns);
        match self.kind {
            PerfTimerKind::KstackAlloc => record_kstack_alloc_ns(elapsed),
            PerfTimerKind::TcbNew => record_tcb_new_ns(elapsed),
            PerfTimerKind::TaskUserResNew => record_task_user_res_new_ns(elapsed),
            PerfTimerKind::FutexWait => record_futex_wait_ns(elapsed),
            PerfTimerKind::FutexWake => record_futex_wake_ns(elapsed),
            PerfTimerKind::FutexWakeOne => record_futex_wake_one_ns(elapsed),
            PerfTimerKind::Mmap => record_mmap_ns(elapsed),
            PerfTimerKind::Munmap => record_munmap_ns(elapsed),
            PerfTimerKind::Mprotect => record_mprotect_ns(elapsed),
        }
    }
}

pub fn now_ns() -> usize {
    current_time().as_nanos() as usize
}

pub fn elapsed_since(start_ns: usize) -> usize {
    now_ns().saturating_sub(start_ns)
}

pub fn scope_timer(kind: PerfTimerKind) -> PerfTimer {
    PerfTimer {
        kind,
        start_ns: now_ns(),
    }
}

pub fn record_clone_thread_ns(elapsed_ns: usize) {
    record_timed(
        &CLONE_THREAD_CALLS,
        &CLONE_THREAD_NS_TOTAL,
        &CLONE_THREAD_NS_MAX,
        elapsed_ns,
    );
}

pub fn record_clone_process_ns(elapsed_ns: usize) {
    record_timed(
        &CLONE_PROCESS_CALLS,
        &CLONE_PROCESS_NS_TOTAL,
        &CLONE_PROCESS_NS_MAX,
        elapsed_ns,
    );
}

pub fn record_exit_ns(elapsed_ns: usize) {
    record_timed(&EXIT_CALLS, &EXIT_NS_TOTAL, &EXIT_NS_MAX, elapsed_ns);
}

pub fn record_futex_wake_woken(woken: usize) {
    FUTEX_WAKE_WOKEN_TOTAL.fetch_add(woken, Ordering::Relaxed);
}

pub fn record_futex_wake_one_woken(woken: usize) {
    FUTEX_WAKE_ONE_WOKEN_TOTAL.fetch_add(woken, Ordering::Relaxed);
}

pub fn record_futex_wait_block() {
    FUTEX_WAIT_BLOCK_CALLS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_futex_wait_suspend() {
    FUTEX_WAIT_SUSPEND_CALLS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_block_call() {
    BLOCK_CALLS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_block_schedule() {
    BLOCK_SCHEDULE_CALLS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_block_fast_return() {
    BLOCK_FAST_RETURN_CALLS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_suspend_call() {
    SUSPEND_CALLS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_suspend_schedule() {
    SUSPEND_SCHEDULE_CALLS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_preempt_call() {
    PREEMPT_CALLS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_preempt_schedule() {
    PREEMPT_SCHEDULE_CALLS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_first_run_delay(elapsed_ns: usize) {
    record_timed(
        &FIRST_RUN_CALLS,
        &FIRST_RUN_NS_TOTAL,
        &FIRST_RUN_NS_MAX,
        elapsed_ns,
    );
}

pub fn record_ready_queue_push(len_after_push: usize) {
    READY_QUEUE_PUSHES.fetch_add(1, Ordering::Relaxed);
    update_atomic_max(&READY_QUEUE_MAX_LEN, len_after_push);
}

pub fn record_ready_queue_fetch() {
    READY_QUEUE_FETCHES.fetch_add(1, Ordering::Relaxed);
}

pub fn record_proc_smaps_read() {
    PROC_SMAPS_READ_CALLS.fetch_add(1, Ordering::Relaxed);
}

pub fn record_proc_smaps_render(elapsed_ns: usize, areas: usize, bytes: usize) {
    record_timed(
        &PROC_SMAPS_RENDER_CALLS,
        &PROC_SMAPS_RENDER_NS_TOTAL,
        &PROC_SMAPS_RENDER_NS_MAX,
        elapsed_ns,
    );
    PROC_SMAPS_RENDER_AREAS_TOTAL.fetch_add(areas, Ordering::Relaxed);
    PROC_SMAPS_RENDER_BYTES_TOTAL.fetch_add(bytes, Ordering::Relaxed);
}

pub fn record_exec_file_mapping(bytes: usize, pages: usize) {
    EXEC_FILE_MAPPINGS.fetch_add(1, Ordering::Relaxed);
    EXEC_FILE_LAZY_BYTES.fetch_add(bytes, Ordering::Relaxed);
    EXEC_FILE_LAZY_PAGES.fetch_add(pages, Ordering::Relaxed);
}

pub fn record_file_fault_shared_page() {
    FILE_FAULT_SHARED_PAGES.fetch_add(1, Ordering::Relaxed);
}

pub fn record_file_fault_private_copy() {
    FILE_FAULT_PRIVATE_COPIES.fetch_add(1, Ordering::Relaxed);
}

pub fn record_file_fault_zero_page() {
    FILE_FAULT_ZERO_PAGES.fetch_add(1, Ordering::Relaxed);
}

pub fn record_mprotect_phase(sample: MprotectPhaseSample) {
    let accounted_ns = sample
        .vm_lock_ns
        .saturating_add(sample.preflight_ns)
        .saturating_add(sample.vma_update_ns)
        .saturating_add(sample.pte_walk_ns)
        .saturating_add(sample.tlb_ns);
    let unaccounted_ns = sample.elapsed_ns.saturating_sub(accounted_ns);
    MPROTECT_PHASE_CALLS.fetch_add(1, Ordering::Relaxed);
    MPROTECT_PHASE_ELAPSED_NS_TOTAL.fetch_add(sample.elapsed_ns, Ordering::Relaxed);
    update_atomic_max(&MPROTECT_PHASE_ELAPSED_NS_MAX, sample.elapsed_ns);
    MPROTECT_INNER_NS_TOTAL.fetch_add(sample.inner_ns, Ordering::Relaxed);
    update_atomic_max(&MPROTECT_INNER_NS_MAX, sample.inner_ns);
    MPROTECT_ACCOUNTED_NS_TOTAL.fetch_add(accounted_ns, Ordering::Relaxed);
    MPROTECT_UNACCOUNTED_NS_TOTAL.fetch_add(unaccounted_ns, Ordering::Relaxed);
    update_atomic_max(&MPROTECT_UNACCOUNTED_NS_MAX, unaccounted_ns);
    if sample.context_switches != 0 {
        MPROTECT_CONTEXT_SWITCH_CALLS.fetch_add(1, Ordering::Relaxed);
        MPROTECT_CONTEXT_SWITCHES_TOTAL.fetch_add(sample.context_switches, Ordering::Relaxed);
    }
    MPROTECT_VM_LOCK_NS_TOTAL.fetch_add(sample.vm_lock_ns, Ordering::Relaxed);
    MPROTECT_PREFLIGHT_NS_TOTAL.fetch_add(sample.preflight_ns, Ordering::Relaxed);
    MPROTECT_VMA_UPDATE_NS_TOTAL.fetch_add(sample.vma_update_ns, Ordering::Relaxed);
    MPROTECT_PTE_WALK_NS_TOTAL.fetch_add(sample.pte_walk_ns, Ordering::Relaxed);
    MPROTECT_TLB_NS_TOTAL.fetch_add(sample.tlb_ns, Ordering::Relaxed);
    MPROTECT_PREFIX_EXTENSIONS.fetch_add(sample.prefix_extensions, Ordering::Relaxed);
    MPROTECT_VMA_SPLITS.fetch_add(sample.vma_splits, Ordering::Relaxed);
    MPROTECT_VMA_MERGES.fetch_add(sample.vma_merges, Ordering::Relaxed);
    MPROTECT_PTES_WALKED.fetch_add(sample.ptes_walked, Ordering::Relaxed);
    MPROTECT_PTES_PRESENT.fetch_add(sample.ptes_present, Ordering::Relaxed);
    MPROTECT_PTES_CHANGED.fetch_add(sample.ptes_changed, Ordering::Relaxed);
    match sample.tlb_kind {
        "none" => {
            MPROTECT_TLB_NONE_CALLS.fetch_add(1, Ordering::Relaxed);
        }
        "local_page" => {
            MPROTECT_TLB_LOCAL_PAGE_CALLS.fetch_add(1, Ordering::Relaxed);
        }
        "local_all" => {
            MPROTECT_TLB_LOCAL_ALL_CALLS.fetch_add(1, Ordering::Relaxed);
        }
        "remote" => {
            MPROTECT_TLB_REMOTE_CALLS.fetch_add(1, Ordering::Relaxed);
        }
        "icache" => {
            MPROTECT_TLB_ICACHE_CALLS.fetch_add(1, Ordering::Relaxed);
        }
        _ => {}
    }
}

pub fn mprotect_phase_snapshot() -> MprotectPhaseStatsSnapshot {
    MprotectPhaseStatsSnapshot {
        calls: MPROTECT_PHASE_CALLS.load(Ordering::Relaxed),
        elapsed_ns_total: MPROTECT_PHASE_ELAPSED_NS_TOTAL.load(Ordering::Relaxed),
        elapsed_ns_max: MPROTECT_PHASE_ELAPSED_NS_MAX.load(Ordering::Relaxed),
        inner_ns_total: MPROTECT_INNER_NS_TOTAL.load(Ordering::Relaxed),
        inner_ns_max: MPROTECT_INNER_NS_MAX.load(Ordering::Relaxed),
        accounted_ns_total: MPROTECT_ACCOUNTED_NS_TOTAL.load(Ordering::Relaxed),
        unaccounted_ns_total: MPROTECT_UNACCOUNTED_NS_TOTAL.load(Ordering::Relaxed),
        unaccounted_ns_max: MPROTECT_UNACCOUNTED_NS_MAX.load(Ordering::Relaxed),
        context_switch_calls: MPROTECT_CONTEXT_SWITCH_CALLS.load(Ordering::Relaxed),
        context_switches_total: MPROTECT_CONTEXT_SWITCHES_TOTAL.load(Ordering::Relaxed),
        vm_lock_ns_total: MPROTECT_VM_LOCK_NS_TOTAL.load(Ordering::Relaxed),
        preflight_ns_total: MPROTECT_PREFLIGHT_NS_TOTAL.load(Ordering::Relaxed),
        vma_update_ns_total: MPROTECT_VMA_UPDATE_NS_TOTAL.load(Ordering::Relaxed),
        pte_walk_ns_total: MPROTECT_PTE_WALK_NS_TOTAL.load(Ordering::Relaxed),
        tlb_ns_total: MPROTECT_TLB_NS_TOTAL.load(Ordering::Relaxed),
        prefix_extensions: MPROTECT_PREFIX_EXTENSIONS.load(Ordering::Relaxed),
        vma_splits: MPROTECT_VMA_SPLITS.load(Ordering::Relaxed),
        vma_merges: MPROTECT_VMA_MERGES.load(Ordering::Relaxed),
        ptes_walked: MPROTECT_PTES_WALKED.load(Ordering::Relaxed),
        ptes_present: MPROTECT_PTES_PRESENT.load(Ordering::Relaxed),
        ptes_changed: MPROTECT_PTES_CHANGED.load(Ordering::Relaxed),
        tlb_none_calls: MPROTECT_TLB_NONE_CALLS.load(Ordering::Relaxed),
        tlb_local_page_calls: MPROTECT_TLB_LOCAL_PAGE_CALLS.load(Ordering::Relaxed),
        tlb_local_all_calls: MPROTECT_TLB_LOCAL_ALL_CALLS.load(Ordering::Relaxed),
        tlb_remote_calls: MPROTECT_TLB_REMOTE_CALLS.load(Ordering::Relaxed),
        tlb_icache_calls: MPROTECT_TLB_ICACHE_CALLS.load(Ordering::Relaxed),
    }
}

pub fn record_anon_fault_phase(sample: AnonFaultPhaseSample) {
    ANON_FAULT_CALLS.fetch_add(1, Ordering::Relaxed);
    match sample.kind {
        AnonFaultKind::Heap => ANON_FAULT_HEAP_CALLS.fetch_add(1, Ordering::Relaxed),
        AnonFaultKind::Stack => ANON_FAULT_STACK_CALLS.fetch_add(1, Ordering::Relaxed),
        AnonFaultKind::Mmap => ANON_FAULT_MMAP_CALLS.fetch_add(1, Ordering::Relaxed),
        AnonFaultKind::Shared => ANON_FAULT_SHARED_CALLS.fetch_add(1, Ordering::Relaxed),
        AnonFaultKind::Elf => ANON_FAULT_ELF_CALLS.fetch_add(1, Ordering::Relaxed),
    };
    ANON_FAULT_TOTAL_NS.fetch_add(sample.total_ns, Ordering::Relaxed);
    update_atomic_max(&ANON_FAULT_TOTAL_NS_MAX, sample.total_ns);
    ANON_FAULT_FRAME_ALLOC_NS.fetch_add(sample.frame_alloc_ns, Ordering::Relaxed);
    update_atomic_max(&ANON_FAULT_FRAME_ALLOC_NS_MAX, sample.frame_alloc_ns);
    ANON_FAULT_ZERO_NS.fetch_add(sample.zero_ns, Ordering::Relaxed);
    update_atomic_max(&ANON_FAULT_ZERO_NS_MAX, sample.zero_ns);
    ANON_FAULT_PUBLISH_NS.fetch_add(sample.publish_ns, Ordering::Relaxed);
    ANON_FAULT_PAGE_TABLE_NS.fetch_add(sample.page_table_ns, Ordering::Relaxed);
    update_atomic_max(&ANON_FAULT_PAGE_TABLE_NS_MAX, sample.page_table_ns);
    ANON_FAULT_ICACHE_NS.fetch_add(sample.icache_ns, Ordering::Relaxed);
    ANON_FAULT_TLB_NS.fetch_add(sample.tlb_ns, Ordering::Relaxed);
    update_atomic_max(&ANON_FAULT_TLB_NS_MAX, sample.tlb_ns);
}

pub fn anon_fault_phase_snapshot() -> AnonFaultPhaseStatsSnapshot {
    AnonFaultPhaseStatsSnapshot {
        calls: ANON_FAULT_CALLS.load(Ordering::Relaxed),
        heap_calls: ANON_FAULT_HEAP_CALLS.load(Ordering::Relaxed),
        stack_calls: ANON_FAULT_STACK_CALLS.load(Ordering::Relaxed),
        mmap_calls: ANON_FAULT_MMAP_CALLS.load(Ordering::Relaxed),
        shared_calls: ANON_FAULT_SHARED_CALLS.load(Ordering::Relaxed),
        elf_calls: ANON_FAULT_ELF_CALLS.load(Ordering::Relaxed),
        total_ns: ANON_FAULT_TOTAL_NS.load(Ordering::Relaxed),
        total_ns_max: ANON_FAULT_TOTAL_NS_MAX.load(Ordering::Relaxed),
        frame_alloc_ns: ANON_FAULT_FRAME_ALLOC_NS.load(Ordering::Relaxed),
        frame_alloc_ns_max: ANON_FAULT_FRAME_ALLOC_NS_MAX.load(Ordering::Relaxed),
        zero_ns: ANON_FAULT_ZERO_NS.load(Ordering::Relaxed),
        zero_ns_max: ANON_FAULT_ZERO_NS_MAX.load(Ordering::Relaxed),
        publish_ns: ANON_FAULT_PUBLISH_NS.load(Ordering::Relaxed),
        page_table_ns: ANON_FAULT_PAGE_TABLE_NS.load(Ordering::Relaxed),
        page_table_ns_max: ANON_FAULT_PAGE_TABLE_NS_MAX.load(Ordering::Relaxed),
        icache_ns: ANON_FAULT_ICACHE_NS.load(Ordering::Relaxed),
        tlb_ns: ANON_FAULT_TLB_NS.load(Ordering::Relaxed),
        tlb_ns_max: ANON_FAULT_TLB_NS_MAX.load(Ordering::Relaxed),
    }
}

pub fn record_page_table_activation(skipped: bool) {
    if skipped {
        PAGE_TABLE_ACTIVATION_SKIPS.fetch_add(1, Ordering::Relaxed);
    } else {
        PAGE_TABLE_ACTIVATIONS.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn record_idle_wfi() {
    IDLE_WFI_CALLS.fetch_add(1, Ordering::Relaxed);
}

pub fn reset() {
    CLONE_THREAD_CALLS.store(0, Ordering::Relaxed);
    CLONE_THREAD_NS_TOTAL.store(0, Ordering::Relaxed);
    CLONE_THREAD_NS_MAX.store(0, Ordering::Relaxed);
    CLONE_PROCESS_CALLS.store(0, Ordering::Relaxed);
    CLONE_PROCESS_NS_TOTAL.store(0, Ordering::Relaxed);
    CLONE_PROCESS_NS_MAX.store(0, Ordering::Relaxed);
    EXIT_CALLS.store(0, Ordering::Relaxed);
    EXIT_NS_TOTAL.store(0, Ordering::Relaxed);
    EXIT_NS_MAX.store(0, Ordering::Relaxed);
    KSTACK_ALLOC_CALLS.store(0, Ordering::Relaxed);
    KSTACK_ALLOC_NS_TOTAL.store(0, Ordering::Relaxed);
    KSTACK_ALLOC_NS_MAX.store(0, Ordering::Relaxed);
    TCB_NEW_CALLS.store(0, Ordering::Relaxed);
    TCB_NEW_NS_TOTAL.store(0, Ordering::Relaxed);
    TCB_NEW_NS_MAX.store(0, Ordering::Relaxed);
    TASK_USER_RES_NEW_CALLS.store(0, Ordering::Relaxed);
    TASK_USER_RES_NEW_NS_TOTAL.store(0, Ordering::Relaxed);
    TASK_USER_RES_NEW_NS_MAX.store(0, Ordering::Relaxed);
    FUTEX_WAIT_CALLS.store(0, Ordering::Relaxed);
    FUTEX_WAIT_NS_TOTAL.store(0, Ordering::Relaxed);
    FUTEX_WAIT_NS_MAX.store(0, Ordering::Relaxed);
    FUTEX_WAIT_BLOCK_CALLS.store(0, Ordering::Relaxed);
    FUTEX_WAIT_SUSPEND_CALLS.store(0, Ordering::Relaxed);
    FUTEX_WAKE_CALLS.store(0, Ordering::Relaxed);
    FUTEX_WAKE_NS_TOTAL.store(0, Ordering::Relaxed);
    FUTEX_WAKE_NS_MAX.store(0, Ordering::Relaxed);
    FUTEX_WAKE_WOKEN_TOTAL.store(0, Ordering::Relaxed);
    FUTEX_WAKE_ONE_CALLS.store(0, Ordering::Relaxed);
    FUTEX_WAKE_ONE_NS_TOTAL.store(0, Ordering::Relaxed);
    FUTEX_WAKE_ONE_NS_MAX.store(0, Ordering::Relaxed);
    FUTEX_WAKE_ONE_WOKEN_TOTAL.store(0, Ordering::Relaxed);
    BLOCK_CALLS.store(0, Ordering::Relaxed);
    BLOCK_SCHEDULE_CALLS.store(0, Ordering::Relaxed);
    BLOCK_FAST_RETURN_CALLS.store(0, Ordering::Relaxed);
    SUSPEND_CALLS.store(0, Ordering::Relaxed);
    SUSPEND_SCHEDULE_CALLS.store(0, Ordering::Relaxed);
    PREEMPT_CALLS.store(0, Ordering::Relaxed);
    PREEMPT_SCHEDULE_CALLS.store(0, Ordering::Relaxed);
    FIRST_RUN_CALLS.store(0, Ordering::Relaxed);
    FIRST_RUN_NS_TOTAL.store(0, Ordering::Relaxed);
    FIRST_RUN_NS_MAX.store(0, Ordering::Relaxed);
    READY_QUEUE_PUSHES.store(0, Ordering::Relaxed);
    READY_QUEUE_FETCHES.store(0, Ordering::Relaxed);
    READY_QUEUE_MAX_LEN.store(0, Ordering::Relaxed);
    PROC_SMAPS_READ_CALLS.store(0, Ordering::Relaxed);
    PROC_SMAPS_RENDER_CALLS.store(0, Ordering::Relaxed);
    PROC_SMAPS_RENDER_NS_TOTAL.store(0, Ordering::Relaxed);
    PROC_SMAPS_RENDER_NS_MAX.store(0, Ordering::Relaxed);
    PROC_SMAPS_RENDER_AREAS_TOTAL.store(0, Ordering::Relaxed);
    PROC_SMAPS_RENDER_BYTES_TOTAL.store(0, Ordering::Relaxed);
    MMAP_CALLS.store(0, Ordering::Relaxed);
    MMAP_NS_TOTAL.store(0, Ordering::Relaxed);
    MMAP_NS_MAX.store(0, Ordering::Relaxed);
    MUNMAP_CALLS.store(0, Ordering::Relaxed);
    MUNMAP_NS_TOTAL.store(0, Ordering::Relaxed);
    MUNMAP_NS_MAX.store(0, Ordering::Relaxed);
    MPROTECT_CALLS.store(0, Ordering::Relaxed);
    MPROTECT_NS_TOTAL.store(0, Ordering::Relaxed);
    MPROTECT_NS_MAX.store(0, Ordering::Relaxed);
    MPROTECT_PHASE_CALLS.store(0, Ordering::Relaxed);
    MPROTECT_PHASE_ELAPSED_NS_TOTAL.store(0, Ordering::Relaxed);
    MPROTECT_PHASE_ELAPSED_NS_MAX.store(0, Ordering::Relaxed);
    MPROTECT_INNER_NS_TOTAL.store(0, Ordering::Relaxed);
    MPROTECT_INNER_NS_MAX.store(0, Ordering::Relaxed);
    MPROTECT_ACCOUNTED_NS_TOTAL.store(0, Ordering::Relaxed);
    MPROTECT_UNACCOUNTED_NS_TOTAL.store(0, Ordering::Relaxed);
    MPROTECT_UNACCOUNTED_NS_MAX.store(0, Ordering::Relaxed);
    MPROTECT_CONTEXT_SWITCH_CALLS.store(0, Ordering::Relaxed);
    MPROTECT_CONTEXT_SWITCHES_TOTAL.store(0, Ordering::Relaxed);
    MPROTECT_VM_LOCK_NS_TOTAL.store(0, Ordering::Relaxed);
    MPROTECT_PREFLIGHT_NS_TOTAL.store(0, Ordering::Relaxed);
    MPROTECT_VMA_UPDATE_NS_TOTAL.store(0, Ordering::Relaxed);
    MPROTECT_PTE_WALK_NS_TOTAL.store(0, Ordering::Relaxed);
    MPROTECT_TLB_NS_TOTAL.store(0, Ordering::Relaxed);
    MPROTECT_PREFIX_EXTENSIONS.store(0, Ordering::Relaxed);
    MPROTECT_VMA_SPLITS.store(0, Ordering::Relaxed);
    MPROTECT_VMA_MERGES.store(0, Ordering::Relaxed);
    MPROTECT_PTES_WALKED.store(0, Ordering::Relaxed);
    MPROTECT_PTES_PRESENT.store(0, Ordering::Relaxed);
    MPROTECT_PTES_CHANGED.store(0, Ordering::Relaxed);
    MPROTECT_TLB_NONE_CALLS.store(0, Ordering::Relaxed);
    MPROTECT_TLB_LOCAL_PAGE_CALLS.store(0, Ordering::Relaxed);
    MPROTECT_TLB_LOCAL_ALL_CALLS.store(0, Ordering::Relaxed);
    MPROTECT_TLB_REMOTE_CALLS.store(0, Ordering::Relaxed);
    MPROTECT_TLB_ICACHE_CALLS.store(0, Ordering::Relaxed);
    ANON_FAULT_CALLS.store(0, Ordering::Relaxed);
    ANON_FAULT_HEAP_CALLS.store(0, Ordering::Relaxed);
    ANON_FAULT_STACK_CALLS.store(0, Ordering::Relaxed);
    ANON_FAULT_MMAP_CALLS.store(0, Ordering::Relaxed);
    ANON_FAULT_SHARED_CALLS.store(0, Ordering::Relaxed);
    ANON_FAULT_ELF_CALLS.store(0, Ordering::Relaxed);
    ANON_FAULT_TOTAL_NS.store(0, Ordering::Relaxed);
    ANON_FAULT_TOTAL_NS_MAX.store(0, Ordering::Relaxed);
    ANON_FAULT_FRAME_ALLOC_NS.store(0, Ordering::Relaxed);
    ANON_FAULT_FRAME_ALLOC_NS_MAX.store(0, Ordering::Relaxed);
    ANON_FAULT_ZERO_NS.store(0, Ordering::Relaxed);
    ANON_FAULT_ZERO_NS_MAX.store(0, Ordering::Relaxed);
    ANON_FAULT_PUBLISH_NS.store(0, Ordering::Relaxed);
    ANON_FAULT_PAGE_TABLE_NS.store(0, Ordering::Relaxed);
    ANON_FAULT_PAGE_TABLE_NS_MAX.store(0, Ordering::Relaxed);
    ANON_FAULT_ICACHE_NS.store(0, Ordering::Relaxed);
    ANON_FAULT_TLB_NS.store(0, Ordering::Relaxed);
    ANON_FAULT_TLB_NS_MAX.store(0, Ordering::Relaxed);
    EXEC_FILE_MAPPINGS.store(0, Ordering::Relaxed);
    EXEC_FILE_LAZY_BYTES.store(0, Ordering::Relaxed);
    EXEC_FILE_LAZY_PAGES.store(0, Ordering::Relaxed);
    FILE_FAULT_SHARED_PAGES.store(0, Ordering::Relaxed);
    FILE_FAULT_PRIVATE_COPIES.store(0, Ordering::Relaxed);
    FILE_FAULT_ZERO_PAGES.store(0, Ordering::Relaxed);
    PAGE_TABLE_ACTIVATIONS.store(0, Ordering::Relaxed);
    PAGE_TABLE_ACTIVATION_SKIPS.store(0, Ordering::Relaxed);
    polyhal::multicore::reset_tlb_shootdown_calls();
    IDLE_WFI_CALLS.store(0, Ordering::Relaxed);
}

pub fn snapshot() -> PerfStatsSnapshot {
    PerfStatsSnapshot {
        clone_thread_calls: CLONE_THREAD_CALLS.load(Ordering::Relaxed),
        clone_thread_ns_total: CLONE_THREAD_NS_TOTAL.load(Ordering::Relaxed),
        clone_thread_ns_max: CLONE_THREAD_NS_MAX.load(Ordering::Relaxed),
        clone_process_calls: CLONE_PROCESS_CALLS.load(Ordering::Relaxed),
        clone_process_ns_total: CLONE_PROCESS_NS_TOTAL.load(Ordering::Relaxed),
        clone_process_ns_max: CLONE_PROCESS_NS_MAX.load(Ordering::Relaxed),
        exit_calls: EXIT_CALLS.load(Ordering::Relaxed),
        exit_ns_total: EXIT_NS_TOTAL.load(Ordering::Relaxed),
        exit_ns_max: EXIT_NS_MAX.load(Ordering::Relaxed),
        kstack_alloc_calls: KSTACK_ALLOC_CALLS.load(Ordering::Relaxed),
        kstack_alloc_ns_total: KSTACK_ALLOC_NS_TOTAL.load(Ordering::Relaxed),
        kstack_alloc_ns_max: KSTACK_ALLOC_NS_MAX.load(Ordering::Relaxed),
        tcb_new_calls: TCB_NEW_CALLS.load(Ordering::Relaxed),
        tcb_new_ns_total: TCB_NEW_NS_TOTAL.load(Ordering::Relaxed),
        tcb_new_ns_max: TCB_NEW_NS_MAX.load(Ordering::Relaxed),
        task_user_res_new_calls: TASK_USER_RES_NEW_CALLS.load(Ordering::Relaxed),
        task_user_res_new_ns_total: TASK_USER_RES_NEW_NS_TOTAL.load(Ordering::Relaxed),
        task_user_res_new_ns_max: TASK_USER_RES_NEW_NS_MAX.load(Ordering::Relaxed),
        futex_wait_calls: FUTEX_WAIT_CALLS.load(Ordering::Relaxed),
        futex_wait_ns_total: FUTEX_WAIT_NS_TOTAL.load(Ordering::Relaxed),
        futex_wait_ns_max: FUTEX_WAIT_NS_MAX.load(Ordering::Relaxed),
        futex_wait_block_calls: FUTEX_WAIT_BLOCK_CALLS.load(Ordering::Relaxed),
        futex_wait_suspend_calls: FUTEX_WAIT_SUSPEND_CALLS.load(Ordering::Relaxed),
        futex_wake_calls: FUTEX_WAKE_CALLS.load(Ordering::Relaxed),
        futex_wake_ns_total: FUTEX_WAKE_NS_TOTAL.load(Ordering::Relaxed),
        futex_wake_ns_max: FUTEX_WAKE_NS_MAX.load(Ordering::Relaxed),
        futex_wake_woken_total: FUTEX_WAKE_WOKEN_TOTAL.load(Ordering::Relaxed),
        futex_wake_one_calls: FUTEX_WAKE_ONE_CALLS.load(Ordering::Relaxed),
        futex_wake_one_ns_total: FUTEX_WAKE_ONE_NS_TOTAL.load(Ordering::Relaxed),
        futex_wake_one_ns_max: FUTEX_WAKE_ONE_NS_MAX.load(Ordering::Relaxed),
        futex_wake_one_woken_total: FUTEX_WAKE_ONE_WOKEN_TOTAL.load(Ordering::Relaxed),
        block_calls: BLOCK_CALLS.load(Ordering::Relaxed),
        block_schedule_calls: BLOCK_SCHEDULE_CALLS.load(Ordering::Relaxed),
        block_fast_return_calls: BLOCK_FAST_RETURN_CALLS.load(Ordering::Relaxed),
        suspend_calls: SUSPEND_CALLS.load(Ordering::Relaxed),
        suspend_schedule_calls: SUSPEND_SCHEDULE_CALLS.load(Ordering::Relaxed),
        preempt_calls: PREEMPT_CALLS.load(Ordering::Relaxed),
        preempt_schedule_calls: PREEMPT_SCHEDULE_CALLS.load(Ordering::Relaxed),
        first_run_calls: FIRST_RUN_CALLS.load(Ordering::Relaxed),
        first_run_ns_total: FIRST_RUN_NS_TOTAL.load(Ordering::Relaxed),
        first_run_ns_max: FIRST_RUN_NS_MAX.load(Ordering::Relaxed),
        ready_queue_pushes: READY_QUEUE_PUSHES.load(Ordering::Relaxed),
        ready_queue_fetches: READY_QUEUE_FETCHES.load(Ordering::Relaxed),
        ready_queue_max_len: READY_QUEUE_MAX_LEN.load(Ordering::Relaxed),
        proc_smaps_read_calls: PROC_SMAPS_READ_CALLS.load(Ordering::Relaxed),
        proc_smaps_render_calls: PROC_SMAPS_RENDER_CALLS.load(Ordering::Relaxed),
        proc_smaps_render_ns_total: PROC_SMAPS_RENDER_NS_TOTAL.load(Ordering::Relaxed),
        proc_smaps_render_ns_max: PROC_SMAPS_RENDER_NS_MAX.load(Ordering::Relaxed),
        proc_smaps_render_areas_total: PROC_SMAPS_RENDER_AREAS_TOTAL.load(Ordering::Relaxed),
        proc_smaps_render_bytes_total: PROC_SMAPS_RENDER_BYTES_TOTAL.load(Ordering::Relaxed),
        mmap_calls: MMAP_CALLS.load(Ordering::Relaxed),
        mmap_ns_total: MMAP_NS_TOTAL.load(Ordering::Relaxed),
        mmap_ns_max: MMAP_NS_MAX.load(Ordering::Relaxed),
        munmap_calls: MUNMAP_CALLS.load(Ordering::Relaxed),
        munmap_ns_total: MUNMAP_NS_TOTAL.load(Ordering::Relaxed),
        munmap_ns_max: MUNMAP_NS_MAX.load(Ordering::Relaxed),
        mprotect_calls: MPROTECT_CALLS.load(Ordering::Relaxed),
        mprotect_ns_total: MPROTECT_NS_TOTAL.load(Ordering::Relaxed),
        mprotect_ns_max: MPROTECT_NS_MAX.load(Ordering::Relaxed),
        exec_file_mappings: EXEC_FILE_MAPPINGS.load(Ordering::Relaxed),
        exec_file_lazy_bytes: EXEC_FILE_LAZY_BYTES.load(Ordering::Relaxed),
        exec_file_lazy_pages: EXEC_FILE_LAZY_PAGES.load(Ordering::Relaxed),
        file_fault_shared_pages: FILE_FAULT_SHARED_PAGES.load(Ordering::Relaxed),
        file_fault_private_copies: FILE_FAULT_PRIVATE_COPIES.load(Ordering::Relaxed),
        file_fault_zero_pages: FILE_FAULT_ZERO_PAGES.load(Ordering::Relaxed),
        page_table_activations: PAGE_TABLE_ACTIVATIONS.load(Ordering::Relaxed),
        page_table_activation_skips: PAGE_TABLE_ACTIVATION_SKIPS.load(Ordering::Relaxed),
        tlb_shootdown_calls: polyhal::multicore::tlb_shootdown_calls(),
        idle_wfi_calls: IDLE_WFI_CALLS.load(Ordering::Relaxed),
    }
}

fn record_kstack_alloc_ns(elapsed_ns: usize) {
    record_timed(
        &KSTACK_ALLOC_CALLS,
        &KSTACK_ALLOC_NS_TOTAL,
        &KSTACK_ALLOC_NS_MAX,
        elapsed_ns,
    );
}

fn record_tcb_new_ns(elapsed_ns: usize) {
    record_timed(
        &TCB_NEW_CALLS,
        &TCB_NEW_NS_TOTAL,
        &TCB_NEW_NS_MAX,
        elapsed_ns,
    );
}

fn record_task_user_res_new_ns(elapsed_ns: usize) {
    record_timed(
        &TASK_USER_RES_NEW_CALLS,
        &TASK_USER_RES_NEW_NS_TOTAL,
        &TASK_USER_RES_NEW_NS_MAX,
        elapsed_ns,
    );
}

fn record_futex_wait_ns(elapsed_ns: usize) {
    record_timed(
        &FUTEX_WAIT_CALLS,
        &FUTEX_WAIT_NS_TOTAL,
        &FUTEX_WAIT_NS_MAX,
        elapsed_ns,
    );
}

fn record_futex_wake_ns(elapsed_ns: usize) {
    record_timed(
        &FUTEX_WAKE_CALLS,
        &FUTEX_WAKE_NS_TOTAL,
        &FUTEX_WAKE_NS_MAX,
        elapsed_ns,
    );
}

fn record_futex_wake_one_ns(elapsed_ns: usize) {
    record_timed(
        &FUTEX_WAKE_ONE_CALLS,
        &FUTEX_WAKE_ONE_NS_TOTAL,
        &FUTEX_WAKE_ONE_NS_MAX,
        elapsed_ns,
    );
}

fn record_mmap_ns(elapsed_ns: usize) {
    record_timed(&MMAP_CALLS, &MMAP_NS_TOTAL, &MMAP_NS_MAX, elapsed_ns);
}

fn record_munmap_ns(elapsed_ns: usize) {
    record_timed(&MUNMAP_CALLS, &MUNMAP_NS_TOTAL, &MUNMAP_NS_MAX, elapsed_ns);
}

fn record_mprotect_ns(elapsed_ns: usize) {
    record_timed(
        &MPROTECT_CALLS,
        &MPROTECT_NS_TOTAL,
        &MPROTECT_NS_MAX,
        elapsed_ns,
    );
}

fn record_timed(
    calls: &AtomicUsize,
    total_ns: &AtomicUsize,
    max_ns: &AtomicUsize,
    elapsed_ns: usize,
) {
    calls.fetch_add(1, Ordering::Relaxed);
    total_ns.fetch_add(elapsed_ns, Ordering::Relaxed);
    update_atomic_max(max_ns, elapsed_ns);
}

fn update_atomic_max(atom: &AtomicUsize, value: usize) {
    let mut old = atom.load(Ordering::Relaxed);
    while value > old {
        match atom.compare_exchange_weak(old, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(next) => old = next,
        }
    }
}
