use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use lazy_static::lazy_static;
use log::{debug, warn};

use crate::fs::page::pagecache::is_disk_backed_cache_id;
use crate::fs::vfs::file::File;
use crate::sync::SpinNoIrqLock;

/// Number of dirty pages to write back at a syscall return point.
///
/// Keep this small: loop-backed mkfs can dirty thousands of pages, and charging
/// all of that work to the next path lookup makes tests look like path
/// resolution is stuck.
pub const DEFAULT_WRITEBACK_BUDGET: usize = 8;

/// Shared file object stored in the deferred write-back queue.
pub type FileRef = Arc<dyn File + Send + Sync>;

lazy_static! {
    static ref WRITEBACK_QUEUE: SpinNoIrqLock<VecDeque<FileRef>> =
        SpinNoIrqLock::new(VecDeque::new());
}

static WRITEBACK_REQUESTED: AtomicBool = AtomicBool::new(false);
static WRITEBACK_DRAIN_SEQ: AtomicUsize = AtomicUsize::new(0);
static DEFERRED_WRITEBACK_DEADLINE_NS: AtomicUsize = AtomicUsize::new(0);

/// Periodic write-back cadence for ordinary buffered writes.
///
/// Linux's default dirty write-back interval is five seconds.  Keeping the
/// same cadence lets short-lived compiler files accumulate into useful
/// batches instead of turning every close syscall into synchronous block I/O.
const DEFERRED_WRITEBACK_INTERVAL_NS: usize = 5_000_000_000;

fn monotonic_now_ns() -> usize {
    polyhal::timer::current_time().as_nanos() as usize
}

/// Mark that a small amount of queued write-back should run soon.
pub fn request_writeback() {
    DEFERRED_WRITEBACK_DEADLINE_NS.store(0, Ordering::Release);
    WRITEBACK_REQUESTED.store(true, Ordering::Release);
}

/// Consume the pending write-back request flag.
pub fn take_writeback_request() -> bool {
    WRITEBACK_REQUESTED.swap(false, Ordering::AcqRel)
}

