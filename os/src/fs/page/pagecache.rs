#[deny(unused_doc_comments)]
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use polyhal::common::FrameTracker;
use crate::sync::SleepLock;
use crate::sync::SpinNoIrqLock;

/// 页缓存最大页数（4096 页 ≈ 16MB）
const MAX_PAGE_CACHE_PAGES: usize = 4096;

lazy_static! {
    ///
    pub static ref PAGE_CACHE: SleepLock<PageCache> = SleepLock::new(PageCache::new());
}

///
pub struct Page {
    ///
    pub frame: Arc<FrameTracker>,
    ///脏页标记
    pub dirty: bool, 
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
    cache: BTreeMap<(usize, usize), Arc<SpinNoIrqLock<Page>>>,
    /// LRU 队列，队尾为最新访问，队首为最久未访问
    lru: VecDeque<(usize, usize)>,
    /// 最大页数
    max_pages: usize,
}

impl PageCache {
    ///
    pub fn new() -> Self {
        Self {
            cache: BTreeMap::new(),
            lru: VecDeque::new(),
            max_pages: MAX_PAGE_CACHE_PAGES,
        }
    }

    /// 获取缓存页，并在 LRU 中更新访问顺序
    pub fn get_page(&mut self, inode_id: usize, page_id: usize) -> Option<Arc<SpinNoIrqLock<Page>>> {
        let key = (inode_id, page_id);
        if let Some(page) = self.cache.get(&key).cloned() {
            if let Some(pos) = self.lru.iter().position(|&k| k == key) {
                self.lru.remove(pos);
                self.lru.push_back(key);
            }
            Some(page)
        } else {
            None
        }
    }

    /// 插入缓存页，超过容量上限时按 LRU 淘汰最旧的页。
    /// 淘汰时会跳过脏页，防止未写回的数据丢失。
    pub fn insert_page(&mut self, inode_id: usize, page_id: usize, page: Arc<SpinNoIrqLock<Page>>) {
        let key = (inode_id, page_id);
        if self.cache.contains_key(&key) {
            if let Some(pos) = self.lru.iter().position(|&k| k == key) {
                self.lru.remove(pos);
                self.lru.push_back(key);
            }
            return;
        }
        // 淘汰旧页时保护脏页：若最旧页为脏，则跳过并放到尾部，寻找下一个可淘汰的干净页
        let mut checked = 0;
        while self.cache.len() >= self.max_pages && checked < self.cache.len() {
            if let Some(old_key) = self.lru.pop_front() {
                checked += 1;
                if let Some(old_page) = self.cache.get(&old_key) {
                    if old_page.lock().dirty {
                        self.lru.push_back(old_key);
                        continue;
                    }
                }
                self.cache.remove(&old_key);
            } else {
                break;
            }
        }
        self.cache.insert(key, page);
        self.lru.push_back(key);
    }

    /// 移除指定 inode 的所有缓存页。
    /// 脏页会被保留，直到显式写回或 LRU 淘汰。
    pub fn remove_inode_pages(&mut self, inode_id: usize) {
        let keys_to_remove: Vec<(usize, usize)> = self
            .cache
            .keys()
            .filter(|(ino, _)| *ino == inode_id)
            .cloned()
            .collect();
        for key in keys_to_remove {
            // 保护脏页：truncate 时不应丢失尚未写回的数据
            if let Some(page) = self.cache.get(&key) {
                if page.lock().dirty {
                    continue;
                }
            }
            self.cache.remove(&key);
            if let Some(pos) = self.lru.iter().position(|&k| k == key) {
                self.lru.remove(pos);
            }
        }
    }
}
