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
sys_ioctl,暂时采取全部返回0的手段先糊弄
修复exit_current_and_run_next的bug
加入sys_exit_group
# ai

# 待讲
1.每周的进度表
2.时间线
3.注释采用中文
4.改队友的代码采用注释源代码的方法,方便知道发生了什么修改