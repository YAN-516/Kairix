use crate::{mm::copy_to_user, task::current_user_token};




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
            release: Self::set_field("5.10.0"),
            version: Self::set_field("#1 SMP 2026-03-27"),
            machine: Self::set_field("RISC-V"),
            domainname: Self::set_field("localdomain"),
        }
    }

    fn set_field(s: &str)-> [u8; 65] {
        let bytes = s.as_bytes();
        let len = core::cmp::min(bytes.len(), 64);
        let mut field = [0; 65];
        field[..len].copy_from_slice(&bytes[..len]);
        field
    }
}

pub fn sys_uname(buf: *mut u8) -> isize {
    const EFAULT: isize = -14;
    if buf.is_null() {
        return EFAULT;
    }
    let default_utsname = UtsName::default();
    let token =current_user_token();
    let uts_bytes: &[u8] = unsafe {
        core::slice::from_raw_parts(
            &default_utsname as *const _ as *const u8,
            core::mem::size_of::<UtsName>(),
        )
    };
    copy_to_user(token, buf, uts_bytes);
    0
}