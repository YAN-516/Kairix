use crate::fs::vfs::Dentry;
use crate::sync::SleepLock;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use lazy_static::lazy_static;

/// Dentry 缓存容量上限。LTP 会创建大量 /tmp/LTP_* 临时路径，容量太小会挤掉
/// /bin、/sbin、/lib 等热路径，导致后续 execve 反复回到底层文件系统扫目录。
const DCACHE_MAX_SIZE: usize = 32768;

/// Updating both LRU trees on every hit turns a read-mostly cache lookup into
/// four global tree mutations and two path allocations.  A hit still advances
/// the access generation, but an entry is repositioned at most once per this
/// many global accesses.  Entries that have become old are therefore refreshed
/// before eviction while repeatedly hot entries avoid redundant metadata work.
const DCACHE_LRU_TOUCH_INTERVAL: usize = 256;

/// LRU 元数据
struct LruMeta {
    /// generation -> path，按时间顺序找到最久未访问
    order: BTreeMap<usize, String>,
    /// path -> generation
    path_to_gen: BTreeMap<String, usize>,
    /// 单调递增访问计数器
    next_gen: usize,
    /// Sum of the path bytes retained by `order` (and `path_to_gen`).
    path_bytes: usize,
}

/// Dentry 缓存内部状态，合并到一把锁下
struct DentryCacheInner {
    dcache: BTreeMap<String, Arc<dyn Dentry>>,
    lru: LruMeta,
    pinned: BTreeSet<String>,
    path_bytes: usize,
    pinned_path_bytes: usize,
    tmp_entries: usize,
    tmp_path_bytes: usize,
    ltp_tmp_entries: usize,
    ltp_tmp_path_bytes: usize,
    path_length_counts: BTreeMap<usize, usize>,
    get_calls: usize,
    get_hits: usize,
    lru_touches: usize,
    lru_touch_skips: usize,
}

/// 带 LRU 淘汰和挂载点保护的 Dentry 缓存
pub struct DentryCache {
    /// Lookups update LRU metadata and subtree invalidation can be O(n), so
    /// contenders must sleep instead of spinning while an owner is preempted.
    inner: SleepLock<DentryCacheInner>,
    max_size: usize,
    get_lock_wait_ns: AtomicUsize,
    get_lock_wait_max_ns: AtomicUsize,
    get_lock_hold_ns: AtomicUsize,
    get_lock_hold_max_ns: AtomicUsize,
}

#[derive(Debug, Clone, Copy)]
pub struct DentryCacheStats {
    pub entries: usize,
    pub pinned: usize,
    pub lru_entries: usize,
    pub max_size: usize,
    pub path_bytes: usize,
    pub lru_path_bytes: usize,
    pub pinned_path_bytes: usize,
    pub tmp_entries: usize,
    pub tmp_path_bytes: usize,
    pub ltp_tmp_entries: usize,
    pub ltp_tmp_path_bytes: usize,
    pub max_path_len: usize,
    pub lock_busy: bool,
    pub get_calls: usize,
    pub get_hits: usize,
    pub lru_touches: usize,
    pub lru_touch_skips: usize,
    pub get_lock_wait_ns: usize,
    pub get_lock_wait_max_ns: usize,
    pub get_lock_hold_ns: usize,
    pub get_lock_hold_max_ns: usize,
}

