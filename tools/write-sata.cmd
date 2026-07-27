echo Kairix SATA rootfs writer
echo Source: USB /install/sata-rootfs.img
echo Target: SCSI device 0, starting at LBA 0

usb start
scsi scan
scsi device 0

setenv load00 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x00000000
setenv write00 scsi write ${loadaddr} 0x00000000 0x00020000
setenv load01 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x04000000
setenv write01 scsi write ${loadaddr} 0x00020000 0x00020000
setenv load02 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x08000000
setenv write02 scsi write ${loadaddr} 0x00040000 0x00020000
setenv load03 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x0c000000
setenv write03 scsi write ${loadaddr} 0x00060000 0x00020000
setenv load04 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x10000000
setenv write04 scsi write ${loadaddr} 0x00080000 0x00020000
setenv load05 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x14000000
setenv write05 scsi write ${loadaddr} 0x000a0000 0x00020000
setenv load06 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x18000000
setenv write06 scsi write ${loadaddr} 0x000c0000 0x00020000
setenv load07 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x1c000000
setenv write07 scsi write ${loadaddr} 0x000e0000 0x00020000
setenv load08 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x20000000
setenv write08 scsi write ${loadaddr} 0x00100000 0x00020000
setenv load09 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x24000000
setenv write09 scsi write ${loadaddr} 0x00120000 0x00020000
setenv load10 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x28000000
setenv write10 scsi write ${loadaddr} 0x00140000 0x00020000
setenv load11 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x2c000000
setenv write11 scsi write ${loadaddr} 0x00160000 0x00020000
setenv load12 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x30000000
setenv write12 scsi write ${loadaddr} 0x00180000 0x00020000
setenv load13 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x34000000
setenv write13 scsi write ${loadaddr} 0x001a0000 0x00020000
setenv load14 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x38000000
setenv write14 scsi write ${loadaddr} 0x001c0000 0x00020000
setenv load15 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x3c000000
setenv write15 scsi write ${loadaddr} 0x001e0000 0x00020000
setenv load16 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x40000000
setenv write16 scsi write ${loadaddr} 0x00200000 0x00020000
setenv load17 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x44000000
setenv write17 scsi write ${loadaddr} 0x00220000 0x00020000
setenv load18 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x48000000
setenv write18 scsi write ${loadaddr} 0x00240000 0x00020000
setenv load19 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x4c000000
setenv write19 scsi write ${loadaddr} 0x00260000 0x00020000
setenv load20 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x50000000
setenv write20 scsi write ${loadaddr} 0x00280000 0x00020000
setenv load21 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x54000000
setenv write21 scsi write ${loadaddr} 0x002a0000 0x00020000
setenv load22 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x58000000
setenv write22 scsi write ${loadaddr} 0x002c0000 0x00020000
setenv load23 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x5c000000
setenv write23 scsi write ${loadaddr} 0x002e0000 0x00020000
setenv load24 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x60000000
setenv write24 scsi write ${loadaddr} 0x00300000 0x00020000
setenv load25 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x64000000
setenv write25 scsi write ${loadaddr} 0x00320000 0x00020000
setenv load26 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x68000000
setenv write26 scsi write ${loadaddr} 0x00340000 0x00020000
setenv load27 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x6c000000
setenv write27 scsi write ${loadaddr} 0x00360000 0x00020000
setenv load28 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x70000000
setenv write28 scsi write ${loadaddr} 0x00380000 0x00020000
setenv load29 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x74000000
setenv write29 scsi write ${loadaddr} 0x003a0000 0x00020000
setenv load30 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x78000000
setenv write30 scsi write ${loadaddr} 0x003c0000 0x00020000
setenv load31 fatload usb 0:1 ${loadaddr} /install/sata-rootfs.img 0x04000000 0x7c000000
setenv write31 scsi write ${loadaddr} 0x003e0000 0x00020000

setenv batch0 run load00 write00 load01 write01 load02 write02 load03 write03 load04 write04 load05 write05 load06 write06 load07 write07
setenv batch1 run load08 write08 load09 write09 load10 write10 load11 write11 load12 write12 load13 write13 load14 write14 load15 write15
setenv batch2 run load16 write16 load17 write17 load18 write18 load19 write19 load20 write20 load21 write21 load22 write22 load23 write23
setenv batch3 run load24 write24 load25 write25 load26 write26 load27 write27 load28 write28 load29 write29 load30 write30 load31 write31
setenv sata_done echo SATA ROOTFS WRITE COMPLETE

run batch0 batch1 batch2 batch3 sata_done

