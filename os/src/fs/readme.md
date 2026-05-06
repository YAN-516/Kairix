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
堆的碎片处理机制
懒分配现在还是一整个区域的，不是一页一页
整理makefile
栈的自动扩大可能还有问题
不确定堆和内存和栈是否还有泄漏问题
# commit
修复bug
修复因为之前代码修改而加入的竞争死锁bug，优化pipe
# ai
glibc和musl的iozone都大概33分，关键在于反向读和预读取

# 待讲
1.每周的进度表
2.时间线
3.注释采用中文
4.改队友的代码采用注释源代码的方法,方便知道发生了什么修改


./lmbench_all bw_pipe -P 1



musl # ./lmbench_all bw_pipe -P 1
[ERROR] sys_sigaction: signum=28, act=0x3ffffed740, oldact=0x0
[ WARN] area vpn 0x10..0x162
[ WARN] area vpn 0x162..0x166
[ WARN] area vpn 0x166..0x16c
[ WARN] area vpn 0x3ffffde..0x3ffffee
[ WARN] alloc tid: 0
[ WARN] success
[ WARN] fork a new process with pid 3, parent pid = 2
[ERROR] fork a new process with pid 3, parent pid = 2
[ERROR] sys_sigaction: signum=20, act=0x3ffffed7b0, oldact=0x0
[ERROR] sys_sigaction: signum=22, act=0x3ffffed7b0, oldact=0x0
[ERROR] sys_sigaction: signum=2, act=0x3ffffed7b0, oldact=0x0
[ERROR] sys_sigaction: signum=15, act=0x3ffffed7b0, oldact=0x0
[ERROR] sys_sigaction: signum=3, act=0x3ffffed7b0, oldact=0x0
[ERROR] [sys_execve] path=./lmbench_all cwd_name=musl
[ERROR] Executing program: ./lmbench_all
[ WARN] [MEMDEBUG] UserMapArea dropped with 338 remaining frames, type=Elf, range=0x10000..0x161fac
[ WARN] [MEMDEBUG] UserMapArea dropped with 4 remaining frames, type=Elf, range=0x162ff0..0x165380
[ WARN] [MEMDEBUG] UserMapArea dropped with 6 remaining frames, type=Heap, range=0x166000..0x16c000
[ WARN] [MEMDEBUG] UserMapArea dropped with 16 remaining frames, type=Stack, range=0x3ffffde000..0x3ffffee000
[ WARN] [MEMDEBUG] UserMapArea dropped with 1 remaining frames, type=TrapContext, range=0x3fffffe000..0x3ffffff000
[ WARN] ustack 0x3ffffde000..0x3ffffee000
[ERROR] [MEMDEBUG] heap: user=557393 actual=703152 total=33554432 free=32851280
[ERROR] [MEMDEBUG] frames: alloc=4001 free=24 delta=3977 | memory: free=4241465344 total=4294967296
[ERROR] [MEMDEBUG] heap: user=557393 actual=703152 total=33554432 free=32851280
[ERROR] [MEMDEBUG] frames: alloc=4001 free=24 delta=3977 | memory: free=4241465344 total=4294967296
[ERROR] [MEMDEBUG] heap: user=557393 actual=703152 total=33554432 free=32851280
[ERROR] [MEMDEBUG] frames: alloc=4001 free=24 delta=3977 | memory: free=4241465344 total=4294967296
[ERROR] [MEMDEBUG] heap: user=557393 actual=703152 total=33554432 free=32851280
[ERROR] [MEMDEBUG] frames: alloc=4001 free=24 delta=3977 | memory: free=4241465344 total=4294967296
[ERROR] [MEMDEBUG] heap: user=557393 actual=703152 total=33554432 free=32851280
[ERROR] [MEMDEBUG] frames: alloc=4001 free=24 delta=3977 | memory: free=4241465344 total=4294967296
[ERROR] [MEMDEBUG] heap: user=557393 actual=703152 total=33554432 free=32851280
[ERROR] [MEMDEBUG] frames: alloc=4001 free=24 delta=3977 | memory: free=4241465344 total=4294967296
[ERROR] sys_sigaction: signum=15, act=0x3ffffeda90, oldact=0x3ffffedab0
[ERROR] sys_sigaction: signum=17, act=0x3ffffeda90, oldact=0x3ffffedab0
[ WARN] area vpn 0x10..0xb6
[ WARN] area vpn 0xb6..0xcc
[ WARN] area vpn 0xcc..0xcd
[ WARN] area vpn 0x3ffffde..0x3ffffee
[ WARN] alloc tid: 0
[ WARN] success
[ WARN] fork a new process with pid 4, parent pid = 3
[ERROR] fork a new process with pid 4, parent pid = 3
[ERROR] sys_sigaction: signum=17, act=0x3ffffeda60, oldact=0x3ffffeda80
[ WARN] area vpn 0x10..0xb6
[ WARN] area vpn 0xb6..0xcc
[ WARN] area vpn 0xcc..0xcd
[ WARN] area vpn 0x3ffffde..0x3ffffee
[ WARN] alloc tid: 0
[ WARN] success
[ WARN] fork a new process with pid 5, parent pid = 4
[ERROR] fork a new process with pid 5, parent pid = 4
[ERROR] sys_sigaction: signum=15, act=0x3ffffeda60, oldact=0x3ffffeda80
[ERROR] [MEMDEBUG] heap: user=916193 actual=1400320 total=33554432 free=32154112
[ERROR] [MEMDEBUG] frames: alloc=4097 free=26 delta=4071 | memory: free=4241080320 total=4294967296
[ERROR] [MEMDEBUG] heap: user=916193 actual=1400320 total=33554432 free=32154112
[ERROR] [MEMDEBUG] frames: alloc=4097 free=26 delta=4071 | memory: free=4241080320 total=4294967296
[ERROR] [MEMDEBUG] heap: user=916449 actual=1400576 total=33554432 free=32153856
[ERROR] [MEMDEBUG] frames: alloc=4097 free=26 delta=4071 | memory: free=4241080320 total=4294967296
[ERROR] [MEMDEBUG] heap: user=916449 actual=1400576 total=33554432 free=32153856
[ERROR] [MEMDEBUG] frames: alloc=4097 free=26 delta=4071 | memory: free=4241080320 total=4294967296
[ERROR] [MEMDEBUG] heap: user=916193 actual=1400320 total=33554432 free=32154112
[ERROR] [MEMDEBUG] frames: alloc=4097 free=26 delta=4071 | memory: free=4241080320 total=4294967296
[ERROR] sys_kill: pid=5, sig=9
[ERROR] sys_sigaction: signum=17, act=0x3ffffed8b0, oldact=0x3ffffed8d0
[ERROR] sys_sigaction: signum=17, act=0x3ffffeda90, oldact=0x3ffffedab0
[ERROR] sys_sigaction: signum=14, act=0x3ffffeda90, oldact=0x3ffffedab0
[ERROR] sys_setitimer: pid = 3, which=0, new_value=0x3ffffedc00, old_value=0x3ffffedc20
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
[ WARN] cpu 2: no tasks available in run_tasks
