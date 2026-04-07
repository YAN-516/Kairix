use alloc::sync::Arc;
use spin::Mutex;

pub mod device;
pub mod icmp;
pub mod ip;
pub mod loopback;
pub mod route;
pub mod skb;
pub mod udp;
// ========== 新增模块 ==========
pub mod arp;
pub mod ethernet;
pub mod neighbor;
pub mod virtio;

// 其他模块...

use crate::net::device::DeviceManager;
use crate::net::loopback::LoopbackDevice;
use crate::net::route::RouteTable;
use crate::net::virtio::VirtIONetDevice;

/// 全局网络设备管理器
static DEVICE_MANAGER: Mutex<Option<DeviceManager>> = Mutex::new(None);

/// 全局路由表
static ROUTE_TABLE: Mutex<Option<RouteTable>> = Mutex::new(None);

/// 初始化网络子系统（修改版）
pub fn init() {
    // 初始化设备管理器
    let mut device_manager = DeviceManager::new();

    // 创建并注册回环设备
    let loopback = Arc::new(LoopbackDevice::new());
    loopback.init();
    device_manager.register(loopback.clone());

    // 初始化路由表
    let mut route_table = RouteTable::new();
    route_table.add_loopback_route(loopback.clone());

    // // ========== 新增：VirtIO-net 设备初始化 ==========
    // let mut virtio_net = VirtIONetDevice::new("eth0");
    // if virtio_net.probe() {
    //     match virtio_net.init_device() {
    //         Ok(()) => {
    //             // 设置本机 IP（示例：192.168.1.100）
    //             let my_ip = 0xC0A80164; // 192.168.1.100
    //             virtio_net.set_ip(my_ip);
    //             let virtio_net_arc = Arc::new(virtio_net);

    //             // 注册设备
    //             device_manager.register(virtio_net_arc.clone());

    //             // 添加到本地 IP 列表
    //             ip::add_local_ip(my_ip);

    //             // 添加默认路由指向网关 192.168.1.1
    //             route_table.add_entry(0, 0, 0xC0A80101, virtio_net_arc.clone());

    //             log::info!(
    //                 "VirtIO-net device registered with IP {}.{}.{}.{}",
    //                 (my_ip >> 24) & 0xFF,
    //                 (my_ip >> 16) & 0xFF,
    //                 (my_ip >> 8) & 0xFF,
    //                 my_ip & 0xFF
    //             );
    //         }
    //         Err(e) => {
    //             log::warn!("Failed to initialize VirtIO-net device: {}", e);
    //         }
    //     }
    // } else {
    //     log::info!("No VirtIO-net device found");
    // }
    // // ================================================

    *DEVICE_MANAGER.lock() = Some(device_manager);
    *ROUTE_TABLE.lock() = Some(route_table);

    log::info!("Network subsystem initialized");
}

#[allow(unused)]
/// 获取设备管理器
pub fn device_manager() -> &'static Mutex<Option<DeviceManager>> {
    &DEVICE_MANAGER
}

/// 获取路由表
pub fn route_table() -> &'static Mutex<Option<RouteTable>> {
    &ROUTE_TABLE
}
