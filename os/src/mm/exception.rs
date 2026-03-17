use super::address::*;

use super::page_table;
use super::page_table::*;
use crate::task::task::TaskControlBlock;
use crate::trap::TrapContext;
///
pub trait AreaPageFaultException{
    ///
    fn handle_store_page_fault_area(&mut self, vpn: VirtPageNum) -> Option<PhysPageNum>;
}
///
pub trait SetPageFaultException {
    ///
    fn handle_store_page_fault_set(&mut self, va: VirtAddr, trap_cx: &TrapContext) -> Option<()>;
}
