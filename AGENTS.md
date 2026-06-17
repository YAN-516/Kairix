# Kairix —— AI 编码代理项目指南

> 本文件面向 AI 编码代理。目标是让第一次进入 `/workspace` 的代理快速理解当前内核、构建路径、测试入口和高风险区域。内容基于仓库当前代码与文档整理；如果 Makefile、启动流程、测试镜像或核心模块发生变化，请同步更新本文件。

---

## 1. 项目概览

**Kairix** 是一个以 Rust 编写的现代化教学/竞赛操作系统内核，当前支持 **RISC-V 64** 与 **LoongArch64** 两个 QEMU `virt` 平台。内核以 `#![no_std]` 方式运行，依赖 `alloc`，通过本地 `polyhal` 屏蔽多架构差异，并以 ext4 SD 卡镜像作为主要运行环境。

当前主线能力包括：

- 多架构内核启动、陷阱处理、上下文切换与基础 SMP 支持。
- VFS 抽象，根文件系统默认挂载 ext4，同时支持 ext2/ext3、FAT32、devfs、procfs、sysfs、tmpfs、`/dev/shm`。
- 基于缺页异常的用户地址空间懒分配、COW、mmap、共享内存、用户栈扩展和 RISC-V `rt_sigreturn` 跳板页。
- 进程、线程、信号、futex、pipe、pidfd、基础调度与等待语义。
- 自研网络栈和 socket 层，覆盖 ARP、IPv4、ICMP、TCP、UDP、raw socket、loopback 与 VirtIO-net。
- 页缓存 LRU、脏页标记、延迟写回队列，以及低内存时的轻量 clean page reclaim。
- 面向 libc-test、LTP、iozone、lmbench、netperf、iperf、busybox 等测试脚本的镜像注入与自动运行流程。

关键事实：

- 仓库根目录：`/workspace`
- 默认架构：`riscv64`
- Rust Edition：2024
- 默认 toolchain：`nightly-2025-01-18`
- 主要 target：
  - `riscv64gc-unknown-none-elf`
  - `loongarch64-unknown-none`
- 顶层评测产物：`kernel-rv`、`kernel-la`
- 外部 SD 卡镜像：`sdcard-rv.img`、`sdcard-la.img`

---

## 2. 技术栈

| 层级 | 当前实现 |
|------|----------|
| 语言 | Rust `no_std` + `alloc`；部分 C FFI 来自 `lwext4_rust` |
| 构建 | Cargo + Make；默认离线 vendored 依赖 |
| 模拟器 | QEMU 9.2.1；`qemu-system-riscv64` / `qemu-system-loongarch64` |
| Boot | RISC-V 使用 RustSBI；LoongArch 使用 OpenSBI/直接 `-kernel` 路径 |
| HAL | 本地 `polyhal`、`polyhal-boot`、`polyhal-trap`、`polyhal-macro` |
| 文件系统 | ext4/ext3/ext2、FAT32、devfs、procfs、sysfs、tmpfs、etcfs、page cache |
| 块设备 | VirtIO block，RISC-V 走 MMIO，LoongArch 走 PCI |
| 网络 | VirtIO-net PCI/MMIO 相关探测，自研二到四层协议栈 |
| 内存 | `buddy_system_allocator`、自研 VMSet/VMA、懒分配、COW、page cache reclaim |
| 用户态 | Rust 用户库与若干测试程序；镜像中运行 musl/glibc 测试二进制 |

---

## 3. 目录结构

```text
/workspace
├── os/                  # 内核 crate，核心代码
├── user/                # no_std 用户态运行库和 Rust 用户程序
├── polyhal/             # 本地多架构 HAL 工作区
├── lwext4_rust/         # ext4 C 库绑定与 Rust 封装
├── rust-fatfs/          # FAT 文件系统依赖源码
├── tools/               # mkfs.ext 工具构建、镜像动态库注入、Cargo fallback config
├── bootloader/          # rustsbi-qemu.bin 等启动固件
├── libc-test/           # musl libc-test 源码/构建脚本
├── libc-bench/          # libc benchmark
├── netperf-2.7.0/       # netperf 源码
├── iperf/               # iperf 源码
├── docs/                # 测试记录、TODO、LTP 清单
├── judge/               # 评测辅助目录
├── Makefile             # 顶层构建/运行入口
├── README.md            # 项目简述，可能滞后于 Makefile
├── dev-env-info.md      # 容器工具链记录
└── rust-toolchain.toml  # 默认 nightly、组件和 target
```

