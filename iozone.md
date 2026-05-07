
# 实现了LRU
#### OS COMP TEST GROUP START iozone-musl ####
iozone automatic measurements
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

        Run began: Thu Jan  1 00:00:23 1970

        Auto Mode
        Record Size 1 kB
        File size set to 4096 kB
        Command line used: ./iozone -a -r 1k -s 4m
        Output is in kBytes/sec
        Time Resolution = 0.000017 seconds.
        Processor cache size set to 1024 kBytes.
        Processor cache line size set to 32 bytes.
        File stride size set to 17 * record size.
                                                                    random    random      bkwd     record     stride                                        
              kB  reclen    write    rewrite      read    reread      read     write      read    rewrite       read    fwrite  frewrite     fread   freread
            4096       1     14065     24650     24306     24074      9972      9793     16588      10777      15387     22397     23525     14663     13678

iozone test complete.
iozone throughput write/read measurements
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

        Run began: Thu Jan  1 00:00:43 1970

        Record Size 1 kB
        File size set to 1024 kB
        Command line used: ./iozone -t 4 -i 0 -i 1 -r 1k -s 1m
        Output is in kBytes/sec
        Time Resolution = 0.000029 seconds.
        Processor cache size set to 1024 kBytes.
        Processor cache line size set to 32 bytes.
        File stride size set to 17 * record size.
        Throughput test with 4 processes
        Each process writes a 1024 kByte file in 1 kByte records

        Children see throughput for  4 initial writers  =    4247.53 kB/sec
        Parent sees throughput for  4 initial writers   =     284.00 kB/sec
        Min throughput per process                      =     217.27 kB/sec 
        Max throughput per process                      =    3564.70 kB/sec
        Avg throughput per process                      =    1061.88 kB/sec
        Min xfer                                        =     757.00 kB

        Children see throughput for  4 rewriters        =    4999.82 kB/sec
        Parent sees throughput for  4 rewriters         =    1628.22 kB/sec
        Min throughput per process                      =     531.86 kB/sec 
        Max throughput per process                      =    3042.89 kB/sec
        Avg throughput per process                      =    1249.95 kB/sec
        Min xfer                                        =     788.00 kB

        Children see throughput for  4 readers          =   21808.27 kB/sec
        Parent sees throughput for  4 readers           =   19731.39 kB/sec
        Min throughput per process                      =    5001.48 kB/sec 
        Max throughput per process                      =    5965.80 kB/sec
        Avg throughput per process                      =    5452.07 kB/sec
        Min xfer                                        =     813.00 kB

        Children see throughput for 4 re-readers        =   22204.79 kB/sec
        Parent sees throughput for 4 re-readers         =   20105.67 kB/sec
        Min throughput per process                      =    5200.97 kB/sec 
        Max throughput per process                      =    5960.59 kB/sec
        Avg throughput per process                      =    5551.20 kB/sec
        Min xfer                                        =     806.00 kB



iozone test complete.
iozone throughput random-read measurements
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

        Run began: Thu Jan  1 00:01:00 1970

        Record Size 1 kB
        File size set to 1024 kB
        Command line used: ./iozone -t 4 -i 0 -i 2 -r 1k -s 1m
        Output is in kBytes/sec
        Time Resolution = 0.000016 seconds.
        Processor cache size set to 1024 kBytes.
        Processor cache line size set to 32 bytes.
        File stride size set to 17 * record size.
        Throughput test with 4 processes
        Each process writes a 1024 kByte file in 1 kByte records

        Children see throughput for  4 initial writers  =    4215.40 kB/sec
        Parent sees throughput for  4 initial writers   =     407.76 kB/sec
        Min throughput per process                      =     134.39 kB/sec 
        Max throughput per process                      =    3490.35 kB/sec
        Avg throughput per process                      =    1053.85 kB/sec
        Min xfer                                        =    1009.00 kB

        Children see throughput for  4 rewriters        =    6458.44 kB/sec
        Parent sees throughput for  4 rewriters         =    1415.93 kB/sec
        Min throughput per process                      =     461.13 kB/sec 
        Max throughput per process                      =    4032.10 kB/sec
        Avg throughput per process                      =    1614.61 kB/sec
        Min xfer                                        =     885.00 kB

        Children see throughput for 4 random readers    =   11936.20 kB/sec
        Parent sees throughput for 4 random readers     =   11234.51 kB/sec
        Min throughput per process                      =    2852.01 kB/sec 
        Max throughput per process                      =    3148.24 kB/sec
        Avg throughput per process                      =    2984.05 kB/sec
        Min xfer                                        =     909.00 kB

        Children see throughput for 4 random writers    =    4239.73 kB/sec
        Parent sees throughput for 4 random writers     =    1464.39 kB/sec
        Min throughput per process                      =     564.39 kB/sec 
        Max throughput per process                      =    2227.91 kB/sec
        Avg throughput per process                      =    1059.93 kB/sec
        Min xfer                                        =     902.00 kB



