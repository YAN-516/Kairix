#![allow(missing_docs)]

use crate::error::{SysError, SysResult, SyscallResult};
use crate::fs::vfs::Dentry;
use crate::fs::vfs::File;
use crate::mm::translated_ref;
use crate::sync::SpinNoIrqLock;
use crate::task::{current_process, current_user_token, pid2process, ProcessControlBlock};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use spin::MutexGuard;

pub const LANDLOCK_ABI_VERSION: usize = 6;
pub const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1 << 0;

const LANDLOCK_RULE_PATH_BENEATH: i32 = 1;
const LANDLOCK_RULE_NET_PORT: i32 = 2;

pub const LANDLOCK_ACCESS_FS_EXECUTE: u64 = 1 << 0;
pub const LANDLOCK_ACCESS_FS_WRITE_FILE: u64 = 1 << 1;
pub const LANDLOCK_ACCESS_FS_READ_FILE: u64 = 1 << 2;
pub const LANDLOCK_ACCESS_FS_READ_DIR: u64 = 1 << 3;
pub const LANDLOCK_ACCESS_FS_REMOVE_DIR: u64 = 1 << 4;
pub const LANDLOCK_ACCESS_FS_REMOVE_FILE: u64 = 1 << 5;
pub const LANDLOCK_ACCESS_FS_MAKE_CHAR: u64 = 1 << 6;
pub const LANDLOCK_ACCESS_FS_MAKE_DIR: u64 = 1 << 7;
pub const LANDLOCK_ACCESS_FS_MAKE_REG: u64 = 1 << 8;
pub const LANDLOCK_ACCESS_FS_MAKE_SOCK: u64 = 1 << 9;
pub const LANDLOCK_ACCESS_FS_MAKE_FIFO: u64 = 1 << 10;
pub const LANDLOCK_ACCESS_FS_MAKE_BLOCK: u64 = 1 << 11;
pub const LANDLOCK_ACCESS_FS_MAKE_SYM: u64 = 1 << 12;
pub const LANDLOCK_ACCESS_FS_REFER: u64 = 1 << 13;
pub const LANDLOCK_ACCESS_FS_TRUNCATE: u64 = 1 << 14;
pub const LANDLOCK_ACCESS_FS_IOCTL_DEV: u64 = 1 << 15;

pub const LANDLOCK_ACCESS_NET_BIND_TCP: u64 = 1 << 0;
pub const LANDLOCK_ACCESS_NET_CONNECT_TCP: u64 = 1 << 1;

pub const LANDLOCK_SCOPE_ABSTRACT_UNIX_SOCKET: u64 = 1 << 0;
pub const LANDLOCK_SCOPE_SIGNAL: u64 = 1 << 1;

pub const ALL_FS_ACCESS: u64 = LANDLOCK_ACCESS_FS_EXECUTE
    | LANDLOCK_ACCESS_FS_WRITE_FILE
    | LANDLOCK_ACCESS_FS_READ_FILE
    | LANDLOCK_ACCESS_FS_READ_DIR
    | LANDLOCK_ACCESS_FS_REMOVE_DIR
    | LANDLOCK_ACCESS_FS_REMOVE_FILE
    | LANDLOCK_ACCESS_FS_MAKE_CHAR
    | LANDLOCK_ACCESS_FS_MAKE_DIR
    | LANDLOCK_ACCESS_FS_MAKE_REG
    | LANDLOCK_ACCESS_FS_MAKE_SOCK
    | LANDLOCK_ACCESS_FS_MAKE_FIFO
    | LANDLOCK_ACCESS_FS_MAKE_BLOCK
    | LANDLOCK_ACCESS_FS_MAKE_SYM
    | LANDLOCK_ACCESS_FS_REFER
    | LANDLOCK_ACCESS_FS_TRUNCATE
    | LANDLOCK_ACCESS_FS_IOCTL_DEV;
pub const ALL_NET_ACCESS: u64 = LANDLOCK_ACCESS_NET_BIND_TCP | LANDLOCK_ACCESS_NET_CONNECT_TCP;
pub const ALL_SCOPES: u64 = LANDLOCK_SCOPE_ABSTRACT_UNIX_SOCKET | LANDLOCK_SCOPE_SIGNAL;

