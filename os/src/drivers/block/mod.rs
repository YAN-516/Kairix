#[cfg(all(target_arch = "loongarch64", board = "2k1000"))]
pub mod ahci;
pub mod partition;
#[cfg(target_arch = "loongarch64")]
#[allow(dead_code)]
pub mod pci;
#[cfg(all(target_arch = "loongarch64", not(board = "2k1000")))]
#[allow(dead_code)]
mod probe;
pub mod ramdisk;
#[cfg(board = "visionfive2")]
pub mod vf2_sd;
#[allow(dead_code)]
pub mod virtio_blk;
#[cfg(not(board = "2k1000"))]
use crate::board::BlockDeviceImpl;
use crate::devices::BlockDevice;
use crate::error::SysResult;
use crate::sync::SleepLock;
#[cfg(all(target_arch = "loongarch64", board = "2k1000"))]
pub use ahci::AhciBlock;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use lazy_static::*;
pub use ramdisk::RamDisk;
#[cfg(board = "visionfive2")]
pub use vf2_sd::Vf2SdBlock;
#[cfg(not(board = "2k1000"))]
pub use virtio_blk::VirtIOBlock;

const BLOCK_READ_COALESCE_MIN_BYTES: usize = 16 * 1024;
const BLOCK_READ_COALESCE_BYTES: usize = 64 * 1024;
const BLOCK_READ_COALESCE_TRIGGER_BYTES: usize = 4 * 1024;

struct BlockReadCoalesceState {
    data: [u8; BLOCK_READ_COALESCE_BYTES],
    start_block: usize,
    valid_blocks: usize,
    last_read_start: usize,
    last_read_end: usize,
    target_bytes: usize,
    cache_hit_blocks: usize,
}

impl BlockReadCoalesceState {
    const fn new() -> Self {
        Self {
            data: [0; BLOCK_READ_COALESCE_BYTES],
            start_block: 0,
            valid_blocks: 0,
            last_read_start: usize::MAX,
            last_read_end: usize::MAX,
            target_bytes: BLOCK_READ_COALESCE_MIN_BYTES,
            cache_hit_blocks: 0,
        }
    }

    fn invalidate(&mut self) {
        self.valid_blocks = 0;
        self.last_read_start = usize::MAX;
        self.last_read_end = usize::MAX;
        self.target_bytes = BLOCK_READ_COALESCE_MIN_BYTES;
        self.cache_hit_blocks = 0;
    }

    fn contains(&self, start_block: usize, blocks: usize) -> bool {
        let Some(end_block) = start_block.checked_add(blocks) else {
            return false;
        };
        let Some(cache_end) = self.start_block.checked_add(self.valid_blocks) else {
            return false;
        };
        self.valid_blocks != 0 && start_block >= self.start_block && end_block <= cache_end
    }

    /// Adapt the next read-ahead window from the amount of the previous
    /// window that demand reads actually consumed. Reaching at least 1/4 of a
    /// window grows it; consuming less than 1/8 shrinks it. This retains large
    /// sequential transfers without forcing every two adjacent metadata reads
    /// to fetch 64 KiB.
    fn adapt_target_bytes(&mut self) -> usize {
        if self.valid_blocks != 0 {
            if self.cache_hit_blocks.saturating_mul(4) >= self.valid_blocks {
                self.target_bytes = self
                    .target_bytes
                    .saturating_mul(2)
                    .min(BLOCK_READ_COALESCE_BYTES);
            } else if self.cache_hit_blocks.saturating_mul(8) < self.valid_blocks {
                self.target_bytes = (self.target_bytes / 2).max(BLOCK_READ_COALESCE_MIN_BYTES);
            }
        }
        self.cache_hit_blocks = 0;
        self.target_bytes
    }
}

static BLOCK_READ_COALESCE: SleepLock<BlockReadCoalesceState> =
    SleepLock::new(BlockReadCoalesceState::new());

