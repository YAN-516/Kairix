#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::{string::String, vec::Vec};
use user_lib::{AT_FDCWD, OpenFlags, close, getdents64, open, read};

const DEFAULT_REPO: &str = ".";
const MAX_ARG_LEN: usize = 512;
const MAX_PATH_LEN: usize = 512;
const MAX_INDEX_LEN: usize = 1024 * 1024;
const MAX_FILE_LEN: usize = 1024 * 1024;
const DT_DIR: u8 = 4;
const DT_REG: u8 = 8;

#[derive(Clone)]
struct IndexEntry {
    path: String,
    oid: [u8; 20],
}

struct StatusState {
    changed: bool,
}

#[unsafe(no_mangle)]
pub fn main_with_args(argc: usize, argv: *const usize) -> i32 {
    let repo_dir = match parse_args(argc, argv) {
        Some(v) => v,
        None => return -1,
    };

    match run_gitstatus(repo_dir) {
        Some(()) => 0,
        None => -1,
    }
}

fn parse_args(argc: usize, argv: *const usize) -> Option<&'static str> {
    let mut repo_dir = DEFAULT_REPO;
    let mut positional = 0usize;
    let mut i = 1usize;
    while i < argc {
        let arg = argv_str(argv, i)?;
        if arg == "-h" || arg == "--help" {
            print_usage();
            return None;
        } else if starts_with(arg, "-") {
            println!("unknown option: {}", arg);
            return None;
        } else if positional == 0 {
            repo_dir = arg;
            positional += 1;
        } else {
            println!("too many arguments");
            return None;
        }
        i += 1;
    }
    Some(repo_dir)
}

fn run_gitstatus(repo_dir: &str) -> Option<()> {
    let git_dir = join_path(repo_dir, ".git")?;
    let index_path = join_path(&git_dir, "index")?;
    let index = read_small_file(&index_path, MAX_INDEX_LEN)?;
    let entries = parse_git_index(&index)?;
    let mut state = StatusState { changed: false };

    for entry in &entries {
        let path = join_path(repo_dir, &entry.path)?;
        match read_small_file(&path, MAX_FILE_LEN) {
            Some(data) => {
                let oid = git_blob_oid(&data);
                if oid != entry.oid {
                    println!("modified: {}", entry.path);
                    state.changed = true;
                }
            }
            None => {
                println!("deleted: {}", entry.path);
                state.changed = true;
            }
        }
    }

    scan_untracked(repo_dir, "", &entries, &mut state)?;

    if !state.changed {
        println!("nothing to commit, working tree clean");
    }
    Some(())
}

fn parse_git_index(data: &[u8]) -> Option<Vec<IndexEntry>> {
    if data.len() < 12 + 20 || &data[..4] != b"DIRC" {
        println!("invalid git index");
        return None;
    }
    let version = read_be_u32(data, 4)?;
    if version != 2 {
        println!("unsupported git index version: {}", version);
        return None;
    }
    let count = read_be_u32(data, 8)? as usize;
    let mut entries = Vec::new();
    let mut pos = 12usize;
    for _ in 0..count {
        let start = pos;
        if pos + 62 > data.len().saturating_sub(20) {
            println!("truncated git index");
            return None;
        }
        pos += 40;
        let mut oid = [0u8; 20];
        oid.copy_from_slice(&data[pos..pos + 20]);
        pos += 20;
        let flags = read_be_u16(data, pos)?;
        pos += 2;
        let name_len = (flags & 0x0fff) as usize;
        if pos + name_len > data.len().saturating_sub(20) {
            println!("truncated git index path");
            return None;
        }
        let path_bytes = &data[pos..pos + name_len];
        if path_bytes.is_empty()
            || path_bytes.iter().any(|&b| b == 0)
            || !is_safe_rel_path_bytes(path_bytes)
        {
            println!("unsafe git index path");
            return None;
        }
        let path = match core::str::from_utf8(path_bytes) {
            Ok(v) => String::from(v),
            Err(_) => {
                println!("non-utf8 git index path");
                return None;
            }
        };
        pos += name_len;
        while pos < data.len().saturating_sub(20) && data[pos] != 0 {
            pos += 1;
        }
        if pos >= data.len().saturating_sub(20) {
            println!("unterminated git index path");
            return None;
        }
        pos += 1;
        while (pos - start) % 8 != 0 {
            if pos >= data.len().saturating_sub(20) {
                println!("truncated git index padding");
                return None;
            }
            pos += 1;
        }
        entries.push(IndexEntry { path, oid });
    }
    Some(entries)
}

fn scan_untracked(
    repo_dir: &str,
    rel_dir: &str,
    entries: &[IndexEntry],
    state: &mut StatusState,
) -> Option<()> {
    let dir_path = if rel_dir.is_empty() {
        String::from(repo_dir)
    } else {
        join_path(repo_dir, rel_dir)?
    };
    let fd = open(
        AT_FDCWD,
        &dir_path,
        OpenFlags::RDONLY | OpenFlags::O_DIRECTORY,
        0,
    );
    if fd < 0 {
        println!("open directory failed: {}", dir_path);
        return None;
    }
    let fd = fd as usize;
    let mut buf = [0u8; 2048];
    loop {
        let n = getdents64(fd, &mut buf);
        if n < 0 {
            println!("read directory failed: {}", dir_path);
            let _ = close(fd);
            return None;
        }
        if n == 0 {
            break;
        }
        if parse_dirents(repo_dir, rel_dir, &buf[..n as usize], entries, state).is_none() {
            let _ = close(fd);
            return None;
        }
    }
    let _ = close(fd);
    Some(())
}

