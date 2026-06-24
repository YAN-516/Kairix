use crate::sync::SleepLock;
#[deny(unused_doc_comments)]
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use polyhal::common::FrameTracker;
use spin::RwLock;

/// 磁盘文件系统页缓存最大页数（4096 页 ≈ 16MB）
pub const MAX_DISK_PAGE_CACHE_PAGES: usize = 4096;
/// Backward-compatible name for the disk-backed page-cache limit.
pub const MAX_PAGE_CACHE_PAGES: usize = MAX_DISK_PAGE_CACHE_PAGES;
/// Page cache namespace tag for tmpfs inodes.
pub const PAGE_CACHE_FS_TMPFS: usize = 1;
/// Page cache namespace tag for FAT32 inodes.
pub const PAGE_CACHE_FS_FAT32: usize = 2;
/// Page cache namespace tag for lwext4-backed inodes.
pub const PAGE_CACHE_FS_EXT4: usize = 3;

const PAGE_CACHE_TAG_SHIFT: usize = 60;
const PAGE_CACHE_INODE_MASK: usize = (1usize << PAGE_CACHE_TAG_SHIFT) - 1;

/// Combine a filesystem namespace tag with an inode number for page-cache keys.
pub fn tagged_inode_id(fs_tag: usize, inode_id: usize) -> usize {
    (fs_tag << PAGE_CACHE_TAG_SHIFT) | (inode_id & PAGE_CACHE_INODE_MASK)
}

/// Return the filesystem namespace tag carried by a page-cache inode id.
pub fn page_cache_fs_tag(inode_id: usize) -> usize {
    inode_id >> PAGE_CACHE_TAG_SHIFT
}

/// Return whether an inode id belongs to a disk-backed filesystem cache.
pub fn is_disk_backed_cache_id(inode_id: usize) -> bool {
    matches!(
        page_cache_fs_tag(inode_id),
        PAGE_CACHE_FS_FAT32 | PAGE_CACHE_FS_EXT4
    )
}

lazy_static! {
    ///
    pub static ref PAGE_CACHE: SleepLock<PageCache> = SleepLock::new(PageCache::new());
}

///
pub struct Page {
    ///
    pub frame: Arc<FrameTracker>,
    ///
    pub dirty: bool, //脏页标记！
}

impl Page {
    ///
    pub fn new(frame: Arc<FrameTracker>) -> Self {
        Self {
            frame,
            dirty: false,
        }
    }

    /// 往缓存页的指定偏移处写入数据，并自动标为脏页
    pub fn modify(&mut self, page_offset: usize, data: &[u8]) {
        let dst_buffer =
            &mut self.frame.ppn.get_bytes_array()[page_offset..page_offset + data.len()];
        dst_buffer.copy_from_slice(data);
        self.dirty = true;
    }
}

///
pub struct PageCache {
    // Key: (inode_id, page_id)
    // Value: 使用单独的 RwLock 保护每一个页，细化锁粒度！
    cache: BTreeMap<(usize, usize), Arc<RwLock<Page>>>,
    /// generation -> key，用于按时间顺序找到最久未访问的页
    lru_order: BTreeMap<usize, (usize, usize)>,
    /// key -> generation，用于 O(log n) 更新访问顺序
    lru_gen: BTreeMap<(usize, usize), usize>,
    /// 单调递增的访问计数器
    next_gen: usize,
    /// 磁盘文件系统页缓存最大页数
    max_disk_pages: usize,
    /// 当前磁盘文件系统缓存页数
    disk_pages: usize,
}

impl PageCache {
    ///
    pub fn new() -> Self {
        Self {
            cache: BTreeMap::new(),
            lru_order: BTreeMap::new(),
            lru_gen: BTreeMap::new(),
            next_gen: 0,
            max_disk_pages: MAX_DISK_PAGE_CACHE_PAGES,
            disk_pages: 0,
        }
    }

    /// 更新 key 的 LRU 时间戳到最新
    fn touch(&mut self, key: (usize, usize)) {
        if let Some(old_gen) = self.lru_gen.remove(&key) {
            self.lru_order.remove(&old_gen);
        }
        let g = self.next_gen;
        self.next_gen += 1;
        self.lru_gen.insert(key, g);
        self.lru_order.insert(g, key);
    }

