#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::vec::Vec;
use user_lib::{execve, exit};

const MAX_ARG_LEN: usize = 512;

struct Command {
    bin: &'static str,
    argv0: &'static str,
}

#[unsafe(no_mangle)]
pub fn main_with_args(argc: usize, argv: *const usize) -> i32 {
    if argc < 2 {
        print_usage();
        return -1;
    }

    let subcmd = match argv_str(argv, 1) {
        Some("-h") | Some("--help") | Some("help") => {
            print_usage();
            return 0;
        }
        Some(v) => v,
        None => {
            println!("invalid command");
            return -1;
        }
    };

    let cmd = match resolve_command(subcmd) {
        Some(v) => v,
        None => {
            println!("git: unsupported command '{}'", subcmd);
            print_usage();
            return -1;
        }
    };

    let mut child_args = Vec::new();
    child_args.push(cmd.argv0);
    let mut i = 2usize;
    while i < argc {
        match argv_str(argv, i) {
            Some(v) => child_args.push(v),
            None => {
                println!("invalid argument");
                return -1;
            }
        }
        i += 1;
    }

    let env = ["PATH=/bin:/sbin:/musl:/usr/bin"];
    let ret = execve(cmd.bin, &child_args, &env);
    println!("git: exec failed: {} {}", cmd.bin, ret);
    exit(-1);
}

fn resolve_command(name: &str) -> Option<Command> {
    match name {
        "add" => Some(Command {
            bin: "/bin/gitadd",
            argv0: "gitadd",
        }),
        "clone" => Some(Command {
            bin: "/bin/gitclone",
            argv0: "gitclone",
        }),
        "commit" => Some(Command {
            bin: "/bin/gitcommit",
            argv0: "gitcommit",
        }),
        "fetch" => Some(Command {
            bin: "/bin/gitfetch",
            argv0: "gitfetch",
        }),
        "pull" => Some(Command {
            bin: "/bin/gitpull",
            argv0: "gitpull",
        }),
        "push" => Some(Command {
            bin: "/bin/gitpush",
            argv0: "gitpush",
        }),
        "status" => Some(Command {
            bin: "/bin/gitstatus",
            argv0: "gitstatus",
        }),
        "log" => Some(Command {
            bin: "/bin/gitlog",
            argv0: "gitlog",
        }),
        "ls" | "ls-remote" => Some(Command {
            bin: "/bin/gitls",
            argv0: "gitls",
        }),
        "pack" | "verify-pack" => Some(Command {
            bin: "/bin/gitpack",
            argv0: "gitpack",
        }),
        "checkout-pack" => Some(Command {
            bin: "/bin/gitcheckout",
            argv0: "gitcheckout",
        }),
        "pkt-test" => Some(Command {
            bin: "/bin/gitpkt_test",
            argv0: "gitpkt_test",
        }),
        _ => None,
    }
}

fn argv_str(argv: *const usize, idx: usize) -> Option<&'static str> {
    cstr_to_str(unsafe { *argv.add(idx) as *const u8 })
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

fn print_usage() {
    println!("usage: git <command> [args]");
    println!("commands:");
    println!("  add [--repo DIR] <path>...         add files to index");
    println!("  clone <url> <dir> [options]        clone repository");
    println!("  commit [--repo DIR] -m MESSAGE     create local commit");
    println!("  pull <repo-dir> [options]          fetch and update worktree");
    println!("  push [repo-dir] [url] --key PATH   push current HEAD over SSH");
    println!("  status [repo-dir]                  show worktree changes");
    println!("  log [repo-dir]                     show commit history");
    println!("  fetch <url> [options]              fetch pack file");
    println!("  ls-remote <url> [options]          list remote refs");
    println!("  pack [pack-file]                   inspect pack file");
    println!("  checkout-pack <pack> <dir> [...]   checkout a pack file");
    println!("  pkt-test                           run pkt-line selftest");
}
