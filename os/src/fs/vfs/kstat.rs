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
    pub st_fs_flags: u32,
    pub st_mnt_id: u64,
    pub stx_attributes: u64,
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
    pub stx_mnt_id: u64,
    pub stx_dio_mem_align: u32,
    pub stx_dio_offset_align: u32,
    __statx_pad2: [u64; 12],
}

const _: [(); 256] = [(); core::mem::size_of::<Statx>()];

const STATX_TYPE: u32 = 0x0000_0001;
const STATX_MODE: u32 = 0x0000_0002;
const STATX_NLINK: u32 = 0x0000_0004;
const STATX_UID: u32 = 0x0000_0008;
const STATX_GID: u32 = 0x0000_0010;
const STATX_ATIME: u32 = 0x0000_0020;
const STATX_MTIME: u32 = 0x0000_0040;
const STATX_CTIME: u32 = 0x0000_0080;
const STATX_INO: u32 = 0x0000_0100;
const STATX_SIZE: u32 = 0x0000_0200;
const STATX_BLOCKS: u32 = 0x0000_0400;
const STATX_MNT_ID: u32 = 0x0000_1000;
const STATX_BASIC_STATS: u32 = STATX_TYPE
    | STATX_MODE
    | STATX_NLINK
    | STATX_UID
    | STATX_GID
    | STATX_ATIME
    | STATX_MTIME
    | STATX_CTIME
    | STATX_INO
    | STATX_SIZE
    | STATX_BLOCKS;

const FS_COMPR_FL: u32 = 0x0000_0004;
const FS_IMMUTABLE_FL: u32 = 0x0000_0010;
const FS_APPEND_FL: u32 = 0x0000_0020;
const FS_NODUMP_FL: u32 = 0x0000_0040;

const STATX_ATTR_COMPRESSED: u64 = 0x0000_0004;
const STATX_ATTR_IMMUTABLE: u64 = 0x0000_0010;
const STATX_ATTR_APPEND: u64 = 0x0000_0020;
const STATX_ATTR_NODUMP: u64 = 0x0000_0040;
pub const STATX_ATTR_MOUNT_ROOT: u64 = 0x0000_2000;

const STATX_SUPPORTED_ATTRIBUTES: u64 = STATX_ATTR_COMPRESSED
    | STATX_ATTR_IMMUTABLE
    | STATX_ATTR_APPEND
    | STATX_ATTR_NODUMP
    | STATX_ATTR_MOUNT_ROOT;

const fn linux_major(dev: u64) -> u32 {
    (((dev >> 8) & 0x0000_0fff) | ((dev >> 32) & 0xffff_f000)) as u32
}

const fn linux_minor(dev: u64) -> u32 {
    ((dev & 0x0000_00ff) | ((dev >> 12) & 0xffff_ff00)) as u32
}

fn statx_attributes_from_fs_flags(flags: u32) -> u64 {
    let mut attrs = 0;
    if flags & FS_COMPR_FL != 0 {
        attrs |= STATX_ATTR_COMPRESSED;
    }
    if flags & FS_IMMUTABLE_FL != 0 {
        attrs |= STATX_ATTR_IMMUTABLE;
    }
    if flags & FS_APPEND_FL != 0 {
        attrs |= STATX_ATTR_APPEND;
    }
    if flags & FS_NODUMP_FL != 0 {
        attrs |= STATX_ATTR_NODUMP;
    }
    attrs
}

pub fn kstat_to_statx(kstat: &Kstat) -> Statx {
    // 有些 statx 字段只能用默认值/0或者不用填
    Statx {
        stx_mask: STATX_BASIC_STATS | STATX_MNT_ID,
        stx_blksize: kstat.st_blksize as u32,
        stx_attributes: statx_attributes_from_fs_flags(kstat.st_fs_flags) | kstat.stx_attributes,
        stx_nlink: kstat.st_nlink,
        stx_uid: kstat.st_uid,
        stx_gid: kstat.st_gid,
        stx_mode: kstat.st_mode as u16,
        stx_ino: kstat.st_ino,
        stx_size: kstat.st_size as u64,
        stx_blocks: kstat.st_blocks,
        stx_attributes_mask: STATX_SUPPORTED_ATTRIBUTES,
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
        stx_rdev_major: linux_major(kstat.st_rdev),
        stx_rdev_minor: linux_minor(kstat.st_rdev),
        stx_dev_major: linux_major(kstat.st_dev),
        stx_dev_minor: linux_minor(kstat.st_dev),
        stx_mnt_id: if kstat.st_mnt_id == 0 {
            1
        } else {
            kstat.st_mnt_id
        },
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
