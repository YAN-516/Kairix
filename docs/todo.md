# TODO
1. 调度策略的优化：已重构为 MLFQ-lite（4 级反馈队列、队内 RR、时间片降级、唤醒提升、aging 防饿死）
2. 模块化与冗余代码整理：详见 [kernel-modularization-refactor-plan.md](kernel-modularization-refactor-plan.md)
3. 上板子
4. 决赛第一阶段测试用例
5. 网络部分
6. 接入真实的文件系统
7. GUI
8. 贪吃蛇

待做：
CPU负载平衡
kstack cache（待考虑）
队列要放到队首，不然创建3000个线程要等很久才有输出


cyclic
优化热路径 lmbench iozone libcbench
优化上下文切换的代价
现在的文件全部都是靠硬编码的，创建功能还是有点不足
不同CPU的时钟不一样

考虑异步文件系统，或者ext4换锁，现在是一把全局大锁，测试8核



推荐实现路线
第一阶段：先达到大部分缓存并行收益（已完成并验证）
将全局 PAGE_CACHE 拆成 64 个分片。
按 (mount_id, inode, page_id) 选择分片。
inode registry 同样分片。
页缓存命中只获取 cache shard 和 page lock。
缓存缺失才进入 lwext4 mount gate。
这一阶段不改变磁盘格式和 lwext4 正确性，风险最低。对 rustc 的源码、rlib、元数据重复读取会很有帮助。
第二阶段：实现“并行读、串行写”（已完成并验证）
将 mount gate 改为 mount RwLock。
给 lwext4 block cache 增加内部短临界区锁。
不同文件的 read/stat/open-existing 走 mount 读锁。
create、unlink、rename、truncate、writeback 和 journal 走写锁。
同一个打开文件的 fpos/fsize 继续由现有 Ext4File.ext4file 锁保护。
不能直接把当前 mount gate 换成 RwLock，因为 lwext4 的读路径也会修改 LRU、引用计数和 RB tree。
第三阶段：目录、inode、块组三级锁（已实现，待运行验证）
建议固定锁顺序：
mount 生命周期锁
  → journal/事务锁
  → block-group 锁（按 bgid 排序）
  → inode/目录锁（按 inode 号排序）
  → block-cache shard
  → page lock
其中：
普通文件读只需要 inode 读锁。
同一目录创建、删除和查找由目录 inode 锁协调。
rename 同时锁源目录和目标目录，按 inode 编号排序，避免 ABBA 死锁。
分配 inode/block 时只锁涉及的块组。
journal 提交仍可以串行，数据准备可以并行。


“多请求块设备”

第四阶段：纯 Rust 原生 ext4
要完全达到图片中的“原生 ext4”，需要逐步替换：
superblock 和 block-group descriptor
inode/extent tree
bitmap allocator
目录项和 HTree
hardlink、symlink、rename、truncate
xattr
journal replay、transaction、checkpoint
fsync/sync 和崩溃一致性







