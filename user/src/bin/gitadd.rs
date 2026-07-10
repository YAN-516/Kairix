#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::{string::String, vec::Vec};
use user_lib::{AT_FDCWD, OpenFlags, close, getdents64, mkdir, open, read, write};

const DEFAULT_REPO: &str = ".";
const MAX_ARG_LEN: usize = 512;
const MAX_PATH_LEN: usize = 512;
const MAX_INDEX_LEN: usize = 1024 * 1024;
const MAX_FILE_LEN: usize = 1024 * 1024;
const DT_DIR: u8 = 4;
const DT_REG: u8 = 8;
const MODE_REG: u32 = 0o100644;

#[derive(Clone)]
struct IndexEntry {
    path: String,
    oid: [u8; 20],
    size: usize,
    mode: u32,
}

struct Config {
    repo_dir: Option<&'static str>,
    paths: Vec<&'static str>,
}

#[unsafe(no_mangle)]
pub fn main_with_args(argc: usize, argv: *const usize) -> i32 {
    let cfg = match parse_args(argc, argv) {
        Some(v) => v,
        None => return -1,
    };

    match run_gitadd(&cfg) {
        Some(()) => 0,
        None => -1,
    }
}

fn parse_args(argc: usize, argv: *const usize) -> Option<Config> {
    let mut cfg = Config {
        repo_dir: None,
        paths: Vec::new(),
    };
    let mut i = 1usize;
    while i < argc {
        let arg = argv_str(argv, i)?;
        if arg == "-h" || arg == "--help" {
            print_usage();
            return None;
        } else if arg == "--repo" {
            i += 1;
            if i >= argc {
                println!("missing value for --repo");
                return None;
            }
            cfg.repo_dir = argv_str(argv, i);
        } else if let Some(v) = strip_prefix(arg, "--repo=") {
            if v.is_empty() {
                println!("invalid repo path");
                return None;
            }
            cfg.repo_dir = Some(v);
        } else if starts_with(arg, "-") {
            println!("unknown option: {}", arg);
            return None;
        } else {
            cfg.paths.push(arg);
        }
        i += 1;
    }
    Some(cfg)
}

fn run_gitadd(cfg: &Config) -> Option<()> {
    let mut repo_input = cfg.repo_dir.unwrap_or(DEFAULT_REPO);
    let mut path_start = 0usize;
    let mut add_dot = false;
    let mut paths_from_cwd = cfg.repo_dir.is_none();

    if cfg.repo_dir.is_none() {
        if cfg.paths.len() > 1 && is_repo_dir(cfg.paths[0]) {
            repo_input = cfg.paths[0];
            path_start = 1;
            paths_from_cwd = false;
        } else if cfg.paths.len() == 1 && is_repo_dir(cfg.paths[0]) {
            repo_input = cfg.paths[0];
            add_dot = true;
            paths_from_cwd = false;
        }
    }

    if cfg.paths.len() == path_start && !add_dot {
        print_usage();
        return None;
    }

    let repo_dir = match user_lib::git::discover_repository(repo_input) {
        Some(v) => v,
        None => {
            println!("not a git repository: {}", repo_input);
            return None;
        }
    };
    let cwd_prefix = if paths_from_cwd {
        let cwd = user_lib::git::current_directory()?;
        user_lib::git::repository_relative_path(&repo_dir, &cwd)?
    } else {
        String::new()
    };

    let git_dir = join_path(&repo_dir, ".git")?;
    let index_path = join_path(&git_dir, "index")?;
    let index = read_small_file(&index_path, MAX_INDEX_LEN)?;
    let mut entries = parse_git_index(&index)?;

    if add_dot {
        add_tree(&repo_dir, "", &git_dir, &mut entries, true)?;
    } else {
        for &path in &cfg.paths[path_start..] {
            let path = normalize_repo_path(&repo_dir, &cwd_prefix, path, paths_from_cwd)?;
            add_path(&repo_dir, &path, &git_dir, &mut entries)?;
        }
    }

    write_git_index(&index_path, &mut entries)?;
    Some(())
}

fn normalize_repo_path(
    repo_dir: &str,
    cwd_prefix: &str,
    path: &str,
    paths_from_cwd: bool,
) -> Option<String> {
    let path = normalize_add_path(path)?;
    if !paths_from_cwd {
        return Some(String::from(path));
    }
    if path.starts_with('/') {
        return user_lib::git::repository_relative_path(repo_dir, path);
    }
    if path == "." {
        return Some(if cwd_prefix.is_empty() {
            String::from(".")
        } else {
            String::from(cwd_prefix)
        });
    }
    if cwd_prefix.is_empty() {
        return Some(String::from(path));
    }
    if cwd_prefix.len() + path.len() + 1 > MAX_PATH_LEN {
        println!("path too long");
        return None;
    }
    let mut out = String::from(cwd_prefix);
    out.push('/');
    out.push_str(path);
    Some(out)
}

