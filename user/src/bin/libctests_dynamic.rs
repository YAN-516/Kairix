#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

use user_lib::{execve, fork, waitpid};

// functional 目录下的测试
static FUNCTIONAL_TESTS: &[&str] = &[
    "src/functional/argv.exe",
    "src/functional/basename.exe",
    "src/functional/clocale_mbfuncs.exe",
    "src/functional/clock_gettime.exe",
    "src/functional/dirname.exe",
    "src/functional/dlopen.exe",
    "src/functional/env.exe",
    "src/functional/fdopen.exe",
    "src/functional/fnmatch.exe",
    "src/functional/fscanf.exe",
    "src/functional/fwscanf.exe",
    "src/functional/iconv_open.exe",
    "src/functional/inet_pton.exe",
    "src/functional/mbc.exe",
    "src/functional/memstream.exe",
    "src/functional/pthread_cancel-points.exe",
    "src/functional/pthread_cancel.exe",
    "src/functional/pthread_cond.exe",
    "src/functional/pthread_tsd.exe",
    "src/functional/qsort.exe",
    "src/functional/random.exe",
    "src/functional/search_hsearch.exe",
    "src/functional/search_insque.exe",
    "src/functional/search_lsearch.exe",
    "src/functional/search_tsearch.exe",
    "src/functional/sem_init.exe",
    "src/functional/setjmp.exe",
    "src/functional/snprintf.exe",
    "src/functional/socket.exe",
    "src/functional/sscanf.exe",
    "src/functional/sscanf_long.exe",
    "src/functional/stat.exe",
    "src/functional/strftime.exe",
    "src/functional/string.exe",
    "src/functional/string_memcpy.exe",
    "src/functional/string_memmem.exe",
    "src/functional/string_memset.exe",
    "src/functional/string_strchr.exe",
    "src/functional/string_strcspn.exe",
    "src/functional/string_strstr.exe",
    "src/functional/strptime.exe",
    "src/functional/strtod.exe",
    "src/functional/strtod_simple.exe",
    "src/functional/strtof.exe",
    "src/functional/strtol.exe",
    "src/functional/strtold.exe",
    "src/functional/swprintf.exe",
    "src/functional/tgmath.exe",
    "src/functional/time.exe",
    "src/functional/tls_init.exe",
    "src/functional/tls_local_exec.exe",
    "src/functional/udiv.exe",
    "src/functional/ungetc.exe",
    "src/functional/utime.exe",
    "src/functional/wcsstr.exe",
    "src/functional/wcstol.exe",
];

// regression 目录下的测试
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
    "src/regression/tls_get_new-dtv.exe",
    "src/regression/uselocale-0.exe",
    "src/regression/wcsncpy-read-overflow.exe",
    "src/regression/wcsstr-false-negative.exe",
];

/// 替换字符串中的所有 '-' 为 '_'
/// 由于 no_std 没有 String，使用 match 在编译期建立映射表
fn replace_dash_with_underscore(name: &str) -> &str {
    match name {
        "pthread_cancel-points" => "pthread_cancel_points",
        "pthread_cancel" => "pthread_cancel",
        "pthread_cond" => "pthread_cond",
        "pthread_tsd" => "pthread_tsd",
        "search_hsearch" => "search_hsearch",
        "search_insque" => "search_insque",
        "search_lsearch" => "search_lsearch",
        "search_tsearch" => "search_tsearch",
        "sscanf_long" => "sscanf_long",
        "string_memcpy" => "string_memcpy",
        "string_memmem" => "string_memmem",
        "string_memset" => "string_memset",
        "string_strchr" => "string_strchr",
        "string_strcspn" => "string_strcspn",
        "string_strstr" => "string_strstr",
        "strtod_simple" => "strtod_simple",
        "tls_align" => "tls_align",
        "daemon-failure" => "daemon_failure",
        "dn_expand-empty" => "dn_expand_empty",
        "dn_expand-ptr-0" => "dn_expand_ptr_0",
        "fflush-exit" => "fflush_exit",
        "fgets-eof" => "fgets_eof",
        "fgetwc-buffering" => "fgetwc_buffering",
        "fpclassify-invalid-ld80" => "fpclassify_invalid_ld80",
        "ftello-unflushed-append" => "ftello_unflushed_append",
        "getpwnam_r-crash" => "getpwnam_r_crash",
        "getpwnam_r-errno" => "getpwnam_r_errno",
        "iconv-roundtrips" => "iconv_roundtrips",
        "inet_ntop-v4mapped" => "inet_ntop_v4mapped",
        "inet_pton-empty-last-field" => "inet_pton_empty_last_field",
        "iswspace-null" => "iswspace_null",
        "lrand48-signextend" => "lrand48_signextend",
        "lseek-large" => "lseek_large",
        "malloc-0" => "malloc_0",
        "mbsrtowcs-overflow" => "mbsrtowcs_overflow",
        "memmem-oob-read" => "memmem_oob_read",
        "memmem-oob" => "memmem_oob",
        "mkdtemp-failure" => "mkdtemp_failure",
        "mkstemp-failure" => "mkstemp_failure",
        "printf-1e9-oob" => "printf_1e9_oob",
        "printf-fmt-g-round" => "printf_fmt_g_round",
        "printf-fmt-g-zeros" => "printf_fmt_g_zeros",
        "printf-fmt-n" => "printf_fmt_n",
        "pthread-robust-detach" => "pthread_robust_detach",
        "pthread_cancel-sem_wait" => "pthread_cancel_sem_wait",
        "pthread_cond-smasher" => "pthread_cond_smasher",
        "pthread_condattr_setclock" => "pthread_condattr_setclock",
        "pthread_exit-cancel" => "pthread_exit_cancel",
        "pthread_once-deadlock" => "pthread_once_deadlock",
        "pthread_rwlock-ebusy" => "pthread_rwlock_ebusy",
        "putenv-doublefree" => "putenv_doublefree",
        "regex-backref-0" => "regex_backref_0",
        "regex-bracket-icase" => "regex_bracket_icase",
        "regex-ere-backref" => "regex_ere_backref",
        "regex-escaped-high-byte" => "regex_escaped_high_byte",
        "regex-negated-range" => "regex_negated_range",
        "regexec-nosub" => "regexec_nosub",
        "rewind-clear-error" => "rewind_clear_error",
        "rlimit-open-files" => "rlimit_open_files",
        "scanf-bytes-consumed" => "scanf_bytes_consumed",
        "scanf-match-literal-eof" => "scanf_match_literal_eof",
        "scanf-nullbyte-char" => "scanf_nullbyte_char",
        "setvbuf-unget" => "setvbuf_unget",
        "sigprocmask-internal" => "sigprocmask_internal",
        "sscanf-eof" => "sscanf_eof",
        "syscall-sign-extend" => "syscall_sign_extend",
        "tls_get_new-dtv" => "tls_get_new_dtv",
        "uselocale-0" => "uselocale_0",
        "wcsncpy-read-overflow" => "wcsncpy_read_overflow",
        "wcsstr-false-negative" => "wcsstr_false_negative",
        _ => name,
    }
}

