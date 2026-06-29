![哈工大深圳](./docs/hitsz_logo.jpg)
# Kairix

## 项目描述
**Kairix** 是由Unicus团队开发的一款基于Rust语言，支持RISC-V和LoongArch架构的多核宏内核操作系统内核。

## 完成情况
### 初赛
截止6月29日17时13分，Kairix已通过初赛大部分测试点，并在排行榜上位于前列：

![初赛排行榜1](./docs/初赛排行榜1.png)

![初赛排行榜2](./docs/初赛排行榜2.png)


### 功能介绍
*   **文件系统**
    提供类Linux的VFS架构，支持带LRU淘汰的Dentry Cache和统一Page Cache。支持Ext4、FAT32等磁盘文件系统，以及内存文件系统（tmpfs）、进程文件系统（procfs）。并具备较灵活的挂载管理能力。
*   **内存管理**
    基于缺页异常的动态内存映射技术，使用懒分配和copy_on_write策略，优化内存利用率，支持共享内存区域映射，便于高效资源共享。
*   **内存安全**
    完全由Rust语言实现，利用其所有权系统降低缓冲区溢出和空指针异常的风险。
*   **进程管理**
    支持多进程并发执行，每个进程都有自己的地址空间和资源，通过系统调用进行通信和资源管理。
*   **信号处理**
    实现了符合POSIX标准的信号系统，支持异步信号处理，支持用户自定义信号处理例程。
*   **设备驱动**
    复用一部分polyhal和DelOn1x的代码，支持MMIO（内存映射I/O）、PCI/ECAM设备探测、VirtIO块设备与VirtIO-net设备驱动。
*   **网络模块**
    自研网络栈、支持TCP UDP套接字、支持本地回环设备和IPv4协议栈。
