// use super::page_table;
// use super::page_table::PTEFlags;
use super::heap::*;
use super::vm_area::{KernelMapArea, MapType, UserMapArea, cow_mapping_flags};
use super::{
    COW, UserMapAreaType,
    exception::{self, *},
    vm_area,
};
use super::{LazyAlloc, frame_alloc};
use crate::config;
use crate::config::MMAP_BASE;
use crate::config::MMIO;
use alloc::collections::BTreeMap;
use polyhal::println;
use polyhal_trap::trapframe::TrapFrameArgs;
// use crate::config::{
//     KERNEL_STACK_SIZE, MEMORY_END, MMIO, TRAP_CONTEXT, USER_MEMORY_SPACE, USER_STACK_BASE,
//     USER_STACK_SIZE,
// };
use crate::fs::File;
use crate::fs::vfs::OpenFlags;
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::file::open_file;
use crate::mm::MmapType;
use crate::mm::vm_area::KernelAreaType;
use crate::mm::{MapArea, vm_set};
use crate::sync::{BlockingMutexGuard, SleepLock, SpinNoIrq, SpinNoIrqLock};
use crate::task::task::TaskControlBlock;
use crate::task::{current_task, current_trap_cx, current_user_token};
use crate::trap::{self};
use alloc::collections::btree_map::Range;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use bitflags::Flags;
use core::arch::{self, asm};
use core::cell::RefCell;
use core::error;
use core::iter::Map;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use core::task;
use lazy_static::*;
use log::*;
use polyhal::common::FrameTracker;
use polyhal::consts::VIRT_ADDR_START;
use polyhal::print;
use polyhal::{consts::*, hart_id};
// use riscv::addr::{Page, page};
// use riscv::paging::PTE;
use crate::Signal;
use crate::current_process;
pub use polyhal::pagetable::*;
pub use polyhal::utils::addr::*;

#[cfg(target_arch = "riscv64")]
use riscv::register::satp;

#[cfg(target_arch = "riscv64")]
const USER_RT_SIGRETURN_TRAMPOLINE_CODE: [u8; 8] = [0x93, 0x08, 0xb0, 0x08, 0x73, 0x00, 0x00, 0x00];
#[cfg(target_arch = "loongarch64")]
const USER_RT_SIGRETURN_TRAMPOLINE_CODE: [u8; 8] = [0x0b, 0x2c, 0x82, 0x03, 0x00, 0x00, 0x2b, 0x00];

// use crate::arch::riscv::sfence_vma_va;
// use crate::arch::TLB;
use crate::task::exit_current_and_run_next;
// use crate::trap::self;
use lazy_static::*;
// use sbi_rt::Sta;

unsafe extern "C" {
    safe fn stext();
    safe fn etext();
    safe fn srodata();
    safe fn erodata();
    safe fn sdata();
    safe fn edata();
    safe fn _sbss();
    safe fn _ebss();
    #[allow(unused)]
    safe fn ekernel();
}
///
#[derive(Debug)]
pub enum ExceptionType {
    ///
    Cow,
    ///
    None,
    ///
    Read,
    ///
    Execute,
    ///
    Write,
    ///
    Lazy,
}

lazy_static! {
    /// a memory set instance through lazy_static! managing kernel space
    pub static ref KERNEL_VMSET: Arc<SpinNoIrqLock<KernelVMSet>> =
        Arc::new(SpinNoIrqLock::new(KernelVMSet::new()));
}

// The idle scheduler must never keep executing through the page-table root of
// the task that most recently ran on that CPU. Once the task is reaped, that
// root frame may be reused even though the CPU is still fetching kernel code
// through it. Cache the permanent kernel root so switching back on the idle
// stack never needs to acquire KERNEL_VMSET or allocate a temporary object.
static KERNEL_PAGE_TABLE_TOKEN: AtomicUsize = AtomicUsize::new(0);
static ACTIVE_PAGE_TABLE_TOKENS: [AtomicUsize; crate::config::MAX_CPU_NUM] =
    [const { AtomicUsize::new(0) }; crate::config::MAX_CPU_NUM];

/// Publish the page-table root currently installed on this CPU.
///
/// Unlike the user-TLB active mask in polyhal, this remains set while the CPU
/// is executing in the kernel. Kernel code can still perform software page
/// table walks, and the architecture continues fetching kernel mappings
/// through that root until the scheduler installs another one.
pub(crate) fn record_active_page_table_token(token: usize) {
    let cpu = hart_id();
    if cpu < ACTIVE_PAGE_TABLE_TOKENS.len() {
        ACTIVE_PAGE_TABLE_TOKENS[cpu].store(token, Ordering::Release);
    }
}

/// Return CPUs that still have `token` installed as their hardware root.
pub(crate) fn active_page_table_mask(token: usize) -> usize {
    if token == 0 {
        return 0;
    }
    let mut mask = 0usize;
    for (cpu, active) in ACTIVE_PAGE_TABLE_TOKENS.iter().enumerate() {
        if active.load(Ordering::Acquire) == token {
            mask |= 1usize << cpu;
        }
    }
    mask
}

pub(crate) fn activate_kernel_page_table() -> bool {
    let token = KERNEL_PAGE_TABLE_TOKEN.load(Ordering::Acquire);
    assert_ne!(token, 0, "kernel page-table token is not initialized");
    let unchanged = PageTable::from_token(token).change();
    record_active_page_table_token(token);
    unchanged
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub(crate) struct ForkCowStats {
    pub active: bool,
    pub parent_pid: usize,
    pub owner_cpu: usize,
    pub phase: usize,
    pub work_index: usize,
    pub work_total: usize,
    pub resident_pages_done: usize,
    pub area_subphase: usize,
    pub area_page_index: usize,
    pub area_page_total: usize,
}

static FORK_COW_ACTIVE: AtomicBool = AtomicBool::new(false);
static FORK_COW_PARENT_PID: AtomicUsize = AtomicUsize::new(0);
static FORK_COW_OWNER_CPU: AtomicUsize = AtomicUsize::new(usize::MAX);
static FORK_COW_PHASE: AtomicUsize = AtomicUsize::new(0);
static FORK_COW_WORK_INDEX: AtomicUsize = AtomicUsize::new(0);
static FORK_COW_WORK_TOTAL: AtomicUsize = AtomicUsize::new(0);
static FORK_COW_RESIDENT_PAGES_DONE: AtomicUsize = AtomicUsize::new(0);
static FORK_COW_AREA_SUBPHASE: AtomicUsize = AtomicUsize::new(0);
static FORK_COW_AREA_PAGE_INDEX: AtomicUsize = AtomicUsize::new(0);
static FORK_COW_AREA_PAGE_TOTAL: AtomicUsize = AtomicUsize::new(0);

struct ForkCowTraceGuard;

impl ForkCowTraceGuard {
    fn begin(parent_pid: usize, area_count: usize) -> Self {
        FORK_COW_PARENT_PID.store(parent_pid, Ordering::Relaxed);
        FORK_COW_OWNER_CPU.store(hart_id(), Ordering::Relaxed);
        FORK_COW_WORK_INDEX.store(0, Ordering::Relaxed);
        FORK_COW_WORK_TOTAL.store(area_count, Ordering::Relaxed);
        FORK_COW_RESIDENT_PAGES_DONE.store(0, Ordering::Relaxed);
        FORK_COW_AREA_SUBPHASE.store(0, Ordering::Relaxed);
        FORK_COW_AREA_PAGE_INDEX.store(0, Ordering::Relaxed);
        FORK_COW_AREA_PAGE_TOTAL.store(0, Ordering::Relaxed);
        FORK_COW_PHASE.store(1, Ordering::Release);
        FORK_COW_ACTIVE.store(true, Ordering::Release);
        Self
    }

    fn progress(&self, phase: usize, index: usize, total: usize) {
        FORK_COW_WORK_INDEX.store(index, Ordering::Relaxed);
        FORK_COW_WORK_TOTAL.store(total, Ordering::Relaxed);
        FORK_COW_PHASE.store(phase, Ordering::Release);
    }

    fn add_resident_pages(&self, pages: usize) {
        FORK_COW_RESIDENT_PAGES_DONE.fetch_add(pages, Ordering::Relaxed);
    }

    fn area_progress(&self, subphase: usize, index: usize, total: usize) {
        FORK_COW_AREA_PAGE_INDEX.store(index, Ordering::Relaxed);
        FORK_COW_AREA_PAGE_TOTAL.store(total, Ordering::Relaxed);
        FORK_COW_AREA_SUBPHASE.store(subphase, Ordering::Release);
    }
}

impl Drop for ForkCowTraceGuard {
    fn drop(&mut self) {
        FORK_COW_ACTIVE.store(false, Ordering::Release);
    }
}

pub(crate) fn fork_cow_stats() -> ForkCowStats {
    ForkCowStats {
        active: FORK_COW_ACTIVE.load(Ordering::Acquire),
        parent_pid: FORK_COW_PARENT_PID.load(Ordering::Relaxed),
        owner_cpu: FORK_COW_OWNER_CPU.load(Ordering::Relaxed),
        phase: FORK_COW_PHASE.load(Ordering::Acquire),
        work_index: FORK_COW_WORK_INDEX.load(Ordering::Relaxed),
        work_total: FORK_COW_WORK_TOTAL.load(Ordering::Relaxed),
        resident_pages_done: FORK_COW_RESIDENT_PAGES_DONE.load(Ordering::Relaxed),
        area_subphase: FORK_COW_AREA_SUBPHASE.load(Ordering::Acquire),
        area_page_index: FORK_COW_AREA_PAGE_INDEX.load(Ordering::Relaxed),
        area_page_total: FORK_COW_AREA_PAGE_TOTAL.load(Ordering::Relaxed),
    }
}

#[cfg(target_arch = "riscv64")]
fn for_each_physical_memory_region(min_start: usize, mut f: impl FnMut(usize, usize)) {
    let mut emit = |start: usize, end: usize| {
        let start = start.max(min_start);
        if start < end {
            f(start, end);
        }
    };

    for &(start, size) in polyhal::mem::get_mem_areas() {
        emit(start, start + size);
    }
}

const INTERP_SCRATCH_SIZE: usize = 4 * 1024 * 1024;
const ELF_HEADER_BUFFER_MAX_SIZE: usize = 1024 * 1024;
const ELF_INTERP_PATH_MAX_SIZE: usize = 4096;

static INTERP_SCRATCH: SleepLock<[u8; INTERP_SCRATCH_SIZE]> =
    SleepLock::new([0; INTERP_SCRATCH_SIZE]);

struct InterpImageGuard {
    buffer: BlockingMutexGuard<'static, [u8; INTERP_SCRATCH_SIZE], SpinNoIrq>,
    len: usize,
}

impl InterpImageGuard {
    fn as_slice(&self) -> &[u8] {
        &self.buffer[..self.len]
    }
}

fn read_interp_image(file: &Arc<dyn File>, path: &str) -> Option<InterpImageGuard> {
    let size = file.get_inode().map(|inode| inode.get_size()).unwrap_or(0);
    if size > INTERP_SCRATCH_SIZE {
        warn!(
            "[from_elf] interpreter too large for scratch buffer: path={} size={} limit={}",
            path, size, INTERP_SCRATCH_SIZE
        );
        return None;
    }

    let mut buffer = INTERP_SCRATCH.lock();
    let mut offset = 0usize;
    while offset < size {
        let read_size = match file.read_at_direct(offset, &mut buffer[offset..size]) {
            Ok(n) => n,
            Err(err) => {
                warn!(
                    "[from_elf] Failed to read interpreter: path={} offset={} err={:?}",
                    path, offset, err
                );
                return None;
            }
        };
        if read_size == 0 {
            break;
        }
        offset += read_size;
    }
    if offset != size {
        warn!(
            "[from_elf] short interpreter read: path={} expected={} actual={}",
            path, size, offset
        );
        return None;
    }

    Some(InterpImageGuard {
        buffer,
        len: offset,
    })
}

fn elf_segment_data<'a>(
    image_name: &str,
    image: &'a [u8],
    offset: u64,
    file_size: u64,
) -> Option<&'a [u8]> {
    if file_size == 0 {
        return Some(&[]);
    }
    let Some(end) = offset.checked_add(file_size) else {
        warn!(
            "[from_elf] invalid {} segment: offset={:#x} filesz={:#x} overflows",
            image_name, offset, file_size
        );
        return None;
    };
    if end > image.len() as u64 {
        warn!(
            "[from_elf] truncated {} segment: offset={:#x} filesz={:#x} end={} image_len={}",
            image_name,
            offset,
            file_size,
            end,
            image.len()
        );
        return None;
    }
    Some(&image[offset as usize..end as usize])
}

