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

    let env = ["PATH=/usr/bin:/bin:/sbin:/musl"];
    let ret = execve(cmd.bin, &child_args, &env);
    println!("git: exec failed: {} {}", cmd.bin, ret);
    exit(-1);
}

fn resolve_command(name: &str) -> Option<Command> {
    match name {
        "init" => Some(Command {
            bin: "/bin/kgitinit",
            argv0: "kgitinit",
        }),
        "add" => Some(Command {
            bin: "/bin/kgitadd",
            argv0: "kgitadd",
        }),
        "clone" => Some(Command {
            bin: "/bin/kgitclone",
            argv0: "kgitclone",
        }),
        "commit" => Some(Command {
            bin: "/bin/kgitcommit",
            argv0: "kgitcommit",
        }),
        "config" => Some(Command {
            bin: "/bin/kgitconfig",
            argv0: "kgitconfig",
        }),
        "fetch" => Some(Command {
            bin: "/bin/kgitfetch",
            argv0: "kgitfetch",
        }),
        "pull" => Some(Command {
            bin: "/bin/kgitpull",
            argv0: "kgitpull",
        }),
        "push" => Some(Command {
            bin: "/bin/kgitpush",
            argv0: "kgitpush",
        }),
        "remote" => Some(Command {
            bin: "/bin/kgitremote",
            argv0: "kgitremote",
        }),
        "branch" => Some(Command {
            bin: "/bin/kgitbranch",
            argv0: "kgitbranch",
        }),
        "checkout" | "switch" => Some(Command {
            bin: "/bin/kgitcheckoutref",
            argv0: "kgitcheckoutref",
        }),
        "status" => Some(Command {
            bin: "/bin/kgitstatus",
            argv0: "kgitstatus",
        }),
        "log" => Some(Command {
            bin: "/bin/kgitlog",
            argv0: "kgitlog",
        }),
        "ls" | "ls-remote" => Some(Command {
            bin: "/bin/kgitls",
            argv0: "kgitls",
        }),
        "pack" | "verify-pack" => Some(Command {
            bin: "/bin/kgitpack",
            argv0: "kgitpack",
        }),
        "checkout-pack" => Some(Command {
            bin: "/bin/kgitcheckout",
            argv0: "kgitcheckout",
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
    println!("  init [directory]                   create an empty repository");
    println!("  add [--repo DIR] <path>...         add files to index");
    println!("  clone <url> <dir> [options]        clone repository");
    println!("  commit [--repo DIR] -m MESSAGE     create local commit");
    println!("  config --global <key> <value>      set user config");
    println!("  pull [repo-dir] [remote] [...]     fetch and update worktree");
    println!("  push [repo-dir] [remote] --key PATH push current HEAD over SSH");
    println!("  remote add <name> <url>            add a remote");
    println!("  branch [-a|-r|-vv] [name]          list/create/delete branches");
    println!("  checkout <branch>                  switch branch");
    println!("  status [repo-dir]                  show worktree changes");
    println!("  log [repo-dir]                     show commit history");
    println!("  fetch <url> [options]              fetch pack file");
    println!("  ls-remote <url> [options]          list remote refs");
    println!("  pack [pack-file]                   inspect pack file");
    println!("  checkout-pack <pack> <dir> [...]   checkout a pack file");
    println!("  pkt-test                           run pkt-line selftest");
}
