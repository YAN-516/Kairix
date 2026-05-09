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
换镜像
看ltp
看评分机制

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
修改 syscall/signal.rs 中的 handle_signals，让它在没有 sa_restorer 时，使用一个更安全的 restorer 机制（而不是放在栈上）
# commit

# ai
glibc和musl的iozone都大概33分，关键在于反向读和预读取
lmbench 还有优化空间
# 待讲
1.每周的进度表
2.时间线
3.注释采用中文
4.改队友的代码采用注释源代码的方法,方便知道发生了什么修改


#### OS COMP TEST GROUP START lmbench-glibc ####
latency measurements
Simple syscall: 14.3265 microseconds
Simple read: 22.5011 microseconds
Simple write: 21.8838 microseconds
Simple stat: 356.3404 microseconds
Simple fstat: 24.9094 microseconds
Simple open/close: 356.0696 microseconds
Select on 100 fd's: 77.9444 microseconds
Signal handler installation: 20.0025 microseconds
Signal handler overhead: 1.7647 microseconds
make[1]: Leaving directory '/workspace/os'


Signal handler overhead: 87.7412 microseconds
[ERROR] exit_current_and_run_next: tid=0 exit_code=0
[ERROR] sys_rt_sigreturn: using saved_mask=0x0
[ERROR] [DEBUG waitpid] parent_pid=3 found zombie child pid=22 exit_code=0
[ERROR] fork a new process with pid 25, parent pid = 3
[ERROR] sys_sigaction: signum=3, act=0x3ffffed6e0, oldact=0x0
[ERROR] [sys_execve] path=./lmbench_all cwd_name=glibc
[ERROR] Executing program: ./lmbench_all


this is parent
/ # cd glibc
/glibc # ./lmbench_all lat_sig -P 1 prot lat_sig
make[1]: Leaving directory '/workspace/os'
root@ubuntu20:/workspace/os# 

帮我找到为什么会崩溃的原因，这个不是我主动退出的，如果我开LOG=ERROR，这个测试就不会崩溃，如果LOG=OFF，执行这个就会崩溃