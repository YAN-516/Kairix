<!-- From: /workspace/AGENTS.md -->
# Kairix / KaiRix 操作系统内核

> 本文档面向 AI 编程助手。阅读者被假设对该项目一无所知。所有信息均基于当前仓库的实际内容整理而成。

---

## 项目概览

**Kairix**（亦写作 **KaiRix**）是一款基于 **Rust** 语言和 **RISC-V 64 位（RV64GC）** 架构的 Unix-like 操作系统内核，目标为 POSIX 兼容并能够运行标准 C 程序（包括 musl libc 与 glibc 动态链接的 BusyBox 等）。它通过标准系统调用接口与用户态程序交互。

该项目受 rCore / Chronix 等教学内核启发，但已演进为具备完整 VFS 层、多文件系统支持、网络协议栈、POSIX 信号、多核调度、懒分配与写时复制（COW）、动态链接器加载等功能的功能型内核。

### 核心能力

- **VFS & 多文件系统支持**：采用 VFS-first 设计，同时挂载 ext4（通过 lwext4 C 绑定）、devfs、tempfs、procfs。FAT32 代码存在但尚未作为根文件系统挂载。
- **内存管理**：SV39 分页、懒分配（Lazy Allocation）、写时复制（COW）、`mmap` / `munmap` / `mprotect` / `madvise` / `brk`、内核堆分配器。
- **进程与线程管理**：`fork`、`execve`、`clone`、`waitpid`、多线程基础（`thread_create`、`waittid`）、进程组与会话（`setpgid`、`getpgrp` 等）、动态链接器（PT_INTERP）加载。
- **POSIX 信号**：`kill`、`sigaction`、`sigprocmask`，支持默认、忽略与自定义信号处理函数，在 `trap_return` 返回用户态前投递。
- **网络**：回环设备、ARP、IP、ICMP、UDP、VirtIO-net PCI/MMIO 驱动框架（当前默认未启用）。
- **POSIX 兼容性**：设计目标为运行标准 musl libc 与 glibc 二进制（BusyBox 等）。

---

## 技术栈

| 层级 | 技术 |
|------|------|
| 语言 | Rust（`nightly-2025-01-18`） |
| 内核 Edition | 2024（`os/`、`user/`） |
| `lwext4_rust` Edition | 2018 |
| 目标架构 | `riscv64gc-unknown-none-elf` |
| 额外 Rust 组件 | `rust-src`、`llvm-tools`、`rustfmt`、`clippy` |
| 模拟器 | QEMU `qemu-system-riscv64`（最低要求 7.0.0，推荐 9.2.1） |
| 引导固件 | RustSBI-QEMU（`bootloader/rustsbi-qemu.bin`） |
| 构建工具 | Make + Cargo |
| 容器环境 | `docker.educg.net/cg/os-contest:20250614`（见 `.devcontainer/devcontainer.json`） |

### 内核主要依赖

- `riscv`：RISC-V CSR / 寄存器访问及内联汇编。
- `virtio-drivers`：VirtIO 块设备驱动。
- `lwext4_rust`（本地路径依赖）：ext4 文件系统的 C 库 FFI 绑定。
- `fatfs`（git 依赖）：FAT32 文件系统。
- `xmas-elf`：ELF 解析，用于程序加载与动态链接器识别。
- `buddy_system_allocator`：内核与用户态堆分配。
- `spin`：`no_std` 下的 `Mutex`、`RwLock`。
- `sbi-rt`：SBI 运行时调用。
- `bitflags`、`lazy_static`、`log` 等工具库。

此外，`os/vendor/` 目录下包含 53 个 vendored crate（含 `bindgen`、`virtio-drivers`、`fatfs` 等），说明项目可能需要在离线或隔离环境中构建。

---

## 目录结构

