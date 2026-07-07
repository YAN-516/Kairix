#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::{string::String, vec::Vec};
use user_lib::{AT_FDCWD, OpenFlags, close, mkdir, open, read, write};

const DEFAULT_PACK: &str = "/musl/gitfetch.pack";
const DEFAULT_OUT_DIR: &str = "/musl/checkout";
const PACK_TRAILER_LEN: usize = 20;
const PACK_STREAM_BUF_SIZE: usize = 4096;
const MAX_OBJECT_SIZE: usize = 1024 * 1024;
const MAX_HUFFMAN_BITS: usize = 15;
const MAX_CHECKOUT_PATH: usize = 512;

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
    symbol: usize,
    len: u8,
    code: u16,
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
    data: Vec<u8>,
}

#[derive(Clone)]
struct PackedObject {
    pack_offset: usize,
    typ: u8,
    oid: [u8; 20],
    data: Vec<u8>,
}

struct PackStream {
    fd: usize,
    buf: [u8; PACK_STREAM_BUF_SIZE],
    pos: usize,
    len: usize,
    offset: usize,
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
    let path = if argc > 1 {
        match argv_str(argv, 1) {
            Some("-h") | Some("--help") => {
                print_usage();
                return 0;
            }
            Some(v) => v,
            None => {
                println!("invalid pack path");
                return -1;
            }
        }
    } else {
        DEFAULT_PACK
    };
    let out_dir = if argc > 2 {
        match argv_str(argv, 2) {
            Some(v) => v,
            None => {
                println!("invalid output dir");
                return -1;
            }
        }
    } else {
        DEFAULT_OUT_DIR
    };

    checkout_pack(path, out_dir)
}

