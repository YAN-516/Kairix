VirtIOBlk：本质上是一个硬件的操控面板，从(0x1000_1000, 0x00_1000)，被UPSafeCell包裹


文件系统的数据流：
exec("")->sys_exec句柄寻址->osinode，维护了当前进程读取到的字节数->vfs->到具体文件，inode层->lwext4->Disk(处理块和字节的差异)->VirtIOBLk(将数据搬入内存中)->qemu(DMA读取具体的镜像文件)


## 注意事项
VirtoHal在取消跳板页之后需要进行修改
VirtoHal存在内存泄漏,重复回收的风险
未来 KernelDevOp 的 read/write 接口将是异步化改造的核心点。
VfsNodeOps 的使用是为了支持多文件系统
# 架构概览
本文档旨在展示本目录中各文件之间的逻辑关系，以及自底向上的文件系统层级结构。
## 层级结构
### 1. lwext4 文件系统底层适配 (硬件抽象层)
设计思路
在 lwext4_rust 的源代码中，blockdev.rs 定义了核心的 KernelDevOp trait。该接口是整个文件系统的基石，因为它定义了文件系统如何与底层的块设备进行交互。
code
Rust
// 位于 lwext4_rust/src/blockdev.rs
pub trait KernelDevOp {
    type DevType;

    fn write(dev: &mut Self::DevType, buf: &[u8]) -> Result<usize, i32>;
    fn read(dev: &mut Self::DevType, buf: &mut [u8]) -> Result<usize, i32>;
    fn seek(dev: &mut Self::DevType, off: i64, whence: i32) -> Result<i64, i32>;
    fn flush(dev: &mut Self::DevType) -> Result<usize, i32>
    where
        Self: Sized;
}
### 具体实现过程
1. 实现 HAL trait：首先，我们需要利用 Chronix 内核的物理内存分配逻辑为块设备实现 Hal trait。这是使 VirtIOBlk 结构体能够在内核环境中正常工作的必要前提。
磁盘抽象 (disk.rs)：在 disk.rs 中，我们通过 VirtIOBlk 初始化 Disk 结构体，并为其实现基础的磁盘访问逻辑。
对接驱动接口：为 Disk 结构体实现 KernelDevOp trait。完成此步后，Disk 便正式作为一个合格的底层设备，为上层文件系统提供读写服务。
(注：该部分代码主要参考并适配自 examples/src。)
2. lwext4 文件系统核心
此层直接引用 lwext4 的核心代码库，它封装并暴露了文件系统的主要操作接口：Ext4File。
关于 Ext4File：它对底层的 C 语言操作进行了 Rust 包装。通过该结构体，我们能为 Rust OS 的上层提供标准的文件系统接口，包括：file_open (打开)、file_read (读取)、file_write (写入)、file_seek (寻址)、file_close (关闭)、file_rename (重命名) 以及 lwext4_dir_entries (遍历目录) 等。
3. lwext4 文件系统上层抽象 (VFS 层)
此层负责将具体的 ext4 操作映射到内核通用的虚拟文件系统 (VFS) 接口上。

### 模块依赖关系：
the relationship:
- File
    - Stdio
    - OSInode
        - FileWrapper + VfsNodeOps
            - Ext4File
        - Ext4FileSystem + VfsOps

1. File (通用的文件 trait 抽象)
2. Stdio (标准输入输出设备)
3. OSInode (操作系统层面的 Inode 封装)
4. FileWrapper + VfsNodeOps (针对单个文件的具体操作)
5.  内部封装了 Ext4File
6. Ext4FileSystem + VfsOps (针对整个文件系统的全局操作)

### 实现细节：
欲了解 Ext4FileSystem 对 VfsOps 的实现，以及 FileWrapper 对 VfsNodeOps 的具体逻辑，请参考 ext4fs.rs。
我们通过组合 FileWrapper 和 Ext4FileSystem 来构建最终的 OSInode。
文件系统管理的其他辅助代码（如文件描述符管理等）则直接沿用了 rCore-Tutorial 第六章 的成熟设计。
