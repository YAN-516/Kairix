# Kairix Trap 处理流程

本文档根据当前仓库中的实际代码，梳理 Kairix 在 RISC-V64 与 LoongArch64 上的 trap 入口、上下文保存、架构解码、内核分发、调度以及返回流程。

这里的 trap 泛指：

- 系统调用；
- 同步异常，例如缺页、非法指令和断点；
- 时钟中断；
- 核间中断（IPI）；
- 外部设备中断。

相关代码主要位于：

- `polyhal/polyhal-trap/src/trap/`：架构 trap 入口、现场保存和硬件原因解码；
- `polyhal/polyhal-trap/src/trapframe/`：各架构的 `TrapFrame`；
- `os/src/main.rs::kernel_interrupt`：操作系统统一 trap 分发；
- `os/src/trap/mod.rs`：用户缺页处理及 trap 诊断；
- `os/src/task/mod.rs`：用户返回、任务挂起和抢占；
- `os/src/task/processor.rs`：调度器和 KContext 切换；
- `polyhal/polyhal/src/components/kcontext/`：各架构的 `KContext`。

仓库中已有一张高层示意图：[Trap处理流程.svg](./Trap处理流程.svg)。本文档重点补充实际控制流、两层上下文以及架构差异。

## 1. 总体设计

当前内核的 trap 模型可以概括为：

> 每任务 TrapFrame + 每任务内核续体 KContext + 架构层解码 + OS 统一分发；用户态可抢占，内核态不可抢占。

用户态 trap 的实际主线如下：

```text
调度器
  │ context_switch(idle → task)
  ▼
task_entry()
  │ prepare_user_return()
  ▼
run_user_task(TrapFrame)
  │ user_restore()
  ▼
sret / ertn
  │
  │ 用户态发生 syscall / fault / timer / IPI
  ▼
架构 trap vector
  │ 保存用户现场到当前任务 TrapFrame
  │ 恢复该任务原先挂起的内核栈和内核寄存器
  ▼
返回 run_user_task()
  │ 读取硬件 trap 原因并生成 TrapType
  ▼
kernel_interrupt()
  │ syscall / page fault / timer / signal / schedule
  ▼
返回 run_user_task()，再返回 task_entry()
  │ 下一轮 prepare_user_return()
  ▼
user_restore() → sret / ertn → 用户态
```

内核态 trap 不需要恢复某个用户任务的内核等待点，而是在当前内核栈上创建临时 TrapFrame：

```text
被打断的内核代码
  │ trap
  ▼
架构 trap vector
  │ 在当前内核栈上分配临时 TrapFrame
  │ 保存内核寄存器、PC 和状态
  ▼
架构 trap 解码器
  │ IPI 快速处理，或调用 kernel_interrupt()
  ▼
恢复临时 TrapFrame
  │
  ▼
sret / ertn → 继续原来的内核代码
```

## 2. TrapFrame 与 KContext

内核中有两类容易混淆的上下文。

| 项目 | TrapFrame | KContext |
| --- | --- | --- |
| 描述对象 | 用户程序或异常现场 | 任务在内核中的执行续体 |
| 产生时机 | syscall、异常、中断 | `context_switch()` |
| 栈指针含义 | 用户 `sp`（内核 trap 时是临时内核现场） | 内核 `sp` |
| PC 含义 | `sepc` / `ERA` | 内核返回地址 `ra` / `kpc` |
| 保存范围 | 通用寄存器、状态、PC、用户 FP/向量状态 | ABI callee-saved 寄存器、内核栈和返回地址 |
| 典型恢复指令 | `sret` / `ertn` | 普通 `ret` |
| 是否切换特权级 | 可以 | 不会 |
| 主要修改者 | syscall、signal、page fault、trap 返回代码 | 调度器和上下文切换代码 |

当前长期用户 TrapFrame 位于：

```text
TaskControlBlock
└── TaskControlBlockInner
    ├── trap_cx: TrapFrame
    └── task_cx: KContext
```

### 2.1 TrapFrame

RISC-V64 TrapFrame 保存：

```text
x0-x31
sstatus
sepc
f0-f31
fcsr
```

LoongArch64 TrapFrame 保存：

```text
r0-r31
PRMD
ERA
vr0-vr31（完整 LSX 128 位状态）
FCC
FCSR
```

