//! 内核全局错误码定义与结果类型。
//!
//! 本模块提供 [`SysError`] 枚举，对应 Linux `errno.h` 中定义的标准错误码。
//! 所有系统调用及内核子系统（VFS、内存管理、进程管理等）均使用 [`SysResult<T>`]
//! 或 [`SyscallResult`] 传递错误信息，以替代此前分散在各处的 `isize` 负值魔术数字。
//!
//! ## 设计约定
//!
//! - 成功时返回具体的非负值（`usize` 或泛型 `T`）。
//! - 失败时返回 [`SysError`]，由 trap handler 统一转换为负值写入用户态 `a0`。
//! - 错误码数值与 Linux 保持一致（`#[repr(i32)]`），确保 musl/glibc 用户态库能够正确识别。

/// 系统调用结果类型。
///
/// 成功时返回非负的 `usize`（文件描述符、读取字节数、内存地址等）；
/// 失败时返回 [`SysError`]，由 trap handler 转换为负值写入用户态寄存器。
pub type SyscallResult = Result<usize, SysError>;

/// 通用内核结果类型。
///
/// 用于非系统调用层（如 VFS、内存管理、网络等）返回成功值或错误码。
pub type SysResult<T> = Result<T, SysError>;

/// Linux 标准错误码。
///
/// 每个变体的数值与 `uapi/asm-generic/errno.h` 中的定义保持一致。
/// 内核代码应优先使用枚举变体而非裸整数，以获得类型安全及可读性。
///
/// 参考：
/// - <https://elixir.bootlin.com/linux/v6.8.9/source/include/uapi/asm-generic/errno.h>
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum SysError {
    /// Operation not permitted (`EPERM`, 1)。
    EPERM = 1,
    /// No such file or directory (`ENOENT`, 2)。
    ENOENT = 2,
    /// No such process (`ESRCH`, 3)。
    ESRCH = 3,
    /// Interrupted system call (`EINTR`, 4)。
    EINTR = 4,
    /// I/O error (`EIO`, 5)。
    EIO = 5,
    /// No such device or address (`ENXIO`, 6)。
    ENXIO = 6,
    /// Argument list too long (`E2BIG`, 7)。
    E2BIG = 7,
    /// Exec format error (`ENOEXEC`, 8)。
    ENOEXEC = 8,
    /// Bad file number (`EBADF`, 9)。
    EBADF = 9,
    /// No child processes (`ECHILD`, 10)。
    ECHILD = 10,
    /// Try again / Resource temporarily unavailable (`EAGAIN`, 11)。
    EAGAIN = 11,
    /// Out of memory (`ENOMEM`, 12)。
    ENOMEM = 12,
    /// Permission denied (`EACCES`, 13)。
    EACCES = 13,
    /// Bad address (`EFAULT`, 14)。
    EFAULT = 14,
    /// Block device required (`ENOTBLK`, 15)。
    ENOTBLK = 15,
    /// Device or resource busy (`EBUSY`, 16)。
    EBUSY = 16,
    /// File exists (`EEXIST`, 17)。
    EEXIST = 17,
    /// Cross-device link (`EXDEV`, 18)。
    EXDEV = 18,
    /// No such device (`ENODEV`, 19)。
    ENODEV = 19,
    /// Not a directory (`ENOTDIR`, 20)。
    ENOTDIR = 20,
    /// Is a directory (`EISDIR`, 21)。
    EISDIR = 21,
    /// Invalid argument (`EINVAL`, 22)。
    EINVAL = 22,
    /// File table overflow (`ENFILE`, 23)。
    ENFILE = 23,
    /// Too many open files (`EMFILE`, 24)。
    EMFILE = 24,
    /// Not a typewriter (`ENOTTY`, 25)。
    ENOTTY = 25,
    /// Text file busy (`ETXTBSY`, 26)。
    ETXTBSY = 26,
    /// File too large (`EFBIG`, 27)。
    EFBIG = 27,
    /// No space left on device (`ENOSPC`, 28)。
    ENOSPC = 28,
    /// Illegal seek (`ESPIPE`, 29)。
    ESPIPE = 29,
    /// Read-only file system (`EROFS`, 30)。
    EROFS = 30,
    /// Too many links (`EMLINK`, 31)。
    EMLINK = 31,
    /// Broken pipe (`EPIPE`, 32)。
    EPIPE = 32,
    /// Math argument out of domain of func (`EDOM`, 33)。
    EDOM = 33,
    /// Math result not representable (`ERANGE`, 34)。
    ERANGE = 34,
    /// Resource deadlock would occur (`EDEADLK`, 35)。
    EDEADLK = 35,
    /// File name too long (`ENAMETOOLONG`, 36)。
    ENAMETOOLONG = 36,
    /// No record locks available (`ENOLCK`, 37)。
    ENOLCK = 37,
    /// Invalid system call number (`ENOSYS`, 38)。
    ENOSYS = 38,
    /// Directory not empty (`ENOTEMPTY`, 39)。
    ENOTEMPTY = 39,
    /// Too many symbolic links encountered (`ELOOP`, 40)。
    ELOOP = 40,
    /// No data available (`ENODATA`, 61)。
    ENODATA = 61,
    /// Value too large for defined data type (`EOVERFLOW`, 75)。
    EOVERFLOW = 75,
    /// Socket operation on non-socket (`ENOTSOCK`, 88)。
    ENOTSOCK = 88,
    /// Protocol not available (`ENOPROTOOPT`, 92)。
    ENOPROTOOPT = 92,
    /// Protocol not supported (`EPROTONOSUPPORT`, 93)。
    EPROTONOSUPPORT = 93,
    /// Operation not supported on transport endpoint (`EOPNOTSUPP`, 95)。
    EOPNOTSUPP = 95,
    /// Address family not supported by protocol (`EAFNOSUPPORT`, 97)。
    EAFNOSUPPORT = 97,
    /// Address already in use (`EADDRINUSE`, 98)。
    EADDRINUSE = 98,
    /// Address not available (`EADDRNOTAVAIL`, 99)。
    EADDRNOTAVAIL = 99,
    /// Connection reset by peer (`ECONNRESET`, 104)。
    ECONNRESET = 104,
    /// Transport endpoint is already connected (`EISCONN`, 106)。
    EISCONN = 106,
    /// Transport endpoint is not connected (`ENOTCONN`, 107)。
    ENOTCONN = 107,
    /// Connection refused (`ECONNREFUSED`, 111)。
    ECONNREFUSED = 111,
    /// Operation now in progress (`EINPROGRESS`, 115)。
    EINPROGRESS = 115,
    /// Stale file handle (`ESTALE`, 116)。
    ESTALE = 116,
    /// Operation cancelled (`ECANCELED`, 125)。
    ECANCELED = 125,
}


