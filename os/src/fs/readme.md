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
dentry缓存还可以优化
文件系统很多地方可以优化

堆的碎片处理机制
懒分配现在还是一整个区域的，不是一页一页
栈的自动扩大可能还有问题
不确定堆和内存和栈是否还有泄漏问题
./lmbench_all lat_proc -P 1 fork

整理makefile
# commit
优化文件系统，实现页缓存预读,实现dentry的负缓存，修复一些隐藏bug
修复sys_pselect6 和 sys_ppoll，避免永久睡死
# ai
glibc和musl的iozone都大概33分，关键在于反向读和预读取
lmbench 还有优化空间
# 待讲
1.每周的进度表
2.时间线
3.注释采用中文
4.改队友的代码采用注释源代码的方法,方便知道发生了什么修改

# 报错日志


# ./lmbench_all lat_sig -P 1 install
# ./lmbench_all lat_sig -P 1 catch
# ./lmbench_all lat_sig -P 1 prot lat_sig

/glibc # busybox sh lmbench_testcode.sh
latency measurements
Simple syscall: 18.4819 microseconds
Simple read: 22.2510 microseconds
Simple write: 20.3602 microseconds
Simple stat: 302.9313 microseconds
Simple fstat: 25.0266 microseconds
Simple open/close: 317.3683 microseconds
Select on 100 fd's: 79.4711 microseconds
Signal handler installation: 18.6500 microseconds
Signal handler overhead: 1.4563 microseconds
make[1]: Leaving directory '/workspace/os'


./busybox mkdir -p /var/tmp
./busybox touch /var/tmp/lmbench
cp hello /tmp
./lmbench_all lat_pagefault -P 1 /var/tmp/XXX

