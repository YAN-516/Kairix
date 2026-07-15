    .section .rodata.gcc_toolchain,"a",@progbits
    .balign 16
    .global gcc_toolchain_archive_start
gcc_toolchain_archive_start:
    .incbin "../tools/target/gcc-riscv64/gcc-riscv64-musl.tar.gz"
    .global gcc_toolchain_archive_end
gcc_toolchain_archive_end:
    .balign 16
