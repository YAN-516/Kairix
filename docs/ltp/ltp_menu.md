https://linux-test-project.readthedocs.io/en/latest/users/quick_start.html
[×]为通过或者部分通过
===== abort =======
- [x] abort01                        2      p2

===== accept =======
- [ ] accept01                       5
- [ ] accept02                       1 存在问题,藏起来
- [×] accept03                       23 p23
- [ ] accept4_01                     9 存在问题,藏起来

235
===== access =======
- [×] access01                       199    p199
- [×] access02                       16     p16
- [×] access03                       8      p8
- [×] access04                       12     p12

===== acct =======
- [ ] acct01                         10
- [ ] acct02                         1

===== add_key =======
- [ ] add_key01                      8
- [ ] add_key02                      9
- [ ] add_key03                      1
- [ ] add_key04                      1
- [ ] add_key05                      5

===== adjtimex =======
- [ ] adjtimex01                     2
- [ ] adjtimex02                     8
- [ ] adjtimex03                     1

===== af_alg =======
- [ ] af_alg01                       14
- [ ] af_alg02                       1
- [ ] af_alg03                       1
- [ ] af_alg04                       6
- [ ] af_alg05                       1
- [ ] af_alg06                       1
- [ ] af_alg07                       1
- [ ] af_alg08                       1

===== alarm =======
- [ ] alarm02                        6
- [ ] alarm03                        2
- [ ] alarm05                        3
- [ ] alarm06                        2
- [ ] alarm07                        2

===== arch_prctl =======
- [ ] arch_prctl01                   4

===== asapi =======
- [ ] asapi_02                       12

===== aslr =======
- [ ] aslr01                         1

===== autogroup =======
- [ ] autogroup01                    1

===== bind =======
- [ ] bind01                         7
- [ ] bind02                         1
- [ ] bind03                         3
- [ ] bind04                         16
- [ ] bind05                         14 有问题,且无法被跳过

===== block_dev =======
- [ ] block_dev                      1

===== bpf_map =======
- [ ] bpf_map01                      7

===== bpf_prog =======
- [ ] bpf_prog01                     2
- [ ] bpf_prog02                     3
- [ ] bpf_prog03                     1
- [ ] bpf_prog04                     1
- [ ] bpf_prog05                     2
- [ ] bpf_prog06                     2
- [ ] bpf_prog07                     2

4
===== brk =======
- [×] brk01                          2
- [×] brk02                          2

===== cachestat =======
- [ ] cachestat01                    4
- [ ] cachestat02                    1
- [ ] cachestat03                    1
- [ ] cachestat04                    1

===== can_bcm =======
- [ ] can_bcm01                      1

===== can_filter =======
- [ ] can_filter                     1

===== can_rcv_own_msgs =======
- [ ] can_rcv_own_msgs               1

===== capget =======
- [ ] capget01                       6
- [ ] capget02                       5

===== capset =======
- [ ] capset01                       3
- [ ] capset02                       6
- [ ] capset03                       1
- [ ] capset04                       1

===== cfs_bandwidth =======
- [ ] cfs_bandwidth01                2

===== cgroup_core =======
- [ ] cgroup_core01                  1
- [ ] cgroup_core02                  1
- [ ] cgroup_core03                  2

===== chdir =======
- [ ] chdir01                        79 p77sk3
- [ ] chdir02                        1
- [ ] chdir04                        3

===== chmod =======
- [ ] chmod01                        32
- [ ] chmod03                        4
- [ ] chmod05                        1
- [ ] chmod06                        9
- [ ] chmod07                        1
- [ ] chmod08                        3
- [ ] chmod09                        1

===== chown =======
- [ ] chown01                        1
- [ ] chown01_16                     1
- [ ] chown02                        2
- [ ] chown02_16                     1
- [ ] chown03                        1
- [ ] chown03_16                     1
- [ ] chown04                        8
- [ ] chown04_16                     1
- [ ] chown05                        6
- [ ] chown05_16                     1

===== chroot =======
- [ ] chroot01                       1
- [ ] chroot02                       2
- [ ] chroot03                       5
- [ ] chroot04                       1

===== clock_adjtime =======
- [ ] clock_adjtime01                9
- [ ] clock_adjtime02                6

===== clock_getres =======
- [ ] clock_getres01                 44

===== clock_gettime =======
- [ ] clock_gettime01                16
- [ ] clock_gettime02                10
- [ ] clock_gettime03                24
- [ ] clock_gettime04                6

===== clock_nanosleep =======
- [ ] clock_nanosleep01              14
- [ ] clock_nanosleep02              7
- [ ] clock_nanosleep03              2
- [ ] clock_nanosleep04              4

===== clock_settime =======
- [ ] clock_settime01                4
- [ ] clock_settime02                12
- [ ] clock_settime03                1
- [ ] clock_settime04                4

===== clone =======
- [x] clone01                        2  p2
- [x] clone02                        -  
- [x] clone03                        1  p1
- [ ] clone04                        1 不通过的原因疑似是镜像中musl版本的问题，暂搁置
- [x] clone05                        1  p1
- [x] clone06                        1  p1
- [x] clone07                        1  p1
- [x] clone08                        5  p5
- [x] clone09                        1  p1
- [ ] clone10                        1
- [ ] clone11                        6
- [x] clone301                       7  p7
- [x] clone302                       12 p12
- [ ] clone303                       1  TCONF: V2 'base' controller required, but it's mounted on V1
- [ ] clone304                       13

===== close =======
- [ ] close01                        3
- [ ] close02                        1

===== close_range =======
- [ ] close_range01                  20  过于复杂
- [×] close_range02                  11 p9

===== confstr =======
- [×] confstr01                      34 p32

===== connect =======
- [ ] connect02                      1

38
===== copy_file_range =======
- [ ] copy_file_range01              20 p20
- [ ] copy_file_range02              28 p24 sk14
- [×] copy_file_range03              2  p2

===== crash =======
- [ ] crash02                        1

===== creat =======
- [×] creat01                        6 p6
- [×] creat03                        1 p1
- [ ] creat04                        2
- [×] creat05                        1 p1
- [ ] creat06                        8
- [ ] creat07                        1
- [×] creat08                        9 p9
- [ ] creat09                        32

===== crypto_user =======
- [ ] crypto_user01                  1
- [ ] crypto_user02                  1

===== cve-2014- =======
- [ ] cve-2014-0196                  1

===== cve-2015- =======
- [ ] cve-2015-3290                  1

===== cve-2016- =======
- [ ] cve-2016-10044                 1
- [ ] cve-2016-7042                  1
- [ ] cve-2016-7117                  1

===== cve-2017- =======
- [ ] cve-2017-16939                 1
- [ ] cve-2017-17052                 1
- [ ] cve-2017-17053                 1
- [ ] cve-2017-2618                  1
- [ ] cve-2017-2671                  1

===== cve-2022- =======
- [ ] cve-2022-4378                  7

===== cve-2025- =======
- [ ] cve-2025-21756                 1
- [ ] cve-2025-38236                 1

===== delete_module =======
- [ ] delete_module01                1
- [ ] delete_module02                5
- [ ] delete_module03                1

===== dio_append =======
- [ ] dio_append                     1

===== dio_read =======
- [ ] dio_read                       1

===== dio_sparse =======
- [ ] dio_sparse                     1

===== dio_truncate =======
- [ ] dio_truncate                   1

===== dirtyc0w =======
- [ ] dirtyc0w                       1

===== dirtypipe =======
- [ ] dirtypipe                      1

===== dup =======
- [x] dup01                          2  p2
- [x] dup02                          2  p2
- [x] dup03                          1  p1
- [x] dup04                          2  p2
- [x] dup05                          1  p1
- [x] dup06                          1  p1
- [x] dup07                          3  p3
- [x] dup201                         4  p4
- [x] dup202                         6  p6
- [x] dup203                         4  p4
- [x] dup204                         4  p4
- [x] dup205                         1  p1
- [x] dup206                         1  p1
- [x] dup207                         2  p2
- [x] dup3_01                        2  p2
- [x] dup3_02                        3  p3

===== epoll_create =======
- [ ] epoll_create01                 4
- [ ] epoll_create02                 4
- [ ] epoll_create1_01               2
- [ ] epoll_create1_02               2

