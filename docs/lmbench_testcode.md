#### OS COMP TEST GROUP START lmbench-musl ####
latency measurements
Simple syscall: 5.3355 microseconds
Simple read: 17.9302 microseconds
Simple write: 17.9683 microseconds
Simple stat: 57.5439 microseconds
Simple fstat: 30.2048 microseconds
Simple open/close: 474.2158 microseconds
Select on 100 fd's: 127.4501 microseconds
Signal handler installation: 7.0246 microseconds
Signal handler overhead: 39.0703 microseconds
Protection fault: 39.2707 microseconds
Pipe latency: 674.8347 microseconds
Process fork+exit: 38313.5862 microseconds
Process fork+execve: 38909.8519 microseconds
Process fork+/bin/sh -c: 700577.0000 microseconds
File /var/tmp/XXX write bandwidth:1333 KB/sec
Pagefaults on /var/tmp/XXX: 85.9189 microseconds
0.524288 274
file system latency
0k      117     104     107
1k      49      49      114
4k      76      69      117
10k     56      41      117
Bandwidth measurements
Pipe bandwidth: 48.96 MB/sec
0.524288 179.99
0.524288 135.32
0.524288 8867.21
0.524288 50.16
context switch overhead

"size=32k ovr=89.25
2 244.74
4 236.18
8 231.75
16 220.93
24 214.96
32 268.22
64 189.24
96 124.16
#### OS COMP TEST GROUP END lmbench-musl ######## OS COMP TEST GROUP START libcbench-musl ####
b_malloc_sparse (0)
  time: 0.953100960, virt: 39756, res: 39580, dirty: 0

b_malloc_bubble (0)
  time: 0.808972720, virt: 39756, res: 39580, dirty: 0

b_malloc_tiny1 (0)
  time: 0.042882400, virt: 1008, res: 832, dirty: 0

b_malloc_tiny2 (0)
  time: 0.030749920, virt: 1008, res: 832, dirty: 0

b_malloc_big1 (0)
  time: 0.423564720, virt: 80460, res: 14224, dirty: 0

b_malloc_big2 (0)
  time: 0.313510720, virt: 80460, res: 14224, dirty: 0

b_malloc_thread_stress (0)
  time: 0.206261440, virt: 412, res: 172, dirty: 0

b_malloc_thread_local (0)
  time: 0.197074960, virt: 436, res: 212, dirty: 0

b_string_strstr ("abcdefghijklmnopqrstuvwxyz")
  time: 0.038418640, virt: 384, res: 132, dirty: 0

b_string_strstr ("azbycxdwevfugthsirjqkplomn")
  time: 0.050098880, virt: 384, res: 132, dirty: 0

b_string_strstr ("aaaaaaaaaaaaaacccccccccccc")
  time: 0.037338720, virt: 384, res: 132, dirty: 0

b_string_strstr ("aaaaaaaaaaaaaaaaaaaaaaaaac")
  time: 0.037036880, virt: 384, res: 132, dirty: 0

b_string_strstr ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaac")
  time: 0.050950240, virt: 384, res: 132, dirty: 0

b_string_memset (0)
  time: 0.047513040, virt: 384, res: 132, dirty: 0

b_string_strchr (0)
  time: 0.047307280, virt: 384, res: 132, dirty: 0

b_string_strlen (0)
  time: 0.044648320, virt: 384, res: 132, dirty: 0

b_pthread_createjoin_serial1 (0)
  time: 2.841596160, virt: 384, res: 132, dirty: 0

b_pthread_createjoin_serial2 (0)
  time: 2.875486640, virt: 384, res: 132, dirty: 0

b_pthread_create_serial1 (0)
  time: 16.185646640, virt: 50384, res: 10132, dirty: 0

b_pthread_uselesslock (0)
  time: 0.205891840, virt: 384, res: 132, dirty: 0

b_utf8_bigbuf (0)
  time: 0.130007280, virt: 384, res: 132, dirty: 0

b_utf8_onebyone (0)
  time: 0.230218880, virt: 384, res: 132, dirty: 0

b_stdio_putcgetc (0)
  time: 1.339862480, virt: 384, res: 132, dirty: 0

b_stdio_putcgetc_unlocked (0)
  time: 1.047870000, virt: 384, res: 132, dirty: 0

b_regex_compile ("(a|b|c)*d*b")
  time: 0.184212320, virt: 400, res: 148, dirty: 0

b_regex_search ("(a|b|c)*d*b")
  time: 0.157004160, virt: 400, res: 400, dirty: 0

b_regex_search ("a{25}b")
  time: 0.522352720, virt: 404, res: 404, dirty: 0

#### OS COMP TEST GROUP END libcbench-musl ####


i=1
while [ $i -le 50 ]; do
    echo "=== round $i ==="
    /bin/iozone_regression || break
    cat /proc/kairix_perf
    i=$((i + 1))
done