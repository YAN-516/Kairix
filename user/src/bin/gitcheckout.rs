#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::{string::String, vec::Vec};
use user_lib::{AT_FDCWD, OpenFlags, close, mkdir, open, pread64, read, write};

const DEFAULT_PACK: &str = "/musl/gitfetch.pack";
const DEFAULT_OUT_DIR: &str = "/musl/checkout";
const DEFAULT_META: &str = "/musl/gitclone.meta";
const PACK_TRAILER_LEN: usize = 20;
const PACK_STREAM_BUF_SIZE: usize = 4096;
const INFLATE_WINDOW_SIZE: usize = 32768;
const INFLATE_WRITE_BUF_SIZE: usize = 4096;
const MAX_TREE_DATA_SIZE: usize = 8192;
const MAX_OBJECT_SIZE: usize = 4 * 1024 * 1024;
const MAX_HUFFMAN_BITS: usize = 15;
const MAX_HUFFMAN_ENTRIES: usize = 320;
const MAX_CHECKOUT_PATH: usize = 512;
const MAX_META_LEN: usize = 64 * 1024;
const MAX_LOOSE_OBJECT_FILE_LEN: usize = MAX_OBJECT_SIZE + 4096;
const GIT_INDEX_VERSION: u32 = 2;

#[derive(Clone, Copy)]
struct PackObjectHeader {
    typ: u8,
    size: usize,
}

#[derive(Clone, Copy)]
struct InflateResult {
    consumed: usize,
}

#[derive(Clone, Copy)]
struct HuffEntry {
    symbol: u16,
    len: u8,
    code: u16,
}

#[derive(Clone, Copy)]
struct HuffTable {
    entries: [HuffEntry; MAX_HUFFMAN_ENTRIES],
    len: usize,
}

#[derive(Clone, Copy)]
enum DeltaBase {
    None,
    Offset(usize),
    Oid([u8; 20]),
}

struct RawObject {
    pack_offset: usize,
    typ: u8,
    base: DeltaBase,
    oid: [u8; 20],
    resolved: bool,
    size: usize,
    data: Vec<u8>,
    data_spilled: bool,
}

type PackedObject = RawObject;

struct ObjectDb<'a> {
    packed: &'a [PackedObject],
    git_dir: Option<&'a str>,
}

struct CheckoutConfig {
    pack_path: &'static str,
    out_dir: &'static str,
    write_git: bool,
    meta_path: Option<&'static str>,
    remote_name: &'static str,
    verbose: bool,
}

struct GitMeta {
    oid: Option<[u8; 20]>,
    ref_name: Option<String>,
    url: Option<String>,
    remote_refs: Vec<GitMetaRef>,
}

struct GitMetaRef {
    name: String,
    oid: [u8; 20],
}

struct IndexEntry {
    path: String,
    mode: u32,
    oid: [u8; 20],
    size: usize,
}

enum ArgResult {
    Ok,
    Help,
    Error,
}

struct PackStream {
    fd: usize,
    buf: [u8; PACK_STREAM_BUF_SIZE],
    pos: usize,
    len: usize,
    offset: usize,
}

struct FileInflateOutput {
    fd: usize,
    window: [u8; INFLATE_WINDOW_SIZE],
    window_pos: usize,
    written: usize,
    expected: usize,
    adler_a: u32,
    adler_b: u32,
    buf: [u8; INFLATE_WRITE_BUF_SIZE],
    buf_len: usize,
}

struct HashFileWriter {
    fd: usize,
    sha: Sha1State,
    written: usize,
}

impl PackStream {
    fn open(path: &str) -> Option<Self> {
        let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
        if fd < 0 {
            println!("open pack failed: {}", fd);
            return None;
        }
        Some(Self {
            fd: fd as usize,
            buf: [0u8; PACK_STREAM_BUF_SIZE],
            pos: 0,
            len: 0,
            offset: 0,
        })
    }

    fn offset(&self) -> usize {
        self.offset
    }

    fn read_byte(&mut self) -> Option<u8> {
        if self.pos == self.len {
            let n = read(self.fd, &mut self.buf);
            if n < 0 {
                println!("read pack failed: {}", n);
                return None;
            }
            if n == 0 {
                return None;
            }
            self.pos = 0;
            self.len = n as usize;
        }
        let b = self.buf[self.pos];
        self.pos += 1;
        self.offset += 1;
        Some(b)
    }

    fn read_exact(&mut self, out: &mut [u8]) -> Option<()> {
        for b in out {
            *b = self.read_byte()?;
        }
        Some(())
    }
}

impl Drop for PackStream {
    fn drop(&mut self) {
        let _ = close(self.fd);
    }
}

impl FileInflateOutput {
    fn open(path: &str, expected: usize) -> Option<Self> {
        let fd = open(
            AT_FDCWD,
            path,
            OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::WRONLY,
            0o644,
        );
        if fd < 0 {
            println!("open output failed: {}", fd);
            return None;
        }
        Some(Self {
            fd: fd as usize,
            window: [0u8; INFLATE_WINDOW_SIZE],
            window_pos: 0,
            written: 0,
            expected,
            adler_a: 1,
            adler_b: 0,
            buf: [0u8; INFLATE_WRITE_BUF_SIZE],
            buf_len: 0,
        })
    }

    fn push(&mut self, byte: u8) -> Option<()> {
        if self.written >= self.expected {
            println!("inflated object too large");
            return None;
        }
        self.window[self.window_pos] = byte;
        self.window_pos = (self.window_pos + 1) % INFLATE_WINDOW_SIZE;
        self.written += 1;
        update_adler32_byte(&mut self.adler_a, &mut self.adler_b, byte);

        self.buf[self.buf_len] = byte;
        self.buf_len += 1;
        if self.buf_len == self.buf.len() {
            self.flush()?;
        }
        Some(())
    }

    fn copy_from_distance(&mut self, distance: usize, len: usize) -> Option<()> {
        if distance == 0 || distance > self.written || distance > INFLATE_WINDOW_SIZE {
            println!("invalid deflate distance");
            return None;
        }
        for _ in 0..len {
            let pos = (self.window_pos + INFLATE_WINDOW_SIZE - distance) % INFLATE_WINDOW_SIZE;
            let byte = self.window[pos];
            self.push(byte)?;
        }
        Some(())
    }

    fn flush(&mut self) -> Option<()> {
        if self.buf_len == 0 {
            return Some(());
        }
        if !write_all_fd(self.fd, &self.buf[..self.buf_len]) {
            return None;
        }
        self.buf_len = 0;
        Some(())
    }

    fn adler32(&self) -> u32 {
        (self.adler_b << 16) | self.adler_a
    }

    fn finish(mut self) -> Option<()> {
        self.flush()?;
        if self.written != self.expected {
            println!(
                "object size mismatch: expected {} inflated {}",
                self.expected, self.written
            );
            return None;
        }
        Some(())
    }
}

impl Drop for FileInflateOutput {
    fn drop(&mut self) {
        let _ = close(self.fd);
    }
}

impl HashFileWriter {
    fn open(path: &str, typ: u8, size: usize) -> Option<Self> {
        let fd = open(
            AT_FDCWD,
            path,
            OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::WRONLY,
            0o644,
        );
        if fd < 0 {
            println!("open output failed: {}", fd);
            return None;
        }
        let mut sha = Sha1State::new();
        let mut header = Vec::new();
        header.extend_from_slice(object_type_name(typ).as_bytes());
        header.push(b' ');
        append_usize(&mut header, size);
        header.push(0);
        sha.update(&header);
        Some(Self {
            fd: fd as usize,
            sha,
            written: 0,
        })
    }

    fn write_bytes(&mut self, data: &[u8]) -> Option<()> {
        if !write_all_fd(self.fd, data) {
            return None;
        }
        self.sha.update(data);
        self.written += data.len();
        Some(())
    }

    fn finish(mut self) -> ([u8; 20], usize) {
        let written = self.written;
        let sha = core::mem::replace(&mut self.sha, Sha1State::new());
        let oid = sha.finish();
        let _ = close(self.fd);
        self.fd = usize::MAX;
        (oid, written)
    }
}

impl Drop for HashFileWriter {
    fn drop(&mut self) {
        if self.fd != usize::MAX {
            let _ = close(self.fd);
        }
    }
}

struct StreamBitReader<'a> {
    stream: &'a mut PackStream,
    current: u8,
    bit_pos: u8,
}

impl<'a> StreamBitReader<'a> {
    fn new(stream: &'a mut PackStream) -> Self {
        Self {
            stream,
            current: 0,
            bit_pos: 8,
        }
    }

    fn stream_offset(&self) -> usize {
        self.stream.offset()
    }

    fn read_bits(&mut self, n: usize) -> Option<u32> {
        let mut out = 0u32;
        for i in 0..n {
            if self.bit_pos == 8 {
                self.current = self.stream.read_byte()?;
                self.bit_pos = 0;
            }
            let bit = (self.current >> self.bit_pos) & 1;
            out |= (bit as u32) << i;
            self.bit_pos += 1;
        }
        Some(out)
    }

    fn align_byte(&mut self) {
        self.bit_pos = 8;
    }

    fn read_byte(&mut self) -> Option<u8> {
        self.align_byte();
        self.stream.read_byte()
    }

    fn read_u16_le(&mut self) -> Option<u16> {
        let lo = self.read_byte()? as u16;
        let hi = self.read_byte()? as u16;
        Some(lo | (hi << 8))
    }
}

#[unsafe(no_mangle)]
pub fn main_with_args(argc: usize, argv: *const usize) -> i32 {
    let mut cfg = CheckoutConfig {
        pack_path: DEFAULT_PACK,
        out_dir: DEFAULT_OUT_DIR,
        write_git: false,
        meta_path: None,
        remote_name: "origin",
        verbose: true,
    };
    match parse_args(argc, argv, &mut cfg) {
        ArgResult::Help => {
            print_usage();
            0
        }
        ArgResult::Error => -1,
        ArgResult::Ok => checkout_pack(&cfg),
    }
}