===== epoll_ctl =======
- [×] epoll_ctl01                    3 p3
- [×] epoll_ctl02                    9 p9
- [×] epoll_ctl03                    256 p256
- [×] epoll_ctl04                    1 p1
- [×] epoll_ctl05                    1 p1

===== epoll_pwait =======
- [ ] epoll_pwait01                  4
- [ ] epoll_pwait02                  2
- [ ] epoll_pwait03                  14
- [ ] epoll_pwait04                  2
- [ ] epoll_pwait05                  3
- [ ] epoll_pwait06                  4

===== epoll_wait =======
- [ ] epoll_wait01                   3
- [ ] epoll_wait02                   7
- [ ] epoll_wait03                   5
- [ ] epoll_wait04                   1
- [ ] epoll_wait05                   1 卡死
- [ ] epoll_wait06                   9 卡死
- [ ] epoll_wait07                   5 卡死

===== eventfd =======
- [ ] eventfd01                      4
- [ ] eventfd02                      5
- [ ] eventfd03                      3
- [ ] eventfd04                      3
- [ ] eventfd05                      2
- [ ] eventfd2_01                    2
- [ ] eventfd2_02                    2
- [ ] eventfd2_03                    2

===== execl =======
- [ ] execl01                        1
- [ ] execle01                       1
- [ ] execlp01                       1

===== execv =======
- [ ] execv01                        1
- [ ] execve01                       1
- [ ] execve02                       1
- [ ] execve03                       6
- [ ] execve04                       1
- [ ] execve05                       8
- [ ] execve06                       1
- [ ] execveat01                     4
- [ ] execveat02                     4
- [ ] execveat03                     1
- [ ] execvp01                       1

===== exit =======
- [x] exit01                         -
- [x] exit02                         1  p1

===== exit_group =======
- [ ] exit_group01                   1

===== faccessat =======
- [ ] faccessat01                    3
- [ ] faccessat02                    2
- [ ] faccessat201                   7
- [ ] faccessat202                   6

11
===== fallocate =======
- [×] fallocate03                    8 p8
- [×] fallocate04                    12 p25
- [ ] fallocate05                    17 很慢,不打算实现
- [ ] fallocate06                    27 存在pass，但是还要写很大的文件

===== fanotify =======
- [×] fanotify01                     390 p390
- [×] fanotify02                     8   p8
- [×] fanotify03                     25  p21
- [×] fanotify04                     9   p9
- [ ] fanotify05                     3  会卡住
- [×] fanotify06                     18 p9s1 overlayfs is not configured in this kerne
- [×] fanotify07                     2  p2
- [×] fanotify08                     2  p2
- [×] fanotify09                     76 p74f2
- [×] fanotify10                     445 p1047
- [×] fanotify11                     2   p2
- [×] fanotify12                     10  p10
- [×] fanotify13                     110 p75 s120 需要overlayfs
- [×] fanotify14                     315 p285
- [×] fanotify15                     50  p50
- [×] fanotify16                     770 p770
- [×] fanotify17                     4   p1 s3 fanotify inside user namespace is not supported
- [×] fanotify18                     9   p9
- [×] fanotify19                     16  p16
- [×] fanotify20                     10  p10
- [×] fanotify21                     40  p10
- [ ] fanotify22                     4  TCONF: Couldn't find 'debugfs' in $PATH
- [×] fanotify23                     6   p6
- [ ] fanotify24                     5   不存在 
- [ ] fanotify25                     1   不存在

===== fchdir =======
- [ ] fchdir01                       1
- [ ] fchdir02                       1
- [ ] fchdir03                       1

===== fchmod =======
- [ ] fchmod01                       8
- [ ] fchmod02                       1
- [ ] fchmod03                       1
- [ ] fchmod04                       1
- [ ] fchmod05                       1
- [ ] fchmod06                       3
- [ ] fchmodat01                     6
- [ ] fchmodat02                     6
- [ ] fchmodat2_01                   5
- [ ] fchmodat2_02                   1

===== fchown =======
- [ ] fchown01                       1
- [ ] fchown01_16                    1
- [ ] fchown02                       2
- [ ] fchown02_16                    1
- [ ] fchown03                       1
- [ ] fchown03_16                    1
- [ ] fchown04                       3
- [ ] fchown04_16                    1
- [ ] fchown05                       6
- [ ] fchown05_16                    1
- [ ] fchownat01                     6
- [ ] fchownat02                     3
- [ ] fchownat03                     10

46
===== fcntl =======
- [×] fcntl02                        6 p6
- [×] fcntl02_64                     6 p6
- [×] fcntl03                        1
- [×] fcntl03_64                     1
- [×] fcntl04                        1
- [×] fcntl04_64                     1
- [ ] fcntl05                        6 p4f2 需要flock
- [ ] fcntl05_64                     6
- [×] fcntl08                        1
- [×] fcntl08_64                     1
- [×] fcntl12                        1
- [×] fcntl12_64                     1
- [ ] fcntl13                        4 p2f2 需要flock
- [ ] fcntl13_64                     4 p2f2
- [ ] fcntl14                        2 需要flock
- [ ] fcntl14_64                     2 需要flock
- [ ] fcntl15                        12需要flock
- [ ] fcntl15_64                     12需要flock
- [ ] fcntl27                        2 需要flock
- [ ] fcntl27_64                     2 需要flock
- [×] fcntl29                        3 p3
- [×] fcntl29_64                     3 p3
- [×] fcntl30                        4 p4
- [×] fcntl30_64                     4 p4
- [ ] fcntl33                        7 暂时没找到bug，可能要改tmp
- [ ] fcntl33_64                     7 暂时没找到bug
- [ ] fcntl34                        1 需要flock
- [ ] fcntl34_64                     1  需要flock
- [ ] fcntl35                        2
- [ ] fcntl35_64                     2
- [ ] fcntl36                        7 OFD文件锁
- [ ] fcntl36_64                     7 OFD文件锁
- [ ] fcntl37                        3 会崩溃，加入了规避机制
- [ ] fcntl37_64                     3 会崩溃，加入了规避机制
- [ ] fcntl38                        2 .config
- [ ] fcntl38_64                     2  .config
- [ ] fcntl39                        4 .config
- [ ] fcntl39_64                     4  .config
- [ ] fcntl40                        1 不存在
- [ ] fcntl40_64                     1 不存在

===== fdatasync =======
- [ ] fdatasync03                    4

===== fgetxattr =======
- [×] fgetxattr01                    17 p25
- [×] fgetxattr02                    13 p13
- [×] fgetxattr03                    1 p1

===== file_attr =======
- [ ] file_attr01                    8
- [ ] file_attr02                    1
- [ ] file_attr03                    1
- [ ] file_attr04                    2
- [ ] file_attr05                    4

===== finit_module =======
- [ ] finit_module01                 1
- [ ] finit_module02                 1

===== flistxattr =======
- [×] flistxattr01                   1 p1
- [×] flistxattr02                   2 p2
- [×] flistxattr03                   2 p2

===== flock =======
- [ ] flock01                        3
- [ ] flock02                        4
- [ ] flock03                        3
- [ ] flock04                        6
- [ ] flock06                        4
- [ ] flock07                        2

===== fork =======
- [x] fork01                         2  p2
- [x] fork03                         1  p1
- [x] fork04                         3  p3
- [x] fork07                         1  p1
- [x] fork08                         1  p1
- [x] fork09                         1  p1,无summary
- [x] fork10                         2  p2
- [ ] fork14                         1 主要问题是用户虚拟地址空间大小不足，导致的mmap失败，暂搁置，可能是SV39架构问题

===== fork_procs =======
- [ ] fork_procs                     1

===== fpathconf =======
- [ ] fpathconf01                    9

===== fremovexattr =======
- [×] fremovexattr01                 5 p5
- [×] fremovexattr02                 11 p15

===== fs_fill =======
- [ ] fs_fill                        10

===== fsconfig =======
- [×] fsconfig01                     17 p5
- [×] fsconfig02                     26 p26
- [×] fsconfig03                     5  p5

===== fsetxattr =======
- [×] fsetxattr01                    31 p45
- [ ] fsetxattr02                    7  需要 Linux 的 brd 驱动

===== fsmount =======
- [×] fsmount01                      150 p80
- [×] fsmount02                      15  p15

===== fsopen =======
- [×] fsopen01                       10 p10
- [×] fsopen02                       2  p2

