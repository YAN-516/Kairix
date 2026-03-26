use alloc::string::String;
use alloc::vec::Vec;

use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use alloc::sync::Arc;
use crate::fs::vfs::Dentry;
use alloc::format;
use log::*;
use crate::task::current_process;
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
pub fn resolve_path(cwd: Arc<dyn Dentry>, path: &str) -> Option<Arc<dyn Dentry>> {
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
    Some(current)
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
pub fn get_start_dentry(dirfd: isize, path: &str) -> Result<Arc<dyn Dentry>, isize> {
    let process = current_process();
    let inner = process.inner_exclusive_access();
    if path.starts_with('/') {
        return Ok(GLOBAL_DCACHE.get("/").unwrap().clone());
    } else if dirfd == AT_FDCWD {
        return Ok(inner.cwd.clone());
    } else {
        let fd = dirfd as usize;
        if fd >= inner.fd_table.len() || inner.fd_table[fd].is_none() {
            return Err(-9); 
        }
        let file = inner.fd_table[fd].as_ref().unwrap();
        return Ok(file.get_dentry());
    };
}