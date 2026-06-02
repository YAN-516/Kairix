use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use crate::error::SysResult;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::fstype::FsTypeInner;
use crate::fs::FsType;
use crate::fs::Dentry;
use crate::fs::MountFlags;
use crate::devices::BlockDevice;
use crate::fs::tmpfs::superblock::TempSuperBlock;
use crate::fs::SuperBlockInner;
use crate::fs::tmpfs::inode::TempInode;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::tmpfs::dentry::TempDentry;
use crate::fs::vfs::inode::inode_alloc;
use lazy_static::lazy_static;
use spin::mutex::Mutex;

lazy_static! {
    static ref PERSISTENT_DEVICE_ROOTS: Mutex<BTreeMap<String, Arc<dyn Dentry>>> =
        Mutex::new(BTreeMap::new());
}

fn clone_tmpfs_subtree(
    source: Arc<dyn Dentry>,
    parent: Option<Arc<dyn Dentry>>,
    name: &str,
) -> Arc<dyn Dentry> {
    let cloned = TempDentry::new(name, parent);
    if let Some(inode) = source.get_inode() {
        cloned.set_inode(inode);
    }
    for (child_name, child) in source.children() {
        let cloned_child = clone_tmpfs_subtree(child, Some(cloned.clone()), &child_name);
        cloned.add_child(cloned_child);
    }
    cloned
}

/// Return the stable tmpfs root used to emulate a formatted test mount.
///
/// Several LTP tests create files, unmount the test filesystem, then mount the
/// same device again and expect the files to still exist.  Kairix maps ext2/3/4
/// test mounts to tmpfs, so keep one in-memory tree per mount point to model
/// that persistence without leaking state between unrelated temporary mounts.
pub fn get_or_create_persistent_root(
    mount_key: &str,
    name: &str,
    parent: Option<Arc<dyn Dentry>>,
) -> Arc<dyn Dentry> {
    let mut roots = PERSISTENT_DEVICE_ROOTS.lock();
    if let Some(root) = roots.get(mount_key).cloned() {
        if root.name() == name {
            return root;
        }
        let cloned = clone_tmpfs_subtree(root, parent, name);
        roots.insert(mount_key.to_string(), cloned.clone());
        return cloned;
    }

    let root_inode = Arc::new(TempInode::new(InodeMode::DIR));
    let root_dentry = TempDentry::new(name, parent);
    root_dentry.set_inode(root_inode);
    roots.insert(mount_key.to_string(), root_dentry.clone());
    root_dentry
}

/// Check whether a tmpfs root belongs to a persisted device-backed mount.
pub fn is_persistent_device_root(root: &Arc<dyn Dentry>) -> bool {
    PERSISTENT_DEVICE_ROOTS
        .lock()
        .values()
        .any(|stored| Arc::ptr_eq(stored, root))
}

/// Drop emulated device-backed tmpfs roots after the backing block device is
/// rewritten, e.g. by mkfs during LTP filesystem matrix tests.
pub fn clear_persistent_device_roots() {
    PERSISTENT_DEVICE_ROOTS.lock().clear();
}

/// The temporary filesystem type
pub struct TempFsType {
    inner: FsTypeInner,
}

impl TempFsType {
    ///
    pub fn new(name: &str) -> Arc<Self> {
        Arc::new(Self {
            inner: FsTypeInner::new(name),
        })
    }
}

impl FsType for TempFsType {
    fn inner(&self) -> &FsTypeInner {
        &self.inner
    }
    fn mount(&self, name: &str, parent: Option<Arc<dyn Dentry>>, flags: MountFlags, dev: Option<Arc<dyn BlockDevice>>) -> SysResult<Arc<dyn Dentry>> {
        let root_inode = Arc::new(TempInode::new(InodeMode::DIR));
        let root_dentry = TempDentry::new(name, parent.clone());
        root_dentry.set_inode(root_inode);
        let superblock = Arc::new(TempSuperBlock::new(SuperBlockInner::new(dev, Some(root_dentry.clone()), flags)));
        GLOBAL_DCACHE.insert(root_dentry.path(), root_dentry.clone());
        GLOBAL_DCACHE.pin(root_dentry.path());
        self.add_sb(&root_dentry.path(), superblock.clone());
        Ok(root_dentry)
    }
    fn kill_sb(&self) -> isize {
        todo!()
    }
}