impl DentryCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            inner: SleepLock::new_fair(DentryCacheInner {
                dcache: BTreeMap::new(),
                lru: LruMeta {
                    order: BTreeMap::new(),
                    path_to_gen: BTreeMap::new(),
                    next_gen: 0,
                    path_bytes: 0,
                },
                pinned: BTreeSet::new(),
                path_bytes: 0,
                pinned_path_bytes: 0,
                tmp_entries: 0,
                tmp_path_bytes: 0,
                ltp_tmp_entries: 0,
                ltp_tmp_path_bytes: 0,
                path_length_counts: BTreeMap::new(),
                get_calls: 0,
                get_hits: 0,
                lru_touches: 0,
                lru_touch_skips: 0,
            }),
            max_size,
            get_lock_wait_ns: AtomicUsize::new(0),
            get_lock_wait_max_ns: AtomicUsize::new(0),
            get_lock_hold_ns: AtomicUsize::new(0),
            get_lock_hold_max_ns: AtomicUsize::new(0),
        }
    }

    /// Return dentry-cache stats without blocking on the cache lock.
    pub fn try_stats(&self) -> DentryCacheStats {
        let Some(inner) = self.inner.try_lock() else {
            return DentryCacheStats {
                entries: 0,
                pinned: 0,
                lru_entries: 0,
                max_size: self.max_size,
                path_bytes: 0,
                lru_path_bytes: 0,
                pinned_path_bytes: 0,
                tmp_entries: 0,
                tmp_path_bytes: 0,
                ltp_tmp_entries: 0,
                ltp_tmp_path_bytes: 0,
                max_path_len: 0,
                lock_busy: true,
                get_calls: 0,
                get_hits: 0,
                lru_touches: 0,
                lru_touch_skips: 0,
                get_lock_wait_ns: self.get_lock_wait_ns.load(Ordering::Relaxed),
                get_lock_wait_max_ns: self.get_lock_wait_max_ns.load(Ordering::Relaxed),
                get_lock_hold_ns: self.get_lock_hold_ns.load(Ordering::Relaxed),
                get_lock_hold_max_ns: self.get_lock_hold_max_ns.load(Ordering::Relaxed),
            };
        };
        DentryCacheStats {
            entries: inner.dcache.len(),
            pinned: inner.pinned.len(),
            lru_entries: inner.lru.order.len(),
            max_size: self.max_size,
            path_bytes: inner.path_bytes,
            lru_path_bytes: inner.lru.path_bytes,
            pinned_path_bytes: inner.pinned_path_bytes,
            tmp_entries: inner.tmp_entries,
            tmp_path_bytes: inner.tmp_path_bytes,
            ltp_tmp_entries: inner.ltp_tmp_entries,
            ltp_tmp_path_bytes: inner.ltp_tmp_path_bytes,
            max_path_len: inner
                .path_length_counts
                .last_key_value()
                .map_or(0, |(&length, _)| length),
            lock_busy: false,
            get_calls: inner.get_calls,
            get_hits: inner.get_hits,
            lru_touches: inner.lru_touches,
            lru_touch_skips: inner.lru_touch_skips,
            get_lock_wait_ns: self.get_lock_wait_ns.load(Ordering::Relaxed),
            get_lock_wait_max_ns: self.get_lock_wait_max_ns.load(Ordering::Relaxed),
            get_lock_hold_ns: self.get_lock_hold_ns.load(Ordering::Relaxed),
            get_lock_hold_max_ns: self.get_lock_hold_max_ns.load(Ordering::Relaxed),
        }
    }

    fn add_path_stats(inner: &mut DentryCacheInner, path: &str) {
        let len = path.len();
        inner.path_bytes += len;
        *inner.path_length_counts.entry(len).or_insert(0) += 1;
        if path == "/tmp" || path.starts_with("/tmp/") {
            inner.tmp_entries += 1;
            inner.tmp_path_bytes += len;
            if path.starts_with("/tmp/LTP_") {
                inner.ltp_tmp_entries += 1;
                inner.ltp_tmp_path_bytes += len;
            }
        }
    }

    fn remove_path_stats(inner: &mut DentryCacheInner, path: &str) {
        let len = path.len();
        inner.path_bytes = inner.path_bytes.saturating_sub(len);
        let remove_length = if let Some(count) = inner.path_length_counts.get_mut(&len) {
            *count -= 1;
            *count == 0
        } else {
            false
        };
        if remove_length {
            inner.path_length_counts.remove(&len);
        }
        if path == "/tmp" || path.starts_with("/tmp/") {
            inner.tmp_entries = inner.tmp_entries.saturating_sub(1);
            inner.tmp_path_bytes = inner.tmp_path_bytes.saturating_sub(len);
            if path.starts_with("/tmp/LTP_") {
                inner.ltp_tmp_entries = inner.ltp_tmp_entries.saturating_sub(1);
                inner.ltp_tmp_path_bytes = inner.ltp_tmp_path_bytes.saturating_sub(len);
            }
        }
    }

    fn record_max(maximum: &AtomicUsize, value: usize) {
        let mut current = maximum.load(Ordering::Relaxed);
        while value > current {
            match maximum.compare_exchange_weak(
                current,
                value,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => current = observed,
            }
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
        if inner.dcache.remove(path).is_some() {
            Self::remove_path_stats(inner, path);
        }
        if inner.pinned.remove(path) {
            inner.pinned_path_bytes = inner.pinned_path_bytes.saturating_sub(path.len());
        }
        if let Some(g) = inner.lru.path_to_gen.remove(path) {
            inner.lru.order.remove(&g);
            inner.lru.path_bytes = inner.lru.path_bytes.saturating_sub(path.len());
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

    /// Mark a path as recently used, coalescing redundant hit-side updates.
    fn touch(inner: &mut DentryCacheInner, path: &str, force: bool) {
        let g = inner.lru.next_gen;
        inner.lru.next_gen = inner.lru.next_gen.wrapping_add(1);
        if !force
            && inner
                .lru
                .path_to_gen
                .get(path)
                .is_some_and(|old_gen| g.wrapping_sub(*old_gen) < DCACHE_LRU_TOUCH_INTERVAL)
        {
            inner.lru_touch_skips += 1;
            return;
        }
        if let Some(old_gen) = inner.lru.path_to_gen.remove(path) {
            inner.lru.order.remove(&old_gen);
        } else {
            inner.lru.path_bytes += path.len();
        }
        inner.lru.path_to_gen.insert(path.to_string(), g);
        inner.lru.order.insert(g, path.to_string());
        inner.lru_touches += 1;
    }

    /// 从缓存中获取 dentry，并更新 LRU 访问顺序
    pub fn get(&self, path: &str) -> Option<Arc<dyn Dentry>> {
        let lock_started_ns = polyhal::timer::current_time().as_nanos() as usize;
        let mut inner = self.inner.lock();
        let lock_acquired_ns = polyhal::timer::current_time().as_nanos() as usize;
        let wait_ns = lock_acquired_ns.saturating_sub(lock_started_ns);
        self.get_lock_wait_ns.fetch_add(wait_ns, Ordering::Relaxed);
        Self::record_max(&self.get_lock_wait_max_ns, wait_ns);
        inner.get_calls += 1;
        let res = inner.dcache.get(path).cloned();
        if res.is_some() {
            inner.get_hits += 1;
            Self::touch(&mut inner, path, false);
        }
        drop(inner);
        let hold_ns =
            (polyhal::timer::current_time().as_nanos() as usize).saturating_sub(lock_acquired_ns);
        self.get_lock_hold_ns.fetch_add(hold_ns, Ordering::Relaxed);
        Self::record_max(&self.get_lock_hold_max_ns, hold_ns);
        res
    }

    /// 插入 dentry。如果已存在则更新值并刷新 LRU；如果超容则淘汰最老的非 pinned 条目
    pub fn insert(&self, path: String, dentry: Arc<dyn Dentry>) {
        let mut inner = self.inner.lock();

        // 已存在：更新值 + 刷新 LRU 位置
        if inner.dcache.contains_key(&path) {
            inner.dcache.insert(path.clone(), dentry);
            Self::touch(&mut inner, &path, true);
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
                debug_assert_eq!(inner.lru.path_to_gen.get(&old_path), Some(&oldest_gen));
                Self::touch(&mut inner, &old_path, true);
                continue;
            }

            Self::remove_path_locked(&mut inner, &old_path);
        }

        inner.dcache.insert(path.clone(), dentry);
        Self::add_path_stats(&mut inner, &path);
        Self::touch(&mut inner, &path, true);
    }

    /// 从缓存中移除指定路径
    pub fn remove(&self, path: &str) {
        let mut inner = self.inner.lock();
        Self::remove_path_locked(&mut inner, path);
    }

    /// 将路径标记为 pinned（如挂载点），pinned 条目不会被 LRU 淘汰
    pub fn pin(&self, path: String) {
        let mut inner = self.inner.lock();
        let path_len = path.len();
        if inner.pinned.insert(path) {
            inner.pinned_path_bytes += path_len;
        }
    }

    /// 取消 pinned 标记
    pub fn unpin(&self, path: &str) {
        let mut inner = self.inner.lock();
        if inner.pinned.remove(path) {
            inner.pinned_path_bytes = inner.pinned_path_bytes.saturating_sub(path.len());
        }
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
