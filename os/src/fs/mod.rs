//参考chronix
pub(crate) mod config;
///
pub mod devfs;
///
pub mod etc;
pub mod fat32;
/// File handle ABI helpers shared by VFS syscalls and notify events.
pub mod file_handle;
///
pub mod lwext4;
/// Filesystem event notification backends.
pub mod notify;
///
pub mod page;
/// pidfd support
pub mod pidfd;
/// Pipe and Unix socketpair file implementations.
pub mod pipe;
///
pub mod procfs;
///
pub mod sysfs;
///
pub mod tmpfs;
pub mod vfs;
/// Deferred write-back support for dirty VFS files.
pub mod writeback;
use alloc::boxed::Box;
use alloc::collections::btree_map::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use lazy_static::lazy_static;
use log::*;
use lwext4_rust::InodeTypes;
use spin::mutex::Mutex;

pub use self::lwext4::file::Ext4File;
pub use self::lwext4::superblock::Ext4SuperBlock;
pub use self::vfs::file::File;
pub use self::vfs::superblock::{SuperBlock, SuperBlockInner};
use crate::drivers::BLOCK_DEVICE;
use crate::fs::devfs::fstype::DevFsType;
use crate::fs::devfs::init_devfs;
use crate::fs::etc::init_etcfs;
use crate::fs::fat32::fstype::Fat32FsType;
use crate::fs::lwext4::{dentry::Ext4Dentry, fstype::Ext4FsType, inode::Ext4Inode};
use crate::fs::procfs::fstype::ProcFsType;
use crate::fs::procfs::init_procfs;
use crate::fs::sysfs::init_sysfs;
use crate::fs::sysfs::sysfs_block::SysfsStatDentry;
use crate::fs::sysfs::sysfs_block::SysfsStatInode;
use crate::fs::tmpfs::dentry::TempDentry;
use crate::fs::tmpfs::fstype::TempFsType;
use crate::fs::tmpfs::init_tempfs;
use crate::fs::tmpfs::inode::TempInode;
use crate::fs::vfs::{
    Dentry,
    dcache::GLOBAL_DCACHE,
    fstype::{FsType, MountFlags},
    inode::{Inode, InodeMode},
    path::resolve_path,
};
///
pub static FS_MANAGER: Mutex<BTreeMap<String, Arc<dyn FsType>>> = Mutex::new(BTreeMap::new());

/// the name of disk fs
pub const DISK_FS_NAME: &str = "ext4";

/// 根据绝对路径查找对应的 superblock（最长前缀匹配）
pub fn find_superblock_by_path(path: &str) -> Option<Arc<dyn SuperBlock>> {
    let fs_mgr = FS_MANAGER.lock();
    let mut best_sb: Option<Arc<dyn SuperBlock>> = None;
    let mut best_len = 0usize;
    for (_name, fstype) in fs_mgr.iter() {
        let supers = fstype.inner().supers.lock();
        for (mp, sb) in supers.iter() {
            if path.starts_with(mp) {
                let matched = if mp.ends_with('/') {
                    true
                } else {
                    path.len() == mp.len() || path.as_bytes().get(mp.len()) == Some(&b'/')
                };
                if matched && mp.len() >= best_len {
                    best_len = mp.len();
                    best_sb = Some(sb.clone());
                }
            }
        }
    }
    best_sb
}
/// register all filesystem
fn register_all_fs() {
    let diskfs = Ext4FsType::new(DISK_FS_NAME);
    FS_MANAGER.lock().insert(diskfs.name().to_string(), diskfs);

    let ext2fs = Ext4FsType::new("ext2");
    FS_MANAGER.lock().insert(ext2fs.name().to_string(), ext2fs);

    let ext3fs = Ext4FsType::new("ext3");
    FS_MANAGER.lock().insert(ext3fs.name().to_string(), ext3fs);

    let fat32fs = Fat32FsType::new("fat32");
    FS_MANAGER
        .lock()
        .insert(fat32fs.name().to_string(), fat32fs);

    let devfs = DevFsType::new("devfs");
    FS_MANAGER.lock().insert(devfs.name().to_string(), devfs);

    let etcfs = TempFsType::new("etc");
    FS_MANAGER.lock().insert(etcfs.name().to_string(), etcfs);

    let procfs = ProcFsType::new("proc");
    FS_MANAGER.lock().insert(procfs.name().to_string(), procfs);

    let tmpfs = TempFsType::new("tmpfs");
    FS_MANAGER.lock().insert(tmpfs.name().to_string(), tmpfs);

    let sysfs = TempFsType::new("sysfs");
    FS_MANAGER.lock().insert(sysfs.name().to_string(), sysfs);
}

