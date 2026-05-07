#![allow(missing_docs)]

use crate::net::ip::ip_queue_xmit;
use crate::net::skb::Skb;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};
use log::{error, info};
use polyhal::println;
use crate::sync::SpinNoIrqLock;

pub const TCP_FLAG_FIN: u8 = 0x01;
pub const TCP_FLAG_SYN: u8 = 0x02;
pub const TCP_FLAG_RST: u8 = 0x04;
pub const TCP_FLAG_PSH: u8 = 0x08;
pub const TCP_FLAG_ACK: u8 = 0x10;

const DEFAULT_WINDOW: u16 = 4096;
const KERNEL_TCP_SERVICE_PORT: u16 = 8080;

static KERNEL_NEXT_ISS: AtomicU32 = AtomicU32::new(0x1234_0000);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KernelTcpState {
    SynReceived,
    Established,
    LastAck,
}

#[derive(Clone, Copy, Debug)]
struct KernelTcpConn {
    remote_ip: u32,
    local_ip: u32,
    remote_port: u16,
    local_port: u16,
    state: KernelTcpState,
    snd_nxt: u32,
    rcv_nxt: u32,
}

#[derive(Clone)]
struct PendingTx {
    local_ip: u32,
    remote_ip: u32,
    local_port: u16,
    remote_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    payload: Vec<u8>,
}

static KERNEL_TCP_CONNS: SpinNoIrqLock<Vec<KernelTcpConn>> = SpinNoIrqLock::new(Vec::new());

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct TcpHeader {
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    data_offset_reserved: u8,
    flags: u8,
    window: u16,
    checksum: u16,
    urgent_ptr: u16,
}

impl TcpHeader {
    fn size() -> usize {
        core::mem::size_of::<TcpHeader>()
    }

    fn header_len(&self) -> usize {
        ((self.data_offset_reserved >> 4) as usize) * 4
    }
}