fn parse_args(argc: usize, argv: *const usize, cfg: &mut CheckoutConfig) -> ArgResult {
    let mut positional = 0usize;
    let mut i = 1usize;
    while i < argc {
        let arg = match argv_str(argv, i) {
            Some(v) => v,
            None => {
                println!("invalid argument");
                return ArgResult::Error;
            }
        };
        if arg == "-h" || arg == "--help" {
            return ArgResult::Help;
        } else if arg == "--git" {
            cfg.write_git = true;
            if cfg.meta_path.is_none() {
                cfg.meta_path = Some(DEFAULT_META);
            }
        } else if arg == "-q" || arg == "--quiet" {
            cfg.verbose = false;
        } else if arg == "--remote" {
            i += 1;
            if i >= argc {
                println!("missing value for --remote");
                return ArgResult::Error;
            }
            cfg.remote_name = match argv_str(argv, i) {
                Some(v) if is_safe_remote_name(v) => v,
                _ => {
                    println!("invalid remote name");
                    return ArgResult::Error;
                }
            };
        } else if let Some(v) = strip_prefix(arg, "--remote=") {
            if !is_safe_remote_name(v) {
                println!("invalid remote name");
                return ArgResult::Error;
            }
            cfg.remote_name = v;
        } else if arg == "--meta" {
            i += 1;
            if i >= argc {
                println!("missing value for --meta");
                return ArgResult::Error;
            }
            cfg.meta_path = match argv_str(argv, i) {
                Some(v) if !v.is_empty() => Some(v),
                _ => {
                    println!("invalid meta path");
                    return ArgResult::Error;
                }
            };
        } else if let Some(v) = strip_prefix(arg, "--meta=") {
            if v.is_empty() {
                println!("invalid meta path");
                return ArgResult::Error;
            }
            cfg.meta_path = Some(v);
        } else if starts_with(arg, "-") {
            println!("unknown option: {}", arg);
            return ArgResult::Error;
        } else if positional == 0 {
            cfg.pack_path = arg;
            positional += 1;
        } else if positional == 1 {
            cfg.out_dir = arg;
            positional += 1;
        } else {
            println!("too many arguments");
            return ArgResult::Error;
        }
        i += 1;
    }
    ArgResult::Ok
}

fn checkout_pack(cfg: &CheckoutConfig) -> i32 {
    let path = cfg.pack_path;
    let out_dir = cfg.out_dir;
    println!("pack: {}", path);
    println!("checkout: {}", out_dir);

    let mut stream = match PackStream::open(path) {
        Some(v) => v,
        None => return -1,
    };

    let mut header = [0u8; 12];
    if stream.read_exact(&mut header).is_none() {
        println!("invalid pack: too small");
        return -1;
    }
    if &header[..4] != b"PACK" {
        println!("invalid pack: missing PACK magic");
        return -1;
    }

    let version = read_be_u32(&header, 4);
    let objects = read_be_u32(&header, 8);
    println!("magic: PACK");
    println!("version: {}", version);
    println!("objects: {}", objects);

    if version != 2 && version != 3 {
        println!("unsupported pack version");
        return -1;
    }
    if objects == 0 {
        println!("empty pack");
        let mut trailer = [0u8; PACK_TRAILER_LEN];
        if stream.read_exact(&mut trailer).is_none() {
            println!("invalid pack: missing checksum");
            return -1;
        }
        print_trailer(&trailer);
        return 0;
    }

    let mut raw_objects = Vec::new();
    let mut index = 0u32;
    while index < objects {
        let offset = stream.offset();
        let header = match parse_object_header_stream(&mut stream) {
            Some(v) => v,
            None => {
                println!("invalid object header at {}", offset);
                return -1;
            }
        };
        if cfg.verbose {
            println!("object[{}].offset: {}", index, offset);
            println!("object[{}].type: {}", index, object_type_name(header.typ));
            println!("object[{}].size: {}", index, header.size);
        }

        let mut base = DeltaBase::None;
        if header.typ == 6 {
            let base_start = stream.offset();
            let base_offset = match parse_ofs_delta_base_stream(&mut stream, offset) {
                Some(v) => v,
                None => {
                    println!("invalid ofs-delta base");
                    return -1;
                }
            };
            let used = stream.offset() - base_start;
            if cfg.verbose {
                println!("object[{}].delta base bytes: {}", index, used);
                println!("object[{}].delta base offset: {}", index, base_offset);
            }
            base = DeltaBase::Offset(base_offset);
        } else if header.typ == 7 {
            let mut base_oid = [0u8; 20];
            if stream.read_exact(&mut base_oid).is_none() {
                println!("invalid ref-delta base");
                return -1;
            }
            if cfg.verbose {
                print!("object[{}].delta base oid:", index);
                print_oid(&base_oid);
                println!("");
            }
            base = DeltaBase::Oid(base_oid);
        }

        let mut oid = [0u8; 20];
        let mut resolved = false;
        let mut data_spilled = false;
        let mut object = Vec::new();
        let zlib_offset = stream.offset();
        if header.size > MAX_OBJECT_SIZE {
            println!("object too large: {}", header.size);
            return -1;
        }
        let inflated = if header.typ == 2 || header.typ == 3 {
            let mut path_buf = [0u8; 64];
            let path = match spill_object_path(offset, &mut path_buf) {
                Some(v) => v,
                None => return -1,
            };
            let inflated =
                match inflate_zlib_stream_to_file(&mut stream, zlib_offset, header.size, path) {
                    Some(v) => v,
                    None => return -1,
                };
            let object_oid = match git_object_oid_from_file(header.typ, header.size, path) {
                Some(v) => v,
                None => return -1,
            };
            oid = object_oid;
            resolved = true;
            data_spilled = true;
            inflated
        } else {
            let inflated =
                match inflate_zlib_stream(&mut stream, zlib_offset, header.size, &mut object) {
                    Some(v) => v,
                    None => return -1,
                };
            if object.len() != header.size {
                println!(
                    "object[{}].size mismatch: header {} inflated {}",
                    index,
                    header.size,
                    object.len()
                );
                return -1;
            }
            if header.typ != 6 && header.typ != 7 {
                oid = git_object_oid(header.typ, &object);
                resolved = true;
            }
            inflated
        };
        if cfg.verbose {
            println!("object[{}].zlib offset: {}", index, zlib_offset);
            println!("object[{}].zlib bytes: {}", index, inflated.consumed);
        }
        if cfg.verbose && resolved {
            print!("object[{}].oid: ", index);
            print_oid(&oid);
            println!("");
        }
        raw_objects.push(RawObject {
            pack_offset: offset,
            typ: header.typ,
            base,
            oid,
            resolved,
            size: header.size,
            data: object,
            data_spilled,
        });

        index += 1;
    }

    let mut trailer = [0u8; PACK_TRAILER_LEN];
    if stream.read_exact(&mut trailer).is_none() {
        println!("invalid pack: missing checksum");
        return -1;
    }

    print_trailer(&trailer);
    let existing_git_dir = join_path(out_dir, ".git");
    let parsed_objects =
        match resolve_objects(raw_objects, cfg.verbose, existing_git_dir.as_deref()) {
            Some(v) => v,
            None => return -1,
        };
    let object_db = ObjectDb {
        packed: &parsed_objects,
        git_dir: existing_git_dir.as_deref(),
    };
    let commit = match select_checkout_commit(&object_db, cfg.meta_path) {
        Some(v) => v,
        None => {
            println!("no commit object found");
            return -1;
        }
    };
    let root_tree = match commit_tree_oid(&commit.data) {
        Some(v) => v,
        None => {
            println!("commit has no tree");
            return -1;
        }
    };
    print!("root tree: ");
    print_oid(&root_tree);
    println!("");

    let _ = mkdir(out_dir, 0o755);
    if cfg.write_git
        && !write_git_repository(
            out_dir,
            &parsed_objects,
            &commit.oid,
            &root_tree,
            cfg.meta_path,
            cfg.remote_name,
        )
    {
        return -1;
    }
    match checkout_tree(&object_db, &root_tree, out_dir) {
        Some(()) => {
            println!("checkout complete: {}", out_dir);
            0
        }
        None => -1,
    }
}

fn parse_object_header_stream(stream: &mut PackStream) -> Option<PackObjectHeader> {
    let first = stream.read_byte()?;
    let typ = (first >> 4) & 0x07;
    let mut size = (first & 0x0f) as usize;
    let mut shift = 4usize;
    let mut b = first;

    while b & 0x80 != 0 {
        if shift >= usize::BITS as usize {
            return None;
        }
        b = stream.read_byte()?;
        size |= ((b & 0x7f) as usize) << shift;
        shift += 7;
    }

    if typ == 0 || typ == 5 || typ > 7 {
        return None;
    }
    Some(PackObjectHeader { typ, size })
}

fn inflate_zlib_stream(
    stream: &mut PackStream,
    zlib_offset: usize,
    expected_size: usize,
    out: &mut Vec<u8>,
) -> Option<InflateResult> {
    let mut reader = StreamBitReader::new(stream);
    let cmf = reader.read_byte()?;
    let flg = reader.read_byte()?;
    let compression_method = cmf & 0x0f;
    let header_value = ((cmf as u16) << 8) | flg as u16;
    if compression_method != 8 || header_value % 31 != 0 || flg & 0x20 != 0 {
        println!("invalid or unsupported zlib header");
        return None;
    }

    loop {
        let final_block = reader.read_bits(1)? != 0;
        let block_type = reader.read_bits(2)? as u8;
        match block_type {
            0 => inflate_stored_block(&mut reader, out)?,
            1 => {
                let (litlen, dist) = fixed_huffman_tables()?;
                inflate_huffman_block(&mut reader, &litlen, &dist, out)?;
            }
            2 => {
                let (litlen, dist) = dynamic_huffman_tables(&mut reader)?;
                inflate_huffman_block(&mut reader, &litlen, &dist, out)?;
            }
            _ => {
                println!("unsupported deflate block type");
                return None;
            }
        }
        if out.len() > MAX_OBJECT_SIZE || out.len() > expected_size {
            println!("inflated object too large");
            return None;
        }
        if final_block {
            break;
        }
    }

    reader.align_byte();
    let mut checksum = [0u8; 4];
    for item in &mut checksum {
        *item = reader.read_byte()?;
    }
    let got = read_be_u32(&checksum, 0);
    let want = adler32(out);
    if got != want {
        println!("zlib adler32 mismatch");
        return None;
    }
    Some(InflateResult {
        consumed: reader.stream_offset() - zlib_offset,
    })
}

