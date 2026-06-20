use crate::current_user_token;
use crate::error::SyscallResult;
use crate::mm::{translated_byte_buffer, translated_byte_buffer_for_write};

pub fn sys_tls_connect(fd: usize, host_ptr: *const u8, host_len: usize) -> SyscallResult {
    let token = current_user_token();
    let mut host = alloc::string::String::new();
    for part in translated_byte_buffer(token, host_ptr, host_len)? {
        host.push_str(core::str::from_utf8(part).map_err(|_| crate::error::SysError::EINVAL)?);
    }
    crate::tls::connect(fd, &host)
}

pub fn sys_tls_write(tls_id: usize, buf: *const u8, len: usize) -> SyscallResult {
    let token = current_user_token();
    let parts = translated_byte_buffer(token, buf, len)?;
    let mut total = 0usize;
    for part in parts {
        total += crate::tls::write(tls_id, part)?;
    }
    Ok(total)
}

pub fn sys_tls_read(tls_id: usize, buf: *mut u8, len: usize) -> SyscallResult {
    let token = current_user_token();
    let parts = translated_byte_buffer_for_write(token, buf, len)?;
    let mut total = 0usize;
    for part in parts {
        let n = crate::tls::read(tls_id, part)?;
        total += n;
        if n < part.len() {
            break;
        }
    }
    Ok(total)
}

pub fn sys_tls_close(tls_id: usize) -> SyscallResult {
    crate::tls::close(tls_id)
}
