<!-- From: /workspace/AGENTS.md -->
# Kairix / KaiRix 操作系统内核

> 本文档面向 AI 编程助手。阅读者被假设对该项目一无所知。所有信息均基于当前仓库的实际内容整理而成。

---

## 项目概览

**Kairix**（亦写作 **KaiRix**）是一款基于 **Rust** 语言的 Unix-like 操作系统内核，设计目标为 POSIX 兼容并能够运行标准 C 程序（包括 musl libc 与 glibc 动态链接的 BusyBox 等）。它通过标准系统调用接口与用户态程序交互。

该项目受 rCore / Chronix 等教学内核启发，但已演进为具备完整 VFS 层、多文件系统支持、网络协议栈、POSIX 信号、多核调度、懒分配与写时复制（COW）、动态链接器加载、页缓存 LRU 淘汰、dentry 缓存 LRU 淘汰等功能的功能型内核。

### 核心能力

- **多架构支持**：通过 polyhal 硬件抽象层同时支持 **RISC-V 64（RV64GC）** 与 **LoongArch64** 架构，QEMU 模拟运行。
- **VFS & 多文件系统支持**：采用 VFS-first 设计，同时挂载 ext4（通过 lwext4 C 绑定）、devfs、tempfs、procfs、tmpfs。FAT32 代码存在但尚未作为根文件系统挂载。
- **内存管理**：SV39 / LoongArch 分页、懒分配（Lazy Allocation）、写时复制（COW）、`mmap` / `munmap` / `mprotect` / `madvise` / `msync` / `brk`、内核堆分配器。
- **进程与线程管理**：`fork`、`execve`、`clone`、`waitpid`、多线程基础（`thread_create`、`waittid`）、进程组与会话（`setpgid`、`getpgrp` 等）、动态链接器（PT_INTERP）加载。
- **POSIX 信号**：`kill`、`sigaction`、`sigprocmask`、`rt_sigtimedwait`、`rt_sigsuspend`，支持默认、忽略与自定义信号处理函数，在 trap 返回用户态前投递。阻塞任务也可被终止信号终止。
- **网络**：回环设备、ARP、IP、ICMP、UDP、TCP 基础、VirtIO-net PCI/MMIO 驱动框架（`main.rs` 中 `net::init()` 已启用，默认初始化）。
- **POSIX 兼容性**：设计目标为运行标准 musl libc 与 glibc 二进制（BusyBox 等）。
- **缓存与淘汰**：页缓存（Page Cache）与 Dentry 缓存均实现 LRU 淘汰机制。
- **软连接**：已实现符号链接（`symlinkat` / `readlinkat`）。

---

## 技术栈

| 层级 | 技术 |
|------|------|
| 语言 | Rust（`nightly-2025-01-18`） |
| 内核 Edition | 2024（`os/`、`user/`） |
| `lwext4_rust` Edition | 2018 |
| 目标架构 | `riscv64gc-unknown-none-elf`、`loongarch64-unknown-none` |
| 额外 Rust 组件 | `rust-src`、`llvm-tools`、`rustfmt`、`clippy` |
| 模拟器 | QEMU `qemu-system-riscv64` / `qemu-system-loongarch64`（推荐 9.x） |
| 引导固件 | RISC-V: RustSBI-QEMU（`bootloader/rustsbi-qemu.bin`）；LoongArch: OpenSBI（`bootloader/opensbi-qemu.bin`） |
| 构建工具 | Make + Cargo |
| 容器环境 | `docker.educg.net/cg/os-contest:20250614`（见 `.devcontainer/devcontainer.json`） |

### 内核主要依赖

- `polyhal` / `polyhal-trap` / `polyhal-boot`（本地路径依赖）：多架构硬件抽象层，提供启动、中断、上下文切换、页表、定时器、关机等统一接口。
- `riscv`：RISC-V CSR / 寄存器访问及内联汇编。
- `loongArch64`：LoongArch64 寄存器访问。
- `virtio-drivers`：VirtIO 块设备驱动。
- `lwext4_rust`（本地路径依赖）：ext4 文件系统的 C 库 FFI 绑定。
- `fatfs`（git 依赖）：FAT32 文件系统。
- `xmas-elf`：ELF 解析，用于程序加载与动态链接器识别。
- `buddy_system_allocator`：内核与用户态堆分配。
- `spin`：`no_std` 下的 `Mutex`、`RwLock`。
- `sbi-rt`：SBI 运行时调用。
- `bitflags`、`lazy_static`、`log`、`flat_device_tree` 等工具库。

此外，`os/vendor/` 目录下包含大量 vendored crate（含 `bindgen`、`virtio-drivers`、`fatfs`、`polyhal` 相关等），说明项目可能需要在离线或隔离环境中构建。

---

