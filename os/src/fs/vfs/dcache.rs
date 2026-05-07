use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use crate::sync::SpinLock;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use crate::fs::vfs::Dentry;
use lazy_static::lazy_static;

/// Dentry 缓存容量上限
const DCACHE_MAX_SIZE: usize = 8192;
/// 负缓存容量上限
const NEGATIVE_CACHE_MAX_SIZE: usize = 1024;

/// 带 LRU 淘汰、挂载点保护和负缓存的 Dentry 缓存
pub struct DentryCache {
    dcache: SpinLock<BTreeMap<String, Arc<dyn Dentry>>>,
    /// 负缓存队列：最近确认不存在的路径
    negative: SpinLock<VecDeque<String>>,
    /// 负缓存快速查找集合
    negative_set: SpinLock<BTreeSet<String>>,
    lru: SpinLock<VecDeque<String>>,
    pinned: SpinLock<BTreeSet<String>>,
    max_size: usize,
}

impl DentryCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            dcache: SpinLock::new(BTreeMap::new()),
            negative: SpinLock::new(VecDeque::new()),
            negative_set: SpinLock::new(BTreeSet::new()),
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

    /// 检查路径是否在负缓存中（最近解析失败，文件不存在）
    pub fn is_negative(&self, path: &str) -> bool {
        self.negative_set.lock().contains(path)
    }

    /// 插入负缓存条目（路径不存在）
    pub fn insert_negative(&self, path: String) {
        let mut set = self.negative_set.lock();
        let mut queue = self.negative.lock();
        if set.contains(&path) {
            // 已存在，刷新到尾部
            if let Some(pos) = queue.iter().position(|p| p == &path) {
                let p = queue.remove(pos).unwrap();
                queue.push_back(p);
            }
            return;
        }
        while queue.len() >= NEGATIVE_CACHE_MAX_SIZE {
            if let Some(old) = queue.pop_front() {
                set.remove(&old);
            } else {
                break;
            }
        }
        set.insert(path.clone());
        queue.push_back(path);
    }

    /// 从负缓存中移除（创建/删除文件时失效）
    fn remove_negative(&self, path: &str) {
        let mut set = self.negative_set.lock();
        let mut queue = self.negative.lock();
        if set.remove(path) {
            if let Some(pos) = queue.iter().position(|p| p == path) {
                queue.remove(pos);
            }
        }
    }

    /// 插入 dentry。如果已存在则更新值并刷新 LRU；如果超容则淘汰最老的非 pinned 条目
    pub fn insert(&self, path: String, dentry: Arc<dyn Dentry>) {
        // 新路径创建成功后，失效可能存在的负缓存
        self.remove_negative(&path);

        let mut cache = self.dcache.lock();
        let mut lru = self.lru.lock();
        if cache.contains_key(&path) {
            cache.insert(path.clone(), dentry);
            if let Some(pos) = lru.iter().position(|p| p == &path) {
                let p = lru.remove(pos).unwrap();
                lru.push_back(p);
            }
            return;
        }
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
        self.remove_negative(path);
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

    #[allow(unused)]
    pub fn len(&self) -> usize {
        self.dcache.lock().len()
    }
}

lazy_static! {
    pub static ref GLOBAL_DCACHE: DentryCache = DentryCache::new(DCACHE_MAX_SIZE);
}
