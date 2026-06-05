#![allow(missing_docs)]

use alloc::format;
use alloc::sync::Arc;
use log::{info, warn};

use crate::error::{SysError, SysResult};
use crate::fs::vfs::dcache::GLOBAL_DCACHE;
use crate::fs::vfs::file::{find_dentry, open_file};
use crate::fs::vfs::inode::InodeMode;
use crate::fs::vfs::path::split_parent_and_name;
use crate::fs::vfs::{Dentry, OpenFlags};

#[repr(align(16))]
struct AlignedBytes<const N: usize>([u8; N]);

#[cfg(target_arch = "riscv64")]
static INITPROC_ELF: AlignedBytes<
    { include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/initproc").len() },
> = AlignedBytes(*include_bytes!(
    "../../user/target/riscv64gc-unknown-none-elf/release/initproc"
));
#[cfg(target_arch = "loongarch64")]
static INITPROC_ELF: AlignedBytes<
    { include_bytes!("../../user/target/loongarch64-unknown-none/release/initproc").len() },
> = AlignedBytes(*include_bytes!(
    "../../user/target/loongarch64-unknown-none/release/initproc"
));

#[cfg(target_arch = "riscv64")]
const MKFS_EXT2: &[u8] = include_bytes!("../../tools/target/mkfs-riscv64/sbin/mkfs.ext2");
#[cfg(target_arch = "riscv64")]
const MKFS_EXT3: &[u8] = include_bytes!("../../tools/target/mkfs-riscv64/sbin/mkfs.ext3");
#[cfg(target_arch = "riscv64")]
const MKFS_EXT4: &[u8] = include_bytes!("../../tools/target/mkfs-riscv64/sbin/mkfs.ext4");

#[cfg(target_arch = "loongarch64")]
const MKFS_EXT2: &[u8] = include_bytes!("../../tools/target/mkfs-loongarch64/sbin/mkfs.ext2");
#[cfg(target_arch = "loongarch64")]
const MKFS_EXT3: &[u8] = include_bytes!("../../tools/target/mkfs-loongarch64/sbin/mkfs.ext3");
#[cfg(target_arch = "loongarch64")]
const MKFS_EXT4: &[u8] = include_bytes!("../../tools/target/mkfs-loongarch64/sbin/mkfs.ext4");

const MKE2FS_CONF: &[u8] = include_bytes!("../../tools/mke2fs.conf");

const MKFS_EXT2_WRAPPER: &[u8] = b"#!/bin/sh\nreal=\"${0}.real\"\nif [ ! -x \"$real\" ]; then\n    real=\"/sbin/mkfs.ext2.real\"\nfi\nexport MKE2FS_CONFIG=\"/sbin/mke2fs.conf\"\nexec \"$real\" -F -E lazy_itable_init=1,nodiscard \"$@\"\n";
const MKFS_EXT3_WRAPPER: &[u8] = b"#!/bin/sh\nreal=\"${0}.real\"\nif [ ! -x \"$real\" ]; then\n    real=\"/sbin/mkfs.ext3.real\"\nfi\nexport MKE2FS_CONFIG=\"/sbin/mke2fs.conf\"\nexec \"$real\" -F -E lazy_itable_init=1,lazy_journal_init=1,nodiscard \"$@\"\n";
const MKFS_EXT4_WRAPPER: &[u8] = b"#!/bin/sh\nreal=\"${0}.real\"\nif [ ! -x \"$real\" ]; then\n    real=\"/sbin/mkfs.ext4.real\"\nfi\nexport MKE2FS_CONFIG=\"/sbin/mke2fs.conf\"\nexec \"$real\" -F -E lazy_itable_init=1,lazy_journal_init=1,nodiscard -O ^metadata_csum,^metadata_csum_seed,^orphan_file \"$@\"\n";

pub fn initproc_image() -> &'static [u8] {
    &INITPROC_ELF.0
}

pub fn install_runtime_files() {
    for path in [
        "/bin",
        "/sbin",
        "/lib",
        "/lib64",
        "/musl",
        "/musl/ltp",
        "/musl/ltp/testcases",
        "/musl/ltp/testcases/bin",
    ] {
        if let Err(err) = ensure_dir(path) {
            warn!("[embedded] failed to ensure {}: {:?}", path, err);
        }
    }

    install_dynamic_runtime();

    if let Err(err) = write_file("/sbin/mke2fs.conf", MKE2FS_CONF, 0o644) {
        warn!("[embedded] failed to install /sbin/mke2fs.conf: {:?}", err);
    }

    for dir in ["/bin", "/sbin", "/musl/ltp/testcases/bin"] {
        install_mkfs_tool(dir, "mkfs.ext2", MKFS_EXT2, MKFS_EXT2_WRAPPER);
        install_mkfs_tool(dir, "mkfs.ext3", MKFS_EXT3, MKFS_EXT3_WRAPPER);
        install_mkfs_tool(dir, "mkfs.ext4", MKFS_EXT4, MKFS_EXT4_WRAPPER);
    }

    info!("[embedded] runtime files installed");
}

fn install_dynamic_runtime() {
    #[cfg(target_arch = "riscv64")]
    install_riscv64_dynamic_runtime();

    #[cfg(target_arch = "loongarch64")]
    install_loongarch64_dynamic_runtime();
}

