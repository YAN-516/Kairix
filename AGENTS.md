# Kairix OS 项目指南

> 本文档面向 AI 编程助手，用于快速了解 Kairix 内核的架构、构建方式、代码组织及开发规范。项目的主要注释和文档语言为中文，因此本指南使用中文撰写。

---

## 1. 项目概览

**Kairix** 是一款基于 Rust 语言开发的现代化操作系统内核，目标架构为 **RISC-V 64** 与 **LoongArch 64**，可在 QEMU 模拟器上运行。内核设计参考了 rCore/Chronix 的教学内核路线，但在功能完整性上向比赛/生产环境延伸：

- **多架构支持**：同一套内核源码通过 `polyhal` 硬件抽象层适配 RISC-V 与龙芯。
- **多文件系统**：具备完整的 VFS 层，支持同时挂载 ext4、FAT32、devfs、procfs、tmpfs 等。
- **懒分配（Lazy Allocation）**：基于缺页异常的动态内存映射，仅在访问时分配物理页。
- **内存安全**：内核与用户库均使用 Rust 编写，利用所有权系统消除缓冲区溢出和空指针异常。
- **Unix / POSIX 风格**：提供标准进程管理、信号处理、管道、线程、BSD Socket、Futex 等系统调用，能够运行经过 musl/glibc 链接的 C 语言程序（POSIX 兼容仍在完善中）。
- **网络协议栈**：内置 TCP/IP 协议栈，支持 VirtIO-net 网卡、loopback、ARP、ICMP、UDP、TCP。

---

## 2. 技术栈与依赖

| 层级 | 技术/工具 | 说明 |
|------|-----------|------|
| 语言 | Rust | 内核、用户库、驱动均使用 Rust |
| 工具链 | `nightly-2025-01-18` | 指定在 `rust-toolchain.toml`，需要 `rust-src`、`llvm-tools`、`rustfmt`、`clippy` |
| 目标 | `riscv64gc-unknown-none-elf`、`loongarch64-unknown-none` | 内核编译目标 |
| 模拟器 | QEMU 9.2.1 | `qemu-system-riscv64` / `qemu-system-loongarch64` |
| Bootloader | rustsbi-qemu (RISC-V)、opensbi (LoongArch) | 预编译二进制位于 `bootloader/` |
| HAL | `polyhal` | 自研多架构硬件抽象层，位于 `polyhal/` |
| 文件系统 | `lwext4_rust` (ext4)、`rust-fatfs` (FAT32) | 子目录包含源码 |
| 构建系统 | Cargo + GNU Make | 顶层 Makefile 委托给 `os/Makefile` |
| 容器环境 | Docker + Dev Containers | 镜像 `zhouzhouyi/os-contest:20260104` |

---

## 3. 关键配置文件

- **`rust-toolchain.toml`**：Rust 工具链与组件清单。
- **`os/Cargo.toml`**：内核 crate 依赖（`polyhal`、`lwext4_rust`、`fatfs`、`virtio-drivers`、`spin`、`log` 等）。
- **`user/Cargo.toml`**：用户库 crate 依赖（`buddy_system_allocator`、`bitflags`）。
- **`polyhal/Cargo.toml`**：HAL 工作区，包含 `polyhal`、`polyhal-boot`、`polyhal-trap`、`polyhal-macro`。
- **`os/Makefile`**：内核编译、QEMU 启动、磁盘镜像注入的核心 Makefile。
- **`user/Makefile`**：用户程序编译为 ELF 和二进制文件。
- **`Dockerfile` / `.devcontainer/devcontainer.json`**：开发容器配置，已预装 QEMU、交叉 GDB、Rust 工具链。

---

## 4. 目录结构