    /// 从缓存和 LRU 元数据中移除一页。
    fn remove_key(&mut self, key: (usize, usize)) -> Option<Arc<RwLock<Page>>> {
        let removed = self.cache.remove(&key);
        if removed.is_some() && is_disk_backed_cache_id(key.0) {
            self.disk_pages = self.disk_pages.saturating_sub(1);
        }
        if let Some(g) = self.lru_gen.remove(&key) {
            self.lru_order.remove(&g);
        }
        removed
    }

    /// 把暂时不能回收的 LRU 项移到队尾。
    fn rotate_lru_entry(&mut self, old_gen: usize, key: (usize, usize)) {
        self.lru_order.remove(&old_gen);
        let new_gen = self.next_gen;
        self.next_gen += 1;
        self.lru_gen.insert(key, new_gen);
        self.lru_order.insert(new_gen, key);
    }

    /// 尝试淘汰一个磁盘文件系统干净页；脏页和 tmpfs 页不会在这里被淘汰。
    /// 返回是否成功淘汰了一页。
    fn evict_one_disk_clean(&mut self) -> bool {
        // 最多绕一圈，避免无限循环
        let attempts = self.lru_order.len();
        for _ in 0..attempts {
            let Some((&oldest_gen, &old_key)) = self.lru_order.first_key_value() else {
                return false;
            };

            if !self.cache.contains_key(&old_key) {
                self.lru_order.remove(&oldest_gen);
                self.lru_gen.remove(&old_key);
                continue;
            }
            if !is_disk_backed_cache_id(old_key.0) {
                self.rotate_lru_entry(oldest_gen, old_key);
                continue;
            }

            // 只回收磁盘文件系统的干净页；脏页等待 writeback 后再回收。
            let keep = match self.cache.get(&old_key) {
                Some(page_lock) => match page_lock.try_read() {
                    Some(page) => page.dirty || Arc::strong_count(page_lock) > 1,
                    None => true,
                },
                None => false,
            };
            if keep {
                self.rotate_lru_entry(oldest_gen, old_key);
                continue;
            }

            self.remove_key(old_key);
            return true;
        }
        false
    }

    /// 获取缓存页，不更新 LRU。
    pub fn get_page(&self, inode_id: usize, page_id: usize) -> Option<Arc<RwLock<Page>>> {
        self.cache.get(&(inode_id, page_id)).cloned()
    }

    /// 获取缓存页，并把命中的页刷新到 LRU 队尾。
    pub fn get_page_touch(&mut self, inode_id: usize, page_id: usize) -> Option<Arc<RwLock<Page>>> {
        let key = (inode_id, page_id);
        let page = self.cache.get(&key).cloned();
        if page.is_some() {
            self.touch(key);
        }
        page
    }

    /// 插入缓存页，磁盘文件系统页超过容量上限时按 LRU 淘汰最旧的干净磁盘页。
    /// 返回 `true` 表示磁盘页缓存处于压力状态（已满且无法淘汰干净页，发生了临时超容）。
    pub fn insert_page(
        &mut self,
        inode_id: usize,
        page_id: usize,
        page: Arc<RwLock<Page>>,
    ) -> bool {
        let key = (inode_id, page_id);
        if self.cache.contains_key(&key) {
            // 已存在，仅更新 LRU 顺序
            self.touch(key);
            return false;
        }

        let mut under_pressure = false;
        let disk_backed = is_disk_backed_cache_id(inode_id);
        while disk_backed && self.disk_pages >= self.max_disk_pages {
            if !self.evict_one_disk_clean() {
                // 全是脏页且无法淘汰，允许临时超容
                under_pressure = true;
                break;
            }
        }

        self.cache.insert(key, page);
        if disk_backed {
            self.disk_pages += 1;
        }
        self.touch(key);
        under_pressure
    }

    /// 统计当前脏页数量
    pub fn dirty_pages_count(&self) -> usize {
        self.cache
            .values()
            .filter(|page_lock| page_lock.read().dirty)
            .count()
    }

    /// 统计当前磁盘文件系统脏页数量。
    pub fn dirty_disk_pages_count(&self) -> usize {
        self.cache
            .iter()
            .filter(|((inode_id, _), page_lock)| {
                is_disk_backed_cache_id(*inode_id) && page_lock.read().dirty
            })
            .count()
    }

    /// 统计当前缓存页总数
    pub fn pages_count(&self) -> usize {
        self.cache.len()
    }

    /// 统计当前磁盘文件系统缓存页总数。
    pub fn disk_pages_count(&self) -> usize {
        self.disk_pages
    }

