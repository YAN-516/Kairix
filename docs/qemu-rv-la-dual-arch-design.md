# Kairix RISC-V64 / LoongArch64 QEMU 双架构设计

本文根据当前工作区源码，整理 Kairix 在 QEMU 上的 RISC-V64（下文简称 RV）与
LoongArch64（下文简称 LA）双架构设计。内容覆盖构建入口、启动、地址空间、
Trap/TrapFrame、SMP、定时器、VirtIO、rootfs 和用户 ABI，并标明当前抽象边界与
仍需收敛的架构差异。

本文只讨论：

- `ARCH=riscv64 BOARD=qemu`
- `ARCH=loongarch64 BOARD=qemu`

VisionFive 2 和 Loongson 2K1000 属于板级实现，不在本文展开。更详细的 Trap 控制流
见 [Trap 处理流程](./Trap处理流程.md)，LA QEMU 的逐阶段启动细节见
[LoongArch QEMU 启动流程](./loongarch-qemu-boot-flow.md)。

## 1. 设计结论

Kairix 的双架构实现不是两套独立内核，而是：

> 一套共享 Rust 宏内核 + PolyHAL 公共硬件接口 + RV/LA 架构后端 + QEMU 设备传输差异。

```text
                  Makefile / Cargo target / linker script
                                   |
                                   v
                 +-----------------------------------+
                 |          os/src 共享宏内核        |
                 | task / syscall / mm / fs / net   |
                 | socket / signal common / drivers |
                 +-----------------+-----------------+
                                   |
                                   v
                 +-----------------------------------+
                 |             PolyHAL API           |
                 | PageTable / TrapType / IRQ/Timer  |
                 | KContext / PerCPU / SMP / Console |
                 +-----------------+-----------------+
                                   |
                      +------------+------------+
                      |                         |
                      v                         v
              RISC-V64 backend          LoongArch64 backend
              Sv39 / SBI / sret         PGD / IOCSR / ertn
                      |                         |
                      v                         v
              QEMU virt + OpenSBI       QEMU LoongArch virt
              VirtIO-MMIO               VirtIO-PCI
```

共享程度最高的是：

- 调度器、进程线程和信号公共语义；
- 系统调用主体和 Linux errno 语义；
- VFS、ext4、tmpfs、procfs、page cache 和 swap；
- mmap、懒分配、COW、mprotect 和用户缺页处理；
- TCP/IP、UDP、socket 和 VirtIO-net 上层逻辑；
- SMP TLB shootdown、内存屏障和 scheduler IPI 的公共状态机。

必须按架构分开的部分是：

- 最早期汇编入口和特权寄存器；
- 页表项位定义、页表根切换和 TLB 指令；
- Trap 汇编、异常原因解码和返回指令；
- TrapFrame、signal frame 和 Linux 寄存器 ABI；
- 定时器和 IPI 的硬件提交方式；
- QEMU 上的 VirtIO-MMIO / VirtIO-PCI transport。

## 2. QEMU 构建与运行矩阵

`os/Makefile` 根据 `ARCH` 选择 Rust target 和链接脚本：

| 项目 | RV QEMU | LA QEMU |
| --- | --- | --- |
| `ARCH` | `riscv64` | `loongarch64` |
| Rust target | `riscv64gc-unknown-none-elf` | `loongarch64-unknown-none` |
| 链接脚本 | `os/src/linker-riscv64.ld` | `os/src/linker-loongarch64.ld` |
| 内核 ELF | `os/target/riscv64gc-unknown-none-elf/release/os` | `os/target/loongarch64-unknown-none/release/os` |
| 顶层产物 | `kernel-rv` | `kernel-la` |
| 默认 CPU 数 | 8 | 12 |
| 默认内存 | 8 GiB | 36 GiB |
| 固件 | QEMU `virt` + OpenSBI | QEMU 默认 LA 启动环境 |
| 块设备 | `virtio-blk-device`，MMIO | `virtio-blk-pci`，PCI |
| 网卡 | `virtio-net-device`，MMIO | `virtio-net-pci`，PCI |
| 根镜像 | `sdcard-rv.img` | `sdcard-la.img` |

