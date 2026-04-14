use alloc::sync::{Arc, Weak};
use fatfs::info;
use spin::{Mutex, MutexGuard};
use lazy_static::lazy_static;
use log::*;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::DentryInner;
use crate::fs::Dentry;
use crate::fs::vfs::inode::InodeInner;
use crate::mm::UserBuffer;
use crate::fs::Inode;
use crate::fs::vfs::FileInner;
use crate::fs::File;
use crate::sbi::console_getchar;   
use crate::console::print;     
use crate::task::suspend_current_and_run_next;
use crate::fs::vfs::OpenFlags;
use core::sync::atomic::Ordering;
#[repr(C)]
#[derive(Clone, Copy)]
/// 终端窗口大小
pub struct WinSize {
    /// 行数
    pub ws_row: u16,
    /// 列数
    pub ws_col: u16,
    /// 水平分辨率（像素）
    pub ws_xpixel: u16,
    /// 垂直分辨率（像素）
    pub ws_ypixel: u16,
}

impl Default for WinSize {
    fn default() -> Self {
        Self { ws_row: 24, ws_col: 80, ws_xpixel: 0, ws_ypixel: 0 }
    }
}

/// 终端状态
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Termios {
    /// 输入模式
    pub c_iflag: u32,
    /// 输出模式
    pub c_oflag: u32,
    /// 控制模式
    pub c_cflag: u32,
    /// 本地模式
    pub c_lflag: u32,
    /// 控制线路
    pub c_line: u8,
    /// 特殊控制字符
    pub c_cc: [u8; 19],
    /// 输入速度
    pub c_ispeed: u32,
    /// 输出速度
    pub c_ospeed: u32,
}

impl Default for Termios {
    fn default() -> Self {
        Self {
            c_iflag: 0o66402,   
            c_oflag: 0o5,
            c_cflag: 0o2277,
            c_lflag: 0o105073,  
            c_line: 0,
            c_cc: [3, 28, 127, 21, 4, 0, 1, 0, 17, 19, 26, 255, 18, 15, 23, 22, 255, 0, 0],
            c_ispeed: 0,
            c_ospeed: 0,
        }
    }
}

impl Termios {
    /// 判断是否开启了 ICRNL 
    pub fn is_icrnl(&self) -> bool {
        const ICRNL: u32 = 0o0000400;
        self.c_iflag & ICRNL != 0
    }
    /// 输入回显
    pub fn is_echo(&self) -> bool {
        const ECHO: u32 = 0o0000010;
        self.c_lflag & ECHO != 0
    }
}

///
pub struct TtyState {
    ///
    pub termios: Termios,
    ///
    pub winsize: WinSize,
    ///
    pub fg_pgid: i32,
}

impl Default for TtyState {
    fn default() -> Self {
        Self {
            termios: Termios::default(),
            winsize: WinSize::default(),
            fg_pgid: 1,
        }
    }
}

lazy_static! {
    ///
    pub static ref TTY_STATE: Mutex<TtyState> = Mutex::new(TtyState::default());
}

///
pub struct TtyFile {
    inner: Mutex<FileInner>,
}

impl TtyFile {
    ///
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self {
            inner: Mutex::new(FileInner { offset: 0, dentry }),
        }
    }
}

impl File for TtyFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool { true }
    fn writable(&self) -> bool { true }

    fn read(&self, mut buf: UserBuffer) -> usize {
        let mut nread = 0usize;
        for slice in buf.buffers.iter_mut() {
            for b in slice.iter_mut() {
                loop {
                    let ch = console_getchar();
                    if ch != 0 {
                        let mut c = ch as u8;
                        
                        let state = TTY_STATE.lock();
                        let icrnl = state.termios.is_icrnl();
                        let _echo = state.termios.is_echo();
                        drop(state);

                        if icrnl && c == b'\r' {
                            c = b'\n';
                        }
                        
                        // if echo {
                        //     print!("{}", c as char);
                        // }

                        *b = c;
                        nread += 1;
                        break;
                    } else {
                        suspend_current_and_run_next();
                    }
                }
            }
        }
        nread
    }

    fn write(&self, buf: UserBuffer) -> usize {
        let mut nwritten = 0usize;
        for slice in buf.buffers.iter() {
            if let Ok(s) = core::str::from_utf8(slice) {
                print!("{}", s);
            } else {
                for &ch in slice.iter() {
                    print!("{}", ch as char);
                }
            }
            nwritten += slice.len();
        }
        nwritten
    }

    fn open(&self) -> Result<usize, i32> { Ok(0) }
    fn release(&self) -> Result<usize, i32> { Ok(0) }
}

///
pub struct TtyDentry {
    inner: DentryInner,
}

impl TtyDentry {
    ///
    pub fn new(name: &str, parent: Option<Weak<dyn Dentry>>) -> Self {
        Self {
            inner: DentryInner::new(name, parent),
        }
    }
}

impl Dentry for TtyDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }
    fn name(&self) -> &str {
        &self.inner.name
    }
    fn open(self: Arc<Self>, _flags: OpenFlags,_mode: InodeMode) -> Option<Arc<dyn File>> {
        Some(Arc::new(TtyFile::new(self)))
    }
}
#[allow(unused)]
///
pub struct TtyInode {
    inner: InodeInner,
}

impl TtyInode {
    ///
    pub fn new() -> Self {
        Self {
            inner: InodeInner::new(0, 0, InodeMode::CHAR),
        }
    }
}

impl Inode for TtyInode {

    fn get_mode(&self) -> InodeMode {
        self.inner.mode
    }

    fn set_size(&self, new_size: usize) {
        self.inner.size.store(new_size, Ordering::SeqCst);
    }
    fn get_size(&self)->usize {
        info!("size:{}", self.inner.size.load(Ordering::SeqCst));
        self.inner.size.load(Ordering::SeqCst)
    }
}