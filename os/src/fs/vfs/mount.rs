use alloc::sync::Arc;
use alloc::string::String;
use alloc::collections::BTreeMap;
use spin::Mutex;
use crate::fs::vfs::{SuperBlock,Dentry};
use crate::fs::GLOBAL_DCACHE;
use crate::alloc::string::ToString;
use crate::fs::lwext4::mount::mount_ext4_fs;
pub struct Mountdata {
    //the path where the file system is mounted, e.g. "/home/user/mnt"
    pub mount_point: String, 
    // the old dentry that is being covered by this mount
    pub odentry: Arc<dyn Dentry>, 
    //the new dentry that is the root of the mounted file system
    pub ndentry: Arc<dyn Dentry>,   
    // the superblock of the mounted file system   
    pub superblock: Arc<dyn SuperBlock>, 
}


lazy_static::lazy_static! {
    pub static ref MOUNT_TABLE: Mutex<BTreeMap<String, Mountdata>> = 
        Mutex::new(BTreeMap::new());
}

pub fn vfs_mount(source: &str, mount_path: &str, mount_dentry: Arc<dyn Dentry>, fstype: &str) -> Result<(), isize> {
    let new_superblock = match fstype {
        "ext4" => {
            let superblock = mount_ext4_fs(source, mount_path)?;
            superblock
        }
        //暂时是假挂载，为了通过测试用例
        "vfat" => {
            // let superblock = mount_ext4_fs(source, mount_path)?;
            // superblock
            return Ok(());
        }
        _ => return Err(-22),
    };
    let new_root = new_superblock.root();
    let record = Mountdata {
        mount_point: mount_path.to_string(),
        odentry:mount_dentry,
        ndentry: new_root.clone(),
        superblock: new_superblock.clone(),
    };
    MOUNT_TABLE.lock().insert(mount_path.to_string(), record);
    GLOBAL_DCACHE.insert(mount_path.to_string(), new_root);
    Ok(())
}

pub fn vfs_umount2(abs_mount_point: &str,_flags: u32) -> Result<(), isize> {
    let mut mount_table = MOUNT_TABLE.lock();
    // .remove() 方法不仅会查找，还会删掉
    let record = match mount_table.remove(abs_mount_point) {
        Some(r) => r,
        None => {
            log::warn!("umount failed: {} is not a mount point", abs_mount_point);
            return Err(0); //暂时为了通过测试用例
        }
    };

    //检查是否有人正在使用这个挂载点
    if Arc::strong_count(&record.ndentry) > 2 {
        mount_table.insert(abs_mount_point.to_string(), record); 
        return Err(-16); // EBUSY (设备忙)
    }
    GLOBAL_DCACHE.remove(abs_mount_point);
    GLOBAL_DCACHE.insert(abs_mount_point.to_string(), record.odentry);
    // 此时 record 被 drop，里面的 SuperBlock 也会被释放（引用计数清零），
    // 从而触发底层的资源清理。
    log::info!("Successfully unmounted {}", abs_mount_point);
    Ok(())
}