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
时间戳
进程退出时不关闭 fd_table 的问题
# commit

# ai
修复 mmap 系统调用语义
文件：mm.rs
修复点：

sys_mmap 增加参数校验（flags 组合、对齐、溢出）
MAP_FIXED 时按区间裁剪冲突 VMA
sys_munmap(start, len) 按区间生效，不再只删“起点正好匹配”的 area
sys_mprotect 恢复实际生效逻辑（按页更新页表权限）
修复 MAP_PRIVATE 文件映射

文件：vm_set.rs
修复点：
MAP_PRIVATE 缺页时，从 page cache 拷贝到私有 frame，不再直接共享缓存页
mmap flags 改为 flags & 0x3 解析 shared/private，避免组合位误判

修复 execve 对 ELF 的回退逻辑
文件：process.rs
修复点：
只有“非 ELF”才走 busybox sh 回退
ELF 加载失败（例如解释器缺失）不再当脚本执行，避免伪语法错误

修复 run-sdcard 镜像注入
文件：Makefile
修复点：
在 do-patch-sdcard 里自动补齐 /lib/ld-musl-riscv64-sf.so.1
优先拷贝 /musl/lib/ld-musl-riscv64-sf.so.1
若不存在则用 /musl/lib/libc.so 生成该路径（musl 常见部署方式）

# 待讲
1.每周的进度表
2.时间线
3.注释采用中文
4.改队友的代码采用注释源代码的方法,方便知道发生了什么修改