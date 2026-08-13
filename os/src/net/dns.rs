#![allow(dead_code)]

//! DNS IPv4 解析器。
//!
//! 当前实现面向内核网络栈的最小需求：通过 UDP 查询 A 记录，支持 CNAME
//! 跟随、IPv4 字面量短路、超时等待和基础 DNS name 压缩解析。

use crate::error::{SysError, SysResult};
use crate::socket::udp::{UdpSocket, register_udp_socket, send_udp_packet, unregister_udp_socket};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

/// DNS 服务端默认端口。
const DNS_PORT: u16 = 53;
/// A 记录类型，用于查询 IPv4 地址。
const DNS_TYPE_A: u16 = 1;
/// CNAME 记录类型，用于处理别名跳转。
const DNS_TYPE_CNAME: u16 = 5;
/// Internet 类 DNS 记录。
const DNS_CLASS_IN: u16 = 1;
/// 标准查询并请求服务端递归解析。
const DNS_QUERY_FLAGS: u16 = 0x0100;
/// 当前查询只包含一个 question。
const DNS_QUERY_QUESTION_COUNT: u16 = 1;
/// 查询报文中 answer/authority/additional 三个区段均为空。
const DNS_QUERY_EMPTY_SECTION_COUNT: u16 = 0;
/// UDP DNS 报文的传统最大长度，不处理 EDNS 扩展包。
const DNS_MAX_PACKET: usize = 512;
/// 单次 DNS 查询超时时间，单位为微秒。
const DNS_TIMEOUT_US: usize = 3_000_000;
/// CNAME 递归解析的最大深度，避免别名环导致无限递归。
const DNS_MAX_RECURSION: usize = 4;

/// UDP socket 注册守卫。
///
/// DNS 查询临时占用一个本地 UDP 端口。通过 RAII 在函数返回或出错时
/// 自动注销 socket，避免端口残留在全局 UDP 表中。
struct UdpRegistration {
    /// 注册到 UDP 表中的本地端口。
    port: u16,
    /// 需要在 drop 时注销的 socket。
    socket: Arc<Mutex<UdpSocket>>,
}

/// DNS 报文头，固定为 12 字节。
#[derive(Debug, Copy, Clone)]
struct DnsHeader {
    /// transaction id，用于匹配请求和响应。
    id: u16,
    /// DNS flags，本查询设置 RD 请求递归解析。
    flags: u16,
    /// question 区段条目数。
    question_count: u16,
    /// answer 区段条目数。
    answer_count: u16,
    /// authority 区段条目数。
    authority_count: u16,
    /// additional 区段条目数。
    additional_count: u16,
}

impl DnsHeader {
    /// 构造一个只包含单个 question 的递归查询头。
    fn query(txid: u16) -> Self {
        Self {
            id: txid,
            flags: DNS_QUERY_FLAGS,
            question_count: DNS_QUERY_QUESTION_COUNT,
            answer_count: DNS_QUERY_EMPTY_SECTION_COUNT,
            authority_count: DNS_QUERY_EMPTY_SECTION_COUNT,
            additional_count: DNS_QUERY_EMPTY_SECTION_COUNT,
        }
    }

    /// 将 DNS header 写入报文缓冲区。
    fn write_to(&self, packet: &mut Vec<u8>) {
        push_u16(packet, self.id);
        push_u16(packet, self.flags);
        push_u16(packet, self.question_count);
        push_u16(packet, self.answer_count);
        push_u16(packet, self.authority_count);
        push_u16(packet, self.additional_count);
    }
}

/// DNS question 区段。
struct DnsQuestion<'a> {
    /// 待查询域名，写入时会转换为 DNS label 编码。
    name: &'a str,
    /// 查询类型，如 A 或 CNAME。
    qtype: u16,
    /// 查询类，当前只使用 IN。
    qclass: u16,
}

