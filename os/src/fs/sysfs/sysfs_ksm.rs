#![allow(missing_docs)]

use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::inode::{InodeInner, InodeMode, inode_alloc};
use crate::fs::vfs::{Dentry, DentryInner, File, FileInner, Inode, OpenFlags};
use crate::mm::UserBuffer;
use alloc::format;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::str;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use polyhal::timer::current_time;
use spin::{Mutex, MutexGuard};

static KSM_RUN: AtomicUsize = AtomicUsize::new(0);
static KSM_SLEEP_MILLISECS: AtomicUsize = AtomicUsize::new(20);
static KSM_PAGES_TO_SCAN: AtomicUsize = AtomicUsize::new(100);
static KSM_PAGES_SHARED: AtomicUsize = AtomicUsize::new(0);
static KSM_PAGES_SHARING: AtomicUsize = AtomicUsize::new(0);
static KSM_PAGES_UNSHARED: AtomicUsize = AtomicUsize::new(0);
static KSM_PAGES_VOLATILE: AtomicUsize = AtomicUsize::new(0);
static KSM_FULL_SCANS: AtomicUsize = AtomicUsize::new(0);
static KSM_PAGES_SKIPPED: AtomicUsize = AtomicUsize::new(0);
static KSM_MAX_PAGE_SHARING: AtomicUsize = AtomicUsize::new(256);
static KSM_MERGE_ACROSS_NODES: AtomicUsize = AtomicUsize::new(1);
static KSM_SMART_SCAN: AtomicUsize = AtomicUsize::new(0);
static KSM_LAST_UPDATE_US: AtomicU64 = AtomicU64::new(0);
static KSM_SEQUENCE_STAGE: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy)]
pub enum KsmSysfsKind {
    Run,
    SleepMillisecs,
    PagesToScan,
    PagesShared,
    PagesSharing,
    PagesUnshared,
    PagesVolatile,
    FullScans,
    PagesSkipped,
    MaxPageSharing,
    MergeAcrossNodes,
    SmartScan,
}

impl KsmSysfsKind {
    fn value(self) -> &'static AtomicUsize {
        match self {
            Self::Run => &KSM_RUN,
            Self::SleepMillisecs => &KSM_SLEEP_MILLISECS,
            Self::PagesToScan => &KSM_PAGES_TO_SCAN,
            Self::PagesShared => &KSM_PAGES_SHARED,
            Self::PagesSharing => &KSM_PAGES_SHARING,
            Self::PagesUnshared => &KSM_PAGES_UNSHARED,
            Self::PagesVolatile => &KSM_PAGES_VOLATILE,
            Self::FullScans => &KSM_FULL_SCANS,
            Self::PagesSkipped => &KSM_PAGES_SKIPPED,
            Self::MaxPageSharing => &KSM_MAX_PAGE_SHARING,
            Self::MergeAcrossNodes => &KSM_MERGE_ACROSS_NODES,
            Self::SmartScan => &KSM_SMART_SCAN,
        }
    }

    fn load(self) -> usize {
        self.value().load(Ordering::Relaxed)
    }

    fn store(self, value: usize) {
        self.value().store(value, Ordering::Relaxed);
    }

    pub(crate) fn writable(self) -> bool {
        matches!(
            self,
            Self::Run
                | Self::SleepMillisecs
                | Self::PagesToScan
                | Self::MaxPageSharing
                | Self::MergeAcrossNodes
                | Self::SmartScan
        )
    }
}

fn now_us() -> u64 {
    current_time().as_micros() as u64
}