===== fspick =======
- [×] fspick01                       80 p20
- [×] fspick02                       15 p15

===== fsplough =======
- [ ] fsplough                       3

===== fstat =======
- [×] fstat02                        6 p6
- [×] fstat02_64                     6 p6
- [ ] fstat03                        2
- [ ] fstat03_64                     2
- [×] fstatfs01                      10 p10
- [×] fstatfs01_64                   10 p10
- [×] fstatfs02                      2  p2
- [×] fstatfs02_64                   2  p2

===== fsx-linux =======
- [ ] fsx-linux                      1

===== fsync =======
- [×] fsync01                        50 p50
- [ ] fsync02                        1   应该是缺文件，暂时不想做
- [×] fsync03                        5   p5
- [ ] fsync04                        4  卡住，暂时忽略

===== ftruncate =======
- [×] ftruncate01                    2 p1f1要动底层库，暂时不想动
- [×] ftruncate01_64                 2 p1f1
- [×] ftruncate03                    4 p1f3
- [×] ftruncate03_64                 4 p1f3
- [ ] ftruncate04                    1
- [ ] ftruncate04_64                 1

===== futex_cmp_requeue =======
- [ ] futex_cmp_requeue01            7
- [ ] futex_cmp_requeue02            3

===== futex_wait =======
- [ ] futex_wait01                   4
- [ ] futex_wait02                   1
- [ ] futex_wait03                   1
- [ ] futex_wait04                   1
- [ ] futex_wait05                   7
- [ ] futex_waitv01                  1
- [ ] futex_waitv02                  1
- [ ] futex_waitv03                  1

===== futex_wait_bitset =======
- [ ] futex_wait_bitset01            2

===== futex_wake =======
- [ ] futex_wake01                   6
- [ ] futex_wake02                   11
- [ ] futex_wake03                   11
- [ ] futex_wake04                   1

===== getaddrinfo =======
- [ ] getaddrinfo_01                 22

===== getcontext =======
- [ ] getcontext01                   2

===== getcpu =======
- [ ] getcpu01                       1
- [ ] getcpu02                       2

===== getcwd =======
- [ ] getcwd01                       5
- [×] getcwd02                       3 p3
- [×] getcwd03                       1 p1
- [ ] getcwd04                       1 TCONF: Test needs at least 2 CPUs online

===== getdents =======
- [×] getdents01                     16 p3 s1 syscall(-1) __NR_getdents not supported on your arch
- [×] getdents02                     80 p12 s1 syscall(-1) __NR_getdents not supported on your arch

===== getdomainname =======
- [×] getdomainname01                1 p1

===== getegid =======
- [×] getegid01                      1 p1
- [×] getegid01_16                   1 p1
- [×] getegid02                      1 p1
- [×] getegid02_16                   1 p1

===== geteuid =======
- [×] geteuid01                      1 p1
- [ ] geteuid01_16                   1
- [×] geteuid02                      2 p2
- [ ] geteuid02_16                   1

===== getgid =======
- [×] getgid01                       1 p1
- [ ] getgid01_16                    1
- [×] getgid03                       1 p1
- [ ] getgid03_16                    1

===== gethostbyname_r =======
- [ ] gethostbyname_r01              1

===== gethostid =======
- [ ] gethostid01                    5

===== gethostname =======
- [×] gethostname01                  1 p1
- [ ] gethostname02                  1

===== getitimer =======
- [ ] getitimer01                    30
- [ ] getitimer02                    3

===== getpagesize =======
- [×] getpagesize01                  1 p1

===== getpeername =======
- [ ] getpeername01                  7

===== getpgid =======
- [x] getpgid01                      8  p8
- [x] getpgid02                      2  p2

===== getpgrp =======
- [×] getpgrp01                      2  p2

===== getpid =======
- [x] getpid01                       100 p100
- [x] getpid02                       2  p2

===== getppid =======
- [x] getppid01                      1  p1
- [x] getppid02                      1  p1

===== getpriority =======
- [ ] getpriority01                  3
- [ ] getpriority02                  4

===== getrandom =======
- [×] getrandom01                    4 p4
- [×] getrandom02                    4 p2
- [×] getrandom03                    9 p9
- [×] getrandom04                    1 p1
- [ ] getrandom05                    4

===== getrlimit =======
- [×] getrlimit01                    16 p16
- [ ] getrlimit02                    2  死锁
- [ ] getrlimit03                    16

===== getrusage =======
- [ ] getrusage01                    2
- [ ] getrusage02                    4
- [ ] getrusage03                    9

===== getsid =======
- [ ] getsid01                       1
- [ ] getsid02                       1

===== getsockname =======
- [ ] getsockname01                  6

===== getsockopt =======
- [ ] getsockopt01                   9
- [ ] getsockopt02                   1

===== gettid =======
- [x] gettid01                       2  p2
- [x] gettid02                       11 p11

===== gettimeofday =======
- [ ] gettimeofday01                 3
- [ ] gettimeofday02                 1

===== getuid =======
- [ ] getuid01                       1
- [ ] getuid01_16                    1
- [ ] getuid03                       2
- [ ] getuid03_16                    1

===== getxattr =======
- [×] getxattr01                     4 p4
- [×] getxattr02                     14 p20
- [×] getxattr03                     3 p3
- [ ] getxattr04                     1 Couldn't find 'mkfs.xfs' in $PATH at tst_cmd.c:75

===== hugefallocate =======
- [ ] hugefallocate01                1
- [ ] hugefallocate02                1

===== hugefork =======
- [ ] hugefork01                     2
- [ ] hugefork02                     1

===== hugemmap =======
- [ ] hugemmap01                     1
- [ ] hugemmap02                     1
- [ ] hugemmap04                     1
- [ ] hugemmap05                     1
- [ ] hugemmap06                     5
- [ ] hugemmap07                     1
- [ ] hugemmap08                     2
- [ ] hugemmap09                     1
- [ ] hugemmap10                     1
- [ ] hugemmap11                     1
- [ ] hugemmap12                     1
- [ ] hugemmap13                     1
- [ ] hugemmap14                     1
- [ ] hugemmap15                     1
- [ ] hugemmap16                     1
- [ ] hugemmap17                     1
- [ ] hugemmap18                     1
- [ ] hugemmap19                     1
- [ ] hugemmap20                     4
- [ ] hugemmap21                     1
- [ ] hugemmap22                     2
- [ ] hugemmap23                     6
- [ ] hugemmap24                     1
- [ ] hugemmap25                     1
- [ ] hugemmap26                     1
- [ ] hugemmap27                     1
- [ ] hugemmap28                     1
- [ ] hugemmap29                     1
- [ ] hugemmap30                     1
- [ ] hugemmap31                     1
- [ ] hugemmap32                     1

===== hugeshmat =======
- [ ] hugeshmat01                    3
- [ ] hugeshmat02                    2
- [ ] hugeshmat03                    1
- [ ] hugeshmat04                    3
- [ ] hugeshmat05                    1

===== hugeshmctl =======
- [ ] hugeshmctl01                   4
- [ ] hugeshmctl02                   8
- [ ] hugeshmctl03                   3

===== hugeshmdt =======
- [ ] hugeshmdt01                    1

===== hugeshmget =======
- [ ] hugeshmget01                   1
- [ ] hugeshmget02                   4
- [ ] hugeshmget03                   1
- [ ] hugeshmget05                   1
- [ ] hugeshmget06                   1

===== icmp_rate_limit =======
- [ ] icmp_rate_limit01              1

===== in6 =======
- [ ] in6_01                         5
- [ ] in6_02                         4

===== init_module =======
- [ ] init_module01                  1
- [ ] init_module02                  1

===== inotify =======
- [×] inotify01                      7 p7
- [×] inotify02                      9 p9
- [×] inotify03                      3 p3
- [×] inotify04                      5 p5
- [ ] inotify05                      1 
- [×] inotify06                      1 p1
- [ ] inotify07                      4 TCONF: overlayfs is not configured in this kernel
- [ ] inotify08                      3 TCONF: overlayfs is not configured in this kernel
- [ ] inotify09                      1 会打崩内核，暂时藏起来
- [×] inotify10                      10 p10
- [ ] inotify11                      1 很慢，暂时藏起来
- [×] inotify12                      9 p9

===== inotify_init1 =======
- [×] inotify_init1_01               4 p4
- [×] inotify_init1_02               4 p4