```
/workspace/
├── bootloader/           # RustSBI 引导固件
│   └── rustsbi-qemu.bin
├── os/                   # 内核源码（核心）
│   ├── src/              # 内核 Rust 源码
│   ├── .cargo/           # Cargo 构建配置（目标、链接脚本、rustflags）
│   ├── scripts/          # 辅助脚本（如 QEMU 版本检查）
│   ├── vendor/           # 离线依赖（vendored crates）
│   ├── Cargo.toml
│   ├── Makefile
│   └── build.rs          # 生成 link_app.S（将用户应用嵌入内核，当前主要用磁盘镜像加载）
├── user/                 # 用户态库与应用程序
│   ├── src/bin/          # 用户程序（initproc、shell、测试程序等）
│   ├── .cargo/           # Cargo 构建配置
│   ├── Cargo.toml
│   └── Makefile
├── lwext4_rust/          # lwext4 的 Rust FFI 绑定与复杂构建脚本
│   ├── c/                # C 源码子模块（lwext4）
│   ├── src/
│   ├── build.rs          # C 库编译 + bindgen 生成绑定
│   └── Cargo.toml
├── rust-fatfs/           # FAT32 实现（fork / vendored）
├── easy-fs/              # （当前几乎为空，仅有 target/ 与 Cargo.lock）
├── easy-fs-fuse/         # （当前几乎为空）
├── polyhal/              # （当前几乎为空，未在内核构建中活跃使用）
├── sdcard-rv.img         # 比赛环境预置磁盘镜像
├── sdcard-rv-noltp.img   # 比赛环境预置磁盘镜像（无 LTP）
├── Makefile              # 顶层 Makefile（仅构建内核本身，不打包用户应用）
├── Dockerfile            # 开发环境镜像构建
├── rust-toolchain.toml   # Rust 工具链锁定
├── basic.md              # 早期系统调用实现清单（已部分过时）
├── 随想.md               # 开发随笔
└── AGENTS.md             # 本文件
```

---

## 构建与运行

### 进入 Docker / Dev Container

项目默认在容器 `/workspace`（即本目录）下开发。VS Code Dev Container 配置已存在于 `.devcontainer/devcontainer.json`，指定的镜像为 `docker.educg.net/cg/os-contest:20250614`。

### 常用命令

**在 `os/` 目录下操作（推荐）：**

```bash
cd /workspace/os

# 构建并运行内核，同时将 user/bin 下的应用打包进 ext4 镜像 fs.img
make run

# 使用比赛磁盘镜像 sdcard-rv.img 运行（会自动将 initproc/user_shell/ls/basictests 注入镜像，并补齐动态链接库）
make run-sdcard

# 使用无 LTP 的比赛磁盘镜像运行
make run-sdcard-rv-noltp

# 仅构建内核与用户镜像
make build

# GDB 调试（会启动 tmux 分屏，左侧 QEMU，右侧 GDB）
make debug
```

**顶层 `/workspace/Makefile`**：仅构建内核 ELF 并转换为裸二进制，不编译用户应用、不制作文件系统镜像。

### 构建流程（`os/Makefile`）

1. 进入 `../user` 编译用户态应用（生成 ELF 二进制）。若设置 `TEST=1`，`user/Makefile` 会将 `usertests` 复制为 `initproc`。
2. 创建 64MB 的 ext4 磁盘镜像 `fs.img`（`dd` + `mkfs.ext4`）。
   - 格式化时显式禁用了 `metadata_csum`、`64bit`、`extra_isize`，以确保与 `lwext4` 的最大兼容性。
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

默认 `CPU=1`，可通过环境变量或 Makefile 变量调整。

### 比赛镜像注入（`run-sdcard` / `run-sdcard-rv-noltp`）

`do-patch-sdcard` 目标会将以下文件注入到外部 ext4 镜像中：
- `initproc`、`user_shell`、`ls`、`basictests`
- 创建 `/bin/ls` 硬链接指向 busybox（用于 `which ls` 测试）
- 补齐动态链接库路径：
  - `/lib/ld-linux-riscv64-lp64d.so.1`（glibc loader）
  - `/lib/libc.so.6`、`/lib/libm.so.6`（glibc）
  - `/lib/riscv64-linux-gnu/` 下的同名库（glibc 多路径兼容）
  - `/lib/ld-musl-riscv64-sf.so.1`（musl loader，优先从 `/musl/lib/` 拷贝，不存在则用 `/musl/lib/libc.so` 生成）

---

## 运行时架构

### 启动序列

