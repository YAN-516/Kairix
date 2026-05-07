use alloc::collections::{BTreeMap, BTreeSet};
use crate::sync::SpinLock;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use crate::fs::vfs::Dentry;
use lazy_static::lazy_static;

/// Dentry 缓存容量上限
const DCACHE_MAX_SIZE: usize = 8192;

/// LRU 元数据，合并到一把锁下减少锁竞争
struct LruMeta {
    /// generation -> path，按时间顺序找到最久未访问
    order: BTreeMap<usize, String>,
    /// path -> generation
    path_to_gen: BTreeMap<String, usize>,
    /// 单调递增访问计数器
    next_gen: usize,
}

/// 带 LRU 淘汰和挂载点保护的 Dentry 缓存
pub struct DentryCache {
    dcache: SpinLock<BTreeMap<String, Arc<dyn Dentry>>>,
    lru: SpinLock<LruMeta>,
    pinned: SpinLock<BTreeSet<String>>,
    max_size: usize,
}

impl DentryCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            dcache: SpinLock::new(BTreeMap::new()),
            lru: SpinLock::new(LruMeta {
                order: BTreeMap::new(),
                path_to_gen: BTreeMap::new(),
                next_gen: 0,
            }),
            pinned: SpinLock::new(BTreeSet::new()),
            max_size,
        }
    }

    /// 将 path 标记为最近访问（O(log n)）
    fn touch(&self, path: &str) {
        let mut lru = self.lru.lock();
        if let Some(old_gen) = lru.path_to_gen.remove(path) {
            lru.order.remove(&old_gen);
        }
        let g = lru.next_gen;
        lru.next_gen += 1;
        lru.path_to_gen.insert(path.to_string(), g);
        lru.order.insert(g, path.to_string());
    }

    /// 从缓存中获取 dentry，并更新 LRU 访问顺序
    pub fn get(&self, path: &str) -> Option<Arc<dyn Dentry>> {
        let res = self.dcache.lock().get(path).cloned();
        if res.is_some() {
            self.touch(path);
        }
        res
    }

    /// 插入 dentry。如果已存在则更新值并刷新 LRU；如果超容则淘汰最老的非 pinned 条目
    pub fn insert(&self, path: String, dentry: Arc<dyn Dentry>) {
        let mut cache = self.dcache.lock();

        // 已存在：更新值 + 刷新 LRU 位置
        if cache.contains_key(&path) {
            cache.insert(path.clone(), dentry);
            drop(cache);
            self.touch(&path);
            return;
        }

        // 新条目：超容时淘汰最老的非 pinned 条目
        while cache.len() >= self.max_size {
            let mut lru = self.lru.lock();
            let pinned = self.pinned.lock();

            let Some((&oldest_gen, old_path)) = lru.order.first_key_value() else {
                drop(lru);
                drop(pinned);
                break;
            };
            let old_path = old_path.clone();

            if pinned.contains(&old_path) {
                // pinned 条目给第二次机会：移到最新位置
                drop(pinned);
                lru.order.remove(&oldest_gen);
                let g = lru.next_gen;
                lru.next_gen += 1;
                lru.order.insert(g, old_path.clone());
                lru.path_to_gen.insert(old_path, g);
                drop(lru);
                continue;
            }

            drop(pinned);
            lru.order.remove(&oldest_gen);
            lru.path_to_gen.remove(&old_path);
            drop(lru);
            cache.remove(&old_path);
        }

        cache.insert(path.clone(), dentry);
        drop(cache);
        self.touch(&path);
    }

    /// 从缓存中移除指定路径
    pub fn remove(&self, path: &str) {
        let mut cache = self.dcache.lock();
        cache.remove(path);
        drop(cache);

        let mut lru = self.lru.lock();
        if let Some(g) = lru.path_to_gen.remove(path) {
            lru.order.remove(&g);
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
