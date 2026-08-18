# Kairix LoongArch 在 QEMU 上的启动流程

本文整理当前 Kairix 内核在 LoongArch64 QEMU virt 平台上的启动流程。内容以当前
工作区源码为准，覆盖从 `make lkernel`、PolyHAL 早期入口、SMP 拉核，到 Kairix
创建 PID 1 并第一次进入用户态的完整调用链。

需要特别说明：Kairix 是自研 Rust 宏内核，不是 Linux，因此流程中不存在 Linux
的 `start_kernel()`。Kairix 对应的上层内核入口是导出为 `_main_for_arch` 的
`os::main()`。

## 1. 总体调用链

```text
make lkernel
  |
  +-- 编译 loongarch64-unknown-none 内核 ELF
  +-- 复制为 kernel-la
  +-- patch sdcard-la.img
  `-- qemu-system-loongarch64 -kernel kernel-la ...
          |
          v
      _start                          PolyHAL 汇编入口
          |
          v
      rust_tmp_main                   主核早期 Rust 初始化
          |
          +-- clear_bss
          +-- init_dtb_once
          +-- set_local_thread_pointer
          +-- init_cpu
          +-- CtorType::Cpu
          +-- parse_system_info
          +-- CtorType::Platform
          `-- CtorType::HALDriver
                  |
                  v
              call_real_main
                  |
          +-------+--------------------------------+
          |                                        |
          | 主核                                   | 从核
          |                                        |
          +-- 预留所有 per-CPU 区域                +-- _secondary_start
          +-- 分配从核栈                           +-- 从 mailbox 读取 SP
          +-- mailbox 写入口和栈                   +-- _rust_secondary_main
          +-- IPI 拉起从核                         +-- 初始化本核 CPU/trap
          +-- CtorType::KernelService              +-- 等待主核 INIT_DONE
          +-- CtorType::Normal                     `-- _secondary_for_arch
          +-- _main_for_arch                              |
          |                                               v
          v                                           run_tasks
      os::main
          |
          +-- heap/frame/page table
          +-- processor/network/filesystem/swap
          +-- 创建 PID 1 initproc
          +-- 发布 INIT_COMPLETED
          `-- run_tasks
                  |
                  v
              task_entry
                  |
                  v
              user_restore
                  |
                  v
                ertn
                  |
                  v
              用户态 PID 1
```

## 2. 构建与 QEMU 启动参数

顶层 `Makefile` 中，LoongArch 默认配置为：

```make
BOARD  ?= qemu
LA_CPU ?= 12
LA_MEM ?= 36G
```

`make lkernel` 依次执行：

```text
make -C os ARCH=loongarch64 BOARD=qemu build
cp os/target/loongarch64-unknown-none/release/os kernel-la
make -C os ... patch-sdcard
qemu-system-loongarch64 ...
```

实际 QEMU 参数的核心部分是：

```bash
qemu-system-loongarch64 \
    -kernel kernel-la \
    -m 36G \
    -smp 12 \
    -nographic \
    -drive file=sdcard-la.img,if=none,format=raw,id=x0 \
    -device virtio-blk-pci,drive=x0 \
    -device virtio-net-pci,netdev=net0 \
    -netdev user,id=net0 \
    -rtc base=utc
```

QEMU 命令没有显式指定 `-bios` 和 `-machine`，使用 QEMU 的默认 LoongArch 启动
环境。QEMU/其默认启动环境负责装载 `kernel-la` ELF，并把控制权交给 ELF 的
入口 `_start`。

相关源码：

- `Makefile`
- `os/Makefile`

## 3. 内核链接地址与入口

LoongArch 链接脚本 `os/src/linker-loongarch64.ld` 指定：

```ld
ENTRY(_start)
BASE_ADDRESS = 0x9000000080000000;
```

地址关系为：

```text
内核物理基址：       0x0000000080000000
cached DMW 虚拟地址：0x9000000080000000
ELF 入口：           _start
```

链接脚本还在 BSS 区域中预留了启动栈 `bstack..bstack_top`。主核刚进入内核时先
使用这块共享的早期启动栈，之后任务和从核会使用各自的内核栈。

