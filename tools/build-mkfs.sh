#!/bin/bash
# 交叉编译 mkfs 工具脚本
# 产物输出到 tools/mkfs-riscv64/ 和 tools/mkfs-loongarch64/

set -e

TOOLS_DIR="$(cd "$(dirname "$0")" && pwd)"
WORK_DIR="$TOOLS_DIR/.build-tmp"
mkdir -p "$WORK_DIR"
cd "$WORK_DIR"

# 下载源码
download_sources() {
    if [ ! -f e2fsprogs-1.47.0.tar.gz ]; then
        wget https://mirrors.edge.kernel.org/pub/linux/kernel/people/tytso/e2fsprogs/v1.47.0/e2fsprogs-1.47.0.tar.gz
    fi
    if [ ! -f dosfstools-4.2.tar.gz ]; then
        wget https://github.com/dosfstools/dosfstools/releases/download/v4.2/dosfstools-4.2.tar.gz
    fi
    tar -xzf e2fsprogs-1.47.0.tar.gz
    tar -xzf dosfstools-4.2.tar.gz
}

build_e2fsprogs() {
    local host=$1
    local cc=$2
    local out=$3
    local build_dir="build_${host%%-*}-e2fs"

    cd "$WORK_DIR/e2fsprogs-1.47.0"
    rm -rf "$build_dir"
    mkdir -p "$build_dir"
    cd "$build_dir"

    ../configure --host="$host" --prefix="$(pwd)/out" \
        --disable-defrag --disable-e2initrd-helper --disable-nls \
        --disable-fsck --disable-libblkid \
        --disable-uuidd --disable-debugfs --disable-e2undo \
        --disable-chattr --disable-lsattr \
        --enable-libuuid --enable-libblkid \
        CC="$cc" CFLAGS="-static -O2" LDFLAGS="-static"

    make -j"$(nproc)"
    make install

    mkdir -p "$out/sbin"
    cp out/sbin/mkfs.ext2 "$out/sbin/"
    cp out/sbin/mkfs.ext3 "$out/sbin/"
    cp out/sbin/mkfs.ext4 "$out/sbin/"
}

build_dosfstools() {
    local host=$1
    local cc=$2
    local out=$3
    local build_dir="build_${host%%-*}-fat"

    cd "$WORK_DIR/dosfstools-4.2"
    rm -rf "$build_dir"
    mkdir -p "$build_dir"
    cd "$build_dir"

    # dosfstools-4.2 自带的 config.sub 不支持 loongarch64，需要替换
    if [ "${host%%-*}" = "loongarch64" ]; then
        cp /usr/share/automake-1.16/config.sub ../config.sub
    fi

    ../configure --host="$host" --prefix="$(pwd)/out" \
        CC="$cc" CFLAGS="-static -O2" LDFLAGS="-static"

    make -j"$(nproc)"
    make install

    mkdir -p "$out/sbin"
    cp out/sbin/mkfs.fat "$out/sbin/"
}

main() {
    download_sources

    build_e2fsprogs riscv64-linux-gnu riscv64-linux-gnu-gcc "$TOOLS_DIR/mkfs-riscv64"
    build_dosfstools riscv64-linux-gnu riscv64-linux-gnu-gcc "$TOOLS_DIR/mkfs-riscv64"

    build_e2fsprogs loongarch64-linux-gnu loongarch64-linux-gnu-gcc "$TOOLS_DIR/mkfs-loongarch64"
    build_dosfstools loongarch64-linux-gnu loongarch64-linux-gnu-gcc "$TOOLS_DIR/mkfs-loongarch64"

    echo "Build finished."
    echo "RISC-V: $TOOLS_DIR/mkfs-riscv64/sbin/"
    echo "LoongArch: $TOOLS_DIR/mkfs-loongarch64/sbin/"
}

main "$@"