impl<'a> DnsQuestion<'a> {
    /// 构造一个 IN/A 查询。
    fn a_record(name: &'a str) -> Self {
        Self {
            name,
            qtype: DNS_TYPE_A,
            qclass: DNS_CLASS_IN,
        }
    }

    /// 将 question 写入报文缓冲区。
    fn write_to(&self, packet: &mut Vec<u8>) -> SysResult<()> {
        push_name(packet, self.name)?;
        push_u16(packet, self.qtype);
        push_u16(packet, self.qclass);
        Ok(())
    }
}

impl Drop for UdpRegistration {
    /// 离开作用域时注销临时 UDP socket。
    fn drop(&mut self) {
        unregister_udp_socket(self.port, self.socket.clone());
    }
}

/// 根据当前运行平台选择默认 DNS 服务器地址。
pub fn default_server() -> u32 {
    #[cfg(board = "visionfive2")]
    {
        crate::net::VF2_DNS_SERVER
    }
    #[cfg(board = "2k1000")]
    {
        crate::net::LS2K_DNS_SERVER
    }
    #[cfg(not(any(board = "visionfive2", board = "2k1000")))]
    {
        crate::net::QEMU_USER_DNS_SERVER
    }
}

/// 使用默认 DNS 服务器解析域名的 IPv4 地址。
pub fn resolve_ipv4(name: &str) -> SysResult<u32> {
    resolve_ipv4_with_server(name, default_server())
}

/// 使用指定 DNS 服务器解析域名的 IPv4 地址。
pub fn resolve_ipv4_with_server(name: &str, server: u32) -> SysResult<u32> {
    resolve_ipv4_inner(name, server, 0)
}

/// DNS A 记录解析的内部实现。
/// 该函数同时处理 IPv4 字面量、发送 UDP 查询、等待响应以及 CNAME 递归。
fn resolve_ipv4_inner(name: &str, server: u32, depth: usize) -> SysResult<u32> {
    if depth >= DNS_MAX_RECURSION {
        return Err(SysError::ELOOP);
    }
    // 传入值已经是 IPv4 文本地址时无需发起 DNS 查询。
    if let Some(ip) = parse_ipv4_literal(name) {
        return Ok(ip);
    }

    let txid = next_txid();
    let query = build_query(name, txid)?;
    let socket = Arc::new(Mutex::new(UdpSocket::new()));
    {
        // connect 只记录远端地址并分配/绑定本地端口，实际数据仍通过 UDP 发送。
        socket
            .lock()
            .connect(server, DNS_PORT)
            .map_err(|_| SysError::ENETUNREACH)?;
    }
    let src = socket.lock().local_addr().ok_or(SysError::EINVAL)?;
    register_udp_socket(src.1, socket.clone());
    let _registration = UdpRegistration {
        port: src.1,
        socket: socket.clone(),
    };

    // DNS 查询使用 UDP 发送到服务端 53 端口。
    send_udp_packet(src, &query, server, DNS_PORT)?;

    let deadline = crate::timer::get_time_us().saturating_add(DNS_TIMEOUT_US);
    let mut buf = [0u8; DNS_MAX_PACKET];
    loop {
        // 在无后台网络线程的环境中，主动轮询所有网卡以驱动接收路径。
        crate::net::poll_rx_all();
        match socket.lock().try_recv_from(&mut buf) {
            Ok((len, src_ip, src_port)) => {
                // 忽略同一端口上收到的非目标 DNS 服务器响应。
                if src_ip != server || src_port != DNS_PORT {
                    continue;
                }
                match parse_response(&buf[..len], txid)? {
                    DnsAnswer::A(ip) => return Ok(ip),
                    DnsAnswer::Cname(cname) => {
                        return resolve_ipv4_inner(&cname, server, depth + 1);
                    }
                    DnsAnswer::NoData => return Err(SysError::ENODATA),
                }
            }
            Err(SysError::EAGAIN) => {
                // 当前没有可读数据时让出 CPU，直到超时或收到响应。
                if crate::timer::get_time_us() >= deadline {
                    return Err(SysError::ETIMEDOUT);
                }
                crate::task::suspend_current_and_run_next();
            }
            Err(err) => return Err(err),
        }
    }
}