iozone test complete.
iozone throughput read-backwards measurements
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

        Run began: Thu Jan  1 00:01:18 1970

        Record Size 1 kB
        File size set to 1024 kB
        Command line used: ./iozone -t 4 -i 0 -i 3 -r 1k -s 1m
        Output is in kBytes/sec
        Time Resolution = 0.000016 seconds.
        Processor cache size set to 1024 kBytes.
        Processor cache line size set to 32 bytes.
        File stride size set to 17 * record size.
        Throughput test with 4 processes
        Each process writes a 1024 kByte file in 1 kByte records

        Children see throughput for  4 initial writers  =    3439.79 kB/sec
        Parent sees throughput for  4 initial writers   =     376.69 kB/sec
        Min throughput per process                      =     121.58 kB/sec 
        Max throughput per process                      =    2764.03 kB/sec
        Avg throughput per process                      =     859.95 kB/sec
        Min xfer                                        =     983.00 kB

        Children see throughput for  4 rewriters        =    9454.23 kB/sec
        Parent sees throughput for  4 rewriters         =    2448.03 kB/sec
        Min throughput per process                      =     823.69 kB/sec 
        Max throughput per process                      =    5932.48 kB/sec
        Avg throughput per process                      =    2363.56 kB/sec
        Min xfer                                        =     858.00 kB

        Children see throughput for 4 reverse readers   =   10841.44 kB/sec
        Parent sees throughput for 4 reverse readers    =   10191.65 kB/sec
        Min throughput per process                      =    2589.81 kB/sec 
        Max throughput per process                      =    2795.51 kB/sec
        Avg throughput per process                      =    2710.36 kB/sec
        Min xfer                                        =     977.00 kB



iozone test complete.
iozone throughput stride-read measurements
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

        Run began: Thu Jan  1 00:01:32 1970

        Record Size 1 kB
        File size set to 1024 kB
        Command line used: ./iozone -t 4 -i 0 -i 5 -r 1k -s 1m
        Output is in kBytes/sec
        Time Resolution = 0.000016 seconds.
        Processor cache size set to 1024 kBytes.
        Processor cache line size set to 32 bytes.
        File stride size set to 17 * record size.
        Throughput test with 4 processes
        Each process writes a 1024 kByte file in 1 kByte records

        Children see throughput for  4 initial writers  =    3410.64 kB/sec
        Parent sees throughput for  4 initial writers   =     377.84 kB/sec
        Min throughput per process                      =     139.41 kB/sec 
        Max throughput per process                      =    2712.07 kB/sec
        Avg throughput per process                      =     852.66 kB/sec
        Min xfer                                        =     926.00 kB

        Children see throughput for  4 rewriters        =    7537.55 kB/sec
        Parent sees throughput for  4 rewriters         =    1922.85 kB/sec
        Min throughput per process                      =     623.42 kB/sec 
        Max throughput per process                      =    4810.40 kB/sec
        Avg throughput per process                      =    1884.39 kB/sec
        Min xfer                                        =     925.00 kB

        Children see throughput for 4 stride readers    =   13403.22 kB/sec
        Parent sees throughput for 4 stride readers     =   12139.72 kB/sec
        Min throughput per process                      =    3161.50 kB/sec 
        Max throughput per process                      =    3503.48 kB/sec
        Avg throughput per process                      =    3350.81 kB/sec
        Min xfer                                        =     980.00 kB



