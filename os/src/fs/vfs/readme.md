重构vfs层思路:
打算定义几个trait
1.inode,用于多文件系统的inode抽象
2.file,用于管道等,这里要存入进程的概念,和offset,注意offset不能存在inode里面
3.superblock:管理挂载位置,根目录,磁盘总容量

先自己定义一次vfs层的接口
然后通过fat32和lwext4接入的时候,再次重构vfs层
最后在正式接入上层的系统调用的时候,根据上层需要的函数,再次对vfs层进行重构




待做:
1.将超级块和inode的trait放置不同的文件内
2.重构osinode,尝试将具体的inode和文件系统改成dyn
3.重构vfs层和ext4fs,将公用的属性放置vfs层,特殊的属性特殊实现
4.修改ext4fs和superblock的模块引入
5.修改lookup逻辑,之前的逻辑是一种偷懒的逻辑
6.考虑增加一个全局挂载表,以记录不同文件系统的根节点

寻找文件的递归逻辑放置在了vfs层
单层的是在具体的文件系统内


目前已经实现:
1.重构了vfs层的inode trait
2.根据现在的vfs层修改了具体的Ext4inode