File /var/tmp/XXX write bandwidth:392 KB/sec
[ERROR] sys_rt_sigreturn: using saved_mask=0x0
[ERROR] [DEBUG waitpid] parent_pid=3 found zombie child pid=3434 exit_code=0
[ERROR] fork a new process with pid 3435, parent pid = 3
[ERROR] sys_sigaction: signum=3, act=0x3ffffed6e0, oldact=0x0
[ERROR] [sys_execve] path=./lmbench_all cwd_name=glibc
[ERROR] Executing program: ./lmbench_all
[ERROR] [MEMDEBUG] heap: user=18661451 actual=24781888 total=67108864 free=42326976
[ERROR] [MEMDEBUG] frames: alloc=156648 free=79218 delta=77430 | memory: free=3906940928 total=4294967296
[ERROR] [MEMDEBUG] heap: user=18661451 actual=24781888 total=67108864 free=42326976
[ERROR] [MEMDEBUG] frames: alloc=156648 free=79218 delta=77430 | memory: free=3906940928 total=4294967296
[ERROR] [MEMDEBUG] heap: user=18661451 actual=24781888 total=67108864 free=42326976
[ERROR] [MEMDEBUG] frames: alloc=156648 free=79218 delta=77430 | memory: free=3906940928 total=4294967296
[ERROR] [MEMDEBUG] heap: user=18661451 actual=24781888 total=67108864 free=42326976
[ERROR] [MEMDEBUG] frames: alloc=156648 free=79218 delta=77430 | memory: free=3906940928 total=4294967296
[ERROR] [MEMDEBUG] heap: user=18661451 actual=24781888 total=67108864 free=42326976
[ERROR] [MEMDEBUG] frames: alloc=156648 free=79218 delta=77430 | memory: free=3906940928 total=4294967296
[ERROR] [MEMDEBUG] heap: user=18661451 actual=24781888 total=67108864 free=42326976
[ERROR] [MEMDEBUG] frames: alloc=156648 free=79218 delta=77430 | memory: free=3906940928 total=4294967296
[ERROR] sys_sigaction: signum=15, act=0x3ffffecf68, oldact=0x3ffffecff8
[ERROR] sys_sigaction: signum=17, act=0x3ffffecf68, oldact=0x3ffffecff8
[ERROR] fork a new process with pid 3436, parent pid = 3435
[ERROR] sys_sigaction: signum=17, act=0x3ffffecf38, oldact=0x3ffffecfc8
[ERROR] sys_sigaction: signum=15, act=0x3ffffecf38, oldact=0x3ffffecfc8
[ERROR] [MEMDEBUG] heap: user=18941616 actual=25330144 total=67108864 free=41778720
[ERROR] [MEMDEBUG] frames: alloc=156680 free=79219 delta=77461 | memory: free=3906813952 total=4294967296
[ERROR] [MEMDEBUG] heap: user=18941616 actual=25330144 total=67108864 free=41778720
[ERROR] [MEMDEBUG] frames: alloc=156680 free=79219 delta=77461 | memory: free=3906813952 total=4294967296
[ERROR] [MEMDEBUG] heap: user=18941616 actual=25330144 total=67108864 free=41778720
[ERROR] [MEMDEBUG] frames: alloc=156680 free=79219 delta=77461 | memory: free=3906813952 total=4294967296
[ERROR] sys_sigaction: signum=17, act=0x3ffffecd58, oldact=0x3ffffecde8
[ERROR] sys_sigaction: signum=17, act=0x3ffffecf68, oldact=0x3ffffecff8
[ERROR] sys_sigaction: signum=14, act=0x3ffffecf68, oldact=0x3ffffecff8
[ERROR] sys_setitimer: pid = 3435, which=0, new_value=0x3ffffed1d8, old_value=0x3ffffed1b8
[ERROR] sys_setitimer: pid = 3435, which=0, new_value=0x3ffffed1d8, old_value=0x3ffffed1b8
[ERROR] sys_sigaction: signum=14, act=0x3ffffecf68, oldact=0x3ffffecff8
# double free or corruption (!prev)
[ERROR] sys_tgkill: tgid=3435, tid=0, sig=6
[ERROR] sys_sigaction: signum=6, act=0x3ffffecda8, oldact=0x0
[ERROR] sys_tgkill: tgid=3435, tid=0, sig=6
[ERROR] sys_rt_sigreturn: using saved_mask=0x0
[ERROR] [DEBUG waitpid] parent_pid=3 found zombie child pid=3435 exit_code=127
[ERROR] fork a new process with pid 3437, parent pid = 3
[ERROR] sys_sigaction: signum=3, act=0x3ffffed6e0, oldact=0x0
[ERROR] [sys_execve] path=./lmbench_all cwd_name=glibc
[ERROR] Executing program: ./lmbench_all
[ERROR] [MEMDEBUG] heap: user=18671403 actual=24794976 total=67108864 free=42313888
[ERROR] [MEMDEBUG] frames: alloc=157030 free=79556 delta=77474 | memory: free=3906760704 total=4294967296
[ERROR] [MEMDEBUG] heap: user=18671403 actual=24794976 total=67108864 free=42313888
[ERROR] [MEMDEBUG] frames: alloc=157030 free=79556 delta=77474 | memory: free=3906760704 total=4294967296
[ERROR] [MEMDEBUG] heap: user=18671403 actual=24794976 total=67108864 free=42313888
[ERROR] [MEMDEBUG] frames: alloc=157030 free=79556 delta=77474 | memory: free=3906760704 total=4294967296
[ERROR] [MEMDEBUG] heap: user=18671403 actual=24794976 total=67108864 free=42313888
[ERROR] [MEMDEBUG] frames: alloc=157030 free=79556 delta=77474 | memory: free=3906760704 total=4294967296
[ERROR] [MEMDEBUG] heap: user=18671403 actual=24794976 total=67108864 free=42313888
[ERROR] [MEMDEBUG] frames: alloc=157030 free=79556 delta=77474 | memory: free=3906760704 total=4294967296


