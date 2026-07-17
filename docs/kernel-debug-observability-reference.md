# Kairix 内核调试信息参考

本文记录当前工作区中的内核调试标签、运行时快照字段和进度编号。以后看到
`KERNEL_STALL`、`PSELECT_STALL`、`scheduler_phases`、`ext4_flush.phase`
等输出时，可以直接在本文中查找含义。

本文以当前源码为准，主要对应：

- `os/src/task/processor.rs`
- `os/src/task/manager.rs`
- `os/src/task/mod.rs`
- `os/src/task/task.rs`
- `os/src/task/id.rs`
- `os/src/main.rs`
- `os/src/interrupts.rs`
- `os/src/fs/lwext4/`
- `os/src/drivers/block/virtio_blk.rs`
- `polyhal/polyhal-trap/src/trap/{riscv64,loongarch64}.rs`

## 1. 通用约定

### 1.1 数组下标

所有 CPU 数组都按 CPU/hart 编号排列。例如：

```text
ready_tasks: [2, 1, 0, 0]
```

表示 CPU0 有 2 个 Ready 队列任务，CPU1 有 1 个，CPU2/CPU3 为 0。

当前编译期上限是 4 个 CPU，但真正启用哪些 CPU 以 `online_mask` 为准。

### 1.2 常见哨兵值

在 64 位平台上：

```text
18446744073709551615 == usize::MAX
```

通常表示：

- 没有 CPU/hart 所有者；
- 没有 PID；
- 没有 syscall；
- 没有队列层级；
- 该字段尚未记录。

`0xffffff...`、`0x9000...` 等高地址可能是正常内核虚拟地址，不能仅因数值很大就
判断为损坏。

### 1.3 `Some`、`None` 和 `active`

- `Some(x)`：采样时存在有效对象或值。
- `None`：采样时不存在，或者 try-lock 失败后无法取得。
- `active: true`：子系统当前正在执行该操作。
- `active: false`：操作已经结束；其余 phase、inode、block 等字段可能保留的是
  上一次操作的最终值。

因此 `active: false, phase: complete` 是正常状态，不代表当前仍卡在 complete。

### 1.4 快照不是原子事务

一个 stall 快照依次采集 processor、run queue、task、timer、page cache、ext4 和
block I/O。各部分之间没有全局快照锁，因此系统在采样期间发生调度时，可能出现
短暂的跨字段不一致，例如：

- `current_samples` 中还有一个任务，但 `task_states` 中已经变成 Ready；
- `ready_tasks` 与 `physical_ready_tasks` 在一个入队边界上相差 1；
- `scheduler_phase` 已前进，但 `scheduler_pid` 还是前一个边界的值。

判断死锁应比较多次快照中是否持续停在相同位置，而不是要求单份快照所有字段
严格一致。

### 1.5 `println!` 与 LOG 输出

- 名称带 `_VISIBLE` 的标签使用直接打印，在 `LOG=OFF` 时仍然可见。
- `KERNEL_STALL`、`PSELECT_STALL`、`PSELECT_LONG_STALL`、
  `IO_PROGRESS_STALL`、`FORK_CLONE_STATE`、`FORK_CLONE_ENQUEUE_*`、
  `EXECVE_STALL` 等核心信息也是直接打印。
- 同名但没有 `_VISIBLE` 的版本通常使用 `warn!`，可能被 `LOG=OFF` 隐藏。
- `[IOZONE_HANG ...]` 大部分使用 `warn!`；是否可见取决于日志配置。

## 2. Stall 快照标签

| 标签 | 触发条件 | `sequence` 含义 |
| --- | --- | --- |
| `KERNEL_STALL` | 某 CPU 的 idle 调度循环持续空转，且任务/timer 状态不满足完全静默条件 | idle 自旋次数 |
| `PSELECT_STALL` | `pselect6(nfds=0)` 累计次数每达到 1000 次，前 32 次输出 | zero-fd pselect 累计次数 |
| `PSELECT_LONG_STALL` | zero-fd pselect 达到 100000 次后，每 100000 次输出 | zero-fd pselect 累计次数 |
| `IO_PROGRESS_STALL` | 至少 4 个 syscall 活跃，并且 read/write/pread/pwrite/fsync 总入口计数 5 秒没有变化 | I/O watchdog 输出序号 |

这些标签表示“watchdog 决定采样”，不是“已经证明发生死锁”。真正判断需要结合：

- `stalled_mask` 和 scheduler heartbeat；
- phase 是否在重复快照中保持不变；
- 锁的 `locked/owner/waiters`；
- I/O、page cache、writeback、timer 计数是否继续变化。

## 3. Scheduler phase 总表

`scheduler_phases[cpu]` 是该 CPU 最近写入的进度边界。它表示“已经到达这个边界”，
不一定表示 CPU 正在执行该编号对应的那一行之后的第一条语句。

### 3.1 主调度循环：1–14

| Phase | 含义 |
| ---: | --- |
| 0 | 尚未记录/初始化状态 |
| 1 | 主循环维护工作完成，准备进入 deferred reaper；在无任务分支也会重新写入 |
| 2 | 已从本 CPU run queue 取到任务，准备验证任务状态并安装到 Processor |
| 3 | 已把任务安装为当前任务，准备取得 PCB、切换用户页表 |
| 4 | 用户页表已激活，马上调用 `context_switch` 切换到任务上下文 |
| 5 | `context_switch` 已返回 idle 栈，并已恢复内核根页表，准备处理退出任务的重入队状态 |
| 6 | 任务切出后的 `on_cpu` 清理及必要重入队完成 |
| 7 | 准备读取硬件时间并更新 scheduler heartbeat |
| 8 | scheduler heartbeat 更新完成 |
| 9 | `check_timers()` 已返回 |
| 10 | 准备调用 `check_timers()` |
| 11 | deferred exited-task reaper 已完成，准备 timer maintenance |
| 12 | 准备执行 I/O progress watchdog |
| 13 | timer maintenance、网络轮询等维护工作完成，准备 fetch task |
| 14 | 紧邻 `fetch_task(cpu)` 之前 |

常见判断：

- 长时间固定在 7：可能停在硬件时钟读取或 heartbeat 更新边界。
- 长时间固定在 10：重点检查 timer queue。
- 长时间固定在 14：重点检查本 CPU run queue 获取路径。
- phase 1 本身不是异常，它是主循环常见边界。

### 3.2 Idle watchdog：20–38