    /// Reclaim up to `max_pages` clean disk-backed pages from the cache.
    ///
    /// Dirty, tmpfs, or locked pages are kept and rotated to the back of the
    /// LRU list. This is used by the frame allocator under memory pressure, so
    /// it must not perform write-back or block on page locks.
    pub fn reclaim_clean_pages(&mut self, max_pages: usize) -> usize {
        let mut reclaimed = 0usize;
        while reclaimed < max_pages {
            let attempts = self.lru_order.len();
            if attempts == 0 {
                break;
            }

            let mut reclaimed_one = false;
            for _ in 0..attempts {
                let Some((&oldest_gen, &old_key)) = self.lru_order.first_key_value() else {
                    return reclaimed;
                };

                if !self.cache.contains_key(&old_key) {
                    self.lru_order.remove(&oldest_gen);
                    self.lru_gen.remove(&old_key);
                    continue;
                }
                if !is_disk_backed_cache_id(old_key.0) {
                    self.rotate_lru_entry(oldest_gen, old_key);
                    continue;
                }

                let keep = match self.cache.get(&old_key) {
                    Some(page_lock) => match page_lock.try_read() {
                        Some(page) => page.dirty || Arc::strong_count(page_lock) > 1,
                        None => true,
                    },
                    None => false,
                };
                if keep {
                    self.rotate_lru_entry(oldest_gen, old_key);
                    continue;
                }

                self.remove_key(old_key);
                reclaimed += 1;
                reclaimed_one = true;
                break;
            }

            if !reclaimed_one {
                break;
            }
        }
        reclaimed
    }

    /// Trim clean disk-backed cache pages until the configured capacity is reached.
    pub fn trim_clean_to_limit(&mut self) -> usize {
        let excess = self.disk_pages.saturating_sub(self.max_disk_pages);
        self.reclaim_clean_pages(excess)
    }

    /// 获取指定 inode 的所有脏页，按 page_id 升序排列。
    /// 使用 BTreeMap::range 只遍历该 inode 在缓存中的页，避免扫描整个文件范围。
    pub fn get_inode_dirty_pages(&self, inode_id: usize) -> Vec<(usize, Arc<RwLock<Page>>)> {
        let mut result = Vec::new();
        for ((_, page_id), page_lock) in self.cache.range((inode_id, 0)..(inode_id, usize::MAX)) {
            if page_lock.read().dirty {
                result.push((*page_id, page_lock.clone()));
            }
        }
        result
    }

    /// 获取指定 inode 的前 `limit` 个脏页，并返回是否仍有更多脏页。
    pub fn get_inode_dirty_pages_limited(
        &self,
        inode_id: usize,
        limit: usize,
    ) -> (Vec<(usize, Arc<RwLock<Page>>)>, bool) {
        if limit == 0 {
            return (Vec::new(), false);
        }
        let mut result = Vec::new();
        let mut has_more = false;
        for ((_, page_id), page_lock) in self.cache.range((inode_id, 0)..(inode_id, usize::MAX)) {
            if !page_lock.read().dirty {
                continue;
            }
            if result.len() >= limit {
                has_more = true;
                break;
            }
            result.push((*page_id, page_lock.clone()));
        }
        (result, has_more)
    }

    /// 移除指定 inode 的所有缓存页（通常在 truncate / O_TRUNC / unlink 时调用）
    pub fn remove_inode_pages(&mut self, inode_id: usize) {
        let keys_to_remove: Vec<(usize, usize)> = self
            .cache
            .keys()
            .filter(|(ino, _)| *ino == inode_id)
            .cloned()
            .collect();
        for key in keys_to_remove {
            self.remove_key(key);
        }
    }

    /// 移除指定 inode 的单个缓存页。
    pub fn remove_page(&mut self, inode_id: usize, page_id: usize) {
        let key = (inode_id, page_id);
        self.remove_key(key);
    }

    /// 移除 inode 集合的所有缓存页，用于卸载临时文件系统子树。
    pub fn remove_inode_set_pages(&mut self, inode_ids: &[usize]) {
        let mut sorted_inode_ids = inode_ids.to_vec();
        sorted_inode_ids.sort_unstable();
        sorted_inode_ids.dedup();
        let keys_to_remove: Vec<(usize, usize)> = self
            .cache
            .keys()
            .filter(|(ino, _)| sorted_inode_ids.binary_search(ino).is_ok())
            .cloned()
            .collect();
        for key in keys_to_remove {
            self.remove_key(key);
        }
    }
}
