use super::device::{NetDevice, NetDeviceFlags, XmitError};
use super::skb::Skb;
use crate::net::ip::ip_rcv;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};
use polyhal::println;
use crate::sync::SpinNoIrqLock;

#[allow(unused)]
/// 回环设备
pub struct LoopbackDevice {
    name: String,
    running: AtomicBool,
    rx_handler: SpinNoIrqLock<Option<Box<dyn Fn(Skb) + Send + Sync>>>,
}

#[allow(unused)]
impl LoopbackDevice {
    pub fn new() -> Self {
        Self {
            name: String::from("loopback"),
            running: AtomicBool::new(false),
            rx_handler: SpinNoIrqLock::new(None),
        }
    }

    pub fn init(&self) {
        self.running.store(true, Ordering::Release);
        self.register_ip_handler();
        log::info!("Loopback device initialized");
    }

    pub fn register_ip_handler(&self) {
        let dev: Arc<dyn NetDevice> = Arc::new(self.clone());
        self.set_rx_handler(Box::new(move |mut skb| {
            skb.dev = Some(dev.clone());
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

    fn hard_start_xmit(&self, mut skb: Skb) -> Result<(Skb, u32, u16), &'static str> {
        if !self.running.load(Ordering::Acquire) {
            return Err(XmitError::Invalid.into());
        }

        // println!("Loopback: transmitting packet of {} bytes", skb.len());

        if let Some(handler) = self.rx_handler.lock().as_ref() {
            let mut rx_skb = skb.clone();
            rx_skb.dev = Some(Arc::new(self.clone()));
            // println!("Loopback: delivering packet to RX handler");
            handler(rx_skb);
            Ok((skb, 0, 0))
        } else {
            Ok((skb, 0, 0))
        }
    }

    fn set_rx_handler(&self, handler: Box<dyn Fn(Skb) + Send + Sync>) {
        *self.rx_handler.lock() = Some(handler);
    }

    // ========== 新增方法实现 ==========
    fn mac_addr(&self) -> [u8; 6] {
        [0; 6] // 回环设备没有 MAC 地址
    }

    fn ip_addr(&self) -> u32 {
        0 // 回环设备没有固定 IP
    }
}

impl Clone for LoopbackDevice {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            running: AtomicBool::new(self.running.load(Ordering::Acquire)),
            rx_handler: SpinNoIrqLock::new(None),
        }
    }
}