## 目录结构

```
/workspace/
├── bootloader/           # 引导固件（RustSBI / OpenSBI）
│   ├── rustsbi-qemu.bin
│   └── opensbi-qemu.bin
├── os/                   # 内核源码（核心）
│   ├── src/              # 内核 Rust 源码
│   ├── .cargo/           # Cargo 构建配置（目标、链接脚本、rustflags）
│   ├── scripts/          # 辅助脚本（如 QEMU 版本检查）
│   ├── vendor/           # 离线依赖（vendored crates）
│   ├── Cargo.toml
│   ├── Makefile
│   └── build.rs          # 生成 src/link_app.S（将用户应用嵌入内核数据段，当前主要用磁盘镜像加载）
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
├── polyhal/              # 多架构硬件抽象层（polyhal / polyhal-boot / polyhal-trap / polyhal-macro）
│   ├── polyhal/
│   ├── polyhal-boot/
│   ├── polyhal-trap/
│   ├── polyhal-macro/
│   └── example/
├── easy-fs/              # （当前几乎为空）
├── easy-fs-fuse/         # （当前几乎为空）
├── sdcard-rv.img         # 比赛环境预置磁盘镜像（RISC-V）
├── sdcard-rv-noltp.img   # 比赛环境预置磁盘镜像（RISC-V，无 LTP）
├── sdcard-la.img         # 比赛环境预置磁盘镜像（LoongArch）
├── Makefile              # 顶层 Makefile（仅构建内核本身，不打包用户应用）
├── Dockerfile            # 开发环境镜像构建
├── rust-toolchain.toml   # Rust 工具链锁定
├── .devcontainer/        # VS Code Dev Container 配置
├── .vscode/              # VS Code 设置
├── basic.md              # 早期系统调用实现清单（已部分过时）
├── libctest.md           # libctest 测试统计与说明
├── lmbench_testcode.md   # lmbench 性能测试基准数据
├── dev-env-info.md       # 开发环境信息
├── 随想.md               # 开发随笔（部分信息已过时）
└── AGENTS.md             # 本文件
```

---

## 构建与运行

### 开发环境

项目默认在容器 `/workspace`（即本目录）下开发。VS Code Dev Container 配置已存在于 `.devcontainer/devcontainer.json`，指定的镜像为 `docker.educg.net/cg/os-contest:20250614`。容器启动时会自动安装 `e2fsprogs`、`e2tools`、`openssh-client`。

`Dockerfile` 说明：多阶段构建，第一阶段从源码编译 QEMU 7.0.0，第二阶段安装 `gdb-multiarch`、Rust nightly 工具链、`cargo-binutils`、`rust-src`、`llvm-tools`，并将 `gdb-multiarch` 软链接为 `riscv64-unknown-elf-gdb`。

### 常用命令

**在 `os/` 目录下操作（推荐）：**

```bash
cd /workspace/os

# 构建并运行内核（RISC-V 默认），同时将 user/bin 下的应用打包进 ext4 镜像 fs.img
# 默认会尝试配置网桥，若不需要网络可直接使用 run-inner
make run

# 使用比赛磁盘镜像 sdcard-rv.img 运行（会自动将 initproc/user_shell/ls/basictests/libctests 注入镜像，并补齐动态链接库）
make run-sdcard

# 使用无 LTP 的比赛磁盘镜像运行
make run-sdcard-rv-noltp

# 仅构建内核与用户镜像
make build

# GDB 调试（会启动 tmux 分屏，左侧 QEMU，右侧 GDB）
make debug

# 指定架构（RISC-V 或 LoongArch）
make ARCH=riscv64 run
make ARCH=loongarch64 run
```

**顶层 `/workspace/Makefile`**：仅构建内核 ELF 并转换为裸二进制，不编译用户应用、不制作文件系统镜像。

### 构建流程（`os/Makefile`）

1. 进入 `../user` 编译用户态应用（生成 ELF 二进制）。若设置 `TEST=1`，`user/Makefile` 会将 `usertests` 复制为 `initproc`。
2. 创建 64MB 的 ext4 磁盘镜像 `fs.img`（`dd` + `mkfs.ext4`）。
   - 格式化时显式禁用了 `metadata_csum`、`64bit`、`extra_isize`，以确保与 `lwext4` 的最大兼容性。
3. 使用 `e2tools`（`e2cp`）将用户二进制复制进 `fs.img`。
4. `cargo build --release --target $(TARGET)` 编译内核。
5. `rust-objcopy` 将 ELF 转换为裸二进制 `os.bin`。
6. 启动 QEMU，将 `fs.img` 挂载为 VirtIO 块设备。

### QEMU 启动参数（RISC-V 摘要）

