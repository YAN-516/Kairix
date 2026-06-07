use crate::net::device::NetDevice;
use crate::net::loopback::LoopbackDevice;
use alloc::sync::Arc;
use alloc::vec::Vec;
use log::info;

/// 路由条目
#[derive(Clone)]
#[allow(unused)]
pub struct RouteEntry {
    pub dest: u32,               // 目标网络
    pub mask: u32,               // 子网掩码
    pub gateway: u32,            // 网关
    pub dev: Arc<dyn NetDevice>, // 输出设备
}

#[allow(unused)]
/// 路由表
pub struct RouteTable {
    entries: Vec<RouteEntry>,
}

#[allow(unused)]
impl RouteTable {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// 添加回环路由
    pub fn add_loopback_route(&mut self, dev: Arc<LoopbackDevice>) {
        self.entries.push(RouteEntry {
            dest: 0x7F000000,
            mask: 0xFF000000,
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

    /// 查找路由（修改：支持最长前缀匹配）
    pub fn lookup(&self, dest: u32) -> Option<&RouteEntry> {
        self.entries
            .iter()
            .filter(|entry| dest & entry.mask == entry.dest)
            .max_by_key(|entry| entry.mask.count_ones())
    }
}

/// 全局路由查找函数
pub fn route_lookup(dest: u32) -> Result<(Arc<dyn NetDevice>, u32), &'static str> {
    use crate::net::route_table;

    let route_table = route_table().lock();
    let table = route_table.as_ref().ok_or("Route table not initialized")?;

    if let Some(entry) = table.lookup(dest) {
        let nexthop = if entry.gateway != 0 {
            entry.gateway
        } else {
            dest
        };
        Ok((entry.dev.clone(), nexthop))
    } else {
        info!(
            "Route lookup failed for destination {}.{}.{}.{}",
            (dest >> 24) & 0xFF,
            (dest >> 16) & 0xFF,
            (dest >> 8) & 0xFF,
            dest & 0xFF
        );
        Err("No route to destination")
    }
}