/// get the file system by name
pub fn get_filesystem(name: &str) -> Arc<dyn FsType> {
    FS_MANAGER.lock().get(name).unwrap().clone()
}

#[cfg(target_arch = "loongarch64")]
const LOONGARCH_FDT_VADDR: usize = 0x9000_0000_0ecc_f480;
#[cfg(target_arch = "loongarch64")]
const INITRD_SCAN_END: usize = 0x9000_0000_9800_0000;
#[cfg(target_arch = "loongarch64")]
const INITRD_SCAN_STEP: usize = 0x1000;

#[cfg(target_arch = "loongarch64")]
#[derive(Clone, Copy)]
struct InitrdRange {
    start: usize,
    len: usize,
}

#[cfg(target_arch = "loongarch64")]
fn be_usize(bytes: &[u8]) -> Option<usize> {
    match bytes.len() {
        4 => Some(u32::from_be_bytes(bytes.try_into().ok()?) as usize),
        8 => Some(u64::from_be_bytes(bytes.try_into().ok()?) as usize),
        _ => None,
    }
}

#[cfg(target_arch = "loongarch64")]
fn initrd_addr_to_vaddr(addr: usize) -> usize {
    if addr >= polyhal::consts::VIRT_ADDR_START {
        addr
    } else {
        addr + polyhal::consts::VIRT_ADDR_START
    }
}

#[cfg(target_arch = "loongarch64")]
fn initrd_scan_start() -> usize {
    unsafe extern "C" {
        safe fn ekernel();
    }

    let kernel_end = ekernel as usize;
    (kernel_end + INITRD_SCAN_STEP - 1) & !(INITRD_SCAN_STEP - 1)
}

#[cfg(target_arch = "loongarch64")]
fn ext4_len_from_superblock(vaddr: usize) -> Option<usize> {
    const EXT4_SUPER_OFFSET: usize = 1024;
    const EXT4_MAGIC_OFFSET: usize = EXT4_SUPER_OFFSET + 0x38;
    const EXT4_BLOCKS_COUNT_LO_OFFSET: usize = EXT4_SUPER_OFFSET + 0x04;
    const EXT4_LOG_BLOCK_SIZE_OFFSET: usize = EXT4_SUPER_OFFSET + 0x18;

    let magic = unsafe { core::ptr::read_unaligned((vaddr + EXT4_MAGIC_OFFSET) as *const u16) };
    if magic != 0xef53 {
        return None;
    }

    let blocks =
        unsafe { core::ptr::read_unaligned((vaddr + EXT4_BLOCKS_COUNT_LO_OFFSET) as *const u32) }
            as usize;
    let log_block_size =
        unsafe { core::ptr::read_unaligned((vaddr + EXT4_LOG_BLOCK_SIZE_OFFSET) as *const u32) }
            as usize;
    let block_size = 1024usize.checked_shl(log_block_size as u32)?;
    blocks.checked_mul(block_size)
}

#[cfg(target_arch = "loongarch64")]
fn find_initrd_from_fdt() -> Option<InitrdRange> {
    let fdt = unsafe { flat_device_tree::Fdt::from_ptr(LOONGARCH_FDT_VADDR as *const u8) }.ok()?;
    let chosen = fdt.find_node("/chosen")?;
    let start = chosen
        .property("linux,initrd-start")
        .and_then(|prop| be_usize(prop.value))?;
    let end = chosen
        .property("linux,initrd-end")
        .and_then(|prop| be_usize(prop.value))?;
    if end <= start {
        warn!(
            "[initrd] invalid FDT initrd range: start={:#x}, end={:#x}",
            start, end
        );
        return None;
    }

    Some(InitrdRange {
        start: initrd_addr_to_vaddr(start),
        len: end - start,
    })
}

