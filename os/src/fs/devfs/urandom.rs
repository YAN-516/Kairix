#![allow(missing_docs)]
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::inode::inode_alloc;
use crate::fs::vfs::inode::{InodeInner, InodeMode, make_rdev};
use crate::fs::vfs::{DentryInner, FileInner};
use crate::fs::{Dentry, File, Inode, String};
use crate::mm::UserBuffer;
#[cfg(target_arch = "riscv64")]
use crate::sbi;
#[cfg(target_arch = "riscv64")]
use crate::sbi::get_tp;
#[cfg(target_arch = "loongarch64")]
use crate::sbi_la::get_tp;
#[cfg(target_arch = "riscv64")]
use crate::timer::get_time_us;
use alloc::sync::{Arc, Weak};
use core::sync::atomic::{AtomicU64, Ordering};
use polyhal::timer::current_time;
use spin::{Mutex, MutexGuard};

lazy_static::lazy_static! {
    static ref RNG_STATE: Mutex<RandomState> = Mutex::new(RandomState::new());
}

static SEED_SEQUENCE: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);

struct RandomState {
    key: [u32; 8],
    counter: u64,
    nonce: [u32; 2],
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut value = *state;
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] = (state[d] ^ state[a]).rotate_left(16);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_left(12);
    state[a] = state[a].wrapping_add(state[b]);
    state[d] = (state[d] ^ state[a]).rotate_left(8);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_left(7);
}

impl RandomState {
    fn new() -> Self {
        let stack_sample = &SEED_SEQUENCE as *const AtomicU64 as usize as u64;
        let realtime = crate::timer::realtime_ns();
        let mut seed = (realtime as u64)
            ^ (realtime >> 64) as u64
            ^ (current_time().as_nanos() as u64).rotate_left(17)
            ^ (get_tp() as u64).rotate_left(33)
            ^ stack_sample
            ^ SEED_SEQUENCE.fetch_add(0xd1b5_4a32_d192_ed03, Ordering::Relaxed);
        let mut key = [0u32; 8];
        for pair in key.chunks_exact_mut(2) {
            let word = splitmix64(&mut seed);
            pair[0] = word as u32;
            pair[1] = (word >> 32) as u32;
        }
        let nonce_word = splitmix64(&mut seed);
        Self {
            key,
            counter: splitmix64(&mut seed),
            nonce: [nonce_word as u32, (nonce_word >> 32) as u32],
        }
    }

    fn block(&mut self) -> [u8; 64] {
        let initial = [
            0x6170_7865,
            0x3320_646e,
            0x7962_2d32,
            0x6b20_6574,
            self.key[0],
            self.key[1],
            self.key[2],
            self.key[3],
            self.key[4],
            self.key[5],
            self.key[6],
            self.key[7],
            self.counter as u32,
            (self.counter >> 32) as u32,
            self.nonce[0],
            self.nonce[1],
        ];
        self.counter = self.counter.wrapping_add(1);
        let mut working = initial;
        for _ in 0..10 {
            quarter_round(&mut working, 0, 4, 8, 12);
            quarter_round(&mut working, 1, 5, 9, 13);
            quarter_round(&mut working, 2, 6, 10, 14);
            quarter_round(&mut working, 3, 7, 11, 15);
            quarter_round(&mut working, 0, 5, 10, 15);
            quarter_round(&mut working, 1, 6, 11, 12);
            quarter_round(&mut working, 2, 7, 8, 13);
            quarter_round(&mut working, 3, 4, 9, 14);
        }
        let mut output = [0u8; 64];
        for (index, word) in working.iter_mut().enumerate() {
            *word = word.wrapping_add(initial[index]);
            output[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        output
    }

    fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(64) {
            let block = self.block();
            chunk.copy_from_slice(&block[..chunk.len()]);
        }
        // Rekey after every request so compromise of the current state does
        // not reveal bytes returned by earlier requests.
        let rekey = self.block();
        for (index, key) in self.key.iter_mut().enumerate() {
            *key = u32::from_le_bytes(rekey[index * 4..index * 4 + 4].try_into().unwrap());
        }
        self.nonce[0] = u32::from_le_bytes(rekey[32..36].try_into().unwrap());
        self.nonce[1] = u32::from_le_bytes(rekey[36..40].try_into().unwrap());
    }
}

/// Fill a buffer from the kernel's ChaCha20-based random generator.
pub fn fill_random(buf: &mut [u8]) {
    RNG_STATE.lock().fill(buf);
}

pub struct UrandomFile {
    inner: Mutex<FileInner>,
}

impl UrandomFile {
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self {
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags: OpenFlags::empty(),
            }),
        }
    }
}

