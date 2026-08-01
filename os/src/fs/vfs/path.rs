use alloc::string::String;
use alloc::vec::Vec;

use crate::alloc::string::ToString;
use crate::error::{SysError, SysResult};
use crate::fs::vfs::Dentry;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::inode::InodeMode;
use crate::task::current_process;
use alloc::format;
use alloc::sync::Arc;
use log::*;

const READLINKAT_SYSCALL_ID: usize = 78;
const READLINKAT_PATH_SLOW_NS: usize = 10_000_000;

// readlinkat path-resolution stages published in CURRENT_TASK_SYSCALL_STAGES:
// 781xx=get_start_dentry, 782xx=generic component walk.  Filesystem-specific
// implementations reserve 783xx for procfs and 784xx for lwext4.
#[inline]
fn record_readlinkat_path_stage(stage: usize) {
    crate::task::processor::record_current_syscall_stage_nolock(READLINKAT_SYSCALL_ID, stage);
}

#[inline]
fn path_diag_now_ns() -> usize {
    polyhal::timer::current_time().as_nanos() as usize
}

fn log_slow_readlinkat_path_step(
    step: &'static str,
    started_ns: usize,
    component_index: usize,
    component_count: usize,
    path: &str,
    outcome: &str,
) {
    let elapsed_ns = path_diag_now_ns().saturating_sub(started_ns);
    if elapsed_ns < READLINKAT_PATH_SLOW_NS {
        return;
    }
    let (pid, tid) = crate::task::current_task()
        .map(|task| (task.process_id(), task.global_tid()))
        .unwrap_or((usize::MAX, usize::MAX));
    error!(
        "[READLINKAT_PATH_SLOW] cpu={} pid={} tid={} step={} elapsed_ns={} component_index={} component_count={} path={} outcome={} dcache={:?}",
        polyhal::arch::hart_id(),
        pid,
        tid,
        step,
        elapsed_ns,
        component_index,
        component_count,
        path,
        outcome,
        GLOBAL_DCACHE.try_stats(),
    );
}

/// Constraints applied while walking a pathname.  These checks live in the
/// component walker so symlink expansion and mount transitions cannot bypass
/// them between a preliminary validation and the actual open.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct PathResolutionOptions {
    pub no_xdev: bool,
    pub no_magiclinks: bool,
    pub no_symlinks: bool,
    pub beneath: bool,
    pub in_root: bool,
}

fn is_proc_magiclink(path: &str) -> bool {
    if !path.starts_with("/proc/") {
        return false;
    }
    path.ends_with("/exe")
        || path.ends_with("/cwd")
        || path.ends_with("/root")
        || path
            .split('/')
            .collect::<Vec<_>>()
            .windows(2)
            .any(|parts| parts[0] == "fd" && parts[1].parse::<usize>().is_ok())
}
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
fn can_use_full_path_cache(path: &str) -> bool {
    if path == "/" {
        return true;
    }
    if !path.starts_with('/') || path.ends_with('/') || path.starts_with("/proc/") {
        return false;
    }
    if path.as_bytes().windows(2).any(|window| window == b"//") {
        return false;
    }
    !path.split('/').any(|part| part == "." || part == "..")
}

fn cached_absolute_path(path: &str, follow_last: bool) -> Option<Arc<dyn Dentry>> {
    if !can_use_full_path_cache(path) {
        return None;
    }
    let cached = GLOBAL_DCACHE.get(path)?;
    if cached.path() != path {
        return None;
    }
    let inode = cached.get_inode()?;
    if follow_last && inode.get_mode().contains(InodeMode::LINK) {
        return None;
    }
    Some(cached)
}