```bash
qemu-system-riscv64 \
  -machine virt \
  -nographic \
  -bios ../bootloader/rustsbi-qemu.bin \
  -device loader,file=target/riscv64gc-unknown-none-elf/release/os.bin,addr=0x80200000 \
  -drive file=fs.img,if=none,format=raw,id=x0 \
  -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
  -m 4G \
  -netdev bridge,id=n0,br=br0 \
  -device virtio-net-pci,netdev=n0 \
  -smp 4
```

默认 `CPU=4`，`MEMORY_END=0x1_8000_0000`（QEMU 分配 `-m 4G`）。可通过环境变量或 Makefile 变量调整。

### 比赛镜像注入（`run-sdcard` / `run-sdcard-rv-noltp`）

`do-patch-sdcard` 目标会将以下文件注入到外部 ext4 镜像中：
- `initproc`、`user_shell`、`ls`、`basictests`、`libctests_static`、`libctests_dynamic`
- 创建 `/bin/ls`、`/bin/sleep` 硬链接指向 busybox（用于 `which` 测试）
- 补齐动态链接库路径（RISC-V）：
  - `/lib/ld-linux-riscv64-lp64d.so.1`（glibc loader）
  - `/lib/libc.so.6`、`/lib/libm.so.6`（glibc）
  - `/lib/riscv64-linux-gnu/` 下的同名库（glibc 多路径兼容）
  - `/lib/ld-musl-riscv64-sf.so.1`（musl loader，优先从 `/musl/lib/` 拷贝，不存在则用 `/musl/lib/libc.so` 生成）
- 补齐动态链接库路径（LoongArch）：
  - `/lib64/ld-linux-loongarch-lp64d.so.1`、`libc.so.6`、`libm.so.6`、`libdl.so.2`、`libpthread.so.0`
  - `/lib/ld-musl-loongarch-lp64d.so.1`（musl loader）

---

## 运行时架构

### 启动序列

1. **RustSBI / OpenSBI** 将内核加载到物理地址 `0x80200000`（RISC-V）或 `0x80000000`（LoongArch）。
2. `polyhal-boot` 提供的 `#[polyhal::arch_entry]` 宏进入 `main(id, first)`。
3. **CPU 0（`first=true`）**：
   - 清零 BSS
   - 初始化日志子系统
   - 初始化内存管理（内核堆、页帧分配器）
   - 调用 `polyhal::common::init(&PageAllocImpl)` 初始化 polyhal 公共层
   - 初始化中断处理（`polyhal_trap::trap::init_trap()`）
   - 初始化每 CPU 调度器状态（`init_processors`）
   - 初始化网络子系统（`net::init()`，注册回环设备并探测 VirtIO-net）
   - 初始化文件系统（注册 ext4/devfs/etc/procfs/tmpfs，挂载根目录、/dev、/etc、/proc、/tmp、/dev/shm）
   - 通过 `polyhal::multicore` 启动其他 CPU
   - 将 `initproc` 加入就绪队列（`task::add_initproc()`）
4. **其他 CPU**：初始化 trap 后直接进入调度器。
5. 所有 CPU 执行 `task::run_tasks()`，采用抢占式调度（依赖定时器中断，10ms 周期）。

### 地址空间布局（关键常量）

- `VIRT_ADDR_START`（polyhal）：`0xffff_ffc0_0000_0000`
- `MMAP_BASE`：`0x4000_0000`
- `PAGE_SIZE`：`4096`（`0x1000`）
- `MAX_CPU_NUM`：`4`
- `_MAX_THREAD_NUM`：`16`
- `BLOCK_SIZE`：`512`
- `MEMORY_END`（qemu 板级）：`0x1_8000_0000`

### 中断与 Trap 处理

- 通过 `polyhal-trap` 提供统一 trap 入口，由 `#[polyhal::arch_interrupt]` 注解的 `kernel_interrupt` 函数处理。
- `TrapType::SysCall`：系统调用分发。
- `TrapType::StorePageFault` / `LoadPageFault` / `InstructionPageFault`：缺页异常处理（懒分配 / COW / 栈自动扩展 / 指令权限修复）。
- `TrapType::Timer`：定时器中断，触发抢占式调度；同时检查 `alarm` 与 `ITIMER_REAL` 到期，并每约 5 秒打印堆与页帧统计。
- `TrapType::IllegalInstruction`：发送 `SIGILL`。
- 返回用户态前（`handle_signals`）投递待处理的异步信号；若进程已被标记为 zombie，直接退出当前任务。

---

## 内核代码组织（`os/src/`）

### 顶层模块

