# lwext4 锁分层与后续拆锁边界

## 当前锁层次

当前内核不再使用一把全局锁串行化全部 ext4 数据路径：

1. `LWEXT4_LOCK` 只负责 mount/unmount 和兼容路径。生命周期操作会按 `mount_id` 顺序冻结所有已注册 mount gate，防止 lwext4 全局 `s_mp`/`s_bdevices` 表与普通路径查找并发修改。
2. 每个 ext4 mount 有独立的 `Lwext4MountGate`。文件数据、目录、元数据、xattr 和 stat 操作只锁所属 mount，不同 ext4 分区可以并行。
3. `Ext4File.ext4file` 继续保护单个 lwext4 文件描述符的 `fpos`、`fsize` 等状态，锁顺序固定为 mount gate -> `ext4file`。
4. 页缓存锁不允许在持有 `PAGE_CACHE` 时等待；ext4 写回也不会持有 mount gate 等待 page lock。页忙时先退出短事务，再重试或留给下一批写回。

跨 mount 的 rename/link 在进入 lwext4 前返回 `EXDEV`。

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