| Phase | 含义 |
| ---: | --- |
| 20 | 本轮没有取到任务，进入 idle 分支并增加 idle spin |
| 21 | 准备采集 ready queue/writeback 的轻量 idle 信息 |
| 22 | 轻量 idle 信息采集完成 |
| 23 | `[IOZONE_HANG sched_idle]` 输出完成 |
| 24 | 准备执行完整 idle stall 快照 |
| 25 | 完整 idle stall 快照返回 |
| 26 | `dump_stall_snapshot()` 开始，准备判定是否属于正常静默 |
| 27 | task state 静默条件采集完成 |
| 28 | 当前 CPU 的诊断缓冲区忙，无法继续完整快照 |
| 29 | 已确定需要输出 `KERNEL_STALL`，准备采集完整快照 |
| 30 | `print_runtime_snapshot()` 开始，准备采集 ProcessorTaskStats |
| 31 | ProcessorTaskStats 已完成，正在/准备采集 LoadBalanceStats |
| 32 | LoadBalanceStats 已完成，准备取得 TaskStateStats 缓冲区 |
| 33 | TaskStateStats 填充完成，准备采集文件系统、块设备等子系统 |
| 34 | 子系统统计采集完成，马上打印主快照 |
| 35 | 主快照打印完成 |
| 36 | `KERNEL_STALL` 主快照完成，准备输出 fork/clone 状态 |
| 37 | fork/clone 状态输出阶段完成 |
| 38 | runtime snapshot 的 TaskStateStats 缓冲区忙，提前返回 |

注意：输出快照的观察 CPU 会主动把自己的 phase 改成 30–35，所以看到
`KERNEL_STALL cpu=1` 同时 `scheduler_phases[1]=30`，通常只是 CPU1 正在生成这条
日志，不表示它卡在 phase 30。

### 3.3 Run queue 取任务：40–49

| Phase | 含义 |
| ---: | --- |
| 40 | 已获得目标 run queue 锁 |
| 41 | 本地队列候选数大于 1，准备执行 MLFQ aging |
| 42 | aging 完成或无需 aging |
| 43 | 候选任务数和队列元信息已记录，准备扫描候选任务 |
| 44 | 准备从最高非空 MLFQ 层取一个任务 |
| 45 | `pop_next()` 已返回一个任务或空 |
| 46 | 对任务的 `ready_queued -> on_cpu` 原子 claim 已完成 |
| 47 | 候选扫描结束，准备释放 run queue 锁 |
| 48 | run queue 锁已释放，准备清理 stale task/owner intent |
| 49 | stale task 的延迟 drop 已完成，claim 路径结束 |

远程 claim 分支仍保留在代码中，但当前 `fetch_task()` 只调用
`claim_task_from_cpu(cpu, cpu)`，所以当前没有实际 work stealing。

### 3.4 Timer queue：50–57

| Phase | 含义 |
| ---: | --- |
| 50 | `check_timers()` 入口 |
| 51 | timer queue try-lock 失败，本轮直接返回 |
| 52 | 已取得 timer queue 锁 |
| 53 | 队列为空、最早 deadline 未到，或已把一个到期桶移出并释放锁 |
| 54 | 已输出 TIMER_EXPIRE，准备逐个唤醒任务 |
| 55 | 本轮 timer 检查全部完成 |
| 56 | 准备唤醒一个到期任务 |
| 57 | 一个到期任务唤醒完成 |

### 3.5 Deferred exited-task 回收：60–74

| Phase | 含义 |
| ---: | --- |
| 60 | deferred exited-task reaper 新一轮开始 |
| 61 | deferred queue try-lock 失败，本轮返回 |
| 62 | 已取得 deferred queue 锁，准备 pop |
| 63 | 已取得一个待回收项 |
| 64 | 准备调用 `task.release_exited_resources()` |
| 65 | TaskControlBlock 内的退出资源释放完成 |
| 66 | detached user resources 以及 task Arc drop 完成 |
| 67 | deferred queue 已空，reaper 返回 |
| 68 | 待回收 payload 拆分边界 |
| 69 | 已取出其中的 task Arc |
| 70 | `TaskControlBlock::release_exited_resources()` 开始 |
| 71 | 已取得 task inner 锁，清理 signal context，并取走 TaskUserRes |
| 72 | task inner 锁已释放，准备释放 TaskUserRes |
| 73 | TaskUserRes 释放完成，准备释放 kernel stack |
| 74 | kernel stack 释放完成 |

### 3.6 TaskStateStats 采集：80–90

| Phase | 含义 |
| ---: | --- |
| 80 | task-state 采集开始 |
| 81 | PID2PCB/process table try-lock 失败，设置 `process_table_busy=true` 后返回 |
| 82 | 已取得进程表，可开始遍历进程 |
| 83 | 准备检查一个进程 |
| 84 | 该进程 inner try-lock 失败，累计 `process_locks_busy` |
| 85 | 已取得该进程 inner 锁，准备遍历 tasks |
| 86 | 准备检查一个 task |
| 87 | task inner try-lock 失败，累计 `task_locks_busy` |
| 88 | 已取得 task inner 锁，准备记录状态、syscall 和上下文 |
| 89 | 一个 task 记录完成 |
| 90 | 所有可访问进程/task 记录完成 |

### 3.7 Timer 抢占：100–107

| Phase | 含义 |
| ---: | --- |
| 100 | `preempt_current_and_run_next()` 入口 |
| 101 | 已从 Processor 取出当前任务 |
| 102 | zombie/orphan/process-exit 检查完成，准备扣减 MLFQ 时间片 |
| 103 | 已更新时间片、MLFQ 层级以及 requeue 标志 |
| 104 | 抢占路径内部的额外边界，当前与 103 相邻 |
| 105 | 紧邻 `schedule(task_cx_ptr)` 之前 |
| 106 | `schedule()` 返回到被抢占调用链之后 |
| 107 | 用户返回循环已执行 `prepare_user_return()`，准备进入架构用户恢复函数 |

### 3.8 Idle 和后台维护：110–140

