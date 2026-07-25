use crate::fs::Dentry;
use crate::fs::File;
use crate::fs::Inode;
use crate::fs::vfs::DentryInner;
use crate::fs::vfs::FileInner;
use crate::fs::vfs::inode::InodeInner;
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::inode::make_rdev;
use crate::mm::UserBuffer;
// #[cfg(target_arch = "riscv64")]
// use crate::sbi::console_getchar;
use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::inode::inode_alloc;
use crate::mm::{translated_ref, write_user_value};
use crate::task::suspend_current_and_run_next;
use crate::task::{current_task, current_user_token};
use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use fatfs::info;
use lazy_static::lazy_static;
use log::*;
use polyhal::debug_console::DebugConsole;
use polyhal::print;
use spin::{Mutex, MutexGuard};
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
        Self {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        }
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

#[repr(C)]
#[derive(Clone, Copy)]
struct KernelTermios {
    c_iflag: u32,
    c_oflag: u32,
    c_cflag: u32,
    c_lflag: u32,
    c_line: u8,
    c_cc: [u8; 19],
}

const _: [(); 36] = [(); core::mem::size_of::<KernelTermios>()];

impl From<Termios> for KernelTermios {
    fn from(value: Termios) -> Self {
        Self {
            c_iflag: value.c_iflag,
            c_oflag: value.c_oflag,
            c_cflag: value.c_cflag,
            c_lflag: value.c_lflag,
            c_line: value.c_line,
            c_cc: value.c_cc,
        }
    }
}

impl From<KernelTermios> for Termios {
    fn from(value: KernelTermios) -> Self {
        Self {
            c_iflag: value.c_iflag,
            c_oflag: value.c_oflag,
            c_cflag: value.c_cflag,
            c_lflag: value.c_lflag,
            c_line: value.c_line,
            c_cc: value.c_cc,
            c_ispeed: 0,
            c_ospeed: 0,
        }
    }
}

