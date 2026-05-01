// /workspace/os/src/syscall/futex.rs
//! Futex (Fast Userspace muTex) implementation.
//! 
//! Futex provides efficient user-space synchronization primitives that only
//! enter the kernel when contention occurs. This is crucial for implementing
//! pthread mutexes, condition variables, and semaphores.

use super::*;
use alloc::vec::Vec;
use crate::error::{SysError, SyscallResult};
use crate::task::{block_current_and_run_next, current_task, wakeup_task, TaskControlBlock};
use crate::mm::translated_refmut;
use crate::task::current_user_token;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use lazy_static::*;
use spin::Mutex;

/// Futex operations
#[allow(unused)]
pub const FUTEX_WAIT: i32 = 0;
///
pub const FUTEX_WAKE: i32 = 1;
#[allow(unused)]
///
pub const FUTEX_FD: i32 = 2;
#[allow(unused)]
///
pub const FUTEX_REQUEUE: i32 = 3;
#[allow(unused)]
///
pub const FUTEX_CMP_REQUEUE: i32 = 4;
#[allow(unused)]
///
pub const FUTEX_WAKE_OP: i32 = 5;
#[allow(unused)]
///
pub const FUTEX_LOCK_PI: i32 = 6;
#[allow(unused)]
///
pub const FUTEX_UNLOCK_PI: i32 = 7;
#[allow(unused)]
///
pub const FUTEX_TRYLOCK_PI: i32 = 8;
#[allow(unused)]
///
pub const FUTEX_WAIT_BITSET: i32 = 9;
#[allow(unused)]
///
pub const FUTEX_WAKE_BITSET: i32 = 10;

/// Structure to track tasks waiting on a futex
struct FutexWaiter {
    task: Arc<TaskControlBlock>,
    _val: u32,  // Expected value at wait time (for FUTEX_WAIT)
}

lazy_static! {
    static ref FUTEX_WAITERS: Mutex<BTreeMap<usize, Vec<FutexWaiter>>> = 
        Mutex::new(BTreeMap::new());
}

/// Wake up threads waiting on a futex
/// 
/// # Arguments
/// * `uaddr` - User address of the futex word
/// * `nr_wake` - Number of threads to wake up
/// 
/// # Returns
/// Number of threads woken up
pub fn futex_wake(uaddr: usize, nr_wake: i32) -> SyscallResult {
    let mut waiters = FUTEX_WAITERS.lock();
    
    let count = match waiters.get_mut(&uaddr) {
        None => 0,
        Some(waiter_list) => {
            let wake_count = nr_wake.min(waiter_list.len() as i32) as usize;
            for _ in 0..wake_count {
                if let Some(waiter) = waiter_list.pop() {
                    wakeup_task(waiter.task);
                }
            }
            wake_count
        }
    };
    
    // Clean up empty entries
    if let Some(list) = waiters.get(&uaddr) {
        if list.is_empty() {
            waiters.remove(&uaddr);
        }
    }
    
    Ok(count)
}

/// Wait on a futex
/// 
/// # Arguments
/// * `uaddr` - User address of the futex word
/// * `val` - Expected value (atomic compare)
/// 
/// # Returns
/// Ok(0) on success, Err on failure
pub fn futex_wait(uaddr: usize, val: u32) -> SyscallResult {
    let token = current_user_token();
    
    // Verify the user address is accessible and read the current value
    let futex_val = *(translated_refmut(token, uaddr as *mut u32));
    
    // Compare current value with expected value
    if futex_val != val {
        return Err(SysError::EAGAIN);
    }
    
    // Add current task to the wait list
    let current_task = current_task().unwrap();
    let mut waiters = FUTEX_WAITERS.lock();
    
    waiters.entry(uaddr).or_insert_with(Vec::new).push(FutexWaiter {
        task: Arc::clone(&current_task),
        _val: val,
    });
    
    drop(waiters);
    
    // Block the current task
    block_current_and_run_next();
    
    Ok(0)
}

/// The futex() system call
/// 
/// # Arguments
/// * `uaddr` - User address of the futex word
/// * `op` - Operation to perform (FUTEX_WAIT, FUTEX_WAKE, etc.)
/// * `val` - Value argument (varies by operation)
/// * `timeout` - Optional timeout (not implemented)
/// * `uaddr2` - Second futex address (for requeue operations)
/// * `val3` - Third value argument
pub fn sys_futex(uaddr: usize, op: i32, val: i32, _timeout: usize, _uaddr2: usize, _val3: usize) -> SyscallResult {
    match op {
        FUTEX_WAIT => {
            // FUTEX_WAIT: uaddr, val, timeout, ...
            futex_wait(uaddr, val as u32)
        }
        FUTEX_WAKE => {
            // FUTEX_WAKE: uaddr, nr_wake, ...
            futex_wake(uaddr, val)
        }
        FUTEX_WAKE_OP => {
            // FUTEX_WAKE_OP: uaddr, nr_wake, nr_wake2, uaddr2, val3
            // This is a more complex operation that atomically wakes threads
            // and performs an operation on a second futex
            let nr_wake = val;
            let nr_wake2 = _timeout as i32;
            
            // First wake threads on uaddr
            let count1 = futex_wake(uaddr, nr_wake)?;
            
            // Then wake threads on uaddr2
            let count2 = futex_wake(_uaddr2, nr_wake2)?;
            
            Ok(count1 + count2)
        }
        FUTEX_CMP_REQUEUE => {
            // FUTEX_CMP_REQUEUE: uaddr, nr_wake, nr_requeue, uaddr2, val3
            let nr_wake = val;
            let nr_requeue = _timeout as i32;
            
            let mut waiters = FUTEX_WAITERS.lock();
            
            // First, drain all waiters from uaddr
            let mut all_waiters: Vec<FutexWaiter> = match waiters.remove(&uaddr) {
                None => Vec::new(),
                Some(list) => list,
            };
            
            // Wake nr_wake threads
            let wake_count = nr_wake.min(all_waiters.len() as i32) as usize;
            for _ in 0..wake_count {
                if let Some(waiter) = all_waiters.pop() {
                    wakeup_task(waiter.task);
                }
            }
            
            // Requeue remaining threads to uaddr2
            if !all_waiters.is_empty() {
                let requeue_count = nr_requeue.min(all_waiters.len() as i32) as usize;
                let mut requeued = Vec::with_capacity(requeue_count);
                for _ in 0..requeue_count {
                    if let Some(waiter) = all_waiters.pop() {
                        requeued.push(waiter);
                    }
                }
                if !requeued.is_empty() {
                    waiters.entry(_uaddr2).or_insert_with(Vec::new).extend(requeued);
                }
                // If there are still remaining waiters, put them back
                if !all_waiters.is_empty() {
                    waiters.insert(uaddr, all_waiters);
                }
            }
            
            Ok(wake_count)
        }
        _ => {
            Err(SysError::ENOSYS)
        }
    }
}