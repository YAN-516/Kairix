#### OS COMP TEST GROUP START lmbench-glibc ####
latency measurements
Simple syscall: 240.7361 microseconds
Simple read: 277.1725 microseconds
Simple write: 274.6791 microseconds
Simple stat: 775.7628 microseconds
Simple fstat: 256.6642 microseconds
Simple open/close: 1379.7385 microseconds
Select on 100 fd's: 399.5037 microseconds
Signal handler installation: 244.6709 microseconds
Pipe latency: 1271.0720 microseconds
[kernel] Panicked at /workspace/polyhal/polyhal/src/pagetable/mod.rs:209 called Option::unwrap() on a None value
#### OS COMP TEST GROUP START lmbench-glibc ####
latency measurements
Simple syscall: 35.9749 microseconds
Simple read: 352.1538 microseconds
Simple write: 74.6871 microseconds
Simple stat: 843.8571 microseconds
Simple fstat: 262.7500 microseconds
Simple open/close: 1453.5984 microseconds
Select on 100 fd's: 463.0870 microseconds
Signal handler installation: 246.9690 microseconds
Signal handler overhead: 482.6065 microseconds
Protection fault: 287.5049 microseconds
Pipe latency: 1389.1997 microseconds
Process fork+exit: 7486.9248 microseconds
[kernel] Panicked at src/task/id.rs:122 failed to allocate kernel stack frame



#### OS COMP TEST GROUP START lmbench-glibc ####
latency measurements
Simple syscall: 11.9010 microseconds
Simple read: 29.3155 microseconds
Simple write: 24.0203 microseconds
Simple stat: 298.8857 microseconds
Simple fstat: 19.5985 microseconds
Simple open/close: 471.0004 microseconds
Select on 100 fd's: 121.2292 microseconds
Signal handler installation: 14.7908 microseconds
Signal handler overhead: 44.9725 microseconds
Protection fault: 4.0257 microseconds
Pipe latency: 373.0270 microseconds
[OOM] kstack_alloc failed: id=26612 range=[0xffffffffc585b000, 0xffffffffc5863000) failed_vpn=0xffffffffc5862 pages=7/8 stack_size=32768 page_size=4096
[OOM] frames: used_pages=227029 free_pages=0 fresh_free_pages=0 recycled_pages=0 total_pages=227029 free_bytes=0 total_bytes=929910784 alloc_count=2068166 free_count=1841137 delta=227029
[OOM] heap: user=79487888 actual=93952672 free=40265056 total=134217728
[OOM] ids: kstack_current=26613 kstack_live=26612 kstack_recycled=1 pid_current=11345 pid_live=11345 pid_recycled=0 deferred_exited_tasks=0
[OOM] tasks: processes=6 locked_processes=1 zombie_processes=0 child_refs=5 task_slots=5 zombie_task_slots=0 ready_queue_tasks=2
[OOM] page_cache: pages=2539 dirty=0 disk_pages=1 disk_dirty=0 disk_limit=4096 tmpfs=2538 tmpfs_swapped=2538 fat32=0 ext4=1 unknown=0 writeback_pending=0
[OOM] swap: enabled=true used_slots=2538 free_slots=30230 total_slots=32768 alloc_count=2538 free_count=0
[kernel] Panicked at src/task/id.rs:279 failed to allocate kernel stack frame



swap
调试日志
处理内核栈回收