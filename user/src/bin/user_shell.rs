#![no_std]
#![no_main]
#![allow(clippy::println_empty_string)]

extern crate alloc;

#[macro_use]
extern crate user_lib;

use alloc::string::String;
use alloc::vec::Vec;
use user_lib::console::getchar;
use user_lib::{chdir, execve, exit, fork, getcwd, waitpid,getpid, setpgid, ioctl};
const TIOCSPGRP: usize = 0x5410;
const LF: u8 = 0x0au8;
const CR: u8 = 0x0du8;
const DL: u8 = 0x7fu8;
const BS: u8 = 0x08u8;

fn print_prompt() {
    let mut buf = [0u8; 128];
    if getcwd(&mut buf, 128) >= 0 {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let path = core::str::from_utf8(&buf[..len]).unwrap_or("unknown");
        print!(
            "\x1b[1m\x1b[32mroot@kairix\x1b[0m:\x1b[1m\x1b[34m{}\x1b[0m$ ",
            path
        );
    } else {
        print!("\x1b[1m\x1b[32mroot@kairix\x1b[0m:\x1b[1m\x1b[31m?\x1b[0m$ ");
    }
}

fn parse_args(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for c in line.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
            }
            ' ' | '\t' | '\n' | '\r' if !in_quotes => {
                if !current.is_empty() {
                    args.push(core::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

fn handle_builtin(args: &[String]) -> bool {
    if args.is_empty() {
        return true;
    }
    match args[0].as_str() {
        "cd" => {
            let target = if args.len() > 1 { args[1].as_str() } else { "/" };
            if chdir(target) < 0 {
                println!("cd: {}: No such file or directory", target);
            }
            true
        }
        "exit" => {
            println!("Bye!");
            true
        }
        "help" => {
            println!("Built-in commands: cd, exit, help");
            true
        }
        _ => false,
    }
}


fn execute_external(args: &[String]) {
    let pid = fork();
    if pid == 0 {
        let my_pid = getpid() as i32;
        setpgid(0, 0); 
        ioctl(0, TIOCSPGRP, &my_pid as *const i32 as usize);
        let cmd = &args[0];
        let args_str: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let env= [".","/","/bin","/musl", "/musl/basic"]; 
        if cmd.contains('/') {
            execve(cmd, &args_str, &[]);
        } else {
            for path in env.iter() {
                let mut full_path = String::from(*path);
                if !full_path.ends_with('/') {
                    full_path.push('/');
                }
                full_path.push_str(cmd);
                println!("full path {}", full_path);
                execve(&full_path, &args_str, &[]);
            }
        }
        println!("Command not found: {}", cmd);
        exit(-4);
    } else {
        let child_pid = pid as i32;
        let my_pid = getpid() as i32;
        setpgid(child_pid, child_pid);
        ioctl(0, TIOCSPGRP, &child_pid as *const i32 as usize);
        let mut exit_code: i32 = 0;
        let exit_pid = waitpid(pid as usize, &mut exit_code);
        assert_eq!(pid, exit_pid);
        ioctl(0, TIOCSPGRP, &my_pid as *const i32 as usize);
    }
}
#[unsafe(no_mangle)]
pub fn main() -> i32 {
    let my_pid = getpid() as i32;
    setpgid(0, 0); 
    ioctl(0, TIOCSPGRP, &my_pid as *const i32 as usize); 
    println!("Rust User Shell is ready!");
    print_prompt();
    let mut line: String = String::new();
    loop {
        let c = getchar();
        match c {
            LF | CR => {
                println!("");
                let args = parse_args(&line);
                if !args.is_empty() {
                    if !handle_builtin(&args) {
                        execute_external(&args);
                    }
                }
                line.clear();
                print_prompt();
            }
            BS | DL => {
                if !line.is_empty() {
                    print!("{} {}", BS as char, BS as char);
                    line.pop();
                }
            }
            _ => {
                print!("{}", c as char);
                line.push(c as char);
            }
        }
    }
}
