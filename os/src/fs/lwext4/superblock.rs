use crate::error::SysError;
use crate::fs::SuperBlockInner;
use crate::fs::lwext4::disk::Disk;
use crate::fs::lwext4::{
    Lwext4MountGate, Lwext4Op, lwext4_err_to_sys, unregister_lwext4_mount_gate,
    with_lwext4_lifecycle_lock_op, with_lwext4_mount_lock_op,
};
use crate::fs::vfs::SuperBlock;
use crate::fs::vfs::kstat::Statfs;
use alloc::ffi::CString;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use core::mem::ManuallyDrop;
use log::info;
use lwext4_rust::Ext4BlockWrapper;
use lwext4_rust::bindings::{ext4_mount_point_stats, ext4_mount_stats};

/// The Ext4SuperBlock
#[allow(dead_code)]
pub struct Ext4SuperBlock {
    inner: SuperBlockInner,
    block: ManuallyDrop<Ext4BlockWrapper<Disk>>,
    mount_point: String,
    mount_gate: Arc<Lwext4MountGate>,
}

unsafe impl Sync for Ext4SuperBlock {}
unsafe impl Send for Ext4SuperBlock {}

impl Ext4SuperBlock {
    /// Create a new Ext4 super block
    pub fn new(
        inner: SuperBlockInner,
        dev_name: &str,
        mount_point: &str,
        mount_gate: Arc<Lwext4MountGate>,
    ) -> Result<Self, SysError> {
        // let disk =Disk::new(BLOCK_DEVICE.clone());
        let block_device = inner.device.as_ref().unwrap().clone();
        let disk = Disk::new(block_device);
        let mount_point = normalize_ext4_mount_point(mount_point);

        info!(
            "Got Disk size:{}, position:{}",
            disk.size(),
            disk.position()
        );
        let read_only = inner.is_readonly();
        let block = with_lwext4_lifecycle_lock_op(Lwext4Op::Mount, || {
            Ext4BlockWrapper::<Disk>::new(disk, dev_name, &mount_point, read_only)
        })
        .map_err(lwext4_err_to_sys)?;

        Ok(Self {
            inner,
            block: ManuallyDrop::new(block),
            mount_point,
            mount_gate,
        })
    }
}

impl Drop for Ext4SuperBlock {
    fn drop(&mut self) {
        with_lwext4_lifecycle_lock_op(Lwext4Op::Mount, || unsafe {
            ManuallyDrop::drop(&mut self.block);
            unregister_lwext4_mount_gate(self.mount_gate.mount_id());
        });
    }
}

fn normalize_ext4_mount_point(path: &str) -> String {
    if path == "/" {
        return "/".to_string();
    }
    if path.ends_with('/') {
        path.to_string()
    } else {
        alloc::format!("{}/", path)
    }
}

impl SuperBlock for Ext4SuperBlock {
    fn inner(&self) -> &SuperBlockInner {
        &self.inner
    }

    fn statfs(&self) -> Statfs {
        let cpath = CString::new(self.mount_point.clone()).unwrap();
        let mut stats = ext4_mount_stats {
            inodes_count: 0,
            free_inodes_count: 0,
            blocks_count: 0,
            free_blocks_count: 0,
            block_size: 0,
            block_group_count: 0,
            blocks_per_group: 0,
            inodes_per_group: 0,
            volume_name: [0; 16],
        };
        with_lwext4_mount_lock_op(&self.mount_gate, Lwext4Op::Stat, || unsafe {
            ext4_mount_point_stats(cpath.as_ptr(), &mut stats);
        });
        let mut stat = Statfs::new();
        stat.f_type = 0xEF53;
        stat.f_bsize = stats.block_size as i64;
        stat.f_blocks = stats.blocks_count as i64;
        stat.f_bfree = stats.free_blocks_count as i64;
        stat.f_bavail = stats.free_blocks_count as i64;
        stat.f_files = stats.inodes_count as i64;
        stat.f_ffree = stats.free_inodes_count as i64;
        stat.f_frsize = stats.block_size as i64;
        stat
    }
}
