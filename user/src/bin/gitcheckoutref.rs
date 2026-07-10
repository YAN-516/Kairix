#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::{string::String, vec::Vec};
use user_lib::{AT_FDCWD, OpenFlags, close, linkat, mkdir, open, read, unlinkat, write};

const DEFAULT_REPO: &str = ".";
const MAX_ARG_LEN: usize = 512;
const MAX_PATH_LEN: usize = 512;
const MAX_INDEX_LEN: usize = 1024 * 1024;
const MAX_FILE_LEN: usize = 2 * 1024 * 1024;
const MAX_OBJECT_FILE_LEN: usize = 2 * 1024 * 1024;
const MAX_OBJECT_SIZE: usize = 2 * 1024 * 1024;
const GIT_INDEX_VERSION: u32 = 2;

#[derive(Clone)]
struct IndexEntry {
    path: String,
    oid: [u8; 20],
    mode: u32,
    size: usize,
}

struct Config {
    repo_dir: &'static str,
    new_branch: Option<&'static str>,
    target: Option<&'static str>,
}

struct Target {
    branch: String,
    oid: [u8; 20],
    create_branch: bool,
}

#[unsafe(no_mangle)]
pub fn main_with_args(argc: usize, argv: *const usize) -> i32 {
    let cfg = match parse_args(argc, argv) {
        Some(v) => v,
        None => return -1,
    };
    match run_checkout(&cfg) {
        Some(()) => 0,
        None => -1,
    }
}

fn parse_args(argc: usize, argv: *const usize) -> Option<Config> {
    let mut cfg = Config {
        repo_dir: DEFAULT_REPO,
        new_branch: None,
        target: None,
    };
    let mut repo_set = false;
    let mut i = 1usize;
    while i < argc {
        let arg = argv_str(argv, i)?;
        if arg == "-h" || arg == "--help" {
            print_usage();
            return None;
        } else if arg == "-b" {
            i += 1;
            if i >= argc {
                println!("missing branch name for -b");
                return None;
            }
            cfg.new_branch = argv_str(argv, i);
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
        } else if cfg.target.is_none() {
            cfg.target = Some(arg);
        } else if !repo_set && user_lib::git::discover_repository(arg).is_some() {
            cfg.repo_dir = arg;
            repo_set = true;
        } else {
            println!("too many arguments");
            return None;
        }
        i += 1;
    }
    if cfg.target.is_none() {
        print_usage();
        return None;
    }
    Some(cfg)
}

fn run_checkout(cfg: &Config) -> Option<()> {
    let repo_dir = match user_lib::git::discover_repository(cfg.repo_dir) {
        Some(v) => v,
        None => {
            println!("not a git repository: {}", cfg.repo_dir);
            return None;
        }
    };
    let git_dir = join_path(&repo_dir, ".git")?;
    let target = resolve_target(&git_dir, cfg)?;

    let index_path = join_path(&git_dir, "index")?;
    let old_index = read_small_file(&index_path, MAX_INDEX_LEN)?;
    let old_entries = parse_git_index(&old_index)?;
    if !working_tree_clean(&repo_dir, &old_entries) {
        println!("working tree has local changes; commit or clean before checkout");
        return None;
    }

    let commit = read_typed_object(&git_dir, &target.oid, "commit")?;
    print!("checkout commit: ");
    print_oid(&target.oid);
    println!("");
    let root_tree = commit_tree_oid(&commit)?;
    print!("root tree: ");
    print_oid(&root_tree);
    println!("");
    let mut new_entries = Vec::new();
    collect_tree_entries(&git_dir, &root_tree, "", &mut new_entries)?;
    new_entries.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));

    remove_old_tracked_files(&repo_dir, &old_entries, &new_entries);
    write_worktree(&repo_dir, &git_dir, &new_entries)?;
    write_git_index(&index_path, &mut new_entries)?;
    update_head_and_branch(&git_dir, &target)?;

    println!("Switched to branch '{}'", target.branch);
    Some(())
}

