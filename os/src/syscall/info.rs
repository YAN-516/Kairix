use crate::error::{SysError, SyscallResult};
use crate::{mm::copy_to_user, task::current_user_token};

#[cfg(target_arch = "riscv64")]
const UTS_MACHINE: &str = "riscv64";
#[cfg(target_arch = "loongarch64")]
const UTS_MACHINE: &str = "loongarch64";

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct UtsName {
    pub sysname: [u8; 65],
    pub nodename: [u8; 65],
    pub release: [u8; 65],
    pub version: [u8; 65],
    pub machine: [u8; 65],
    pub domainname: [u8; 65],
}

impl UtsName {
    fn default() -> Self {
        Self {
            sysname: Self::set_field("Linux"),
            nodename: Self::set_field("Linux"),
            release: Self::set_field("6.10.0"),
            version: Self::set_field("#1 SMP 2026-03-27"),
            machine: Self::set_field(UTS_MACHINE),
            domainname: Self::set_field("localdomain"),
        }
    }

    fn set_field(s: &str) -> [u8; 65] {
        let bytes = s.as_bytes();
        let len = core::cmp::min(bytes.len(), 64);
        let mut field = [0; 65];
        field[..len].copy_from_slice(&bytes[..len]);
        field
    }
}

/// Linux getcpu(2): both output pointers are optional and the cache argument
/// is only a historical userspace cache hint. Kairix currently exposes a
/// single NUMA node, while the CPU number is the actual scheduler CPU.
pub fn sys_getcpu(cpu: *mut u32, node: *mut u32, _cache: usize) -> SyscallResult {
    let token = current_user_token();
    if !cpu.is_null() {
        copy_to_user(
            token,
            cpu.cast(),
            &(polyhal::arch::hart_id() as u32).to_ne_bytes(),
        )?;
    }
    if !node.is_null() {
        copy_to_user(token, node.cast(), &0u32.to_ne_bytes())?;
    }
    Ok(0)
}

pub fn sys_uname(buf: *mut u8) -> SyscallResult {
    if buf.is_null() {
        return Err(SysError::EFAULT);
    }
    let default_utsname = UtsName::default();
    let token = current_user_token();
    let uts_bytes: &[u8] = unsafe {
        core::slice::from_raw_parts(
            &default_utsname as *const _ as *const u8,
            core::mem::size_of::<UtsName>(),
        )
    };
    copy_to_user(token, buf, uts_bytes)?;
    Ok(0)
}
