#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{EpollEvent, close, epoll_create1, epoll_ctl, epoll_wait, eventfd2, read, write};

const EFD_SEMAPHORE: i32 = 1;
const EFD_NONBLOCK: i32 = 0o4000;
const EFD_CLOEXEC: i32 = 0o2_000_000;
const EPOLL_CTL_ADD: i32 = 1;
const EPOLLIN: u32 = 0x001;

fn close_pair(epfd: isize, eventfd: isize) {
    if epfd >= 0 {
        let _ = close(epfd as usize);
    }
    if eventfd >= 0 {
        let _ = close(eventfd as usize);
    }
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[eventfd_epoll_test] start");

    let eventfd = eventfd2(0, EFD_NONBLOCK | EFD_CLOEXEC);
    let epfd = epoll_create1(EFD_CLOEXEC);
    if eventfd < 0 || epfd < 0 {
        println!(
            "[eventfd_epoll_test] FAIL create: eventfd={} epfd={}",
            eventfd, epfd
        );
        close_pair(epfd, eventfd);
        return 1;
    }

    let registration = EpollEvent {
        events: EPOLLIN,
        data: 0x1234_5678_9abc_def0,
    };
    let ctl = epoll_ctl(
        epfd as usize,
        EPOLL_CTL_ADD,
        eventfd as usize,
        &registration,
    );
    if ctl != 0 {
        println!("[eventfd_epoll_test] FAIL epoll_ctl={}", ctl);
        close_pair(epfd, eventfd);
        return 2;
    }

    let mut events = [EpollEvent::default(); 1];
    let before = epoll_wait(epfd as usize, &mut events, 0);
    if before != 0 {
        println!("[eventfd_epoll_test] FAIL initially ready={}", before);
        close_pair(epfd, eventfd);
        return 3;
    }

    let value = 7u64;
    let write_ret = write(eventfd as usize, &value.to_ne_bytes());
    let ready = epoll_wait(epfd as usize, &mut events, 0);
    if write_ret != 8
        || ready != 1
        || events[0].events & EPOLLIN == 0
        || events[0].data != registration.data
    {
        println!(
            "[eventfd_epoll_test] FAIL readiness: write={} ready={} events={:#x} data={:#x}",
            write_ret, ready, events[0].events, events[0].data
        );
        close_pair(epfd, eventfd);
        return 4;
    }

    let mut result = [0u8; 8];
    let read_ret = read(eventfd as usize, &mut result);
    if read_ret != 8 || u64::from_ne_bytes(result) != value {
        println!(
            "[eventfd_epoll_test] FAIL read: ret={} value={}",
            read_ret,
            u64::from_ne_bytes(result)
        );
        close_pair(epfd, eventfd);
        return 5;
    }
    if epoll_wait(epfd as usize, &mut events, 0) != 0 || read(eventfd as usize, &mut result) != -11
    {
        println!("[eventfd_epoll_test] FAIL drain/nonblock");
        close_pair(epfd, eventfd);
        return 6;
    }

    let large = u32::MAX as u64;
    if write(eventfd as usize, &large.to_ne_bytes()) != 8
        || write(eventfd as usize, &2u64.to_ne_bytes()) != 8
        || read(eventfd as usize, &mut result) != 8
        || u64::from_ne_bytes(result) != large + 2
    {
        println!("[eventfd_epoll_test] FAIL 64-bit counter");
        close_pair(epfd, eventfd);
        return 7;
    }

    let semaphore = eventfd2(2, EFD_SEMAPHORE | EFD_NONBLOCK);
    if semaphore < 0 {
        println!("[eventfd_epoll_test] FAIL semaphore create={}", semaphore);
        close_pair(epfd, eventfd);
        return 8;
    }
    let first = read(semaphore as usize, &mut result);
    let first_value = u64::from_ne_bytes(result);
    let second = read(semaphore as usize, &mut result);
    let second_value = u64::from_ne_bytes(result);
    let third = read(semaphore as usize, &mut result);
    let _ = close(semaphore as usize);
    close_pair(epfd, eventfd);
    if first != 8 || second != 8 || first_value != 1 || second_value != 1 || third != -11 {
        println!(
            "[eventfd_epoll_test] FAIL semaphore: first={}/{} second={}/{} third={}",
            first, first_value, second, second_value, third
        );
        return 9;
    }

    println!("[eventfd_epoll_test] PASS");
    0
}
