use fatfs::Dir;
use crate::fs::fat32::io::FatIoAdapter;
use fatfs::{LossyOemCpConverter,NullTimeProvider};
pub struct Fat32Dir(Dir<'static, FatIoAdapter, NullTimeProvider, LossyOemCpConverter>);