对用户任务而言，TrapFrame 中的 PC、SP、参数和返回值直接决定下一次返回用户态后的执行状态。信号处理也是通过把 TrapFrame 的 PC 改成 signal handler、把 SP 改成 signal frame 来实现的。

### 2.2 KContext

KContext 只保存恢复内核函数调用链所需的状态：

- 内核栈指针；
- 内核返回地址；
- ABI 规定的 callee-saved 寄存器；
- 架构定义的少量附加状态。

KContext 不需要像 TrapFrame 一样保存所有 caller-saved 寄存器，因为 `context_switch()` 本身是一个函数调用边界，调用者本来就不能假设 `a*`、`t*` 等 caller-saved 寄存器在函数返回后保持不变。

### 2.3 两者如何配合

用户态 timer 抢占会依次保存两层状态：

```text
用户态运行
  │ timer trap
  ▼
TrapFrame 保存用户寄存器、用户 PC、用户 SP
  │ kernel_interrupt → preempt_current_and_run_next
  ▼
KContext 保存 trap handler 当前使用的内核栈和内核返回点
  │ context_switch(task → idle)
  ▼
调度器运行其他任务
```

任务以后重新运行时顺序相反：

```text
恢复 KContext
  │ 从 schedule() 中的 context_switch() 之后继续
  ▼
继续完成原来的 trap handler
  │
  ▼
恢复 TrapFrame
  │
  ▼
sret / ertn → 用户态
```

## 3. 任务如何第一次进入用户态

新任务初始化时会分别准备两份上下文：

```text
KContext.ksp = 任务内核栈顶
KContext.kpc = task_entry

TrapFrame.SP = 初始用户栈顶
TrapFrame.PC = ELF entry point
```

调度器选择任务后，通过 `context_switch(idle → task)` 恢复任务 KContext。新任务第一次恢复时进入 `task_entry()`。

`task_entry()` 的主要循环是：

1. 激活进程地址空间；
2. 检查任务、进程的 zombie/stop 状态；
3. 调用 `prepare_user_return()`；
4. 调用 `run_user_task(&mut TrapFrame)`；
5. 用户发生 trap 后，`run_user_task()` 返回，进入下一轮。

## 4. 什么是“恢复内核续体”

用户态运行期间，负责把任务送入用户态的内核调用链仍保留在任务自己的内核栈上：

```text
task_entry()
└── run_user_task(ctx)
    └── user_restore(ctx)
        └── sret / ertn → 用户态
```

进入用户态前，`user_restore()` 会保存内核 SP、RA、callee-saved 寄存器以及必要的 `gp/tp` 状态。用户 trap 后，`uservec/user_vec` 保存用户现场，再恢复这些内核状态并执行普通 `ret`。

这个 `ret` 使原来的 `user_restore()` 调用表现为“终于返回了”，程序随后执行：

```rust
user_restore(context);
kernel_callback(context); // RISC-V64
```

LoongArch64 对应调用 `loongarch64_trap_handler(context)`。

因此，“恢复内核续体”不是恢复一段被用户打断的内核代码，而是恢复：

> 当初负责进入用户态，并等待用户下一次 trap 回来的任务内核执行现场。

调度器拥有另一份独立的 idle KContext。只有 trap handler 决定调度时，任务才会通过 `schedule()` 切回调度器。

## 5. RISC-V64 Trap 流程

### 5.1 初始化

`polyhal-trap` 初始化时：

1. 把 `stvec` 设置为 `kernelvec`；
2. 使用 Direct 模式；
3. 开启 TLB shootdown software IPI。

所有 S 态 trap 首先进入 `kernelvec`。

### 5.2 用户返回：user_restore

RISC-V64 `user_restore()` 执行：

1. 在任务内核栈保存内核 callee-saved 寄存器、`gp/tp/ra`；
2. 把保存后的内核 SP 写入 `TrapFrame.x[0]`；
3. 把 TrapFrame 地址写入 `sscratch`；
4. 让 `sp` 指向 TrapFrame；
5. 恢复用户浮点和通用寄存器；
6. 恢复 `sstatus/sepc`；
7. 执行 `sret`。

此时 `sscratch != 0`，它同时充当“当前正在运行用户态”的入口标记和 TrapFrame 指针。

### 5.3 kernelvec 判断来源

`kernelvec` 首条关键操作是交换 `sp` 与 `sscratch`：

```asm
csrrw sp, sscratch, sp
```