impl Default for Termios {
    fn default() -> Self {
        Self {
            c_iflag: 0o66402,
            c_oflag: 0o5,
            c_cflag: 0o2277,
            c_lflag: 0o105073,
            c_line: 0,
            c_cc: [
                3, 28, 127, 21, 4, 0, 1, 0, 17, 19, 26, 255, 18, 15, 23, 22, 255, 0, 0,
            ],
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

    fn is_canonical(&self) -> bool {
        const ICANON: u32 = 0o0000002;
        self.c_lflag & ICANON != 0
    }

    fn min_read_chars(&self) -> usize {
        const VMIN: usize = 6;
        self.c_cc[VMIN] as usize
    }

    fn read_timeout_deciseconds(&self) -> u8 {
        const VTIME: usize = 5;
        self.c_cc[VTIME]
    }

    fn maps_newline_to_crlf(&self) -> bool {
        const OPOST: u32 = 0o0000001;
        const ONLCR: u32 = 0o0000004;
        self.c_oflag & (OPOST | ONLCR) == (OPOST | ONLCR)
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
    static ref TTY_WRITE_LOCK: Mutex<()> = Mutex::new(());
    static ref TTY_LINE_BUFFERS: Mutex<Vec<TtyLineBuffer>> = Mutex::new(Vec::new());
    static ref TTY_INPUT_BUFFER: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());
}

const TTY_LINE_BUFFER_LIMIT: usize = 128;
const ALT_SCREEN_EXIT: &[u8] = b"\x1b[?1049l";

struct TtyLineBuffer {
    owner: usize,
    data: Vec<u8>,
}

fn current_tty_owner() -> usize {
    current_task()
        .map(|task| task.inner_exclusive_access().global_tid)
        .unwrap_or(usize::MAX)
}

fn fill_tty_input_buffer() {
    let mut input = TTY_INPUT_BUFFER.lock();
    while let Some(ch) = DebugConsole::getchar() {
        if ch == 0 {
            break;
        }
        input.push_back(ch as u8);
    }
}

fn tty_input_ready() -> bool {
    fill_tty_input_buffer();
    !TTY_INPUT_BUFFER.lock().is_empty()
}

fn read_tty_input() -> Option<u8> {
    if let Some(ch) = TTY_INPUT_BUFFER.lock().pop_front() {
        return Some(ch);
    }
    DebugConsole::getchar().and_then(|ch| (ch != 0).then_some(ch as u8))
}

fn wrap_alt_screen_sequences(bytes: &[u8]) -> Option<Vec<u8>> {
    if !bytes
        .windows(ALT_SCREEN_EXIT.len())
        .any(|window| window == ALT_SCREEN_EXIT)
    {
        return None;
    }

    let mut output = Vec::with_capacity(bytes.len() + 8);
    let mut offset = 0;
    while offset < bytes.len() {
        let remaining = &bytes[offset..];
        if remaining.starts_with(ALT_SCREEN_EXIT) {
            output.extend_from_slice(ALT_SCREEN_EXIT);
            output.extend_from_slice(b"\x1b[r\x1b[2J\x1b[H");
            offset += ALT_SCREEN_EXIT.len();
        } else {
            output.push(bytes[offset]);
            offset += 1;
        }
    }
    Some(output)
}

fn should_start_line_buffer(bytes: &[u8]) -> bool {
    bytes.starts_with(b"cpid: ")
}

fn print_tty_bytes(bytes: &[u8]) {
    if let Ok(s) = core::str::from_utf8(bytes) {
        print!("{}", s);
    } else {
        for &ch in bytes.iter() {
            print!("{}", ch as char);
        }
    }
}

fn write_tty_bytes(owner: usize, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        let mut line_buffers = TTY_LINE_BUFFERS.lock();
        if let Some(pos) = line_buffers
            .iter()
            .position(|line_buffer| line_buffer.owner == owner)
        {
            let newline_pos = bytes.iter().position(|&byte| byte == b'\n');
            let take_len = newline_pos.map_or(bytes.len(), |idx| idx + 1);
            line_buffers[pos].data.extend_from_slice(&bytes[..take_len]);
            let should_flush =
                newline_pos.is_some() || line_buffers[pos].data.len() >= TTY_LINE_BUFFER_LIMIT;
            let output = if should_flush {
                Some(line_buffers.swap_remove(pos).data)
            } else {
                None
            };
            drop(line_buffers);

            if let Some(output) = output {
                print_tty_bytes(&output);
            }
            bytes = &bytes[take_len..];
        } else if owner != usize::MAX && should_start_line_buffer(bytes) {
            let newline_pos = bytes.iter().position(|&byte| byte == b'\n');
            let take_len = newline_pos.map_or(bytes.len(), |idx| idx + 1);
            let mut data = Vec::new();
            data.extend_from_slice(&bytes[..take_len]);
            if newline_pos.is_some() || data.len() >= TTY_LINE_BUFFER_LIMIT {
                drop(line_buffers);
                print_tty_bytes(&data);
            } else {
                line_buffers.push(TtyLineBuffer { owner, data });
            }
            bytes = &bytes[take_len..];
        } else {
            drop(line_buffers);
            print_tty_bytes(bytes);
            break;
        }
    }
}

///
pub struct TtyFile {
    inner: Mutex<FileInner>,
}

impl TtyFile {
    ///
    pub fn new(dentry: Arc<dyn Dentry>) -> Self {
        Self {
            inner: Mutex::new(FileInner {
                offset: 0,
                dentry,
                flags: OpenFlags::empty(),
            }),
        }
    }
}

impl File for TtyFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        self.inner.lock()
    }

    fn readable(&self) -> bool {
        true
    }

    fn read_ready(&self) -> Option<bool> {
        Some(tty_input_ready())
    }

    fn requires_active_poll(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    fn read(&self, mut buf: UserBuffer) -> SysResult<usize> {
        let mut nread = 0usize;
        let nonblock = self.get_fileinner().flags.contains(OpenFlags::O_NONBLOCK);
        let (canonical, min_read_chars, read_timeout_deciseconds) = {
            let state = TTY_STATE.lock();
            (
                state.termios.is_canonical(),
                state.termios.min_read_chars(),
                state.termios.read_timeout_deciseconds(),
            )
        };
        let read_deadline = if !canonical && min_read_chars == 0 && read_timeout_deciseconds > 0 {
            Some(
                polyhal::timer::current_time().as_millis() + read_timeout_deciseconds as u128 * 100,
            )
        } else {
            None
        };
        for slice in buf.buffers.iter_mut() {
            for b in slice.iter_mut() {
                loop {
                    match read_tty_input() {
                        Some(ch) => {
                            let mut c = ch;

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

                            // Interactive programs such as Vim use non-canonical mode
                            // and expect read(2) to return after VMIN bytes, not after
                            // filling the whole userspace buffer.
                            if (!canonical && (min_read_chars == 0 || nread >= min_read_chars))
                                || (canonical && c == b'\n')
                            {
                                return Ok(nread);
                            }
                            break;
                        }
                        _ => {
                            if nonblock
                                || (!canonical && min_read_chars == 0 && read_deadline.is_none())
                            {
                                return if nread > 0 {
                                    Ok(nread)
                                } else {
                                    if nonblock {
                                        Err(SysError::EAGAIN)
                                    } else {
                                        Ok(0)
                                    }
                                };
                            }
                            if read_deadline.is_some_and(|deadline| {
                                polyhal::timer::current_time().as_millis() >= deadline
                            }) {
                                return Ok(nread);
                            }
                            suspend_current_and_run_next();
                        }
                    }
                }
            }
        }
        Ok(nread)
    }

    fn write(&self, buf: UserBuffer) -> SysResult<usize> {
        let mut nwritten = 0usize;
        let owner = current_tty_owner();
        let maps_newline_to_crlf = TTY_STATE.lock().termios.maps_newline_to_crlf();
        let _guard = TTY_WRITE_LOCK.lock();
        for slice in buf.buffers.iter() {
            let wrapped = wrap_alt_screen_sequences(slice);
            let bytes = wrapped.as_deref().unwrap_or(slice);
            if maps_newline_to_crlf && bytes.contains(&b'\n') {
                let mut output = Vec::with_capacity(bytes.len() + 1);
                for &byte in bytes.iter() {
                    if byte == b'\n' {
                        output.push(b'\r');
                    }
                    output.push(byte);
                }
                write_tty_bytes(owner, &output);
            } else {
                write_tty_bytes(owner, bytes);
            }
            nwritten += slice.len();
        }
        Ok(nwritten)
    }

    fn ioctl(&self, request: usize, argp: usize) -> SyscallResult {
        const TCGETS: usize = 0x5401;
        const TCSETS: usize = 0x5402;
        const TCSETSW: usize = 0x5403;
        const TCSETSF: usize = 0x5404;
        const TIOCGWINSZ: usize = 0x5413;
        const TIOCSPGRP: usize = 0x5410;
        const TIOCGPGRP: usize = 0x540F;

        let token = current_user_token();
        match request {
            TCGETS => {
                if argp == 0 {
                    return Err(SysError::EINVAL);
                }
                write_user_value(
                    token,
                    argp as *mut KernelTermios,
                    &KernelTermios::from(TTY_STATE.lock().termios),
                )?;
                Ok(0)
            }
            TCSETS | TCSETSW | TCSETSF => {
                if argp == 0 {
                    return Err(SysError::EINVAL);
                }
                let user_t = translated_ref(token, argp as *const KernelTermios)?;
                TTY_STATE.lock().termios = Termios::from(*user_t);
                Ok(0)
            }
            TIOCGWINSZ => {
                if argp == 0 {
                    return Err(SysError::EINVAL);
                }
                write_user_value(token, argp as *mut WinSize, &TTY_STATE.lock().winsize)?;
                Ok(0)
            }
            TIOCGPGRP => {
                info!("TtyFile ioctl TIOCGPGRP called");
                if argp == 0 {
                    return Err(SysError::EINVAL);
                }
                info!("Current foreground pgid: {}", TTY_STATE.lock().fg_pgid);
                write_user_value(token, argp as *mut i32, &TTY_STATE.lock().fg_pgid)?;
                Ok(0)
            }
            TIOCSPGRP => {
                if argp == 0 {
                    return Err(SysError::EINVAL);
                }
                let pgrp = *translated_ref(token, argp as *const i32)?;
                info!("TtyFile ioctl TIOCSPGRP called, new pgid: {}", pgrp);
                TTY_STATE.lock().fg_pgid = pgrp;
                Ok(0)
            }
            _ => Err(SysError::ENOTTY),
        }
    }

    fn open(&self) -> SyscallResult {
        Ok(0)
    }
    fn release(&self) -> SyscallResult {
        Ok(0)
    }
}