## 4. `_start`：最早期汇编初始化

入口位于：

```text
polyhal/polyhal-boot/src/arch/loongarch64.rs::_start
```

### 4.1 配置 DMW

`_start` 首先配置两个 Direct Mapping Window：

```text
DMW0：0x8000_xxxx_xxxx_xxxx，非缓存，PLV0 可访问
DMW1：0x9000_xxxx_xxxx_xxxx，缓存，  PLV0 可访问
```

DMW 使内核可以在普通多级页表完全建立之前，通过固定高地址直接访问物理内存和
MMIO。

### 4.2 设置 CPU 初始状态

随后设置：

```text
CRMD：PLV0、IE=0、PG=1
PRMD：PIE=0、PWE=0
EUEN：FPE=0、SXE=0、ASXE=0、BTE=0
```

早期阶段先关闭中断、浮点和向量扩展，等进入受控的 Rust 初始化代码后再逐项打开。

### 4.3 设置栈并进入 Rust

汇编入口还会：

1. 向早期串口输出 `P`，表示 DMW/分页状态已经建立。
2. 设置 `$sp = bstack_top`。
3. 从 CPU ID CSR `0x20` 读取核号到 `$a0`。
4. 向串口输出 `J`，表示准备跳转 Rust。
5. 跳转到 `rust_tmp_main(hart_id)`。

```text
_start -> rust_tmp_main(hart_id)
```

## 5. `rust_tmp_main`：主核早期平台初始化

入口位于：

```text
polyhal/polyhal-boot/src/arch/loongarch64.rs::rust_tmp_main
```

执行顺序如下：

```text
clear_bss
  -> init_dtb_once
  -> set_local_thread_pointer
  -> init_cpu
  -> 执行 CtorType::Cpu
  -> parse_system_info
  -> 执行 CtorType::Platform
  -> 执行 CtorType::HALDriver
  -> 设置 LoongArch 早期 INIT_DONE
  -> call_real_main
```

### 5.1 清理 BSS

`clear_bss()` 把 `_sbss.._ebss` 清零，使 Rust 静态变量处于有效初始状态。

QEMU 路径直接使用固定 DTB 地址，因此不需要像 2K1000 路径那样在清 BSS 前复制
DTB。

### 5.2 初始化 DTB 与早期内存区域

QEMU LoongArch 使用固定 DTB 地址：

```text
DTB 物理地址：0x00100000
DTB cached 地址：0x9000000000100000
```

`init_dtb_once()` 会：

1. 验证 FDT magic 和结构。
2. 保存 DTB 地址与大小。
3. 遍历 `/memory` 节点。
4. 把可用物理内存加入 PolyHAL 的 `MEM_AREA`。
5. 从可用范围中排除内核镜像和 DTB 本身。

如果 DTB 初始化失败，当前 LoongArch QEMU 路径会加入兜底内存范围：

```text
0x80000000..0x100000000
```

### 5.3 初始化 per-CPU 区域

per-CPU 区域是每个物理 CPU 各自独立的一份内核局部数据，不是用户 TLS，也不是
CPU 内核栈。

链接时，所有 `#[percpu]` 数据会组成一份 `percpu` section 模板。启动时：

```text
为 CPU N 分配一块物理内存
  -> 把 percpu section 模板复制进去
  -> 转换为 cached DMW 地址
  -> 把当前 CPU 的 per-CPU 基址写入 $r21
```

内核访问 per-CPU 变量时，通过：

```text
$r21 + 变量在 percpu section 中的偏移
```

定位当前 CPU 自己的副本。

主核稍后还会在拉起从核前，为所有 CPU 预留 per-CPU 区域，避免从核并发启动时与
主核的早期栈分配发生竞争。

### 5.4 `init_cpu()`

当前 `init_cpu()` 做两件事：

```rust
euen::set_fpe(true); // 允许浮点指令
euen::set_sxe(true); // 允许 128-bit LSX 指令
```

