#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    accept, bind, close, connect, exit, fork, listen, recvfrom, sendto, sleep, socket, wait,
};

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const SOCK_DGRAM: i32 = 2;
const IPPROTO_TCP: i32 = 6;
const LOOPBACK: u32 = 0x7f00_0001;
const UDP_PORT: u16 = 9210;
const TCP_PORT: u16 = 9211;

#[repr(C)]
#[derive(Clone, Copy)]
struct SockAddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: u32,
    sin_zero: [u8; 8],
}

impl SockAddrIn {
    fn loopback(port: u16) -> Self {
        Self {
            sin_family: AF_INET as u16,
            sin_port: port.to_be(),
            sin_addr: LOOPBACK.to_be(),
            sin_zero: [0; 8],
        }
    }
}

fn bind_loopback(fd: usize, port: u16) -> bool {
    let addr = SockAddrIn::loopback(port);
    bind(
        fd,
        &addr as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    ) == 0
}

fn udp_blocking_wakeup() -> bool {
    let receiver = socket(AF_INET, SOCK_DGRAM, 0);
    if receiver < 0 || !bind_loopback(receiver as usize, UDP_PORT) {
        println!("[SOCKET-WAIT] UDP setup failed fd={}", receiver);
        return false;
    }

    let child = fork();
    if child < 0 {
        let _ = close(receiver as usize);
        return false;
    }
    if child == 0 {
        let _ = close(receiver as usize);
        sleep(20);
        let sender = socket(AF_INET, SOCK_DGRAM, 0);
        if sender < 0 {
            exit(-1);
        }
        let destination = SockAddrIn::loopback(UDP_PORT);
        let byte = [0x5au8];
        let sent = sendto(
            sender as usize,
            byte.as_ptr(),
            byte.len(),
            0,
            &destination as *const SockAddrIn as *const u8,
            core::mem::size_of::<SockAddrIn>(),
        );
        let _ = close(sender as usize);
        exit(if sent == 1 { 0 } else { -1 });
    }

    let mut byte = [0u8; 1];
    let received = recvfrom(
        receiver as usize,
        byte.as_mut_ptr(),
        byte.len(),
        0,
        core::ptr::null_mut(),
        core::ptr::null_mut(),
    );
    let mut child_code = -1;
    let waited = wait(&mut child_code);
    let _ = close(receiver as usize);
    received == 1 && byte[0] == 0x5a && waited == child && child_code == 0
}

fn tcp_blocking_wakeup() -> bool {
    let listener = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if listener < 0
        || !bind_loopback(listener as usize, TCP_PORT)
        || listen(listener as usize, 4) != 0
    {
        println!("[SOCKET-WAIT] TCP setup failed fd={}", listener);
        return false;
    }

    let child = fork();
    if child < 0 {
        let _ = close(listener as usize);
        return false;
    }
    if child == 0 {
        let _ = close(listener as usize);
        sleep(20);
        let client = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        if client < 0 {
            exit(-1);
        }
        let destination = SockAddrIn::loopback(TCP_PORT);
        let connected = connect(
            client as usize,
            &destination as *const SockAddrIn as *const u8,
            core::mem::size_of::<SockAddrIn>(),
        );
        let byte = [0xa5u8];
        let sent = if connected == 0 {
            sendto(
                client as usize,
                byte.as_ptr(),
                byte.len(),
                0,
                core::ptr::null(),
                0,
            )
        } else {
            connected
        };
        let _ = close(client as usize);
        exit(if connected == 0 && sent == 1 { 0 } else { -1 });
    }

    let accepted = accept(
        listener as usize,
        core::ptr::null_mut(),
        core::ptr::null_mut(),
    );
    let mut byte = [0u8; 1];
    let received = if accepted >= 0 {
        recvfrom(
            accepted as usize,
            byte.as_mut_ptr(),
            byte.len(),
            0,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        )
    } else {
        accepted
    };
    let mut child_code = -1;
    let waited = wait(&mut child_code);
    if accepted >= 0 {
        let _ = close(accepted as usize);
    }
    let _ = close(listener as usize);
    accepted >= 0 && received == 1 && byte[0] == 0xa5 && waited == child && child_code == 0
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[SOCKET-WAIT] start");
    let udp_ok = udp_blocking_wakeup();
    println!("[SOCKET-WAIT] UDP blocking wake ok={}", udp_ok);
    let tcp_ok = tcp_blocking_wakeup();
    println!("[SOCKET-WAIT] TCP blocking wake ok={}", tcp_ok);
    if udp_ok && tcp_ok {
        println!("[SOCKET-WAIT] PASS");
        0
    } else {
        println!("[SOCKET-WAIT] FAIL");
        -1
    }
}
