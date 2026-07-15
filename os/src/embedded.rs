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
const HTTPGET_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/httpget");
#[cfg(target_arch = "loongarch64")]
const HTTPGET_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/httpget");

#[cfg(target_arch = "riscv64")]
const HTTPSGET_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/httpsget");
#[cfg(target_arch = "loongarch64")]
const HTTPSGET_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/httpsget");

#[cfg(target_arch = "riscv64")]
const GITPKT_TEST_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/gitpkt_test");
#[cfg(target_arch = "loongarch64")]
const GITPKT_TEST_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/gitpkt_test");

#[cfg(target_arch = "riscv64")]
const GIT_ELF: &[u8] = include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/git");
#[cfg(target_arch = "loongarch64")]
const GIT_ELF: &[u8] = include_bytes!("../../user/target/loongarch64-unknown-none/release/git");

#[cfg(target_arch = "riscv64")]
const GITINIT_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/gitinit");
#[cfg(target_arch = "loongarch64")]
const GITINIT_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/gitinit");

#[cfg(target_arch = "riscv64")]
const GITADD_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/gitadd");
#[cfg(target_arch = "loongarch64")]
const GITADD_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/gitadd");

#[cfg(target_arch = "riscv64")]
const GITCOMMIT_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/gitcommit");
#[cfg(target_arch = "loongarch64")]
const GITCOMMIT_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/gitcommit");

#[cfg(target_arch = "riscv64")]
const GITCONFIG_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/gitconfig");
#[cfg(target_arch = "loongarch64")]
const GITCONFIG_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/gitconfig");

#[cfg(target_arch = "riscv64")]
const GITPUSH_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/gitpush");
#[cfg(target_arch = "loongarch64")]
const GITPUSH_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/gitpush");

#[cfg(target_arch = "riscv64")]
const GITLS_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/gitls");
#[cfg(target_arch = "loongarch64")]
const GITLS_ELF: &[u8] = include_bytes!("../../user/target/loongarch64-unknown-none/release/gitls");

#[cfg(target_arch = "riscv64")]
const GITFETCH_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/gitfetch");
#[cfg(target_arch = "loongarch64")]
const GITFETCH_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/gitfetch");

#[cfg(target_arch = "riscv64")]
const GITPACK_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/gitpack");
#[cfg(target_arch = "loongarch64")]
const GITPACK_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/gitpack");

#[cfg(target_arch = "riscv64")]
const GITCHECKOUT_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/gitcheckout");
#[cfg(target_arch = "loongarch64")]
const GITCHECKOUT_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/gitcheckout");

#[cfg(target_arch = "riscv64")]
const GITCHECKOUTREF_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/gitcheckoutref");
#[cfg(target_arch = "loongarch64")]
const GITCHECKOUTREF_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/gitcheckoutref");

#[cfg(target_arch = "riscv64")]
const GITBRANCH_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/gitbranch");
#[cfg(target_arch = "loongarch64")]
const GITBRANCH_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/gitbranch");

#[cfg(target_arch = "riscv64")]
const GITCLONE_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/gitclone");
#[cfg(target_arch = "loongarch64")]
const GITCLONE_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/gitclone");

#[cfg(target_arch = "riscv64")]
const GITPULL_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/gitpull");
#[cfg(target_arch = "loongarch64")]
const GITPULL_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/gitpull");

#[cfg(target_arch = "riscv64")]
const GITREMOTE_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/gitremote");
#[cfg(target_arch = "loongarch64")]
const GITREMOTE_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/gitremote");

#[cfg(target_arch = "riscv64")]
const GITLOG_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/gitlog");
#[cfg(target_arch = "loongarch64")]
const GITLOG_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/gitlog");

#[cfg(target_arch = "riscv64")]
const GITSTATUS_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/gitstatus");
#[cfg(target_arch = "loongarch64")]
const GITSTATUS_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/gitstatus");

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

