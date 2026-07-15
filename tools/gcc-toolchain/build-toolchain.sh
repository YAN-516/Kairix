#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ALPINE_RELEASE="v3.22"
ALPINE_REPOSITORY="${ALPINE_REPOSITORY:-https://dl-cdn.alpinelinux.org/alpine}"
GCC_VERSION="14.2.0"

ARCH="${1:-}"
case "$ARCH" in
    riscv64)
        TARGET_TRIPLE="riscv64-alpine-linux-musl"
        ELF_MACHINE="RISC-V"
        MUSL_LOADER="ld-musl-riscv64.so.1"
        ;;
    loongarch64)
        TARGET_TRIPLE="loongarch64-alpine-linux-musl"
        ELF_MACHINE="LoongArch"
        MUSL_LOADER="ld-musl-loongarch64.so.1"
        ;;
    *)
        echo "usage: $0 <riscv64|loongarch64>" >&2
        exit 2
        ;;
esac

LOCK_FILE="$SCRIPT_DIR/$ARCH-packages.lock"
INSTALLER_LOCK_FILE="$SCRIPT_DIR/$ARCH-installer.lock"
CACHE_DIR="$REPO_ROOT/tools/.build-tmp/gcc-toolchain/$ALPINE_RELEASE/$ARCH"
OUTPUT_DIR="$REPO_ROOT/tools/target/gcc-$ARCH"
ROOTFS_DIR="$OUTPUT_DIR/rootfs"
GZIP_ARCHIVE="$OUTPUT_DIR/gcc-$ARCH-musl.tar.gz"
INSTALLER_DIR="$OUTPUT_DIR/installer"
INSTALLER_BUSYBOX="$OUTPUT_DIR/gcc-toolchain-busybox"

for command in curl gzip sha256sum strings tar readelf; do
    if ! command -v "$command" >/dev/null 2>&1; then
        echo "error: required command '$command' was not found" >&2
        exit 1
    fi
done

mkdir -p "$CACHE_DIR" "$OUTPUT_DIR"

download_package() {
    local checksum="$1"
    local filename="$2"
    local destination="$CACHE_DIR/$filename"
    local url="$ALPINE_REPOSITORY/$ALPINE_RELEASE/main/$ARCH/$filename"

    if [ -f "$destination" ] && printf '%s  %s\n' "$checksum" "$destination" | sha256sum -c - >/dev/null 2>&1; then
        echo "  cached $filename"
        return
    fi

    echo "  downloading $filename"
    curl -fL --retry 3 --connect-timeout 20 "$url" -o "$destination.part"
    printf '%s  %s\n' "$checksum" "$destination.part" | sha256sum -c - >/dev/null
    mv "$destination.part" "$destination"
}

fetch_lock() {
    local lock_file="$1"
    while read -r checksum filename extra; do
        case "$checksum" in
            ''|'#'*) continue ;;
        esac
        if [ -n "${extra:-}" ] || [ -z "${filename:-}" ]; then
            echo "error: malformed entry in $lock_file" >&2
            exit 1
        fi
        download_package "$checksum" "$filename"
    done < "$lock_file"
}

echo "Fetching pinned Alpine $ALPINE_RELEASE $ARCH packages..."
fetch_lock "$LOCK_FILE"
fetch_lock "$INSTALLER_LOCK_FILE"

echo "Assembling $ARCH target root filesystem..."
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

echo "Preparing the embedded $ARCH archive installer..."
rm -rf "$INSTALLER_DIR"
mkdir -p "$INSTALLER_DIR"
while read -r checksum filename extra; do
    case "$checksum" in
        ''|'#'*) continue ;;
    esac
    tar \
        --extract \
        --gzip \
        --file "$CACHE_DIR/$filename" \
        --directory "$INSTALLER_DIR" \
        --warning=no-unknown-keyword \
        bin/busybox.static
done < "$INSTALLER_LOCK_FILE"
cp "$INSTALLER_DIR/bin/busybox.static" "$INSTALLER_BUSYBOX"
chmod 0755 "$INSTALLER_BUSYBOX"

if ! strings "$INSTALLER_BUSYBOX" | grep 'tar (busybox)' >/dev/null; then
    echo "error: embedded BusyBox does not provide the tar applet" >&2
    exit 1
fi
if ! readelf -h "$INSTALLER_BUSYBOX" | grep "Machine:.*$ELF_MACHINE" >/dev/null \
    || readelf -l "$INSTALLER_BUSYBOX" | grep 'INTERP' >/dev/null; then
    echo "error: embedded BusyBox is not a static $ARCH executable" >&2
    exit 1
fi
rm -rf "$INSTALLER_DIR"

require_file() {
    local path="$1"
    if [ ! -e "$ROOTFS_DIR/$path" ]; then
        echo "error: assembled toolchain is missing /$path" >&2
        exit 1
    fi
}

require_file usr/bin/gcc
require_file usr/bin/as
require_file usr/bin/ld
require_file usr/include/stdio.h
require_file usr/lib/crt1.o
require_file usr/lib/libc.a
require_file "usr/libexec/gcc/$TARGET_TRIPLE/$GCC_VERSION/cc1"
require_file "usr/lib/gcc/$TARGET_TRIPLE/$GCC_VERSION/libgcc.a"
require_file "lib/$MUSL_LOADER"

machine="$(readelf -h "$ROOTFS_DIR/usr/bin/gcc" | awk -F: '/Machine:/ { sub(/^[[:space:]]+/, "", $2); print $2 }')"
if [[ "$machine" != *"$ELF_MACHINE"* ]]; then
    echo "error: /usr/bin/gcc has unexpected ELF machine '$machine'" >&2
    exit 1
fi

interpreter="$(readelf -l "$ROOTFS_DIR/usr/bin/gcc" | sed -n 's/.*Requesting program interpreter: \(.*\)]/\1/p')"
if [ "$interpreter" != "/lib/$MUSL_LOADER" ]; then
    echo "error: /usr/bin/gcc has unexpected interpreter '$interpreter'" >&2
    exit 1
fi

cp "$LOCK_FILE" "$OUTPUT_DIR/packages.lock"
cp "$INSTALLER_LOCK_FILE" "$OUTPUT_DIR/installer.lock"
printf '%s\n' \
    "Alpine release: $ALPINE_RELEASE" \
    "Architecture: $ARCH" \
    "Target: $TARGET_TRIPLE" \
    'GCC: 14.2.0-r6' \
    'binutils: 2.44-r3' \
    'musl: 1.2.5-r12' \
    > "$OUTPUT_DIR/toolchain-info.txt"

echo "Creating $GZIP_ARCHIVE for kernel embedding..."
tar \
    --create \
    --directory "$ROOTFS_DIR" \
    --sort=name \
    --mtime='UTC 2025-01-01' \
    --owner=0 \
    --group=0 \
    --numeric-owner \
    --file=- \
    . | gzip -n -9 > "$GZIP_ARCHIVE"

echo "Native $ARCH GCC toolchain is ready:"
echo "  rootfs:   $ROOTFS_DIR ($(du -sh "$ROOTFS_DIR" | cut -f1))"
echo "  embed:    $GZIP_ARCHIVE ($(du -h "$GZIP_ARCHIVE" | cut -f1))"
echo "  unpacker: $INSTALLER_BUSYBOX ($(du -h "$INSTALLER_BUSYBOX" | cut -f1))"
echo "  target:   $TARGET_TRIPLE"
echo "  version:  GCC 14.2.0, binutils 2.44, musl 1.2.5"