| 文件 / 模块 | 职责 |
|-------------|------|
| `main.rs` | 入口点、启动序列、模块声明。设置了 `#![deny(missing_docs)]` 与 `#![deny(warnings)]`，但部分子模块用 `#[allow(missing_docs)]` 覆盖。 |
| `config.rs` | 常量（最大 CPU 数、最大线程数、`MMAP_BASE` 等），并 re-export 板级配置。 |
| `console.rs` / `logging.rs` | 打印宏与 `log` crate 集成。 |
| `sbi.rs` / `sbi_la.rs` | 架构相关 SBI / firmware 调用封装。 |
| `timer.rs` | RISC-V 定时器中断与 `get_time` 辅助函数。 |
| `lang_items.rs` | Panic 处理、分配错误处理。 |
| `error.rs` | 全局错误码 `SysError` 与统一结果类型 `SyscallResult` / `SysResult`，替代此前分散的负值魔术数字。 |
| `arch/` | 架构相关代码（`riscv.rs`、`loongarch64.rs` 及其子目录），入口宏、汇编封装。 |
| `boards/qemu.rs` | 板级配置（`CLOCK_FREQ = 12500000`、`MEMORY_END = 0x1_8000_0000`、MMIO 区域、块设备类型）。 |
| `sync/` | 同步原语：参考 Titanix 重构后的新锁体系，包括 `SpinNoIrqLock`、`SpinLock`、`SleepLock`、`BlockingMutex`、`ReentrantMutex` 等。 |

### 内存管理（`mm/`）

- **`page_table.rs`**：（已迁移至 polyhal）`VirtAddr`、`PhysAddr`、`VirtPageNum`、`PhysPageNum` 等由 polyhal 提供。
- **`frame_allocator.rs`**：物理页帧分配（基于 bitmap / stack），含 `FrameTracker`。
- **`heap_allocator.rs`** / **`heap.rs`**：内核堆初始化与测试（`buddy_system_allocator`）。
- **`vm_area.rs`**：内存映射区域（`MapArea` / `UserMapArea` / `KernelMapArea`）定义，涵盖堆、栈、ELF 段、mmap、trap context 等类型。
- **`vm_set.rs`**：`UserVMSet` / `KernelVMSet` — 完整地址空间管理。
  - **懒分配**：用户堆、mmap 区域在首次缺页时才分配物理页。
  - **栈管理**：用户栈改为**立即分配**（作为连续整体映射），但支持**自动向下扩展**（通过缺页异常检测并扩展栈边界）。
  - **COW**：`fork` 克隆只读页表，首次写入触发物理拷贝。
  - **mmap 支持**：文件映射与匿名映射，集成页缓存（`fs/page/pagecache.rs`）。
  - **动态链接器加载**：`from_elf` 解析 `PT_INTERP` 段，自动加载解释器（glibc / musl ld.so）到用户空间并调整入口点。
- **`exception.rs`**：缺页异常处理 trait（`SetPageFaultException`）。

### 任务管理（`task/`）

- **`process.rs`**：`ProcessControlBlock`（PCB），含 fd_table、信号处理（`SignalHandlers`）、子进程、CWD、`vm_set`、进程组（`pgid`）、rlimit（`rlimit_nofile`）、umask、`alarm`、alive_thread_count 等。
- **`task.rs`**：`TaskControlBlock`（TCB），每线程状态（trap context、内核栈、任务状态 `TaskStatus`）。
- **`manager.rs`**：全局就绪队列、PID→Process 映射、任务唤醒与移除。
- **`processor.rs`**：每 CPU 当前任务跟踪（`current_task`、`current_trap_cx`、`current_process` 等）。
- **`context.rs` / `switch.rs`**：任务上下文与上下文切换（已迁移至 polyhal `KContext` / `kcontext_switch`）。
- **`signal.rs`**：POSIX 信号定义、`SignalSet`、`SigAction`、`SignalHandlers`、默认行为处理。
- **`id.rs`**：PID、TID、内核栈分配器、进程组 ID 分配器。

### 中断处理（`trap/`）

- **`mod.rs`**：缺页异常处理函数（`handle_page_fault`、`handle_store_page_fault`、`handle_load_page_fault`）。
  - 实际 trap 入口由 `polyhal-trap` 统一提供，通过 `#[polyhal::arch_interrupt]` 宏分发到 `kernel_interrupt`（`main.rs` 中）。
  - 用户陷阱：系统调用、缺页异常、非法指令、定时器中断。
  - 内核陷阱：缺页异常、非法指令、定时器中断。
- `InstructionPageFault` 支持自动修复缺失的 X 权限 PTE。

### 文件系统（`fs/`）

采用 **VFS-first** 设计，参考 Linux / Chronix 风格：

#### VFS 层（`fs/vfs/`）