交换后：

- `sp != 0`：原来的 `sscratch` 保存了 TrapFrame 地址，说明来自用户态，跳转到 `uservec`；
- `sp == 0`：说明来自内核态，从 `sscratch` 取回原内核 SP，进入内核 trap 路径。

Rust 层会再次使用 TrapFrame 中保存的 `sstatus.SPP` 判断 trap 来源，避免只依赖入口瞬间的 scratch CSR 状态。

### 5.4 uservec

`uservec` 只负责上下文切换，不负责解释 syscall 或异常原因：

1. 保存用户通用寄存器到 TrapFrame；
2. 从 `sscratch` 取出并保存用户 SP；
3. 保存 `sstatus/sepc`；
4. 清零 `sscratch`，建立“正在内核态”的入口约定；
5. 保存用户 FP 寄存器和 FCSR；
6. 从 `TrapFrame.x[0]` 恢复任务内核 SP；
7. 恢复进入用户态前保存的内核寄存器；
8. 执行普通 `ret`，回到 `run_user_task()`。

### 5.5 kernel_callback

RISC-V64 `kernel_callback()` 是架构 trap 解码器。它读取：

- `scause`：trap 类型；
- `stval`：缺页地址或异常附加信息；
- TrapFrame 中的 `sstatus/sepc`：来源和返回地址。

主要映射如下：

| RISC-V 原因 | TrapType |
| --- | --- |
| UserEnvCall | `SysCall` |
| SupervisorTimer | `Timer` |
| SupervisorSoft | IPI 快速路径或 `Reschedule` |
| Load/Store/InstructionPageFault | 对应 PageFault |
| IllegalInstruction | `IllegalInstruction` |
| Breakpoint | `Breakpoint` |
| SupervisorExternal | `SupervisorExternal` |

普通 trap 随后通过 `_interrupt_for_arch()` 调用 OS 的 `kernel_interrupt()`。

Software IPI 是特殊路径：TLB shootdown、内存屏障等无锁工作可以直接在架构层完成；只有打断用户态且携带 reschedule/timer-recovery 原因时，才生成 `TrapType::Reschedule` 进入 OS 调度路径。

## 6. LoongArch64 Trap 流程

### 6.1 初始化

LoongArch64 初始化时：

1. 配置独立的 TLB refill 入口；
2. 设置 `ECFG.VS = 0`；
3. 把 `EENTRY` 设置为 `trap_vector_base`；
4. 开启 TLB shootdown IPI。

### 6.2 Scratch CSR

LoongArch64 使用三个 KSAVE CSR：

| CSR | 用途 |
| --- | --- |
| `KSAVE_KSP` | 保存任务内核栈 |
| `KSAVE_CTX` | 保存 TrapFrame 地址 |
| `KSAVE_USP` | 暂存用户栈或入口 SP |

### 6.3 user_restore

LoongArch64 `user_restore()`：

1. 在任务内核栈保存内核 `ra/tp/s*` 等寄存器；
2. 把内核 SP 写入 `KSAVE_KSP`；
3. 把 TrapFrame 地址写入 `KSAVE_CTX`；
4. 从 TrapFrame 恢复 `PRMD/ERA`、通用寄存器、LSX、FCC、FCSR；
5. 执行 `ertn`。

### 6.4 trap_vector_base 与 user_vec

trap 进入后，入口读取 `PRMD.PPLV`：

- `PPLV != 0`：来自用户态，跳转到 `user_vec`；
- `PPLV == 0`：来自内核态，在当前内核栈创建临时 TrapFrame。

`user_vec` 从 `KSAVE_CTX` 找到当前任务 TrapFrame，保存完整用户现场，再从 `KSAVE_KSP` 恢复内核续体并 `ret` 回 `run_user_task()`。

### 6.5 loongarch64_trap_handler

LoongArch64 使用 `ESTAT/BADV/PRMD/ERA` 解码 trap。主要映射包括：

| LoongArch 原因 | TrapType / 行为 |
| --- | --- |
| Syscall | `SysCall` |
| TIMER IRQ | 清 timer pending，生成 `Timer` |
| IPI IRQ | IPI 快速路径或 `Reschedule` |
| Load/Store/Fetch PageFault | 对应 PageFault |
| PageModifyFault | `StorePageFault` |
| AddressNotAligned | 模拟未对齐访存或转成 PageFault |
| InstructionNotExist/PrivilegeIllegal | `IllegalInstruction` |
| FloatingPointUnavailable | 开启 FPU，返回 `Handled` 重试 |
| FPE ECODE | `FloatingPointException` |

