//! System V Shared Memory syscalls
//!
//! Implements shmget, shmat, shmdt, shmctl for SysV IPC shared memory.
//! This is needed by applications like iozone that use shared memory
//! for multi-process throughput testing.
//!
//! RISC-V Linux syscall numbers:
//! - shmget: 194
//! - shmctl: 195
//! - shmat: 196
//! - shmdt: 197
use alloc::vec::Vec;
use crate::error::{SysError, SyscallResult};
use crate::mm::frame_alloc;
use crate::mm::vm_area::{MapArea, UserMapArea, UserMapAreaType, MapType, MmapType};
use crate::task::current_process;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use polyhal::common::FrameTracker;
use polyhal::consts::PAGE_SIZE;
use polyhal::pagetable::*;
use polyhal::utils::addr::{VirtAddr, VirtPageNum, VPNRange};
use crate::sync::SpinNoIrqLock;

/// Maximum number of shared memory segments
const SHMMNI: usize = 128;
/// Minimum shared memory segment size (1 byte)
const SHMMIN: usize = 1;
/// Maximum shared memory segment size (256 MB)
const SHMMAX: usize = 256 * 1024 * 1024;

/// IPC_CREAT flag
const IPC_CREAT: i32 = 0o1000;
/// IPC_EXCL flag
const IPC_EXCL: i32 = 0o2000;
/// IPC_RMID command
const IPC_RMID: i32 = 0;
/// IPC_SET command
const IPC_SET: i32 = 1;
/// IPC_STAT command
const IPC_STAT: i32 = 2;
/// IPC_INFO command
const IPC_INFO: i32 = 3;
/// SHM_INFO command
const SHM_INFO: i32 = 12;
/// SHM_STAT command
const SHM_STAT: i32 = 13;
/// SHM_STAT_ANY command
const SHM_STAT_ANY: i32 = 14;

/// SHM_RDONLY flag for shmat
const SHM_RDONLY: i32 = 0o10000;
// /// SHM_REMAP flag for shmat
// const SHM_REMAP: i32 = 0o20000;

/// A shared memory segment descriptor
struct ShmSegment {
    /// Key (IPC_PRIVATE = 0 means private)
    key: i32,
    /// Size in bytes (page-aligned)
    size: usize,
    /// Number of pages
    num_pages: usize,
    /// Permission mode
    mode: i32,
    /// Physical pages backing this segment
    pages: Vec<Arc<FrameTracker>>,
    /// Number of current attaches
    shm_nattch: usize,
    /// Creator PID
    shm_cpid: usize,
    /// Last operation PID
    shm_lpid: usize,
    /// Marked for destruction (IPC_RMID)
    destroyed: bool,
}

/// Global shared memory state
struct ShmState {
    /// Next shmid to assign
    next_id: usize,
    /// Map from shmid to segment
    segments: BTreeMap<usize, ShmSegment>,
    /// Map from key to shmid (for key-based lookup)
    key_to_id: BTreeMap<i32, usize>,
}

impl ShmState {
    #[allow(dead_code)]
    fn new() -> Self {
        ShmState {
            next_id: 0,
            segments: BTreeMap::new(),
            key_to_id: BTreeMap::new(),
        }
    }

