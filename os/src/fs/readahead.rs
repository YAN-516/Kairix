use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};

use lazy_static::lazy_static;

use crate::error::SysError;
use crate::fs::vfs::file::File;
use crate::sync::SpinNoIrqLock;

const MMAP_READAHEAD_PAGES: usize = 16;
const MAX_MERGED_READAHEAD_PAGES: usize = 32;
const MAX_QUEUED_READAHEAD: usize = 128;
const MAX_RETRIES: usize = 16;

type FileRef = Arc<dyn File + Send + Sync>;

struct ReadaheadRequest {
    file: FileRef,
    inode: usize,
    start_page: usize,
    page_count: usize,
    retries: usize,
}

/// Lock-free counters are intentionally separate from the queue so scheduler
/// and stall diagnostics can inspect progress without joining filesystem lock
/// ordering. They also make runtime validation possible without per-fault logs.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct ReadaheadStats {
    /// Mapping readahead requests accepted by the bounded queue.
    pub queued: usize,
    /// Requests that finished, including already-cached windows.
    pub completed: usize,
    /// New page-cache pages loaded by background workers.
    pub pages_loaded: usize,
    /// Busy or generation-changing requests requeued for a later idle pass.
    pub retries: usize,
    /// Old requests evicted when the bounded queue was full.
    pub dropped: usize,
    /// Requests abandoned after an I/O error or retry exhaustion.
    pub failed: usize,
    /// Requests currently executing on idle CPUs.
    pub active: usize,
}

lazy_static! {
    static ref READAHEAD_QUEUE: SpinNoIrqLock<VecDeque<ReadaheadRequest>> =
        SpinNoIrqLock::new(VecDeque::new());
}

static READAHEAD_QUEUED: AtomicUsize = AtomicUsize::new(0);
static READAHEAD_COMPLETED: AtomicUsize = AtomicUsize::new(0);
static READAHEAD_PAGES_LOADED: AtomicUsize = AtomicUsize::new(0);
static READAHEAD_RETRIES: AtomicUsize = AtomicUsize::new(0);
static READAHEAD_DROPPED: AtomicUsize = AtomicUsize::new(0);
static READAHEAD_FAILED: AtomicUsize = AtomicUsize::new(0);
static READAHEAD_ACTIVE: AtomicUsize = AtomicUsize::new(0);

/// Return cumulative background mapping-readahead progress without queue locks.
pub fn readahead_stats() -> ReadaheadStats {
    ReadaheadStats {
        queued: READAHEAD_QUEUED.load(Ordering::Relaxed),
        completed: READAHEAD_COMPLETED.load(Ordering::Relaxed),
        pages_loaded: READAHEAD_PAGES_LOADED.load(Ordering::Relaxed),
        retries: READAHEAD_RETRIES.load(Ordering::Relaxed),
        dropped: READAHEAD_DROPPED.load(Ordering::Relaxed),
        failed: READAHEAD_FAILED.load(Ordering::Relaxed),
        active: READAHEAD_ACTIVE.load(Ordering::Relaxed),
    }
}

/// Queue the pages following a successful file-backed mapping fault.
///
/// Requests are best effort, bounded, and deduplicated by cache inode. A
/// truncate is handled by the filesystem generation check at execution time.
pub fn queue_mmap_readahead(file: FileRef, start_page: usize) {
    let Some(inode) = file.cache_inode_id() else {
        return;
    };
    let end_page = start_page.saturating_add(MMAP_READAHEAD_PAGES);
    let mut queue = READAHEAD_QUEUE.lock();
    if let Some(request) = queue.iter_mut().find(|request| {
        let request_end = request.start_page.saturating_add(request.page_count);
        request.inode == inode && request.start_page <= end_page && start_page <= request_end
    }) {
        let merged_start = request.start_page.min(start_page);
        let merged_end = request
            .start_page
            .saturating_add(request.page_count)
            .max(end_page);
        if merged_end.saturating_sub(merged_start) <= MAX_MERGED_READAHEAD_PAGES {
            request.start_page = merged_start;
            request.page_count = merged_end - merged_start;
            return;
        }
    }
    if queue.len() >= MAX_QUEUED_READAHEAD {
        queue.pop_front();
        READAHEAD_DROPPED.fetch_add(1, Ordering::Relaxed);
    }
    queue.push_back(ReadaheadRequest {
        file,
        inode,
        start_page,
        page_count: MMAP_READAHEAD_PAGES,
        retries: 0,
    });
    READAHEAD_QUEUED.fetch_add(1, Ordering::Relaxed);
}

struct ActiveRequest;

impl ActiveRequest {
    fn begin() -> Self {
        READAHEAD_ACTIVE.fetch_add(1, Ordering::Relaxed);
        Self
    }
}

impl Drop for ActiveRequest {
    fn drop(&mut self) {
        READAHEAD_ACTIVE.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Execute at most one queued mapping readahead request from an idle CPU.
/// Returns true when work completed. A busy request is requeued and returns
/// false so the caller follows its normal idle/WFI path instead of hot-looping.
pub fn service_one_background_request() -> bool {
    let Some(mut request) = READAHEAD_QUEUE.lock().pop_front() else {
        return false;
    };
    // Idle schedulers normally run with interrupts disabled. Admit the
    // existing lock-free timer/IPI subset while this bounded disk read polls,
    // then restore the scheduler's IRQ-off state through the RAII guard.
    let _interruptible = crate::InterruptibleKernelSection::enter();
    let _active = ActiveRequest::begin();
    match request
        .file
        .readahead_pages(request.start_page, request.page_count)
    {
        Ok(pages) => {
            READAHEAD_PAGES_LOADED.fetch_add(pages, Ordering::Relaxed);
            READAHEAD_COMPLETED.fetch_add(1, Ordering::Relaxed);
            true
        }
        Err(SysError::EAGAIN) if request.retries < MAX_RETRIES => {
            request.retries += 1;
            READAHEAD_RETRIES.fetch_add(1, Ordering::Relaxed);
            READAHEAD_QUEUE.lock().push_back(request);
            false
        }
        Err(_) => {
            READAHEAD_FAILED.fetch_add(1, Ordering::Relaxed);
            true
        }
    }
}
