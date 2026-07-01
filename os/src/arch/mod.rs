#[cfg(target_arch = "loongarch64")]
pub mod loongarch64;
#[cfg(target_arch = "riscv64")]
pub mod riscv;
#[allow(missing_docs)]
#[cfg(target_arch = "riscv64")]
pub mod riscv_dir;
pub struct TLB;
