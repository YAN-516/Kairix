use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::RwLock;

use super::device::{NetDevice, NetDeviceFlags, XmitError};
use super::skb::Skb;

/// 回环设备
pub struct LoopbackDevice {
    name: String,
    running: AtomicBool,
    rx_handler: RwLock<Option<Box<dyn Fn(Skb) + Send + Sync>>>,
}

impl LoopbackDevice {
    pub fn new() -> Self {
        Self {
            name: String::from("lo"),
            running: AtomicBool::new(false),
            rx_handler: RwLock::new(None),
        }
    }

    pub fn init(&self) {
        self.running.store(true, Ordering::Release);
        log::info!("Loopback device initialized");
    }
}

impl NetDevice for LoopbackDevice {
    fn name(&self) -> &str {
        &self.name
    }

    fn mtu(&self) -> u16 {
        65536
    }

    fn flags(&self) -> NetDeviceFlags {
        let mut flags = NetDeviceFlags::UP | NetDeviceFlags::RUNNING;
        flags |= NetDeviceFlags::LOOPBACK;
        flags
    }

    fn hard_start_xmit(&self, mut skb: Skb) -> Result<(), XmitError> {
        if !self.running.load(Ordering::Acquire) {
            return Err(XmitError::Invalid);
        }

        log::debug!("Loopback: transmitting packet of {} bytes", skb.len);

        // 回环设备直接注入接收路径
        if let Some(handler) = self.rx_handler.read().as_ref() {
            skb.dev = Some(Arc::new(self.clone()));
            handler(skb);
        }

        Ok(())
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
