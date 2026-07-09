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