fn inflate_zlib_stream_to_file(
    stream: &mut PackStream,
    zlib_offset: usize,
    expected_size: usize,
    path: &str,
) -> Option<InflateResult> {
    let mut reader = StreamBitReader::new(stream);
    let cmf = reader.read_byte()?;
    let flg = reader.read_byte()?;
    let compression_method = cmf & 0x0f;
    let header_value = ((cmf as u16) << 8) | flg as u16;
    if compression_method != 8 || header_value % 31 != 0 || flg & 0x20 != 0 {
        println!("invalid or unsupported zlib header");
        return None;
    }

    let mut out = FileInflateOutput::open(path, expected_size)?;
    loop {
        let final_block = reader.read_bits(1)? != 0;
        let block_type = reader.read_bits(2)? as u8;
        match block_type {
            0 => inflate_stored_block_file(&mut reader, &mut out)?,
            1 => {
                let (litlen, dist) = fixed_huffman_tables()?;
                inflate_huffman_block_file(&mut reader, &litlen, &dist, &mut out)?;
            }
            2 => {
                let (litlen, dist) = dynamic_huffman_tables(&mut reader)?;
                inflate_huffman_block_file(&mut reader, &litlen, &dist, &mut out)?;
            }
            _ => {
                println!("unsupported deflate block type");
                return None;
            }
        }
        if final_block {
            break;
        }
    }

    reader.align_byte();
    let mut checksum = [0u8; 4];
    for item in &mut checksum {
        *item = reader.read_byte()?;
    }
    let got = read_be_u32(&checksum, 0);
    let want = out.adler32();
    if got != want {
        println!("zlib adler32 mismatch");
        return None;
    }
    out.finish()?;
    Some(InflateResult {
        consumed: reader.stream_offset() - zlib_offset,
    })
}

fn inflate_stored_block(reader: &mut StreamBitReader<'_>, out: &mut Vec<u8>) -> Option<()> {
    reader.align_byte();
    let len = reader.read_u16_le()? as usize;
    let nlen = reader.read_u16_le()?;
    if nlen != !(len as u16) {
        println!("invalid stored deflate block length");
        return None;
    }
    for _ in 0..len {
        out.push(reader.read_byte()?);
    }
    Some(())
}

fn inflate_stored_block_file(
    reader: &mut StreamBitReader<'_>,
    out: &mut FileInflateOutput,
) -> Option<()> {
    reader.align_byte();
    let len = reader.read_u16_le()? as usize;
    let nlen = reader.read_u16_le()?;
    if nlen != !(len as u16) {
        println!("invalid stored deflate block length");
        return None;
    }
    for _ in 0..len {
        out.push(reader.read_byte()?)?;
    }
    Some(())
}

fn inflate_huffman_block(
    reader: &mut StreamBitReader<'_>,
    litlen: &HuffTable,
    dist: &HuffTable,
    out: &mut Vec<u8>,
) -> Option<()> {
    loop {
        let sym = decode_symbol(reader, litlen)?;
        match sym {
            0..=255 => out.push(sym as u8),
            256 => return Some(()),
            257..=285 => {
                let idx = sym - 257;
                let mut len = LENGTH_BASE[idx] as usize;
                let extra = LENGTH_EXTRA[idx] as usize;
                if extra > 0 {
                    len += reader.read_bits(extra)? as usize;
                }

                let dist_sym = decode_symbol(reader, dist)?;
                if dist_sym >= DIST_BASE.len() {
                    println!("invalid distance symbol");
                    return None;
                }
                let mut distance = DIST_BASE[dist_sym] as usize;
                let dist_extra = DIST_EXTRA[dist_sym] as usize;
                if dist_extra > 0 {
                    distance += reader.read_bits(dist_extra)? as usize;
                }
                if distance == 0 || distance > out.len() {
                    println!("invalid deflate distance");
                    return None;
                }
                for _ in 0..len {
                    let b = out[out.len() - distance];
                    out.push(b);
                    if out.len() > MAX_OBJECT_SIZE {
                        println!("inflated object too large");
                        return None;
                    }
                }
            }
            _ => {
                println!("invalid literal/length symbol");
                return None;
            }
        }
    }
}

fn inflate_huffman_block_file(
    reader: &mut StreamBitReader<'_>,
    litlen: &HuffTable,
    dist: &HuffTable,
    out: &mut FileInflateOutput,
) -> Option<()> {
    loop {
        let sym = decode_symbol(reader, litlen)?;
        match sym {
            0..=255 => out.push(sym as u8)?,
            256 => return Some(()),
            257..=285 => {
                let idx = sym - 257;
                let mut len = LENGTH_BASE[idx] as usize;
                let extra = LENGTH_EXTRA[idx] as usize;
                if extra > 0 {
                    len += reader.read_bits(extra)? as usize;
                }

                let dist_sym = decode_symbol(reader, dist)?;
                if dist_sym >= DIST_BASE.len() {
                    println!("invalid distance symbol");
                    return None;
                }
                let mut distance = DIST_BASE[dist_sym] as usize;
                let dist_extra = DIST_EXTRA[dist_sym] as usize;
                if dist_extra > 0 {
                    distance += reader.read_bits(dist_extra)? as usize;
                }
                out.copy_from_distance(distance, len)?;
            }
            _ => {
                println!("invalid literal/length symbol");
                return None;
            }
        }
    }
}

fn fixed_huffman_tables() -> Option<(HuffTable, HuffTable)> {
    let mut lit_lengths = [0u8; 288];
    for item in lit_lengths.iter_mut().take(144) {
        *item = 8;
    }
    for item in lit_lengths.iter_mut().take(256).skip(144) {
        *item = 9;
    }
    for item in lit_lengths.iter_mut().take(280).skip(256) {
        *item = 7;
    }
    for item in lit_lengths.iter_mut().skip(280) {
        *item = 8;
    }
    let dist_lengths = [5u8; 32];
    Some((build_huffman(&lit_lengths)?, build_huffman(&dist_lengths)?))
}

fn dynamic_huffman_tables(reader: &mut StreamBitReader<'_>) -> Option<(HuffTable, HuffTable)> {
    let hlit = reader.read_bits(5)? as usize + 257;
    let hdist = reader.read_bits(5)? as usize + 1;
    let hclen = reader.read_bits(4)? as usize + 4;
    if hlit > 286 || hdist > 32 {
        println!("invalid dynamic huffman counts");
        return None;
    }

    let mut code_lengths = [0u8; 19];
    for &symbol in CODE_LENGTH_ORDER.iter().take(hclen) {
        code_lengths[symbol] = reader.read_bits(3)? as u8;
    }
    let code_table = build_huffman(&code_lengths)?;

    let total = hlit + hdist;
    let mut lengths = [0u8; MAX_HUFFMAN_ENTRIES];
    let mut lengths_len = 0usize;
    while lengths_len < total {
        let sym = decode_symbol(reader, &code_table)?;
        match sym {
            0..=15 => {
                lengths[lengths_len] = sym as u8;
                lengths_len += 1;
            }
            16 => {
                let repeat = reader.read_bits(2)? as usize + 3;
                if lengths_len == 0 {
                    return None;
                }
                let prev = lengths[lengths_len - 1];
                for _ in 0..repeat {
                    if lengths_len >= total {
                        println!("too many dynamic huffman lengths");
                        return None;
                    }
                    lengths[lengths_len] = prev;
                    lengths_len += 1;
                }
            }
            17 => {
                let repeat = reader.read_bits(3)? as usize + 3;
                for _ in 0..repeat {
                    if lengths_len >= total {
                        println!("too many dynamic huffman lengths");
                        return None;
                    }
                    lengths[lengths_len] = 0;
                    lengths_len += 1;
                }
            }
            18 => {
                let repeat = reader.read_bits(7)? as usize + 11;
                for _ in 0..repeat {
                    if lengths_len >= total {
                        println!("too many dynamic huffman lengths");
                        return None;
                    }
                    lengths[lengths_len] = 0;
                    lengths_len += 1;
                }
            }
            _ => return None,
        }
    }

    let litlen = build_huffman(&lengths[..hlit])?;
    let dist = build_huffman(&lengths[hlit..])?;
    Some((litlen, dist))
}

fn build_huffman(lengths: &[u8]) -> Option<HuffTable> {
    let mut counts = [0u16; MAX_HUFFMAN_BITS + 1];
    for &len in lengths {
        if len as usize > MAX_HUFFMAN_BITS {
            return None;
        }
        if len != 0 {
            counts[len as usize] += 1;
        }
    }

    let mut next_code = [0u16; MAX_HUFFMAN_BITS + 1];
    let mut code = 0u16;
    for bits in 1..=MAX_HUFFMAN_BITS {
        code = (code + counts[bits - 1]) << 1;
        next_code[bits] = code;
    }

    let empty = HuffEntry {
        symbol: 0,
        len: 0,
        code: 0,
    };
    let mut out = HuffTable {
        entries: [empty; MAX_HUFFMAN_ENTRIES],
        len: 0,
    };
    for (symbol, &len) in lengths.iter().enumerate() {
        if len == 0 {
            continue;
        }
        if out.len >= out.entries.len() || symbol > u16::MAX as usize {
            return None;
        }
        let code = next_code[len as usize];
        next_code[len as usize] += 1;
        out.entries[out.len] = HuffEntry {
            symbol: symbol as u16,
            len,
            code: reverse_bits(code, len),
        };
        out.len += 1;
    }
    Some(out)
}

