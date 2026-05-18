# Kairix —— AI 编码代理项目指南

> 本文件面向 AI 编码代理。阅读者应对本项目一无所知，所有信息均基于实际代码与文档，不做假设。

---

## 1. 项目概览

**Kairix** 是一款基于 Rust 语言开发的现代化操作系统内核，支持 **RISC-V 64** 与 **LoongArch64** 两种架构。内核运行在 QEMU 模拟器上，具备完整的 VFS 文件系统抽象、基于缺页异常的懒分配（lazy allocation）内存管理、标准进程/信号/管道系统调用接口，目标是通过 libc-test、LTP、iozone、lmbench、netperf 等业界标准测试套件验证兼容性与性能。

- **仓库根目录**: `/workspace`
- **默认架构**: `riscv64`（可在 Makefile 中通过 `ARCH=` 切换）
- **Rust Edition**: 2024
- **目标平台**: `riscv64gc-unknown-none-elf`、`loongarch64-unknown-none`

---

## 2. 技术栈

| 层级 | 技术/工具 | 说明 |
|------|-----------|------|
| 语言 | Rust | 内核与用户库均使用 `#![no_std]`，依赖 `alloc` |
| 构建 | Cargo + Make | 顶层 Makefile 仅做分发，`os/Makefile` 与 `user/Makefile` 负责实际构建 |
| 模拟器 | QEMU 9.2.1 | `qemu-system-riscv64` / `qemu-system-loongarch64`，`virt` 机器类型 |
| Bootloader | RustSBI (RISC-V) / OpenSBI (LoongArch) | 固件二进制位于 `bootloader/` |
| HAL | `polyhal` | 本地多架构硬件抽象层，含 `polyhal-boot`、`polyhal-trap`、`polyhal-macro` |
| 文件系统 | FAT32、ext4、devfs、procfs、tmpfs | FAT32 使用上游 `rust-fatfs`；ext4 使用 `lwext4_rust`（C 绑定） |
| 网络 | VirtIO-net PCI | 自研网络协议栈（ARP/IP/ICMP/TCP/UDP），支持 socket 层 |
| 内存管理 | buddy_system_allocator + 自研页表 | 物理页帧分配、页缓存、懒分配 |

---

## 3. 项目结构

```
workspace/
├── os/                 # 内核代码（核心）
├── user/               # 用户态运行时库 + 28 个独立应用
├── polyhal/            # 硬件抽象层（HAL）
│   ├── polyhal/
│   ├── polyhal-boot/
│   ├── polyhal-trap/
│   └── polyhal-macro/
├── lwext4_rust/        # ext4 文件系统（C 库 + bindgen FFI）
├── rust-fatfs/         # FAT 文件系统（纯 Rust，no_std）
├── libc-test/          # musl C 库兼容性测试
├── libc-bench/         # C 库性能基准测试
├── ltp/                # Linux Test Project 内核 syscall 测试
├── netperf-2.7.0/      # 网络性能测试工具源码
├── docs/               # 测试文档、结果记录、待办事项
│   ├── ltp/            # LTP 测试通过情况详细清单
│   ├── basic.md        # 基础 syscall 实现清单
│   ├── libctest.md     # libc-test 通过情况
│   ├── iozone.md       # iozone 测试结果
│   ├── lmbench_testcode.md # lmbench 测试结果
│   └── readme.md       # 内部待办与注意事项
├── bootloader/         # rustsbi-qemu.bin 等固件
├── Makefile            # 顶层 Makefile（封装 os/Makefile）
├── Dockerfile          # 开发环境镜像（基于 Ubuntu 20.04）
├── .devcontainer/      # VS Code Dev Container 配置
└── .vscode/            # VS Code 设置（rust-analyzer 等）
```

---

## 4. 构建与运行命令

### 4.1 顶层快捷命令（`/` 目录）

| 命令 | 作用 |
|------|------|
| `make rkernel` | 构建并运行 RISC-V 内核，挂载 `sdcard-rv.img` |
| `make lkernel` | 构建并运行 LoongArch 内核，挂载 `sdcard-la.img` |
| `make all` | 两种架构均构建，并将内核二进制复制到根目录（离线评测用） |
| `make clean` | 清理内核构建产物 |