`EUEN` 是每个 CPU 独立的 CSR，因此主核和每个从核都必须分别执行。打开 LSX 后，
trap 和上下文切换代码必须保存、恢复完整的 128-bit 向量寄存器；当前 LoongArch
`TrapFrame` 已为此保留完整状态。

## 6. PolyHAL 构造器机制

PolyHAL 使用静态构造器表组织分阶段初始化。注册方式例如：

```rust
ph_ctor!(TRAP_INIT, CtorType::Cpu, init);
```

宏在编译时生成一个静态 `PHInitWrap`：

```rust
#[used(linker)]
#[link_section = "ph_init"]
static TRAP_INIT: PHInitWrap = PHInitWrap {
    priority: CtorType::Cpu,
    func: init,
};
```

链接器把各模块的注册项合并到 `ph_init` section。运行时，`ph_init_iter()` 通过
`__start_ph_init..__stop_ph_init` 遍历这张静态表，并按 `CtorType` 过滤：

```rust
ph_init_iter(CtorType::Cpu).for_each(|x| (x.func)());
```

等价于：

```rust
for constructor in ph_init_section {
    if constructor.priority == CtorType::Cpu {
        (constructor.func)();
    }
}
```

这不是运行时动态注册；“注册”发生在源码宏展开和最终链接阶段。

### 6.1 当前 LoongArch 各构造器阶段

| 阶段 | 执行范围 | 当前 LoongArch QEMU 的主要效果 |
| --- | --- | --- |
| `Cpu` | 每个 CPU | 执行 `TRAP_INIT -> trap::init()` |
| `Platform` | 仅主核一次 | 执行 `ARCH_INIT_TIMER -> timer::init()` |
| `HALDriver` | 仅主核一次 | 当前配置没有有效注册项，空遍历 |
| `KernelService` | 仅主核一次 | 当前源码没有注册项，空遍历 |
| `Normal` | 仅主核一次 | 当前源码没有注册项，空遍历 |

#### `CtorType::Cpu`

当前最关键的注册项是：

```rust
ph_ctor!(TRAP_INIT, CtorType::Cpu, init);
```

LoongArch `trap::init()` 会：

```text
设置 4 KiB TLB 页大小
  -> 配置 PWCL/PWCH 硬件页表遍历
  -> 设置 TLB refill 入口
  -> EENTRY = trap_vector_base
  -> 启用本核 IPI/TLB shootdown
```

这些 CSR 都是每核状态，所以每个 CPU 必须执行。

#### `parse_system_info()`

`parse_system_info()` 位于 `Cpu` 和 `Platform` 构造器之间，它不是构造器，而是一个
显式调用。它会：

1. 打印 PolyHAL banner 和架构信息。
2. 读取 DTB 的启动核 ID。
3. 统计 `/cpus/cpu` 节点数量。
4. 设置全局 `CPU_NUM`。
5. 打印 `/chosen/bootargs`。
6. 打印 DTB 报告的原始内存范围。
7. 打印排除内核、DTB 和早期占用后实际可分配的 `MEM_AREA`。

其中最关键的状态修改是设置 `CPU_NUM`，后面的 `call_real_main()` 根据它决定拉起
多少个从核。设备树失败时，CPU 数量退化为 1。

#### `CtorType::Platform`

当前注册项是：

```rust
ph_ctor!(ARCH_INIT_TIMER, CtorType::Platform, init);
```

LoongArch timer 初始化会：

```text
停止硬件定时器
  -> 设置周期模式
  -> 清 pending timer interrupt
  -> 在 ECFG 中允许 TIMER/SWI/HWI0/IPI
  -> 按 1000 Hz 装载 1 ms 周期
```

这里设置的是具体中断线掩码，不等于立即打开 `CRMD.IE` 全局中断。

#### `CtorType::HALDriver`

它是 PolyHAL 为串口、中断控制器、HAL logger 等基础驱动保留的初始化阶段。

当前内核依赖 PolyHAL 时启用了 `boot`、`trap`，但关闭了默认 feature，也没有启用
PolyHAL 的 `logger` feature。因此 `CONSOLE_INIT` 不会编译进当前 LoongArch 内核；
其他现有 HALDriver 注册项属于其他架构。这个阶段当前是空遍历。

