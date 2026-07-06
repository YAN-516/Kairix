#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;
extern crate alloc;

use alloc::vec::Vec;
use user_lib::git::{
    GitRef, PktLine, encode_pkt_data, encode_pkt_flush, parse_pkt_line, parse_ref_advertisement,
    write_pkt_data, write_pkt_flush,
};

#[unsafe(no_mangle)]
fn main() -> i32 {
    let mut ok = true;

    let mut encoded = Vec::new();
    ok &= expect(
        "encode data",
        encode_pkt_data(b"hello\n", &mut encoded).is_ok(),
    );
    ok &= expect("encoded header", encoded.as_slice() == b"000ahello\n");

    match parse_pkt_line(&encoded) {
        Ok((PktLine::Data(data), used)) => {
            ok &= expect("parse data body", data == b"hello\n");
            ok &= expect("parse data used", used == encoded.len());
        }
        _ => ok = expect("parse data", false),
    }

    let mut flush = [0u8; 4];
    ok &= expect("write flush", write_pkt_flush(&mut flush).is_ok());
    ok &= expect("flush bytes", &flush == b"0000");
    ok &= expect(
        "parse flush",
        matches!(parse_pkt_line(&flush), Ok((PktLine::Flush, 4))),
    );

    let mut fixed = [0u8; 16];
    let n = write_pkt_data(b"abc", &mut fixed).unwrap_or(0);
    ok &= expect("write fixed data", &fixed[..n] == b"0007abc");

    let mut adv = Vec::new();
    let _ = encode_pkt_data(b"# service=git-upload-pack\n", &mut adv);
    encode_pkt_flush(&mut adv);
    let _ = encode_pkt_data(
        b"1111111111111111111111111111111111111111 HEAD\0multi_ack side-band-64k\n",
        &mut adv,
    );
    let _ = encode_pkt_data(
        b"2222222222222222222222222222222222222222 refs/heads/main\n",
        &mut adv,
    );
    encode_pkt_flush(&mut adv);

    let mut refs = [GitRef { oid: "", name: "" }; 4];
    let mut caps = [""; 4];
    match parse_ref_advertisement(&adv, &mut refs, &mut caps) {
        Ok(parsed) => {
            ok &= expect("refs len", parsed.refs.len() == 2);
            ok &= expect(
                "head oid",
                parsed.head.map(|r| r.oid) == Some("1111111111111111111111111111111111111111"),
            );
            ok &= expect("main ref", parsed.refs[1].name == "refs/heads/main");
            ok &= expect("caps len", parsed.capabilities.len() == 2);
            ok &= expect("cap side-band", parsed.capabilities[1] == "side-band-64k");
        }
        Err(err) => {
            println!("parse refs failed: {:?}", err);
            ok = false;
        }
    }

    if ok {
        println!("git pkt-line selftest: ok");
        0
    } else {
        println!("git pkt-line selftest: failed");
        -1
    }
}

fn expect(name: &str, cond: bool) -> bool {
    if cond {
        println!("[ok] {}", name);
    } else {
        println!("[fail] {}", name);
    }
    cond
}
