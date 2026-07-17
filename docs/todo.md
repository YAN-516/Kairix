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
busybox 存在fail
CPU负载平衡
kstack cache（待考虑）
队列要放到队首，不然创建3000个线程要等很久才有输出


cyclic
优化热路径 lmbench iozone libcbench
优化上下文切换的代价
现在的文件全部都是靠硬编码的，创建功能还是有点不足
glibc的iozone存在死锁
sleep 存在问题，不同CPU的时钟不一样



写一份debug的报告，就是phase代表什么
考虑异步文件系统，测试4核

feat(kernel): 提升SMP并发正确性、调度能力与可观测性

- 新增只读的/proc/kairix_perf内核运行状态快照
- 修复wait4语义并延迟回收退出任务资源
- 修复ext4页缓存写回竞态与锁顺序问题
- 实现per-CPU MLFQ和基于负载的任务入队分配
- 修复RV64/LA64浮点上下文保存与恢复
- 完善SysV SHM和匿名MAP_SHARED的fork共享语义
- 优化fork/COW、页表构建和TLB刷新
- 修复定时器、信号返回及调度状态导致的CPU失活
- 加固堆、页帧分配器、互斥锁及任务所有权状态
- 优化VirtIO、fanotify、网络和定时器的锁顺序
- 新增SMP、fsync、SHM、浮点、fork 和 iozone回归测试
- 补充内核卡死日志及调度 phase 说明文档