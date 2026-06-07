#!/usr/bin/env python3
"""Patch LoongArch musl scheduler stubs into real syscall wrappers.

Some LoongArch musl builds ship sched_getparam/getscheduler/setparam/
setscheduler as ENOSYS stubs even though the Linux syscall ABI has these
numbers.  cyclictest calls them during privilege setup, so the stubs fail
before the kernel has a chance to handle the syscall.
"""

from __future__ import annotations

import argparse
import struct
import sys
from pathlib import Path


ADDID_SP_MINUS_16 = 0x02FFC063
ST_D_RA_SP_8 = 0x29C02061
LD_D_RA_SP_8 = 0x28C02061
ADDID_SP_PLUS_16 = 0x02C04063
JIRL_ZERO_RA_0 = 0x4C000020
SYSCALL_0 = 0x002B0000
SLLI_W_A0_A0_0 = 0x00408084
ADDW_A0_NEG_ENOSYS = 0x02BF6804
SYSCALL_RET_ADDR = 0x2046C


def addiw_a7(syscall_id: int) -> int:
    return 0x0280000B | ((syscall_id & 0xFFF) << 10)


def bl_to(target: int, pc: int) -> int:
    delta = target - pc
    if delta % 4 != 0:
        raise ValueError(f"unaligned LoongArch branch delta: {delta}")
    imm = delta // 4
    if not (-(1 << 25) <= imm < (1 << 25)):
        raise ValueError(f"LoongArch bl target out of range: pc={pc:#x}, target={target:#x}")
    imm &= (1 << 26) - 1
    return 0x54000000 | ((imm & 0xFFFF) << 10) | ((imm >> 16) & 0x3FF)


def words_to_bytes(words: list[int]) -> bytes:
    return b"".join(struct.pack("<I", word) for word in words)


def patch_one(data: bytearray, name: str, offset: int, syscall_id: int, old_bl: int) -> bool:
    expected = words_to_bytes(
        [
            ADDID_SP_MINUS_16,
            ADDW_A0_NEG_ENOSYS,
            ST_D_RA_SP_8,
            old_bl,
            LD_D_RA_SP_8,
            SLLI_W_A0_A0_0,
            ADDID_SP_PLUS_16,
            JIRL_ZERO_RA_0,
        ]
    )
    found = bytes(data[offset : offset + len(expected)])
    if len(found) != len(expected):
        print(
            f"  warning: {name} offset {offset:#x} is outside this file; leaving it unchanged",
            file=sys.stderr,
        )
        return False
    if found == expected:
        replacement = words_to_bytes(
            [
                ADDID_SP_MINUS_16,
                ST_D_RA_SP_8,
                addiw_a7(syscall_id),
                SYSCALL_0,
                bl_to(SYSCALL_RET_ADDR, offset + 16),
                LD_D_RA_SP_8,
                ADDID_SP_PLUS_16,
                JIRL_ZERO_RA_0,
            ]
        )
        data[offset : offset + len(replacement)] = replacement
        print(f"  patched {name} -> syscall {syscall_id}")
        return True

    replacement = words_to_bytes(
        [
            ADDID_SP_MINUS_16,
            ST_D_RA_SP_8,
            addiw_a7(syscall_id),
            SYSCALL_0,
            bl_to(SYSCALL_RET_ADDR, offset + 16),
            LD_D_RA_SP_8,
            ADDID_SP_PLUS_16,
            JIRL_ZERO_RA_0,
        ]
    )
    if found == replacement:
        print(f"  {name} already patched")
        return False

    if SYSCALL_0 in struct.unpack("<8I", found):
        print(f"  {name} already contains a syscall wrapper; leaving it unchanged")
        return False

    print(
        f"  warning: {name} bytes did not match known LoongArch musl stub at offset {offset:#x}; "
        "leaving it unchanged",
        file=sys.stderr,
    )
    return False


def patch_file(path: Path) -> bool:
    data = bytearray(path.read_bytes())
    changed = False
    for name, offset, syscall_id, old_bl in [
        ("sched_getparam", 0x544E0, 121, 0x54BF83FF),
        ("sched_getscheduler", 0x54500, 120, 0x54BF63FF),
        ("sched_setparam", 0x54544, 118, 0x54BF1FFF),
        ("sched_setscheduler", 0x54564, 119, 0x54BEFFFF),
    ]:
        changed |= patch_one(data, name, offset, syscall_id, old_bl)
    if changed:
        path.write_bytes(data)
    return changed


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("files", nargs="+", type=Path)
    args = parser.parse_args()

    ok = True
    for path in args.files:
        if not path.exists():
            continue
        print(f"Patching {path}")
        try:
            patch_file(path)
        except Exception as err:  # noqa: BLE001 - command-line diagnostics
            print(f"  error: {err}", file=sys.stderr)
            ok = False
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
