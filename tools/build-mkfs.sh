#!/usr/bin/env bash
# Cross-compile e2fsprogs mkfs.ext tools from a vendored source tarball.
# Outputs are build artifacts under tools/target/mkfs-<arch>/sbin/.

set -euo pipefail

TOOLS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC_DIR="$TOOLS_DIR/src"
TARGET_DIR="$TOOLS_DIR/target"
WORK_DIR="$TARGET_DIR/mkfs-build"
E2FSPROGS_VERSION="1.47.0"
E2FSPROGS_TARBALL="e2fsprogs-${E2FSPROGS_VERSION}.tar.gz"
E2FSPROGS_TARBALL_PATH="$SRC_DIR/$E2FSPROGS_TARBALL"
E2FSPROGS_SRC="$WORK_DIR/e2fsprogs-${E2FSPROGS_VERSION}"
MKFS_EXT_TOOLS=(mkfs.ext2 mkfs.ext3 mkfs.ext4)

usage() {
    cat <<EOF
Usage: $0 [all|riscv64|loongarch64|clean]...

Builds mkfs.ext2, mkfs.ext3 and mkfs.ext4 from:
  $E2FSPROGS_TARBALL_PATH
EOF
}

prepare_sources() {
    if [ ! -f "$E2FSPROGS_TARBALL_PATH" ]; then
        echo "Error: missing source tarball: $E2FSPROGS_TARBALL_PATH" >&2
        echo "Put e2fsprogs-${E2FSPROGS_VERSION}.tar.gz under tools/src before running make all." >&2
        exit 1
    fi

    mkdir -p "$WORK_DIR"

    if [ ! -d "$E2FSPROGS_SRC" ]; then
        tar -xzf "$E2FSPROGS_TARBALL_PATH" -C "$WORK_DIR"
    fi
}

arch_host() {
    case "$1" in
        riscv64) echo "riscv64-linux-gnu" ;;
        loongarch64) echo "loongarch64-linux-gnu" ;;
        *)
            echo "Error: unsupported ARCH '$1'." >&2
            exit 1
            ;;
    esac
}

arch_cc() {
    case "$1" in
        riscv64) echo "riscv64-linux-gnu-gcc" ;;
        loongarch64) echo "loongarch64-linux-gnu-gcc" ;;
        *)
            echo "Error: unsupported ARCH '$1'." >&2
            exit 1
            ;;
    esac
}

tools_ready() {
    local out=$1
    local tool

    for tool in "${MKFS_EXT_TOOLS[@]}"; do
        if [ ! -x "$out/sbin/$tool" ]; then
            return 1
        fi
    done
}

build_e2fsprogs() {
    local arch=$1
    local host
    local cc
    local out
    local build_dir
    local prefix
    local tool

    host="$(arch_host "$arch")"
    cc="$(arch_cc "$arch")"
    out="$TARGET_DIR/mkfs-$arch"

    if tools_ready "$out"; then
        echo "mkfs.ext tools for $arch already exist: $out/sbin"
        return
    fi

    if ! command -v "$cc" >/dev/null 2>&1; then
        echo "Error: missing cross compiler '$cc'." >&2
        exit 1
    fi

    prepare_sources

    build_dir="$WORK_DIR/build-$arch-e2fsprogs"
    prefix="$build_dir/out"
    rm -rf "$build_dir"
    mkdir -p "$build_dir"

    echo "Building e2fsprogs mkfs.ext tools for $arch..."
    (
        cd "$build_dir"
        "$E2FSPROGS_SRC/configure" --host="$host" --prefix="$prefix" \
            --disable-defrag --disable-e2initrd-helper --disable-nls \
            --disable-fsck --disable-libblkid \
            --disable-uuidd --disable-debugfs --disable-e2undo \
            --disable-chattr --disable-lsattr \
            --enable-libuuid --enable-libblkid \
            CC="$cc" CFLAGS="-static -O2" LDFLAGS="-static"
        make -j"${JOBS:-$(nproc)}" libs
        make -C misc mke2fs
    )

    mkdir -p "$out/sbin"
    for tool in "${MKFS_EXT_TOOLS[@]}"; do
        cp "$build_dir/misc/mke2fs" "$out/sbin/$tool"
    done

    if command -v "${host}-strip" >/dev/null 2>&1; then
        "${host}-strip" "$out"/sbin/mkfs.ext* || true
    fi

    echo "Built mkfs.ext tools for $arch: $out/sbin"
}

clean() {
    rm -rf \
        "$WORK_DIR" \
        "$TARGET_DIR/mkfs-riscv64" \
        "$TARGET_DIR/mkfs-loongarch64"
}

main() {
    if [ "$#" -eq 0 ]; then
        set -- all
    fi

    for target in "$@"; do
        case "$target" in
            all)
                build_e2fsprogs riscv64
                build_e2fsprogs loongarch64
                ;;
            riscv64 | loongarch64)
                build_e2fsprogs "$target"
                ;;
            clean)
                clean
                ;;
            -h | --help | help)
                usage
                ;;
            *)
                usage >&2
                exit 1
                ;;
        esac
    done
}

main "$@"
