use alloc::string::String;
use alloc::vec::Vec;

use crate::error::{SysError, SysResult};
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use alloc::sync::Arc;
use crate::fs::vfs::Dentry;
use crate::fs::vfs::inode::InodeMode;
use alloc::format;
use log::*;
use crate::task::current_process;
use crate::alloc::string::ToString;

///get the dentry of the path
///the path can be absolute or relative, if it is relative,
///we will use the cwd to build the absolute path, 
///and then find the dentry of the absolute path
/// Resolves a path string into a VFS `Dentry` node.
/// 
/// # Conceptual Examples
/// 
/// ```
/// // Assume `cwd` points to "/home/user"
/// 
/// // Absolute path ignores `cwd` and starts from root.
/// let dentry = resolve_path(cwd, "/etc/passwd");
/// // Resolves to: "/etc/passwd"
/// 
/// // Relative path appends to `cwd`.
/// let dentry = resolve_path(cwd, "docs/test.txt");
/// // Resolves to: "/home/user/docs/test.txt"
/// 
/// // `.` means current directory (stays at same level).
/// let dentry = resolve_path(cwd, "./file.txt");
/// // Resolves to: "/home/user/file.txt"
/// 
/// // `..` goes back to the parent directory.
/// let dentry = resolve_path(cwd, "../other");
/// // Resolves to: "/home/other"
/// 
/// // `..` safely stops at root `/` without crashing.
/// let dentry = resolve_path(cwd, "../../../../bin");
/// // Resolves to: "/bin"
/// 
/// // Multiple slashes are automatically skipped.
/// let dentry = resolve_path(cwd, "a//b///c");
/// // Resolves to: "/home/user/a/b/c"
/// ```
fn resolve_path_inner(cwd: Arc<dyn Dentry>, path: &str, follow_last: bool) -> SysResult<Arc<dyn Dentry>> {
    const MAX_SYMLINK_FOLLOWS: usize = 40;
    let mut symlink_count = 0;

    let mut current = if path.starts_with('/') {
        GLOBAL_DCACHE.get("/").unwrap().clone()
    } else {
        cwd
    };

    let mut parts: Vec<String> = path.split('/').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
    let mut i = 0;

    while i < parts.len() {
        let part = parts[i].clone();
        let is_last = i == parts.len() - 1;

        match part.as_str() {
            "." => {
                i += 1;
                continue;
            }
            ".." => {
                current = current.parent().unwrap_or(current);
                i += 1;
                continue;
            }
            name => {
                // 路径中间组件必须是目录，否则返回 ENOTDIR
                if let Some(inode) = current.get_inode() {
                    if !inode.get_mode().contains(InodeMode::DIR) {
                        return Err(SysError::ENOTDIR);
                    }
                } else {
                    return Err(SysError::ENOTDIR);
                }
                let next_path = if current.path() == "/" {
                    format!("/{}", name)
                } else {
                    format!("{}/{}", current.path(), name)
                };

                let next_dentry = if let Some(cached_node) = GLOBAL_DCACHE.get(&next_path) {
                    if cached_node.path() == next_path {
                        cached_node
                    } else {
                        let d = current.find(name)?;
                        info!("Resolved path (cache stale): {}", next_path);
                        GLOBAL_DCACHE.insert(next_path, d.clone());
                        d
                    }
                } else {
                    let d = current.find(name)?;
                    info!("Resolved path: {}", next_path);
                    GLOBAL_DCACHE.insert(next_path, d.clone());
                    d
                };

                // 检查是否为符号链接
                if let Some(inode) = next_dentry.get_inode() {
                    if inode.get_mode().contains(InodeMode::LINK) {
                        // 如果是最后一个组件且不跟随，直接返回 symlink 本身
                        if is_last && !follow_last {
                            return Ok(next_dentry);
                        }

                        if symlink_count >= MAX_SYMLINK_FOLLOWS {
                            return Err(SysError::ELOOP);
                        }
                        symlink_count += 1;

                        let target = inode.readlink().map_err(|e| {
                            let code = if e < 0 { e } else { -e };
                            SysError::try_from(code).unwrap_or(SysError::EINVAL)
                        })?;

                        let is_absolute = target.starts_with('/');

                        // 构建新的剩余路径
                        let remaining: String = parts[i + 1..].join("/");
                        let new_path = if remaining.is_empty() {
                            target
                        } else if target.ends_with('/') {
                            format!("{}{}", target, remaining)
                        } else {
                            format!("{}/{}", target, remaining)
                        };

                        // 根据 symlink 目标是绝对还是相对，确定起点
                        if is_absolute {
                            current = GLOBAL_DCACHE.get("/").unwrap().clone();
                        }
                        // 相对路径保持 current 不变

                        // 重新拆分路径
                        parts = new_path.split('/').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
                        i = 0;
                        continue;
                    }
                }

                current = next_dentry;
                i += 1;
            }
        }
    }
    Ok(current)
}

