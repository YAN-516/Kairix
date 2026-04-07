注：

dentry 部分暂时没有加锁


思考架构：
延迟写
镜像同步

# 待做：
size的问题
/dev/urandom和/dev/null

完善busybox的系统调用
信号机制
多用户组
flush 里面的size
sys_rt_sigprocmask,sys_rt_sigaction,sys_ioctl,sys_setpgid,sys_ppoll,sys_gettid，sys_fcntl的完善

# 注意事项；
要考虑锁的问题了，该找个时候统一一下锁，现在的锁太乱了
暂时没有写页面置换算法，可能使用LRU？
没实现fixed map


# commit
加入系统调用sys_rt_sigprocmask,sys_rt_sigaction，sys_ioctl，sys_setpgid先返回0欺骗
加入系统调用sys_fcntl，sys_writev，sys_ppoll
修复多个bug
成功进入busyboxsh，并且成功执行脚本
暂时是单用户组
使用ai先写出桩系统调用先欺骗先,使用ai进行debug
# ai

# 待讲
1.每周的进度表
2.时间线
3.注释采用中文
4.改队友的代码采用注释源代码的方法,方便知道发生了什么修改