//!Stdin & Stdout
use super::vfs::file::File;
use crate::mm::UserBuffer;
#[cfg(target_arch = "riscv64")]
use crate::sbi::console_getchar;
use polyhal::debug_console::DebugConsole;
use polyhal::{print, println};

use crate::fs::vfs::FileInner;
use crate::task::suspend_current_and_run_next;
use spin::MutexGuard;
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
        assert_eq!(user_buf.len(), 1);
        // busy loop
        let c;
        loop {
            // c = console_getchar();
            if let Some(buf) = DebugConsole::getchar(){
                if buf == 0 {
                    suspend_current_and_run_next();
                    continue;
                } else {
                    c = buf;
                    break;
                }
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