未对齐访问辅助代码支持 exception-table fixup。被标注的内核访存发生异常时，可以把 `ERA` 改到 fixup 地址并继续，而不是直接 panic。

## 7. kernel_interrupt：OS 统一分发

`os/src/main.rs::kernel_interrupt()` 由 `#[polyhal::arch_interrupt]` 导出为 `_interrupt_for_arch`，是两种架构共用的 OS trap handler。

进入时首先保存 `trapped_from_user`，因为后续 syscall、signal 或异常处理可能修改 TrapFrame 中的特权状态。用户态来源还会：

1. 记录用户执行时间和用户 PC/SP/RA；
2. 检查任务与 CPU owner 是否一致；
3. 撤销当前 CPU 的 user-TLB-active 标记；
4. RISC-V64 设置 SUM，允许内核访问用户映射。

### 7.1 SysCall

系统调用分支：

1. 把保存的 PC 前移一条 syscall 指令；
2. 从 TrapFrame 读取 syscall ID 和六个参数；
3. 调用 `syscall(id, args)`；
4. 把成功值或负 errno 写入返回寄存器。

ABI 映射：

| 架构 | syscall ID | 参数 | 返回值 |
| --- | --- | --- | --- |
| RISC-V64 | `a7/x17` | `a0-a5` | `a0/x10` |
| LoongArch64 | `a7/r11` | `a0-a5/r4-r9` | `a0/r4` |

成功的 `execve` 和 `rt_sigreturn` 可以替换完整 TrapFrame，因此通用返回逻辑不会再次覆盖它们设置的寄存器。

### 7.2 Page Fault

用户态缺页进入 `os/src/trap/mod.rs::handle_page_fault()`，根据 trap 类型确定 Read/Write/Execute，然后依次尝试：

1. 文件映射缺页；
2. COW 写缺页；
3. lazy allocation；
4. VMA 权限与 PTE 权限修复；
5. 指令缓存和 TLB 同步。

结果处理：

- `PageFaultError::Normal`：修复完成，重试原指令；
- `BeyondFileSize`：向当前线程投递 `SIGBUS/BUS_ADRERR`；
- 其他失败：投递 `SIGSEGV`，并区分 `SEGV_MAPERR/SEGV_ACCERR`。

内核态 PageFault 不走通用恢复：handler 会打印当前页表、PTE、地址翻译和 I/O 状态，然后 panic。LoongArch exception-table fixup 是少数例外。

### 7.3 Timer

Timer 分支执行：

1. 更新 `/proc/interrupts` 计数；
2. 记录 syscall stall 诊断信息；
3. 请求延迟 timer maintenance；
4. 重新设置约 10ms 的下一次 timer；
5. 仅在 trap 来自用户态时调用 `preempt_current_and_run_next()`。

当前内核不允许 timer 在任意 Rust/C 内核指令处异步切走内核续体。因此：

- 用户态 timer：可以抢占；
- 内核态 timer：只记账和重装，不调度。

### 7.4 IPI

IPI 原因由 `polyhal::multicore::handle_ipi()` 无锁处理，包括：

- TLB shootdown；
- 指令缓存同步；
- 内存屏障；
- reschedule kick；
- timer recovery。

仅当 IPI 打断用户态并要求重新调度时，架构层才把它转换成 `TrapType::Reschedule`。内核态 IPI 即使观察到 reschedule 原因，也不会异步切走当前内核续体。

### 7.5 Illegal Instruction 和 FPE

用户非法指令会记录 PC、映射内容和寄存器窗口，然后投递 `SIGILL`。浮点算术异常投递 `SIGFPE`。

LoongArch 的 `FloatingPointUnavailable` 属于 lazy-enable：handler 开启当前 CPU 的 FPU 后返回 `Handled`，不向用户发送信号，原指令会被重新执行。

### 7.6 Trap 返回前的公共处理

来自用户态的普通 trap 在返回前还会：

1. 检查 task/process 是否 zombie、orphan 或 stopped；
2. 处理 pending signal；
3. 在 syscall 返回路径按需执行少量 writeback/reclaim；
4. 根据退出状态终止当前任务；
5. 调用 `prepare_user_return()`。

