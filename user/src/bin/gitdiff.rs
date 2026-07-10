#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::{string::String, vec::Vec};
use user_lib::{AT_FDCWD, OpenFlags, close, open, read};

const DEFAULT_REPO: &str = ".";
const MAX_ARG_LEN: usize = 512;
const MAX_PATH_LEN: usize = 512;
const MAX_INDEX_LEN: usize = 1024 * 1024;
const MAX_FILE_LEN: usize = 2 * 1024 * 1024;
const MAX_OBJECT_FILE_LEN: usize = 2 * 1024 * 1024;
const MAX_OBJECT_SIZE: usize = 2 * 1024 * 1024;

#[derive(Clone)]
struct IndexEntry {
    path: String,
    oid: [u8; 20],
}

#[derive(Clone)]
struct TreeEntry {
    path: String,
    oid: [u8; 20],
}

struct Config {
    repo_dir: &'static str,
    cached: bool,
    paths: Vec<&'static str>,
}

#[unsafe(no_mangle)]
pub fn main_with_args(argc: usize, argv: *const usize) -> i32 {
    let cfg = match parse_args(argc, argv) {
        Some(v) => v,
        None => return -1,
    };
    match run_gitdiff(&cfg) {
        Some(()) => 0,
        None => -1,
    }
}

fn parse_args(argc: usize, argv: *const usize) -> Option<Config> {
    let mut cfg = Config {
        repo_dir: DEFAULT_REPO,
        cached: false,
        paths: Vec::new(),
    };
    let mut repo_set = false;
    let mut i = 1usize;
    while i < argc {
        let arg = argv_str(argv, i)?;
        if arg == "-h" || arg == "--help" {
            print_usage();
            return None;
        } else if arg == "--cached" || arg == "--staged" {
            cfg.cached = true;
        } else if arg == "--repo" {
            i += 1;
            if i >= argc {
                println!("missing value for --repo");
                return None;
            }
            cfg.repo_dir = argv_str(argv, i)?;
            repo_set = true;
        } else if let Some(v) = strip_prefix(arg, "--repo=") {
            cfg.repo_dir = v;
            repo_set = true;
        } else if starts_with(arg, "-") {
            println!("unknown option: {}", arg);
            return None;
        } else if !repo_set && user_lib::git::discover_repository(arg).is_some() {
            cfg.repo_dir = arg;
            repo_set = true;
        } else {
            cfg.paths.push(arg);
        }
        i += 1;
    }
    Some(cfg)
}

fn run_gitdiff(cfg: &Config) -> Option<()> {
    let repo_dir = match user_lib::git::discover_repository(cfg.repo_dir) {
        Some(v) => v,
        None => {
            println!("not a git repository: {}", cfg.repo_dir);
            return None;
        }
    };
    let git_dir = join_path(&repo_dir, ".git")?;
    let index_path = join_path(&git_dir, "index")?;
    let index = read_small_file(&index_path, MAX_INDEX_LEN)?;
    let entries = parse_git_index(&index)?;
    if cfg.cached {
        diff_cached(&git_dir, &entries, &cfg.paths)
    } else {
        diff_worktree(&repo_dir, &git_dir, &entries, &cfg.paths)
    }
}

fn diff_worktree(
    repo_dir: &str,
    git_dir: &str,
    entries: &[IndexEntry],
    paths: &[&str],
) -> Option<()> {
    for entry in entries {
        if !path_selected(&entry.path, paths) {
            continue;
        }
        let old = read_blob(git_dir, &entry.oid)?;
        let full_path = join_path(repo_dir, &entry.path)?;
        let new = read_small_file(&full_path, MAX_FILE_LEN).unwrap_or_else(Vec::new);
        if old != new {
            print_text_diff(&entry.path, &old, &new);
        }
    }
    Some(())
}

fn diff_cached(git_dir: &str, entries: &[IndexEntry], paths: &[&str]) -> Option<()> {
    let head_entries = read_head_tree_entries(git_dir).unwrap_or_else(Vec::new);
    for entry in entries {
        if !path_selected(&entry.path, paths) {
            continue;
        }
        match find_tree_entry(&head_entries, &entry.path) {
            Some(head) if head.oid == entry.oid => {}
            Some(head) => {
                let old = read_blob(git_dir, &head.oid)?;
                let new = read_blob(git_dir, &entry.oid)?;
                print_text_diff(&entry.path, &old, &new);
            }
            None => {
                let new = read_blob(git_dir, &entry.oid)?;
                print_text_diff(&entry.path, &[], &new);
            }
        }
    }
    for head in head_entries {
        if !path_selected(&head.path, paths) || find_index_entry(entries, &head.path).is_some() {
            continue;
        }
        let old = read_blob(git_dir, &head.oid)?;
        print_text_diff(&head.path, &old, &[]);
    }
    Some(())
}

fn print_text_diff(path: &str, old: &[u8], new: &[u8]) {
    println!("diff --git a/{} b/{}", path, path);
    println!("--- a/{}", path);
    println!("+++ b/{}", path);
    print_lines('-', old);
    print_lines('+', new);
}