#[cfg(target_arch = "loongarch64")]
fn scan_initrd_memory() -> Option<InitrdRange> {
    let mut addr = initrd_scan_start();
    info!(
        "[initrd] scan memory range {:#x}..{:#x}",
        addr, INITRD_SCAN_END
    );
    while addr + 0x1000 < INITRD_SCAN_END {
        let b0 = unsafe { core::ptr::read_volatile(addr as *const u8) };
        let b1 = unsafe { core::ptr::read_volatile((addr + 1) as *const u8) };
        if b0 == 0x1f && b1 == 0x8b {
            warn!(
                "[initrd] found gzip image at {:#x}, but gzip initrd decompression is not implemented yet",
                addr
            );
            return None;
        }

        if let Some(len) = ext4_len_from_superblock(addr) {
            info!(
                "[initrd] found ext4 image by scan: start={:#x}, len={:#x}",
                addr, len
            );
            return Some(InitrdRange { start: addr, len });
        }

        addr += INITRD_SCAN_STEP;
    }

    None
}

#[cfg(target_arch = "loongarch64")]
fn find_initrd() -> Option<InitrdRange> {
    if let Some(range) = find_initrd_from_fdt() {
        info!(
            "[initrd] found from FDT: start={:#x}, len={:#x}",
            range.start, range.len
        );
        return Some(range);
    }

    warn!("[initrd] FDT has no linux,initrd-start/end; scanning memory");
    scan_initrd_memory()
}

#[cfg(target_arch = "loongarch64")]
fn mount_loongarch64_initrd_or_tmpfs_root() -> Arc<dyn Dentry> {
    if let Some(initrd) = find_initrd() {
        let magic0 = unsafe { core::ptr::read_volatile(initrd.start as *const u8) };
        let magic1 = unsafe { core::ptr::read_volatile((initrd.start + 1) as *const u8) };
        info!(
            "[initrd] candidate first bytes: {:02x} {:02x}",
            magic0, magic1
        );

        let rootfs = get_filesystem("ext4");
        let dev = Arc::new(crate::drivers::block::RamDisk::new(
            initrd.start,
            initrd.len,
        ));
        match rootfs.mount("/", None, MountFlags::empty(), Some(dev)) {
            Ok(root_dentry) => {
                info!("[initrd] mounted initrd as ext4 root");
                return root_dentry;
            }
            Err(err) => {
                warn!(
                    "[initrd] failed to mount initrd as ext4 root: {:?}; fallback to tmpfs root",
                    err
                );
            }
        }
    } else {
        warn!("[initrd] no usable initrd found; fallback to tmpfs root");
    }

    let tmpfs = get_filesystem("tmpfs");
    tmpfs
        .mount("/", None, MountFlags::empty(), None)
        .expect("failed to mount tmpfs root")
}

#[cfg(all(target_arch = "loongarch64", not(board = "2k1000")))]
fn mount_loongarch64_root() -> Arc<dyn Dentry> {
    let rootfs = get_filesystem("ext4");
    match rootfs.mount("/", None, MountFlags::empty(), Some(BLOCK_DEVICE.clone())) {
        Ok(root_dentry) => {
            info!("[rootfs] mounted virtio block as ext4 root");
            root_dentry
        }
        Err(err) => {
            warn!(
                "[rootfs] failed to mount virtio block as ext4 root: {:?}; trying initrd/tmpfs",
                err
            );
            mount_loongarch64_initrd_or_tmpfs_root()
        }
    }
}

#[cfg(all(target_arch = "loongarch64", board = "2k1000"))]
fn mount_loongarch64_root() -> Arc<dyn Dentry> {
    mount_loongarch64_initrd_or_tmpfs_root()
}

