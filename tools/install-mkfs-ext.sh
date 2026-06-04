#!/usr/bin/env bash
# Install e2fsprogs mkfs.ext tools into a mounted test image.

set -euo pipefail

if [ "$#" -ne 3 ]; then
    echo "Usage: $0 <arch> <mount-dir> <mkfs-tools-dir>" >&2
    exit 1
fi

ARCH="$1"
MNT="$2"
MKFS_DIR="$3"
TOOLS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

case "$ARCH" in
    riscv64 | loongarch64) ;;
    *)
        echo "Error: unsupported ARCH '$ARCH'." >&2
        exit 1
        ;;
esac

install_wrapper() {
    local tool="$1"
    local dir="$2"
    local real="$dir/$tool.real"
    local wrapper="$dir/$tool"

    cp "$MKFS_DIR/$tool" "$real"

    {
        printf '%s\n' '#!/bin/sh'
        printf '%s\n' 'real="${0}.real"'
        printf '%s\n' 'if [ ! -x "$real" ]; then'
        printf '%s\n' "    real=\"/sbin/$tool.real\""
        printf '%s\n' 'fi'
        printf '%s\n' 'export MKE2FS_CONFIG="/sbin/mke2fs.conf"'
        case "$tool" in
            mkfs.ext2)
                printf '%s\n' 'exec "$real" -F -E lazy_itable_init=1,nodiscard "$@"'
                ;;
            mkfs.ext3)
                printf '%s\n' 'exec "$real" -F -E lazy_itable_init=1,lazy_journal_init=1,nodiscard "$@"'
                ;;
            mkfs.ext4)
                printf '%s\n' 'exec "$real" -F -E lazy_itable_init=1,lazy_journal_init=1,nodiscard -O ^metadata_csum,^metadata_csum_seed,^orphan_file "$@"'
                ;;
        esac
    } > "$wrapper"

    chmod +x "$real" "$wrapper"
}

mkdir -p "$MNT/bin" "$MNT/sbin" "$MNT/musl/ltp/testcases/bin"

for dir in "$MNT/bin" "$MNT/sbin" "$MNT/musl/ltp/testcases/bin"; do
    for tool in mkfs.ext2 mkfs.ext3 mkfs.ext4; do
        rm -f "$dir/$tool" "$dir/$tool.real"
    done
done

for tool in mkfs.ext2 mkfs.ext3 mkfs.ext4; do
    if [ ! -x "$MKFS_DIR/$tool" ]; then
        echo "Error: missing $MKFS_DIR/$tool. Run make mkfs-tools first." >&2
        exit 1
    fi

    for dir in "$MNT/bin" "$MNT/sbin" "$MNT/musl/ltp/testcases/bin"; do
        install_wrapper "$tool" "$dir"
    done
done

cp "$TOOLS_DIR/mke2fs.conf" "$MNT/sbin/mke2fs.conf"