iozone test complete.
iozone throughput fwrite/fread measurements
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

        Run began: Thu Jan  1 00:01:46 1970

        Record Size 1 kB
        File size set to 1024 kB
        Command line used: ./iozone -t 4 -i 6 -i 7 -r 1k -s 1m
        Output is in kBytes/sec
        Time Resolution = 0.000017 seconds.
        Processor cache size set to 1024 kBytes.
        Processor cache line size set to 32 bytes.
        File stride size set to 17 * record size.
        Throughput test with 4 processes
        Each process writes a 1024 kByte file in 1 kByte records

        Children see throughput for  4 fwriters         =    3757.86 kB/sec
        Parent sees throughput for  4 fwriters          =     368.62 kB/sec
        Min throughput per process                      =     118.47 kB/sec 
        Max throughput per process                      =    3058.14 kB/sec
        Avg throughput per process                      =     939.47 kB/sec
        Min xfer                                        =    1024.00 kB

        Children see throughput for  4 freaders         =   15123.48 kB/sec
        Parent sees throughput for  4 freaders          =   12552.60 kB/sec
        Min throughput per process                      =    3496.05 kB/sec 
        Max throughput per process                      =    4118.02 kB/sec
        Avg throughput per process                      =    3780.87 kB/sec
        Min xfer                                        =    1024.00 kB



iozone test complete.
iozone throughput pwrite/pread measurements
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

        Run began: Thu Jan  1 00:02:00 1970

        Record Size 1 kB
        File size set to 1024 kB
        Command line used: ./iozone -t 4 -i 9 -i 10 -r 1k -s 1m
        Output is in kBytes/sec
        Time Resolution = 0.000016 seconds.
        Processor cache size set to 1024 kBytes.
        Processor cache line size set to 32 bytes.
        File stride size set to 17 * record size.
        Throughput test with 4 processes
        Each process writes a 1024 kByte file in 1 kByte records

        Children see throughput for 4 pwrite writers    =    3990.15 kB/sec
        Parent sees throughput for 4 pwrite writers     =     395.07 kB/sec
        Min throughput per process                      =     206.30 kB/sec 
        Max throughput per process                      =    3051.35 kB/sec
        Avg throughput per process                      =     997.54 kB/sec
        Min xfer                                        =     888.00 kB

        Children see throughput for 4 pread readers     =   20389.70 kB/sec
        Parent sees throughput for 4 pread readers      =   15849.09 kB/sec
        Min throughput per process                      =    4404.80 kB/sec 
        Max throughput per process                      =    6083.40 kB/sec
        Avg throughput per process                      =    5097.42 kB/sec
        Min xfer                                        =     916.00 kB



iozone test complete.
iozone throughtput pwritev/preadv measurements
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

        Run began: Thu Jan  1 00:02:11 1970

        Selected test not available on the version.
        Record Size 1 kB
        File size set to 1024 kB
        Command line used: ./iozone -t 4 -i 11 -i 12 -r 1k -s 1m
        Output is in kBytes/sec
        Time Resolution = 0.000016 seconds.
        Processor cache size set to 1024 kBytes.
        Processor cache line size set to 32 bytes.
        File stride size set to 17 * record size.
        Throughput test with 4 processes
        Each process writes a 1024 kByte file in 1 kByte records

        Children see throughput for  4 initial writers  =    4753.70 kB/sec
        Parent sees throughput for  4 initial writers   =     426.24 kB/sec
        Min throughput per process                      =     141.22 kB/sec 
        Max throughput per process                      =    4095.44 kB/sec
        Avg throughput per process                      =    1188.43 kB/sec
        Min xfer                                        =     873.00 kB

        Children see throughput for  4 rewriters        =   10300.59 kB/sec
        Parent sees throughput for  4 rewriters         =    2828.09 kB/sec
        Min throughput per process                      =     906.88 kB/sec 
        Max throughput per process                      =    6140.23 kB/sec
        Avg throughput per process                      =    2575.15 kB/sec
        Min xfer                                        =     921.00 kB



iozone test complete.
#### OS COMP TEST GROUP END iozone-musl ####



