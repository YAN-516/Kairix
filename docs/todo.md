# TODO

1. 调度策略的优化：已重构为 MLFQ-lite（4 级反馈队列、队内 RR、时间片降级、唤醒提升、aging 防饿死）
2. 模块化与冗余代码整理：详见 [kernel-modularization-refactor-plan.md](kernel-modularization-refactor-plan.md)
3. 上板子
4. 决赛第一阶段测试用例
5. 网络部分
6. 接入真实的文件系统
7. GUI
8. 贪吃蛇

文件系统 size truncate 修复
SMP调度死锁修复

待做：
busybox 存在fail
CPU负载平衡
kstack cache（待考虑）
队列要放到队首，不然创建3000个线程要等很久才有输出
文件系统的size问题
iperf卡死

## 模块化整理优先级

- P0：删除重复 import，替换 VFS trait 中的 `todo!/unimplemented!()` 默认实现。
- P1：继续拆分 `syscall/fs/mod.rs`，优先处理 `dir`、`path_ops` 等仍留在主模块里的路径/目录操作。
- P1：`syscall/signal/common.rs` 已恢复抽取；后续只继续抽低耦合 helper，架构 ABI/sigframe 保留在 riscv64/loongarch64。
- P2：拆分 `main.rs` 中的 trap/syscall/page fault/timer/return-to-user 处理。
- P2：瘦身 VFS `File` 胖接口，改成显式能力扩展 trait。
- P3：整理 `mm` 的 ELF/fault 边界，以及 `net/virtio` 与驱动层边界。