fn decode_symbol(reader: &mut StreamBitReader<'_>, table: &HuffTable) -> Option<usize> {
    let mut code = 0u16;
    for len in 1..=MAX_HUFFMAN_BITS {
        code |= (reader.read_bits(1)? as u16) << (len - 1);
        for entry in table.entries.iter().take(table.len) {
            if entry.len as usize == len && entry.code == code {
                return Some(entry.symbol as usize);
            }
        }
    }
    println!("invalid huffman code");
    None
}

fn reverse_bits(mut code: u16, len: u8) -> u16 {
    let mut out = 0u16;
    for _ in 0..len {
        out = (out << 1) | (code & 1);
        code >>= 1;
    }
    out
}

const CODE_LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];

const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];

const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];

const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

fn adler32(input: &[u8]) -> u32 {
    adler32_parts(&[input])
}

fn adler32_parts(parts: &[&[u8]]) -> u32 {
    const MOD: u32 = 65521;
    let mut a = 1u32;
    let mut b = 0u32;
    for part in parts {
        for &byte in *part {
            a = (a + byte as u32) % MOD;
            b = (b + a) % MOD;
        }
    }
    (b << 16) | a
}

fn parse_ofs_delta_base_stream(stream: &mut PackStream, object_offset: usize) -> Option<usize> {
    let mut bytes = 0usize;
    let mut value = 0usize;
    loop {
        let b = stream.read_byte()?;
        bytes += 1;
        value = if bytes == 1 {
            (b & 0x7f) as usize
        } else {
            ((value + 1) << 7) | (b & 0x7f) as usize
        };
        if b & 0x80 == 0 {
            if value > object_offset {
                return None;
            }
            return Some(object_offset - value);
        }
        if bytes > 10 {
            return None;
        }
    }
}

fn resolve_objects(
    mut raw: Vec<RawObject>,
    verbose: bool,
    git_dir: Option<&str>,
) -> Option<Vec<PackedObject>> {
    let mut remaining = raw.iter().filter(|obj| !obj.resolved).count();
    while remaining > 0 {
        let before = remaining;
        for idx in 0..raw.len() {
            if raw[idx].resolved {
                continue;
            }
            let mut loose_base = None;
            let base_idx = match raw[idx].base {
                DeltaBase::Offset(offset) => raw
                    .iter()
                    .position(|v| v.resolved && v.pack_offset == offset),
                DeltaBase::Oid(oid) => {
                    let found = raw.iter().position(|v| v.resolved && v.oid == oid);
                    if found.is_none() {
                        if let Some(dir) = git_dir {
                            loose_base = read_loose_object(dir, &oid);
                        }
                    }
                    found
                }
                DeltaBase::None => None,
            };
            let base_typ = match (base_idx, loose_base.as_ref()) {
                (Some(base_idx), _) => raw[base_idx].typ,
                (None, Some(base)) => base.typ,
                (None, None) => continue,
            };
            if base_typ == 2 || base_typ == 3 {
                let mut path_buf = [0u8; 64];
                let path = spill_object_path(raw[idx].pack_offset, &mut path_buf)?;
                let (oid, resolved_size) = {
                    let pack_base;
                    let base = if let Some(base_idx) = base_idx {
                        &raw[base_idx]
                    } else {
                        pack_base = loose_base.as_ref()?;
                        pack_base
                    };
                    let delta = &raw[idx].data;
                    apply_delta_to_file(base, delta, base_typ, path)?
                };
                if verbose {
                    print!("resolved delta at {} -> ", raw[idx].pack_offset);
                    print_oid(&oid);
                    println!("");
                }
                raw[idx].typ = base_typ;
                raw[idx].oid = oid;
                raw[idx].size = resolved_size;
                raw[idx].data = Vec::new();
                raw[idx].data_spilled = true;
                raw[idx].resolved = true;
            } else {
                let data = {
                    let pack_base;
                    let base = if let Some(base_idx) = base_idx {
                        &raw[base_idx]
                    } else {
                        pack_base = loose_base.as_ref()?;
                        pack_base
                    };
                    let delta = &raw[idx].data;
                    apply_delta_from_object(base, delta)?
                };
                let resolved_size = data.len();
                let oid = git_object_oid(base_typ, &data);
                if verbose {
                    print!("resolved delta at {} -> ", raw[idx].pack_offset);
                    print_oid(&oid);
                    println!("");
                }
                raw[idx].typ = base_typ;
                raw[idx].oid = oid;
                raw[idx].size = resolved_size;
                raw[idx].data = data;
                raw[idx].data_spilled = false;
                raw[idx].resolved = true;
            }
            remaining -= 1;
        }
        if remaining == before {
            println!("unresolved delta objects: {}", remaining);
            return None;
        }
    }

    Some(raw)
}

fn read_loose_object(git_dir: &str, oid: &[u8; 20]) -> Option<PackedObject> {
    let oid_hex = oid_to_hex(oid);
    let object_dir = join_path(&join_path(git_dir, "objects")?, &oid_hex[..2])?;
    let object_path = join_path(&object_dir, &oid_hex[2..])?;
    let compressed = read_file_limited(&object_path, MAX_LOOSE_OBJECT_FILE_LEN)?;
    let inflated = inflate_zlib_stored_bytes(&compressed, MAX_OBJECT_SIZE + 64)?;
    let (typ, size, data_start) = parse_loose_object_header(&inflated)?;
    if inflated.len() - data_start != size {
        println!("loose object size mismatch");
        return None;
    }
    Some(PackedObject {
        pack_offset: usize::MAX,
        typ,
        base: DeltaBase::None,
        oid: *oid,
        resolved: true,
        size,
        data: inflated[data_start..].to_vec(),
        data_spilled: false,
    })
}

fn parse_loose_object_header(input: &[u8]) -> Option<(u8, usize, usize)> {
    let space = find_byte(input, b' ')?;
    let nul = find_byte(&input[space + 1..], 0)? + space + 1;
    let typ = core::str::from_utf8(&input[..space]).ok()?;
    let size = parse_decimal_usize(&input[space + 1..nul])?;
    let typ = match typ {
        "commit" => 1,
        "tree" => 2,
        "blob" => 3,
        _ => return None,
    };
    Some((typ, size, nul + 1))
}