static BLOCK_LOGICAL_READS: AtomicUsize = AtomicUsize::new(0);
static BLOCK_LOGICAL_READ_SECTORS: AtomicUsize = AtomicUsize::new(0);
static BLOCK_BACKEND_READS: AtomicUsize = AtomicUsize::new(0);
static BLOCK_BACKEND_READ_SECTORS: AtomicUsize = AtomicUsize::new(0);
static BLOCK_COALESCED_READS: AtomicUsize = AtomicUsize::new(0);
static BLOCK_COALESCED_READ_SECTORS: AtomicUsize = AtomicUsize::new(0);
static BLOCK_READ_CACHE_HITS: AtomicUsize = AtomicUsize::new(0);
static BLOCK_READ_CACHE_HIT_SECTORS: AtomicUsize = AtomicUsize::new(0);
static BLOCK_WRITE_INVALIDATIONS: AtomicUsize = AtomicUsize::new(0);

/// Cumulative evidence for adaptive small-read coalescing at the common block
/// device boundary. `backend_*` counts calls that actually reached hardware;
/// cache hits remain visible through the separate logical counters.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct BlockReadCoalesceStats {
    pub logical_reads: usize,
    pub logical_read_sectors: usize,
    pub backend_reads: usize,
    pub backend_read_sectors: usize,
    pub coalesced_reads: usize,
    pub coalesced_read_sectors: usize,
    pub cache_hits: usize,
    pub cache_hit_sectors: usize,
    pub write_invalidations: usize,
}

pub fn block_read_coalesce_stats() -> BlockReadCoalesceStats {
    BlockReadCoalesceStats {
        logical_reads: BLOCK_LOGICAL_READS.load(Ordering::Relaxed),
        logical_read_sectors: BLOCK_LOGICAL_READ_SECTORS.load(Ordering::Relaxed),
        backend_reads: BLOCK_BACKEND_READS.load(Ordering::Relaxed),
        backend_read_sectors: BLOCK_BACKEND_READ_SECTORS.load(Ordering::Relaxed),
        coalesced_reads: BLOCK_COALESCED_READS.load(Ordering::Relaxed),
        coalesced_read_sectors: BLOCK_COALESCED_READ_SECTORS.load(Ordering::Relaxed),
        cache_hits: BLOCK_READ_CACHE_HITS.load(Ordering::Relaxed),
        cache_hit_sectors: BLOCK_READ_CACHE_HIT_SECTORS.load(Ordering::Relaxed),
        write_invalidations: BLOCK_WRITE_INVALIDATIONS.load(Ordering::Relaxed),
    }
}
// #[cfg(target_arch = "riscv64")]
struct BlockDeviceSlot {
    backend: crate::sync::SpinNoIrqLock<Option<Arc<dyn BlockDevice>>>,
}

impl BlockDeviceSlot {
    #[cfg(not(board = "2k1000"))]
    fn backend(&self) -> Arc<dyn BlockDevice> {
        let mut backend = self.backend.lock();
        if backend.is_none() {
            *backend = Some(Arc::new(BlockDeviceImpl::new()));
        }
        backend.as_ref().unwrap().clone()
    }

    #[cfg(board = "2k1000")]
    fn backend(&self) -> Arc<dyn BlockDevice> {
        self.backend
            .lock()
            .as_ref()
            .unwrap_or_else(|| panic!("2K1000 block device used before SATA/initrd registration"))
            .clone()
    }
}

impl BlockDevice for BlockDeviceSlot {
    fn size(&self) -> u64 {
        self.backend().size()
    }

    fn block_size(&self) -> usize {
        self.backend().block_size()
    }