1. **RustSBI** 将内核加载到物理地址 `0x80200000`，内核链接基址为虚拟地址 `0xFFFFFFC080200000`（见 `os/src/linker-qemu.ld`）。
2. `arch/riscv/entry.rs` / `main.rs` 进入 `pre_main`（裸函数汇编入口） → `main(id, first)`。
3. **CPU 0（`first=true`）**：
   - 清零 BSS
   - 初始化日志子系统
   - 初始化内存管理（内核堆、页帧分配器、内核页表 `KERNEL_VMSET`）
   - 初始化中断处理（统一入口 `__alltraps`）
   - 初始化网络子系统
   - 初始化每 CPU 调度器状态（`init_processors`）
   - 初始化文件系统（注册 ext4/devfs/etc/procfs，挂载根目录、/dev、/etc、/proc）
   - 通过 `sbi::hart_start()` 启动其他 CPU
   - 将 `initproc` 加入就绪队列（`task::add_initproc()`）
4. **其他 CPU**：初始化中断后直接进入调度器。
5. 所有 CPU 执行 `task::run_tasks()`，采用抢占式调度（依赖 Supervisor 时钟中断）。

### 地址空间布局（关键常量）

- `KERNEL_SPACE_OFFSET` / `VIRT_RAM_OFFSET`：`0xffff_ffc0_0000_0000`
- `KERNEL_MEMORY_SPACE`：`0xffff_ffc0_0000_0000` ~ `0xffff_ffff_ffff_ffff`
- `USER_MEMORY_SPACE`：`0x0` ~ `0x3f_ffff_ffff`
- `TRAP_CONTEXT`：`USER_MEMORY_SPACE.1 + 1 - PAGE_SIZE`
- `USER_STACK_BASE`：`TRAP_CONTEXT - MAX_THREAD_NUM * PAGE_SIZE`
- `MMAP_BASE`：`0x4000_0000`
- 页大小：`4096`（`0x1000`）

---

## 内核代码组织（`os/src/`）

### 顶层模块

| 文件 / 模块 | 职责 |
|-------------|------|
| `main.rs` | 入口点、启动序列、模块声明。设置了 `#![deny(missing_docs)]` 与 `#![deny(warnings)]`，但部分子模块用 `#[allow(missing_docs)]` 覆盖。 |
| `config.rs` | 常量（栈大小、页大小、内存布局、最大 CPU 数 `MAX_CPU_NUM = 4`、最大线程数 `MAX_THREAD_NUM = 16` 等）。 |
| `console.rs` / `logging.rs` | 打印宏与 `log` crate 集成。 |
| `sbi.rs` | SBI 固件调用（关机、hart 启动、定时器、获取 hart ID）。 |
| `lang_items.rs` | Panic 处理、分配错误处理。 |
| `arch/riscv/` | 架构相关代码、入口宏、`sfence.vma`、RISC-V 特定汇编封装。 |
| `boards/qemu.rs` | 板级配置（`CLOCK_FREQ = 12500000`、`MEMORY_END = 0x8800_0000`、MMIO 区域）。 |

### 内存管理（`mm/`）

- **`address.rs`**：`VirtAddr`、`PhysAddr`、`VirtPageNum`、`PhysPageNum`、地址转换与步进。
- **`page_table.rs`**：SV39 三级页表、`PTEFlags`、`PageTableEntry`、用户缓冲区翻译（`UserBuffer`、`translated_byte_buffer` 等）。
- **`frame_allocator.rs`**：物理页帧分配（基于 bitmap / stack），含 `FrameTracker`。
- **`heap_allocator.rs`** / **`heap.rs`**：内核堆初始化与测试（`buddy_system_allocator`）。
- **`vm_area.rs`**：内存映射区域（`VMArea`）定义，涵盖堆、栈、ELF 段、mmap、trap context 等类型。
- **`vm_set.rs`**：`UserVMSet` / `KernelVMSet` — 完整地址空间管理。
  - **懒分配**：用户栈、堆、mmap 区域在首次缺页时才分配物理页。
  - **COW**：`fork` 克隆只读页表，首次写入触发物理拷贝。
  - **mmap 支持**：文件映射与匿名映射，集成页缓存（`fs/page/pagecache.rs`）。
  - **动态链接器加载**：`from_elf` 解析 `PT_INTERP` 段，自动加载解释器（glibc / musl ld.so）到用户空间并调整入口点。