fn checksum_fold(mut sum: u32) -> u16 {
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn tcp_checksum(src_ip: u32, dst_ip: u32, segment: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    sum += ((src_ip >> 16) & 0xFFFF) as u32;
    sum += (src_ip & 0xFFFF) as u32;
    sum += ((dst_ip >> 16) & 0xFFFF) as u32;
    sum += (dst_ip & 0xFFFF) as u32;
    sum += 6u32;
    sum += segment.len() as u32;

    let mut i = 0usize;
    while i + 1 < segment.len() {
        let word = ((segment[i] as u16) << 8) | (segment[i + 1] as u16);
        sum += word as u32;
        i += 2;
    }
    if i < segment.len() {
        sum += (segment[i] as u32) << 8;
    }

    checksum_fold(sum)
}

fn tcp_seq_advance(seq: u32, flags: u8, payload_len: usize) -> u32 {
    let syn_fin = ((flags & TCP_FLAG_SYN != 0) as u32) + ((flags & TCP_FLAG_FIN != 0) as u32);
    seq.wrapping_add(payload_len as u32).wrapping_add(syn_fin)
}

fn send_unmatched_rst(
    src_ip: u32,
    dst_ip: u32,
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    payload_len: usize,
) {
    if (flags & TCP_FLAG_RST) != 0 {
        return;
    }

    if (flags & TCP_FLAG_ACK) != 0 {
        let _ = tcp_send_segment(dst_ip, src_ip, dst_port, src_port, ack, 0, TCP_FLAG_RST, &[
        ]);
        return;
    }

    let ack_num = tcp_seq_advance(seq, flags, payload_len);
    let _ = tcp_send_segment(
        dst_ip,
        src_ip,
        dst_port,
        src_port,
        0,
        ack_num,
        TCP_FLAG_RST | TCP_FLAG_ACK,
        &[],
    );
}

fn find_kernel_conn_idx(
    conns: &[KernelTcpConn],
    remote_ip: u32,
    local_ip: u32,
    remote_port: u16,
    local_port: u16,
) -> Option<usize> {
    conns.iter().position(|c| {
        c.remote_ip == remote_ip
            && c.local_ip == local_ip
            && c.remote_port == remote_port
            && c.local_port == local_port
    })
}

fn dispatch_kernel_tcp_service(
    src_ip: u32,
    dst_ip: u32,
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    payload: &[u8],
) -> bool {
    let mut pending: Vec<PendingTx> = Vec::new();
    let mut handled = false;

    {
        let mut conns = KERNEL_TCP_CONNS.lock();
        if let Some(idx) = find_kernel_conn_idx(&conns, src_ip, dst_ip, src_port, dst_port) {
            handled = true;
            let conn = &mut conns[idx];

            if (flags & TCP_FLAG_RST) != 0 {
                conns.swap_remove(idx);
            } else {
                match conn.state {
                    KernelTcpState::SynReceived => {
                        if (flags & TCP_FLAG_ACK) != 0 && ack == conn.snd_nxt {
                            conn.state = KernelTcpState::Established;
                        }
                    }
                    KernelTcpState::Established => {
                        if (flags & TCP_FLAG_FIN) != 0 {
                            conn.rcv_nxt = seq.wrapping_add(1);
                            pending.push(PendingTx {
                                local_ip: conn.local_ip,
                                remote_ip: conn.remote_ip,
                                local_port: conn.local_port,
                                remote_port: conn.remote_port,
                                seq: conn.snd_nxt,
                                ack: conn.rcv_nxt,
                                flags: TCP_FLAG_ACK,
                                payload: Vec::new(),
                            });
                            pending.push(PendingTx {
                                local_ip: conn.local_ip,
                                remote_ip: conn.remote_ip,
                                local_port: conn.local_port,
                                remote_port: conn.remote_port,
                                seq: conn.snd_nxt,
                                ack: conn.rcv_nxt,
                                flags: TCP_FLAG_FIN | TCP_FLAG_ACK,
                                payload: Vec::new(),
                            });
                            conn.snd_nxt = conn.snd_nxt.wrapping_add(1);
                            conn.state = KernelTcpState::LastAck;
                        } else if !payload.is_empty() {
                            conn.rcv_nxt = seq.wrapping_add(payload.len() as u32);
                            pending.push(PendingTx {
                                local_ip: conn.local_ip,
                                remote_ip: conn.remote_ip,
                                local_port: conn.local_port,
                                remote_port: conn.remote_port,
                                seq: conn.snd_nxt,
                                ack: conn.rcv_nxt,
                                flags: TCP_FLAG_ACK,
                                payload: Vec::new(),
                            });
                            pending.push(PendingTx {
                                local_ip: conn.local_ip,
                                remote_ip: conn.remote_ip,
                                local_port: conn.local_port,
                                remote_port: conn.remote_port,
                                seq: conn.snd_nxt,
                                ack: conn.rcv_nxt,
                                flags: TCP_FLAG_ACK | TCP_FLAG_PSH,
                                payload: payload.to_vec(),
                            });
                            conn.snd_nxt = conn.snd_nxt.wrapping_add(payload.len() as u32);
                        }
                    }
                    KernelTcpState::LastAck => {
                        if (flags & TCP_FLAG_ACK) != 0 && ack == conn.snd_nxt {
                            conns.swap_remove(idx);
                        }
                    }
                }
            }
        } else if dst_port == KERNEL_TCP_SERVICE_PORT
            && (flags & TCP_FLAG_SYN) != 0
            && (flags & TCP_FLAG_ACK) == 0
        {
            handled = true;
            let iss = KERNEL_NEXT_ISS.fetch_add(0x1000, Ordering::Relaxed);
            let conn = KernelTcpConn {
                remote_ip: src_ip,
                local_ip: dst_ip,
                remote_port: src_port,
                local_port: dst_port,
                state: KernelTcpState::SynReceived,
                snd_nxt: iss.wrapping_add(1),
                rcv_nxt: seq.wrapping_add(1),
            };
            pending.push(PendingTx {
                local_ip: conn.local_ip,
                remote_ip: conn.remote_ip,
                local_port: conn.local_port,
                remote_port: conn.remote_port,
                seq: iss,
                ack: conn.rcv_nxt,
                flags: TCP_FLAG_SYN | TCP_FLAG_ACK,
                payload: Vec::new(),
            });
            conns.push(conn);
        }
    }

    for tx in pending.iter() {
        let _ = tcp_send_segment(
            tx.local_ip,
            tx.remote_ip,
            tx.local_port,
            tx.remote_port,
            tx.seq,
            tx.ack,
            tx.flags,
            &tx.payload,
        );
    }

    handled
}

fn is_kernel_service_reflection(src_ip: u32, dst_ip: u32, src_port: u16, dst_port: u16) -> bool {
    let conns = KERNEL_TCP_CONNS.lock();
    conns.iter().any(|c| {
        c.local_ip == src_ip
            && c.remote_ip == dst_ip
            && c.local_port == src_port
            && c.remote_port == dst_port
    })
}

fn try_dispatch(
    src_ip: u32,
    dst_ip: u32,
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    payload: &[u8],
) -> bool {
    crate::socket::tcp::dispatch_tcp_segment(
        src_ip, dst_ip, src_port, dst_port, seq, ack, flags, payload,
    )
}

fn try_dispatch_or_rst(
    src_ip: u32,
    dst_ip: u32,
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    payload: &[u8],
    payload_len: usize,
) -> bool {
    // Loopback may feed back packets just sent by kernel service; avoid reflexive RST.
    let _ret1 = is_kernel_service_reflection(src_ip, dst_ip, src_port, dst_port);
    let _ret2 =
        dispatch_kernel_tcp_service(src_ip, dst_ip, src_port, dst_port, seq, ack, flags, payload);
    // error!(
    //     "Attempting to dispatch TCP segment: {}.{}.{}.{}:{} -> {}.{}.{}.{}:{} flags=0x{:02x} seq={} ack={} payload_len={}",
    //     (src_ip >> 24) & 0xFF,
    //     (src_ip >> 16) & 0xFF,
    //     (src_ip >> 8) & 0xFF,
    //     src_ip & 0xFF,
    //     src_port,
    //     (dst_ip >> 24) & 0xFF,
    //     (dst_ip >> 16) & 0xFF,
    //     (dst_ip >> 8) & 0xFF,
    //     dst_ip & 0xFF,
    //     dst_port,
    //     flags,
    //     seq,
    //     ack,
    //     payload_len
    // );

    if try_dispatch(src_ip, dst_ip, src_port, dst_port, seq, ack, flags, payload) {
        return true;
    }
    send_unmatched_rst(
        src_ip,
        dst_ip,
        src_port,
        dst_port,
        seq,
        ack,
        flags,
        payload_len,
    );
    false
}

pub fn tcp_send_segment(
    local_ip: u32,
    remote_ip: u32,
    local_port: u16,
    remote_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    payload: &[u8],
) -> Result<(), &'static str> {
    let total_len = TcpHeader::size() + payload.len();
    let mut skb = Skb::new(total_len);
    let seg = skb.put(total_len).ok_or("tcp skb alloc failed")?;

    for b in seg.iter_mut() {
        *b = 0;
    }

    let hdr = unsafe { &mut *(seg.as_mut_ptr() as *mut TcpHeader) };
    hdr.src_port = local_port.to_be();
    hdr.dst_port = remote_port.to_be();
    hdr.seq = seq.to_be();
    hdr.ack = ack.to_be();
    hdr.data_offset_reserved = 5 << 4;
    hdr.flags = flags;
    hdr.window = DEFAULT_WINDOW.to_be();
    hdr.checksum = 0;
    hdr.urgent_ptr = 0;

    if !payload.is_empty() {
        seg[TcpHeader::size()..].copy_from_slice(payload);
    }

    let checksum = tcp_checksum(local_ip, remote_ip, seg);
    let hdr = unsafe { &mut *(seg.as_mut_ptr() as *mut TcpHeader) };
    hdr.checksum = checksum.to_be();
    // error!(
    //     "TCP: sending segment {}.{}.{}.{}:{} -> {}.{}.{}.{}:{} flags=0x{:02x} seq={} ack={} payload_len={}",
    //     (local_ip >> 24) & 0xFF,
    //     (local_ip >> 16) & 0xFF,
    //     (local_ip >> 8) & 0xFF,
    //     local_ip & 0xFF,
    //     local_port,
    //     (remote_ip >> 24) & 0xFF,
    //     (remote_ip >> 16) & 0xFF,
    //     (remote_ip >> 8) & 0xFF,
    //     remote_ip & 0xFF,
    //     remote_port,
    //     flags,
    //     seq,
    //     ack,
    //     payload.len()
    // );
    ip_queue_xmit(skb, local_ip, remote_ip, 6)?;
    Ok(())
}