fn resolve_path_inner(
    cwd: Arc<dyn Dentry>,
    path: &str,
    follow_last: bool,
    options: PathResolutionOptions,
) -> SysResult<Arc<dyn Dentry>> {
    const MAX_SYMLINK_FOLLOWS: usize = 40;
    let mut symlink_count = 0;

    record_readlinkat_path_stage(78200);
    if options == PathResolutionOptions::default() {
        record_readlinkat_path_stage(78201);
        let cache_started_ns = path_diag_now_ns();
        let cached = cached_absolute_path(path, follow_last);
        log_slow_readlinkat_path_step(
            "full_path_cache",
            cache_started_ns,
            0,
            0,
            path,
            if cached.is_some() { "hit" } else { "miss" },
        );
        if let Some(cached) = cached {
            record_readlinkat_path_stage(78202);
            return Ok(cached);
        }
    }

    if options.beneath && options.in_root {
        return Err(SysError::EINVAL);
    }
    if options.beneath && path.starts_with('/') {
        return Err(SysError::EXDEV);
    }

    record_readlinkat_path_stage(78203);
    let mut current = if path.starts_with('/') && !options.in_root {
        let root_started_ns = path_diag_now_ns();
        let root = GLOBAL_DCACHE.get("/").unwrap().clone();
        log_slow_readlinkat_path_step("root_dcache", root_started_ns, 0, 0, path, "hit");
        root
    } else {
        cwd
    };
    let boundary = current.clone();
    let boundary_sb = options
        .no_xdev
        .then(|| crate::fs::find_superblock_by_path(&boundary.path()))
        .flatten();

    if options.no_xdev {
        let current_sb = crate::fs::find_superblock_by_path(&current.path());
        if boundary_sb
            .as_ref()
            .zip(current_sb.as_ref())
            .is_some_and(|(left, right)| !Arc::ptr_eq(left, right))
        {
            return Err(SysError::EXDEV);
        }
    }

    if path.is_empty() {
        return Ok(current);
    }

    record_readlinkat_path_stage(78204);
    let split_started_ns = path_diag_now_ns();
    let mut parts: Vec<String> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    log_slow_readlinkat_path_step(
        "split_components",
        split_started_ns,
        0,
        parts.len(),
        path,
        "ok",
    );
    let mut i = 0;

    while i < parts.len() {
        record_readlinkat_path_stage(78210);
        let part = parts[i].clone();
        let is_last = i == parts.len() - 1;

        match part.as_str() {
            "." => {
                i += 1;
                continue;
            }
            ".." => {
                if Arc::ptr_eq(&current, &boundary) && (options.beneath || options.in_root) {
                    if options.beneath {
                        return Err(SysError::EXDEV);
                    }
                } else {
                    if options.no_xdev && current.get_mount_dentry().is_some() {
                        return Err(SysError::EXDEV);
                    }
                    let parent = current.parent().unwrap_or_else(|| current.clone());
                    if options.no_xdev {
                        let parent_sb = crate::fs::find_superblock_by_path(&parent.path());
                        if boundary_sb
                            .as_ref()
                            .zip(parent_sb.as_ref())
                            .is_some_and(|(left, right)| !Arc::ptr_eq(left, right))
                        {
                            return Err(SysError::EXDEV);
                        }
                    }
                    current = parent;
                }
                i += 1;
                continue;
            }
            name => {
                // 路径中间组件必须是目录，否则返回 ENOTDIR
                record_readlinkat_path_stage(78211);
                let inode_started_ns = path_diag_now_ns();
                if let Some(inode) = current.get_inode() {
                    log_slow_readlinkat_path_step(
                        "parent_get_inode",
                        inode_started_ns,
                        i,
                        parts.len(),
                        path,
                        "present",
                    );
                    if inode.get_mode().get_type() != InodeMode::DIR {
                        return Err(SysError::ENOTDIR);
                    }
                } else {
                    log_slow_readlinkat_path_step(
                        "parent_get_inode",
                        inode_started_ns,
                        i,
                        parts.len(),
                        path,
                        "missing",
                    );
                    return Err(SysError::ENOTDIR);
                }
                record_readlinkat_path_stage(78212);
                let parent_path_started_ns = path_diag_now_ns();
                let current_path = current.path();
                log_slow_readlinkat_path_step(
                    "parent_path",
                    parent_path_started_ns,
                    i,
                    parts.len(),
                    path,
                    "ok",
                );
                let next_path = if current_path == "/" {
                    format!("/{}", name)
                } else {
                    format!("{}/{}", current_path, name)
                };

                let dynamic_proc = next_path.starts_with("/proc/self/")
                    || next_path
                        .as_bytes()
                        .get(6..)
                        .is_some_and(|rest| rest.iter().any(|byte| *byte == b'/'));

                let next_dentry = if !dynamic_proc {
                    record_readlinkat_path_stage(78213);
                    let component_cache_started_ns = path_diag_now_ns();
                    let cached_node = GLOBAL_DCACHE.get(&next_path);
                    log_slow_readlinkat_path_step(
                        "component_dcache",
                        component_cache_started_ns,
                        i,
                        parts.len(),
                        &next_path,
                        if cached_node.is_some() { "hit" } else { "miss" },
                    );
                    if let Some(cached_node) = cached_node {
                        // 如果缓存 dentry 的 parent 已被 LRU 淘汰，path() 会返回错误路径，
                        // 导致后续 ext4_fopen 使用错误路径而 panic。这里做一致性校验。
                        record_readlinkat_path_stage(78214);
                        if cached_node.path() == next_path {
                            cached_node
                        } else {
                            record_readlinkat_path_stage(78215);
                            let find_started_ns = path_diag_now_ns();
                            let found = current.find(name);
                            log_slow_readlinkat_path_step(
                                "dentry_find_stale_cache",
                                find_started_ns,
                                i,
                                parts.len(),
                                &next_path,
                                if found.is_ok() { "ok" } else { "error" },
                            );
                            let d = found?;
                            debug!("Resolved path (cache stale): {}", next_path);
                            record_readlinkat_path_stage(78216);
                            let insert_started_ns = path_diag_now_ns();
                            GLOBAL_DCACHE.insert(next_path.clone(), d.clone());
                            log_slow_readlinkat_path_step(
                                "dcache_insert_stale",
                                insert_started_ns,
                                i,
                                parts.len(),
                                path,
                                "ok",
                            );
                            d
                        }
                    } else {
                        record_readlinkat_path_stage(78217);
                        let find_started_ns = path_diag_now_ns();
                        let found = current.find(name);
                        log_slow_readlinkat_path_step(
                            "dentry_find_cache_miss",
                            find_started_ns,
                            i,
                            parts.len(),
                            &next_path,
                            if found.is_ok() { "ok" } else { "error" },
                        );
                        let d = found?;
                        debug!("Resolved path: {}", next_path);
                        record_readlinkat_path_stage(78218);
                        let insert_started_ns = path_diag_now_ns();
                        GLOBAL_DCACHE.insert(next_path.clone(), d.clone());
                        log_slow_readlinkat_path_step(
                            "dcache_insert_miss",
                            insert_started_ns,
                            i,
                            parts.len(),
                            path,
                            "ok",
                        );
                        d
                    }
                } else {
                    record_readlinkat_path_stage(78219);
                    let find_started_ns = path_diag_now_ns();
                    let found = current.find(name);
                    log_slow_readlinkat_path_step(
                        "dynamic_proc_find",
                        find_started_ns,
                        i,
                        parts.len(),
                        &next_path,
                        if found.is_ok() { "ok" } else { "error" },
                    );
                    found?
                };

                if options.no_xdev {
                    let next_sb = crate::fs::find_superblock_by_path(&next_dentry.path());
                    let crossed_superblock = boundary_sb
                        .as_ref()
                        .zip(next_sb.as_ref())
                        .is_some_and(|(left, right)| !Arc::ptr_eq(left, right));
                    if crossed_superblock || next_dentry.get_mount_dentry().is_some() {
                        return Err(SysError::EXDEV);
                    }
                }

                // 检查是否为符号链接
                record_readlinkat_path_stage(78220);
                if let Some(inode) = next_dentry.get_inode() {
                    if inode.get_mode().contains(InodeMode::LINK) {
                        // 如果是最后一个组件且不跟随，直接返回 symlink 本身
                        if is_last && !follow_last {
                            return Ok(next_dentry);
                        }

                        if options.no_symlinks
                            || (options.no_magiclinks && is_proc_magiclink(&next_dentry.path()))
                        {
                            return Err(SysError::ELOOP);
                        }

                        if symlink_count >= MAX_SYMLINK_FOLLOWS {
                            return Err(SysError::ELOOP);
                        }
                        symlink_count += 1;

                        record_readlinkat_path_stage(78221);
                        let readlink_started_ns = path_diag_now_ns();
                        let target_result = inode.readlink();
                        log_slow_readlinkat_path_step(
                            "intermediate_symlink_readlink",
                            readlink_started_ns,
                            i,
                            parts.len(),
                            &next_path,
                            if target_result.is_ok() { "ok" } else { "error" },
                        );
                        let target = target_result.map_err(|e| {
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
                            if options.beneath {
                                return Err(SysError::EXDEV);
                            }
                            current = if options.in_root {
                                boundary.clone()
                            } else {
                                GLOBAL_DCACHE.get("/").unwrap().clone()
                            };
                            if options.no_xdev {
                                let target_sb = crate::fs::find_superblock_by_path(&current.path());
                                if boundary_sb
                                    .as_ref()
                                    .zip(target_sb.as_ref())
                                    .is_some_and(|(left, right)| !Arc::ptr_eq(left, right))
                                {
                                    return Err(SysError::EXDEV);
                                }
                            }
                        }
                        // 相对路径保持 current 不变

                        // 重新拆分路径
                        record_readlinkat_path_stage(78222);
                        parts = new_path
                            .split('/')
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string())
                            .collect();
                        i = 0;
                        continue;
                    }
                }

                current = next_dentry;
                i += 1;
            }
        }
    }
    record_readlinkat_path_stage(78299);
    Ok(current)
}