fn elf_program_headers_in_bounds(image_name: &str, image: &[u8], elf: &xmas_elf::ElfFile) -> bool {
    let ph_offset = elf.header.pt2.ph_offset() as usize;
    let ph_entry_size = elf.header.pt2.ph_entry_size() as usize;
    let ph_count = elf.header.pt2.ph_count() as usize;
    let Some(ph_table_size) = ph_entry_size.checked_mul(ph_count) else {
        warn!(
            "[from_elf] invalid {} program header table size",
            image_name
        );
        return false;
    };
    let Some(ph_end) = ph_offset.checked_add(ph_table_size) else {
        warn!("[from_elf] invalid {} program header table end", image_name);
        return false;
    };
    if ph_end > image.len() {
        warn!(
            "[from_elf] truncated {} program header table: end={} image_len={}",
            image_name,
            ph_end,
            image.len()
        );
        return false;
    }
    true
}

fn read_exact_file_at(
    file: &Arc<dyn File>,
    path: &str,
    offset: usize,
    buf: &mut [u8],
    label: &str,
) -> Option<()> {
    let mut done = 0usize;
    while done < buf.len() {
        let read = match file.read_at_direct(offset + done, &mut buf[done..]) {
            Ok(n) => n,
            Err(err) => {
                warn!(
                    "[from_elf_file] failed to read {}: path={} offset={} err={:?}",
                    label,
                    path,
                    offset + done,
                    err
                );
                return None;
            }
        };
        if read == 0 {
            warn!(
                "[from_elf_file] short {} read: path={} expected={} actual={}",
                label,
                path,
                buf.len(),
                done
            );
            return None;
        }
        done += read;
    }
    Some(())
}

fn read_elf_header_image(file: &Arc<dyn File>, path: &str, file_size: usize) -> Option<Vec<u8>> {
    let prefix_len = file_size.min(PAGE_SIZE);
    if prefix_len == 0 {
        return None;
    }
    let mut prefix = vec![0u8; prefix_len];
    read_exact_file_at(file, path, 0, &mut prefix, "ELF header")?;
    let prefix_elf = match xmas_elf::ElfFile::new(&prefix) {
        Ok(elf) => elf,
        Err(_) => {
            info!("[DEBUG execve] Not an ELF file! Returning ENOEXEC.");
            return None;
        }
    };

    let ph_offset = prefix_elf.header.pt2.ph_offset() as usize;
    let ph_entry_size = prefix_elf.header.pt2.ph_entry_size() as usize;
    let ph_count = prefix_elf.header.pt2.ph_count() as usize;
    let ph_table_size = ph_entry_size.checked_mul(ph_count)?;
    let ph_end = ph_offset.checked_add(ph_table_size)?;
    if ph_end > file_size {
        warn!(
            "[from_elf_file] truncated program header table: path={} end={} file_size={}",
            path, ph_end, file_size
        );
        return None;
    }
    if ph_end > ELF_HEADER_BUFFER_MAX_SIZE {
        warn!(
            "[from_elf_file] program header table too large: path={} end={} limit={}",
            path, ph_end, ELF_HEADER_BUFFER_MAX_SIZE
        );
        return None;
    }
    if ph_end <= prefix.len() {
        return Some(prefix);
    }

    let mut headers = vec![0u8; ph_end];
    read_exact_file_at(file, path, 0, &mut headers, "ELF program headers")?;
    Some(headers)
}

fn read_interp_path_from_file(
    file: &Arc<dyn File>,
    path: &str,
    file_size: usize,
    offset: u64,
    len: u64,
) -> Option<String> {
    if len == 0 || len > ELF_INTERP_PATH_MAX_SIZE as u64 {
        warn!(
            "[from_elf_file] invalid interpreter path length: path={} len={}",
            path, len
        );
        return None;
    }
    let end = offset.checked_add(len)?;
    if end > file_size as u64 {
        warn!(
            "[from_elf_file] truncated interpreter path: path={} end={} file_size={}",
            path, end, file_size
        );
        return None;
    }
    let mut data = vec![0u8; len as usize];
    read_exact_file_at(file, path, offset as usize, &mut data, "interpreter path")?;
    let nul = data
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(data.len());
    core::str::from_utf8(&data[..nul])
        .ok()
        .map(|path| path.to_string())
}
///
#[derive(Debug, Clone, Copy)]
pub enum AccessType {
    ///
    Read,
    ///
    Write,
    ///
    Execute,
    ///
    None,
}

#[allow(missing_docs)]
pub trait VMSpace {
    fn page_table(&self) -> &PageTable;
    fn page_table_mut(&mut self) -> &mut PageTable;
    fn new_bare() -> Self;
    fn token(&self) -> usize;
    fn remove_area_with_start_vpn(&mut self, start_vpn: VirtPageNum);
    fn activate(&self);
    fn translate(&self, vpn: VirtPageNum) -> Option<PTE> {
        self.page_table().translate(vpn)
    }
}
///
pub struct VMSet<A: MapArea> {
    ///
    pub page_table: PageTable,
    areas: Vec<A>,
}
///
impl<A: MapArea> VMSet<A> {
    ///
    pub fn recycle_data_pages(&mut self) {
        self.areas.clear();
    }
    ///
    pub fn init() -> Self {
        Self {
            page_table: PageTable::new(),
            areas: Vec::new(),
        }
    }
}

impl VMSpace for UserVMSet {
    fn page_table(&self) -> &PageTable {
        &self.page_table
    }

    fn page_table_mut(&mut self) -> &mut PageTable {
        &mut self.page_table
    }

    fn new_bare() -> Self {
        Self {
            page_table: PageTable::new(),
            areas: Vec::new(),
            membarrier_registrations: 0,
            process_owners: 1,
        }
    }
    fn token(&self) -> usize {
        self.page_table.token()
    }

    fn remove_area_with_start_vpn(&mut self, start_vpn: VirtPageNum) {
        drop(self.take_area_with_start_vpn(start_vpn));
    }

    fn activate(&self) {
        // let satp = self.page_table.token();
        // unsafe {
        //     satp::write(satp);
        //     asm!("sfence.vma");
        // }
        // if hart_id() !=0 {
        // warn!("activating user page table on hart {}, pa={:#x}", hart_id(), self.page_table.root_ppn.0<<12);
        // }
        self.page_table.change();
        record_active_page_table_token(self.token());
    }
}
#[allow(missing_docs)]
pub struct UserVMSet {
    pub page_table: PageTable,
    pub areas: Vec<UserMapArea>,
    /// Expedited membarrier registration is mm-scoped and therefore shared by
    /// non-thread CLONE_VM peers as well as ordinary pthreads.
    pub membarrier_registrations: u32,
    /// Number of process control blocks that own this mm through CLONE_VM.
    /// Transient Arc clones used by page faults, KSM, or diagnostics must not
    /// affect address-space teardown decisions.
    pub process_owners: usize,
}

#[derive(Debug)]
///
pub enum PageFaultError {
    ///
    InvalidAddress, // 发送 SIGSEGV
    ///
    BeyondFileSize, // 发送 SIGBUS
    ///
    OutOfMemory, // 发送 SIGSEGV/终止当前进程
    ///
    InvalidMapping, // 发送 SIGSEGV
    ///
    Normal, //正常
}

fn log_user_page_fault_oom(area: &UserMapArea, va: VirtAddr, access: AccessType, reason: &str) {
    log::error!(
        "[OOM] user_page_fault alloc failed: reason={} type={:?} va={:#x} vpn={:#x} range=[{:#x}, {:#x}) perm={:#x} lazy={} cow={} resident_pages={} access={:?}",
        reason,
        area.areatype(),
        va.0,
        va.floor().0,
        area.start_va().0,
        area.end_va().0,
        area.perm().bits(),
        area.get_lazy_flag(),
        area.cow_flag(),
        area.data_frames.len(),
        access
    );
    crate::task::print_oom_snapshot();
}