| Phase | 含义 |
| ---: | --- |
| 110 | idle 调度栈准备执行一次 `spin_loop()` |
| 111 | idle `spin_loop()` 返回 |
| 112 | 准备轮询网络接收队列 |
| 113 | 网络接收轮询完成 |
| 120 | TaskUserRes `release_with_process()` 开始 |
| 121 | 确认存在 PCB，准备取得 process inner |
| 122 | 已取得 process inner，准备释放 TID |
| 123 | TID 已释放，准备摘除用户栈 VM area |
| 124 | 用户栈 area 已摘除，准备摘除 trap-context area |
| 125 | trap-context area 已摘除，process inner 临界区即将结束 |
| 126 | process inner 已释放，准备清空用户栈 frames |
| 127 | 用户栈 area/frames 已释放，准备清空 trap-context frames |
| 128 | trap-context area/frames 已释放 |
| 129 | 没有 PCB 的资源释放快速路径完成 |
| 130 | deferred timer maintenance 入口 |
| 131 | 取得本轮 maintenance 执行权 |
| 132 | maintenance tick 已读取，准备周期性内存统计 |
| 133 | MEMDEBUG 阶段完成，准备 alarm/itimer 维护 |
| 134 | 已取得 TIMER_PROCS 锁 |
| 135 | 正在检查一个带 alarm/itimer 的进程 |
| 136 | TIMER_PROCS 遍历完成，准备移除失效 PID |
| 137 | TIMER_PROCS 锁已释放，准备发送 SIGALRM |
| 138 | SIGALRM 投递阶段完成，准备周期性 writeback/reclaim |
| 139 | 准备执行后台 reclaim |
| 140 | 后台 reclaim 完成 |

#### Phase 140–147 的编号复用

run queue 的 `pop_next()` 也使用动态编号：

| Phase | 另一种含义 |
| ---: | --- |
| 140 | MLFQ level 0：`pop_front()` 前 |
| 141 | MLFQ level 0：`pop_front()` 后 |
| 142 | MLFQ level 1：`pop_front()` 前 |
| 143 | MLFQ level 1：`pop_front()` 后 |
| 144 | MLFQ level 2：`pop_front()` 前 |
| 145 | MLFQ level 2：`pop_front()` 后 |
| 146 | MLFQ level 3：`pop_front()` 前 |
| 147 | MLFQ level 3：`pop_front()` 后 |

因此 phase 140 有歧义：既可能是后台 reclaim 完成，也可能是 level 0 队列
`pop_front()` 之前。结合 `run_queue_pop_level`、当前调用上下文和重复快照判断。

### 3.9 页表切换和用户态返回：150–163

| Phase | 含义 |
| ---: | --- |
| 150 | 准备激活选中任务的用户页表 |
| 151 | 用户页表激活完成 |
| 152 | 任务已切回 idle 栈，准备恢复永久内核根页表 |
| 153 | 内核根页表恢复完成 |
| 160 | 进入架构 `user_restore`，尚未保存内核 callee-saved 上下文 |
| 161 | 已切换到 TrapFrame，准备恢复浮点寄存器 |
| 162 | 浮点寄存器已恢复，准备恢复通用寄存器 |
| 163 | 用户通用寄存器已恢复，紧邻 RISC-V `sret` 或 LoongArch `ertn` 之前 |

160–163 由 RV64/LA64 汇编直接写入，不经过 `record_scheduler_phase()`。因此这些
phase 不会同步刷新 `scheduler_pid`、`scheduler_irq_enabled`、`scheduler_sp` 和
`scheduler_ra`。phase 163 长时间保留也可能仅表示 CPU 已经返回用户态、尚未再次
进入 Rust 调度器，不能单独视为失活。

`prepare_user_return()` 不只在 phase 107 对应的任务入口执行。普通 syscall、用户
缺页、已处理异常、信号处理以及 timer 抢占恢复都会从 trap vector 直接返回用户态，
这些出口现在也统一恢复 RV64 `SSTATUS`、LA64 `PRMD` 和当前 CPU 的 timer interrupt
mask。若快照显示某 CPU 长期停在用户 PC、`active_syscall=None` 且 timer heartbeat
不再变化，应检查该统一返回边界，而不是将 phase 107 本身判断为调度器死锁。

## 4. ProcessorTaskStats 字段

| 字段 | 含义 | 异常线索 |
| --- | --- | --- |
| `current_tasks` | 所有 Processor 中当前安装的 task 数量 | 小于实际运行任务数且 `locked_processors>0` 时统计不完整 |
| `locked_processors` | 采样时无法 try-lock 的 Processor 数量 | 持续非 0 时检查对应 Processor 锁 |
| `current_samples[cpu]` | `(pid, active_syscall, UserContextSnapshot)` | `None` 通常表示该 CPU 当前在 idle scheduler |
| `idle_contexts[cpu]` | `(idle_sp, idle_ra)` | 用于验证 idle 上下文是否被覆盖 |
| `scheduler_phases[cpu]` | 最近 phase | 查第 3 节 |
| `scheduler_pids[cpu]` | 最近 Rust phase 携带的 PID | `usize::MAX` 表示该 phase 未绑定任务；160–163 不更新它 |
| `scheduler_irq_enabled[cpu]` | 写入最近 Rust phase 时 IRQ 是否开启 | idle scheduler 中 false 通常正常 |
| `scheduler_sps[cpu]` | 最近采样的内核栈指针 | 应位于该 CPU scheduler/kernel stack 范围 |
| `scheduler_ras[cpu]` | 最近 phase 记录点的返回地址 | 可配合符号表定位调用点 |
| `scheduler_stack_cpus[cpu]` | 根据 SP 推断的 scheduler stack 所属 CPU | 与数组下标不一致时可能是跨 CPU 栈污染 |

`UserContextSnapshot`：

- `pc`：最近用户 PC；
- `ra`：用户返回地址寄存器；
- `sp`：用户栈指针；
- `tls`：用户 TLS/线程指针；
- `fcsr`：浮点控制状态。

全 0 可能表示任务正在内核路径、尚未发布用户快照，不一定是上下文损坏。

## 5. LoadBalanceStats 字段

