use alloc::alloc::{alloc, dealloc, Layout};
use alloc::slice::from_raw_parts_mut;
use alloc::string::String;
use core::cmp::min;
use core::ffi::{c_char, c_int, c_size_t, c_void};
use core::sync::atomic::{AtomicUsize, Ordering};

static LWEXT4_ALLOC_CURRENT_USER: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_ALLOC_CURRENT_ACTUAL: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_ALLOC_PEAK_USER: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_ALLOC_PEAK_ACTUAL: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static LWEXT4_FREE_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Snapshot of C-side lwext4 allocations served by this libc shim.
#[derive(Clone, Copy, Debug)]
pub struct Lwext4AllocStats {
    /// Bytes requested by live allocations.
    pub current_user: usize,
    /// Bytes currently charged including allocation headers.
    pub current_actual: usize,
    /// Peak requested live bytes.
    pub peak_user: usize,
    /// Peak actual live bytes.
    pub peak_actual: usize,
    /// Successful allocation calls.
    pub alloc_count: usize,
    /// Free calls for non-null pointers.
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
    malloc(size)
}

#[linkage = "weak"]
#[no_mangle]
pub extern "C" fn calloc(m: c_size_t, n: c_size_t) -> *mut c_void {
    let mem = malloc(m * n);

    extern "C" {
        pub fn memset(dest: *mut c_void, c: c_int, n: c_size_t) -> *mut c_void;
    }
    unsafe { memset(mem, 0, m * n) }
}

#[linkage = "weak"]
#[no_mangle]
pub extern "C" fn realloc(memblock: *mut c_void, size: c_size_t) -> *mut c_void {
    if memblock.is_null() {
        warn!("realloc a a null mem pointer");
        return malloc(size);
    }

    let ptr = memblock.cast::<MemoryControlBlock>();
    let old_size = unsafe { ptr.sub(1).read().size };
    info!("realloc from {} to {}", old_size, size);

    let mem = malloc(size);

    unsafe {
        let old_size = min(size, old_size);
        let mbuf = from_raw_parts_mut(mem as *mut u8, old_size);
        mbuf.copy_from_slice(from_raw_parts_mut(memblock as *mut u8, old_size));
    }
    free(memblock);

    mem
}

#[no_mangle]
pub extern "C" fn ext4_user_free(p: *mut c_void) {
    free(p)
}

struct MemoryControlBlock {
    size: usize,
}
const CTRL_BLK_SIZE: usize = core::mem::size_of::<MemoryControlBlock>();

/// Allocate size bytes memory and return the memory address.
#[linkage = "weak"]
#[no_mangle]
pub extern "C" fn malloc(size: c_size_t) -> *mut c_void {
    // Allocate `(actual length) + 8`. The lowest 8 Bytes are stored in the actual allocated space size.
    let actual_size = size + CTRL_BLK_SIZE;
    let layout = Layout::from_size_align(actual_size, 8).unwrap();
    unsafe {
        let ptr = alloc(layout);
        assert!(!ptr.is_null(), "malloc failed");
        //debug!("malloc {}@{:p}", size + CTRL_BLK_SIZE, ptr);

        let ptr = ptr.cast::<MemoryControlBlock>();
        ptr.write(MemoryControlBlock { size });
        LWEXT4_ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        let current_user = LWEXT4_ALLOC_CURRENT_USER.fetch_add(size, Ordering::Relaxed) + size;
        let current_actual =
            LWEXT4_ALLOC_CURRENT_ACTUAL.fetch_add(actual_size, Ordering::Relaxed) + actual_size;
        raise_peak(&LWEXT4_ALLOC_PEAK_USER, current_user);
        raise_peak(&LWEXT4_ALLOC_PEAK_ACTUAL, current_actual);
        ptr.add(1).cast()
    }
}

/// Deallocate memory at ptr address
#[linkage = "weak"]
#[no_mangle]
pub extern "C" fn free(ptr: *mut c_void) {
    if ptr.is_null() {
        warn!("free a null pointer !");
        return;
    }
    //debug!("free pointer {:p}", ptr);

    let ptr = ptr.cast::<MemoryControlBlock>();
    assert!(ptr as usize > CTRL_BLK_SIZE, "free a null pointer"); // ?
    unsafe {
        let ptr = ptr.sub(1);
        let size = ptr.read().size;
        let actual_size = size + CTRL_BLK_SIZE;
        let layout = Layout::from_size_align(actual_size, 8).unwrap();
        LWEXT4_FREE_COUNT.fetch_add(1, Ordering::Relaxed);
        LWEXT4_ALLOC_CURRENT_USER.fetch_sub(size, Ordering::Relaxed);
        LWEXT4_ALLOC_CURRENT_ACTUAL.fetch_sub(actual_size, Ordering::Relaxed);
        dealloc(ptr.cast(), layout)
    }
}
