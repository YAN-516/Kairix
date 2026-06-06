#!/usr/bin/env bash
# Patch dynamic linker/library paths in a mounted Kairix SD card image.

set -euo pipefail

if [ "$#" -ne 2 ]; then
    echo "Usage: $0 <arch> <mount-dir>" >&2
    exit 1
fi

ARCH="$1"
MNT="$2"

setup_riscv64() {
    echo "Step 4: Setting up /lib for RISC-V dynamic linking..."
    mkdir -p "$MNT/lib" "$MNT/lib/riscv64-linux-gnu"

    if [ -f "$MNT/glibc/lib/ld-linux-riscv64-lp64d.so.1" ]; then
        cp "$MNT/glibc/lib/ld-linux-riscv64-lp64d.so.1" "$MNT/lib/ld-linux-riscv64-lp64d.so.1"
        echo "  Copied ld-linux-riscv64-lp64d.so.1 to /lib/"
    fi

    for lib in libc.so.6 libm.so.6 libc.so libm.so; do
        if [ -f "$MNT/glibc/lib/$lib" ]; then
            cp "$MNT/glibc/lib/$lib" "$MNT/lib/$lib"
            cp "$MNT/glibc/lib/$lib" "$MNT/lib/riscv64-linux-gnu/$lib"
            echo "  Copied $lib to /lib/ and /lib/riscv64-linux-gnu/"
        fi
    done

    local libgcc_src=""
    for src in "$MNT/glibc/lib/libgcc_s.so.1" /usr/riscv64-linux-gnu/lib/libgcc_s.so.1; do
        if [ -f "$src" ]; then
            libgcc_src="$src"
            break
        fi
    done

    if [ -n "$libgcc_src" ]; then
        cp "$libgcc_src" "$MNT/lib/libgcc_s.so.1"
        cp "$libgcc_src" "$MNT/lib/riscv64-linux-gnu/libgcc_s.so.1"
        echo "  Copied libgcc_s.so.1 to /lib/ and /lib/riscv64-linux-gnu/"
    else
        echo "  Warning: libgcc_s.so.1 not found for RISC-V glibc"
    fi

    if [ -f "$MNT/musl/lib/ld-musl-riscv64-sf.so.1" ]; then
        cp "$MNT/musl/lib/ld-musl-riscv64-sf.so.1" "$MNT/lib/ld-musl-riscv64-sf.so.1"
        echo "  Copied musl loader ld-musl-riscv64-sf.so.1 to /lib/"
    elif [ -f "$MNT/musl/lib/libc.so" ]; then
        cp "$MNT/musl/lib/libc.so" "$MNT/lib/ld-musl-riscv64-sf.so.1"
        echo "  Created /lib/ld-musl-riscv64-sf.so.1 from musl libc.so"
    else
        echo "  Warning: musl loader not found under /musl/lib, dynamic musl binaries may fail"
    fi

    echo "Step 4b: Setting up double-float musl loader for LTP..."
    if [ -f /opt/riscv64-linux-musl-cross/riscv64-linux-musl/lib/libc.so ]; then
        cp /opt/riscv64-linux-musl-cross/riscv64-linux-musl/lib/libc.so "$MNT/lib/ld-musl-riscv64.so.1"
        echo "  Copied host double-float musl libc.so to /lib/ld-musl-riscv64.so.1"
    else
        echo "  Warning: host double-float musl libc.so not found"
    fi
}

setup_loongarch64() {
    echo "Step 4: Setting up /lib64 and /usr/lib64 for LoongArch64 glibc..."
    mkdir -p "$MNT/lib64" "$MNT/usr/lib64"

    for lib in ld-linux-loongarch-lp64d.so.1 libc.so.6 libm.so.6 libdl.so.2 libpthread.so.0; do
        if [ -f "$MNT/glibc/lib/$lib" ]; then
            cp "$MNT/glibc/lib/$lib" "$MNT/lib64/"
            cp "$MNT/glibc/lib/$lib" "$MNT/usr/lib64/"
            echo "  Copied $lib to /lib64/ and /usr/lib64/"
        fi
    done

    local libgcc_src=""
    for src in \
        "$MNT/glibc/lib/libgcc_s.so.1" \
        /opt/gcc-13.2.0-loongarch64-linux-gnu/loongarch64-linux-gnu/lib64/libgcc_s.so.1 \
        /opt/toolchain-loongarch64-linux-gnu-gcc8-host-x86_64-2022-07-18/sysroot/usr/lib64/libgcc_s.so.1; do
        if [ -f "$src" ]; then
            libgcc_src="$src"
            break
        fi
    done

    if [ -n "$libgcc_src" ]; then
        cp "$libgcc_src" "$MNT/lib64/libgcc_s.so.1"
        cp "$libgcc_src" "$MNT/usr/lib64/libgcc_s.so.1"
        echo "  Copied libgcc_s.so.1 to /lib64/ and /usr/lib64/"
    else
        echo "  Warning: libgcc_s.so.1 not found for LoongArch64 glibc"
    fi

    echo "Step 5: Setting up /lib for LoongArch64 musl..."
    mkdir -p "$MNT/lib"

    if [ -f "$MNT/musl/lib/ld-musl-loongarch-lp64d.so.1" ]; then
        cp "$MNT/musl/lib/ld-musl-loongarch-lp64d.so.1" "$MNT/lib/"
        echo "  Copied musl loader to /lib/"
    elif [ -f "$MNT/musl/lib/libc.so" ]; then
        cp "$MNT/musl/lib/libc.so" "$MNT/lib/ld-musl-loongarch-lp64d.so.1"
        echo "  Created /lib/ld-musl-loongarch-lp64d.so.1 from musl libc.so"
    fi

    if [ -f "$MNT/lib/ld-musl-loongarch-lp64d.so.1" ]; then
        cp "$MNT/lib/ld-musl-loongarch-lp64d.so.1" "$MNT/lib64/ld-musl-loongarch-lp64d.so.1"
        cp "$MNT/lib/ld-musl-loongarch-lp64d.so.1" "$MNT/usr/lib64/ld-musl-loongarch-lp64d.so.1"
        echo "  Copied musl loader to /lib64/ and /usr/lib64/ for PT_INTERP compatibility"
    fi
}

case "$ARCH" in
    riscv64) setup_riscv64 ;;
    loongarch64) setup_loongarch64 ;;
    *)
        echo "Error: unsupported ARCH '$ARCH'." >&2
        exit 1
        ;;
esac
