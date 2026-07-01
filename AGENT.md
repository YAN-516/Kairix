# Kairix Agent Guide

本文档面向在本仓库中协作的 AI agent。除非用户另有明确说明，以下规则适用于整个仓库。

## 项目概览

Kairix 是一个 Rust 实现的多架构多核宏内核，主要支持 RISC-V 和 LoongArch。仓库中的主要目录如下：

- `os/`：内核主体代码，包含系统调用、VFS/文件系统、内存管理、任务调度、trap、驱动、网络栈等。
- `user/`：用户态运行时库和测试/示例程序，测试入口通常位于 `user/src/bin/`。
- `polyhal/`：多架构硬件抽象层及启动/trap 支持。
- `lwext4_rust/`、`rust-fatfs/`：文件系统支持库。
- `tools/`：镜像、文件系统和构建辅助工具。
- `docs/`：架构图、测试说明和项目文档。

## 硬性规则

- 不要启动内核、QEMU 或任何模拟器后台任务。
- 不要运行会启动内核的目标，包括但不限于：
  - `make rkernel`
  - `make rkernel_test`
  - `make lkernel`
  - `make lkernel_test`
  - `make -C os run-inner`
  - `make -C os debug`
  - `make -C os gdbserver`
  - `make -C os run-sdcard`
  - `make -C os sdcard`
  - `qemu-system-*`
- 谨慎处理 sdcard 镜像。不要默认执行会挂载、写入或 patch 镜像的目标，例如 `make all`、`make -C os patch-sdcard`、`make -C os do-patch-sdcard`，除非用户明确要求并接受镜像变更。
- 不要把未经用户运行确认的修改描述为“已运行验证”或“runtime-tested”。如果只做了静态检查、编译或代码审阅，要明确说明。
- 不要无关重构。内核调试时优先做小范围、可解释的改动。
- 不要回滚用户已有改动。遇到非自己造成的工作区变更时，应先读懂并与之协作。

## 工作流程

1. 先定位子系统和架构路径。优先用 `rg` / `rg --files` 查找系统调用、结构体、feature gate、架构差异和既有测试。
2. 修改前阅读最近的实现和调用链。共享逻辑要同时留意 `riscv64` 与 `loongarch64` 的对应实现。
3. 针对不明确的问题，先形成小假设，再补充可观测性：
   - 在状态变化、错误分支或边界条件处加清晰日志。
   - 日志标签应能定位子系统，例如 `[sys_openat]`、`[vfs]`、`[cow]`、`[signal]`、`[socket]`。
4. 需要复现行为时，优先在 `user/src/bin/` 添加窄范围用户态测试程序；只有当既有测试正好覆盖同类行为时才扩展它。
5. 保持修改贴近问题所在模块。跨模块改动要解释原因和影响范围。

## 可用验证

只运行不会启动内核、不会隐式进入 QEMU、不会修改 sdcard 镜像的检查。常见可选项：

- `cargo fmt` 或指定 manifest 的格式化命令。
- `cargo check` / `cargo build`，前提是目标和配置不会触发 QEMU。
- `make -C user elf`
- `make -C user build`
- `make -C os ARCH=riscv64 build`
- `make -C os ARCH=loongarch64 build`
- `rg`、`rustc` 诊断、反汇编生成、文档检查等纯静态操作。

注意：`make -C os ARCH=... build` 会构建用户程序并准备内嵌运行时文件，但不应启动 QEMU。运行前仍需确认 Makefile 当前内容没有被改成运行目标。

## 调试和测试习惯

- 系统调用问题：从 `os/src/syscall/` 的 syscall 分发、参数读取、用户指针访问、错误码映射和返回值语义开始追踪。
- VFS/文件系统问题：同时查看 `os/src/fs/` 下的 VFS 层、具体文件系统实现、page cache/dentry cache、挂载逻辑和用户态测试。
- 内存问题：重点检查 `os/src/mm/`、缺页路径、懒分配、COW、mmap、权限位和 TLB/地址空间切换。
- 任务/调度/信号问题：查看 `os/src/task/`、`os/src/trap/`、`os/src/syscall/signal/`，注意上下文保存恢复和异步状态变更。
- 网络/socket 问题：查看 `os/src/net/` 与 `os/src/socket/`，优先确认协议状态机、阻塞/唤醒和资源释放路径。

## 代码风格

- 遵循现有模块结构和命名风格，优先复用已有 helper、错误类型和同步原语。
- Rust 代码保持小而直接；只有在能减少真实复杂度或符合现有模式时才新增抽象。
- 注释应解释非显然的状态机、内存安全条件或架构差异，避免重复代码表面含义。
- 新增用户态测试应尽量自包含，输出可 grep 的结果，失败路径要明确。

## 回复用户时

- 说明改了哪些文件、解决了什么问题、采用了什么验证。
- 如果不能运行运行时验证，要明确写出原因，并给出用户可运行的具体命令。
- 涉及日志或测试程序时，说明预期看到的关键信号。
- 对内核行为仍不确定时，直接标出剩余假设和下一步观测点。
