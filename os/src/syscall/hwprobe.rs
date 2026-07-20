//! RISC-V hardware probing syscall.

use crate::config::MAX_CPU_NUM;
use crate::error::{SysError, SyscallResult};
use crate::mm::{copy_to_user, translated_byte_buffer};
use crate::task::current_user_token;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

const KEY_MVENDORID: i64 = 0;
const KEY_MARCHID: i64 = 1;
const KEY_MIMPID: i64 = 2;
const KEY_BASE_BEHAVIOR: i64 = 3;
const KEY_IMA_EXT_0: i64 = 4;
const KEY_CPUPERF_0: i64 = 5;
const KEY_ZICBOZ_BLOCK_SIZE: i64 = 6;
const KEY_HIGHEST_VIRT_ADDRESS: i64 = 7;
const KEY_TIME_CSR_FREQ: i64 = 8;
const KEY_MISALIGNED_SCALAR_PERF: i64 = 9;
const KEY_MISALIGNED_VECTOR_PERF: i64 = 10;
const KEY_VENDOR_EXT_THEAD_0: i64 = 11;
const KEY_ZICBOM_BLOCK_SIZE: i64 = 12;
const KEY_VENDOR_EXT_SIFIVE_0: i64 = 13;
const KEY_VENDOR_EXT_MIPS_0: i64 = 14;
const KEY_ZICBOP_BLOCK_SIZE: i64 = 15;
const KEY_IMA_EXT_1: i64 = 16;
const MAX_KEY: i64 = KEY_IMA_EXT_1;

const BASE_BEHAVIOR_IMA: u64 = 1;
const IMA_FD: u64 = 1 << 0;
const IMA_C: u64 = 1 << 1;
const MISALIGNED_UNKNOWN: u64 = 0;
const MISALIGNED_VECTOR_UNSUPPORTED: u64 = 4;
const WHICH_CPUS: u32 = 1;
const PAIR_SIZE: usize = 16;
const CPUSET_BYTES: usize = core::mem::size_of::<usize>();

