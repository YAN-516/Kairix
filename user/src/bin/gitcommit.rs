#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::{string::String, vec::Vec};
use user_lib::{AT_FDCWD, OpenFlags, close, get_time, mkdir, open, read, write};

const DEFAULT_REPO: &str = ".";
const MAX_ARG_LEN: usize = 512;
const MAX_PATH_LEN: usize = 512;
const MAX_INDEX_LEN: usize = 1024 * 1024;
const MAX_REF_LEN: usize = 256;
const MODE_TREE: &str = "40000";
const DEFAULT_EPOCH_SECONDS: usize = 1783468800; // 2026-07-08 00:00:00 +0000

#[derive(Clone)]
struct IndexEntry {
    path: String,
    oid: [u8; 20],
    mode: u32,
}

struct Config {
    repo_dir: &'static str,
    message: Option<&'static str>,
    date: Option<usize>,
}

#[unsafe(no_mangle)]
pub fn main_with_args(argc: usize, argv: *const usize) -> i32 {
    let cfg = match parse_args(argc, argv) {
        Some(v) => v,
        None => return -1,
    };
    match run_gitcommit(&cfg) {
        Some(()) => 0,
        None => -1,
    }
}

fn parse_args(argc: usize, argv: *const usize) -> Option<Config> {
    let mut cfg = Config {
        repo_dir: DEFAULT_REPO,
        message: None,
        date: None,
    };
    let mut positional = 0usize;
    let mut i = 1usize;
    while i < argc {
        let arg = argv_str(argv, i)?;
        if arg == "-h" || arg == "--help" {
            print_usage();
            return None;
        } else if arg == "-m" || arg == "--message" {
            i += 1;
            if i >= argc {
                println!("missing commit message");
                return None;
            }
            cfg.message = argv_str(argv, i);
        } else if let Some(v) = strip_prefix(arg, "--message=") {
            cfg.message = Some(v);
        } else if let Some(v) = strip_prefix(arg, "-m") {
            if v.is_empty() {
                println!("missing commit message");
                return None;
            }
            cfg.message = Some(v);
        } else if arg == "--date" {
            i += 1;
            if i >= argc {
                println!("missing value for --date");
                return None;
            }
            cfg.date = match argv_str(argv, i).and_then(parse_date_arg) {
                Some(v) => Some(v),
                None => {
                    println!("invalid date; use YYYY-MM-DD HH:MM:SS or epoch seconds");
                    return None;
                }
            };
        } else if let Some(v) = strip_prefix(arg, "--date=") {
            cfg.date = match parse_date_arg(v) {
                Some(v) => Some(v),
                None => {
                    println!("invalid date; use YYYY-MM-DD HH:MM:SS or epoch seconds");
                    return None;
                }
            };
        } else if arg == "--repo" {
            i += 1;
            if i >= argc {
                println!("missing value for --repo");
                return None;
            }
            cfg.repo_dir = argv_str(argv, i)?;
        } else if let Some(v) = strip_prefix(arg, "--repo=") {
            cfg.repo_dir = v;
        } else if starts_with(arg, "-") {
            println!("unknown option: {}", arg);
            return None;
        } else if positional == 0 {
            cfg.repo_dir = arg;
            positional += 1;
        } else {
            println!("too many arguments");
            return None;
        }
        i += 1;
    }
    if cfg.message.unwrap_or("").is_empty() {
        println!("missing commit message");
        print_usage();
        return None;
    }
    Some(cfg)
}