/// 解析路径，默认跟随所有符号链接（包括最后一个组件）。
pub fn resolve_path(cwd: Arc<dyn Dentry>, path: &str) -> SysResult<Arc<dyn Dentry>> {
    resolve_path_inner(cwd, path, true)
}

/// 解析路径，中间组件跟随符号链接，但最后一个组件如果是符号链接则直接返回 symlink 本身。
pub fn resolve_path_nofollow_last(cwd: Arc<dyn Dentry>, path: &str) -> SysResult<Arc<dyn Dentry>> {
    resolve_path_inner(cwd, path, false)
}

//return the parent path and the name of the file or directory, if the path is "/", return ("/", "")
/// ```
/// // `name` may be a file or directory.
/// let (parent, name) = split_parent_and_name("/parent/test/name");
/// assert_eq!(parent, "/parent/test".to_string());
/// assert_eq!(name, "name".to_string());
///
/// // The path may be a relative path.
/// let (parent, name) = split_parent_and_name("parent/test/name");
/// assert_eq!(parent, "parent/test".to_string());
/// assert_eq!(name, "name".to_string());
///
/// // The root directory may be a parent.
/// let (parent, name) = split_parent_and_name("/parent");
/// assert_eq!(parent, "/".to_string());
/// assert_eq!(name, "parent".to_string());
///
/// // If the path is just a root directory, the parent is "/" and
/// // the name is empty (handled safely by VFS).
/// let (parent, name) = split_parent_and_name("/");
/// assert_eq!(parent, "/".to_string());
/// assert_eq!(name, "".to_string());
///
/// // If the path is just a file name, the parent defaults to "." 
/// // (Current Working Directory), and the name is the whole path.
/// let (parent, name) = split_parent_and_name("parent");
/// assert_eq!(parent, ".".to_string());
/// assert_eq!(name, "parent".to_string());
///
/// // Trailing slashes are safely trimmed and ignored.
/// let (parent, name) = split_parent_and_name("/parent/test/");
/// assert_eq!(parent, "/parent".to_string());
/// assert_eq!(name, "test".to_string());
/// ```
pub fn split_parent_and_name(path: &str) -> (String, String) {
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        return (String::from("/"), String::from(""));
    }
    match path.rfind('/') {
        Some(idx) => {
            let parent = if idx == 0 {
                "/"
            } else {
                let p = path[..idx].trim_end_matches('/');
                if p.is_empty() { "/" } else { p }
            };
            let name = &path[idx + 1..];
            (String::from(parent), String::from(name))
        }
        None => {
            (String::from("."), String::from(path))
        }
    }
}


pub const AT_FDCWD: isize = -100;
/// return the dentry of the start point of the path, which is determined by dirfd
/// 1 /
/// 2 cwd
/// 3 dirfd
pub fn get_start_dentry(dirfd: isize, path: &str) -> SysResult<Arc<dyn Dentry>> {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if path.starts_with('/') {
        return Ok(GLOBAL_DCACHE.get("/").unwrap().clone());
    } else if dirfd == AT_FDCWD {
        return Ok(inner.cwd.clone());
    } else {
        let fd = dirfd as usize;
        if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
            return Err(SysError::EBADF); 
        }
        let file = inner.fd_table[fd].as_ref().unwrap();
        // 相对路径 + 显式 dirfd 的语义要求该 fd 必须可作为目录起点。
        // 对于 pipe/socket/tty 等无目录语义的 fd，返回 ENOTDIR，避免触发 get_dentry panic。
        let inode = match file.get_inode() {
            Some(inode) => inode,
            None => return Err(SysError::ENOTDIR),
        };
        if !inode.get_mode().contains(crate::fs::vfs::inode::InodeMode::DIR) {
            return Err(SysError::ENOTDIR);
        }
        return Ok(file.get_dentry());
    };
}

pub fn route_path(absolute_path: &str) -> (Arc<dyn Dentry>, String) { 
    let mut current_path = absolute_path;
    loop {
        if let Some(dentry) = GLOBAL_DCACHE.get(current_path) {
            let relative_path = if current_path == absolute_path {
                "."
            } else if current_path == "/" {
                &absolute_path[1..]
            } else {
                &absolute_path[current_path.len() + 1..]
            };
            return (dentry.clone(), relative_path.to_string());
        }
        match current_path.rfind('/') {
            Some(0) => {
                current_path = "/";
            }
            Some(idx) => {
                current_path = &current_path[..idx];
            }
            None => {
                break;
            }
        }
    }
    panic!("VFS fatal: root dentry not found!");
}