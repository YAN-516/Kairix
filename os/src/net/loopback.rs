//! 回环网络设备。
//!
//! 回环设备不经过二层封装，发送路径会直接把包交回注册的接收 handler。

use super::device::{NetDevice, NetDeviceFlags, XmitError};
use super::skb::Skb;
use crate::net::ip::ip_rcv;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::RwLock;

#[allow(unused)]
/// 回环设备。
pub struct LoopbackDevice {
    /// 设备名称。
    name: String,
    /// 设备是否已经初始化并可收发。
    running: AtomicBool,
    /// 回环包的接收回调。
    rx_handler: RwLock<Option<Box<dyn Fn(Skb) + Send + Sync>>>,
}

#[allow(unused)]
impl LoopbackDevice {
    /// 创建尚未启动的回环设备。
    pub fn new() -> Self {
        Self {
            name: String::from("loopback"),
            running: AtomicBool::new(false),
            rx_handler: RwLock::new(None),
        }
    }

    /// 启动回环设备并注册 IP 层接收处理。
    pub fn init(&self) {
        self.running.store(true, Ordering::Release);
        self.register_ip_handler();
        log::info!("Loopback device initialized");
    }

    /// 注册默认 IP 接收 handler。
    ///
    /// 回环设备发出的包不带以太网头，直接进入 IP 层。
    pub fn register_ip_handler(&self) {
        let dev: Arc<dyn NetDevice> = Arc::new(self.clone());
        self.set_rx_handler(Box::new(move |mut skb| {
            skb.dev = Some(dev.clone());
            if let Err(e) = ip_rcv(skb) {
                log::info!("IP layer error: {}", e);
            }
        }));
        log::info!("Loopback: IP handler registered");
    }
}

#[allow(unused)]
impl NetDevice for LoopbackDevice {
    fn name(&self) -> &str {
        &self.name
    }

    fn mtu(&self) -> u16 {
        65535
    }

    fn flags(&self) -> NetDeviceFlags {
        let mut flags = NetDeviceFlags::UP | NetDeviceFlags::RUNNING;
        flags |= NetDeviceFlags::LOOPBACK;
        flags
    }

    fn hard_start_xmit(&self, mut skb: Skb) -> Result<(Skb, u32, u16), &'static str> {
        if !self.running.load(Ordering::Acquire) {
            return Err(XmitError::Invalid.into());
        }

        // println!("Loopback: transmitting packet of {} bytes", skb.len());

        if let Some(handler) = self.rx_handler.read().as_ref() {
            // 回环发送就是本地接收，handler 执行完即表示该包已经交给上层。
            skb.dev = Some(Arc::new(self.clone()));
            // println!("Loopback: delivering packet to RX handler");
            handler(skb);
            Ok((Skb::new(0), 0, 0))
        } else {
            Ok((skb, 0, 0))
        }
    }

    fn set_rx_handler(&self, handler: Box<dyn Fn(Skb) + Send + Sync>) {
        *self.rx_handler.write() = Some(handler);
    }

    fn mac_addr(&self) -> [u8; 6] {
        [0; 6] // 回环设备没有 MAC 地址
    }

    fn ip_addr(&self) -> u32 {
        0 // 回环设备没有固定 IP
    }
}

impl Clone for LoopbackDevice {
    /// 克隆设备元数据。
    ///
    /// handler 不复制，避免多个克隆对象同时持有同一个可变接收入口。
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            running: AtomicBool::new(self.running.load(Ordering::Acquire)),
            rx_handler: RwLock::new(None),
        }
    }
}
