注：

dentry 部分暂时没有加锁


思考架构：
延迟写
镜像同步

# 待做：
开始完善系统调用,准备运行busybox

过五个系统调用
futex_wake
明天再过五个
# 注意事项；
要考虑锁的问题了，该找个时候统一一下锁，现在的锁太乱了
暂时没有写页面置换算法，可能使用LRU？
没实现fixed map
# commit
实现sys_set_tid_address，修改sys_exit
重构from_elf和execve,原本的from_elf存在bug-非对齐 ELF 段加载偏移(本质上是copydata的问题，之前是无脑从0offset开始)
加入sys_getuid，默认root
# ai

# 待讲
1.每周的进度表
2.时间线
3.注释采用中文
4.改队友的代码采用注释源代码的方法,方便知道发生了什么修改