注意：当前仓库没有顶层 `ltp/` 源码目录，LTP 相关记录主要在 `docs/ltp/`，实际测试二进制通常来自 SD 卡镜像中的 `/musl/ltp`、`/glibc/ltp`。

---

## 4. 构建与运行

### 4.1 顶层命令

在 `/workspace` 下：

| 命令 | 作用 |
|------|------|
| `make rkernel` | 构建 RISC-V，`AUTO_TEST=0` 注入镜像并进入交互 shell |
| `make rkernel_test` | 构建 RISC-V，`AUTO_TEST=1`，按 `initproc` 白名单自动跑测试 |
| `make lkernel` | 构建 LoongArch64，`AUTO_TEST=0`，进入交互 shell |
| `make lkernel_test` | 构建 LoongArch64，`AUTO_TEST=1` 自动跑测试 |
| `make all` | 构建 mkfs 工具、两种架构内核，并在镜像存在时 patch SD 卡 |
| `make mkfs-tools` | 为两种架构构建 `mkfs.ext2/3/4` |
| `make clean` | 清理内核、用户程序与顶层产物 |

顶层 QEMU 路径直接运行 `kernel-rv` / `kernel-la`，内存为 1G，单核，挂载 `sdcard-rv.img` / `sdcard-la.img`。

### 4.2 内核命令

在 `/workspace/os` 下：

| 命令 | 作用 |
|------|------|
| `make build` | `env` + `prepare-embedded` + 内核 ELF + strip 后 `.bin` |
| `make kernel` | 构建内核 ELF，依赖 `prepare-embedded` |
| `make prepare-embedded` | 先构建 mkfs.ext 工具，再构建用户态 ELF，供 `include_bytes!` 嵌入 |
| `make run-inner` | 使用 `SELECTED_IMG` 运行当前内核 |
| `make run-sdcard` | patch 外部 SD 卡镜像，启动 bridge，再运行 QEMU |
| `make patch-sdcard` | 将用户程序、mkfs 工具、动态库支持注入 SD 卡镜像 |
| `make debug` | RISC-V QEMU + GDB tmux 分屏 |
| `make gdbserver` | RISC-V QEMU `-s -S` |
| `make gdbclient` | 连接 `localhost:1234` |
| `make disasm` / `make disasm-vim` | 查看反汇编 |
| `make clean` | Cargo clean + 清理用户程序 + 清理 mkfs 构建物 |

常用变量：

- `ARCH=riscv64|loongarch64`
- `MODE=release`
- `LOG=OFF|ERROR|WARN|INFO|DEBUG|TRACE`
- `AUTO_TEST=1|0`
- `CPU=1`，可手动增大，但 SMP 相关路径要谨慎验证
- `CARGO_OFFLINE=1` 默认启用，使用 vendored 依赖
- `SELECTED_IMG=...` 指定 QEMU 磁盘镜像
- `SDCARD_IMG=...` 指定 patch 的外部镜像
- `NET_BACKEND=user|bridge|auto`
- `NET_DUMP=1` 默认生成 `os/qemu-net.pcap`
- `BRIDGE_IF=br0`

### 4.3 用户态命令

在 `/workspace/user` 下：

| 命令 | 作用 |
|------|------|
| `make elf` | 构建所有 `user/src/bin/*.rs` ELF |
| `make binary` | ELF 转 raw binary |
| `make build` | `binary` |
| `make clean` | Cargo clean |

默认 target 为 `riscv64gc-unknown-none-elf`。LoongArch 构建使用：

```bash
make TARGET=loongarch64-unknown-none build
```

`TEST=1` 时，`user/Makefile` 会把 `usertests` 复制为 `initproc`，但当前正式流程主要依赖 `initproc.rs` 自己的自动测试脚本白名单。

---

## 5. 镜像注入与自动测试流程

`os/Makefile` 的 `patch-sdcard` / `do-patch-sdcard` 会：

1. 执行 `../user && make build MODE=$(MODE) TARGET=$(TARGET)`。
2. 对目标镜像运行 `e2fsck -f -y`。
3. mount 镜像到临时目录。
4. 根据 `AUTO_TEST` 创建或删除 `/.initproc-no-autotest`：
   - `AUTO_TEST=0` / `false` / `off`：创建该文件，启动后进入交互 shell。
   - 其他值：删除该文件，启动后自动跑官方脚本白名单。