```
kairix/
├── bootloader/          # 预编译 bootloader（rustsbi-qemu.bin、opensbi-qemu.bin）
├── docs/                # 测试文档（basic.md、libctest.md、lmbench_testcode.md、iozone.md）
├── libc-bench/          # libc 性能基准测试
├── libc-test/           # musl 静态/动态链接功能测试套件
├── ltp/                 # Linux Test Project 用例（部分 C 文件）
├── lwext4_rust/         # ext4 文件系统 Rust 绑定（基于 lwext4 C 库）
├── netperf-2.7.0/       # netperf 网络性能测试源码
├── os/                  # 内核源码
│   ├── src/
│   │   ├── arch/        # 架构相关代码（riscv、loongarch）
│   │   ├── boards/      # 板级配置（qemu.rs：时钟频率、MMIO、块设备类型）
│   │   ├── drivers/     # 设备驱动（virtio-blk、PCI 探测）
│   │   ├── fs/          # 文件系统（vfs、devfs、procfs、tmpfs、fat32、lwext4、pagecache）
│   │   ├── mm/          # 内存管理（frame_allocator、heap_allocator、vm_set、vm_area）
│   │   ├── net/         # 网络协议栈（virtio-net、ethernet、arp、ip、icmp、tcp、udp）
│   │   ├── socket/      # BSD Socket 层与 fd 表集成
│   │   ├── syscall/     # 系统调用实现（fs、mm、process、signal、net、futex 等）
│   │   ├── task/        # 进程/线程管理（process、task、processor、manager、signal）
│   │   ├── trap/        # Trap/中断/异常处理（缺页、COW、系统调用、时钟中断）
│   │   ├── sync/        # 同步原语（SpinLock、SleepLock、ReentrantLock、Mutex）
│   │   ├── config.rs    # 内核常量（MAX_CPU_NUM、MMAP_BASE、BLOCK_SIZE 等）
│   │   ├── main.rs      # 内核入口与初始化流程
│   │   └── ...
│   ├── build.rs         # 生成 link_app.S，将 user/src/bin 下的应用嵌入内核
│   └── Makefile
├── patches/             # 对第三方 crate 的补丁（如 cty）
├── polyhal/             # 多架构硬件抽象层
│   ├── polyhal/         # HAL 核心（页表、中断、指令、上下文、定时器）
│   ├── polyhal-boot/    # 启动抽象（各架构入口代码）
│   ├── polyhal-trap/    # Trap 框架与 TrapFrame
│   └── polyhal-macro/   # 过程宏（arch_entry、arch_interrupt、percpu）
├── rust-fatfs/          # FAT32 文件系统库（no_std）
├── temp_check_mnt/      # 临时挂载目录（构建时使用）
├── temp_mnt/            # 临时挂载目录（构建时使用）
└── user/                # 用户态库与应用程序
    ├── src/
    │   ├── bin/         # 应用源码（initproc、user_shell、ls、basictests、libctests 等）
    │   ├── lib.rs       # 用户库（syscall 封装、堆分配、_start 入口）
    │   ├── syscall.rs   # 系统调用号与内联汇编
    │   └── ...
    └── Makefile
```

---

## 5. 构建与运行

### 5.1 环境准备

建议在提供的 Docker / Dev Container 中工作，镜像已包含所有依赖。若自行配置，需要：

```bash
# 安装目标、cargo-binutils、rust-src、llvm-tools
rustup target add riscv64gc-unknown-none-elf loongarch64-unknown-none
cargo install cargo-binutils
rustup component add rust-src llvm-tools-preview
```

### 5.2 常用构建命令

所有命令默认在 **项目根目录** 或 **`os/` 目录** 下执行。

| 命令 | 作用 |
|------|------|
| `make env`（在 `os/` 下） | 检查并安装缺失的 Rust target 与 cargo-binutils |
| `make build`（在 `os/` 下） | 编译内核 ELF 并生成 `.bin` 文件 |
| `make run-sdcard` | 编译用户程序、将二进制注入到 `sdcard-rv.img`（或 `sdcard-la.img`），然后启动 QEMU |
| `make run` | 使用用户自制的 `fs.img`（`user/target/.../fs.img`）启动 |
| `make debug` | 启动 QEMU + GDB 调试会话（tmux 分屏） |
| `make gdbserver` | 仅启动 QEMU 的 GDB server（`-s -S`） |
| `make clean` | 清理内核与用户编译产物 |
| `make all`（在根目录） | vendor 依赖，分别构建 RISC-V 与 LoongArch 内核，复制到根目录 |

### 5.3 磁盘镜像注入机制

`make run-sdcard` 会执行 `do-patch-sdcard` 目标，流程如下：