impl SysError {
    /// 返回该错误码对应的文本描述。
    ///
    /// 主要用于日志输出，例如：
    /// ```ignore
    /// warn!("openat failed: {}", e.as_str());
    /// ```
    pub const fn as_str(&self) -> &'static str {
        use SysError::*;
        match self {
            EPERM => "Operation not permitted",
            ENOENT => "No such file or directory",
            ESRCH => "No such process",
            EINTR => "Interrupted system call",
            EIO => "I/O error",
            ENXIO => "No such device or address",
            E2BIG => "Argument list too long",
            ENOEXEC => "Exec format error",
            EBADF => "Bad file number",
            ECHILD => "No child processes",
            EAGAIN => "Try again",
            ENOMEM => "Out of memory",
            EACCES => "Permission denied",
            EFAULT => "Bad address",
            ENOTBLK => "Block device required",
            EBUSY => "Device or resource busy",
            EEXIST => "File exists",
            EXDEV => "Cross-device link",
            ENODEV => "No such device",
            ENOTDIR => "Not a directory",
            EISDIR => "Is a directory",
            EINVAL => "Invalid argument",
            ENFILE => "File table overflow",
            EMFILE => "Too many open files",
            ENOTTY => "Not a typewriter",
            ETXTBSY => "Text file busy",
            EFBIG => "File too large",
            ENOSPC => "No space left on device",
            ESPIPE => "Illegal seek",
            EROFS => "Read-only file system",
            EMLINK => "Too many links",
            EPIPE => "Broken pipe",
            EDOM => "Math argument out of domain of func",
            ERANGE => "Math result not representable",
            EDEADLK => "Resource deadlock would occur",
            ENAMETOOLONG => "File name too long",
            ENOLCK => "No record locks available",
            ENOSYS => "Invalid system call number",
            ENOTEMPTY => "Directory not empty",
            ELOOP => "Too many symbolic links encountered",
            ENODATA => "No data",
            EOVERFLOW => "Value too large",
            ENOTSOCK => "Socket operation on non-socket",
            ENOPROTOOPT => "Protocol not available",
            EPROTONOSUPPORT => "Protocol not supported",
            EOPNOTSUPP => "Operation not supported",
            EAFNOSUPPORT => "Address family not supported",
            EADDRINUSE => "Address already in use",
            EADDRNOTAVAIL => "Address not available",
            ECONNRESET => "Connection reset",
            EISCONN => "Transport endpoint is already connected",
            ENOTCONN => "Transport endpoint is not connected",
            ECONNREFUSED => "Connection refused",
            EINPROGRESS => "Operation now in progress",
            ESTALE => "Stale file handle",
            ECANCELED => "Operation cancelled",
        }
    }

    /// 返回该错误码对应的数值（`i32`）。
    ///
    /// 在 trap handler 中用于构造用户态可见的负值返回值：
    /// `(-e.code()) as usize`。
    pub const fn code(self) -> i32 {
        self as i32
    }
}