static ID_READY: [AtomicBool; MAX_CPU_NUM] = [const { AtomicBool::new(false) }; MAX_CPU_NUM];
static MVENDORID: [AtomicUsize; MAX_CPU_NUM] = [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static MARCHID: [AtomicUsize; MAX_CPU_NUM] = [const { AtomicUsize::new(0) }; MAX_CPU_NUM];
static MIMPID: [AtomicUsize; MAX_CPU_NUM] = [const { AtomicUsize::new(0) }; MAX_CPU_NUM];

/// Cache the SBI machine identifiers on the CPU that is about to become online.
pub(crate) fn record_current_cpu(cpu: usize) {
    if cpu >= MAX_CPU_NUM {
        return;
    }
    MVENDORID[cpu].store(sbi_rt::get_mvendorid(), Ordering::Relaxed);
    MARCHID[cpu].store(sbi_rt::get_marchid(), Ordering::Relaxed);
    MIMPID[cpu].store(sbi_rt::get_mimpid(), Ordering::Relaxed);
    ID_READY[cpu].store(true, Ordering::Release);
}

fn checked_user_range(address: usize, len: usize) -> Result<(), SysError> {
    if len == 0 {
        return Ok(());
    }
    if address == 0 {
        return Err(SysError::EFAULT);
    }
    let end = address.checked_add(len).ok_or(SysError::EFAULT)?;
    if end == 0 || end - 1 > polyhal::consts::USER_MEMORY_SPACE.1 {
        return Err(SysError::EFAULT);
    }
    Ok(())
}

fn read_user(address: usize, dst: &mut [u8]) -> Result<(), SysError> {
    checked_user_range(address, dst.len())?;
    if dst.is_empty() {
        return Ok(());
    }
    let buffers = translated_byte_buffer(current_user_token(), address as *const u8, dst.len())?;
    let mut copied = 0;
    for buffer in buffers {
        let len = buffer.len().min(dst.len() - copied);
        dst[copied..copied + len].copy_from_slice(&buffer[..len]);
        copied += len;
    }
    if copied == dst.len() {
        Ok(())
    } else {
        Err(SysError::EFAULT)
    }
}

fn write_user(address: usize, src: &[u8]) -> Result<(), SysError> {
    checked_user_range(address, src.len())?;
    if !src.is_empty() {
        copy_to_user(current_user_token(), address as *mut u8, src)?;
    }
    Ok(())
}

fn read_pair(address: usize) -> Result<(i64, u64), SysError> {
    let mut bytes = [0u8; PAIR_SIZE];
    read_user(address, &mut bytes)?;
    Ok((
        i64::from_ne_bytes(bytes[0..8].try_into().unwrap()),
        u64::from_ne_bytes(bytes[8..16].try_into().unwrap()),
    ))
}

fn write_pair(address: usize, key: i64, value: u64) -> Result<(), SysError> {
    let mut bytes = [0u8; PAIR_SIZE];
    bytes[0..8].copy_from_slice(&key.to_ne_bytes());
    bytes[8..16].copy_from_slice(&value.to_ne_bytes());
    write_user(address, &bytes)
}

fn read_cpuset(address: usize, size: usize) -> Result<usize, SysError> {
    let size = size.min(CPUSET_BYTES);
    let mut bytes = [0u8; CPUSET_BYTES];
    read_user(address, &mut bytes[..size])?;
    Ok(usize::from_ne_bytes(bytes))
}

fn write_cpuset(address: usize, size: usize, mask: usize) -> Result<(), SysError> {
    let bytes = mask.to_ne_bytes();
    write_user(address, &bytes[..size.min(CPUSET_BYTES)])
}

fn id_for_mask(values: &[AtomicUsize; MAX_CPU_NUM], cpus: usize) -> u64 {
    let mut common = None;
    for cpu in 0..MAX_CPU_NUM {
        if cpus & (1usize << cpu) == 0 {
            continue;
        }
        if !ID_READY[cpu].load(Ordering::Acquire) {
            return u64::MAX;
        }
        let value = values[cpu].load(Ordering::Relaxed) as u64;
        match common {
            Some(old) if old != value => return u64::MAX,
            None => common = Some(value),
            _ => {}
        }
    }
    common.unwrap_or(u64::MAX)
}

fn value_for_key(key: i64, cpus: usize) -> Option<u64> {
    match key {
        KEY_MVENDORID => Some(id_for_mask(&MVENDORID, cpus)),
        KEY_MARCHID => Some(id_for_mask(&MARCHID, cpus)),
        KEY_MIMPID => Some(id_for_mask(&MIMPID, cpus)),
        KEY_BASE_BEHAVIOR => Some(BASE_BEHAVIOR_IMA),
        // Kairix's rv64gc userspace ABI and context-switch implementation
        // guarantee F, D and C. Optional extensions are deliberately not
        // advertised without architecture discovery support.
        KEY_IMA_EXT_0 => Some(IMA_FD | IMA_C),
        KEY_CPUPERF_0 | KEY_MISALIGNED_SCALAR_PERF => Some(MISALIGNED_UNKNOWN),
        KEY_ZICBOZ_BLOCK_SIZE | KEY_ZICBOM_BLOCK_SIZE | KEY_ZICBOP_BLOCK_SIZE => Some(0),
        KEY_HIGHEST_VIRT_ADDRESS => Some(polyhal::consts::USER_MEMORY_SPACE.1 as u64),
        KEY_TIME_CSR_FREQ => Some(crate::config::_CLOCK_FREQ as u64),
        KEY_MISALIGNED_VECTOR_PERF => Some(MISALIGNED_VECTOR_UNSUPPORTED),
        KEY_VENDOR_EXT_THEAD_0
        | KEY_VENDOR_EXT_SIFIVE_0
        | KEY_VENDOR_EXT_MIPS_0
        | KEY_IMA_EXT_1 => Some(0),
        _ => None,
    }
}

fn key_is_bitmask(key: i64) -> bool {
    matches!(
        key,
        KEY_BASE_BEHAVIOR
            | KEY_IMA_EXT_0
            | KEY_CPUPERF_0
            | KEY_VENDOR_EXT_THEAD_0
            | KEY_VENDOR_EXT_SIFIVE_0
            | KEY_VENDOR_EXT_MIPS_0
            | KEY_IMA_EXT_1
    )
}

fn pair_matches(key: i64, actual: u64, requested: u64) -> bool {
    if key_is_bitmask(key) {
        actual & requested == requested
    } else {
        actual == requested
    }
}

fn selected_cpus(cpuset_size: usize, cpuset: usize) -> Result<usize, SysError> {
    let online = crate::task::manager::online_cpu_mask();
    let selected = if cpuset_size == 0 && cpuset == 0 {
        online
    } else {
        read_cpuset(cpuset, cpuset_size)? & online
    };
    if selected == 0 {
        Err(SysError::EINVAL)
    } else {
        Ok(selected)
    }
}

fn get_values(
    pairs: usize,
    pair_count: usize,
    cpuset_size: usize,
    cpuset: usize,
    flags: u32,
) -> SyscallResult {
    if flags != 0 {
        return Err(SysError::EINVAL);
    }
    let cpus = selected_cpus(cpuset_size, cpuset)?;
    for index in 0..pair_count {
        let address = pairs
            .checked_add(index.checked_mul(PAIR_SIZE).ok_or(SysError::EFAULT)?)
            .ok_or(SysError::EFAULT)?;
        let (key, _) = read_pair(address)?;
        match value_for_key(key, cpus) {
            Some(value) => write_pair(address, key, value)?,
            None => write_pair(address, -1, 0)?,
        }
    }
    Ok(0)
}

fn get_matching_cpus(
    pairs: usize,
    pair_count: usize,
    cpuset_size: usize,
    cpuset: usize,
    flags: u32,
) -> SyscallResult {
    if flags != WHICH_CPUS || cpuset_size == 0 || cpuset == 0 {
        return Err(SysError::EINVAL);
    }
    let online = crate::task::manager::online_cpu_mask();
    let requested = read_cpuset(cpuset, cpuset_size)?;
    let mut cpus = if requested == 0 {
        online
    } else {
        requested & online
    };
    let mut invalid_key = false;

    for index in 0..pair_count {
        let address = pairs
            .checked_add(index.checked_mul(PAIR_SIZE).ok_or(SysError::EFAULT)?)
            .ok_or(SysError::EFAULT)?;
        let (key, requested_value) = read_pair(address)?;
        if !(0..=MAX_KEY).contains(&key) {
            invalid_key = true;
            write_pair(address, -1, 0)?;
            continue;
        }
        if invalid_key {
            continue;
        }
        for cpu in 0..MAX_CPU_NUM {
            let bit = 1usize << cpu;
            if cpus & bit == 0 {
                continue;
            }
            let actual = value_for_key(key, bit).ok_or(SysError::EINVAL)?;
            if !pair_matches(key, actual, requested_value) {
                cpus &= !bit;
            }
        }
    }
    if invalid_key {
        cpus = 0;
    }
    write_cpuset(cpuset, cpuset_size, cpus)?;
    Ok(0)
}

/// Query RISC-V machine IDs and userspace-visible ISA behavior for a CPU set.
pub fn sys_riscv_hwprobe(
    pairs: usize,
    pair_count: usize,
    cpuset_size: usize,
    cpuset: usize,
    flags: u32,
) -> SyscallResult {
    if flags & WHICH_CPUS != 0 {
        get_matching_cpus(pairs, pair_count, cpuset_size, cpuset, flags)
    } else {
        get_values(pairs, pair_count, cpuset_size, cpuset, flags)
    }
}
