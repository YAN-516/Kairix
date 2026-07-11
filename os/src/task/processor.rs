use super::task_entry;
use super::{ProcessControlBlock, TaskControlBlock};
use super::{TaskStatus, fetch_task};
use crate::config::MAX_CPU_NUM;
#[cfg(target_arch = "riscv64")]
use crate::sbi::*;
use crate::set_init_completed;
use crate::sync::SpinNoIrqLock;
use crate::task::check_timers;
use crate::wait_for_init;
use alloc::sync::Arc;
#[cfg(target_arch = "loongarch64")]
use core::arch::asm;
use core::sync::atomic::{AtomicUsize, Ordering};
use lazy_static::*;
use log::{debug, error, info, warn};
use polyhal::VirtAddr;
use polyhal::consts::KERNEL_STACK_SIZE;
use polyhal::kcontext::{KContext, context_switch};
use polyhal_trap::trapframe::{TrapFrame, TrapFrameArgs};

#[cfg(target_arch = "loongarch64")]
use crate::sbi_la::*;

pub struct Processor {
    current: Option<Arc<TaskControlBlock>>,
    idle_task_cx: KContext,
}

#[derive(Debug, Clone, Copy)]
pub struct ProcessorTaskStats {
    pub current_tasks: usize,
    pub locked_processors: usize,
}
impl Processor {
    pub fn new() -> Self {
        Self {
            current: None,
            idle_task_cx: KContext::blank(),
        }
    }
    fn get_idle_task_cx_ptr(&mut self) -> *mut KContext {
        &mut self.idle_task_cx as *mut _
    }
    pub fn take_current(&mut self) -> Option<Arc<TaskControlBlock>> {
        self.current.take()
    }
    pub fn current(&self) -> Option<Arc<TaskControlBlock>> {
        self.current.as_ref().map(Arc::clone)
    }
}

pub static mut PROCESSORS: [Option<SpinNoIrqLock<Processor>>; MAX_CPU_NUM] =
    [const { None }; MAX_CPU_NUM];
#[cfg(target_arch = "loongarch64")]
static LA64_SCHED_DEBUG_COUNT: AtomicUsize = AtomicUsize::new(0);
#[cfg(target_arch = "loongarch64")]
static LA64_PID2_SCHED_DEBUG_COUNT: AtomicUsize = AtomicUsize::new(0);
#[cfg(target_arch = "loongarch64")]
static LA64_SKIP_DEBUG_COUNT: AtomicUsize = AtomicUsize::new(0);

