注：

dentry 部分暂时没有加锁


思考架构：
延迟写
镜像同步

# 待做：
/dev/urandom

完善busybox的系统调用
多用户组
flush 里面的size
/etc/localtime
软连接，可能需要修改底层ext4
dev,fat32,procfs
# 注意事项；
要考虑锁的问题了，该找个时候统一一下锁，现在的锁太乱了
暂时没有写页面置换算法，可能使用LRU？
没实现fixed map



# 待做
软连接
信号和多线程之间的关系还是有问题
锁

dentry锁还存在问题
感觉页缓存还存在问题，查找文件很慢
# commit
`context_switch` 在切换任务时错误地保存和恢复了 `tp`（thread pointer）寄存器。`tp` 是 __per-CPU 标识符__，用于标识当前代码运行在哪个 CPU 上，不应该被任务上下文携带。


实现shm
# ai
translated_byte_buffer
# 待讲
1.每周的进度表
2.时间线
3.注释采用中文
4.改队友的代码采用注释源代码的方法,方便知道发生了什么修改