### 4.2 内核构建（`os/` 目录）

| 命令 | 作用 |
|------|------|
| `make build` | 完整构建，生成剥离后的内核二进制 `.bin` |
| `make kernel` | 仅编译内核 ELF（`cargo build --release`） |
| `make run-inner` | 使用 QEMU 直接运行当前构建的内核 |
| `make run-sdcard` | 运行并挂载外部 ext4 SD 卡镜像（比赛环境） |
| `make debug` | 启动 QEMU + GDB 调试（RISC-V，tmux 分屏） |
| `make gdbserver` | 仅启动 QEMU GDB server（`-s -S`） |
| `make gdbclient` | 启动 GDB 客户端并连接 localhost:1234 |
| `make patch-sdcard` | 将用户程序与动态库注入到 SD 卡镜像 |
| `make clean` | `cargo clean` + 清理用户应用 |
| `make env` | 检查并安装 Rust target、`cargo-binutils`、`rust-src`、`llvm-tools-preview` |

**常用变量**：
- `ARCH=riscv64|loongarch64`
- `MODE=release`（默认）
- `LOG=INFO`
- `CPU=4`
- `NET_BACKEND=user|bridge|auto`
- `SELECTED_IMG=...`（指定磁盘镜像路径）

### 4.3 用户应用构建（`user/` 目录）

| 命令 | 作用 |
|------|------|
| `make build` | 编译所有用户 ELF 并转为 `.bin` |
| `make elf` | 仅编译 ELF（`cargo build --release`） |
| `make binary` | ELF 转 raw binary |
| `make clean` | 清理 |

- 默认 target：`riscv64gc-unknown-none-elf`
- 切换架构：`TARGET=loongarch64-unknown-none make build`
- `TEST=1` 时，会将 `usertests` 复制为 `initproc`

### 4.4 QEMU 运行参数要点

- RISC-V: `-machine virt -bios bootloader/rustsbi-qemu.bin -device loader,file=os.bin,addr=0x80200000 -drive file=fs.img -device virtio-blk-device -m 4G -smp 4`
- LoongArch: `-machine virt -kernel os.elf -drive file=fs.img -device virtio-blk-pci -m 4G -smp 4`
- 网络默认使用 `-netdev user`，可选 bridge 模式

---

## 5. 开发环境

### 5.1 容器与镜像

- **Dockerfile**: 基于 Ubuntu 20.04，从源码构建 QEMU 7.0.0+，安装 Rust nightly、`cargo-binutils`、GDB 等。
- **Dev Container**: `.devcontainer/devcontainer.json` 使用预构建镜像 `zhouzhouyi/os-contest:20260104`，以 **root** 身份运行，带 `--privileged --network=host`。
- **VS Code 扩展**: `rust-analyzer`、`even-better-toml`、`vscode-lldb`、`software-visual-studio-code-riscv-asm`。

### 5.2 工具链版本（参考 `dev-env-info.md`）

- `rustc`: `1.86.0-nightly`（默认 toolchain `nightly-2025-01-18`）
- `qemu`: `9.2.1`
- 已安装 target:
  - `riscv64gc-unknown-none-elf`
  - `loongarch64-unknown-none`
  - `riscv64imac-unknown-none-elf`
  - `loongarch64-unknown-linux-gnu`
  - `x86_64-unknown-linux-gnu`
- 可选 GCC 交叉编译器:
  - `riscv64-unknown-elf-gcc` (8.2.0)
  - `riscv64-linux-gnu-gcc` (11.4.0)
  - `loongarch64-linux-gnu-gcc` (13.2.0)

### 5.3 Cargo 配置

- `os/.cargo/config.toml` 与 `user/.cargo/config.toml` 均使用 **vendored** 依赖（`vendor/` 目录），支持离线构建。
- 通过 `-Tsrc/linker-*.ld` 指定自定义链接脚本。
- Rustflags 强制启用帧指针：`-Cforce-frame-pointers=yes`。

