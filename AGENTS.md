# Kairix / KaiRix 操作系统内核

> 本文档面向 AI 编程助手。阅读者被假设对该项目一无所知。

---

## 项目概览

**Kairix**（亦写作 **KaiRix**）是一款基于 **Rust** 语言和 **RISC-V 64 位（RV64GC）** 架构的现代化操作系统内核，定位为 Unix-like / POSIX 兼容内核。它能够运行标准 C 程序（包括 BusyBox），通过标准系统调用接口与用户态程序交互。

该项目受 rCore / Chronix 等教学内核启发，但已演进为具备网络、信号、多核支持、完整 VFS 层等特性的功能型内核。

### 核心能力

- **VFS & 多文件系统支持**：同时挂载 ext4（通过 lwext4 C 绑定）、FAT32（rust-fatfs）、devfs、tempfs、procfs（部分）。
- **内存管理**：SV39 分页、懒分配（Lazy Allocation）、写时复制（COW）、mmap / munmap、内核堆分配器。
- **进程管理**：fork、execve、clone、waitpid、POSIX 信号、进程组、多线程基础。
- **网络**：回环设备、ARP、IP、ICMP、UDP、VirtIO-net 驱动框架。
- **POSIX 兼容性**：设计目标为运行标准 C 二进制（BusyBox、musl libc 程序）。

---

## 技术栈

| 层级 | 技术 |
|------|------|
| 语言 | Rust（`nightly-2025-01-18`，内核与用户态使用 edition 2024） |
| 目标架构 | `riscv64gc-unknown-none-elf` |
| 额外 Rust 组件 | `rust-src`、`llvm-tools`、`rustfmt`、`clippy` |
| 模拟器 | QEMU `qemu-system-riscv64`（文档推荐 9.2.1） |
| 引导固件 | RustSBI-QEMU（`bootloader/rustsbi-qemu.bin`） |
| 构建工具 | Make + Cargo |
| 容器环境 | `docker.educg.net/cg/os-contest:20250614`（见 `.devcontainer/devcontainer.json`） |

### 内核主要依赖

- `riscv`：RISC-V CSR / 寄存器访问及内联汇编。
- `virtio-drivers`：VirtIO 块设备与网络设备驱动。
- `lwext4_rust`（本地路径依赖）：ext4 文件系统。
- `fatfs`（git 依赖）：FAT32 文件系统。
- `xmas-elf`：ELF 解析，用于程序加载。
- `buddy_system_allocator`：内核堆分配。
- `spin`：`no_std` 下的 `Mutex`、`RwLock`。
- `sbi-rt`：SBI 运行时调用。
- `bitflags`、`lazy_static`、`log` 等工具库。

此外，`os/vendor/` 目录下包含约 60 个 vendored crate（含 `bindgen`、`virtio-drivers`、`fatfs` 等），说明项目可能需要在离线或隔离环境中构建。

---

## 目录结构

```
/workspace/
├── bootloader/           # RustSBI 引导固件
│   └── rustsbi-qemu.bin
├── os/                   # 内核源码（核心）
│   ├── src/
│   ├── .cargo/
│   ├── scripts/
│   ├── vendor/           # 离线依赖
│   ├── Cargo.toml
│   ├── Makefile
│   └── build.rs
├── user/                 # 用户态库与应用程序
│   ├── src/bin/          # 用户程序（initproc、shell、测试程序等）
│   ├── .cargo/
│   ├── Cargo.toml
│   └── Makefile
├── lwext4_rust/          # lwext4 的 Rust FFI 绑定与构建脚本
│   ├── c/                # C 源码子模块
│   ├── src/
│   ├── build.rs          # 复杂的 C 库编译 + bindgen 生成
│   └── Cargo.toml
├── rust-fatfs/           # FAT32 实现（fork / vendored）
├── easy-fs/              # （当前几乎为空，仅有 target/ 与 Cargo.lock）
├── easy-fs-fuse/         # （当前几乎为空）
├── polyhal/              # （当前几乎为空，未在内核构建中活跃使用）
├── sdcard-rv.img         # 比赛环境预置磁盘镜像
├── Makefile              # 顶层 Makefile（仅内核，不打包用户应用）
├── Dockerfile            # 开发环境镜像构建
├── rust-toolchain.toml   # Rust 工具链锁定
└── AGENTS.md             # 本文件
```

---

## 构建与运行

### 进入 Docker / Dev Container

项目默认在容器 `/workspace`（即本目录）下开发。VS Code Dev Container 配置已存在于 `.devcontainer/devcontainer.json`。

### 常用命令

**在 `os/` 目录下操作（推荐）：**