fn checkout_pack(path: &str, out_dir: &str) -> i32 {
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
        println!("object[{}].offset: {}", index, offset);
        println!("object[{}].type: {}", index, object_type_name(header.typ));
        println!("object[{}].size: {}", index, header.size);

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
            println!("object[{}].delta base bytes: {}", index, used);
            println!("object[{}].delta base offset: {}", index, base_offset);
            base = DeltaBase::Offset(base_offset);
        } else if header.typ == 7 {
            let mut oid = [0u8; 20];
            if stream.read_exact(&mut oid).is_none() {
                println!("invalid ref-delta base");
                return -1;
            }
            print!("object[{}].delta base oid:", index);
            print_oid(&oid);
            println!("");
            base = DeltaBase::Oid(oid);
        }

        let zlib_offset = stream.offset();
        let mut object = Vec::new();
        let inflated = match inflate_zlib_stream(&mut stream, zlib_offset, header.size, &mut object)
        {
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
        println!("object[{}].zlib offset: {}", index, zlib_offset);
        println!("object[{}].zlib bytes: {}", index, inflated.consumed);
        if header.typ != 6 && header.typ != 7 {
            let oid = git_object_oid(header.typ, &object);
            print!("object[{}].oid: ", index);
            print_oid(&oid);
            println!("");
        }
        raw_objects.push(RawObject {
            pack_offset: offset,
            typ: header.typ,
            base,
            data: object,
        });

        index += 1;
    }

    let mut trailer = [0u8; PACK_TRAILER_LEN];
    if stream.read_exact(&mut trailer).is_none() {
        println!("invalid pack: missing checksum");
        return -1;
    }

    print_trailer(&trailer);
    let parsed_objects = match resolve_objects(&raw_objects) {
        Some(v) => v,
        None => return -1,
    };
    let commit = match select_tip_commit(&parsed_objects) {
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
    match checkout_tree(&parsed_objects, &root_tree, out_dir) {
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

fn inflate_huffman_block(
    reader: &mut StreamBitReader<'_>,
    litlen: &[HuffEntry],
    dist: &[HuffEntry],
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

fn fixed_huffman_tables() -> Option<(Vec<HuffEntry>, Vec<HuffEntry>)> {
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

fn dynamic_huffman_tables(
    reader: &mut StreamBitReader<'_>,
) -> Option<(Vec<HuffEntry>, Vec<HuffEntry>)> {
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
    let mut lengths = Vec::new();
    while lengths.len() < total {
        let sym = decode_symbol(reader, &code_table)?;
        match sym {
            0..=15 => lengths.push(sym as u8),
            16 => {
                let repeat = reader.read_bits(2)? as usize + 3;
                let prev = *lengths.last()?;
                for _ in 0..repeat {
                    lengths.push(prev);
                }
            }
            17 => {
                let repeat = reader.read_bits(3)? as usize + 3;
                for _ in 0..repeat {
                    lengths.push(0);
                }
            }
            18 => {
                let repeat = reader.read_bits(7)? as usize + 11;
                for _ in 0..repeat {
                    lengths.push(0);
                }
            }
            _ => return None,
        }
        if lengths.len() > total {
            println!("too many dynamic huffman lengths");
            return None;
        }
    }

    let litlen = build_huffman(&lengths[..hlit])?;
    let dist = build_huffman(&lengths[hlit..])?;
    Some((litlen, dist))
}

fn build_huffman(lengths: &[u8]) -> Option<Vec<HuffEntry>> {
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

    let mut out = Vec::new();
    for (symbol, &len) in lengths.iter().enumerate() {
        if len == 0 {
            continue;
        }
        let code = next_code[len as usize];
        next_code[len as usize] += 1;
        out.push(HuffEntry {
            symbol,
            len,
            code: reverse_bits(code, len),
        });
    }
    Some(out)
}

fn decode_symbol(reader: &mut StreamBitReader<'_>, table: &[HuffEntry]) -> Option<usize> {
    let mut code = 0u16;
    for len in 1..=MAX_HUFFMAN_BITS {
        code |= (reader.read_bits(1)? as u16) << (len - 1);
        for entry in table {
            if entry.len as usize == len && entry.code == code {
                return Some(entry.symbol);
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
    const MOD: u32 = 65521;
    let mut a = 1u32;
    let mut b = 0u32;
    for &byte in input {
        a = (a + byte as u32) % MOD;
        b = (b + a) % MOD;
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

fn resolve_objects(raw: &[RawObject]) -> Option<Vec<PackedObject>> {
    let mut resolved = Vec::new();

    for obj in raw {
        if obj.typ != 6 && obj.typ != 7 {
            let oid = git_object_oid(obj.typ, &obj.data);
            resolved.push(PackedObject {
                pack_offset: obj.pack_offset,
                typ: obj.typ,
                oid,
                data: obj.data.clone(),
            });
        }
    }

    let mut remaining = raw
        .iter()
        .filter(|obj| obj.typ == 6 || obj.typ == 7)
        .count();
    while remaining > 0 {
        let before = remaining;
        for obj in raw {
            if obj.typ != 6 && obj.typ != 7 {
                continue;
            }
            if resolved.iter().any(|v| v.pack_offset == obj.pack_offset) {
                continue;
            }
            let base = match obj.base {
                DeltaBase::Offset(offset) => resolved.iter().find(|v| v.pack_offset == offset),
                DeltaBase::Oid(oid) => resolved.iter().find(|v| v.oid == oid),
                DeltaBase::None => None,
            };
            let Some(base) = base else {
                continue;
            };
            let data = apply_delta(&base.data, &obj.data)?;
            let oid = git_object_oid(base.typ, &data);
            print!("resolved delta at {} -> ", obj.pack_offset);
            print_oid(&oid);
            println!("");
            resolved.push(PackedObject {
                pack_offset: obj.pack_offset,
                typ: base.typ,
                oid,
                data,
            });
            remaining -= 1;
        }
        if remaining == before {
            println!("unresolved delta objects: {}", remaining);
            return None;
        }
    }

    Some(resolved)
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
    let mut framed = Vec::new();
    framed.extend_from_slice(object_type_name(typ).as_bytes());
    framed.push(b' ');
    append_usize(&mut framed, data.len());
    framed.push(0);
    framed.extend_from_slice(data);
    sha1(&framed)
}

fn commit_tree_oid(data: &[u8]) -> Option<[u8; 20]> {
    let prefix = b"tree ";
    if data.len() < prefix.len() + 40 || &data[..prefix.len()] != prefix {
        return None;
    }
    parse_hex_oid(&data[prefix.len()..prefix.len() + 40])
}

fn select_tip_commit(objects: &[PackedObject]) -> Option<&PackedObject> {
    let mut first_commit = None;
    for obj in objects {
        if obj.typ != 1 {
            continue;
        }
        if first_commit.is_none() {
            first_commit = Some(obj);
        }
        if !is_parent_of_any_commit(&obj.oid, objects) {
            return Some(obj);
        }
    }
    first_commit
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

fn checkout_tree(objects: &[PackedObject], tree_oid: &[u8; 20], out_dir: &str) -> Option<()> {
    let tree = find_object(objects, tree_oid, 2)?;
    let mut pos = 0usize;
    while pos < tree.data.len() {
        let mode_start = pos;
        while pos < tree.data.len() && tree.data[pos] != b' ' {
            pos += 1;
        }
        if pos >= tree.data.len() {
            println!("invalid tree entry");
            return None;
        }
        let mode = bytes_to_string(&tree.data[mode_start..pos]);
        pos += 1;

        let name_start = pos;
        while pos < tree.data.len() && tree.data[pos] != 0 {
            pos += 1;
        }
        if pos + 21 > tree.data.len() {
            println!("invalid tree entry");
            return None;
        }
        let name = bytes_to_string(&tree.data[name_start..pos]);
        if name.is_empty() || name.as_bytes().iter().any(|&b| b == b'/') {
            println!("unsupported tree entry name");
            return None;
        }
        pos += 1;
        let mut oid = [0u8; 20];
        oid.copy_from_slice(&tree.data[pos..pos + 20]);
        pos += 20;

        let path = join_path(out_dir, &name)?;
        if mode == "40000" {
            let _ = mkdir(&path, 0o755);
            checkout_tree(objects, &oid, &path)?;
        } else {
            let blob = find_object(objects, &oid, 3)?;
            if !write_file(&path, &blob.data) {
                return None;
            }
            println!("wrote {}", path);
        }
    }
    Some(())
}

fn find_object<'a>(
    objects: &'a [PackedObject],
    oid: &[u8; 20],
    typ: u8,
) -> Option<&'a PackedObject> {
    for obj in objects {
        if obj.typ == typ && &obj.oid == oid {
            return Some(obj);
        }
    }
    print!("missing object ");
    print_oid(oid);
    println!("");
    None
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

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
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
    println!("usage: gitcheckout [pack-file] [output-dir]");
    println!("default: gitcheckout /musl/gitfetch.pack /musl/checkout");
}
