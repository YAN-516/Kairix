use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use crate::sync::SpinLock;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use crate::fs::vfs::Dentry;
use lazy_static::lazy_static;

/// Dentry 缓存容量上限
const DCACHE_MAX_SIZE: usize = 8192;

/// 带 LRU 淘汰和挂载点保护的 Dentry 缓存
pub struct DentryCache {
    dcache: SpinLock<BTreeMap<String, Arc<dyn Dentry>>>,
    lru: SpinLock<VecDeque<String>>,
    pinned: SpinLock<BTreeSet<String>>,
    max_size: usize,
}

impl DentryCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            dcache: SpinLock::new(BTreeMap::new()),
            lru: SpinLock::new(VecDeque::new()),
            pinned: SpinLock::new(BTreeSet::new()),
            max_size,
        }
    }

    /// 从缓存中获取 dentry，并更新 LRU 访问顺序
    pub fn get(&self, path: &str) -> Option<Arc<dyn Dentry>> {
        let res = self.dcache.lock().get(path).cloned();
        if res.is_some() {
            let mut lru = self.lru.lock();
            if let Some(pos) = lru.iter().position(|p| p == path) {
                let p = lru.remove(pos).unwrap();
                lru.push_back(p);
            }
        }
        res
    }

    /// 插入 dentry。如果已存在则更新值并刷新 LRU；如果超容则淘汰最老的非 pinned 条目
    pub fn insert(&self, path: String, dentry: Arc<dyn Dentry>) {
        let mut cache = self.dcache.lock();
        let mut lru = self.lru.lock();

        // 已存在：更新值 + 刷新 LRU 位置
        if cache.contains_key(&path) {
            cache.insert(path.clone(), dentry);
            if let Some(pos) = lru.iter().position(|p| p == &path) {
                let p = lru.remove(pos).unwrap();
                lru.push_back(p);
            }
            return;
        }

        // 新条目：超容时淘汰最老的非 pinned 条目
        let mut skipped = 0;
        while cache.len() >= self.max_size {
            if let Some(old) = lru.pop_front() {
                if self.pinned.lock().contains(&old) {
                    lru.push_back(old);
                    skipped += 1;
                    // 如果绕了一圈全是 pinned，放弃淘汰，允许临时超容
                    if skipped >= lru.len() {
                        break;
                    }
                    continue;
                }
                cache.remove(&old);
            } else {
                break;
            }
        }

        cache.insert(path.clone(), dentry);
        lru.push_back(path);
    }

    /// 从缓存中移除指定路径
    pub fn remove(&self, path: &str) {
        let mut cache = self.dcache.lock();
        let mut lru = self.lru.lock();
        cache.remove(path);
        if let Some(pos) = lru.iter().position(|p| p == path) {
            lru.remove(pos);
        }
    }

    /// 将路径标记为 pinned（如挂载点），pinned 条目不会被 LRU 淘汰
    pub fn pin(&self, path: String) {
        self.pinned.lock().insert(path);
    }

    /// 取消 pinned 标记
    pub fn unpin(&self, path: &str) {
        self.pinned.lock().remove(path);
    }

    /// 当前缓存条目数（调试用）
    #[allow(unused)]
    pub fn len(&self) -> usize {
        self.dcache.lock().len()
    }
}

lazy_static! {
    pub static ref GLOBAL_DCACHE: DentryCache = DentryCache::new(DCACHE_MAX_SIZE);
}