只编译、不启动 QEMU 的命令是：

```bash
make -C os ARCH=riscv64 BOARD=qemu build
make -C os ARCH=loongarch64 BOARD=qemu build
```

顶层运行入口是：

```bash
make rkernel
make lkernel
```

这两个顶层目标会在启动 QEMU 前 patch 对应的 sdcard 镜像，不是只读构建操作。

当前 QEMU 命令的核心区别为：

```text
RV:
qemu-system-riscv64
  -machine virt
  -bios default
  -kernel kernel-rv
  -device virtio-blk-device
  -device virtio-net-device

LA:
qemu-system-loongarch64
  -kernel kernel-la
  -device virtio-blk-pci
  -device virtio-net-pci
```

## 3. 源码分层

### 3.1 共享内核层

`os/src` 是实际的 Kairix 宏内核：

```text
os/src/main.rs       公共初始化、统一 Trap 分发
os/src/task/         进程、线程、调度和 KContext
os/src/syscall/      Linux 风格系统调用
os/src/mm/           VM、COW、缺页、swap、页帧和堆
os/src/fs/           VFS、ext4、tmpfs、procfs、page cache
os/src/net/          自研网络协议栈和网卡接入
os/src/socket/       socket 对象和系统调用后端
os/src/drivers/      块设备与 transport 适配
```

### 3.2 PolyHAL 层

PolyHAL 被拆成三部分：

```text
polyhal/polyhal/         页表、timer、IRQ、SMP、PerCPU、KContext
polyhal/polyhal-boot/    主核/从核最早期启动
polyhal/polyhal-trap/    Trap 入口、TrapFrame、异常解码和用户返回
```

`polyhal-macro::define_arch_mods!()` 根据 Rust 的 `target_arch` 只编译当前架构后端，
再把它重新导出成同名公共接口。例如上层统一调用：

```rust
polyhal::timer::set_next_timer(...)
polyhal::PageTable::current()
polyhal::multicore::send_reschedule_ipi(...)
polyhal_trap::trap::run_user_task(...)
```

具体执行的是 RV 还是 LA 版本由编译目标决定，不在运行时动态判断。

### 3.3 QEMU board 层

`BOARD=qemu` 选择 `os/src/boards/qemu.rs`，向公共内核提供：

- `MMIO` 映射范围；
- `_CLOCK_FREQ`；
- 默认块设备类型 `VirtIOBlock`。

当前这个 QEMU board 文件同时包含 RV VirtIO-MMIO、LA RTC 和 PCI ECAM/MMIO window，
因此它更接近“两种 QEMU virt 平台的地址并集”，还不是严格拆分的 RV-QEMU 与
LA-QEMU board 描述。

## 4. 两条启动链

双架构最后都会进入同一个 Kairix `os::main()`，但进入公共入口之前的启动所有权
并不一致：RV 启动仍由 `os/src/arch` 持有，LA 启动已经由 `polyhal-boot` 持有。

### 4.1 RV QEMU 启动

```text
QEMU virt
  -> OpenSBI
  -> PA 0x80200000
  -> os/src/arch/riscv_dir/entry.rs::_start
  -> 为低地址和高半区建立早期 Sv39 1 GiB 映射
  -> 设置 boot stack、tp=hart id、satp 和浮点状态
  -> rust_main(hartid, dtb)
  -> clear_bss（主核）
  -> init_dtb_once(dtb)
  -> parse_system_info()
  -> _main_for_arch(id, first)
  -> os::main(id, first)
```

链接脚本把 ELF 链接在高半区：

```text
物理加载地址：0x0000000080200000
内核虚拟地址：0xffffffc080200000
直映偏移：    0xffffffc000000000
```

OpenSBI 按 RISC-V boot ABI 传入：

```text
a0 = hart id
a1 = DTB 物理地址
```

RV 入口通过 `BSP_DONE` 判断首个进入的 hart。首核清 BSS、解析 DTB 并进入公共初始化；
后续 hart 等待早期初始化完成后，以 `first=false` 进入同一个内核入口。

### 4.2 LA QEMU 启动