| 字段 | 含义 | 判读方法 |
| --- | --- | --- |
| `remote_enqueues` | 任务被推送到非当前 CPU 的累计次数 | 增长表示入队时负载分散生效 |
| `steal_attempts` | work-steal 尝试次数 | 当前 fetch 路径不偷任务，通常为 0 |
| `steal_successes` | work-steal 成功次数 | 当前通常为 0 |
| `ready_tasks[cpu]` | 原子维护的逻辑 Ready 计数 | 与 physical 对照检查记账 |
| `online_mask` | 在线 CPU 位图 | `3` 即 CPU0、CPU1 在线 |
| `stalled_mask` | scheduler heartbeat 超过 100ms 未更新的在线 CPU 位图 | 非 0 才是调度心跳层面的失活候选 |
| `scheduler_heartbeats_ns` | 各 CPU 最近进入 scheduler loop 的时间 | 与快照 `now_ns` 比较 |
| `timer_interrupt_heartbeats_ns` | 各 CPU 最近处理硬件 timer IRQ 的时间 | idle scheduler IRQ 关闭时可以比 scheduler heartbeat 更旧 |
| `timer_programming` | 各 CPU 最近一次 one-shot timer 编程证据 | 见第 9 节 |
| `physical_ready_tasks[cpu]` | try-lock 成功后直接读取的真实队列长度 | `None` 表示采样时队列锁忙 |
| `local_fetch_pending[cpu]` | 队列所有 CPU 正准备获取自己的队列 | true 时远程访问应退让 |
| `remote_queue_mutation_pending_mask[target]` | 哪些 source CPU 正等待修改 target 队列 | 位图长期不清零提示远程入队路径停滞 |
| `run_queue_locked` | 每 CPU run queue 锁状态 | true 时结合 owner 字段 |
| `run_queue_owner_harts/lines` | run queue 持锁 hart 和获取源码行 | `usize::MAX/0` 表示无所有者 |
| `local_fetch_contentions` | 本 CPU 获取自己队列时 try-lock 失败累计次数 | 持续快速增长表示队列竞争 |
| `local_empty_mismatches` | 逻辑 Ready 非 0 但物理队列为空的累计次数 | 非 0 提示 Ready 记账或队列所有权异常 |
| `run_queue_pop_candidates` | 最近一次 pop 扫描开始时的候选数 | 历史进度，不是当前长度 |
| `run_queue_pop_level` | 最近扫描到的 MLFQ 层级 | `usize::MAX` 表示尚未扫描 |
| `run_queue_pop_len/capacity` | 最近 pop 前该层 VecDeque 长度/容量 | `len>capacity` 必为损坏 |
| `run_queue_pop_first_*` | VecDeque 第一段 slice 地址和长度 | 用于定位 ring buffer 元数据损坏 |
| `run_queue_pop_second_*` | VecDeque 绕回后的第二段 slice | 两段长度之和必须等于 len |

`ready_tasks` 是逻辑原子计数，`physical_ready_tasks` 是实际 VecDeque 长度。短暂差异可能
处于入队/出队边界；重复快照持续差异才需要追查。

## 6. TaskStateStats 字段

| 字段 | 含义 |
| --- | --- |
| `process_table_busy` | PID2PCB try-lock 失败；其余 task 数量通常不完整 |
| `process_locks_busy` | 无法 try-lock 的进程数量 |
| `first_busy_process_pid` | 第一个锁忙的 PID |
| `first_busy_process_owner_cpu/line` | 该进程 inner 锁的持有 CPU 和获取源码行 |
| `task_locks_busy` | 无法 try-lock 的 task 数量 |
| `total` | 成功访问到的 task 总数 |
| `ready/running/blocked/zombie/sleep` | 各 TaskStatus 数量 |
| `ready_unowned` | Ready，但既不在 ready queue，也没有 `on_cpu` 所有权；持续非 0 是丢任务信号 |
| `running_not_on_cpu` | Running，但 `on_cpu` 为空；持续非 0 是状态机错误 |
| `blocked_queued` | Blocked 却仍在 ready queue；持续非 0 是重复入队/阻塞竞态信号 |
| `active_syscalls` | 非 zombie task 中 active syscall 数量 |
| `first_active_syscall/pid` | 第一个观察到的 syscall/PID |
| `active_samples` | `(pid, syscall_id, syscall_stage)`，最多保存 `MAX_CPU_NUM` 项 |
| `workload_sample_count` | PID>3 的 workload task 总数，可能大于数组容量 |
| `workload_samples` | `(pid, status, syscall, queued_cpu, on_cpu_cpu)`，最多 8 项 |
| `workload_context_samples` | 对应 workload 的用户上下文快照 |

三个最重要的不变量：

```text
Ready   => ready_queued 或 on_cpu 至少一个成立
Running => on_cpu 必须成立
Blocked => 不应位于 ready queue
```

## 7. Active syscall 和 syscall stage

每次进入 syscall 时记录 syscall ID，退出 syscall 时自动清除。timer IRQ 每次为当前
active syscall 增加 `ticks`：

- 第 500 tick 输出一次 `SYSCALL_STALL`；
- 之后每 5000 tick 再输出一次。

常见 syscall ID：

| ID | syscall |
| ---: | --- |
| 22 | `epoll_pwait` |
| 35 | `unlinkat` |
| 56 | `openat` |
| 57 | `close` |
| 62 | `lseek` |
| 63 / 64 | `read` / `write` |
| 67 / 68 | `pread64` / `pwrite64` |
| 72 / 73 | `pselect6` / `ppoll` |
| 82 | `fsync` |
| 98 | `futex` |
| 101 | `nanosleep` |
| 115 | `clock_nanosleep` |
| 124 | `sched_yield` |
| 220 | `clone/fork` |
| 221 | `execve` |
| 260 | `wait4/waitpid` |
| 276 | `renameat2` |

### 7.1 `unlinkat` stage 1–8

| Stage | 含义 |
| ---: | --- |
| 0 | 该 syscall 尚未进入细分阶段 |
| 1 | 用户路径已翻译，准备解析起始 dentry/父目录 |
| 2 | 父目录和文件名解析完成 |
| 3 | 目标 dentry 已找到，准备权限、类型和 nlink 检查 |
| 4 | 准备执行实际 `parent.unlink()` |
| 5 | unlink 已成功，准备处理已关闭文件的 page cache/writeback |
| 6 | page-cache 清理完成，准备发送 inotify delete |
| 7 | inotify 完成，准备发送 fanotify delete |
| 8 | fanotify delete 完成，unlinkat 即将返回 |

### 7.2 fanotify delete stage 100–121

这些 stage 会覆盖 unlinkat 的 stage 7：

| Stage | 含义 |
| ---: | --- |
| 100 | 开始为被删除 dentry 计算 fanotify event path |
| 101 | live fanotify instances 已取得，准备遍历 |
| 110 | 准备向一个 instance 发送 `FAN_DELETE` |
| 111 | `FAN_DELETE` 完成，准备发送 `FAN_DELETE_SELF` |
| 112 | 当前 instance 的两个 delete 事件处理完成 |
| 120 | 所有 instance 完成，准备清理 renamed-dentry cache |
| 121 | renamed-dentry cache 清理完成 |

