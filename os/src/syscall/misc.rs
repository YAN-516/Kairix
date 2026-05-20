use crate::error::{SysError, SyscallResult};
use crate::fs::devfs::urandom::fill_random;
use crate::fs::vfs::{File, FileInner};
use crate::mm::copy_to_user;
use crate::mm::{UserBuffer, get_free_memory, get_total_memory, translated_refmut, translated_str};
use crate::task::{
    block_current_and_run_next, current_process, current_task, current_user_token,
    exit_current_and_run_next, num_processes, pid2process, suspend_current_and_run_next,
};
use polyhal::timer::current_time;

#[cfg(target_arch = "riscv64")]
use crate::timer::*;
use crate::trap::_set_sum_bit;
use alloc::sync::Arc;
use alloc::string::String;
use alloc::vec::Vec;
use core::mem::size_of;
use spin::MutexGuard;

const LINUX_CAPABILITY_VERSION_3: u32 = 0x20080522;
const O_CLOEXEC: i32 = 0o2000000;
const O_NONBLOCK: u32 = 0o0004000;

struct AnonFdFile {
    _name: &'static str,
    status_flags: u32,
}

impl AnonFdFile {
    fn new(name: &'static str, status_flags: u32) -> Self {
        Self {
            _name: name,
            status_flags,
        }
    }
}

impl File for AnonFdFile {
    fn get_fileinner(&self) -> MutexGuard<'_, FileInner> {
        panic!("anonymous fd has no FileInner")
    }

    fn get_inode(&self) -> Option<Arc<dyn crate::fs::vfs::inode::Inode>> {
        None
    }

    fn get_offset(&self) -> usize {
        0
    }

    fn set_offset(&self, _new_offset: usize) {}

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    fn read(&self, _buf: UserBuffer) -> Result<usize, SysError> {
        Err(SysError::EBADF)
    }

    fn write(&self, _buf: UserBuffer) -> Result<usize, SysError> {
        Err(SysError::EBADF)
    }

    fn status_flags(&self) -> u32 {
        self.status_flags
    }
}

fn alloc_anon_fd(name: &'static str, cloexec: bool, status_flags: u32) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let fd = inner.alloc_fd()?;
    inner.fd_table[fd] = Some(Arc::new(AnonFdFile::new(name, status_flags)));
    if cloexec && fd < inner.fd_flags.len() {
        inner.fd_flags[fd] |= 1;
    }
    Ok(fd)
}

fn cloexec_from_flags(flags: i32) -> bool {
    flags & O_CLOEXEC != 0
}

fn status_from_flags(flags: i32) -> u32 {
    if flags & O_NONBLOCK as i32 != 0 {
        O_NONBLOCK
    } else {
        0
    }
}

pub fn sys_epoll_create1(flags: i32) -> SyscallResult {
    if flags == 1 {
        return alloc_anon_fd("epoll", false, 0);
    }
    if flags & !O_CLOEXEC != 0 {
        return Err(SysError::EINVAL);
    }
    alloc_anon_fd("epoll", cloexec_from_flags(flags), 0)
}

pub fn sys_eventfd2(_initval: usize, flags: i32) -> SyscallResult {
    const EFD_SEMAPHORE: i32 = 1;
    if flags & !(EFD_SEMAPHORE | O_CLOEXEC | O_NONBLOCK as i32) != 0 {
        return Err(SysError::EINVAL);
    }
    alloc_anon_fd("eventfd", cloexec_from_flags(flags), status_from_flags(flags))
}

pub fn sys_signalfd4(fd: isize, _mask: usize, _sizemask: usize, flags: i32) -> SyscallResult {
    if flags & !(O_CLOEXEC | O_NONBLOCK as i32) != 0 {
        return Err(SysError::EINVAL);
    }
    if fd >= 0 {
        return Ok(fd as usize);
    }
    alloc_anon_fd("signalfd", cloexec_from_flags(flags), status_from_flags(flags))
}

pub fn sys_pidfd_open(pid: usize, flags: u32) -> SyscallResult {
    if flags != 0 {
        return Err(SysError::EINVAL);
    }
    if pid2process(pid).is_none() {
        return Err(SysError::ESRCH);
    }
    alloc_anon_fd("pidfd", false, 0)
}