# 未实现LRU
#### OS COMP TEST GROUP START iozone-musl ####
iozone automatic measurements
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

        Run began: Thu Jan  1 00:00:25 1970

        Auto Mode
        Record Size 1 kB
        File size set to 4096 kB
        Command line used: ./iozone -a -r 1k -s 4m
        Output is in kBytes/sec
        Time Resolution = 0.000012 seconds.
        Processor cache size set to 1024 kBytes.
        Processor cache line size set to 32 bytes.
        File stride size set to 17 * record size.
                                                                    random    random      bkwd     record     stride                                        
              kB  reclen    write    rewrite      read    reread      read     write      read    rewrite       read    fwrite  frewrite     fread   freread
            4096       1     17208     38337     38200     38138     13010     12837     12818      13112      12843     16182     16596     12391     12624

iozone test complete.
iozone throughput write/read measurements
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

        Run began: Thu Jan  1 00:00:39 1970

        Record Size 1 kB
        File size set to 1024 kB
        Command line used: ./iozone -t 4 -i 0 -i 1 -r 1k -s 1m
        Output is in kBytes/sec
        Time Resolution = 0.000012 seconds.
        Processor cache size set to 1024 kBytes.
        Processor cache line size set to 32 bytes.
        File stride size set to 17 * record size.
        Throughput test with 4 processes
        Each process writes a 1024 kByte file in 1 kByte records

        Children see throughput for  4 initial writers  =    4071.33 kB/sec
        Parent sees throughput for  4 initial writers   =     523.32 kB/sec
        Min throughput per process                      =       0.00 kB/sec 
        Max throughput per process                      =    4071.33 kB/sec
        Avg throughput per process                      =    1017.83 kB/sec
        Min xfer                                        =       0.00 kB

        Children see throughput for  4 rewriters        =   36169.69 kB/sec
        Parent sees throughput for  4 rewriters         =    3285.91 kB/sec
        Min throughput per process                      =       0.00 kB/sec 
        Max throughput per process                      =   36169.69 kB/sec
        Avg throughput per process                      =    9042.42 kB/sec
        Min xfer                                        =       0.00 kB

        Children see throughput for  4 readers          =   37390.00 kB/sec
        Parent sees throughput for  4 readers           =   11558.34 kB/sec
        Min throughput per process                      =       0.00 kB/sec 
        Max throughput per process                      =   37390.00 kB/sec
        Avg throughput per process                      =    9347.50 kB/sec
        Min xfer                                        =       0.00 kB

        Children see throughput for 4 re-readers        =   38027.33 kB/sec
        Parent sees throughput for 4 re-readers         =   11671.78 kB/sec
        Min throughput per process                      =       0.00 kB/sec 
        Max throughput per process                      =   38027.33 kB/sec
        Avg throughput per process                      =    9506.83 kB/sec
        Min xfer                                        =       0.00 kB



iozone test complete.
iozone throughput random-read measurements
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

        Run began: Thu Jan  1 00:00:45 1970

        Record Size 1 kB
        File size set to 1024 kB
        Command line used: ./iozone -t 4 -i 0 -i 2 -r 1k -s 1m
        Output is in kBytes/sec
        Time Resolution = 0.000011 seconds.
        Processor cache size set to 1024 kBytes.
        Processor cache line size set to 32 bytes.
        File stride size set to 17 * record size.
        Throughput test with 4 processes
        Each process writes a 1024 kByte file in 1 kByte records

        Children see throughput for  4 initial writers  =   21360.92 kB/sec
        Parent sees throughput for  4 initial writers   =     173.10 kB/sec
        Min throughput per process                      =       0.00 kB/sec 
        Max throughput per process                      =   21360.92 kB/sec
        Avg throughput per process                      =    5340.23 kB/sec
        Min xfer                                        =       0.00 kB

        Children see throughput for  4 rewriters        =   25082.67 kB/sec
        Parent sees throughput for  4 rewriters         =    2865.91 kB/sec
        Min throughput per process                      =       0.00 kB/sec 
        Max throughput per process                      =   25082.67 kB/sec
        Avg throughput per process                      =    6270.67 kB/sec
        Min xfer                                        =       0.00 kB

        Children see throughput for 4 random readers    =   17885.21 kB/sec
        Parent sees throughput for 4 random readers     =    8380.60 kB/sec
        Min throughput per process                      =       0.00 kB/sec 
        Max throughput per process                      =   17885.21 kB/sec
        Avg throughput per process                      =    4471.30 kB/sec
        Min xfer                                        =       0.00 kB

        Children see throughput for 4 random writers    =   23771.94 kB/sec
        Parent sees throughput for 4 random writers     =     880.27 kB/sec
        Min throughput per process                      =       0.00 kB/sec 
        Max throughput per process                      =   23771.94 kB/sec
        Avg throughput per process                      =    5942.98 kB/sec
        Min xfer                                        =       0.00 kB