VirtIO block、VirtIO net 和 PCI 不在这里初始化，它们稍后由 Kairix 的
`net::init()`、`fs::init()` 初始化。

#### `CtorType::KernelService` 与 `CtorType::Normal`

这两个阶段在 PolyHAL 拉起从核后、进入 Kairix 前由主核执行：

```rust
ph_init_iter(CtorType::KernelService).for_each(|x| (x.func)());
ph_init_iter(CtorType::Normal).for_each(|x| (x.func)());
```

当前源码没有相应注册项，所以都是空遍历。它们仍构成一个扩展点和同步边界：主核
执行完这两个阶段后才发布 `INIT_DONE`，从核随后才能进入 Kairix。

## 7. `call_real_main` 与 SMP 拉核

`call_real_main(hartid)` 通过原子变量 `IS_BOOT` 判断当前是不是第一个到达的 CPU。

### 7.1 主核路径

主核读取 `parse_system_info()` 设置的 `CPU_NUM`，并限制到 PolyHAL 的
`MAX_CPU_NUM`。当前 LoongArch 上限为 12。

然后执行：

```text
为 0..CPU_NUM 预留全部 per-CPU 区域
  -> 为每个从核分配 4 MiB 启动栈
  -> mailbox 发送 _secondary_start
  -> mailbox 发送从核栈顶
  -> 发送启动 IPI
```

### 7.2 LoongArch Mailbox

Mailbox 是 LoongArch IOCSR 提供的每核硬件消息寄存器，不是内存消息队列，也不是
进程 IPC。

当前启动协议为：

```rust
csr_mail_send(_secondary_start, hart_id, 0); // MAIL_BUF0：入口
csr_mail_send(stack_top,       hart_id, 1); // MAIL_BUF1：栈顶
send_ipi_single(hart_id, 1);               // action bit 0：启动通知
```

Mailbox 负责传数据，IPI 负责通知或唤醒：

```text
CPU0                                 CPU N
 |                                     |
 +-- MAIL_BUF0 = _secondary_start ---->|
 +-- MAIL_BUF1 = stack_top ------------>|
 `-- startup IPI ---------------------->|
                                       `-- 进入 _secondary_start
```

从核固件/QEMU 等待路径使用 mailbox 0 的入口完成跳转。进入 `_secondary_start` 后，
从核代码使用 `iocsrrd.d` 从本核 `MAIL_BUF1` 读取 `$sp`。

运行时 IPI 使用 `1 << 1`，与启动 IPI 的 `1 << 0` 分开，避免 TLB shootdown 等路径
误清除启动事件。

### 7.3 从核路径

从核调用链为：

```text
固件/QEMU 从核等待路径
  -> _secondary_start
  -> DMW、CRMD、PRMD、EUEN 初始配置
  -> 从 MAIL_BUF1 读取 SP
  -> _rust_secondary_main
  -> 等待 LoongArch 早期 INIT_DONE
  -> set_local_thread_pointer
  -> init_cpu
  -> CtorType::Cpu
  -> call_real_main
  -> 等待 PolyHAL INIT_DONE
  -> _secondary_for_arch
```

## 8. 三层主从核同步

当前流程存在三层含义不同的同步标志：

| 同步标志 | 发布位置 | 从核等待目的 |
| --- | --- | --- |
| LoongArch boot `INIT_DONE` | `rust_tmp_main()` 末尾 | DTB、CPU、平台/HAL 早期初始化完成 |
| `call_real_main::INIT_DONE` | KernelService/Normal 之后 | PolyHAL 所有主核构造器完成 |
| Kairix `INIT_COMPLETED` | `os::main()` 创建 PID 1 后 | 堆、内存、网络、文件系统、调度器等完成 |

最终，从核只有通过第三层同步后才进入自己的 `run_tasks()`。

## 9. Kairix 主核初始化

主核由 PolyHAL 调用导出符号 `_main_for_arch`，对应：

