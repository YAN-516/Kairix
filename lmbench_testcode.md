# musl
latency measurements
Simple syscall: 12.4821 microseconds
Simple read: 19.4370 microseconds
Simple write: 19.6879 microseconds
Simple stat: 317.4150 microseconds
Simple fstat: 20.1218 microseconds
Simple open/close: 332.5369 microseconds
Select on 100 fd's: 71.4881 microseconds
Signal handler installation: 18.1932 microseconds
Signal handler overhead: 17.8200 microseconds
Protection fault: 7.8133 microseconds
Pipe latency: 167.7728 microseconds
Process fork+exit: 3258.5593 microseconds
Process fork+execve: 3419.0737 microseconds
Process fork+/bin/sh -c: 202407.8333 microseconds
File /var/tmp/XXX write bandwidth:455 KB/sec
Pagefaults on /var/tmp/XXX: 24.7904 microseconds
0.524288 151
file system latency
0k      75      65      140
1k      53      47      88
4k      50      47      84
10k     31      30      79
Bandwidth measurements
Pipe bandwidth: 18.71 MB/sec
0.524288 391.70
0.524288 288.97
0.524288 9553.26
0.524288 150.29
context switch overhead

"size=32k ovr=53.25
2 63.07
4 58.23
8 36.36
16 36.46
24 35.27
32 35.71
64 34.67
96 35.75
# glibc
LOG=ERROR能够通关


# 别人队伍的指标 musl
latency measurements
Simple syscall: 27.4238 microseconds
Simple read: 82.3686 microseconds
Simple write: 94.0697 microseconds
Simple stat: 134.4342 microseconds
Simple fstat: 59.2982 microseconds
Simple open/close: 384.0631 microseconds
Select on 100 fd's: 759.1323 microseconds
Signal handler installation: 52.1025 microseconds
Signal handler overhead: 868.5849 microseconds
Protection fault: 111.5801 microseconds
Pipe latency: 515.3678 microseconds
Process fork+exit: 1753.6386 microseconds
Process fork+execve: 1959.1157 microseconds
Process fork+/bin/sh -c: 25361.4878 microseconds
File /var/tmp/XXX write bandwidth:29198 KB/sec
Pagefaults on /var/tmp/XXX: 77.9769 microseconds
0.524288 476
file system latency
0k      366     342     747
1k      341     316     697
4k      345     299     696
10k     326     287     676
Bandwidth measurements
Pipe bandwidth: 97.93 MB/sec
0.524288 273.83
0.524288 183.13
0.524288 6408.29
0.524288 40.74
context switch overhead