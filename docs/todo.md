# TODO
1. 调度策略的优化：已重构为 MLFQ-lite（4 级反馈队列、队内 RR、时间片降级、唤醒提升、aging 防饿死）
2. 模块化与冗余代码整理：详见 [kernel-modularization-refactor-plan.md](kernel-modularization-refactor-plan.md)
3. 上板子
4. 决赛第一阶段测试用例
5. 网络部分
6. 接入真实的文件系统
7. GUI
8. 贪吃蛇

待做：
CPU负载平衡
kstack cache（待考虑）
队列要放到队首，不然创建3000个线程要等很久才有输出


cyclic
优化热路径 lmbench iozone libcbench
优化上下文切换的代价
现在的文件全部都是靠硬编码的，创建功能还是有点不足
不同CPU的时钟不一样



写一份debug的报告，就是phase代表什么
考虑异步文件系统，或者ext4换锁，现在是一把全局大锁，测试8核

8核8m


[kernel] Panicked at src/main.rs:505 [kernel] page fault in kernel mode: trap_type=InstructionPageFault(0), bad addr=0x0, current_root_ppn=0x81a11, current_translate=None, pte_info=Some((0, 0, PTEFlags(0x0), false, false, false, false, false)), ctx=Context {
    ra: 0x0,
    sp: 0xffffffffffb5aca0,
    gp: 0x0,
    tp: 0x4,
    t0: 0x1,
    t1: 0xffffffc0812d68e0,
    t2: 0xe0,
    s0: 0x0,
    s1: 0x4,
    a0: 0x0,
    a1: 0x0,
    a2: 0x3569fcb3a,
    a3: 0xffffffffffb5ad38,
    a4: 0x3569de2f2,
    a5: 0xffffffffffb5b130,
    a6: 0x0,
    a7: 0x54494d45,
    s2: 0x0,
    s3: 0xffffffc0812d6800,
    s4: 0x1,
    s5: 0xffffffc084f01000,
    s6: 0x1,
    s7: 0xffffffc084f01010,
    s8: 0xffffffc08137d020,
    s9: 0xffffffc084ea9000,
    s10: 0xffffffc084f01000,
    s11: 0xffffffc08138da78,
    t3: 0xffffffc0818eefa8,
    t4: 0x0,
    t5: 0x0,
    t6: 0x10040,
    sstatus: Sstatus {
        bits: 0x8000000200046100,
    },
    sepc: 0x0,
    f: [
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
        0x0,
    ],
    fcsr: 0x0,
}