# Kairix LTP 文件系统模块实现计划（ltp_fs 分支）

> 本计划面向 **ltp_fs** 分支，仅覆盖文件系统（FS）相关测例。信号模块已由其他同学负责，不在本文档范围内。  
> 制定原则：**先修 Bug 解锁已有高分，再按「工作量/分值」性价比逐层补齐。**

---

## 一、当前现状盘点

### 1.1 已具备基础（syscall 已实现）

| 类别 | 已实现内容 |
|------|-----------|
| 基础文件 IO | openat / close / read / write / pread64 / pwrite64 / lseek |
| 目录操作 | mkdirat / unlinkat / getdents64 / chdir / getcwd |
| 链接 | linkat（硬链）/ symlinkat / readlinkat |
| 属性 | fstat / fstatat / statx / fchmodat / fchownat / utimensat / umask |
| 高级 IO | readv / writev / sendfile / pselect6 / ppoll / ioctl(TTY/RTC) |
| 文件系统 | statfs / fsync / ftruncate / sync(桩) / mount(桩) / umount2(桩) |
| 定时器 fd | timerfd_create / timerfd_settime / timerfd_gettime（本分支新增） |

> **注**：信号相关 syscall（sigaction / kill / sigpending / sigwaitinfo / signalfd 等）由其他同学负责，不计入本计划。

### 1.2 关键缺口

| 缺口 | 当前状态 | 对 LTP 的影响 |
|------|---------|--------------|
| **fcntl 文件锁** | 仅支持 dupfd/getfl/setfl；**F_SETLK/F_GETLK/F_SETLKW 未实现** | fcntl15(12) 及大量 fcntlXX 测例失败 |
| **mount/umount** | 桩实现（直接返回 0） | mount03(76), mount07(56), mount01~08 等 ≈ **180 分**无法通过 |
| **xattr** | VFS 层无接口；但 **lwext4 底层已支持** ext4_setxattr/getxattr/listxattr/removexattr | fsetxattr01(31), setxattr01(31), fgetxattr01(17) 等 ≈ **150+ 分** |
| **fallocate** | 完全未实现；lwext4 无 fallocate API | fallocate06(27), fallocate04(12), fallocate05(17) ≈ **64 分** |
| **splice/tee/vmsplice** | 完全未实现 | splice07(591) 等 ≈ **620 分** |
| **inotify** | 完全未实现 | inotify10(10), inotify02(9) 等 ≈ **60 分** |
| **fanotify** | 完全未实现 | fanotify16(770) 等 ≈ **2200+ 分** |
| **copy_file_range** | 完全未实现 | copy_file_range02(28) 等 ≈ **50 分** |
| **close_range** | 完全未实现 | close_range01(20), close_range02(11) = **31 分** |
| **sync_file_range** | 完全未实现 | sync_file_range02(12), sync_file_range01(5) = **17 分** |
| **name_to_handle_at / open_by_handle_at** | 完全未实现 | name_to_handle_at01(27) 等 ≈ **52 分** |
| **新 mount API** | fsopen/fsconfig/fsmount/fspick/move_mount/open_tree/mount_setattr 缺失 | fsmount01(150) 等 ≈ **400 分** |
| **ioctl FS 扩展** | fiemap / ficlone / ficlonerange 缺失 | ioctl_ficlone04(600), ioctl_fiemap01(57) ≈ **670 分** |

### 1.3 已知隐患（os/src/fs/readme.md 提及）

1. **dentry 锁策略混乱**：`GLOBAL_DCACHE` 和部分 dentry 操作未充分加锁，多核下可能死锁或竞态。
2. **页缓存性能差**：查找文件慢，且**不支持脏页回刷**（影响 fsync 语义）。

---

## 二、分阶段实施计划

### 阶段 0：修 Bug + 确保基础高分通过（P0，第 1 周）

**目标**：让「已有 syscall」对应的 LTP 高分测例真正跑通，不丢冤枉分。