/// 从路径解析测试名和 entry 类型
/// 例如: "src/functional/argv.exe" -> ("entry-static.exe", "argv")
///       "src/regression/malloc-0.exe" -> ("entry-static.exe", "malloc_0")
fn parse_test_path(path: &str) -> (&str, &str) {
    // 去掉 "src/" 前缀
    let without_prefix = if path.starts_with("src/") {
        &path[4..]
    } else {
        path
    };

    // 找到最后一个 '/' 分割目录和文件名
    let slash_pos = without_prefix.rfind('/').unwrap_or(0);
    let _dir = &without_prefix[..slash_pos];
    let filename_with_ext = &without_prefix[slash_pos + 1..];

    // 去掉 ".exe" 后缀得到测试名
    let test_name_raw = if filename_with_ext.ends_with(".exe") {
        &filename_with_ext[..filename_with_ext.len() - 4]
    } else {
        filename_with_ext
    };

    // 将 '-' 替换为 '_'（匹配官方的 sed 's/-/_/g'）
    let test_name = replace_dash_with_underscore(test_name_raw);

    // 统一使用 entry-static.exe（根据官方 static.txt 的处理方式）
    let entry_type = "entry-static.exe";

    (entry_type, test_name)
}

#[allow(unused)]
fn run_tests(tests: &[&str], category: &str) -> (i32, i32) {
    let mut pass_num = 0;
    let total = tests.len() as i32;

    for test_path in tests {
        let (entry_type, test_name) = parse_test_path(test_path);

        // println!(
        //     "\x1b[34m[{}] Running {} ({} {})...\x1b[0m",
        //     category, test_name, entry_type, test_name
        // );

        let pid = fork();
        if pid == 0 {
            // 子进程：执行 ./runtest.exe -w <entry_type> <test_name>
            let argv: &[&str] = &["./runtest.exe", "-w", entry_type, test_name];

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

            // if exit_code == 0 {
            //     pass_num += 1;
            //     println!(
            //         "\x1b[32m[{}] Test {} PASSED (exit code 0)\x1b[0m",
            //         category, test_name
            //     );
            // } else {
            //     println!(
            //         "\x1b[31m[{}] Test {} FAILED (exit code {})\x1b[0m",
            //         category, test_name, exit_code
            //     );
            // }
        }
    }
    (pass_num, total)
}

#[allow(unused)]
#[unsafe(no_mangle)]
pub fn main() -> () {
    println!("--- Starting Musl libc Tests ---");

    // 运行 functional 测试
    println!("\n\x1b[1m=== Functional Tests ===\x1b[0m");
    let (func_pass, func_total) = run_tests(FUNCTIONAL_TESTS, "Functional");

    // 运行 regression 测试
    println!("\n\x1b[1m=== Regression Tests ===\x1b[0m");
    let (reg_pass, reg_total) = run_tests(REGRESSION_TESTS, "Regression");
}
