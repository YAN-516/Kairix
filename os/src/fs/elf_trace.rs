//! Focused lifecycle diagnostics for compiler-generated build-script ELFs.
//!
//! These logs intentionally cover only files whose basename contains
//! `build-script-build` under Cargo's target build directory. This keeps the
//! evidence useful during build storms while following one inode from writes
//! and truncates through close/rename and exec.

use crate::fs::page::pagecache::PAGE_CACHE;
use crate::fs::vfs::Dentry;
use crate::fs::vfs::file::File;
use crate::fs::vfs::inode::Inode;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};
use log::error;

const ELF64_HEADER_LEN: usize = 64;
const WRITE_MILESTONE: usize = 1024 * 1024;
const TRACKED_INODE_SLOTS: usize = 32;
static TRACKED_BUILD_SCRIPT_INODES: [AtomicUsize; TRACKED_INODE_SLOTS] =
    [const { AtomicUsize::new(0) }; TRACKED_INODE_SLOTS];
static TRACKED_RMETA_INODES: [AtomicUsize; TRACKED_INODE_SLOTS] =
    [const { AtomicUsize::new(0) }; TRACKED_INODE_SLOTS];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ElfHeaderState {
    hash: u32,
    program_end: Option<usize>,
    section_offset: usize,
    section_entry_size: usize,
    section_count: usize,
    section_end: Option<usize>,
    truncated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CachedElfHeaderState {
    ppn: usize,
    dirty: bool,
    dirty_generation: usize,
    header: ElfHeaderState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CachedPageSample {
    page_id: usize,
    ppn: usize,
    dirty: bool,
    dirty_generation: usize,
    hash: u32,
    prefix: u64,
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

fn header_hash(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .take(ELF64_HEADER_LEN)
        .fold(0x811c_9dc5u32, |hash, byte| {
            hash.wrapping_mul(0x0100_0193) ^ (*byte as u32)
        })
}

fn parse_elf_header(bytes: &[u8], file_size: usize) -> Option<ElfHeaderState> {
    if bytes.len() < ELF64_HEADER_LEN || bytes.get(..4)? != [0x7f, b'E', b'L', b'F'] {
        return None;
    }
    // Kairix currently supports 64-bit little-endian RISC-V and LoongArch.
    if bytes[4] != 2 || bytes[5] != 1 {
        return None;
    }

    let program_offset = read_u64(bytes, 0x20)? as usize;
    let section_offset = read_u64(bytes, 0x28)? as usize;
    let program_entry_size = read_u16(bytes, 0x36)? as usize;
    let program_count = read_u16(bytes, 0x38)? as usize;
    let section_entry_size = read_u16(bytes, 0x3a)? as usize;
    let section_count = read_u16(bytes, 0x3c)? as usize;
    let program_end = program_entry_size
        .checked_mul(program_count)
        .and_then(|size| program_offset.checked_add(size));
    let section_end = section_entry_size
        .checked_mul(section_count)
        .and_then(|size| section_offset.checked_add(size));

    Some(ElfHeaderState {
        hash: header_hash(bytes),
        program_end,
        section_offset,
        section_entry_size,
        section_count,
        section_end,
        truncated: section_end.is_none_or(|end| end > file_size),
    })
}

fn inode_id(inode: &Arc<dyn Inode>) -> usize {
    inode.cache_inode_id().unwrap_or_else(|| inode.get_ino())
}

fn cached_header(inode: &Arc<dyn Inode>) -> Option<CachedElfHeaderState> {
    let cache_id = inode.cache_inode_id()?;
    let page = PAGE_CACHE.get_page(cache_id, 0)?;
    let page = page.try_read()?;
    let dirty = page.dirty;
    let dirty_generation = page.dirty_generation();
    let frame = page.resident_frame()?;
    let ppn = frame.ppn.0;
    let mut bytes = [0u8; ELF64_HEADER_LEN];
    bytes.copy_from_slice(&frame.ppn.get_bytes_array()[..ELF64_HEADER_LEN]);
    drop(page);
    Some(CachedElfHeaderState {
        ppn,
        dirty,
        dirty_generation,
        header: parse_elf_header(&bytes, inode.get_size())?,
    })
}

/// Return whether `path` is a Cargo-generated build-script executable or one
/// of the linker's temporary files for that executable.
pub(crate) fn is_build_script_path(path: &str) -> bool {
    let in_build_dir =
        path.contains("target/debug/build/") || path.contains("target/release/build/");
    in_build_dir
        && path
            .rsplit('/')
            .next()
            .is_some_and(|name| name.contains("build-script-build"))
}

fn is_libc_rmeta_path(path: &str) -> bool {
    let in_deps = path.contains("target/debug/deps/") || path.contains("target/release/deps/");
    in_deps
        && path
            .rsplit('/')
            .next()
            .is_some_and(|name| name.starts_with("liblibc-") && name.ends_with(".rmeta"))
}

fn tracked_inode(slots: &[AtomicUsize; TRACKED_INODE_SLOTS], inode: &Arc<dyn Inode>) -> bool {
    let id = inode_id(inode);
    slots.iter().any(|slot| slot.load(Ordering::Acquire) == id)
}

fn remember_inode(slots: &[AtomicUsize; TRACKED_INODE_SLOTS], inode: &Arc<dyn Inode>) {
    let id = inode_id(inode);
    if slots.iter().any(|slot| slot.load(Ordering::Acquire) == id) {
        return;
    }
    for slot in slots {
        if slot
            .compare_exchange(0, id, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return;
        }
    }
    slots[id % TRACKED_INODE_SLOTS].store(id, Ordering::Release);
}

fn remember_path_identity(path: &str, inode: &Arc<dyn Inode>) {
    if is_build_script_path(path) {
        remember_inode(&TRACKED_BUILD_SCRIPT_INODES, inode);
    }
    if is_libc_rmeta_path(path) {
        remember_inode(&TRACKED_RMETA_INODES, inode);
    }
}

fn is_tracked_build_script(path: &str, inode: &Arc<dyn Inode>) -> bool {
    is_build_script_path(path) || tracked_inode(&TRACKED_BUILD_SCRIPT_INODES, inode)
}

fn is_tracked_rmeta(path: &str, inode: &Arc<dyn Inode>) -> bool {
    is_libc_rmeta_path(path) || tracked_inode(&TRACKED_RMETA_INODES, inode)
}

fn cached_page_sample(inode: &Arc<dyn Inode>, page_id: usize) -> Option<CachedPageSample> {
    let cache_id = inode.cache_inode_id()?;
    let page = PAGE_CACHE.get_page(cache_id, page_id)?;
    let page = page.try_read()?;
    let dirty = page.dirty;
    let dirty_generation = page.dirty_generation();
    let frame = page.resident_frame()?;
    let bytes = frame.ppn.get_bytes_array();
    let hash = bytes.iter().fold(0x811c_9dc5u32, |hash, byte| {
        hash.wrapping_mul(0x0100_0193) ^ (*byte as u32)
    });
    let mut prefix_bytes = [0u8; 8];
    prefix_bytes.copy_from_slice(&bytes[..8]);
    Some(CachedPageSample {
        page_id,
        ppn: frame.ppn.0,
        dirty,
        dirty_generation,
        hash,
        prefix: u64::from_le_bytes(prefix_bytes),
    })
}

fn cached_artifact_samples(inode: &Arc<dyn Inode>) -> [Option<CachedPageSample>; 3] {
    let last_page = inode.get_size().saturating_sub(1) / 4096;
    [
        cached_page_sample(inode, 0),
        cached_page_sample(inode, last_page / 2),
        cached_page_sample(inode, last_page),
    ]
}

fn log_inode_state(event: &str, pid: usize, fd: Option<usize>, path: &str, inode: &Arc<dyn Inode>) {
    remember_path_identity(path, inode);
    if is_tracked_build_script(path, inode) {
        error!(
            "[ELF_BUILD_STATE] event={} pid={} fd={:?} path={} inode={:#x} size={} generation={} cache_header={:?}",
            event,
            pid,
            fd,
            path,
            inode_id(inode),
            inode.get_size(),
            inode.page_cache_generation(),
            cached_header(inode),
        );
    }
    if is_tracked_rmeta(path, inode) {
        error!(
            "[RMETA_BUILD_STATE] event={} pid={} fd={:?} path={} inode={:#x} size={} generation={} cache_samples={:?}",
            event,
            pid,
            fd,
            path,
            inode_id(inode),
            inode.get_size(),
            inode.page_cache_generation(),
            cached_artifact_samples(inode),
        );
    }
}

/// Log the current inode/page-cache state at a file lifecycle boundary.
pub(crate) fn log_file_state<F: File + ?Sized>(
    event: &str,
    pid: usize,
    fd: Option<usize>,
    file: &Arc<F>,
) {
    let Some(inode) = file.get_inode() else {
        return;
    };
    let path = file.get_dentry().path();
    log_inode_state(event, pid, fd, &path, &inode);
}

/// Log only significant write transitions: the initial/header write, each
/// MiB boundary, a short write, or any size invariant violation.
pub(crate) fn log_write_result<F: File + ?Sized>(
    op: &str,
    pid: usize,
    fd: usize,
    file: &Arc<F>,
    offset: usize,
    requested: usize,
    written: usize,
    old_size: usize,
) {
    let Some(inode) = file.get_inode() else {
        return;
    };
    let path = file.get_dentry().path();
    remember_path_identity(&path, &inode);
    let trace_elf = is_tracked_build_script(&path, &inode);
    let trace_rmeta = is_tracked_rmeta(&path, &inode);
    if !trace_elf && !trace_rmeta {
        return;
    }
    let new_size = inode.get_size();
    let write_end = offset.checked_add(written);
    let crossed_milestone = write_end
        .is_some_and(|end| offset / WRITE_MILESTONE != end.saturating_sub(1) / WRITE_MILESTONE);
    let invariant_broken = write_end.is_none_or(|end| new_size < end) || new_size < old_size;
    if offset != 0 && !crossed_milestone && written == requested && !invariant_broken {
        return;
    }
    if trace_elf {
        error!(
            "[ELF_BUILD_WRITE] op={} pid={} fd={} path={} inode={:#x} offset={} requested={} written={} write_end={:?} old_size={} new_size={} generation={} invariant_broken={} cache_header={:?}",
            op,
            pid,
            fd,
            path,
            inode_id(&inode),
            offset,
            requested,
            written,
            write_end,
            old_size,
            new_size,
            inode.page_cache_generation(),
            invariant_broken,
            cached_header(&inode),
        );
    }
    if trace_rmeta {
        error!(
            "[RMETA_BUILD_WRITE] op={} pid={} fd={} path={} inode={:#x} offset={} requested={} written={} write_end={:?} old_size={} new_size={} generation={} invariant_broken={} cache_samples={:?}",
            op,
            pid,
            fd,
            path,
            inode_id(&inode),
            offset,
            requested,
            written,
            write_end,
            old_size,
            new_size,
            inode.page_cache_generation(),
            invariant_broken,
            cached_artifact_samples(&inode),
        );
    }
}

/// Log a truncation before and after it changes the inode.
pub(crate) fn log_truncate<F: File + ?Sized>(
    event: &str,
    pid: usize,
    fd: Option<usize>,
    file: &Arc<F>,
    old_size: usize,
    requested_size: usize,
) {
    let Some(inode) = file.get_inode() else {
        return;
    };
    let path = file.get_dentry().path();
    remember_path_identity(&path, &inode);
    let trace_elf = is_tracked_build_script(&path, &inode);
    let trace_rmeta = is_tracked_rmeta(&path, &inode);
    if !trace_elf && !trace_rmeta {
        return;
    }
    if trace_elf {
        error!(
            "[ELF_BUILD_TRUNCATE] event={} pid={} fd={:?} path={} inode={:#x} old_size={} requested_size={} observed_size={} generation={} cache_header={:?}",
            event,
            pid,
            fd,
            path,
            inode_id(&inode),
            old_size,
            requested_size,
            inode.get_size(),
            inode.page_cache_generation(),
            cached_header(&inode),
        );
    }
    if trace_rmeta {
        error!(
            "[RMETA_BUILD_TRUNCATE] event={} pid={} fd={:?} path={} inode={:#x} old_size={} requested_size={} observed_size={} generation={} cache_samples={:?}",
            event,
            pid,
            fd,
            path,
            inode_id(&inode),
            old_size,
            requested_size,
            inode.get_size(),
            inode.page_cache_generation(),
            cached_artifact_samples(&inode),
        );
    }
}

/// Log the inode identity across a namespace rename.
pub(crate) fn log_rename_state(
    event: &str,
    pid: usize,
    old_path: &str,
    new_path: &str,
    dentry: &Arc<dyn Dentry>,
) {
    let Some(inode) = dentry.get_inode() else {
        return;
    };
    remember_path_identity(old_path, &inode);
    remember_path_identity(new_path, &inode);
    if is_tracked_build_script(old_path, &inode) || is_tracked_build_script(new_path, &inode) {
        error!(
            "[ELF_BUILD_RENAME] event={} pid={} old={} new={} inode={:#x} size={} generation={} cache_header={:?}",
            event,
            pid,
            old_path,
            new_path,
            inode_id(&inode),
            inode.get_size(),
            inode.page_cache_generation(),
            cached_header(&inode),
        );
    }
    if is_tracked_rmeta(old_path, &inode) || is_tracked_rmeta(new_path, &inode) {
        error!(
            "[RMETA_BUILD_RENAME] event={} pid={} old={} new={} inode={:#x} size={} generation={} cache_samples={:?}",
            event,
            pid,
            old_path,
            new_path,
            inode_id(&inode),
            inode.get_size(),
            inode.page_cache_generation(),
            cached_artifact_samples(&inode),
        );
    }
}

/// Log hard-link publication of a compiler artifact.
pub(crate) fn log_link_state(
    event: &str,
    pid: usize,
    old_path: &str,
    new_path: &str,
    dentry: &Arc<dyn Dentry>,
) {
    let Some(inode) = dentry.get_inode() else {
        return;
    };
    remember_path_identity(old_path, &inode);
    remember_path_identity(new_path, &inode);
    if is_tracked_build_script(old_path, &inode) || is_tracked_build_script(new_path, &inode) {
        error!(
            "[ELF_BUILD_LINK] event={} pid={} old={} new={} inode={:#x} size={} generation={} cache_header={:?}",
            event,
            pid,
            old_path,
            new_path,
            inode_id(&inode),
            inode.get_size(),
            inode.page_cache_generation(),
            cached_header(&inode),
        );
    }
    if is_tracked_rmeta(old_path, &inode) || is_tracked_rmeta(new_path, &inode) {
        error!(
            "[RMETA_BUILD_LINK] event={} pid={} old={} new={} inode={:#x} size={} generation={} cache_samples={:?}",
            event,
            pid,
            old_path,
            new_path,
            inode_id(&inode),
            inode.get_size(),
            inode.page_cache_generation(),
            cached_artifact_samples(&inode),
        );
    }
}

/// Compare the direct-read ELF header used by exec with page-cache page zero.
pub(crate) fn log_exec_header_compare<F: File + ?Sized>(
    pid: usize,
    path: &str,
    file: &Arc<F>,
    direct_header: &[u8],
) {
    if !is_build_script_path(path) {
        return;
    }
    let Some(inode) = file.get_inode() else {
        return;
    };
    let file_size = inode.get_size();
    let direct = parse_elf_header(direct_header, file_size);
    let cached = cached_header(&inode);
    let headers_match = direct
        .zip(cached)
        .is_some_and(|(disk, cache)| disk == cache.header);
    error!(
        "[ELF_EXEC_INTEGRITY] pid={} path={} inode={:#x} size={} generation={} direct_header={:?} cache_header={:?} headers_match={}",
        pid,
        path,
        inode_id(&inode),
        file_size,
        inode.page_cache_generation(),
        direct,
        cached,
        headers_match,
    );
}