/// DNS 响应中当前解析器关心的结果类型。
enum DnsAnswer {
    /// 成功解析到 A 记录。
    A(u32),
    /// 只解析到 CNAME，需要继续查询别名指向的名称。
    Cname(String),
    /// 响应合法，但没有可用的 A/CNAME 结果。
    NoData,
}

/// 构造一个标准递归查询报文。
/// 查询体只包含一个 question，问题类型固定为 IN/A。
fn build_query(name: &str, txid: u16) -> SysResult<Vec<u8>> {
    let header = DnsHeader::query(txid);
    let question = DnsQuestion::a_record(name);
    let mut packet = Vec::new();
    header.write_to(&mut packet);
    question.write_to(&mut packet)?;
    Ok(packet)
}

/// 解析 DNS 响应报文，提取 A 记录或 CNAME 记录。
fn parse_response(packet: &[u8], txid: u16) -> SysResult<DnsAnswer> {
    if packet.len() < 12 {
        return Err(SysError::EINVAL);
    }
    if read_u16(packet, 0)? != txid {
        return Err(SysError::EINVAL);
    }
    let flags = read_u16(packet, 2)?;
    // QR 位必须为 1，表示这是响应而不是查询。
    if flags & 0x8000 == 0 {
        return Err(SysError::EINVAL);
    }
    // RCODE 非 0 表示服务端返回了错误。
    if flags & 0x000f != 0 {
        return Err(SysError::ENOENT);
    }

    let qdcount = read_u16(packet, 4)? as usize;
    let ancount = read_u16(packet, 6)? as usize;
    let mut offset = 12usize;
    for _ in 0..qdcount {
        // 跳过 question 区域中的 QNAME、QTYPE 和 QCLASS。
        offset = skip_name(packet, offset)?;
        offset = offset.checked_add(4).ok_or(SysError::EINVAL)?;
        if offset > packet.len() {
            return Err(SysError::EINVAL);
        }
    }

    let mut cname = None;
    for _ in 0..ancount {
        // 每个 answer 由 NAME、TYPE、CLASS、TTL、RDLENGTH 和 RDATA 组成。
        offset = skip_name(packet, offset)?;
        if offset.checked_add(10).ok_or(SysError::EINVAL)? > packet.len() {
            return Err(SysError::EINVAL);
        }
        let typ = read_u16(packet, offset)?;
        let class = read_u16(packet, offset + 2)?;
        let rdlen = read_u16(packet, offset + 8)? as usize;
        offset += 10;
        let rdata_end = offset.checked_add(rdlen).ok_or(SysError::EINVAL)?;
        if rdata_end > packet.len() {
            return Err(SysError::EINVAL);
        }

        // 优先返回 IPv4 A 记录；RDATA 正好是 4 字节网络序 IPv4 地址。
        if typ == DNS_TYPE_A && class == DNS_CLASS_IN && rdlen == 4 {
            return Ok(DnsAnswer::A(
                ((packet[offset] as u32) << 24)
                    | ((packet[offset + 1] as u32) << 16)
                    | ((packet[offset + 2] as u32) << 8)
                    | packet[offset + 3] as u32,
            ));
        }
        // 暂存 CNAME，若本响应没有 A 记录，再由调用者递归查询别名。
        if typ == DNS_TYPE_CNAME && class == DNS_CLASS_IN {
            if let Ok((name, _)) = read_name(packet, offset) {
                cname = Some(name);
            }
        }
        offset = rdata_end;
    }

    if let Some(name) = cname {
        Ok(DnsAnswer::Cname(name))
    } else {
        Ok(DnsAnswer::NoData)
    }
}