- **`exception.rs`**：缺页异常处理 trait（`SetPageFaultException`）。

### 任务管理（`task/`）

- **`process.rs`**：`ProcessControlBlock`（PCB），含 fd_table、信号处理（`SignalHandlers`）、子进程、CWD、`vm_set`、进程组等。
- **`task.rs`**：`TaskControlBlock`（TCB），每线程状态（trap context、内核栈、任务状态 `TaskStatus`）。
- **`manager.rs`**：全局就绪队列、PID→Process 映射、任务唤醒与移除。
- **`processor.rs`**：每 CPU 当前任务跟踪（`current_task`、`current_trap_cx`、`current_process` 等）。
- **`context.rs` / `switch.rs` / `switch.S`**：任务上下文与汇编级上下文切换。
- **`signal.rs`**：POSIX 信号定义、`SignalSet`、`SigAction`、`SignalHandlers`、默认行为处理。
- **`id.rs`**：PID、TID、内核栈分配器。

### 中断处理（`trap/`）

- **`mod.rs`**：统一入口 `__alltraps`，同时处理用户态与内核态陷阱。
  - 通过 `sstatus.SPP` 区分来源。
  - 用户陷阱：系统调用（`ecall`）、缺页异常（Store/Load/Instruction PageFault）、非法指令、时钟中断。
  - 内核陷阱：缺页异常、非法指令、时钟中断（使用独立内核栈帧保存区域，支持嵌套）。
- **`trap.S`**：寄存器保存/恢复汇编代码。
- **`context.rs`**：`TrapContext` — 完整通用寄存器组 + `sepc`、`sstatus`、`kernel_sp`。

### 文件系统（`fs/`）

采用 **VFS-first** 设计，参考 Linux / Chronix 风格：

#### VFS 层（`fs/vfs/`）

- **`dentry.rs`**：`Dentry` trait + `DentryInner`（名称、父节点、子节点 BTreeMap、inode 引用）。
- **`inode.rs`**：`Inode` trait（元数据、读写、截断、查找、创建、删除、重命名等）。
- **`file.rs`**：`File` trait（读写、定位、打开、关闭、刷新、`get_cache_frame` 用于 mmap 页缓存）。
- **`superblock.rs`**：`SuperBlock` trait。
- **`fstype.rs`**：`FsType` trait，用于文件系统注册与挂载。
- **`dcache.rs`**：全局 dentry 缓存 `GLOBAL_DCACHE`（路径字符串 → `Arc<dyn Dentry>`）。
- **`path.rs`**：路径解析（绝对 / 相对路径分解）。
- **`kstat.rs`**：`Kstat` 结构，用于 `fstat` / `fstatat`。
- **`mount.rs`**：挂载点管理相关结构。

#### 具体文件系统

- **`lwext4/`**：ext4 实现，包装 `lwext4_rust`。
  - `disk.rs`：块设备适配器（`dyn BlockDevice`）。
  - `inode.rs`、`dentry.rs`、`file.rs`、`superblock.rs`、`fstype.rs`：VFS 适配层。
  - `ext4/`：基于 lwext4 绑定的目录/文件辅助函数。
- **`devfs/`**：设备文件系统。
  - 现有设备：`/dev/null`、`/dev/tty`、`/dev/urandom`（代码存在，init 中尚未启用）、`/dev/rtc`、`/dev/rtc0`。
- **`tempfs/`**：基于 RAM 的文件系统（用于 `/etc`）。
- **`fat32/`**：FAT32 实现（部分完成，基于 `rust-fatfs`，代码存在但尚未挂载为根文件系统）。
- **`procfs/`**：进程文件系统。
  - 现有文件：`/proc/meminfo`、`/proc/mounts`。
- **`page/`**：mmap 页缓存（`pagecache.rs`）。
- **`etc/mod.rs`**：初始化 `/etc` 下的文件（`passwd`、`adjtime`、`group`、`localtime` 等，当前为占位空文件）。

