# Kairix

**Kairix** 是由 团队开发的一款基于 Rust 语言和 RISC-V 架构的现代化操作系统内核。

## 特性

*   **异步架构：无栈调度**
    引入轻量化异步协程调度机制，极大地降低了任务上下文切换的内存与时间开销。
*   **存储抽象：多文件系统支持**
    具备完善的 VFS 层抽象，支持同时挂载和管理多种不同类型的文件系统（如 EasyFS, FAT32 等）。
*   **智能管理：懒分配**
    基于缺页异常的动态内存映射技术，仅在实际访问时分配物理页面，显著优化内存利用率。
*   **内存安全**：完全由 Rust 语言实现，利用其所有权系统从根源消除缓冲区溢出和空指针异常。
*   **Unix 风格**：提供标准的进程管理、信号处理、管道通信等系统调用接口。

---

## 环境

### 1.下载大赛官方提供的镜像(https://github.com/oscomp/testsuits-for-oskernel/tree/pre-2025)
```
#需安装docker，在官方的根目录下
1.make docker  #进入docker环境
2.make         #构建镜像文件
```

### 2.即可在vscode中通过打开容器的功能打开开发功能

## 构建与运行
### 1.克隆仓库
```
git clone https://github.com/YAN-516/Kairix/tree/master
cd kairix
```
### 2.构建内核
```
cd os
make build
```
### 3.运行内核
```
make run
```
### 4.调试内核(待做)
1.使用gdb

2.log日志输出


## 开发
### 目录结构(├── └── │)
```
kairix/
├──bootloader   #启动代码
├──os           #内核代码
└──user         #用户态代码
```
## 注意事项
默认的路径是kairix，对应容器中挂载的workspace


## 贡献
欢迎提交Issue和Pull Request！
## 许可证
## 致谢
chronix:文件系统
感谢所有为kairix项目做出贡献的开发者。