5. 复制 `initproc`、`user_shell`、`ls`、`basictests`、`libctests_static`、`libctests_dynamic` 到镜像根目录。
6. 不再在 patch 阶段创建 `/bin` busybox 链接；这些由 `initproc` 运行时处理。
7. 安装真实 `mkfs.ext2/3/4` 工具及 wrapper。
8. 调用 `tools/setup-sdcard-libs.sh` 设置 glibc/musl 动态链接器与共享库路径。

内核启动后，`os/src/embedded.rs` 还会确保 `/bin`、`/sbin`、`/lib`、`/lib64`、`/usr/lib64`、`/musl/ltp/testcases/bin` 等目录存在，复制可用的动态运行库，并安装嵌入式 `mkfs.ext2/3/4` 和 `mke2fs.conf`。

`user/src/bin/initproc.rs` 是自动测试总控：

- 建立 busybox 常用命令到 `/bin` 的软链接。
- 根据 `os/src/syscall/ltp_exec_filter.rs` 的 LTP 白名单，在 `/sdcard/musl`、`/sdcard/glibc` 下构建过滤视图。
- 优先运行 `/sdcard/...` 中存在的测试脚本，否则回退到 `/musl/...`、`/glibc/...`。
- 当前白名单脚本顺序包括 iozone、LTP、basic、busybox、cyclictest、libctest、libcbench、lua、lmbench、iperf、netperf 的 musl/glibc 组合。
- 每个脚本结束后会清理 `/tmp`、回收僵尸进程、`sync`，最后 `poweroff(last_exit)`。

---

## 6. QEMU 与网络

`os/Makefile` 当前 QEMU 参数要点：

- RISC-V：
  - `-machine virt`
  - `-bios ../bootloader/rustsbi-qemu.bin`
  - `-device loader,file=$(KERNEL_BIN),addr=0x80200000`
  - `-device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0`
  - 默认 `-m 1G -smp $(CPU)`
- LoongArch64：
  - `-machine virt`
  - `-kernel $(KERNEL_ELF)`
  - `-device virtio-blk-pci,drive=hd0`
  - 默认 `-m 1G -smp $(CPU)`
- 网络：
  - 默认 `NET_BACKEND=user`
  - `NET_BACKEND=bridge` 使用 `make net-up-bridge BRIDGE_IF=br0`
  - `NET_BACKEND=auto` 会根据 bridge 是否存在选择
  - 默认 `NET_DUMP=1`，抓包到 `os/qemu-net.pcap`

顶层 Makefile 的 QEMU 命令与 `os/Makefile` 略有不同：顶层直接加载 `kernel-rv` / `kernel-la`，并使用外部 `sdcard-*.img`。

---

## 7. 代码组织

### 7.1 内核 `os/src`

| 路径 | 说明 |
|------|------|
| `main.rs` | 内核入口、主核初始化、trap 统一处理、timer、信号交付、延迟写回触发、启动调度 |
| `arch/` | 架构相关启动/辅助代码 |
| `boards/qemu.rs` | QEMU 板级常量和块设备类型 |
| `config.rs` | 常量，如 `MAX_CPU_NUM`、`MMAP_BASE`、RISC-V signal trampoline VA |
| `embedded.rs` | 启动后向根文件系统安装运行时文件、动态库、mkfs.ext 工具 |
| `drivers/block/` | VirtIO block、PCI/MMIO 探测、块设备抽象 |
| `devices/` | 设备层入口 |
| `fs/` | 文件系统总入口、挂载、page cache、writeback |
| `fs/vfs/` | dentry、inode、file、path、superblock、fstype、dcache |
| `fs/lwext4/` | ext2/3/4 实现，基于 `lwext4_rust` |
| `fs/fat32/` | FAT32 实现 |
| `fs/devfs/` | `/dev/null`、`zero`、`tty`、`rtc`、`urandom`、loop 设备等 |
| `fs/procfs/` | `/proc` 文件，如 maps、smaps、status、mounts、meminfo、pagemap、fanotify/inotify 信息 |
| `fs/sysfs/` | 当前主要提供 block 相关 sysfs 节点 |
| `fs/tmpfs/` | tmpfs 与 `/dev/shm` |
| `mm/` | frame/heap allocator、VMSet/VMA、lazy fault、COW、reclaim |
| `net/` | ARP、Ethernet、IPv4、ICMP、TCP、UDP、route、neighbor、loopback、skb、VirtIO-net |
| `socket/` | TCP/UDP/raw socket 层 |
| `sync/` | `SpinNoIrqLock`、`SleepLock`、可重入锁等同步原语 |
| `syscall/` | syscall 分发与 fs/mm/net/process/thread/signal/time/futex/shm/landlock/inotify/fanotify 等实现 |
| `task/` | 进程、线程、调度器、任务上下文、ID 分配、信号状态 |
| `trap/` | trap context、汇编入口和 trap 支撑 |
| `timer.rs` | timer 设置与时间读取 |
| `sbi.rs` / `sbi_la.rs` | 架构固件调用 |

