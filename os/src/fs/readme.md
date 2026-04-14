注：

dentry 部分暂时没有加锁


思考架构：
延迟写
镜像同步

# 待做：
/dev/urandom

完善busybox的系统调用
信号机制
多用户组
flush 里面的size


dev,fat32,procfs
# 注意事项；
要考虑锁的问题了，该找个时候统一一下锁，现在的锁太乱了
暂时没有写页面置换算法，可能使用LRU？
没实现fixed map


# commit
加入/dev/tty,修改open_file的逻辑,放到vfs层里面
修改read_all,使其符合页缓存的机制
修改find_dentry
使用tty替换Stdin和Stdout
修复ls卡死的bug
# ai

# 待讲
1.每周的进度表
2.时间线
3.注释采用中文
4.改队友的代码采用注释源代码的方法,方便知道发生了什么修改