    .section .rodata.rustc_toolchain,"a",@progbits
    .balign 16
    .global rustc_toolchain_archive_start
rustc_toolchain_archive_start:
    .incbin "../tools/target/rustc-loongarch64/rustc-loongarch64-musl.tar.gz"
    .global rustc_toolchain_archive_end
rustc_toolchain_archive_end:
    .balign 16
