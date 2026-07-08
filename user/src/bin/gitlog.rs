#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::{string::String, vec::Vec};
use user_lib::{AT_FDCWD, OpenFlags, close, open, read};

const DEFAULT_REPO: &str = ".";
const MAX_ARG_LEN: usize = 512;
const MAX_REF_FILE_LEN: usize = 256;
const MAX_OBJECT_FILE_LEN: usize = 1024 * 1024;
const MAX_OBJECT_SIZE: usize = 1024 * 1024;
const MAX_COMMITS: usize = 64;

struct Config {
    repo_dir: &'static str,
    max_count: usize,
}

#[unsafe(no_mangle)]
pub fn main_with_args(argc: usize, argv: *const usize) -> i32 {
    let mut cfg = Config {
        repo_dir: DEFAULT_REPO,
        max_count: MAX_COMMITS,
    };
    if !parse_args(argc, argv, &mut cfg) {
        return -1;
    }

    match run_gitlog(&cfg) {
        Some(()) => 0,
        None => -1,
    }
}

fn parse_args(argc: usize, argv: *const usize, cfg: &mut Config) -> bool {
    let mut positional = 0usize;
    let mut i = 1usize;
    while i < argc {
        let arg = match argv_str(argv, i) {
            Some(v) => v,
            None => {
                println!("invalid argument");
                return false;
            }
        };
        if arg == "-h" || arg == "--help" {
            print_usage();
            return false;
        } else if arg == "-n" || arg == "--max-count" {
            i += 1;
            if i >= argc {
                println!("missing value for {}", arg);
                return false;
            }
            cfg.max_count = match argv_str(argv, i).and_then(parse_usize) {
                Some(v) if v > 0 => v,
                _ => {
                    println!("invalid max count");
                    return false;
                }
            };
        } else if let Some(v) = strip_prefix(arg, "--max-count=") {
            cfg.max_count = match parse_usize(v) {
                Some(v) if v > 0 => v,
                _ => {
                    println!("invalid max count");
                    return false;
                }
            };
        } else if starts_with(arg, "-") {
            println!("unknown option: {}", arg);
            return false;
        } else if positional == 0 {
            cfg.repo_dir = arg;
            positional += 1;
        } else {
            println!("too many arguments");
            return false;
        }
        i += 1;
    }
    true
}

fn run_gitlog(cfg: &Config) -> Option<()> {
    let git_dir = join_path(cfg.repo_dir, ".git")?;
    let mut oid = read_head_oid(&git_dir)?;
    let mut count = 0usize;
    while count < cfg.max_count {
        let object = read_loose_object(&git_dir, &oid)?;
        let commit = parse_loose_object(&object, "commit")?;
        print_commit(&oid, commit);
        count += 1;
        match commit_parent_oid(commit) {
            Some(parent) => oid = parent,
            None => break,
        }
    }
    Some(())
}

fn read_head_oid(git_dir: &str) -> Option<[u8; 20]> {
    let head_path = join_path(git_dir, "HEAD")?;
    let head_data = read_small_file(&head_path, MAX_REF_FILE_LEN)?;
    let head = trim_ascii_str(&head_data)?;
    if let Some(ref_name) = strip_prefix(head, "ref: ") {
        if !is_safe_ref_name(ref_name) {
            println!("unsafe HEAD ref");
            return None;
        }
        let ref_path = join_path(git_dir, ref_name)?;
        let ref_data = read_small_file(&ref_path, MAX_REF_FILE_LEN)?;
        let oid = trim_ascii_str(&ref_data)?;
        return parse_hex_oid_str(oid);
    }
    parse_hex_oid_str(head)
}

fn read_loose_object(git_dir: &str, oid: &[u8; 20]) -> Option<Vec<u8>> {
    let oid_hex = oid_to_hex(oid);
    let objects_dir = join_path(git_dir, "objects")?;
    let object_dir = join_path(&objects_dir, &oid_hex[..2])?;
    let object_path = join_path(&object_dir, &oid_hex[2..])?;
    let compressed = read_small_file(&object_path, MAX_OBJECT_FILE_LEN)?;
    let mut out = Vec::new();
    inflate_zlib_stored(&compressed, &mut out)?;
    Some(out)
}