impl SetPageFaultException for UserVMSet {
    fn handle_unalloc_page_fault(
        &mut self,
        va: VirtAddr,
        access: AccessType,
    ) -> Option<PageFaultError> {
        // warn!("unalloc handler");
        // info!("[DEBUG] handle_unalloc_page_fault: va={:#x}", va.0);
        let _area = self.find_area(va)?;
        // info!(
        //     "[DEBUG] found area: start={:#x}, end={:#x}, type={:?}",
        //     area.start_va().0,
        //     area.end_va().0,
        //     area.areatype()
        // );
        let fault_vpn = va.floor();

        // 已映射则无需重复处理，避免二次 map 触发 panic。
        // 兜底：如果已有 PTE 是 RISC-V 保留组合 W=1,R=0，修正它并刷 TLB，否则死循环。
        // 另外，如果 PTE 权限与 area 当前权限不一致（例如 mprotect 修改了权限但 PTE 未更新），
        // 也需要更新 PTE 权限，否则会陷入缺页死循环。
        // 先检查 PTE 是否存在，如果存在则尝试修正权限
        let area_has_frame = self
            .areas
            .iter()
            .find(|area| area.range_va().contains(&va))
            .map(|area| area.data_frames.contains_key(&fault_vpn))
            .unwrap_or(false);
        let pte_exists = self.page_table.find_pte(fault_vpn).map(|pte| {
            let flags = pte.flags();
            let ppn = pte.ppn();
            (flags, ppn)
        });
        if let Some((flags, ppn)) = pte_exists {
            if !flags.contains(PTEFlags::V) {
                // PTE 无效，继续处理
            } else if !area_has_frame {
                // LoongArch user page tables inherit kernel high-half mappings.
                // Low user VAs can alias those entries in the hardware page-table
                // format, so lazy anonymous/file VMAs must replace stale inherited
                // PTEs with their own frames instead of "fixing" the old PTE flags.
                warn!(
                    "drop stale inherited PTE for lazy user page: va={:#x}, flags={:?}, ppn={:#x}",
                    va.0, flags, ppn.0
                );
                self.page_table.unmap_page(fault_vpn);
                TLB::flush_vaddr(va);
            } else if flags.writable() && !flags.readable() {
                // RISC-V 保留组合 W=1,R=0，修正它
                if let Some(pte) = self.page_table.find_pte(fault_vpn) {
                    pte.set_flag(flags | PTEFlags::from(MappingFlags::from(MapPermission::R)));
                }
                TLB::flush_vaddr(va);
                return Some(PageFaultError::Normal);
            } else {
                // 检查 PTE 权限是否与 area 当前权限一致
                if let Some(area) = self.find_area(va) {
                    let expected_base =
                        PTEFlags::from(MappingFlags::from(*area.perm())) | PTEFlags::V;
                    let perm_mask = PTEFlags::from(MappingFlags::from(
                        MapPermission::R | MapPermission::W | MapPermission::X | MapPermission::U,
                    )) | PTEFlags::V;
                    if (flags & perm_mask) != (expected_base & perm_mask) {
                        info!(
                            "fixing PTE permissions from {:?} to {:?}",
                            flags, expected_base
                        );
                        if let Some(pte) = self.page_table.find_pte(fault_vpn) {
                            let new_flags = (flags & !perm_mask) | expected_base;
                            *pte = PTE::new(ppn, new_flags);
                        }
                        TLB::flush_vaddr(va);
                    }
                }
                return Some(PageFaultError::Normal);
            }
        }

        let (target_ppn, mut mappingflags) = {
            let area = self.find_area(va)?;
            let mut frame_needs_cow = false;
            let mut writable_private_page = false;
            let frame = if let Some(existing) = area.data_frames.get(&fault_vpn) {
                // Existing pages in a COW VMA may still be shared with the
                // fork peer. Keep them read-only until handle_cow_page_fault
                // proves or creates private ownership.
                frame_needs_cow = area.cow_flag;
                existing.clone()
            } else {
                let new_frame = match area.areatype() {
                    UserMapAreaType::Heap
                    | UserMapAreaType::Stack
                    | UserMapAreaType::Elf
                    | UserMapAreaType::TrapContext
                    | UserMapAreaType::RtSigreturnTrampoline => {
                        let Some(frame) = frame_alloc() else {
                            log_user_page_fault_oom(area, va, access, "anonymous");
                            return Some(PageFaultError::OutOfMemory);
                        };
                        Arc::new(frame)
                    }
                    UserMapAreaType::Mmap | UserMapAreaType::Shm => {
                        if let Some(file) = &area.map_file {
                            let offset_in_area = (fault_vpn.0 - area.start_vpn().0) * PAGE_SIZE;
                            let file_offset = area.file_offset + offset_in_area;
                            let page_id = file_offset / PAGE_SIZE;

                            // 检查文件大小，如果访问超出文件末尾，返回零页
                            let _file_size =
                                file.get_inode().map(|inode| inode.get_size()).unwrap_or(0);

                            // 检查文件大小，如果访问超出文件末尾，返回零页
                            let file_size =
                                file.get_inode().map(|inode| inode.get_size()).unwrap_or(0);
                            if file_offset >= file_size {
                                // 发送 SIGBUS 信号
                                info!(
                                    "[DEBUG] handle_unalloc_page_fault: va={:#x} beyond file size, sending SIGBUS",
                                    va.0
                                );
                                // let process = current_process();
                                // if let Some(signal) = Signal::from_i32(10) { // SIGBUS = 10
                                //     crate::syscall::signal::deliver_signal(&process, signal);
                                // }
                                return Some(PageFaultError::BeyondFileSize);
                            } else {
                                let Some(file_frame) = file.get_cache_frame(page_id) else {
                                    return Some(PageFaultError::InvalidMapping);
                                };
                                if area.flags == MmapType::MapPrivate
                                    && matches!(access, AccessType::Write)
                                {
                                    let Some(frame) = frame_alloc() else {
                                        log_user_page_fault_oom(area, va, access, "private_file");
                                        return Some(PageFaultError::OutOfMemory);
                                    };
                                    let private_frame = Arc::new(frame);
                                    // 复制文件内容到私有帧（只复制文件实际存在的部分）
                                    let copy_size = (file_size - file_offset).min(PAGE_SIZE);
                                    private_frame.ppn.get_bytes_array()[..copy_size]
                                        .copy_from_slice(
                                            &file_frame.ppn.get_bytes_array()[..copy_size],
                                        );
                                    // 超出文件部分清零
                                    if copy_size < PAGE_SIZE {
                                        private_frame.ppn.get_bytes_array()[copy_size..].fill(0);
                                    }
                                    writable_private_page = true;
                                    crate::task::perf_stats::record_file_fault_private_copy();
                                    private_frame
                                } else {
                                    frame_needs_cow =
                                        area.flags == MmapType::MapPrivate && area.cow_flag;
                                    crate::task::perf_stats::record_file_fault_shared_page();
                                    file_frame
                                }
                            }
                        } else {
                            if area.shared_anonymous.is_some() {
                                let Some(frame) = area.allocate_shared_anonymous_frame(fault_vpn)
                                else {
                                    log_user_page_fault_oom(
                                        area,
                                        va,
                                        access,
                                        "shared_anonymous_mmap",
                                    );
                                    return Some(PageFaultError::OutOfMemory);
                                };
                                frame
                            } else {
                                let Some(frame) = frame_alloc() else {
                                    log_user_page_fault_oom(area, va, access, "anonymous_mmap");
                                    return Some(PageFaultError::OutOfMemory);
                                };
                                Arc::new(frame)
                            }
                        }
                    } // _ => return None,
                };
                area.data_frames.insert(fault_vpn, new_frame.clone());
                if area.data_frames.len() >= area.vpn_range().count() {
                    area.clear_lazy_flag();
                }
                new_frame
            };
            // A page absent at fork and allocated later from anonymous
            // backing is private to this address space. The VMA-level COW bit
            // must not force that fresh frame read-only. File-backed private
            // pages remain COW only while they still reference page cache.
            let flags = if frame_needs_cow && !writable_private_page {
                cow_mapping_flags(*area.perm())
            } else {
                area.initial_mapping_flags()
            };
            (frame.ppn, flags)
        };
        if mappingflags.contains(MappingFlags::X) && !mappingflags.contains(MappingFlags::R) {
            mappingflags |= MappingFlags::R;
        }
        self.page_table
            .map_page(fault_vpn, target_ppn, mappingflags, MappingSize::Page4KB);
        if mappingflags.contains(MappingFlags::X) {
            polyhal::multicore::synchronize_instruction_cache(self.token());
        }
        TLB::flush_vaddr(va);
        // info!("handle_unalloc_page_fault mapped vpn {:#x} ok", fault_vpn.0);
        Some(PageFaultError::Normal)
    }

    fn handle_cow_page_fault(&mut self, va: VirtAddr) -> Option<PageFaultError> {
        let vpn = va.floor();
        let _pte = self.page_table.translate(vpn)?;

        // 如果 PTE 已经是可写的，说明这个页已经处理过 COW，直接返回
        if let Some(pte) = self.page_table.translate(vpn) {
            if pte.writable() {
                return Some(PageFaultError::Normal);
            }
        }

        let area = self.find_area(va)?;
        let _area_perm = *area.perm();

        let ppn = {
            let old_frame = area.data_frames.get(&vpn)?;
            let ppn = old_frame.ppn;
            if Arc::strong_count(old_frame) == 1 {
                // 引用计数为 1，不需要复制，直接恢复写权限
                area.perm_mut().insert(MapPermission::W);
                ppn
            } else {
                let Some(new_frame_tracker) = frame_alloc() else {
                    log_user_page_fault_oom(area, va, AccessType::Write, "cow");
                    return Some(PageFaultError::OutOfMemory);
                };
                let new_frame = Arc::new(new_frame_tracker);
                let new_ppn = new_frame.ppn;
                new_ppn
                    .get_bytes_array()
                    .copy_from_slice(old_frame.ppn.get_bytes_array());
                area.data_frames.insert(vpn, new_frame);
                area.perm_mut().insert(MapPermission::W);
                new_ppn
            }
        };

        let flags = PTEFlags::from(MappingFlags::from(*area.perm())) | PTEFlags::V;
        let page_table = self.page_table_mut();
        if let Some(pte) = page_table.find_pte(vpn) {
            *pte = PTE::new(ppn, flags);
        }
        // Other threads of this process may be executing this address space on
        // different CPUs. They must stop using the old COW translation before
        // the replaced frame can become recyclable.
        polyhal::multicore::shootdown_tlb_all(self.token());
        Some(PageFaultError::Normal)
    }

    fn handle_store_page_fault_set(
        &mut self,
        va: VirtAddr,
        access: AccessType,
    ) -> Option<PageFaultError> {
        // println!(
        //     "enter page fault handler, va = {:#x}, access type = {:?}",
        //     va.0, access
        // );
        let exceptiontype: ExceptionType;

        if let Some(area) = self.find_area(va) {
            exceptiontype = area.access_check(access);
            trace!(
                "perm {:?}",
                PTEFlags::from(MappingFlags::from(*area.perm()))
            );
        } else {
            match access {
                AccessType::Write | AccessType::Read => {
                    if self.try_expand_stack(va).is_some() {
                        return Some(PageFaultError::Normal);
                    }
                }
                _ => {}
            }
            error!("no vma found for va: {:#x}", va.0);
            return None;
        }

        // println!(
        //     "enter page fault handler, va = {:#x},{:?}",
        //     va.0, exceptiontype
        // );
        match exceptiontype {
            ExceptionType::Cow => {
                // 如果 PTE 不存在（lazy 分配的页），按 unalloc 处理而不是 COW
                if self.page_table.translate(va.floor()).is_some() {
                    self.handle_cow_page_fault(va)
                } else {
                    self.handle_unalloc_page_fault(va, access)
                }
            }
            ExceptionType::Write => self.handle_unalloc_page_fault(va, access),
            ExceptionType::Read => self.handle_unalloc_page_fault(va, access),
            _ => {
                log::error!("permission denied");
                None
            }
        }
        // if let Some(pte) = pg.find_pte(vpn) {
        //     println!("PTE: {:?}", pte);
        //     println!("  Valid: {}", pte.is_valid());
        //     println!("  Read: {}", pte.readable());
        //     println!("  Write: {}", pte.writable());
        //     println!("  Execute: {}", pte.executable());
        // } else {
        //     println!("No PTE found!");
        // }
    }
}

impl UserVMSet {
    /// Unmap and detach one user area without dropping its owned frames.
    ///
    /// Process teardown callers often hold `ProcessControlBlockInner` while
    /// updating the VM layout. Returning the area lets them release that lock
    /// before the area's last `FrameTracker` references enter the global frame
    /// allocator, preserving the process-lock -> frame-lock ordering boundary.
    pub fn take_area_with_start_vpn(&mut self, start_vpn: VirtPageNum) -> Option<UserMapArea> {
        let idx = self.find_area_start_vpn_index(start_vpn)?;
        let area = self.areas.remove(idx);
        for vpn in area.vpn_range() {
            if area.data_frames.contains_key(&vpn) {
                self.page_table.unmap_page(vpn);
            }
        }
        Some(area)
    }

    ///
    pub fn recycle_data_pages(&mut self) -> Vec<UserMapArea> {
        let mut areas = Vec::new();
        core::mem::swap(&mut areas, &mut self.areas);
        areas
    }
    ///
    pub fn release_user_space(&mut self) -> (Vec<UserMapArea>, usize) {
        let areas = self.recycle_data_pages();
        let released_page_table_pages = self.page_table.frames.len().saturating_sub(1);
        let user_root_entries = PageTable::PTE_NUM_IN_PAGE / 2;
        let root_entries = self.page_table.root().get_pte_array();
        for pte in root_entries.iter_mut().take(user_root_entries) {
            *pte = PTE::empty();
        }
        // Page-table FrameTrackers must remain alive until every CPU can no
        // longer walk or cache any of their entries.
        polyhal::multicore::shootdown_tlb_all(self.token());
        if self.page_table.frames.len() > 1 {
            self.page_table.frames.truncate(1);
        }
        (areas, released_page_table_pages)
    }
    ///
    // pub fn init() -> Self {
    //     Self {
    //         page_table: PageTable::init(),
    //         areas: Vec::new(),
    //     }
    // }

    ///
    pub fn get_heap_area_mut(&mut self) -> &mut UserMapArea {
        let idx = self
            .areas
            .iter()
            .enumerate()
            .filter(|(_, area)| area.areatype() == UserMapAreaType::Heap)
            .max_by_key(|(_, area)| area.end_va())
            .map(|(idx, _)| idx)
            .unwrap();
        &mut self.areas[idx]
    }
    ///
    pub fn get_heap_area(&self) -> &UserMapArea {
        self.areas
            .iter()
            .filter(|area| area.areatype() == UserMapAreaType::Heap)
            .max_by_key(|area| area.end_va())
            .unwrap()
    }

    ///
    pub fn find_area(&mut self, va: VirtAddr) -> Option<&mut UserMapArea> {
        let idx = self.find_area_index(va)?;
        self.areas.get_mut(idx)
    }

    fn find_area_index(&self, va: VirtAddr) -> Option<usize> {
        let vpn = va.floor();
        let mut left = 0;
        let mut right = self.areas.len();
        while left < right {
            let mid = (left + right) / 2;
            if self.areas[mid].start_vpn() <= vpn {
                left = mid + 1;
            } else {
                right = mid;
            }
        }
        if left == 0 {
            return None;
        }
        let idx = left - 1;
        (vpn < self.areas[idx].end_vpn()).then_some(idx)
    }

