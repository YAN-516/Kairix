//! Minimal swapfile backend for reclaiming tmpfs page-cache pages.
//!
//! This is intentionally narrower than a full Linux-style swap subsystem: it
//! provides fixed-size slots backed by a preallocated file and raw direct I/O.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};

use lazy_static::lazy_static;
use log::{info, warn};
use polyhal::consts::PAGE_SIZE;

use crate::error::{SysError, SysResult};
use crate::fs::File;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::file::open_file;
use crate::fs::vfs::inode::InodeMode;
use crate::sync::SpinNoIrqLock;

const SWAP_FILE_PATH: &str = "/.kairix_swap";
const SWAP_SIZE: usize = 128 * 1024 * 1024;

/// A fixed-size page slot in the swapfile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwapSlot {
    index: usize,
}

struct SwapState {
    file: Option<Arc<dyn File>>,
    free_slots: Vec<usize>,
    total_slots: usize,
}

impl SwapState {
    fn new() -> Self {
        Self {
            file: None,
            free_slots: Vec::new(),
            total_slots: 0,
        }
    }
}

lazy_static! {
    static ref SWAP_STATE: SpinNoIrqLock<SwapState> = SpinNoIrqLock::new(SwapState::new());
}

static SWAP_ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static SWAP_FREE_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Snapshot of swap state.
#[derive(Debug, Clone, Copy)]
pub struct SwapStats {
    /// Total swap slots.
    pub total_slots: usize,
    /// Currently free swap slots.
    pub free_slots: usize,
    /// Currently used swap slots.
    pub used_slots: usize,
    /// Cumulative successful slot allocations.
    pub alloc_count: usize,
    /// Cumulative slot frees.
    pub free_count: usize,
    /// Whether swap has an initialized backing file.
    pub enabled: bool,
}

/// Initialize the swapfile after the root filesystem is mounted.
pub fn init() {
    match init_inner() {
        Ok(stats) => info!(
            "[swap] enabled file={} total_slots={} bytes={}",
            SWAP_FILE_PATH,
            stats.total_slots,
            stats.total_slots * PAGE_SIZE
        ),
        Err(err) => warn!("[swap] disabled: init failed: {:?}", err),
    }
}

fn init_inner() -> SysResult<SwapStats> {
    let root = GLOBAL_DCACHE.get("/").ok_or(SysError::ENOENT)?;
    let flags = OpenFlags::RDWR | OpenFlags::O_CREAT | OpenFlags::O_TRUNC;
    let mode = InodeMode::FILE | InodeMode::from_bits_truncate(0o600);
    let file = open_file(root, SWAP_FILE_PATH, flags, mode)?;

    let total_slots = SWAP_SIZE / PAGE_SIZE;
    let mut free_slots: Vec<usize> = (0..total_slots).collect();
    free_slots.reverse();

    let mut state = SWAP_STATE.lock();
    state.file = Some(file);
    state.free_slots = free_slots;
    state.total_slots = total_slots;
    drop(state);

    Ok(stats())
}

fn backing_file() -> Option<Arc<dyn File>> {
    SWAP_STATE.lock().file.clone()
}

/// Allocate one swap slot.
pub fn alloc_slot() -> Option<SwapSlot> {
    let mut state = SWAP_STATE.lock();
    let slot = state.free_slots.pop()?;
    SWAP_ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
    Some(SwapSlot { index: slot })
}

/// Return a swap slot to the free list.
pub fn free_slot(slot: SwapSlot) {
    let mut state = SWAP_STATE.lock();
    if slot.index >= state.total_slots || state.free_slots.iter().any(|idx| *idx == slot.index) {
        warn!("[swap] invalid or duplicate free slot {}", slot.index);
        return;
    }
    state.free_slots.push(slot.index);
    SWAP_FREE_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Write one page into a swap slot.
pub fn write_slot(slot: SwapSlot, page: &[u8]) -> SysResult<()> {
    if page.len() != PAGE_SIZE {
        return Err(SysError::EINVAL);
    }
    let file = backing_file().ok_or(SysError::ENODEV)?;
    let written = file.write_at_direct(slot.index * PAGE_SIZE, page)?;
    if written == PAGE_SIZE {
        Ok(())
    } else {
        Err(SysError::EIO)
    }
}

/// Read one page from a swap slot.
pub fn read_slot(slot: SwapSlot, page: &mut [u8]) -> SysResult<()> {
    if page.len() != PAGE_SIZE {
        return Err(SysError::EINVAL);
    }
    let file = backing_file().ok_or(SysError::ENODEV)?;
    let read = file.read_at_direct(slot.index * PAGE_SIZE, page)?;
    if read < PAGE_SIZE {
        page[read..].fill(0);
    }
    Ok(())
}

/// Return the current swap state.
pub fn stats() -> SwapStats {
    let state = SWAP_STATE.lock();
    let free_slots = state.free_slots.len();
    SwapStats {
        total_slots: state.total_slots,
        free_slots,
        used_slots: state.total_slots.saturating_sub(free_slots),
        alloc_count: SWAP_ALLOC_COUNT.load(Ordering::Relaxed),
        free_count: SWAP_FREE_COUNT.load(Ordering::Relaxed),
        enabled: state.file.is_some(),
    }
}
