注：

dentry 部分暂时没有加锁


思考架构：
延迟写
镜像同步

commit:
修改execve函数，实现多参数，增大堆的大小
修改初始化，压入auxv，解决之前sp撞墙的bug
修改TrapContext，新增tp寄存器的切换，因为busybox在用户态启动的时候会将一部分地址写入tp寄存器

使用ai绘制栈内部数据分布，方便队友理解