### 7.3 `renameat2` stage 20–26

| Stage | 含义 |
| ---: | --- |
| 20 | 新旧用户路径已翻译并完成长度检查 |
| 21 | 旧父目录和旧 dentry 已解析 |
| 22 | 新父目录已解析，准备权限、superblock 和只读检查 |
| 23 | 准备执行底层 dentry `rename()` |
| 24 | 底层 rename 已完成，准备发送 inotify move |
| 25 | inotify move 已完成，准备发送 fanotify move |
| 26 | fanotify move 已完成，`renameat2` 即将返回 |

当底层文件系统是 FAT32/VFAT 时，stage 23 会继续细分为 230–241：

| Stage | 含义 |
| ---: | --- |
| 230 | 进入 FAT32 dentry `rename()`，尚未查找源 dentry |
| 231 | 源 dentry 已找到，准备校验目标父目录与目录层级关系 |
| 232 | 目标 dentry、类型及空目录校验完成 |
| 233 | 准备获取 FAT32 文件系统串行锁；长时间停留表示锁竞争/锁所有者未推进 |
| 234 | FAT32 文件系统锁已获取，准备打开源、目标磁盘目录 |
| 235 | 源、目标磁盘目录均已打开 |
| 236 | 准备删除磁盘上已存在的目标项 |
| 237 | 已存在目标项删除完成 |
| 238 | 准备执行 fatfs `Dir::rename()` |
| 239 | fatfs `Dir::rename()` 已完成 |
| 240 | FAT32 文件系统锁已释放，准备更新 VFS dentry/dcache |
| 241 | VFS dentry/dcache 更新完成，底层 rename 即将返回 |

### 7.4 fanotify move stage 200–221

这些 stage 会覆盖 `renameat2` 的 stage 25：

| Stage | 含义 |
| ---: | --- |
| 200 | 进入 fanotify move，准备解析 rename 后的新 dentry |
| 201 | 新 dentry 解析完成，准备获取 live fanotify instances |
| 202 | live instances 已取得，准备遍历 |
| 210 | 开始处理一个 instance 的 rename/move 事件 |
| 211 | 目录 `FAN_RENAME` 已处理，准备检查 `FAN_MOVED_FROM` |
| 212 | `FAN_MOVED_FROM` 已处理，准备处理非目录 `FAN_RENAME` |
| 213 | rename 事件已处理，准备检查 `FAN_MOVED_TO` |
| 214 | `FAN_MOVED_TO` 已处理，准备处理非目录 `FAN_MOVE_SELF` |
| 215 | 当前 instance 的 move 事件全部完成 |
| 220 | 所有 instance 完成，准备使用 dentry/inode 更新 renamed-path cache |
| 221 | renamed-path cache 更新完成 |

## 8. Timer 调试信息

### 8.1 TimerQueueStats

| 字段 | 含义 |
| --- | --- |
| `lock_busy` | timer queue try-lock 是否失败 |
| `owner_hart/owner_line` | 锁忙时的所有者和获取行 |
| `deadlines` | 不同 deadline bucket 数量 |
| `tasks` | 所有 bucket 内等待任务总数 |
| `now_ns` | 采样时当前时间 |
| `earliest_deadline_ns` | 最早 deadline |
| `overdue_tasks` | deadline 已到但仍留在队列的任务数 |

`earliest_deadline_ns > now_ns` 且 `overdue_tasks=0` 表示 timer 队列正常等待未来时间。

### 8.2 TimerProgrammingStats

| 字段 | 含义 |
| --- | --- |
| `observed_ticks` | 生成快照时观察到的硬件 tick |
| `counts[cpu]` | 各 CPU timer 编程累计次数 |
| `current_ticks[cpu]` | 该 CPU 最近编程前读取的 tick |
| `deadline_ticks[cpu]` | 最近提交给 HAL/SBI 的绝对 deadline |
| `errors[cpu]` | 最近错误码，0 表示成功 |

正常情况下 `deadline_ticks > current_ticks`。`TIMER_PROGRAM_ERROR_VISIBLE` 表示 HAL/SBI
明确返回错误。

### 8.3 Timer 标签

- `TIMER_EXPIRE[_VISIBLE]`：一个到期 bucket 已从 timer queue 移出。
- `TIMER_EXPIRE_DONE[_VISIBLE]`：该 bucket 中所有任务已执行 wakeup。
- `TIMER_IRQ_SCHED_STALL[_VISIBLE]`：某 CPU scheduler heartbeat 超过 1 秒不变；
  `observer_cpu` 是打印者，`stalled_cpu` 才是被怀疑的 CPU。
- `CLOCK_NANOSLEEP_QUEUED_VISIBLE`：任务已进入 timer queue。
- `CLOCK_NANOSLEEP_RESUME_VISIBLE`：任务从阻塞恢复；比较 `now_ns` 和 `deadline_ns`。

完整的 `CLOCK_NANOSLEEP` 顺序是：

| 标签/动作 | 含义 |
| --- | --- |
| `CLOCK_NANOSLEEP enter` | 参数已换算，记录调用时刻、deadline、duration 和 flags |
| `CLOCK_NANOSLEEP_QUEUED_VISIBLE` / `queued` | 已调用 `add_timer()`，任务位于 timer queue |
| `CLOCK_NANOSLEEP block` | 准备执行 `block_current_and_run_next()` |
| `CLOCK_NANOSLEEP_RESUME_VISIBLE` / `resume` | block 返回，任务重新得到执行 |
| `CLOCK_NANOSLEEP done` | deadline 条件满足，syscall 准备成功返回 |

只有 QUEUED/RESUME 的 `_VISIBLE` 版本保证在 `LOG=OFF` 时可见。

## 9. 文件系统和块设备 phase

### 9.1 BlockingMutexStats

| 字段 | 含义 |
| --- | --- |
| `inner_busy` | 保护 mutex 状态/等待队列的内部 spinlock 忙 |
| `locked` | 数据锁当前被持有 |
| `handoff` | 预留字段；当前实现恒为 false |
| `waiters` | wait_queue 中 Weak 项数 |
| `live_waiters` | 仍能 upgrade 的有效等待任务数 |
| `owner_hart/owner_pid/owner_line` | 当前持锁 CPU、PID 和获取源码行 |

