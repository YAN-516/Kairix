//! LTP testcase helpers.

use alloc::vec::Vec;

const LTP_TESTCASE_BIN_PREFIXES: &[&str] = &[
    "/musl/ltp/testcases/bin/",
    "/glibc/ltp/testcases/bin/",
    "/sdcard/musl/ltp/testcases/bin/",
    "/sdcard/glibc/ltp/testcases/bin/",
];

/// Returns the reason an LTP testcase path should be rejected before opening it.
pub(crate) fn reject_reason_for_exec_path(cwd_path: &str, path: &str) -> Option<&'static str> {
    if !path.contains("ltp") && !cwd_path.contains("ltp") {
        return None;
    }

    let mut components = Vec::new();
    if !path.starts_with('/') {
        push_clean_components(&mut components, cwd_path);
    }
    push_clean_components(&mut components, path);

    let case_name = match components.as_slice() {
        ["musl", "ltp", "testcases", "bin", case_name]
        | ["glibc", "ltp", "testcases", "bin", case_name]
        | ["sdcard", "musl", "ltp", "testcases", "bin", case_name]
        | ["sdcard", "glibc", "ltp", "testcases", "bin", case_name] => *case_name,
        _ => return None,
    };

    reject_reason_for_case(case_name)
}

/// Returns the reason an opened LTP testcase should be rejected.
pub(crate) fn reject_reason(path: &str, case_name: &str) -> Option<&'static str> {
    if !is_ltp_testcase_bin_path(path) {
        return None;
    }

    reject_reason_for_case(case_name)
}

fn reject_reason_for_case(case_name: &str) -> Option<&'static str> {
    if case_name.ends_with(".sh") {
        return Some("ltp shell script");
    }
    if !LTP_EXEC_WHITELIST.contains(&case_name) {
        return Some("not in whitelist");
    }
    None
}

fn push_clean_components<'a>(components: &mut Vec<&'a str>, path: &'a str) {
    for component in path.split('/').filter(|component| !component.is_empty()) {
        match component {
            "." => {}
            ".." => {
                components.pop();
            }
            name => components.push(name),
        }
    }
}

fn is_ltp_testcase_bin_path(path: &str) -> bool {
    LTP_TESTCASE_BIN_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
}

