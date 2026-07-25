use alloc::alloc::{Layout, alloc, dealloc};
use alloc::string::String;
use core::cmp::min;
use core::ffi::{c_char, c_int, c_size_t, c_void};
use core::ptr::{copy_nonoverlapping, null_mut, read_unaligned, write_unaligned};
use core::sync::atomic::{AtomicUsize, Ordering};

static LWEXT4_ALLOC_CURRENT_USER: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_ALLOC_CURRENT_ACTUAL: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_ALLOC_PEAK_USER: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_ALLOC_PEAK_ACTUAL: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_FREE_COUNT: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_ALLOC_SEQUENCE: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_ALLOC_TRACK_OVERFLOWS: AtomicUsize = AtomicUsize::new(0);

const LIVE_ALLOCATION_SLOT_COUNT: usize = 2048;
// The provenance table exists to diagnose block-I/O buffers. Tracking every
// tiny pathname and journal bookkeeping allocation makes a bounded table
// useless during metadata-heavy workloads and turns overflow into an O(n)
// scan on every allocation. All ext4 physical/logical block buffers are at
// least one sector, so exclude smaller allocations from this table while the
// allocator's full header/footer validation remains active for every object.
const LIVE_ALLOCATION_MIN_SIZE: usize = 512;
const LIVE_ALLOCATION_TOMBSTONE: usize = 1;
const LIVE_ALLOCATION_RESERVED: usize = usize::MAX;

struct LiveAllocationSlot {
    pointer: AtomicUsize,
    size: AtomicUsize,
    allocation_id: AtomicUsize,
    allocation_site: AtomicUsize,
}

impl LiveAllocationSlot {
    const fn new() -> Self {
        Self {
            pointer: AtomicUsize::new(0),
            size: AtomicUsize::new(0),
            allocation_id: AtomicUsize::new(0),
            allocation_site: AtomicUsize::new(0),
        }
    }
}

static LIVE_ALLOCATIONS: [LiveAllocationSlot; LIVE_ALLOCATION_SLOT_COUNT] =
    [const { LiveAllocationSlot::new() }; LIVE_ALLOCATION_SLOT_COUNT];

/// Lock-free provenance for an exact pointer returned by the lwext4 allocator.
#[derive(Clone, Copy, Debug)]
pub struct Lwext4AllocationPointerInfo {
    /// Whether the pointer is still registered as a live allocation.
    pub live: bool,
    /// Requested allocation size, valid when `live` is true.
    pub size: usize,
    /// Monotonic allocation identity, valid when `live` is true.
    pub allocation_id: usize,
    /// C return address which requested the allocation.
    pub allocation_site: usize,
    /// Number of allocations which could not be represented in the table.
    pub tracking_overflows: usize,
}

fn live_allocation_hash(pointer: usize) -> usize {
    (pointer >> MALLOC_ALIGN.trailing_zeros()).wrapping_mul(0x9e37_79b9)
        % LIVE_ALLOCATION_SLOT_COUNT
}

fn register_live_allocation(pointer: usize, size: usize, allocation_id: usize, site: usize) {
    if size < LIVE_ALLOCATION_MIN_SIZE {
        return;
    }
    let start = live_allocation_hash(pointer);
    for offset in 0..LIVE_ALLOCATION_SLOT_COUNT {
        let slot = &LIVE_ALLOCATIONS[(start + offset) % LIVE_ALLOCATION_SLOT_COUNT];
        let observed = slot.pointer.load(Ordering::Acquire);
        if observed == pointer {
            error!(
                "[LWEXT4_ALLOC_TRACK_DUPLICATE] ptr={:#x} size={} allocation_id={} site={:#x}",
                pointer, size, allocation_id, site,
            );
            return;
        }
        if observed != 0 && observed != LIVE_ALLOCATION_TOMBSTONE {
            continue;
        }
        if slot
            .pointer
            .compare_exchange(
                observed,
                LIVE_ALLOCATION_RESERVED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            continue;
        }
        slot.size.store(size, Ordering::Relaxed);
        slot.allocation_id.store(allocation_id, Ordering::Relaxed);
        slot.allocation_site.store(site, Ordering::Relaxed);
        slot.pointer.store(pointer, Ordering::Release);
        return;
    }
    let overflows = LWEXT4_ALLOC_TRACK_OVERFLOWS.fetch_add(1, Ordering::Relaxed) + 1;
    error!(
        "[LWEXT4_ALLOC_TRACK_OVERFLOW] ptr={:#x} size={} allocation_id={} site={:#x} overflows={}",
        pointer, size, allocation_id, site, overflows,
    );
}

