use alloc::sync::Arc;
use crate::fs::{Ext4Inode,SuperBlock,Inode,SuperBlockInner,Ext4Dentry,Ext4SuperBlock};
use crate::fs::vfs::Dentry;
use lwext4_rust::InodeTypes;
use crate::fs::BLOCK_DEVICE;
///the mount function for ext4 file system
pub fn mount_ext4_fs(_special: &str, dir: &str) -> Result<Arc<dyn SuperBlock>, isize> {
    let root_inode = Arc::new(Ext4Inode::new(
        0, 
        InodeTypes::EXT4_DE_DIR
    )) as Arc<dyn Inode>;
    let root_dentry =Ext4Dentry::new(
        dir, 
        None      
    );
    root_dentry.set_inode(root_inode);
    let superblock = Arc::new(Ext4SuperBlock::new(
        SuperBlockInner::new(
            //暂时假挂载
            Some(BLOCK_DEVICE.clone()),
            Some(root_dentry.clone() as Arc<dyn Dentry>)
        )
    ));

    Ok(superblock)
}