fn resolve_target(git_dir: &str, cfg: &Config) -> Option<Target> {
    let target = cfg.target?;
    if let Some(new_branch) = cfg.new_branch {
        if !is_safe_branch_name(new_branch) {
            println!("invalid branch name: {}", new_branch);
            return None;
        }
        let branch_ref = make_head_ref(new_branch)?;
        if read_ref_oid(git_dir, &branch_ref).is_some() {
            println!("branch already exists: {}", new_branch);
            return None;
        }
        let oid = resolve_start_oid(git_dir, target)?;
        return Some(Target {
            branch: String::from(new_branch),
            oid,
            create_branch: true,
        });
    }

    let branch_name = normalize_branch_arg(target);
    if !is_safe_branch_name(branch_name) {
        println!("invalid branch name: {}", target);
        return None;
    }

    let local_ref = make_head_ref(branch_name)?;
    if let Some(oid) = read_ref_oid(git_dir, &local_ref) {
        return Some(Target {
            branch: String::from(branch_name),
            oid,
            create_branch: false,
        });
    }

    let remote_ref = make_origin_ref(branch_name)?;
    if let Some(oid) = read_ref_oid(git_dir, &remote_ref) {
        return Some(Target {
            branch: String::from(branch_name),
            oid,
            create_branch: true,
        });
    }

    println!("branch not found: {}", target);
    None
}

fn resolve_start_oid(git_dir: &str, start: &str) -> Option<[u8; 20]> {
    let name = normalize_branch_arg(start);
    if !is_safe_branch_name(name) {
        println!("invalid start point: {}", start);
        return None;
    }
    let local_ref = make_head_ref(name)?;
    if let Some(oid) = read_ref_oid(git_dir, &local_ref) {
        return Some(oid);
    }
    let remote_ref = make_origin_ref(name)?;
    if let Some(oid) = read_ref_oid(git_dir, &remote_ref) {
        return Some(oid);
    }
    println!("start point not found: {}", start);
    None
}

fn normalize_branch_arg(input: &str) -> &str {
    if let Some(v) = strip_prefix(input, "origin/") {
        v
    } else if let Some(v) = strip_prefix(input, "remotes/origin/") {
        v
    } else {
        input
    }
}

fn make_head_ref(branch: &str) -> Option<String> {
    let mut out = String::from("refs/heads/");
    out.push_str(branch);
    Some(out)
}

fn make_origin_ref(branch: &str) -> Option<String> {
    let mut out = String::from("refs/remotes/origin/");
    out.push_str(branch);
    Some(out)
}

fn read_ref_oid(git_dir: &str, ref_name: &str) -> Option<[u8; 20]> {
    let path = join_path(git_dir, ref_name)?;
    let data = read_small_file(&path, 256)?;
    let text = trim_ascii_str(&data)?;
    parse_hex_oid(text.as_bytes())
}

fn working_tree_clean(repo_dir: &str, entries: &[IndexEntry]) -> bool {
    for entry in entries {
        let path = match join_path(repo_dir, &entry.path) {
            Some(v) => v,
            None => return false,
        };
        let Some(data) = read_small_file(&path, MAX_FILE_LEN) else {
            println!("deleted: {}", entry.path);
            return false;
        };
        if git_blob_oid(&data) != entry.oid {
            println!("modified: {}", entry.path);
            return false;
        }
    }
    true
}

fn remove_old_tracked_files(
    repo_dir: &str,
    old_entries: &[IndexEntry],
    new_entries: &[IndexEntry],
) {
    for entry in old_entries {
        if index_contains_path(new_entries, &entry.path) {
            continue;
        }
        if let Some(path) = join_path(repo_dir, &entry.path) {
            let _ = unlinkat(AT_FDCWD, &path, 0);
        }
    }
}

fn index_contains_path(entries: &[IndexEntry], path: &str) -> bool {
    for entry in entries {
        if entry.path == path {
            return true;
        }
    }
    false
}

fn write_worktree(repo_dir: &str, git_dir: &str, entries: &[IndexEntry]) -> Option<()> {
    for entry in entries {
        let path = join_path(repo_dir, &entry.path)?;
        ensure_parent_dirs(&path)?;
        let data = read_typed_object(git_dir, &entry.oid, "blob")?;
        if !write_checkout_file(&path, &data) {
            return None;
        }
        println!("wrote {}", path);
    }
    Some(())
}

