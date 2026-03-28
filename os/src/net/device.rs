use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::bitflags;

bitflags! {
    #[derive(Clone, Copy, Debug)]
    pub struct NetDeviceFlags: u32 {
        const UP = 1 << 0;
        const RUNNING = 1 << 1;
        const LOOPBACK = 1 << 2;
        const BROADCAST = 1 << 3;
    }
}

/// 发送错误类型
#[derive(Debug)]
pub enum XmitError {
    Busy,
    Invalid,
    Other,
}

impl From<XmitError> for &str {
    fn from(s: XmitError) -> Self {
        match s {
            XmitError::Busy => "Busy",
            XmitError::Invalid => "Invalid",
            XmitError::Other => "Other",
        }
    }
}

/// 网络设备特征
pub trait NetDevice: Send + Sync {
    fn name(&self) -> &str;
    fn mtu(&self) -> u16;
    fn flags(&self) -> NetDeviceFlags;
    fn hard_start_xmit(&self, skb: super::skb::Skb) -> Result<(), XmitError>;
    fn set_rx_handler(&self, handler: Box<dyn Fn(super::skb::Skb) + Send + Sync>);
}

/// 网络设备管理器
pub struct DeviceManager {
    devices: Vec<Arc<dyn NetDevice>>,
}

impl DeviceManager {
    ///初始化网络设备序列
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
        }
    }
    //添加一个网络设备
    pub fn register(&mut self, device: Arc<dyn NetDevice>) {
        self.devices.push(device);
    }
    ///获得名字为name的设备
    pub fn get_by_name(&self, name: &str) -> Option<Arc<dyn NetDevice>> {
        self.devices.iter().find(|dev| dev.name() == name).cloned()
    }
    ///获取所有设备
    pub fn get_all(&self) -> &[Arc<dyn NetDevice>] {
        &self.devices
    }
}
