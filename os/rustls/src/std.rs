//! Minimal no_std compatibility surface for rustls' existing std-shaped APIs.

pub use alloc::format;

pub mod borrow {
    pub use alloc::borrow::*;
}

pub mod collections {
    pub use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
    pub use hashbrown::HashMap;

    pub mod hash_map {
        pub use hashbrown::hash_map::*;
    }
}

pub mod string {
    pub use alloc::string::*;
}

pub mod vec {
    pub use alloc::vec::*;
}

pub mod time {
    use core::ops::Add;
    use core::sync::atomic::{AtomicU64, Ordering};
    pub use core::time::Duration;

    #[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
    pub struct Instant(Duration);

    impl Instant {
        pub fn now() -> Self {
            static TICKS: AtomicU64 = AtomicU64::new(0);
            Self(Duration::from_nanos(TICKS.fetch_add(1, Ordering::Relaxed)))
        }
    }

    impl Add<Duration> for Instant {
        type Output = Self;

        fn add(self, rhs: Duration) -> Self::Output {
            Self(self.0 + rhs)
        }
    }

    #[derive(Clone, Copy, Debug)]
    pub struct SystemTimeError;
}

pub mod sync {
    pub use alloc::sync::Arc;
    use core::fmt;

    pub struct Mutex<T> {
        inner: spin::Mutex<T>,
    }

    pub type MutexGuard<'a, T> = spin::MutexGuard<'a, T>;

    impl<T: fmt::Debug> fmt::Debug for Mutex<T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Mutex").finish_non_exhaustive()
        }
    }

    impl<T> Mutex<T> {
        pub fn new(data: T) -> Self {
            Self { inner: spin::Mutex::new(data) }
        }

        pub fn lock(&self) -> Result<MutexGuard<'_, T>, ()> {
            Ok(self.inner.lock())
        }
    }

    pub struct RwLock<T> {
        inner: spin::RwLock<T>,
    }

    pub type RwLockReadGuard<'a, T> = spin::RwLockReadGuard<'a, T>;
    pub type RwLockWriteGuard<'a, T> = spin::RwLockWriteGuard<'a, T>;

    impl<T: fmt::Debug> fmt::Debug for RwLock<T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("RwLock").finish_non_exhaustive()
        }
    }

    impl<T> RwLock<T> {
        pub fn new(data: T) -> Self {
            Self { inner: spin::RwLock::new(data) }
        }

        pub fn read(&self) -> Result<RwLockReadGuard<'_, T>, ()> {
            Ok(self.inner.read())
        }

        pub fn write(&self) -> Result<RwLockWriteGuard<'_, T>, ()> {
            Ok(self.inner.write())
        }
    }

    pub struct OnceLock<T> {
        inner: spin::Once<T>,
    }

    impl<T> OnceLock<T> {
        pub const fn new() -> Self {
            Self { inner: spin::Once::new() }
        }

        pub fn set(&self, value: T) -> Result<(), T> {
            if self.inner.get().is_some() {
                return Err(value);
            }
            let _ = self.inner.call_once(|| value);
            Ok(())
        }

        pub fn get(&self) -> Option<&T> {
            self.inner.get()
        }
    }
}

pub mod io {
    use core::fmt;
    use core::ops::Deref;

    pub type Result<T> = core::result::Result<T, Error>;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum ErrorKind {
        BrokenPipe,
        InvalidData,
        InvalidInput,
        Other,
        UnexpectedEof,
        WouldBlock,
        WriteZero,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Error {
        kind: ErrorKind,
    }

    impl Error {
        pub fn new<E>(_kind: ErrorKind, _error: E) -> Self {
            Self { kind: _kind }
        }

        pub fn other<E>(_error: E) -> Self {
            Self { kind: ErrorKind::Other }
        }

        pub fn kind(&self) -> ErrorKind {
            self.kind
        }
    }

    impl From<ErrorKind> for Error {
        fn from(kind: ErrorKind) -> Self {
            Self { kind }
        }
    }

    impl fmt::Display for Error {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{:?}", self.kind)
        }
    }

    #[derive(Clone, Copy)]
    pub struct IoSlice<'a>(&'a [u8]);

    impl<'a> IoSlice<'a> {
        pub const fn new(buf: &'a [u8]) -> Self {
            Self(buf)
        }
    }

    impl Deref for IoSlice<'_> {
        type Target = [u8];

        fn deref(&self) -> &Self::Target {
            self.0
        }
    }

    pub trait Read {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize>;

        fn read_exact(&mut self, mut buf: &mut [u8]) -> Result<()> {
            while !buf.is_empty() {
                match self.read(buf)? {
                    0 => return Err(ErrorKind::UnexpectedEof.into()),
                    n => {
                        let tmp = buf;
                        buf = &mut tmp[n..];
                    }
                }
            }
            Ok(())
        }
    }

    impl Read for &[u8] {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            let amt = core::cmp::min(buf.len(), self.len());
            let (read, rest) = self.split_at(amt);
            buf[..amt].copy_from_slice(read);
            *self = rest;
            Ok(amt)
        }
    }

    impl<T: Read + ?Sized> Read for &mut T {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
            (**self).read(buf)
        }
    }

    pub trait BufRead: Read {
        fn fill_buf(&mut self) -> Result<&[u8]>;
        fn consume(&mut self, amt: usize);
    }

    pub trait Write {
        fn write(&mut self, buf: &[u8]) -> Result<usize>;

        fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> Result<usize> {
            for buf in bufs {
                if !buf.is_empty() {
                    return self.write(buf);
                }
            }
            Ok(0)
        }

        fn write_all(&mut self, mut buf: &[u8]) -> Result<()> {
            while !buf.is_empty() {
                match self.write(buf)? {
                    0 => return Err(ErrorKind::WriteZero.into()),
                    n => buf = &buf[n..],
                }
            }
            Ok(())
        }

        fn flush(&mut self) -> Result<()>;
    }

    impl<T: Write + ?Sized> Write for &mut T {
        fn write(&mut self, buf: &[u8]) -> Result<usize> {
            (**self).write(buf)
        }

        fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> Result<usize> {
            (**self).write_vectored(bufs)
        }

        fn write_all(&mut self, buf: &[u8]) -> Result<()> {
            (**self).write_all(buf)
        }

        fn flush(&mut self) -> Result<()> {
            (**self).flush()
        }
    }
}