- **`dentry.rs`**：`Dentry` trait + `DentryInner`（名称、父节点、子节点 BTreeMap、inode 引用）。
- **`inode.rs`**：`Inode` trait（元数据、读写、截断、查找、创建、删除、重命名、`readlink`、`symlink` 等）。
- **`file.rs`**：`File` trait（读写、定位、打开、关闭、刷新、`get_cache_frame` 用于 mmap 页缓存）。
- **`superblock.rs`**：`SuperBlock` trait。
- **`fstype.rs`**：`FsType` trait，用于文件系统注册与挂载。
- **`dcache.rs`**：全局 dentry 缓存 `GLOBAL_DCACHE`（路径字符串 → `Arc<dyn Dentry>`）。**已实现 LRU 淘汰**（容量上限 8192），并支持 `pin` 保护挂载点不被淘汰。
- **`path.rs`**：路径解析（绝对 / 相对路径分解）。
- **`kstat.rs`**：`Kstat` 结构，用于 `fstat` / `fstatat`。
- **`mount.rs`**：挂载点管理相关结构。

#### 具体文件系统

- **`lwext4/`**：ext4 实现，包装 `lwext4_rust`。
  - `disk.rs`：块设备适配器（`dyn BlockDevice`）。
  - `inode.rs`、`dentry.rs`、`file.rs`、`superblock.rs`、`fstype.rs`：VFS 适配层。
  - `ext4/`：基于 lwext4 绑定的目录/文件辅助函数。
- **`devfs/`**：设备文件系统。
  - 现有设备：`/dev/null`、`/dev/tty`、`/dev/zero`、`/dev/urandom`（代码存在，init 中已启用）、`/dev/rtc`、`/dev/rtc0`。
- **`tempfs/`**：基于 RAM 的文件系统（用于 `/etc`、`/tmp`、`/dev/shm`）。
- **`fat32/`**：FAT32 实现（部分完成，基于 `rust-fatfs`，代码存在但尚未挂载为根文件系统）。
- **`procfs/`**：进程文件系统。
  - 现有文件：`/proc/meminfo`、`/proc/mounts`、`/proc/self`、`/proc/[pid]/smaps`。
- **`page/`**：mmap 页缓存（`pagecache.rs`）。**已实现 LRU 淘汰**（最大 4096 页 ≈ 16MB）。
- **`etc/mod.rs`**：初始化 `/etc` 下的占位文件（`passwd`、`adjtime`、`group`、`localtime` 等）。

#### 文件系统初始化（`fs/mod.rs`）

1. 注册 `ext4`、`devfs`、`etc`（tempfs）、`procfs`、`tmpfs`。
2. 使用 `BLOCK_DEVICE`（`VirtIOBlock`）在 `/` 挂载根 ext4。
3. 在 `/dev` 挂载 devfs 并调用 `init_devfs()` 创建设备节点。
4. 在 `/dev/shm` 挂载 tmpfs（支持 `shm_open`）。
5. 在 `/etc` 挂载 tempfs 并调用 `init_etcfs()` 填充内容。
6. 在 `/proc` 挂载 procfs 并调用 `init_procfs()`。
7. 在 `/tmp` 挂载 tmpfs。

### 系统调用（`syscall/`）

在 `syscall/mod.rs` 中按编号分发，子模块包括：

- **`fs.rs`**：`openat`、`close`、`read`、`write`、`writev`、`readv`、`getdents64`、`mkdirat`、`unlinkat`、`linkat`、`symlinkat`、`readlinkat`、`chdir`、`getcwd`、`fstat`、`fstatat`、`dup`、`dup2`、`pipe`、`fcntl`、`ioctl`、`mount`、`umount2`、`fsync`、`sync`、`sendfile`、`statfs`、`faccessat`、`lseek`、`utimensat`、`renameat2`、`pread64`、`pwrite64`、`statx`、`ftruncate`。
- **`process.rs`**：`exit`、`exit_group`、`fork`、`clone`、`execve`、`waitpid`、`yield`、`getpid`、`getppid`、`getuid`、`geteuid`、`getegid`、`gettid`、`setpgid`、`setpgrp`、`getpgid`、`getpgrp`、`set_tid_address`、`set_robust_list`、`get_robust_list`、`umask`、`getrusage`。
- **`mm.rs`**：`mmap`、`munmap`、`mprotect`、`brk`、`madvise`、`msync`。
- **`signal.rs`**：`kill`、`tkill`、`tgkill`、`sigaction`、`sigprocmask`、`rt_sigtimedwait`、`rt_sigreturn`、`rt_sigsuspend`、`setitimer`、`getitimer`。
- **`time.rs`**：`get_time`、`times`、`sleep`、`clock_gettime`、`clock_nanosleep`。
- **`net.rs`**：`socket`、`bind`、`listen`、`accept`、`connect`、`sendto`、`recvfrom`、`getsockname`、`getpeername`、`setsockopt`、`getsockopt`、`shutdown`。
- **`thread.rs`**：`thread_create`、`waittid`。
- **`info.rs`**：`uname`、`sysinfo`、`syslog`（桩）、`prlimit64`（桩）、`getrandom`（桩）。
- **`pipe.rs`**：管道创建（`sys_pipe`）。
- **`futex.rs`**：`futex` 系统调用（支持 `FUTEX_WAIT`、`FUTEX_WAKE`、`FUTEX_REQUEUE` 及超时检查）。
- **`shm.rs`**：System V 共享内存（`shmget`、`shmctl`、`shmat`、`shmdt`）。
- **`misc.rs`**：`ppoll`、`pselect6` 等辅助或桩实现。