启动顺序大致为：

1. `main.rs` 中主核初始化日志、heap、frame allocator、polyhal、trap、内核 VM。
2. `net::init()`，`init_processors()`。
3. `fs::init()` 挂载根 ext4、devfs、`/dev/shm`、etcfs、procfs、tmpfs、sysfs。
4. `embedded::install_runtime_files()` 补运行时工具与库。
5. `task::add_initproc()` 添加嵌入的 `initproc`。
6. 唤醒从核并进入 `task::run_tasks()`。

### 7.2 用户态 `user/src`

| 路径 | 说明 |
|------|------|
| `lib.rs` | `_start`、堆初始化、syscall 安全封装、用户态 API |
| `syscall.rs` | syscall 号、RISC-V `ecall` / LoongArch `syscall 0` |
| `console.rs` | `print!`、`println!`、输入输出 |
| `lang_items.rs` | panic handler |
| `bin/initproc.rs` | 当前启动进程、busybox 链接、自动测试白名单 |
| `bin/user_shell.rs` | 简单交互 shell |
| `bin/libctests_*`、`libcbench`、`basictests`、`usertests` | 测试入口 |
| `bin/ping*`、`tcp_*` | 网络测试程序 |
| 其他 `bin/*.rs` | 基础文件、fork、sleep、signal、栈溢出等测试程序 |

当前 `user/src/bin` 约 29 个 Rust 用户程序，Makefile 会按文件名批量构建 ELF 和 `.bin`。

### 7.3 `polyhal`

`polyhal` 是本项目多架构抽象层：

- `polyhal/polyhal`：页表、地址、timer、irq、启动相关公共接口。
- `polyhal/polyhal-boot`：架构入口宏。
- `polyhal/polyhal-trap`：trap 类型与 trap frame。
- `polyhal/polyhal-macro`：过程宏。

新增架构相关代码时，优先通过 `polyhal` 已有抽象解决；只有确实无法表达时再加 `#[cfg(target_arch = "...")]`。

---

## 8. 内存、文件系统和信号现状

内存管理：

- `UserMapArea` 记录 VMA 范围、权限、COW、lazy、growdown、文件映射、共享内存 ID 等。
- 用户访问内核复制路径通过 `translated_byte_buffer*`，必要时会触发 lazy fault。
- RISC-V 当前有 `USER_RT_SIGRETURN_TRAMPOLINE`，避免把 signal restorer 代码放到用户栈上。
- 页缓存已有 LRU 元数据，最大默认约 4096 页。
- `mm/reclaim.rs` 提供低水位/高水位检测，只在分配回退路径回收 clean cache page，并请求延迟写回。
- 仍未实现完整 swap/page replacement，低内存和大量脏页场景要特别谨慎。

文件系统：

- 根文件系统名 `DISK_FS_NAME` 当前是 `ext4`。
- `FS_MANAGER` 注册 ext4/ext3/ext2/fat32/devfs/etc/proc/tmpfs/sysfs。
- `GLOBAL_DCACHE` 会 pin `/`、`/dev`、`/dev/shm`、`/etc`、`/proc`、`/tmp`、`/sys`。
- `fs/writeback.rs` 维护延迟写回队列；`sync`、`umount`、文件关闭和 syscall 返回路径都可能触发写回。
- loop 设备会 lazy queue backing file，mkfs 和 mount 相关测试容易触发大量脏页。

信号：