```rust
#[polyhal::arch_entry]
fn main(id: usize, first: bool) -> bool
```

预期主核走 `first == true` 分支，初始化顺序是：

```text
logging::init
  -> heap_allocator::init_heap
  -> frame_allocator::init_frame_allocator
  -> heap_allocator::enable_heap_growth
  -> common::init(PageAllocImpl)
  -> init_trap
  -> mm::init
  -> init_processors
  -> net::init
  -> fs::init
  -> embedded::install_runtime_files
  -> mm::swap::init
  -> task::add_initproc
  -> set_init_completed
  -> set_next_trigger
  -> task::run_tasks
```

### 9.1 堆、页帧与页表

启动初期先使用静态 bootstrap heap。页帧分配器接管 DTB 剩余内存时，会冻结
PolyHAL 早期分配器，确保早期分配的从核栈和 per-CPU 区域不会与普通页帧重叠。

`mm::init()` 创建并激活内核页表。

### 9.2 Processor 与网络

`init_processors()` 为所有支持的 CPU 建立调度器 `Processor` 对象。

QEMU 路径的 `net::init()` 会：

1. 注册 loopback。
2. 扫描 PCI VirtIO-net，失败时尝试 VirtIO MMIO。
3. 配置 QEMU user network 地址 `10.0.2.15`。
4. 配置网关 `10.0.2.2` 和 DNS `10.0.2.3`。

### 9.3 根文件系统

`fs::init()` 首先注册 ext2/ext3/ext4、FAT32、devfs、procfs、tmpfs 等文件系统。

LoongArch QEMU 优先把 VirtIO PCI block 设备作为 ext4 根文件系统。块设备第一次被
访问时会懒创建 `VirtIOBlock`：

```text
固定 cached DTB 地址 0x9000000000100000
  -> 查找 pci-host-ecam-generic
  -> 枚举 PCI bus 0
  -> 找到 virtio-blk-pci
  -> feature/virtqueue 初始化
  -> 挂载 ext4 /
```

如果 VirtIO ext4 挂载失败，则尝试 FDT/initrd，最后退化到 tmpfs 根文件系统。

根目录建立后继续挂载 `/dev`、`/dev/shm`、`/proc`、`/tmp`、`/sys` 等特殊文件
系统，并安装内嵌运行时文件。

## 10. 创建 PID 1 并首次进入用户态

`initproc` ELF 通过 `include_bytes!` 编译进内核，不依赖启动时从磁盘读取：

```text
user/target/loongarch64-unknown-none/release/initproc
```

`task::add_initproc()` 最终触发：

```text
ProcessControlBlock::new(initproc ELF)
  -> 解析 ELF program headers
  -> 建立用户地址空间
  -> 映射程序、用户栈和 TrapFrame
  -> TrapFrame.ERA = ELF entry
  -> TrapFrame.SP = initial user stack
  -> KContext.KPC = task_entry
  -> 把 PID 1 主线程加入 ready queue
```

主核完成上述工作后调用 `set_init_completed()`，释放正在等待的从核。所有 CPU 随后
进入 `run_tasks()`。

调度器第一次选中 PID 1 时：

```text
run_tasks
  -> fetch_task
  -> 激活 PID 1 用户页表
  -> context_switch
  -> task_entry
  -> prepare_user_return
  -> run_user_task
  -> user_restore
  -> ertn
```

`prepare_user_return()` 把 LoongArch `PRMD` 设置为：

```text
PPLV = 3：异常返回后进入用户态
PIE  = 1：异常返回后允许中断
```

`user_restore` 恢复通用寄存器、浮点/LSX 状态、ERA 和用户栈，最后执行 `ertn`，
完成第一次进入用户态。

## 11. 用户态陷入内核的反向路径

用户程序执行系统调用、发生缺页或收到定时器/IPI 后，流程为：

```text
用户态
  -> trap_vector_base
  -> 保存 TrapFrame
  -> loongarch64_trap_handler
  -> 分类 SysCall/Timer/PageFault/IPI/IllegalInstruction
  -> _interrupt_for_arch
  -> os::kernel_interrupt
  -> syscall/page fault/timer/signal 处理
  -> prepare_user_return
  -> user_restore/ertn
  -> 用户态
```

