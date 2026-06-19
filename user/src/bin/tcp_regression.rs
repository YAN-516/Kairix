#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{
    accept, bind, close, connect, fork, listen, recvfrom, recvmsg, sendmsg, sendto, shutdown,
    sleep, socket, wait, yield_,
};

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const LOOPBACK_IP: u32 = 0x7F000001;
const QEMU_HOST_IP: u32 = 0x0A000202;
const TCP_PORT: u16 = 9100;
const RST_PORT: u16 = 9101;
const HOST_PROBE_PORT: u16 = 80;
const SHUT_WR: i32 = 1;
const SMALL_TOTAL: usize = 64;
const BIG_TOTAL: usize = 12 * 1024;

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
struct Iovec {
    base: usize,
    len: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Msghdr {
    msg_name: usize,
    msg_namelen: u32,
    __pad1: u32,
    msg_iov: usize,
    msg_iovlen: usize,
    msg_control: usize,
    msg_controllen: usize,
    msg_flags: i32,
    __pad2: i32,
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

fn send_all(fd: usize, buf: &[u8]) -> bool {
    let mut off = 0;
    while off < buf.len() {
        let n = sendto(
            fd,
            buf[off..].as_ptr(),
            buf.len() - off,
            0,
            core::ptr::null(),
            0,
        );
        if n <= 0 {
            println!("[TCP-REG] send failed: {}", n);
            return false;
        }
        off += n as usize;
    }
    true
}

fn recv_exact(fd: usize, buf: &mut [u8]) -> bool {
    let mut off = 0;
    let mut addr_len = 0usize;
    while off < buf.len() {
        let n = recvfrom(
            fd,
            buf[off..].as_mut_ptr(),
            buf.len() - off,
            0,
            core::ptr::null_mut(),
            &mut addr_len as *mut usize,
        );
        if n <= 0 {
            println!("[TCP-REG] recv_exact failed: {}", n);
            return false;
        }
        off += n as usize;
    }
    true
}

fn connect_loop(ip: u32, port: u16, retries: usize) -> isize {
    let addr = SockAddrIn::new(ip, port);
    for _ in 0..retries {
        let fd = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
        if fd < 0 {
            return fd;
        }
        let ret = connect(
            fd as usize,
            &addr as *const SockAddrIn as *const u8,
            core::mem::size_of::<SockAddrIn>(),
        );
        if ret >= 0 {
            return fd;
        }
        let _ = close(fd as usize);
        sleep(20);
    }
    -1
}

fn accept_one(listen_fd: usize) -> isize {
    let mut peer = SockAddrIn::new(0, 0);
    let mut peer_len = core::mem::size_of::<SockAddrIn>();
    loop {
        let fd = accept(
            listen_fd,
            &mut peer as *mut SockAddrIn as *mut u8,
            &mut peer_len as *mut usize,
        );
        if fd >= 0 {
            return fd;
        }
        yield_();
    }
}

fn server_small(conn_fd: usize) -> bool {
    let mut cmd = [0u8; 1];
    if !recv_exact(conn_fd, &mut cmd) || cmd[0] != b'M' {
        return false;
    }
    let mut payload = [0u8; SMALL_TOTAL];
    if !recv_exact(conn_fd, &mut payload) {
        return false;
    }
    send_all(conn_fd, &payload)
}

fn server_big(conn_fd: usize) -> bool {
    let mut cmd = [0u8; 1];
    if !recv_exact(conn_fd, &mut cmd) || cmd[0] != b'B' {
        return false;
    }
    let mut chunk = [0u8; 1024];
    let mut sent = 0;
    while sent < BIG_TOTAL {
        let take = core::cmp::min(chunk.len(), BIG_TOTAL - sent);
        for i in 0..take {
            chunk[i] = b'A' + ((sent + i) % 26) as u8;
        }
        if !send_all(conn_fd, &chunk[..take]) {
            return false;
        }
        sent += take;
    }
    true
}


fn server_msg(conn_fd: usize) -> bool {
    let mut cmd = [0u8; 1];
    if !recv_exact(conn_fd, &mut cmd) || cmd[0] != b'G' {
        return false;
    }
    let mut payload = [0u8; 9];
    if !recv_exact(conn_fd, &mut payload) || &payload != b"hello-msg" {
        return false;
    }
    let part1 = b"reply-";
    let part2 = b"msg";
    let iov = [
        Iovec { base: part1.as_ptr() as usize, len: part1.len() },
        Iovec { base: part2.as_ptr() as usize, len: part2.len() },
    ];
    let msg = Msghdr {
        msg_name: 0,
        msg_namelen: 0,
        __pad1: 0,
        msg_iov: iov.as_ptr() as usize,
        msg_iovlen: iov.len(),
        msg_control: 0,
        msg_controllen: 0,
        msg_flags: 0,
        __pad2: 0,
    };
    sendmsg(conn_fd, &msg as *const Msghdr as usize, 0) == 9
}

fn server_half_close(conn_fd: usize) -> bool {
    let mut cmd = [0u8; 1];
    if !recv_exact(conn_fd, &mut cmd) || cmd[0] != b'H' {
        return false;
    }
    let mut buf = [0u8; 32];
    let mut addr_len = 0usize;
    loop {
        let n = recvfrom(
            conn_fd,
            buf.as_mut_ptr(),
            buf.len(),
            0,
            core::ptr::null_mut(),
            &mut addr_len as *mut usize,
        );
        if n == 0 {
            break;
        }
        if n < 0 {
            println!("[TCP-REG][server] half-close recv failed: {}", n);
            return false;
        }
    }
    send_all(conn_fd, b"half-ok")
}

fn server_main() -> i32 {
    println!("[TCP-REG][server] start");
    let listen_fd = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if listen_fd < 0 {
        println!("[TCP-REG][server] socket failed: {}", listen_fd);
        return -1;
    }
    let addr = SockAddrIn::new(LOOPBACK_IP, TCP_PORT);
    if bind(
        listen_fd as usize,
        &addr as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    ) < 0
    {
        println!("[TCP-REG][server] bind failed");
        let _ = close(listen_fd as usize);
        return -1;
    }
    if listen(listen_fd as usize, 4) < 0 {
        println!("[TCP-REG][server] listen failed");
        let _ = close(listen_fd as usize);
        return -1;
    }

    let mut ok = true;
    for idx in 0..4 {
        let conn_fd = accept_one(listen_fd as usize);
        if conn_fd < 0 {
            ok = false;
            break;
        }
        let conn = conn_fd as usize;
        let case_ok = match idx {
            0 => server_small(conn),
            1 => server_big(conn),
            2 => server_msg(conn),
            _ => server_half_close(conn),
        };
        let _ = close(conn);
        if !case_ok {
            ok = false;
            break;
        }
    }

    let _ = close(listen_fd as usize);
    println!("[TCP-REG][server] done ok={}", ok);
    if ok { 0 } else { -1 }
}

fn client_small() -> bool {
    println!("[TCP-REG][client] multi-small start");
    let fd = connect_loop(LOOPBACK_IP, TCP_PORT, 50);
    if fd < 0 {
        println!("[TCP-REG][client] multi-small connect failed: {}", fd);
        return false;
    }
    if !send_all(fd as usize, b"M") {
        let _ = close(fd as usize);
        return false;
    }
    for i in 0..SMALL_TOTAL {
        let byte = [i as u8];
        if !send_all(fd as usize, &byte) {
            let _ = close(fd as usize);
            return false;
        }
    }
    let mut echo = [0u8; SMALL_TOTAL];
    let ok = recv_exact(fd as usize, &mut echo)
        && echo
            .iter()
            .enumerate()
            .all(|(idx, byte)| *byte == idx as u8);
    let _ = close(fd as usize);
    println!("[TCP-REG][client] multi-small ok={}", ok);
    ok
}

fn client_big() -> bool {
    println!("[TCP-REG][client] big-response start");
    let fd = connect_loop(LOOPBACK_IP, TCP_PORT, 50);
    if fd < 0 {
        println!("[TCP-REG][client] big connect failed: {}", fd);
        return false;
    }
    if !send_all(fd as usize, b"B") {
        let _ = close(fd as usize);
        return false;
    }
    let mut buf = [0u8; 513];
    let mut got = 0;
    let mut ok = true;
    while got < BIG_TOTAL {
        let take = core::cmp::min(buf.len(), BIG_TOTAL - got);
        if !recv_exact(fd as usize, &mut buf[..take]) {
            ok = false;
            break;
        }
        for i in 0..take {
            if buf[i] != b'A' + ((got + i) % 26) as u8 {
                ok = false;
                break;
            }
        }
        if !ok {
            break;
        }
        got += take;
    }
    let _ = close(fd as usize);
    println!("[TCP-REG][client] big-response ok={} bytes={}", ok, got);
    ok
}


fn client_msg() -> bool {
    println!("[TCP-REG][client] sendmsg-recvmsg start");
    let fd = connect_loop(LOOPBACK_IP, TCP_PORT, 50);
    if fd < 0 {
        println!("[TCP-REG][client] msg connect failed: {}", fd);
        return false;
    }
    let cmd = b"G";
    let part1 = b"hello";
    let part2 = b"-msg";
    let send_iov = [
        Iovec { base: cmd.as_ptr() as usize, len: cmd.len() },
        Iovec { base: part1.as_ptr() as usize, len: part1.len() },
        Iovec { base: part2.as_ptr() as usize, len: part2.len() },
    ];
    let send_hdr = Msghdr {
        msg_name: 0,
        msg_namelen: 0,
        __pad1: 0,
        msg_iov: send_iov.as_ptr() as usize,
        msg_iovlen: send_iov.len(),
        msg_control: 0,
        msg_controllen: 0,
        msg_flags: 0,
        __pad2: 0,
    };
    if sendmsg(fd as usize, &send_hdr as *const Msghdr as usize, 0) != 10 {
        println!("[TCP-REG][client] sendmsg failed");
        let _ = close(fd as usize);
        return false;
    }

    let mut recv_a = [0u8; 6];
    let mut recv_b = [0u8; 3];
    let recv_iov = [
        Iovec { base: recv_a.as_mut_ptr() as usize, len: recv_a.len() },
        Iovec { base: recv_b.as_mut_ptr() as usize, len: recv_b.len() },
    ];
    let mut recv_hdr = Msghdr {
        msg_name: 0,
        msg_namelen: 0,
        __pad1: 0,
        msg_iov: recv_iov.as_ptr() as usize,
        msg_iovlen: recv_iov.len(),
        msg_control: 0,
        msg_controllen: 0,
        msg_flags: 0,
        __pad2: 0,
    };
    let n = recvmsg(fd as usize, &mut recv_hdr as *mut Msghdr as usize, 0);
    let ok = n == 9 && &recv_a == b"reply-" && &recv_b == b"msg";
    let _ = close(fd as usize);
    println!("[TCP-REG][client] sendmsg-recvmsg ok={} bytes={}", ok, n);
    ok
}

fn client_half_close() -> bool {
    println!("[TCP-REG][client] half-close start");
    let fd = connect_loop(LOOPBACK_IP, TCP_PORT, 50);
    if fd < 0 {
        println!("[TCP-REG][client] half connect failed: {}", fd);
        return false;
    }
    if !send_all(fd as usize, b"H") {
        let _ = close(fd as usize);
        return false;
    }
    if shutdown(fd as usize, SHUT_WR) < 0 {
        println!("[TCP-REG][client] shutdown(SHUT_WR) failed");
        let _ = close(fd as usize);
        return false;
    }
    let mut reply = [0u8; 7];
    let ok = recv_exact(fd as usize, &mut reply) && &reply == b"half-ok";
    let _ = close(fd as usize);
    println!("[TCP-REG][client] half-close ok={}", ok);
    ok
}

fn client_rst() -> bool {
    println!("[TCP-REG][client] rst start");
    let addr = SockAddrIn::new(LOOPBACK_IP, RST_PORT);
    let fd = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if fd < 0 {
        println!("[TCP-REG][client] rst socket failed: {}", fd);
        return false;
    }
    let ret = connect(
        fd as usize,
        &addr as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    );
    let _ = close(fd as usize);
    let ok = ret < 0;
    println!("[TCP-REG][client] rst ok={} ret={}", ok, ret);
    ok
}

fn qemu_host_probe() -> bool {
    println!("[TCP-REG][client] qemu-host probe start");
    let fd = connect_loop(QEMU_HOST_IP, HOST_PROBE_PORT, 1);
    if fd < 0 {
        println!("[TCP-REG][client] qemu-host probe skipped: no listener on 10.0.2.2:80");
        return true;
    }
    let _ = close(fd as usize);
    println!("[TCP-REG][client] qemu-host probe connected");
    true
}

fn client_main() -> i32 {
    let mut ok = true;
    ok &= client_small();
    ok &= client_big();
    ok &= client_msg();
    ok &= client_half_close();
    ok &= client_rst();
    ok &= qemu_host_probe();
    if ok { 0 } else { -1 }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[TCP-REG] start");
    let pid = fork();
    if pid < 0 {
        println!("[TCP-REG] fork failed: {}", pid);
        return -1;
    }
    if pid == 0 {
        return server_main();
    }

    sleep(50);
    let client_ret = client_main();
    let mut server_code = 0;
    let waited = wait(&mut server_code);
    println!(
        "[TCP-REG] done client={} server_pid={} server_code={}",
        client_ret, waited, server_code
    );
    if client_ret == 0 && server_code == 0 {
        0
    } else {
        -1
    }
}
