# musl
latency measurements
Simple syscall: 13.6366 microseconds
Simple read: 21.0362 microseconds
Simple write: 21.2443 microseconds
Simple stat: 286.6710 microseconds
Simple fstat: 22.5329 microseconds
Simple open/close: 311.4101 microseconds
Select on 100 fd's: 77.8854 microseconds
Signal handler installation: 19.4250 microseconds
Signal handler overhead: 18.2460 microseconds
Protection fault: 8.0103 microseconds
Pipe latency: 122.3345 microseconds
Process fork+exit: 3190.5572 microseconds
Process fork+execve: 3468.2466 microseconds
Process fork+/bin/sh -c: 209103.6667 microseconds
File /var/tmp/XXX write bandwidth:498 KB/sec
Pagefaults on /var/tmp/XXX: 34.9794 microseconds
0.524288 179
file system latency
0k      58      52      116
1k      44      39      73
4k      43      40      81
10k     24      23      67
Bandwidth measurements
Pipe bandwidth: 19.54 MB/sec
0.524288 114.92
0.524288 185.21
0.524288 9267.89
0.524288 106.37
context switch overhead

"size=32k ovr=107.87
# glibc
latency measurements
Simple syscall: 13.9073 microseconds
Simple read: 20.9037 microseconds
Simple write: 19.6539 microseconds
Simple stat: 294.2486 microseconds
Simple fstat: 22.7437 microseconds
Simple open/close: 314.6132 microseconds
Select on 100 fd's: 83.6115 microseconds
Signal handler installation: 19.1074 microseconds
Signal handler overhead: 1.2270 microseconds


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