fn inflate_zlib_stored_bytes(input: &[u8], max_output: usize) -> Option<Vec<u8>> {
    if input.len() < 6 {
        return None;
    }
    let cmf = input[0];
    let flg = input[1];
    let compression_method = cmf & 0x0f;
    let header_value = ((cmf as u16) << 8) | flg as u16;
    if compression_method != 8 || header_value % 31 != 0 || flg & 0x20 != 0 {
        println!("invalid or unsupported loose zlib header");
        return None;
    }

    let mut pos = 2usize;
    let mut out = Vec::new();
    loop {
        if pos + 5 > input.len() {
            return None;
        }
        let final_block = input[pos] & 1 != 0;
        if input[pos] & 0x06 != 0 {
            println!("unsupported loose deflate block type");
            return None;
        }
        pos += 1;
        let len = u16::from_le_bytes([input[pos], input[pos + 1]]) as usize;
        let nlen = u16::from_le_bytes([input[pos + 2], input[pos + 3]]);
        pos += 4;
        if nlen != !(len as u16) || pos + len > input.len() {
            println!("invalid loose stored deflate block");
            return None;
        }
        if out.len() + len > max_output {
            println!("loose object too large");
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
    let got = read_be_u32(input, pos);
    let want = adler32(&out);
    if got != want {
        println!("loose zlib adler32 mismatch");
        return None;
    }
    Some(out)
}

fn parse_decimal_usize(input: &[u8]) -> Option<usize> {
    if input.is_empty() {
        return None;
    }
    let mut value = 0usize;
    for &b in input {
        if !b.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add((b - b'0') as usize)?;
    }
    Some(value)
}

fn apply_delta_from_object(base: &PackedObject, delta: &[u8]) -> Option<Vec<u8>> {
    if base.data_spilled {
        let mut path_buf = [0u8; 64];
        let path = spill_object_path(base.pack_offset, &mut path_buf)?;
        let data = read_file_limited(path, base.size)?;
        apply_delta(&data, delta)
    } else {
        apply_delta(&base.data, delta)
    }
}

fn apply_delta_to_file(
    base: &PackedObject,
    delta: &[u8],
    typ: u8,
    path: &str,
) -> Option<([u8; 20], usize)> {
    let mut pos = 0usize;
    let source_size = read_delta_size(delta, &mut pos)?;
    let target_size = read_delta_size(delta, &mut pos)?;
    if source_size != base.size {
        println!(
            "delta source size mismatch: base {} delta {}",
            base.size, source_size
        );
        return None;
    }

    let mut base_fd = None;
    if base.data_spilled {
        let mut path_buf = [0u8; 64];
        let base_path = spill_object_path(base.pack_offset, &mut path_buf)?;
        let fd = open(AT_FDCWD, base_path, OpenFlags::RDONLY, 0);
        if fd < 0 {
            println!("open delta base failed: {}", fd);
            return None;
        }
        base_fd = Some(fd as usize);
    }

    let mut out = HashFileWriter::open(path, typ, target_size)?;
    while pos < delta.len() {
        let opcode = delta[pos];
        pos += 1;
        if opcode & 0x80 != 0 {
            let mut copy_offset = 0usize;
            let mut copy_size = 0usize;
            if opcode & 0x01 != 0 {
                copy_offset |= read_delta_byte(delta, &mut pos)? as usize;
            }
            if opcode & 0x02 != 0 {
                copy_offset |= (read_delta_byte(delta, &mut pos)? as usize) << 8;
            }
            if opcode & 0x04 != 0 {
                copy_offset |= (read_delta_byte(delta, &mut pos)? as usize) << 16;
            }
            if opcode & 0x08 != 0 {
                copy_offset |= (read_delta_byte(delta, &mut pos)? as usize) << 24;
            }
            if opcode & 0x10 != 0 {
                copy_size |= read_delta_byte(delta, &mut pos)? as usize;
            }
            if opcode & 0x20 != 0 {
                copy_size |= (read_delta_byte(delta, &mut pos)? as usize) << 8;
            }
            if opcode & 0x40 != 0 {
                copy_size |= (read_delta_byte(delta, &mut pos)? as usize) << 16;
            }
            if copy_size == 0 {
                copy_size = 0x10000;
            }
            if copy_offset
                .checked_add(copy_size)
                .map_or(true, |end| end > base.size)
            {
                println!("delta copy out of range");
                return None;
            }
            if let Some(fd) = base_fd {
                copy_base_range_from_fd(fd, copy_offset, copy_size, &mut out)?;
            } else {
                out.write_bytes(&base.data[copy_offset..copy_offset + copy_size])?;
            }
        } else if opcode != 0 {
            let insert_len = opcode as usize;
            if pos + insert_len > delta.len() {
                println!("delta insert out of range");
                return None;
            }
            out.write_bytes(&delta[pos..pos + insert_len])?;
            pos += insert_len;
        } else {
            println!("invalid delta opcode 0");
            return None;
        }
        if out.written > MAX_OBJECT_SIZE {
            println!("delta result too large");
            return None;
        }
    }

    if out.written != target_size {
        println!(
            "delta target size mismatch: got {} expected {}",
            out.written, target_size
        );
        return None;
    }
    if let Some(fd) = base_fd {
        let _ = close(fd);
    }
    Some(out.finish())
}

fn copy_base_range_from_fd(
    fd: usize,
    offset: usize,
    len: usize,
    out: &mut HashFileWriter,
) -> Option<()> {
    let mut buf = [0u8; 4096];
    let mut done = 0usize;
    while done < len {
        let chunk_len = (len - done).min(buf.len());
        let n = pread64(fd, &mut buf[..chunk_len], offset + done);
        if n < 0 {
            println!("pread delta base failed: {}", n);
            return None;
        }
        if n == 0 {
            println!("short delta base read");
            return None;
        }
        let n = (n as usize).min(chunk_len);
        out.write_bytes(&buf[..n])?;
        done += n;
    }
    Some(())
}

fn apply_delta(base: &[u8], delta: &[u8]) -> Option<Vec<u8>> {
    let mut pos = 0usize;
    let source_size = read_delta_size(delta, &mut pos)?;
    let target_size = read_delta_size(delta, &mut pos)?;
    if source_size != base.len() {
        println!(
            "delta source size mismatch: base {} delta {}",
            base.len(),
            source_size
        );
        return None;
    }

    let mut out = Vec::new();
    while pos < delta.len() {
        let opcode = delta[pos];
        pos += 1;
        if opcode & 0x80 != 0 {
            let mut copy_offset = 0usize;
            let mut copy_size = 0usize;
            if opcode & 0x01 != 0 {
                copy_offset |= read_delta_byte(delta, &mut pos)? as usize;
            }
            if opcode & 0x02 != 0 {
                copy_offset |= (read_delta_byte(delta, &mut pos)? as usize) << 8;
            }
            if opcode & 0x04 != 0 {
                copy_offset |= (read_delta_byte(delta, &mut pos)? as usize) << 16;
            }
            if opcode & 0x08 != 0 {
                copy_offset |= (read_delta_byte(delta, &mut pos)? as usize) << 24;
            }
            if opcode & 0x10 != 0 {
                copy_size |= read_delta_byte(delta, &mut pos)? as usize;
            }
            if opcode & 0x20 != 0 {
                copy_size |= (read_delta_byte(delta, &mut pos)? as usize) << 8;
            }
            if opcode & 0x40 != 0 {
                copy_size |= (read_delta_byte(delta, &mut pos)? as usize) << 16;
            }
            if copy_size == 0 {
                copy_size = 0x10000;
            }
            if copy_offset + copy_size > base.len() {
                println!("delta copy out of range");
                return None;
            }
            out.extend_from_slice(&base[copy_offset..copy_offset + copy_size]);
        } else if opcode != 0 {
            let insert_len = opcode as usize;
            if pos + insert_len > delta.len() {
                println!("delta insert out of range");
                return None;
            }
            out.extend_from_slice(&delta[pos..pos + insert_len]);
            pos += insert_len;
        } else {
            println!("invalid delta opcode 0");
            return None;
        }
        if out.len() > MAX_OBJECT_SIZE {
            println!("delta result too large");
            return None;
        }
    }

    if out.len() != target_size {
        println!(
            "delta target size mismatch: got {} expected {}",
            out.len(),
            target_size
        );
        return None;
    }
    Some(out)
}

fn read_delta_size(input: &[u8], pos: &mut usize) -> Option<usize> {
    let mut out = 0usize;
    let mut shift = 0usize;
    loop {
        let b = read_delta_byte(input, pos)?;
        out |= ((b & 0x7f) as usize) << shift;
        if b & 0x80 == 0 {
            return Some(out);
        }
        shift += 7;
        if shift >= usize::BITS as usize {
            return None;
        }
    }
}

fn read_delta_byte(input: &[u8], pos: &mut usize) -> Option<u8> {
    let b = *input.get(*pos)?;
    *pos += 1;
    Some(b)
}

fn bytes_to_string(input: &[u8]) -> String {
    let mut out = String::new();
    for &b in input {
        if b.is_ascii_graphic() || b == b' ' {
            out.push(b as char);
        } else {
            out.push('.');
        }
    }
    out
}

fn git_object_oid(typ: u8, data: &[u8]) -> [u8; 20] {
    let mut header = Vec::new();
    header.extend_from_slice(object_type_name(typ).as_bytes());
    header.push(b' ');
    append_usize(&mut header, data.len());
    header.push(0);
    sha1_parts(&[&header, data])
}

fn git_object_oid_from_file(typ: u8, size: usize, path: &str) -> Option<[u8; 20]> {
    let mut header = Vec::new();
    header.extend_from_slice(object_type_name(typ).as_bytes());
    header.push(b' ');
    append_usize(&mut header, size);
    header.push(0);

    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        println!("open input failed: {}", fd);
        return None;
    }
    let fd = fd as usize;
    let mut sha = Sha1State::new();
    sha.update(&header);
    let mut total = 0usize;
    let mut buf = [0u8; 4096];
    while total < size {
        let n = read(fd, &mut buf);
        if n < 0 {
            println!("read input failed: {}", n);
            let _ = close(fd);
            return None;
        }
        if n == 0 {
            println!("short input file");
            let _ = close(fd);
            return None;
        }
        let n = (n as usize).min(size - total);
        sha.update(&buf[..n]);
        total += n;
    }
    let _ = close(fd);
    Some(sha.finish())
}

fn commit_tree_oid(data: &[u8]) -> Option<[u8; 20]> {
    let prefix = b"tree ";
    if data.len() < prefix.len() + 40 || &data[..prefix.len()] != prefix {
        return None;
    }
    parse_hex_oid(&data[prefix.len()..prefix.len() + 40])
}

fn select_tip_commit(objects: &[PackedObject]) -> Option<PackedObject> {
    let mut first_commit = None;
    for obj in objects {
        if obj.typ != 1 {
            continue;
        }
        if first_commit.is_none() {
            first_commit = Some(clone_object_for_lookup(obj));
        }
        if !is_parent_of_any_commit(&obj.oid, objects) {
            return Some(clone_object_for_lookup(obj));
        }
    }
    first_commit
}

fn select_checkout_commit(db: &ObjectDb<'_>, meta_path: Option<&str>) -> Option<PackedObject> {
    if let Some(meta) = meta_path.and_then(read_git_meta) {
        if let Some(oid) = meta.oid {
            if let Some(commit) = find_object(db, &oid, 1) {
                return Some(commit);
            }
            print!("checkout commit missing from pack: ");
            print_oid(&oid);
            println!("");
            return None;
        }
    }
    select_tip_commit(db.packed)
}

fn is_parent_of_any_commit(oid: &[u8; 20], objects: &[PackedObject]) -> bool {
    for obj in objects {
        if obj.typ == 1 && commit_has_parent(&obj.data, oid) {
            return true;
        }
    }
    false
}

fn commit_has_parent(data: &[u8], oid: &[u8; 20]) -> bool {
    let mut pos = 0usize;
    while pos < data.len() {
        let line_start = pos;
        while pos < data.len() && data[pos] != b'\n' {
            pos += 1;
        }
        let line = &data[line_start..pos];
        if line.len() == 47 && &line[..7] == b"parent " {
            if let Some(parent) = parse_hex_oid(&line[7..]) {
                if &parent == oid {
                    return true;
                }
            }
        }
        pos += usize::from(pos < data.len());
    }
    false
}

fn checkout_tree(db: &ObjectDb<'_>, tree_oid: &[u8; 20], out_dir: &str) -> Option<()> {
    let tree = find_object(db, tree_oid, 2)?;
    let mut tree_buf = [0u8; MAX_TREE_DATA_SIZE];
    let tree_data = object_data(&tree, &mut tree_buf, 2)?;
    let mut pos = 0usize;
    while pos < tree_data.len() {
        let mode_start = pos;
        while pos < tree_data.len() && tree_data[pos] != b' ' {
            pos += 1;
        }
        if pos >= tree_data.len() {
            println!("invalid tree entry");
            return None;
        }
        let mode = bytes_to_string(&tree_data[mode_start..pos]);
        pos += 1;

        let name_start = pos;
        while pos < tree_data.len() && tree_data[pos] != 0 {
            pos += 1;
        }
        if pos + 21 > tree_data.len() {
            println!("invalid tree entry");
            return None;
        }
        let name = bytes_to_string(&tree_data[name_start..pos]);
        if name.is_empty() || name.as_bytes().iter().any(|&b| b == b'/') {
            println!("unsupported tree entry name");
            return None;
        }
        pos += 1;
        let mut oid = [0u8; 20];
        oid.copy_from_slice(&tree_data[pos..pos + 20]);
        pos += 20;

        let path = join_path(out_dir, &name)?;
        if mode == "40000" {
            let _ = mkdir(&path, 0o755);
            checkout_tree(db, &oid, &path)?;
        } else {
            let blob = find_object(db, &oid, 3)?;
            let ok = if blob.data_spilled {
                let mut src_buf = [0u8; 64];
                let Some(src) = spill_object_path(blob.pack_offset, &mut src_buf) else {
                    return None;
                };
                copy_file(src, &path)
            } else {
                write_file(&path, &blob.data)
            };
            if !ok {
                return None;
            }
            println!("wrote {}", path);
        }
    }
    Some(())
}

fn find_object(db: &ObjectDb<'_>, oid: &[u8; 20], typ: u8) -> Option<PackedObject> {
    for obj in db.packed {
        if obj.typ == typ && &obj.oid == oid {
            return Some(clone_object_for_lookup(obj));
        }
    }
    if let Some(git_dir) = db.git_dir {
        if let Some(obj) = read_loose_object(git_dir, oid) {
            if obj.typ == typ {
                return Some(obj);
            }
        }
    }
    print!("missing object ");
    print_oid(oid);
    println!("");
    None
}

fn clone_object_for_lookup(obj: &PackedObject) -> PackedObject {
    PackedObject {
        pack_offset: obj.pack_offset,
        typ: obj.typ,
        base: obj.base,
        oid: obj.oid,
        resolved: obj.resolved,
        size: obj.size,
        data: obj.data.clone(),
        data_spilled: obj.data_spilled,
    }
}

fn object_data<'a>(
    obj: &'a PackedObject,
    scratch: &'a mut [u8],
    expected_type: u8,
) -> Option<&'a [u8]> {
    if obj.typ != expected_type {
        return None;
    }
    if obj.data_spilled {
        let mut path_buf = [0u8; 64];
        let path = spill_object_path(obj.pack_offset, &mut path_buf)?;
        if obj.size > scratch.len() {
            println!("object too large for stack buffer: {}", obj.size);
            return None;
        }
        read_file_into_buf(path, obj.size, scratch)?;
        Some(&scratch[..obj.size])
    } else {
        Some(&obj.data)
    }
}