/// 解析路径，默认跟随所有符号链接（包括最后一个组件）。
pub fn resolve_path(cwd: Arc<dyn Dentry>, path: &str) -> SysResult<Arc<dyn Dentry>> {
    resolve_path_inner(cwd, path, true, PathResolutionOptions::default())
}

/// 解析路径，中间组件跟随符号链接，但最后一个组件如果是符号链接则直接返回 symlink 本身。
pub fn resolve_path_nofollow_last(cwd: Arc<dyn Dentry>, path: &str) -> SysResult<Arc<dyn Dentry>> {
    resolve_path_inner(cwd, path, false, PathResolutionOptions::default())
}

pub fn resolve_path_with_options(
    cwd: Arc<dyn Dentry>,
    path: &str,
    follow_last: bool,
    options: PathResolutionOptions,
) -> SysResult<Arc<dyn Dentry>> {
    resolve_path_inner(cwd, path, follow_last, options)
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
        None => (String::from("."), String::from(path)),
    }
}

pub const AT_FDCWD: isize = -100;
/// return the dentry of the start point of the path, which is determined by dirfd
/// 1 /
/// 2 cwd
/// 3 dirfd
pub fn get_start_dentry(dirfd: isize, path: &str) -> SysResult<Arc<dyn Dentry>> {
    record_readlinkat_path_stage(78100);
    if path.starts_with('/') {
        // Dentry-cache lookup takes a SleepLock and may cooperatively block.
        // Never retain ProcessInnerGuard here: it also owns the possibly shared
        // CLONE_FILES gate, and blocking with that gate held prevents another
        // thread in the same files_struct from making progress or waking the
        // cache owner.
        record_readlinkat_path_stage(78101);
        return Ok(GLOBAL_DCACHE.get("/").unwrap().clone());
    }

    record_readlinkat_path_stage(78102);
    let process = current_process();
    if dirfd == AT_FDCWD {
        record_readlinkat_path_stage(78103);
        let fs_context = {
            let inner = process.inner_exclusive_access();
            inner.fs_context.clone()
        };
        record_readlinkat_path_stage(78104);
        return Ok(fs_context.lock().cwd.clone());
    }

    record_readlinkat_path_stage(78105);
    let file = {
        // Taking an Arc reference is the fdget-style snapshot: close may remove
        // the descriptor after this point, but this operation retains the open
        // file description until path-start validation finishes.
        let inner = process.inner_exclusive_access();
        let fd = dirfd as usize;
        inner
            .fd_table
            .get(fd)
            .and_then(|entry| entry.as_ref())
            .cloned()
            .ok_or(SysError::EBADF)?
    };

    // 相对路径 + 显式 dirfd 的语义要求该 fd 必须可作为目录起点。
    // 对于 pipe/socket/tty 等无目录语义的 fd，返回 ENOTDIR，避免触发 get_dentry panic。
    record_readlinkat_path_stage(78106);
    let inode = file.get_inode().ok_or(SysError::ENOTDIR)?;
    if inode.get_mode().get_type() != crate::fs::vfs::inode::InodeMode::DIR {
        return Err(SysError::ENOTDIR);
    }
    record_readlinkat_path_stage(78107);
    Ok(file.get_dentry())
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