#[cfg(not(target_arch = "loongarch64"))]
fn mount_root() -> Arc<dyn Dentry> {
    let rootfs = get_filesystem("ext4");
    rootfs
        .mount("/", None, MountFlags::empty(), Some(BLOCK_DEVICE.clone()))
        .unwrap()
}

#[cfg(target_arch = "loongarch64")]
fn mount_root() -> Arc<dyn Dentry> {
    mount_loongarch64_root()
}

/// init the file system
pub fn init() {
    register_all_fs();

    let root_dentry = mount_root();
    GLOBAL_DCACHE.insert("/".to_string(), root_dentry.clone());
    GLOBAL_DCACHE.pin("/".to_string());

    //mount the devfs
    let devfs = get_filesystem("devfs");
    let devfs_dentry = devfs
        .mount("dev", Some(root_dentry.clone()), MountFlags::empty(), None)
        .unwrap();
    init_devfs(devfs_dentry.clone());
    root_dentry.add_child(devfs_dentry.clone());
    info!("[FS] insert path: {}", devfs_dentry.path());
    GLOBAL_DCACHE.insert(devfs_dentry.path(), devfs_dentry.clone());
    GLOBAL_DCACHE.pin(devfs_dentry.path());

    // mount /dev/shm (required by shm_open)
    let shm_tmpfs = get_filesystem("tmpfs");
    let shm_dentry = shm_tmpfs
        .mount("shm", Some(devfs_dentry.clone()), MountFlags::empty(), None)
        .unwrap();
    devfs_dentry.add_child(shm_dentry.clone());
    info!("[FS] insert path: {}", shm_dentry.path());
    GLOBAL_DCACHE.insert(shm_dentry.path(), shm_dentry.clone());
    GLOBAL_DCACHE.pin(shm_dentry.path());

    //mount the etc tmpfs
    let etcfs = get_filesystem("etc");
    let etc_dentry = etcfs
        .mount("etc", Some(root_dentry.clone()), MountFlags::empty(), None)
        .unwrap();
    init_etcfs(etc_dentry.clone());
    root_dentry.add_child(etc_dentry.clone());
    info!("[FS] insert path: {}", etc_dentry.path());
    GLOBAL_DCACHE.insert(etc_dentry.path(), etc_dentry.clone());
    GLOBAL_DCACHE.pin(etc_dentry.path());

    //mount the proc
    let procfs = get_filesystem("proc");
    let proc_dentry = procfs
        .mount("proc", Some(root_dentry.clone()), MountFlags::empty(), None)
        .unwrap();
    init_procfs(proc_dentry.clone());
    root_dentry.add_child(proc_dentry.clone());
    info!("[FS] insert path: {}", proc_dentry.path());
    GLOBAL_DCACHE.insert(proc_dentry.path(), proc_dentry.clone());
    GLOBAL_DCACHE.pin(proc_dentry.path());

    //mount the tmpfs
    let tmpfs = get_filesystem("tmpfs");
    let tmp_dentry = tmpfs
        .mount("tmp", Some(root_dentry.clone()), MountFlags::empty(), None)
        .unwrap();
    init_tempfs(tmp_dentry.clone());
    root_dentry.add_child(tmp_dentry.clone());
    info!("[FS] insert path: {}", tmp_dentry.path());
    GLOBAL_DCACHE.insert(tmp_dentry.path(), tmp_dentry.clone());
    GLOBAL_DCACHE.pin(tmp_dentry.path());

    //mount the sysfs
    let sysfs = get_filesystem("sysfs");
    let sys_dentry = sysfs
        .mount("sys", Some(root_dentry.clone()), MountFlags::empty(), None)
        .unwrap();
    init_sysfs(sys_dentry.clone());
    root_dentry.add_child(sys_dentry.clone());
    info!("[FS] insert path: {}", sys_dentry.path());
    GLOBAL_DCACHE.insert(sys_dentry.path(), sys_dentry.clone());
    GLOBAL_DCACHE.pin(sys_dentry.path());
}