/// LTP testcase binaries that are allowed to run in the default test view.
pub const LTP_EXEC_WHITELIST: &[&str] = &[
    "abort01",
    "accept03",
    "access01",
    "access02",
    "access03",
    "access04",
    "brk01",
    "brk02",
    "chdir01",
    "clone01",
    "clone02",
    "clone03",
    "clone05",
    "clone06",
    "clone07",
    "clone08",
    "clone09",
    "clone301",
    "clone302",
    "close_range02",
    "confstr01",
    "copy_file_range03",
    "creat01",
    "creat03",
    // "creat05",
    "creat08",
    "dup01",
    "dup02",
    "dup03",
    "dup04",
    "dup05",
    "dup06",
    "dup07",
    "dup201",
    "dup202",
    "dup203",
    "dup204",
    "dup205",
    "dup206",
    "dup207",
    "dup3_01",
    "dup3_02",
    "epoll_ctl01",
    "epoll_ctl02",
    "epoll_ctl03",
    "epoll_ctl04",
    "epoll_ctl05",
    "exit01",
    "exit02",
    "fallocate03",
    "fallocate04",
    "fanotify01",
    "fanotify02",
    "fanotify03",
    "fanotify04",
    // "fanotify06",慢
    // "fanotify07",
    "fanotify08",
    "fanotify09",
    "fanotify10",
    "fanotify11",
    "fanotify12",
    "fanotify13",
    "fanotify14",
    "fanotify15",
    "fanotify16",
    // "fanotify17",慢
    // "fanotify18",慢
    "fanotify19",
    "fanotify20",
    "fanotify21",
    // "fanotify23",慢
    "fcntl02",
    "fcntl02_64",
    "fcntl03",
    "fcntl03_64",
    "fcntl04",
    "fcntl04_64",
    "fcntl08",
    "fcntl08_64",
    "fcntl12",
    "fcntl12_64",
    "fcntl29",
    "fcntl29_64",
    "fcntl30",
    "fcntl30_64",
    "fgetxattr01",
    "fgetxattr02",
    "fgetxattr03",
    "flistxattr01",
    "flistxattr02",
    "flistxattr03",
    // "fork01",
    // "fork03",
    // "fork04",
    // "fork07",
    // "fork08",
    // "fork09",
    // "fork10",
    // "fremovexattr01",
    "fremovexattr02",
    // "fsconfig01",慢
    "fsconfig02",
    // "fsconfig03",
    "fsetxattr01",
    "fsopen02",
    "fspick01",
    "fspick02",
    // "fstat02",慢
    // "fstat02_64",
    "fstatfs01",
    "fstatfs01_64",
    "fstatfs02",
    "fstatfs02_64",
    // "fsync01",
    // "fsync03",
    "ftruncate01",
    "ftruncate01_64",
    "ftruncate03",
    "ftruncate03_64",
    "getcwd02",
    "getcwd03",
    "getdents01",
    "getdents02",
    "getdomainname01",
    "getegid01",
    "getegid01_16",
    "getegid02",
    "getegid02_16",
    "geteuid01",
    "geteuid02",
    "getgid01",
    "getgid03",
    "gethostname01",
    "getpagesize01",
    "getpgid01",
    "getpgid02",
    "getpgrp01",
    "getpid01",
    "getpid02",
    "getppid01",
    "getppid02",
    "gettid01",
    "gettid02",
    "getxattr01",
    "getxattr02",
    // "getxattr03",
    "inotify01",
    "inotify02",
    // "inotify03",
    "inotify04",
    // "inotify06",
    "inotify10",
    "inotify12",
    "inotify_init1_01",
    "inotify_init1_02",
    "lgetxattr01",
    "lgetxattr02",
    "listxattr01",
    "listxattr02",
    "listxattr03",
    "llistxattr01",
    "llistxattr02",
    "llistxattr03",
    // "lremovexattr01",
    "madvise01",
    "madvise02",
    "madvise05",
    "madvise10",
    "memfd_create01",
    "mlock01",
    "mlock02",
    "mlock03",
    "mlock04",
    "mlock05",
    "mmap01",
    "mmap02",
    "mmap03",
    "mmap04",
    "mmap05",
    "mmap08",
    "mmap09",
    "mmap10",
    "mmap12",
    "mmap13",
    "mmap14",
    "mmap15",
    "mmap17",
    // "mmap18",
    "mmap19",
    "mmap20",
    "mmap21",
    "mmap22",
    // "mmapstress01",
    // "mmapstress04",
    // "mount01",
    // "mount02",
    // "mount03",
    // "mount04",
    // "mount05",
    // "mount06",
    // "mount07",
    // "mount_setattr01",
    // "move_mount01",
    // "move_mount02",
    "open01",
    "open02",
    "open03",
    // "open04",
    "open06",
    "open07",
    "open08",
    "open09",
    "open10",
    "open11",
    "openat01",
    "openat02",
    "openat04",
    "openat201",
    "openat202",
    "openat203",
    // "open_tree01",慢
    "open_tree02",
    "pipe01",
    "pipe02",
    "pipe03",
    "pipe06",
    "pipe08",
    "pipe10",
    "pipe11",
    // "pipe12", suspen很久
    "pipe13",
    "pipe14",
    //慢，但是能跑
    // "rename01",
    // "rename03",
    // "rename04",
    // "rename05",
    // "rename06",
    // "rename07",
    // "rename08",
    // "rename09",
    // "rename10",
    // "rename12",
    // "rename13",
    "sbrk01",
    "sbrk02",
    "setxattr01",
    "setxattr02",
    "setxattr03",
    "splice01",
    "splice03",
    "splice04",
    "splice07",
    "splice09",
    "sync_file_range01",
    "wait01",
    "wait02",
    "wait401",
    "wait402",
    "wait403",
    "waitid01",
    "waitid02",
    "waitid03",
    "waitid04",
    "waitid05",
    "waitid06",
    "waitid07",
    "waitid08",
    "waitid09",
    "waitid10",
    "waitid11",
    "waitpid01",
    "waitpid03",
    "waitpid04",
    //fail
    // "waitpid06",
    // "waitpid07",
    // "waitpid08",
    // "waitpid09",
    // "waitpid10",
    // "waitpid11",
    // "waitpid12",
    // "waitpid13",
];