fn write_git_repository(
    out_dir: &str,
    objects: &[PackedObject],
    commit_oid: &[u8; 20],
    root_tree: &[u8; 20],
    meta_path: Option<&str>,
    remote_name: &str,
) -> bool {
    let meta = meta_path.and_then(read_git_meta).unwrap_or(GitMeta {
        oid: None,
        ref_name: None,
        url: None,
        remote_refs: Vec::new(),
    });
    let git_dir = match join_path(out_dir, ".git") {
        Some(v) => v,
        None => return false,
    };

    if mkdir(&git_dir, 0o755) < 0 {
        // Existing directories are fine on repeated tests.
    }
    for dir in ["objects", "refs", "refs/heads", "refs/remotes"] {
        let path = match join_path(&git_dir, dir) {
            Some(v) => v,
            None => return false,
        };
        let _ = mkdir(&path, 0o755);
    }

    for obj in objects {
        if !write_loose_object(&git_dir, obj) {
            return false;
        }
    }

    if !write_git_head_and_refs(&git_dir, commit_oid, meta.ref_name.as_deref(), remote_name) {
        return false;
    }
    if !write_all_remote_refs(&git_dir, &meta.remote_refs, remote_name) {
        return false;
    }
    if !write_git_config(&git_dir, meta.url.as_deref(), remote_name) {
        return false;
    }
    let object_db = ObjectDb {
        packed: objects,
        git_dir: Some(&git_dir),
    };
    if !write_git_index(&git_dir, &object_db, root_tree) {
        return false;
    }

    println!("wrote git metadata: {}", git_dir);
    true
}

fn read_git_meta(path: &str) -> Option<GitMeta> {
    let data = read_small_file(path, MAX_META_LEN)?;
    let mut meta = GitMeta {
        oid: None,
        ref_name: None,
        url: None,
        remote_refs: Vec::new(),
    };
    for line in data.split(|&b| b == b'\n') {
        if let Some(rest) = strip_bytes_prefix(line, b"oid ") {
            meta.oid = parse_hex_oid(rest);
        } else if let Some(rest) = strip_bytes_prefix(line, b"ref ") {
            if let Ok(v) = core::str::from_utf8(rest) {
                if !v.is_empty() {
                    meta.ref_name = Some(String::from(v));
                }
            }
        } else if let Some(rest) = strip_bytes_prefix(line, b"url ") {
            if let Ok(v) = core::str::from_utf8(rest) {
                if !v.is_empty() {
                    meta.url = Some(String::from(v));
                }
            }
        } else if let Some(rest) = strip_bytes_prefix(line, b"remote-ref ") {
            if let Some(space) = find_byte(rest, b' ') {
                let oid = &rest[..space];
                let name = &rest[space + 1..];
                if let (Some(oid), Ok(name)) = (parse_hex_oid(oid), core::str::from_utf8(name)) {
                    if is_safe_head_ref(name) {
                        meta.remote_refs.push(GitMetaRef {
                            name: String::from(name),
                            oid,
                        });
                    }
                }
            }
        }
    }
    Some(meta)
}

fn write_loose_object(git_dir: &str, obj: &PackedObject) -> bool {
    let oid_hex = oid_to_hex(&obj.oid);
    let object_dir = match join_path(
        &match join_path(git_dir, "objects") {
            Some(v) => v,
            None => return false,
        },
        &oid_hex[..2],
    ) {
        Some(v) => v,
        None => return false,
    };
    let _ = mkdir(&object_dir, 0o755);
    let object_path = match join_path(&object_dir, &oid_hex[2..]) {
        Some(v) => v,
        None => return false,
    };

    let mut header = Vec::new();
    header.extend_from_slice(object_type_name(obj.typ).as_bytes());
    header.push(b' ');
    append_usize(&mut header, obj.size);
    header.push(0);

    if obj.data_spilled {
        let mut path_buf = [0u8; 64];
        let Some(data_path) = spill_object_path(obj.pack_offset, &mut path_buf) else {
            return false;
        };
        write_zlib_store_file_from_path(&object_path, &header, data_path, obj.size)
    } else {
        write_zlib_store_file(&object_path, &[&header, &obj.data])
    }
}

fn write_git_head_and_refs(
    git_dir: &str,
    commit_oid: &[u8; 20],
    ref_name: Option<&str>,
    remote_name: &str,
) -> bool {
    let oid_hex = oid_to_hex(commit_oid);
    let branch = ref_name.and_then(|v| if is_safe_head_ref(v) { Some(v) } else { None });

    let head_path = match join_path(git_dir, "HEAD") {
        Some(v) => v,
        None => return false,
    };

    if let Some(branch) = branch {
        let mut head = Vec::new();
        head.extend_from_slice(b"ref: ");
        head.extend_from_slice(branch.as_bytes());
        head.push(b'\n');
        if !write_file(&head_path, &head) {
            return false;
        }
        if !mkdir_ref_parents(git_dir, branch) {
            return false;
        }
        let ref_path = match join_path(git_dir, branch) {
            Some(v) => v,
            None => return false,
        };
        let mut ref_data = Vec::new();
        ref_data.extend_from_slice(oid_hex.as_bytes());
        ref_data.push(b'\n');
        if !write_file(&ref_path, &ref_data) {
            return false;
        }
        write_origin_tracking_ref(git_dir, branch, remote_name, &ref_data)
    } else {
        let mut head = Vec::new();
        head.extend_from_slice(oid_hex.as_bytes());
        head.push(b'\n');
        write_file(&head_path, &head)
    }
}

fn write_origin_tracking_ref(
    git_dir: &str,
    branch_ref: &str,
    remote_name: &str,
    ref_data: &[u8],
) -> bool {
    let Some(branch_name) = strip_prefix(branch_ref, "refs/heads/") else {
        return true;
    };
    if branch_name.is_empty() || branch_name.ends_with('/') {
        return true;
    }

    let mut remote_ref = String::new();
    remote_ref.push_str("refs/remotes/");
    remote_ref.push_str(remote_name);
    remote_ref.push('/');
    remote_ref.push_str(branch_name);
    if !is_safe_remote_ref(&remote_ref) {
        return true;
    }
    if !mkdir_ref_parents(git_dir, &remote_ref) {
        return false;
    }
    let ref_path = match join_path(git_dir, &remote_ref) {
        Some(v) => v,
        None => return false,
    };
    if !write_file(&ref_path, ref_data) {
        return false;
    }
    println!("wrote remote ref: {}", remote_ref);
    true
}

fn write_all_remote_refs(git_dir: &str, refs: &[GitMetaRef], remote_name: &str) -> bool {
    for r in refs {
        let mut ref_data = Vec::new();
        let oid_hex = oid_to_hex(&r.oid);
        ref_data.extend_from_slice(oid_hex.as_bytes());
        ref_data.push(b'\n');
        if !write_origin_tracking_ref(git_dir, &r.name, remote_name, &ref_data) {
            return false;
        }
    }
    true
}

