//! Lightweight kernel memory reclaim.
//!
//! The allocator may call into this module when it is out of free frames.  That
//! path must stay non-blocking: reclaim clean cache pages only and request
//! deferred write-back for dirty pages.

use core::sync::atomic::{AtomicBool, Ordering};

use log::warn;
use polyhal::consts::PAGE_SIZE;

use crate::fs::page::pagecache::{MAX_PAGE_CACHE_PAGES, PAGE_CACHE};

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

/// Try to make memory available for an allocation fallback path.
pub fn try_reclaim_for_allocation(target_pages: usize) -> usize {
    let target_pages = target_pages.max(ALLOC_RECLAIM_BATCH);
    let reclaimed = reclaim_clean_page_cache(target_pages);
    if reclaimed == 0 {
        request_background_reclaim();
    }
    reclaimed
}

/// Poll cache and memory pressure, requesting deferred reclaim if needed.
pub fn poll_background_reclaim() {
    let mut should_reclaim = below_low_watermark();
    if let Some(cache) = PAGE_CACHE.try_lock() {
        let dirty = cache.dirty_pages_count();
        let pages = cache.pages_count();
        if dirty > MAX_PAGE_CACHE_PAGES / 2 || pages > MAX_PAGE_CACHE_PAGES {
            should_reclaim = true;
        }
    }
    if should_reclaim {
        request_background_reclaim();
    }
}

/// Return the number of dirty pages to write back in one syscall-return pass.
pub fn writeback_budget() -> usize {
    if below_high_watermark() {
        BACKGROUND_WRITEBACK_BUDGET
    } else {
        crate::fs::writeback::DEFAULT_WRITEBACK_BUDGET
    }
}