===== input =======
- [ ] input01                        1
- [ ] input02                        1
- [ ] input03                        1
- [ ] input04                        1
- [ ] input05                        1
- [ ] input06                        1

===== io_cancel =======
- [ ] io_cancel01                    1

===== io_control =======
- [ ] io_control01                   24

===== io_destroy =======
- [ ] io_destroy02                   1

===== io_getevents =======
- [ ] io_getevents01                 1

===== io_setup =======
- [ ] io_setup02                     5

===== io_submit =======
- [ ] io_submit02                    2
- [ ] io_submit03                    7
- [ ] io_submit04                    1

===== io_uring =======
- [ ] io_uring01                     6
- [ ] io_uring02                     1
- [ ] io_uring03                     7

===== ioctl =======
- [ ] ioctl01                        9
- [ ] ioctl02                        1
- [ ] ioctl03                        8
- [ ] ioctl04                        4
- [ ] ioctl05                        3
- [ ] ioctl06                        9
- [ ] ioctl07                        1
- [ ] ioctl08                        1
- [ ] ioctl09                        8
- [ ] ioctl10                        1
- [ ] ioctl_ns01                     2
- [ ] ioctl_ns02                     1
- [ ] ioctl_ns03                     1
- [ ] ioctl_ns04                     1
- [ ] ioctl_ns05                     2
- [ ] ioctl_ns06                     1
- [ ] ioctl_ns07                     4
- [ ] ioctl_sg01                     1

===== ioctl_ficlone =======
- [ ] ioctl_ficlone01                1   不存在
- [ ] ioctl_ficlone02                10  不存在
- [ ] ioctl_ficlone03                1    不存在
- [ ] ioctl_ficlone04                600 不存在

===== ioctl_ficlonerange =======
- [ ] ioctl_ficlonerange01           1 不存在
- [ ] ioctl_ficlonerange02           1 不存在

===== ioctl_fiemap =======
- [ ] ioctl_fiemap01                 57 不存在

===== ioctl_getlbmd =======
- [ ] ioctl_getlbmd01                1

===== ioctl_loop =======
- [ ] ioctl_loop01                   1
- [ ] ioctl_loop02                   16
- [ ] ioctl_loop03                   1
- [ ] ioctl_loop04                   3
- [ ] ioctl_loop05                   8
- [ ] ioctl_loop06                   6
- [ ] ioctl_loop07                   12

===== ioctl_pidfd =======
- [ ] ioctl_pidfd01                  1
- [ ] ioctl_pidfd02                  1
- [ ] ioctl_pidfd03                  1
- [ ] ioctl_pidfd04                  1
- [ ] ioctl_pidfd05                  1
- [ ] ioctl_pidfd06                  1

===== ioperm =======
- [ ] ioperm01                       2
- [ ] ioperm02                       2

===== iopl =======
- [ ] iopl01                         5
- [ ] iopl02                         2

===== ioprio_get =======
- [ ] ioprio_get01                   1

===== ioprio_set =======
- [ ] ioprio_set01                   2
- [ ] ioprio_set02                   3
- [ ] ioprio_set03                   3

===== irqbalance =======
- [ ] irqbalance01                   1

===== kallsyms =======
- [ ] kallsyms                       1

===== kcmp =======
- [ ] kcmp01                         5
- [ ] kcmp02                         6
- [ ] kcmp03                         4

===== keyctl =======
- [ ] keyctl01                       2
- [ ] keyctl02                       1
- [ ] keyctl03                       1
- [ ] keyctl04                       1
- [ ] keyctl05                       3
- [ ] keyctl06                       1
- [ ] keyctl07                       2
- [ ] keyctl08                       1
- [ ] keyctl09                       1

===== kill =======
- [ ] kill03                         3
- [ ] kill05                         1
- [ ] kill06                         1
- [ ] kill08                         1
- [ ] kill11                         24
- [ ] kill13                         1

===== kmsg =======
- [ ] kmsg01                         4

===== ksm =======
- [ ] ksm01                          42
- [ ] ksm03                          42
- [ ] ksm05                          1
- [ ] ksm07                          1

===== kvm_pagefault =======
- [ ] kvm_pagefault01                1

===== kvm_svm =======
- [ ] kvm_svm01                      1
- [ ] kvm_svm02                      1
- [ ] kvm_svm03                      2
- [ ] kvm_svm04                      1

===== kvm_vmx =======
- [ ] kvm_vmx01                      2
- [ ] kvm_vmx02                      2

===== landlock =======
- [ ] landlock01                     6 不存在
- [ ] landlock02                     8 不存在
- [ ] landlock03                     5 不存在
- [ ] landlock04                     726 不存在
- [ ] landlock05                     4 不存在
- [ ] landlock06                     4 不存在 
- [ ] landlock07                     2 不存在 
- [ ] landlock08                     2 不存在 
- [ ] landlock09                     3 不存在
- [ ] landlock10                     3 不存在

===== lchown =======
- [ ] lchown01                       12
- [ ] lchown01_16                    12
- [ ] lchown02                       8
- [ ] lchown02_16                    8

===== lftest =======
- [ ] lftest                         1

===== lgetxattr =======
- [×] lgetxattr01                    2 p2
- [×] lgetxattr02                    3 p3

===== link =======
- [ ] link02                         2
- [ ] link04                         14
- [ ] link05                         1
- [ ] link08                         4

===== listmount =======
- [ ] listmount01                    1
- [ ] listmount02                    1
- [ ] listmount03                    1
- [ ] listmount04                    1

===== listxattr =======
- [×] listxattr01                    1 p1
- [×] listxattr02                    4 p4
- [×] listxattr03                    2 p2

===== llistxattr =======
- [×] llistxattr01                   1 p1
- [×] llistxattr02                   4 p4
- [×] llistxattr03                   2 p2

===== llseek =======
- [ ] llseek01                       5
- [ ] llseek02                       2
- [ ] llseek03                       18

===== lremovexattr =======
- [×] lremovexattr01                 5 p5

===== lseek =======
- [ ] lseek01                        4
- [ ] lseek02                        15
- [ ] lseek07                        2
- [ ] lseek11                        15

===== lsm_get_self_attr =======
- [ ] lsm_get_self_attr01            1
- [ ] lsm_get_self_attr02            1
- [ ] lsm_get_self_attr03            1

===== lsm_list_modules =======
- [ ] lsm_list_modules01             1
- [ ] lsm_list_modules02             1

===== lsm_set_self_attr =======
- [ ] lsm_set_self_attr01            1

===== lstat =======
- [ ] lstat01                        1
- [ ] lstat01_64                     1
- [ ] lstat02                        6
- [ ] lstat02_64                     6
- [ ] lstat03                        12
- [ ] lstat03_64                     12

===== madvise =======
- [×] madvise01                      20
- [×] madvise02                      13     p4f8s1
- [ ] madvise03                      1
- [×] madvise05                      1      p1
- [ ] madvise06                      3
- [ ] madvise07                      1
- [ ] madvise08                      2
- [ ] madvise09                      3
- [×] madvise10                      12     p9f2s1
- [ ] madvise11                      1
- [ ] madvise12                      1

===== mallinfo =======
- [ ] mallinfo01                     2
- [ ] mallinfo02                     2
- [ ] mallinfo2_01                   1

===== mallocstress =======
- [ ] mallocstress                   1

===== mallopt =======
- [ ] mallopt01                      5

===== max_map_count =======
- [ ] max_map_count                  1

===== meltdown =======
- [ ] meltdown                       1

===== membarrier =======
- [ ] membarrier01                   12

===== memcg_test =======
- [ ] memcg_test_3                   1

===== memcmp =======
- [ ] memcmp01                       2

===== memcontrol =======
- [ ] memcontrol01                   2
- [ ] memcontrol02                   45
- [ ] memcontrol03                   21
- [ ] memcontrol04                   36

===== memcpy =======
- [ ] memcpy01                       2

===== memfd_create =======
- [×] memfd_create01                 157    p131f7
- [ ] memfd_create02                 14
- [ ] memfd_create03                 3
- [ ] memfd_create04                 9

===== memset =======
- [ ] memset01                       1

===== mesgq_nstest =======
- [ ] mesgq_nstest                   1

===== mincore =======
- [ ] mincore02                      2
- [ ] mincore03                      2
- [ ] mincore04                      1