```text
QEMU LoongArch virt
  -> polyhal-boot::_start
  -> 配置 0x8... uncached DMW
  -> 配置 0x9... cached DMW
  -> 设置 CRMD/PRMD/EUEN
  -> 设置 bstack_top
  -> rust_tmp_main(hartid)
  -> clear_bss
  -> init_dtb_once(0x00100000)
  -> set_local_thread_pointer(hartid)
  -> init_cpu：打开 FPE 和 SXE
  -> Cpu/Platform/HALDriver 构造器
  -> parse_system_info()
  -> call_real_main(hartid)
  -> _main_for_arch(id, first)
  -> os::main(id, first)
```

LA 链接地址为：

```text
物理内存基址：   0x0000000080000000
cached DMW 地址：0x9000000080000000
ELF 入口：       _start
```

QEMU LA 当前使用固定 DTB 地址：

```text
DTB PA：        0x00100000
DTB cached VA：0x9000000000100000
```

若 DTB 解析失败，启动代码会注册兜底物理内存范围
`0x80000000..0x100000000`。

### 4.3 公共内核初始化

两条启动链最终通过 `#[polyhal::arch_entry]` 导出的 `_main_for_arch` 进入
`os/src/main.rs::main()`：

```text
logging
  -> kernel heap
  -> frame allocator
  -> 向 PolyHAL 注册 PageAlloc
  -> trap init
  -> kernel VM
  -> processors
  -> network
  -> filesystem/rootfs
  -> embedded runtime files
  -> swap
  -> initproc
  -> timer
  -> run_tasks
```

这部分是 RV/LA 完全共享的内核生命周期。

## 5. 地址空间与页表

### 5.1 公共 VM 模型

两种架构都使用：

- 4 KiB 基础页；
- 三级页表；
- 每级 512 个 PTE；
- 低 39 位用户地址空间；
- 高地址内核直映/内核映射；
- 同一套 `PageTable`、`PTE`、`MappingFlags` 和 `VMSpace` 上层接口。

公共页表代码负责：

```text
find_pte / find_pte_create
map_page / unmap_page
translate / translate_va
active address-space TLB shootdown
页表帧生命周期管理
```

### 5.2 架构差异

| 项目 | RV QEMU | LA QEMU |
| --- | --- | --- |
| 内核直映起点 | `0xffffffc000000000` | `0x9000000000000000` cached DMW |
| 用户空间 | `0..0x3fffffffff` | `0..0x3fffffffff` |
| 页表根 | `satp.PPN` | `PGDL/PGDH` |
| 页表模式 | Sv39，`satp.MODE=8` | PWCL/PWCH 配置的三级页表 |
| 权限位 | R/W/X/U/A/D 正向位 | W、PLV_USER，NR/NX 反向位 |
| 单地址 TLB 刷新 | `sfence.vma va, x0` | `invtlb 0x05, asid, pair_va` |
| 全局 TLB 刷新 | `sfence.vma` | `invtlb 0x00` |
| token | `MODE | root_ppn` | `root_ppn` |

LA 的一个 TLB 项同时描述偶数和奇数两个 4 KiB 页，因此单地址刷新必须先把地址按
8 KiB pair 对齐，并携带当前 ASID。RV 的 `sfence.vma` 可以直接以单页 VA 为目标。

### 5.3 TLB refill 与普通缺页

RV 的页表遍历和 TLB refill 主要由硬件完成。找不到合法 PTE 时，CPU 产生
instruction/load/store page fault。

LA 配置了专门的 `TLBRENTRY`：

```text
TLB miss
  -> tlb_fill
  -> lddir 逐级遍历页表
  -> ldpte 读取偶/奇 PTE
  -> tlbfill
  -> ertn
```

页表里已有合法映射时，TLB refill 可以不进入通用 OS 缺页路径；页表本身缺项或权限
不允许时，才会形成交给共享 VM 层处理的 `TrapType::*PageFault`。

## 6. Trap 总体模型

PolyHAL Trap 层负责把两种架构的硬件异常统一翻译为：