```bash
cd /workspace/os

# 构建并运行内核，同时将 user/bin 下的应用打包进 ext4 镜像 fs.img
make run

# 使用比赛磁盘镜像 sdcard-rv.img 运行
make run-sdcard

# 仅构建
make build

# GDB 调试（会启动 tmux 分屏）
make debug
```

**顶层 `/workspace/Makefile`**：仅构建内核本身，不打包用户应用。

### 构建流程（`os/Makefile`）

1. 进入 `../user` 编译用户态应用（生成 ELF 二进制）。
2. 创建 64MB 的 ext4 磁盘镜像 `fs.img`（`dd` + `mkfs.ext4`）。
3. 使用 `e2tools`（`e2cp`）将用户二进制复制进 `fs.img`。
4. `cargo build --release` 编译内核。
5. `rust-objcopy` 将 ELF 转换为裸二进制 `os.bin`。
6. 启动 QEMU，将 `fs.img` 挂载为 VirtIO 块设备。

### QEMU 启动参数（摘要）

```bash
qemu-system-riscv64 \
  -machine virt \
  -nographic \
  -bios ../bootloader/rustsbi-qemu.bin \
  -device loader,file=target/riscv64gc-unknown-none-elf/release/os.bin,addr=0x80200000 \
  -drive file=fs.img,if=none,format=raw,id=x0 \
  -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
  -smp $(CPU)
```

---

## 运行时架构

### 启动序列

1. **RustSBI** 将内核加载到物理地址 `0x80200000`。
2. `arch/riscv/entry.rs` / `main.rs` 进入 `pre_main` → `main`。
3. **CPU 0（first=true）**：
   - 清零 BSS
   - 初始化日志
   - 初始化内存管理（堆、页帧分配器、内核页表）
   - 初始化中断处理
   - 初始化网络子系统
   - 初始化文件系统（挂载根目录 ext4、devfs、tempfs 等）
   - 通过 `sbi::hart_start()` 启动其他 CPU
   - 将 `initproc` 加入调度器
4. **其他 CPU**：初始化中断后进入调度器。
5. 所有 CPU 执行 `task::run_tasks()`，采用协作 / 抢占式调度（依赖时钟中断）。

---

## 内核代码组织（`os/src/`）

### 顶层模块

| 文件 / 模块 | 职责 |
|-------------|------|
| `main.rs` | 入口点、启动序列 |
| `config.rs` | 常量（栈大小、页大小、内存布局） |
| `console.rs` / `logging.rs` | 打印宏与 `log` crate 集成 |
| `sbi.rs` | SBI 固件调用（关机、hart 启动、定时器） |
| `lang_items.rs` | Panic 处理、分配错误处理 |
| `arch/riscv/` | 架构相关代码、入口宏、`sfence.vma` |
| `boards/qemu.rs` | 板级配置（时钟频率、内存终点、MMIO 区域） |

### 内存管理（`mm/`）

- **`address.rs`**：`VirtAddr`、`PhysAddr`、`VirtPageNum`、`PhysPageNum`。
- **`page_table.rs`**：SV39 三级页表、`PTEFlags`、`PageTableEntry`。
- **`frame_allocator.rs`**：物理页帧分配（基于 bitmap / stack）。
- **`heap_allocator.rs`**：内核堆（`buddy_system_allocator`）。
- **`vm_area.rs`**：内存映射区域（堆、栈、ELF、mmap、trap context）。
- **`vm_set.rs`**：`UserVMSet` / `KernelVMSet` — 完整地址空间管理。
  - **懒分配**：首次缺页时才分配物理页。
  - **COW**：`fork` 克隆只读页表，首次写入触发物理拷贝。
  - **mmap 支持**：文件映射与匿名映射，集成页缓存。
- **`exception.rs`**：缺页异常类型与处理 trait。

### 任务管理（`task/`）

- **`process.rs`**：`ProcessControlBlock`（PCB），含 fd_table、信号处理、子进程、CWD、VM set。
- **`task.rs`**：`TaskControlBlock`（TCB），每线程状态（trap context、内核栈、状态）。
- **`manager.rs`**：全局就绪队列、PID→Process 映射。
- **`processor.rs`**：每 CPU 当前任务跟踪。
- **`context.rs` / `switch.rs` / `switch.S`**：任务上下文与汇编级上下文切换。
- **`signal.rs`**：POSIX 信号定义、`SignalSet`、`SigAction`、`SignalHandlers`。
- **`id.rs`**：PID / 内核栈 / TID 分配器。

### 中断处理（`trap/`）

- **`mod.rs`**：统一入口 `__alltraps`，同时处理用户态与内核态陷阱。
  - 通过 `sstatus.SPP` 区分来源。
  - 用户陷阱：系统调用（`ecall`）、缺页、非法指令、时钟中断。
  - 内核陷阱：缺页、时钟中断（使用独立内核栈帧）。