1. 在 `user/` 下编译用户程序（`initproc`、`user_shell`、`ls`、`basictests`、`libctests_static`、`libctests_dynamic`）。
2. 使用 `e2fsck` 检查，然后 `mount` 外部镜像（`sdcard-rv.img` / `sdcard-la.img`）。
3. 将上述用户程序复制到镜像根目录，并设置可执行权限。
4. 根据架构复制 glibc / musl 的动态链接器到 `/lib` 或 `/lib64`：
   - RISC-V：`ld-linux-riscv64-lp64d.so.1`、`libc.so.6`、`libm.so.6`、`ld-musl-riscv64*.so.1`
   - LoongArch：`ld-linux-loongarch-lp64d.so.1`、`libc.so.6`、`libm.so.6`、`libdl.so.2`、`libpthread.so.0`、`ld-musl-loongarch-lp64d.so.1`
5. `sync && umount`。

因此，**比赛或完整测试必须依赖外部磁盘镜像**，镜像中需预先放置好 `busybox`、`glibc`、`musl` 库及测试用例。

---

## 6. 运行时架构

### 6.1 启动流程

1. Bootloader（rustsbi/opensbi）将内核加载到物理地址 `0x8020_0000`（RISC-V）或 `0x8000_0000`（LoongArch）。
2. 进入 `main.rs` 中的 `main` 函数（由 `#[polyhal::arch_entry]` 标记）：
   - 清 BSS
   - 初始化日志 (`logging::init`)
   - 初始化内核堆分配器 (`heap_allocator::init_heap`)
   - 初始化物理页框分配器 (`frame_allocator::init_frame_allocator`)
   - 初始化 `polyhal::common`（传入 `PageAllocImpl`）
   - 初始化 Trap (`init_trap`)
   - 初始化内存管理 (`mm::init`，激活内核页表 `KERNEL_VMSET`)
   - 初始化网络子系统 (`net::init`)
   - 初始化多核调度结构 (`init_processors`)
   - 初始化文件系统 (`fs::init`，挂载 ext4 根目录、devfs、procfs、tmpfs 等)
   - 加载 `initproc` (`task::add_initproc`)
   - 设置定时器中断，进入调度器 (`task::run_tasks`)
3. `initproc` 从根文件系统读取，设置 `/bin` 下的 busybox 软链接，随后 `execve("/bin/sh")` 启动 shell。

### 6.2 内存管理

- **页表**：基于 `polyhal` 的 `PageTable`，RISC-V 使用 SV39。
- **懒分配**：`mmap` 等映射不立即分配物理页，首次访问触发 `StorePageFault` / `LoadPageFault` 时，在 `trap::handle_page_fault` 中调用 `vm_set.handle_unalloc_page_fault` 按需分配。
- **写时复制（COW）**：`fork` 时共享只读页，写触发 `StorePageFault`，`vm_set.handle_cow_page_fault` 复制物理页并重新映射。
- **栈自动扩展**：若缺页地址落在栈 VMA 附近，自动向下扩展栈区（`try_expand_stack`）。
- **内核空间**：`KERNEL_VMSET` 全局统一管理，包含内核代码/数据段、MMIO、 trampoline 等映射。

### 6.3 进程与线程模型

- **进程（ProcessControlBlock）**：拥有独立地址空间（`vm_set`）、文件描述符表、信号状态、子进程列表。
- **线程（TaskControlBlock）**：共享进程的地址空间与资源，拥有独立的内核栈、trap 上下文、调度上下文（`KContext`）。
- **调度**：基于时间片（约 100ms）的抢占式轮转调度。`Timer` 中断触发 `suspend_current_and_run_next`。
- **同步原语**：内核提供 `SpinLock`、`SpinNoIrqLock`、`SleepLock`、`ReentrantLock`、`BlockingMutex` 等，统一在 `sync/` 下管理。

### 6.4 Trap 与系统调用

- **统一入口**：`polyhal_trap` 提供架构无关的 `TrapFrame` 与 `TrapType`。
- **内核态 Trap**：`#[polyhal::arch_interrupt]` 标记的 `kernel_interrupt` 函数统一处理：
  - `SysCall`：读取参数，调用 `syscall::syscall`，结果写回 `TrapFrameArgs::RET`，刷新 TLB。
  - `StorePageFault` / `LoadPageFault` / `InstructionPageFault`：尝试懒分配/COW/权限修复；失败则发送 `SIGSEGV` 或 `SIGILL`。
  - `Timer`：处理 `alarm`/`itimer_real` 超时、Futex 超时检查、堆/页框统计打印，然后 `suspend_current_and_run_next`。
  - 返回用户态前：调用 `handle_signals` 投递 pending 的异步信号；若进程已标记为 zombie，则直接 `exit_current_and_run_next`。