---

## 6. 代码组织

### 6.1 内核 `os/src/`

| 模块 | 说明 |
|------|------|
| `main.rs` | 内核入口。`rust_main()` 完成初始化后调用 `task::run_tasks()` 进入用户态 |
| `arch/` | 架构相关启动与入口代码（`riscv.rs`、`loongarch64.rs` 等） |
| `board/qemu.rs` | 板级常数（`MEMORY_END`、MMIO 区域）及 `BlockDeviceImpl` |
| `config.rs` | 内核常数（`MAX_CPU_NUM`、`MMAP_BASE`、`BLOCK_SIZE` 等） |
| `drivers/` | 块设备驱动（VirtIOBlk、PCI 探测） |
| `fs/` | 文件系统层 |
| `fs/vfs/` | VFS 核心：`dentry`、`inode`、`file`、`superblock`、`path`、`dcache`、`fstype` |
| `fs/devfs/` | 设备文件：`null`、`zero`、`tty`、`rtc`、`urandom` 等 |
| `fs/procfs/` | proc 文件系统：`maps`、`meminfo`、`mounts`、`smaps` 等 |
| `fs/tempfs/` | tmpfs |
| `fs/fat32/` | FAT32 实现（封装 `rust-fatfs`） |
| `fs/lwext4/` | ext4 实现（封装 `lwext4_rust`） |
| `fs/page/` | 页缓存（Page Cache） |
| `mm/` | 内存管理：`frame_allocator`、`heap_allocator`、`vm_area`、`vm_set`、`exception` |
| `net/` | 网络协议栈 + virtio-net 驱动：`arp`、`ip`、`icmp`、`tcp`、`udp`、`ethernet`、`route` 等 |
| `socket/` | Socket 层：`TCP`、`UDP`、`raw` |
| `sync/` | 同步原语：`SpinNoIrqLock`、`SleepLock`、`ReentrantMutex` |
| `syscall/` | 系统调用实现：`fs.rs`、`process.rs`、`net.rs`、`pipe.rs`、`mm.rs`、`signal/` 等 |
| `task/` | 进程/线程管理：`process.rs`、`task.rs`、`manager.rs`、`processor.rs`、`signal.rs` |
| `trap/` | 陷阱/中断处理（统一的用户态/内核态入口） |
| `timer.rs` | 定时器中断设置 |
| `sbi.rs` / `sbi_la.rs` | 固件/SBI 调用封装 |

### 6.2 用户库 `user/src/`

| 模块 | 说明 |
|------|------|
| `lib.rs` | 用户库根：`_start` 入口、堆初始化（`buddy_system_allocator`）、安全 syscall 封装 |
| `syscall.rs` | syscall 号定义及架构相关内联汇编（RISC-V `ecall` / LoongArch `syscall 0`） |
| `console.rs` | `print!` / `println!` 宏与 `getchar()` |
| `lang_items.rs` | `#[panic_handler]` |
| `bin/` | 28 个独立二进制（每个 `.rs` 对应一个 ELF）：`initproc`、`user_shell`、`usertests`、`libctests_static`、`libctests_dynamic`、`libcbench`、`hello_world`、`forktest`、`ls`、`ping` 等 |

### 6.3 HAL `polyhal/`

`polyhal` 是本地多架构硬件抽象层，包含四个 crate：

| Crate | 职责 |
|-------|------|
| `polyhal` | 核心 HAL：页表、中断、定时器、上下文切换、调试控制台、地址工具等 |
| `polyhal-boot` | 启动入口抽象：`define_entry!` 宏、架构相关启动初始化 |
| `polyhal-trap` | 陷阱处理与陷阱帧：`trapframe`（各架构寄存器结构） |
| `polyhal-macro` | 过程宏：`define_arch_mods!`、`pub_use_arch!`、`percpu` 等 |

`os`  crate 通过 `polyhal::pagetable::*`、`polyhal::timer::*`、`polyhal::instruction::*` 等接口屏蔽架构差异。

