#![allow(dead_code, missing_docs)]

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

#[cfg(all(target_arch = "riscv64", board = "visionfive2"))]
static USER_SHELL_ELF: AlignedBytes<
    { include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/user_shell").len() },
> = AlignedBytes(*include_bytes!(
    "../../user/target/riscv64gc-unknown-none-elf/release/user_shell"
));
#[cfg(all(target_arch = "loongarch64", board = "visionfive2"))]
static USER_SHELL_ELF: AlignedBytes<
    { include_bytes!("../../user/target/loongarch64-unknown-none/release/user_shell").len() },
> = AlignedBytes(*include_bytes!(
    "../../user/target/loongarch64-unknown-none/release/user_shell"
));

#[cfg(all(target_arch = "riscv64", board = "visionfive2"))]
const LS_ELF: &[u8] = include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/ls");
#[cfg(all(target_arch = "loongarch64", board = "visionfive2"))]
const LS_ELF: &[u8] = include_bytes!("../../user/target/loongarch64-unknown-none/release/ls");

#[cfg(target_arch = "riscv64")]
const HTTPGET_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/httpget");
#[cfg(target_arch = "loongarch64")]
const HTTPGET_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/httpget");

#[cfg(target_arch = "riscv64")]
const SSH_TEST_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/sshtest");
#[cfg(target_arch = "loongarch64")]
const SSH_TEST_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/sshtest");

#[cfg(target_arch = "riscv64")]
const SSH_EXEC_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/sshexec");
#[cfg(target_arch = "loongarch64")]
const SSH_EXEC_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/sshexec");

#[cfg(target_arch = "riscv64")]
const SSH_SHELL_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/sshshell");
#[cfg(target_arch = "loongarch64")]
const SSH_SHELL_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/sshshell");

#[cfg(target_arch = "riscv64")]
const TCP_REGRESSION_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/tcp_regression");
#[cfg(target_arch = "loongarch64")]
const TCP_REGRESSION_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/tcp_regression");

#[cfg(target_arch = "riscv64")]
const SOCKET_SEMANTICS_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/socket_semantics");
#[cfg(target_arch = "loongarch64")]
const SOCKET_SEMANTICS_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/socket_semantics");

#[cfg(target_arch = "riscv64")]
const UDP_CHECKSUM_REGRESSION_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/udp_checksum_regression");
#[cfg(target_arch = "loongarch64")]
const UDP_CHECKSUM_REGRESSION_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/udp_checksum_regression");

#[cfg(target_arch = "riscv64")]
const SMP_LOAD_TEST_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/smp_load_test");
#[cfg(target_arch = "loongarch64")]
const SMP_LOAD_TEST_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/smp_load_test");

#[cfg(target_arch = "riscv64")]
const FP_CONTEXT_TEST_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/fp_context_test");
#[cfg(target_arch = "loongarch64")]
const FP_CONTEXT_TEST_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/fp_context_test");

#[cfg(target_arch = "riscv64")]
const SHM_FUTEX_TEST_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/shm_futex_test");
#[cfg(target_arch = "loongarch64")]
const SHM_FUTEX_TEST_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/shm_futex_test");

#[cfg(target_arch = "riscv64")]
const CONCURRENT_FSYNC_TEST_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/concurrent_fsync_test");
#[cfg(target_arch = "loongarch64")]
const CONCURRENT_FSYNC_TEST_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/concurrent_fsync_test");

#[cfg(target_arch = "riscv64")]
const GLIBC_FORK_SELECT_TEST_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/glibc_fork_select_test");
#[cfg(target_arch = "loongarch64")]
const GLIBC_FORK_SELECT_TEST_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/glibc_fork_select_test");

#[cfg(target_arch = "riscv64")]
const RSEQ_HWPROBE_TEST_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/rseq_hwprobe_test");
#[cfg(target_arch = "loongarch64")]
const RSEQ_HWPROBE_TEST_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/rseq_hwprobe_test");

#[cfg(target_arch = "riscv64")]
const IOZONE_REGRESSION_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/iozone_regression");
#[cfg(target_arch = "loongarch64")]
const IOZONE_REGRESSION_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/iozone_regression");

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
#[cfg(embed_ssh_test_key)]
const SSH_TEST_KEY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/id_ed25519"));

#[cfg(not(board = "visionfive2"))]
const RESOLV_CONF: &[u8] = b"nameserver 10.0.2.3\noptions timeout:2 attempts:2\n";
#[cfg(board = "visionfive2")]
const RESOLV_CONF: &[u8] = b"nameserver 1.1.1.1\noptions timeout:2 attempts:2\n";
#[cfg(not(board = "visionfive2"))]
const HOSTS: &[u8] = b"127.0.0.1 localhost\n10.0.2.15 kairix\n";
#[cfg(board = "visionfive2")]
const HOSTS: &[u8] = b"127.0.0.1 localhost\n192.168.10.2 kairix\n";

const MKFS_EXT2_WRAPPER: &[u8] = b"#!/bin/sh\nreal=\"${0}.real\"\nif [ ! -x \"$real\" ]; then\n    real=\"/sbin/mkfs.ext2.real\"\nfi\nexport MKE2FS_CONFIG=\"/sbin/mke2fs.conf\"\nexec \"$real\" -F -E lazy_itable_init=1,nodiscard \"$@\"\n";
const MKFS_EXT3_WRAPPER: &[u8] = b"#!/bin/sh\nreal=\"${0}.real\"\nif [ ! -x \"$real\" ]; then\n    real=\"/sbin/mkfs.ext3.real\"\nfi\nexport MKE2FS_CONFIG=\"/sbin/mke2fs.conf\"\nexec \"$real\" -F -E lazy_itable_init=1,lazy_journal_init=1,nodiscard \"$@\"\n";
const MKFS_EXT4_WRAPPER: &[u8] = b"#!/bin/sh\nreal=\"${0}.real\"\nif [ ! -x \"$real\" ]; then\n    real=\"/sbin/mkfs.ext4.real\"\nfi\nexport MKE2FS_CONFIG=\"/sbin/mke2fs.conf\"\nexec \"$real\" -F -E lazy_itable_init=1,lazy_journal_init=1,nodiscard -O ^metadata_csum,^metadata_csum_seed,^orphan_file \"$@\"\n";

pub fn initproc_image() -> &'static [u8] {
    &INITPROC_ELF.0
}