fn ensure_parent_dirs(path: &str) -> Option<()> {
    let bytes = path.as_bytes();
    let mut current = String::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'/' {
            if !current.is_empty() {
                let _ = mkdir(&current, 0o755);
            }
        }
        current.push(bytes[i] as char);
        i += 1;
    }
    Some(())
}

fn write_checkout_file(path: &str, data: &[u8]) -> bool {
    let tmp_path = match checkout_tmp_path(path) {
        Some(v) => v,
        None => return false,
    };
    let _ = unlinkat(AT_FDCWD, &tmp_path, 0);
    if !write_file(&tmp_path, data) {
        let _ = unlinkat(AT_FDCWD, &tmp_path, 0);
        return false;
    }
    match read_small_file(&tmp_path, MAX_FILE_LEN) {
        Some(written) if written == data => {}
        _ => {
            println!("checkout temp verify failed: {}", tmp_path);
            let _ = unlinkat(AT_FDCWD, &tmp_path, 0);
            return false;
        }
    }

    let _ = unlinkat(AT_FDCWD, path, 0);
    if linkat(AT_FDCWD, &tmp_path, AT_FDCWD, path, 0) < 0 {
        println!("checkout link failed: {}", path);
        let _ = unlinkat(AT_FDCWD, &tmp_path, 0);
        return false;
    }
    let _ = unlinkat(AT_FDCWD, &tmp_path, 0);

    match read_small_file(path, MAX_FILE_LEN) {
        Some(written) if written == data => true,
        _ => {
            println!("checkout verify failed: {}", path);
            false
        }
    }
}

fn checkout_tmp_path(path: &str) -> Option<String> {
    const SUFFIX: &str = ".gitcheckout-tmp";
    if path.len() + SUFFIX.len() > MAX_PATH_LEN {
        println!("path too long");
        return None;
    }
    let mut out = String::from(path);
    out.push_str(SUFFIX);
    Some(out)
}

fn update_head_and_branch(git_dir: &str, target: &Target) -> Option<()> {
    let branch_ref = make_head_ref(&target.branch)?;
    if !mkdir_ref_parents(git_dir, &branch_ref) {
        return None;
    }
    let ref_path = join_path(git_dir, &branch_ref)?;
    let mut ref_data = Vec::new();
    append_oid_hex(&mut ref_data, &target.oid);
    ref_data.push(b'\n');
    if !write_file(&ref_path, &ref_data) {
        return None;
    }

    let head_path = join_path(git_dir, "HEAD")?;
    let mut head = Vec::new();
    head.extend_from_slice(b"ref: ");
    head.extend_from_slice(branch_ref.as_bytes());
    head.push(b'\n');
    if !write_file(&head_path, &head) {
        return None;
    }
    if target.create_branch {
        println!("created branch '{}'", target.branch);
    }
    Some(())
}

fn collect_tree_entries(
    git_dir: &str,
    tree_oid: &[u8; 20],
    prefix: &str,
    out: &mut Vec<IndexEntry>,
) -> Option<()> {
    let tree = read_typed_object(git_dir, tree_oid, "tree")?;
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
            let blob = read_typed_object(git_dir, &oid, "blob")?;
            print!("checkout blob: ");
            print_oid(&oid);
            println!(" {}", path);
            out.push(IndexEntry {
                path,
                oid,
                mode: git_index_mode(mode),
                size: blob.len(),
            });
        }
    }
    Some(())
}

fn read_typed_object(git_dir: &str, oid: &[u8; 20], typ: &str) -> Option<Vec<u8>> {
    let object = read_loose_object(git_dir, oid)?;
    Some(Vec::from(parse_loose_object(&object, typ)?))
}

fn read_loose_object(git_dir: &str, oid: &[u8; 20]) -> Option<Vec<u8>> {
    let oid_hex = oid_to_hex(oid);
    let object_dir = join_path(&join_path(git_dir, "objects")?, &oid_hex[..2])?;
    let object_path = join_path(&object_dir, &oid_hex[2..])?;
    let compressed = read_small_file(&object_path, MAX_OBJECT_FILE_LEN)?;
    let mut out = Vec::new();
    inflate_zlib_stored(&compressed, &mut out)?;
    let got = sha1(&out);
    if &got != oid {
        print!("loose object hash mismatch: want ");
        print_oid(oid);
        print!(" got ");
        print_oid(&got);
        println!("");
        return None;
    }
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
    if body.len() != size || body.len() > MAX_OBJECT_SIZE {
        return None;
    }
    Some(body)
}

