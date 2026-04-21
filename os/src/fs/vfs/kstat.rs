#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct Kstat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_mode: u32,
    pub st_nlink: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub st_rdev: u64,
    pub __pad: u64,
    pub st_size: i64,
    pub st_blksize: i32,
    pub __pad2: i32,
    pub st_blocks: u64,
    pub st_atime_sec: i64,
    pub st_atime_nsec: i64,
    pub st_mtime_sec: i64,
    pub st_mtime_nsec: i64,
    pub st_ctime_sec: i64,
    pub st_ctime_nsec: i64,
    pub __unused: [u32; 2],
}
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct Statx {
    pub stx_mask: u32,
    pub stx_blksize: u32,
    pub stx_attributes: u64,
    pub stx_nlink: u32,
    pub stx_uid: u32,
    pub stx_gid: u32,
    pub stx_mode: u16,
    __statx_pad1: [u16; 1],
    pub stx_ino: u64,
    pub stx_size: u64,
    pub stx_blocks: u64,
    pub stx_attributes_mask: u64,
    pub stx_atime: StatxTimestamp,
    pub stx_btime: StatxTimestamp,
    pub stx_ctime: StatxTimestamp,
    pub stx_mtime: StatxTimestamp,
    pub stx_rdev_major: u32,
    pub stx_rdev_minor: u32,
    pub stx_dev_major: u32,
    pub stx_dev_minor: u32,
    __statx_pad2: [u64; 14],
}

pub fn kstat_to_statx(kstat: &Kstat) -> Statx {
    // 有些 statx 字段只能用默认值/0或者不用填
    Statx {
        stx_blksize: kstat.st_blksize as u32,
        stx_nlink: kstat.st_nlink,
        stx_uid: kstat.st_uid,
        stx_gid: kstat.st_gid,
        stx_mode: kstat.st_mode as u16,
        stx_ino: kstat.st_ino,
        stx_size: kstat.st_size as u64,
        stx_blocks: kstat.st_blocks,
        stx_atime: StatxTimestamp {
            // 假设你有这个结构体
            tv_sec: kstat.st_atime_sec as i64,
            tv_nsec: kstat.st_atime_nsec as u32,
            __statx_timestamp_pad1: [0],
        },
        stx_mtime: StatxTimestamp {
            tv_sec: kstat.st_mtime_sec as i64,
            tv_nsec: kstat.st_mtime_nsec as u32,
            __statx_timestamp_pad1: [0],
        },
        stx_ctime: StatxTimestamp {
            tv_sec: kstat.st_ctime_sec as i64,
            tv_nsec: kstat.st_ctime_nsec as u32,
            __statx_timestamp_pad1: [0],
        },
        stx_rdev_major: ((kstat.st_rdev >> 32) & 0xffff_ffff) as u32,
        stx_rdev_minor: (kstat.st_rdev & 0xffff_ffff) as u32,
        stx_dev_major: ((kstat.st_dev >> 32) & 0xffff_ffff) as u32,
        stx_dev_minor: (kstat.st_dev & 0xffff_ffff) as u32,
        // 其余字段——保留原有默认/0
        ..Default::default()
    }
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct StatxTimestamp {
    pub tv_sec: i64,
    pub tv_nsec: u32,
    pub __statx_timestamp_pad1: [i32; 1],
}

impl Kstat {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Statx {
    pub fn new() -> Self {
        Self::default()
    }
}
/// 文件系统统计信息，用于 statfs
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Statfs {
    pub f_type: i64,
    pub f_bsize: i64,
    pub f_blocks: i64,
    pub f_bfree: i64,
    pub f_bavail: i64,
    pub f_files: i64,
    pub f_ffree: i64,
    pub f_fsid: i64,
    pub f_namelen: i64,
    pub f_frsize: i64,
    pub f_flags: i64,
    pub f_spare: [i64; 4],
}

impl Statfs {
    pub fn new() -> Self {
        Self {
            f_type: 0,
            f_bsize: 0,
            f_blocks: 0,
            f_bfree: 0,
            f_bavail: 0,
            f_files: 0,
            f_ffree: 0,
            f_fsid: 0,
            f_namelen: 255,
            f_frsize: 0,
            f_flags: 0,
            f_spare: [0; 4],
        }
    }
}