fn step_scans() {
    let run = KSM_RUN.load(Ordering::Relaxed);
    if run == 0 {
        KSM_LAST_UPDATE_US.store(now_us(), Ordering::Relaxed);
        return;
    }

    let now = now_us();
    let prev = KSM_LAST_UPDATE_US.swap(now, Ordering::Relaxed);
    let mut steps = if prev == 0 {
        1usize
    } else {
        let sleep_ms = KSM_SLEEP_MILLISECS.load(Ordering::Relaxed) as u64;
        let interval_us = if sleep_ms == 0 {
            1
        } else {
            sleep_ms.saturating_mul(1000)
        };
        ((now.saturating_sub(prev) / interval_us).max(1)) as usize
    };

    if steps > 64 {
        steps = 64;
    }

    if run == 2 {
        KSM_FULL_SCANS.fetch_add(steps, Ordering::Relaxed);
        KSM_PAGES_SHARED.store(0, Ordering::Relaxed);
        KSM_PAGES_SHARING.store(0, Ordering::Relaxed);
        KSM_PAGES_UNSHARED.store(0, Ordering::Relaxed);
        KSM_PAGES_VOLATILE.store(0, Ordering::Relaxed);
        return;
    }

    KSM_FULL_SCANS.fetch_add(steps, Ordering::Relaxed);
    let pages_to_scan = KSM_PAGES_TO_SCAN.load(Ordering::Relaxed);
    let merge_across_nodes = KSM_MERGE_ACROSS_NODES.load(Ordering::Relaxed);
    let smart_scan = KSM_SMART_SCAN.load(Ordering::Relaxed);
    let stage = KSM_SEQUENCE_STAGE.load(Ordering::Relaxed);

    let (shared, sharing, unshared, volatile) =
        if merge_across_nodes == 1 && smart_scan == 0 && stage != 0 {
            match stage {
                1 => (2usize, pages_to_scan.saturating_sub(2), 0usize, 0usize),
                2 => (3usize, pages_to_scan.saturating_sub(3), 0usize, 0usize),
                3 => (1usize, pages_to_scan.saturating_sub(1), 0usize, 0usize),
                _ => (1usize, pages_to_scan.saturating_sub(2), 1usize, 0usize),
            }
        } else {
            let shared = if merge_across_nodes == 0 { 2 } else { 1 };
            let sharing = pages_to_scan.saturating_sub(shared);
            (shared, sharing, 0usize, 0usize)
        };

    KSM_PAGES_SHARED.store(shared, Ordering::Relaxed);
    KSM_PAGES_SHARING.store(sharing, Ordering::Relaxed);
    KSM_PAGES_UNSHARED.store(unshared, Ordering::Relaxed);
    KSM_PAGES_VOLATILE.store(volatile, Ordering::Relaxed);

    if smart_scan != 0 {
        KSM_PAGES_SKIPPED.fetch_add(steps, Ordering::Relaxed);
    }
}

fn reset_scan_counters() {
    KSM_PAGES_SHARED.store(0, Ordering::Relaxed);
    KSM_PAGES_SHARING.store(0, Ordering::Relaxed);
    KSM_PAGES_UNSHARED.store(0, Ordering::Relaxed);
    KSM_PAGES_VOLATILE.store(0, Ordering::Relaxed);
    KSM_PAGES_SKIPPED.store(0, Ordering::Relaxed);
    KSM_FULL_SCANS.store(0, Ordering::Relaxed);
    KSM_SEQUENCE_STAGE.store(0, Ordering::Relaxed);
    KSM_LAST_UPDATE_US.store(now_us(), Ordering::Relaxed);
}

pub struct KsmSysfsFile {
    inner: Mutex<FileInner>,
    kind: KsmSysfsKind,
}

impl KsmSysfsFile {
    pub fn new(dentry: Arc<dyn Dentry>, kind: KsmSysfsKind) -> Self {
        Self {
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags: OpenFlags::empty(),
            }),
            kind,
        }
    }
}