| 任务 | 具体工作 | 解锁分值 |
|------|---------|---------|
| **access01 调优** | `sys_faccessat` 当前仅做 `resolve_path` 简单判断。需补全 root 绕开权限检查、AT_EACCESS、以及对 SUID/SGID 文件的特殊处理。 | **199** |
| **mount 桩语义修正** | 当前 mount/umount 直接返回 0，LTP 会检查 `/proc/mounts` 或挂载点状态。建议：<br>1) 至少支持 `MS_REMOUNT` / `MS_BIND` 的基础语义；<br>2) 无法支持的 flag 返回 `EINVAL` 而非假成功。 | mount 系列 ≈ **180** |
| **fcntl FD 标志修复** | `F_GETFD`/`F_SETFD` 目前只查 `SOCKET_MANAGER`，普通文件的 `FD_CLOEXEC` 完全丢失。需在 `fd_table` 旁维护 `fd_flags` 数组，让非 socket 也能 get/set `O_NONBLOCK` / `O_APPEND` / `FD_CLOEXEC`。 | fcntl 基础 ≈ **30** |
| **dentry 锁加固** | 给 `GLOBAL_DCACHE` 的 insert/remove 加锁；`DentryInner::children` 已用 `Mutex`，但某些遍历路径可能绕过。 | 稳定性 |

**阶段 0 预期收益**：≈ **400+ 分**

---

### 阶段 1：快速补 syscall（低 hanging fruit）（P1，第 1~2 周）

**策略**：实现简单、独立、不依赖底层文件系统改动的系统调用。

| 系统调用 | 实现要点 | 涉及测例 | 分值 |
|---------|---------|---------|------|
| `close_range` | `close_range(first, last, flags)`：循环关闭 `[first, last]` 范围内的 fd；`CLOSE_RANGE_UNSHARE` 可先返回 `EINVAL`。 | close_range01(20), close_range02(11) | **31** |
| `sync_file_range` | 当前无脏页回刷，可先调用 `file.flush()` 或全局遍历 fd_table flush；未来接脏页回刷。 | sync_file_range02(12), sync_file_range01(5) | **17** |
| `fallocate` | **lwext4 无 fallocate API**，策略：<br>1. 对 tmpfs/devfs/procfs：通过 `vec.resize()` 预分配，支持 `FALLOC_FL_KEEP_SIZE`；<br>2. 对 ext4：返回 `EOPNOTSUPP`。<br>*注：LTP fallocate 测例通常在 tmpfs 或通用文件上运行，部分场景可过。* | fallocate06(27), fallocate04(12), fallocate05(17), fallocate03(8) | **64** |
| `copy_file_range` | 页缓存层做拷贝：读取 `in_fd` 的 `get_cache_frame` 页，写入 `out_fd`。可先做非零拷贝正确版本。 | copy_file_range02(28), copy_file_range01(20), copy_file_range03(2) | **50** |

**阶段 1 预期收益**：≈ **162 分**

---

### 阶段 2：xattr 扩展属性（P2，第 2~3 周）

**核心利好**：`lwext4_rust/src/bindings.rs` 中已有 `ext4_setxattr` / `ext4_getxattr` / `ext4_listxattr` / `ext4_removexattr`，**底层完全支持**，只需在 VFS + ext4 适配层打通。

#### 2.1 VFS 层扩展

```rust
// Inode trait 新增
fn setxattr(&self, name: &str, value: &[u8], flags: u32) -> SysResult<()> { Err(EOPNOTSUPP) }
fn getxattr(&self, name: &str, buf: &mut [u8]) -> SysResult<usize> { Err(EOPNOTSUPP) }
fn listxattr(&self, buf: &mut [u8]) -> SysResult<usize> { Err(EOPNOTSUPP) }
fn removexattr(&self, name: &str) -> SysResult<()> { Err(EOPNOTSUPP) }
```

