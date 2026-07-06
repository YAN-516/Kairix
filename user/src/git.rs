use alloc::vec::Vec;
use core::str;

pub const PKT_HEADER_LEN: usize = 4;
pub const PKT_MAX_LEN: usize = 0xffff;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PktLineError {
    Incomplete,
    InvalidHex,
    InvalidLength,
    TooLong,
    OutputTooSmall,
    InvalidUtf8,
    InvalidRefLine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PktLine<'a> {
    Flush,
    Data(&'a [u8]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitRef<'a> {
    pub oid: &'a str,
    pub name: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefAdvertisement<'a> {
    pub head: Option<GitRef<'a>>,
    pub refs: &'a [GitRef<'a>],
    pub capabilities: &'a [&'a str],
}

pub fn parse_pkt_line(input: &[u8]) -> Result<(PktLine<'_>, usize), PktLineError> {
    if input.len() < PKT_HEADER_LEN {
        return Err(PktLineError::Incomplete);
    }

    let len = decode_len(&input[..PKT_HEADER_LEN])?;
    match len {
        0 => Ok((PktLine::Flush, PKT_HEADER_LEN)),
        1..=3 => Err(PktLineError::InvalidLength),
        _ => {
            if len > input.len() {
                return Err(PktLineError::Incomplete);
            }
            Ok((PktLine::Data(&input[PKT_HEADER_LEN..len]), len))
        }
    }
}

pub fn parse_pkt_lines(input: &[u8]) -> Result<Vec<PktLine<'_>>, PktLineError> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < input.len() {
        let (line, used) = parse_pkt_line(&input[pos..])?;
        out.push(line);
        pos += used;
    }
    Ok(out)
}

pub fn encode_pkt_data(data: &[u8], out: &mut Vec<u8>) -> Result<(), PktLineError> {
    let len = data.len() + PKT_HEADER_LEN;
    if len > PKT_MAX_LEN {
        return Err(PktLineError::TooLong);
    }
    append_len(len, out);
    out.extend_from_slice(data);
    Ok(())
}

pub fn encode_pkt_flush(out: &mut Vec<u8>) {
    out.extend_from_slice(b"0000");
}

pub fn write_pkt_data(data: &[u8], out: &mut [u8]) -> Result<usize, PktLineError> {
    let len = data.len() + PKT_HEADER_LEN;
    if len > PKT_MAX_LEN {
        return Err(PktLineError::TooLong);
    }
    if out.len() < len {
        return Err(PktLineError::OutputTooSmall);
    }
    write_len(len, &mut out[..PKT_HEADER_LEN]);
    out[PKT_HEADER_LEN..len].copy_from_slice(data);
    Ok(len)
}

pub fn write_pkt_flush(out: &mut [u8]) -> Result<usize, PktLineError> {
    if out.len() < PKT_HEADER_LEN {
        return Err(PktLineError::OutputTooSmall);
    }
    out[..PKT_HEADER_LEN].copy_from_slice(b"0000");
    Ok(PKT_HEADER_LEN)
}

pub fn parse_ref_advertisement<'a>(
    input: &'a [u8],
    refs_out: &'a mut [GitRef<'a>],
    caps_out: &'a mut [&'a str],
) -> Result<RefAdvertisement<'a>, PktLineError> {
    let mut pos = 0usize;
    let mut refs_len = 0usize;
    let mut caps_len = 0usize;
    let mut head = None;
    let mut skipped_service = false;

    while pos < input.len() {
        let (pkt, used) = parse_pkt_line(&input[pos..])?;
        pos += used;

        let data = match pkt {
            PktLine::Flush => continue,
            PktLine::Data(data) => data,
        };
        if data.is_empty() {
            continue;
        }

        let line = trim_lf(str::from_utf8(data).map_err(|_| PktLineError::InvalidUtf8)?);
        if line.is_empty() {
            continue;
        }

        if !skipped_service && line.starts_with("# service=") {
            skipped_service = true;
            continue;
        }
        skipped_service = true;

        let (line, caps) = split_caps(line);
        if caps_len == 0 {
            caps_len = split_capabilities(caps, caps_out)?;
        }

        let git_ref = parse_ref_line(line)?;
        if git_ref.name == "HEAD" {
            head = Some(git_ref);
        }

        if refs_len >= refs_out.len() {
            return Err(PktLineError::OutputTooSmall);
        }
        refs_out[refs_len] = git_ref;
        refs_len += 1;
    }

    Ok(RefAdvertisement {
        head,
        refs: &refs_out[..refs_len],
        capabilities: &caps_out[..caps_len],
    })
}

fn parse_ref_line(line: &str) -> Result<GitRef<'_>, PktLineError> {
    let sep = line.find(' ').ok_or(PktLineError::InvalidRefLine)?;
    let oid = &line[..sep];
    let name = &line[sep + 1..];
    if oid.is_empty() || name.is_empty() || !oid.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(PktLineError::InvalidRefLine);
    }
    Ok(GitRef { oid, name })
}

fn split_caps(line: &str) -> (&str, &str) {
    match line.as_bytes().iter().position(|&b| b == 0) {
        Some(pos) => (&line[..pos], &line[pos + 1..]),
        None => (line, ""),
    }
}

fn split_capabilities<'a>(caps: &'a str, out: &mut [&'a str]) -> Result<usize, PktLineError> {
    if caps.is_empty() {
        return Ok(0);
    }

    let mut len = 0usize;
    for cap in caps.split(' ') {
        if cap.is_empty() {
            continue;
        }
        if len >= out.len() {
            return Err(PktLineError::OutputTooSmall);
        }
        out[len] = cap;
        len += 1;
    }
    Ok(len)
}

fn trim_lf(s: &str) -> &str {
    s.strip_suffix('\n').unwrap_or(s)
}

fn decode_len(header: &[u8]) -> Result<usize, PktLineError> {
    let mut len = 0usize;
    for &b in header {
        len = (len << 4) | hex_value(b)? as usize;
    }
    Ok(len)
}

fn hex_value(b: u8) -> Result<u8, PktLineError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(PktLineError::InvalidHex),
    }
}

fn append_len(len: usize, out: &mut Vec<u8>) {
    let mut buf = [0u8; PKT_HEADER_LEN];
    write_len(len, &mut buf);
    out.extend_from_slice(&buf);
}

fn write_len(len: usize, out: &mut [u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out[0] = HEX[(len >> 12) & 0xf];
    out[1] = HEX[(len >> 8) & 0xf];
    out[2] = HEX[(len >> 4) & 0xf];
    out[3] = HEX[len & 0xf];
}
