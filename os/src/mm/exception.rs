use super::address::*;

use super::page_table;
use super::page_table::*;
use super::vm_set::AccessType;
use super::UserMapArea;
use crate::task::task::TaskControlBlock;
// use crate::trap::TrapContext;
///
pub trait AreaPageFaultException{
    ///
    fn handle_cow_fault(&mut self, vpn: VirtPageNum) -> Option<PhysPageNum>;
}
///
pub trait SetPageFaultException {
    ///
    fn handle_store_page_fault_set(&mut self, va: VirtAddr, accsess: AccessType) -> Option<()>;
    ///
    fn handle_cow_page_fault(&mut self, va: VirtAddr) -> Option<()>;
    ///
    fn handle_unalloc_page_fault(&mut self, va: VirtAddr) -> Option<()>;
}