```rust
TrapType::SysCall
TrapType::Timer
TrapType::Reschedule
TrapType::LoadPageFault(va)
TrapType::StorePageFault(va)
TrapType::InstructionPageFault(va)
TrapType::IllegalInstruction(detail)
TrapType::FloatingPointException(detail)
TrapType::Handled
```

公共流程为：

```text
用户任务运行
  -> 架构 Trap 汇编保存 TrapFrame
  -> 架构 cause/ECODE 解码
  -> TrapType
  -> _interrupt_for_arch
  -> os::kernel_interrupt
  -> syscall / page fault / signal / timer / schedule
  -> 架构用户恢复
  -> sret / ertn
```

`os::kernel_interrupt()` 不需要理解 `scause` 或 LA `ESTAT.ECODE`，只处理统一的
`TrapType`。

## 7. 双架构 TrapFrame

TrapFrame 是每个用户任务的完整用户态 CPU 现场。它与调度器保存内核执行续体的
`KContext` 不同：

```text
TrapFrame：用户程序暂停在哪里，恢复后从哪里继续
KContext：任务的内核调用链暂停在哪里，调度回来后从哪里继续
```

### 7.1 RV TrapFrame

```rust
pub struct TrapFrame {
    pub x: [usize; 32],
    pub sstatus: Sstatus,
    pub sepc: usize,
    pub f: [u64; 32],
    pub fcsr: usize,
}
```

布局：

```text
0       x0-x31         256 B
256     sstatus          8 B
264     sepc             8 B
272     f0-f31         256 B
528     fcsr             8 B
----------------------------
总大小                  536 B
内核临时压栈大小        544 B（16 字节对齐）
```

### 7.2 LA TrapFrame

```rust
pub struct TrapFrame {
    pub regs: [usize; 32],
    pub prmd: usize,
    pub era: usize,
    pub vr: [[u64; 2]; 32],
    pub fcc: [u8; 8],
    pub fcsr: usize,
}
```

布局：

```text
0       r0-r31          256 B
256     PRMD              8 B
264     ERA               8 B
272     vr0-vr31        512 B
784     FCC               8 B
792     FCSR              8 B
-----------------------------
总大小                  800 B
内核临时压栈大小        800 B
```

LA 保存完整 128 位 LSX 寄存器。标量浮点寄存器与每个 LSX 寄存器的低 64 位重叠，
所以不能只保存标量低半部分。

### 7.3 Linux ABI 映射

| 语义 | RV | LA | 公共访问方式 |
| --- | --- | --- | --- |
| PC | `sepc` | `ERA` | `ctx.pc()` / `SEPC` |
| 返回地址 | `x1/ra` | `r1/ra` | `RA` |
| 栈指针 | `x2/sp` | `r3/sp` | `SP` |
| TLS | `x4/tp` | `r2/tp` | `TLS` |
| 参数 0-5 | `x10-x15` | `r4-r9` | `ctx.args()` |
| 返回值 | `x10/a0` | `r4/a0` | `RET` |
| syscall id | `x17/a7` | `r11/a7` | `SYSCALL` |

公共层通过 `TrapFrameArgs` 的 `Index/IndexMut` 实现访问寄存器，避免系统调用主体到处
出现架构 cfg。

### 7.4 RV Trap 进入与返回

RV 使用 `stvec`、`sscratch` 和 `sret`：

```text
run_user_task
  -> user_restore
  -> 保存等待用户返回的内核 SP/RA/callee-saved 状态
  -> sscratch = TrapFrame 地址
  -> 恢复用户 GPR/FP/sstatus/sepc
  -> sret

用户 Trap
  -> kernelvec
  -> csrrw sp, sscratch, sp
  -> 根据交换结果区分用户/内核来源
  -> uservec 保存用户 TrapFrame
  -> 恢复原内核续体
  -> kernel_callback
```

### 7.5 LA Trap 进入与返回

LA 使用 `EENTRY`、`KSAVE_*` CSR 和 `ertn`：

```text
run_user_task
  -> user_restore
  -> KSAVE_KSP = 内核栈
  -> KSAVE_CTX = TrapFrame 地址
  -> 恢复 GPR/PRMD/ERA/LSX/FCC/FCSR
  -> ertn

用户 Trap
  -> trap_vector_base
  -> 根据 PRMD.PPLV 区分用户/内核来源
  -> user_vec 从 KSAVE_CTX 找到 TrapFrame
  -> SAVE_REGS
  -> 从 KSAVE_KSP 恢复原内核续体
  -> loongarch64_trap_handler
```

