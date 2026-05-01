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
/// Converts any path into a clean, absolute path.
/// 
/// - `cwd`: Current Working Directory. It must be an absolute path. 
///          If `path` is already absolute, `cwd` will be ignored.
/// - `path`: The target path input by the user. It can be absolute or relative.
// pub fn build_absolute_path(cwd: &str, path: &str) -> String {
//     let mut stack = Vec::new();
//     // If it is a relative path, push all parts of `cwd` into the stack first.
//     if !path.starts_with('/') {
//         for part in cwd.split('/').filter(|s| !s.is_empty()) {
//             stack.push(part);
//         }
//     }
//     //
//     for part in path.split('/').filter(|s| !s.is_empty()) {
//         match part {
//             "." => {
//             }
//             ".." => {
//                 stack.pop();
//             }
//             _ => {
//                 // Normal directory or file: add it to the stack
//                 stack.push(part);
//             }
//         }
//     }
//     // Rebuild the final absolute path from the stack.
//     if stack.is_empty() {
//         String::from("/")
//     } else {
//         let mut abs_path = String::from("/");
//         abs_path.push_str(&stack.join("/"));
//         abs_path
//     }
// }

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
pub fn resolve_path(cwd: Arc<dyn Dentry>, path: &str) -> SysResult<Arc<dyn Dentry>> {
    let mut current = if path.starts_with('/') {
        GLOBAL_DCACHE.get("/").unwrap().clone()
    } else {
        cwd
    };

    for part in path.split('/').filter(|s| !s.is_empty()) {
        match part {
            "." => {
                continue;
            }
            ".." => {
                current = current.parent().unwrap_or(current);
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

                if let Some(cached_node) = GLOBAL_DCACHE.get(&next_path) {
                    current = cached_node;
                } else {
                    let next_dentry = current.find(name)?;
                    info!("Resolved path: {}", next_path);
                    GLOBAL_DCACHE.insert(next_path, next_dentry.clone());
                    current = next_dentry;
                }
            }
        }
    }
    Ok(current)
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

// 这是一个极其强悍的路径解析路由中心
pub fn route_path(absolute_path: &str) -> (Arc<dyn Dentry>, String) {
    // 假设 absolute_path 是 "/musl/basic/mnt/test.txt"
    
    let mut current_path = absolute_path;

    // 从最长路径开始，一层层往上剥，看谁在 DCACHE 里（也就是寻找最近的挂载点或已缓存目录）
    loop {
        if let Some(dentry) = GLOBAL_DCACHE.get(current_path) {
            // 找到了最近的主管节点！
            // 计算剩下需要交给这个节点去底层解析的相对路径
            let relative_path = if current_path == absolute_path {
                // 正好是这个节点本身
                "."
            } else if current_path == "/" {
                // 如果回退到了根目录，相对路径就是去除了开头 '/' 的部分
                &absolute_path[1..]
            } else {
                // 比如 current_path 是 "/musl/basic/mnt"
                // 截取后面的 "/test.txt"，然后再去掉开头的 '/' 变成 "test.txt"
                &absolute_path[current_path.len() + 1..]
            };

            // 返回 (负责管这个路径的 Dentry, 剩下要处理的相对路径)
            return (dentry.clone(), relative_path.to_string());
        }

        // 如果没找到，剥离最后一层目录，继续往上找
        // "/musl/basic/mnt/test.txt" -> "/musl/basic/mnt" -> "/musl/basic" -> "/musl" -> "/"
        match current_path.rfind('/') {
            Some(0) => {
                // 退到了根目录 "/"
                current_path = "/";
            }
            Some(idx) => {
                // 截断到上一个 '/'
                current_path = &current_path[..idx];
            }
            None => {
                // 不可能是绝对路径，理论上不会走到这里
                break;
            }
        }
    }

    // 兜底：如果 DCACHE 连 "/" 都没有，说明内核没初始化好
    panic!("VFS fatal: root dentry not found!");
}