#[cfg(target_arch = "riscv64")]
const GCC_WRAPPER_ELF: &[u8] =
    include_bytes!("../../user/target/riscv64gc-unknown-none-elf/release/gcc");
#[cfg(target_arch = "riscv64")]
const GCC_TOOLCHAIN_BUSYBOX: &[u8] =
    include_bytes!("../../tools/target/gcc-riscv64/gcc-toolchain-busybox");
#[cfg(target_arch = "riscv64")]
const GCC_MUSL_LOADER: &[u8] =
    include_bytes!("../../tools/target/gcc-riscv64/rootfs/lib/ld-musl-riscv64.so.1");
#[cfg(target_arch = "riscv64")]
const GCC_TOOLCHAIN_ARCHIVE_PATH: &str = "/.gcc-toolchain-riscv64.tar.gz";
#[cfg(target_arch = "riscv64")]
const GCC_MUSL_LOADER_PATH: &str = "/lib/ld-musl-riscv64.so.1";
#[cfg(target_arch = "riscv64")]
const GCC_CC1_PATH: &str = "/usr/libexec/gcc/riscv64-alpine-linux-musl/14.2.0/cc1";
#[cfg(target_arch = "riscv64")]
const GCC_ARCH_LABEL: &str = "RISC-V";

#[cfg(target_arch = "loongarch64")]
const GCC_WRAPPER_ELF: &[u8] =
    include_bytes!("../../user/target/loongarch64-unknown-none/release/gcc");
#[cfg(target_arch = "loongarch64")]
const GCC_TOOLCHAIN_BUSYBOX: &[u8] =
    include_bytes!("../../tools/target/gcc-loongarch64/gcc-toolchain-busybox");
#[cfg(target_arch = "loongarch64")]
const GCC_MUSL_LOADER: &[u8] =
    include_bytes!("../../tools/target/gcc-loongarch64/rootfs/lib/ld-musl-loongarch64.so.1");
#[cfg(target_arch = "loongarch64")]
const GCC_TOOLCHAIN_ARCHIVE_PATH: &str = "/.gcc-toolchain-loongarch64.tar.gz";
#[cfg(target_arch = "loongarch64")]
const GCC_MUSL_LOADER_PATH: &str = "/lib/ld-musl-loongarch64.so.1";
#[cfg(target_arch = "loongarch64")]
const GCC_CC1_PATH: &str = "/usr/libexec/gcc/loongarch64-alpine-linux-musl/14.2.0/cc1";
#[cfg(target_arch = "loongarch64")]
const GCC_ARCH_LABEL: &str = "LoongArch64";

const GCC_TOOLCHAIN_BUSYBOX_PATH: &str = "/bin/gcc-toolchain-busybox";

#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(include_str!("gcc_toolchain.S"));
#[cfg(target_arch = "loongarch64")]
core::arch::global_asm!(include_str!("gcc_toolchain_loongarch64.S"));

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
unsafe extern "C" {
    static gcc_toolchain_archive_start: u8;
    static gcc_toolchain_archive_end: u8;
}

const MKE2FS_CONF: &[u8] = include_bytes!("../../tools/mke2fs.conf");
#[cfg(embed_ssh_test_key)]
const SSH_TEST_KEY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/id_ed25519"));