pub fn sys_fanotify_init(flags: u32, event_f_flags: u32) -> SyscallResult {
    const FAN_CLASS_MASK: u32 = 0x3;
    const FAN_CLOEXEC: u32 = 0x0000_0001;
    const FAN_NONBLOCK: u32 = 0x0000_0002;
    let allowed = FAN_CLASS_MASK | FAN_CLOEXEC | FAN_NONBLOCK;
    if flags & !allowed != 0 {
        return Err(SysError::EINVAL);
    }
    let status_flags = if flags & FAN_NONBLOCK != 0 || event_f_flags & O_NONBLOCK != 0 {
        O_NONBLOCK
    } else {
        0
    };
    alloc_anon_fd("fanotify", flags & FAN_CLOEXEC != 0, status_flags)
}

pub fn sys_userfaultfd(flags: i32) -> SyscallResult {
    if flags & !(O_CLOEXEC | O_NONBLOCK as i32) != 0 {
        return Err(SysError::EINVAL);
    }
    alloc_anon_fd("userfaultfd", cloexec_from_flags(flags), status_from_flags(flags))
}

pub fn sys_perf_event_open(_attr: usize, _pid: isize, _cpu: isize, _group_fd: isize, flags: u32) -> SyscallResult {
    if flags & !O_CLOEXEC as u32 != 0 {
        return Err(SysError::EINVAL);
    }
    alloc_anon_fd("perf_event", flags & O_CLOEXEC as u32 != 0, 0)
}

pub fn sys_io_uring_setup(entries: u32, _params: usize) -> SyscallResult {
    if entries == 0 {
        return Err(SysError::EINVAL);
    }
    alloc_anon_fd("io_uring", false, 0)
}

pub fn sys_bpf(cmd: u32, _attr: usize, _size: u32) -> SyscallResult {
    const BPF_MAP_CREATE: u32 = 0;
    if cmd != BPF_MAP_CREATE {
        return Err(SysError::EINVAL);
    }
    alloc_anon_fd("bpf_map", false, 0)
}

pub fn sys_fsopen(fs_name: *const u8, flags: u32) -> SyscallResult {
    const FSOPEN_CLOEXEC: u32 = 0x1;
    if fs_name.is_null() {
        return Err(SysError::EFAULT);
    }
    if flags & !FSOPEN_CLOEXEC != 0 {
        return Err(SysError::EINVAL);
    }
    let _ = translated_str(current_user_token(), fs_name)?;
    alloc_anon_fd("fsopen", flags & FSOPEN_CLOEXEC != 0, 0)
}

pub fn sys_fspick(_dfd: isize, path: *const u8, flags: u32) -> SyscallResult {
    const FSPICK_CLOEXEC: u32 = 0x1;
    if path.is_null() {
        return Err(SysError::EFAULT);
    }
    if flags & !FSPICK_CLOEXEC != 0 {
        return Err(SysError::EINVAL);
    }
    let _ = translated_str(current_user_token(), path)?;
    alloc_anon_fd("fspick", flags & FSPICK_CLOEXEC != 0, 0)
}

pub fn sys_open_tree(_dfd: isize, path: *const u8, flags: u32) -> SyscallResult {
    const OPEN_TREE_CLOEXEC: u32 = 0x0008_0000;
    const OPEN_TREE_CLONE: u32 = 1;
    const AT_EMPTY_PATH: u32 = 0x1000;
    const AT_RECURSIVE: u32 = 0x8000;
    if path.is_null() {
        return Err(SysError::EFAULT);
    }
    if flags & !(OPEN_TREE_CLONE | OPEN_TREE_CLOEXEC | AT_EMPTY_PATH | AT_RECURSIVE) != 0 {
        return Err(SysError::EINVAL);
    }
    let _ = translated_str(current_user_token(), path)?;
    alloc_anon_fd("open_tree", flags & OPEN_TREE_CLOEXEC != 0, 0)
}

pub fn sys_memfd_create(name: *const u8, flags: u32) -> SyscallResult {
    const MFD_CLOEXEC: u32 = 0x0001;
    const MFD_ALLOW_SEALING: u32 = 0x0002;
    if name.is_null() {
        return Err(SysError::EFAULT);
    }
    if flags & !(MFD_CLOEXEC | MFD_ALLOW_SEALING) != 0 {
        return Err(SysError::EINVAL);
    }
    let _ = translated_str(current_user_token(), name)?;
    alloc_anon_fd("memfd", flags & MFD_CLOEXEC != 0, 0)
}

