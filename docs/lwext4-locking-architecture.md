# lwext4 锁分层与后续拆锁边界

## 当前锁层次

当前内核不再使用一把全局锁串行化全部 ext4 数据路径：

1. `LWEXT4_LOCK` 只负责 mount/unmount 和兼容路径。生命周期操作会按 `mount_id` 顺序冻结所有已注册 mount gate，防止 lwext4 全局 `s_mp`/`s_bdevices` 表与普通路径查找并发修改。
2. 每个 ext4 mount 有独立的 `Lwext4MountGate`。文件数据、目录、元数据、xattr 和 stat 操作只锁所属 mount，不同 ext4 分区可以并行。
3. `Ext4File.ext4file` 继续保护单个 lwext4 文件描述符的 `fpos`、`fsize` 等状态，锁顺序固定为 mount gate -> `ext4file`。
4. 页缓存锁不允许在持有 `PAGE_CACHE` 时等待；ext4 写回也不会持有 mount gate 等待 page lock。页忙时先退出短事务，再重试或留给下一批写回。

跨 mount 的 rename/link 在进入 lwext4 前返回 `EXDEV`。

## 四阶段并发改造状态

### 阶段一：Rust 页缓存与 inode registry 分片（已完成并通过运行验证）

- `PAGE_CACHE` 已从单个 `SleepLock<PageCache>` 改为 64 个独立 shard；
- shard 按 inode 选择，同一 inode 的所有页面保留在同一 shard，使 truncate、unlink、O_TRUNC 的整 inode 失效仍保持原子；
- 不同 inode 的查找、插入、LRU 更新和删除可以真正并行；
- 页面重复加载使用 `insert_page_if_absent()` 原子发布，竞争加载者复用同一个 `Arc<RwLock<Page>>`；
- inode writeback snapshot 只获取所属 shard，不再冻结整个系统的页缓存；
- ext4 inode 共享实例表也拆为 64 个 shard，同号 inode 仍复用同一个 `Ext4InodeSharedState`；
- reclaim 轮询所有 shard，只使用 `try_lock()`，不会因某一个繁忙文件阻塞全部回收。

阶段一没有放宽 lwext4 C 层的正确性边界。缓存 miss、metadata 和 writeback 仍经过 mount gate。

### 阶段二：并行读、串行写（已完成并通过运行验证）

- 每个 mount gate 已改为 writer-priority 读写门控；`read`、`stat`、目录遍历和已有文件 open/close 使用共享门，写操作等待期间停止接纳新的非递归 reader；
- `/proc/kairix_perf` 的 mount gate 状态包含 `active_readers`、`max_active_readers`、`writer_active` 和 `waiting_writers`，测试结束后仍可用峰值确认是否发生同 mount 并行读；
- create、unlink、rename、truncate、writeback、xattr 和 journal 继续使用 mount 独占门，尚未提前放行任何元数据修改；
- `ext4_bcache` 的 LBA/LRU tree、dirty list、refcount 和容量统计由短时 state lock 保护，内存分配和物理 I/O 不在该短临界区内执行；
- 同一 LBA 并发 miss 使用 `BC_LOADING` 协调，只允许一个 CPU 填充缓存块，其他 reader 保持引用并协作让出 CPU；
- cache shake 会先 pin dirty buffer，再释放 state lock 执行 I/O，避免拿着 bcache 短锁等待块设备；
- lwext4 块设备回调已从共享 cursor 的 `seek + read/write` 改为绝对偏移 `read_at/write_at`，并行请求不会互相覆盖设备位置；
- 非创建、非截断的 `ext4_fopen2()` 不再切换/刷新 writeback 状态，因此已有文件 open 是纯只读操作；
- `Ext4File.ext4file` 继续串行单个 open-file description 的 `fpos/fsize`。

这一阶段仍未让 journal、allocator 或 inode 修改并行。底层 VirtIO 当前仍可能串行提交物理请求；真正的多请求设备队列属于第四阶段。

### 阶段三：目录、inode、块组三级锁（锁域已实现，写并行暂缓放行）

每个 `ext4_fs` 现在拥有独立的并发状态：