### 网络（`net/`）

- **`device.rs`**：`DeviceManager`，管理网络接口。
- **`loopback.rs`**：回环网络设备。
- **`arp.rs`**、**`ethernet.rs`**、**`ip.rs`**、**`icmp.rs`**、**`udp.rs`**、**`tcp.rs`**、**`neighbor.rs`**、**`route.rs`**：TCP/IP 协议栈分层实现。
- **`skb.rs`**：Socket Buffer（网络包）管理。
- **`virtio/`**：VirtIO-net PCI/MMIO 驱动与 virtqueue 管理（`config.rs`、`device.rs`、`pci.rs`、`virtqueue.rs`）。
- **网络初始化**：`main.rs` 中调用 `net::init()`，注册回环设备，探测并初始化 VirtIO-net PCI 设备，配置 IP `10.0.2.15` 与网关 `10.0.2.2`。
- **收包轮询**：`poll_rx_all()` 仅在阻塞型 socket 系统调用（如 `accept`、`recvfrom`）等待时调用，无独立中断驱动收包线程。

### Socket 层（`socket/`）

- **`raw.rs`**、**`udp.rs`**、**`tcp.rs`**：Socket 抽象层，供系统调用与网络协议栈交互。

### 驱动（`drivers/`）

- **`block/virtio_blk.rs`**：VirtIO 块设备驱动。自定义 `VirtioHal` 通过内核页帧分配器实现 DMA 内存分配。
- **`block/pci.rs`** / **`probe.rs`**：PCI 总线扫描与设备探测。

---

## 用户态代码（`user/`）

### `user_lib`（`src/lib.rs`）

一个 `no_std` 用户态运行时库，提供：

- `_start` 入口点（初始化 32KB 用户堆、调用 `main()`、然后 `exit()`）。
- 系统调用封装（`open`、`read`、`write`、`fork`、`execve`、`mmap`、`socket`、`bind`、`sendto`、`recvfrom`、`symlinkat`、`linkat` 等）。
- `SignalSet`、`SigAction`、`SigHandler` 定义。
- `OpenFlags` bitflags（`RDONLY`、`WRONLY`、`RDWR`、`O_CREAT`、`O_TRUNC`、`O_DIRECTORY`）。

### 应用程序（`src/bin/`）

| 程序 | 作用 |
|------|------|
| `initproc.rs` | PID 1，从文件系统加载并 exec `user_shell`，负责回收僵尸进程。**当前默认启动 busybox 的 sh**（优先尝试 `/musl/busybox` 或 `/bin/busybox`，并自动创建常用命令软链接）。 |
| `user_shell.rs` | 交互式 shell（备用），支持内建命令（`cd`、`exit`、`help`）及通过 `PATH` 搜索执行外部命令。 |
| `usertests.rs` | 内核自带测试套件（14 个成功测试 + 1 个失败测试 `stack_overflow`，严格检查退出码）。 |
| `usertests_simple.rs` | 轻量版测试套件（11 个测试，不严格检查退出码）。 |
| `basictests.rs` | musl libc 基础测试套件（fork + execve 运行 31 个外部测试用例）。 |
| `libctests_static.rs` / `libctests_dynamic.rs` | libctest 手动测试入口（静态链接 / 动态链接），用于统计当前能够通过的标准 C 库测试。 |
| `ls.rs` | 简易 `ls` 实现。 |
| `ping.rs` / `ping2.rs` | 网络 ping 工具。 |
| `signal_test.rs` | 信号处理测试。 |
| `tcp_socket_test.rs` / `tcp_test.rs` | TCP 网络测试。 |
| `hello_world.rs`、`forktest.rs`、`yield.rs` 等 | 各类简单功能测试。 |

`user/Makefile` 将 `src/bin/` 下的所有 `.rs` 编译为 ELF 二进制。若设置 `TEST=1`，会将 `usertests` 复制为 `initproc`。

---

## 开发约定

### Cargo 配置