impl File for UrandomFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        false
    }

    fn read(&self, buf: UserBuffer) -> SysResult<usize> {
        let mut total = 0usize;
        for slice in buf.buffers.into_iter() {
            fill_random(slice);
            total += slice.len();
        }
        Ok(total)
    }

    fn write(&self, _buf: UserBuffer) -> SysResult<usize> {
        Ok(0)
    }
}

unsafe impl Send for UrandomDentry {}
unsafe impl Sync for UrandomDentry {}

pub struct UrandomDentry {
    inner: DentryInner,
}

impl UrandomDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<UrandomDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
        })
    }
}

impl Dentry for UrandomDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn name(&self) -> &str {
        &self.inner.name
    }

    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(UrandomFile::new(self)))
    }
}

pub struct UrandomInode {
    inner: InodeInner,
}

impl UrandomInode {
    pub fn new() -> Self {
        let mode = InodeMode::CHAR;
        Self {
            inner: InodeInner::new(inode_alloc(), 0, mode, make_rdev(1, 9) as usize),
        }
    }
}

impl Inode for UrandomInode {
    fn get_mode(&self) -> InodeMode {
        self.inner.mode
    }
    fn set_size(&self, new_size: usize) {
        self.inner.size.store(new_size, Ordering::SeqCst);
    }
    fn get_size(&self) -> usize {
        self.inner.size.load(Ordering::SeqCst)
    }
    fn get_ino(&self) -> usize {
        self.inner.ino
    }
    fn get_nlink(&self) -> usize {
        self.inner.nlink.load(Ordering::SeqCst)
    }
    fn get_rdev(&self) -> usize {
        self.inner.rdev.load(Ordering::Relaxed)
    }
    fn set_rdev(&self, rdev: usize) {
        self.inner.rdev.store(rdev, Ordering::Relaxed);
    }
    fn inc_nlink(&self) {
        self.inner.nlink.fetch_add(1, Ordering::SeqCst);
    }
    fn dec_nlink(&self) {
        self.inner.nlink.fetch_sub(1, Ordering::SeqCst);
    }

    fn get_atime(&self) -> (i64, i64) {
        (
            self.inner.atime_sec.load(Ordering::Relaxed),
            self.inner.atime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_atime(&self, sec: i64, nsec: i64) {
        self.inner.atime_sec.store(sec, Ordering::Relaxed);
        self.inner.atime_nsec.store(nsec, Ordering::Relaxed);
    }

    fn get_mtime(&self) -> (i64, i64) {
        (
            self.inner.mtime_sec.load(Ordering::Relaxed),
            self.inner.mtime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_mtime(&self, sec: i64, nsec: i64) {
        self.inner.mtime_sec.store(sec, Ordering::Relaxed);
        self.inner.mtime_nsec.store(nsec, Ordering::Relaxed);
    }

    fn get_ctime(&self) -> (i64, i64) {
        (
            self.inner.ctime_sec.load(Ordering::Relaxed),
            self.inner.ctime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_ctime(&self, sec: i64, nsec: i64) {
        self.inner.ctime_sec.store(sec, Ordering::Relaxed);
        self.inner.ctime_nsec.store(nsec, Ordering::Relaxed);
    }
}