其中：

- `EENTRY` 指向 `trap_vector_base`。
- `TLBRENTRY` 指向 `tlb_fill`。
- `kernel_interrupt` 是 `#[polyhal::arch_interrupt]` 导出的 Kairix 回调。

## 12. 当前实现需要注意的事项

### 12.1 `_main_for_arch` ABI 签名不一致

PolyHAL 当前声明：

```rust
fn _main_for_arch(hartid: usize);
```

Kairix 实际导出：

```rust
fn main(id: usize, first: bool) -> bool;
```

两者参数和返回类型不一致。当前 ELF 反汇编中，主核调用前 `$a1` 恰好为 1，因此
会进入 `first` 分支，但源码层面没有 ABI 保证。编译器或优化结果变化后，主核可能
跳过完整初始化。应优先改成显式且签名一致的包装入口。

### 12.2 trap 初始化重复

当前主核和从核都通过 `CtorType::Cpu` 执行一次 `trap::init()`，随后 Kairix 主入口或
从核入口又调用一次 `init_trap()`。当前流程因此存在重复配置 EENTRY、TLB 参数和
IPI 的情况。

### 12.3 QEMU DTB 地址是硬编码的

PolyHAL、VirtIO block 和 VirtIO net 的 LoongArch QEMU 路径依赖固定地址：

```text
0x00100000 / 0x9000000000100000
```

当前没有统一从启动参数传递 DTB 地址。如果更换 QEMU 启动方式或固件布局，需要
同步检查这些引用。

### 12.4 早期与调度阶段的 timer 周期不同

PolyHAL Platform constructor 初始使用 1000 Hz，即约 1 ms 周期；Kairix 后面的
`set_next_trigger()` 使用 100 Hz，即约 10 ms 调度周期。进入 Kairix 后会重新编程
本核定时器。

### 12.5 `processor_start()` 不负责 LoongArch QEMU 拉核

LoongArch QEMU 从核已经由 PolyHAL mailbox/IPI 路径拉起。Kairix 的
`processor_start()` 在该配置下不会再次执行实际拉核操作，但仍可能打印唤醒日志，
调试日志时不要把它误认为真正的 CPU 启动点。

## 13. 关键源码索引

| 内容 | 源码位置 |
| --- | --- |
| QEMU 命令 | `Makefile` |
| LoongArch 构建与链接参数 | `os/Makefile` |
| 链接地址与 `_start` | `os/src/linker-loongarch64.ld` |
| `_start`、`rust_tmp_main`、从核入口 | `polyhal/polyhal-boot/src/arch/loongarch64.rs` |
| `call_real_main`、构造器最后阶段 | `polyhal/polyhal-boot/src/arch/mod.rs` |
| 构造器定义和遍历 | `polyhal/polyhal/src/ctor.rs` |
| DTB、MEM_AREA、CPU_NUM | `polyhal/polyhal/src/mem.rs` |
| per-CPU 分配与 `$r21` | `polyhal/polyhal/src/components/percpu/mod.rs` |
| LoongArch mailbox/IPI | `polyhal/polyhal/src/components/multicore/loongarch64.rs` |
| LoongArch timer | `polyhal/polyhal/src/components/timer/loongarch64.rs` |
| LoongArch trap/TLB/user restore | `polyhal/polyhal-trap/src/trap/loongarch64.rs` |
| LoongArch TrapFrame | `polyhal/polyhal-trap/src/trapframe/loongarch64.rs` |
| Kairix 主核/从核入口 | `os/src/main.rs` |
| Kairix 内存初始化 | `os/src/mm/` |
| 网络初始化 | `os/src/net/mod.rs` |
| 根文件系统初始化 | `os/src/fs/mod.rs` |
| VirtIO block | `os/src/drivers/block/virtio_blk.rs` |
| PID 1 创建与首次用户态 | `os/src/task/process.rs`、`os/src/task/mod.rs` |

