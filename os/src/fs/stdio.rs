//!Stdin & Stdout
use super::vfs::file::File;
use crate::mm::UserBuffer;
use crate::sbi::console_getchar;
use crate::task::suspend_current_and_run_next;
use spin::MutexGuard;
use crate::fs::vfs::FileInner;
///Standard input
pub struct Stdin;
///Standard output
pub struct Stdout;

impl File for Stdin {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        panic!("[Stdin]: don not support get file_inner")
    }

    fn readable(&self) -> bool {
        true
    }
    fn writable(&self) -> bool {
        false
    }
    fn read(&self, mut user_buf: UserBuffer) -> usize {
        if user_buf.len() == 0 {
            return 0;
        }
        let mut c: usize;
        loop {
            c = console_getchar();
            if c == 0 || c == 255 || c == usize::MAX {
                suspend_current_and_run_next();
                continue;
            } else {
                break;
            }
        }
        let ch = c as u8;
        unsafe {
            user_buf.buffers[0].as_mut_ptr().write_volatile(ch);
        }
        1
    }
    
    fn write(&self, _user_buf: UserBuffer) -> usize {
        panic!("Cannot write to stdin!");
    }
}

impl File for Stdout {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        panic!("[Stdout]: don not support get file_inner")
    }
    fn readable(&self) -> bool {
        false
    }
    fn writable(&self) -> bool {
        true
    }
    fn read(&self, _user_buf: UserBuffer) -> usize {
        panic!("Cannot read from stdout!");
    }
    fn write(&self, user_buf: UserBuffer) -> usize {
        for buffer in user_buf.buffers.iter() {
            print!("{}", core::str::from_utf8(*buffer).unwrap());
        }
        user_buf.len()
    }
}
