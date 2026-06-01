#![allow(missing_docs)]
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::inode::inode_alloc;
use crate::fs::vfs::inode::InodeInner;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::DentryInner;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::OpenFlags;
use crate::fs::Dentry;
use crate::fs::File;
use crate::fs::Inode;
use crate::mm::vm_area::MapArea;
use crate::mm::UserBuffer;
use crate::task::current_process;
use alloc::format;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use core::sync::atomic::Ordering;
use log::*;
use polyhal::consts::PAGE_SIZE;
use polyhal::pagetable::MapPermission;
use spin::{Mutex, MutexGuard};

/// /proc/self/smaps 文件。
pub struct SmapsFile {
    inner: Mutex<FileInner>,
}

impl SmapsFile {
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

impl File for SmapsFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }
    fn writable(&self) -> bool {
        false
    }

    fn read(&self, mut buf: UserBuffer) -> SysResult<usize> {
        let mut inner = self.get_fileinner();
        let process = current_process();
        let proc_inner = process.inner_exclusive_access();

        let mut info = String::new();
        for area in proc_inner.vm_set.areas.iter() {
            let start = area.start_va().0;
            let end = area.end_va().0;
            let perm = area.map_perm;
            let perm_str = format!(
                "{}{}{}{}",
                if perm.contains(MapPermission::R) {
                    'r'
                } else {
                    '-'
                },
                if perm.contains(MapPermission::W) {
                    'w'
                } else {
                    '-'
                },
                if perm.contains(MapPermission::X) {
                    'x'
                } else {
                    '-'
                },
                if perm.contains(MapPermission::U) {
                    'p'
                } else {
                    '-'
                },
            );
            let size_kb = (end - start) / 1024;
            let rss_kb = area.data_frames.len() * PAGE_SIZE / 1024;
            let typ = match area.area_type {
                crate::mm::vm_area::UserMapAreaType::Elf => "elf",
                crate::mm::vm_area::UserMapAreaType::Stack => "stack",
                crate::mm::vm_area::UserMapAreaType::Heap => "heap",
                crate::mm::vm_area::UserMapAreaType::TrapContext => "trap",
                crate::mm::vm_area::UserMapAreaType::Mmap => "mmap",
                crate::mm::vm_area::UserMapAreaType::Shm => "shm",
            };
            info.push_str(&format!(
                "{:08x}-{:08x} {} 00000000 00:00 0          {}\n\
                 Size:                  {:>8} kB\n\
                 Rss:                   {:>8} kB\n\
                 Pss:                   {:>8} kB\n\
                 Shared_Clean:          {:>8} kB\n\
                 Shared_Dirty:          {:>8} kB\n\
                 Private_Clean:         {:>8} kB\n\
                 Private_Dirty:         {:>8} kB\n\
                 Locked:                {:>8} kB\n",
                start, end, perm_str, typ, size_kb, rss_kb, rss_kb, 0, 0, rss_kb, 0, rss_kb
            ));
        }
        drop(proc_inner);

        let data = info.as_bytes();
        let offset = inner.offset;
        if offset >= data.len() {
            return Ok(0);
        }

        let remaining = &data[offset..];
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
            inode.set_size(data.len());
        }
        Ok(total)
    }

    fn write(&self, _buf: UserBuffer) -> SysResult<usize> {
        Ok(0)
    }

    fn open(&self) -> SyscallResult {
        Ok(0)
    }
    fn release(&self) -> SyscallResult {
        Ok(0)
    }
}

/// /proc/self/smaps 的 dentry。
pub struct SmapsDentry {
    inner: DentryInner,
}

impl SmapsDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<SmapsDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
        })
    }
}

impl Dentry for SmapsDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }
    fn name(&self) -> &str {
        &self.inner.name
    }
    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(SmapsFile::new(self)))
    }
}

/// /proc/self/smaps 的 inode。
pub struct SmapsInode {
    inner: InodeInner,
}

impl SmapsInode {
    pub fn new() -> Self {
        Self {
            inner: InodeInner::new(inode_alloc(), 0, InodeMode::FILE, 0),
        }
    }
}

impl Inode for SmapsInode {
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
        self.inner.rdev.load(core::sync::atomic::Ordering::Relaxed)
    }
    fn set_rdev(&self, rdev: usize) {
        self.inner
            .rdev
            .store(rdev, core::sync::atomic::Ordering::Relaxed);
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