    fn read_block(&self, block_id: usize, buf: &mut [u8]) {
        let backend = self.backend();
        let block_size = backend.block_size();
        assert_ne!(buf.len(), 0);
        assert_ne!(block_size, 0);
        assert_eq!(buf.len() % block_size, 0);
        let requested_blocks = buf.len() / block_size;
        let requested_end = block_id
            .checked_add(requested_blocks)
            .expect("block read range overflow");
        BLOCK_LOGICAL_READS.fetch_add(1, Ordering::Relaxed);
        BLOCK_LOGICAL_READ_SECTORS.fetch_add(requested_blocks, Ordering::Relaxed);

        let mut state = BLOCK_READ_COALESCE.lock();
        if state.contains(block_id, requested_blocks) {
            let offset = (block_id - state.start_block) * block_size;
            buf.copy_from_slice(&state.data[offset..offset + buf.len()]);
            state.last_read_start = block_id;
            state.last_read_end = requested_end;
            state.cache_hit_blocks = state.cache_hit_blocks.saturating_add(requested_blocks);
            BLOCK_READ_CACHE_HITS.fetch_add(1, Ordering::Relaxed);
            BLOCK_READ_CACHE_HIT_SECTORS.fetch_add(requested_blocks, Ordering::Relaxed);
            return;
        }

        let related_to_previous =
            block_id == state.last_read_end || block_id == state.last_read_start;
        let can_coalesce = buf.len() <= BLOCK_READ_COALESCE_TRIGGER_BYTES
            && related_to_previous
            && block_size <= BLOCK_READ_COALESCE_BYTES
            && BLOCK_READ_COALESCE_BYTES % block_size == 0;
        state.last_read_start = block_id;
        state.last_read_end = requested_end;

        if can_coalesce {
            let alignment_blocks = (4096 / block_size).max(1);
            let window_start = block_id / alignment_blocks * alignment_blocks;
            let capacity_blocks = (backend.size() as usize) / block_size;
            let target_bytes = state.adapt_target_bytes();
            let window_blocks =
                (target_bytes / block_size).min(capacity_blocks.saturating_sub(window_start));
            if window_blocks >= requested_end.saturating_sub(window_start) {
                let window_bytes = window_blocks * block_size;
                backend.read_block(window_start, &mut state.data[..window_bytes]);
                state.start_block = window_start;
                state.valid_blocks = window_blocks;
                let offset = (block_id - window_start) * block_size;
                buf.copy_from_slice(&state.data[offset..offset + buf.len()]);
                BLOCK_BACKEND_READS.fetch_add(1, Ordering::Relaxed);
                BLOCK_BACKEND_READ_SECTORS.fetch_add(window_blocks, Ordering::Relaxed);
                BLOCK_COALESCED_READS.fetch_add(1, Ordering::Relaxed);
                BLOCK_COALESCED_READ_SECTORS.fetch_add(window_blocks, Ordering::Relaxed);
                return;
            }
        }

        drop(state);
        backend.read_block(block_id, buf);
        BLOCK_BACKEND_READS.fetch_add(1, Ordering::Relaxed);
        BLOCK_BACKEND_READ_SECTORS.fetch_add(requested_blocks, Ordering::Relaxed);
    }

    fn write_block(&self, block_id: usize, buf: &[u8]) {
        let backend = self.backend();
        // Keep the cache lock through the synchronous write. A reader must not
        // refill or consume the old window between invalidation and device
        // completion.
        let mut state = BLOCK_READ_COALESCE.lock();
        if state.valid_blocks != 0 {
            BLOCK_WRITE_INVALIDATIONS.fetch_add(1, Ordering::Relaxed);
        }
        state.invalidate();
        backend.write_block(block_id, buf)
    }

    fn flush(&self) -> SysResult<()> {
        self.backend().flush()
    }
}

lazy_static! {
    static ref BLOCK_DEVICE_SLOT: Arc<BlockDeviceSlot> = Arc::new(BlockDeviceSlot {
        backend: crate::sync::SpinNoIrqLock::new(None),
    });
    pub static ref BLOCK_DEVICE: Arc<dyn BlockDevice> = BLOCK_DEVICE_SLOT.clone();
}

#[allow(unused)]
pub fn set_block_device(device: Arc<dyn BlockDevice>) {
    BLOCK_READ_COALESCE.lock().invalidate();
    *BLOCK_DEVICE_SLOT.backend.lock() = Some(device);
}

#[allow(unused)]
pub fn block_device_test() {
    let block_device = BLOCK_DEVICE.clone();
    let mut write_buffer = [0u8; 512];
    let mut read_buffer = [0u8; 512];
    for i in 0..512 {
        for byte in write_buffer.iter_mut() {
            *byte = i as u8;
        }
        block_device.write_block(i as usize, &write_buffer);
        block_device.read_block(i as usize, &mut read_buffer);
        assert_eq!(write_buffer, read_buffer);
    }
    polyhal::println!("block device test passed!");
}
