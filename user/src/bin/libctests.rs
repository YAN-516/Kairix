#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{execve, fork, waitpid};

// functional 目录下的测试使用 entry-static.exe
static FUNCTIONAL_TESTS: &[&str] = &[
    // "src/functional/argv.exe",
    // "src/functional/basename.exe",
    // "src/functional/clocale_mbfuncs.exe",
    // "src/functional/clock_gettime.exe",
    // "src/functional/dirname.exe",
    // "src/functional/env.exe",
    // "src/functional/fdopen.exe",
    // "src/functional/fnmatch.exe",
    // "src/functional/fscanf.exe",
    // "src/functional/fwscanf.exe",
    // "src/functional/iconv_open.exe",
    // "src/functional/inet_pton.exe",
    // "src/functional/mbc.exe",
    // "src/functional/memstream.exe",
    // // "src/functional/pthread_cancel-points.exe",
    // // "src/functional/pthread_cancel.exe",
    // // "src/functional/pthread_cond.exe",
    // // "src/functional/pthread_tsd.exe",
    // "src/functional/qsort.exe",
    // "src/functional/random.exe",
    // "src/functional/search_hsearch.exe",
    // "src/functional/search_insque.exe",
    // "src/functional/search_lsearch.exe",
    // "src/functional/search_tsearch.exe",
    // "src/functional/setjmp.exe",
    // "src/functional/snprintf.exe",
    // // "src/functional/socket.exe",
    // "src/functional/sscanf.exe",
    // "src/functional/sscanf_long.exe",
    // "src/functional/stat.exe",
    // "src/functional/strftime.exe",
    // "src/functional/string.exe",
    // "src/functional/string_memcpy.exe",
    // "src/functional/string_memmem.exe",
    // "src/functional/string_memset.exe",
    // "src/functional/string_strchr.exe",
    // "src/functional/string_strcspn.exe",
    // "src/functional/string_strstr.exe",
    // "src/functional/strptime.exe",
    // "src/functional/strtod.exe",
    // "src/functional/strtod_simple.exe",
    // "src/functional/strtof.exe",
    // "src/functional/strtol.exe",
    // "src/functional/strtold.exe",
    // "src/functional/swprintf.exe",
    // "src/functional/tgmath.exe",
    // "src/functional/time.exe",
    // "src/functional/tls_align.exe",
    // "src/functional/udiv.exe",
    // "src/functional/ungetc.exe",
    // // "src/functional/utime.exe",
    "src/functional/wcsstr.exe",
    "src/functional/wcstol.exe",
];

// regression 目录下的测试使用 entry-dynamic.exe
static REGRESSION_TESTS: &[&str] = &[
    "src/regression/daemon-failure.exe",
    "src/regression/dn_expand-empty.exe",
    "src/regression/dn_expand-ptr-0.exe",
    "src/regression/fflush-exit.exe",
    "src/regression/fgets-eof.exe",
    "src/regression/fgetwc-buffering.exe",
    "src/regression/fpclassify-invalid-ld80.exe",
    "src/regression/ftello-unflushed-append.exe",
    "src/regression/getpwnam_r-crash.exe",
    "src/regression/getpwnam_r-errno.exe",
    "src/regression/iconv-roundtrips.exe",
    "src/regression/inet_ntop-v4mapped.exe",
    "src/regression/inet_pton-empty-last-field.exe",
    "src/regression/iswspace-null.exe",
    "src/regression/lrand48-signextend.exe",
    "src/regression/lseek-large.exe",
    "src/regression/malloc-0.exe",
    "src/regression/mbsrtowcs-overflow.exe",
    "src/regression/memmem-oob-read.exe",
    "src/regression/memmem-oob.exe",
    "src/regression/mkdtemp-failure.exe",
    "src/regression/mkstemp-failure.exe",
    "src/regression/printf-1e9-oob.exe",
    "src/regression/printf-fmt-g-round.exe",
    "src/regression/printf-fmt-g-zeros.exe",
    "src/regression/printf-fmt-n.exe",
    "src/regression/pthread-robust-detach.exe",
    "src/regression/pthread_cancel-sem_wait.exe",
    "src/regression/pthread_cond-smasher.exe",
    "src/regression/pthread_condattr_setclock.exe",
    "src/regression/pthread_exit-cancel.exe",
    "src/regression/pthread_once-deadlock.exe",
    "src/regression/pthread_rwlock-ebusy.exe",
    "src/regression/putenv-doublefree.exe",
    "src/regression/regex-backref-0.exe",
    "src/regression/regex-bracket-icase.exe",
    "src/regression/regex-ere-backref.exe",
    "src/regression/regex-escaped-high-byte.exe",
    "src/regression/regex-negated-range.exe",
    "src/regression/regexec-nosub.exe",
    "src/regression/rewind-clear-error.exe",
    "src/regression/rlimit-open-files.exe",
    "src/regression/scanf-bytes-consumed.exe",
    "src/regression/scanf-match-literal-eof.exe",
    "src/regression/scanf-nullbyte-char.exe",
    "src/regression/setvbuf-unget.exe",
    "src/regression/sigprocmask-internal.exe",
    "src/regression/sscanf-eof.exe",
    "src/regression/statvfs.exe",
    "src/regression/strverscmp.exe",
    "src/regression/syscall-sign-extend.exe",
    "src/regression/uselocale-0.exe",
    "src/regression/wcsncpy-read-overflow.exe",
    "src/regression/wcsstr-false-negative.exe",
];