fn run_gitcommit(cfg: &Config) -> Option<()> {
    let repo_dir = resolve_repo_dir(cfg.repo_dir)?;
    let git_dir = join_path(&repo_dir, ".git")?;
    let index_path = join_path(&git_dir, "index")?;
    let index = read_small_file(&index_path, MAX_INDEX_LEN)?;
    let entries = parse_git_index(&index)?;
    if entries.is_empty() {
        println!("nothing to commit");
        return Some(());
    }

    let parent = read_head_oid(&git_dir);
    let tree_oid = write_tree_for_prefix(&git_dir, &entries, "")?;
    let seconds = commit_time_seconds(cfg);
    let commit_oid =
        write_commit_object(&git_dir, &tree_oid, parent.as_ref(), cfg.message?, seconds)?;
    update_head_ref(&git_dir, &commit_oid)?;

    print!("[commit ");
    print_oid(&commit_oid);
    println!("] {}", cfg.message?);
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

fn write_tree_for_prefix(git_dir: &str, entries: &[IndexEntry], prefix: &str) -> Option<[u8; 20]> {
    let mut body = Vec::new();
    let mut i = 0usize;
    while i < entries.len() {
        if !is_direct_child(prefix, &entries[i].path) {
            i += 1;
            continue;
        }
        let name = child_name(prefix, &entries[i].path)?;
        if child_has_slash(name) {
            let dir = first_component(name)?;
            let child_prefix = join_rel_path(prefix, dir)?;
            let tree_oid = write_tree_for_prefix(git_dir, entries, &child_prefix)?;
            append_tree_entry(&mut body, MODE_TREE, dir, &tree_oid);
            while i < entries.len() && is_under_dir(&entries[i].path, &child_prefix) {
                i += 1;
            }
        } else {
            let mode = index_mode_str(entries[i].mode);
            append_tree_entry(&mut body, mode, name, &entries[i].oid);
            i += 1;
        }
    }
    let (oid, framed) = git_object("tree", &body);
    write_loose_object(git_dir, &oid, &framed)?;
    Some(oid)
}

fn write_commit_object(
    git_dir: &str,
    tree_oid: &[u8; 20],
    parent: Option<&[u8; 20]>,
    message: &str,
    seconds: usize,
) -> Option<[u8; 20]> {
    let author = "Kairix <kairix@example.local>";
    let mut body = Vec::new();
    body.extend_from_slice(b"tree ");
    append_oid_hex(&mut body, tree_oid);
    body.push(b'\n');
    if let Some(parent) = parent {
        body.extend_from_slice(b"parent ");
        append_oid_hex(&mut body, parent);
        body.push(b'\n');
    }
    body.extend_from_slice(b"author ");
    body.extend_from_slice(author.as_bytes());
    body.push(b' ');
    append_usize(&mut body, seconds);
    body.extend_from_slice(b" +0000\ncommitter ");
    body.extend_from_slice(author.as_bytes());
    body.push(b' ');
    append_usize(&mut body, seconds);
    body.extend_from_slice(b" +0000\n\n");
    body.extend_from_slice(message.as_bytes());
    body.push(b'\n');

    let (oid, framed) = git_object("commit", &body);
    write_loose_object(git_dir, &oid, &framed)?;
    OkOid(oid).into()
}

fn commit_time_seconds(cfg: &Config) -> usize {
    if let Some(seconds) = cfg.date {
        return seconds;
    }
    let now_ms = get_time();
    if now_ms <= 0 {
        return DEFAULT_EPOCH_SECONDS;
    }
    let seconds = (now_ms as usize) / 1000;
    if seconds < 1_000_000_000 {
        DEFAULT_EPOCH_SECONDS + seconds
    } else {
        seconds
    }
}

fn parse_date_arg(input: &str) -> Option<usize> {
    parse_usize(input).or_else(|| parse_datetime_utc(input))
}

fn parse_datetime_utc(input: &str) -> Option<usize> {
    let bytes = input.as_bytes();
    if bytes.len() != 19 {
        return None;
    }
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || (bytes[10] != b' ' && bytes[10] != b'T')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year = parse_fixed_digits(bytes, 0, 4)?;
    let month = parse_fixed_digits(bytes, 5, 2)?;
    let day = parse_fixed_digits(bytes, 8, 2)?;
    let hour = parse_fixed_digits(bytes, 11, 2)?;
    let minute = parse_fixed_digits(bytes, 14, 2)?;
    let second = parse_fixed_digits(bytes, 17, 2)?;
    datetime_to_epoch_seconds(year, month, day, hour, minute, second)
}

fn parse_fixed_digits(input: &[u8], start: usize, len: usize) -> Option<usize> {
    if start + len > input.len() {
        return None;
    }
    let mut out = 0usize;
    for &b in &input[start..start + len] {
        if !b.is_ascii_digit() {
            return None;
        }
        out = out.checked_mul(10)?.checked_add((b - b'0') as usize)?;
    }
    Some(out)
}

fn datetime_to_epoch_seconds(
    year: usize,
    month: usize,
    day: usize,
    hour: usize,
    minute: usize,
    second: usize,
) -> Option<usize> {
    if year < 1970 || month < 1 || month > 12 || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let dim = days_in_month(year, month);
    if day < 1 || day > dim {
        return None;
    }
    let mut days = 0usize;
    let mut y = 1970usize;
    while y < year {
        days = days.checked_add(if is_leap_year(y) { 366 } else { 365 })?;
        y += 1;
    }
    let mut m = 1usize;
    while m < month {
        days = days.checked_add(days_in_month(year, m))?;
        m += 1;
    }
    days = days.checked_add(day - 1)?;
    days.checked_mul(86400)?
        .checked_add(hour.checked_mul(3600)?)?
        .checked_add(minute.checked_mul(60)?)?
        .checked_add(second)
}

fn days_in_month(year: usize, month: usize) -> usize {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap_year(year: usize) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

struct OkOid([u8; 20]);

impl From<OkOid> for Option<[u8; 20]> {
    fn from(value: OkOid) -> Self {
        Some(value.0)
    }
}

fn update_head_ref(git_dir: &str, oid: &[u8; 20]) -> Option<()> {
    let head_path = join_path(git_dir, "HEAD")?;
    let head_data = read_small_file(&head_path, MAX_REF_LEN)?;
    let head = trim_ascii_str(&head_data)?;
    let mut data = Vec::new();
    append_oid_hex(&mut data, oid);
    data.push(b'\n');
    if let Some(ref_name) = strip_prefix(head, "ref: ") {
        if !is_safe_ref_name(ref_name) {
            println!("unsafe HEAD ref");
            return None;
        }
        mkdir_ref_parents(git_dir, ref_name)?;
        let ref_path = join_path(git_dir, ref_name)?;
        if write_file(&ref_path, &data) {
            Some(())
        } else {
            None
        }
    } else if write_file(&head_path, &data) {
        Some(())
    } else {
        None
    }
}

fn parse_git_index(data: &[u8]) -> Option<Vec<IndexEntry>> {
    if data.len() < 12 + 20 || &data[..4] != b"DIRC" || read_be_u32(data, 4)? != 2 {
        println!("invalid git index");
        return None;
    }
    let count = read_be_u32(data, 8)? as usize;
    let mut entries = Vec::new();
    let mut pos = 12usize;
    for _ in 0..count {
        let start = pos;
        if pos + 62 > data.len().saturating_sub(20) {
            return None;
        }
        pos += 24;
        let mode = read_be_u32(data, pos)?;
        pos += 16;
        let mut oid = [0u8; 20];
        oid.copy_from_slice(&data[pos..pos + 20]);
        pos += 20;
        let flags = read_be_u16(data, pos)?;
        pos += 2;
        let name_len = (flags & 0x0fff) as usize;
        if pos + name_len > data.len().saturating_sub(20) {
            return None;
        }
        let path_bytes = &data[pos..pos + name_len];
        if !is_safe_rel_path_bytes(path_bytes) {
            return None;
        }
        let path = String::from(core::str::from_utf8(path_bytes).ok()?);
        pos += name_len;
        while pos < data.len().saturating_sub(20) && data[pos] != 0 {
            pos += 1;
        }
        pos += 1;
        while (pos - start) % 8 != 0 {
            pos += 1;
        }
        entries.push(IndexEntry { path, oid, mode });
    }
    entries.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    Some(entries)
}

fn read_head_oid(git_dir: &str) -> Option<[u8; 20]> {
    let head_path = join_path(git_dir, "HEAD")?;
    let head_data = read_small_file(&head_path, MAX_REF_LEN)?;
    let head = trim_ascii_str(&head_data)?;
    if let Some(ref_name) = strip_prefix(head, "ref: ") {
        if !is_safe_ref_name(ref_name) {
            return None;
        }
        let ref_path = join_path(git_dir, ref_name)?;
        let ref_data = read_small_file(&ref_path, MAX_REF_LEN)?;
        return parse_hex_oid(trim_ascii_str(&ref_data)?.as_bytes());
    }
    parse_hex_oid(head.as_bytes())
}

fn append_tree_entry(out: &mut Vec<u8>, mode: &str, name: &str, oid: &[u8; 20]) {
    out.extend_from_slice(mode.as_bytes());
    out.push(b' ');
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    out.extend_from_slice(oid);
}

fn git_object(typ: &str, data: &[u8]) -> ([u8; 20], Vec<u8>) {
    let mut framed = Vec::new();
    framed.extend_from_slice(typ.as_bytes());
    framed.push(b' ');
    append_usize(&mut framed, data.len());
    framed.push(0);
    framed.extend_from_slice(data);
    (sha1(&framed), framed)
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

fn mkdir_ref_parents(git_dir: &str, ref_name: &str) -> Option<()> {
    let bytes = ref_name.as_bytes();
    let mut path = String::new();
    path.push_str(git_dir);
    let mut start = 0usize;
    while start < bytes.len() {
        let mut end = start;
        while end < bytes.len() && bytes[end] != b'/' {
            end += 1;
        }
        if end == bytes.len() {
            return Some(());
        }
        path.push('/');
        path.push_str(core::str::from_utf8(&bytes[start..end]).ok()?);
        let _ = mkdir(&path, 0o755);
        start = end + 1;
    }
    Some(())
}

fn is_direct_child(prefix: &str, path: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    starts_with(path, prefix) && path.as_bytes().get(prefix.len()) == Some(&b'/')
}

fn child_name<'a>(prefix: &str, path: &'a str) -> Option<&'a str> {
    if prefix.is_empty() {
        Some(path)
    } else if path.len() > prefix.len() + 1 {
        Some(&path[prefix.len() + 1..])
    } else {
        None
    }
}

fn child_has_slash(path: &str) -> bool {
    path.as_bytes().iter().any(|&b| b == b'/')
}

fn first_component(path: &str) -> Option<&str> {
    match path.as_bytes().iter().position(|&b| b == b'/') {
        Some(pos) => Some(&path[..pos]),
        None => Some(path),
    }
}

fn is_under_dir(path: &str, dir: &str) -> bool {
    path == dir
        || (path.len() > dir.len()
            && path.as_bytes().get(dir.len()) == Some(&b'/')
            && starts_with(path, dir))
}

fn index_mode_str(mode: u32) -> &'static str {
    match mode {
        0o100755 => "100755",
        _ => "100644",
    }
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
    if parent.len() + name.len() + 2 > MAX_PATH_LEN {
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
    if path.is_empty() || path.starts_with(b"/") || path.ends_with(b"/") {
        return false;
    }
    let mut start = 0usize;
    while start < path.len() {
        let mut end = start;
        while end < path.len() && path[end] != b'/' {
            end += 1;
        }
        let part = &path[start..end];
        if part.is_empty() || part == b"." || part == b".." || part.iter().any(|&b| b == 0) {
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

fn append_oid_hex(out: &mut Vec<u8>, oid: &[u8; 20]) {
    for &b in oid {
        push_hex_byte_vec(out, b);
    }
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

fn push_hex_byte_vec(out: &mut Vec<u8>, b: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(HEX[(b >> 4) as usize]);
    out.push(HEX[(b & 0x0f) as usize]);
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
    println!("usage: git commit [--repo DIR] -m MESSAGE [--date \"YYYY-MM-DD HH:MM:SS\"]");
}
