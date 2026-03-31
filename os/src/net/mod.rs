use alloc::sync::Arc;
use spin::Mutex;

pub mod device;
pub mod icmp;
pub mod ip;
pub mod loopback;
pub mod route;
pub mod skb;
pub mod udp;

use crate::net::device::DeviceManager;
use crate::net::loopback::LoopbackDevice;
use crate::net::route::RouteTable;
#[allow(unused)]
/// 全局网络设备管理器
static DEVICE_MANAGER: Mutex<Option<DeviceManager>> = Mutex::new(None);
#[allow(unused)]

/// 全局路由表
static ROUTE_TABLE: Mutex<Option<RouteTable>> = Mutex::new(None);
#[allow(unused)]

/// 初始化网络子系统
pub fn init() {
    // 初始化设备管理器
    let mut device_manager = DeviceManager::new();

    // 创建并注册回环设备
    let loopback = Arc::new(LoopbackDevice::new());
    loopback.init();
    device_manager.register(loopback.clone());

    *DEVICE_MANAGER.lock() = Some(device_manager);

    // 初始化路由表
    let mut route_table = RouteTable::new();
    route_table.add_loopback_route(loopback);
    *ROUTE_TABLE.lock() = Some(route_table);

    log::info!("Network subsystem initialized");
}
#[allow(unused)]
/// 获取设备管理器
pub fn device_manager() -> &'static Mutex<Option<DeviceManager>> {
    &DEVICE_MANAGER
}
#[allow(unused)]
/// 获取路由表
pub fn route_table() -> &'static Mutex<Option<RouteTable>> {
    &ROUTE_TABLE
}