    fn find_area_start_vpn_index(&self, start_vpn: VirtPageNum) -> Option<usize> {
        let mut left = 0;
        let mut right = self.areas.len();
        while left < right {
            let mid = (left + right) / 2;
            match self.areas[mid].start_vpn().cmp(&start_vpn) {
                core::cmp::Ordering::Less => left = mid + 1,
                core::cmp::Ordering::Greater => right = mid,
                core::cmp::Ordering::Equal => return Some(mid),
            }
        }
        None
    }

    fn area_insert_index(&self, start_va: VirtAddr) -> usize {
        let start_vpn = start_va.floor();
        let mut left = 0;
        let mut right = self.areas.len();
        while left < right {
            let mid = (left + right) / 2;
            if self.areas[mid].start_vpn() <= start_vpn {
                left = mid + 1;
            } else {
                right = mid;
            }
        }
        left
    }

    /// Insert a user VMA while preserving ascending start address order.
    pub fn insert_area_sorted(&mut self, map_area: UserMapArea) {
        let idx = self.area_insert_index(map_area.start_va());
        self.areas.insert(idx, map_area);
    }

    /// 尝试向下扩展用户栈，用于处理栈溢出时的缺页异常
    pub(crate) fn try_expand_stack(&mut self, va: VirtAddr) -> Option<()> {
        // 获取当前用户态 sp（trap 上下文中保存的 sp）
        info!("[DEBUG] try_expand_stack called for va={:#x}", va.0);

        let current_sp = current_trap_cx()[TrapFrameArgs::SP];
        info!("[DEBUG] current_sp={:#x}", current_sp);

        // 找到 va 下方最近的栈区域（包括 Stack 类型和带有 growdown_flag 的 Mmap 类型）
        let mut best_idx = None;
        // let mut best_start = 0usize;
        let mut best_distance = usize::MAX; // 修改为距离而不是起始地址

        for (idx, area) in self.areas.iter().enumerate() {
            // 支持两种类型的区域：Stack 类型 和 带有 growdown_flag 的 Mmap 类型
            let is_stack_type = area.areatype() == UserMapAreaType::Stack;
            let is_growdown_mmap = area.areatype() == UserMapAreaType::Mmap && area.growdown_flag;
            info!(
                "[DEBUG] area {}: type={:?}, start={:#x}, growdown_flag={}",
                idx,
                area.areatype(),
                area.start_va().0,
                area.growdown_flag
            );
            if !is_stack_type && !is_growdown_mmap {
                continue;
            }
            let area_start = area.start_va().0;
            if va.0 < area_start {
                let near_area = area_start.saturating_sub(va.0) <= STACK_EXPAND_LIMIT;
                let near_sp = va.0 >= current_sp.saturating_sub(PAGE_SIZE);
                info!(
                    "[DEBUG] area {}: va < area_start={}, near_area={}, near_sp={}",
                    idx,
                    va.0 < area_start,
                    near_area,
                    near_sp
                );
                if near_area || near_sp {
                    // if area_start > best_start {
                    //     best_start = area_start;
                    //     best_idx = Some(idx);
                    // }
                    // 计算距离，选择最近的区域
                    let distance = area_start - va.0;
                    if distance < best_distance {
                        best_distance = distance;
                        best_idx = Some(idx);
                    }
                }
            }
        }
        if best_idx.is_none() {
            info!("[DEBUG] try_expand_stack: no suitable area found");
            return None;
        }
        let idx = best_idx?;
        let new_start_vpn = va.floor();
        let old_start_vpn = self.areas[idx].start_vpn();
        info!(
            "[DEBUG] new_start_vpn={:#x}, old_start_vpn={:#x}",
            new_start_vpn.0, old_start_vpn.0
        );

        if new_start_vpn >= old_start_vpn {
            return None;
        }
        if new_start_vpn >= old_start_vpn {
            info!("[DEBUG] try_expand_stack: new_start_vpn >= old_start_vpn, returning None");
            return None;
        }
        let new_start_va = VirtAddr::from(new_start_vpn.0 * PAGE_SIZE);
        let old_start_va = VirtAddr::from(old_start_vpn.0 * PAGE_SIZE);

        // 总大小限制
        info!(
            "[DEBUG] stack size after expansion: {} bytes",
            old_start_va.0 - new_start_va.0
        );

        if old_start_va.0 - new_start_va.0 > MAX_STACK_SIZE {
            info!("[DEBUG] try_expand_stack: exceeds MAX_STACK_SIZE");

            return None;
        }

        // 检查扩展后是否会与任何其他区域重叠（包括其他线程的栈）
        for other in self.areas.iter() {
            // 只有当新区域真正与其他区域重叠时才阻止扩展
            // 当 new_start_va == other.end_va 时，两个区域相邻，不算重叠
            if new_start_va.0 < other.end_va().0 && old_start_va.0 > other.start_va().0 {
                info!(
                    "[DEBUG] try_expand_stack: would overlap with area {:?} (start={:#x}, end={:#x})",
                    other.areatype(),
                    other.start_va().0,
                    other.end_va().0
                );
                return None;
            }
        }

        // 只映射缺页地址所在的那一页，避免一次性分配大量物理页
        let frame = frame_alloc()?;
        let ppn = frame.ppn;
        let zero_ptr = ((ppn.0 << 12) + VIRT_ADDR_START) as *mut u8;
        unsafe {
            core::ptr::write_bytes(zero_ptr, 0, PAGE_SIZE);
        }
        let mut area = self.areas.remove(idx);
        area.data_frames.insert(new_start_vpn, Arc::new(frame));
        self.page_table.map_page(
            new_start_vpn,
            ppn,
            area.map_perm.into(),
            MappingSize::Page4KB,
        );
        area.range_va_mut().start = new_start_va;
        if area.data_frames.len() >= area.vpn_range().count() {
            area.clear_lazy_flag();
        }
        self.insert_area_sorted(area);
        polyhal::multicore::shootdown_tlb_all(self.token());
        Some(())
    }

    #[allow(missing_docs)]
    pub fn insert_framed_area(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
        permission: MapPermission,
        area_type: UserMapAreaType,
        file_info: Option<(Option<Arc<dyn File>>, usize, usize)>,
    ) {
        match area_type {
            UserMapAreaType::Heap => self.push(
                UserMapArea::new(
                    start_va,
                    end_va,
                    MapType::Framed,
                    permission,
                    area_type,
                    true,
                ),
                None,
                start_va.0,
            ),
            UserMapAreaType::Mmap => {
                let mut map_area = UserMapArea::new(
                    start_va,
                    end_va,
                    MapType::Framed,
                    permission,
                    area_type,
                    true,
                );
                if let Some((file, file_offset, flags)) = file_info {
                    // 文件映射
                    map_area.map_file = file;
                    map_area.file_offset = file_offset;
                    map_area.flags = match flags & 0x3 {
                        0x1 => MmapType::MapShared,
                        0x2 => MmapType::MapPrivate,
                        _ => MmapType::MapPrivate,
                    };
                    if map_area.map_file.is_some()
                        && map_area.flags == MmapType::MapPrivate
                        && permission.contains(MapPermission::W)
                    {
                        // Linux MAP_PRIVATE initially shares clean page-cache
                        // pages and creates a private copy only on first write.
                        map_area.set_cow_flag();
                    }
                    if map_area.map_file.is_none() && map_area.flags == MmapType::MapShared {
                        map_area.enable_shared_anonymous();
                    }
                } else {
                    // 匿名映射
                    map_area.map_file = None;
                    map_area.flags = MmapType::MapPrivate;
                }

                self.push(map_area, None, start_va.0);
            }
            UserMapAreaType::Stack => {
                // User stacks are demand-paged. The initial exec stack and later
                // stack accesses fault in only the pages they actually touch.
                self.push(
                    UserMapArea::new(
                        start_va,
                        end_va,
                        MapType::Framed,
                        permission,
                        area_type,
                        true,
                    ),
                    None,
                    start_va.0,
                );
            }
            UserMapAreaType::TrapContext | UserMapAreaType::RtSigreturnTrampoline => self.push(
                UserMapArea::new(
                    start_va,
                    end_va,
                    MapType::Framed,
                    permission,
                    area_type,
                    false,
                ),
                None,
                start_va.0,
            ),

            _ => self.push(
                UserMapArea::new(
                    start_va,
                    end_va,
                    MapType::Framed,
                    permission,
                    area_type,
                    false,
                ),
                None,
                start_va.0,
            ),
        }
    }

    /// Insert a user area using frames allocated by the caller.
    pub fn insert_framed_area_with_frames(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
        permission: MapPermission,
        area_type: UserMapAreaType,
        data_frames: BTreeMap<VirtPageNum, Arc<FrameTracker>>,
    ) {
        self.push(
            UserMapArea::with_frames(
                start_va,
                end_va,
                MapType::Framed,
                permission,
                area_type,
                data_frames,
            ),
            None,
            start_va.0,
        );
    }

    #[cfg(target_arch = "riscv64")]
    ///继承内核页表映射
    pub fn from_kernel(kernel_vm_set: &KernelVMSet) -> Self {
        trace!("from_kernel");
        let page_table = PageTable::new();
        page_table
            .root()
            .get_pte_array()
            .copy_from_slice(&kernel_vm_set.page_table.root().get_pte_array()[..]);
        Self {
            page_table: page_table,
            areas: Vec::new(),
            membarrier_registrations: 0,
            process_owners: 1,
        }
    }
    #[cfg(all(target_arch = "loongarch64", not(board = "2k1000")))]
    ///
    pub fn from_kernel(_kernel_vm_set: &KernelVMSet) -> Self {
        trace!("from_kernel");
        let page_table = PageTable::new();
        let dst_root = page_table.root().get_pte_array();
        let src_root = _kernel_vm_set.page_table.root().get_pte_array();
        let kernel_half = dst_root.len() / 2;
        dst_root[kernel_half..].copy_from_slice(&src_root[kernel_half..]);
        Self {
            page_table: page_table,
            areas: Vec::new(),
            membarrier_registrations: 0,
            process_owners: 1,
        }
    }

    #[cfg(all(target_arch = "loongarch64", board = "2k1000"))]
    ///
    pub fn from_kernel(_kernel_vm_set: &KernelVMSet) -> Self {
        trace!("from_kernel");
        let page_table = PageTable::new();
        page_table
            .root()
            .get_pte_array()
            .copy_from_slice(&_kernel_vm_set.page_table.root().get_pte_array()[..]);
        Self {
            page_table: page_table,
            areas: Vec::new(),
            membarrier_registrations: 0,
            process_owners: 1,
        }
    }
    ///
    pub fn push(&mut self, mut map_area: UserMapArea, data: Option<&[u8]>, exact_start_va: usize) {
        if !map_area.lazy_flag {
            map_area.map(&mut self.page_table);
            if let Some(data) = data {
                trace!("perm {:?}", map_area.perm().contains(MapPermission::X));
                map_area.copy_data(&self.page_table, data, exact_start_va);
                if map_area.perm().contains(MapPermission::X) {
                    polyhal::multicore::synchronize_instruction_cache(self.token());
                }
            }
        } else if !map_area.data_frames.is_empty() {
            // lazy 但已有预分配的物理页（如共享内存）：直接建立映射，不复制的帧
            let flags = if map_area.cow_flag {
                cow_mapping_flags(map_area.map_perm)
            } else {
                map_area.initial_mapping_flags()
            };
            for (&vpn, frame) in map_area.data_frames.iter() {
                self.page_table
                    .map_page(vpn, frame.ppn, flags, MappingSize::Page4KB);
            }
        }
        // 否则 lazy 且 data_frames 为空（普通 mmap/堆/栈），不预映射

        self.insert_area_sorted(map_area);
    }