===== mkdir =======
- [ ] mkdir02                        1
- [ ] mkdir03                        11
- [ ] mkdir04                        1
- [ ] mkdir05                        1
- [ ] mkdir09                        30
- [ ] mkdirat02                      4

===== mknod =======
- [ ] mknod01                        7
- [ ] mknod02                        2
- [ ] mknod03                        1
- [ ] mknod04                        2
- [ ] mknod05                        2
- [ ] mknod06                        6
- [ ] mknod07                        6
- [ ] mknod08                        2
- [ ] mknod09                        1

10
===== mlock =======
- [×] mlock01                        4
- [×] mlock02                        3  p2f1
- [×] mlock03                        1
- [×] mlock04                        1
- [×] mlock05                        2
- [ ] mlock201                       8
- [ ] mlock202                       4
- [ ] mlock203                       1

===== mmap =======
- [×] mmap01                         1
- [×] mmap02                         1
- [×] mmap03                         2
- [×] mmap04                         14
- [×] mmap05                         1
- [×] mmap08                         1
- [×] mmap09                         3
- [×] mmap10                         3
- [×] mmap12                         1
- [×] mmap13                         1
- [×] mmap14                         1
- [×] mmap15                         1
- [ ] mmap16                         10
- [×] mmap17                         1
- [×] mmap18                         4 p2f2
- [×] mmap19                         1
- [×] mmap20                         1
- [×] mmap21                         1
- [×] mmap22                         1

===== mmapstress =======
- [×] mmapstress01                   1
- [×] mmapstress04                   1


===== mount =======
- [×] mount01                        10 p10
- [×] mount02                        12 p12
- [×] mount03                        76 p66 f6
- [×] mount04                        1  p1b2
- [×] mount05                        8 p8
- [×] mount06                        8 p8
- [×] mount07                        56 p56
- [ ] mount08                        1 不存在
- [ ] mountns01                      2
- [ ] mountns02                      2
- [ ] mountns03                      2
- [ ] mountns04                      1

===== mount_setattr =======
- [×] mount_setattr01                65 p30
- [ ] mount_setattr02                8  未找到

===== move_mount =======
- [×] move_mount01                   30 p30
- [×] move_mount02                   25 p25
- [ ] move_mount03                   1  未找到

===== move_pages =======
- [ ] move_pages04                   1

===== mprotect =======
- [ ] mprotect05                     1

===== mq_notify =======
- [ ] mq_notify01                    7
- [ ] mq_notify02                    2
- [ ] mq_notify03                    7

===== mq_open =======
- [ ] mq_open01                      10

===== mq_timedreceive =======
- [ ] mq_timedreceive01              30

===== mq_timedsend =======
- [ ] mq_timedsend01                 34

===== mq_unlink =======
- [ ] mq_unlink01                    4

===== mqns =======
- [ ] mqns_01                        1
- [ ] mqns_02                        1
- [ ] mqns_03                        1
- [ ] mqns_04                        1

===== mremap =======
- [ ] mremap06                       3
- [ ] mremap07                       3

===== mseal =======
- [ ] mseal01                        6
- [ ] mseal02                        1

===== msg_comm =======
- [ ] msg_comm                       1

===== msgctl =======
- [ ] msgctl01                       14
- [ ] msgctl02                       2
- [ ] msgctl03                       2
- [ ] msgctl04                       14
- [ ] msgctl06                       10
- [ ] msgctl12                       3

===== msgget =======
- [ ] msgget01                       1
- [ ] msgget02                       6
- [ ] msgget03                       1
- [ ] msgget04                       2
- [ ] msgget05                       1

===== msgrcv =======
- [ ] msgrcv01                       4
- [ ] msgrcv02                       8
- [ ] msgrcv03                       3
- [ ] msgrcv05                       1
- [ ] msgrcv06                       1
- [ ] msgrcv07                       15
- [ ] msgrcv08                       1

===== msgsnd =======
- [ ] msgsnd01                       3
- [ ] msgsnd02                       6
- [ ] msgsnd05                       2
- [ ] msgsnd06                       1

===== msgstress =======
- [ ] msgstress01                    1

===== msync =======
- [ ] msync04                        4

===== mtest =======
- [ ] mtest01                        1

===== munlock =======
- [ ] munlock01                      4
- [ ] munlock02                      1
- [ ] munlockall01                   2

===== munmap =======
- [ ] munmap01                       2
- [ ] munmap03                       3
- [ ] munmap04                       1

===== name_to_handle_at =======
- [ ] name_to_handle_at01            27
- [ ] name_to_handle_at02            9
- [ ] name_to_handle_at03            1

===== nanosleep =======
- [ ] nanosleep01                    7
- [ ] nanosleep02                    2
- [ ] nanosleep04                    3

===== netns_netlink =======
- [ ] netns_netlink                  1

===== newuname =======
- [ ] newuname01                     6

===== nfs05_make_tree =======
- [ ] nfs05_make_tree                1

===== nft =======
- [ ] nft02                          1

===== nice =======
- [ ] nice01                         3
- [ ] nice02                         1
- [ ] nice03                         1
- [ ] nice04                         1
- [ ] nice05                         1

===== oom =======
- [ ] oom01                          5

===== open =======
- [×] open01                         2 p2
- [×] open02                         2 p2
- [×] open03                         1 p1
- [×] open04                         1 p1
- [×] open06                         1 p1
- [×] open07                         5 p5
- [×] open08                         6 p6
- [×] open09                         2 p2
- [×] open10                         9 p9
- [×] open11                         28 p28
- [ ] open12                         20 no summary
- [ ] open13                         14 no summary
- [ ] open14                         25 no summary
- [ ] open15                         2 不存在
- [×] openat01                       5 p5
- [ ] openat02                       44 broken
- [×] openat04                       10 p12
- [×] openat201                      16 p16
- [×] openat202                      9 p9
- [×] openat203                      9 p9

===== open_by_handle_at =======
- [ ] open_by_handle_at01            9
- [ ] open_by_handle_at02            7

===== open_tree =======
- [×] open_tree01                    10 p10
- [×] open_tree02                    15 p15

===== overcommit_memory =======
- [ ] overcommit_memory              8

===== pathconf =======
- [ ] pathconf01                     17
- [ ] pathconf02                     6

===== pause =======
- [ ] pause01                        22
- [ ] pause02                        1

===== pcrypt_aead =======
- [ ] pcrypt_aead01                  1

===== perf_event_open =======
- [ ] perf_event_open02              1
- [ ] perf_event_open03              1

===== personality =======
- [ ] personality01                  19
- [ ] personality02                  1

===== pidfd_getfd =======
- [ ] pidfd_getfd01                  1
- [ ] pidfd_getfd02                  5

===== pidfd_open =======
- [ ] pidfd_open01                   1
- [ ] pidfd_open02                   3
- [ ] pidfd_open03                   1
- [ ] pidfd_open04                   3

===== pidfd_send_signal =======
- [ ] pidfd_send_signal01            2
- [ ] pidfd_send_signal02            4
- [ ] pidfd_send_signal03            1

===== pidns =======
- [ ] pidns01                        2
- [ ] pidns02                        4
- [ ] pidns03                        1
- [ ] pidns04                        2
- [ ] pidns05                        12
- [ ] pidns06                        3
- [ ] pidns10                        3
- [ ] pidns12                        3
- [ ] pidns13                        1
- [ ] pidns16                        4
- [ ] pidns17                        22
- [ ] pidns20                        1
- [ ] pidns30                        6
- [ ] pidns31                        6
- [ ] pidns32                        2

===== pipe =======
- [×] pipe01                         1 p1
- [×] pipe02                         1 p1
- [×] pipe03                         2 p2
- [×] pipe06                         1 p1
- [ ] pipe07                         2
- [×] pipe08                         1 p1
- [×] pipe10                         1 p1
- [×] pipe11                         70 p70
- [×] pipe12                         6 p6
- [×] pipe13                         4 p4
- [×] pipe14                         1 p1
- [ ] pipe15                         1
- [ ] pipe2_01                       7
- [ ] pipe2_02                       1
- [ ] pipe2_04                       2

===== pivot_root =======
- [ ] pivot_root01                   5

===== pkey =======
- [ ] pkey01                         1

