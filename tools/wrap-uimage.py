#!/usr/bin/env python3
import argparse
import struct
import time
import zlib


UIMAGE_MAGIC = 0x27051956


def main() -> None:
    parser = argparse.ArgumentParser(description="Wrap a raw kernel binary as a legacy uImage.")
    parser.add_argument("--ref", required=True, help="reference legacy uImage used to copy the arch byte")
    parser.add_argument("--kernel", required=True, help="raw kernel binary")
    parser.add_argument("--out", required=True, help="output uImage")
    parser.add_argument("--name", default="Kairix", help="uImage name")
    parser.add_argument("--load", default="0x80000000", help="load address")
    parser.add_argument("--entry", default="0x80000000", help="entry address")
    args = parser.parse_args()

    with open(args.ref, "rb") as f:
        ref_hdr = f.read(64)
    if len(ref_hdr) != 64 or struct.unpack(">I", ref_hdr[:4])[0] != UIMAGE_MAGIC:
        raise SystemExit(f"{args.ref} is not a legacy uImage")

    arch = ref_hdr[29]
    with open(args.kernel, "rb") as f:
        data = f.read()

    name = args.name.encode("ascii")[:32].ljust(32, b"\0")
    load = int(args.load, 0)
    entry = int(args.entry, 0)

    hdr = bytearray(64)
    struct.pack_into(
        ">7I4B32s",
        hdr,
        0,
        UIMAGE_MAGIC,
        0,
        int(time.time()),
        len(data),
        load,
        entry,
        zlib.crc32(data) & 0xFFFFFFFF,
        5,
        arch,
        2,
        0,
        name,
    )
    struct.pack_into(">I", hdr, 4, zlib.crc32(hdr) & 0xFFFFFFFF)

    with open(args.out, "wb") as f:
        f.write(hdr)
        f.write(data)

    print(f"use arch byte: 0x{arch:02x}")
    print(f"wrote {args.out} ({len(data)} payload bytes)")


if __name__ == "__main__":
    main()