fn unregister_live_allocation(pointer: usize, size: usize, allocation_id: usize) {
    if size < LIVE_ALLOCATION_MIN_SIZE {
        return;
    }
    let start = live_allocation_hash(pointer);
    for offset in 0..LIVE_ALLOCATION_SLOT_COUNT {
        let slot = &LIVE_ALLOCATIONS[(start + offset) % LIVE_ALLOCATION_SLOT_COUNT];
        let observed = slot.pointer.load(Ordering::Acquire);
        if observed == 0 {
            break;
        }
        if observed != pointer {
            continue;
        }
        let tracked_id = slot.allocation_id.load(Ordering::Relaxed);
        if tracked_id != allocation_id {
            error!(
                "[LWEXT4_ALLOC_TRACK_ID_MISMATCH] ptr={:#x} allocation_id={} tracked_id={}",
                pointer, allocation_id, tracked_id,
            );
        }
        slot.pointer
            .store(LIVE_ALLOCATION_TOMBSTONE, Ordering::Release);
        return;
    }
    error!(
        "[LWEXT4_ALLOC_TRACK_MISSING] ptr={:#x} allocation_id={}",
        pointer, allocation_id,
    );
}

/// Return allocation provenance without dereferencing `pointer`.
pub fn allocation_pointer_info(pointer: usize) -> Lwext4AllocationPointerInfo {
    let start = live_allocation_hash(pointer);
    for offset in 0..LIVE_ALLOCATION_SLOT_COUNT {
        let slot = &LIVE_ALLOCATIONS[(start + offset) % LIVE_ALLOCATION_SLOT_COUNT];
        let observed = slot.pointer.load(Ordering::Acquire);
        if observed == 0 {
            break;
        }
        if observed == pointer {
            return Lwext4AllocationPointerInfo {
                live: true,
                size: slot.size.load(Ordering::Relaxed),
                allocation_id: slot.allocation_id.load(Ordering::Relaxed),
                allocation_site: slot.allocation_site.load(Ordering::Relaxed),
                tracking_overflows: LWEXT4_ALLOC_TRACK_OVERFLOWS.load(Ordering::Relaxed),
            };
        }
    }
    Lwext4AllocationPointerInfo {
        live: false,
        size: 0,
        allocation_id: 0,
        allocation_site: 0,
        tracking_overflows: LWEXT4_ALLOC_TRACK_OVERFLOWS.load(Ordering::Relaxed),
    }
}

const MALLOC_ALIGN: usize = 16;
const LIVE_MAGIC: usize = 0x4c57_4558_5434_4d41;
const FREED_MAGIC: usize = 0x4652_4545_4434_4d41;
const HEADER_TAIL_MAGIC: usize = 0x4845_4144_5441_494c;
const FOOTER_MAGIC: usize = 0x4c57_4558_5445_4e44;

/// Snapshot of C-side lwext4 allocations served by this libc shim.
#[derive(Clone, Copy, Debug)]
pub struct Lwext4AllocStats {
    /// Bytes requested by live allocations.
    pub current_user: usize,
    /// Bytes currently charged including allocator metadata.
    pub current_actual: usize,
    /// Peak requested live bytes.
    pub peak_user: usize,
    /// Peak actual live bytes.
    pub peak_actual: usize,
    /// Successful allocation calls.
    pub alloc_count: usize,
    /// Successful deallocation calls.
    pub free_count: usize,
}

/// Return current lwext4 C allocation accounting.
pub fn allocation_stats() -> Lwext4AllocStats {
    Lwext4AllocStats {
        current_user: LWEXT4_ALLOC_CURRENT_USER.load(Ordering::Relaxed),
        current_actual: LWEXT4_ALLOC_CURRENT_ACTUAL.load(Ordering::Relaxed),
        peak_user: LWEXT4_ALLOC_PEAK_USER.load(Ordering::Relaxed),
        peak_actual: LWEXT4_ALLOC_PEAK_ACTUAL.load(Ordering::Relaxed),
        alloc_count: LWEXT4_ALLOC_COUNT.load(Ordering::Relaxed),
        free_count: LWEXT4_FREE_COUNT.load(Ordering::Relaxed),
    }
}