===== poll =======
- [ ] poll01                         6
- [ ] poll02                         7
- [ ] poll03                         3
- [ ] poll04                         3

===== posix_fadvise =======
- [ ] posix_fadvise01                6
- [ ] posix_fadvise01_64             6
- [ ] posix_fadvise02                6
- [ ] posix_fadvise02_64             6
- [ ] posix_fadvise03                32
- [ ] posix_fadvise03_64             32
- [ ] posix_fadvise04                6
- [ ] posix_fadvise04_64             6

===== ppoll =======
- [ ] ppoll01                        20

===== prctl =======
- [ ] prctl01                        2
- [ ] prctl02                        18
- [ ] prctl03                        6
- [ ] prctl05                        8
- [ ] prctl06                        15
- [ ] prctl07                        1
- [ ] prctl08                        14
- [ ] prctl09                        7
- [ ] prctl10                        5

===== pread =======
- [ ] pread01                        1
- [ ] pread01_64                     1
- [ ] pread02                        3
- [ ] pread02_64                     3
- [ ] preadv01                       3
- [ ] preadv01_64                    3
- [ ] preadv02                       8
- [ ] preadv02_64                    8
- [ ] preadv03                       12
- [ ] preadv03_64                    12
- [ ] preadv201                      6
- [ ] preadv201_64                   6
- [ ] preadv202                      8
- [ ] preadv202_64                   8
- [ ] preadv203                      5
- [ ] preadv203_64                   5

===== proc_sched_rt =======
- [ ] proc_sched_rt01                5

===== process_madvise =======
- [ ] process_madvise01              1

===== process_vm =======
- [ ] process_vm01                   25

===== process_vm_readv =======
- [ ] process_vm_readv02             1
- [ ] process_vm_readv03             32

===== process_vm_writev =======
- [ ] process_vm_writev02            2

===== pselect =======
- [ ] pselect01                      7
- [ ] pselect01_64                   7
- [ ] pselect02                      3
- [ ] pselect02_64                   3
- [ ] pselect03                      1
- [ ] pselect03_64                   1

===== pt_test =======
- [ ] pt_test                        1

===== ptem =======
- [ ] ptem01                         9
- [ ] ptem02                         17
- [ ] ptem03                         2
- [ ] ptem04                         10
- [ ] ptem05                         10
- [ ] ptem06                         2

===== ptrace =======
- [ ] ptrace01                       4
- [ ] ptrace02                       1
- [ ] ptrace03                       2
- [ ] ptrace05                       2
- [ ] ptrace06                       48
- [ ] ptrace07                       1
- [ ] ptrace08                       3
- [ ] ptrace09                       1
- [ ] ptrace10                       1
- [ ] ptrace11                       1

===== pty =======
- [ ] pty01                          2
- [ ] pty02                          1
- [ ] pty04                          2
- [ ] pty05                          1
- [ ] pty08                          2
- [ ] pty09                          1

===== pwrite =======
- [ ] pwrite01                       1
- [ ] pwrite01_64                    1
- [ ] pwrite02                       5
- [ ] pwrite02_64                    5
- [ ] pwrite03                       1
- [ ] pwrite03_64                    1
- [ ] pwrite04                       1
- [ ] pwrite04_64                    1
- [ ] pwritev01                      3
- [ ] pwritev01_64                   3
- [ ] pwritev02                      7
- [ ] pwritev02_64                   7
- [ ] pwritev03                      12
- [ ] pwritev03_64                   12
- [ ] pwritev201                     6
- [ ] pwritev201_64                  6
- [ ] pwritev202                     7
- [ ] pwritev202_64                  7

===== quotactl =======
- [ ] quotactl01                     1
- [ ] quotactl04                     1
- [ ] quotactl06                     1
- [ ] quotactl08                     1
- [ ] quotactl09                     1

===== read =======
- [ ] read01                         1
- [ ] read02                         5
- [ ] read03                         1
- [ ] read04                         1
- [ ] readdir01                      5
- [ ] readdir21                      5
- [ ] readv01                        10
- [ ] readv02                        5

===== read_all =======
- [ ] read_all                       1

===== readahead =======
- [ ] readahead01                    23
- [ ] readahead02                    12

===== readlink =======
- [ ] readlink01                     2
- [ ] readlink03                     8
- [ ] readlinkat01                   12
- [ ] readlinkat02                   6

===== realpath =======
- [ ] realpath01                     1

===== reboot =======
- [ ] reboot01                       2
- [ ] reboot02                       2

===== recvmmsg =======
- [ ] recvmmsg01                     10

===== recvmsg =======
- [ ] recvmsg01                      10
- [ ] recvmsg02                      1
- [ ] recvmsg03                      1

===== remap_file_pages =======
- [ ] remap_file_pages02             4

===== rename =======
- [×] rename01                       40 p40
- [×] rename03                       40 p22f18
- [×] rename04                       5  p5
- [×] rename05                       5  p5
- [×] rename06                       5  p5
- [×] rename07                       5  p5
- [×] rename08                       10 p10
- [×] rename09                       1  p1
- [×] rename10                       10 p10
- [×] rename12                       4  p4
- [×] rename13                       12 p12
- [ ] rename15                       75 不存在

===== request_key =======
- [ ] request_key01                  2
- [ ] request_key02                  3
- [ ] request_key03                  4
- [ ] request_key04                  1
- [ ] request_key05                  1
- [ ] request_key06                  4

===== rmdir =======
- [ ] rmdir01                        1
- [ ] rmdir02                        9
- [ ] rmdir03                        2

===== rt_sigqueueinfo =======
- [ ] rt_sigqueueinfo01              2
- [ ] rt_sigqueueinfo02              3

===== rt_sigsuspend =======
- [ ] rt_sigsuspend01                2

===== rt_sigtimedwait =======
- [ ] rt_sigtimedwait01              19

===== rt_tgsigqueueinfo =======
- [ ] rt_tgsigqueueinfo01            3

===== rtc =======
- [ ] rtc02                          1

2
===== sbrk =======
- [×] sbrk01                         3  p1f2
- [×] sbrk02                         1

===== sched_football =======
- [ ] sched_football                 1

===== sched_get_priority_max =======
- [ ] sched_get_priority_max01       6
- [ ] sched_get_priority_max02       1

===== sched_get_priority_min =======
- [ ] sched_get_priority_min01       6
- [ ] sched_get_priority_min02       1

===== sched_getaffinity =======
- [ ] sched_getaffinity01            4

===== sched_getattr =======
- [ ] sched_getattr02                4

===== sched_getparam =======
- [ ] sched_getparam01               4
- [ ] sched_getparam03               6

===== sched_getscheduler =======
- [ ] sched_getscheduler01           2
- [ ] sched_getscheduler02           2

===== sched_rr_get_interval =======
- [ ] sched_rr_get_interval01        2
- [ ] sched_rr_get_interval02        2
- [ ] sched_rr_get_interval03        2

===== sched_setaffinity =======
- [ ] sched_setaffinity01            4

===== sched_setparam =======
- [ ] sched_setparam01               2
- [ ] sched_setparam02               2
- [ ] sched_setparam03               2
- [ ] sched_setparam04               8
- [ ] sched_setparam05               2

===== sched_setscheduler =======
- [ ] sched_setscheduler01           8
- [ ] sched_setscheduler02           2
- [ ] sched_setscheduler03           6
- [ ] sched_setscheduler04           8

===== sctp_big_chunk =======
- [ ] sctp_big_chunk                 1

===== seccomp =======
- [ ] seccomp01                      1

===== select =======
- [ ] select01                       26
- [ ] select02                       23
- [ ] select03                       40
- [ ] select04                       8

===== sem_comm =======
- [ ] sem_comm                       1

===== sem_nstest =======
- [ ] sem_nstest                     1

===== semctl =======
- [ ] semctl01                       13
- [ ] semctl02                       1
- [ ] semctl03                       8
- [ ] semctl04                       2
- [ ] semctl05                       3
- [ ] semctl07                       16
- [ ] semctl09                       16

===== semget =======
- [ ] semget01                       3
- [ ] semget02                       6
- [ ] semget05                       1

===== semop =======
- [ ] semop01                        4
- [ ] semop02                        26
- [ ] semop03                        8
- [ ] semop04                        1

===== semtest_2ns =======
- [ ] semtest_2ns                    2