- journal/transaction 使用可重入的 per-filesystem 锁，从 `ext4_trans_start()` 持有到 commit/abort；不同 mount 的事务不再互相阻塞；
- truncate 分批提交使用不释放 journal 锁的 checkpoint，禁止在仍持有 inode 引用时把 journal 所有权交给其他任务；
- inode 使用 256 个读写锁分片，普通文件读取、stat、只读路径解析、xattr 读取和目录遍历获取共享锁，修改 inode/extent 的路径获取独占锁；
- block group 使用 64 个独占锁分片，锁覆盖 group descriptor、inode/block bitmap 和引用生命周期；
- block-cache writeback 嵌套计数在 cache state lock 下更新，防止并发写者丢失增减操作并提前触发全局 dirty flush；
- journal inode 的长期引用不固定占用普通 inode shard；
- mount gate 继续允许 read、seek、只读目录遍历和 stat 并行；metadata、write、truncate、xattr 和 writeback 暂时保留在独占侧；
- `/proc/kairix_perf` 的每个 mount 增加 `stage3` 统计，可观察 journal/inode/block-group 的获取、竞争和最大并行度。

当前安全锁层次为：

```text
mount 生命周期锁
  -> journal/transaction 锁
  -> inode/目录锁
  -> block-group 锁
  -> block-cache shard
  -> page lock
```

lwext4 目前仍只有一个 `curr_trans`，因此 rename/link/create/unlink 等多 inode 修改先由最外层 journal 锁串行化，不会出现两个 namespace writer 以相反顺序等待 inode 的 ABBA 环。普通路径遍历在没有 `FILETYPE` feature 时会先释放父 inode，再读取子 inode，避免 reader 在持有一个 inode shard 时等待另一个 shard。将来若把 `curr_trans` 拆成多个并行 transaction context，必须先为 rename/link 建立按 inode 号排序的显式预锁集合。

底层 I/O 继续使用 buffer pin + drop-lock + I/O + revalidate，不拿 bcache 状态锁等待设备。阶段三暂不并行 journal commit；页缓存数据准备和无关 inode 的只读访问可以并行，事务提交与 allocator 元数据更新保持正确串行。

当前 lwext4 的 JBD transaction queue、`ext4_buf.end_write_arg` 与 cache eviction callback 仍共享对象生命周期。仅用 journal/inode/block-group 锁不足以允许修改操作和 cache eviction 并发：commit 释放 `jbd_buf` 时，另一个 CPU 的回写回调可能仍在访问同一对象。因而修改类操作必须继续由 mount gate 独占，直到 JBD callback 状态获得独立同步并将唯一 `curr_trans` 拆为显式 transaction context；否则会表现为 allocator header 损坏、重复释放或 use-after-free。

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

## 为什么仍保留 mount gate

阶段三锁域加入后，只读数据和元数据查询持有 mount gate 的共享侧；修改操作仍持有独占侧。gate 当前有三个不能删除的职责：

- mount/unmount 必须冻结该实例的全部调用，防止释放 `ext4_fs.concurrency`、bcache 和块设备时仍有活动引用；
- 全 mount cache flush/writeback 需要稳定遍历 dirty 状态，当前仍使用独占 gate，不与普通数据路径交错。
- JBD transaction、buffer completion callback 与 cache eviction 尚未形成独立且完整的并发生命周期域，修改操作必须避免与这些回调交错。

后续继续提高写并行度时，应按以下顺序推进：

1. 将唯一 `curr_trans` 改成显式 transaction context，只在 journal 分配、commit 和 checkpoint 时持有短 journal 锁；
2. rename/link 在进入多个 inode 前建立按 inode 号排序的预锁集合；
3. 将 block cache 的单 state lock 进一步按 LBA 分片，并为 dirty/writeback 队列保留独立短锁；
4. 将 superblock 汇总计数改成原子更新或独立短锁，使不同 block group 的 allocator 真正并行；
5. 完成故障注入、fsync/rename/truncate 和崩溃恢复测试后，再考虑缩小全 mount flush 的独占区间。

mount gate 现在是生命周期与全局 flush 边界，不再是普通 ext4 读写的大锁。