    fn alloc_id(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

/// Global shared memory state (protected by SpinNoIrqLock)
static SHM_STATE: SpinNoIrqLock<ShmState> = SpinNoIrqLock::new(ShmState {
    next_id: 0,
    segments: BTreeMap::new(),
    key_to_id: BTreeMap::new(),
});

/// Get the current process's PID
fn current_pid() -> usize {
    let process = current_process();
    process.pid.0
}

/// ipc_perm structure (as seen by userspace)
#[repr(C)]
struct IpcPerm {
    key: i32,
    uid: u32,
    gid: u32,
    cuid: u32,
    cgid: u32,
    mode: i32,
    __pad1: u16,
    seq: u16,
    __pad2: u64,
    __unused1: u64,
    __unused2: u64,
}

/// shmid_ds structure (as seen by userspace)
#[repr(C)]
struct ShmIdDs {
    shm_perm: IpcPerm,
    shm_segsz: usize,
    shm_atime: u64,
    shm_dtime: u64,
    shm_ctime: u64,
    shm_cpid: i32,
    shm_lpid: i32,
    shm_nattch: usize,
    __unused4: u64,
    __unused5: u64,
}

/// shminfo structure for IPC_INFO
#[repr(C)]
struct ShmInfo {
    shmmax: usize,
    shmmin: usize,
    shmmni: usize,
    shmseg: usize,
    shmall: usize,
    __unused: [u64; 4],
}

/// System call: shmget - allocate a shared memory segment
///
/// # Arguments
/// * `key` - IPC key (IPC_PRIVATE=0 for private)
/// * `size` - Size in bytes
/// * `shmflg` - Flags (IPC_CREAT, IPC_EXCL, permission bits)
///
/// # Returns
/// * Shared memory identifier on success
pub fn sys_shmget(key: i32, size: usize, shmflg: i32) -> SyscallResult {
    // Validate size
    if size == 0 || size > SHMMAX {
        return Err(SysError::EINVAL);
    }

    let page_aligned_size = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let num_pages = page_aligned_size / PAGE_SIZE;

    let pid = current_pid();

    let mut state = SHM_STATE.lock();

    // Check if key already exists
    if key != 0 {
        if let Some(&shmid) = state.key_to_id.get(&key) {
            if let Some(seg) = state.segments.get(&shmid) {
                // Check size compatibility
                if size > seg.size {
                    return Err(SysError::EINVAL);
                }
                // If IPC_EXCL and IPC_CREAT, fail
                if (shmflg & IPC_EXCL) != 0 && (shmflg & IPC_CREAT) != 0 {
                    return Err(SysError::EEXIST);
                }
                return Ok(shmid);
            }
        }
    }

    // If IPC_CREAT not set and key doesn't exist, fail
    if (shmflg & IPC_CREAT) == 0 {
        return Err(SysError::ENOENT);
    }

    // Check SHMMNI limit
    if state.segments.len() >= SHMMNI {
        return Err(SysError::ENOSPC);
    }

    // Allocate physical pages
    let mut pages = Vec::new();
    for _ in 0..num_pages {
        match frame_alloc() {
            Some(frame) => pages.push(Arc::new(frame)),
            None => return Err(SysError::ENOMEM),
        }
    }

    let mode = shmflg & 0o777;
    let shmid = state.alloc_id();
    let seg = ShmSegment {
        key,
        size: page_aligned_size,
        num_pages,
        mode,
        pages,
        shm_nattch: 0,
        shm_cpid: pid,
        shm_lpid: pid,
        destroyed: false,
    };

    if key != 0 {
        state.key_to_id.insert(key, shmid);
    }
    state.segments.insert(shmid, seg);

    Ok(shmid)
}

/// System call: shmat - attach shared memory segment
///
/// # Arguments
/// * `shmid` - Shared memory identifier
/// * `shmaddr` - Desired virtual address (0 = let kernel choose)
/// * `shmflg` - Flags (SHM_RDONLY, SHM_REMAP)
///
/// # Returns
/// * Attached virtual address on success
pub fn sys_shmat(shmid: usize, shmaddr: *const u8, shmflg: i32) -> SyscallResult {
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    let mut state = SHM_STATE.lock();

    let seg = match state.segments.get(&shmid) {
        Some(s) => s,
        None => return Err(SysError::EINVAL),
    };

    let size = seg.size;
    let num_pages = seg.num_pages;

    // Find a free area in the process's address space
    let readonly = (shmflg & SHM_RDONLY) != 0;
    let map_perm = if readonly {
        MapPermission::R | MapPermission::U
    } else {
        MapPermission::R | MapPermission::W | MapPermission::U
    };

    let target_start = if shmaddr.is_null() {
        // Let kernel choose - use mmap area
        match inner.vm_set.find_free_area(0, size) {
            Some(addr) => addr,
            None => return Err(SysError::ENOMEM),
        }
    } else {
        let addr = shmaddr as usize;
        if (addr & (PAGE_SIZE - 1)) != 0 {
            return Err(SysError::EINVAL);
        }
        // Check for overlap with existing areas
        let end_addr = addr + size;
        for area in inner.vm_set.areas.iter() {
            if !(end_addr <= area.start_va().0 || addr >= area.end_va().0) {
                return Err(SysError::EINVAL);
            }
        }
        addr
    };

    let start_va = VirtAddr::from(target_start);
    let end_va = VirtAddr::from(target_start + size);

    let mut map_area = UserMapArea::new(
        start_va,
        end_va,
        MapType::Framed,
        map_perm,
        UserMapAreaType::Shm,
        true, // lazy_flag = true
    );
    map_area.shmid = Some(shmid);

    // Pre-populate data_frames with the shared physical pages
    for i in 0..num_pages {
        let vpn = VirtPageNum::from((target_start + i * PAGE_SIZE) / PAGE_SIZE);
        map_area.data_frames.insert(vpn, seg.pages[i].clone());
    }

    inner.vm_set.areas.push(map_area);

    // Manually map the shared physical pages into the page table
    let page_table = &mut inner.vm_set.page_table;
    let mapping_flags = if readonly {
        MappingFlags::U | MappingFlags::R
    } else {
        MappingFlags::U | MappingFlags::R | MappingFlags::W
    };
    for i in 0..num_pages {
        let vpn = VirtPageNum::from((target_start + i * PAGE_SIZE) / PAGE_SIZE);
        let ppn = seg.pages[i].ppn;
        page_table.map_page(vpn, ppn, mapping_flags, MappingSize::Page4KB);
    }

    // Update the segment's attach count within the same critical section
    if let Some(seg) = state.segments.get_mut(&shmid) {
        seg.shm_nattch += 1;
        seg.shm_lpid = current_pid();
    }

    drop(state);
    drop(inner);
    drop(process);

    TLB::flush_all();
    Ok(target_start)
}

/// System call: shmdt - detach shared memory segment
///
/// # Arguments
/// * `shmaddr` - Virtual address of the attached segment
///
/// # Returns
/// * 0 on success
pub fn sys_shmdt(shmaddr: *const u8) -> SyscallResult {
    let addr = shmaddr as usize;
    if (addr & (PAGE_SIZE - 1)) != 0 {
        return Err(SysError::EINVAL);
    }

    let process = current_process();
    let mut inner = process.inner_exclusive_access();

    // Look through all areas to find a shared memory area starting at this address
    let mut area_idx = None;
    let mut found_shmid = None;
    for (i, area) in inner.vm_set.areas.iter().enumerate() {
        if area.start_va().0 == addr
            && area.areatype() == UserMapAreaType::Shm
        {
            area_idx = Some(i);
            found_shmid = area.shmid;
            break;
        }
    }

    let idx = match area_idx {
        Some(i) => i,
        None => return Err(SysError::EINVAL),
    };

    let area = &inner.vm_set.areas[idx];
    let start_vpn = area.start_vpn();
    let end_vpn = area.end_vpn();

    // Unmap the pages
    let page_table = &mut inner.vm_set.page_table;
    for vpn in VPNRange::new(start_vpn, end_vpn) {
        page_table.unmap_page(vpn);
    }

    // Remove the area
    inner.vm_set.areas.remove(idx);

    drop(inner);
    drop(process);

    // Update the segment's attach count precisely
    if let Some(shmid) = found_shmid {
        let mut state = SHM_STATE.lock();
        if let Some(seg) = state.segments.get_mut(&shmid) {
            if seg.shm_nattch > 0 {
                seg.shm_nattch -= 1;
                seg.shm_lpid = current_pid();
            }
            if seg.destroyed && seg.shm_nattch == 0 {
                state.segments.remove(&shmid);
            }
        }
    }

    TLB::flush_all();
    Ok(0)
}

/// System call: shmctl - shared memory control
///
/// # Arguments
/// * `shmid` - Shared memory identifier
/// * `cmd` - Control command (IPC_RMID, IPC_SET, IPC_STAT, etc.)
/// * `buf` - Buffer for data
///
/// # Returns
/// * 0 on success (or info for IPC_INFO/SHM_INFO)
pub fn sys_shmctl(shmid: usize, cmd: i32, buf: *mut u8) -> SyscallResult {
    match cmd {
        IPC_INFO => {
            // Return shminfo structure
            if buf.is_null() {
                return Err(SysError::EFAULT);
            }
            let info = ShmInfo {
                shmmax: SHMMAX,
                shmmin: SHMMIN,
                shmmni: SHMMNI,
                shmseg: 128,
                shmall: 0,
                __unused: [0; 4],
            };
            let state = SHM_STATE.lock();
            let slice = unsafe {
                core::slice::from_raw_parts_mut(buf, core::mem::size_of::<ShmInfo>())
            };
            let info_bytes = unsafe {
                core::slice::from_raw_parts(
                    &info as *const ShmInfo as *const u8,
                    core::mem::size_of::<ShmInfo>(),
                )
            };
            slice.copy_from_slice(info_bytes);
            Ok(state.segments.len())
        }
        SHM_INFO => {
            if buf.is_null() {
                return Err(SysError::EFAULT);
            }
            let state = SHM_STATE.lock();
            let used_ids = state.segments.len();
            let info = ShmInfo {
                shmmax: SHMMAX,
                shmmin: SHMMIN,
                shmmni: SHMMNI,
                shmseg: 128,
                shmall: used_ids,
                __unused: [0; 4],
            };
            let slice = unsafe {
                core::slice::from_raw_parts_mut(buf, core::mem::size_of::<ShmInfo>())
            };
            let info_bytes = unsafe {
                core::slice::from_raw_parts(
                    &info as *const ShmInfo as *const u8,
                    core::mem::size_of::<ShmInfo>(),
                )
            };
            slice.copy_from_slice(info_bytes);
            Ok(used_ids)
        }
        IPC_STAT => {
            if buf.is_null() {
                return Err(SysError::EFAULT);
            }
            let state = SHM_STATE.lock();
            let seg = match state.segments.get(&shmid) {
                Some(s) => s,
                None => return Err(SysError::EINVAL),
            };
            let ds = ShmIdDs {
                shm_perm: IpcPerm {
                    key: seg.key,
                    uid: 0,
                    gid: 0,
                    cuid: 0,
                    cgid: 0,
                    mode: seg.mode,
                    __pad1: 0,
                    seq: 0,
                    __pad2: 0,
                    __unused1: 0,
                    __unused2: 0,
                },
                shm_segsz: seg.size,
                shm_atime: 0,
                shm_dtime: 0,
                shm_ctime: 0,
                shm_cpid: seg.shm_cpid as i32,
                shm_lpid: seg.shm_lpid as i32,
                shm_nattch: seg.shm_nattch,
                __unused4: 0,
                __unused5: 0,
            };
            let slice = unsafe {
                core::slice::from_raw_parts_mut(buf, core::mem::size_of::<ShmIdDs>())
            };
            let ds_bytes = unsafe {
                core::slice::from_raw_parts(
                    &ds as *const ShmIdDs as *const u8,
                    core::mem::size_of::<ShmIdDs>(),
                )
            };
            slice.copy_from_slice(ds_bytes);
            Ok(0)
        }
        SHM_STAT | SHM_STAT_ANY => {
            if buf.is_null() {
                return Err(SysError::EFAULT);
            }
            let state = SHM_STATE.lock();
            let seg = match state.segments.get(&shmid) {
                Some(s) => s,
                None => return Err(SysError::EINVAL),
            };
            let ds = ShmIdDs {
                shm_perm: IpcPerm {
                    key: seg.key,
                    uid: 0,
                    gid: 0,
                    cuid: 0,
                    cgid: 0,
                    mode: seg.mode,
                    __pad1: 0,
                    seq: 0,
                    __pad2: 0,
                    __unused1: 0,
                    __unused2: 0,
                },
                shm_segsz: seg.size,
                shm_atime: 0,
                shm_dtime: 0,
                shm_ctime: 0,
                shm_cpid: seg.shm_cpid as i32,
                shm_lpid: seg.shm_lpid as i32,
                shm_nattch: seg.shm_nattch,
                __unused4: 0,
                __unused5: 0,
            };
            let slice = unsafe {
                core::slice::from_raw_parts_mut(buf, core::mem::size_of::<ShmIdDs>())
            };
            let ds_bytes = unsafe {
                core::slice::from_raw_parts(
                    &ds as *const ShmIdDs as *const u8,
                    core::mem::size_of::<ShmIdDs>(),
                )
            };
            slice.copy_from_slice(ds_bytes);
            Ok(shmid)
        }
        IPC_SET => {
            if buf.is_null() {
                return Err(SysError::EFAULT);
            }
            let ds = unsafe { &*(buf as *const ShmIdDs) };
            let mut state = SHM_STATE.lock();
            if let Some(seg) = state.segments.get_mut(&shmid) {
                seg.mode = ds.shm_perm.mode & 0o777;
            } else {
                return Err(SysError::EINVAL);
            }
            Ok(0)
        }
        IPC_RMID => {
            let mut state = SHM_STATE.lock();
            // First get the key without holding a mutable borrow on the segment
            let key = match state.segments.get(&shmid) {
                Some(s) => s.key,
                None => return Err(SysError::EINVAL),
            };
            if key != 0 {
                state.key_to_id.remove(&key);
            }
            let seg = match state.segments.get_mut(&shmid) {
                Some(s) => s,
                None => return Err(SysError::EINVAL),
            };
            seg.destroyed = true;
            // If no process is attached, destroy immediately
            let nattch = seg.shm_nattch;
            if nattch == 0 {
                state.segments.remove(&shmid);
            }
            Ok(0)
        }
        _ => {
            Err(SysError::EINVAL)
        }
    }
}

/// Release shm attaches for a set of memory areas.
/// Called during execve / exit when the address space is torn down.
pub fn release_shm_attaches(areas: &[UserMapArea]) {
    let mut state = SHM_STATE.lock();
    for area in areas.iter() {
        if area.areatype() == UserMapAreaType::Shm {
            if let Some(shmid) = area.shmid {
                if let Some(seg) = state.segments.get_mut(&shmid) {
                    if seg.shm_nattch > 0 {
                        seg.shm_nattch -= 1;
                    }
                    if seg.destroyed && seg.shm_nattch == 0 {
                        state.segments.remove(&shmid);
                    }
                }
            }
        }
    }
}

/// Inherit shm attaches for a child process (fork / clone).
/// Called after the child's address space has been set up.
pub fn fork_inherit_shm_attach(areas: &[UserMapArea], pid: usize) {
    let mut state = SHM_STATE.lock();
    for area in areas.iter() {
        if area.areatype() == UserMapAreaType::Shm {
            if let Some(shmid) = area.shmid {
                if let Some(seg) = state.segments.get_mut(&shmid) {
                    seg.shm_nattch += 1;
                    seg.shm_lpid = pid;
                }
            }
        }
    }
}
