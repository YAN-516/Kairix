pub mod sysfs_block;
pub mod sysfs_ksm;
use alloc::sync::Arc;
use crate::fs::Dentry;
use crate::fs::tmpfs::dentry::TempDentry;
use crate::fs::tmpfs::inode::TempInode;
use crate::fs::InodeMode;
use crate::fs::GLOBAL_DCACHE;
use crate::alloc::string::ToString;
use log::*;
use crate::fs::SysfsStatDentry;
use crate::fs::SysfsStatInode;
use crate::fs::sysfs::sysfs_ksm::{KsmSysfsDentry, KsmSysfsInode, KsmSysfsKind, reset_ksm_state};
///
pub fn init_sysfs(root_dentry: Arc<dyn Dentry>){
    reset_ksm_state();

    let block_dentry =TempDentry::new("block", Some(root_dentry.clone()));
    let block_inode = Arc::new(TempInode::new(InodeMode::DIR));
    block_dentry.set_inode(block_inode);
    root_dentry.add_child(block_dentry.clone());
    GLOBAL_DCACHE.insert("/sys/block".to_string(), block_dentry.clone());
    info!("[FS] insert path: /sys/block");

    let loop0_dentry = crate::fs::tmpfs::dentry::TempDentry::new("loop0", Some(block_dentry.clone()));
    let loop0_inode = Arc::new(crate::fs::tmpfs::inode::TempInode::new(InodeMode::DIR));
    loop0_dentry.set_inode(loop0_inode);
    block_dentry.add_child(loop0_dentry.clone());
    GLOBAL_DCACHE.insert("/sys/block/loop0".to_string(), loop0_dentry.clone());
    info!("[FS] insert path: /sys/block/loop0");

    let stat_dentry = SysfsStatDentry::new("stat", Some(loop0_dentry.clone()));
    let stat_inode = Arc::new(SysfsStatInode::new());
    stat_dentry.set_inode(stat_inode);
    loop0_dentry.add_child(stat_dentry.clone());
    GLOBAL_DCACHE.insert("/sys/block/loop0/stat".to_string(), stat_dentry.clone());
    info!("[FS] insert path: /sys/block/loop0/stat");

    let kernel_dentry = TempDentry::new("kernel", Some(root_dentry.clone()));
    let kernel_inode = Arc::new(TempInode::new(InodeMode::DIR));
    kernel_dentry.set_inode(kernel_inode);
    root_dentry.add_child(kernel_dentry.clone());
    GLOBAL_DCACHE.insert("/sys/kernel".to_string(), kernel_dentry.clone());
    info!("[FS] insert path: /sys/kernel");

    let mm_dentry = TempDentry::new("mm", Some(kernel_dentry.clone()));
    let mm_inode = Arc::new(TempInode::new(InodeMode::DIR));
    mm_dentry.set_inode(mm_inode);
    kernel_dentry.add_child(mm_dentry.clone());
    GLOBAL_DCACHE.insert("/sys/kernel/mm".to_string(), mm_dentry.clone());
    info!("[FS] insert path: /sys/kernel/mm");

    let ksm_dentry = TempDentry::new("ksm", Some(mm_dentry.clone()));
    let ksm_inode = Arc::new(TempInode::new(InodeMode::DIR));
    ksm_dentry.set_inode(ksm_inode);
    mm_dentry.add_child(ksm_dentry.clone());
    GLOBAL_DCACHE.insert("/sys/kernel/mm/ksm".to_string(), ksm_dentry.clone());
    info!("[FS] insert path: /sys/kernel/mm/ksm");

    add_ksm_file(ksm_dentry.clone(), "run", KsmSysfsKind::Run);
    add_ksm_file(
        ksm_dentry.clone(),
        "sleep_millisecs",
        KsmSysfsKind::SleepMillisecs,
    );
    add_ksm_file(
        ksm_dentry.clone(),
        "pages_to_scan",
        KsmSysfsKind::PagesToScan,
    );
    add_ksm_file(
        ksm_dentry.clone(),
        "pages_shared",
        KsmSysfsKind::PagesShared,
    );
    add_ksm_file(
        ksm_dentry.clone(),
        "pages_sharing",
        KsmSysfsKind::PagesSharing,
    );
    add_ksm_file(
        ksm_dentry.clone(),
        "pages_unshared",
        KsmSysfsKind::PagesUnshared,
    );
    add_ksm_file(
        ksm_dentry.clone(),
        "pages_volatile",
        KsmSysfsKind::PagesVolatile,
    );
    add_ksm_file(
        ksm_dentry.clone(),
        "full_scans",
        KsmSysfsKind::FullScans,
    );
    add_ksm_file(
        ksm_dentry.clone(),
        "pages_skipped",
        KsmSysfsKind::PagesSkipped,
    );
    add_ksm_file(
        ksm_dentry.clone(),
        "max_page_sharing",
        KsmSysfsKind::MaxPageSharing,
    );
    add_ksm_file(
        ksm_dentry.clone(),
        "merge_across_nodes",
        KsmSysfsKind::MergeAcrossNodes,
    );
    add_ksm_file(ksm_dentry, "smart_scan", KsmSysfsKind::SmartScan);
}

fn add_ksm_file(parent: Arc<dyn Dentry>, name: &str, kind: KsmSysfsKind) {
    let dentry = KsmSysfsDentry::new(name, Some(parent.clone()), kind);
    let inode = Arc::new(KsmSysfsInode::new(kind.writable()));
    dentry.set_inode(inode);
    parent.add_child(dentry.clone());
    let path = alloc::format!("/sys/kernel/mm/ksm/{}", name);
    GLOBAL_DCACHE.insert(path.clone(), dentry);
    info!("[FS] insert path: {}", path);
}