#### 2.2 ext4 层实现
- 通过 `CString` 构造 path / name，调用 `lwext4_rust::bindings::ext4_setxattr` 等 FFI。
- 注意 `name_index`：lwext4 要求传入 `EXT4_XATTR_INDEX_USER` 等，对 `user.*` 前缀解析后传 `1`（USER）。

#### 2.3 syscall 层
- `setxattr` / `lsetxattr` / `fsetxattr`
- `getxattr` / `lgetxattr` / `fgetxattr`
- `listxattr` / `llistxattr` / `flistxattr`
- `removexattr` / `lremovexattr` / `fremovexattr`

#### 2.4 涉及测例与分值

| 测例 | 分值 |
|------|------|
| fsetxattr01 | 31 |
| setxattr01 | 31 |
| fgetxattr01 | 17 |
| fgetxattr02 | 13 |
| getxattr02 | 14 |
| fremovexattr02 | 11 |
| flistxattr02 | 12 |
| getxattr01 | 4 |
| getxattr03 | 3 |
| listxattr02 | 8 |
| llistxattr02 | 8 |
| lgetxattr02 | 3 |
| … | … |

**阶段 2 预期收益**：≈ **150+ 分**

---

### 阶段 3：inotify + fcntl 文件锁（P3，第 3 周）

#### 3.1 inotify

| syscall | 说明 |
|---------|------|
| `inotify_init1(flags)` | 创建匿名 fd，关联一个 `InotifyInstance` |
| `inotify_add_watch(fd, path, mask)` | 在 path 上注册监听事件，返回 watch descriptor |
| `inotify_rm_watch(fd, wd)` | 移除监听 |

**内核侧改动**：
- 新增 `InotifyManager` 全局实例（或每个 inotify fd 一个实例）。
- VFS 层在 `create` / `unlink` / `write` / `chmod` / `rename` 等路径末尾，检查父目录/文件是否被 inotify 监视，投递 `inotify_event` 到对应实例的环形缓冲区。
- `read(inotify_fd)` 时返回 `struct inotify_event` 数组。

#### 3.2 fcntl 文件锁（POSIX lock）

- 支持 `F_SETLK` / `F_GETLK` / `F_SETLKW`
- 在 `ProcessControlBlock` 或全局维护锁表：`Vec<PosixLock>`（每个元素包含 pid、fd、type、start、len）。
- `F_SETLKW` 若冲突则阻塞：可用简单轮询 `suspend_current_and_run_next()` 实现。
- 进程退出时自动释放该进程持有的所有锁。

**阶段 3 预期收益**：inotify ≈ **60** + fcntl 锁 ≈ **50** = **110 分**

---

### 阶段 4：splice / tee / vmsplice（P4，第 4 周，可选）

**分值极高但工作量极大**。建议采用**分层实现**策略：

1. **第一阶段（功能正确）**：不做零拷贝，直接走 `read` + `write` 路径。
   - `splice(fd_in, off_in, fd_out, off_out, len, flags)`：从 in 读 len 字节，写到 out。
   - `tee(fd_in, fd_out, len, flags)`：同上，但不消耗 in 的 offset（需内部缓存）。
   - `vmsplice(fd, iov, nr_segs, flags)`：将用户页数据写入管道。
   - 这样可解锁 splice 系列测例的 **基础分**。

2. **第二阶段（零拷贝优化）**：管道和文件页缓存之间直接传递 `Page` 引用，减少 memcpy。
   - 依赖当前页缓存和管道的底层重构，风险较高。

**阶段 4 预期收益**：splice 系列 ≈ **620 分**

---

### 阶段 5：新 mount API（P5，第 4~5 周，可选）

Linux 5.x 引入的新挂载 API，LTP 有大量测例覆盖。

