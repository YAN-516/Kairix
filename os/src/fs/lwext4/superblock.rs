use crate::error::SysError;
use crate::fs::lwext4::lwext4_err_to_sys;
use crate::fs::vfs::SuperBlock;
use crate::fs::SuperBlockInner;
use lwext4_rust::Ext4BlockWrapper;
use crate::fs::lwext4::disk::Disk;
use log::info;
use crate::fs::vfs::kstat::Statfs;
use lwext4_rust::bindings::{ext4_mount_point_stats, ext4_mount_stats};
use alloc::ffi::CString;
use alloc::string::{String, ToString};

/// The Ext4SuperBlock
#[allow(dead_code)]
pub struct Ext4SuperBlock {
    inner:SuperBlockInner,
    block: Ext4BlockWrapper<Disk>,
    mount_point: String,
}

unsafe impl Sync for Ext4SuperBlock {}
unsafe impl Send for Ext4SuperBlock {}

impl Ext4SuperBlock {
    /// Create a new Ext4 super block
    pub fn new(inner:SuperBlockInner, dev_name: &str, mount_point: &str) -> Result<Self, SysError> {
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
        let block = Ext4BlockWrapper::<Disk>::new(disk, dev_name, &mount_point, read_only)
            .map_err(lwext4_err_to_sys)?;
       
        Ok(Self { inner, block, mount_point })
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
        unsafe {
            ext4_mount_point_stats(cpath.as_ptr(), &mut stats);
        }
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