- RISC-V 和 LoongArch 分别在 `syscall/signal/riscv64.rs`、`loongarch64.rs`。
- 当前已加入信号跳板页思路，并修过 sleep/取消语义相关问题。
- 同步异常信号（SIGSEGV、SIGBUS、SIGILL）在 trap 里会解除阻塞后投递，避免 sigreturn 后重复陷入。
- `docs/readme.md` 仍提示“信号和多线程之间的关系还是有问题”，改动该区域必须配合 pthread、cancel、cond、tsd、robust detach 等测试验证。

---

## 9. Syscall 与测试热点

`os/src/syscall/mod.rs` 使用 Linux 风格 syscall 号分发，当前覆盖范围较广，包括：

- 文件系统：openat、close、read/write/readv/writev、getdents、stat/fstat/statx、mount/umount、fallocate、copy_file_range、splice、sync/fsync/fdatasync、xattr、openat2、close_range 等。
- 进程/线程：fork、clone/clone3、execve、wait/waitid/waitpid、exit/exit_group、setpgid/setsid、gettid、set_tid_address、robust list。
- 内存：brk、mmap、munmap、mprotect、msync、madvise、mlock、shm。
- 信号/时间：rt_sigaction、rt_sigprocmask、rt_sigsuspend、rt_sigtimedwait、rt_sigreturn、kill/tkill/tgkill、clock、timerfd、itimer。
- 同步：futex。
- 网络：socket、bind、listen、accept、connect、sendto/recvfrom、getsockopt/setsockopt、socketpair、shutdown。
- 竞赛兼容：landlock、fanotify、inotify、pidfd、cap、bpf/io_uring/userfaultfd 等部分 stub 或简化实现。

LTP 执行过滤：

- `os/src/syscall/ltp_exec_filter.rs` 当前启用 **whitelist**。
- `initproc` 会读取这个源文件中的白名单，只为白名单 case 建立 `/sdcard/.../ltp/testcases/bin` 软链接。
- 修改 LTP 支持时，除了实现 syscall，也要同步检查这个白名单和 `docs/ltp/` 记录。

---

## 10. 测试资料与当前缺口

主要文档：

- `docs/basic.md`：基础 syscall 勾选表。
- `docs/libctest.md`：libc-test 当前结果。
- `docs/ltp/ltp_menu.md`：LTP case 细表与分数记录。
- `docs/ltp/ltp_rank.md`、`ltp_todo_fs.md`、`ltp_fs_signal_plan.md`：LTP 排名/计划/待办。
- `docs/iozone.md`：iozone 结果，记录了 LRU 后表现。
- `docs/lmbench_testcode.md`：lmbench 结果与对比。
- `docs/readme.md`：最新 TODO 和团队备注，优先级通常高于 README。

当前已知缺口和高风险点：

- 多用户/用户组仍待完善。
- 文件锁仍待实现。
- signal + multithread 关系仍存在问题。
- dentry 锁和 dentry cache 仍有性能/正确性优化空间。
- page cache 已有 LRU 与延迟写回，但查找和大量脏页场景仍需优化。
- lazy allocation 仍有区域级处理痕迹，逐页语义需谨慎核对。
- 栈自动扩大、堆碎片、内存/栈泄漏仍需排查。
- landlock、fanotify、inotify 当前实现偏临时，改动前先看测试期望。
- LoongArch lmbench 仍未拿全分。
- cyclictest 分数原因仍需定位。
- glibc/musl iozone 约 33 分，反向读和预读取仍是关键优化点。

libc-test 当前主要未通过项集中在：

- pthread cancel / cond / tsd / robust detach 相关。
- socket。
- dynamic 下还有 tls 与 `sem_init`。

---

## 11. 开发环境

Dev Container：

- `.devcontainer/devcontainer.json` 使用镜像 `zhouzhouyi/os-contest:20260510`。
- `remoteUser` 为 `root`。
- 工作目录 `/workspace`。
- `--privileged --network=host`。
- 清空 HTTP/HTTPS 代理，并为 GitHub 添加固定 host。

工具链记录：

- `rustc 1.86.0-nightly (2025-01-10)`，默认 `nightly-2025-01-18`。
- QEMU 9.2.1。
- 已安装 target 记录见 `dev-env-info.md`。
- `rust-toolchain.toml` 包含 `rust-src`、`llvm-tools`、`rustfmt`、`clippy`，默认只列出 RISC-V target；LoongArch target 由容器环境提供。
- `os/.cargo/config.toml` 与 `user/.cargo/config.toml` 默认使用 `vendor/` 替代 crates.io。