fn print_lines(prefix: char, data: &[u8]) {
    if data.is_empty() {
        return;
    }
    let mut start = 0usize;
    while start < data.len() {
        let mut end = start;
        while end < data.len() && data[end] != b'\n' {
            end += 1;
        }
        print!("{}", prefix);
        print_lossy(&data[start..end]);
        println!("");
        start = if end < data.len() { end + 1 } else { end };
    }
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

fn path_selected(path: &str, filters: &[&str]) -> bool {
    if filters.is_empty() {
        return true;
    }
    for filter in filters {
        let filter = normalize_filter(filter);
        if path == filter || starts_with_path(path, filter) {
            return true;
        }
    }
    false
}

fn normalize_filter(path: &str) -> &str {
    strip_prefix(path, "./").unwrap_or(path)
}

fn starts_with_path(path: &str, prefix: &str) -> bool {
    path.len() > prefix.len()
        && path.as_bytes().get(prefix.len()) == Some(&b'/')
        && starts_with(path, prefix)
}

fn read_head_tree_entries(git_dir: &str) -> Option<Vec<TreeEntry>> {
    let head_oid = read_head_oid(git_dir)?;
    let object = read_loose_object(git_dir, &head_oid)?;
    let commit = parse_loose_object(&object, "commit")?;
    let tree_oid = commit_tree_oid(commit)?;
    let mut entries = Vec::new();
    collect_tree_entries(git_dir, &tree_oid, "", &mut entries)?;
    Some(entries)
}

fn collect_tree_entries(
    git_dir: &str,
    tree_oid: &[u8; 20],
    prefix: &str,
    out: &mut Vec<TreeEntry>,
) -> Option<()> {
    let object = read_loose_object(git_dir, tree_oid)?;
    let tree = parse_loose_object(&object, "tree")?;
    let mut pos = 0usize;
    while pos < tree.len() {
        let mode_start = pos;
        while pos < tree.len() && tree[pos] != b' ' {
            pos += 1;
        }
        if pos >= tree.len() {
            return None;
        }
        let mode = core::str::from_utf8(&tree[mode_start..pos]).ok()?;
        pos += 1;
        let name_start = pos;
        while pos < tree.len() && tree[pos] != 0 {
            pos += 1;
        }
        if pos + 21 > tree.len() {
            return None;
        }
        let name = core::str::from_utf8(&tree[name_start..pos]).ok()?;
        pos += 1;
        let mut oid = [0u8; 20];
        oid.copy_from_slice(&tree[pos..pos + 20]);
        pos += 20;
        let path = join_rel_path(prefix, name)?;
        if mode == "40000" {
            collect_tree_entries(git_dir, &oid, &path, out)?;
        } else {
            out.push(TreeEntry { path, oid });
        }
    }
    Some(())
}

fn read_head_oid(git_dir: &str) -> Option<[u8; 20]> {
    let head_path = join_path(git_dir, "HEAD")?;
    let head_data = read_small_file(&head_path, 256)?;
    let head = trim_ascii_str(&head_data)?;
    if let Some(ref_name) = strip_prefix(head, "ref: ") {
        let ref_path = join_path(git_dir, ref_name)?;
        let ref_data = read_small_file(&ref_path, 256)?;
        let oid = trim_ascii_str(&ref_data)?;
        return parse_hex_oid(oid.as_bytes());
    }
    parse_hex_oid(head.as_bytes())
}

fn commit_tree_oid(data: &[u8]) -> Option<[u8; 20]> {
    let prefix = b"tree ";
    if data.len() < prefix.len() + 40 || &data[..prefix.len()] != prefix {
        return None;
    }
    parse_hex_oid(&data[prefix.len()..prefix.len() + 40])
}

fn read_blob(git_dir: &str, oid: &[u8; 20]) -> Option<Vec<u8>> {
    let object = read_loose_object(git_dir, oid)?;
    Some(Vec::from(parse_loose_object(&object, "blob")?))
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
        return None;
    }
    let size = parse_usize(&header[space + 1..])?;
    let body = &object[nul + 1..];
    if body.len() != size || body.len() > MAX_OBJECT_SIZE {
        return None;
    }
    Some(body)
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
        if path_bytes.is_empty() || path_bytes.iter().any(|&b| b == 0) {
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

fn find_tree_entry<'a>(entries: &'a [TreeEntry], path: &str) -> Option<&'a TreeEntry> {
    entries.iter().find(|entry| entry.path == path)
}

fn find_index_entry<'a>(entries: &'a [IndexEntry], path: &str) -> Option<&'a IndexEntry> {
    entries.iter().find(|entry| entry.path == path)
}

fn inflate_zlib_stored(input: &[u8], out: &mut Vec<u8>) -> Option<()> {
    if input.len() < 6 || input[0] != 0x78 {
        println!("unsupported loose object compression");
        return None;
    }
    let mut pos = 2usize;
    loop {
        if pos >= input.len() {
            return None;
        }
        let header = input[pos];
        pos += 1;
        let final_block = header & 1 != 0;
        let block_type = (header >> 1) & 0x03;
        if block_type != 0 {
            println!("unsupported loose object deflate block");
            return None;
        }
        if pos + 4 > input.len() {
            return None;
        }
        let len = u16::from_le_bytes([input[pos], input[pos + 1]]) as usize;
        let nlen = u16::from_le_bytes([input[pos + 2], input[pos + 3]]);
        if nlen != !(len as u16) {
            return None;
        }
        pos += 4;
        if pos + len > input.len() || out.len() + len > MAX_OBJECT_SIZE {
            return None;
        }
        out.extend_from_slice(&input[pos..pos + len]);
        pos += len;
        if final_block {
            break;
        }
    }
    if pos + 4 > input.len() {
        return None;
    }
    let got = read_be_u32(input, pos)?;
    if got != adler32(out) {
        return None;
    }
    Some(())
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

fn find_byte(input: &[u8], byte: u8) -> Option<usize> {
    input.iter().position(|&b| b == byte)
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
    println!("usage: git diff [repo-dir] [--cached] [path...]");
    println!("       git diff --repo DIR [--cached] [path...]");
}