file system latency
[ERROR] fork a new process with pid 3439, parent pid = 3
[ERROR] sys_sigaction: signum=3, act=0x3ffffed6e0, oldact=0x0
[ERROR] [sys_execve] path=./lmbench_all cwd_name=glibc
[ERROR] Executing program: ./lmbench_all
0k[ERROR] [MEMDEBUG] heap: user=18681355 actual=24808064 total=67108864 free=42300800
[ERROR] [MEMDEBUG] frames: alloc=157409 free=79891 delta=77518 | memory: free=3906580480 total=4294967296
[ERROR] [MEMDEBUG] heap: user=18681355 actual=24808064 total=67108864 free=42300800
[ERROR] [MEMDEBUG] frames: alloc=157409 free=79891 delta=77518 | memory: free=3906580480 total=4294967296
[ERROR] [MEMDEBUG] heap: user=18681355 actual=24808064 total=67108864 free=42300800
[ERROR] [MEMDEBUG] frames: alloc=157409 free=79891 delta=77518 | memory: free=3906580480 total=4294967296
[ERROR] [MEMDEBUG] heap: user=18681355 actual=24808064 total=67108864 free=42300800
[ERROR] [MEMDEBUG] frames: alloc=157409 free=79891 delta=77518 | memory: free=3906580480 total=4294967296
[ERROR] [MEMDEBUG] heap: user=18681355 actual=24808064 total=67108864 free=42300800
[ERROR] [MEMDEBUG] frames: alloc=157409 free=79891 delta=77518 | memory: free=3906580480 total=4294967296
[ERROR] [MEMDEBUG] heap: user=18681355 actual=24808064 total=67108864 free=42300800
[ERROR] [MEMDEBUG] frames: alloc=157409 free=79891 delta=77518 | memory: free=3906580480 total=4294967296
[ERROR] sys_sigaction: signum=15, act=0x3ffffed7d8, oldact=0x3ffffed868
[ERROR] sys_sigaction: signum=17, act=0x3ffffed7d8, oldact=0x3ffffed868
[ERROR] fork a new process with pid 3440, parent pid = 3439
[ERROR] sys_sigaction: signum=17, act=0x3ffffed7a8, oldact=0x3ffffed838
[ERROR] sys_sigaction: signum=15, act=0x3ffffed7a8, oldact=0x3ffffed838
[ERROR] [MEMDEBUG] heap: user=19058094 actual=25486064 total=67108864 free=41622800
[ERROR] [MEMDEBUG] frames: alloc=157443 free=79892 delta=77551 | memory: free=3906445312 total=4294967296
[ERROR] sys_sigaction: signum=17, act=0x3ffffed5c8, oldact=0x3ffffed658
[ERROR] sys_sigaction: signum=17, act=0x3ffffed7d8, oldact=0x3ffffed868
[ERROR] sys_sigaction: signum=14, act=0x3ffffed7d8, oldact=0x3ffffed868
[ERROR] sys_setitimer: pid = 3439, which=0, new_value=0x3ffffeda48, old_value=0x3ffffeda28
[ERROR] [DEBUG waitpid] parent_pid=3439 found zombie child pid=3440 exit_code=0
[ERROR] sys_setitimer: pid = 3439, which=0, new_value=0x3ffffeda48, old_value=0x3ffffeda28
[ERROR] sys_sigaction: signum=14, act=0x3ffffed7d8, oldact=0x3ffffed868
        46      44[ERROR] sys_sigaction: signum=15, act=0x3ffffed7d8, oldact=0x3ffffed868