fn write_git_config(git_dir: &str, url: Option<&str>, remote_name: &str) -> bool {
    let config_path = match join_path(git_dir, "config") {
        Some(v) => v,
        None => return false,
    };
    let mut data = Vec::new();
    data.extend_from_slice(
        b"[core]\n\trepositoryformatversion = 0\n\tfilemode = false\n\tbare = false\n",
    );
    if let Some(url) = url {
        data.extend_from_slice(b"[remote \"");
        data.extend_from_slice(remote_name.as_bytes());
        data.extend_from_slice(b"\"]\n\turl = ");
        data.extend_from_slice(url.as_bytes());
        data.extend_from_slice(b"\n\tfetch = +refs/heads/*:refs/remotes/");
        data.extend_from_slice(remote_name.as_bytes());
        data.extend_from_slice(b"/*\n");
    }
    write_file(&config_path, &data)
}

fn is_safe_remote_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && !name.contains('/')
        && !name.contains("..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn write_git_index(git_dir: &str, db: &ObjectDb<'_>, root_tree: &[u8; 20]) -> bool {
    let mut entries = Vec::new();
    if collect_index_entries(db, root_tree, "", &mut entries).is_none() {
        return false;
    }
    entries.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));

    let index_path = match join_path(git_dir, "index") {
        Some(v) => v,
        None => return false,
    };
    let mut data = Vec::new();
    data.extend_from_slice(b"DIRC");
    append_be_u32(&mut data, GIT_INDEX_VERSION);
    append_be_u32(&mut data, entries.len() as u32);

    for entry in &entries {
        if !append_index_entry(&mut data, entry) {
            return false;
        }
    }

    let checksum = sha1(&data);
    data.extend_from_slice(&checksum);
    if !write_file(&index_path, &data) {
        return false;
    }
    println!("wrote git index: {} entries", entries.len());
    true
}

fn collect_index_entries(
    db: &ObjectDb<'_>,
    tree_oid: &[u8; 20],
    prefix: &str,
    out: &mut Vec<IndexEntry>,
) -> Option<()> {
    let tree = find_object(db, tree_oid, 2)?;
    let mut tree_buf = [0u8; MAX_TREE_DATA_SIZE];
    let tree_data = object_data(&tree, &mut tree_buf, 2)?;
    let mut pos = 0usize;
    while pos < tree_data.len() {
        let mode_start = pos;
        while pos < tree_data.len() && tree_data[pos] != b' ' {
            pos += 1;
        }
        if pos >= tree_data.len() {
            println!("invalid tree entry");
            return None;
        }
        let mode = bytes_to_string(&tree_data[mode_start..pos]);
        pos += 1;

        let name_start = pos;
        while pos < tree_data.len() && tree_data[pos] != 0 {
            pos += 1;
        }
        if pos + 21 > tree_data.len() {
            println!("invalid tree entry");
            return None;
        }
        let name = bytes_to_string(&tree_data[name_start..pos]);
        if name.is_empty() || name.as_bytes().iter().any(|&b| b == b'/') {
            println!("unsupported tree entry name");
            return None;
        }
        pos += 1;
        let mut oid = [0u8; 20];
        oid.copy_from_slice(&tree_data[pos..pos + 20]);
        pos += 20;

        let rel_path = join_rel_path(prefix, &name)?;
        if mode == "40000" {
            collect_index_entries(db, &oid, &rel_path, out)?;
        } else {
            let blob = find_object(db, &oid, 3)?;
            out.push(IndexEntry {
                path: rel_path,
                mode: git_index_mode(&mode),
                oid,
                size: blob.size,
            });
        }
    }
    Some(())
}

fn append_index_entry(out: &mut Vec<u8>, entry: &IndexEntry) -> bool {
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
        return false;
    }
    let name_len = path.len().min(0x0fff) as u16;
    append_be_u16(out, name_len);
    out.extend_from_slice(path);
    out.push(0);
    while (out.len() - entry_start) % 8 != 0 {
        out.push(0);
    }
    true
}

fn git_index_mode(mode: &str) -> u32 {
    match mode {
        "100755" => 0o100755,
        "120000" => 0o120000,
        "160000" => 0o160000,
        _ => 0o100644,
    }
}

