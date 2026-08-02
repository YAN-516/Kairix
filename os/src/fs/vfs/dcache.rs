use crate::fs::vfs::Dentry;
use crate::sync::SleepLock;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use lazy_static::lazy_static;

const DCACHE_MAX_SIZE: usize = 32768;
const DCACHE_SHARDS: usize = 64;
const DCACHE_LRU_TOUCH_INTERVAL: usize = 256;

struct LruMeta {
    order: BTreeMap<usize, String>,
    path_to_gen: BTreeMap<String, usize>,
    next_gen: usize,
    path_bytes: usize,
}

struct CachedDentry {
    dentry: Arc<dyn Dentry>,
    last_touch_access: usize,
}

struct DentryCacheShard {
    entries: BTreeMap<String, CachedDentry>,
}

struct DentryCacheMeta {
    lru: LruMeta,
    pinned: BTreeSet<String>,
    entries: usize,
    path_bytes: usize,
    pinned_path_bytes: usize,
    tmp_entries: usize,
    tmp_path_bytes: usize,
    ltp_tmp_entries: usize,
    ltp_tmp_path_bytes: usize,
    path_length_counts: BTreeMap<usize, usize>,
    lru_touches: usize,
}

/// Sharded dentry cache with globally ordered namespace mutations and LRU.
pub struct DentryCache {
    shards: [SleepLock<DentryCacheShard>; DCACHE_SHARDS],
    /// Keep insert/remove/subtree invalidation and capacity eviction ordered.
    mutation: SleepLock<()>,
    meta: SleepLock<DentryCacheMeta>,
    max_size: usize,
    get_calls: AtomicUsize,
    get_hits: AtomicUsize,
    lru_touch_skips: AtomicUsize,
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
            shards: core::array::from_fn(|_| {
                SleepLock::new(DentryCacheShard {
                    entries: BTreeMap::new(),
                })
            }),
            mutation: SleepLock::new_fair(()),
            meta: SleepLock::new_fair(DentryCacheMeta {
                lru: LruMeta {
                    order: BTreeMap::new(),
                    path_to_gen: BTreeMap::new(),
                    next_gen: 0,
                    path_bytes: 0,
                },
                pinned: BTreeSet::new(),
                entries: 0,
                path_bytes: 0,
                pinned_path_bytes: 0,
                tmp_entries: 0,
                tmp_path_bytes: 0,
                ltp_tmp_entries: 0,
                ltp_tmp_path_bytes: 0,
                path_length_counts: BTreeMap::new(),
                lru_touches: 0,
            }),
            max_size,
            get_calls: AtomicUsize::new(0),
            get_hits: AtomicUsize::new(0),
            lru_touch_skips: AtomicUsize::new(0),
            get_lock_wait_ns: AtomicUsize::new(0),
            get_lock_wait_max_ns: AtomicUsize::new(0),
            get_lock_hold_ns: AtomicUsize::new(0),
            get_lock_hold_max_ns: AtomicUsize::new(0),
        }
    }

    /// Return dentry-cache stats without blocking on mutation metadata.
    pub fn try_stats(&self) -> DentryCacheStats {
        let get_calls = self.get_calls.load(Ordering::Relaxed);
        let get_hits = self.get_hits.load(Ordering::Relaxed);
        let lru_touch_skips = self.lru_touch_skips.load(Ordering::Relaxed);
        let get_lock_wait_ns = self.get_lock_wait_ns.load(Ordering::Relaxed);
        let get_lock_wait_max_ns = self.get_lock_wait_max_ns.load(Ordering::Relaxed);
        let get_lock_hold_ns = self.get_lock_hold_ns.load(Ordering::Relaxed);
        let get_lock_hold_max_ns = self.get_lock_hold_max_ns.load(Ordering::Relaxed);
        let Some(meta) = self.meta.try_lock() else {
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
                get_calls,
                get_hits,
                lru_touches: 0,
                lru_touch_skips,
                get_lock_wait_ns,
                get_lock_wait_max_ns,
                get_lock_hold_ns,
                get_lock_hold_max_ns,
            };
        };
        DentryCacheStats {
            entries: meta.entries,
            pinned: meta.pinned.len(),
            lru_entries: meta.lru.order.len(),
            max_size: self.max_size,
            path_bytes: meta.path_bytes,
            lru_path_bytes: meta.lru.path_bytes,
            pinned_path_bytes: meta.pinned_path_bytes,
            tmp_entries: meta.tmp_entries,
            tmp_path_bytes: meta.tmp_path_bytes,
            ltp_tmp_entries: meta.ltp_tmp_entries,
            ltp_tmp_path_bytes: meta.ltp_tmp_path_bytes,
            max_path_len: meta
                .path_length_counts
                .last_key_value()
                .map_or(0, |(&length, _)| length),
            lock_busy: false,
            get_calls,
            get_hits,
            lru_touches: meta.lru_touches,
            lru_touch_skips,
            get_lock_wait_ns,
            get_lock_wait_max_ns,
            get_lock_hold_ns,
            get_lock_hold_max_ns,
        }
    }

    #[inline]
    fn shard_index(path: &str) -> usize {
        let hash = path
            .as_bytes()
            .iter()
            .fold(0xcbf2_9ce4_8422_2325usize, |hash, byte| {
                (hash ^ (*byte as usize)).wrapping_mul(0x100_0000_01b3)
            });
        (hash ^ (hash >> 17) ^ (hash >> 31)) & (DCACHE_SHARDS - 1)
    }

    fn add_path_stats(meta: &mut DentryCacheMeta, path: &str) {
        let len = path.len();
        meta.entries += 1;
        meta.path_bytes += len;
        *meta.path_length_counts.entry(len).or_insert(0) += 1;
        if path == "/tmp" || path.starts_with("/tmp/") {
            meta.tmp_entries += 1;
            meta.tmp_path_bytes += len;
            if path.starts_with("/tmp/LTP_") {
                meta.ltp_tmp_entries += 1;
                meta.ltp_tmp_path_bytes += len;
            }
        }
    }

    fn remove_path_stats(meta: &mut DentryCacheMeta, path: &str) {
        let len = path.len();
        meta.entries = meta.entries.saturating_sub(1);
        meta.path_bytes = meta.path_bytes.saturating_sub(len);
        let remove_length = if let Some(count) = meta.path_length_counts.get_mut(&len) {
            *count -= 1;
            *count == 0
        } else {
            false
        };
        if remove_length {
            meta.path_length_counts.remove(&len);
        }
        if path == "/tmp" || path.starts_with("/tmp/") {
            meta.tmp_entries = meta.tmp_entries.saturating_sub(1);
            meta.tmp_path_bytes = meta.tmp_path_bytes.saturating_sub(len);
            if path.starts_with("/tmp/LTP_") {
                meta.ltp_tmp_entries = meta.ltp_tmp_entries.saturating_sub(1);
                meta.ltp_tmp_path_bytes = meta.ltp_tmp_path_bytes.saturating_sub(len);
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

    fn matching_paths(shard: &DentryCacheShard, prefix: &str) -> alloc::vec::Vec<String> {
        let start = prefix.to_string();
        if let Some(end) = Self::prefix_upper_bound(prefix) {
            shard
                .entries
                .range(start..end)
                .map(|(path, _)| path.clone())
                .collect()
        } else {
            shard
                .entries
                .range(start..)
                .filter(|(path, _)| path.starts_with(prefix))
                .map(|(path, _)| path.clone())
                .collect()
        }
    }

    fn touch(meta: &mut DentryCacheMeta, path: &str) {
        let generation = meta.lru.next_gen;
        meta.lru.next_gen = meta.lru.next_gen.wrapping_add(1);
        if let Some(old_generation) = meta.lru.path_to_gen.remove(path) {
            meta.lru.order.remove(&old_generation);
        } else {
            meta.lru.path_bytes += path.len();
        }
        meta.lru.path_to_gen.insert(path.to_string(), generation);
        meta.lru.order.insert(generation, path.to_string());
        meta.lru_touches += 1;
    }

    fn remove_path_locked(
        shard: &mut DentryCacheShard,
        meta: &mut DentryCacheMeta,
        path: &str,
    ) -> bool {
        let removed = shard.entries.remove(path).is_some();
        if removed {
            Self::remove_path_stats(meta, path);
        }
        if meta.pinned.remove(path) {
            meta.pinned_path_bytes = meta.pinned_path_bytes.saturating_sub(path.len());
        }
        if let Some(generation) = meta.lru.path_to_gen.remove(path) {
            meta.lru.order.remove(&generation);
            meta.lru.path_bytes = meta.lru.path_bytes.saturating_sub(path.len());
        }
        removed
    }

    fn remove_path_under_mutation(&self, path: &str) {
        let index = Self::shard_index(path);
        let mut shard = self.shards[index].lock();
        let mut meta = self.meta.lock();
        Self::remove_path_locked(&mut shard, &mut meta, path);
    }

    fn remove_prefix_under_mutation(&self, prefix: &str) {
        for shard_lock in &self.shards {
            let mut shard = shard_lock.lock();
            let paths = Self::matching_paths(&shard, prefix);
            if paths.is_empty() {
                continue;
            }
            let mut meta = self.meta.lock();
            for path in paths {
                Self::remove_path_locked(&mut shard, &mut meta, &path);
            }
        }
    }

    fn evict_one(&self) -> bool {
        let candidate = {
            let meta = self.meta.lock();
            meta.lru
                .order
                .iter()
                .find(|(_, path)| !meta.pinned.contains(path.as_str()))
                .map(|(generation, path)| (*generation, path.clone()))
        };
        let Some((candidate_generation, candidate)) = candidate else {
            return false;
        };
        let index = Self::shard_index(&candidate);
        let mut shard = self.shards[index].lock();
        let mut meta = self.meta.lock();
        if meta.pinned.contains(&candidate) {
            return true;
        }
        if meta.lru.path_to_gen.get(&candidate).copied() != Some(candidate_generation) {
            // A concurrent lookup refreshed this entry after candidate
            // selection. Retry capacity enforcement with the new oldest path.
            return true;
        }
        Self::remove_path_locked(&mut shard, &mut meta, &candidate);
        true
    }

    /// The common lookup path locks one hash shard. Global LRU metadata is
    /// touched only after this entry has aged by the configured interval.
    pub fn get(&self, path: &str) -> Option<Arc<dyn Dentry>> {
        let access = self.get_calls.fetch_add(1, Ordering::Relaxed);
        let lock_started_ns = polyhal::timer::current_time().as_nanos() as usize;
        let index = Self::shard_index(path);
        let mut shard = self.shards[index].lock();
        let lock_acquired_ns = polyhal::timer::current_time().as_nanos() as usize;
        let wait_ns = lock_acquired_ns.saturating_sub(lock_started_ns);
        self.get_lock_wait_ns.fetch_add(wait_ns, Ordering::Relaxed);
        Self::record_max(&self.get_lock_wait_max_ns, wait_ns);

        let mut should_touch = false;
        let result = shard.entries.get_mut(path).map(|entry| {
            self.get_hits.fetch_add(1, Ordering::Relaxed);
            if access.wrapping_sub(entry.last_touch_access) >= DCACHE_LRU_TOUCH_INTERVAL {
                entry.last_touch_access = access;
                should_touch = true;
            } else {
                self.lru_touch_skips.fetch_add(1, Ordering::Relaxed);
            }
            entry.dentry.clone()
        });
        if should_touch {
            Self::touch(&mut self.meta.lock(), path);
        }
        drop(shard);

        let hold_ns =
            (polyhal::timer::current_time().as_nanos() as usize).saturating_sub(lock_acquired_ns);
        self.get_lock_hold_ns.fetch_add(hold_ns, Ordering::Relaxed);
        Self::record_max(&self.get_lock_hold_max_ns, hold_ns);
        result
    }

    pub fn insert(&self, path: String, dentry: Arc<dyn Dentry>) {
        let _mutation = self.mutation.lock();
        let index = Self::shard_index(&path);
        let access = self.get_calls.load(Ordering::Relaxed);

        {
            let mut shard = self.shards[index].lock();
            if shard.entries.contains_key(&path) {
                shard.entries.insert(path.clone(), CachedDentry {
                    dentry,
                    last_touch_access: access,
                });
                Self::touch(&mut self.meta.lock(), &path);
                return;
            }
        }

        while self.meta.lock().entries >= self.max_size {
            if !self.evict_one() {
                break;
            }
        }

        let mut shard = self.shards[index].lock();
        let mut meta = self.meta.lock();
        shard.entries.insert(path.clone(), CachedDentry {
            dentry,
            last_touch_access: access,
        });
        Self::add_path_stats(&mut meta, &path);
        Self::touch(&mut meta, &path);
    }

    pub fn remove(&self, path: &str) {
        let _mutation = self.mutation.lock();
        self.remove_path_under_mutation(path);
    }

    pub fn pin(&self, path: String) {
        let _mutation = self.mutation.lock();
        let path_len = path.len();
        let mut meta = self.meta.lock();
        if meta.pinned.insert(path) {
            meta.pinned_path_bytes += path_len;
        }
    }

    pub fn unpin(&self, path: &str) {
        let _mutation = self.mutation.lock();
        let mut meta = self.meta.lock();
        if meta.pinned.remove(path) {
            meta.pinned_path_bytes = meta.pinned_path_bytes.saturating_sub(path.len());
        }
    }

    pub fn remove_prefix(&self, prefix: &str) {
        let _mutation = self.mutation.lock();
        self.remove_prefix_under_mutation(prefix);
    }

    pub fn remove_subtree(&self, root: &str) {
        let _mutation = self.mutation.lock();
        self.remove_path_under_mutation(root);
        if root != "/" {
            self.remove_prefix_under_mutation(&alloc::format!("{}/", root));
        }
    }

    #[allow(unused)]
    pub fn len(&self) -> usize {
        self.meta.lock().entries
    }
}

lazy_static! {
    pub static ref GLOBAL_DCACHE: DentryCache = DentryCache::new(DCACHE_MAX_SIZE);
}