| syscall | 作用 |
|---------|------|
| `fsopen(fstype, flags)` | 创建 fs context，返回 fd |
| `fsconfig(fd, cmd, key, value, aux)` | 配置 mount 参数（source、type 等）|
| `fsmount(fd, flags, ms_flags)` | 执行挂载，返回 mount fd |
| `fspick(dirfd, path, flags)` | 选择一个已有挂载点，返回 mount fd |
| `move_mount(from_dfd, from_path, to_dfd, to_path, flags)` | 移动挂载点 |
| `open_tree(dfd, path, flags)` | 复制或打开一个挂载树，返回 fd |
| `mount_setattr(dfd, path, flags, attr, size)` | 修改挂载属性 |

**实现建议**：
- 新增 `MountManager` 全局结构，管理 `fs_context`。
- 由于 Kairix 目前只支持 ext4/devfs/procfs/tmpfs 等少量文件系统，`fsopen` 可针对这些类型做有限支持。
- `fsmount` 最终调用现有的 `do_mount` 逻辑。

**阶段 5 预期收益**：mount API 系列 ≈ **400 分**

---

### 阶段 6：ioctl 扩展 + fanotify（P6，长期，低优先级）

| 功能 | 分值 | 难度评估 |
|------|------|---------|
| ioctl_ficlone / fiemap / ficlonerange | ≈ **670** | 高。需要为 ext4 新增 ioctl 处理路径 |
| fanotify 全系列 | ≈ **2200+** | 极高。需要完整的权限事件框架、路径回溯、mark 管理 |

**建议**：放到最后，若时间充裕再投入。fanotify 虽然分值诱人，但通常需要 Weeks 级别开发。

---

## 三、执行路线图（六周规划）

**总策略**：前两周主攻「修 Bug + 轻量 syscall」，快速拿分；中间两周做「xattr + inotify/文件锁」，完善 VFS 能力；最后两周冲击「splice + mount API + ioctl」，收割高分。

```
Week 1: 阶段 0 — 修 Bug，确保基础高分通过
  ├─ access01 调优（解锁 199 分）
  ├─ mount 桩语义修正（解锁 ~180 分）
  ├─ fcntl FD 标志修复（解锁 ~30 分）
  └─ dentry 锁加固（稳定性）
  本周目标：≈ 400 分

Week 2: 阶段 1 — 快速补齐独立 syscall
  ├─ close_range（解锁 31 分）
  ├─ sync_file_range（解锁 17 分）
  ├─ fallocate(tmpfs/devfs 支持，ext4 返回 EOPNOTSUPP)（解锁 64 分）
  └─ copy_file_range（解锁 50 分）
  本周目标：≈ 162 分

Week 3: 阶段 2 — xattr 扩展属性全打通
  ├─ VFS 层 Inode trait 扩展
  ├─ ext4 层 FFI 绑定（lwext4 已有底层支持）
  └─ syscall 层：setxattr / getxattr / listxattr / removexattr 及 l/f 变体
  本周目标：≈ 150+ 分

Week 4: 阶段 3 — inotify + fcntl 文件锁
  ├─ inotify_init1 / inotify_add_watch / inotify_rm_watch（解锁 ~60 分）
  └─ fcntl F_SETLK / F_GETLK / F_SETLKW（解锁 ~50 分）
  本周目标：≈ 110 分

Week 5: 阶段 4 — splice / tee / vmsplice + name_to_handle_at
  ├─ splice 功能正确版（非零拷贝，read+write 兜底）（解锁 ~620 分）
  ├─ tee / vmsplice 配套实现
  └─ name_to_handle_at / open_by_handle_at（解锁 ~52 分）
  本周目标：≈ 670 分

Week 6: 阶段 5/6 — 新 mount API + ioctl 扩展
  ├─ fsopen / fsconfig / fsmount / fspick / move_mount / open_tree / mount_setattr
  │   （解锁 ~400 分）
  └─ ioctl fiemap / ficlone / ficlonerange（视时间选做，解锁部分高分）
  本周目标：≈ 400+ 分
```