`locked=false`、`waiters=0`、owner 为 `usize::MAX` 是正常空闲状态。

### 9.2 Lwext4LockStats

| Phase | 含义 |
| ---: | --- |
| 0 | 空闲 |
| 2 | 已取得全局 lwext4 BlockingMutex，准备发布 owner |
| 3 | owner/递归深度已发布，正在执行 lwext4 操作 |
| 4 | lwext4 操作闭包已经返回 |
| 5 | owner、PID、syscall 和 recursion 已清除 |
| 6 | 正在释放 BlockingMutex guard |

其他字段：

- `owner`：TaskControlBlock 地址；无 task 上下文时使用基于 hart 的特殊值；
- `owner_pid`：持有者进程；0 表示非任务上下文；
- `owner_syscall`：持有者当时的 active syscall；
- `recursion`：同一 owner 的递归进入深度。

### 9.3 Ext4FlushStats

主 phase：

| Phase | 含义 |
| ---: | --- |
| 0 | 尚未运行/空闲 |
| 1 | flush 已开始，正在等待/取得 ext4 file lock |
| 2 | 执行写回前的初始 truncate |
| 3 | 正在逐页写回 dirty pages |
| 4 | 执行最终文件长度 truncate |
| 5 | 执行 lwext4 file cache flush |
| 6 | flush 完成；`active=false` 时是历史最终状态 |

当主 phase 为 3 时，`page_phase`：

| Page phase | 含义 |
| ---: | --- |
| 0 | 当前没有页 |
| 1 | 等待当前页 write lock |
| 2 | 页锁已取得 |
| 3 | 正在 `file_seek` |
| 4 | seek 完成，准备取得 frame/写数据 |
| 5 | 正在调用 lwext4 `file_write` |
| 6 | file_write 已返回 |
| 7 | 当前页完成、跳过或提前结束 |

辅助字段：`pid`、`inode`、`dirty_pages`、`pages_done`、`current_page`、
`file_size`。判断是否无进展时，应比较多次快照中的 `pages_done/current_page/page_phase`。

### 9.4 VirtioBlockIoStats

`op`：0=无，1=读，2=写。

| Phase | 含义 |
| ---: | --- |
| 0 | 尚未运行/空闲 |
| 1 | VirtIO 设备锁已取得，请求开始 |
| 2 | 等待全局 bounce buffer 锁 |
| 3 | bounce buffer 已取得，准备/处理一个 chunk |
| 4 | 正在提交非阻塞 VirtIO 请求 |
| 5 | 等待 used ring 出现对应 token |
| 6 | token 已完成，正在 complete request |
| 7 | 整个请求完成；`active=false` 时是历史最终状态 |
| 41 | 提交期间正在把 DMA 虚拟地址翻译为物理地址 |

辅助字段：

- `block_id`：整个请求起始 sector；
- `sectors`：整个请求 sector 数；
- `chunk_sector`：当前 bounce chunk 起始 sector；
- `token`：当前 VirtIO token；
- `polls`：等待 used ring 的自旋次数。

`active=true, phase=5` 且多份快照中 token/polls 一直不变，才是块设备完成路径
停滞的强信号。

### 9.5 PageCacheAtomicStats

| 字段 | 含义 |
| --- | --- |
| `pages` | 当前 page cache 总页数 |
| `tmpfs_pages/fat32_pages/ext4_pages/unknown_pages` | 按文件系统分类的当前页数 |
| `insert_count/remove_count` | 成功插入/移除的累计次数 |

应满足分类总和约等于 `pages`。`insert_count-remove_count` 应与当前 pages 大致一致；
如果重复快照完全不变，要结合 workload 是否仍应产生 I/O 判断。

### 9.6 Writeback 标签

- `IOZONE_HANG writeback_drain_enter`：开始按 page budget 排空队列。
- `IOZONE_HANG writeback_flush_enter`：准备 flush 一个 inode/file。
- `IOZONE_HANG writeback_flush_done`：该 file 返回 `(flushed_pages, has_more)`。
- `IOZONE_HANG writeback_drain_done`：本轮预算处理完成。
- `writeback_pending=Some(n)`：采样成功，队列有 n 个 file。
- `writeback_pending=None`：writeback queue try-lock 失败，不等价于 0。

## 10. Fork/clone 和 COW 进度

`FORK_CLONE_STATE` 只在 `clone.active=true` 时输出。

### 10.1 ForkCloneStats

| Phase | 含义 |
| ---: | --- |
| 1 | fork/clone 开始，parent PID 和 owner CPU 已发布 |
| 2 | 已取得 parent process inner，开始复制/共享地址空间 |
| 3 | 地址空间 COW/共享创建完成，继续复制 fd 和进程属性 |
| 4 | parent 快照完成，开始创建 child PCB、socket、kernel stack |
| 5 | child TCB 已构造，开始设置调度属性、TrapFrame、TID/父子关系 |
| 6 | child 已标记 Ready 并完成调度队列入队 |

字段：

- `generation`：fork/clone 操作累计代数；
- `parent_pid`：当前父进程；
- `owner_cpu`：执行 clone 的 CPU；
- `active`：当前操作是否仍在上述路径中。

`FORK_CLONE_ENQUEUE_ENTER/DONE` 包围 child 的 `add_task()`。只有 ENTER 没有 DONE，
说明停在目标 CPU 选择或 run queue 入队路径。

### 10.2 ForkCowStats

主 phase：

| Phase | 含义 |
| ---: | --- |
| 1 | COW clone 开始 |
| 2 | child VMSet/kernel mappings 初始化完成 |
| 3 | 遍历每个 UserMapArea，创建共享/COW/直接复制描述 |
| 4 | 复制 trap context、rt_sigreturn 等必须直接复制的页 |
| 5 | 更新 parent PTE 为 COW/只读标志 |
| 6 | parent PTE 更新完成，执行精确或全局 TLB flush |
| 7 | COW VMSet 构建完成 |

`area_subphase`（主 phase 3 内）：

| Subphase | 含义 |
| ---: | --- |
| 1 | 扫描/收集当前 area 的 resident pages |
| 2 | 构造 child `UserMapArea` |
| 3 | 把 resident pages 映射进 child 页表 |
| 4 | 当前 area 完成 |

`work_index/work_total` 表示主阶段进度；`area_page_index/area_page_total` 表示当前
area 内页进度；`resident_pages_done` 是累计观察到的 resident page 数。