- **SUM 位**：RISC-V 内核在处理 trap 期间设置 `sstatus.SUM`，允许 S 态访问用户页。

### 6.5 文件系统

- **VFS**：核心抽象包括 `Dentry`、`Inode`、`File`、`SuperBlock`、`FsType`。
- **Dentry Cache**：`GLOBAL_DCACHE` 全局缓存路径到 dentry 的映射，支持 `pin` 防止被回收。
- **已挂载文件系统**：
  - `/` — ext4（基于 `lwext4_rust`，块设备为 `VirtIOBlock`）
  - `/dev` — devfs（tty、null、zero、urandom、rtc、loop、cpu_dma_latency 等）
  - `/dev/shm` — tmpfs（供 `shm_open` 使用）
  - `/etc` — tmpfs（hosts、group、passwd、adjtime、localtime 等）
  - `/proc` — procfs（meminfo、mounts、smaps、self 等）
  - `/tmp` — tmpfs
- **页缓存**：`fs::page::pagecache` 为文件读写提供缓存层（部分文件系统使用）。

### 6.6 网络

- **设备层**：`DeviceManager` 管理所有网络设备。启动时探测 `VirtIO-net`，注册为 `eth0`；同时注册 `loopback`。
- **协议栈**：自研精简 TCP/IP，包含 ARP、IP、ICMP、UDP、TCP。
- **Socket 层**：`socket::SocketManager` 按 `(pid, fd)` 管理 `Socket` 结构，支持 Raw、UDP、TCP 三种类型。Socket 通过 `SocketFile` 实现 `File` trait，融入进程 fd 表。
- **QEMU 网络后端**：支持 `-netdev user`（默认）与 `-netdev bridge`，可通过 `NET_BACKEND` 环境变量切换。

---

## 7. 代码风格与开发规范

- **文档要求**：`os/src/main.rs` 设置了 `#![deny(missing_docs)]`，新建公共模块/函数应尽量添加文档注释。
- **警告处理**：`#![deny(warnings)]` 开启，但部分文件使用 `#![allow(unused)]` / `#![allow(missing_docs)]` 临时豁免。提交前建议消除 warnings。
- ** unsafe 使用**：
  - 仅用于硬件寄存器访问、内联汇编、启动代码、TrapFrame 的裸指针操作。
  - 内存分配、页表遍历尽量封装在安全抽象后。
- **锁与并发**：
  - 优先使用 `SpinNoIrqLock` 保护短临界区；长操作考虑 `SleepLock` 或 `BlockingMutex`。
  - **锁顺序**：避免 `task.inner -> process.inner` 与 `process.inner -> task.inner` 的循环依赖。已有代码在 `exit_current_and_run_next` 等路径中通过提前收集信息、释放锁后再获取下一层锁来避免死锁。
- **日志规范**：使用 `log::error!`、`warn!`、`info!`、`debug!`、`trace!`。`LOG=DEBUG` 等环境变量可在 Makefile 中传入控制输出级别。
- **polyhal 宏**：
  - 内核入口函数使用 `#[polyhal::arch_entry]`。
  - 中断处理函数使用 `#[polyhal::arch_interrupt]`。
  - Per-CPU 数据使用 `#[polyhal::percpu]`。

---

## 8. 测试策略

项目没有根目录 CI/CD，测试以 **手动在 QEMU 中运行** 为主，测试套件分布在多个子目录中：

| 测试套件 | 位置 | 说明 |
|----------|------|------|
| 基础系统调用测试 | `user/src/bin/basictests.rs` | 验证 fork、exec、pipe、yield 等基本功能 |
| 用户综合测试 | `user/src/bin/usertests.rs` | 覆盖更多系统调用场景 |
| libc 功能测试 | `libc-test/` | musl 静态/动态链接测试，生成 `entry-static.exe`、`entry-dynamic.exe` |
| libc 性能基准 | `libc-bench/` | malloc、pthread、regex 等性能测试 |
| lmbench | `docs/lmbench_testcode.md` | 系统调用/文件/进程/上下文切换延迟与带宽指标 |
| iozone | `docs/iozone.md` | 文件系统读写性能 |
| ltp | `ltp/` | Linux Test Project 部分用例（如 access） |
| netperf | `netperf-2.7.0/`、`netperf_testcode.sh` | 网络吞吐与往返延迟测试 |