fn parse_loose_object<'a>(object: &'a [u8], expected_type: &str) -> Option<&'a [u8]> {
    let nul = find_byte(object, 0)?;
    let header = core::str::from_utf8(&object[..nul]).ok()?;
    let space = header.as_bytes().iter().position(|&b| b == b' ')?;
    let typ = &header[..space];
    if typ != expected_type {
        println!("unexpected object type: {}", typ);
        return None;
    }
    let size = parse_usize(&header[space + 1..])?;
    let body = &object[nul + 1..];
    if body.len() != size {
        println!("loose object size mismatch");
        return None;
    }
    Some(body)
}

fn print_commit(oid: &[u8; 20], commit: &[u8]) {
    print!("commit ");
    print_oid(oid);
    println!("");
    if let Some(author) = commit_line_value(commit, b"author ") {
        print!("Author: ");
        print_lossy(author);
        println!("");
    }
    if let Some(committer) = commit_line_value(commit, b"committer ") {
        print!("Commit: ");
        print_lossy(committer);
        println!("");
    }
    if let Some(message) = commit_message(commit) {
        println!("");
        print_indented_message(message);
    }
    println!("");
}

fn commit_parent_oid(commit: &[u8]) -> Option<[u8; 20]> {
    commit_line_value(commit, b"parent ").and_then(parse_hex_oid)
}

fn commit_line_value<'a>(commit: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    let mut pos = 0usize;
    while pos < commit.len() {
        let start = pos;
        while pos < commit.len() && commit[pos] != b'\n' {
            pos += 1;
        }
        let line = &commit[start..pos];
        if starts_with_bytes(line, prefix) {
            return Some(&line[prefix.len()..]);
        }
        if pos < commit.len() {
            pos += 1;
        }
    }
    None
}

fn commit_message(commit: &[u8]) -> Option<&[u8]> {
    let mut i = 0usize;
    while i + 1 < commit.len() {
        if commit[i] == b'\n' && commit[i + 1] == b'\n' {
            return Some(&commit[i + 2..]);
        }
        i += 1;
    }
    None
}

fn print_indented_message(message: &[u8]) {
    let mut line_start = true;
    for &b in message {
        if line_start {
            print!("    ");
            line_start = false;
        }
        if b == b'\n' {
            println!("");
            line_start = true;
        } else if b.is_ascii_graphic() || b == b' ' || b == b'\t' {
            print!("{}", b as char);
        } else {
            print!(".");
        }
    }
    if !message.ends_with(b"\n") {
        println!("");
    }
}

fn inflate_zlib_stored(input: &[u8], out: &mut Vec<u8>) -> Option<()> {
    if input.len() < 6 {
        println!("invalid zlib stream");
        return None;
    }
    let cmf = input[0];
    let flg = input[1];
    let header = ((cmf as u16) << 8) | flg as u16;
    if cmf & 0x0f != 8 || header % 31 != 0 || flg & 0x20 != 0 {
        println!("invalid zlib header");
        return None;
    }

    let mut pos = 2usize;
    loop {
        if pos + 5 > input.len() {
            println!("truncated deflate block");
            return None;
        }
        let block = input[pos];
        pos += 1;
        let final_block = block & 1 != 0;
        let block_type = (block >> 1) & 0x03;
        if block_type != 0 {
            println!("unsupported loose object deflate block");
            return None;
        }
        let len = input[pos] as usize | ((input[pos + 1] as usize) << 8);
        let nlen = input[pos + 2] as u16 | ((input[pos + 3] as u16) << 8);
        pos += 4;
        if nlen != !(len as u16) {
            println!("invalid stored block length");
            return None;
        }
        if pos + len + 4 > input.len() {
            println!("truncated stored block");
            return None;
        }
        if out.len() + len > MAX_OBJECT_SIZE {
            println!("object too large");
            return None;
        }
        out.extend_from_slice(&input[pos..pos + len]);
        pos += len;
        if final_block {
            break;
        }
    }

    if pos + 4 > input.len() {
        println!("truncated zlib checksum");
        return None;
    }
    let got = read_be_u32(input, pos);
    let want = adler32(out);
    if got != want {
        println!("zlib adler32 mismatch");
        return None;
    }
    Some(())
}