## 11. I/O 活动计数与 IOZONE 标签

`io_activity={reads,writes,preads,pwrites,fsyncs}` 是 syscall 入口累计数，不是成功字节数。

- 计数变化：相应 syscall 仍在进入。
- 计数不变：没有新入口；已有 syscall 可能仍卡在内部。
- 计数增长但 workload 无输出：可能卡在用户态协调、频繁失败，或每次调用仍完成但
  上层条件未满足。

read/write/pread/pwrite 的 `IOZONE_HANG` ENTER/DONE 仅对前 64 次以及每 256 次
采样输出；错误日志不受该采样限制。fsync ENTER/DONE 当前每次输出。

字段：

- `seq`：该 syscall 类型独立累计序号；
- `offset`：调用前文件偏移，pread/pwrite 则为显式偏移；
- `inode`：优先使用 page-cache inode ID，否则使用文件系统 inode；
- `path`：采样时 dentry path；
- ENTER 无对应 DONE：重点查看对应文件系统锁、ext4 flush 和 block I/O phase。

### 11.1 调度和 wakeup 标签

| 标签 | 含义 |
| --- | --- |
| `IOZONE_HANG sched_idle` | 某 CPU 本轮没有取得任务；包含 idle spin、各 CPU ready queue 和 writeback 状态 |
| `IOZONE_HANG wakeup_enter` | `wakeup_task()` 入口，记录原状态、pending、on_cpu 和 queued |
| `IOZONE_HANG wakeup_on_cpu` | 目标任务仍被某 CPU 持有，不能立即入队；设置 pending wakeup/requeue 状态 |
| `IOZONE_HANG wakeup_running` | 状态仍为 Running，记录 pending wakeup，等待切出边界处理 |
| `IOZONE_HANG wakeup_enqueue` | 阻塞任务已转成 Ready，准备执行全局入队 CPU 选择 |

判断 wakeup 是否丢失时，应检查一次 ENTER 后是否到达 on_cpu、running 或 enqueue
三类终点之一，并结合后续 `ready_queued/on_cpu` 状态。

## 12. 锁、内存分配器和损坏检测

### 12.1 Generic SpinMutex panic

```text
SpinMutex: deadlock detected after 0x1000000 retries ...
owner_hart=... owner=file:line waiter_hart=... waiter=file:line
```

- `owner` 是当前持锁方的获取位置；
- `waiter` 是最近写入共享诊断槽的等待方，多个 CPU 等待时可能被覆盖；
- 固定 retry 次数只能说明等待很久，不自动证明存在锁顺序环。

### 12.2 FrameAllocator panic

```text
FrameAllocator: deadlock detected ... owner_hart=... owner_line=...
```

表示全局物理 frame allocator 锁长时间未释放。结合 active syscall、fork COW、OOM
和 heap growth 状态判断持锁路径。

### 12.3 KernelHeapAllocator timeout

```text
KernelHeapAllocator lock timeout: ... owner_op=... owner_ptr=...
owner_size=... owner_align=...
```

`owner_op`：

| 值 | 操作 |
| ---: | --- |
| 0 | none |
| 1 | alloc |
| 2 | dealloc |
| 3 | grow/add extent |
| 4 | stats |
| 5 | init |

该锁使用硬件时间 2 秒超时，而不是通用 SpinMutex retry 次数。

### 12.4 Buddy allocator panic

- `buddy double free`：同一 block 已经存在于对应 free list，再次释放。
- `buddy free-list cycle/corruption`：遍历节点数超过该层理论上限，说明链表有环或元数据损坏。

### 12.5 Run queue corruption

`RUN_QUEUE_CORRUPT` 检查：

```text
len <= capacity
first_len + second_len == len
```

失败会先直接打印再 panic。`first_ptr/second_ptr` 用于判断 VecDeque backing buffer 或
head/tail 元数据是否被覆盖。

### 12.6 Scheduler identity corruption

`SCHED_IDENTITY_CORRUPTION` 表示：

- hart/TP 报告的 CPU 与调度循环期望 CPU 不同；或
- 当前 SP 所在 scheduler stack 属于另一个 CPU。

字段 `boundary` 是 `run_tasks()` 内的身份检查点，不是 scheduler phase：

- 0：进入 `run_tasks()`；
- 1：每轮禁用 IRQ 后；
- 2：`check_timers()` 返回后。

## 13. 其他直接标签

| 标签 | 含义 |
| --- | --- |
| `SCHEDULER_CPU_STALLED_VISIBLE` | 完整快照发现 `stalled_mask != 0`，列出 heartbeat、phase、SP、RA 和 idle context |
| `SCHED_DIAG_BUFFER_BUSY_VISIBLE` | 当前 CPU 的静态 TaskStateStats 缓冲区发生意外重入/未释放 |
| `EXECVE_STALL` | idle stall 分类中发现 active syscall 221，同时打印 frame allocator 状态 |
| `SYSCALL_STALL_VISIBLE` | active syscall 已经历 500 或 5000 的倍数个 timer tick |
| `SIGRETURN_STATUS_SANITIZED` | sigreturn 用户状态寄存器包含不允许恢复的位，内核已清洗；RV/LA 都有对应输出 |
| `LA64_SPURIOUS_TRAP` | LoongArch 收到当前无法分类的空/spurious trap，输出 estat/era/badv |
| `MEMDEBUG` | 周期性内存、frame、page cache、swap 和 inode 保留量统计 |
| `OOM` | 分配失败时的一次性保留对象、page cache、frame、heap、task、fd、mount 等全量快照 |

### 13.1 OOM/MEMDEBUG 字段组

OOM 快照较长，以下是每组输出的用途：

