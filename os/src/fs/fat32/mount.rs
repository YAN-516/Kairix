use alloc::sync::Arc;
use crate::devices::BlockDevice;
use crate::fs::SuperBlock;
use crate::fs::fat32::io::FatIoAdapter;
use fatfs::FileSystem;
use crate::fs::SuperBlockInner;
use crate::fs::fat32::superblock::Fat32SuperBlock;
use crate::fs::fat32::dentry::Fat32Dentry;
use crate::fs::Dentry;
pub fn mount_fat32_fs(device: Arc<dyn BlockDevice>, mount_point: &str) -> Result<Arc<dyn SuperBlock>, isize> {
    let io_adapter = FatIoAdapter::new(device.clone());
    let fs_instance = FileSystem::new(io_adapter, fatfs::FsOptions::new())
        .map_err(|_| -22)?; 
        
    let fs_arc = Arc::new(fs_instance);
    
    let real_root = fs_arc.root_dir();
    let root_dentry = Arc::new(Fat32Dentry::new(mount_point, real_root, fs_arc.clone()));
    
    // 4. 组装出你的 SuperBlock
    let superblock = Arc::new(Fat32SuperBlock {
        inner: SuperBlockInner {
            device: Some(device),
            root: Some(root_dentry.clone() as Arc<dyn Dentry>),
        },
        block: fs_arc,
    });
    
    Ok(superblock)
}