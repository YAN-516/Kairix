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

信号和多线程之间的关系还是有问题
dentry锁还存在问题
感觉页缓存还存在问题，查找文件很慢,LRU可以考虑优化，暂时不支持脏页回刷
dentry缓存还可以优化

堆的碎片处理机制
懒分配现在还是一整个区域的，不是一页一页
栈的自动扩大可能还有问题
不确定堆和内存和栈是否还有泄漏问题
./lmbench_all lat_proc -P 1 fork

整理makefile
# commit

# ai
glibc和musl的iozone都大概33分，关键在于反向读和预读取
lmbench 还有优化空间
# 待讲
1.每周的进度表
2.时间线
3.注释采用中文
4.改队友的代码采用注释源代码的方法,方便知道发生了什么修改

/glibc # busybox sh lmbench_testcode.sh
#### OS COMP TEST GROUP START lmbench-glibc ####
latency measurements
Simple syscall: 13.2970 microseconds
Simple read: 21.3136 microseconds
Simple write: 19.2308 microseconds
Simple stat: 320.4785 microseconds
Simple fstat: 22.3438 microseconds
Simple open/close: 336.3787 microseconds
Select on 100 fd's: 78.8880 microseconds
Signal handler installation: 18.4603 microseconds
Signal handler overhead: 83.1148 microseconds
make[1]: Leaving directory '/workspace/os'