fn mkdir_ref_parents(git_dir: &str, ref_name: &str) -> bool {
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

fn is_safe_head_ref(input: &str) -> bool {
    if !starts_with(input, "refs/heads/") {
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

fn is_safe_remote_ref(input: &str) -> bool {
    if !starts_with(input, "refs/remotes/") {
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

fn read_small_file(path: &str, max_len: usize) -> Option<Vec<u8>> {
    read_file_limited(path, max_len)
}

fn read_file_limited(path: &str, max_len: usize) -> Option<Vec<u8>> {
    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        return None;
    }
    let fd = fd as usize;
    let mut out = Vec::new();
    let mut buf = [0u8; 128];
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

fn read_file_into_buf(path: &str, expected_len: usize, out: &mut [u8]) -> Option<()> {
    if expected_len > out.len() {
        return None;
    }
    let fd = open(AT_FDCWD, path, OpenFlags::RDONLY, 0);
    if fd < 0 {
        println!("open input failed: {}", fd);
        return None;
    }
    let fd = fd as usize;
    let mut total = 0usize;
    while total < expected_len {
        let n = read(fd, &mut out[total..expected_len]);
        if n < 0 {
            println!("read input failed: {}", n);
            let _ = close(fd);
            return None;
        }
        if n == 0 {
            println!("short input file");
            let _ = close(fd);
            return None;
        }
        total += n as usize;
    }
    let _ = close(fd);
    Some(())
}

fn spill_object_path<'a>(offset: usize, out: &'a mut [u8; 64]) -> Option<&'a str> {
    let prefix = b"/tmp/gitcheckout-";
    out[..prefix.len()].copy_from_slice(prefix);
    let mut pos = prefix.len();
    append_usize_buf(out, &mut pos, offset)?;
    core::str::from_utf8(&out[..pos]).ok()
}

fn write_file(path: &str, data: &[u8]) -> bool {
    let fd = open(
        AT_FDCWD,
        path,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::WRONLY,
        0o644,
    );
    if fd < 0 {
        println!("open output failed: {}", fd);
        return false;
    }
    let fd = fd as usize;
    let mut written = 0usize;
    while written < data.len() {
        let n = write(fd, &data[written..]);
        if n < 0 {
            println!("write output failed: {}", n);
            let _ = close(fd);
            return false;
        }
        if n == 0 {
            println!("write output returned 0");
            let _ = close(fd);
            return false;
        }
        written += n as usize;
    }
    let _ = close(fd);
    true
}

fn copy_file(src: &str, dst: &str) -> bool {
    let in_fd = open(AT_FDCWD, src, OpenFlags::RDONLY, 0);
    if in_fd < 0 {
        println!("open input failed: {}", in_fd);
        return false;
    }
    let out_fd = open(
        AT_FDCWD,
        dst,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::WRONLY,
        0o644,
    );
    if out_fd < 0 {
        println!("open output failed: {}", out_fd);
        let _ = close(in_fd as usize);
        return false;
    }
    let in_fd = in_fd as usize;
    let out_fd = out_fd as usize;
    let mut buf = [0u8; 4096];
    loop {
        let n = read(in_fd, &mut buf);
        if n < 0 {
            println!("read input failed: {}", n);
            let _ = close(in_fd);
            let _ = close(out_fd);
            return false;
        }
        if n == 0 {
            break;
        }
        if !write_all_fd(out_fd, &buf[..n as usize]) {
            let _ = close(in_fd);
            let _ = close(out_fd);
            return false;
        }
    }
    let _ = close(in_fd);
    let _ = close(out_fd);
    true
}

fn write_zlib_store_file(path: &str, parts: &[&[u8]]) -> bool {
    let fd = open(
        AT_FDCWD,
        path,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::WRONLY,
        0o644,
    );
    if fd < 0 {
        println!("open output failed: {}", fd);
        return false;
    }
    let fd = fd as usize;

    if !write_all_fd(fd, &[0x78, 0x01]) {
        let _ = close(fd);
        return false;
    }

    let mut remaining = parts.iter().map(|part| part.len()).sum::<usize>();
    if remaining == 0 {
        if !write_all_fd(fd, &[1, 0, 0, 0xff, 0xff]) {
            let _ = close(fd);
            return false;
        }
    } else {
        for part in parts {
            let mut pos = 0usize;
            while pos < part.len() {
                let chunk_len = (part.len() - pos).min(65535);
                remaining -= chunk_len;
                let final_block = remaining == 0;
                let len = chunk_len as u16;
                let nlen = !len;
                let block_header = [
                    if final_block { 1 } else { 0 },
                    (len & 0xff) as u8,
                    (len >> 8) as u8,
                    (nlen & 0xff) as u8,
                    (nlen >> 8) as u8,
                ];
                if !write_all_fd(fd, &block_header)
                    || !write_all_fd(fd, &part[pos..pos + chunk_len])
                {
                    let _ = close(fd);
                    return false;
                }
                pos += chunk_len;
            }
        }
    }

    let sum = adler32_parts(parts);
    let ok = write_all_fd(fd, &sum.to_be_bytes());
    let _ = close(fd);
    ok
}

fn write_zlib_store_file_from_path(
    path: &str,
    header: &[u8],
    data_path: &str,
    data_len: usize,
) -> bool {
    let out_fd = open(
        AT_FDCWD,
        path,
        OpenFlags::O_CREAT | OpenFlags::O_TRUNC | OpenFlags::WRONLY,
        0o644,
    );
    if out_fd < 0 {
        println!("open output failed: {}", out_fd);
        return false;
    }
    let in_fd = open(AT_FDCWD, data_path, OpenFlags::RDONLY, 0);
    if in_fd < 0 {
        println!("open input failed: {}", in_fd);
        let _ = close(out_fd as usize);
        return false;
    }
    let out_fd = out_fd as usize;
    let in_fd = in_fd as usize;

    if !write_all_fd(out_fd, &[0x78, 0x01]) {
        let _ = close(in_fd);
        let _ = close(out_fd);
        return false;
    }

    let mut a = 1u32;
    let mut b = 0u32;
    let mut remaining = header.len() + data_len;
    if remaining == 0 {
        if !write_all_fd(out_fd, &[1, 0, 0, 0xff, 0xff]) {
            let _ = close(in_fd);
            let _ = close(out_fd);
            return false;
        }
    } else if !write_zlib_store_chunk(out_fd, header, &mut remaining, &mut a, &mut b) {
        let _ = close(in_fd);
        let _ = close(out_fd);
        return false;
    }

    let mut buf = [0u8; 4096];
    let mut read_total = 0usize;
    while read_total < data_len {
        let n = read(in_fd, &mut buf);
        if n < 0 {
            println!("read input failed: {}", n);
            let _ = close(in_fd);
            let _ = close(out_fd);
            return false;
        }
        if n == 0 {
            println!("short input file");
            let _ = close(in_fd);
            let _ = close(out_fd);
            return false;
        }
        let n = (n as usize).min(data_len - read_total);
        if !write_zlib_store_chunk(out_fd, &buf[..n], &mut remaining, &mut a, &mut b) {
            let _ = close(in_fd);
            let _ = close(out_fd);
            return false;
        }
        read_total += n;
    }

    let sum = (b << 16) | a;
    let ok = write_all_fd(out_fd, &sum.to_be_bytes());
    let _ = close(in_fd);
    let _ = close(out_fd);
    ok
}

fn write_zlib_store_chunk(
    fd: usize,
    data: &[u8],
    remaining: &mut usize,
    a: &mut u32,
    b: &mut u32,
) -> bool {
    let mut pos = 0usize;
    while pos < data.len() {
        let chunk_len = (data.len() - pos).min(65535);
        *remaining -= chunk_len;
        let final_block = *remaining == 0;
        let len = chunk_len as u16;
        let nlen = !len;
        let block_header = [
            if final_block { 1 } else { 0 },
            (len & 0xff) as u8,
            (len >> 8) as u8,
            (nlen & 0xff) as u8,
            (nlen >> 8) as u8,
        ];
        let chunk = &data[pos..pos + chunk_len];
        update_adler32(a, b, chunk);
        if !write_all_fd(fd, &block_header) || !write_all_fd(fd, chunk) {
            return false;
        }
        pos += chunk_len;
    }
    true
}

fn update_adler32(a: &mut u32, b: &mut u32, data: &[u8]) {
    for &byte in data {
        update_adler32_byte(a, b, byte);
    }
}

fn update_adler32_byte(a: &mut u32, b: &mut u32, byte: u8) {
    const MOD: u32 = 65521;
    *a = (*a + byte as u32) % MOD;
    *b = (*b + *a) % MOD;
}

fn write_all_fd(fd: usize, data: &[u8]) -> bool {
    let mut written = 0usize;
    while written < data.len() {
        let n = write(fd, &data[written..]);
        if n < 0 {
            println!("write output failed: {}", n);
            return false;
        }
        if n == 0 {
            println!("write output returned 0");
            return false;
        }
        written += n as usize;
    }
    true
}

fn join_path(parent: &str, name: &str) -> Option<String> {
    if parent.len() + name.len() + 2 > MAX_CHECKOUT_PATH {
        println!("checkout path too long");
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
    if parent.len() + name.len() + 2 > MAX_CHECKOUT_PATH {
        println!("index path too long");
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

fn strip_bytes_prefix<'a>(input: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    if input.len() >= prefix.len() && &input[..prefix.len()] == prefix {
        Some(&input[prefix.len()..])
    } else {
        None
    }
}

fn find_byte(input: &[u8], byte: u8) -> Option<usize> {
    input.iter().position(|&b| b == byte)
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

fn append_usize_buf(out: &mut [u8], pos: &mut usize, mut value: usize) -> Option<()> {
    let mut tmp = [0u8; 20];
    let mut n = 0usize;
    if value == 0 {
        tmp[0] = b'0';
        n = 1;
    } else {
        while value > 0 {
            tmp[n] = b'0' + (value % 10) as u8;
            value /= 10;
            n += 1;
        }
    }
    while n > 0 {
        n -= 1;
        if *pos >= out.len() {
            return None;
        }
        out[*pos] = tmp[n];
        *pos += 1;
    }
    Some(())
}

struct Sha1State {
    h0: u32,
    h1: u32,
    h2: u32,
    h3: u32,
    h4: u32,
    total_len: u64,
    block: [u8; 64],
    block_len: usize,
}

impl Sha1State {
    fn new() -> Self {
        Self {
            h0: 0x67452301,
            h1: 0xefcdab89,
            h2: 0x98badcfe,
            h3: 0x10325476,
            h4: 0xc3d2e1f0,
            total_len: 0,
            block: [0u8; 64],
            block_len: 0,
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.total_len += data.len() as u64;
        for &byte in data {
            self.block[self.block_len] = byte;
            self.block_len += 1;
            if self.block_len == 64 {
                sha1_process_block(
                    &self.block,
                    &mut self.h0,
                    &mut self.h1,
                    &mut self.h2,
                    &mut self.h3,
                    &mut self.h4,
                );
                self.block_len = 0;
            }
        }
    }

    fn finish(mut self) -> [u8; 20] {
        let bit_len = self.total_len * 8;
        self.block[self.block_len] = 0x80;
        self.block_len += 1;
        if self.block_len > 56 {
            for b in self.block.iter_mut().skip(self.block_len) {
                *b = 0;
            }
            sha1_process_block(
                &self.block,
                &mut self.h0,
                &mut self.h1,
                &mut self.h2,
                &mut self.h3,
                &mut self.h4,
            );
            self.block_len = 0;
        }
        for b in self.block.iter_mut().take(56).skip(self.block_len) {
            *b = 0;
        }
        self.block[56..64].copy_from_slice(&bit_len.to_be_bytes());
        sha1_process_block(
            &self.block,
            &mut self.h0,
            &mut self.h1,
            &mut self.h2,
            &mut self.h3,
            &mut self.h4,
        );

        let mut out = [0u8; 20];
        out[..4].copy_from_slice(&self.h0.to_be_bytes());
        out[4..8].copy_from_slice(&self.h1.to_be_bytes());
        out[8..12].copy_from_slice(&self.h2.to_be_bytes());
        out[12..16].copy_from_slice(&self.h3.to_be_bytes());
        out[16..20].copy_from_slice(&self.h4.to_be_bytes());
        out
    }
}

fn sha1(input: &[u8]) -> [u8; 20] {
    sha1_parts(&[input])
}

fn sha1_parts(parts: &[&[u8]]) -> [u8; 20] {
    let mut h0 = 0x67452301u32;
    let mut h1 = 0xefcdab89u32;
    let mut h2 = 0x98badcfeu32;
    let mut h3 = 0x10325476u32;
    let mut h4 = 0xc3d2e1f0u32;

    let total_len = parts.iter().map(|part| part.len() as u64).sum::<u64>();
    let bit_len = total_len * 8;
    let mut block = [0u8; 64];
    let mut block_len = 0usize;

    for part in parts {
        for &byte in *part {
            block[block_len] = byte;
            block_len += 1;
            if block_len == 64 {
                sha1_process_block(&block, &mut h0, &mut h1, &mut h2, &mut h3, &mut h4);
                block_len = 0;
            }
        }
    }

    block[block_len] = 0x80;
    block_len += 1;
    if block_len > 56 {
        for b in block.iter_mut().skip(block_len) {
            *b = 0;
        }
        sha1_process_block(&block, &mut h0, &mut h1, &mut h2, &mut h3, &mut h4);
        block_len = 0;
    }
    for b in block.iter_mut().take(56).skip(block_len) {
        *b = 0;
    }
    block[56..64].copy_from_slice(&bit_len.to_be_bytes());
    sha1_process_block(&block, &mut h0, &mut h1, &mut h2, &mut h3, &mut h4);

    let mut out = [0u8; 20];
    out[..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

fn sha1_process_block(
    chunk: &[u8; 64],
    h0: &mut u32,
    h1: &mut u32,
    h2: &mut u32,
    h3: &mut u32,
    h4: &mut u32,
) {
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

    let mut a = *h0;
    let mut b = *h1;
    let mut c = *h2;
    let mut d = *h3;
    let mut e = *h4;

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

    *h0 = (*h0).wrapping_add(a);
    *h1 = (*h1).wrapping_add(b);
    *h2 = (*h2).wrapping_add(c);
    *h3 = (*h3).wrapping_add(d);
    *h4 = (*h4).wrapping_add(e);
}

fn print_trailer(trailer: &[u8; PACK_TRAILER_LEN]) {
    print!("trailer sha1:");
    for &b in trailer {
        print!("{:02x}", b);
    }
    println!("");
}

fn read_be_u32(input: &[u8], offset: usize) -> u32 {
    ((input[offset] as u32) << 24)
        | ((input[offset + 1] as u32) << 16)
        | ((input[offset + 2] as u32) << 8)
        | input[offset + 3] as u32
}

fn object_type_name(typ: u8) -> &'static str {
    match typ {
        1 => "commit",
        2 => "tree",
        3 => "blob",
        4 => "tag",
        6 => "ofs-delta",
        7 => "ref-delta",
        _ => "unknown",
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
            if len > 512 {
                return None;
            }
        }
        core::str::from_utf8(core::slice::from_raw_parts(ptr, len)).ok()
    }
}

fn print_usage() {
    println!("usage: gitcheckout [pack-file] [output-dir] [--git] [--meta PATH] [--quiet]");
    println!("default pack: {}", DEFAULT_PACK);
    println!("default output: {}", DEFAULT_OUT_DIR);
    println!("      --git        write minimal .git metadata and loose objects");
    println!("  -q, --quiet      hide per-object checkout logs");
    println!(
        "      --meta PATH  metadata from gitfetch, default {}",
        DEFAULT_META
    );
}