[ERROR] sys_sigaction: signum=17, act=0x3ffffed7d8, oldact=0x3ffffed868
[ERROR] fork a new process with pid 3441, parent pid = 3439
[ERROR] sys_sigaction: signum=17, act=0x3ffffed7a8, oldact=0x3ffffed838
[ERROR] sys_sigaction: signum=15, act=0x3ffffed7a8, oldact=0x3ffffed838
[ERROR] sys_sigaction: signum=17, act=0x3ffffed6c8, oldact=0x3ffffed758
[ERROR] sys_sigaction: signum=17, act=0x3ffffed6c8, oldact=0x3ffffed758
[ERROR] [MEMDEBUG] heap: user=19165558 actual=25630264 total=67108864 free=41478600
[ERROR] [MEMDEBUG] frames: alloc=157481 free=79908 delta=77573 | memory: free=3906355200 total=4294967296
[ERROR] sys_sigaction: signum=17, act=0x3ffffed6c8, oldact=0x3ffffed758
[ERROR] sys_sigaction: signum=17, act=0x3ffffed6c8, oldact=0x3ffffed758
[ERROR] sys_sigaction: signum=17, act=0x3ffffed6c8, oldact=0x3ffffed758
[ERROR] [MEMDEBUG] heap: user=19272198 actual=25774208 total=67108864 free=41334656
[ERROR] [MEMDEBUG] frames: alloc=157481 free=79908 delta=77573 | memory: free=3906355200 total=4294967296
[ERROR] sys_sigaction: signum=17, act=0x3ffffed6c8, oldact=0x3ffffed758
[ERROR] sys_sigaction: signum=17, act=0x3ffffed6c8, oldact=0x3ffffed758
[ERROR] sys_sigaction: signum=17, act=0x3ffffed6c8, oldact=0x3ffffed758
[ERROR] [MEMDEBUG] heap: user=19291100 actual=25798888 total=67108864 free=41309976
[ERROR] [MEMDEBUG] frames: alloc=157484 free=79908 delta=77576 | memory: free=3906342912 total=4294967296
[ERROR] sys_sigaction: signum=17, act=0x3ffffed6c8, oldact=0x3ffffed758
[ERROR] sys_sigaction: signum=17, act=0x3ffffed6c8, oldact=0x3ffffed758
[ERROR] sys_sigaction: signum=17, act=0x3ffffed6c8, oldact=0x3ffffed758
[ERROR] [MEMDEBUG] heap: user=19264314 actual=25760392 total=67108864 free=41348472
[ERROR] [MEMDEBUG] frames: alloc=157484 free=79908 delta=77576 | memory: free=3906342912 total=4294967296
[ERROR] sys_sigaction: signum=17, act=0x3ffffed6c8, oldact=0x3ffffed758
[ERROR] sys_sigaction: signum=17, act=0x3ffffed6c8, oldact=0x3ffffed758
[ERROR] [MEMDEBUG] heap: user=19310114 actual=25825984 total=67108864 free=41282880
[ERROR] [MEMDEBUG] frames: alloc=157484 free=79908 delta=77576 | memory: free=3906342912 total=4294967296
[ERROR] sys_sigaction: signum=17, act=0x3ffffed6c8, oldact=0x3ffffed758
[ERROR] sys_sigaction: signum=17, act=0x3ffffed6c8, oldact=0x3ffffed758
[ERROR] sys_sigaction: signum=17, act=0x3ffffed6c8, oldact=0x3ffffed758
[ERROR] sys_sigaction: signum=17, act=0x3ffffed5c8, oldact=0x3ffffed658
[ERROR] sys_sigaction: signum=17, act=0x3ffffed7d8, oldact=0x3ffffed868
[ERROR] sys_sigaction: signum=14, act=0x3ffffed7d8, oldact=0x3ffffed868
[ERROR] sys_setitimer: pid = 3439, which=0, new_value=0x3ffffeda48, old_value=0x3ffffeda28
[ERROR] [DEBUG waitpid] parent_pid=3439 found zombie child pid=3441 exit_code=0
[ERROR] sys_setitimer: pid = 3439, which=0, new_value=0x3ffffeda48, old_value=0x3ffffeda28
[ERROR] sys_sigaction: signum=14, act=0x3ffffed7d8, oldact=0x3ffffed868
        104
