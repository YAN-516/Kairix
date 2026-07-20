#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use core::arch::asm;

const SYS_RSEQ: usize = 293;
#[cfg(target_arch = "riscv64")]
const SYS_RISCV_HWPROBE: usize = 258;
const RSEQ_FLAG_UNREGISTER: usize = 1;
const RSEQ_SIGNATURE: u32 = 0x5305_3053;

#[repr(C, align(32))]
struct Rseq {
    cpu_id_start: u32,
    cpu_id: u32,
    rseq_cs: u64,
    flags: u32,
    node_id: u32,
    mm_cid: u32,
    padding: u32,
}

#[cfg(target_arch = "riscv64")]
#[repr(C)]
#[derive(Clone, Copy)]
struct HwprobePair {
    key: i64,
    value: u64,
}

#[cfg(target_arch = "riscv64")]
#[inline(always)]
unsafe fn raw_syscall(id: usize, args: [usize; 6]) -> isize {
    let ret: isize;
    unsafe {
        asm!(
            "ecall",
            inlateout("x10") args[0] => ret,
            in("x11") args[1],
            in("x12") args[2],
            in("x13") args[3],
            in("x14") args[4],
            in("x15") args[5],
            in("x17") id,
        );
    }
    ret
}

#[cfg(target_arch = "loongarch64")]
#[inline(always)]
unsafe fn raw_syscall(id: usize, args: [usize; 6]) -> isize {
    let ret: isize;
    unsafe {
        asm!(
            "syscall 0",
            inlateout("$a0") args[0] => ret,
            in("$a1") args[1],
            in("$a2") args[2],
            in("$a3") args[3],
            in("$a4") args[4],
            in("$a5") args[5],
            in("$a7") id,
        );
    }
    ret
}

fn test_rseq() -> bool {
    let mut area = Rseq {
        cpu_id_start: 0,
        cpu_id: 0,
        rseq_cs: u64::MAX,
        flags: u32::MAX,
        node_id: u32::MAX,
        mm_cid: u32::MAX,
        padding: 0,
    };
    let address = &mut area as *mut Rseq as usize;
    let len = core::mem::size_of::<Rseq>();
    let register =
        unsafe { raw_syscall(SYS_RSEQ, [address, len, 0, RSEQ_SIGNATURE as usize, 0, 0]) };
    println!(
        "[rseq_hwprobe_test] rseq register={} cpu_start={} cpu={} node={} mm_cid={}",
        register, area.cpu_id_start, area.cpu_id, area.node_id, area.mm_cid
    );
    if register != 0
        || area.cpu_id_start != area.cpu_id
        || area.cpu_id == u32::MAX
        || area.cpu_id >= 8
        || area.node_id != 0
        || area.mm_cid != area.cpu_id
        || area.rseq_cs != 0
        || area.flags != 0
    {
        return false;
    }

    let duplicate =
        unsafe { raw_syscall(SYS_RSEQ, [address, len, 0, RSEQ_SIGNATURE as usize, 0, 0]) };
    let wrong_signature = unsafe {
        raw_syscall(SYS_RSEQ, [
            address,
            len,
            RSEQ_FLAG_UNREGISTER,
            RSEQ_SIGNATURE.wrapping_add(1) as usize,
            0,
            0,
        ])
    };
    let unregister = unsafe {
        raw_syscall(SYS_RSEQ, [
            address,
            len,
            RSEQ_FLAG_UNREGISTER,
            RSEQ_SIGNATURE as usize,
            0,
            0,
        ])
    };
    println!(
        "[rseq_hwprobe_test] rseq duplicate={} wrong_sig={} unregister={} final_cpu={}",
        duplicate, wrong_signature, unregister, area.cpu_id
    );
    duplicate == -16 && wrong_signature == -1 && unregister == 0 && area.cpu_id == u32::MAX
}

#[cfg(target_arch = "riscv64")]
fn test_hwprobe() -> bool {
    let mut pairs = [
        HwprobePair { key: 0, value: 0 },
        HwprobePair { key: 1, value: 0 },
        HwprobePair { key: 2, value: 0 },
        HwprobePair { key: 3, value: 0 },
        HwprobePair { key: 4, value: 0 },
        HwprobePair { key: 5, value: 0 },
        HwprobePair { key: 6, value: 0 },
        HwprobePair { key: 7, value: 0 },
        HwprobePair { key: 8, value: 0 },
        HwprobePair { key: 999, value: 1 },
    ];
    let query = unsafe {
        raw_syscall(SYS_RISCV_HWPROBE, [
            pairs.as_mut_ptr() as usize,
            pairs.len(),
            0,
            0,
            0,
            0,
        ])
    };
    println!(
        "[rseq_hwprobe_test] hwprobe query={} base={:#x} ext0={:#x} va={:#x} time={} unknown=({}, {})",
        query,
        pairs[3].value,
        pairs[4].value,
        pairs[7].value,
        pairs[8].value,
        pairs[9].key,
        pairs[9].value
    );
    if query != 0
        || pairs[3].value != 1
        || pairs[4].value & 0b11 != 0b11
        || pairs[7].value == 0
        || pairs[8].value == 0
        || pairs[9].key != -1
        || pairs[9].value != 0
    {
        return false;
    }

    let mut required = HwprobePair { key: 3, value: 1 };
    let mut cpus: usize = 0;
    let which = unsafe {
        raw_syscall(SYS_RISCV_HWPROBE, [
            &mut required as *mut HwprobePair as usize,
            1,
            core::mem::size_of::<usize>(),
            &mut cpus as *mut usize as usize,
            1,
            0,
        ])
    };
    let invalid_flags = unsafe {
        raw_syscall(SYS_RISCV_HWPROBE, [
            pairs.as_mut_ptr() as usize,
            1,
            0,
            0,
            2,
            0,
        ])
    };
    println!(
        "[rseq_hwprobe_test] hwprobe which={} cpus={:#x} invalid_flags={}",
        which, cpus, invalid_flags
    );
    which == 0 && cpus != 0 && invalid_flags == -22
}

#[unsafe(no_mangle)]
pub fn main() -> i32 {
    println!("[rseq_hwprobe_test] start");
    let passed = test_rseq();
    #[cfg(target_arch = "riscv64")]
    let passed = passed && test_hwprobe();
    if passed {
        println!("[rseq_hwprobe_test] PASS");
        0
    } else {
        println!("[rseq_hwprobe_test] FAIL");
        1
    }
}
