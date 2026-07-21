# lwext4 锁分层与后续拆锁边界

## 当前锁层次

当前内核不再使用一把全局锁串行化全部 ext4 数据路径：

1. `LWEXT4_LOCK` 只负责 mount/unmount 和兼容路径。生命周期操作会按 `mount_id` 顺序冻结所有已注册 mount gate，防止 lwext4 全局 `s_mp`/`s_bdevices` 表与普通路径查找并发修改。
2. 每个 ext4 mount 有独立的 `Lwext4MountGate`。文件数据、目录、元数据、xattr 和 stat 操作只锁所属 mount，不同 ext4 分区可以并行。
3. `Ext4File.ext4file` 继续保护单个 lwext4 文件描述符的 `fpos`、`fsize` 等状态，锁顺序固定为 mount gate -> `ext4file`。
4. 页缓存锁不允许在持有 `PAGE_CACHE` 时等待；ext4 写回也不会持有 mount gate 等待 page lock。页忙时先退出短事务，再重试或留给下一批写回。

跨 mount 的 rename/link 在进入 lwext4 前返回 `EXDEV`。

## 四阶段并发改造状态

### 阶段一：Rust 页缓存与 inode registry 分片（已实现，待运行验证）

- `PAGE_CACHE` 已从单个 `SleepLock<PageCache>` 改为 64 个独立 shard；
- shard 按 inode 选择，同一 inode 的所有页面保留在同一 shard，使 truncate、unlink、O_TRUNC 的整 inode 失效仍保持原子；
- 不同 inode 的查找、插入、LRU 更新和删除可以真正并行；
- 页面重复加载使用 `insert_page_if_absent()` 原子发布，竞争加载者复用同一个 `Arc<RwLock<Page>>`；
- inode writeback snapshot 只获取所属 shard，不再冻结整个系统的页缓存；
- ext4 inode 共享实例表也拆为 64 个 shard，同号 inode 仍复用同一个 `Ext4InodeSharedState`；
- reclaim 轮询所有 shard，只使用 `try_lock()`，不会因某一个繁忙文件阻塞全部回收。

阶段一没有放宽 lwext4 C 层的正确性边界。缓存 miss、metadata 和 writeback 仍经过 mount gate。

### 阶段二：并行读、串行写（未放行）

在给 `ext4_bcache` 的 LBA tree、LRU、dirty list 和引用计数增加并发保护前，不能直接把 mount gate 换成读写锁。完成后：

- read、stat、已有文件 open 和目录 lookup 使用 mount read gate；
- create、unlink、rename、truncate、writeback、xattr 和 journal 使用 write gate；
- `Ext4File.ext4file` 继续串行单个 open-file description 的 `fpos/fsize`。

### 阶段三：目录、inode、块组三级锁（未放行）

固定锁顺序为：

```text
mount 生命周期锁
  -> journal/transaction 锁
  -> block-group 锁（按 bgid 排序）
  -> inode/目录锁（按 inode 号排序）
  -> block-cache shard
  -> page lock
```

rename/link 涉及多个目录或 inode 时必须按编号排序。底层 I/O 应使用 buffer pin + drop-lock + I/O + revalidate，避免拿着高级元数据锁等待设备。

### 阶段四：原生 Rust ext4 与多请求块设备（设计边界）

纯 Rust 路径需要逐步替换 superblock/block-group、inode/extent、bitmap allocator、目录/HTree、xattr 和 journal。任何未实现格式或 feature 必须安全回退到现有 lwext4 路径，不能静默弱化 fsync、rename、truncate 或崩溃一致性。

VirtIO 块设备还需从“一把设备锁 + 一个 bounce buffer + 单 in-flight token”升级为请求槽池、独立 DMA buffer 和多个 in-flight token；否则上层拆锁后仍受单请求队列限制。

## 统计信息

`lwext4_lock_stats()`（也会出现在 `/proc/kairix_perf` 和 stall 快照中）包含：

- 总调用、获取、递归进入和竞争次数；
- 总/最长等待时间；
- 总/最长持锁时间；
- 当前和最近操作类型；
- 按 `Other`、`Mount`、`Metadata`、`OpenClose`、`Read`、`Write`、`Seek`、`Truncate`、`Writeback`、`Directory`、`Xattr`、`Stat` 分类的统计；
- 每个 mount 的路径、owner、当前操作、等待者和累计时间。

## 为什么尚不能直接删除同一 mount 的 gate

lwext4 C 层目前还有三个共享状态域，不具备无锁并发条件：

- `ext4_fs.curr_trans` 是每个 filesystem 唯一的当前 journal transaction；
- `ext4_bcache` 的 buffer refcount、LBA/LRU tree、dirty list 和 cache writeback 状态不是原子或线程安全结构；
- inode/extent 修改还会共同更新 block/inode bitmap、block-group descriptor 和 superblock 计数器。

因此只增加 per-inode 锁并删除 mount gate 会在 journal、allocator 或 block cache 中产生数据竞争。C 层应按以下顺序继续拆分：

1. 将 `curr_trans` 改成显式 transaction context，先建立每 mount metadata/journal 锁；
2. 给 inode/extent 操作引入按 `(mount, inode)` 排序的锁，rename/link 同时锁多个 inode 时严格按 inode 号排序；
3. 将 block cache 按 LBA 分片，每个分片保护 refcount、lookup 和 LRU，另设短时 dirty/writeback 队列锁；
4. allocator 使用 per-block-group 锁，superblock 汇总计数使用原子更新或独立短锁；
5. 完成故障注入、fsync/rename/truncate/崩溃恢复测试后，才逐类移除外层 mount gate。

在这些条件满足前，mount gate 是同一 ext4 实例的正确性边界，不应通过关闭锁来换取局部性能。