1k[ERROR] sys_sigaction: signum=15, act=0x3ffffed7d8, oldact=0x3ffffed868
[ERROR] sys_sigaction: signum=17, act=0x3ffffed7d8, oldact=0x3ffffed868
[ERROR] fork a new process with pid 3442, parent pid = 3439
[ERROR] sys_sigaction: signum=17, act=0x3ffffed7a8, oldact=0x3ffffed838
[ERROR] sys_sigaction: signum=15, act=0x3ffffed7a8, oldact=0x3ffffed838
[ERROR] sys_sigaction: signum=17, act=0x3ffffed6c8, oldact=0x3ffffed758
[ERROR] [kernel] Panicked at src/drivers/block/virtio_blk.rs:228 Error when writing VirtIOBlk: IoError



# 1
iozone throughput write/read measurements
[ERROR] sys_rt_sigreturn: using saved_mask=0x0
[ERROR] [DEBUG waitpid] parent_pid=3 found zombie child pid=7 exit_code=0
[ERROR] fork a new process with pid 8, parent pid = 3
[ERROR] sys_sigaction: signum=3, act=0x3ffffed6e0, oldact=0x0
[ERROR] [sys_execve] path=./iozone cwd_name=musl
[ERROR] Executing program: ./iozone
[ERROR] sys_sigaction: signum=2, act=0x3ffffeda50, oldact=0x3ffffeda70
[ERROR] sys_sigaction: signum=15, act=0x3ffffeda50, oldact=0x3ffffeda70
        Iozone: Performance Test of File I/O
                Version $Revision: 3.506 $
                Compiled for 64 bit mode.
                Build: linux 

        Contributors:William Norcott, Don Capps, Isom Crawford, Kirby Collins
                     Al Slater, Scott Rhine, Mike Wisner, Ken Goss
                     Steve Landherr, Brad Smith, Mark Kelly, Dr. Alain CYR,
                     Randy Dunlap, Mark Montague, Dan Million, Gavin Brebner,
                     Jean-Marc Zucconi, Jeff Blomberg, Benny Halevy, Dave Boone,
                     Erik Habbinga, Kris Strecker, Walter Wong, Joshua Root,
                     Fabrice Bacchella, Zhenghua Xue, Qin Li, Darren Sawyer,
                     Vangel Bojaxhi, Ben England, Vikentsi Lapa,
                     Alexey Skidanov, Sudhir Kumar.

        Run began: Thu Jan  1 00:00:36 1970

        Record Size 1 kB
        File size set to 1024 kB
        Command line used: ./iozone -t 4 -i 0 -i 1 -r 1k -s 1m
        Output is in kBytes/sec
        Time Resolution = 0.000016 seconds.
        Processor cache size set to 1024 kBytes.
        Processor cache line size set to 32 bytes.
        File stride size set to 17 * record size.
        Throughput test with 4 processes
        Each process writes a 1024 kByte file in 1 kByte records