#[cfg(board = "visionfive2")]
pub fn install_runtime_files() {
    install_visionfive2_runtime_files();
}

#[cfg(not(board = "visionfive2"))]
pub fn install_runtime_files() {
    for path in [
        "/bin",
        "/sbin",
        "/lib",
        "/lib64",
        "/etc",
        "/usr",
        "/usr/lib64",
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

    if let Err(err) = write_file("/etc/resolv.conf", RESOLV_CONF, 0o644) {
        warn!("[embedded] failed to install /etc/resolv.conf: {:?}", err);
    }
    if let Err(err) = write_file("/etc/hosts", HOSTS, 0o644) {
        warn!("[embedded] failed to install /etc/hosts: {:?}", err);
    }

    if let Err(err) = write_file("/sbin/mke2fs.conf", MKE2FS_CONF, 0o644) {
        warn!("[embedded] failed to install /sbin/mke2fs.conf: {:?}", err);
    }

    #[cfg(embed_ssh_test_key)]
    {
        if let Err(err) = write_file("/musl/id_ed25519", SSH_TEST_KEY, 0o600) {
            warn!("[embedded] failed to install /musl/id_ed25519: {:?}", err);
        }
    }

    #[cfg(not(embed_ssh_test_key))]
    info!("[embedded] SSH test key not embedded; set KAIRIX_SSH_TEST_KEY to install it");

    install_embedded_app("httpget", HTTPGET_ELF);
    install_embedded_app("sshtest", SSH_TEST_ELF);
    install_embedded_app("sshexec", SSH_EXEC_ELF);
    install_embedded_app("sshshell", SSH_SHELL_ELF);
    install_embedded_app("tcp_regression", TCP_REGRESSION_ELF);
    install_embedded_app("socket_semantics", SOCKET_SEMANTICS_ELF);
    install_embedded_app("udp_checksum_regression", UDP_CHECKSUM_REGRESSION_ELF);
    install_embedded_app("smp_load_test", SMP_LOAD_TEST_ELF);
    install_embedded_app("fp_context_test", FP_CONTEXT_TEST_ELF);
    install_embedded_app("shm_futex_test", SHM_FUTEX_TEST_ELF);
    install_embedded_app("concurrent_fsync_test", CONCURRENT_FSYNC_TEST_ELF);
    install_embedded_app("glibc_fork_select_test", GLIBC_FORK_SELECT_TEST_ELF);
    install_embedded_app("rseq_hwprobe_test", RSEQ_HWPROBE_TEST_ELF);
    install_embedded_app("iozone_regression", IOZONE_REGRESSION_ELF);

    for dir in ["/bin", "/sbin", "/musl/ltp/testcases/bin"] {
        install_mkfs_tool(dir, "mkfs.ext2", MKFS_EXT2, MKFS_EXT2_WRAPPER);
        install_mkfs_tool(dir, "mkfs.ext3", MKFS_EXT3, MKFS_EXT3_WRAPPER);
        install_mkfs_tool(dir, "mkfs.ext4", MKFS_EXT4, MKFS_EXT4_WRAPPER);
    }

    info!("[embedded] runtime files installed");
}

#[cfg(board = "visionfive2")]
fn install_visionfive2_runtime_files() {
    for path in [
        "/bin",
        "/etc",
        "/tmp",
        "/musl",
        "/musl/ltp",
        "/musl/ltp/testcases",
        "/musl/ltp/testcases/bin",
    ] {
        if let Err(err) = ensure_dir(path) {
            warn!("[embedded] failed to ensure {}: {:?}", path, err);
        }
    }

    if let Err(err) = write_file("/.initproc-no-autotest", b"", 0o644) {
        warn!("[embedded] failed to disable autotest: {:?}", err);
    }
    if let Err(err) = write_file("/bin/user_shell", &USER_SHELL_ELF.0, 0o755) {
        warn!("[embedded] failed to install /bin/user_shell: {:?}", err);
    }
    if !file_exists("/bin/sh") {
        if let Err(err) = write_file("/bin/sh", &USER_SHELL_ELF.0, 0o755) {
            warn!("[embedded] failed to install /bin/sh: {:?}", err);
        }
    }
    if !file_exists("/bin/ls") {
        if let Err(err) = write_file("/bin/ls", LS_ELF, 0o755) {
            warn!("[embedded] failed to install /bin/ls: {:?}", err);
        }
    }
    if let Err(err) = write_file("/etc/resolv.conf", RESOLV_CONF, 0o644) {
        warn!("[embedded] failed to install /etc/resolv.conf: {:?}", err);
    }
    if let Err(err) = write_file("/etc/hosts", HOSTS, 0o644) {
        warn!("[embedded] failed to install /etc/hosts: {:?}", err);
    }

    for dir in ["/musl/ltp/testcases/bin"] {
        install_mkfs_tool(dir, "mkfs.ext2", MKFS_EXT2, MKFS_EXT2_WRAPPER);
        install_mkfs_tool(dir, "mkfs.ext3", MKFS_EXT3, MKFS_EXT3_WRAPPER);
        install_mkfs_tool(dir, "mkfs.ext4", MKFS_EXT4, MKFS_EXT4_WRAPPER);
    }

    info!("[embedded] VisionFive 2 minimal runtime files installed");
}

fn install_dynamic_runtime() {
    #[cfg(target_arch = "riscv64")]
    install_riscv64_dynamic_runtime();

    #[cfg(target_arch = "loongarch64")]
    install_loongarch64_dynamic_runtime();
}

#[cfg(target_arch = "riscv64")]
fn install_riscv64_dynamic_runtime() {
    // The bundled test runtime may be older than the disk's distro runtime.
    // Install it only as a complete fallback, never mix it into a distro
    // glibc or musl runtime.
    let system_musl_present = file_exists("/lib/ld-musl-riscv64.so.1");
    let system_glibc_present = file_exists("/lib/riscv64-linux-gnu/libc.so.6");

    if system_musl_present {
        if file_exists("/lib/libgcc_s.so.1") {
            // Older kernels may have copied the bundled glibc libgcc into /lib.
            // Replace that stale file with Alpine's matching musl runtime library.
            copy_file_if_exists("/usr/lib/libgcc_s.so.1", "/lib/libgcc_s.so.1", 0o755);
        }
    } else if !system_glibc_present {
        if let Err(err) = ensure_dir("/lib/riscv64-linux-gnu") {
            warn!(
                "[embedded] failed to ensure /lib/riscv64-linux-gnu: {:?}",
                err
            );
        }

        copy_file_if_missing(
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
            copy_file_if_missing(&src, &format!("/lib/{}", lib), 0o755);
            copy_file_if_missing(&src, &format!("/lib/riscv64-linux-gnu/{}", lib), 0o755);
        }
    }

    copy_first_existing_if_missing(
        &["/musl/lib/ld-musl-riscv64-sf.so.1", "/musl/lib/libc.so"],
        "/lib/ld-musl-riscv64-sf.so.1",
        0o755,
    );

    copy_first_existing_if_missing(
        &["/musl/lib/ld-musl-riscv64.so.1", "/musl/lib/libc.so"],
        "/lib/ld-musl-riscv64.so.1",
        0o755,
    );
}

#[cfg(target_arch = "loongarch64")]
fn install_loongarch64_dynamic_runtime() {
    // Keep the distro loader and libraries as one ABI-compatible runtime set.
    let system_glibc_present = [
        "/lib64/libc.so.6",
        "/usr/lib64/libc.so.6",
        "/lib/loongarch64-linux-gnu/libc.so.6",
        "/usr/lib/loongarch64-linux-gnu/libc.so.6",
    ]
    .iter()
    .any(|path| file_exists(path));
    let system_musl_present = file_exists("/lib/ld-musl-loongarch-lp64d.so.1");

    if !system_glibc_present && !system_musl_present {
        for lib in [
            "ld-linux-loongarch-lp64d.so.1",
            "libc.so.6",
            "libm.so.6",
            "libdl.so.2",
            "libpthread.so.0",
            "libgcc_s.so.1",
        ] {
            let src = format!("/glibc/lib/{}", lib);
            copy_file_if_missing(&src, &format!("/lib64/{}", lib), 0o755);
            copy_file_if_missing(&src, &format!("/usr/lib64/{}", lib), 0o755);
        }
    }

    copy_first_existing_if_missing(
        &[
            "/musl/lib/ld-musl-loongarch-lp64d.so.1",
            "/musl/lib/libc.so",
        ],
        "/lib/ld-musl-loongarch-lp64d.so.1",
        0o755,
    );
    copy_file_if_missing(
        "/lib/ld-musl-loongarch-lp64d.so.1",
        "/lib64/ld-musl-loongarch-lp64d.so.1",
        0o755,
    );
    copy_file_if_missing(
        "/lib/ld-musl-loongarch-lp64d.so.1",
        "/usr/lib64/ld-musl-loongarch-lp64d.so.1",
        0o755,
    );
}

fn copy_first_existing_if_missing(srcs: &[&str], dst: &str, perm: u32) {
    if file_exists(dst) {
        return;
    }

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

fn copy_file_if_missing(src: &str, dst: &str, perm: u32) {
    if file_exists(dst) {
        return;
    }

    match copy_file(src, dst, perm) {
        Ok(()) | Err(SysError::ENOENT) => {}
        Err(err) => warn!("[embedded] failed to copy {} to {}: {:?}", src, dst, err),
    }
}

fn copy_file_if_exists(src: &str, dst: &str, perm: u32) {
    match copy_file(src, dst, perm) {
        Ok(()) | Err(SysError::ENOENT) => {}
        Err(err) => warn!("[embedded] failed to copy {} to {}: {:?}", src, dst, err),
    }
}

fn file_exists(path: &str) -> bool {
    let Ok(root) = root_dentry() else {
        return false;
    };
    open_file(root, path, OpenFlags::RDONLY, InodeMode::FILE).is_ok()
}

fn copy_file(src: &str, dst: &str, perm: u32) -> SysResult<()> {
    let root = root_dentry()?;
    let src_file = open_file(root.clone(), src, OpenFlags::RDONLY, InodeMode::FILE)?;
    let mode = InodeMode::FILE | InodeMode::from_bits_truncate(perm);
    let dst_file = open_file(
        root,
        dst,
        OpenFlags::O_CREAT | OpenFlags::WRONLY | OpenFlags::O_TRUNC,
        mode,
    )?;
    if let Some(inode) = dst_file.get_inode() {
        inode.set_mode(mode);
    }

    let mut buf = [0u8; 4096];
    let mut src_offset = 0usize;
    let mut dst_offset = 0usize;
    loop {
        let read_len = src_file.read_at_direct(src_offset, &mut buf)?;
        if read_len == 0 {
            break;
        }
        src_offset += read_len;

        let mut written_total = 0usize;
        while written_total < read_len {
            let written = dst_file.write_at_direct(dst_offset, &buf[written_total..read_len])?;
            if written == 0 {
                return Err(SysError::EIO);
            }
            written_total += written;
            dst_offset += written;
        }
    }
    dst_file.flush();
    Ok(())
}

fn install_embedded_app(name: &str, data: &[u8]) {
    let root_path = format!("/{}", name);
    let bin_path = format!("/bin/{}", name);
    if let Err(err) = write_file(&root_path, data, 0o755) {
        warn!("[embedded] failed to install {}: {:?}", root_path, err);
    }
    if let Err(err) = write_file(&bin_path, data, 0o755) {
        warn!("[embedded] failed to install {}: {:?}", bin_path, err);
    }
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