| 组名 | 主要字段和用途 |
| --- | --- |
| `kernel_heap_alloc failed` | 本次申请 size/alignment、buddy 向上取整大小、heap total/free |
| `heap_bucket` | 各尺寸桶当前请求字节、rounded 字节、存活 allocation、累计 alloc/free；用于找泄漏尺寸 |
| `heap` | 用户请求量、allocator 实际占用、free、total |
| `heap_growth` | 动态扩容是否启用、grown bytes、extent 次数、失败次数/原因、limit |
| `frames` | frame alloc/free 累计数、used/free、fresh/recycled、总页数 |
| `page_cache` | 当前页、dirty 页、磁盘页、tmpfs/swap、各文件系统分类、LRU 元数据 |
| `page_cache_atomic` | 无需 page-cache 锁的分类页数及 insert/remove 累计数 |
| `tmpfs_inode` | tmpfs inode 创建/销毁、当前类型分布、xattr 和 symlink 保留字节 |
| `process_mem` | 进程 VM area、各 area 类型的 resident frame 数和最大持有进程 |
| `process_refs` | fd slots、open files、child refs、最大强引用和对应 PID |
| `task_retention` | task slots、zombie slots、kernel stack/task 强引用、ready/current/timer task |
| `task_lifecycle` | TCB created/dropped/live delta、deferred exited task 数 |
| `task_ids` / `ids` | kernel-stack ID、PID handle 和 raw PID 的当前/累计/回收状态 |
| `process_registry` | registry live/dead/hidden 进程、隐藏 zombie/task/fd/child refs、锁状态 |
| `tid2task` | TID weak map 中 entries/live/dead 和锁状态 |
| `futex` | futex queue 和 waiter 数 |
| `pipe` | pipe buffer/page 创建、销毁、当前、峰值和保留字节 |
| `dcache` | dentry 数、pinned/LRU/tmp/LTP tmp 路径数及路径字符串字节 |
| `new_mount` | fs_context、PID 关联、mount attrs 和锁状态 |
| `fs_retention` | filesystem/superblock 数以及 super table 锁状态 |
| `inode_holes` | punch-hole 元数据保留页数 |
| `lwext4_alloc` | C lwext4 allocator 当前/峰值 user/actual bytes 和 alloc/free delta |
| `writeback` | pending file 数；`queue_lock_busy=true` 表示无法采样，不是 0 |
| `swap` | enabled、slot used/free/total、alloc/free 次数 |

常用不变量：

```text
task_live_delta       = task_created - task_dropped
process live_delta    = process_created - process_dropped
frame allocated_delta = frame_alloc_count - frame_free_count
alloc/free delta 持续增长且 workload 已退出，通常表示资源保留或泄漏
```

`MEMDEBUG UserMapArea dropped with remaining frames` 表示 area Drop 时仍带 resident
frames；这可能是正常由 Drop 统一释放，也可能帮助定位资源回收过晚。应结合相同类型 area
和 frame delta 是否最终回落判断。

### 13.2 LoongArch 专用标签

这些标签主要使用 `warn!`，`LOG=OFF` 时通常不可见：

| 标签 | 含义 |
| --- | --- |
| `la64 rq add reject` | task 状态不是 Ready，拒绝入 run queue |
| `la64 rq add mark-ready failed` | `ready_queued/on_cpu` 所有权检查失败，未重复入队 |
| `la64 rq add ok` | task 成功进入指定 CPU 队列 |
| `la64 rq fetch` | task 从队列取出并完成 claim |
| `la64 sched skip non-ready` | fetch 后发现 task 已不再 Ready，调度器跳过 |
| `la64 sched cpu=... switch` | 即将切换到任务，列出 ERA、SP、RET 和 TID |
| `la64 fork prepared child` | child TrapFrame/kernel stack 准备完成 |
| `la64 fork queued child` | child 即将进入 scheduler queue |
| `la64 block enter` | blocking path 入口状态 |
| `la64 block cancel zombie` | task 已 zombie，取消阻塞 |
| `la64 block clear stale zombie flag` | 清理与实际状态不一致的辅助 zombie 标志 |
| `la64 block cancel pending_wakeup` | block 前已收到 wakeup，取消真正切出 |
| `la64 block switch out` | 准备从任务栈切回 idle scheduler |
| `la64 block returned` | 被唤醒后 block 调用返回 |
| `la64 wait4 enter/block/WNOHANG` | wait4 的查询、阻塞和非阻塞返回边界 |
| `la64 setpgid enter/done` | setpgid 参数解析和完成边界 |
| `LA64_SPURIOUS_TRAP` | 无法归入正常处理分支的 LoongArch trap，记录 ESTAT/ERA/BADV |

### 13.3 信号返回标签

`SIGRETURN_STATUS_SANITIZED` 的字段：

- `arch`：`riscv64` 或 `loongarch64`；
- `pid`：执行 sigreturn 的进程；
- `raw`：用户 signal frame 请求恢复的原状态；
- `sanitized`：内核清除特权/非法位后的实际状态。

偶尔出现表示用户 frame 带有不能直接恢复的状态位；持续出现且伴随错误 PC/SP 时再检查
signal frame ABI 或上下文被覆盖。

## 14. `/proc/kairix_perf`

可以读取：

```sh
cat /proc/kairix_perf
```

它提供不依赖 stall watchdog 的当前快照，包括：

- task 创建/销毁/live delta；
- deferred exited task 数；
- processor 当前任务数和锁忙数；
- remote enqueue、steal、ready queue、online mask；
- TaskStateStats 的状态机异常计数；
- page cache 分类和累计插入/移除；
- page cache、lwext4 lock 状态；
- ext4 flush、VirtIO block I/O 进度；
- writeback pending 文件数。

读取该文件本身也是非原子多子系统采样，应按第 1.4 节解释。

## 15. 推荐判读顺序

看到新的 stall 日志时，按以下顺序检查：

1. 看 `online_mask`，确认实际在线 CPU。
2. 看 `stalled_mask` 和 scheduler heartbeat，区分真正 scheduler 心跳停止与普通 idle。
3. 查本页 phase 表，注意 140 的复用和 160–163 的汇编含义。
4. 看 `current_samples`、`active_samples` 和 syscall stage。
5. 检查三个任务状态异常字段：`ready_unowned`、`running_not_on_cpu`、
   `blocked_queued`。
6. 对照 `ready_tasks` 与 `physical_ready_tasks`。
7. 检查 run queue、timer、page cache、lwext4、ext4 flush、block I/O 和 frame allocator
   是否有锁所有者或 active phase。
8. 比较至少两份快照：phase、计数和进度字段是否全部不变。
9. 只有“同一进度长期不变 + 存在等待者/未完成工作 + 心跳或业务计数不前进”时，
   才把它定性为死锁或 CPU 失活。

## 16. 维护要求

以后新增或修改 phase 时，应同步更新本文，并尽量避免复用编号。新增进度结构时至少
记录：

- phase/op 数值到代码位置的映射；
- `active=false` 时字段是否保留历史值；
- 哪些字段是累计计数，哪些是当前值；
- `None/usize::MAX/0` 的哨兵语义；
- 日志使用 `println!` 还是受 LOG 级别控制。
