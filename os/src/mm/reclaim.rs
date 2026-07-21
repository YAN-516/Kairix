//! Lightweight kernel memory reclaim.
//!
//! The allocator may call into this module when it is out of free frames.  That
//! path must stay non-blocking: reclaim clean cache pages only and request
//! deferred write-back for dirty pages.

use core::sync::atomic::{AtomicBool, Ordering};

use log::warn;
use polyhal::consts::PAGE_SIZE;

use crate::fs::page::pagecache::{PAGE_CACHE, disk_page_cache_limit_pages};

/// Start background reclaim when free memory drops below this watermark.
pub const LOW_WATERMARK_PAGES: usize = 16 * 1024;
/// Keep pushing write-back/reclaim until free memory reaches this watermark.
pub const HIGH_WATERMARK_PAGES: usize = 32 * 1024;

const ALLOC_RECLAIM_BATCH: usize = 256;
const BACKGROUND_WRITEBACK_BUDGET: usize = 512;

static BACKGROUND_RECLAIM_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Return the current number of free physical pages.
pub fn free_pages() -> usize {
    crate::mm::get_free_memory() / PAGE_SIZE
}

/// Return whether free memory is below the point where reclaim should start.
pub fn below_low_watermark() -> bool {
    free_pages() < LOW_WATERMARK_PAGES
}

/// Return whether reclaim should continue pushing toward the high watermark.
pub fn below_high_watermark() -> bool {
    free_pages() < HIGH_WATERMARK_PAGES
}

/// Request delayed write-back/reclaim from a safe syscall-return context.
pub fn request_background_reclaim() {
    BACKGROUND_RECLAIM_REQUESTED.store(true, Ordering::Relaxed);
    crate::fs::writeback::request_writeback();
}

/// Consume the pending background reclaim request flag.
pub fn take_background_reclaim_request() -> bool {
    BACKGROUND_RECLAIM_REQUESTED.swap(false, Ordering::Relaxed)
}

/// Trim clean page-cache pages down to the configured cache limit.
pub fn trim_clean_page_cache_to_limit() -> usize {
    let Some(mut cache) = PAGE_CACHE.try_lock() else {
        return 0;
    };
    cache.trim_clean_to_limit()
}

/// Reclaim up to `target_pages` clean page-cache pages without blocking.
pub fn reclaim_clean_page_cache(target_pages: usize) -> usize {
    let Some(mut cache) = PAGE_CACHE.try_lock() else {
        return 0;
    };
    let reclaimed = cache.reclaim_clean_pages(target_pages);
    if reclaimed > 0 {
        warn!("[MEMDEBUG] reclaimed {} clean page-cache pages", reclaimed);
    }
    reclaimed
}

/// Swap out up to `target_pages` resident tmpfs page-cache pages.
pub fn swap_out_tmpfs_page_cache(target_pages: usize) -> usize {
    let Some(mut cache) = PAGE_CACHE.try_lock() else {
        return 0;
    };
    let swapped = cache.swap_out_tmpfs_pages(target_pages);
    if swapped > 0 {
        warn!("[MEMDEBUG] swapped out {} tmpfs page-cache pages", swapped);
    }
    swapped
}

/// Try to make memory available for an allocation fallback path.
pub fn try_reclaim_for_allocation(target_pages: usize) -> usize {
    let target_pages = target_pages.max(ALLOC_RECLAIM_BATCH);
    let mut reclaimed = reclaim_clean_page_cache(target_pages);
    if reclaimed < target_pages {
        reclaimed += swap_out_tmpfs_page_cache(target_pages - reclaimed);
    }
    if reclaimed == 0 {
        request_background_reclaim();
    }
    reclaimed
}

/// Poll cache and memory pressure, requesting deferred reclaim if needed.
pub fn poll_background_reclaim() {
    // This function runs from the scheduler's deferred timer-maintenance path.
    // Waiting for FRAME_ALLOCATOR there can pin an entire CPU on its idle stack
    // with interrupts disabled while runnable tasks accumulate in its queue.
    // A busy allocator only postpones this advisory sample until the next tick.
    let below_low_watermark =
        crate::mm::try_frame_stats().is_some_and(|stats| stats.free_pages < LOW_WATERMARK_PAGES);
    let should_reclaim = below_low_watermark || page_cache_needs_writeback();
    if should_reclaim {
        request_background_reclaim();
    }
}

fn page_cache_needs_writeback() -> bool {
    // PAGE_CACHE is a BlockingMutex. Its `try_lock()` still takes the mutex's
    // internal wait-queue spinlock, and dropping a successful guard takes that
    // spinlock again, so it is not safe on the scheduler stack. Use the
    // lock-free namespace counters here. Without a separate atomic dirty-page
    // count, half of the cache limit is a conservative write-back threshold;
    // an unnecessary request is harmless and is drained in task context.
    let stats = crate::fs::page::pagecache::atomic_stats();
    let disk_pages = stats.fat32_pages.saturating_add(stats.ext4_pages);
    disk_pages > disk_page_cache_limit_pages() / 2
}

/// Return the number of dirty pages to write back in one syscall-return pass.
pub fn writeback_budget() -> usize {
    if below_high_watermark() || page_cache_needs_writeback() {
        BACKGROUND_WRITEBACK_BUDGET
    } else {
        crate::fs::writeback::DEFAULT_WRITEBACK_BUDGET
    }
}