fn raise_peak(peak: &AtomicUsize, value: usize) {
    let mut current = peak.load(Ordering::Relaxed);
    while value > current {
        match peak.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

#[cfg(feature = "print")]
#[linkage = "weak"]
#[no_mangle]
unsafe extern "C" fn printf(str: *const c_char, mut args: ...) -> c_int {
    // extern "C" { pub fn printf(arg1: *const c_char, ...) -> c_int; }
    use printf_compat::{format, output};

    let mut s = String::new();
    let bytes_written = format(str as _, args.as_va_list(), output::fmt_write(&mut s));
    //println!("{}", s);
    info!("{}", s);

    bytes_written
}

#[cfg(not(feature = "print"))]
#[linkage = "weak"]
#[no_mangle]
unsafe extern "C" fn printf(str: *const c_char, mut args: ...) -> c_int {
    use core::ffi::CStr;
    let c_str = unsafe { CStr::from_ptr(str) };
    //let arg1 = args.arg::<usize>();

    info!("[lwext4] {:?}", c_str);
    0
}

#[no_mangle]
pub extern "C" fn ext4_user_malloc(size: c_size_t) -> *mut c_void {
    malloc_with_site(size, 0)
}

#[no_mangle]
pub extern "C" fn ext4_user_malloc_site(size: c_size_t, site: usize) -> *mut c_void {
    malloc_with_site(size, site)
}

#[linkage = "weak"]
#[no_mangle]
pub extern "C" fn calloc(m: c_size_t, n: c_size_t) -> *mut c_void {
    calloc_with_site(m, n, 0)
}

#[no_mangle]
pub extern "C" fn ext4_user_calloc(m: c_size_t, n: c_size_t) -> *mut c_void {
    calloc_with_site(m, n, 0)
}

#[no_mangle]
pub extern "C" fn ext4_user_calloc_site(m: c_size_t, n: c_size_t, site: usize) -> *mut c_void {
    calloc_with_site(m, n, site)
}

fn calloc_with_site(m: c_size_t, n: c_size_t, site: usize) -> *mut c_void {
    let Some(size) = m.checked_mul(n) else {
        error!(
            "[LWEXT4_ALLOC_OVERFLOW] operation=calloc count={} element_size={} site={:#x}",
            m, n, site,
        );
        return null_mut();
    };
    let mem = malloc_with_site(size, site);
    if mem.is_null() {
        return mem;
    }

    extern "C" {
        pub fn memset(dest: *mut c_void, c: c_int, n: c_size_t) -> *mut c_void;
    }
    unsafe { memset(mem, 0, size) }
}

#[linkage = "weak"]
#[no_mangle]
pub extern "C" fn realloc(memblock: *mut c_void, size: c_size_t) -> *mut c_void {
    realloc_with_site(memblock, size, 0)
}

#[no_mangle]
pub extern "C" fn ext4_user_realloc(memblock: *mut c_void, size: c_size_t) -> *mut c_void {
    realloc_with_site(memblock, size, 0)
}

#[no_mangle]
pub extern "C" fn ext4_user_realloc_site(
    memblock: *mut c_void,
    size: c_size_t,
    site: usize,
) -> *mut c_void {
    realloc_with_site(memblock, size, site)
}

fn realloc_with_site(memblock: *mut c_void, size: c_size_t, site: usize) -> *mut c_void {
    if memblock.is_null() {
        warn!("realloc a a null mem pointer");
        return malloc_with_site(size, site);
    }

    let (_, header, _, _) = unsafe { validate_allocation(memblock, site, "realloc") };
    let old_size = header.size;
    info!("realloc from {} to {}", old_size, size);

    let mem = malloc_with_site(size, site);
    if mem.is_null() {
        return null_mut();
    }

    unsafe {
        copy_nonoverlapping(memblock.cast::<u8>(), mem.cast::<u8>(), min(size, old_size));
    }
    free_with_site(memblock, site);

    mem
}

#[no_mangle]
pub extern "C" fn ext4_user_free(p: *mut c_void) {
    free_with_site(p, 0)
}

#[no_mangle]
pub extern "C" fn ext4_user_free_site(p: *mut c_void, site: usize) {
    free_with_site(p, site)
}

/// Return the immutable allocation identity stored in the allocator header.
///
/// This is called by lwext4 immediately after allocating a block-cache data
/// buffer, while the pointer is known to be live. It lets the C descriptor
/// retain provenance without dereferencing a possibly corrupted data pointer
/// later in the writeback path.
#[no_mangle]
pub extern "C" fn ext4_user_allocation_id(p: *mut c_void) -> usize {
    if p.is_null() {
        return 0;
    }
    let (_, header, _, _) = unsafe { validate_allocation(p, 0, "allocation_id") };
    header.allocation_id
}

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct MemoryControlBlock {
    base_magic: usize,
    size: usize,
    size_check: usize,
    allocation_id: usize,
    allocation_site: usize,
    tail_magic: usize,
}
const CTRL_BLK_SIZE: usize = core::mem::size_of::<MemoryControlBlock>();
const FOOTER_SIZE: usize = core::mem::size_of::<usize>();

const _: () = assert!(CTRL_BLK_SIZE % MALLOC_ALIGN == 0);

fn allocation_layout(size: usize) -> Option<(usize, Layout)> {
    let actual_size = CTRL_BLK_SIZE.checked_add(size)?.checked_add(FOOTER_SIZE)?;
    let layout = Layout::from_size_align(actual_size, MALLOC_ALIGN).ok()?;
    Some((actual_size, layout))
}

fn allocation_magic(header: *const MemoryControlBlock) -> usize {
    LIVE_MAGIC ^ header as usize
}

fn freed_magic(header: *const MemoryControlBlock) -> usize {
    FREED_MAGIC ^ header as usize
}

fn header_tail_magic(header: *const MemoryControlBlock, base_magic: usize) -> usize {
    HEADER_TAIL_MAGIC ^ (header as usize).rotate_left(11) ^ base_magic.rotate_left(23)
}

fn footer_magic(user: *const u8, header: MemoryControlBlock) -> usize {
    FOOTER_MAGIC
        ^ (user as usize).rotate_left(17)
        ^ header.size
        ^ header.allocation_id.rotate_left(7)
}

unsafe fn validate_allocation(
    ptr: *mut c_void,
    free_site: usize,
    operation: &'static str,
) -> (*mut MemoryControlBlock, MemoryControlBlock, usize, Layout) {
    let user_addr = ptr as usize;
    if user_addr % MALLOC_ALIGN != 0 || user_addr < CTRL_BLK_SIZE {
        error!(
            "[LWEXT4_ALLOC_CORRUPTION] operation={} ptr={:#x} free_site={:#x} reason=unaligned_or_low_pointer align={}",
            operation, user_addr, free_site, MALLOC_ALIGN,
        );
        panic!("lwext4 allocator received an invalid pointer");
    }

    let header_ptr = ptr.cast::<MemoryControlBlock>().sub(1);
    let header = header_ptr.read();
    let live_magic = allocation_magic(header_ptr);
    let live_tail_magic = header_tail_magic(header_ptr, live_magic);
    let freed_magic = freed_magic(header_ptr);
    let was_freed = header.base_magic == freed_magic
        || header.tail_magic == header_tail_magic(header_ptr, freed_magic);
    let layout = allocation_layout(header.size);
    if header.base_magic != live_magic
        || header.tail_magic != live_tail_magic
        || header.size_check != !header.size
        || layout.is_none()
    {
        error!(
            "[LWEXT4_ALLOC_CORRUPTION] operation={} ptr={:#x} header={:#x} free_site={:#x} base_magic={:#x} expected_base_magic={:#x} tail_magic={:#x} expected_tail_magic={:#x} was_freed={} size={} size_check={:#x} allocation_id={} allocation_site={:#x} alloc_count={} free_count={}",
            operation,
            user_addr,
            header_ptr as usize,
            free_site,
            header.base_magic,
            live_magic,
            header.tail_magic,
            live_tail_magic,
            was_freed,
            header.size,
            header.size_check,
            header.allocation_id,
            header.allocation_site,
            LWEXT4_ALLOC_COUNT.load(Ordering::Relaxed),
            LWEXT4_FREE_COUNT.load(Ordering::Relaxed),
        );
        panic!("lwext4 allocation header is corrupt");
    }

    let (actual_size, layout) = match layout {
        Some(layout) => layout,
        None => unreachable!("invalid lwext4 allocation layout passed validation"),
    };
    let footer_ptr = ptr.cast::<u8>().add(header.size).cast::<usize>();
    let footer = read_unaligned(footer_ptr);
    let expected_footer = footer_magic(ptr.cast(), header);
    if footer != expected_footer {
        error!(
            "[LWEXT4_ALLOC_CORRUPTION] operation={} ptr={:#x} header={:#x} free_site={:#x} reason=tail_overwrite size={} footer={:#x} expected_footer={:#x} allocation_id={} allocation_site={:#x}",
            operation,
            user_addr,
            header_ptr as usize,
            free_site,
            header.size,
            footer,
            expected_footer,
            header.allocation_id,
            header.allocation_site,
        );
        panic!("lwext4 allocation tail is corrupt");
    }

    (header_ptr, header, actual_size, layout)
}

/// Allocate size bytes memory and return the memory address.
#[linkage = "weak"]
#[no_mangle]
pub extern "C" fn malloc(size: c_size_t) -> *mut c_void {
    malloc_with_site(size, 0)
}

fn malloc_with_site(size: c_size_t, site: usize) -> *mut c_void {
    let Some((actual_size, layout)) = allocation_layout(size) else {
        error!(
            "[LWEXT4_ALLOC_OVERFLOW] operation=malloc size={} site={:#x}",
            size, site,
        );
        return null_mut();
    };
    unsafe {
        let ptr = alloc(layout);
        if ptr.is_null() {
            error!(
                "[LWEXT4_ALLOC_FAILURE] size={} actual_size={} align={} site={:#x}",
                size, actual_size, MALLOC_ALIGN, site,
            );
            return null_mut();
        }
        //debug!("malloc {}@{:p}", size + CTRL_BLK_SIZE, ptr);

        let ptr = ptr.cast::<MemoryControlBlock>();
        let allocation_id = LWEXT4_ALLOC_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1;
        let base_magic = allocation_magic(ptr);
        let header = MemoryControlBlock {
            base_magic,
            size,
            size_check: !size,
            allocation_id,
            allocation_site: site,
            tail_magic: header_tail_magic(ptr, base_magic),
        };
        ptr.write(header);
        let user = ptr.add(1).cast::<u8>();
        write_unaligned(user.add(size).cast::<usize>(), footer_magic(user, header));
        LWEXT4_ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        let current_user = LWEXT4_ALLOC_CURRENT_USER.fetch_add(size, Ordering::Relaxed) + size;
        let current_actual =
            LWEXT4_ALLOC_CURRENT_ACTUAL.fetch_add(actual_size, Ordering::Relaxed) + actual_size;
        raise_peak(&LWEXT4_ALLOC_PEAK_USER, current_user);
        raise_peak(&LWEXT4_ALLOC_PEAK_ACTUAL, current_actual);
        register_live_allocation(user as usize, size, allocation_id, site);
        user.cast()
    }
}

/// Deallocate memory at ptr address
#[linkage = "weak"]
#[no_mangle]
pub extern "C" fn free(ptr: *mut c_void) {
    free_with_site(ptr, 0)
}

fn free_with_site(ptr: *mut c_void, site: usize) {
    if ptr.is_null() {
        warn!("free a null pointer !");
        return;
    }
    //debug!("free pointer {:p}", ptr);

    unsafe {
        let (header_ptr, header, actual_size, layout) = validate_allocation(ptr, site, "free");
        unregister_live_allocation(ptr as usize, header.size, header.allocation_id);
        let freed_magic = freed_magic(header_ptr);
        header_ptr.write(MemoryControlBlock {
            base_magic: freed_magic,
            tail_magic: header_tail_magic(header_ptr, freed_magic),
            ..header
        });
        LWEXT4_FREE_COUNT.fetch_add(1, Ordering::Relaxed);
        LWEXT4_ALLOC_CURRENT_USER.fetch_sub(header.size, Ordering::Relaxed);
        LWEXT4_ALLOC_CURRENT_ACTUAL.fetch_sub(actual_size, Ordering::Relaxed);
        dealloc(header_ptr.cast(), layout)
    }
}