iozone test complete.
iozone throughput read-backwards measurements
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

        Run began: Thu Jan  1 00:00:56 1970

        Record Size 1 kB
        File size set to 1024 kB
        Command line used: ./iozone -t 4 -i 0 -i 3 -r 1k -s 1m
        Output is in kBytes/sec
        Time Resolution = 0.000011 seconds.
        Processor cache size set to 1024 kBytes.
        Processor cache line size set to 32 bytes.
        File stride size set to 17 * record size.
        Throughput test with 4 processes
        Each process writes a 1024 kByte file in 1 kByte records

        Children see throughput for  4 initial writers  =   22694.03 kB/sec
        Parent sees throughput for  4 initial writers   =     168.11 kB/sec
        Min throughput per process                      =       0.00 kB/sec 
        Max throughput per process                      =   22694.03 kB/sec
        Avg throughput per process                      =    5673.51 kB/sec
        Min xfer                                        =       0.00 kB

        Children see throughput for  4 rewriters        =   36596.26 kB/sec
        Parent sees throughput for  4 rewriters         =    3118.60 kB/sec
        Min throughput per process                      =       0.00 kB/sec 
        Max throughput per process                      =   36596.26 kB/sec
        Avg throughput per process                      =    9149.07 kB/sec
        Min xfer                                        =       0.00 kB

        Children see throughput for 4 reverse readers   =   23295.49 kB/sec
        Parent sees throughput for 4 reverse readers    =    8504.84 kB/sec
        Min throughput per process                      =       0.00 kB/sec 
        Max throughput per process                      =   23295.49 kB/sec
        Avg throughput per process                      =    5823.87 kB/sec
        Min xfer                                        =       0.00 kB



iozone test complete.
iozone throughput stride-read measurements
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

        Run began: Thu Jan  1 00:01:06 1970

        Record Size 1 kB
        File size set to 1024 kB
        Command line used: ./iozone -t 4 -i 0 -i 5 -r 1k -s 1m
        Output is in kBytes/sec
        Time Resolution = 0.000012 seconds.
        Processor cache size set to 1024 kBytes.
        Processor cache line size set to 32 bytes.
        File stride size set to 17 * record size.
        Throughput test with 4 processes
        Each process writes a 1024 kByte file in 1 kByte records

        Children see throughput for  4 initial writers  =   22449.25 kB/sec
        Parent sees throughput for  4 initial writers   =     161.45 kB/sec
        Min throughput per process                      =       0.00 kB/sec 
        Max throughput per process                      =   22449.25 kB/sec
        Avg throughput per process                      =    5612.31 kB/sec
        Min xfer                                        =       0.00 kB

        Children see throughput for  4 rewriters        =   35103.36 kB/sec
        Parent sees throughput for  4 rewriters         =    3042.26 kB/sec
        Min throughput per process                      =       0.00 kB/sec 
        Max throughput per process                      =   35103.36 kB/sec
        Avg throughput per process                      =    8775.84 kB/sec
        Min xfer                                        =       0.00 kB

        Children see throughput for 4 stride readers    =   21342.23 kB/sec
        Parent sees throughput for 4 stride readers     =    8223.64 kB/sec
        Min throughput per process                      =       0.00 kB/sec 
        Max throughput per process                      =   21342.23 kB/sec
        Avg throughput per process                      =    5335.56 kB/sec
        Min xfer                                        =       0.00 kB



