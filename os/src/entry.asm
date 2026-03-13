    .section .text.entry
    .globl _start
_start:
    la sp, boot_stack_top
    la t0, rust_main
    li t1, 0xffffffc000000000
    #sub t0, t0, t1
    jr t0
    #call rust_main

    .section .bss.stack
    .globl boot_stack_lower_bound
boot_stack_lower_bound:
    .space 4096 * 16
    .globl boot_stack_top
boot_stack_top: