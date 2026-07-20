#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

const ENV_COUNT: usize = 128;
const ENV_VALUE: &[u8] =
    b"BUILDSTORM_VFORK_ENV=abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";

unsafe fn cstr_eq(mut ptr: *const u8, expected: &[u8]) -> bool {
    for expected_byte in expected {
        if unsafe { *ptr } != *expected_byte {
            return false;
        }
        ptr = unsafe { ptr.add(1) };
    }
    unsafe { *ptr == 0 }
}

#[unsafe(no_mangle)]
pub fn main_with_args(argc: usize, argv: *const usize) -> i32 {
    let envp = unsafe { argv.add(argc + 1) };
    for index in 0..ENV_COUNT {
        let ptr = unsafe { *envp.add(index) as *const u8 };
        if ptr.is_null() || !unsafe { cstr_eq(ptr, ENV_VALUE) } {
            println!(
                "[vfork_exec_env_target] FAIL: invalid env entry index={}",
                index
            );
            return 1;
        }
    }
    if unsafe { *envp.add(ENV_COUNT) } != 0 {
        println!("[vfork_exec_env_target] FAIL: envp is not terminated");
        return 2;
    }

    println!("[vfork_exec_env_target] PASS env_count={}", ENV_COUNT);
    42
}