iozone test complete.
iozone throughput fwrite/fread measurements
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

        Run began: Thu Jan  1 00:01:16 1970

        Record Size 1 kB
        File size set to 1024 kB
        Command line used: ./iozone -t 4 -i 6 -i 7 -r 1k -s 1m
        Output is in kBytes/sec
        Time Resolution = 0.000011 seconds.
        Processor cache size set to 1024 kBytes.
        Processor cache line size set to 32 bytes.
        File stride size set to 17 * record size.
        Throughput test with 4 processes
        Each process writes a 1024 kByte file in 1 kByte records

        Children see throughput for  4 fwriters         =   50221.92 kB/sec
        Parent sees throughput for  4 fwriters          =     645.22 kB/sec
        Min throughput per process                      =   11187.96 kB/sec 
        Max throughput per process                      =   14332.70 kB/sec
        Avg throughput per process                      =   12555.48 kB/sec
        Min xfer                                        =    1024.00 kB

        Children see throughput for  4 freaders         =  113596.98 kB/sec
        Parent sees throughput for  4 freaders          =   18159.17 kB/sec
        Min throughput per process                      =   26184.57 kB/sec 
        Max throughput per process                      =   29652.79 kB/sec
        Avg throughput per process                      =   28399.25 kB/sec
        Min xfer                                        =    1024.00 kB



iozone test complete.
iozone throughput pwrite/pread measurements
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

        Run began: Thu Jan  1 00:01:25 1970

        Record Size 1 kB
        File size set to 1024 kB
        Command line used: ./iozone -t 4 -i 9 -i 10 -r 1k -s 1m
        Output is in kBytes/sec
        Time Resolution = 0.000012 seconds.
        Processor cache size set to 1024 kBytes.
        Processor cache line size set to 32 bytes.
        File stride size set to 17 * record size.
        Throughput test with 4 processes
        Each process writes a 1024 kByte file in 1 kByte records

        Children see throughput for 4 pwrite writers    =   12553.94 kB/sec
        Parent sees throughput for 4 pwrite writers     =     170.02 kB/sec
        Min throughput per process                      =       0.00 kB/sec 
        Max throughput per process                      =   12553.94 kB/sec
        Avg throughput per process                      =    3138.49 kB/sec
        Min xfer                                        =       0.00 kB

        Children see throughput for 4 pread readers     =   35500.09 kB/sec
        Parent sees throughput for 4 pread readers      =    8906.75 kB/sec
        Min throughput per process                      =       0.00 kB/sec 
        Max throughput per process                      =   35500.09 kB/sec
        Avg throughput per process                      =    8875.02 kB/sec
        Min xfer                                        =       0.00 kB



iozone test complete.
iozone throughtput pwritev/preadv measurements
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

        Run began: Thu Jan  1 00:01:34 1970

        Selected test not available on the version.
        Record Size 1 kB
        File size set to 1024 kB
        Command line used: ./iozone -t 4 -i 11 -i 12 -r 1k -s 1m
        Output is in kBytes/sec
        Time Resolution = 0.000011 seconds.
        Processor cache size set to 1024 kBytes.
        Processor cache line size set to 32 bytes.
        File stride size set to 17 * record size.
        Throughput test with 4 processes
        Each process writes a 1024 kByte file in 1 kByte records

        Children see throughput for  4 initial writers  =   22239.60 kB/sec
        Parent sees throughput for  4 initial writers   =     156.02 kB/sec
        Min throughput per process                      =       0.00 kB/sec 
        Max throughput per process                      =   22239.60 kB/sec
        Avg throughput per process                      =    5559.90 kB/sec
        Min xfer                                        =       0.00 kB

        Children see throughput for  4 rewriters        =   35936.13 kB/sec
        Parent sees throughput for  4 rewriters         =    3089.38 kB/sec
        Min throughput per process                      =       0.00 kB/sec 
        Max throughput per process                      =   35936.13 kB/sec
        Avg throughput per process                      =    8984.03 kB/sec
        Min xfer                                        =       0.00 kB



iozone test complete.
#### OS COMP TEST GROUP END iozone-musl ####
root@kairix:/musl$ 