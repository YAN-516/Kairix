#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{accept, bind, close, connect, fork, listen, recvfrom, sendto, sleep, socket, wait};

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const LOOPBACK_IP: u32 = 0x7F000001;
const TCP_PORT: u16 = 9000;

#[repr(C)]
#[derive(Clone, Copy)]
struct SockAddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: u32,
    sin_zero: [u8; 8],
}

impl SockAddrIn {
    fn new(ip: u32, port: u16) -> Self {
        Self {
            sin_family: AF_INET as u16,
            sin_port: port.to_be(),
            sin_addr: ip,
            sin_zero: [0; 8],
        }
    }
}

fn server_main() -> i32 {
    println!("[TCP-SOCK-TEST][server] start");
    let fd = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if fd < 0 {
        println!("[TCP-SOCK-TEST][server] socket failed: {}", fd);
        return -1;
    }

    let addr = SockAddrIn::new(LOOPBACK_IP, TCP_PORT);
    if bind(
        fd as usize,
        &addr as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    ) < 0
    {
        println!("[TCP-SOCK-TEST][server] bind failed");
        let _ = close(fd as usize);
        return -1;
    }

    if listen(fd as usize, 1) < 0 {
        println!("[TCP-SOCK-TEST][server] listen failed");
        let _ = close(fd as usize);
        return -1;
    }

    let mut peer = SockAddrIn::new(0, 0);
    let mut peer_len = core::mem::size_of::<SockAddrIn>();
    let conn_fd = loop {
        let accepted = accept(
            fd as usize,
            &mut peer as *mut SockAddrIn as *mut u8,
            &mut peer_len as *mut usize,
        );
        if accepted >= 0 {
            break accepted as usize;
        }
    };
    println!("[TCP-SOCK-TEST][server] accepted client");

    let mut buf = [0u8; 64];
    let mut from_len = 0usize;
    let payload_len = loop {
        let n = recvfrom(
            conn_fd,
            buf.as_mut_ptr(),
            buf.len(),
            0,
            core::ptr::null_mut(),
            &mut from_len as *mut usize,
        );
        if n > 0 {
            break n as usize;
        }
    };

    let got = &buf[..payload_len];
    println!("[TCP-SOCK-TEST][server] got {} bytes", payload_len);
    if sendto(conn_fd, got.as_ptr(), got.len(), 0, core::ptr::null(), 0) < 0 {
        println!("[TCP-SOCK-TEST][server] write echo failed");
        let _ = close(conn_fd);
        let _ = close(fd as usize);
        return -1;
    }

    let _ = close(conn_fd);
    let _ = close(fd as usize);
    println!("[TCP-SOCK-TEST][server] done");
    0
}

fn client_main() -> i32 {
    println!("[TCP-SOCK-TEST][client] start");
    let addr = SockAddrIn::new(LOOPBACK_IP, TCP_PORT);
    let fd = loop {
        let fd = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        if fd < 0 {
            println!("[TCP-SOCK-TEST][client] socket failed: {}", fd);
            return -1;
        }

        let ret = connect(
            fd as usize,
            &addr as *const SockAddrIn as *const u8,
            core::mem::size_of::<SockAddrIn>(),
        );
        if ret >= 0 {
            break fd as usize;
        }

        let _ = close(fd as usize);
    };
    println!("[TCP-SOCK-TEST][client] connected");

    let payload = b"tcp-socket-hello";
    if sendto(
        fd as usize,
        payload.as_ptr(),
        payload.len(),
        0,
        core::ptr::null(),
        0,
    ) < 0
    {
        println!("[TCP-SOCK-TEST][client] write failed");
        let _ = close(fd as usize);
        return -1;
    }

    let mut buf = [0u8; 64];
    let mut from_len = 0usize;
    let got_len = loop {
        let n = recvfrom(
            fd as usize,
            buf.as_mut_ptr(),
            buf.len(),
            0,
            core::ptr::null_mut(),
            &mut from_len as *mut usize,
        );
        if n > 0 {
            break n as usize;
        }
    };

    if &buf[..got_len] != payload {
        println!("[TCP-SOCK-TEST][client] payload mismatch");
        let _ = close(fd as usize);
        return -1;
    }

    let _ = close(fd as usize);
    println!("[TCP-SOCK-TEST][client] done");
    0
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let pid = fork();
    if pid < 0 {
        println!("[TCP-SOCK-TEST] fork failed: {}", pid);
        return -1;
    }

    if pid == 0 {
        server_main()
    } else {
        sleep(20);
        let ret = client_main();
        let mut exit_code = 0;
        let _ = wait(&mut exit_code);
        ret
    }
}
