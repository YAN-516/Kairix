use super::device::{NetDevice, NetDeviceFlags, XmitError};
use super::skb::Skb;
use crate::net::ip::ip_rcv;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::RwLock;
#[allow(unused)]
/// 回环设备
pub struct LoopbackDevice {
    name: String,
    running: AtomicBool,
    rx_handler: RwLock<Option<Box<dyn Fn(Skb) + Send + Sync>>>,
}
#[allow(unused)]
impl LoopbackDevice {
    pub fn new() -> Self {
        Self {
            name: String::from("loopback"),
            running: AtomicBool::new(false),
            rx_handler: RwLock::new(None),
        }
    }

    pub fn init(&self) {
        self.running.store(true, Ordering::Release);
        self.register_ip_handler();
        log::info!("Loopback device initialized");
    }

    pub fn register_ip_handler(&self) {
        let mut dev: Arc<dyn NetDevice> = Arc::new(self.clone());
        self.set_rx_handler(Box::new(move |mut skb| {
            // 设置设备引用，供后续发送回复使用
            skb.dev = Some(dev.clone());

            // 调用 IP 层处理
            if let Err(e) = ip_rcv(skb) {
                log::error!("IP layer error: {}", e);
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

    fn hard_start_xmit(&self, mut skb: Skb) -> Result<Skb, XmitError> {
        if !self.running.load(Ordering::Acquire) {
            return Err(XmitError::Invalid);
        }

        log::debug!("Loopback: transmitting packet of {} bytes", skb.len());

        // 回环设备直接注入接收路径
        if let Some(handler) = self.rx_handler.read().as_ref() {
            skb.dev = Some(Arc::new(self.clone()));
            let ret = ip_rcv(skb);
            if let Ok(skb) = ret {
                Ok(skb)
            } else {
                Err(XmitError::Invalid)
            }
        } else {
            Ok(skb)
        }
    }

    fn set_rx_handler(&self, handler: Box<dyn Fn(Skb) + Send + Sync>) {
        *self.rx_handler.write() = Some(handler);
    }
}

impl Clone for LoopbackDevice {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            running: AtomicBool::new(self.running.load(Ordering::Acquire)),
            rx_handler: RwLock::new(None),
        }
    }
}