    fn push_elf_load_area(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_perm: MapPermission,
        data: &[u8],
        exact_start_va: usize,
    ) -> Option<()> {
        let mut map_area = UserMapArea::new(
            start_va,
            end_va,
            MapType::Framed,
            map_perm,
            UserMapAreaType::Elf,
            true,
        );
        let mut copied = 0usize;
        while copied < data.len() {
            let va = exact_start_va + copied;
            let vpn = VirtAddr::from(va).floor();
            let page_offset = va % PAGE_SIZE;
            let copy_len = (PAGE_SIZE - page_offset).min(data.len() - copied);
            if !map_area.data_frames.contains_key(&vpn) {
                let Some(frame) = frame_alloc() else {
                    return None;
                };
                frame.ppn.get_bytes_array().fill(0);
                map_area.data_frames.insert(vpn, Arc::new(frame));
            }
            let frame = map_area.data_frames.get(&vpn).unwrap();
            frame.ppn.get_bytes_array()[page_offset..page_offset + copy_len]
                .copy_from_slice(&data[copied..copied + copy_len]);
            copied += copy_len;
        }
        for (&vpn, frame) in map_area.data_frames.iter() {
            self.page_table
                .map_page(vpn, frame.ppn, map_perm.into(), MappingSize::Page4KB);
        }
        if map_perm.contains(MapPermission::X) {
            polyhal::multicore::synchronize_instruction_cache(self.token());
        }
        self.insert_area_sorted(map_area);
        Some(())
    }

    fn push_elf_load_area_from_file(
        &mut self,
        file: &Arc<dyn File>,
        path: &str,
        backing_file_size: usize,
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_perm: MapPermission,
        file_offset: u64,
        segment_file_size: u64,
        exact_start_va: usize,
    ) -> Option<()> {
        let segment_file_size = segment_file_size as usize;
        let file_offset = file_offset as usize;
        let file_end = file_offset.checked_add(segment_file_size)?;
        if file_end > backing_file_size {
            warn!(
                "[from_elf_file] truncated LOAD segment: path={} offset={:#x} filesz={:#x} end={} file_size={}",
                path, file_offset, segment_file_size, file_end, backing_file_size
            );
            return None;
        }

        let page_delta = exact_start_va.checked_sub(start_va.0)?;
        let aligned_file_offset = file_offset.checked_sub(page_delta)?;
        if aligned_file_offset % PAGE_SIZE != 0 {
            warn!(
                "[from_elf_file] unaligned LOAD relation: path={} vaddr={:#x} aligned_vaddr={:#x} offset={:#x}",
                path, exact_start_va, start_va.0, file_offset
            );
            return None;
        }
        let file_zero_start = exact_start_va.checked_add(segment_file_size)?;
        if file_zero_start > end_va.0 {
            warn!(
                "[from_elf_file] LOAD filesz exceeds memsz: path={} file_end_va={:#x} mem_end_va={:#x}",
                path, file_zero_start, end_va.0
            );
            return None;
        }

        let mut map_area = UserMapArea::new(
            start_va,
            end_va,
            MapType::Framed,
            map_perm,
            UserMapAreaType::Elf,
            true,
        );
        map_area.map_file = Some(file.clone());
        map_area.file_offset = aligned_file_offset;
        map_area.file_zero_start = Some(file_zero_start);
        map_area.flags = MmapType::MapPrivate;
        if map_perm.contains(MapPermission::W) {
            map_area.set_cow_flag();
        }
        crate::task::perf_stats::record_exec_file_mapping(
            segment_file_size,
            (end_va.0 - start_va.0) / PAGE_SIZE,
        );
        self.insert_area_sorted(map_area);
        Some(())
    }

    fn install_rt_sigreturn_trampoline(&mut self) {
        let start = config::USER_RT_SIGRETURN_TRAMPOLINE;
        let end = start + PAGE_SIZE;
        if self
            .areas
            .iter()
            .any(|area| start < area.end_va().0 && end > area.start_va().0)
        {
            warn!(
                "rt_sigreturn trampoline overlaps an existing user area at {:#x}..{:#x}",
                start, end
            );
            return;
        }
        self.push(
            UserMapArea::new(
                VirtAddr::from(start),
                VirtAddr::from(end),
                MapType::Framed,
                MapPermission::R | MapPermission::X | MapPermission::U,
                UserMapAreaType::RtSigreturnTrampoline,
                false,
            ),
            Some(&USER_RT_SIGRETURN_TRAMPOLINE_CODE),
            start,
        );
    }

