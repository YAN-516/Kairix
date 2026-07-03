use crate::error::{SysError, SyscallResult};
use crate::fs::devfs::urandom::fill_random;
use crate::fs::vfs::{File, FileInner};
use crate::mm::copy_to_user;
use crate::mm::{get_free_memory, get_total_memory, translated_refmut, UserBuffer};
use crate::task::{current_process, current_task, current_user_token, num_processes, pid2process};
use polyhal::timer::current_time;

#[cfg(target_arch = "riscv64")]
use crate::timer::*;
use crate::trap::_set_sum_bit;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem::size_of;
use spin::MutexGuard;

const LINUX_CAPABILITY_VERSION_3: u32 = 0x20080522;
const O_CLOEXEC: i32 = 0o2000000;
const O_NONBLOCK: u32 = 0o0004000;
struct AnonFdFile {
    name: &'static str,
    status_flags: u32,
}

impl AnonFdFile {
    fn new(name: &'static str, status_flags: u32) -> Self {
        Self { name, status_flags }
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

    fn is_open_tree_fd(&self) -> bool {
        self.name == "open_tree"
    }
}

pub(crate) fn alloc_anon_fd(name: &'static str, cloexec: bool, status_flags: u32) -> SyscallResult {
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

pub fn sys_eventfd2(_initval: usize, flags: i32) -> SyscallResult {
    const EFD_SEMAPHORE: i32 = 1;
    if flags & !(EFD_SEMAPHORE | O_CLOEXEC | O_NONBLOCK as i32) != 0 {
        return Err(SysError::EINVAL);
    }
    alloc_anon_fd(
        "eventfd",
        cloexec_from_flags(flags),
        status_from_flags(flags),
    )
}

pub fn sys_signalfd4(fd: isize, _mask: usize, _sizemask: usize, flags: i32) -> SyscallResult {
    if flags & !(O_CLOEXEC | O_NONBLOCK as i32) != 0 {
        return Err(SysError::EINVAL);
    }
    if fd >= 0 {
        return Ok(fd as usize);
    }
    alloc_anon_fd(
        "signalfd",
        cloexec_from_flags(flags),
        status_from_flags(flags),
    )
}

pub fn sys_pidfd_open(pid: usize, flags: u32) -> SyscallResult {
    if flags != 0 {
        return Err(SysError::EINVAL);
    }
    if pid2process(pid).is_none() {
        return Err(SysError::ESRCH);
    }
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let fd = inner.alloc_fd()?;
    inner.fd_table[fd] = Some(Arc::new(crate::fs::pidfd::PidFdFile::new(pid)));
    Ok(fd)
}

pub fn sys_userfaultfd(flags: i32) -> SyscallResult {
    if flags & !(O_CLOEXEC | O_NONBLOCK as i32) != 0 {
        return Err(SysError::EINVAL);
    }
    alloc_anon_fd(
        "userfaultfd",
        cloexec_from_flags(flags),
        status_from_flags(flags),
    )
}

pub fn sys_perf_event_open(
    _attr: usize,
    _pid: isize,
    _cpu: isize,
    _group_fd: isize,
    flags: u32,
) -> SyscallResult {
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

    let has_cap_sys_admin = current_process().inner_exclusive_access().has_cap_sys_admin;
    let mut effective0 = !0u32;
    let mut permitted0 = !0u32;
    const CAP_SYS_ADMIN: u32 = 21;
    if !has_cap_sys_admin {
        effective0 &= !(1 << CAP_SYS_ADMIN);
        permitted0 &= !(1 << CAP_SYS_ADMIN);
    }

    // V3 requires two CapUserData structs (64 capabilities)
    let data0 = translated_refmut(token, datap as *mut CapUserData)?;
    data0.effective = effective0;
    data0.permitted = permitted0;
    data0.inheritable = !0u32;

    let data1 = translated_refmut(token, unsafe { (datap as *mut CapUserData).add(1) })?;
    data1.effective = !0u32;
    data1.permitted = !0u32;
    data1.inheritable = !0u32;

    Ok(0)
}

/// capset: set process capabilities.
pub fn sys_capset(hdrp: usize, datap: usize) -> SyscallResult {
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
            return Err(SysError::EPERM);
        }
    }

    const CAP_SYS_ADMIN: u32 = 21;
    let data0 = translated_refmut(token, datap as *mut CapUserData)?;
    current_process().inner_exclusive_access().has_cap_sys_admin =
        data0.effective & (1 << CAP_SYS_ADMIN) != 0;
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
    copy_to_user(token, buf, &local_buf)?;
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
    copy_to_user(token, info as *mut u8, src_bytes)?;
    Ok(0)
}

/// membarrier: issue memory barriers on a set of CPUs.
/// This provides a way to synchronize memory accesses across CPUs.
/// For simplicity, we implement a basic version that supports the query command
/// and performs a full memory barrier for other commands.
pub fn sys_membarrier(cmd: i32, flags: i32, _cpu_mask: *mut u64) -> SyscallResult {
    // membarrier command constants
    const MEMBARRIER_CMD_QUERY: i32 = 0;
    const MEMBARRIER_CMD_GLOBAL: i32 = 1;
    const MEMBARRIER_CMD_GLOBAL_EXPEDITED: i32 = 2;
    const MEMBARRIER_CMD_REGISTER_GLOBAL_EXPEDITED: i32 = 3;
    const MEMBARRIER_CMD_PRIVATE_EXPEDITED: i32 = 4;
    const MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED: i32 = 5;

    // Check flags - only flag currently defined is MEMBARRIER_FLAG_CPU_MASK
    if flags != 0 {
        return Err(SysError::EINVAL);
    }

    match cmd {
        MEMBARRIER_CMD_QUERY => {
            // Return supported commands
            // We support: QUERY, GLOBAL, GLOBAL_EXPEDITED
            let supported = (1 << MEMBARRIER_CMD_GLOBAL) | (1 << MEMBARRIER_CMD_GLOBAL_EXPEDITED);
            Ok(supported)
        }
        MEMBARRIER_CMD_GLOBAL | MEMBARRIER_CMD_GLOBAL_EXPEDITED => {
            // Perform a full memory barrier
            // On RISC-V, we use sfence.vma for TLB flush and fence for memory ordering
            #[cfg(target_arch = "riscv64")]
            unsafe {
                core::arch::asm!("fence", options(nomem, nostack));
            }
            #[cfg(target_arch = "loongarch64")]
            unsafe {
                // LoongArch: dbar 0 performs a full memory barrier
                core::arch::asm!("dbar 0", options(nomem, nostack));
            }
            Ok(0)
        }
        MEMBARRIER_CMD_REGISTER_GLOBAL_EXPEDITED => {
            // Register for global expedited membarrier
            // In our simple implementation, we just return success
            Ok(0)
        }
        MEMBARRIER_CMD_PRIVATE_EXPEDITED | MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED => {
            // Private expedited commands require PRIV_CAP_MEMBARRIER capability
            // which we don't support in this simple implementation
            Err(SysError::EPERM)
        }
        _ => Err(SysError::EINVAL),
    }
}
