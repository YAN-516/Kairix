macro_rules! includes_trap_macros {
    () => {
        r#"
        .ifndef REGS_TRAP_MACROS_FLAG
        .equ REGS_TRAP_MACROS_FLAG, 1

        // 2, 4, 1
        .macro FIXUP_EX from, to, fix
        .if \fix
            .section .fixup, "ax"
        \to: 
            li.w	$a0, -1
            jr	$ra
            .previous
        .endif
            .section __ex_table, "a"
            .word	\from\()b, \to\()b
            .previous
        .endm

        .equ KSAVE_KSP,  0x30
        .equ KSAVE_CTX,  0x31
        .equ KSAVE_USP,  0x32
        .equ LA_CSR_PGDL,          0x19    /* Page table base address when VA[47] = 0 */
        .equ LA_CSR_PGDH,          0x1a    /* Page table base address when VA[47] = 1 */
        .equ LA_CSR_PGD,           0x1b    /* Page table base */
        .equ LA_CSR_TLBRENTRY,     0x88    /* TLB refill exception entry */
        .equ LA_CSR_TLBRBADV,      0x89    /* TLB refill badvaddr */
        .equ LA_CSR_TLBRERA,       0x8a    /* TLB refill ERA */
        .equ LA_CSR_TLBRSAVE,      0x8b    /* KScratch for TLB refill exception */
        .equ LA_CSR_TLBRELO0,      0x8c    /* TLB refill entrylo0 */
        .equ LA_CSR_TLBRELO1,      0x8d    /* TLB refill entrylo1 */
        .equ LA_CSR_TLBREHI,       0x8e    /* TLB refill entryhi */
        .equ TF_VR,                34*8
        .equ TF_FCC,               TF_VR + 32*16
        .equ TF_FCSR,              TF_FCC + 8
        .macro SAVE_REGS
            st.d    $ra, $sp,  1*8
            st.d    $tp, $sp,  2*8
            st.d    $a0, $sp,  4*8
            st.d    $a1, $sp,  5*8
            st.d    $a2, $sp,  6*8
            st.d    $a3, $sp,  7*8
            st.d    $a4, $sp,  8*8
            st.d    $a5, $sp,  9*8
            st.d    $a6, $sp, 10*8
            st.d    $a7, $sp, 11*8
            st.d    $t0, $sp, 12*8
            st.d    $t1, $sp, 13*8
            st.d    $t2, $sp, 14*8
            st.d    $t3, $sp, 15*8
            st.d    $t4, $sp, 16*8
            st.d    $t5, $sp, 17*8
            st.d    $t6, $sp, 18*8
            st.d    $t7, $sp, 19*8
            st.d    $t8, $sp, 20*8
            st.d    $r21,$sp, 21*8
            st.d    $fp, $sp, 22*8
            st.d    $s0, $sp, 23*8
            st.d    $s1, $sp, 24*8
            st.d    $s2, $sp, 25*8
            st.d    $s3, $sp, 26*8
            st.d    $s4, $sp, 27*8
            st.d    $s5, $sp, 28*8
            st.d    $s6, $sp, 29*8
            st.d    $s7, $sp, 30*8
            st.d    $s8, $sp, 31*8
            csrrd   $t0, KSAVE_USP
            st.d    $t0, $sp,  3*8

            csrrd	$t0, 0x1
            st.d	$t0, $sp, 8*32  // prmd

            csrrd   $t0, 0x6        
            st.d    $t0, $sp, 8*33  // era

            vst     $vr0,  $sp, TF_VR + 0*16
            vst     $vr1,  $sp, TF_VR + 1*16
            vst     $vr2,  $sp, TF_VR + 2*16
            vst     $vr3,  $sp, TF_VR + 3*16
            vst     $vr4,  $sp, TF_VR + 4*16
            vst     $vr5,  $sp, TF_VR + 5*16
            vst     $vr6,  $sp, TF_VR + 6*16
            vst     $vr7,  $sp, TF_VR + 7*16
            vst     $vr8,  $sp, TF_VR + 8*16
            vst     $vr9,  $sp, TF_VR + 9*16
            vst     $vr10, $sp, TF_VR + 10*16
            vst     $vr11, $sp, TF_VR + 11*16
            vst     $vr12, $sp, TF_VR + 12*16
            vst     $vr13, $sp, TF_VR + 13*16
            vst     $vr14, $sp, TF_VR + 14*16
            vst     $vr15, $sp, TF_VR + 15*16
            vst     $vr16, $sp, TF_VR + 16*16
            vst     $vr17, $sp, TF_VR + 17*16
            vst     $vr18, $sp, TF_VR + 18*16
            vst     $vr19, $sp, TF_VR + 19*16
            vst     $vr20, $sp, TF_VR + 20*16
            vst     $vr21, $sp, TF_VR + 21*16
            vst     $vr22, $sp, TF_VR + 22*16
            vst     $vr23, $sp, TF_VR + 23*16
            vst     $vr24, $sp, TF_VR + 24*16
            vst     $vr25, $sp, TF_VR + 25*16
            vst     $vr26, $sp, TF_VR + 26*16
            vst     $vr27, $sp, TF_VR + 27*16
            vst     $vr28, $sp, TF_VR + 28*16
            vst     $vr29, $sp, TF_VR + 29*16
            vst     $vr30, $sp, TF_VR + 30*16
            vst     $vr31, $sp, TF_VR + 31*16

            movcf2gr   $t0, $fcc0
            st.b       $t0, $sp, TF_FCC + 0
            movcf2gr   $t0, $fcc1
            st.b       $t0, $sp, TF_FCC + 1
            movcf2gr   $t0, $fcc2
            st.b       $t0, $sp, TF_FCC + 2
            movcf2gr   $t0, $fcc3
            st.b       $t0, $sp, TF_FCC + 3
            movcf2gr   $t0, $fcc4
            st.b       $t0, $sp, TF_FCC + 4
            movcf2gr   $t0, $fcc5
            st.b       $t0, $sp, TF_FCC + 5
            movcf2gr   $t0, $fcc6
            st.b       $t0, $sp, TF_FCC + 6
            movcf2gr   $t0, $fcc7
            st.b       $t0, $sp, TF_FCC + 7
            movfcsr2gr $t0, $fcsr0
            st.w       $t0, $sp, TF_FCSR
        .endm

        // TrapFrame layout after regs[32], prmd and era:
        // f[0..32] starts at slot 34, fcc[0..8] at slot 66 and fcsr at slot 67.
        .macro SAVE_FP_REGS
            fst.d $f0,  $sp, 34*8
            fst.d $f1,  $sp, 35*8
            fst.d $f2,  $sp, 36*8
            fst.d $f3,  $sp, 37*8
            fst.d $f4,  $sp, 38*8
            fst.d $f5,  $sp, 39*8
            fst.d $f6,  $sp, 40*8
            fst.d $f7,  $sp, 41*8
            fst.d $f8,  $sp, 42*8
            fst.d $f9,  $sp, 43*8
            fst.d $f10, $sp, 44*8
            fst.d $f11, $sp, 45*8
            fst.d $f12, $sp, 46*8
            fst.d $f13, $sp, 47*8
            fst.d $f14, $sp, 48*8
            fst.d $f15, $sp, 49*8
            fst.d $f16, $sp, 50*8
            fst.d $f17, $sp, 51*8
            fst.d $f18, $sp, 52*8
            fst.d $f19, $sp, 53*8
            fst.d $f20, $sp, 54*8
            fst.d $f21, $sp, 55*8
            fst.d $f22, $sp, 56*8
            fst.d $f23, $sp, 57*8
            fst.d $f24, $sp, 58*8
            fst.d $f25, $sp, 59*8
            fst.d $f26, $sp, 60*8
            fst.d $f27, $sp, 61*8
            fst.d $f28, $sp, 62*8
            fst.d $f29, $sp, 63*8
            fst.d $f30, $sp, 64*8
            fst.d $f31, $sp, 65*8
            movcf2gr $t0, $fcc0
            st.b $t0, $sp, 66*8+0
            movcf2gr $t0, $fcc1
            st.b $t0, $sp, 66*8+1
            movcf2gr $t0, $fcc2
            st.b $t0, $sp, 66*8+2
            movcf2gr $t0, $fcc3
            st.b $t0, $sp, 66*8+3
            movcf2gr $t0, $fcc4
            st.b $t0, $sp, 66*8+4
            movcf2gr $t0, $fcc5
            st.b $t0, $sp, 66*8+5
            movcf2gr $t0, $fcc6
            st.b $t0, $sp, 66*8+6
            movcf2gr $t0, $fcc7
            st.b $t0, $sp, 66*8+7
            movfcsr2gr $t0, $fcsr0
            st.d $t0, $sp, 67*8
        .endm

        .macro LOAD_REGS
            ld.d    $t0, $sp, 32*8
            csrwr   $t0, 0x1        // Write PRMD(PLV PIE PWE) to prmd

            ld.d    $t0, $sp, 33*8
            csrwr   $t0, 0x6        // Write Exception Address to ERA

            ld.bu      $t0, $sp, TF_FCC + 0
            movgr2cf   $fcc0, $t0
            ld.bu      $t0, $sp, TF_FCC + 1
            movgr2cf   $fcc1, $t0
            ld.bu      $t0, $sp, TF_FCC + 2
            movgr2cf   $fcc2, $t0
            ld.bu      $t0, $sp, TF_FCC + 3
            movgr2cf   $fcc3, $t0
            ld.bu      $t0, $sp, TF_FCC + 4
            movgr2cf   $fcc4, $t0
            ld.bu      $t0, $sp, TF_FCC + 5
            movgr2cf   $fcc5, $t0
            ld.bu      $t0, $sp, TF_FCC + 6
            movgr2cf   $fcc6, $t0
            ld.bu      $t0, $sp, TF_FCC + 7
            movgr2cf   $fcc7, $t0
            ld.w       $t0, $sp, TF_FCSR
            movgr2fcsr $fcsr0, $t0

            vld     $vr0,  $sp, TF_VR + 0*16
            vld     $vr1,  $sp, TF_VR + 1*16
            vld     $vr2,  $sp, TF_VR + 2*16
            vld     $vr3,  $sp, TF_VR + 3*16
            vld     $vr4,  $sp, TF_VR + 4*16
            vld     $vr5,  $sp, TF_VR + 5*16
            vld     $vr6,  $sp, TF_VR + 6*16
            vld     $vr7,  $sp, TF_VR + 7*16
            vld     $vr8,  $sp, TF_VR + 8*16
            vld     $vr9,  $sp, TF_VR + 9*16
            vld     $vr10, $sp, TF_VR + 10*16
            vld     $vr11, $sp, TF_VR + 11*16
            vld     $vr12, $sp, TF_VR + 12*16
            vld     $vr13, $sp, TF_VR + 13*16
            vld     $vr14, $sp, TF_VR + 14*16
            vld     $vr15, $sp, TF_VR + 15*16
            vld     $vr16, $sp, TF_VR + 16*16
            vld     $vr17, $sp, TF_VR + 17*16
            vld     $vr18, $sp, TF_VR + 18*16
            vld     $vr19, $sp, TF_VR + 19*16
            vld     $vr20, $sp, TF_VR + 20*16
            vld     $vr21, $sp, TF_VR + 21*16
            vld     $vr22, $sp, TF_VR + 22*16
            vld     $vr23, $sp, TF_VR + 23*16
            vld     $vr24, $sp, TF_VR + 24*16
            vld     $vr25, $sp, TF_VR + 25*16
            vld     $vr26, $sp, TF_VR + 26*16
            vld     $vr27, $sp, TF_VR + 27*16
            vld     $vr28, $sp, TF_VR + 28*16
            vld     $vr29, $sp, TF_VR + 29*16
            vld     $vr30, $sp, TF_VR + 30*16
            vld     $vr31, $sp, TF_VR + 31*16

            ld.d    $ra, $sp, 1*8
            ld.d    $tp, $sp, 2*8
            ld.d    $a0, $sp, 4*8
            ld.d    $a1, $sp, 5*8
            ld.d    $a2, $sp, 6*8
            ld.d    $a3, $sp, 7*8
            ld.d    $a4, $sp, 8*8
            ld.d    $a5, $sp, 9*8
            ld.d    $a6, $sp, 10*8
            ld.d    $a7, $sp, 11*8
            ld.d    $t0, $sp, 12*8
            ld.d    $t1, $sp, 13*8
            ld.d    $t2, $sp, 14*8
            ld.d    $t3, $sp, 15*8
            ld.d    $t4, $sp, 16*8
            ld.d    $t5, $sp, 17*8
            ld.d    $t6, $sp, 18*8
            ld.d    $t7, $sp, 19*8
            ld.d    $t8, $sp, 20*8
            ld.d    $r21,$sp, 21*8
            ld.d    $fp, $sp, 22*8
            ld.d    $s0, $sp, 23*8
            ld.d    $s1, $sp, 24*8
            ld.d    $s2, $sp, 25*8
            ld.d    $s3, $sp, 26*8
            ld.d    $s4, $sp, 27*8
            ld.d    $s5, $sp, 28*8
            ld.d    $s6, $sp, 29*8
            ld.d    $s7, $sp, 30*8
            ld.d    $s8, $sp, 31*8
            
            // restore sp
            ld.d    $sp, $sp, 3*8
        .endm

        .macro LOAD_FP_REGS
            ld.d $t0, $sp, 67*8
            movgr2fcsr $fcsr0, $t0
            ld.b $t0, $sp, 66*8+0
            movgr2cf $fcc0, $t0
            ld.b $t0, $sp, 66*8+1
            movgr2cf $fcc1, $t0
            ld.b $t0, $sp, 66*8+2
            movgr2cf $fcc2, $t0
            ld.b $t0, $sp, 66*8+3
            movgr2cf $fcc3, $t0
            ld.b $t0, $sp, 66*8+4
            movgr2cf $fcc4, $t0
            ld.b $t0, $sp, 66*8+5
            movgr2cf $fcc5, $t0
            ld.b $t0, $sp, 66*8+6
            movgr2cf $fcc6, $t0
            ld.b $t0, $sp, 66*8+7
            movgr2cf $fcc7, $t0
            fld.d $f0,  $sp, 34*8
            fld.d $f1,  $sp, 35*8
            fld.d $f2,  $sp, 36*8
            fld.d $f3,  $sp, 37*8
            fld.d $f4,  $sp, 38*8
            fld.d $f5,  $sp, 39*8
            fld.d $f6,  $sp, 40*8
            fld.d $f7,  $sp, 41*8
            fld.d $f8,  $sp, 42*8
            fld.d $f9,  $sp, 43*8
            fld.d $f10, $sp, 44*8
            fld.d $f11, $sp, 45*8
            fld.d $f12, $sp, 46*8
            fld.d $f13, $sp, 47*8
            fld.d $f14, $sp, 48*8
            fld.d $f15, $sp, 49*8
            fld.d $f16, $sp, 50*8
            fld.d $f17, $sp, 51*8
            fld.d $f18, $sp, 52*8
            fld.d $f19, $sp, 53*8
            fld.d $f20, $sp, 54*8
            fld.d $f21, $sp, 55*8
            fld.d $f22, $sp, 56*8
            fld.d $f23, $sp, 57*8
            fld.d $f24, $sp, 58*8
            fld.d $f25, $sp, 59*8
            fld.d $f26, $sp, 60*8
            fld.d $f27, $sp, 61*8
            fld.d $f28, $sp, 62*8
            fld.d $f29, $sp, 63*8
            fld.d $f30, $sp, 64*8
            fld.d $f31, $sp, 65*8
        .endm

        .endif
        "#
    }
}