///
pub struct TtyDentry {
    inner: DentryInner,
}

impl TtyDentry {
    ///
    pub fn new(name: &str, parent: Option<Arc<dyn Dentry>>) -> Arc<Self> {
        let parent_weak = parent.as_ref().map(|p| Arc::downgrade(p));
        Arc::new_cyclic(|_me: &Weak<TtyDentry>| Self {
            inner: DentryInner::new(name, parent_weak.clone()),
        })
    }
}

impl Dentry for TtyDentry {
    fn get_dentryinner(&self) -> &DentryInner {
        &self.inner
    }
    fn name(&self) -> &str {
        &self.inner.name
    }
    fn open(self: Arc<Self>, _flags: OpenFlags, _mode: InodeMode) -> SysResult<Arc<dyn File>> {
        Ok(Arc::new(TtyFile::new(self)))
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
            inner: InodeInner::new(inode_alloc(), 0, InodeMode::CHAR, make_rdev(5, 0) as usize),
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
    fn get_size(&self) -> usize {
        info!("size:{}", self.inner.size.load(Ordering::SeqCst));
        self.inner.size.load(Ordering::SeqCst)
    }

    fn get_ino(&self) -> usize {
        self.inner.ino
    }

    fn get_nlink(&self) -> usize {
        self.inner.nlink.load(Ordering::SeqCst)
    }
    fn get_rdev(&self) -> usize {
        self.inner.rdev.load(Ordering::Relaxed)
    }
    fn set_rdev(&self, rdev: usize) {
        self.inner.rdev.store(rdev, Ordering::Relaxed);
    }
    fn inc_nlink(&self) {
        self.inner.nlink.fetch_add(1, Ordering::SeqCst);
    }
    fn dec_nlink(&self) {
        self.inner.nlink.fetch_sub(1, Ordering::SeqCst);
    }

    fn get_atime(&self) -> (i64, i64) {
        (
            self.inner.atime_sec.load(Ordering::Relaxed),
            self.inner.atime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_atime(&self, sec: i64, nsec: i64) {
        self.inner.atime_sec.store(sec, Ordering::Relaxed);
        self.inner.atime_nsec.store(nsec, Ordering::Relaxed);
    }

    fn get_mtime(&self) -> (i64, i64) {
        (
            self.inner.mtime_sec.load(Ordering::Relaxed),
            self.inner.mtime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_mtime(&self, sec: i64, nsec: i64) {
        self.inner.mtime_sec.store(sec, Ordering::Relaxed);
        self.inner.mtime_nsec.store(nsec, Ordering::Relaxed);
    }

    fn get_ctime(&self) -> (i64, i64) {
        (
            self.inner.ctime_sec.load(Ordering::Relaxed),
            self.inner.ctime_nsec.load(Ordering::Relaxed),
        )
    }

    fn set_ctime(&self, sec: i64, nsec: i64) {
        self.inner.ctime_sec.store(sec, Ordering::Relaxed);
        self.inner.ctime_nsec.store(nsec, Ordering::Relaxed);
    }
}
