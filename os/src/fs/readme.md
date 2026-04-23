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

# commit
glibc有问题
修复clone的问题
# ai
内核在 fork 复制阶段，主动去碰了那些还没分配的 lazy 页面，结果被 translate() 的 Some(PTE(0)) 骗进了一个未映射的内核地址，触发无限 page fault 循环。translate() 不检查 Valid 位
# 待讲
1.每周的进度表
2.时间线
3.注释采用中文
4.改队友的代码采用注释源代码的方法,方便知道发生了什么修改