use alloc::string::String;
use alloc::vec::Vec;

use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use alloc::sync::Arc;
use crate::fs::vfs::Dentry;
use alloc::format;
use log::*;
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