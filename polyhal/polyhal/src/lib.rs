#![no_std]
#![no_main]
#![feature(naked_functions)]
#![feature(cfg_version)]
#![feature(decl_macro)]
#![feature(used_with_arg)]
#![feature(step_trait)]
#![cfg_attr(target_arch = "riscv64", feature(riscv_ext_intrinsics))]
#![cfg_attr(target_arch = "x86_64", feature(abi_x86_interrupt))]

// extern crate alloc;
extern crate log;
extern crate alloc;

#[macro_use]
pub mod ctor;
#[macro_use]
pub mod debug_console;
#[macro_use]
pub mod utils;

pub mod arch;
pub use arch::*;
mod components;
pub mod mem;
pub use components::*;
pub mod pagetable;

pub use utils::addr::{PhysAddr, VirtAddr};

#[cfg(feature = "boot")]
pub use polyhal_macro::arch_entry;
#[cfg(feature = "trap")]
pub use polyhal_macro::arch_interrupt;
pub use polyhal_macro::percpu;

// Re export the Module like Structure.
pub use pagetable::{MappingFlags, MappingSize, PageTable};
