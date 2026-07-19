# Embedded Rust compiler toolchain

These scripts assemble Alpine v3.22's native musl `rustc` and LLVM runtime for
RISC-V 64 or LoongArch64. The archive contains only packages not already in the
embedded GCC toolchain.

Build one payload from the repository root:

```sh
bash tools/rustc-toolchain/build-riscv64.sh
bash tools/rustc-toolchain/build-loongarch64.sh
```

Package versions and SHA-256 checksums are pinned in the architecture-specific
lock files. The resulting archive is written below `tools/target/rustc-ARCH/`.
