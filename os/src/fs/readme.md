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
修复bug dentry缓存，添加文件

./busybox mkdir -p /var/tmp
./busybox touch /var/tmp/lmbench
cp hello /tmp
./lmbench_all lat_pagefault -P 1 /var/tmp/XXX
# ai
glibc和musl的iozone都大概33分，关键在于反向读和预读取
lmbench 还有优化空间
# 待讲
1.每周的进度表
2.时间线
3.注释采用中文
4.改队友的代码采用注释源代码的方法,方便知道发生了什么修改

File /var/tmp/XXX write bandwidth:455 KB/sec
[ERROR] sys_rt_sigreturn: using saved_mask=0x0
[ERROR] [DEBUG waitpid] parent_pid=3 found zombie child pid=3651 exit_code=0
[ERROR] fork a new process with pid 3652, parent pid = 3
[ERROR] sys_sigaction: signum=3, act=0x3ffffed6e0, oldact=0x0
[ERROR] [sys_execve] path=./lmbench_all cwd_name=glibc
[ERROR] Executing program: ./lmbench_all
[ERROR] [MEMDEBUG] heap: user=19694049 actual=26124528 total=67108864 free=40984336
[ERROR] [MEMDEBUG] frames: alloc=135266 free=53271 delta=81995 | memory: free=3888357376 total=4294967296
[ERROR] [MEMDEBUG] heap: user=19694049 actual=26124528 total=67108864 free=40984336
[ERROR] [MEMDEBUG] frames: alloc=135266 free=53271 delta=81995 | memory: free=3888357376 total=4294967296
[ERROR] sys_sigaction: signum=15, act=0x3ffffecf68, oldact=0x3ffffecff8
[ERROR] sys_sigaction: signum=17, act=0x3ffffecf68, oldact=0x3ffffecff8
[ERROR] fork a new process with pid 3653, parent pid = 3652
[ERROR] sys_sigaction: signum=17, act=0x3ffffecf38, oldact=0x3ffffecfc8
[ERROR] sys_sigaction: signum=15, act=0x3ffffecf38, oldact=0x3ffffecfc8
[ERROR] sys_sigaction: signum=17, act=0x3ffffecd58, oldact=0x3ffffecde8
[ERROR] sys_sigaction: signum=17, act=0x3ffffecf68, oldact=0x3ffffecff8
[ERROR] sys_sigaction: signum=14, act=0x3ffffecf68, oldact=0x3ffffecff8
[ERROR] sys_setitimer: pid = 3652, which=0, new_value=0x3ffffed1d8, old_value=0x3ffffed1b8
[ERROR] sys_setitimer: pid = 3652, which=0, new_value=0x3ffffed1d8, old_value=0x3ffffed1b8
[ERROR] sys_sigaction: signum=14, act=0x3ffffecf68, oldact=0x3ffffecff8
double free or corruption (!prev)
[ERROR] sys_tgkill: tgid=3652, tid=0, sig=6
[ERROR] sys_sigaction: signum=6, act=0x3ffffecda8, oldact=0x0
[ERROR] sys_tgkill: tgid=3652, tid=0, sig=6
[ERROR] sys_rt_sigreturn: using saved_mask=0x0
[ERROR] [DEBUG waitpid] parent_pid=3 found zombie child pid=3652 exit_code=127
[ERROR] fork a new process with pid 3654, parent pid = 3
[ERROR] sys_sigaction: signum=3, act=0x3ffffed6e0, oldact=0x0
[ERROR] [sys_execve] path=./lmbench_all cwd_name=glibc
[ERROR] Executing program: ./lmbench_all
[ERROR] [MEMDEBUG] heap: user=19704001 actual=26137616 total=67108864 free=40971248
[ERROR] [MEMDEBUG] frames: alloc=135648 free=53609 delta=82039 | memory: free=3888177152 total=4294967296
[ERROR] [MEMDEBUG] heap: user=19704001 actual=26137616 total=67108864 free=40971248
[ERROR] [MEMDEBUG] frames: alloc=135648 free=53609 delta=82039 | memory: free=3888177152 total=4294967296
[ERROR] [MEMDEBUG] heap: user=19704001 actual=26137616 total=67108864 free=40971248
[ERROR] [MEMDEBUG] frames: alloc=135648 free=53609 delta=82039 | memory: free=3888177152 total=4294967296
[ERROR] [MEMDEBUG] heap: user=19704001 actual=26137616 total=67108864 free=40971248
[ERROR] [MEMDEBUG] frames: alloc=135648 free=53609 delta=82039 | memory: free=3888177152 total=4294967296
[ERROR] [MEMDEBUG] heap: user=19704001 actual=26137616 total=67108864 free=40971248
[ERROR] [MEMDEBUG] frames: alloc=135648 free=53609 delta=82039 | memory: free=3888177152 total=4294967296
[ERROR] [MEMDEBUG] heap: user=19704001 actual=26137616 total=67108864 free=40971248
[ERROR] [MEMDEBUG] frames: alloc=135648 free=53609 delta=82039 | memory: free=3888177152 total=4294967296
[ERROR] sys_sigaction: signum=15, act=0x3ffffed7d8, oldact=0x3ffffed868
[ERROR] sys_sigaction: signum=17, act=0x3ffffed7d8, oldact=0x3ffffed868
[ERROR] fork a new process with pid 3655, parent pid = 3654
[ERROR] sys_sigaction: signum=17, act=0x3ffffed7a8, oldact=0x3ffffed838
[ERROR] sys_sigaction: signum=15, act=0x3ffffed7a8, oldact=0x3ffffed838
[ERROR] [MEMDEBUG] heap: user=19984310 actual=26686064 total=67108864 free=40422800
[ERROR] [MEMDEBUG] frames: alloc=135680 free=53610 delta=82070 | memory: free=3888050176 total=4294967296
[ERROR] [MEMDEBUG] heap: user=19984310 actual=26686064 total=67108864 free=40422800
[ERROR] [MEMDEBUG] frames: alloc=135680 free=53610 delta=82070 | memory: free=3888050176 total=4294967296
[ERROR] [MEMDEBUG] heap: user=19984310 actual=26686064 total=67108864 free=40422800
[ERROR] [MEMDEBUG] frames: alloc=135680 free=53610 delta=82070 | memory: free=3888050176 total=4294967296
[ERROR] sys_sigaction: signum=17, act=0x3ffffed5c8, oldact=0x3ffffed658
[ERROR] sys_sigaction: signum=17, act=0x3ffffed7d8, oldact=0x3ffffed868
[ERROR] sys_sigaction: signum=14, act=0x3ffffed7d8, oldact=0x3ffffed868
[ERROR] sys_setitimer: pid = 3654, which=0, new_value=0x3ffffeda48, old_value=0x3ffffeda28
[ERROR] [DEBUG waitpid] parent_pid=3654 found zombie child pid=3655 exit_code=0
[ERROR] sys_setitimer: pid = 3654, which=0, new_value=0x3ffffeda48, old_value=0x3ffffeda28
[ERROR] sys_sigaction: signum=14, act=0x3ffffed7d8, oldact=0x3ffffed868
0.524288 183