[ERROR] fork a new process with pid 9, parent pid = 8
[ERROR] no vma found for va 0x483018
[ERROR] [kernel] in application, bad addr = 0x483018, ctx: Context {
    ra: 0x40651c,
    sp: 0x3ffffec930,
    gp: 0x57348,
    tp: 0x486cd0,
    t0: 0x66,
    t1: 0x1253c,
    t2: 0xff,
    s0: 0x483010,
    s1: 0x484fa8,
    a0: 0x483010,
    a1: 0x26,
    a2: 0x1000,
    a3: 0x483010,
    a4: 0xcc1,
    a5: 0xcc0,
    a6: 0x1,
    a7: 0x87,
    s2: 0x1f,
    s3: 0x484988,
    s4: 0xffffffff80000000,
    s5: 0x18,
    s6: 0x47a9b0,
    s7: 0x22fdd63cc95386d,
    s8: 0x26,
    s9: 0x484c10,
    s10: 0x484fac,
    s11: 0x0,
    t3: 0x4061ac,
    t4: 0x0,
    t5: 0x13,
    t6: 0x1,
    sstatus: Sstatus {
        bits: 0x8000000200046000,
    },
    sepc: 0x405aac,
    fsx: [
        0x0,
        0x0,
    ],
} sending SIGSEGV.
[ERROR] fork a new process with pid 10, parent pid = 8
[ERROR] no vma found for va 0x483018
[ERROR] [kernel] in application, bad addr = 0x483018, ctx: Context {
    ra: 0x40651c,
    sp: 0x3ffffec930,
    gp: 0x57348,
    tp: 0x486cd0,
    t0: 0x66,
    t1: 0x1253c,
    t2: 0xff,
    s0: 0x483010,
    s1: 0x484fa8,
    a0: 0x483010,
    a1: 0x26,
    a2: 0x1000,
    a3: 0x483010,
    a4: 0xcc1,
    a5: 0xcc0,
    a6: 0xf,
    a7: 0x87,
    s2: 0x1f,
    s3: 0x484988,
    s4: 0xffffffff80000000,
    s5: 0x18,
    s6: 0x47a9b0,
    s7: 0x22fdd63cc95386d,
    s8: 0x26,
    s9: 0x484c10,
    s10: 0x484fac,
    s11: 0x1,
    t3: 0x4061ac,
    t4: 0x0,
    t5: 0x13,
    t6: 0x1,
    sstatus: Sstatus {
        bits: 0x8000000200046000,
    },
    sepc: 0x405aac,
    fsx: [
        0x0,
        0x0,
    ],
} sending SIGSEGV.
[ERROR] fork a new process with pid 11, parent pid = 8
[ERROR] no vma found for va 0x483018
[ERROR] [kernel] in application, bad addr = 0x483018, ctx: Context {
    ra: 0x40651c,
    sp: 0x3ffffec930,
    gp: 0x57348,
    tp: 0x486cd0,
    t0: 0x66,
    t1: 0x1253c,
    t2: 0xff,
    s0: 0x483010,
    s1: 0x484fa8,
    a0: 0x483010,
    a1: 0x26,
    a2: 0x1000,
    a3: 0x483010,
    a4: 0xcc1,
    a5: 0xcc0,
    a6: 0xf,
    a7: 0x87,
    s2: 0x1f,
    s3: 0x484988,
    s4: 0xffffffff80000000,
    s5: 0x18,
    s6: 0x47a9b0,
    s7: 0x22fdd63cc95386d,
    s8: 0x26,
    s9: 0x484c10,
    s10: 0x484fac,
    s11: 0x2,
    t3: 0x4061ac,
    t4: 0x0,
    t5: 0x13,
    t6: 0x1,
    sstatus: Sstatus {
        bits: 0x8000000200046000,
    },
    sepc: 0x405aac,
    fsx: [
        0x0,
        0x0,
    ],
} sending SIGSEGV.
[ERROR] fork a new process with pid 12, parent pid = 8
[ERROR] no vma found for va 0x483018
[ERROR] [kernel] in application, bad addr = 0x483018, ctx: Context {
    ra: 0x40651c,
    sp: 0x3ffffec930,
    gp: 0x57348,
    tp: 0x486cd0,
    t0: 0x66,
    t1: 0x1253c,
    t2: 0xff,
    s0: 0x483010,
    s1: 0x484fa8,
    a0: 0x483010,
    a1: 0x26,
    a2: 0x1000,
    a3: 0x483010,
    a4: 0xcc1,
    a5: 0xcc0,
    a6: 0xf,
    a7: 0x87,
    s2: 0x1f,
    s3: 0x484988,
    s4: 0xffffffff80000000,
    s5: 0x18,
    s6: 0x47a9b0,
    s7: 0x22fdd63cc95386d,
    s8: 0x26,
    s9: 0x484c10,
    s10: 0x484fac,
    s11: 0x3,
    t3: 0x4061ac,
    t4: 0x0,
    t5: 0x13,
    t6: 0x1,
    sstatus: Sstatus {
        bits: 0x8000000200046000,
    },
    sepc: 0x405aac,
    fsx: [
        0x0,
        0x0,
    ],
} sending SIGSEGV.
结论：bug 的原因是 from_elf 中没有将 ELF segment 的 start_va 向下对齐到页面边界，导致 va_range 和实际页表映射范围不一致。当程序访问对齐后的页面开头到 start_va 之间的地址时，find_area 找不到。