### 7.6 LA Trap 的额外工作

相比 RV，LA Trap 后端还处理：

- TLB refill；
- 整数/指针非对齐 load/store 模拟；
- 内核访问用户地址失败时的 exception-table fixup；
- FPU unavailable 时打开 FPE 并重试原指令；
- LA `ESTAT.ECODE/ESUBCODE` 到公共 `TrapType` 的翻译。

这些工作都留在 PolyHAL Trap 后端，上层缺页、信号和调度语义仍然共用。

## 8. SMP、CPU 标识与 IPI

### 8.1 CPU 数量

| 项目 | RV QEMU | LA QEMU |
| --- | ---: | ---: |
| PolyHAL `MAX_CPU_NUM` | 8 | 12 |
| QEMU 默认 `-smp` | 8 | 12 |
| CPU ID 来源 | RV `tp` | `CPUID` CSR |
| PerCPU 基址 | PolyHAL RV 路径使用 `gp` | LA 使用 `$r21` |

### 8.2 从核启动差异

RV 从核由 Kairix 公共入口之后的 `processor_start()` 拉起：

```text
主核完成主要 OS 初始化
  -> SBI HSM hart_start(hart, 0x80200000, opaque)
  -> 从核重新进入 RV _start
  -> early INIT_DONE
  -> os::main(id, false)
  -> run_tasks
```

LA 从核由 `polyhal-boot::call_real_main()` 在进入 Kairix 公共主入口前拉起：

```text
主核解析 CPU 数
  -> 预留所有 PerCPU 区域
  -> 为从核分配启动栈
  -> IOCSR mailbox 写入口地址和 SP
  -> IPI 拉起从核
  -> _secondary_start
  -> _rust_secondary_main
  -> 等待 PolyHAL INIT_DONE
  -> _secondary_for_arch
  -> os::main(id, false)
```

这是当前双架构启动设计中最明显的不对称。

### 8.3 公共 IPI 状态机

PolyHAL 的公共 multicore 层维护软件 IPI reason：

```text
TLB_SHOOTDOWN
RESCHEDULE
TIMER_RECOVERY
MEMORY_BARRIER
```

发送方先用原子操作发布 reason，再触发硬件 doorbell；接收方处理 reason 并更新 ACK。
架构后端只负责真正发送和确认中断：

| 操作 | RV | LA |
| --- | --- | --- |
| 发送 IPI | SBI `send_ipi` | IOCSR `send_ipi_single` |
| 接收中断 | Supervisor Software Interrupt | LA IPI line 12 |
| 确认 | 清 `sip.SSIP` | 写 `IOCSR_IPI_CLEAR` |
| shootdown 等待 | 只放行 SSIP | 只放行 IPI line |

TLB shootdown 等待过程中只允许 lock-free IPI 路径重入，避免两个 CPU 在关中断状态下
相互等待对方 ACK。

## 9. 定时器与抢占

### 9.1 RV QEMU

```text
读取时间：time CSR
编程方式：SBI set_timer(deadline)
中断类型：SupervisorTimer
模式：软件按 deadline 重编程
```

RV timer 后端当前把频率固定为 `12_500_000 Hz`。

### 9.2 LA QEMU

```text
读取时间：LoongArch Time::read()
频率来源：get_timer_freq()
编程方式：TCFG/TVAL
中断类型：ESTAT timer line
模式：硬件周期定时器
```

LA 在第一次设置有效周期后保持 periodic，不在每次中断中反复停止、重启硬件定时器。

### 9.3 公共抢占语义

两边最终都生成 `TrapType::Timer`，由 `os::kernel_interrupt()`：

```text
记录 timer interrupt
  -> 请求全局 timer maintenance
  -> 设置/确认下一次 10 ms tick
  -> 如果 Trap 来自用户态，preempt_current_and_run_next()
```

当前内核模型是：