---
![整体架构](./docs/整体架构图.svg)
### 项目文档
- [初赛文档](./Unicus初赛文档.pdf)
- [初赛演示视频](https://pan.baidu.com/s/1WML2KYY-YOFzeLGUteyLQQ?pwd=hk9w):提取码：hk9w
- [初赛PPT](./Unicus初赛PPT.pdf)
## 运行方式
进入docker之后

**比赛环境**，可于os目录下（磁盘文件需要提供）
- 键入 `make all` 即可编译得到磁盘镜像以及内核可执行文件
- 键入 `make rkernel`即可编译执行riscv架构的内核。
- 键入 `make lkernel`即可编译执行loongarch架构的内核。

## 开发
### 目录结构
```
Kairix/
├── os/                  # 内核主体代码：进程、内存、VFS、网络、驱动、系统调用等
│   ├── src/
│   │   ├── main.rs      # 内核入口，完成初始化后进入任务调度
│   │   ├── config.rs
│   │   ├── logging.rs   # 日志初始化
│   │   ├── console.rs   # 内核控制台输出封装
│   │   ├── error.rs     # 内核错误码与系统调用错误类型
│   │   ├── lang_items.rs # no_std panic/语言项支持
│   │   ├── timer.rs     # 时钟与定时器相关逻辑
│   │   ├── embedded.rs  # 启动时内置文件安装
│   │   ├── sbi.rs       # RISC-V SBI 调用封装
│   │   ├── sbi_la.rs    # LoongArch 固件调用封装
│   │   ├── entry.asm    # 早期汇编入口
│   │   ├── link_app.S   # 用户程序链接辅助
│   │   ├── linker-*.ld  # RISC-V/LoongArch/QEMU 链接脚本
│   │   ├── arch/        # 架构相关代码，包含 riscv_dir 与 loongarch_dir
│   │   ├── boards/      # 板级配置，当前主要面向 QEMU virt
│   │   ├── devices/     # 通用设备抽象
│   │   ├── drivers/     # 设备驱动，包含 VirtIO 块设备与 PCI 探测
│   │   ├── fs/          # 文件系统子系统
│   │   ├── mm/          # 内存管理
│   │   ├── net/         # 网络协议栈
│   │   ├── socket/      # socket 层
│   │   ├── sync/        # 同步原语
│   │   ├── syscall/     # 系统调用实现
│   │   ├── task/        # 进程/线程管理、调度器、PID、上下文切换
│   │   └── trap/        # trap/异常/中断处理入口与上下文
│   ├── .cargo/          # 内核 crate 本地 Cargo 配置
│   └── vendor/          # 内核构建使用的离线依赖
├── user/                # 用户态运行时库与测试/示例程序
│   ├── src/
│   │   ├── lib.rs       # 用户态运行时入口、堆初始化、系统调用安全封装
│   │   ├── syscall.rs   # 用户态系统调用号与 ecall/syscall 汇编封装
│   │   ├── console.rs   # 用户态 print/println 与输入输出辅助
│   │   ├── lang_items.rs # 用户态 no_std panic/语言项支持
│   │   ├── linker.ld    # 用户程序链接脚本
│   │   └── bin/         # 用户程序与测试入口
│   ├── .cargo/          # 用户程序 crate 本地 Cargo 配置
│   └── vendor/          # 用户程序构建使用的离线依赖
├── polyhal/             # 多架构硬件抽象层
│   ├── polyhal/         # HAL 核心实现
│   ├── polyhal-boot/    # 启动入口与架构初始化
│   ├── polyhal-trap/    # trap/中断上下文抽象
│   ├── polyhal-macro/   # 架构相关过程宏
│   └── example/         # HAL 示例程序
├── bootloader/          # 启动固件，当前包含 rustsbi-qemu.bin
├── lwext4_rust/         # ext4 文件系统绑定与 lwext4 C 库
├── rust-fatfs/          # FAT/FAT32 文件系统实现
├── iperf/               # iperf 网络性能测试工具源码
├── netperf-2.7.0/       # netperf 网络性能测试工具源码
├── tools/               # 镜像与文件系统工具
├── patches/             # 兼容补丁与移植补丁
├── docs/                # 项目文档、架构图与测试说明
├── .devcontainer/
├── .vscode/
├── Makefile
├── rust-toolchain.toml
├── Unicus初赛文档.pdf
├── Unicus初赛PPT.pdf
├── README.md
├── LICENSE
├── .gitignore
├── .dockerignore
└── dev-env-info.md
```
## AI使用情况
Unicus团队使用了AI工具进行辅助开发Kairix内核，使用的模型是kimi2.6和GPT5.5,我们主要使用的范畴包括：
- 1.重复性代码的辅助生成，但是整体架构和思路来源于Unicus团队。
- 2.辅助开发Kairix内核时候的调试，但是日志的位置和内容来源于Unicus团队的编写，AI只是帮助查找BUG。
- 3.文档的润色，但是初稿编写以及最终审查都是Unicus团队进行。
- 4.生成测试代码，由Unicus团队描述测试内容，AI进行代码生成。
- 5.辅助阅读代码，先由AI对参考代码进行凝练，团队成员再进行源码的阅读。
  
## 贡献
欢迎提交Issue和Pull Request！

## 项目人员
哈尔滨工业大学（深圳）：

- 颜晨   1748323932@qq.com:文件系统，进程调度，异常机制。
- 萧鹏   2813498706@qq.com:信号、进程间通信、网络。
- 雷鑫言 250745208@qq.com: 内存管理，多架构设计和硬件抽象层。
- 指导老师：夏文、仇洁婷

## 致谢
- [Chronix](https://gitlab.eduxiji.net/educg-group-36002-2710490/T202518123995568-675):文件系统
- [polyhal](https://github.com/oscomp/polyhal)、[DelOn1x](https://github.com/Ya0rk/myOS/tree/main):多架构设计
- [rcore-os/rCore](https://github.com/rcore-os/rCore): 用户态程序
- [Titanix](https://gitlab.eduxiji.net/202318123101314/oskernel2023-Titanix): 锁
- [PhoenixOS](https://github.com/oscomp/first-prize-osk2024-phoenix)、 [Chronix](https://gitlab.eduxiji.net/educg-group-36002-2710490/T202518123995568-675)、[NighthawkOS](https://gitlab.eduxiji.net/T202518123995755/oskernel2025-nighthawkos): 设计文档

感谢所有为kairix项目做出贡献的开发者。
