//! 简单 IPv4 路由表。
//!
//! 当前网络栈只维护内核内的静态路由，查找时使用最长前缀匹配。

use crate::net::device::NetDevice;
use crate::net::loopback::LoopbackDevice;
use alloc::sync::Arc;
use alloc::vec::Vec;
use log::info;

/// 路由条目。
#[derive(Clone)]
#[allow(unused)]
pub struct RouteEntry {
    /// 目标网络地址，按网络栈内部的主机序 `u32` 保存。
    pub dest: u32,
    /// 子网掩码，和 `dest` 一起参与最长前缀匹配。
    pub mask: u32,
    /// 下一跳网关；为 0 表示目的地址本身就在直连网络上。
    pub gateway: u32,
    /// 该路由对应的输出设备。
    pub dev: Arc<dyn NetDevice>,
}

#[allow(unused)]
/// 路由表
pub struct RouteTable {
    entries: Vec<RouteEntry>,
}

#[allow(unused)]
impl RouteTable {
    /// 创建一个空路由表。
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// 添加回环路由。
    ///
    /// `127.0.0.0/8` 总是走回环设备。
    pub fn add_loopback_route(&mut self, dev: Arc<LoopbackDevice>) {
        self.entries.push(RouteEntry {
            dest: 0x7F000000,
            mask: 0xFF000000,
            gateway: 0,
            dev: dev.clone(),
        });
        log::info!("Added loopback route for 127.0.0.0/8");
    }

    /// 添加一条静态路由。
    pub fn add_entry(&mut self, dest: u32, mask: u32, gateway: u32, dev: Arc<dyn NetDevice>) {
        self.entries.push(RouteEntry {
            dest,
            mask,
            gateway,
            dev,
        });
    }

    /// 查找目的地址对应的最佳路由。
    ///
    /// 多条路由匹配时选择掩码位数最多的条目，即最长前缀匹配。
    pub fn lookup(&self, dest: u32) -> Option<&RouteEntry> {
        self.entries
            .iter()
            .filter(|entry| dest & entry.mask == entry.dest)
            .max_by_key(|entry| entry.mask.count_ones())
    }
}

/// 全局路由查找函数。
///
/// 返回输出设备和下一跳地址；直连路由的下一跳就是 `dest`。
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
