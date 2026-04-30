use alloc::boxed::Box;
use alloc::sync::Arc;
use spin::Mutex;
use log::error;
pub mod device;
pub mod icmp;
pub mod ip;
pub mod loopback;
pub mod route;
pub mod skb;
#[allow(missing_docs)]
pub mod tcp;
pub mod udp;
// ========== 新增模块 ==========
pub mod arp;
pub mod ethernet;
pub mod neighbor;
pub mod virtio;

// 其他模块...

use crate::net::device::DeviceManager;
use crate::net::device::NetDevice;
use crate::net::ethernet::ethernet_rcv;
use crate::net::loopback::LoopbackDevice;
use crate::net::route::RouteTable;
use crate::net::virtio::VirtIONetDevice;

/// 全局网络设备管理器
static DEVICE_MANAGER: Mutex<Option<DeviceManager>> = Mutex::new(None);

/// 全局路由表
static ROUTE_TABLE: Mutex<Option<RouteTable>> = Mutex::new(None);
#[allow(unused)]
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

    // 本地回环地址
    ip::add_local_ip(0x7F000001);

    // ========== VirtIO-net 设备初始化 ==========
    let mut virtio_net = VirtIONetDevice::new("eth0");

    let my_ip = 0x0A00020F; // 10.0.2.15
    let gateway = 0x0A000202; // 10.0.2.2
    error!("=======================");
    if virtio_net.probe() {
        error!("=======================");
        virtio_net.set_ip(my_ip);
        error!("=======================");
        match virtio_net.init_device() {
            Ok(()) => {
                error!("=======================");

                let virtio_net_arc = Arc::new(virtio_net);
                let dev_arc: Arc<dyn crate::net::device::NetDevice> = virtio_net_arc.clone();

                let rx_dev = dev_arc.clone();
                error!("=======================");
                virtio_net_arc.set_rx_handler(Box::new(move |mut skb| {
                    skb.dev = Some(rx_dev.clone());
                    if let Err(e) = ethernet_rcv(skb, rx_dev.clone()) {
                        log::debug!("eth0 rx drop: {}", e);
                    }
                }));

                device_manager.register(virtio_net_arc.clone());
                ip::add_local_ip(my_ip);
                route_table.add_entry(0, 0, gateway, virtio_net_arc.clone());

                log::info!(
                    "VirtIO-net device registered with IP {}.{}.{}.{}",
                    (my_ip >> 24) & 0xFF,
                    (my_ip >> 16) & 0xFF,
                    (my_ip >> 8) & 0xFF,
                    my_ip & 0xFF
                );
            }
            Err(e) => {
                log::warn!("Failed to initialize VirtIO-net device: {}", e);
                log::warn!("Skip default route installation because eth0 is not ready");
            }
        }
    } else {
        log::warn!("No VirtIO-net device found; default route not installed");
    }
    // ================================================

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

#[allow(unused)]
/// 轮询所有网络设备的接收队列。
pub fn poll_rx_all() {
    let manager_guard = DEVICE_MANAGER.lock();
    let Some(manager) = manager_guard.as_ref() else {
        return;
    };

    for dev in manager.get_all().iter() {
        dev.poll_rx();
    }
}
