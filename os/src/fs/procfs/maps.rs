#![allow(missing_docs)]
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::Dentry;
use crate::fs::File;
use crate::fs::Inode;
use crate::fs::vfs::DentryInner;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::inode::InodeInner;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::inode::inode_alloc;
use crate::mm::UserBuffer;
use crate::mm::vm_area::MapArea;
use crate::task::current_process;
use alloc::format;
use alloc::string::String;
use alloc::sync::{Arc, Weak};
use core::sync::atomic::Ordering;
use polyhal::pagetable::MapPermission;
use spin::{Mutex, MutexGuard};
/// /proc/self/maps 文件。
pub struct MapsFile {
    inner: Mutex<FileInner>,
}

impl MapsFile {
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

impl File for MapsFile {
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
            // 判断共享/私有标志
            let share_flag = match area.area_type {
                crate::mm::vm_area::UserMapAreaType::Mmap => {
                    if area.flags == crate::mm::vm_area::MmapType::MapShared {
                        's'
                    } else {
                        'p'
                    }
                }
                crate::mm::vm_area::UserMapAreaType::Shm => 's',
                _ => 'p',
            };

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
                share_flag,
            );
            let typ = match area.area_type {
                crate::mm::vm_area::UserMapAreaType::Elf => "/",
                crate::mm::vm_area::UserMapAreaType::Stack => "[stack]",
                crate::mm::vm_area::UserMapAreaType::Heap => "[heap]",
                crate::mm::vm_area::UserMapAreaType::TrapContext => "[trap]",
                crate::mm::vm_area::UserMapAreaType::RtSigreturnTrampoline => {
                    "[rt_sigreturn]"
                }
                crate::mm::vm_area::UserMapAreaType::Mmap => "/",
                crate::mm::vm_area::UserMapAreaType::Shm => "[shmem]",
            };
            info.push_str(&format!(
                "{:08x}-{:08x} {} 00000000 00:00 0 {}\n",
                start, end, perm_str, typ
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

/// /proc/self/maps 的 dentry。
pub struct MapsDentry {
    inner: DentryInner,
}

impl MapsDentry {
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<MapsDentry>| Self {
            inner: DentryInner::new(name, parent_weak),
        })
    }
}

impl Dentry for MapsDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }
    fn name(&self) -> &str {
        &self.inner.name
    }
    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(MapsFile::new(self)))
    }
}

/// /proc/self/maps 的 inode。
pub struct MapsInode {
    inner: InodeInner,
}

impl MapsInode {
    pub fn new() -> Self {
        Self {
            inner: InodeInner::new(inode_alloc(), 0, InodeMode::FILE, 0),
        }
    }
}

impl Inode for MapsInode {
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