fn add_path(
    repo_dir: &str,
    rel_path: &str,
    git_dir: &str,
    entries: &mut Vec<IndexEntry>,
) -> Option<()> {
    if rel_path == "." {
        return add_tree(repo_dir, "", git_dir, entries, true);
    }
    if !is_safe_rel_path(rel_path) {
        println!("unsafe path: {}", rel_path);
        return None;
    }

    let full_path = join_path(repo_dir, rel_path)?;
    if is_directory(&full_path) {
        return add_tree(repo_dir, rel_path, git_dir, entries, true);
    }

    match read_small_file(&full_path, MAX_FILE_LEN) {
        Some(data) => {
            add_blob(git_dir, rel_path, &data, entries)?;
            Some(())
        }
        None => {
            if remove_index_entry(entries, rel_path) {
                println!("deleted: {}", rel_path);
                Some(())
            } else {
                println!("path not found: {}", rel_path);
                None
            }
        }
    }
}

fn add_tree(
    repo_dir: &str,
    rel_dir: &str,
    git_dir: &str,
    entries: &mut Vec<IndexEntry>,
    stage_deletions: bool,
) -> Option<()> {
    let mut seen = Vec::new();
    collect_tree_files(repo_dir, rel_dir, &mut seen)?;
    for path in &seen {
        let full_path = join_path(repo_dir, path)?;
        let data = read_small_file(&full_path, MAX_FILE_LEN)?;
        add_blob(git_dir, path, &data, entries)?;
    }
    if stage_deletions {
        remove_missing_under(repo_dir, rel_dir, &seen, entries);
    }
    Some(())
}

fn add_blob(
    git_dir: &str,
    rel_path: &str,
    data: &[u8],
    entries: &mut Vec<IndexEntry>,
) -> Option<()> {
    let (oid, framed) = git_blob_object(data);
    write_loose_object(git_dir, &oid, &framed)?;
    let existed = upsert_index_entry(entries, rel_path, oid, data.len());
    if existed {
        println!("updated: {}", rel_path);
    } else {
        println!("added: {}", rel_path);
    }
    Some(())
}

fn collect_tree_files(repo_dir: &str, rel_dir: &str, out: &mut Vec<String>) -> Option<()> {
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
        if parse_dirents(repo_dir, rel_dir, &buf[..n as usize], out).is_none() {
            let _ = close(fd);
            return None;
        }
    }
    let _ = close(fd);
    Some(())
}

fn parse_dirents(repo_dir: &str, rel_dir: &str, buf: &[u8], out: &mut Vec<String>) -> Option<()> {
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
                    collect_tree_files(repo_dir, &rel_path, out)?;
                } else if d_type == DT_REG {
                    out.push(rel_path);
                }
            }
        }
        offset += reclen;
    }
    Some(())
}

fn remove_missing_under(
    repo_dir: &str,
    rel_dir: &str,
    seen: &[String],
    entries: &mut Vec<IndexEntry>,
) {
    let mut i = 0usize;
    while i < entries.len() {
        if is_under_dir(&entries[i].path, rel_dir) && !string_list_contains(seen, &entries[i].path)
        {
            let full_path = match join_path(repo_dir, &entries[i].path) {
                Some(v) => v,
                None => return,
            };
            if !file_exists(&full_path) {
                println!("deleted: {}", entries[i].path);
                entries.remove(i);
                continue;
            }
        }
        i += 1;
    }
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
        pos += 24;
        let mode = read_be_u32(data, pos)?;
        pos += 12;
        let size = read_be_u32(data, pos)? as usize;
        pos += 4;
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
        entries.push(IndexEntry {
            path,
            oid,
            size,
            mode,
        });
    }
    Some(entries)
}

fn write_git_index(path: &str, entries: &mut Vec<IndexEntry>) -> Option<()> {
    entries.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    let mut data = Vec::new();
    data.extend_from_slice(b"DIRC");
    append_be_u32(&mut data, 2);
    append_be_u32(&mut data, entries.len() as u32);
    for entry in entries.iter() {
        append_index_entry(&mut data, entry)?;
    }
    let checksum = sha1(&data);
    data.extend_from_slice(&checksum);
    if write_file(path, &data) {
        println!("wrote git index: {} entries", entries.len());
        Some(())
    } else {
        None
    }
}

fn append_index_entry(out: &mut Vec<u8>, entry: &IndexEntry) -> Option<()> {
    let entry_start = out.len();
    append_be_u32(out, 0);
    append_be_u32(out, 0);
    append_be_u32(out, 0);
    append_be_u32(out, 0);
    append_be_u32(out, 0);
    append_be_u32(out, 0);
    append_be_u32(out, entry.mode);
    append_be_u32(out, 0);
    append_be_u32(out, 0);
    append_be_u32(out, entry.size.min(u32::MAX as usize) as u32);
    out.extend_from_slice(&entry.oid);
    let path = entry.path.as_bytes();
    if path.is_empty() || path.iter().any(|&b| b == 0) {
        println!("invalid index path");
        return None;
    }
    append_be_u16(out, path.len().min(0x0fff) as u16);
    out.extend_from_slice(path);
    out.push(0);
    while (out.len() - entry_start) % 8 != 0 {
        out.push(0);
    }
    Some(())
}

