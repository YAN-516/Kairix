# Native GCC toolchains

This directory assembles real RISC-V64 and LoongArch64 GCC toolchains that run
inside Kairix. It uses pinned Alpine Linux musl packages rather than the
x86_64-hosted cross compilers under `/opt`. Each payload contains target-native
`gcc`, `cc1`, binutils, headers, startup objects, libc, and compiler runtimes.

## Build

Normal kernel builds prepare the matching payload automatically:

```sh
make -C os ARCH=riscv64 build
make -C os ARCH=loongarch64 build
```

To prepare only one payload, run one of:

```sh
bash tools/gcc-toolchain/build-riscv64.sh
bash tools/gcc-toolchain/build-loongarch64.sh
```

`build-toolchain.sh` contains the shared implementation. Architecture-specific
package and installer lock files pin every Alpine package by SHA-256. Downloads
are cached under `tools/.build-tmp/`; generated files are placed under the
ignored `tools/target/gcc-<arch>/` directory:

```text
tools/target/gcc-<arch>/
|-- rootfs/                       # Files installed at filesystem root
|-- gcc-<arch>-musl.tar.gz        # Archive embedded in the kernel
|-- gcc-toolchain-busybox         # Static target-native tar/gzip unpacker
|-- packages.lock
`-- toolchain-info.txt
```

Important architecture-specific paths are:

```text
RISC-V cc1:  /usr/libexec/gcc/riscv64-alpine-linux-musl/14.2.0/cc1
RISC-V ldso: /lib/ld-musl-riscv64.so.1
LA cc1:      /usr/libexec/gcc/loongarch64-alpine-linux-musl/14.2.0/cc1
LA ldso:     /lib/ld-musl-loongarch64.so.1
```

## Embed flow

Each kernel embeds its gzip archive through an architecture-specific assembly
`.incbin` section. It also embeds the static BusyBox unpacker, matching Alpine
musl loader, and `/bin/gcc` wrapper. This keeps the large archive out of the
Rust compiler's memory while producing a self-contained kernel ELF.

On first boot, `initproc` extracts the archive at `/`, verifies `gcc`, `cc1`,
`as`, `ld`, and `stdio.h`, creates `/.gcc-toolchain-14.2.0-installed`, and
removes the temporary payload. Later boots skip extraction. The matching musl
loader is restored before `initproc` on every boot.

The wrapper implements the assignment-specific `gcc -h` and `gcc --h` options
and forwards all compilation commands to `/usr/bin/gcc`.

## Kernel images

After building, copy the self-contained kernel ELFs when needed:

```sh
cp os/target/riscv64gc-unknown-none-elf/release/os kernel-rv
cp os/target/loongarch64-unknown-none/release/os kernel-la
```
