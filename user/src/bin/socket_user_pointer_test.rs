#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{bind, close, recvfrom, sendmsg, sendto, socket};

const AF_INET: i32 = 2;
const SOCK_DGRAM: i32 = 2;
const MSG_DONTWAIT: i32 = 0x40;
const EFAULT: isize = -14;
const LOOPBACK: u32 = 0x7f00_0001;
const PORT: u16 = 9411;

// Passing these pointers to the kernel must fault their lazy ELF pages in via
// the caller's page table. Do not read them in userspace before sendto/sendmsg.
static SENDTO_PAYLOAD: [u8; 1] = [0x5a];
static SENDMSG_PAYLOAD: [u8; 1] = [0xa5];

#[repr(C)]
#[derive(Clone, Copy)]
struct SockAddrIn {
    family: u16,
    port: u16,
    addr: u32,
    zero: [u8; 8],
}

impl SockAddrIn {
    fn loopback(port: u16) -> Self {
        Self {
            family: AF_INET as u16,
            port: port.to_be(),
            addr: LOOPBACK.to_be(),
            zero: [0; 8],
        }
    }
}

#[repr(C)]
struct Iovec {
    base: usize,
    len: usize,
}

#[repr(C)]
struct MsgHdr {
    name: usize,
    name_len: u32,
    pad1: u32,
    iov: usize,
    iov_len: usize,
    control: usize,
    control_len: usize,
    flags: i32,
    pad2: i32,
}

fn recv_byte(fd: usize, expected: u8) -> bool {
    let mut byte = [0u8; 1];
    recvfrom(
        fd,
        byte.as_mut_ptr(),
        byte.len(),
        MSG_DONTWAIT,
        core::ptr::null_mut(),
        core::ptr::null_mut(),
    ) == 1
        && byte[0] == expected
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[socket_user_pointer_test] start");
    let receiver = socket(AF_INET, SOCK_DGRAM, 0);
    let sender = socket(AF_INET, SOCK_DGRAM, 0);
    if receiver < 0 || sender < 0 {
        println!(
            "[socket_user_pointer_test] FAIL: socket receiver={} sender={}",
            receiver, sender
        );
        return 1;
    }

    let receiver = receiver as usize;
    let sender = sender as usize;
    let destination = SockAddrIn::loopback(PORT);
    let bind_result = bind(
        receiver,
        &destination as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    );
    let sendto_result = if bind_result == 0 {
        sendto(
            sender,
            SENDTO_PAYLOAD.as_ptr(),
            SENDTO_PAYLOAD.len(),
            0,
            &destination as *const SockAddrIn as *const u8,
            core::mem::size_of::<SockAddrIn>(),
        )
    } else {
        bind_result
    };
    let sendto_received = sendto_result == 1 && recv_byte(receiver, 0x5a);

    let iov = Iovec {
        base: SENDMSG_PAYLOAD.as_ptr() as usize,
        len: SENDMSG_PAYLOAD.len(),
    };
    let msg = MsgHdr {
        name: &destination as *const SockAddrIn as usize,
        name_len: core::mem::size_of::<SockAddrIn>() as u32,
        pad1: 0,
        iov: &iov as *const Iovec as usize,
        iov_len: 1,
        control: 0,
        control_len: 0,
        flags: 0,
        pad2: 0,
    };
    let sendmsg_result = sendmsg(sender, &msg as *const MsgHdr as usize, 0);
    let sendmsg_received = sendmsg_result == 1 && recv_byte(receiver, 0xa5);

    let invalid_payload = sendto(
        sender,
        1usize as *const u8,
        1,
        0,
        &destination as *const SockAddrIn as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    );
    let invalid_address = sendto(
        sender,
        SENDTO_PAYLOAD.as_ptr(),
        SENDTO_PAYLOAD.len(),
        0,
        1usize as *const u8,
        core::mem::size_of::<SockAddrIn>(),
    );

    let _ = close(sender);
    let _ = close(receiver);
    let passed = bind_result == 0
        && sendto_received
        && sendmsg_received
        && invalid_payload == EFAULT
        && invalid_address == EFAULT;
    println!(
        "[socket_user_pointer_test] bind={} sendto={} sendmsg={} bad_payload={} bad_addr={} result={}",
        bind_result,
        sendto_result,
        sendmsg_result,
        invalid_payload,
        invalid_address,
        if passed { "PASS" } else { "FAIL" }
    );
    if passed { 0 } else { 2 }
}
