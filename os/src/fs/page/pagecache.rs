#[deny(unused_doc_comments)]
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use polyhal::common::FrameTracker;
use spin::RwLock;
use crate::sync::SleepLock;

/// 页缓存最大页数（4096 页 ≈ 16MB）
pub const MAX_PAGE_CACHE_PAGES: usize = 4096;

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
    /// 最大页数
    max_pages: usize,
}

impl PageCache {
    ///
    pub fn new() -> Self {
        Self {
            cache: BTreeMap::new(),
            lru_order: BTreeMap::new(),
            lru_gen: BTreeMap::new(),
            next_gen: 0,
            max_pages: MAX_PAGE_CACHE_PAGES,
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

    /// 尝试淘汰一页。优先淘汰干净页；脏页给第二次机会（移回队尾）。
    /// 返回是否成功淘汰了一页。
    fn evict_one(&mut self) -> bool {
        // 最多绕一圈，避免无限循环
        let attempts = self.lru_order.len();
        for _ in 0..attempts {
            let Some((&oldest_gen, &old_key)) = self.lru_order.first_key_value() else {
                return false;
            };

            // 检查是否为脏页
            if let Some(page_lock) = self.cache.get(&old_key) {
                if let Some(page) = page_lock.try_read() {
                    if page.dirty {
                        // 脏页：给第二次机会，移到最新位置
                        drop(page);
                        self.lru_order.remove(&oldest_gen);
                        let new_gen = self.next_gen;
                        self.next_gen += 1;
                        self.lru_order.insert(new_gen, old_key);
                        self.lru_gen.insert(old_key, new_gen);
                        continue;
                    }
                }
            }

            // 淘汰这一页（干净页或已无法读取）
            self.cache.remove(&old_key);
            self.lru_gen.remove(&old_key);
            self.lru_order.remove(&oldest_gen);
            return true;
        }
        false
    }

    /// 获取缓存页（读操作不更新 LRU，减少锁竞争）
    pub fn get_page(&self, inode_id: usize, page_id: usize) -> Option<Arc<RwLock<Page>>> {
        self.cache.get(&(inode_id, page_id)).cloned()
    }

    /// 插入缓存页，超过容量上限时按 LRU 淘汰最旧的页。
    /// 返回 `true` 表示缓存处于压力状态（已满且无法淘汰干净页，发生了临时超容）。
    pub fn insert_page(&mut self, inode_id: usize, page_id: usize, page: Arc<RwLock<Page>>) -> bool {
        let key = (inode_id, page_id);
        if self.cache.contains_key(&key) {
            // 已存在，仅更新 LRU 顺序
            self.touch(key);
            return false;
        }

        let mut under_pressure = false;
        // 淘汰最旧的页，直到有空位
        while self.cache.len() >= self.max_pages {
            if !self.evict_one() {
                // 全是脏页且无法淘汰，允许临时超容
                under_pressure = true;
                break;
            }
        }

        self.cache.insert(key, page);
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

    /// 获取指定 inode 的所有脏页，按 page_id 升序排列。
    /// 使用 BTreeMap::range 只遍历该 inode 在缓存中的页，避免扫描整个文件范围。
    pub fn get_inode_dirty_pages(&self, inode_id: usize) -> Vec<(usize, Arc<RwLock<Page>>)> {
        let mut result = Vec::new();
        for ((_, page_id), page_lock) in
            self.cache.range((inode_id, 0)..(inode_id, usize::MAX))
        {
            if page_lock.read().dirty {
                result.push((*page_id, page_lock.clone()));
            }
        }
        result
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
            self.cache.remove(&key);
            if let Some(g) = self.lru_gen.remove(&key) {
                self.lru_order.remove(&g);
            }
        }
    }
}