/// 将原始 `i32` 值转换为 [`SysError`]。
///
/// 主要用于与外部 C 库（如 `lwext4`）或硬件返回码交互时进行转换。
/// 若传入的值不在已定义的错误码范围内，返回 `Err(())`。
///
/// # 示例
/// ```ignore
/// let errno = SysError::try_from(2).unwrap(); // SysError::ENOENT
/// ```
impl TryFrom<i32> for SysError {
    type Error = ();

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        use SysError::*;
        match value {
            1 => Ok(EPERM),
            2 => Ok(ENOENT),
            3 => Ok(ESRCH),
            4 => Ok(EINTR),
            5 => Ok(EIO),
            8 => Ok(ENOEXEC),
            9 => Ok(EBADF),
            10 => Ok(ECHILD),
            11 => Ok(EAGAIN),
            12 => Ok(ENOMEM),
            13 => Ok(EACCES),
            14 => Ok(EFAULT),
            17 => Ok(EEXIST),
            20 => Ok(ENOTDIR),
            21 => Ok(EISDIR),
            22 => Ok(EINVAL),
            28 => Ok(ENOSPC),
            29 => Ok(ESPIPE),
            38 => Ok(ENOSYS),
            61 => Ok(ENODATA),
            75 => Ok(EOVERFLOW),
            88 => Ok(ENOTSOCK),
            92 => Ok(ENOPROTOOPT),
            93 => Ok(EPROTONOSUPPORT),
            95 => Ok(EOPNOTSUPP),
            97 => Ok(EAFNOSUPPORT),
            98 => Ok(EADDRINUSE),
            99 => Ok(EADDRNOTAVAIL),
            104 => Ok(ECONNRESET),
            106 => Ok(EISCONN),
            107 => Ok(ENOTCONN),
            111 => Ok(ECONNREFUSED),
            115 => Ok(EINPROGRESS),
            116 => Ok(ESTALE),
            125 => Ok(ECANCELED),
            _ => Err(()),
        }
    }
}