内核态 trap 在具体分支完成后会提前返回，不进入用户 signal、zombie 和用户返回准备路径。

## 8. prepare_user_return

`prepare_user_return()` 负责建立最终的用户返回不变量：

1. 处理 group-stop、zombie 和 orphan 状态；
2. 更新 rseq ABI 区域；
3. 清理可能被 syscall/signal 污染的特权状态；
4. 重新启用当前 CPU 的 timer 中断源；
5. 确认本 CPU 已处理最新 TLB generation；
6. 发布当前 CPU 即将执行该用户地址空间；
7. 更新任务用户运行统计。

RISC-V64 强制设置：

```text
SPP  = User
SPIE = 1
SIE  = 0（由 sret 从 SPIE 恢复）
SUM  = 0
MXR  = 0
```

LoongArch64 强制设置：

```text
PRMD.PPLV = 3
PRMD.PIE  = 1
```

## 9. sret 与 ertn

### 9.1 RISC-V64 sret

trap 进入时，硬件保存：

```text
sepc         ← 被打断的 PC
sstatus.SPP  ← trap 前特权级
sstatus.SPIE ← trap 前的 SIE
sstatus.SIE  ← 0
```

`sret` 大致执行：

```text
PC           ← sepc
当前特权级    ← sstatus.SPP
sstatus.SIE  ← sstatus.SPIE
sstatus.SPIE ← 1
```

因此 `sret` 既可以返回用户态，也可以返回内核态，取决于保存的 SPP。

### 9.2 LoongArch64 ertn

普通异常进入时，硬件保存：

```text
ERA       ← 被打断的 PC
PRMD.PPLV ← 异常前的 CRMD.PLV
PRMD.PIE  ← 异常前的 CRMD.IE
CRMD.PLV  ← 0
CRMD.IE   ← 0
```

`ertn` 大致执行：

```text
PC       ← ERA
CRMD.PLV ← PRMD.PPLV
CRMD.IE  ← PRMD.PIE
```

因此 PPLV=3 返回用户态，PPLV=0 返回内核态。

### 9.3 与普通 ret 的区别

```text
ret
└── 根据 ra 恢复普通函数调用链

sret / ertn
├── 恢复异常返回 PC
├── 恢复特权级
├── 恢复中断使能状态
└── 完成 CPU 异常返回状态转换
```

三者都不会自动恢复所有通用寄存器；对应汇编必须先从 TrapFrame 或内核栈恢复寄存器。

## 10. 内核态 Trap 与嵌套中断

### 10.1 RISC-V64

内核态 trap 进入 `kernelvec` 后：

1. 从 `sscratch` 找回被打断的内核 SP；
2. 在当前内核栈分配一个按 16 字节对齐的临时 TrapFrame；
3. 保存通用寄存器、`sstatus/sepc`；
4. 再次清零 `sscratch`，保证嵌套 trap 仍被识别为内核态；
5. 调用 `kernel_callback()`；
6. 恢复现场并 `sret` 回原内核 PC。

RISC-V 内核 trap 入口当前不保存 FP 寄存器，隐含前提是普通内核代码不使用用户浮点状态。

### 10.2 LoongArch64

LoongArch64 根据 `PRMD.PPLV == 0` 识别内核来源，从 `KSAVE_USP` 恢复入口 SP，在当前内核栈创建临时 TrapFrame，处理完成后恢复寄存器并 `ertn`。

### 10.3 内核不可抢占约束

正常内核临界区关闭全局中断。只有显式进入 `InterruptibleKernelSection` 时，才会临时允许有限的中断：

- RISC-V64：timer 与 software IPI；
- LoongArch64：timer 与 IPI。

这种窗口中的嵌套 timer 只能记账和重装 timer，不能调度；IPI 必须保持无锁、可重入，避免在持有内核锁时递归进入复杂 OS 路径。

## 11. 典型流程

### 11.1 普通 syscall

```text
用户 ecall/syscall
  → uservec/user_vec 保存 TrapFrame
  → 恢复 run_user_task 续体
  → 架构解码为 SysCall
  → kernel_interrupt
  → PC 前移
  → syscall(id, args)
  → 返回值写入 a0
  → pending signal / exit 检查
  → task_entry 下一轮
  → prepare_user_return
  → user_restore
  → sret/ertn
```