- **`trap.S`**：寄存器保存/恢复汇编。
- **`context.rs`**：`TrapContext` — 完整寄存器组 + `sepc`、`sstatus`、`kernel_sp`。

### 文件系统（`fs/`）

采用 **VFS-first** 设计，参考 Linux / Chronix：

#### VFS 层（`fs/vfs/`）

- **`dentry.rs`**：`Dentry` trait + `DentryInner`（名称、父节点、子节点 BTreeMap、inode）。
- **`inode.rs`**：`Inode` trait（元数据、读写、截断、查找、创建、删除）。
- **`file.rs`**：`File` trait（读写、定位、打开、关闭、刷新、`get_cache_frame` 用于 mmap）。
- **`superblock.rs`**：`SuperBlock` trait。
- **`fstype.rs`**：`FSType` trait，用于文件系统注册。
- **`dcache.rs`**：全局 dentry 缓存 `GLOBAL_DCACHE`。
- **`path.rs`**：路径解析（绝对 / 相对）。
- **`kstat.rs`**：`Kstat` 结构，用于 `fstat` / `fstatat`。

#### 具体文件系统

- **`lwext4/`**：ext4 实现，包装 `lwext4_rust`。
  - `disk.rs`：块设备适配器。
  - `inode.rs`、`dentry.rs`、`file.rs`、`superblock.rs`、`fstype.rs`：VFS 适配层。
  - `ext4/`：基于 lwext4 绑定的目录/文件辅助函数。
- **`devfs/`**：设备文件系统（`/dev/null`、`/dev/tty`、`/dev/urandom`）。
- **`tempfs/`**：基于 RAM 的文件系统（用于 `/etc`）。
- **`fat32/`**：FAT32 实现（部分完成，基于 `rust-fatfs`）。
- **`procfs/`**：进程文件系统（极简）。
- **`page/`**：mmap 页缓存（`pagecache.rs`）。

#### 文件系统初始化（`fs/mod.rs`）

1. 注册 `ext4`、`devfs`、`etc`（tempfs）。
2. 使用 `BLOCK_DEVICE` 在 `/` 挂载根 ext4。
3. 在 `/dev` 挂载 devfs。
4. 在 `/etc` 挂载 tempfs 并调用 `init_etcfs()` 填充内容。

### 系统调用（`syscall/`）

在 `syscall/mod.rs` 中分发，子模块包括：

- **`fs.rs`**：`openat`、`close`、`read`、`write`、`getdents64`、`mkdirat`、`unlinkat`、`linkat`、`chdir`、`getcwd`、`fstat`、`fstatat`、`dup`、`dup2`、`pipe`、`fcntl`、`ioctl`、`mount`、`umount2`、`fsync`。
- **`process.rs`**：`exit`、`exit_group`、`fork`、`clone`、`execve`、`waitpid`、`yield`、`getpid`、`getppid`、`getuid`、`geteuid`、`gettid`、`setpgid`、`setpgrp`、`getpgid`、`getpgrp`。
- **`mm.rs`**：`mmap`、`munmap`、`mprotect`、`brk`、`madvise`。
- **`signal.rs`**：`kill`、`sigaction`、`sigprocmask`。
- **`time.rs`**：`get_time`、`times`、`sleep`、`clock_gettime`。
- **`net.rs`**：`socket`、`bind`、`sendto`、`recvfrom`。
- **`thread.rs`**：`thread_create`、`waittid`。
- **`info.rs`**：`uname`。
- **`pipe.rs`**：管道创建。

### 网络（`net/`）

- **`device.rs`**：`DeviceManager`。
- **`loopback.rs`**：回环网络设备。
- **`arp.rs`**、**`ethernet.rs`**、**`ip.rs`**、**`icmp.rs`**、**`udp.rs`**、**`neighbor.rs`**、**`route.rs`**：TCP/IP 协议栈分层。
- **`skb.rs`**：Socket Buffer（包）管理。
- **`virtio/`**：VirtIO-net PCI/MMIO 驱动与 virtqueue 管理。

### 驱动（`drivers/`）

- **`block/virtio_blk.rs`**：VirtIO 块设备驱动。自定义 `VirtioHal` 通过内核页帧分配器实现 DMA 内存分配。

---

## 用户态代码（`user/`）

### `user_lib`（`src/lib.rs`）

一个 `no_std` 用户态运行时库，提供：

- `_start` 入口点（初始化堆、调用 `main()`、然后 `exit()`）。
- 系统调用封装（`open`、`read`、`write`、`fork`、`execve`、`mmap`、`socket` 等）。
- `SignalSet`、`SigAction`、`SigHandler` 定义。
- `OpenFlags` bitflags。