===== send =======
- [ ] send02                         4
- [ ] sendmsg03                      1
- [ ] sendto02                       1
- [ ] sendto03                       2

===== sendfile =======
- [ ] sendfile02                     2
- [ ] sendfile02_64                  2
- [ ] sendfile03                     4
- [ ] sendfile03_64                  4
- [ ] sendfile04                     5
- [ ] sendfile04_64                  5
- [ ] sendfile05                     1
- [ ] sendfile05_64                  1
- [ ] sendfile06                     1
- [ ] sendfile06_64                  1
- [ ] sendfile07                     1
- [ ] sendfile07_64                  1
- [ ] sendfile08                     1
- [ ] sendfile08_64                  1
- [ ] sendfile09                     2
- [ ] sendfile09_64                  2

===== sendmmsg =======
- [ ] sendmmsg01                     4
- [ ] sendmmsg02                     4

===== set_tid_address =======
- [ ] set_tid_address01              1

===== setdomainname =======
- [ ] setdomainname01                2
- [ ] setdomainname02                6
- [ ] setdomainname03                2

===== setegid =======
- [ ] setegid01                      4
- [ ] setegid02                      1

===== setfsgid =======
- [ ] setfsgid01                     2
- [ ] setfsgid01_16                  1
- [ ] setfsgid02                     4
- [ ] setfsgid02_16                  1

===== setfsuid =======
- [ ] setfsuid01                     2
- [ ] setfsuid01_16                  1
- [ ] setfsuid02                     1
- [ ] setfsuid02_16                  1
- [ ] setfsuid03                     1
- [ ] setfsuid03_16                  1

===== setgid =======
- [ ] setgid01                       1
- [ ] setgid01_16                    1
- [ ] setgid02                       1
- [ ] setgid02_16                    1
- [ ] setgid03                       2
- [ ] setgid03_16                    1

===== setgroups =======
- [ ] setgroups01                    1
- [ ] setgroups01_16                 1
- [ ] setgroups02                    3
- [ ] setgroups02_16                 1
- [ ] setgroups03                    3
- [ ] setgroups03_16                 1

===== sethostname =======
- [ ] sethostname01                  2
- [ ] sethostname02                  6
- [ ] sethostname03                  2

===== setitimer =======
- [ ] setitimer01                    18
- [ ] setitimer02                    3

===== setns =======
- [ ] setns01                        25
- [ ] setns02                        8

===== setpgid =======
- [ ] setpgid01                      7
- [ ] setpgid02                      3
- [ ] setpgid03                      3

===== setpgrp =======
- [ ] setpgrp02                      2

===== setpriority =======
- [ ] setpriority01                  3
- [ ] setpriority02                  7

===== setregid =======
- [ ] setregid01                     5
- [ ] setregid01_16                  1
- [ ] setregid02                     12
- [ ] setregid02_16                  1
- [ ] setregid03                     22
- [ ] setregid03_16                  1
- [ ] setregid04                     9
- [ ] setregid04_16                  1

===== setresgid =======
- [ ] setresgid01                    20
- [ ] setresgid01_16                 1
- [ ] setresgid02                    6
- [ ] setresgid02_16                 1
- [ ] setresgid03                    4
- [ ] setresgid03_16                 1
- [ ] setresgid04                    2
- [ ] setresgid04_16                 1

===== setresuid =======
- [ ] setresuid01                    9
- [ ] setresuid01_16                 1
- [ ] setresuid02                    4
- [ ] setresuid02_16                 1
- [ ] setresuid03                    3
- [ ] setresuid03_16                 1
- [ ] setresuid04                    3
- [ ] setresuid04_16                 1
- [ ] setresuid05                    2
- [ ] setresuid05_16                 1

===== setreuid =======
- [ ] setreuid01                     7
- [ ] setreuid01_16                  1
- [ ] setreuid02                     7
- [ ] setreuid02_16                  1
- [ ] setreuid03                     14
- [ ] setreuid03_16                  1
- [ ] setreuid04                     3
- [ ] setreuid04_16                  1
- [ ] setreuid05                     14
- [ ] setreuid05_16                  1
- [ ] setreuid06                     3
- [ ] setreuid06_16                  1
- [ ] setreuid07                     3
- [ ] setreuid07_16                  1

===== setrlimit =======
- [ ] setrlimit02                    2
- [ ] setrlimit03                    2
- [ ] setrlimit04                    1
- [ ] setrlimit05                    1
- [ ] setrlimit06                    2

===== setsockopt =======
- [ ] setsockopt01                   8
- [ ] setsockopt02                   2
- [ ] setsockopt03                   1
- [ ] setsockopt04                   1
- [ ] setsockopt05                   1
- [ ] setsockopt08                   1
- [ ] setsockopt09                   1
- [ ] setsockopt10                   1

===== settimeofday =======
- [ ] settimeofday01                 1
- [ ] settimeofday02                 3

===== setuid =======
- [ ] setuid01                       1
- [ ] setuid01_16                    1
- [ ] setuid03                       1
- [ ] setuid03_16                    1
- [ ] setuid04                       2
- [ ] setuid04_16                    1

===== setxattr =======
- [×] setxattr01                     31 p40f5
- [×] setxattr02                     7 p7
- [×] setxattr03                     2 p2

===== shell_test =======
- [ ] shell_test01                   1
- [ ] shell_test02                   1
- [ ] shell_test03                   1
- [ ] shell_test04                   1
- [ ] shell_test05                   1
- [ ] shell_test06                   1

===== shm_comm =======
- [ ] shm_comm                       1

===== shmat =======
- [ ] shmat01                        4
- [ ] shmat02                        3
- [ ] shmat03                        1
- [ ] shmat04                        1

===== shmctl =======
- [ ] shmctl01                       12
- [ ] shmctl02                       1
- [ ] shmctl03                       4
- [ ] shmctl04                       12
- [ ] shmctl05                       1
- [ ] shmctl07                       4
- [ ] shmctl08                       6

===== shmdt =======
- [ ] shmdt01                        2
- [ ] shmdt02                        2

===== shmem_2nstest =======
- [ ] shmem_2nstest                  1

===== shmget =======
- [ ] shmget02                       8
- [ ] shmget03                       1
- [ ] shmget04                       3
- [ ] shmget05                       2
- [ ] shmget06                       1

===== shmnstest =======
- [ ] shmnstest                      1

===== shmt =======
- [ ] shmt02                         2
- [ ] shmt03                         1
- [ ] shmt04                         2
- [ ] shmt05                         1
- [ ] shmt07                         1
- [ ] shmt08                         1
- [ ] shmt09                         2
- [ ] shmt10                         1

===== shutdown =======
- [ ] shutdown01                     6
- [ ] shutdown02                     4

===== sigaltstack =======
- [ ] sigaltstack02                  2

===== sighold =======
- [ ] sighold02                      1

===== signal =======
- [ ] signal01                       6
- [ ] signal02                       3
- [ ] signal03                       30
- [ ] signal04                       28
- [ ] signal05                       30
- [ ] signalfd01                     7
- [ ] signalfd02                     3

===== sigpending =======
- [ ] sigpending02                   5

===== sigsuspend =======
- [ ] sigsuspend01                   1
- [ ] sigsuspend02                   1

===== sigtimedwait =======
- [ ] sigtimedwait01                 11

===== sigwait =======
- [ ] sigwait01                      3

===== sigwaitinfo =======
- [ ] sigwaitinfo01                  9

===== snd_seq =======
- [ ] snd_seq01                      1

===== snd_timer =======
- [ ] snd_timer01                    1

===== socket =======
- [ ] socket01                       9
- [ ] socket02                       4

===== socketcall =======
- [ ] socketcall01                   1
- [ ] socketcall02                   1
- [ ] socketcall03                   1

===== socketpair =======
- [ ] socketpair01                   10
- [ ] socketpair02                   4

===== splice =======
- [×] splice01                       1 p1
- [ ] splice02                       1 会死锁
- [×] splice03                       7 p7
- [×] splice04                       1 p1
- [ ] splice05                       1
- [ ] splice06                       4
- [×] splice07                       591 p615
- [ ] splice08                       1
- [×] splice09                       1 p2

===== squashfs =======
- [ ] squashfs01                     1

===== stack_clash =======
- [ ] stack_clash                    1

===== starvation =======
- [ ] starvation                     1