> 用户态可被 timer 抢占；任意内核指令点不可被异步调度切走。

内核中特别声明为可中断的区间只允许 timer/IPI 做记账和恢复工作，不会在嵌套内核
Trap 中直接切换当前 Rust/C continuation。

## 10. QEMU VirtIO 与 DMA

### 10.1 块设备

公共 `VirtIOBlock` 根据架构选择 transport：

| 项目 | RV QEMU | LA QEMU |
| --- | --- | --- |
| transport | `MmioTransport` | `PciTransport` |
| 发现方式 | 固定 VirtIO-MMIO 地址 | 从 FDT 找 PCI ECAM 并枚举 |
| QEMU 设备 | `virtio-blk-device` | `virtio-blk-pci` |
| 公共接口 | `BlockDevice` | `BlockDevice` |

RV QEMU 的 VirtIO block MMIO 基址为：

```text
0x10001000 + VIRT_ADDR_START
```

LA QEMU 从固定 DTB cached 地址读取 PCI host 描述，枚举
`pci-host-ecam-generic`，再创建 VirtIO PCI transport。

两边进入 VFS 后都只暴露 `Arc<dyn BlockDevice>`，ext4、page cache 和 writeback 不再
关心底层是 MMIO 还是 PCI。

### 10.2 VirtIO-net

同一套 `VirtIONetDevice` 同时支持 PCI 和 MMIO：

```text
probe PCI
  -> 成功：使用 PCI modern transport
  -> 失败：尝试 VirtIO-MMIO
```

因此：

- LA QEMU 命中 PCI transport；
- RV QEMU 没有对应 PCI 设备，回退到 MMIO transport。

网卡初始化完成后，两边都接入相同的：

```text
NetDevice
  -> ethernet_rcv
  -> ARP / IPv4 / ICMP
  -> TCP / UDP
  -> socket
```

QEMU user networking 使用：

```text
guest IP：10.0.2.15
gateway： 10.0.2.2
DNS：     10.0.2.3
```

### 10.3 DMA 地址转换差异

RV 的 DMA buffer 统一通过当前页表执行 VA -> PA 翻译。

LA 如果地址位于 `0x9...` cached DMW，可直接减去 `VIRT_ADDR_START` 得到 PA；其他
虚拟地址再通过当前页表翻译。LA 设备访问前后还显式使用 `dbar`，以满足其内存顺序
和 DMA 可见性要求。

## 11. rootfs 与实时时钟

### 11.1 RV QEMU rootfs

RV QEMU 直接使用全局 `BLOCK_DEVICE` 挂载 ext4 rootfs。挂载失败当前会 `unwrap()`，
没有 LA QEMU 的 initrd/tmpfs 回退链。

### 11.2 LA QEMU rootfs

LA QEMU 的顺序是：

```text
VirtIO-PCI block ext4
  -> 失败：尝试从 DTB/initrd 内存范围挂载 ext4
  -> 再失败：tmpfs root
```

因此两边的 VFS 和 ext4 实现相同，但启动根文件系统的容错策略不同。

### 11.3 RTC

公共 `realtime_ns()` 先读取平台 RTC 建立 Unix epoch anchor，后续使用单调硬件计数器
推进时间：

| 项目 | RV QEMU | LA QEMU |
| --- | --- | --- |
| RTC 类型 | Goldfish RTC | LS7A-compatible RTC |
| 地址 | `0x00101000` | `0x100d0100` |
| 后续推进 | `polyhal::timer::current_time()` | `polyhal::timer::current_time()` |

## 12. 用户态 ABI

用户程序与内核一样按 target 分别编译，但共享绝大部分 Rust 用户库和 syscall number。

### 12.1 系统调用指令

RV：

```text
a0-a5 = 参数
a7    = syscall id
ecall
a0    = 返回值
```

LA：

```text
a0-a5/r4-r9 = 参数
a7/r11      = syscall id
syscall 0
a0/r4       = 返回值
```

TrapFrame 的 `args()`、`RET` 和 `SYSCALL` 映射把这组差异屏蔽在架构后端。

### 12.2 必须分架构的 Linux ABI

以下内容不能只依靠统一 `TrapFrameArgs`：

