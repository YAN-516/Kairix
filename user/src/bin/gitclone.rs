#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::vec::Vec;
use user_lib::{execve, exit, fork, waitpid};

const FETCH_BIN: &str = "/bin/gitfetch";
const CHECKOUT_BIN: &str = "/bin/gitcheckout";
const DEFAULT_PACK: &str = "/musl/gitclone.pack";
const MAX_ARG_LEN: usize = 512;

struct Config {
    url: Option<&'static str>,
    target_dir: Option<&'static str>,
    pack_path: &'static str,
    fetch_args: Vec<&'static str>,
}

impl Config {
    fn new() -> Self {
        Self {
            url: None,
            target_dir: None,
            pack_path: DEFAULT_PACK,
            fetch_args: Vec::new(),
        }
    }
}

enum ArgResult {
    Ok,
    Help,
    Error,
}

#[unsafe(no_mangle)]
pub fn main_with_args(argc: usize, argv: *const usize) -> i32 {
    let mut cfg = Config::new();
    match parse_args(argc, argv, &mut cfg) {
        ArgResult::Help => {
            print_usage();
            return 0;
        }
        ArgResult::Error => return -1,
        ArgResult::Ok => {}
    }

    let url = match cfg.url {
        Some(v) => v,
        None => {
            print_usage();
            return -1;
        }
    };
    let target_dir = match cfg.target_dir {
        Some(v) => v,
        None => {
            print_usage();
            return -1;
        }
    };

    println!("gitclone fetch: {}", url);
    if !run_gitfetch(&cfg, url) {
        return -1;
    }

    println!("gitclone checkout: {}", target_dir);
    if !run_gitcheckout(cfg.pack_path, target_dir) {
        return -1;
    }

    println!("gitclone complete: {}", target_dir);
    0
}

fn parse_args(argc: usize, argv: *const usize, cfg: &mut Config) -> ArgResult {
    let mut i = 1usize;
    while i < argc {
        let arg = match argv_str(argv, i) {
            Some(v) => v,
            None => {
                println!("invalid argument");
                return ArgResult::Error;
            }
        };

        if arg == "-h" || arg == "--help" {
            return ArgResult::Help;
        } else if arg == "--pack" {
            cfg.pack_path = match next_arg(argc, argv, &mut i, "pack") {
                Some(v) if !v.is_empty() => v,
                _ => {
                    println!("invalid pack path");
                    return ArgResult::Error;
                }
            };
        } else if let Some(v) = strip_prefix(arg, "--pack=") {
            if v.is_empty() {
                println!("invalid pack path");
                return ArgResult::Error;
            }
            cfg.pack_path = v;
        } else if takes_value(arg) {
            cfg.fetch_args.push(arg);
            match next_arg(argc, argv, &mut i, arg) {
                Some(v) => cfg.fetch_args.push(v),
                None => return ArgResult::Error,
            }
        } else if starts_with(arg, "-") {
            cfg.fetch_args.push(arg);
        } else if cfg.url.is_none() {
            cfg.url = Some(arg);
        } else if cfg.target_dir.is_none() {
            cfg.target_dir = Some(arg);
        } else {
            println!("too many arguments");
            return ArgResult::Error;
        }
        i += 1;
    }
    ArgResult::Ok
}

fn run_gitfetch(cfg: &Config, url: &'static str) -> bool {
    let mut args = Vec::new();
    args.push("gitfetch");
    args.push(url);
    args.push("-o");
    args.push(cfg.pack_path);
    for &arg in &cfg.fetch_args {
        args.push(arg);
    }
    run_command(FETCH_BIN, &args)
}

fn run_gitcheckout(pack_path: &'static str, target_dir: &'static str) -> bool {
    let args = ["gitcheckout", pack_path, target_dir];
    run_command(CHECKOUT_BIN, &args)
}

fn run_command(path: &str, args: &[&str]) -> bool {
    let pid = fork();
    if pid < 0 {
        println!("fork failed: {}", pid);
        return false;
    }
    if pid == 0 {
        let env = ["PATH=/bin:/sbin:/musl:/usr/bin"];
        let ret = execve(path, args, &env);
        println!("exec failed: {} {}", path, ret);
        exit(-1);
    }

    let mut code = 0i32;
    let ret = waitpid(pid as usize, &mut code);
    if ret < 0 {
        println!("waitpid failed: {}", ret);
        return false;
    }
    if code != 0 {
        println!("command failed: {} exit {}", path, code);
        return false;
    }
    true
}

fn takes_value(arg: &str) -> bool {
    matches!(
        arg,
        "-d" | "--dns" | "--ip" | "-p" | "--port" | "-u" | "--user" | "--password" | "-i" | "--key"
    )
}

fn argv_str(argv: *const usize, idx: usize) -> Option<&'static str> {
    cstr_to_str(unsafe { *argv.add(idx) as *const u8 })
}

fn next_arg(argc: usize, argv: *const usize, idx: &mut usize, name: &str) -> Option<&'static str> {
    *idx += 1;
    if *idx >= argc {
        println!("missing value for {}", name);
        None
    } else {
        argv_str(argv, *idx)
    }
}

fn cstr_to_str(ptr: *const u8) -> Option<&'static str> {
    if ptr.is_null() {
        return None;
    }
    let mut len = 0usize;
    unsafe {
        while *ptr.add(len) != 0 {
            len += 1;
            if len > MAX_ARG_LEN {
                return None;
            }
        }
        core::str::from_utf8(core::slice::from_raw_parts(ptr, len)).ok()
    }
}

fn strip_prefix<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let bytes = s.as_bytes();
    let prefix = prefix.as_bytes();
    if bytes.len() >= prefix.len() && &bytes[..prefix.len()] == prefix {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

fn starts_with(s: &str, prefix: &str) -> bool {
    strip_prefix(s, prefix).is_some()
}

fn print_usage() {
    println!("usage: gitclone <url> <target-dir> [fetch options]");
    println!("ssh:   gitclone ssh://user@host/repo.git /musl/repo --key /musl/id_ed25519");
    println!("https: gitclone https://github.com/user/repo.git /musl/repo");
    println!("options:");
    println!("      --pack PATH       temporary pack path, default /musl/gitclone.pack");
    println!("  -d, --dns IP          forwarded to gitfetch");
    println!("      --ip IP           forwarded to gitfetch");
    println!("  -p, --port PORT       forwarded to gitfetch");
    println!("  -u, --user USER       forwarded to gitfetch");
    println!("      --password PASS   forwarded to gitfetch");
    println!("  -i, --key PATH        forwarded to gitfetch");
    println!("  -v, --verbose         forwarded to gitfetch");
}
