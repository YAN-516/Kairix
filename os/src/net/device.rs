//! 网络设备抽象和设备注册表。
//!
//! 上层协议栈只依赖 `NetDevice` trait，不直接关心底层是回环、VirtIO、
//! DWMAC 还是其他网卡。

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::bitflags;

use crate::net::skb::Skb;

bitflags! {
    #[derive(Clone, Copy, Debug)]
    /// 网络设备状态标志。
    pub struct NetDeviceFlags: u32 {
        /// 设备已被上层网络栈启用，可以参与收发流程。
        const UP = 1 << 0;
        /// 设备底层链路或驱动处于可运行状态。
        const RUNNING = 1 << 1;
        /// 回环设备标志，表示数据包会在本机内部回送。
        const LOOPBACK = 1 << 2;
        /// 设备支持广播帧发送。
        const BROADCAST = 1 << 3;
    }
}

/// 网络设备发送路径可能返回的错误类型。
#[derive(Debug)]
pub enum XmitError {
    /// 设备发送队列繁忙，调用者可以稍后重试。
    Busy,
    /// 待发送的数据包或设备状态不满足发送要求。
    Invalid,
    /// 其他未细分的发送错误。
    Other,
}

impl From<XmitError> for &str {
    /// 将发送错误转换为简短静态字符串，便于沿用当前驱动接口。
    fn from(s: XmitError) -> Self {
        match s {
            XmitError::Busy => "Busy",
            XmitError::Invalid => "Invalid",
            XmitError::Other => "Other",
        }
    }
}

#[allow(unused)]
/// 网络设备抽象，定义网络栈与具体设备驱动之间的基本接口。
///
/// 设备实现需要可跨上下文共享，因此要求 `Send + Sync`。
pub trait NetDevice: Send + Sync {
    /// 返回设备名称，如 `lo` 或 `eth0`。
    fn name(&self) -> &str;

    /// 返回设备最大传输单元（MTU），单位为字节。
    fn mtu(&self) -> u16;

    /// 返回设备当前状态标志。
    fn flags(&self) -> NetDeviceFlags;

    /// 将一个 socket buffer 交给设备发送。
    ///
    /// 成功返回值沿用当前调用链约定，通常包含已处理的 `Skb` 和附加状态。
    fn hard_start_xmit(&self, skb: super::skb::Skb) -> Result<(Skb, u32, u16), &'static str>;

    /// 注册接收回调。
    ///
    /// 驱动收到完整二层帧或本地回环包后调用该 handler。
    fn set_rx_handler(&self, handler: Box<dyn Fn(super::skb::Skb) + Send + Sync>);

    /// 轮询接收队列（默认设备无需实现）
    fn poll_rx(&self) {}

    /// 获取设备 MAC 地址。
    ///
    /// 非以太网设备默认返回全零地址。
    fn mac_addr(&self) -> [u8; 6] {
        [0; 6] // 默认实现，回环设备返回全零
    }

    /// 获取设备 IPv4 地址。
    ///
    /// 默认值 0 表示未配置 IPv4 地址。
    fn ip_addr(&self) -> u32 {
        0 // 默认实现
    }
}

#[allow(unused)]
/// 网络设备管理器，保存当前注册到内核网络栈的设备列表。
pub struct DeviceManager {
    /// 已注册的网络设备集合。
    ///
    /// 使用 `Arc` 使路由表、邻居表和驱动回调可以共享同一个设备实例。
    devices: Vec<Arc<dyn NetDevice>>,
}

#[allow(unused)]
impl DeviceManager {
    /// 创建一个空的网络设备管理器。
    pub fn new() -> Self {
        Self {
            devices: Vec::new(),
        }
    }

    /// 注册一个网络设备。
    ///
    /// 设备会按注册顺序保存。
    pub fn register(&mut self, device: Arc<dyn NetDevice>) {
        self.devices.push(device);
    }

    /// 按设备名称查找已注册设备。
    pub fn get_by_name(&self, name: &str) -> Option<Arc<dyn NetDevice>> {
        self.devices.iter().find(|dev| dev.name() == name).cloned()
    }

    /// 获取所有已注册设备的只读切片。
    pub fn get_all(&self) -> &[Arc<dyn NetDevice>] {
        &self.devices
    }
}