#### 文件系统初始化（`fs/mod.rs`）

1. 注册 `ext4`、`devfs`、`etc`（tempfs）、`procfs`。
2. 使用 `BLOCK_DEVICE`（`VirtIOBlock`）在 `/` 挂载根 ext4。
3. 在 `/dev` 挂载 devfs 并调用 `init_devfs()` 创建设备节点。
4. 在 `/etc` 挂载 tempfs 并调用 `init_etcfs()` 填充内容。
5. 在 `/proc` 挂载 procfs 并调用 `init_procfs()`。

### 系统调用（`syscall/`）

在 `syscall/mod.rs` 中按编号分发，子模块包括：

- **`fs.rs`**：`openat`、`close`、`read`、`write`、`writev`、`readv`、`getdents64`、`mkdirat`、`unlinkat`、`linkat`、`chdir`、`getcwd`、`fstat`、`fstatat`、`dup`、`dup2`、`pipe`、`fcntl`、`ioctl`、`mount`、`umount2`、`fsync`、`sendfile`、`statfs`、`faccessat`、`lseek`、`utimensat`、`renameat2`。
- **`process.rs`**：`exit`、`exit_group`、`fork`、`clone`、`execve`、`waitpid`、`yield`、`getpid`、`getppid`、`getuid`、`geteuid`、`getegid`、`gettid`、`setpgid`、`setpgrp`、`getpgid`、`getpgrp`、`set_tid_address`、`set_robust_list`。
- **`mm.rs`**：`mmap`、`munmap`、`mprotect`、`brk`、`madvise`。
- **`signal.rs`**：`kill`、`sigaction`、`sigprocmask`。
- **`time.rs`**：`get_time`、`times`、`sleep`、`clock_gettime`。
- **`net.rs`**：`socket`、`bind`、`sendto`、`recvfrom`。
- **`thread.rs`**：`thread_create`、`waittid`。
- **`info.rs`**：`uname`、`sysinfo`、`syslog`（桩）。
- **`pipe.rs`**：管道创建（`sys_pipe`）。
- **`misc.rs`**：`ppoll` 等辅助或桩实现。

### 网络（`net/`）

- **`device.rs`**：`DeviceManager`，管理网络接口。
- **`loopback.rs`**：回环网络设备。
- **`arp.rs`**、**`ethernet.rs`**、**`ip.rs`**、**`icmp.rs`**、**`udp.rs`**、**`neighbor.rs`**、**`route.rs`**：TCP/IP 协议栈分层实现。
- **`skb.rs`**：Socket Buffer（网络包）管理。
- **`virtio/`**：VirtIO-net PCI/MMIO 驱动与 virtqueue 管理（`config.rs`、`device.rs`、`pci.rs`、`virtqueue.rs`）。

### 驱动（`drivers/`）

- **`block/virtio_blk.rs`**：VirtIO 块设备驱动。自定义 `VirtioHal` 通过内核页帧分配器实现 DMA 内存分配。

### Socket 层（`socket/`）

- **`raw.rs`**、**`udp.rs`**：Socket 抽象层，供系统调用与网络协议栈交互。

---

## 用户态代码（`user/`）

### `user_lib`（`src/lib.rs`）

一个 `no_std` 用户态运行时库，提供：

- `_start` 入口点（初始化 32KB 用户堆、调用 `main()`、然后 `exit()`）。
- 系统调用封装（`open`、`read`、`write`、`fork`、`execve`、`mmap`、`socket`、`bind`、`sendto`、`recvfrom` 等）。
- `SignalSet`、`SigAction`、`SigHandler` 定义。
- `OpenFlags` bitflags（`RDONLY`、`WRONLY`、`RDWR`、`O_CREAT`、`O_TRUNC`、`O_DIRECTORY`）。

### 应用程序（`src/bin/`）