fn commit_tree_oid(commit: &[u8]) -> Option<[u8; 20]> {
    let prefix = b"tree ";
    if commit.len() < prefix.len() + 40 || &commit[..prefix.len()] != prefix {
        return None;
    }
    parse_hex_oid(&commit[prefix.len()..prefix.len() + 40])
}

fn parse_git_index(data: &[u8]) -> Option<Vec<IndexEntry>> {
    if data.len() < 12 + 20 || &data[..4] != b"DIRC" {
        println!("invalid git index");
        return None;
    }
    let version = read_be_u32(data, 4)?;
    if version != GIT_INDEX_VERSION {
        println!("unsupported git index version: {}", version);
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
            return None;
        }
        let path = core::str::from_utf8(&data[pos..pos + name_len])
            .ok()
            .map(String::from)?;
        pos += name_len;
        while pos < data.len().saturating_sub(20) && data[pos] != 0 {
            pos += 1;
        }
        if pos >= data.len().saturating_sub(20) {
            return None;
        }
        pos += 1;
        while (pos - start) % 8 != 0 {
            if pos >= data.len().saturating_sub(20) {
                return None;
            }
            pos += 1;
        }
        entries.push(IndexEntry {
            path,
            oid,
            mode,
            size,
        });
    }
    Some(entries)
}

fn write_git_index(path: &str, entries: &mut Vec<IndexEntry>) -> Option<()> {
    entries.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    let mut data = Vec::new();
    data.extend_from_slice(b"DIRC");
    append_be_u32(&mut data, GIT_INDEX_VERSION);
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
    append_be_u16(out, entry.path.len().min(0x0fff) as u16);
    out.extend_from_slice(entry.path.as_bytes());
    out.push(0);
    while (out.len() - entry_start) % 8 != 0 {
        out.push(0);
    }
    Some(())
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
    if read_be_u32(input, pos)? != adler32(out) {
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

fn mkdir_ref_parents(git_dir: &str, ref_name: &str) -> bool {
    let bytes = ref_name.as_bytes();
    let mut path = String::from(git_dir);
    let mut start = 0usize;
    while start < bytes.len() {
        let mut end = start;
        while end < bytes.len() && bytes[end] != b'/' {
            end += 1;
        }
        if end == bytes.len() {
            return true;
        }
        path.push('/');
        match core::str::from_utf8(&bytes[start..end]) {
            Ok(seg) => path.push_str(seg),
            Err(_) => return false,
        }
        let _ = mkdir(&path, 0o755);
        start = end + 1;
    }
    true
}

fn is_safe_branch_name(input: &str) -> bool {
    !input.is_empty()
        && !input.starts_with('/')
        && !input.ends_with('/')
        && !input.contains("..")
        && !input.contains("//")
        && input
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'/')
}

fn git_index_mode(mode: &str) -> u32 {
    match mode {
        "100755" => 0o100755,
        "120000" => 0o120000,
        "160000" => 0o160000,
        _ => 0o100644,
    }
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

fn print_oid(oid: &[u8; 20]) {
    for &b in oid {
        print!("{:02x}", b);
    }
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
    append_oid_hex_string(&mut out, oid);
    out
}

fn append_oid_hex(out: &mut Vec<u8>, oid: &[u8; 20]) {
    for &b in oid {
        push_hex_byte_vec(out, b);
    }
}

fn append_oid_hex_string(out: &mut String, oid: &[u8; 20]) {
    for &b in oid {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
}

fn push_hex_byte_vec(out: &mut Vec<u8>, b: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(HEX[(b >> 4) as usize]);
    out.push(HEX[(b & 0x0f) as usize]);
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

fn append_be_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn append_be_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
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
    println!("usage: git checkout <branch>");
    println!("       git checkout -b <branch> <start-point>");
    println!("       git checkout --repo DIR <branch>");
}