### 应用程序（`src/bin/`）

| 程序 | 作用 |
|------|------|
| `initproc.rs` | PID 1，fork 并 exec `user_shell`，回收僵尸进程。 |
| `user_shell.rs` | 交互式 shell，支持内建命令（`cd`、`exit`、`help`）及通过 `PATH` 搜索执行外部命令。 |
| `ls.rs` | 简易 `ls` 实现。 |
| `basictests.rs` / `usertests.rs` | 测试套件。 |
| `ping.rs` | 网络 ping 工具。 |
| `signal_test.rs` | 信号处理测试。 |
| `hello_world.rs`、`forktest.rs`、`yield.rs` 等 | 各类简单测试。 |

`user/Makefile` 将 `src/bin/` 下的所有 `.rs` 编译为 ELF 二进制。若设置 `TEST=1`，会将 `usertests` 复制为 `initproc`。

---

## 开发约定

### Cargo 配置

- `os/.cargo/config.toml` 与 `user/.cargo/config.toml`：
  - 设定构建目标为 `riscv64gc-unknown-none-elf`。
  - 链接脚本 `-Tsrc/linker.ld`。
  - 强制启用帧指针（`force-frame-pointers`）。

### 构建脚本

- **`os/build.rs`**：生成 `src/link_app.S`，将用户应用二进制嵌入内核数据段。（当前主要使用磁盘镜像加载，该机制为旧有兼容。）
- **`lwext4_rust/build.rs`**：复杂构建脚本，负责：
  - 初始化 `c/lwext4` git 子模块。
  - 打补丁。
  - 通过 `make musl-generic` 构建静态 C 库。
  - 使用 `bindgen` 生成 Rust FFI 绑定。

### 代码风格

- **注释语言**：团队约定以**中文**为主（见 `随想.md`），但部分模块存在中英混合。
- `main.rs` 中设置了 `#![deny(missing_docs)]`，但大量子模块用 `#[allow(missing_docs)]` 覆盖。
- 广泛使用 `UPSafeCell<T>`（基于 `RefCell` 或自旋锁的内部可变性包装）进行内核态单核内部可变性管理。
- 跨 CPU 同步主要使用 `spin::Mutex`。

### 架构决策

1. **全 `no_std`**：内核与用户态均为 `#![no_std]`，通过 `extern crate alloc` 使用堆分配。
2. **VFS 优先**：所有存储通过 `Dentry`、`Inode`、`File`、`SuperBlock` trait 抽象，允许多文件系统并存。
3. **懒内存分配**：用户栈、堆、mmap 区域在首次缺页时才分配物理页。
4. **COW fork**：`fork()` 创建只读共享映射，首次写入触发页拷贝。
5. **统一中断入口**：单个汇编入口处理用户→内核与内核→内核陷阱，按需切换栈。
6. **多核就绪**：支持最多 4 核（`MAX_CPU_NUM = 4`），每核拥有独立调度器与当前任务指针。
7. **信号在 trap 返回时投递**：`trap_return()` 返回用户态前检查待处理信号，支持默认、忽略与自定义处理函数。

---

## 测试策略

- **用户态测试二进制**：`basictests`、`usertests`、`usertests_simple`。
- **手动 shell 测试**：`user_shell` 支持运行任意命令进行验证。
- **比赛环境测试**：`make run-sdcard` 使用 `sdcard-rv.img` 运行外部测试磁盘。
- `basic.md` 中记录了部分系统调用的实现 checklist（`[x]` 表示已实现，`[×]` 表示待完善）。

---

## 部署与容器

- **Docker**：`Dockerfile` 构建多阶段镜像，包含 QEMU、Rust 工具链、GDB。
- **Dev Container**：`.devcontainer/devcontainer.json` 指定了比赛/开发用容器镜像。
- 项目设计在容器内以 `/workspace` 挂载方式运行。

---

## 当前分支上下文（`busybox-fix`）

仓库当前位于 `busybox-fix` 分支，存在未提交修改的文件：
- `os/src/fs/readme.md`
- `os/src/syscall/fs.rs`
- `os/src/syscall/mod.rs`

近期提交显示活跃工作包括：
- 修复 `fstatat` 死锁。
- 修正 `ls` 与 `sys_getdents64` 以符合 Linux 标准。
- 增加 `/etc` 支持，修改 inode `ino` 分配逻辑。
- 合并网络信号修复与 BusyBox 文件系统修复。
- 增加 `geteuid` 系统调用（默认返回 root/0）。
- 修复 `user_shell` 进程组切换，以支持前台/后台作业。
