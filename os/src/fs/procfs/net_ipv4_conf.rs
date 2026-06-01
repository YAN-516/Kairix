#![allow(missing_docs)]
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::Mutex;
use spin::MutexGuard;

use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::procfs::NetNsTagKind;
use crate::fs::vfs::inode::make_rdev;
use crate::fs::vfs::inode::{inode_alloc, InodeInner, InodeMode};
use crate::fs::vfs::{Dentry, DentryInner, File, FileInner, Inode, OpenFlags};
use crate::mm::UserBuffer;
use crate::sync::SpinNoIrqLock;
use crate::task::current_process;
use alloc::format;
use alloc::sync::Weak;
use alloc::vec::Vec;
/// 每个网络命名空间的 tag 值
pub struct NetNsValues {
    pub lo_tag: usize,
    pub default_tag: usize,
}

static NEXT_NET_NS_ID: AtomicUsize = AtomicUsize::new(1);
static NET_NS_VALUES: SpinNoIrqLock<BTreeMap<usize, NetNsValues>> =
    SpinNoIrqLock::new(BTreeMap::new());

/// 分配一个新的网络命名空间，lo_tag 初始化为父命名空间的 default_tag
pub fn alloc_net_ns(parent_ns_id: usize) -> usize {
    let ns_id = NEXT_NET_NS_ID.fetch_add(1, Ordering::SeqCst);
    let default_tag = read_default_tag(parent_ns_id);
    let mut map = NET_NS_VALUES.lock();
    map.insert(
        ns_id,
        NetNsValues {
            lo_tag: default_tag,
            default_tag,
        },
    );
    ns_id
}

/// 读取指定命名空间的 lo/tag 值
pub fn read_lo_tag(ns_id: usize) -> usize {
    if ns_id == 0 {
        // 初始命名空间使用全局默认值
        let map = NET_NS_VALUES.lock();
        map.get(&0).map(|v| v.lo_tag).unwrap_or(0)
    } else {
        let map = NET_NS_VALUES.lock();
        map.get(&ns_id).map(|v| v.lo_tag).unwrap_or(0)
    }
}

/// 写入指定命名空间的 lo/tag 值
pub fn write_lo_tag(ns_id: usize, value: usize) {
    let mut map = NET_NS_VALUES.lock();
    if ns_id == 0 {
        map.entry(0)
            .or_insert_with(|| NetNsValues {
                lo_tag: 0,
                default_tag: 0,
            })
            .lo_tag = value;
    } else {
        if let Some(v) = map.get_mut(&ns_id) {
            v.lo_tag = value;
        }
    }
}

/// 读取指定命名空间的 default/tag 值
pub fn read_default_tag(ns_id: usize) -> usize {
    if ns_id == 0 {
        let map = NET_NS_VALUES.lock();
        map.get(&0).map(|v| v.default_tag).unwrap_or(0)
    } else {
        let map = NET_NS_VALUES.lock();
        map.get(&ns_id).map(|v| v.default_tag).unwrap_or(0)
    }
}

/// 写入指定命名空间的 default/tag 值
pub fn write_default_tag(ns_id: usize, value: usize) {
    let mut map = NET_NS_VALUES.lock();
    if ns_id == 0 {
        map.entry(0)
            .or_insert_with(|| NetNsValues {
                lo_tag: 0,
                default_tag: 0,
            })
            .default_tag = value;
    } else {
        if let Some(v) = map.get_mut(&ns_id) {
            v.default_tag = value;
        }
    }
}

/// /proc/sys/net/ipv4/conf/{lo,default}/tag 文件
pub struct NetNsTagFile {
    inner: Mutex<FileInner>,
    kind: NetNsTagKind,
}

impl NetNsTagFile {
    pub fn new(dentry: Arc<dyn Dentry>, kind: NetNsTagKind) -> Self {
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

impl File for NetNsTagFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    fn read(&self, mut buf: UserBuffer) -> SysResult<usize> {
        let mut inner = self.get_fileinner();
        let process = current_process();
        let ns_id = process.inner_exclusive_access().net_ns_id;
        let value = match self.kind {
            NetNsTagKind::Lo => read_lo_tag(ns_id),
            NetNsTagKind::Default => read_default_tag(ns_id),
        };
        let data = format!("{}\n", value);
        let data_bytes = data.as_bytes();
        let offset = inner.offset;
        if offset >= data_bytes.len() {
            return Ok(0);
        }
        let remaining = &data_bytes[offset..];
        let mut total = 0usize;
        for slice in buf.buffers.iter_mut() {
            let len = slice.len().min(remaining.len() - total);
            if len == 0 {
                break;
            }
            slice[..len].copy_from_slice(&remaining[total..total + len]);
            total += len;
        }
        inner.offset = offset + total;
        if let Some(inode) = inner.dentry.get_inode() {
            inode.set_size(data_bytes.len());
        }
        Ok(total)
    }

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        let mut inner = self.get_fileinner();
        let process = current_process();
        let ns_id = process.inner_exclusive_access().net_ns_id;
        let mut total = 0usize;
        let mut data = Vec::new();
        for slice in buf.buffers.iter() {
            data.extend_from_slice(slice);
            total += slice.len();
        }
        // 解析整数（去除空白）
        let s = core::str::from_utf8(&data).unwrap_or("").trim();
        if let Ok(value) = s.parse::<usize>() {
            match self.kind {
                NetNsTagKind::Lo => write_lo_tag(ns_id, value),
                NetNsTagKind::Default => write_default_tag(ns_id, value),
            }
        }
        inner.offset = 0;
        if let Some(inode) = inner.dentry.get_inode() {
            inode.set_size(total);
        }
        Ok(total)
    }

    fn open(&self) -> SyscallResult {
        Ok(0)
    }

    fn release(&self) -> SyscallResult {
        Ok(0)
    }
}

pub struct NetNsTagDentry {
    inner: DentryInner,
    kind: NetNsTagKind,
}

impl NetNsTagDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>, kind: NetNsTagKind) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<NetNsTagDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
            kind,
        })
    }
}

impl Dentry for NetNsTagDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }

    fn name(&self) -> &str {
        &self.inner.name
    }

    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(NetNsTagFile::new(self.clone(), self.kind)))
    }
}

pub struct NetNsTagInode {
    inner: InodeInner,
}

//待改dev
impl NetNsTagInode {
    pub fn new() -> Self {
        Self {
            inner: InodeInner::new(inode_alloc(), 0, InodeMode::FILE, make_rdev(2, 14) as usize),
        }
    }
}

impl Inode for NetNsTagInode {
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
    fn truncate(&self, _size: u64) -> SysResult<usize> {
        Ok(0)
    }
}