| 程序 | 作用 |
|------|------|
| `initproc.rs` | PID 1，从文件系统加载并 exec `user_shell`，负责回收僵尸进程。 |
| `user_shell.rs` | 交互式 shell，支持内建命令（`cd`、`exit`、`help`）及通过 `PATH` 搜索执行外部命令。已修复进程组切换以支持前台/后台作业。 |
| `ls.rs` | 简易 `ls` 实现。 |
| `basictests.rs` | musl libc 基础测试套件（fork + execve 运行多个外部测试用例）。 |
| `usertests.rs` / `usertests_simple.rs` | 内核自带用户测试套件（覆盖文件、fork、sleep、yield 等）。 |
| `ping.rs` | 网络 ping 工具。 |
| `signal_test.rs` | 信号处理测试。 |
| `hello_world.rs`、`forktest.rs`、`yield.rs` 等 | 各类简单功能测试。 |

`user/Makefile` 将 `src/bin/` 下的所有 `.rs` 编译为 ELF 二进制。若设置 `TEST=1`，会将 `usertests` 复制为 `initproc`。

---

## 开发约定

### Cargo 配置

- `os/.cargo/config.toml` 与 `user/.cargo/config.toml`：
  - 设定构建目标为 `riscv64gc-unknown-none-elf`。
  - 链接脚本 `-Tsrc/linker.ld`。
  - 强制启用帧指针（`-Cforce-frame-pointers=yes`）。

### 构建脚本

- **`os/build.rs`**：生成 `src/link_app.S`，将用户应用二进制嵌入内核数据段。（当前主要使用磁盘镜像加载，该机制为旧有兼容。）
- **`lwext4_rust/build.rs`**：复杂构建脚本，负责：
  - 初始化 `c/lwext4` git 子模块。
  - 打补丁（`c/lwext4-make.patch`）。
  - 通过 `make musl-generic` 构建静态 C 库。
  - 使用 `bindgen` 生成 Rust FFI 绑定（`src/bindings.rs`）。

### 代码风格

- **注释语言**：团队约定以**中文**为主（可参考 `随想.md`），但部分模块存在中英混合。新增代码建议优先使用中文注释。
- `main.rs` 中设置了 `#![deny(missing_docs)]`，但大量子模块用 `#[allow(missing_docs)]` 覆盖。
- 广泛使用 `UPSafeCell<T>` 进行内核态内部可变性管理。**注意**：当前实现已改为内部包裹 `spin::Mutex<T>`（而非早期基于 `RefCell` 的版本）。
- 跨 CPU 同步主要使用 `spin::Mutex`。
- 内核与用户态均为 `#![no_std]`，通过 `extern crate alloc` 使用堆分配。

### 架构决策

1. **全 `no_std`**：内核与用户态均禁用标准库。
2. **VFS 优先**：所有存储通过 `Dentry`、`Inode`、`File`、`SuperBlock` trait 抽象，允许多文件系统并存。
3. **懒内存分配**：用户栈、堆、mmap 区域在首次缺页时才分配物理页。
4. **COW fork**：`fork()` 创建只读共享映射，首次写入触发页拷贝。
5. **统一中断入口**：单个汇编入口 `__alltraps` 处理用户→内核与内核→内核陷阱，按需切换栈。
6. **多核就绪**：支持最多 4 核（`MAX_CPU_NUM = 4`），每核拥有独立调度器与当前任务指针。
7. **信号在 trap 返回时投递**：`trap_return()` 返回用户态前检查待处理信号，支持默认、忽略与自定义处理函数。
8. **根文件系统兼容性**：制作 ext4 镜像时禁用 `metadata_csum`、`64bit`、`extra_isize`，以确保 `lwext4` 能正确读写。
9. **动态链接器支持**：`execve` 加载 ELF 时检测 `PT_INTERP`，自动加载解释器并映射到用户空间，调整程序入口点。已验证 musl 与 glibc 动态链接 BusyBox 可运行。

---

## 测试策略

- **用户态测试二进制**：
  - `usertests`：内核自带测试套件，覆盖文件、进程、睡眠、yield 等基础功能。
  - `basictests`：musl libc 兼容性测试，会依次 `execve` 运行 `chdir`、`clone`、`close`、`dup`、`dup2`、`execve`、`exit`、`fork`、`fstat`、`getcwd`、`getdents`、`getpid`、`getppid`、`gettimeofday`、`mkdir_`、`mmap`、`mount`、`munmap`、`open`、`openat`、`pipe`、`read`、`sleep`、`test_echo`、`times`、`umount`、`uname`、`unlink`、`wait`、`waitpid`、`write`、`yield`、`brk` 等外部测试程序。