fn upsert_index_entry(
    entries: &mut Vec<IndexEntry>,
    path: &str,
    oid: [u8; 20],
    size: usize,
) -> bool {
    for entry in entries.iter_mut() {
        if entry.path == path {
            entry.oid = oid;
            entry.size = size;
            entry.mode = MODE_REG;
            return true;
        }
    }
    entries.push(IndexEntry {
        path: String::from(path),
        oid,
        size,
        mode: MODE_REG,
    });
    false
}

fn remove_index_entry(entries: &mut Vec<IndexEntry>, path: &str) -> bool {
    let mut i = 0usize;
    while i < entries.len() {
        if entries[i].path == path {
            entries.remove(i);
            return true;
        }
        i += 1;
    }
    false
}

fn write_loose_object(git_dir: &str, oid: &[u8; 20], framed: &[u8]) -> Option<()> {
    let oid_hex = oid_to_hex(oid);
    let objects_dir = join_path(git_dir, "objects")?;
    let object_dir = join_path(&objects_dir, &oid_hex[..2])?;
    let _ = mkdir(&object_dir, 0o755);
    let object_path = join_path(&object_dir, &oid_hex[2..])?;
    let compressed = zlib_store(framed);
    if write_file(&object_path, &compressed) {
        Some(())
    } else {
        None
    }
}

fn git_blob_object(data: &[u8]) -> ([u8; 20], Vec<u8>) {
    let mut framed = Vec::new();
    framed.extend_from_slice(b"blob ");
    append_usize(&mut framed, data.len());
    framed.push(0);
    framed.extend_from_slice(data);
    (sha1(&framed), framed)
}

fn is_repo_dir(path: &str) -> bool {
    let git_dir = match join_path(path, ".git") {
        Some(v) => v,
        None => return false,
    };
    let index_path = match join_path(&git_dir, "index") {
        Some(v) => v,
        None => return false,
    };
    let fd = open(AT_FDCWD, &index_path, OpenFlags::RDONLY, 0);
    if fd >= 0 {
        let _ = close(fd as usize);
        true
    } else {
        false
    }
}

fn is_directory(path: &str) -> bool {
    let fd = open(
        AT_FDCWD,
        path,
        OpenFlags::RDONLY | OpenFlags::O_DIRECTORY,
        0,
    );
    if fd >= 0 {
        let _ = close(fd as usize);
        true
    } else {
        false
    }
}

fn file_exists(path: &str) -> bool {
    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd >= 0 {
        let _ = close(fd as usize);
        true
    } else {
        false
    }
}

fn should_visit_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && name != ".git"
}

fn normalize_add_path(path: &str) -> Option<&str> {
    if path.is_empty() {
        None
    } else if let Some(rest) = strip_prefix(path, "./") {
        Some(rest)
    } else {
        Some(path)
    }
}

fn is_under_dir(path: &str, dir: &str) -> bool {
    if dir.is_empty() {
        return true;
    }
    path == dir
        || (path.len() > dir.len()
            && path.as_bytes().get(dir.len()) == Some(&b'/')
            && starts_with(path, dir))
}

fn string_list_contains(list: &[String], value: &str) -> bool {
    for item in list {
        if item == value {
            return true;
        }
    }
    false
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

fn write_file(path: &str, data: &[u8]) -> bool {
    let fd = open(
        AT_FDCWD,
        path,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::WRONLY,
        0o644,
    );
    if fd < 0 {
        println!("open output failed: {}", path);
        return false;
    }
    let fd = fd as usize;
    let mut written = 0usize;
    while written < data.len() {
        let n = write(fd, &data[written..]);
        if n <= 0 {
            println!("write output failed: {}", path);
            let _ = close(fd);
            return false;
        }
        written += n as usize;
    }
    let _ = close(fd);
    true
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

fn is_safe_rel_path(path: &str) -> bool {
    is_safe_rel_path_bytes(path.as_bytes())
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

fn zlib_store(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x78);
    out.push(0x01);
    let mut pos = 0usize;
    while pos < input.len() {
        let remaining = input.len() - pos;
        let chunk_len = remaining.min(65535);
        let final_block = pos + chunk_len == input.len();
        out.push(if final_block { 1 } else { 0 });
        let len = chunk_len as u16;
        let nlen = !len;
        out.push((len & 0xff) as u8);
        out.push((len >> 8) as u8);
        out.push((nlen & 0xff) as u8);
        out.push((nlen >> 8) as u8);
        out.extend_from_slice(&input[pos..pos + chunk_len]);
        pos += chunk_len;
    }
    if input.is_empty() {
        out.push(1);
        out.extend_from_slice(&[0, 0, 0xff, 0xff]);
    }
    let sum = adler32(input);
    out.extend_from_slice(&sum.to_be_bytes());
    out
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

fn append_be_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn append_be_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
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
    println!("usage: git add [--repo DIR] <path>...");
    println!("       git add <repo-dir> <path>...");
    println!("       cd <repo-dir>; git add <path>...");
    println!("examples:");
    println!("  git add --repo /tmp/repo README.md");
    println!("  git add --repo /tmp/repo .");
}
