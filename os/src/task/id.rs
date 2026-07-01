use super::ProcessControlBlock;
// use crate::config::{
//     KERNEL_MEMORY_SPACE, KERNEL_STACK_SIZE, KERNEL_THREAD_STACK_BASE, PAGE_SIZE, TRAP_CONTEXT,
//     USER_STACK_SIZE,
// };
use crate::mm::{
    KERNEL_VMSET, KernelAreaType, MapPermission, UserMapAreaType, VMSpace, frame_alloc,
};

use crate::sync::SpinLock;
use crate::sync::mutex::*;
use alloc::{
    collections::BTreeMap,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::sync::atomic::{AtomicUsize, Ordering};
use lazy_static::*;
use log::{error, info, warn};
pub use polyhal::utils::addr::*;
use polyhal::{consts::*, println};
use polyhal_trap::trapframe::TrapFrame;

static PID_HANDLE_ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static PID_HANDLE_DROP_COUNT: AtomicUsize = AtomicUsize::new(0);
static RAW_PID_ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static RAW_PID_DEALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static KSTACK_HANDLE_ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static KSTACK_HANDLE_DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

pub struct RecycleAllocator {
    current: usize,
    recycled: Vec<usize>,
}

#[derive(Debug, Clone, Copy)]
struct RecycleAllocatorStats {
    current: usize,
    recycled: usize,
    live: usize,
}

impl RecycleAllocator {
    pub fn new() -> Self {
        RecycleAllocator {
            current: 0,
            recycled: Vec::new(),
        }
    }
    pub fn with_start(start: usize) -> Self {
        RecycleAllocator {
            current: start,
            recycled: Vec::new(),
        }
    }
    pub fn alloc(&mut self) -> usize {
        if let Some(id) = self.recycled.pop() {
            id
        } else {
            self.current += 1;
            self.current - 1
        }
    }
    pub fn dealloc(&mut self, id: usize) {
        assert!(id < self.current);
        assert!(
            !self.recycled.iter().any(|i| *i == id),
            "id {} has been deallocated!",
            id
        );
        self.recycled.push(id);
    }

    fn stats(&self) -> RecycleAllocatorStats {
        RecycleAllocatorStats {
            current: self.current,
            recycled: self.recycled.len(),
            live: self.current.saturating_sub(self.recycled.len()),
        }
    }
}

lazy_static! {
    static ref PID_ALLOCATOR: SpinLock<RecycleAllocator> =
        SpinLock::new(RecycleAllocator::with_start(1));
    static ref KSTACK_ALLOCATOR: SpinLock<RecycleAllocator> =
        SpinLock::new(RecycleAllocator::new());
}

#[allow(missing_docs)]
pub const IDLE_PID: usize = 0;
#[allow(missing_docs)]
pub struct PidHandle(pub usize);
#[allow(missing_docs)]
pub fn pid_alloc() -> PidHandle {
    PID_HANDLE_ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
    PidHandle(PID_ALLOCATOR.lock().alloc())
}

/// Allocate a raw PID without creating a PidHandle.
/// Caller is responsible for calling `dealloc_pid` later.
#[allow(missing_docs)]
pub fn alloc_pid_raw() -> usize {
    RAW_PID_ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
    PID_ALLOCATOR.lock().alloc()
}

impl Drop for PidHandle {
    fn drop(&mut self) {
        PID_HANDLE_DROP_COUNT.fetch_add(1, Ordering::Relaxed);
        PID_ALLOCATOR.lock().dealloc(self.0);
    }
}

/// Deallocate a raw PID without owning a PidHandle.
#[allow(missing_docs)]
pub fn dealloc_pid(pid: usize) {
    RAW_PID_DEALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
    PID_ALLOCATOR.lock().dealloc(pid);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PgidHandle(pub usize);

/// Return (bottom, top) of a kernel stack in kernel space.
pub fn kernel_stack_position(kstack_id: usize) -> (usize, usize) {
    let top = KERNEL_THREAD_STACK_BASE - (kstack_id + 1) * (KERNEL_STACK_SIZE + PAGE_SIZE) + 1;
    let bottom = top - KERNEL_STACK_SIZE;
    (bottom, top)
}

fn print_kstack_alloc_failure(
    kstack_id: usize,
    kstack_bottom: usize,
    kstack_top: usize,
    failed_vpn: VirtPageNum,
    allocated_pages: usize,
    required_pages: usize,
    kstack_stats: RecycleAllocatorStats,
) {
    println!(
        "[OOM] kstack_alloc failed: id={} range=[{:#x}, {:#x}) failed_vpn={:#x} pages={}/{} stack_size={} page_size={}",
        kstack_id,
        kstack_bottom,
        kstack_top,
        failed_vpn.0,
        allocated_pages,
        required_pages,
        KERNEL_STACK_SIZE,
        PAGE_SIZE
    );
    print_oom_snapshot_with_kstack_stats(Some(kstack_stats));
}

fn print_user_res_alloc_failure(
    kind: &str,
    tid: usize,
    global_tid: usize,
    range_start: usize,
    range_end: usize,
    failed_vpn: VirtPageNum,
    allocated_pages: usize,
    required_pages: usize,
) {
    println!(
        "[OOM] task_user_res {} alloc failed: tid={} global_tid={} range=[{:#x}, {:#x}) failed_vpn={:#x} pages={}/{} page_size={}",
        kind,
        tid,
        global_tid,
        range_start,
        range_end,
        failed_vpn.0,
        allocated_pages,
        required_pages,
        PAGE_SIZE
    );
    print_oom_snapshot_with_kstack_stats(None);
}

#[allow(missing_docs)]
pub(crate) fn print_oom_snapshot() {
    print_oom_snapshot_with_kstack_stats(None);
}

fn print_oom_snapshot_with_kstack_stats(kstack_stats_override: Option<RecycleAllocatorStats>) {
    let frame = crate::mm::frame_stats();
    let heap = crate::mm::heap_allocator::heap_stats();
    let pid_stats = PID_ALLOCATOR.lock().stats();
    let kstack_stats = kstack_stats_override.unwrap_or_else(|| KSTACK_ALLOCATOR.lock().stats());
    let deferred_tasks = super::deferred_exited_task_count();

    println!(
        "[OOM] frames: used_pages={} free_pages={} fresh_free_pages={} recycled_pages={} total_pages={} free_bytes={} total_bytes={} alloc_count={} free_count={} delta={}",
        frame.used_pages,
        frame.free_pages,
        frame.fresh_free_pages,
        frame.recycled_pages,
        frame.total_pages,
        frame.free_pages * PAGE_SIZE,
        frame.total_pages * PAGE_SIZE,
        frame.alloc_count,
        frame.free_count,
        frame.allocated_delta
    );
    println!(
        "[OOM] heap: user={} actual={} free={} total={}",
        heap.user, heap.actual, heap.free, heap.total
    );
    println!(
        "[OOM] ids: kstack_current={} kstack_live={} kstack_recycled={} pid_current={} pid_live={} pid_recycled={} deferred_exited_tasks={}",
        kstack_stats.current,
        kstack_stats.live,
        kstack_stats.recycled,
        pid_stats.current,
        pid_stats.live,
        pid_stats.recycled,
        deferred_tasks
    );
    let task_lifecycle = crate::task::task::task_lifecycle_stats();
    println!(
        "[OOM] lifecycle: tasks_created={} tasks_dropped={} tasks_live_delta={} kstack_alloc_handles={} kstack_drop_handles={} kstack_handle_delta={} pid_handle_alloc={} pid_handle_drop={} pid_handle_delta={} raw_pid_alloc={} raw_pid_dealloc={} raw_pid_delta={}",
        task_lifecycle.created,
        task_lifecycle.dropped,
        task_lifecycle.live_delta,
        KSTACK_HANDLE_ALLOC_COUNT.load(Ordering::Relaxed),
        KSTACK_HANDLE_DROP_COUNT.load(Ordering::Relaxed),
        KSTACK_HANDLE_ALLOC_COUNT
            .load(Ordering::Relaxed)
            .saturating_sub(KSTACK_HANDLE_DROP_COUNT.load(Ordering::Relaxed)),
        PID_HANDLE_ALLOC_COUNT.load(Ordering::Relaxed),
        PID_HANDLE_DROP_COUNT.load(Ordering::Relaxed),
        PID_HANDLE_ALLOC_COUNT
            .load(Ordering::Relaxed)
            .saturating_sub(PID_HANDLE_DROP_COUNT.load(Ordering::Relaxed)),
        RAW_PID_ALLOC_COUNT.load(Ordering::Relaxed),
        RAW_PID_DEALLOC_COUNT.load(Ordering::Relaxed),
        RAW_PID_ALLOC_COUNT
            .load(Ordering::Relaxed)
            .saturating_sub(RAW_PID_DEALLOC_COUNT.load(Ordering::Relaxed))
    );
    let task_stats = super::task_retention_stats();
    let processor_stats = crate::task::processor::processor_task_stats();
    println!(
        "[OOM] tasks: processes={} locked_processes={} zombie_processes={} child_refs={} max_child_refs={} max_child_refs_pid={} task_slots={} zombie_task_slots={} max_task_slots={} max_task_slots_pid={} ready_queue_tasks={} current_tasks={} locked_processors={} timer_queue_tasks={} timer_queue_lock_busy={}",
        task_stats.processes,
        task_stats.locked_processes,
        task_stats.zombie_processes,
        task_stats.child_refs,
        task_stats.max_child_refs,
        task_stats.max_child_refs_pid,
        task_stats.task_slots,
        task_stats.zombie_task_slots,
        task_stats.max_task_slots,
        task_stats.max_task_slots_pid,
        task_stats.ready_queue_tasks,
        processor_stats.current_tasks,
        processor_stats.locked_processors,
        task_stats.timer_queue_tasks,
        task_stats.timer_queue_lock_busy
    );
    let tid_stats = crate::task::manager::tid2task_stats();
    println!(
        "[OOM] tid2task: entries={} live={} dead={} lock_busy={}",
        tid_stats.entries, tid_stats.live, tid_stats.dead, tid_stats.lock_busy
    );
    let futex_stats = crate::syscall::futex::stats();
    println!(
        "[OOM] futex: queues={} waiters={} lock_busy={}",
        futex_stats.queues, futex_stats.waiters, futex_stats.lock_busy
    );
    if let Some(cache) = crate::fs::page::pagecache::PAGE_CACHE.try_lock() {
        let stats = cache.stats();
        println!(
            "[OOM] page_cache: pages={} dirty={} disk_pages={} disk_dirty={} disk_limit={} tmpfs={} tmpfs_swapped={} fat32={} ext4={} unknown={} writeback_pending={}",
            stats.pages,
            stats.dirty_pages,
            stats.disk_pages,
            stats.dirty_disk_pages,
            stats.max_disk_pages,
            stats.tmpfs_pages,
            stats.swapped_tmpfs_pages,
            stats.fat32_pages,
            stats.ext4_pages,
            stats.unknown_pages,
            crate::fs::writeback::pending_count()
        );
    } else {
        println!(
            "[OOM] page_cache: lock busy writeback_pending={}",
            crate::fs::writeback::pending_count()
        );
    }
    let swap = crate::mm::swap::stats();
    println!(
        "[OOM] swap: enabled={} used_slots={} free_slots={} total_slots={} alloc_count={} free_count={}",
        swap.enabled,
        swap.used_slots,
        swap.free_slots,
        swap.total_slots,
        swap.alloc_count,
        swap.free_count
    );
}

#[allow(missing_docs)]
pub struct KernelStack(pub usize);
#[allow(missing_docs)]
pub fn kstack_alloc() -> KernelStack {
    KSTACK_HANDLE_ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
    let kstack_id = KSTACK_ALLOCATOR.lock().alloc();
    let (kstack_bottom, kstack_top) = kernel_stack_position(kstack_id);
    info!(
        "bottom {:#x}, top {:#x}",
        kstack_bottom >> 12,
        kstack_top >> 12
    );

    let start_vpn = VirtAddr::from(kstack_bottom).floor();
    let end_vpn = VirtAddr::from(kstack_top).ceil();
    let required_pages = end_vpn.0.saturating_sub(start_vpn.0);
    let mut data_frames = BTreeMap::new();
    for vpn in VPNRange::new(start_vpn, end_vpn) {
        let Some(frame) = frame_alloc() else {
            let kstack_stats = {
                let mut allocator = KSTACK_ALLOCATOR.lock();
                allocator.dealloc(kstack_id);
                allocator.stats()
            };
            print_kstack_alloc_failure(
                kstack_id,
                kstack_bottom,
                kstack_top,
                vpn,
                data_frames.len(),
                required_pages,
                kstack_stats,
            );
            panic!("failed to allocate kernel stack frame");
        };
        data_frames.insert(vpn, frame);
    }

    {
        let mut kernel_vmset = KERNEL_VMSET.lock();
        kernel_vmset.insert_framed_area_with_frames(
            kstack_bottom.into(),
            kstack_top.into(),
            MapPermission::R | MapPermission::W,
            KernelAreaType::KernelStack,
            data_frames,
        );
        if let Some(pa) = kernel_vmset
            .page_table()
            .translate_va(VirtAddr::from(kstack_bottom))
        {
            info!("alloc kstack pa {:#x}", pa.0);
        } else {
            error!("not mapped");
        }
    }
    KernelStack(kstack_id)
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        KSTACK_HANDLE_DROP_COUNT.fetch_add(1, Ordering::Relaxed);
        let (kernel_stack_bottom, _) = kernel_stack_position(self.0);
        let kernel_stack_bottom_va: VirtAddr = kernel_stack_bottom.into();
        KERNEL_VMSET
            .lock()
            .remove_area_with_start_vpn(kernel_stack_bottom_va.into());
        KSTACK_ALLOCATOR.lock().dealloc(self.0);
    }
}
#[allow(missing_docs)]
impl KernelStack {
    #[allow(unused)]
    pub fn push_on_top<T>(&self, value: T) -> *mut T
    where
        T: Sized,
    {
        let kernel_stack_top = self.get_top();
        let ptr_mut = (kernel_stack_top - core::mem::size_of::<T>()) as *mut T;
        unsafe {
            *ptr_mut = value;
        }
        ptr_mut
    }
    pub fn get_top(&self) -> usize {
        let (_, kernel_stack_top) = kernel_stack_position(self.0);
        kernel_stack_top
    }
}

pub struct TaskUserRes {
    pub tid: usize,
    pub global_tid: usize,
    pub ustack_base: usize,
    pub process: Weak<ProcessControlBlock>,
    owns_user_res: bool,
}

fn trap_cx_bottom_from_tid(tid: usize) -> usize {
    TRAP_CONTEXT - (tid + 1) * PAGE_SIZE
}

fn ustack_bottom_from_tid(ustack_base: usize, tid: usize) -> usize {
    ustack_base - (tid + 1) * (PAGE_SIZE + USER_STACK_SIZE)
}
#[allow(unused)]
impl TaskUserRes {
    pub fn new(
        process: Arc<ProcessControlBlock>,
        ustack_base: usize,
        alloc_user_res: bool,
        global_tid: usize,
    ) -> Self {
        let tid = process.inner_exclusive_access().alloc_tid();

        let mut task_user_res = Self {
            tid,
            global_tid,
            ustack_base,
            process: Arc::downgrade(&process),
            owns_user_res: false,
        };
        warn!("alloc tid: {}", tid);
        if alloc_user_res {
            task_user_res.alloc_user_res();
        }
        task_user_res
    }

    pub fn alloc_user_res(&mut self) {
        if self.owns_user_res {
            return;
        }
        let ustack_bottom = ustack_bottom_from_tid(self.ustack_base, self.tid);
        let ustack_top = ustack_bottom + USER_STACK_SIZE;
        let trap_cx_bottom = trap_cx_bottom_from_tid(self.tid);
        let trap_cx_top = trap_cx_bottom + PAGE_SIZE;
        let trap_cx_start_vpn = VirtAddr::from(trap_cx_bottom).floor();
        let trap_cx_end_vpn = VirtAddr::from(trap_cx_top).ceil();
        let trap_cx_required_pages = trap_cx_end_vpn.0.saturating_sub(trap_cx_start_vpn.0);
        let mut trap_cx_frames = BTreeMap::new();
        for vpn in VPNRange::new(trap_cx_start_vpn, trap_cx_end_vpn) {
            let Some(frame) = frame_alloc() else {
                print_user_res_alloc_failure(
                    "trap_cx",
                    self.tid,
                    self.global_tid,
                    trap_cx_bottom,
                    trap_cx_top,
                    vpn,
                    trap_cx_frames.len(),
                    trap_cx_required_pages,
                );
                panic!("failed to allocate trap context frame");
            };
            trap_cx_frames.insert(vpn, Arc::new(frame));
        }

        let process = self.process.upgrade().unwrap();
        let mut process_inner = process.inner_exclusive_access();
        // alloc user stack
        warn!("ustack {:#x}..{:#x}", ustack_bottom, ustack_top);
        process_inner.vm_set.insert_framed_area(
            ustack_bottom.into(),
            ustack_top.into(),
            MapPermission::R | MapPermission::W | MapPermission::U | MapPermission::X,
            UserMapAreaType::Stack,
            None,
        );
        // error!("alloc user stack: {:#x} - {:#x}", ustack_bottom, ustack_top);

        // alloc trap_cx
        // // // alloc trap_cx

        process_inner.vm_set.insert_framed_area_with_frames(
            trap_cx_bottom.into(),
            trap_cx_top.into(),
            MapPermission::R | MapPermission::W,
            UserMapAreaType::TrapContext,
            trap_cx_frames,
        );
        self.owns_user_res = true;
        // error!("alloc trap_cx: {:#x} - {:#x}", trap_cx_bottom, trap_cx_top);
    }

    fn dealloc_user_res(&mut self) {
        let process = self.process.upgrade().unwrap();
        let mut process_inner = process.inner_exclusive_access();
        // dealloc tid
        process_inner.dealloc_tid(self.tid);
        if self.owns_user_res {
            // dealloc ustack manually
            let ustack_bottom_va: VirtAddr =
                ustack_bottom_from_tid(self.ustack_base, self.tid).into();
            process_inner
                .vm_set
                .remove_area_with_start_vpn(ustack_bottom_va.into());
            // dealloc trap_cx manually
            let trap_cx_bottom_va: VirtAddr = trap_cx_bottom_from_tid(self.tid).into();
            process_inner
                .vm_set
                .remove_area_with_start_vpn(trap_cx_bottom_va.into());
            self.owns_user_res = false;
        }
    }

    #[allow(unused)]
    pub fn alloc_tid(&mut self) {
        self.tid = self
            .process
            .upgrade()
            .unwrap()
            .inner_exclusive_access()
            .alloc_tid();
    }

    pub fn dealloc_tid(&self) {
        let process = self.process.upgrade().unwrap();
        let mut process_inner = process.inner_exclusive_access();
        process_inner.dealloc_tid(self.tid);
    }

    pub fn rebind_user_res(&mut self, ustack_base: usize) {
        self.ustack_base = ustack_base;
        self.owns_user_res = false;
    }

    pub fn trap_cx_user_va(&self) -> usize {
        trap_cx_bottom_from_tid(self.tid)
    }

    pub fn trap_cx_ppn(&self) -> &'static mut TrapFrame {
        let process = self.process.upgrade().unwrap();
        let task = {
            let process_inner = process.inner_exclusive_access();
            process_inner.tasks[self.tid].as_ref().unwrap().clone()
        };
        let ret = task.inner_exclusive_access().get_trap_cx();
        drop(task);
        ret
        // let trap_cx_bottom_va: VirtAddr = trap_cx_bottom_from_tid(self.tid).into();
        // process_inner
        //     .vm_set
        //     .translate(trap_cx_bottom_va.into())
        //     .unwrap()
        //     .ppn()
    }

    pub fn ustack_base(&self) -> usize {
        self.ustack_base
    }
    pub fn ustack_top(&self) -> usize {
        ustack_bottom_from_tid(self.ustack_base, self.tid) + USER_STACK_SIZE
    }
}

impl Drop for TaskUserRes {
    fn drop(&mut self) {
        if let Some(process) = self.process.upgrade() {
            let mut process_inner = process.inner_exclusive_access();
            process_inner.dealloc_tid(self.tid);
            if self.owns_user_res {
                let ustack_bottom_va: VirtAddr =
                    ustack_bottom_from_tid(self.ustack_base, self.tid).into();
                process_inner
                    .vm_set
                    .remove_area_with_start_vpn(ustack_bottom_va.into());
                let trap_cx_bottom_va: VirtAddr = trap_cx_bottom_from_tid(self.tid).into();
                process_inner
                    .vm_set
                    .remove_area_with_start_vpn(trap_cx_bottom_va.into());
            }
        }
    }
}