fn parse_dirents(
    repo_dir: &str,
    rel_dir: &str,
    buf: &[u8],
    entries: &[IndexEntry],
    state: &mut StatusState,
) -> Option<()> {
    let mut offset = 0usize;
    while offset < buf.len() {
        if offset + 19 > buf.len() {
            break;
        }
        let reclen = u16::from_ne_bytes([buf[offset + 16], buf[offset + 17]]) as usize;
        if reclen == 0 || offset + reclen > buf.len() {
            break;
        }
        let d_type = buf[offset + 18];
        let name_start = offset + 19;
        let mut name_end = name_start;
        while name_end < offset + reclen && buf[name_end] != 0 {
            name_end += 1;
        }
        if let Ok(name) = core::str::from_utf8(&buf[name_start..name_end]) {
            if should_visit_name(name) {
                let rel_path = join_rel_path(rel_dir, name)?;
                if d_type == DT_DIR {
                    scan_untracked(repo_dir, &rel_path, entries, state)?;
                } else if d_type == DT_REG && !is_tracked(&rel_path, entries) {
                    println!("untracked: {}", rel_path);
                    state.changed = true;
                }
            }
        }
        offset += reclen;
    }
    Some(())
}

fn should_visit_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && name != ".git"
}

fn is_tracked(path: &str, entries: &[IndexEntry]) -> bool {
    for entry in entries {
        if entry.path == path {
            return true;
        }
    }
    false
}

fn git_blob_oid(data: &[u8]) -> [u8; 20] {
    let mut framed = Vec::new();
    framed.extend_from_slice(b"blob ");
    append_usize(&mut framed, data.len());
    framed.push(0);
    framed.extend_from_slice(data);
    sha1(&framed)
}

fn read_small_file(path: &str, max_len: usize) -> Option<Vec<u8>> {
    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        return None;
    }
    let fd = fd as usize;
    let mut out = Vec::new();
    let mut buf = [0u8; 2048];
    loop {
        let n = read(fd, &mut buf);
        if n < 0 {
            let _ = close(fd);
            return None;
        }
        if n == 0 {
            break;
        }
        if out.len() + n as usize > max_len {
            println!("file too large: {}", path);
            let _ = close(fd);
            return None;
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
    let _ = close(fd);
    Some(out)
}

fn join_path(parent: &str, name: &str) -> Option<String> {
    if parent.len() + name.len() + 2 > MAX_PATH_LEN {
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

fn join_rel_path(parent: &str, name: &str) -> Option<String> {
    if name.is_empty() || name.as_bytes().iter().any(|&b| b == b'/' || b == 0) {
        return None;
    }
    if parent.len() + name.len() + 2 > MAX_PATH_LEN {
        println!("path too long");
        return None;
    }
    let mut out = String::new();
    if !parent.is_empty() {
        out.push_str(parent);
        out.push('/');
    }
    out.push_str(name);
    Some(out)
}

fn is_safe_rel_path_bytes(path: &[u8]) -> bool {
    if path.starts_with(b"/") || path.ends_with(b"/") {
        return false;
    }
    let mut start = 0usize;
    while start < path.len() {
        let mut end = start;
        while end < path.len() && path[end] != b'/' {
            end += 1;
        }
        let part = &path[start..end];
        if part.is_empty() || part == b"." || part == b".." {
            return false;
        }
        start = end + 1;
    }
    true
}

fn append_usize(out: &mut Vec<u8>, mut value: usize) {
    let mut tmp = [0u8; 20];
    let mut n = 0usize;
    if value == 0 {
        out.push(b'0');
        return;
    }
    while value > 0 {
        tmp[n] = b'0' + (value % 10) as u8;
        value /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        out.push(tmp[n]);
    }
}

fn read_be_u16(input: &[u8], offset: usize) -> Option<u16> {
    if offset + 2 > input.len() {
        return None;
    }
    Some(((input[offset] as u16) << 8) | input[offset + 1] as u16)
}

fn read_be_u32(input: &[u8], offset: usize) -> Option<u32> {
    if offset + 4 > input.len() {
        return None;
    }
    Some(
        ((input[offset] as u32) << 24)
            | ((input[offset + 1] as u32) << 16)
            | ((input[offset + 2] as u32) << 8)
            | input[offset + 3] as u32,
    )
}

fn sha1(input: &[u8]) -> [u8; 20] {
    let mut h0 = 0x67452301u32;
    let mut h1 = 0xefcdab89u32;
    let mut h2 = 0x98badcfeu32;
    let mut h3 = 0x10325476u32;
    let mut h4 = 0xc3d2e1f0u32;

    let bit_len = (input.len() as u64) * 8;
    let mut msg = Vec::new();
    msg.extend_from_slice(input);
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    for b in bit_len.to_be_bytes() {
        msg.push(b);
    }

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            let j = i * 4;
            w[i] = ((chunk[j] as u32) << 24)
                | ((chunk[j + 1] as u32) << 16)
                | ((chunk[j + 2] as u32) << 8)
                | chunk[j + 3] as u32;
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;

        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5a827999),
                20..=39 => (b ^ c ^ d, 0x6ed9eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
                _ => (b ^ c ^ d, 0xca62c1d6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
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
    println!("usage: gitstatus [repo-dir]");
    println!("default repo: {}", DEFAULT_REPO);
}