const RESOLV_CONF: &[u8] = b"nameserver 10.0.2.3\noptions timeout:2 attempts:2\n";
const HOSTS: &[u8] = b"127.0.0.1 localhost\n10.0.2.15 kairix\n";

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

    #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
    install_gcc_payload();

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
    install_embedded_app("httpsget", HTTPSGET_ELF);
    install_embedded_app("gitpkt_test", GITPKT_TEST_ELF);
    install_embedded_app("git", GIT_ELF);
    install_embedded_app("gitinit", GITINIT_ELF);
    install_embedded_app("gitadd", GITADD_ELF);
    install_embedded_app("gitcommit", GITCOMMIT_ELF);
    install_embedded_app("gitconfig", GITCONFIG_ELF);
    install_embedded_app("gitpush", GITPUSH_ELF);
    install_embedded_app("gitls", GITLS_ELF);
    install_embedded_app("gitfetch", GITFETCH_ELF);
    install_embedded_app("gitpack", GITPACK_ELF);
    install_embedded_app("gitcheckout", GITCHECKOUT_ELF);
    install_embedded_app("gitcheckoutref", GITCHECKOUTREF_ELF);
    install_embedded_app("gitbranch", GITBRANCH_ELF);
    install_embedded_app("gitclone", GITCLONE_ELF);
    install_embedded_app("gitpull", GITPULL_ELF);
    install_embedded_app("gitremote", GITREMOTE_ELF);
    install_embedded_app("gitlog", GITLOG_ELF);
    install_embedded_app("gitstatus", GITSTATUS_ELF);
    install_embedded_app("sshtest", SSH_TEST_ELF);
    install_embedded_app("sshexec", SSH_EXEC_ELF);
    install_embedded_app("sshshell", SSH_SHELL_ELF);
    install_embedded_app("tcp_regression", TCP_REGRESSION_ELF);
    install_embedded_app("socket_semantics", SOCKET_SEMANTICS_ELF);
    install_embedded_app("udp_checksum_regression", UDP_CHECKSUM_REGRESSION_ELF);

    #[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
    install_embedded_app("gcc", GCC_WRAPPER_ELF);

    for dir in ["/bin", "/sbin", "/musl/ltp/testcases/bin"] {
        install_mkfs_tool(dir, "mkfs.ext2", MKFS_EXT2, MKFS_EXT2_WRAPPER);
        install_mkfs_tool(dir, "mkfs.ext3", MKFS_EXT3, MKFS_EXT3_WRAPPER);
        install_mkfs_tool(dir, "mkfs.ext4", MKFS_EXT4, MKFS_EXT4_WRAPPER);
    }

    info!("[embedded] runtime files installed");
}

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
fn install_gcc_payload() {
    if let Err(err) = write_file(GCC_MUSL_LOADER_PATH, GCC_MUSL_LOADER, 0o755) {
        warn!("[embedded] failed to install GCC musl loader: {:?}", err);
    }

    let compiler_ready = find_dentry("/usr/bin/gcc").is_ok() && find_dentry(GCC_CC1_PATH).is_ok();
    if compiler_ready {
        info!(
            "[embedded] {} GCC toolchain is already installed",
            GCC_ARCH_LABEL
        );
        return;
    }

    if let Err(err) = write_file(GCC_TOOLCHAIN_BUSYBOX_PATH, GCC_TOOLCHAIN_BUSYBOX, 0o755) {
        warn!(
            "[embedded] failed to install GCC archive unpacker at {}: {:?}",
            GCC_TOOLCHAIN_BUSYBOX_PATH, err
        );
        return;
    }

    let archive = gcc_toolchain_archive();
    if let Err(err) = write_file(GCC_TOOLCHAIN_ARCHIVE_PATH, archive, 0o644) {
        warn!(
            "[embedded] failed to install GCC archive at {}: {:?}",
            GCC_TOOLCHAIN_ARCHIVE_PATH, err
        );
        return;
    }

    info!(
        "[embedded] staged {} GCC archive ({} bytes)",
        GCC_ARCH_LABEL,
        archive.len(),
    );
}

#[cfg(any(target_arch = "riscv64", target_arch = "loongarch64"))]
fn gcc_toolchain_archive() -> &'static [u8] {
    let start = core::ptr::addr_of!(gcc_toolchain_archive_start) as usize;
    let end = core::ptr::addr_of!(gcc_toolchain_archive_end) as usize;
    unsafe { core::slice::from_raw_parts(start as *const u8, end - start) }
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
        copy_file_if_exists(&src, &format!("/usr/lib64/{}", lib), 0o755);
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
    copy_file_if_exists(
        "/lib/ld-musl-loongarch-lp64d.so.1",
        "/usr/lib64/ld-musl-loongarch-lp64d.so.1",
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
