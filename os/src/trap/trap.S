// trap.S
.altmacro

// 宏：保存通用寄存器 x<n> 到当前 sp 偏移 n*8 处
.macro SAVE_GP n
    sd x\n, \n*8(sp)
.endm

// 宏：从当前 sp 偏移 n*8 处恢复通用寄存器 x<n>
.macro LOAD_GP n
    ld x\n, \n*8(sp)
.endm

// 宏：通过 SBI 向控制台输出一个字符（调试用）
.macro SBI_PUTCHAR ch
    li a0, \ch
    li a7, 0x01        // SBI console putchar 调用号
    ecall
.endm

.section .text
.globl __alltraps
.globl __restore
.align 2

// ===================== 统一 Trap 入口 =====================
// 通过 sstatus.SPP 位判断 trap 来源：
//   SPP=0 → 来自用户态 → 保存上下文到用户栈上的 TrapContext
//   SPP=1 → 来自内核态 → 保存上下文到当前内核栈（嵌套安全）
__alltraps:
    // 读取 sstatus.SPP 位（bit 8）
    csrr t0, sstatus
    andi t0, t0, 1 << 8
    bnez t0, trap_from_kernel_entry   // SPP=1，来自内核态

    // ========== 来自用户态的 trap ==========
    // 交换 sp 和 sscratch，此时 sp 指向用户栈上的 TrapContext 位置
    csrrw sp, sscratch, sp

    // 保存通用寄存器到 TrapContext（跳过 sp 和 tp，稍后处理）
    sd x1, 1*8(sp)
    // x2(sp) 稍后保存
    sd x3, 3*8(sp)
    // x4(tp) 未使用，跳过

    // 保存 x5 ~ x31
    .set n, 5
    .rept 27
        SAVE_GP %n
        .set n, n+1
    .endr

    // 保存 sstatus 和 sepc
    csrr t0, sstatus
    csrr t1, sepc
    sd t0, 32*8(sp)
    sd t1, 33*8(sp)

    // 保存用户栈指针（原 sp 在 sscratch 中）
    csrr t2, sscratch
    sd t2, 2*8(sp)

    // 从 TrapContext 中读取内核栈指针（kernel_sp）并切换到内核栈
    ld sp, 34*8(sp)

    // 调用 Rust 实现的用户态 trap 处理函数
    call trap_handler

    // trap_handler 返回后跳转到 __restore 恢复用户态上下文
    j __restore

// ========== 来自内核态的 trap ==========
trap_from_kernel_entry:
    // 注意：当前 sp 指向内核栈，直接在其上分配保存区域
    // 这样即使发生嵌套 trap，每次 trap 都会在内核栈上分配独立的帧，
    // 不会破坏外层 trap 保存的上下文（栈向下增长，内层在更低的地址）。
    addi sp, sp, -36*8      // 分配 36 个槽位（32 个通用 + 4 个系统寄存器）

    // 保存所有通用寄存器（x1 ~ x31）
    sd x1, 1*8(sp)
    sd x2, 2*8(sp)
    sd x3, 3*8(sp)
    sd x4, 4*8(sp)
    .set n, 5
    .rept 27
        SAVE_GP %n
        .set n, n+1
    .endr

    // 保存系统寄存器
    csrr t0, sstatus
    csrr t1, sepc
    csrr t2, scause
    csrr t3, stval
    sd t0, 32*8(sp)   // sstatus
    sd t1, 33*8(sp)   // sepc
    sd t2, 34*8(sp)   // scause
    sd t3, 35*8(sp)   // stval

    // 调用 Rust 实现的内核态 trap 处理函数
    call trap_from_kernel

    // 恢复系统寄存器（scause 和 stval 为只读，恢复可省略，但保留以保持对称）
    ld t0, 32*8(sp)
    ld t1, 33*8(sp)
    csrw sstatus, t0
    csrw sepc, t1
    // scause 和 stval 不需要恢复，它们会在下一次 trap 时被覆盖

    // 恢复通用寄存器
    ld x1, 1*8(sp)
    ld x2, 2*8(sp)
    ld x3, 3*8(sp)
    ld x4, 4*8(sp)
    .set n, 5
    .rept 27
        LOAD_GP %n
        .set n, n+1
    .endr

    // 释放分配的空间，恢复 sp 到进入 trap 前的值
    addi sp, sp, 36*8

    // 返回到内核态发生 trap 的位置
    sret

// ===================== 恢复用户态上下文 =====================
// 参数：a0 = TrapContext 在用户空间的虚拟地址
// 该函数将 a0 写入 sscratch，然后从 TrapContext 恢复所有寄存器并 sret
__restore:
    // 设置 sscratch 为 TrapContext 地址（下次用户态 trap 时自动交换）
    csrw sscratch, a0
    mv sp, a0                // sp 指向用户栈上的 TrapContext

    // 恢复 sstatus 和 sepc
    ld t0, 32*8(sp)
    ld t1, 33*8(sp)
    csrw sstatus, t0
    csrw sepc, t1

    // 恢复通用寄存器（x1, x3, x5 ~ x31）
    ld x1, 1*8(sp)
    ld x3, 3*8(sp)
    .set n, 5
    .rept 27
        LOAD_GP %n
        .set n, n+1
    .endr

    // 恢复用户栈指针（x2）
    ld sp, 2*8(sp)

    // 返回到用户态
    sret