pub const MAX_STACKED_RULESETS: usize = 16;

static NEXT_DOMAIN_ID: AtomicUsize = AtomicUsize::new(1);

#[repr(C)]
#[derive(Clone, Copy)]
struct LandlockRulesetAttrAbi6 {
    handled_access_fs: u64,
    handled_access_net: u64,
    scoped: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct LandlockPathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LandlockNetPortAttr {
    allowed_access: u64,
    port: u64,
}

#[derive(Clone)]
pub struct LandlockPathRule {
    pub path: alloc::string::String,
    pub allowed_access: u64,
}

#[derive(Clone)]
pub struct LandlockNetRule {
    pub port: u16,
    pub allowed_access: u64,
}

#[derive(Clone)]
pub struct LandlockRuleset {
    pub handled_access_fs: u64,
    pub handled_access_net: u64,
    pub scoped: u64,
    pub path_rules: Vec<LandlockPathRule>,
    pub net_rules: Vec<LandlockNetRule>,
}

impl LandlockRuleset {
    fn new(handled_access_fs: u64, handled_access_net: u64, scoped: u64) -> Self {
        Self {
            handled_access_fs,
            handled_access_net,
            scoped,
            path_rules: Vec::new(),
            net_rules: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct LandlockDomain {
    pub layers: Vec<Arc<LandlockRuleset>>,
    pub domain_id: usize,
}

impl LandlockDomain {
    pub fn new() -> Self {
        Self {
            layers: Vec::new(),
            domain_id: 0,
        }
    }
}

struct LandlockRulesetFile {
    ruleset: SpinNoIrqLock<LandlockRuleset>,
}

impl LandlockRulesetFile {
    fn new(ruleset: LandlockRuleset) -> Self {
        Self {
            ruleset: SpinNoIrqLock::new(ruleset),
        }
    }
}

impl File for LandlockRulesetFile {
    fn get_fileinner(&self) -> MutexGuard<'_, crate::fs::vfs::FileInner> {
        panic!("landlock ruleset fd has no FileInner")
    }

    fn get_inode(&self) -> Option<Arc<dyn crate::fs::vfs::inode::Inode>> {
        None
    }

    fn get_offset(&self) -> usize {
        0
    }

    fn set_offset(&self, _new_offset: usize) {}

    fn readable(&self) -> bool {
        false
    }

    fn writable(&self) -> bool {
        false
    }

    fn read(&self, _buf: crate::mm::UserBuffer) -> SysResult<usize> {
        Err(SysError::EBADF)
    }

    fn write(&self, _buf: crate::mm::UserBuffer) -> SysResult<usize> {
        Err(SysError::EBADF)
    }

    fn status_flags(&self) -> u32 {
        0
    }

    fn set_status_flags(&self, _flags: u32) {}

    fn is_landlock_ruleset(&self) -> bool {
        true
    }

    fn landlock_ruleset(&self) -> Option<Arc<LandlockRuleset>> {
        Some(Arc::new(self.ruleset.lock().clone()))
    }

    fn with_landlock_ruleset_mut(
        &self,
        f: &mut dyn FnMut(&mut LandlockRuleset) -> SyscallResult,
    ) -> SyscallResult {
        let mut ruleset = self.ruleset.lock();
        f(&mut ruleset)
    }
}

fn read_ruleset_attr(attr: usize, size: usize) -> SysResult<LandlockRulesetAttrAbi6> {
    if attr == 0 {
        return Err(SysError::EFAULT);
    }
    let token = current_user_token();
    let abi1_size = core::mem::size_of::<u64>();
    let abi4_size = abi1_size + core::mem::size_of::<u64>();
    let abi6_size = abi4_size + core::mem::size_of::<u64>();
    let raw = *translated_ref(token, attr as *const LandlockRulesetAttrAbi6)?;
    let handled_access_net = if size >= abi4_size {
        raw.handled_access_net
    } else {
        0
    };
    let scoped = if size >= abi6_size { raw.scoped } else { 0 };
    Ok(LandlockRulesetAttrAbi6 {
        handled_access_fs: raw.handled_access_fs,
        handled_access_net,
        scoped,
    })
}

fn alloc_ruleset_fd(ruleset: LandlockRuleset) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let fd = inner.alloc_fd()?;
    inner.fd_table[fd] = Some(Arc::new(LandlockRulesetFile::new(ruleset)));
    Ok(fd)
}

fn get_file(fd: i32) -> SysResult<Arc<dyn File + Send + Sync>> {
    if fd < 0 {
        return Err(SysError::EBADF);
    }
    let process = current_process();
    let inner = process.inner_exclusive_access();
    let fd = fd as usize;
    if fd >= inner.fd_table.len() {
        return Err(SysError::EBADF);
    }
    inner.fd_table[fd].as_ref().cloned().ok_or(SysError::EBADF)
}

fn path_matches(rule_path: &str, path: &str) -> bool {
    path == rule_path
        || (path.starts_with(rule_path)
            && (rule_path.ends_with('/') || path.as_bytes().get(rule_path.len()) == Some(&b'/')))
}

fn rules_allow_path(rules: &[LandlockPathRule], path: &str, access: u64) -> bool {
    let allowed = rules
        .iter()
        .filter(|rule| path_matches(&rule.path, path))
        .fold(0, |acc, rule| acc | rule.allowed_access);
    (allowed & access) == access
}

pub fn sys_landlock_create_ruleset(attr: usize, size: usize, flags: u32) -> SyscallResult {
    if flags == LANDLOCK_CREATE_RULESET_VERSION {
        if attr != 0 || size != 0 {
            return Err(SysError::EINVAL);
        }
        return Ok(LANDLOCK_ABI_VERSION);
    }
    if flags != 0 {
        return Err(SysError::EINVAL);
    }

    let abi1_size = core::mem::size_of::<u64>();
    if size < abi1_size {
        return Err(SysError::EINVAL);
    }
    if size > polyhal::consts::PAGE_SIZE {
        return Err(SysError::E2BIG);
    }

    let attr = read_ruleset_attr(attr, size)?;
    if attr.handled_access_fs & !ALL_FS_ACCESS != 0
        || attr.handled_access_net & !ALL_NET_ACCESS != 0
        || attr.scoped & !ALL_SCOPES != 0
    {
        return Err(SysError::EINVAL);
    }
    if attr.handled_access_fs == 0 && attr.handled_access_net == 0 && attr.scoped == 0 {
        return Err(SysError::ENOMSG);
    }

    alloc_ruleset_fd(LandlockRuleset::new(
        attr.handled_access_fs,
        attr.handled_access_net,
        attr.scoped,
    ))
}

pub fn sys_landlock_add_rule(
    ruleset_fd: i32,
    rule_type: i32,
    rule_attr: usize,
    flags: u32,
) -> SyscallResult {
    if flags != 0 {
        return Err(SysError::EINVAL);
    }
    let ruleset_file = get_file(ruleset_fd)?;
    if !ruleset_file.is_landlock_ruleset() {
        return Err(SysError::EBADFD);
    }

    match rule_type {
        LANDLOCK_RULE_PATH_BENEATH => {
            if rule_attr == 0 {
                return Err(SysError::EFAULT);
            }
            let token = current_user_token();
            let attr = *translated_ref(token, rule_attr as *const LandlockPathBeneathAttr)?;
            let allowed_access = attr.allowed_access;
            let parent_fd = attr.parent_fd;
            if allowed_access == 0 {
                return Err(SysError::ENOMSG);
            }
            let parent = get_file(parent_fd)?;
            if parent.get_inode().is_none() {
                return Err(SysError::EBADFD);
            }
            let dentry = parent.get_dentry();
            let parent_path = dentry.path();
            let mut op = |ruleset: &mut LandlockRuleset| {
                if allowed_access & !ruleset.handled_access_fs != 0 {
                    return Err(SysError::EINVAL);
                }
                ruleset.path_rules.push(LandlockPathRule {
                    path: parent_path.clone(),
                    allowed_access,
                });
                Ok(0)
            };
            ruleset_file.with_landlock_ruleset_mut(&mut op)
        }
        LANDLOCK_RULE_NET_PORT => {
            if rule_attr == 0 {
                return Err(SysError::EFAULT);
            }
            let token = current_user_token();
            let attr = *translated_ref(token, rule_attr as *const LandlockNetPortAttr)?;
            if attr.allowed_access == 0 {
                return Err(SysError::ENOMSG);
            }
            if attr.port > u16::MAX as u64 {
                return Err(SysError::EINVAL);
            }
            let mut op = |ruleset: &mut LandlockRuleset| {
                if attr.allowed_access & !ruleset.handled_access_net != 0 {
                    return Err(SysError::EINVAL);
                }
                ruleset.net_rules.push(LandlockNetRule {
                    port: attr.port as u16,
                    allowed_access: attr.allowed_access,
                });
                Ok(0)
            };
            ruleset_file.with_landlock_ruleset_mut(&mut op)
        }
        _ => Err(SysError::EINVAL),
    }
}

pub fn sys_landlock_restrict_self(ruleset_fd: i32, flags: u32) -> SyscallResult {
    if flags != 0 {
        return Err(SysError::EINVAL);
    }
    let ruleset_file = get_file(ruleset_fd)?;
    if !ruleset_file.is_landlock_ruleset() {
        return Err(SysError::EBADFD);
    }
    let ruleset = ruleset_file.landlock_ruleset().ok_or(SysError::EBADFD)?;
    let current = current_process();
    let current_inner = current.inner_exclusive_access();
    if !current_inner.has_cap_sys_admin && !current_inner.no_new_privs {
        return Err(SysError::EPERM);
    }
    drop(current_inner);

    let mut inner = current.inner_exclusive_access();
    if inner.landlock.layers.len() >= MAX_STACKED_RULESETS {
        return Err(SysError::E2BIG);
    }
    inner.landlock.layers.push(ruleset);
    inner.landlock.domain_id = NEXT_DOMAIN_ID.fetch_add(1, Ordering::Relaxed);
    Ok(0)
}

pub fn landlock_check_path(path: &str, access: u64) -> SyscallResult {
    if access == 0 {
        return Ok(0);
    }
    let process = current_process();
    let inner = process.inner_exclusive_access();
    for layer in &inner.landlock.layers {
        if layer.handled_access_fs & access != 0
            && !rules_allow_path(&layer.path_rules, path, access & layer.handled_access_fs)
        {
            return Err(SysError::EACCES);
        }
    }
    Ok(0)
}

pub fn landlock_check_dentry(dentry: &Arc<dyn Dentry>, access: u64) -> SyscallResult {
    landlock_check_path(&dentry.path(), access)
}

pub fn landlock_check_net_port(port: u16, access: u64) -> SyscallResult {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    for layer in &inner.landlock.layers {
        if layer.handled_access_net & access != 0
            && !layer
                .net_rules
                .iter()
                .any(|rule| rule.port == port && (rule.allowed_access & access) == access)
        {
            return Err(SysError::EACCES);
        }
    }
    Ok(0)
}

pub fn landlock_can_signal(
    sender: &Arc<ProcessControlBlock>,
    target: &Arc<ProcessControlBlock>,
) -> bool {
    let sender_inner = sender.inner_exclusive_access();
    let target_inner = target.inner_exclusive_access();
    let sender_signal = sender_inner
        .landlock
        .layers
        .iter()
        .any(|layer| layer.scoped & LANDLOCK_SCOPE_SIGNAL != 0);
    !sender_signal || sender_inner.landlock.domain_id == target_inner.landlock.domain_id
}

pub fn landlock_can_connect_abstract_unix(target_pid: usize) -> bool {
    let current = current_process();
    let Some(target) = pid2process(target_pid) else {
        return true;
    };
    let current_inner = current.inner_exclusive_access();
    let target_inner = target.inner_exclusive_access();
    let current_scoped = current_inner
        .landlock
        .layers
        .iter()
        .any(|layer| layer.scoped & LANDLOCK_SCOPE_ABSTRACT_UNIX_SOCKET != 0);
    !current_scoped || current_inner.landlock.domain_id == target_inner.landlock.domain_id
}