static IDLE_SPINS: [AtomicUsize; MAX_CPU_NUM] = [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
pub fn init_processors() {
    unsafe {
        for i in 0..MAX_CPU_NUM {
            PROCESSORS[i] = Some(SpinNoIrqLock::new(Processor::new()));
        }
    }
}

pub(crate) fn processor_task_stats() -> ProcessorTaskStats {
    let mut current_tasks = 0usize;
    let mut locked_processors = 0usize;
    unsafe {
        for cpu in 0..MAX_CPU_NUM {
            if let Some(processor) = PROCESSORS[cpu].as_ref() {
                if let Some(processor) = processor.try_lock() {
                    if processor.current.is_some() {
                        current_tasks += 1;
                    }
                } else {
                    locked_processors += 1;
                }
            }
        }
    }
    ProcessorTaskStats {
        current_tasks,
        locked_processors,
    }
}
#[allow(missing_docs)]
pub fn run_tasks() {
    let id: usize = get_tp();
    //println!("cpu {} run tasks", id);
    if id == 0 {
        set_init_completed();
        // loop{}
    }
    loop {
        crate::task::reap_deferred_exited_tasks();
        check_timers();
        crate::net::poll_rx_all();
        unsafe {
            if let Some(task) = fetch_task(id) {
                IDLE_SPINS[id].store(0, Ordering::Relaxed);
                // Clone the task before moving ownership
                //println!("cpu {} enter fetch task", id);
                let task_clone = Arc::clone(&task);
                let should_skip = {
                    let task_inner = task.inner_exclusive_access();
                    task_inner.task_status == TaskStatus::Zombie
                };
                if should_skip {
                    let mut processor = PROCESSORS[id].as_mut().unwrap().lock();
                    processor.current = None;
                    continue;
                }
                //println!("cpu {} get processor", id);
                let mut processor = PROCESSORS[id].as_mut().unwrap().lock();
                //println!("cpu {} get processor success", id);
                let mut task_inner = task.inner_exclusive_access();
                if task_inner.task_status == TaskStatus::Zombie {
                    drop(task_inner);
                    processor.current = None;
                    continue;
                }
                if task_inner.task_status != TaskStatus::Ready {
                    #[cfg(target_arch = "loongarch64")]
                    {
                        let pid = task
                            .process
                            .upgrade()
                            .map(|process| process.getpid())
                            .unwrap_or(usize::MAX);
                        if (pid == 1 || pid == 2 || pid == 3)
                            && LA64_SKIP_DEBUG_COUNT.fetch_add(1, Ordering::Relaxed) < 64
                        {
                            warn!(
                                "[la64 sched] skip non-ready: cpu={} pid={} tid={} status={:?} ready_queued={} on_cpu={}",
                                id,
                                pid,
                                task_inner.global_tid,
                                task_inner.task_status,
                                task.is_ready_queued(),
                                task.is_on_cpu(),
                            );
                        }
                    }
                    drop(task_inner);
                    processor.current = None;
                    continue;
                }
                if !task.try_mark_on_cpu(id) {
                    drop(task_inner);
                    processor.current = None;
                    drop(processor);
                    crate::task::add_task_to_cpu(task, id);
                    continue;
                }
                let idle_task_cx_ptr = processor.get_idle_task_cx_ptr();
                // access coming task TCB exclusively
                let next_task_cx_ptr = &task_inner.task_cx as *const KContext;
                task_inner.task_status = TaskStatus::Running;
                #[cfg(target_arch = "loongarch64")]
                {
                    let n = LA64_SCHED_DEBUG_COUNT.fetch_add(1, Ordering::Relaxed);
                    let pid = task
                        .process
                        .upgrade()
                        .map(|process| process.getpid())
                        .unwrap_or(usize::MAX);
                    if pid == 2 && LA64_PID2_SCHED_DEBUG_COUNT.fetch_add(1, Ordering::Relaxed) < 4 {
                        warn!(
                            "[la64 sched] cpu={} switch#{} pid={} tid={} status=Running era={:#x} sp={:#x} ret={:#x}",
                            id,
                            n,
                            pid,
                            task_inner.global_tid,
                            task_inner.trap_cx.era,
                            task_inner.trap_cx[TrapFrameArgs::SP],
                            task_inner.trap_cx[TrapFrameArgs::RET],
                        );
                    }
                }
                //println!("pid:{}", task.process.upgrade().unwrap().getpid());
                drop(task_inner);
                // release coming task TCB manually
                processor.current = Some(task);
                // release processor manually
                drop(processor);
                // Use the cloned task instead of calling current_task() to avoid extra lock acquisition

                let process = match task_clone.process.upgrade() {
                    Some(p) => p,
                    None => {
                        // PCB has been freed (e.g. process killed by signal and reaped by waitpid),
                        // but this orphan task is still in the ready queue. Drop it and continue.
                        let mut processor = PROCESSORS[id].as_mut().unwrap().lock();
                        processor.current = None;
                        task_clone.clear_on_cpu();
                        continue;
                    }
                };

                process.activate_user_page_table();

                if let Some(process) = current_task().unwrap().process.upgrade() {
                    debug!("cpu {} switch to task {}", id, process.getpid());
                }

                context_switch(idle_task_cx_ptr, next_task_cx_ptr);
                task_clone.clear_on_cpu();
                let requeue_after_switch = {
                    let mut task_inner = task_clone.inner_exclusive_access();
                    let requeue = task_inner.requeue_after_switch;
                    if requeue {
                        task_inner.requeue_after_switch = false;
                        task_inner.pending_wakeup = false;
                        if task_inner.task_status != TaskStatus::Zombie {
                            task_inner.task_status = TaskStatus::Ready;
                        }
                    }
                    requeue
                };
                if requeue_after_switch {
                    crate::task::add_task_to_cpu(task_clone, id);
                }
            } else {
                let spins = IDLE_SPINS[id].fetch_add(1, Ordering::Relaxed) + 1;
                if spins == 1 || spins == 1000 || spins % 100_000 == 0 {
                    warn!(
                        "[IOZONE_HANG sched_idle] cpu={} idle_spins={} ready_queues={:?} writeback_pending={} writeback_queued={}",
                        id,
                        spins,
                        crate::task::manager::ready_queue_lengths(),
                        crate::fs::writeback::has_pending_writeback(),
                        crate::fs::writeback::pending_count()
                    );
                }
            }
        }
    }
}
#[allow(missing_docs)]
pub fn take_current_task() -> Option<Arc<TaskControlBlock>> {
    let id: usize = get_tp();
    if id >= MAX_CPU_NUM {
        return None;
    }
    unsafe { PROCESSORS[id].as_mut()?.lock().take_current() }
}
#[allow(missing_docs)]
pub fn current_task() -> Option<Arc<TaskControlBlock>> {
    let id: usize = get_tp();
    if id >= MAX_CPU_NUM {
        return None;
    }
    unsafe { PROCESSORS[id].as_mut()?.lock().current() }
}
#[allow(missing_docs)]
pub fn set_current_task(task: Arc<TaskControlBlock>) {
    let id: usize = get_tp();
    unsafe {
        PROCESSORS[id].as_mut().unwrap().lock().current = Some(task);
    }
}
#[allow(missing_docs)]
pub fn current_process() -> Arc<ProcessControlBlock> {
    current_task().unwrap().process.upgrade().unwrap()
}
#[allow(missing_docs)]
pub fn current_user_token() -> usize {
    let task = current_task().unwrap();
    task.get_user_token()
}
#[allow(missing_docs)]
pub fn current_trap_cx() -> &'static mut TrapFrame {
    current_task()
        .unwrap()
        .inner_exclusive_access()
        .get_trap_cx()
}
#[allow(missing_docs)]
pub fn current_trap_cx_user_va() -> usize {
    current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .trap_cx_user_va()
}
#[allow(missing_docs)]
pub fn current_kstack_top() -> usize {
    current_task().unwrap().kstack.get_top()
}
#[allow(missing_docs)]
pub fn schedule(switched_task_cx_ptr: *mut KContext) {
    // Note: check_timers() is called in run_tasks() loop, so no need to call it here
    // Calling check_timers() in schedule() (which runs in interrupt context) can cause
    // deadlock when another CPU is holding the TASK_MANAGER lock
    let id: usize = get_tp();
    unsafe {
        let mut processor = PROCESSORS[id].as_mut().unwrap().lock();
        let idle_task_cx_ptr = processor.get_idle_task_cx_ptr();
        drop(processor);
        context_switch(switched_task_cx_ptr, idle_task_cx_ptr);
    }
}