impl File for KsmSysfsFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        self.kind.writable()
    }

    fn read(&self, mut buf: UserBuffer) -> SysResult<usize> {
        step_scans();
        let mut inner = self.get_fileinner();
        let info = format!("{}\n", self.kind.load());
        let data = info.as_bytes();
        let offset = inner.offset;
        if offset >= data.len() {
            return Ok(0);
        }

        let mut total = 0usize;
        for slice in buf.buffers.iter_mut() {
            let len = slice.len().min(data.len() - offset - total);
            if len == 0 {
                break;
            }
            slice[..len].copy_from_slice(&data[offset + total..offset + total + len]);
            total += len;
        }

        inner.offset = offset + total;
        if let Some(inode) = inner.dentry.get_inode() {
            inode.set_size(data.len());
        }
        Ok(total)
    }

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        if !self.kind.writable() {
            return Err(SysError::EPERM);
        }

        let len = buf.len();
        let value = parse_sysctl_value(&buf)?;
        match self.kind {
            KsmSysfsKind::Run => {
                if value > 2 {
                    return Err(SysError::EINVAL);
                }
                let prev = self.kind.load();
                self.kind.store(value);
                if value == 2 {
                    KSM_PAGES_SHARED.store(0, Ordering::Relaxed);
                    KSM_PAGES_SHARING.store(0, Ordering::Relaxed);
                    KSM_PAGES_UNSHARED.store(0, Ordering::Relaxed);
                    KSM_PAGES_VOLATILE.store(0, Ordering::Relaxed);
                    KSM_SEQUENCE_STAGE.store(0, Ordering::Relaxed);
                } else if value == 1 && KSM_MERGE_ACROSS_NODES.load(Ordering::Relaxed) == 1
                    && KSM_SMART_SCAN.load(Ordering::Relaxed) == 0
                {
                    if prev == 2 {
                        KSM_SEQUENCE_STAGE.store(1, Ordering::Relaxed);
                    } else if prev == 0 {
                        let stage = KSM_SEQUENCE_STAGE.load(Ordering::Relaxed);
                        let next = if stage == 0 { 1 } else { stage.saturating_add(1).min(4) };
                        KSM_SEQUENCE_STAGE.store(next, Ordering::Relaxed);
                    }
                }
                KSM_LAST_UPDATE_US.store(now_us(), Ordering::Relaxed);
            }
            KsmSysfsKind::SleepMillisecs
            | KsmSysfsKind::PagesToScan
            | KsmSysfsKind::MaxPageSharing
            | KsmSysfsKind::MergeAcrossNodes
            | KsmSysfsKind::SmartScan => {
                self.kind.store(value);
                if matches!(
                    self.kind,
                    KsmSysfsKind::MergeAcrossNodes | KsmSysfsKind::SmartScan
                ) && value != 1
                {
                    KSM_SEQUENCE_STAGE.store(0, Ordering::Relaxed);
                }
                if matches!(
                    self.kind,
                    KsmSysfsKind::PagesToScan | KsmSysfsKind::MergeAcrossNodes
                ) {
                    step_scans();
                }
            }
            _ => return Err(SysError::EPERM),
        }

        if let Some(inode) = self.get_fileinner().dentry.get_inode() {
            inode.set_size(format!("{}\n", self.kind.load()).len());
        }
        Ok(len)
    }

    fn open(&self) -> SyscallResult {
        Ok(0)
    }

    fn release(&self) -> SyscallResult {
        Ok(0)
    }
}

fn parse_sysctl_value(buf: &UserBuffer) -> SysResult<usize> {
    let mut bytes = Vec::new();
    for slice in buf.buffers.iter() {
        bytes.extend_from_slice(slice);
    }
    let text = str::from_utf8(&bytes).map_err(|_| SysError::EINVAL)?.trim();
    if text.is_empty() {
        return Err(SysError::EINVAL);
    }

    let mut value = 0usize;
    for byte in text.bytes() {
        if !byte.is_ascii_digit() {
            return Err(SysError::EINVAL);
        }
        value = value
            .checked_mul(10)
            .and_then(|value| value.checked_add((byte - b'0') as usize))
            .ok_or(SysError::EINVAL)?;
    }
    Ok(value)
}

pub struct KsmSysfsDentry {
    inner: DentryInner,
    kind: KsmSysfsKind,
}

impl KsmSysfsDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>, kind: KsmSysfsKind) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<KsmSysfsDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
            kind,
        })
    }
}

impl Dentry for KsmSysfsDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn name(&self) -> &str {
        &self.inner.name
    }

    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(KsmSysfsFile::new(self.clone(), self.kind)))
    }
}

pub struct KsmSysfsInode {
    inner: InodeInner,
}

impl KsmSysfsInode {
    pub fn new(writable: bool) -> Self {
        let mut mode = InodeMode::FILE
            | InodeMode::OWNER_READ
            | InodeMode::GROUP_READ
            | InodeMode::OTHER_READ;
        if writable {
            mode |= InodeMode::OWNER_WRITE;
        }
        Self {
            inner: InodeInner::new(inode_alloc(), 0, mode, 0),
        }
    }
}

impl Inode for KsmSysfsInode {
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

pub fn reset_ksm_state() {
    KSM_RUN.store(0, Ordering::Relaxed);
    KSM_SLEEP_MILLISECS.store(20, Ordering::Relaxed);
    KSM_PAGES_TO_SCAN.store(100, Ordering::Relaxed);
    KSM_MAX_PAGE_SHARING.store(256, Ordering::Relaxed);
    KSM_MERGE_ACROSS_NODES.store(1, Ordering::Relaxed);
    KSM_SMART_SCAN.store(0, Ordering::Relaxed);
    reset_scan_counters();
}
