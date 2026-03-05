# KaiRix
KaiRix kernel version 0.1.0

## Dependency

### Binaries

* rustc: 1.86.0-nightly (088d49608 2025-01-10)
  (default toolchain: nightly-2025-01-18)

* cargo-binutils: (not installed)
  run `cargo install cargo-binutils` to install

* qemu: 9.2.1
  (supports riscv64 and loongarch64)

* rustsbi-lib: (to be determined)
  (to be added after project setup)

  rustsbi-qemu: (to be determined)
  (to be added after project setup)

  rustsbi-k210: (to be determined)
  (to be added after project setup)

### Rust Targets

* riscv64gc-unknown-none-elf (installed)
* loongarch64-unknown-none (installed)
* riscv64imac-unknown-none-elf (installed)
* loongarch64-unknown-linux-gnu (installed)
* x86_64-unknown-linux-gnu (installed)

### Optional GCC Toolchain

* riscv64-unknown-elf-gcc: 8.2.0
* riscv64-linux-gnu-gcc: 11.4.0
* loongarch64-linux-gnu-gcc: 13.2.0