### 8.1 运行 libc-test 示例

1. 在宿主机（容器）中进入 `libc-test/` 目录，使用交叉编译器编译出静态/动态测试程序。
2. 将生成的 `entry-static.exe`、`entry-dynamic.exe`、`runtest.exe` 及所需 `.so` 放入磁盘镜像的合适位置。
3. 在 Kairix 中执行 `/runtest.exe -w entry-static.exe <test_name>`。

> `docs/libctest.md` 中记录了当前静态/动态测试的通过情况（`[x]` 表示通过，`[ ]` 表示未通过），修改相关模块后应同步更新该文档。

### 8.2 性能基准注意事项

- `lmbench`、`iozone`、`netperf` 的结果受 QEMU 配置（CPU 数、内存、磁盘缓存模式、网络后端）影响显著。
- 对比基准时应保持 `CPU=4`、`NET_BACKEND=user`（或统一的 bridge）等参数一致。
- `docs/lmbench_testcode.md` 中已保存了 musl 与 glibc 下的历史参考数据，新增优化后可追加对比数据。

---

## 9. 安全注意事项

- **用户/内核隔离**：依靠硬件页表和 S/U 模式切换；内核仅在处理 trap 时通过 `SUM` 位访问用户页，完成处理后返回用户态。
- **信号安全**：
  - `SIGSEGV`、`SIGILL` 等同步信号在投递前会被强制从阻塞掩码中移除，防止用户程序通过 `sigprocmask` 阻塞后陷入无限循环。
  - `exit_current_and_run_next` 中处理 `clear_child_tid` 与 `robust_list` 时，使用 `copy_to_user` 或内核直接映射物理地址写零，避免依赖用户页表翻译。
- **页表翻译**：`translated_str`、`translated_byte_buffer` 等函数在遇到未映射页时会尝试触发懒分配；若当前无任务或进程已回收，则返回空值/空字符串，防止 panic。
- **Socket fd 复用**：`SocketManager::add_socket` 在 `(pid, fd)` 已存在时会先关闭旧 socket 再替换，防止 fd 复用导致身份错乱。
- **资源回收**：进程退出时显式回收页框（`FrameTracker`）、关闭 socket（引用计数检查）、刷新文件、释放共享内存附着点，避免内核堆与物理页泄漏。

---

## 10. 部署与交付

- **比赛环境**：通常只需在 `os/` 目录执行 `make run-sdcard`，内核会自动编译并注入用户程序到 `sdcard-rv.img`，随后启动 QEMU。
- **离线构建**：根目录 `make all` 会执行 `cargo vendor`（生成 `os/vendor`、`user/vendor`），然后分别构建两个架构的内核，输出：
  - `os-riscv64.bin`、`os-riscv64`
  - `os-loongarch64.bin`、`os-loongarch64`
- **镜像要求**：确保工作目录中存在对应架构的 `sdcard-rv.img` 或 `sdcard-la.img`，且镜像内已包含 `/glibc`、`/musl`、`busybox` 及测试用例目录。

---

## 11. 给 AI 助手的快速参考

- **新增系统调用**：在 `os/src/syscall/mod.rs` 添加常量编号，在 `os/src/syscall/` 的对应子模块（`fs.rs`、`mm.rs`、`process.rs`、`net.rs` 等）中实现 `sys_xxx`，然后在 `syscall()` 的 `match` 中分发。
- **新增文件系统**：在 `fs/` 下新建目录，实现 `FsType`、`SuperBlock`、`Inode`、`Dentry`、`File` trait；在 `fs/mod.rs` 的 `register_all_fs` 中注册。
- **修改用户程序**：编辑 `user/src/bin/` 下的 `.rs` 文件，执行 `cd user && make build`。
- **调试多核问题**：可在 `main.rs` 的 `kernel_interrupt` 中打印 `get_tp()` 或当前任务 PID，注意使用 `log::error!` 避免被日志级别过滤。
- **内存相关 bug**：若出现缺页 panic，优先检查 `trap::handle_page_fault`、`mm::vm_set::handle_unalloc_page_fault`、`mm::vm_set::handle_cow_page_fault` 的返回值与权限校验逻辑。
