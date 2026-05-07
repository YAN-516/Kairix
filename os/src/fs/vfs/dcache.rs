use alloc::collections::BTreeMap;
use crate::sync::SpinLock;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use crate::fs::vfs::Dentry;
use lazy_static::lazy_static;

///the dentry cache, used to speed up the dentry lookup
pub struct DentryCache {
    dcache: SpinLock<BTreeMap<String, Arc<dyn Dentry>>>,
}

impl DentryCache {
    pub fn new() -> Self {
        Self {
            dcache: SpinLock::new(BTreeMap::new()),
        }
    }

    /// get the dentry from the cache, if not found, return None
    pub fn get(&self, path: &str) -> Option<Arc<dyn Dentry>> {
        self.dcache.lock().get(path).cloned()
    }

    /// insert a dentry into the cache
    pub fn insert(&self, path: String, dentry: Arc<dyn Dentry>) {
        self.dcache.lock().insert(path, dentry);
    }

    pub fn remove(&self, path: &str) {
        self.dcache.lock().remove(path);
    }

    /// pin is kept for API compatibility but no-op without LRU
    pub fn pin(&self, _path: String) {}

    /// unpin is kept for API compatibility but no-op without LRU
    pub fn unpin(&self, _path: &str) {}

    pub fn len(&self) -> usize {
        self.dcache.lock().len()
    }
}

// the global dentry cache, used to speed up the dentry lookup
lazy_static! {
    pub static ref GLOBAL_DCACHE: DentryCache = DentryCache::new();
}