/// Arm periodic write-back without forcing the current syscall to perform it.
fn arm_deferred_writeback() {
    if WRITEBACK_REQUESTED.load(Ordering::Acquire) {
        return;
    }
    let deadline = monotonic_now_ns().saturating_add(DEFERRED_WRITEBACK_INTERVAL_NS);
    let _ = DEFERRED_WRITEBACK_DEADLINE_NS.compare_exchange(
        0,
        deadline,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
}

/// Request one periodic batch when the oldest deferred work reaches its
/// deadline.  This is lock-free because it runs from timer maintenance.
pub fn poll_deferred_writeback() {
    let deadline = DEFERRED_WRITEBACK_DEADLINE_NS.load(Ordering::Acquire);
    if deadline == 0 || monotonic_now_ns() < deadline {
        return;
    }
    if DEFERRED_WRITEBACK_DEADLINE_NS
        .compare_exchange(deadline, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        WRITEBACK_REQUESTED.store(true, Ordering::Release);
    }
}

/// Re-arm periodic write-back when a bounded batch leaves queued work behind.
fn defer_pending_writeback() {
    if !WRITEBACK_QUEUE.lock().is_empty() {
        arm_deferred_writeback();
    }
}

/// Return whether there is any queued or requested write-back work.
pub fn has_pending_writeback() -> bool {
    if WRITEBACK_REQUESTED.load(Ordering::Relaxed) {
        return true;
    }
    !WRITEBACK_QUEUE.lock().is_empty()
}

/// Return queued/requested write-back state without waiting for the queue lock.
pub fn try_has_pending_writeback() -> Option<bool> {
    if WRITEBACK_REQUESTED.load(Ordering::Relaxed) {
        return Some(true);
    }
    WRITEBACK_QUEUE.try_lock().map(|queue| !queue.is_empty())
}

/// Return the number of files waiting in the deferred write-back queue.
pub fn pending_count() -> usize {
    WRITEBACK_QUEUE.lock().len()
}

/// Try to return the deferred write-back queue length without blocking.
pub fn try_pending_count() -> Option<usize> {
    WRITEBACK_QUEUE.try_lock().map(|queue| queue.len())
}

/// Queue a writable regular file for deferred write-back.
fn queue_file_inner(file: FileRef, request: bool) -> bool {
    if file.is_pipe() || file.is_socket() || !file.writable() {
        return false;
    }
    let Some(cache_inode_id) = file.cache_inode_id() else {
        return false;
    };
    if !is_disk_backed_cache_id(cache_inode_id) {
        return false;
    }
    let has_private_state = file.has_private_writeback_state();
    let mut queue = WRITEBACK_QUEUE.lock();
    if queue.iter().any(|queued| {
        if Arc::ptr_eq(queued, &file) {
            return true;
        }
        if has_private_state || queued.has_private_writeback_state() {
            return false;
        }
        queued.cache_inode_id() == Some(cache_inode_id)
    }) {
        drop(queue);
        if request {
            request_writeback();
        }
        return true;
    }
    queue.push_back(file);
    drop(queue);
    if request {
        request_writeback();
    }
    true
}

/// Queue a writable regular file and request write-back soon.
pub fn queue_file(file: FileRef) {
    let _ = queue_file_inner(file, true);
}

/// Queue a writable regular file without immediately requesting write-back.
///
/// This is useful for loop-device backing files: many small block writes should
/// be coalesced, then drained on cache pressure or explicit sync/umount.
pub fn queue_file_lazy(file: FileRef) {
    if queue_file_inner(file, false) {
        arm_deferred_writeback();
    }
}

/// Drop queued write-back work for an inode when the queued file object has no
/// other references.
///
/// This is used by unlink: once the last directory entry is removed, dirty data
/// belonging only to an already-closed file no longer needs to be written back.
/// Files that are still referenced by an fd stay queued so open-unlinked file
/// semantics remain intact.
pub fn discard_closed_inode(cache_inode_id: usize) -> (usize, usize) {
    let mut removed = 0usize;
    let mut kept = 0usize;
    let mut queue = WRITEBACK_QUEUE.lock();
    let len = queue.len();
    for _ in 0..len {
        let Some(file) = queue.pop_front() else {
            break;
        };
        if file.cache_inode_id() == Some(cache_inode_id) {
            if Arc::strong_count(&file) == 1 {
                removed += 1;
                continue;
            }
            kept += 1;
        }
        queue.push_back(file);
    }
    (removed, kept)
}

/// Synchronously flush queued state for one cache inode.
///
/// This is used only when a cache miss observes that the VFS inode size is
/// ahead of the on-disk inode size. It avoids a global sync while ensuring the
/// demand read cannot publish a zero-filled page before the responsible dirty
/// file object has had a chance to write the inode.
pub fn flush_inode_now(cache_inode_id: usize) -> usize {
    let mut flushed_files = 0usize;
    loop {
        let file = {
            let mut queue = WRITEBACK_QUEUE.lock();
            let position = queue
                .iter()
                .position(|file| file.cache_inode_id() == Some(cache_inode_id));
            position.and_then(|position| queue.remove(position))
        };
        let Some(file) = file else {
            break;
        };
        let (_, has_more) = file.flush_pages(usize::MAX);
        flushed_files += 1;
        if has_more {
            let _ = queue_file_inner(file, true);
            break;
        }
    }
    flushed_files
}

/// Flush up to `page_budget` dirty pages from queued files.
pub fn drain_some(page_budget: usize) -> usize {
    let seq = WRITEBACK_DRAIN_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let queued_before = pending_count();
    warn!(
        "[IOZONE_HANG writeback_drain_enter] seq={} budget={} queued_before={}",
        seq, page_budget, queued_before
    );
    let mut flushed = 0;
    while flushed < page_budget {
        let file = {
            let mut queue = WRITEBACK_QUEUE.lock();
            queue.pop_front()
        };
        let Some(file) = file else {
            break;
        };
        let remaining = page_budget - flushed;
        let cache_inode_id = file.cache_inode_id();
        let path = file.get_dentry().path();
        warn!(
            "[IOZONE_HANG writeback_flush_enter] seq={} remaining_budget={} inode={:?} path={}",
            seq, remaining, cache_inode_id, path
        );
        let (flushed_pages, has_more) = file.flush_pages(remaining);
        warn!(
            "[IOZONE_HANG writeback_flush_done] seq={} flushed_pages={} has_more={} inode={:?} path={}",
            seq, flushed_pages, has_more, cache_inode_id, path
        );
        flushed += flushed_pages;
        if has_more {
            let mut queue = WRITEBACK_QUEUE.lock();
            if !queue.iter().any(|queued| Arc::ptr_eq(queued, &file)) {
                queue.push_back(file);
            }
            break;
        }
        if flushed_pages == 0 {
            continue;
        }
    }
    crate::mm::reclaim::trim_clean_page_cache_to_limit();
    warn!(
        "[IOZONE_HANG writeback_drain_done] seq={} flushed={} queued_after={}",
        seq,
        flushed,
        pending_count()
    );
    defer_pending_writeback();
    flushed
}

/// Flush all queued files.
pub fn drain_all() -> usize {
    let mut flushed = 0;
    debug!("[writeback] drain_all begin queued={}", pending_count());
    loop {
        let file = {
            let mut queue = WRITEBACK_QUEUE.lock();
            queue.pop_front()
        };
        let Some(file) = file else {
            break;
        };
        let cache_inode_id = file.cache_inode_id();
        let path = file.get_dentry().path();
        debug!(
            "[writeback] drain_all flushing index={} inode={:?} path={}",
            flushed, cache_inode_id, path
        );
        file.flush();
        debug!(
            "[writeback] drain_all flushed index={} inode={:?} path={}",
            flushed, cache_inode_id, path
        );
        flushed += 1;
    }
    crate::mm::reclaim::trim_clean_page_cache_to_limit();
    debug!("[writeback] drain_all end flushed={}", flushed);
    flushed
}