pub fn sys_memfd_secret(flags: u32) -> SyscallResult {
    if flags != 0 {
        return Err(SysError::EINVAL);
    }
    alloc_anon_fd("memfd_secret", false, 0)
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CapUserHeader {
    version: u32,
    pid: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CapUserData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

/// capget: get process capabilities.
/// For now, all processes are treated as having full capabilities (root).
pub fn sys_capget(hdrp: usize, datap: usize) -> SyscallResult {
    if hdrp == 0 || datap == 0 {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let header = translated_refmut(token, hdrp as *mut CapUserHeader)?;

    if header.version != LINUX_CAPABILITY_VERSION_3 {
        header.version = LINUX_CAPABILITY_VERSION_3;
        return Err(SysError::EINVAL);
    }

    let pid = header.pid;
    if pid < 0 {
        return Err(SysError::EINVAL);
    }
    if pid != 0 {
        let current_pid = current_task()
            .and_then(|t| t.process.upgrade().map(|p| p.getpid() as i32))
            .unwrap_or(0);
        if pid != current_pid {
            return Err(SysError::ESRCH);
        }
    }

    // V3 requires two CapUserData structs (64 capabilities)
    let data0 = translated_refmut(token, datap as *mut CapUserData)?;
    data0.effective = !0u32;
    data0.permitted = !0u32;
    data0.inheritable = !0u32;

    let data1 = translated_refmut(token, unsafe { (datap as *mut CapUserData).add(1) })?;
    data1.effective = !0u32;
    data1.permitted = !0u32;
    data1.inheritable = !0u32;

    Ok(0)
}

/// capset: set process capabilities.
/// For now, accepts but ignores the request (stub implementation).
pub fn sys_capset(hdrp: usize, _datap: usize) -> SyscallResult {
    if hdrp == 0 {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let header = translated_refmut(token, hdrp as *mut CapUserHeader)?;

    if header.version != LINUX_CAPABILITY_VERSION_3 {
        header.version = LINUX_CAPABILITY_VERSION_3;
        return Err(SysError::EINVAL);
    }

    let pid = header.pid;
    if pid < 0 {
        return Err(SysError::EINVAL);
    }
    if pid != 0 {
        let current_pid = current_task()
            .and_then(|t| t.process.upgrade().map(|p| p.getpid() as i32))
            .unwrap_or(0);
        if pid != current_pid {
            return Err(SysError::EPERM);
        }
    }

    // Stub: ignore actual capability changes.
    Ok(0)
}

/// getrandom: fill user buffer with pseudo-random bytes.
/// Since Kairix has no hardware RNG, we use a simple xorshift64 PRNG.
/// 现在复用 /dev/urandom 的 fill_random 实现，避免逐字节拷贝。
pub fn sys_getrandom(buf: *mut u8, buflen: usize, _flags: u32) -> SyscallResult {
    if buflen == 0 {
        return Ok(0);
    }
    if buf.is_null() {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let mut local_buf = Vec::with_capacity(buflen);
    local_buf.resize(buflen, 0u8);
    fill_random(&mut local_buf);
    copy_to_user(token, buf as *const u8, &local_buf);
    Ok(buflen)
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SysInfo {
    pub uptime: i64,
    pub loads: [u64; 3],
    pub totalram: u64,
    pub freeram: u64,
    pub sharedram: u64,
    pub bufferram: u64,
    pub totalswap: u64,
    pub freeswap: u64,
    pub procs: u16,
    pub pad: u16,
    pub totalhigh: u64,
    pub freehigh: u64,
    pub mem_unit: u32,
    pub _f: [u8; 4],
}

impl SysInfo {
    pub fn new() -> Self {
        Self {
            uptime: 0,
            loads: [0; 3],
            totalram: 0,
            freeram: 0,
            sharedram: 0,
            bufferram: 0,
            totalswap: 0,
            freeswap: 0,
            procs: 0,
            pad: 0,
            totalhigh: 0,
            freehigh: 0,
            mem_unit: 1,
            _f: [0; 4],
        }
    }
}

pub fn sys_sysinfo(info: *mut SysInfo) -> SyscallResult {
    if info.is_null() {
        return Err(SysError::EFAULT);
    }
    _set_sum_bit();
    let token = current_user_token();
    let mut sysinfo = SysInfo::new();
    sysinfo.uptime = (current_time().as_micros() / 1_000_000) as i64;
    sysinfo.totalram = get_total_memory() as u64;
    sysinfo.freeram = get_free_memory() as u64;
    sysinfo.procs = num_processes() as u16;
    sysinfo.mem_unit = 1;

    let src_bytes = unsafe {
        core::slice::from_raw_parts(&sysinfo as *const _ as *const u8, size_of::<SysInfo>())
    };
    copy_to_user(token, info as *const u8, src_bytes);
    Ok(0)
}
