use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::bitflags;

use crate::net::skb::Skb;

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

#[allow(unused)]
/// 网络设备trait，定义了网络设备的基本接口
pub trait NetDevice: Send + Sync {
    ///设备名称
    fn name(&self) -> &str;
    //最大传输单元
    fn mtu(&self) -> u16;
    ///状态标志位
    fn flags(&self) -> NetDeviceFlags;
    ///发送数据包
    fn hard_start_xmit(&self, skb: super::skb::Skb) -> Result<(Skb, u32, u16), &'static str>;
    ///接收数据包
    fn set_rx_handler(&self, handler: Box<dyn Fn(super::skb::Skb) + Send + Sync>);
    /// 轮询接收队列（默认设备无需实现）
    fn poll_rx(&self) {}

    /// 获取 MAC 地址（以太网设备）
    fn mac_addr(&self) -> [u8; 6] {
        [0; 6] // 默认实现，回环设备返回全零
    }

    /// 获取 IP 地址
    fn ip_addr(&self) -> u32 {
        0 // 默认实现
    }
}

#[allow(unused)]
/// 网络设备管理器
pub struct DeviceManager {
    devices: Vec<Arc<dyn NetDevice>>,
}

#[allow(unused)]
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
