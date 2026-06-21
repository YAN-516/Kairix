#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{accept, bind, close, connect, fcntl, getsockopt, listen, recvfrom, setsockopt, socket};

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const SOCK_DGRAM: i32 = 2;
const SOCK_NONBLOCK: i32 = 0o0004000;
const O_NONBLOCK: usize = 0o4000;
const IPPROTO_TCP: i32 = 6;
const SOL_SOCKET: i32 = 1;
const SO_ERROR: i32 = 4;
const SO_RCVTIMEO: i32 = 20;
const MSG_DONTWAIT: i32 = 0x40;
const F_GETFL: usize = 3;
const F_SETFL: usize = 4;
const LOOPBACK: u32 = 0x7F000001;
const LISTEN_PORT: u16 = 9201;
const CONNECT_PORT: u16 = 9202;
const EAGAIN_RET: isize = -11;
const EINPROGRESS_RET: isize = -115;

#[repr(C)]
#[derive(Clone, Copy)]
struct SockAddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: u32,
    sin_zero: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SockTimeval {
    sec: i64,
    usec: i64,
}

impl SockAddrIn {
    fn new(ip: u32, port: u16) -> Self {
        Self {
            sin_family: AF_INET as u16,
            sin_port: port.to_be(),
            sin_addr: ip.to_be(),
            sin_zero: [0; 8],
        }
    }
}

fn expect(name: &str, ok: bool) -> bool {
    println!("[SOCK-SEM] {} {}", name, if ok { "ok" } else { "FAIL" });
    ok
}

fn udp_socket_nonblock() -> bool {
    let fd = socket(AF_INET, SOCK_DGRAM | SOCK_NONBLOCK, 0);
    if fd < 0 {
        return expect("udp socket nonblock create", false);
    }
    let mut buf = [0u8; 1];
    let ret = recvfrom(fd as usize, buf.as_mut_ptr(), buf.len(), 0, core::ptr::null_mut(), core::ptr::null_mut());
    let _ = close(fd as usize);
    expect("udp socket O_NONBLOCK recv", ret == EAGAIN_RET)
}

fn udp_fcntl_nonblock() -> bool {
    let fd = socket(AF_INET, SOCK_DGRAM, 0);
    if fd < 0 {
        return expect("udp fcntl create", false);
    }
    let flags = fcntl(fd as usize, F_GETFL, 0);
    let set = if flags >= 0 {
        fcntl(fd as usize, F_SETFL, flags as usize | O_NONBLOCK)
    } else {
        flags
    };
    let mut buf = [0u8; 1];
    let ret = recvfrom(fd as usize, buf.as_mut_ptr(), buf.len(), 0, core::ptr::null_mut(), core::ptr::null_mut());
    let _ = close(fd as usize);
    expect("udp fcntl O_NONBLOCK recv", flags >= 0 && set == 0 && ret == EAGAIN_RET)
}

fn udp_msg_dontwait() -> bool {
    let fd = socket(AF_INET, SOCK_DGRAM, 0);
    if fd < 0 {
        return expect("udp MSG_DONTWAIT create", false);
    }
    let mut buf = [0u8; 1];
    let ret = recvfrom(fd as usize, buf.as_mut_ptr(), buf.len(), MSG_DONTWAIT, core::ptr::null_mut(), core::ptr::null_mut());
    let _ = close(fd as usize);
    expect("udp MSG_DONTWAIT recv", ret == EAGAIN_RET)
}

fn udp_rcvtimeo() -> bool {
    let fd = socket(AF_INET, SOCK_DGRAM, 0);
    if fd < 0 {
        return expect("udp SO_RCVTIMEO create", false);
    }
    let tv = SockTimeval { sec: 0, usec: 20_000 };
    let set = setsockopt(
        fd as usize,
        SOL_SOCKET,
        SO_RCVTIMEO,
        &tv as *const SockTimeval as *const u8,
        core::mem::size_of::<SockTimeval>(),
    );
    let mut got = SockTimeval { sec: -1, usec: -1 };
    let mut got_len = core::mem::size_of::<SockTimeval>() as u32;
    let get = getsockopt(
        fd as usize,
        SOL_SOCKET,
        SO_RCVTIMEO,
        &mut got as *mut SockTimeval as *mut u8,
        &mut got_len as *mut u32,
    );
    let mut buf = [0u8; 1];
    let ret = recvfrom(fd as usize, buf.as_mut_ptr(), buf.len(), 0, core::ptr::null_mut(), core::ptr::null_mut());
    let _ = close(fd as usize);
    expect("udp SO_RCVTIMEO recv", set == 0 && get == 0 && got.usec == 20_000 && ret == EAGAIN_RET)
}

fn tcp_accept_nonblock() -> bool {
    let fd = socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, IPPROTO_TCP);
    if fd < 0 {
        return expect("tcp accept nonblock create", false);
    }
    let addr = SockAddrIn::new(LOOPBACK, LISTEN_PORT);
    let b = bind(fd as usize, &addr as *const SockAddrIn as *const u8, core::mem::size_of::<SockAddrIn>());
    let l = if b == 0 { listen(fd as usize, 4) } else { b };
    let ret = if l == 0 {
        accept(fd as usize, core::ptr::null_mut(), core::ptr::null_mut())
    } else {
        l
    };
    let _ = close(fd as usize);
    expect("tcp O_NONBLOCK accept", b == 0 && l == 0 && ret == EAGAIN_RET)
}

fn tcp_connect_nonblock() -> bool {
    let fd = socket(AF_INET, SOCK_STREAM | SOCK_NONBLOCK, IPPROTO_TCP);
    if fd < 0 {
        return expect("tcp connect nonblock create", false);
    }
    let addr = SockAddrIn::new(LOOPBACK, CONNECT_PORT);
    let ret = connect(fd as usize, &addr as *const SockAddrIn as *const u8, core::mem::size_of::<SockAddrIn>());
    let mut so_error = -1i32;
    let mut optlen = core::mem::size_of::<i32>() as u32;
    let get = getsockopt(
        fd as usize,
        SOL_SOCKET,
        SO_ERROR,
        &mut so_error as *mut i32 as *mut u8,
        &mut optlen as *mut u32,
    );
    let _ = close(fd as usize);
    expect("tcp O_NONBLOCK connect", ret == EINPROGRESS_RET && get == 0 && so_error >= 0)
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[SOCK-SEM] start");
    let mut ok = true;
    ok &= udp_socket_nonblock();
    ok &= udp_fcntl_nonblock();
    ok &= udp_msg_dontwait();
    ok &= udp_rcvtimeo();
    ok &= tcp_accept_nonblock();
    ok &= tcp_connect_nonblock();
    println!("[SOCK-SEM] done ok={}", ok);
    if ok { 0 } else { -1 }
}