### 11.2 用户 timer 抢占

```text
用户态
  → timer trap，保存 TrapFrame
  → kernel_interrupt(Timer)
  → 重装 timer
  → preempt_current_and_run_next
  → 保存 KContext
  → 调度器运行其他任务
  → 以后恢复 KContext
  → 完成 timer trap handler
  → 恢复 TrapFrame
  → 返回用户态
```

### 11.3 用户缺页

```text
用户访存
  → Load/Store/InstructionPageFault
  → handle_page_fault
  ├── lazy/COW/file-backed 修复成功 → 重试原指令
  ├── 文件映射越界 → SIGBUS
  └── 无映射/权限错误 → SIGSEGV
```

### 11.4 内核 timer

```text
内核代码
  → timer trap
  → 当前内核栈临时 TrapFrame
  → 记账并重装 timer
  → 不执行 context switch
  → sret/ertn
  → 继续原内核指令
```

### 11.5 TLB shootdown IPI

```text
IPI
  → 架构 trap 解码器
  → acknowledge IPI
  → flush TLB / 同步 I-cache / 更新 generation ack
  ├── 仅 shootdown → Handled，快速返回
  └── 用户态且需要 reschedule → kernel_interrupt(Reschedule)
```

## 12. 当前实现中的注意点

以下内容是对当前代码的静态观察，不代表期望的最终设计。

### 12.1 外部设备中断分发不完整

RISC-V64 能把 Supervisor External Interrupt 解码为 `TrapType::SupervisorExternal`，但 OS `kernel_interrupt()` 当前没有对应处理分支，会落入默认异常处理。

LoongArch64 trap handler 当前只明确处理 TIMER 和 IPI 中断线，其他硬件中断线会 panic；但 timer 初始化代码还会启用 `HWI0/SWI0/SWI1`，需要确认这些中断是否可能真实触发。

### 12.2 Breakpoint PC 可能被重复推进

架构解码层已经推进断点 PC：

- RISC-V64：加 2；
- LoongArch64：加 4。

OS 的 `Breakpoint` 分支又调用 `ctx.syscall_ok()` 再加 4，然后固定执行 syscall 139。若该路径仍会被使用，需要确认最终 PC 加 6/8 是否符合预期。

### 12.3 用户返回准备目前会执行两次

实际用户 trap 控制流是：

```text
kernel_interrupt 返回
  → 架构 trap 解码器返回
  → run_user_task 返回
  → task_entry 下一轮
  → prepare_user_return
  → user_restore
```

但 `kernel_interrupt()` 末尾也会调用一次 `prepare_user_return()`。这与代码中“直接经架构 trap vector 返回用户态”的注释不完全一致，并会在真正返回用户态之前短暂、重复发布 user-active 状态。

### 12.4 TrapFrame 可变引用生命周期过宽

`TaskControlBlockInner::get_trap_cx(&self)` 当前从共享引用构造 `&'static mut TrapFrame`。这使 Rust 无法通过生命周期保证不存在可变别名。更稳妥的长期方案是使用受锁 guard、明确作用域的可变借用，或把裸指针限制在汇编边界。

### 12.5 部分异常缺少严格的内核来源保护

PageFault 分支明确区分用户态和内核态；IllegalInstruction、FloatingPointException 等分支没有完全相同的来源检查。内核自身出现这类异常时，可能错误地尝试向当前用户任务投递信号，而不是立即报告内核异常。

## 13. 调试定位建议

遇到 trap 相关问题时，可以按以下顺序定位：

1. 确认 TrapFrame 中的 PC、SP、状态寄存器和来源位；
2. 确认架构层解码出的 `TrapType`；
3. 确认是否走了 IPI 快速路径；
4. 确认 `kernel_interrupt()` 是否发生了 schedule；
5. 若发生调度，同时检查 TrapFrame 和 KContext，二者缺一不可；
6. 确认 `prepare_user_return()` 恢复了用户特权级、全局中断和 timer mask；
7. 最后检查 `user_restore` 的 scratch CSR、内核 SP 和 TrapFrame 地址是否一致。

可以重点关注已有诊断状态：

- `polyhal::multicore::trap_progress()`；
- `polyhal::multicore::interrupt_state()`；
- `crate::trap::page_fault_progress()`；
- scheduler phase 与 task kernel phase；
- `/proc/interrupts` 和 `/proc/kairix_perf`。

