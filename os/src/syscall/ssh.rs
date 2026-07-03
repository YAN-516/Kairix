use crate::current_user_token;
use crate::error::{SysError, SyscallResult};
use crate::mm::{translated_byte_buffer, translated_byte_buffer_for_write};

fn read_user_bytes(ptr: *const u8, len: usize) -> Result<alloc::vec::Vec<u8>, SysError> {
    if len == 0 {
        return Ok(alloc::vec::Vec::new());
    }
    let token = current_user_token();
    let parts = translated_byte_buffer(token, ptr, len)?;
    let mut out = alloc::vec::Vec::with_capacity(len);
    for part in parts {
        out.extend_from_slice(part);
    }
    Ok(out)
}

pub fn sys_ssh_connect(fd: usize, ident_ptr: *const u8, ident_len: usize) -> SyscallResult {
    let ident = read_user_bytes(ident_ptr, ident_len)?;
    crate::ssh::connect(fd, &ident)
}

pub fn sys_ssh_write(ssh_id: usize, buf: *const u8, len: usize) -> SyscallResult {
    if len == 0 {
        return crate::ssh::write(ssh_id, &[]);
    }
    let token = current_user_token();
    let parts = translated_byte_buffer(token, buf, len)?;
    let mut total = 0usize;
    for part in parts {
        let n = crate::ssh::write(ssh_id, part)?;
        total += n;
        if n < part.len() {
            break;
        }
    }
    Ok(total)
}

pub fn sys_ssh_read(ssh_id: usize, buf: *mut u8, len: usize) -> SyscallResult {
    if len == 0 {
        return crate::ssh::read(ssh_id, &mut []);
    }
    let token = current_user_token();
    let parts = translated_byte_buffer_for_write(token, buf, len)?;
    let mut total = 0usize;
    for part in parts {
        let n = crate::ssh::read(ssh_id, part)?;
        total += n;
        if n < part.len() {
            break;
        }
    }
    Ok(total)
}

pub fn sys_ssh_close(ssh_id: usize) -> SyscallResult {
    crate::ssh::close(ssh_id)
}

pub fn sys_ssh_auth_password(
    ssh_id: usize,
    username_ptr: *const u8,
    username_len: usize,
    password_ptr: *const u8,
    password_len: usize,
) -> SyscallResult {
    let username = read_user_bytes(username_ptr, username_len)?;
    let password = read_user_bytes(password_ptr, password_len)?;
    let username = core::str::from_utf8(&username).map_err(|_| SysError::EINVAL)?;
    let password = core::str::from_utf8(&password).map_err(|_| SysError::EINVAL)?;
    crate::ssh::auth_password(ssh_id, username, password)
}

pub fn sys_ssh_peer_ident(ssh_id: usize, buf: *mut u8, len: usize) -> SyscallResult {
    if len == 0 {
        return crate::ssh::peer_ident(ssh_id, &mut []);
    }
    let token = current_user_token();
    let parts = translated_byte_buffer_for_write(token, buf, len)?;
    let mut total = 0usize;
    for part in parts {
        let n = crate::ssh::peer_ident(ssh_id, part)?;
        total += n;
        if n < part.len() {
            break;
        }
    }
    Ok(total)
}