    /// Include ELF sections, the rt_sigreturn trampoline and user stack,
    /// also returns user_sp and entry point.
    pub fn from_elf(elf_data: &[u8]) -> Option<(Self, usize, usize, Vec<(usize, usize)>)> {
        let mut vmset = Self::from_kernel(&KERNEL_VMSET.lock());
        // map program headers of elf, with U flag
        let elf = match xmas_elf::ElfFile::new(elf_data) {
            Ok(e) => e,
            Err(_) => {
                info!("[DEBUG execve] Not an ELF file! Returning ENOEXEC.");
                return None; // 不是 ELF，直接返回 None
            }
        };
        if !elf_program_headers_in_bounds("program", elf.input, &elf) {
            return None;
        }
        let elf_header = elf.header;
        let magic = elf_header.pt1.magic;
        assert_eq!(magic, [0x7f, 0x45, 0x4c, 0x46], "invalid elf!");
        let ph_count = elf_header.pt2.ph_count();
        let mut max_end_va: usize = 0;
        let mut phdr_addr = 0;
        let mut interp_path: Option<&str> = None;
        for i in 0..ph_count {
            let ph = elf.program_header(i).unwrap();
            if ph.get_type().unwrap() == xmas_elf::program::Type::Interp {
                let path_bytes = elf_segment_data(
                    "program interpreter",
                    elf.input,
                    ph.offset(),
                    ph.file_size(),
                )?;
                interp_path = core::str::from_utf8(path_bytes)
                    .ok()
                    .and_then(|s| s.split('\0').next());
                if let Some(path) = interp_path {
                    info!(
                        "[from_elf] Dynamic ELF detected, interpreter path: {}",
                        path
                    );
                }
            }
            if ph.get_type().unwrap() == xmas_elf::program::Type::Phdr {
                phdr_addr = ph.virtual_addr() as usize;
            }
            if ph.get_type().unwrap() == xmas_elf::program::Type::Load {
                let raw_start_va: VirtAddr = (ph.virtual_addr() as usize).into();
                let raw_end_va: VirtAddr = ((ph.virtual_addr() + ph.mem_size()) as usize).into();
                // 将虚拟地址范围对齐到页面边界，确保 va_range 与页表映射范围一致
                let start_va = VirtAddr::from(raw_start_va.floor().0 * PAGE_SIZE);
                let end_va = VirtAddr::from(raw_end_va.ceil().0 * PAGE_SIZE);
                // error!("start_va {:#x}, end_va{:#x}", start_va.0, end_va.0);
                let mut map_perm = MapPermission::U;
                let ph_flags = ph.flags();
                if ph_flags.is_read() {
                    map_perm |= MapPermission::R;
                }
                if ph_flags.is_write() {
                    map_perm |= MapPermission::W;
                }
                if ph_flags.is_execute() {
                    map_perm |= MapPermission::X;
                }
                let end_va_usize: usize = raw_end_va.into();
                if end_va_usize > max_end_va {
                    max_end_va = end_va_usize;
                }
                let segment_data =
                    elf_segment_data("program LOAD", elf.input, ph.offset(), ph.file_size())?;
                vmset.push_elf_load_area(
                    start_va,
                    end_va,
                    map_perm,
                    segment_data,
                    raw_start_va.0,
                )?;
            }
        }

        let mut interp_base: usize = 0;
        let mut final_entry = elf.header.pt2.entry_point() as usize;

        if let Some(path) = interp_path {
            let root_dentry = match GLOBAL_DCACHE.get("/") {
                Some(d) => d,
                None => {
                    warn!("[from_elf] Failed to get root dentry, cannot load interpreter");
                    return None;
                }
            };
            let interp_file = match open_file(
                root_dentry,
                path,
                OpenFlags::RDONLY,
                crate::fs::vfs::inode::InodeMode::FILE,
            ) {
                Ok(f) => f,
                Err(_) => {
                    warn!("[from_elf] Failed to open interpreter: {}", path);
                    return None;
                }
            };
            let interp_data = read_interp_image(&interp_file, path)?;
            let interp_data = interp_data.as_slice();
            let interp_elf = match xmas_elf::ElfFile::new(interp_data) {
                Ok(e) => e,
                Err(_) => {
                    warn!("[from_elf] Interpreter is not a valid ELF");
                    return None;
                }
            };
            if !elf_program_headers_in_bounds("interpreter", interp_data, &interp_elf) {
                return None;
            }

            interp_base = (max_end_va + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            info!("[from_elf] Loading interpreter at base {:#x}", interp_base);

            let interp_ph_count = interp_elf.header.pt2.ph_count();
            let mut interp_max_end_va: usize = 0;
            for i in 0..interp_ph_count {
                let ph = interp_elf.program_header(i).unwrap();
                if ph.get_type().unwrap() == xmas_elf::program::Type::Load {
                    let raw_start_va: VirtAddr = (interp_base + ph.virtual_addr() as usize).into();
                    let raw_end_va: VirtAddr =
                        (interp_base + (ph.virtual_addr() + ph.mem_size()) as usize).into();
                    // 将虚拟地址范围对齐到页面边界，确保 va_range 与页表映射范围一致
                    let start_va = VirtAddr::from(raw_start_va.floor().0 * PAGE_SIZE);
                    let end_va = VirtAddr::from(raw_end_va.ceil().0 * PAGE_SIZE);
                    let mut map_perm = MapPermission::U;
                    let ph_flags = ph.flags();
                    if ph_flags.is_read() {
                        map_perm |= MapPermission::R;
                    }
                    if ph_flags.is_write() {
                        map_perm |= MapPermission::W;
                    }
                    if ph_flags.is_execute() {
                        map_perm |= MapPermission::X;
                    }
                    let end_va_usize: usize = raw_end_va.into();
                    if end_va_usize > interp_max_end_va {
                        interp_max_end_va = end_va_usize;
                    }
                    let segment_data = elf_segment_data(
                        "interpreter LOAD",
                        interp_data,
                        ph.offset(),
                        ph.file_size(),
                    )?;
                    vmset.push_elf_load_area(
                        start_va,
                        end_va,
                        map_perm,
                        segment_data,
                        raw_start_va.0,
                    )?;
                }
            }
            max_end_va = interp_max_end_va;
            final_entry = interp_base + interp_elf.header.pt2.entry_point() as usize;
            info!("[from_elf] Interpreter entry point: {:#x}", final_entry);
        }

        let heap_base_vpn = VirtAddr::from(max_end_va).ceil();
        vmset.alloc_user_heap(heap_base_vpn.into());
        vmset.install_rt_sigreturn_trampoline();

        let user_stack_top = USER_STACK_BASE;

        if phdr_addr == 0 {
            // 如果没找到 PHDR 段，Fallback 方案：
            let mut elf_base = 0;
            for i in 0..ph_count {
                if let Ok(ph) = elf.program_header(i) {
                    if ph.get_type().unwrap() == xmas_elf::program::Type::Load {
                        elf_base = ph.virtual_addr() as usize - ph.offset() as usize;
                        break;
                    }
                }
            }
            phdr_addr = elf_base + elf.header.pt2.ph_offset() as usize;
        }
        const AT_PHDR: usize = 3;
        const AT_PHENT: usize = 4;
        const AT_PHNUM: usize = 5;
        const AT_PAGESZ: usize = 6;
        const AT_BASE: usize = 7;
        const AT_FLAGS: usize = 8;
        const AT_ENTRY: usize = 9;
        const AT_UID: usize = 11;
        const AT_EUID: usize = 12;
        const AT_GID: usize = 13;
        const AT_EGID: usize = 14;
        const AT_SECURE: usize = 23;
        const AT_CLKTCK: usize = 17;
        const AT_RSEQ_FEATURE_SIZE: usize = 27;
        const AT_RSEQ_ALIGN: usize = 28;
        let auxv = vec![
            (AT_PHDR, phdr_addr),
            (AT_PHENT, elf.header.pt2.ph_entry_size() as usize),
            (AT_PHNUM, elf.header.pt2.ph_count() as usize),
            (AT_PAGESZ, PAGE_SIZE),
            (AT_BASE, interp_base),
            (AT_FLAGS, 0),
            (AT_ENTRY, elf.header.pt2.entry_point() as usize),
            (AT_UID, 0),
            (AT_EUID, 0),
            (AT_GID, 0),
            (AT_EGID, 0),
            (AT_SECURE, 0),
            (AT_CLKTCK, 100),
            (AT_RSEQ_FEATURE_SIZE, 28),
            (AT_RSEQ_ALIGN, 32),
        ];

        Some((vmset, user_stack_top, final_entry, auxv))
    }

    /// Build a user address space from an ELF file without reading the whole
    /// image into a contiguous kernel heap buffer.
    pub fn from_elf_file(
        file: &Arc<dyn File>,
        path: &str,
    ) -> Option<(Self, usize, usize, Vec<(usize, usize)>)> {
        let active_task = crate::task::current_task();
        if let Some(task) = active_task.as_ref() {
            task.set_active_syscall_stage(22142);
        }
        let file_size = file.get_inode().map(|inode| inode.get_size()).unwrap_or(0);
        if let Some(task) = active_task.as_ref() {
            crate::fs::elf_trace::log_file_state("exec_enter", task.process_id(), None, file);
        }
        let elf_headers = read_elf_header_image(file, path, file_size)?;
        if let Some(task) = active_task.as_ref() {
            crate::fs::elf_trace::log_exec_header_compare(
                task.process_id(),
                path,
                file,
                &elf_headers,
            );
        }
        if let Some(task) = active_task.as_ref() {
            task.set_active_syscall_stage(22143);
        }
        let mut vmset = Self::from_kernel(&KERNEL_VMSET.lock());
        let elf = match xmas_elf::ElfFile::new(&elf_headers) {
            Ok(e) => e,
            Err(_) => {
                info!("[DEBUG execve] Not an ELF file! Returning ENOEXEC.");
                return None;
            }
        };
        if !elf_program_headers_in_bounds("program", elf.input, &elf) {
            return None;
        }
        let elf_header = elf.header;
        let magic = elf_header.pt1.magic;
        assert_eq!(magic, [0x7f, 0x45, 0x4c, 0x46], "invalid elf!");
        let ph_count = elf_header.pt2.ph_count();
        let mut max_end_va: usize = 0;
        let mut phdr_addr = 0;
        let mut interp_path: Option<String> = None;

        for i in 0..ph_count {
            let ph = elf.program_header(i).unwrap();
            if ph.get_type().unwrap() == xmas_elf::program::Type::Interp {
                interp_path =
                    read_interp_path_from_file(file, path, file_size, ph.offset(), ph.file_size());
                if let Some(path) = interp_path.as_ref() {
                    info!(
                        "[from_elf_file] Dynamic ELF detected, interpreter path: {}",
                        path
                    );
                }
            }
            if ph.get_type().unwrap() == xmas_elf::program::Type::Phdr {
                phdr_addr = ph.virtual_addr() as usize;
            }
            if ph.get_type().unwrap() == xmas_elf::program::Type::Load {
                let raw_start_va: VirtAddr = (ph.virtual_addr() as usize).into();
                let raw_end_va: VirtAddr = ((ph.virtual_addr() + ph.mem_size()) as usize).into();
                let start_va = VirtAddr::from(raw_start_va.floor().0 * PAGE_SIZE);
                let end_va = VirtAddr::from(raw_end_va.ceil().0 * PAGE_SIZE);
                let mut map_perm = MapPermission::U;
                let ph_flags = ph.flags();
                if ph_flags.is_read() {
                    map_perm |= MapPermission::R;
                }
                if ph_flags.is_write() {
                    map_perm |= MapPermission::W;
                }
                if ph_flags.is_execute() {
                    map_perm |= MapPermission::X;
                }
                let end_va_usize: usize = raw_end_va.into();
                if end_va_usize > max_end_va {
                    max_end_va = end_va_usize;
                }
                vmset.push_elf_load_area_from_file(
                    file,
                    path,
                    file_size,
                    start_va,
                    end_va,
                    map_perm,
                    ph.offset(),
                    ph.file_size(),
                    raw_start_va.0,
                )?;
            }
        }

        let mut interp_base: usize = 0;
        let mut final_entry = elf.header.pt2.entry_point() as usize;

        if let Some(path) = interp_path.as_deref() {
            if let Some(task) = active_task.as_ref() {
                task.set_active_syscall_stage(22144);
            }
            let root_dentry = match GLOBAL_DCACHE.get("/") {
                Some(d) => d,
                None => {
                    warn!("[from_elf_file] Failed to get root dentry, cannot load interpreter");
                    return None;
                }
            };
            let interp_file = match open_file(
                root_dentry,
                path,
                OpenFlags::RDONLY,
                crate::fs::vfs::inode::InodeMode::FILE,
            ) {
                Ok(f) => f,
                Err(_) => {
                    warn!("[from_elf_file] Failed to open interpreter: {}", path);
                    return None;
                }
            };
            let interp_file_size = interp_file
                .get_inode()
                .map(|inode| inode.get_size())
                .unwrap_or(0);
            let interp_headers = read_elf_header_image(&interp_file, path, interp_file_size)?;
            let interp_elf = match xmas_elf::ElfFile::new(&interp_headers) {
                Ok(e) => e,
                Err(_) => {
                    warn!("[from_elf_file] Interpreter is not a valid ELF");
                    return None;
                }
            };
            if !elf_program_headers_in_bounds("interpreter", interp_elf.input, &interp_elf) {
                return None;
            }

            interp_base = (max_end_va + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
            info!(
                "[from_elf_file] Loading interpreter at base {:#x}",
                interp_base
            );

            let interp_ph_count = interp_elf.header.pt2.ph_count();
            let mut interp_max_end_va: usize = 0;
            for i in 0..interp_ph_count {
                let ph = interp_elf.program_header(i).unwrap();
                if ph.get_type().unwrap() == xmas_elf::program::Type::Load {
                    let raw_start_va: VirtAddr = (interp_base + ph.virtual_addr() as usize).into();
                    let raw_end_va: VirtAddr =
                        (interp_base + (ph.virtual_addr() + ph.mem_size()) as usize).into();
                    let start_va = VirtAddr::from(raw_start_va.floor().0 * PAGE_SIZE);
                    let end_va = VirtAddr::from(raw_end_va.ceil().0 * PAGE_SIZE);
                    let mut map_perm = MapPermission::U;
                    let ph_flags = ph.flags();
                    if ph_flags.is_read() {
                        map_perm |= MapPermission::R;
                    }
                    if ph_flags.is_write() {
                        map_perm |= MapPermission::W;
                    }
                    if ph_flags.is_execute() {
                        map_perm |= MapPermission::X;
                    }
                    let end_va_usize: usize = raw_end_va.into();
                    if end_va_usize > interp_max_end_va {
                        interp_max_end_va = end_va_usize;
                    }
                    vmset.push_elf_load_area_from_file(
                        &interp_file,
                        path,
                        interp_file_size,
                        start_va,
                        end_va,
                        map_perm,
                        ph.offset(),
                        ph.file_size(),
                        raw_start_va.0,
                    )?;
                }
            }
            max_end_va = interp_max_end_va;
            final_entry = interp_base + interp_elf.header.pt2.entry_point() as usize;
            info!(
                "[from_elf_file] Interpreter entry point: {:#x}",
                final_entry
            );
            if let Some(task) = active_task.as_ref() {
                task.set_active_syscall_stage(22145);
            }
        }

        let heap_base_vpn = VirtAddr::from(max_end_va).ceil();
        vmset.alloc_user_heap(heap_base_vpn.into());
        if let Some(task) = active_task.as_ref() {
            task.set_active_syscall_stage(22146);
        }
        vmset.install_rt_sigreturn_trampoline();

        let user_stack_top = USER_STACK_BASE;

        if phdr_addr == 0 {
            let mut elf_base = 0;
            for i in 0..ph_count {
                if let Ok(ph) = elf.program_header(i) {
                    if ph.get_type().unwrap() == xmas_elf::program::Type::Load {
                        elf_base = ph.virtual_addr() as usize - ph.offset() as usize;
                        break;
                    }
                }
            }
            phdr_addr = elf_base + elf.header.pt2.ph_offset() as usize;
        }
        const AT_PHDR: usize = 3;
        const AT_PHENT: usize = 4;
        const AT_PHNUM: usize = 5;
        const AT_PAGESZ: usize = 6;
        const AT_BASE: usize = 7;
        const AT_FLAGS: usize = 8;
        const AT_ENTRY: usize = 9;
        const AT_UID: usize = 11;
        const AT_EUID: usize = 12;
        const AT_GID: usize = 13;
        const AT_EGID: usize = 14;
        const AT_SECURE: usize = 23;
        const AT_CLKTCK: usize = 17;
        const AT_RSEQ_FEATURE_SIZE: usize = 27;
        const AT_RSEQ_ALIGN: usize = 28;
        let auxv = vec![
            (AT_PHDR, phdr_addr),
            (AT_PHENT, elf.header.pt2.ph_entry_size() as usize),
            (AT_PHNUM, elf.header.pt2.ph_count() as usize),
            (AT_PAGESZ, PAGE_SIZE),
            (AT_BASE, interp_base),
            (AT_FLAGS, 0),
            (AT_ENTRY, elf.header.pt2.entry_point() as usize),
            (AT_UID, 0),
            (AT_EUID, 0),
            (AT_GID, 0),
            (AT_EGID, 0),
            (AT_SECURE, 0),
            (AT_CLKTCK, 100),
            (AT_RSEQ_FEATURE_SIZE, 28),
            (AT_RSEQ_ALIGN, 32),
        ];

        if let Some(task) = active_task.as_ref() {
            task.set_active_syscall_stage(22147);
        }
        Some((vmset, user_stack_top, final_entry, auxv))
    }

    #[allow(missing_docs)]
    pub fn from_existed_user(user_vmset: &UserVMSet) -> Self {
        let mut vmset = Self::from_kernel(&KERNEL_VMSET.lock());
        vmset.membarrier_registrations = user_vmset.membarrier_registrations;
        // let mut vmset = Self::new_bare();
        // let pte = user_vmset.translate(VirtPageNum(0x10)).unwrap();
        // println!("user vmset satp {:#x}", user_vmset.token());
        // println!("entry ppn {:#x}", pte.ppn().0);
        // unsafe{
        //     let pgdl: usize;
        //     core::arch::asm!("csrrd {}, 0x1B", out(reg) pgdl);
        //     error!("PGDL = 0x{:016x}", pgdl);
        //     }
        // copy data sections/trap_context/user_stack
        for area in user_vmset.areas.iter() {
            // println!("is lazyalloc {:?}", area.lazy_flag);
            // println!("is cow {:?}", area.cow_flag());
            // println!("area type {:?}", area.areatype());
            let new_area = UserMapArea::from_another(area);

            vmset.push(new_area, None, 0);

            // copy data from another space
            // 只复制已经分配的页面（对 lazy 区域尤其重要）
            for (&vpn, frame) in area.data_frames.iter() {
                let src_ppn = frame.ppn;
                let dst_ppn = vmset.translate(vpn).unwrap().ppn();
                dst_ppn
                    .get_bytes_array()
                    .copy_from_slice(src_ppn.get_bytes_array());
                // info!("src ppn {:#x}, dst ppn {:#x}", src_ppn.0, dst_ppn.0);
            }
        }

        // TLB::flush_all();
        vmset
    }

    /// 为 CLONE_VM 创建共享地址空间：新进程映射相同的物理页，不做 COW
    pub fn from_existed_user_vm(user_vmset: &UserVMSet) -> Self {
        let mut vmset = Self::from_kernel(&KERNEL_VMSET.lock());
        vmset.membarrier_registrations = user_vmset.membarrier_registrations;
        for area in user_vmset.areas.iter() {
            let new_area = UserMapArea::from_another(area);
            for (&vpn, frame) in area.data_frames.iter() {
                vmset.page_table.map_page_no_flush(
                    vpn,
                    frame.ppn,
                    area.initial_mapping_flags(),
                    MappingSize::Page4KB,
                );
            }
            vmset.insert_area_sorted(new_area);
        }
        vmset
    }

    ///
    pub fn from_existed_user_cow(user_vmset: &mut UserVMSet, parent_pid: usize) -> Self {
        const COOPERATIVE_BATCH_PAGES: usize = 256;

        #[inline]
        fn reschedule_large_fork_batch(page_index: usize) {
            if page_index != 0 && page_index % COOPERATIVE_BATCH_PAGES == 0 {
                // The VM sleeping lock remains held, so address-space metadata
                // is stable. Yielding only lets unrelated runnable work use
                // this CPU while a large fork constructs page tables.
                crate::task::suspend_current_and_run_next();
            }
        }

        let fork_cow_trace = ForkCowTraceGuard::begin(parent_pid, user_vmset.areas.len());
        let mut vmset = Self::from_kernel(&KERNEL_VMSET.lock());
        vmset.membarrier_registrations = user_vmset.membarrier_registrations;
        fork_cow_trace.progress(2, 0, user_vmset.areas.len());
        let mut direct_clone_pages: Vec<VirtPageNum> = Vec::new();
        let mut frame_page: Vec<(VirtPageNum, PTEFlags)> = Vec::new();
        let area_count = user_vmset.areas.len();
        for (area_index, area) in user_vmset.areas.iter_mut().enumerate() {
            fork_cow_trace.progress(3, area_index, area_count);
            let resident_pages = area.data_frames.len();
            fork_cow_trace.add_resident_pages(resident_pages);
            fork_cow_trace.area_progress(1, 0, resident_pages);
            if area.areatype() == UserMapAreaType::TrapContext
                || area.areatype() == UserMapAreaType::RtSigreturnTrampoline
            {
                fork_cow_trace.area_progress(2, 0, resident_pages);
                let mut new_area = UserMapArea::from_another(area);
                new_area.data_frames.clear();
                fork_cow_trace.area_progress(3, 0, resident_pages);
                vmset.push(new_area, None, 0);
                for vpn in area.vpn_range() {
                    direct_clone_pages.push(vpn);
                }
                fork_cow_trace.area_progress(4, resident_pages, resident_pages);
            } else if area.areatype() == UserMapAreaType::Shm
                || (area.areatype() == UserMapAreaType::Mmap && area.flags == MmapType::MapShared)
            {
                // 共享内存区域或 mmap MAP_SHARED：父子共享已经实际分配的页。
                // 匿名 MAP_SHARED 还会通过 from_another 克隆 shared_anonymous 后端，
                // 因此尚未 fault 的 lazy 页保持未分配，之后也会发布同一个物理页。
                fork_cow_trace.area_progress(2, 0, resident_pages);
                let new_area = UserMapArea::from_another(area);
                fork_cow_trace.area_progress(3, 0, resident_pages);
                for (page_index, (&vpn, frame)) in area.data_frames.iter().enumerate() {
                    fork_cow_trace.area_progress(3, page_index, resident_pages);
                    reschedule_large_fork_batch(page_index);
                    vmset.page_table.map_page_no_flush(
                        vpn,
                        frame.ppn,
                        area.initial_mapping_flags(),
                        MappingSize::Page4KB,
                    );
                }
                vmset.insert_area_sorted(new_area);
                fork_cow_trace.area_progress(4, resident_pages, resident_pages);
            } else {
                // 私有映射/堆的 lazy 缺页不要在 fork 时补齐。
                // 只对已经存在的物理页建立 COW；未分配页由父子各自在首次访问时处理。
                let was_writable = area.perm().contains(MapPermission::W);
                if was_writable {
                    area.set_cow_flag();
                }
                debug!(
                    "area vpn {:#x}..{:#x}",
                    area.start_vpn().0,
                    area.end_vpn().0
                );

                for (page_index, vpn) in area.data_frames.keys().enumerate() {
                    fork_cow_trace.area_progress(1, page_index, resident_pages);
                    // info!("vpn in dataframes {:#x}", vpn.0);
                    frame_page.push((
                        *vpn,
                        if was_writable {
                            PTEFlags::from(cow_mapping_flags(*area.perm())) | PTEFlags::V
                        } else {
                            PTEFlags::from(MappingFlags::from(*area.perm())) | PTEFlags::V
                        },
                    ));
                }
                fork_cow_trace.area_progress(2, 0, resident_pages);
                let new_area = UserMapArea::from_another(&area);
                let child_flags = if new_area.cow_flag {
                    cow_mapping_flags(new_area.map_perm)
                } else {
                    new_area.map_perm.into()
                };
                fork_cow_trace.area_progress(3, 0, resident_pages);
                for (page_index, (&vpn, frame)) in new_area.data_frames.iter().enumerate() {
                    fork_cow_trace.area_progress(3, page_index, resident_pages);
                    reschedule_large_fork_batch(page_index);
                    vmset.page_table.map_page_no_flush(
                        vpn,
                        frame.ppn,
                        child_flags,
                        MappingSize::Page4KB,
                    );
                }
                vmset.insert_area_sorted(new_area);
                fork_cow_trace.area_progress(4, resident_pages, resident_pages);
            }
        }
        // 直接复制内核预置的用户页：trap context、rt_sigreturn trampoline。
        let direct_clone_count = direct_clone_pages.len();
        for (page_index, vpn) in direct_clone_pages.into_iter().enumerate() {
            fork_cow_trace.progress(4, page_index, direct_clone_count);
            reschedule_large_fork_batch(page_index);
            let Some(src_pte) = user_vmset.page_table.translate(vpn) else {
                error!("fork: missing parent direct-clone pte for vpn {:#x}", vpn.0);
                continue;
            };
            let Some(dst_pte) = vmset.translate(vpn) else {
                error!("fork: missing child direct-clone pte for vpn {:#x}", vpn.0);
                continue;
            };
            dst_pte
                .ppn()
                .get_bytes_array()
                .copy_from_slice(src_pte.ppn().get_bytes_array());
        }
        //设置页表项
        let mut parent_pte_updated = Vec::new();
        let frame_page_count = frame_page.len();
        for (page_index, frame) in frame_page.into_iter().enumerate() {
            fork_cow_trace.progress(5, page_index, frame_page_count);
            reschedule_large_fork_batch(page_index);
            if let Some(pte) = user_vmset.page_table.find_pte(frame.0) {
                if !pte.is_valid() {
                    error!("fork: parent pte not valid for vpn {:#x}", frame.0.0);
                    continue;
                }
                pte.set_flag(frame.1);
                parent_pte_updated.push(frame.0);
            } else {
                error!("fork: missing parent pte for vpn {:#x}", frame.0.0);
            }
        }
        fork_cow_trace.progress(6, parent_pte_updated.len(), parent_pte_updated.len());
        if !parent_pte_updated.is_empty() {
            // fork can be called by one thread while sibling threads execute
            // the same parent address space on other CPUs. Every old writable
            // translation must be invalidated before the VM lock is released;
            // a local flush would let a sibling write through COW protection.
            polyhal::multicore::shootdown_tlb_all(user_vmset.token());
        }
        fork_cow_trace.progress(7, 0, 0);
        vmset
    }

    /// 在用户地址空间找一块没有被占用的虚拟地址区间
    pub fn find_free_area(&self, start: usize, len: usize) -> Option<usize> {
        let page_aligned_len = page_align_up(len)?;
        let mut current_addr = if start == 0 {
            MMAP_BASE
        } else {
            page_align_up(start)?
        };
        let user_end_exclusive = USER_MEMORY_SPACE.1.saturating_add(1);

        for area in self.areas.iter() {
            let area_start = area.start_vpn().0 * PAGE_SIZE;
            let area_end = area.end_vpn().0 * PAGE_SIZE;
            if area_end <= current_addr {
                continue;
            }

            let current_end = current_addr.checked_add(page_aligned_len)?;
            if current_end <= area_start {
                return (current_end <= user_end_exclusive).then_some(current_addr);
            }

            current_addr = page_align_up(area_end)?;
            if current_addr >= user_end_exclusive {
                return None;
            }
        }

        let current_end = current_addr.checked_add(page_aligned_len)?;
        (current_end <= user_end_exclusive).then_some(current_addr)
    }
}

fn page_align_up(addr: usize) -> Option<usize> {
    addr.checked_add(PAGE_SIZE - 1)
        .map(|addr| addr & !(PAGE_SIZE - 1))
}

// impl UserVMSet {
//     // 获取指定范围内的内存区域（不可变引用）
//     pub fn get_areas_in_range(&self, start_va: VirtAddr, end_va: VirtAddr) -> Vec<&UserMapArea> {
//         let mut result = Vec::new();
//         let start = start_va.0;
//         let end = end_va.0;

//         for area in self.areas.iter() {
//             let area_start = area.va_range.start;
//             let area_end = area.va_range.end;

//             // 检查区间是否重叠：[area_start, area_end) 与 [start, end) 有交集
//             if usize::from(area_end) > start && usize::from(area_start) < end {
//                 result.push(area);
//             }
//         }

//         result
//     }

//     // 获取指定范围内的内存区域（可变引用）
//     pub fn get_areas_in_range_mut(
//         &mut self,
//         start_va: VirtAddr,
//         end_va: VirtAddr,
//     ) -> Vec<&mut UserMapArea> {
//         let mut result = Vec::new();
//         let start = start_va.0;
//         let end = end_va.0;

//         // 收集索引避免借用冲突
//         let mut indices = Vec::new();
//         for (i, area) in self.areas.iter().enumerate() {
//             let area_start = area.va_range.start;
//             let area_end = area.va_range.end;

//             if usize::from(area_end) > start && usize::from(area_start) < end {
//                 indices.push(i);
//             }
//         }

//         for i in indices {
//             result.push(&mut self.areas[i]);
//         }

//         result
//     }

//     // 获取完全覆盖指定范围的内存区域
//     pub fn get_areas_covering_range(
//         &self,
//         start_va: VirtAddr,
//         end_va: VirtAddr,
//     ) -> Vec<&UserMapArea> {
//         let mut result = Vec::new();
//         let start = start_va.0;
//         let end = end_va.0;

//         for area in self.areas.iter() {
//             let area_start = area.va_range.start;
//             let area_end = area.va_range.end;

//             // 检查范围是否完全在当前区域内
//             if usize::from(area_end) > start && usize::from(area_start) < end {
//                 result.push(area);
//             }
//         }

//         result
//     }

//     // 检查范围是否完全被内存区域覆盖（可以跨多个区域）
//     pub fn is_range_fully_covered(&self, start_va: VirtAddr, end_va: VirtAddr) -> bool {
//         let start = start_va.0;
//         let end = end_va.0;
//         let mut current = start;

//         // 按起始地址排序
//         let mut sorted_areas: Vec<&UserMapArea> = self.areas.iter().collect();
//         sorted_areas.sort_by_key(|a| a.va_range.start);

//         for area in sorted_areas {
//             let area_start = area.va_range.start;
//             let area_end = area.va_range.end;

//             if usize::from(area_start) <= current && usize::from(area_end) > current {
//                 current = usize::from(area_end);
//                 if current >= end {
//                     return true;
//                 }
//             }
//         }

//         false
//     }
// }
///
pub struct KernelVMSet {
    page_table: PageTable,
    areas: Vec<KernelMapArea>,
}

impl VMSpace for KernelVMSet {
    fn page_table(&self) -> &PageTable {
        &self.page_table
    }

    fn page_table_mut(&mut self) -> &mut PageTable {
        &mut self.page_table
    }

    fn new_bare() -> Self {
        Self {
            page_table: PageTable::new(),
            areas: Vec::new(),
        }
    }
    fn token(&self) -> usize {
        self.page_table.token()
    }

    fn remove_area_with_start_vpn(&mut self, start_vpn: VirtPageNum) {
        drop(self.take_area_with_start_vpn(start_vpn));
    }

    fn activate(&self) {
        // let satp = self.page_table.token();
        // unsafe {
        //     satp::write(satp);
        //     asm!("sfence.vma");
        // }
        warn!("kernel page_table activate");
        self.page_table.change();
        record_active_page_table_token(self.token());
    }
}

impl KernelVMSet {
    /// Unmap and detach one kernel area without dropping its owned frames.
    ///
    /// Callers that hold the global kernel-VM lock must drop the returned area
    /// only after releasing that lock. FrameTracker destruction enters the
    /// frame allocator and must not extend the KernelVMSet critical section.
    pub fn take_area_with_start_vpn(&mut self, start_vpn: VirtPageNum) -> Option<KernelMapArea> {
        let idx = self
            .areas
            .iter()
            .position(|area| area.start_vpn() == start_vpn)?;
        let area = self.areas.swap_remove(idx);
        for vpn in VPNRange::new(area.start_vpn(), area.end_vpn()) {
            self.page_table.unmap_page(vpn);
        }
        Some(area)
    }

    ///
    pub fn recycle_data_pages(&mut self) {
        self.areas.clear();
    }
    ///
    // pub fn init() -> Self {
    //     Self {
    //         page_table: PageTable::init(),
    //         areas: Vec::new(),
    //     }
    // }
    ///
    pub fn insert_framed_area(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
        permission: MapPermission,
        areatype: KernelAreaType,
    ) {
        /*println!("mapping kernel stack");
        println!("  kernel stack top {:#x}", end_va.0);
        println!("  kernel stack bottem {:#x}", start_va.0);*/
        self.push(
            KernelMapArea::new(start_va, end_va, MapType::Framed, permission, areatype),
            None,
        );
    }

    /// Insert a framed kernel area using frames allocated by the caller.
    pub fn insert_framed_area_with_frames(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
        permission: MapPermission,
        areatype: KernelAreaType,
        data_frames: BTreeMap<VirtPageNum, FrameTracker>,
    ) {
        self.push(
            KernelMapArea::with_frames(
                start_va,
                end_va,
                MapType::Framed,
                permission,
                areatype,
                data_frames,
            ),
            None,
        );
    }

    ///
    pub fn push(&mut self, mut map_area: KernelMapArea, data: Option<&[u8]>) {
        map_area.map(&mut self.page_table);
        if let Some(data) = data {
            map_area.copy_data(&self.page_table, data, 0);
        }

        let start_vpn = map_area.start_vpn();
        assert!(
            self.areas
                .iter()
                .all(|existing| existing.start_vpn() != start_vpn),
            "duplicate kernel area start_vpn {:?}",
            start_vpn
        );
        self.areas.push(map_area);
    }

    /// Add an area while constructing a page table that has not been activated.
    fn push_inactive(&mut self, mut map_area: KernelMapArea, data: Option<&[u8]>) {
        map_area.map_no_flush(&mut self.page_table);
        if let Some(data) = data {
            map_area.copy_data(&self.page_table, data, 0);
        }

        let start_vpn = map_area.start_vpn();
        assert!(
            self.areas
                .iter()
                .all(|existing| existing.start_vpn() != start_vpn),
            "duplicate kernel area start_vpn {:?}",
            start_vpn
        );
        self.areas.push(map_area);
    }

    fn prepare_kernel_stack_page_tables(&mut self) {
        for kstack_id in 0..MAX_THREAD_NUM {
            let top =
                KERNEL_THREAD_STACK_BASE - (kstack_id + 1) * (KERNEL_STACK_SIZE + PAGE_SIZE) + 1;
            let bottom = top - KERNEL_STACK_SIZE;
            let start_vpn = VirtAddr::from(bottom).floor();
            let end_vpn = VirtAddr::from(top).ceil();
            for vpn in VPNRange::new(start_vpn, end_vpn) {
                self.page_table.ensure_pte_create(vpn);
            }
        }
    }

    #[cfg(target_arch = "riscv64")]
    ///
    pub fn new() -> Self {
        let mut kvm_set = Self::new_bare();
        // map kernel sections

        polyhal::println!("map kernel sections");
        polyhal::println!(".text [{:#x}, {:#x})", stext as usize, etext as usize);
        polyhal::println!(".rodata [{:#x}, {:#x})", srodata as usize, erodata as usize);
        polyhal::println!(".data [{:#x}, {:#x})", sdata as usize, edata as usize);
        polyhal::println!(".bss [{:#x}, {:#x})", _sbss as usize, _ebss as usize);
        polyhal::println!("mapping .text section");
        // println!("start va {:#x}, end_va {:#x}", stext as usize, etext as usize);

        kvm_set.push_inactive(
            KernelMapArea::new(
                (stext as usize).into(),
                (etext as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::X,
                KernelAreaType::Text,
            ),
            None,
        );
        polyhal::println!("mapping .rodata section");
        // println!("start va {:#x}, end_va {:#x}", srodata as usize, erodata as usize);

        kvm_set.push_inactive(
            KernelMapArea::new(
                (srodata as usize).into(),
                (erodata as usize).into(),
                MapType::Identical,
                MapPermission::R,
                KernelAreaType::Rodata,
            ),
            None,
        );
        polyhal::println!("mapping .data section");
        // println!("start va {:#x}, end_va {:#x}", sdata as usize, edata as usize);
        kvm_set.push_inactive(
            KernelMapArea::new(
                (sdata as usize).into(),
                (edata as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
                KernelAreaType::Data,
            ),
            None,
        );
        let vpn = VirtAddr::from(sdata as usize).floor();
        if let Some(pte) = kvm_set.page_table.translate(vpn) {
            polyhal::println!(
                "  Mapped: PPN={:#x}, flags={:?}",
                pte.ppn().0 << 12,
                pte.flags()
            );
        } else {
            log::error!("  ERROR: MMIO not mapped!");
        }
        polyhal::println!("mapping .bss section");
        polyhal::println!(
            "start va {:#x}, end_va {:#x}",
            _sbss as usize,
            _ebss as usize
        );

        kvm_set.push_inactive(
            KernelMapArea::new(
                (_sbss as usize).into(),
                (_ebss as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
                KernelAreaType::Bss,
            ),
            None,
        );
        println!("mapping allocatable physical memory");
        for_each_physical_memory_region(0, |start, end| {
            println!(
                "start_va {:#x}, end_va {:#x}",
                start + VIRT_ADDR_START,
                end + VIRT_ADDR_START
            );
            kvm_set.push_inactive(
                KernelMapArea::new(
                    (start + VIRT_ADDR_START).into(),
                    (end + VIRT_ADDR_START).into(),
                    MapType::Identical,
                    MapPermission::R | MapPermission::W,
                    KernelAreaType::PhysMem,
                ),
                None,
            );
        });
        polyhal::println!("mapping memory-mapped registers");
        for pair in MMIO {
            polyhal::println!(
                "start_va {:#x} end_va {:#x}",
                (*pair).0,
                (*pair).0 + (*pair).1
            );
            kvm_set.push_inactive(
                KernelMapArea::new(
                    ((*pair).0 + VIRT_ADDR_START).into(),
                    (((*pair).0 + (*pair).1) + VIRT_ADDR_START).into(),
                    MapType::Identical,
                    MapPermission::R
                        | MapPermission::W
                        | MapPermission::G
                        | MapPermission::MAT_NOCACHE,
                    KernelAreaType::MemMappedReg,
                ),
                None,
            );
            // let start_virt = (*pair).0 + VIRT_ADDR_START;

            // let vpn = VirtAddr::from(start_virt).floor();

            // if let Some(pte) = kvm_set.page_table.translate(vpn) {
            //     println!("MMIO {:#x}: PPN={:#x}, flags={:?}", pair.0, pte.ppn().0, pte.flags());
            //     // 检查是否可以访问
            //     unsafe {
            //         let ptr = start_virt as *const u32;
            //         let magic = ptr.read_volatile();
            //         println!("  Magic at {:#x}: {:#x}", start_virt, magic);
            //     }
            // } else {
            //     println!("MMIO {}: NOT MAPPED!", pair.0);
            // }
        }
        kvm_set.prepare_kernel_stack_page_tables();
        KERNEL_PAGE_TABLE_TOKEN.store(kvm_set.page_table.token(), Ordering::Release);
        kvm_set.page_table.change();
        record_active_page_table_token(kvm_set.page_table.token());
        polyhal::println!("map over");

        kvm_set
    }
    #[cfg(target_arch = "loongarch64")]
    ///
    pub fn new() -> Self {
        let mut kvm_set = Self::new_bare();

        polyhal::println!("map loongarch64 kernel sections");
        polyhal::println!(".text [{:#x}, {:#x})", stext as usize, etext as usize);
        kvm_set.push_inactive(
            KernelMapArea::new(
                (stext as usize).into(),
                (etext as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::X,
                KernelAreaType::Text,
            ),
            None,
        );

        polyhal::println!(".rodata [{:#x}, {:#x})", srodata as usize, erodata as usize);
        kvm_set.push_inactive(
            KernelMapArea::new(
                (srodata as usize).into(),
                (erodata as usize).into(),
                MapType::Identical,
                MapPermission::R,
                KernelAreaType::Rodata,
            ),
            None,
        );

        polyhal::println!(".data [{:#x}, {:#x})", sdata as usize, edata as usize);
        kvm_set.push_inactive(
            KernelMapArea::new(
                (sdata as usize).into(),
                (edata as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
                KernelAreaType::Data,
            ),
            None,
        );

        polyhal::println!(".bss [{:#x}, {:#x})", _sbss as usize, _ebss as usize);
        kvm_set.push_inactive(
            KernelMapArea::new(
                (_sbss as usize).into(),
                (_ebss as usize).into(),
                MapType::Identical,
                MapPermission::R | MapPermission::W,
                KernelAreaType::Bss,
            ),
            None,
        );

        polyhal::println!("mapping loongarch64 physical memory");
        let kernel_phys_end = ekernel as usize - VIRT_ADDR_START;
        for &(start, size) in polyhal::mem::get_mem_areas() {
            let end = start + size;
            let start = start.max(kernel_phys_end);
            if start >= end {
                continue;
            }
            polyhal::println!(
                "start_va {:#x}, end_va {:#x}",
                start + VIRT_ADDR_START,
                end + VIRT_ADDR_START
            );
            kvm_set.push_inactive(
                KernelMapArea::new(
                    (start + VIRT_ADDR_START).into(),
                    (end + VIRT_ADDR_START).into(),
                    MapType::Identical,
                    MapPermission::R | MapPermission::W,
                    KernelAreaType::PhysMem,
                ),
                None,
            );
        }

        polyhal::println!("mapping loongarch64 memory-mapped registers");
        for pair in MMIO {
            polyhal::println!("start_va {:#x} end_va {:#x}", pair.0, pair.0 + pair.1);
            kvm_set.push_inactive(
                KernelMapArea::new(
                    (pair.0 + VIRT_ADDR_START).into(),
                    (pair.0 + pair.1 + VIRT_ADDR_START).into(),
                    MapType::Identical,
                    MapPermission::R
                        | MapPermission::W
                        | MapPermission::G
                        | MapPermission::MAT_NOCACHE,
                    KernelAreaType::MemMappedReg,
                ),
                None,
            );
        }

        kvm_set.prepare_kernel_stack_page_tables();
        KERNEL_PAGE_TABLE_TOKEN.store(kvm_set.page_table.token(), Ordering::Release);
        kvm_set.page_table.change();
        record_active_page_table_token(kvm_set.page_table.token());
        polyhal::println!("loongarch64 kernel map over");
        kvm_set
    }
}

#[allow(missing_docs, unused)]
pub fn remap_test() {
    let mut kernel_space = KERNEL_VMSET.lock();
    let mid_text: VirtAddr = (stext as usize + ((etext as usize - stext as usize) >> 1)).into();
    let mid_rodata: VirtAddr =
        (srodata as usize + ((erodata as usize - srodata as usize) >> 1)).into();
    let mid_data: VirtAddr = (sdata as usize + ((edata as usize - sdata as usize) >> 1)).into();
    assert!(
        !kernel_space
            .page_table
            .translate(mid_text.floor())
            .unwrap()
            .writable(),
    );
    assert!(
        !kernel_space
            .page_table
            .translate(mid_rodata.floor())
            .unwrap()
            .writable(),
    );
    assert!(
        !kernel_space
            .page_table
            .translate(mid_data.floor())
            .unwrap()
            .executable(),
    );
    polyhal::println!("remap_test passed!");
}
///
pub fn user_stack_top() -> usize {
    USER_MEMORY_SPACE.1 - PAGE_SIZE
}
///
pub fn user_stack_bottom() -> usize {
    user_stack_top() - USER_STACK_SIZE
}