#[cfg(target_arch = "riscv64")]
fn install_riscv64_dynamic_runtime() {
    if let Err(err) = ensure_dir("/lib/riscv64-linux-gnu") {
        warn!(
            "[embedded] failed to ensure /lib/riscv64-linux-gnu: {:?}",
            err
        );
    }

    copy_file_if_exists(
        "/glibc/lib/ld-linux-riscv64-lp64d.so.1",
        "/lib/ld-linux-riscv64-lp64d.so.1",
        0o755,
    );

    for lib in [
        "libc.so.6",
        "libm.so.6",
        "libc.so",
        "libm.so",
        "libgcc_s.so.1",
    ] {
        let src = format!("/glibc/lib/{}", lib);
        copy_file_if_exists(&src, &format!("/lib/{}", lib), 0o755);
        copy_file_if_exists(&src, &format!("/lib/riscv64-linux-gnu/{}", lib), 0o755);
    }

    copy_first_existing(
        &["/musl/lib/ld-musl-riscv64-sf.so.1", "/musl/lib/libc.so"],
        "/lib/ld-musl-riscv64-sf.so.1",
        0o755,
    );

    copy_first_existing(
        &["/musl/lib/ld-musl-riscv64.so.1", "/musl/lib/libc.so"],
        "/lib/ld-musl-riscv64.so.1",
        0o755,
    );
}

#[cfg(target_arch = "loongarch64")]
fn install_loongarch64_dynamic_runtime() {
    for lib in [
        "ld-linux-loongarch-lp64d.so.1",
        "libc.so.6",
        "libm.so.6",
        "libdl.so.2",
        "libpthread.so.0",
        "libgcc_s.so.1",
    ] {
        let src = format!("/glibc/lib/{}", lib);
        copy_file_if_exists(&src, &format!("/lib64/{}", lib), 0o755);
    }

    copy_first_existing(
        &[
            "/musl/lib/ld-musl-loongarch-lp64d.so.1",
            "/musl/lib/libc.so",
        ],
        "/lib/ld-musl-loongarch-lp64d.so.1",
        0o755,
    );
    copy_file_if_exists(
        "/lib/ld-musl-loongarch-lp64d.so.1",
        "/lib64/ld-musl-loongarch-lp64d.so.1",
        0o755,
    );
}

fn copy_first_existing(srcs: &[&str], dst: &str, perm: u32) {
    for src in srcs {
        match copy_file(src, dst, perm) {
            Ok(()) => return,
            Err(SysError::ENOENT) => {}
            Err(err) => {
                warn!("[embedded] failed to copy {} to {}: {:?}", src, dst, err);
                return;
            }
        }
    }
}

fn copy_file_if_exists(src: &str, dst: &str, perm: u32) {
    match copy_file(src, dst, perm) {
        Ok(()) | Err(SysError::ENOENT) => {}
        Err(err) => warn!("[embedded] failed to copy {} to {}: {:?}", src, dst, err),
    }
}

fn copy_file(src: &str, dst: &str, perm: u32) -> SysResult<()> {
    let root = root_dentry()?;
    let src_file = open_file(root.clone(), src, OpenFlags::RDONLY, InodeMode::FILE)?;
    let mut data = alloc::vec::Vec::new();
    let mut buf = [0u8; 4096];
    let mut offset = 0usize;
    loop {
        let read_len = src_file.read_at_direct(offset, &mut buf)?;
        if read_len == 0 {
            break;
        }
        data.extend_from_slice(&buf[..read_len]);
        offset += read_len;
    }
    write_file(dst, data.as_slice(), perm)
}

fn install_mkfs_tool(dir: &str, tool: &str, real: &[u8], wrapper: &[u8]) {
    let real_path = format!("{}/{}.real", dir, tool);
    let wrapper_path = format!("{}/{}", dir, tool);
    if let Err(err) = write_file(&real_path, real, 0o755) {
        warn!("[embedded] failed to install {}: {:?}", real_path, err);
    }
    if let Err(err) = write_file(&wrapper_path, wrapper, 0o755) {
        warn!("[embedded] failed to install {}: {:?}", wrapper_path, err);
    }
}

fn ensure_dir(path: &str) -> SysResult<()> {
    if let Ok(dentry) = find_dentry(path) {
        let inode = dentry.get_inode().ok_or(SysError::EIO)?;
        return if inode.get_mode().get_type() == InodeMode::DIR {
            Ok(())
        } else {
            Err(SysError::ENOTDIR)
        };
    }

    let (parent_path, name) = split_parent_and_name(path);
    if name.is_empty() {
        return Ok(());
    }
    let parent = find_dentry(&parent_path)?;
    let mode = InodeMode::DIR | InodeMode::from_bits_truncate(0o755);
    parent.create(&name, mode)?;
    Ok(())
}

fn write_file(path: &str, data: &[u8], perm: u32) -> SysResult<()> {
    let root = root_dentry()?;
    let mode = InodeMode::FILE | InodeMode::from_bits_truncate(perm);
    let file = open_file(
        root,
        path,
        OpenFlags::O_CREAT | OpenFlags::WRONLY | OpenFlags::O_TRUNC,
        mode,
    )?;
    if let Some(inode) = file.get_inode() {
        inode.set_mode(mode);
    }

    let mut offset = 0usize;
    while offset < data.len() {
        let end = (offset + 4096).min(data.len());
        let written = file.write_at_direct(offset, &data[offset..end])?;
        if written == 0 {
            return Err(SysError::EIO);
        }
        offset += written;
    }
    file.flush();
    Ok(())
}

fn root_dentry() -> SysResult<Arc<dyn Dentry>> {
    GLOBAL_DCACHE.get("/").ok_or(SysError::ENOENT)
}