- Linux `rt_sigframe` 和 `ucontext/mcontext` 布局；
- `rt_sigreturn` 的寄存器、FP/LSX 恢复；
- 老式 `clone()` 的原始参数顺序；
- ELF `AT_HWCAP`；
- `uname.machine`；
- RV 专属 `riscv_hwprobe`。

因此 signal 目录保留 `riscv64.rs` 与 `loongarch64.rs` 两份 ABI 实现，而信号选择、
pending mask、默认动作和进程语义仍位于共享层。

## 13. 共享边界总结

| 子系统 | 共享层 | RV 后端 | LA 后端 |
| --- | --- | --- | --- |
| 启动 | `os::main` 之后 | OS 自有 entry + SBI | PolyHAL boot + DMW/IOCSR |
| 页表 | map/unmap/translate/frames | Sv39 PTE、SATP、SFENCE | LA PTE、PGDL/H、INVTLB |
| Trap | `TrapType`、OS handler | scause/stval、sret | ESTAT/BADV、ertn、refill |
| TrapFrame | `TrapFrameArgs` | GPR + FP64 | GPR + LSX128 + FCC |
| 调度 | task/run queue/KContext 语义 | RV context asm | LA context asm |
| SMP | reason/generation/ACK | SBI IPI | IOCSR IPI |
| timer | Duration/tick 语义 | time CSR + SBI | Time CSR + TCFG |
| block | `BlockDevice`、VirtIO request | VirtIO-MMIO | VirtIO-PCI |
| network | NetDevice + TCP/IP | MMIO transport | PCI transport |
| VFS/rootfs | VFS/ext4/page cache | 直接 ext4 | ext4 + initrd/tmpfs fallback |
| syscall | syscall 主体 | RV Linux ABI | LA Linux ABI |

## 14. 当前 QEMU 双架构设计的已知问题

### 14.1 启动所有权不统一

RV 的 `_start`、BSP 判断和从核拉起仍在 OS；LA 已经全部放入 `polyhal-boot`。这使
启动同步、boot stack、PerCPU 和从核生命周期存在两种模型。

建议长期统一为：

```text
polyhal-boot：主核/从核入口、早期地址空间、DTB、PerCPU、拉起从核
os：allocator 之后的内核子系统和调度器
```

### 14.2 RV timer 频率硬编码

RV timer 后端固定使用 `12_500_000 Hz`，没有从 DTB 的 `timebase-frequency` 或
board 配置读取。QEMU 当前配置可能与它一致，但这个接口会阻碍同一 RV 后端复用到
其他平台，也容易让 `CLOCK_MONOTONIC`、sleep 和 scheduler tick 产生比例偏差。

### 14.3 LA PTE cache 属性需要确认

LA 的 `MappingFlags -> PTEFlags` 当前初始就加入 `MAT_NOCACHE`。按静态代码推导，
即使上层请求 `MappingFlags::Cache`，该位也可能没有被清掉。需要明确普通 RAM、用户页、
MMIO 和 DMA 页分别应使用什么 MAT。

### 14.4 用户链接脚本带有 RV 架构声明

RV/LA 用户程序当前共用 `user/src/linker.ld`，其中仍写着：

```ld
OUTPUT_ARCH(riscv)
```

LA target 也使用这份脚本。即使当前 linker 容忍该声明，也应拆为 RV/LA 两份链接
脚本，避免 ABI 和工具链行为依赖 linker 的宽松处理。

### 14.5 通用设备 IRQ 尚未完整实现

当前成熟的中断路径主要是 timer 和 IPI。`IRQ::irq_enable/disable` 在 RV/LA 后端
仍未实现完整的设备中断控制，LA 对未知 interrupt line 会 panic。VirtIO 和网卡路径
较多依赖轮询，因此还不能把当前 QEMU 设备模型描述为完整的通用 IRQ 架构。

### 14.6 架构细节仍泄漏到 OS

当前 OS 层仍直接出现：

- `sie/ecfg` 中断掩码；
- `ctx.x/ctx.regs`、`sepc/era`；
- LA TLB CSR 和 `dbar`；
- 架构汇编读取 SP/RA；
- transport 与 `target_arch` 的直接绑定。