/// 从路径解析测试名和 entry 类型
/// 例如: "src/functional/argv.exe" -> ("entry-static.exe", "argv")
///       "src/regression/malloc-0.exe" -> ("entry-dynamic.exe", "malloc-0")
fn parse_test_path<'a>(path: &'a str) -> (&'static str, &'a str){
    // 去掉 "src/" 前缀
    let without_prefix = if path.starts_with("src/") {
        &path[4..]
    } else {
        path
    };

    // 找到最后一个 '/' 分割目录和文件名
    let slash_pos = without_prefix.rfind('/').unwrap_or(0);
    let dir = &without_prefix[..slash_pos];
    let filename_with_ext = &without_prefix[slash_pos + 1..];

    // 去掉 ".exe" 后缀得到测试名
    let test_name = if filename_with_ext.ends_with(".exe") {
        &filename_with_ext[..filename_with_ext.len() - 4]
    } else {
        filename_with_ext
    };

    // 根据目录确定 entry 类型
    let entry_type = if dir == "regression" {
        "entry-dynamic.exe"
    } else {
        // functional 或其他默认使用 static
        "entry-static.exe"
    };

    // 注意：返回的字符串生命周期需要是 'static，但这里 test_name 是切片
    // 由于原始 path 是 &'static str，切片也是 'static
    (entry_type, test_name)
}

fn run_tests(tests: &[&str], category: &str) -> (i32, i32) {
    let mut pass_num = 0;
    let total = tests.len() as i32;

    for test_path in tests {
        let (entry_type, test_name) = parse_test_path(test_path);
        
        println!(
            "\x1b[34m[{}] Running {} ({} {})...\x1b[0m",
            category, test_name, entry_type, test_name
        );

        let pid = fork();
        if pid == 0 {
            // 子进程：执行 ./runtest.exe -w <entry_type> <test_name>
            // argv[0] 必须是程序名本身
            let argv: &[&str] = &[
                "./runtest.exe",
                "-w",
                entry_type,
                test_name,
            ];
            
            execve("./runtest.exe", argv, &[]);

            println!(
                "[{}] Error: Failed to execute ./runtest.exe for {}",
                category, test_name
            );
            user_lib::exit(-1);
        } else {
            let mut exit_code: i32 = 0;
            let wait_pid = waitpid(pid as usize, &mut exit_code);

            assert_eq!(pid, wait_pid);

            
            println!(
                "\x1b[31m[{}] Test {}(exit code {})\x1b[0m",
                category, test_name, exit_code
            );
            
        }
    }
    (pass_num, total)
}

#[unsafe(no_mangle)]
pub fn main() ->(){
    println!("--- Starting Musl libc Tests ---");

    // 运行 functional 测试
    println!("\n\x1b[1m=== Functional Tests ===\x1b[0m");
    let (func_pass, func_total) = run_tests(FUNCTIONAL_TESTS, "Functional");

    // 运行 regression 测试
    println!("\n\x1b[1m=== Regression Tests ===\x1b[0m");
    let (reg_pass, reg_total) = run_tests(REGRESSION_TESTS, "Regression");
    
}