- `os/.cargo/config.toml` 与 `user/.cargo/config.toml`：
  - 设定构建目标为 `riscv64gc-unknown-none-elf` 或 `loongarch64-unknown-none`。
  - 链接脚本 `-Tsrc/linker-riscv64.ld` / `-Tsrc/linker-loongarch64.ld`。
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
- **锁体系重构**：参考 Titanix 重构内核锁，已逐步替换旧锁。目前主要使用以下新锁（定义于 `sync/mutex.rs`）：
  - `SpinNoIrqLock` / `SpinNoIrq`：关中断自旋锁（保护 PCB、TCB 等核心结构）。
  - `SpinLock` / `SpinMutex`：普通自旋锁。
  - `SleepLock`：睡眠锁（支持阻塞等待，用于页缓存等）。
  - `BlockingMutex`：阻塞互斥锁。
  - `ReentrantMutex`：可重入锁。
  - 旧 `spin::Mutex` 已大幅减少（仍保留约 16 处，`SpinNoIrqLock` 约 34 处）。
- 内核与用户态均为 `#![no_std]`，通过 `extern crate alloc` 使用堆分配。
- **错误码统一**：参考 Linux `errno.h`，使用 `os/src/error.rs` 中的 `SysError` 枚举与 `SyscallResult` / `SysResult<T>` 类型，避免在 VFS、内存管理、网络等子系统中散落负值魔术数字。

### 架构决策

1. **全 `no_std`**：内核与用户态均禁用标准库。
2. **VFS 优先**：所有存储通过 `Dentry`、`Inode`、`File`、`SuperBlock` trait 抽象，允许多文件系统并存。
3. **懒内存分配**：用户堆、mmap 区域在首次缺页时才分配物理页；**用户栈改为立即分配**，但支持自动向下扩展。
4. **COW fork**：`fork()` 创建只读共享映射，首次写入触发页拷贝。
5. **polyhal 硬件抽象**：通过 polyhal 统一封装 RISC-V 与 LoongArch64 的启动、trap、页表、定时器、多核、上下文切换，降低多架构维护成本。
6. **多核就绪**：支持最多 4 核（`MAX_CPU_NUM = 4`），每核拥有独立调度器与当前任务指针。
7. **信号在 trap 返回时投递**：`trap_return()` 返回用户态前检查待处理信号，支持默认、忽略与自定义处理函数。
8. **根文件系统兼容性**：制作 ext4 镜像时禁用 `metadata_csum`、`64bit`、`extra_isize`，以确保 `lwext4` 能正确读写。
9. **动态链接器支持**：`execve` 加载 ELF 时检测 `PT_INTERP`，自动加载解释器并映射到用户空间，调整程序入口点。已验证 musl 与 glibc 动态链接 BusyBox 可运行。
10. **缓存淘汰**：页缓存与 Dentry 缓存均实现 LRU，避免无界增长导致 OOM。
11. **initproc 动态补全**：启动时自动探测 busybox 并创建 `/bin` 下常用命令软链接，提升 C 库测试兼容性。

---

## 测试策略

### 内核自检测试

- `heap_test()`、`frame_allocator_test()` 等函数存在于源码中，但当前未在启动序列中调用。

### 用户态集成测试

| 测试程序 | 运行方式 | 说明 |
|----------|----------|------|
| `usertests` | `cd /workspace/os && make run TEST=1` | 14 个成功测试 + 1 个失败测试（`stack_overflow`，期望退出码 `-2`）。严格校验每个子进程退出码。 |
| `usertests_simple` | 手动运行 | 11 个基础测试，不严格检查退出码。 |
| `basictests` | `make run-sdcard` / `make run-sdcard-rv-noltp` | 31 个 musl libc 外部测试（`chdir`、`clone`、`mmap`、`fork`、`pipe`、`brk` 等），检查退出码是否为 0。 |
| `libctests_static` / `libctests_dynamic` | 手动运行 / 参考 `libctest.md` | 标准 C 库兼容性测试，当前静态链接通过 107 项、动态链接通过 110 项（见 `libctest.md` 统计）。未通过项主要集中在 pthread、socket、tls、sem_init。 |
| `signal_test` | 手动运行 | POSIX 信号投递与处理测试。 |
| `ping` | 手动运行 | 网络 ICMP ping 测试。 |
| `lmbench` | `make run-sdcard` 后在 shell 中运行 | 性能基准测试，参考 `lmbench_testcode.md` 中的基准数据。 |

### 手动 / 交互式测试

- `make run` 默认进入 `user_shell`（或 busybox sh），可手动输入命令验证功能。
- `make debug` 启动 tmux 分屏调试环境（左 QEMU `-s -S`，右 `riscv64-unknown-elf-gdb`）。

### 子项目测试

- **`rust-fatfs/`**：包含标准 `cargo test` 测试（`tests/format.rs`、`fsck.rs`、`read.rs`、`write.rs` 等）。主内核构建不自动运行这些测试。
- **`lwext4_rust/`**：包含独立的 CI（`.github/workflows/build.yml`、`test.yml`），通过 QEMU 运行示例内核验证 ext4 绑定。

