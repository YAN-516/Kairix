#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ALPINE_RELEASE="v3.22"
ALPINE_REPOSITORY="${ALPINE_REPOSITORY:-https://dl-cdn.alpinelinux.org/alpine}"
RUST_VERSION="1.87.0"

ARCH="${1:-}"
case "$ARCH" in
    riscv64)
        TARGET_TRIPLE="riscv64-alpine-linux-musl"
        ELF_MACHINE="RISC-V"
        RUST_STDLIB="libstd-d1cec699c063a651.rlib"
        ;;
    loongarch64)
        TARGET_TRIPLE="loongarch64-alpine-linux-musl"
        ELF_MACHINE="LoongArch"
        RUST_STDLIB="libstd-d0669adf99e3d421.rlib"
        ;;
    *)
        echo "usage: $0 <riscv64|loongarch64>" >&2
        exit 2
        ;;
esac

LOCK_FILE="$SCRIPT_DIR/$ARCH-packages.lock"
CACHE_DIR="$REPO_ROOT/tools/.build-tmp/rustc-toolchain/$ALPINE_RELEASE/$ARCH"
OUTPUT_DIR="$REPO_ROOT/tools/target/rustc-$ARCH"
ROOTFS_DIR="$OUTPUT_DIR/rootfs"
ARCHIVE="$OUTPUT_DIR/rustc-$ARCH-musl.tar.gz"

for command in curl gzip sha256sum tar readelf; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "error: required command '$command' was not found" >&2
        exit 1
    fi
done

mkdir -p "$CACHE_DIR" "$OUTPUT_DIR"

while read -r checksum filename extra; do
    case "$checksum" in
        ''|'#'*) continue ;;
    esac
    if [ -n "${extra:-}" ] || [ -z "${filename:-}" ]; then
        echo "error: malformed entry in $LOCK_FILE" >&2
        exit 1
    fi

    package="$CACHE_DIR/$filename"
    if [ -f "$package" ] && printf '%s  %s\n' "$checksum" "$package" | sha256sum -c - >/dev/null 2>&1; then
        echo "  cached $filename"
        continue
    fi

    echo "  downloading $filename"
    curl -fL --retry 3 --connect-timeout 20 \
        "$ALPINE_REPOSITORY/$ALPINE_RELEASE/main/$ARCH/$filename" \
        -o "$package.part"
    printf '%s  %s\n' "$checksum" "$package.part" | sha256sum -c - >/dev/null
    mv "$package.part" "$package"
done < "$LOCK_FILE"

echo "Assembling incremental $ARCH Rust toolchain root filesystem..."
rm -rf "$ROOTFS_DIR"
mkdir -p "$ROOTFS_DIR"

while read -r checksum filename extra; do
    case "$checksum" in
        ''|'#'*) continue ;;
    esac
    tar \
        --extract \
        --gzip \
        --file "$CACHE_DIR/$filename" \
        --directory "$ROOTFS_DIR" \
        --warning=no-unknown-keyword \
        --exclude='.SIGN.*' \
        --exclude='.PKGINFO' \
        --exclude='.INSTALL' \
        --exclude='.post-install' \
        --exclude='.pre-install' \
        --exclude='.trigger'
done < "$LOCK_FILE"

require_file() {
    local path="$1"
    if [ ! -e "$ROOTFS_DIR/$path" ]; then
        echo "error: assembled Rust toolchain is missing /$path" >&2
        exit 1
    fi
}

require_file usr/bin/rustc
require_file usr/lib/libLLVM.so.20.1
require_file usr/lib/libffi.so.8
require_file usr/lib/libxml2.so.2
require_file usr/lib/liblzma.so.5
require_file usr/lib/libscudo.so
require_file "usr/lib/rustlib/$TARGET_TRIPLE/lib/$RUST_STDLIB"

machine="$(readelf -h "$ROOTFS_DIR/usr/bin/rustc" | awk -F: '/Machine:/ { sub(/^[[:space:]]+/, "", $2); print $2 }')"
if [[ "$machine" != *"$ELF_MACHINE"* ]]; then
    echo "error: /usr/bin/rustc has unexpected ELF machine '$machine'" >&2
    exit 1
fi

cp "$LOCK_FILE" "$OUTPUT_DIR/packages.lock"
printf '%s\n' \
    "Alpine release: $ALPINE_RELEASE" \
    "Architecture: $ARCH" \
    "Target: $TARGET_TRIPLE" \
    'Rust: 1.87.0-r1' \
    'LLVM: 20.1.8-r0' \
    > "$OUTPUT_DIR/toolchain-info.txt"

echo "Creating $ARCH Rust archive for kernel embedding..."
tar \
    --create \
    --directory "$ROOTFS_DIR" \
    --sort=name \
    --mtime='UTC 2025-01-01' \
    --owner=0 \
    --group=0 \
    --numeric-owner \
    --file=- \
    . | gzip -n -9 > "$ARCHIVE"

echo "Native $ARCH Rust toolchain is ready:"
echo "  rootfs:  $ROOTFS_DIR ($(du -sh "$ROOTFS_DIR" | cut -f1))"
echo "  embed:   $ARCHIVE ($(du -h "$ARCHIVE" | cut -f1))"
echo "  target:  $TARGET_TRIPLE"
echo "  version: rustc $RUST_VERSION, LLVM 20.1.8"