pub fn tcp_rcv(mut skb: Skb, src_ip: u32, dst_ip: u32) -> Result<(Skb, u32, u16), &'static str> {
    if skb.len() < TcpHeader::size() {
        return Err("TCP packet too short");
    }
    let (src_port, dst_port, seq, ack, flags, hdr_len, payload_len) = {
        let seg = skb.data();
        let hdr = unsafe { &*(seg.as_ptr() as *const TcpHeader) };
        let hdr_len = hdr.header_len();
        if hdr_len < TcpHeader::size() || seg.len() < hdr_len {
            return Err("TCP header invalid");
        }

        if tcp_checksum(src_ip, dst_ip, seg) != 0 {
            return Err("TCP checksum invalid");
        }

        (
            u16::from_be(hdr.src_port),
            u16::from_be(hdr.dst_port),
            u32::from_be(hdr.seq),
            u32::from_be(hdr.ack),
            hdr.flags,
            hdr_len,
            seg.len() - hdr_len,
        )
    };

    info!(
        "TCP: rx {}.{}.{}.{}:{} -> {}.{}.{}.{}:{} flags=0x{:02x} FIN={} SYN={} RST={} PSH={} ACK={} seq={} ack={} payload_len={}",
        (src_ip >> 24) & 0xFF,
        (src_ip >> 16) & 0xFF,
        (src_ip >> 8) & 0xFF,
        src_ip & 0xFF,
        src_port,
        (dst_ip >> 24) & 0xFF,
        (dst_ip >> 16) & 0xFF,
        (dst_ip >> 8) & 0xFF,
        dst_ip & 0xFF,
        dst_port,
        flags,
        (flags & TCP_FLAG_FIN) != 0,
        (flags & TCP_FLAG_SYN) != 0,
        (flags & TCP_FLAG_RST) != 0,
        (flags & TCP_FLAG_PSH) != 0,
        (flags & TCP_FLAG_ACK) != 0,
        seq,
        ack,
        payload_len
    );
    skb.pull(hdr_len);
    let payload = skb.data();

    let is_rst = (flags & TCP_FLAG_RST) != 0;
    let is_syn = (flags & TCP_FLAG_SYN) != 0;
    let is_fin = (flags & TCP_FLAG_FIN) != 0;
    let is_ack = (flags & TCP_FLAG_ACK) != 0;
    let has_data = payload_len > 0;
    // error!(
    //     "TCP: flags: {}{}{}{}{}{}",
    //     if is_syn { "SYN " } else { "" },
    //     if is_ack { "ACK " } else { "" },
    //     if is_fin { "FIN " } else { "" },
    //     if is_rst { "RST " } else { "" },
    //     if (flags & TCP_FLAG_PSH) != 0 {
    //         "PSH "
    //     } else {
    //         ""
    //     },
    //     if has_data { "DATA" } else { "" }
    // );
    // RST should not trigger any response.
    if is_rst {
        return Ok((skb, src_ip, src_port));
    }

    // New connection attempt (SYN without ACK): listener path or reset if closed.
    if is_syn && !is_ack {
        let _ = try_dispatch_or_rst(
            src_ip,
            dst_ip,
            src_port,
            dst_port,
            seq,
            ack,
            flags,
            payload,
            payload_len,
        );
        return Ok((skb, src_ip, src_port));
    }

    // Established-connection traffic and close handshake traffic.
    if has_data || is_fin || is_ack || is_syn {
        // println!("TCP: dispatching to socket layer");
        let _ = try_dispatch_or_rst(
            src_ip,
            dst_ip,
            src_port,
            dst_port,
            seq,
            ack,
            flags,
            payload,
            payload_len,
        );
        return Ok((skb, src_ip, src_port));
    }
    // Unknown/empty control combination: conservative reset.
    send_unmatched_rst(
        src_ip,
        dst_ip,
        src_port,
        dst_port,
        seq,
        ack,
        flags,
        payload_len,
    );
    Ok((skb, src_ip, src_port))
}
