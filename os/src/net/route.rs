use crate::net::device::NetDevice;
use crate::net::loopback::LoopbackDevice;
use alloc::sync::Arc;
use alloc::vec::Vec;

/// 路由条目
#[derive(Clone)]
pub struct RouteEntry {
    pub dest: u32,               // 目标网络
    pub mask: u32,               // 子网掩码
    pub gateway: u32,            // 网关
    pub dev: Arc<dyn NetDevice>, // 输出设备
}

/// 路由表
pub struct RouteTable {
    entries: Vec<RouteEntry>,
}

impl RouteTable {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// 添加回环路由
    pub fn add_loopback_route(&mut self, dev: Arc<LoopbackDevice>) {
        self.entries.push(RouteEntry {
            dest: 0x7F000000, // 127.0.0.0
            mask: 0xFF000000, // 255.0.0.0
            gateway: 0,
            dev: dev.clone(),
        });

        log::info!("Added loopback route for 127.0.0.0/8");
    }

    /// 添加路由
    pub fn add_entry(&mut self, dest: u32, mask: u32, gateway: u32, dev: Arc<dyn NetDevice>) {
        self.entries.push(RouteEntry {
            dest,
            mask,
            gateway,
            dev,
        });
    }

    /// 查找路由
    pub fn lookup(&self, dest: u32) -> Option<&RouteEntry> {
        for entry in &self.entries {
            if dest & entry.mask == entry.dest {
                return Some(entry);
            }
        }
        None
    }
}

/// 全局路由查找函数
pub fn route_lookup(dest: u32) -> Result<Arc<dyn NetDevice>, &'static str> {
    use crate::net::route_table;

    let route_table = route_table().lock();
    let table = route_table.as_ref().ok_or("Route table not initialized")?;

    if let Some(entry) = table.lookup(dest) {
        Ok(entry.dev.clone())
    } else {
        Err("No route to destination")
    }
}
