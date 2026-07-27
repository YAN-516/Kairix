#!/usr/bin/env bash

set -euo pipefail

if [ "$#" -ne 2 ]; then
    echo "Usage: $0 <gcc-rootfs-dir> <target-ext4-image>" >&2
    exit 2
fi

gcc_rootfs="$(realpath "$1")"
target_image="$(realpath "$2")"

if [ ! -x "$gcc_rootfs/usr/bin/gcc" ] || \
   [ ! -x "$gcc_rootfs/usr/libexec/gcc/loongarch64-alpine-linux-musl/14.2.0/cc1" ]; then
    echo "Error: incomplete LoongArch GCC rootfs: $gcc_rootfs" >&2
    exit 1
fi

mnt="$(mktemp -d "${TMPDIR:-/tmp}/kairix-board-gcc.XXXXXX")"
mounted=0
cleanup() {
    status=$?
    if [ "$mounted" = "1" ] && mountpoint -q "$mnt"; then
        if umount "$mnt"; then
            mounted=0
        else
            status=$?
        fi
    fi
    rmdir "$mnt" 2>/dev/null || true
    exit "$status"
}
trap cleanup EXIT INT TERM

e2fsck -f -y "$target_image" || true
mount "$target_image" "$mnt"
mounted=1

available_kib="$(df -Pk "$mnt" | awk 'NR == 2 { print $4 }')"
if [ "$available_kib" -lt 70000 ]; then
    echo "Error: board rootfs has only ${available_kib} KiB free; minimal GCC needs at least 70000 KiB." >&2
    exit 1
fi

echo "Installing minimal LoongArch native GCC into $target_image..."
tar \
    --create \
    --directory "$gcc_rootfs" \
    --exclude='./usr/bin/lto-dump' \
    --exclude='./usr/bin/gcov' \
    --exclude='./usr/bin/gcov-dump' \
    --exclude='./usr/bin/gcov-tool' \
    --exclude='./usr/lib/libc.a' \
    --exclude='./usr/lib/libgomp.a' \
    --exclude='./usr/lib/gcc/loongarch64-alpine-linux-musl/14.2.0/plugin/include' \
    --exclude='./usr/libexec/gcc/loongarch64-alpine-linux-musl/14.2.0/lto1' \
    --exclude='./usr/libexec/gcc/loongarch64-alpine-linux-musl/14.2.0/lto-wrapper' \
    --exclude='./usr/libexec/gcc/loongarch64-alpine-linux-musl/14.2.0/plugin' \
    --file=- \
    . | tar --extract --directory "$mnt" --file=-

for required in \
    usr/bin/gcc \
    usr/bin/as \
    usr/bin/ld \
    usr/include/stdio.h \
    usr/lib/crt1.o \
    usr/libexec/gcc/loongarch64-alpine-linux-musl/14.2.0/cc1 \
    usr/lib/gcc/loongarch64-alpine-linux-musl/14.2.0/libgcc.a \
    lib/ld-musl-loongarch64.so.1; do
    if [ ! -e "$mnt/$required" ]; then
        echo "Error: GCC installation is missing /$required" >&2
        exit 1
    fi
done

sync
used="$(du -sh "$mnt" | cut -f1)"
free="$(df -h "$mnt" | awk 'NR == 2 { print $4 }')"
echo "Board GCC rootfs ready: used=$used free=$free"

umount "$mnt"
mounted=0
rmdir "$mnt"
trap - EXIT INT TERM
