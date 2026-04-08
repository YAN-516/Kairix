注：

dentry 部分暂时没有加锁


思考架构：
延迟写
镜像同步

# 待做：
/dev/urandom和/dev/null

完善busybox的系统调用
信号机制
多用户组
flush 里面的size
sys_rt_sigprocmask,sys_rt_sigaction,sys_ioctl,sys_setpgid,sys_ppoll,sys_gettid,sys_fcntl的完善,实现多核版本

sys_exit_group:xp
# 注意事项；
要考虑锁的问题了，该找个时候统一一下锁，现在的锁太乱了
暂时没有写页面置换算法，可能使用LRU？
没实现fixed map


# commit
fstatat


1.页表切换,satp在初始化的时候没有正确的切换,
2.映射，map多次映射到同一个地方的错误,map和cow的互相冲突
3.进程调度，sys_waitpid（SUM 位缺失）
4.basic 中的"gettimeofday,sleep"暂时没办法过
5.修复basic的mmap(本质来源于trap的寄存器传入参数的问题)
# ai

# 待讲
1.每周的进度表
2.时间线
3.注释采用中文
4.改队友的代码采用注释源代码的方法,方便知道发生了什么修改



shell 进程 fork 出了 ls 子进程。此时 shell 自己的栈被标记为 COW（只读共享）。
ls 正常运行并打印了目录（你看到了 lost+found 等输出），然后 ls 退出。
shell 从 waitpid 醒来，准备继续执行，此时它向自己的栈写入数据。
因为栈是 COW（只读的），触发了 StorePageFault。
你的内核捕获到了缺页，分配了一个新的物理页（准备把数据拷过去），然后调用 map_page 准备重新映射。
原版报错原因：因为这个虚拟地址本来就映射着一个只读页，所以 polyhal 的 map_page 直接 panic 了（mapped before mapping）！
修改后卡死原因：你把报错去掉了。COW 缺页处理时，底层发现有映射直接 return 了，并没有把新页写进 PTE，也没有把只读改成可写。结果 shell 返回用户态一执行，又触发缺页，又忽略，又触发……形成无限缺页死循环（所以你看到打印完 ls 结果就卡死了，[ERROR] asdj 疯狂输出）。

COW 与懒加载（Lazy Alloc）发生冲突！
shell fork 出子进程后，它的 Stack 等内存段被标记为 cow_flag = true。
但是！Stack 区域中并不是所有页都已经分配了物理页。有些页是未分配的懒加载页（空洞）。
ls 退出后，shell 恢复执行，它正好访问到了一个未分配的新栈页。
硬件触发 Store Page Fault。你的 access_check 函数一看：哦？这整个 Area 的 cow_flag 是 true？那这就是 ExceptionType::Cow！
代码进入 handle_cow_page_fault。问题来了，看看你的循环：
Rust
let data = area.data_frames.clone();
for vpn in data.keys() { ... }
【致命漏洞】：因为这个触发缺页的 vpn 是个懒加载新页，它压根就不在 data_frames 里！你的 data.keys() 循环完全错过了它！
函数做了一堆无用功，返回 Some(())。
硬件以为处理好了，重试指令——结果这页还是空的，再次触发缺页！从而陷入了毫无日志、无声无息的无限死循环！