/// 将点分域名写入 DNS label 编码格式。
fn push_name(packet: &mut Vec<u8>, name: &str) -> SysResult<()> {
    let trimmed = name.trim_end_matches('.');
    if trimmed.is_empty() || trimmed.len() > 253 {
        return Err(SysError::EINVAL);
    }
    for label in trimmed.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(SysError::EINVAL);
        }
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
    }
    packet.push(0);
    Ok(())
}

/// 跳过一个 DNS name，返回其后第一个字节的偏移。
fn skip_name(packet: &[u8], offset: usize) -> SysResult<usize> {
    read_name(packet, offset).map(|(_, next)| next)
}

/// 读取 DNS name，支持 RFC 1035 中的压缩指针格式。
///
/// 返回值中的第二项是线性读取时应继续解析的位置；如果遇到压缩指针，
/// 它指向压缩指针本身之后的位置，而不是跳转目标之后的位置。
fn read_name(packet: &[u8], mut offset: usize) -> SysResult<(String, usize)> {
    let mut labels = Vec::new();
    let mut jumped = false;
    let mut next = offset;
    // 防止畸形报文构造指针环导致无限循环。
    let mut guard = 0usize;

    loop {
        if offset >= packet.len() {
            return Err(SysError::EINVAL);
        }
        guard += 1;
        if guard > packet.len() {
            return Err(SysError::ELOOP);
        }
        let len = packet[offset];
        if len & 0xc0 == 0xc0 {
            // 高两位为 11 表示后 14 位是指向另一个 name 的偏移。
            if offset + 1 >= packet.len() {
                return Err(SysError::EINVAL);
            }
            let ptr = (((len & 0x3f) as usize) << 8) | packet[offset + 1] as usize;
            if !jumped {
                next = offset + 2;
            }
            offset = ptr;
            jumped = true;
            continue;
        }
        if len & 0xc0 != 0 {
            return Err(SysError::EINVAL);
        }
        offset += 1;
        if len == 0 {
            // 零长度 label 表示域名结束。
            if !jumped {
                next = offset;
            }
            break;
        }
        let end = offset.checked_add(len as usize).ok_or(SysError::EINVAL)?;
        if end > packet.len() {
            return Err(SysError::EINVAL);
        }
        let label = core::str::from_utf8(&packet[offset..end]).map_err(|_| SysError::EINVAL)?;
        labels.push(label.to_string());
        offset = end;
    }

    Ok((labels.join("."), next))
}

/// 从报文中按网络序读取一个 u16。
fn read_u16(packet: &[u8], offset: usize) -> SysResult<u16> {
    if offset + 2 > packet.len() {
        return Err(SysError::EINVAL);
    }
    Ok(u16::from_be_bytes([packet[offset], packet[offset + 1]]))
}

/// 按网络序向报文追加一个 u16。
fn push_u16(packet: &mut Vec<u8>, value: u16) {
    packet.extend_from_slice(&value.to_be_bytes());
}

/// 生成 DNS transaction ID。
/// 这里使用递增计数，足够区分当前内核中串行/少量并发的临时 DNS 查询。
fn next_txid() -> u16 {
    static TXID: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0x4b58);
    TXID.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

/// 尝试把输入解析为 IPv4 字面量。
fn parse_ipv4_literal(name: &str) -> Option<u32> {
    let mut ip = 0u32;
    let mut count = 0usize;
    for part in name.split('.') {
        if part.is_empty() || part.len() > 3 {
            return None;
        }
        let mut value = 0u32;
        for byte in part.bytes() {
            if !byte.is_ascii_digit() {
                return None;
            }
            value = value * 10 + (byte - b'0') as u32;
            if value > 255 {
                return None;
            }
        }
        ip = (ip << 8) | value;
        count += 1;
    }
    if count == 4 { Some(ip) } else { None }
}
