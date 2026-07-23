use alloc::alloc::{alloc, dealloc, Layout};
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
