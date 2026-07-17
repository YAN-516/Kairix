macro_rules! includes_trap_macros {
    () => {
        r#"
        .ifndef REGS_TRAP_MACROS_FLAG
        .equ REGS_TRAP_MACROS_FLAG, 1

        .macro LDR  reg, offset
            ld  \reg, \offset*8(sp)
        .endm

        .macro STR  reg, offset
            sd  \reg, \offset*8(sp)
        .endm

        .macro LOAD reg, offset
            ld  \reg, \offset*8(sp)
        .endm

        .macro SAVE reg, offset
            sd  \reg, \offset*8(sp)
        .endm

        .macro LOAD_N n
            ld  x\n, \n*8(sp)
        .endm

        .macro SAVE_N n
            sd  x\n, \n*8(sp)
        .endm

        .macro SAVE_GENERAL_REGS
            SAVE    x1, 1
            csrr    x1, sscratch
            SAVE    x1, 2
            .set    n, 3
            .rept   29 
                SAVE_N  %n
            .set    n, n + 1
            .endr

            csrr    t0, sstatus
            csrr    t1, sepc
            SAVE    t0, 32
            SAVE    t1, 33
        .endm

        .macro LOAD_GENERAL_REGS
            LOAD    t0, 32
            LOAD    t1, 33
            csrw    sstatus, t0
            csrw    sepc, t1

            LOAD    x1, 1
            .set    n, 3
            .rept   29
                LOAD_N  %n
            .set    n, n + 1
            .endr
            LOAD    x2, 2
        .endm

        // TrapFrame layout after x[32], sstatus and sepc:
        // f[0..32] starts at slot 34 and fcsr is slot 66.
        .macro SAVE_FP_REGS
            .option push
            .option arch, +f, +d
            fsd f0,  34*8(sp)
            fsd f1,  35*8(sp)
            fsd f2,  36*8(sp)
            fsd f3,  37*8(sp)
            fsd f4,  38*8(sp)
            fsd f5,  39*8(sp)
            fsd f6,  40*8(sp)
            fsd f7,  41*8(sp)
            fsd f8,  42*8(sp)
            fsd f9,  43*8(sp)
            fsd f10, 44*8(sp)
            fsd f11, 45*8(sp)
            fsd f12, 46*8(sp)
            fsd f13, 47*8(sp)
            fsd f14, 48*8(sp)
            fsd f15, 49*8(sp)
            fsd f16, 50*8(sp)
            fsd f17, 51*8(sp)
            fsd f18, 52*8(sp)
            fsd f19, 53*8(sp)
            fsd f20, 54*8(sp)
            fsd f21, 55*8(sp)
            fsd f22, 56*8(sp)
            fsd f23, 57*8(sp)
            fsd f24, 58*8(sp)
            fsd f25, 59*8(sp)
            fsd f26, 60*8(sp)
            fsd f27, 61*8(sp)
            fsd f28, 62*8(sp)
            fsd f29, 63*8(sp)
            fsd f30, 64*8(sp)
            fsd f31, 65*8(sp)
            csrr t0, 0x003
            sd t0, 66*8(sp)
            .option pop
        .endm

        .macro LOAD_FP_REGS
            .option push
            .option arch, +f, +d
            ld t0, 66*8(sp)
            csrw 0x003, t0
            fld f0,  34*8(sp)
            fld f1,  35*8(sp)
            fld f2,  36*8(sp)
            fld f3,  37*8(sp)
            fld f4,  38*8(sp)
            fld f5,  39*8(sp)
            fld f6,  40*8(sp)
            fld f7,  41*8(sp)
            fld f8,  42*8(sp)
            fld f9,  43*8(sp)
            fld f10, 44*8(sp)
            fld f11, 45*8(sp)
            fld f12, 46*8(sp)
            fld f13, 47*8(sp)
            fld f14, 48*8(sp)
            fld f15, 49*8(sp)
            fld f16, 50*8(sp)
            fld f17, 51*8(sp)
            fld f18, 52*8(sp)
            fld f19, 53*8(sp)
            fld f20, 54*8(sp)
            fld f21, 55*8(sp)
            fld f22, 56*8(sp)
            fld f23, 57*8(sp)
            fld f24, 58*8(sp)
            fld f25, 59*8(sp)
            fld f26, 60*8(sp)
            fld f27, 61*8(sp)
            fld f28, 62*8(sp)
            fld f29, 63*8(sp)
            fld f30, 64*8(sp)
            fld f31, 65*8(sp)
            .option pop
        .endm

        .macro LOAD_PERCPU dst, sym
            lui  \dst, %hi(__PERCPU_\sym)
            add  \dst, \dst, gp
            ld   \dst, %lo(__PERCPU_\sym)(\dst)
        .endm

        .macro SAVE_PERCPU sym, temp, src
            lui  \temp, %hi(__PERCPU_\sym)
            add  \temp, \temp, gp
            sd   \src,  %lo(__PERCPU_\sym)(\temp)
        .endm

        .endif
        "#
    };
}