建议继续补充 PolyHAL 公共接口，例如：

```rust
ctx.from_user()
ctx.general_registers()
arch_irq::restrict_to_timer_ipi()
arch::current_sp()
arch::current_ra()
dma::read_barrier()
dma::write_barrier()
```

### 14.7 `os/src/arch` 存在重复实现

`os/src/arch/{riscv,loongarch64}.rs` 还各自实现了一份 TLB flush，但当前 VM 主路径使用
的是 `polyhal::pagetable::TLB`。这类重复实现容易让维护者误判真实调用路径，应在启动
迁移完成后删除或合并。

## 15. 建议的收敛顺序

第一阶段处理静态正确性风险：

1. RV timer frequency 改从 DTB/平台配置获取；
2. 确认 LA PTE MAT/cache 语义；
3. 拆分 RV/LA 用户链接脚本；
4. 明确 QEMU 设备是轮询模型还是要补全 IRQ。

第二阶段统一架构边界：

1. 将 RV 主核/从核启动迁入 `polyhal-boot`；
2. 统一 PerCPU 和 CPU identity 初始化；
3. 把 TrapFrame 的通用读写方法补齐；
4. 把中断掩码、SP/RA、TLB recovery 和 DMA barrier 下沉 PolyHAL。

第三阶段清理板级与 transport：

1. 把 QEMU RV/LA MMIO 描述拆开；
2. transport 选择从 `target_arch` 改成设备发现或 board capability；
3. 删除 OS 内重复 TLB 和已失效的架构注释；
4. 为两种 QEMU 架构建立同构的静态构建检查与用户 ABI 测试。

## 16. 关键源码索引

| 内容 | 文件 |
| --- | --- |
| 顶层 QEMU 命令 | `Makefile` |
| target 与链接选择 | `os/Makefile` |
| 公共内核入口/Trap handler | `os/src/main.rs` |
| QEMU board 配置 | `os/src/boards/qemu.rs` |
| RV 启动入口 | `os/src/arch/riscv_dir/entry.rs` |
| RV 链接脚本 | `os/src/linker-riscv64.ld` |
| LA 启动入口 | `polyhal/polyhal-boot/src/arch/loongarch64.rs` |
| LA 链接脚本 | `os/src/linker-loongarch64.ld` |
| PolyHAL 启动公共层 | `polyhal/polyhal-boot/src/arch/mod.rs` |
| 公共页表 | `polyhal/polyhal/src/pagetable/mod.rs` |
| RV 页表 | `polyhal/polyhal/src/pagetable/riscv64.rs` |
| LA 页表 | `polyhal/polyhal/src/pagetable/loongarch64.rs` |
| 公共 TrapType | `polyhal/polyhal-trap/src/trap/mod.rs` |
| RV Trap | `polyhal/polyhal-trap/src/trap/riscv64.rs` |
| LA Trap | `polyhal/polyhal-trap/src/trap/loongarch64.rs` |
| RV TrapFrame | `polyhal/polyhal-trap/src/trapframe/riscv64.rs` |
| LA TrapFrame | `polyhal/polyhal-trap/src/trapframe/loongarch64.rs` |
| 公共 SMP/IPI 状态机 | `polyhal/polyhal/src/components/multicore/mod.rs` |
| RV SBI IPI | `polyhal/polyhal/src/components/multicore/riscv64.rs` |
| LA IOCSR IPI | `polyhal/polyhal/src/components/multicore/loongarch64.rs` |
| RV timer | `polyhal/polyhal/src/components/timer/riscv64.rs` |
| LA timer | `polyhal/polyhal/src/components/timer/loongarch64.rs` |
| VirtIO block | `os/src/drivers/block/virtio_blk.rs` |
| VirtIO net | `os/src/net/virtio/` |
| rootfs 选择 | `os/src/fs/mod.rs` |
| 用户 syscall ABI | `user/src/syscall.rs` |
| RV signal ABI | `os/src/syscall/signal/riscv64.rs` |
| LA signal ABI | `os/src/syscall/signal/loongarch64.rs` |
