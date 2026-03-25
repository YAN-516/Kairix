注：

dentry 部分暂时没有加锁



commit
1.chronix的inode直接拿取了ext4的操作句柄，感觉不太合理，对此进行改进,我选择将句柄放入ext4file里面，重构vfs层，将inode返回最纯净的inode，只负责记录ino号和inotypes
2.处理路径寻找的问题
3.参考NighthawkOS,加入dentry的一层抽象层dir，将所有与lwext4底层有关的函数全部封装起来
4.加入dcache
5.删除没必要的代码，重构代码逻辑
ai工作部分：
1.一个专门的处理路径的函数，以及路径切割的思路
2.注释采用ai重写一遍，防止因为我的中式英语导致的理解错误
3.使用ai进行debug，所以大部分info是ai写的
待做：
1.暂时将create_file的封装放在ext4的dir里面，后面有时间再重构
2.cwd的具体处理还没搞定，比如cd这些


将cwd改成Arc<dyn Dentry>
重构对应的dentry函数，将原本的全部采用绝对路径的函数转换成使用dentry引用
加入sys_chdir系统调用
加入sys_getcwd系统调用
加入sys_mkdir,暂时忽略传入的0666参数，等到之后支持了这些功能再来完善