===== stat =======
- [ ] stat01                         12
- [ ] stat01_64                      12
- [ ] stat02                         2
- [ ] stat02_64                      2
- [ ] stat03                         6
- [ ] stat03_64                      6
- [ ] stat04                         9
- [ ] stat04_64                      9
- [ ] statfs01                       5
- [ ] statfs01_64                    5
- [ ] statfs02                       6
- [ ] statfs02_64                    6
- [ ] statfs03                       1
- [ ] statfs03_64                    1
- [ ] statvfs01                      15
- [ ] statvfs02                      5
- [ ] statx01                        12
- [ ] statx02                        5
- [ ] statx03                        7
- [ ] statx04                        14
- [ ] statx05                        1
- [ ] statx06                        4
- [ ] statx07                        1
- [ ] statx08                        40
- [ ] statx09                        1
- [ ] statx10                        5
- [ ] statx11                        1
- [ ] statx12                        20

===== statmount =======
- [ ] statmount01                    1
- [ ] statmount02                    1
- [ ] statmount03                    1
- [ ] statmount04                    1
- [ ] statmount05                    1
- [ ] statmount06                    1
- [ ] statmount07                    1
- [ ] statmount08                    1
- [ ] statmount09                    1

===== stime =======
- [ ] stime01                        3
- [ ] stime02                        3

===== stream =======
- [ ] stream01                       4
- [ ] stream02                       3
- [ ] stream03                       7
- [ ] stream04                       3

===== swapoff =======
- [ ] swapoff01                      5
- [ ] swapoff02                      13

===== swapon =======
- [ ] swapon01                       5
- [ ] swapon02                       17
- [ ] swapon03                       5

===== swapping =======
- [ ] swapping01                     1

===== symlink =======
- [ ] symlink02                      1
- [ ] symlink04                      2

===== sync =======
- [ ] sync01                         4
- [ ] syncfs01                       4

5
===== sync_file_range =======
- [×] sync_file_range01              5 p5
- [ ] sync_file_range02              12 同样是超大文件

===== syscall =======
- [ ] syscall01                      3

===== sysctl =======
- [ ] sysctl01                       1
- [ ] sysctl03                       1
- [ ] sysctl04                       1

===== sysfs =======
- [ ] sysfs01                        1
- [ ] sysfs02                        1
- [ ] sysfs03                        1
- [ ] sysfs04                        1
- [ ] sysfs05                        4

===== sysinfo =======
- [ ] sysinfo01                      9
- [ ] sysinfo02                      1
- [ ] sysinfo03                      6

===== syslog =======
- [ ] syslog11                       8
- [ ] syslog12                       6

===== tcindex =======
- [ ] tcindex01                      1

===== tee =======
- [ ] tee01                          1
- [ ] tee02                          3

===== test_1_to_1_initmsg_connect =======
- [ ] test_1_to_1_initmsg_connect    2

===== test_ioctl =======
- [ ] test_ioctl                     2

===== tgkill =======
- [ ] tgkill01                       1
- [ ] tgkill02                       1
- [ ] tgkill03                       6

===== thp =======
- [ ] thp01                          1
- [ ] thp02                          1
- [ ] thp03                          1
- [ ] thp04                          1

===== time =======
- [ ] time01                         2
- [ ] timens01                       8
- [ ] timerfd01                      12
- [ ] timerfd02                      6
- [ ] timerfd04                      4
- [ ] times01                        1
- [ ] times03                        12

===== timer_create =======
- [ ] timer_create01                 40
- [ ] timer_create02                 5
- [ ] timer_create03                 1

===== timer_delete =======
- [ ] timer_delete01                 8
- [ ] timer_delete02                 1

===== timer_getoverrun =======
- [ ] timer_getoverrun01             2

===== timer_gettime =======
- [ ] timer_gettime01                3

===== timer_settime =======
- [ ] timer_settime01                32
- [ ] timer_settime02                48
- [ ] timer_settime03                1

===== timerfd_create =======
- [ ] timerfd_create01               2

===== timerfd_gettime =======
- [ ] timerfd_gettime01              3

===== timerfd_settime =======
- [ ] timerfd_settime01              4
- [ ] timerfd_settime02              1

===== tkill =======
- [ ] tkill01                        2
- [ ] tkill02                        2

===== truncate =======
- [ ] truncate02                     2
- [ ] truncate02_64                  2
- [ ] truncate03                     8
- [ ] truncate03_64                  8

===== uevent =======
- [ ] uevent01                       2
- [ ] uevent03                       1

===== ulimit =======
- [ ] ulimit01                       3

===== umask =======
- [ ] umask01                        1

===== umip_basic_test =======
- [ ] umip_basic_test                5

===== umount =======
- [ ] umount01                       1
- [ ] umount02                       5
- [ ] umount03                       1
- [ ] umount2_02                     7

===== uname =======
- [ ] uname01                        2
- [ ] uname02                        1
- [ ] uname04                        2

===== unlink =======
- [ ] unlink05                       2
- [ ] unlink07                       6
- [ ] unlink08                       4
- [ ] unlink09                       7
- [ ] unlink10                       1
- [ ] unlinkat01                     7

===== unshare =======
- [ ] unshare01                      3
- [ ] unshare02                      2
- [ ] unshare03                      1
- [ ] unshare04                      3
- [ ] unshare05                      1

===== userfaultfd =======
- [ ] userfaultfd01                  2
- [ ] userfaultfd02                  1
- [ ] userfaultfd03                  1
- [ ] userfaultfd04                  1
- [ ] userfaultfd05                  1
- [ ] userfaultfd06                  1

===== userns =======
- [ ] userns02                       2
- [ ] userns03                       7
- [ ] userns04                       2
- [ ] userns05                       3
- [ ] userns07                       1
- [ ] userns08                       1

===== ustat =======
- [ ] ustat01                        1
- [ ] ustat02                        2

===== utime =======
- [ ] utime01                        12
- [ ] utime02                        12
- [ ] utime03                        4
- [ ] utime04                        12
- [ ] utime05                        12
- [ ] utime06                        4
- [ ] utime07                        5
- [ ] utimes01                       7

===== utimensat =======
- [ ] utimensat01                    34

===== utsname =======
- [ ] utsname01                      1
- [ ] utsname02                      2
- [ ] utsname03                      2
- [ ] utsname04                      2

===== vfork =======
- [ ] vfork01                        8
- [ ] vfork02                        1

===== vhangup =======
- [ ] vhangup01                      1
- [ ] vhangup02                      1

===== vmsplice =======
- [ ] vmsplice01                     1
- [ ] vmsplice02                     3
- [ ] vmsplice03                     1
- [ ] vmsplice04                     2

===== vsock =======
- [ ] vsock01                        1

===== wait =======
- [x] wait01                         1  p1
- [x] wait02                         1  p1
- [x] wait401                        3  p3
- [x] wait402                        1  p1
- [x] wait403                        1  p1
- [x] waitid01                       5  p5
- [x] waitid02                       1  p1
- [x] waitid03                       1  p1
- [x] waitid04                       2  p2
- [x] waitid05                       6  p6
- [x] waitid06                       6  p6
- [x] waitid07                       5  p5
- [x] waitid08                       10 p10
- [x] waitid09                       1  p1
- [x] waitid10                       5  p1
- [x] waitid11                       5  p5
- [x] waitpid01                      128    p146
- [x] waitpid03                      2  p2
- [x] waitpid04                      4  p4
- [x] waitpid06                      1  p1
- [x] waitpid07                      1  p1
- [x] waitpid08                      1  p1
- [x] waitpid09                      4  p4
- [x] waitpid10                      1  p1
- [x] waitpid11                      1  p1
- [x] waitpid12                      1  p1
- [x] waitpid13                      1  p1

===== wqueue =======
- [ ] wqueue01                       1
- [ ] wqueue02                       1
- [ ] wqueue03                       1
- [ ] wqueue04                       1
- [ ] wqueue05                       1
- [ ] wqueue06                       1
- [ ] wqueue07                       1
- [ ] wqueue08                       1
- [ ] wqueue09                       1

===== write =======
- [ ] write01                        1
- [ ] write02                        2
- [ ] write03                        1
- [ ] write04                        1
- [ ] write05                        3
- [ ] write06                        2
- [ ] writev01                       6
- [ ] writev07                       8

===== zram =======
- [ ] zram03                         1
