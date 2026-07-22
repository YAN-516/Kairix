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

### 阶段三：显式 transaction context 与元数据锁域（已实现，待运行验证）

每个 `ext4_fs` 现在拥有独立的并发状态：

- transaction 从唯一 `curr_trans` 改为按稳定 task owner 索引的显式 context，支持嵌套事务；不同任务可以并行准备各自的 transaction；
- journal 短锁只保护 commit/checkpoint、checkpoint queue、block-record tree 和 `ext4_buf.end_write_arg` callback 生命周期，不再跨 inode/block-group 操作长期持有；
- truncate checkpoint 在同一 owner context 中原子替换 transaction，不向其他任务暴露已经 commit/free 的旧指针；
- inode 使用 256 个读写锁分片，分片键按 inode-table 物理块计算；这既允许无关 inode 并行，也防止不同 inode 位于同一缓存块时发生并发覆盖；
- block group 使用 64 个独占锁分片，分片键按 group-descriptor 物理块计算，锁覆盖共享 descriptor block、inode/block bitmap 和引用生命周期；
- packed superblock 汇总计数使用独立短锁保护 read-modify-write，避免不同 block group 并行分配时丢失更新；
- block-cache writeback 嵌套计数和 0→flush 转换受 cache state/JBD 生命周期锁共同保护，新 writer 不能进入正在结束的全局 dirty flush；
- journal inode 的长期引用不固定占用普通 inode shard；
- mount gate 对 metadata、read、write、seek、truncate、单文件 writeback、directory、xattr 和 stat 使用共享生命周期门；mount/unmount 与全 mount flush 仍使用独占门；
- rename/link/create/unlink 等多 inode namespace 修改由独立的 per-mount fair serializer 排序，不再阻塞无关文件数据写；在建立多 inode 排序预锁前不并行两个 namespace writer；
- `/proc/kairix_perf` 的每个 mount 增加 `stage3` 统计，可观察 active/max transaction、journal/inode/block-group 的获取、竞争和最大并行度。

当前安全锁层次为：

```text
mount 生命周期锁
  -> namespace serializer（仅目录项修改）
  -> inode/目录锁
  -> block-group 锁
  -> superblock 汇总计数短锁
  -> JBD 对象生命周期短锁
  -> block-cache state lock
  -> page lock
```

rename/link/create/unlink 等多 inode 修改先由 namespace serializer 排序，不会出现两个 namespace writer 以相反顺序等待 inode 的 ABBA 环。普通路径遍历在没有 `FILETYPE` feature 时会先释放父 inode，再读取子 inode，避免 reader 在持有一个 inode shard 时等待另一个 shard。未来若要并行 namespace writer，必须先为 rename/link 建立按 inode-table block、inode 号排序的显式预锁集合。

底层 I/O 继续使用 buffer pin + drop-lock + I/O + revalidate，不拿 bcache state lock 等待设备。不同 transaction 的数据准备、inode/extent 修改和 block-group 分配可以并行；journal commit 仍由短 JBD 生命周期锁串行，以保持日志空间、checkpoint queue 和崩溃恢复顺序。

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

阶段三完成后，普通读写和元数据调用持有 mount gate 的共享侧。gate 仍有两个不能删除的职责：

- mount/unmount 必须冻结该实例的全部调用，防止释放 `ext4_fs.concurrency`、bcache 和块设备时仍有活动引用；
- 全 mount cache flush/writeback 需要稳定遍历 dirty 状态，当前仍使用独占 gate，不与普通数据路径交错。

后续继续提高写并行度时，应按以下顺序推进：

1. rename/link 在进入多个 inode 前建立按 inode-table block、inode 号排序的预锁集合，替换 namespace serializer；
2. 将 block cache 的单 state lock 进一步按 LBA 分片，并为 dirty/writeback 队列保留独立短锁；
3. 将同步 journal commit 改为有序提交队列，在保持 transaction ID 顺序的前提下与调用任务解耦；
4. 完成故障注入、fsync/rename/truncate 和崩溃恢复测试后，再考虑缩小全 mount flush 的独占区间。

mount gate 现在是生命周期与全局 flush 边界，不再是普通 ext4 读写的大锁。
