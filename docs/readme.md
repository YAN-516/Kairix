注：
思考架构：
延迟写
镜像同步
# 待做：
多用户组
# 注意事项；
暂时没有写页面置换算法，可能使用LRU？
没实现fixed map
# 待做
看评分机制

信号和多线程之间的关系还是有问题
dentry锁还存在问题
感觉页缓存还存在问题，查找文件很慢,LRU可以考虑优化，暂时不支持脏页回刷
dentry缓存还可以优化

堆的碎片处理机制
懒分配现在还是一整个区域的，不是一页一页
栈的自动扩大可能还有问题
不确定堆和内存和栈是否还有泄漏问题

整理makefile
修改 syscall/signal.rs 中的 handle_signals，让它在没有 sa_restorer 时，使用一个更安全的 restorer 机制（而不是放在栈上）


考虑如何简化到ltp的路径
加入Test timeouted, sending SIGKILL!机制，防止有些测试用例花费时间太久
mkfs.ext2和工具包的区别
# commit
使用mkfs.ext3和mkfs.ext4桩绕过ltp的检查机制
# ai

glibc和musl的iozone都大概33分，关键在于反向读和预读取
lmbench 还有优化空间
# 待讲
1.每周的进度表
2.时间线
3.注释采用中文
4.改队友的代码采用注释源代码的方法,方便知道发生了什么修改