- **手动 shell 测试**：`user_shell` 支持运行任意命令进行验证。
- **比赛环境测试**：`make run-sdcard` 使用 `sdcard-rv.img` 运行外部测试磁盘，自动将必要文件与动态链接库注入镜像。
- `basic.md` 中记录了部分早期系统调用的实现 checklist（`[x]` 表示已实现，`[×]` 表示待完善），但内容已部分过时，实际实现以源码为准。

---

## 当前分支上下文（`busybox-fix`）

仓库当前位于 `busybox-fix` 分支，工作树干净，领先远程 1 个提交。

近期活跃工作（按 `git log` 摘要）：
- **动态链接支持**：内核 ELF 加载器新增 `PT_INTERP` 解析，支持 musl 与 glibc 动态链接器加载。`Makefile` 的 `do-patch-sdcard` 自动补齐 `/lib/` 下的动态库路径。
- **mmap 语义修复**：增加参数校验、`MAP_FIXED` 区间裁剪、`munmap` 按区间生效、`mprotect` 恢复实际生效逻辑、`MAP_PRIVATE` 文件映射缺页时从 page cache 拷贝到私有 frame。
- **新增系统调用**：`readv`（65）、`lseek`（62）、`renameat2`（276）、`utimensat`（88）、`set_tid_address`（96）、`set_robust_list`（99）、`ppoll`（73）、`getegid`（177）。
- **execve 回退修复**：仅“非 ELF”才走 busybox sh 回退，ELF 加载失败不再当脚本执行。
- **copy_to_user 返回值修复**：修复 `ls` 等工具因返回值非 0 导致的异常行为。
- **sys_brk 返回值修复**：修复 brk 系统调用返回错误地址的 bug。
- **glibc 兼容性修复**：修复 glibc 目录项被错认的 bug、修复 glibc 启动错误。
- 合并此前的网络信号修复与 BusyBox 文件系统修复。

### 已知待办与注意事项（参考 `fs/readme.md`）

- `dentry` 部分**暂时没有加锁**，后续需统一锁策略。
- **软连接**尚未实现（可能需要修改底层 ext4）。
- **页面置换算法**尚未实现。
- **`/dev/urandom`** 代码存在但 init 中尚未启用。
- **多用户组**尚未实现（`getuid`/`geteuid`/`getegid` 固定返回 root/0）。
- **fixed map**（`MAP_FIXED` 的完整语义）仍有边界情况待完善。
- 进程退出时 fd_table 的关闭策略与 Linux 存在差异，部分场景需继续调整。

---

## 部署与容器

- **Docker**：`Dockerfile` 构建多阶段镜像，包含 QEMU、Rust 工具链、GDB。基础镜像为 `ubuntu:20.04`，使用清华镜像源加速。
- **Dev Container**：`.devcontainer/devcontainer.json` 指定了比赛/开发用容器镜像 `docker.educg.net/cg/os-contest:20250614`，并预装了 `e2fsprogs`、`e2tools`、`openssh-client` 等工具。
- 项目设计在容器内以 `/workspace` 挂载方式运行。

---

## 安全与稳定性提示

- 内核运行在 `S` 态（Supervisor mode），没有用户态/内核态的 KASLR 或栈金丝雀保护。
- 内存安全主要依赖 Rust 的所有权与借用检查；但部分区域（如 `unsafe` 汇编、MMIO、DMA 缓冲区、C FFI）仍需人工审查。
- `lwext4_rust` 包含大量 `unsafe` FFI 调用，对 C 库的输入校验（如路径长度、inode 有效性）需要保持在 Rust 侧完成。
- `UPSafeCell` 虽然提供了内部可变性，但本质上是自旋锁；在持有锁期间不应执行可能阻塞或触发缺页的操作，否则容易导致死锁（`fstatat` 死锁即为前车之鉴）。
- **动态链接器加载**通过内核直接读取解释器 ELF 并映射到用户空间，未经过完整的权限隔离，需确保解释器路径校验严格。