---

## 7. 代码风格指南

- **Rust Edition 2024**，`#![no_std]` 环境，禁止直接使用标准库。
- 内核代码应优先通过 `polyhal` 提供的跨架构 API 操作底层，避免直接编写 `#[cfg(target_arch = ...)]` 硬件细节。
- 标志位统一使用 `bitflags!` 宏定义。
- 日志使用 `log` crate 分级输出（`error!`、`warn!`、`info!`、`debug!`、`trace!`）。
- 项目未提供自定义 `rustfmt.toml`，**建议直接使用标准 `cargo fmt` 与 `cargo clippy`** 保持代码风格一致。
- C 语言测试代码（`libc-test`、`ltp`）使用 musl / glibc 交叉编译，编译标志见各目录 `Makefile`。
- **注释语言**：Rust 源码中模块级文档注释以英文为主；Makefile、文档、测试记录以中文为主。新增注释可根据上下文灵活选择，但应保持与所在文件已有注释风格一致。

---

## 8. 测试策略

### 8.1 测试套件总览

| 套件 | 类型 | 位置 | 覆盖范围 |
|------|------|------|----------|
| **libc-test** | C 库兼容性 | `libc-test/` | musl C 库功能与回归测试（string、stdio、pthread、socket、tls、dlopen 等） |
| **libc-bench** | C 库性能 | `libc-bench/` | malloc、string、pthread、regex、stdio 性能 |
| **LTP** | 内核 syscall 正确性 | `ltp/`、`docs/ltp/` | mount、close_range、fcntl、brk、sbrk、mmap 等 |
| **iozone** | 文件系统 I/O 性能 | `docs/iozone.md` | 顺序/随机/反向/stride 读写、吞吐量测试 |
| **lmbench** | OS 微基准 | `docs/lmbench_testcode.md` | syscall 延迟、read/write、stat、fork、pagefault、带宽、上下文切换 |
| **netperf** | 网络性能 | `netperf-2.7.0/`、`netperf_testcode.sh` | TCP/UDP stream 吞吐、RR 延迟、TCP CRR |

### 8.2 运行方式

- **libc-test**
  - 交叉编译：编辑 `Makefile` 中的 `MUSL_LIB` / `PREFIX`，执行 `make disk`。
  - 产物 `entry-static.exe`、`entry-dynamic.exe`、`runtest.exe` 放入 SD 卡镜像，在内核 shell 中运行。
- **libc-bench**
  - `make`（或 `make test`）生成静态二进制 `./libc-bench`，直接在内核中执行。
- **LTP**
  - 将 `ltp/` 下的 `.c` 文件（如 `close_range02.c`、`mount01.c`）交叉编译为 ELF，放入文件系统运行。
  - 详细通过清单与分数记录在 `docs/ltp/ltp_menu.md` 与 `docs/ltp/ltp_fs_signal_plan.md`。
- **iozone**
  - 在内核 shell 中执行：
    - 自动模式：`./iozone -a -r 1k -s 4m`
    - 4 进程吞吐：`./iozone -t 4 -i 0 -i 1 -r 1k -s 1m`
- **lmbench**
  - 标准 lmbench 套件在内核中运行，结果记录于 `docs/lmbench_testcode.md`。
- **netperf**
  - 运行 `netperf_testcode.sh`：先启动 `netserver`，再依次执行 `UDP_STREAM`、`TCP_STREAM`、`UDP_RR`、`TCP_RR`、`TCP_CRR`。

### 8.3 当前主要缺口（参考 `docs/libctest.md` 与 `docs/ltp/`）

- libc-test 剩余未通过项集中在 **pthread**（cancel、cond、tsd、robust detach）、**socket**、**tls**、**sem_init**。
- LTP 大量测试（尤其是 `fcntl` 文件锁、`mount/umount`、`xattr`、`fallocate`、`splice/tee/vmsplice`、`inotify`、`fanotify`、`copy_file_range`、`close_range`、`sync_file_range` 等）仍在推进中。

---