fn read_small_file(path: &str, max_len: usize) -> Option<Vec<u8>> {
    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        println!("open failed: {}", path);
        return None;
    }
    let fd = fd as usize;
    let mut out = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        let n = read(fd, &mut buf);
        if n < 0 {
            println!("read failed: {}", n);
            let _ = close(fd);
            return None;
        }
        if n == 0 {
            break;
        }
        if out.len() + n as usize > max_len {
            println!("file too large");
            let _ = close(fd);
            return None;
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
    let _ = close(fd);
    Some(out)
}

fn join_path(parent: &str, name: &str) -> Option<String> {
    if parent.len() + name.len() + 2 > MAX_ARG_LEN {
        println!("path too long");
        return None;
    }
    let mut out = String::new();
    out.push_str(parent);
    if !parent.ends_with('/') {
        out.push('/');
    }
    out.push_str(name);
    Some(out)
}

fn trim_ascii_str(input: &[u8]) -> Option<&str> {
    let mut start = 0usize;
    let mut end = input.len();
    while start < end && input[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && input[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    core::str::from_utf8(&input[start..end]).ok()
}

fn is_safe_ref_name(input: &str) -> bool {
    if !starts_with(input, "refs/") {
        return false;
    }
    let mut prev_slash = false;
    for &b in input.as_bytes() {
        if b == b'/' {
            if prev_slash {
                return false;
            }
            prev_slash = true;
            continue;
        }
        prev_slash = false;
        if b == b'.' || b == b'\\' || b == 0 || b <= b' ' {
            return false;
        }
    }
    !input.ends_with('/')
}

fn parse_hex_oid_str(input: &str) -> Option<[u8; 20]> {
    parse_hex_oid(input.as_bytes())
}

fn parse_hex_oid(input: &[u8]) -> Option<[u8; 20]> {
    if input.len() != 40 {
        return None;
    }
    let mut out = [0u8; 20];
    for i in 0..20 {
        out[i] = (hex_value(input[i * 2])? << 4) | hex_value(input[i * 2 + 1])?;
    }
    Some(out)
}

fn oid_to_hex(oid: &[u8; 20]) -> String {
    let mut out = String::new();
    for &b in oid {
        push_hex_byte(&mut out, b);
    }
    out
}

fn print_oid(oid: &[u8; 20]) {
    for &b in oid {
        print!("{:02x}", b);
    }
}

fn push_hex_byte(out: &mut String, b: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(HEX[(b >> 4) as usize] as char);
    out.push(HEX[(b & 0x0f) as usize] as char);
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn parse_usize(input: &str) -> Option<usize> {
    let mut out = 0usize;
    if input.is_empty() {
        return None;
    }
    for b in input.bytes() {
        if !b.is_ascii_digit() {
            return None;
        }
        out = out.checked_mul(10)?.checked_add((b - b'0') as usize)?;
    }
    Some(out)
}

fn print_lossy(input: &[u8]) {
    for &b in input {
        if b.is_ascii_graphic() || b == b' ' || b == b'\t' {
            print!("{}", b as char);
        } else {
            print!(".");
        }
    }
}

fn starts_with_bytes(input: &[u8], prefix: &[u8]) -> bool {
    input.len() >= prefix.len() && &input[..prefix.len()] == prefix
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

fn find_byte(input: &[u8], needle: u8) -> Option<usize> {
    for (idx, &b) in input.iter().enumerate() {
        if b == needle {
            return Some(idx);
        }
    }
    None
}

fn read_be_u32(input: &[u8], offset: usize) -> u32 {
    ((input[offset] as u32) << 24)
        | ((input[offset + 1] as u32) << 16)
        | ((input[offset + 2] as u32) << 8)
        | input[offset + 3] as u32
}

fn adler32(input: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let mut a = 1u32;
    let mut b = 0u32;
    for &byte in input {
        a = (a + byte as u32) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
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
    println!("usage: gitlog [repo-dir] [-n COUNT]");
    println!("example: gitlog /musl/repo");
}
