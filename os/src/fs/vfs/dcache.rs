use crate::fs::vfs::Dentry;
use crate::sync::SpinLock;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use lazy_static::lazy_static;

/// Dentry 缓存容量上限。LTP 会创建大量 /tmp/LTP_* 临时路径，容量太小会挤掉
/// /bin、/sbin、/lib 等热路径，导致后续 execve 反复回到底层文件系统扫目录。
const DCACHE_MAX_SIZE: usize = 32768;

/// LRU 元数据
struct LruMeta {
    /// generation -> path，按时间顺序找到最久未访问
    order: BTreeMap<usize, String>,
    /// path -> generation
    path_to_gen: BTreeMap<String, usize>,
    /// 单调递增访问计数器
    next_gen: usize,
}

/// Dentry 缓存内部状态，合并到一把锁下
struct DentryCacheInner {
    dcache: BTreeMap<String, Arc<dyn Dentry>>,
    lru: LruMeta,
    pinned: BTreeSet<String>,
}

/// 带 LRU 淘汰和挂载点保护的 Dentry 缓存
pub struct DentryCache {
    inner: SpinLock<DentryCacheInner>,
    max_size: usize,
}

impl DentryCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            inner: SpinLock::new(DentryCacheInner {
                dcache: BTreeMap::new(),
                lru: LruMeta {
                    order: BTreeMap::new(),
                    path_to_gen: BTreeMap::new(),
                    next_gen: 0,
                },
                pinned: BTreeSet::new(),
            }),
            max_size,
        }
    }

    /// Return the smallest string greater than every key with `prefix`.
    fn prefix_upper_bound(prefix: &str) -> Option<String> {
        let mut bytes = prefix.as_bytes().to_vec();
        for idx in (0..bytes.len()).rev() {
            if bytes[idx] != u8::MAX {
                bytes[idx] += 1;
                bytes.truncate(idx + 1);
                return String::from_utf8(bytes).ok();
            }
        }
        None
    }

    fn remove_path_locked(inner: &mut DentryCacheInner, path: &str) {
        inner.dcache.remove(path);
        inner.pinned.remove(path);
        if let Some(g) = inner.lru.path_to_gen.remove(path) {
            inner.lru.order.remove(&g);
        }
    }

    fn remove_prefix_locked(inner: &mut DentryCacheInner, prefix: &str) {
        let start = prefix.to_string();
        let to_remove: alloc::vec::Vec<String> = if let Some(end) = Self::prefix_upper_bound(prefix)
        {
            inner
                .dcache
                .range(start..end)
                .map(|(path, _)| path.clone())
                .collect()
        } else {
            inner
                .dcache
                .range(start..)
                .filter(|(path, _)| path.starts_with(prefix))
                .map(|(path, _)| path.clone())
                .collect()
        };
        for path in to_remove {
            Self::remove_path_locked(inner, &path);
        }
    }

    /// 将 path 标记为最近访问（O(log n)）
    fn touch(inner: &mut DentryCacheInner, path: &str) {
        if let Some(old_gen) = inner.lru.path_to_gen.remove(path) {
            inner.lru.order.remove(&old_gen);
        }
        let g = inner.lru.next_gen;
        inner.lru.next_gen += 1;
        inner.lru.path_to_gen.insert(path.to_string(), g);
        inner.lru.order.insert(g, path.to_string());
    }

    /// 从缓存中获取 dentry，并更新 LRU 访问顺序
    pub fn get(&self, path: &str) -> Option<Arc<dyn Dentry>> {
        let mut inner = self.inner.lock();
        let res = inner.dcache.get(path).cloned();
        if res.is_some() {
            Self::touch(&mut inner, path);
        }
        res
    }

    /// 插入 dentry。如果已存在则更新值并刷新 LRU；如果超容则淘汰最老的非 pinned 条目
    pub fn insert(&self, path: String, dentry: Arc<dyn Dentry>) {
        let mut inner = self.inner.lock();

        // 已存在：更新值 + 刷新 LRU 位置
        if inner.dcache.contains_key(&path) {
            inner.dcache.insert(path.clone(), dentry);
            Self::touch(&mut inner, &path);
            return;
        }

        // 新条目：超容时淘汰最老的非 pinned 条目
        while inner.dcache.len() >= self.max_size {
            let Some((&oldest_gen, old_path)) = inner.lru.order.first_key_value() else {
                break;
            };
            let old_path = old_path.clone();

            if inner.pinned.contains(&old_path) {
                // pinned 条目给第二次机会：移到最新位置
                inner.lru.order.remove(&oldest_gen);
                let g = inner.lru.next_gen;
                inner.lru.next_gen += 1;
                inner.lru.order.insert(g, old_path.clone());
                inner.lru.path_to_gen.insert(old_path, g);
                continue;
            }

            inner.lru.order.remove(&oldest_gen);
            inner.lru.path_to_gen.remove(&old_path);
            inner.dcache.remove(&old_path);
        }

        inner.dcache.insert(path.clone(), dentry);
        Self::touch(&mut inner, &path);
    }

    /// 从缓存中移除指定路径
    pub fn remove(&self, path: &str) {
        let mut inner = self.inner.lock();
        Self::remove_path_locked(&mut inner, path);
    }

    /// 将路径标记为 pinned（如挂载点），pinned 条目不会被 LRU 淘汰
    pub fn pin(&self, path: String) {
        self.inner.lock().pinned.insert(path);
    }

    /// 取消 pinned 标记
    pub fn unpin(&self, path: &str) {
        self.inner.lock().pinned.remove(path);
    }

    /// 移除所有以给定前缀开头的缓存条目
    pub fn remove_prefix(&self, prefix: &str) {
        let mut inner = self.inner.lock();
        Self::remove_prefix_locked(&mut inner, prefix);
    }

    /// 移除挂载点及其子树的缓存条目，并同步取消 pinned 标记。
    pub fn remove_subtree(&self, root: &str) {
        let mut inner = self.inner.lock();
        Self::remove_path_locked(&mut inner, root);
        if root != "/" {
            Self::remove_prefix_locked(&mut inner, &alloc::format!("{}/", root));
        }
    }

    /// 当前缓存条目数（调试用）
    #[allow(unused)]
    pub fn len(&self) -> usize {
        self.inner.lock().dcache.len()
    }
}

lazy_static! {
    pub static ref GLOBAL_DCACHE: DentryCache = DentryCache::new(DCACHE_MAX_SIZE);
}
