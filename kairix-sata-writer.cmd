echo Kairix SATA rootfs writer
echo Source: USB /install/sata-rootfs.img.part00..part06
echo Image bytes: 15032385536
echo Target sectors: 0x1c00000
echo WARNING: SCSI device 0 will be overwritten
usb start
scsi scan
scsi device 0
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x0
setenv write0 scsi write ${loadaddr} 0x0 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x4000000
setenv write1 scsi write ${loadaddr} 0x20000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x8000000
setenv write2 scsi write ${loadaddr} 0x40000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0xc000000
setenv write3 scsi write ${loadaddr} 0x60000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x10000000
setenv write4 scsi write ${loadaddr} 0x80000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x14000000
setenv write5 scsi write ${loadaddr} 0xa0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x18000000
setenv write6 scsi write ${loadaddr} 0xc0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x1c000000
setenv write7 scsi write ${loadaddr} 0xe0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 8/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x20000000
setenv write0 scsi write ${loadaddr} 0x100000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x24000000
setenv write1 scsi write ${loadaddr} 0x120000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x28000000
setenv write2 scsi write ${loadaddr} 0x140000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x2c000000
setenv write3 scsi write ${loadaddr} 0x160000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x30000000
setenv write4 scsi write ${loadaddr} 0x180000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x34000000
setenv write5 scsi write ${loadaddr} 0x1a0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x38000000
setenv write6 scsi write ${loadaddr} 0x1c0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x3c000000
setenv write7 scsi write ${loadaddr} 0x1e0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 16/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x40000000
setenv write0 scsi write ${loadaddr} 0x200000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x44000000
setenv write1 scsi write ${loadaddr} 0x220000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x48000000
setenv write2 scsi write ${loadaddr} 0x240000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x4c000000
setenv write3 scsi write ${loadaddr} 0x260000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x50000000
setenv write4 scsi write ${loadaddr} 0x280000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x54000000
setenv write5 scsi write ${loadaddr} 0x2a0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x58000000
setenv write6 scsi write ${loadaddr} 0x2c0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x5c000000
setenv write7 scsi write ${loadaddr} 0x2e0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 24/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x60000000
setenv write0 scsi write ${loadaddr} 0x300000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x64000000
setenv write1 scsi write ${loadaddr} 0x320000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x68000000
setenv write2 scsi write ${loadaddr} 0x340000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x6c000000
setenv write3 scsi write ${loadaddr} 0x360000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x70000000
setenv write4 scsi write ${loadaddr} 0x380000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x74000000
setenv write5 scsi write ${loadaddr} 0x3a0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x78000000
setenv write6 scsi write ${loadaddr} 0x3c0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part00 0x4000000 0x7c000000
setenv write7 scsi write ${loadaddr} 0x3e0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 32/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x0
setenv write0 scsi write ${loadaddr} 0x400000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x4000000
setenv write1 scsi write ${loadaddr} 0x420000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x8000000
setenv write2 scsi write ${loadaddr} 0x440000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0xc000000
setenv write3 scsi write ${loadaddr} 0x460000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x10000000
setenv write4 scsi write ${loadaddr} 0x480000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x14000000
setenv write5 scsi write ${loadaddr} 0x4a0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x18000000
setenv write6 scsi write ${loadaddr} 0x4c0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x1c000000
setenv write7 scsi write ${loadaddr} 0x4e0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 40/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x20000000
setenv write0 scsi write ${loadaddr} 0x500000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x24000000
setenv write1 scsi write ${loadaddr} 0x520000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x28000000
setenv write2 scsi write ${loadaddr} 0x540000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x2c000000
setenv write3 scsi write ${loadaddr} 0x560000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x30000000
setenv write4 scsi write ${loadaddr} 0x580000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x34000000
setenv write5 scsi write ${loadaddr} 0x5a0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x38000000
setenv write6 scsi write ${loadaddr} 0x5c0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x3c000000
setenv write7 scsi write ${loadaddr} 0x5e0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 48/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x40000000
setenv write0 scsi write ${loadaddr} 0x600000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x44000000
setenv write1 scsi write ${loadaddr} 0x620000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x48000000
setenv write2 scsi write ${loadaddr} 0x640000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x4c000000
setenv write3 scsi write ${loadaddr} 0x660000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x50000000
setenv write4 scsi write ${loadaddr} 0x680000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x54000000
setenv write5 scsi write ${loadaddr} 0x6a0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x58000000
setenv write6 scsi write ${loadaddr} 0x6c0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x5c000000
setenv write7 scsi write ${loadaddr} 0x6e0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 56/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x60000000
setenv write0 scsi write ${loadaddr} 0x700000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x64000000
setenv write1 scsi write ${loadaddr} 0x720000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x68000000
setenv write2 scsi write ${loadaddr} 0x740000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x6c000000
setenv write3 scsi write ${loadaddr} 0x760000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x70000000
setenv write4 scsi write ${loadaddr} 0x780000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x74000000
setenv write5 scsi write ${loadaddr} 0x7a0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x78000000
setenv write6 scsi write ${loadaddr} 0x7c0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part01 0x4000000 0x7c000000
setenv write7 scsi write ${loadaddr} 0x7e0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 64/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x0
setenv write0 scsi write ${loadaddr} 0x800000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x4000000
setenv write1 scsi write ${loadaddr} 0x820000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x8000000
setenv write2 scsi write ${loadaddr} 0x840000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0xc000000
setenv write3 scsi write ${loadaddr} 0x860000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x10000000
setenv write4 scsi write ${loadaddr} 0x880000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x14000000
setenv write5 scsi write ${loadaddr} 0x8a0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x18000000
setenv write6 scsi write ${loadaddr} 0x8c0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x1c000000
setenv write7 scsi write ${loadaddr} 0x8e0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 72/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x20000000
setenv write0 scsi write ${loadaddr} 0x900000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x24000000
setenv write1 scsi write ${loadaddr} 0x920000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x28000000
setenv write2 scsi write ${loadaddr} 0x940000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x2c000000
setenv write3 scsi write ${loadaddr} 0x960000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x30000000
setenv write4 scsi write ${loadaddr} 0x980000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x34000000
setenv write5 scsi write ${loadaddr} 0x9a0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x38000000
setenv write6 scsi write ${loadaddr} 0x9c0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x3c000000
setenv write7 scsi write ${loadaddr} 0x9e0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 80/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x40000000
setenv write0 scsi write ${loadaddr} 0xa00000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x44000000
setenv write1 scsi write ${loadaddr} 0xa20000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x48000000
setenv write2 scsi write ${loadaddr} 0xa40000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x4c000000
setenv write3 scsi write ${loadaddr} 0xa60000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x50000000
setenv write4 scsi write ${loadaddr} 0xa80000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x54000000
setenv write5 scsi write ${loadaddr} 0xaa0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x58000000
setenv write6 scsi write ${loadaddr} 0xac0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x5c000000
setenv write7 scsi write ${loadaddr} 0xae0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 88/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x60000000
setenv write0 scsi write ${loadaddr} 0xb00000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x64000000
setenv write1 scsi write ${loadaddr} 0xb20000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x68000000
setenv write2 scsi write ${loadaddr} 0xb40000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x6c000000
setenv write3 scsi write ${loadaddr} 0xb60000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x70000000
setenv write4 scsi write ${loadaddr} 0xb80000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x74000000
setenv write5 scsi write ${loadaddr} 0xba0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x78000000
setenv write6 scsi write ${loadaddr} 0xbc0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part02 0x4000000 0x7c000000
setenv write7 scsi write ${loadaddr} 0xbe0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 96/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x0
setenv write0 scsi write ${loadaddr} 0xc00000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x4000000
setenv write1 scsi write ${loadaddr} 0xc20000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x8000000
setenv write2 scsi write ${loadaddr} 0xc40000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0xc000000
setenv write3 scsi write ${loadaddr} 0xc60000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x10000000
setenv write4 scsi write ${loadaddr} 0xc80000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x14000000
setenv write5 scsi write ${loadaddr} 0xca0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x18000000
setenv write6 scsi write ${loadaddr} 0xcc0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x1c000000
setenv write7 scsi write ${loadaddr} 0xce0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 104/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x20000000
setenv write0 scsi write ${loadaddr} 0xd00000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x24000000
setenv write1 scsi write ${loadaddr} 0xd20000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x28000000
setenv write2 scsi write ${loadaddr} 0xd40000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x2c000000
setenv write3 scsi write ${loadaddr} 0xd60000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x30000000
setenv write4 scsi write ${loadaddr} 0xd80000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x34000000
setenv write5 scsi write ${loadaddr} 0xda0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x38000000
setenv write6 scsi write ${loadaddr} 0xdc0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x3c000000
setenv write7 scsi write ${loadaddr} 0xde0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 112/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x40000000
setenv write0 scsi write ${loadaddr} 0xe00000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x44000000
setenv write1 scsi write ${loadaddr} 0xe20000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x48000000
setenv write2 scsi write ${loadaddr} 0xe40000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x4c000000
setenv write3 scsi write ${loadaddr} 0xe60000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x50000000
setenv write4 scsi write ${loadaddr} 0xe80000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x54000000
setenv write5 scsi write ${loadaddr} 0xea0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x58000000
setenv write6 scsi write ${loadaddr} 0xec0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x5c000000
setenv write7 scsi write ${loadaddr} 0xee0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 120/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x60000000
setenv write0 scsi write ${loadaddr} 0xf00000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x64000000
setenv write1 scsi write ${loadaddr} 0xf20000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x68000000
setenv write2 scsi write ${loadaddr} 0xf40000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x6c000000
setenv write3 scsi write ${loadaddr} 0xf60000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x70000000
setenv write4 scsi write ${loadaddr} 0xf80000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x74000000
setenv write5 scsi write ${loadaddr} 0xfa0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x78000000
setenv write6 scsi write ${loadaddr} 0xfc0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part03 0x4000000 0x7c000000
setenv write7 scsi write ${loadaddr} 0xfe0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 128/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x0
setenv write0 scsi write ${loadaddr} 0x1000000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x4000000
setenv write1 scsi write ${loadaddr} 0x1020000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x8000000
setenv write2 scsi write ${loadaddr} 0x1040000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0xc000000
setenv write3 scsi write ${loadaddr} 0x1060000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x10000000
setenv write4 scsi write ${loadaddr} 0x1080000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x14000000
setenv write5 scsi write ${loadaddr} 0x10a0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x18000000
setenv write6 scsi write ${loadaddr} 0x10c0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x1c000000
setenv write7 scsi write ${loadaddr} 0x10e0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 136/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x20000000
setenv write0 scsi write ${loadaddr} 0x1100000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x24000000
setenv write1 scsi write ${loadaddr} 0x1120000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x28000000
setenv write2 scsi write ${loadaddr} 0x1140000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x2c000000
setenv write3 scsi write ${loadaddr} 0x1160000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x30000000
setenv write4 scsi write ${loadaddr} 0x1180000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x34000000
setenv write5 scsi write ${loadaddr} 0x11a0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x38000000
setenv write6 scsi write ${loadaddr} 0x11c0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x3c000000
setenv write7 scsi write ${loadaddr} 0x11e0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 144/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x40000000
setenv write0 scsi write ${loadaddr} 0x1200000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x44000000
setenv write1 scsi write ${loadaddr} 0x1220000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x48000000
setenv write2 scsi write ${loadaddr} 0x1240000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x4c000000
setenv write3 scsi write ${loadaddr} 0x1260000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x50000000
setenv write4 scsi write ${loadaddr} 0x1280000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x54000000
setenv write5 scsi write ${loadaddr} 0x12a0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x58000000
setenv write6 scsi write ${loadaddr} 0x12c0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x5c000000
setenv write7 scsi write ${loadaddr} 0x12e0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 152/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x60000000
setenv write0 scsi write ${loadaddr} 0x1300000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x64000000
setenv write1 scsi write ${loadaddr} 0x1320000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x68000000
setenv write2 scsi write ${loadaddr} 0x1340000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x6c000000
setenv write3 scsi write ${loadaddr} 0x1360000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x70000000
setenv write4 scsi write ${loadaddr} 0x1380000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x74000000
setenv write5 scsi write ${loadaddr} 0x13a0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x78000000
setenv write6 scsi write ${loadaddr} 0x13c0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part04 0x4000000 0x7c000000
setenv write7 scsi write ${loadaddr} 0x13e0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 160/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x0
setenv write0 scsi write ${loadaddr} 0x1400000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x4000000
setenv write1 scsi write ${loadaddr} 0x1420000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x8000000
setenv write2 scsi write ${loadaddr} 0x1440000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0xc000000
setenv write3 scsi write ${loadaddr} 0x1460000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x10000000
setenv write4 scsi write ${loadaddr} 0x1480000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x14000000
setenv write5 scsi write ${loadaddr} 0x14a0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x18000000
setenv write6 scsi write ${loadaddr} 0x14c0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x1c000000
setenv write7 scsi write ${loadaddr} 0x14e0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 168/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x20000000
setenv write0 scsi write ${loadaddr} 0x1500000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x24000000
setenv write1 scsi write ${loadaddr} 0x1520000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x28000000
setenv write2 scsi write ${loadaddr} 0x1540000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x2c000000
setenv write3 scsi write ${loadaddr} 0x1560000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x30000000
setenv write4 scsi write ${loadaddr} 0x1580000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x34000000
setenv write5 scsi write ${loadaddr} 0x15a0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x38000000
setenv write6 scsi write ${loadaddr} 0x15c0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x3c000000
setenv write7 scsi write ${loadaddr} 0x15e0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 176/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x40000000
setenv write0 scsi write ${loadaddr} 0x1600000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x44000000
setenv write1 scsi write ${loadaddr} 0x1620000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x48000000
setenv write2 scsi write ${loadaddr} 0x1640000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x4c000000
setenv write3 scsi write ${loadaddr} 0x1660000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x50000000
setenv write4 scsi write ${loadaddr} 0x1680000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x54000000
setenv write5 scsi write ${loadaddr} 0x16a0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x58000000
setenv write6 scsi write ${loadaddr} 0x16c0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x5c000000
setenv write7 scsi write ${loadaddr} 0x16e0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 184/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x60000000
setenv write0 scsi write ${loadaddr} 0x1700000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x64000000
setenv write1 scsi write ${loadaddr} 0x1720000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x68000000
setenv write2 scsi write ${loadaddr} 0x1740000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x6c000000
setenv write3 scsi write ${loadaddr} 0x1760000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x70000000
setenv write4 scsi write ${loadaddr} 0x1780000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x74000000
setenv write5 scsi write ${loadaddr} 0x17a0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x78000000
setenv write6 scsi write ${loadaddr} 0x17c0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part05 0x4000000 0x7c000000
setenv write7 scsi write ${loadaddr} 0x17e0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 192/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x0
setenv write0 scsi write ${loadaddr} 0x1800000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x4000000
setenv write1 scsi write ${loadaddr} 0x1820000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x8000000
setenv write2 scsi write ${loadaddr} 0x1840000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0xc000000
setenv write3 scsi write ${loadaddr} 0x1860000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x10000000
setenv write4 scsi write ${loadaddr} 0x1880000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x14000000
setenv write5 scsi write ${loadaddr} 0x18a0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x18000000
setenv write6 scsi write ${loadaddr} 0x18c0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x1c000000
setenv write7 scsi write ${loadaddr} 0x18e0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 200/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x20000000
setenv write0 scsi write ${loadaddr} 0x1900000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x24000000
setenv write1 scsi write ${loadaddr} 0x1920000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x28000000
setenv write2 scsi write ${loadaddr} 0x1940000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x2c000000
setenv write3 scsi write ${loadaddr} 0x1960000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x30000000
setenv write4 scsi write ${loadaddr} 0x1980000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x34000000
setenv write5 scsi write ${loadaddr} 0x19a0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x38000000
setenv write6 scsi write ${loadaddr} 0x19c0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x3c000000
setenv write7 scsi write ${loadaddr} 0x19e0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 208/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x40000000
setenv write0 scsi write ${loadaddr} 0x1a00000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x44000000
setenv write1 scsi write ${loadaddr} 0x1a20000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x48000000
setenv write2 scsi write ${loadaddr} 0x1a40000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x4c000000
setenv write3 scsi write ${loadaddr} 0x1a60000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x50000000
setenv write4 scsi write ${loadaddr} 0x1a80000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x54000000
setenv write5 scsi write ${loadaddr} 0x1aa0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x58000000
setenv write6 scsi write ${loadaddr} 0x1ac0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x5c000000
setenv write7 scsi write ${loadaddr} 0x1ae0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 216/224 chunks
setenv load0 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x60000000
setenv write0 scsi write ${loadaddr} 0x1b00000 0x20000
setenv load1 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x64000000
setenv write1 scsi write ${loadaddr} 0x1b20000 0x20000
setenv load2 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x68000000
setenv write2 scsi write ${loadaddr} 0x1b40000 0x20000
setenv load3 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x6c000000
setenv write3 scsi write ${loadaddr} 0x1b60000 0x20000
setenv load4 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x70000000
setenv write4 scsi write ${loadaddr} 0x1b80000 0x20000
setenv load5 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x74000000
setenv write5 scsi write ${loadaddr} 0x1ba0000 0x20000
setenv load6 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x78000000
setenv write6 scsi write ${loadaddr} 0x1bc0000 0x20000
setenv load7 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img.part06 0x4000000 0x7c000000
setenv write7 scsi write ${loadaddr} 0x1be0000 0x20000
setenv sata_batch run load0 write0 load1 write1 load2 write2 load3 write3 load4 write4 load5 write5 load6 write6 load7 write7
run sata_batch
echo SATA progress: 224/224 chunks
echo SATA ROOTFS WRITE COMPLETE
echo Reset the board and boot from the USB menu
