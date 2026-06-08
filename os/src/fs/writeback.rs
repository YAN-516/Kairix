use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};
use lazy_static::lazy_static;
use log::debug;

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

/// Mark that a small amount of queued write-back should run soon.
pub fn request_writeback() {
    WRITEBACK_REQUESTED.store(true, Ordering::Relaxed);
}

/// Consume the pending write-back request flag.
pub fn take_writeback_request() -> bool {
    WRITEBACK_REQUESTED.swap(false, Ordering::Relaxed)
}

/// Return whether there is any queued or requested write-back work.
pub fn has_pending_writeback() -> bool {
    if WRITEBACK_REQUESTED.load(Ordering::Relaxed) {
        return true;
    }
    !WRITEBACK_QUEUE.lock().is_empty()
}

/// Return the number of files waiting in the deferred write-back queue.
pub fn pending_count() -> usize {
    WRITEBACK_QUEUE.lock().len()
}

/// Queue a writable regular file for deferred write-back.
fn queue_file_inner(file: FileRef, request: bool) {
    let Some(cache_inode_id) = file.cache_inode_id() else {
        return;
    };
    if !file.writable() || file.is_pipe() || file.is_socket() {
        return;
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
        return;
    }
    queue.push_back(file);
    drop(queue);
    if request {
        request_writeback();
    }
}

/// Queue a writable regular file and request write-back soon.
pub fn queue_file(file: FileRef) {
    queue_file_inner(file, true);
}

/// Queue a writable regular file without immediately requesting write-back.
///
/// This is useful for loop-device backing files: many small block writes should
/// be coalesced, then drained on cache pressure or explicit sync/umount.
pub fn queue_file_lazy(file: FileRef) {
    queue_file_inner(file, false);
}

/// Flush up to `page_budget` dirty pages from queued files.
pub fn drain_some(page_budget: usize) -> usize {
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
        let (flushed_pages, has_more) = file.flush_pages(remaining);
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
    if let Some(mut cache) = crate::fs::page::pagecache::PAGE_CACHE.try_lock() {
        cache.trim_clean_to_limit();
    }
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
    if let Some(mut cache) = crate::fs::page::pagecache::PAGE_CACHE.try_lock() {
        cache.trim_clean_to_limit();
    }
    debug!("[writeback] drain_all end flushed={}", flushed);
    flushed
}