如果 `.cargo/config.toml` 不存在，Makefile 会回退到：

- `tools/cargo-config-os.toml`
- `tools/cargo-config-user.toml`

---

## 12. 编码规范与协作约定

- 内核和用户库均是 `no_std`，不要引入 `std`。
- 遵循 Rust 2024 edition 和现有模块风格。
- 优先复用 `polyhal`、VFS、task、mm、sync 现有抽象，不要绕过公共接口直接拼架构细节。
- 新增 syscall 时：
  - 在 `syscall/mod.rs` 加 syscall 号和分发。
  - 具体实现放入对应子模块。
  - 参数从用户态读取必须走 `translated_ref*` / `translated_byte_buffer*` 等路径。
  - 返回 Linux 风格 errno，避免 panic。
- 修改内存/页表/信号/trap/调度时：
  - 先读相关锁顺序和当前 trap 返回路径。
  - 避免在持有进程锁时触发用户缺页或可能阻塞的文件操作。
  - 注意 `task.process.upgrade()` 可能失败，当前代码已有孤儿线程处理。
- 修改 FS/VFS 时：
  - 关注 dentry cache、inode mode、mount point、page cache dirty/writeback。
  - `sync`、`umount`、loop、mkfs、LTP mount 测试是高风险验证点。
- 修改网络时：
  - 同时验证 loopback 和 VirtIO-net。
  - 保留 `NET_DUMP=1` 抓包能力，必要时分析 `qemu-net.pcap`。
- 注释语言：
  - Rust 模块文档偏英文。
  - Makefile、docs、团队 TODO 偏中文。
  - 修改队友代码时，可用简短中文注释说明关键行为变化，但不要堆叠无意义注释。
- 格式化：
  - 可使用 `cargo fmt`。
  - `cargo clippy` 在内核场景可能暴露大量既有问题，按任务风险选择运行范围。

---

## 13. 验证建议

按改动范围选择最小有效验证：

- 文档/脚本小改：检查命令或脚本语法即可。
- 用户态库或程序：`cd /workspace/user && make elf`。
- 内核通用改动：`cd /workspace/os && make build`。
- RISC-V 启动与交互：`cd /workspace && make rkernel LOG=INFO`。
- LoongArch 启动与交互：`cd /workspace && make lkernel LOG=INFO`。
- 自动评测路径：`make rkernel_test` 或 `make lkernel_test`。
- 镜像注入逻辑：`cd /workspace/os && make ARCH=riscv64 AUTO_TEST=0 patch-sdcard`。
- 文件系统/写回/mount：重点跑 iozone、LTP mount/umount、mkfs.ext、loop 相关 case。
- 信号/线程：重点跑 libc-test pthread、signal、sleep cancel、futex、robust list 相关。
- 网络：跑 `ping`、TCP/UDP socket 测试、iperf/netperf 脚本，并保留 pcap。

注意：`patch-sdcard` 需要 mount/umount 镜像，通常依赖特权容器；失败时先确认镜像存在、权限、loop/mount 能力和 `e2fsck`。

---

## 14. 快速参考

```bash
# RISC-V 交互运行
cd /workspace
make rkernel LOG=INFO

# RISC-V 自动测试
cd /workspace
make rkernel_test LOG=OFF

# LoongArch64 交互运行
cd /workspace
make lkernel LOG=INFO

# LoongArch64 自动测试
cd /workspace
make lkernel_test LOG=OFF

# 仅构建 RISC-V 内核
cd /workspace/os
make ARCH=riscv64 build

# 仅构建 LoongArch64 内核
cd /workspace/os
make ARCH=loongarch64 build

# patch SD 卡但不运行
cd /workspace/os
make ARCH=riscv64 AUTO_TEST=0 patch-sdcard

# 构建两架构正式产物
cd /workspace
make all
```

最后提醒：`README.md` 和旧测试记录可能滞后；遇到冲突时，以当前 Makefile、`initproc.rs`、`docs/readme.md` 和实际代码为准。

---

> 最后更新：2026-06-17，基于 `/workspace` 当前内核与构建脚本重写。
