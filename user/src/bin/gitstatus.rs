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
const MAX_OBJECT_FILE_LEN: usize = 1024 * 1024;
const MAX_OBJECT_SIZE: usize = 1024 * 1024;
const DT_DIR: u8 = 4;
const DT_REG: u8 = 8;

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
    let repo_dir = resolve_repo_dir(repo_dir)?;
    let git_dir = join_path(&repo_dir, ".git")?;
    let index_path = join_path(&git_dir, "index")?;
    let index = read_small_file(&index_path, MAX_INDEX_LEN)?;
    let entries = parse_git_index(&index)?;
    let head_entries = read_head_tree_entries(&git_dir).unwrap_or_else(Vec::new);
    let mut state = StatusState { changed: false };

    print_staged_changes(&entries, &head_entries, &mut state);
    for entry in &entries {
        let path = join_path(&repo_dir, &entry.path)?;
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

    scan_untracked(&repo_dir, "", &entries, &mut state)?;

    if !state.changed {
        println!("nothing to commit, working tree clean");
    }
    Some(())
}

fn resolve_repo_dir(input: &str) -> Option<String> {
    match user_lib::git::discover_repository(input) {
        Some(v) => Some(v),
        None => {
            println!("not a git repository: {}", input);
            None
        }
    }
}

fn print_staged_changes(
    entries: &[IndexEntry],
    head_entries: &[TreeEntry],
    state: &mut StatusState,
) {
    for entry in entries {
        match find_tree_entry(head_entries, &entry.path) {
            Some(head) if head.oid == entry.oid => {}
            Some(_) => {
                println!("staged: modified {}", entry.path);
                state.changed = true;
            }
            None => {
                println!("staged: added {}", entry.path);
                state.changed = true;
            }
        }
    }
    for head in head_entries {
        if !is_tracked(&head.path, entries) {
            println!("staged: deleted {}", head.path);
            state.changed = true;
        }
    }
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
        if !is_safe_ref_name(ref_name) {
            println!("unsafe HEAD ref");
            return None;
        }
        let ref_path = join_path(git_dir, ref_name)?;
        let ref_data = read_small_file(&ref_path, 256)?;
        let oid = trim_ascii_str(&ref_data)?;
        return parse_hex_oid(oid.as_bytes());
    }
    parse_hex_oid(head.as_bytes())
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

fn commit_tree_oid(data: &[u8]) -> Option<[u8; 20]> {
    let prefix = b"tree ";
    if data.len() < prefix.len() + 40 || &data[..prefix.len()] != prefix {
        return None;
    }
    parse_hex_oid(&data[prefix.len()..prefix.len() + 40])
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

fn find_tree_entry<'a>(entries: &'a [TreeEntry], path: &str) -> Option<&'a TreeEntry> {
    entries.iter().find(|entry| entry.path == path)
}

fn git_blob_oid(data: &[u8]) -> [u8; 20] {
    let mut framed = Vec::new();
    framed.extend_from_slice(b"blob ");
    append_usize(&mut framed, data.len());
    framed.push(0);
    framed.extend_from_slice(data);
    sha1(&framed)
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
    let want = adler32(out);
    if got != want {
        return None;
    }
    Some(())
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