## 9. 部署与发布

### 9.1 产物

执行 `make all` 后，根目录会生成：

- `os-riscv64.bin` / `os-riscv64` — RISC-V 内核二进制与 ELF
- `os-loongarch64.bin` / `os-loongarch64` — LoongArch 内核二进制与 ELF

### 9.2 SD 卡镜像注入流程

`os/Makefile` 中的 `patch-sdcard` / `do-patch-sdcard` 目标会自动：

1. 在 `user/` 目录编译用户应用。
2. 将 `initproc`、`user_shell`、`ls`、`basictests`、`libctests_static`、`libctests_dynamic` 复制到镜像根目录。
3. 根据架构（RISC-V / LoongArch）设置 `/lib`、`/lib64` 下的 glibc / musl 动态链接器与共享库（`libc.so.6`、`libm.so.6`、`ld-linux-*.so.1`、`ld-musl-*.so.1` 等）。
4. 同步并卸载镜像。

### 9.3 网络配置

- **user 模式**（默认）：QEMU 内置用户态网络，无需额外配置。
- **bridge 模式**：需要先在宿主机创建 bridge（`make net-up-bridge BRIDGE_IF=br0`），内核启动后可通过该 bridge 与宿主机通信。
- 抓包：设置 `NET_DUMP=1` 可在 `qemu-net.pcap` 中记录网络流量。

---

## 10. 安全注意事项

- **特权容器**：开发容器以 `root` 运行并开启 `--privileged`，请注意宿主机安全隔离。
- **C FFI 风险**：`lwext4_rust` 包含 C 库 `liblwext4` 的静态链接与 `bindgen` 生成的 FFI 绑定，存在潜在的内存不安全因素。
- **`unsafe` 代码**：内核中大量使用 `unsafe`（陷阱入口/出口、内联汇编、页表直接操作、信号栈注入等），修改这些区域需格外谨慎。
- **用户堆安全**：用户态使用 `buddy_system_allocator` 作为全局分配器，堆溢出或碎片可能导致未定义行为。
- **信号恢复机制**：当前 `syscall/signal.rs` 中的 `handle_signals` 在缺少 `sa_restorer` 时，将恢复代码放在用户栈上执行，存在栈溢出或注入风险（`docs/readme.md` 中已标记为待修复项）。
- **无页面置换与脏页回刷**：当前未实现 swap 或脏页写回，极端内存压力下可能直接 OOM 或丢失数据。

---

## 11. 已知问题与待办事项

以下内容摘录自 `docs/readme.md`，供代理在修改代码时参考：

- 暂未实现页面置换算法（未来可能引入 LRU）。
- 未实现 `fixed map`。
- 信号与多线程的交互关系仍存在问题。
- `dentry` 锁存在性能/正确性问题，需优化。
- 页缓存查找慢，LRU 可优化；**暂不支持脏页回刷**。
- 懒分配目前按**整个区域**映射，而非逐页分配。
- 栈自动扩大机制可能仍有问题。
- 不确定堆、内存、栈是否存在泄漏。
- Makefile 有待进一步整理。
- 需修改 `syscall/signal.rs` 中的 `handle_signals`，在无 `sa_restorer` 时使用更安全的恢复机制（不放在栈上）。
- 需考虑简化 LTP 测试到内核的路径。

---

## 12. 快速参考

```bash
# 进入开发容器后，构建并运行 RISC-V 内核（比赛环境）
cd /workspace/os
make run-sdcard          # 挂载 sdcard-rv.img

# 构建并运行 LoongArch 内核
cd /workspace/os
make ARCH=loongarch64 run-sdcard

# 仅构建内核
cd /workspace/os
make build

# 构建用户应用并注入到 SD 卡镜像
cd /workspace/os
make patch-sdcard

# 调试（RISC-V）
cd /workspace/os
make debug

# 生成两种架构的评测产物
cd /workspace
make all
```

---

> **最后更新**: 基于仓库当前实际内容生成。若后续修改了构建流程、目录结构或测试策略，请务必同步更新本文件。