### 主仓库 CI

**主仓库（`/workspace`）目前没有 GitHub Actions CI**。测试依赖本地 QEMU 环境手动执行。

---

## 安全与稳定性提示

- 内核运行在 `S` 态（Supervisor mode），没有用户态/内核态的 KASLR 或栈金丝雀保护。
- 内存安全主要依赖 Rust 的所有权与借用检查；但部分区域（如 `unsafe` 汇编、MMIO、DMA 缓冲区、C FFI）仍需人工审查。
- `lwext4_rust` 包含大量 `unsafe` FFI 调用，对 C 库的输入校验（如路径长度、inode 有效性）需要保持在 Rust 侧完成。
- `SpinNoIrqLock` 本质上是关中断自旋锁；在持有锁期间不应执行可能阻塞或触发缺页的操作，否则容易导致死锁。
- **动态链接器加载**通过内核直接读取解释器 ELF 并映射到用户空间，未经过完整的权限隔离，需确保解释器路径校验严格。
- **polyhal 多架构兼容**：修改页表权限、trap 处理、上下文切换时需同时考虑 RISC-V 与 LoongArch64 的语义差异（例如 RISC-V Sv39 不允许 `W=1,R=0` 的 PTE 组合，而 polyhal 默认允许 `UW`，需在内核侧校验）。
- **网络栈收包为轮询模式**：`poll_rx_all()` 仅在阻塞 socket 调用时触发，非中断驱动，高负载下可能存在延迟。

---

## 当前分支上下文（`lmbench_testcode`）

仓库当前位于 **`lmbench_testcode`** 分支。

近期活跃工作（按 `git log` 摘要）：
- **dentry 缓存与页缓存 LRU**：为 dentry 缓存加入 LRU 淘汰机制（容量 8192），为页缓存加入 LRU 淘汰（最大 4096 页 ≈ 16MB），修复相关竞争与死锁 bug。
- **lmbench 兼容性**：扩大堆到 64MB、扩大物理内存到 `0x1_8000_0000`（QEMU `-m 4G`），通关 RISC-V musl 的 lmbench 测试。
- **栈管理调整**：栈改回**立即分配**（作为连续整体），但支持**自动向下扩展**，修复栈生长 bug。
- **信号修复**：修复阻塞任务不能被终止信号终止的 bug；同步信号（`SIGSEGV`、`SIGILL`）在投递前强制解除阻塞，避免 `longjmp` 后死循环。
- **锁重构**：参考 Titanix 引入 `SpinNoIrqLock`、`SleepLock`、`BlockingMutex`、`ReentrantMutex` 等新锁，替换大量旧锁，修复多个竞争死锁 bug。
- **软连接实现**：实现 `symlinkat` / `readlinkat`，VFS Inode 层新增 `readlink` / `symlink` 接口。
- **msync 实现**：新增 `sys_msync` 系统调用。
- **BusyBox 集成**：弃用自己的 sh，使用 busybox 的 sh；`initproc` 启动时自动探测 busybox 并创建 `/bin` 下常用命令软链接。
- **PTE 架构整理**：使用 AI 辅助整理 polyhal PTE 权限与架构相关代码，修复 RISC-V 非法 PTE 组合。
- **新增系统调用**：`futex`、`shmget`/`shmctl`/`shmat`/`shmdt`、`pselect6`、`ppoll`、`rt_sigsuspend`、`getsockopt`/`setsockopt`/`shutdown`/`getpeername`/`getsockname`、`statx`、`renameat2`、`prlimit64`、`faccessat`、`utimensat`、`ftruncate`、`sync` 等。
- **网络启用**：`main.rs` 中 `net::init()` 已 uncomment，默认初始化 VirtIO-net 与回环设备。
- **定时器增强**：定时器中断中检查 `alarm` 与 `ITIMER_REAL` 到期，并定期打印堆与页帧统计。

### 已知待办与注意事项

- **pthread 兼容性**：`pthread_cancel_points` 等存在死循环问题，当前 libctest 未通过的项主要集中在 pthread、socket、tls、sem_init。
- **页面置换算法**：已实现页缓存 LRU，但全局物理内存紧张时的主动换出策略仍待完善。
- **fixed map**（`MAP_FIXED` 的完整语义）仍有边界情况待完善。
- 进程退出时 fd_table 的关闭策略与 Linux 存在差异，部分场景需继续调整。
- 锁策略虽已重构，但仍有少量旧 `spin::Mutex` 残留，需找个时机彻底统一。
- **网络中断驱动**：当前为轮询收包，后续可考虑 VirtIO-net 中断优化。
