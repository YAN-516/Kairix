#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{OpenFlags, close, open, read, write};

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("test1");
    let test_str = "Hello, world!";
    let filea = "filea";
    let fd = open(-100, filea, OpenFlags::O_CREAT | OpenFlags::WRONLY, 0);
    assert!(fd > 0);
    let fd = fd as usize;
    write(fd, test_str.as_bytes());
    close(fd);
 
    let fd = open(-100, filea, OpenFlags::RDONLY, 0);
    assert!(fd > 0);
    let fd = fd as usize;
    let mut buffer = [0u8; 100];
    let read_len = read(fd, &mut buffer) as usize;
    close(fd);

    assert_eq!(test_str, core::str::from_utf8(&buffer[..read_len]).unwrap(),);
    println!("file_test passed!");
    0
}