**六周累计预期**：保守 **~1900 分**，乐观 **~2600 分**。

---

## 四、关键代码提示

### 4.1 lwext4 xattr FFI 调用示例

```rust
use core::ffi::{c_char, c_void, c_int};
use alloc::ffi::CString;

let path = CString::new(abs_path).unwrap();
let name = CString::new("user.test").unwrap();
let ret = unsafe {
    lwext4_rust::bindings::ext4_setxattr(
        path.as_ptr(),
        name.as_ptr(),
        name.as_bytes().len(),
        value.as_ptr() as *const c_void,
        value.len(),
    )
};
```

> 注意：`name` 必须包含 `user.` 前缀，lwext4 内部会解析 `name_index`。

### 4.2 close_range 参考实现

```rust
pub fn sys_close_range(first: u32, last: u32, flags: u32) -> SyscallResult {
    const CLOSE_RANGE_UNSHARE: u32 = 1 << 1;
    const CLOSE_RANGE_CLOEXEC: u32 = 1 << 2;
    if flags & !(CLOSE_RANGE_UNSHARE | CLOSE_RANGE_CLOEXEC) != 0 {
        return Err(SysError::EINVAL);
    }
    let process = current_process();
    let mut inner = process.inner_exclusive_access();
    for fd in first..=last {
        if (fd as usize) < inner.fd_table.len() {
            if let Some(file) = inner.fd_table[fd as usize].take() {
                drop(inner);
                file.flush();
                inner = process.inner_exclusive_access();
            }
        }
    }
    Ok(0)
}
```

### 4.3 splice 简化版（非零拷贝）

```rust
pub fn sys_splice(fd_in: usize, off_in: usize, fd_out: usize, off_out: usize, len: usize, _flags: u32) -> SyscallResult {
    let mut buf = vec![0u8; len.min(65536)];
    // 先读 fd_in
    let n = sys_read(fd_in, buf.as_mut_ptr(), buf.len())?;
    // 再写 fd_out
    sys_write(fd_out, buf.as_ptr(), n)?
}
```

---

## 五、风险与应对

| 风险 | 影响 | 应对 |
|------|------|------|
| lwext4 xattr 对长 value / 大量 attr 有 bug | 阶段 2 部分测例失败 | 先用简单 name/value 跑通，复杂场景再调 |
| splice 非零拷贝版被 LTP 检测出非原子 / 性能不达标 | 阶段 4 部分测例失败 | 优先保证功能返回值正确；性能相关 subtest 可后续优化 |
| mount API 测例要求 namespace 支持 | 阶段 5 部分测例失败 | Kairix 无完整 namespace，对 `CLONE_NEWNS` 相关 subtest 预期跳过 |
| dentry 锁死锁问题在 xattr/splice 开发中暴露 | 开发阻塞 | 阶段 0 先加固锁策略，避免后续返工 |

---

## 六、预期总分

按阶段累计（保守估计）：

| 阶段 | 对应周次 | 保守分值 | 乐观分值 |
|------|---------|---------|---------|
| 阶段 0（修 Bug） | Week 1 | 400 | 500 |
| 阶段 1（轻量 syscall） | Week 2 | 160 | 180 |
| 阶段 2（xattr） | Week 3 | 150 | 200 |
| 阶段 3（inotify + 文件锁） | Week 4 | 110 | 150 |
| 阶段 4（splice + handle） | Week 5 | 670 | 700 |
| 阶段 5/6（mount API + ioctl） | Week 6 | 400 | 800 |
| **合计** | **6 周** | **~1890** | **~2530** |

> 注：全量 LTP 中 FS 相关总分约 **5000+**，六周计划已覆盖约 **38%~50%** 的高性价比区间。fanotify 全系列（≈ **2200+** 分）因工作量极大、通常需独立 Weeks 级别投入，未纳入六周常规排期；若比赛周期允许，可在 Week 6 之后作为「冲刺项」单独评估。
