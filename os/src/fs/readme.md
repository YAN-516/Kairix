注：

dentry 部分暂时没有加锁


思考架构：
延迟写
镜像同步

# 待做：
/dev/urandom

完善busybox的系统调用
多用户组
flush 里面的size
/etc/localtime
软连接，可能需要修改底层ext4
dev,fat32,procfs
# 注意事项；
要考虑锁的问题了，该找个时候统一一下锁，现在的锁太乱了
暂时没有写页面置换算法，可能使用LRU？
没实现fixed map



# 待做
软连接
信号和多线程之间的关系还是有问题
锁
测试用例
libctest

感觉页缓存还存在问题，查找文件很慢
# commit
1.ai修改alloc_fd,修复从不检查上限的问题
2.ai修复sys_waitpid的返回值
3.处理libctests内测试用例的名字bug，将包含-的测试用例名字改成_
4.实现sys_pread64和sys_pwrite64
# ai
translated_byte_buffer
# 待讲
1.每周的进度表
2.时间线
3.注释采用中文
4.改队友的代码采用注释源代码的方法,方便知道发生了什么修改

