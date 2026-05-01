#[deny(unused_doc_comments)]
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use lazy_static::lazy_static;
use polyhal::common::FrameTracker;
use crate::sync::SleepLock;
use spin::RwLock;

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
}

impl PageCache {
    ///
    pub fn new() -> Self {
        Self {
            cache: BTreeMap::new(),
        }
    }

    ///
    pub fn get_page(&self, inode_id: usize, page_id: usize) -> Option<Arc<RwLock<Page>>> {
        self.cache.get(&(inode_id, page_id)).cloned()
    }
    ///
    pub fn insert_page(&mut self, inode_id: usize, page_id: usize, page: Arc<RwLock<Page>>) {
        self.cache.insert((inode_id, page_id), page);
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
        }
    }
}
