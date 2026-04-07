#[cfg(target_arch = "riscv64")]
pub mod riscv;
#[allow(missing_docs)]
#[cfg(target_arch = "riscv64")]
pub mod riscv_dir;
// #[cfg(target_arch = "loongarch64")]
// pub mod loongarch_dir;
#[cfg(target_arch = "loongarch64")]
pub mod loongarch64;
pub struct TLB;

// #[macro_export]
// macro_rules! define_entry {
//     ($main_fn: ident) => {
//         #[unsafe(no_mangle)]
//         extern "Rust" fn _main_for_arch(id: usize, first: bool) -> bool {
//             $main_fn(id, first)
//         }
//     };
// }
