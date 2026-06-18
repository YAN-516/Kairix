use polyhal::consts::VIRT_ADDR_START;

const VIRTIO_MMIO_MAGIC: u32 = 0x7472_6976;
const VIRTIO_MMIO_VERSION_LEGACY: u32 = 1;
const VIRTIO_MMIO_VERSION_MODERN: u32 = 2;
const VIRTIO_MMIO_DEVICE_ID_NET: u32 = 1;
const VIRTIO_MMIO_GUEST_PAGE_SIZE: u32 = 4096;

const VIRTIO_MMIO_BASE_START: usize = 0x1000_1000;
const VIRTIO_MMIO_DEVICE_STRIDE: usize = 0x1000;
const VIRTIO_MMIO_DEVICE_COUNT: usize = 8;

const MMIO_MAGIC_VALUE: usize = 0x000;
const MMIO_VERSION: usize = 0x004;
const MMIO_DEVICE_ID: usize = 0x008;
const MMIO_DRIVER_FEATURES: usize = 0x020;
const MMIO_DRIVER_FEATURES_SEL: usize = 0x024;
const MMIO_GUEST_PAGE_SIZE: usize = 0x028;
const MMIO_QUEUE_SEL: usize = 0x030;
const MMIO_QUEUE_NUM_MAX: usize = 0x034;
const MMIO_QUEUE_NUM: usize = 0x038;
const MMIO_QUEUE_ALIGN: usize = 0x03c;
const MMIO_QUEUE_PFN: usize = 0x040;
const MMIO_QUEUE_READY: usize = 0x044;
const MMIO_QUEUE_NOTIFY: usize = 0x050;
const MMIO_STATUS: usize = 0x070;
const MMIO_QUEUE_DESC_LOW: usize = 0x080;
const MMIO_QUEUE_DESC_HIGH: usize = 0x084;
const MMIO_QUEUE_DRIVER_LOW: usize = 0x090;
const MMIO_QUEUE_DRIVER_HIGH: usize = 0x094;
const MMIO_QUEUE_DEVICE_LOW: usize = 0x0a0;
const MMIO_QUEUE_DEVICE_HIGH: usize = 0x0a4;
const MMIO_CONFIG_SPACE: usize = 0x100;

#[derive(Clone, Copy)]
pub(crate) struct MmioNetTransport {
    base: *mut u8,
    phys_base: usize,
    version: u32,
}

unsafe impl Send for MmioNetTransport {}
unsafe impl Sync for MmioNetTransport {}

impl MmioNetTransport {
    fn new(phys_base: usize) -> Option<Self> {
        let base = (phys_base + VIRT_ADDR_START) as *mut u8;
        let transport = Self {
            base,
            phys_base,
            version: 0,
        };

        if transport.read32(MMIO_MAGIC_VALUE) != VIRTIO_MMIO_MAGIC {
            return None;
        }
        if transport.read32(MMIO_DEVICE_ID) != VIRTIO_MMIO_DEVICE_ID_NET {
            return None;
        }
        let version = transport.read32(MMIO_VERSION);
        if version != VIRTIO_MMIO_VERSION_LEGACY && version != VIRTIO_MMIO_VERSION_MODERN {
            log::info!("virtio-mmio net unsupported transport version {}", version);
            return None;
        }

        log::error!("Found VirtIO-mmio net device version {}", version);
        Some(Self {
            base,
            phys_base,
            version,
        })
    }

    #[inline]
    fn read32(&self, offset: usize) -> u32 {
        unsafe { self.base.add(offset).cast::<u32>().read_volatile() }
    }

    #[inline]
    fn write32(&self, offset: usize, value: u32) {
        unsafe {
            self.base.add(offset).cast::<u32>().write_volatile(value);
        }
    }

    pub(crate) fn device_config(&self) -> *mut u8 {
        unsafe { self.base.add(MMIO_CONFIG_SPACE) }
    }

    pub(crate) fn reset(&self) {
        self.write32(MMIO_STATUS, 0);
    }

    pub(crate) fn add_status(&self, status: u8) {
        let current = self.read32(MMIO_STATUS);
        self.write32(MMIO_STATUS, current | status as u32);
    }

    pub(crate) fn status(&self) -> u8 {
        self.read32(MMIO_STATUS) as u8
    }

    pub(crate) fn write_driver_features(&self, features: u64) {
        self.write32(MMIO_DRIVER_FEATURES_SEL, 0);
        self.write32(MMIO_DRIVER_FEATURES, features as u32);
        self.write32(MMIO_DRIVER_FEATURES_SEL, 1);
        self.write32(MMIO_DRIVER_FEATURES, (features >> 32) as u32);
    }

    pub(crate) fn is_legacy(&self) -> bool {
        self.version == VIRTIO_MMIO_VERSION_LEGACY
    }

    pub(crate) fn max_queue_size(&self, queue_idx: u16) -> u16 {
        self.write32(MMIO_QUEUE_SEL, queue_idx as u32);
        self.read32(MMIO_QUEUE_NUM_MAX) as u16
    }

    pub(crate) fn setup_queue(
        &self,
        queue_idx: u16,
        queue_size: u16,
        desc_pa: u64,
        avail_pa: u64,
        used_pa: u64,
    ) {
        self.write32(MMIO_QUEUE_SEL, queue_idx as u32);
        self.write32(MMIO_QUEUE_NUM, queue_size as u32);
        if self.version == VIRTIO_MMIO_VERSION_LEGACY {
            let _ = avail_pa;
            let _ = used_pa;
            self.write32(MMIO_GUEST_PAGE_SIZE, VIRTIO_MMIO_GUEST_PAGE_SIZE);
            self.write32(MMIO_QUEUE_ALIGN, VIRTIO_MMIO_GUEST_PAGE_SIZE);
            self.write32(
                MMIO_QUEUE_PFN,
                (desc_pa / VIRTIO_MMIO_GUEST_PAGE_SIZE as u64) as u32,
            );
        } else {
            self.write32(MMIO_QUEUE_DESC_LOW, desc_pa as u32);
            self.write32(MMIO_QUEUE_DESC_HIGH, (desc_pa >> 32) as u32);
            self.write32(MMIO_QUEUE_DRIVER_LOW, avail_pa as u32);
            self.write32(MMIO_QUEUE_DRIVER_HIGH, (avail_pa >> 32) as u32);
            self.write32(MMIO_QUEUE_DEVICE_LOW, used_pa as u32);
            self.write32(MMIO_QUEUE_DEVICE_HIGH, (used_pa >> 32) as u32);
            self.write32(MMIO_QUEUE_READY, 1);
        }
    }

    pub(crate) fn notify(&self, queue_idx: u16) {
        self.write32(MMIO_QUEUE_NOTIFY, queue_idx as u32);
    }

    #[allow(unused)]
    pub(crate) fn phys_base(&self) -> usize {
        self.phys_base
    }
}

pub(crate) fn probe_virtio_net() -> Option<MmioNetTransport> {
    for idx in 0..VIRTIO_MMIO_DEVICE_COUNT {
        let base = VIRTIO_MMIO_BASE_START + idx * VIRTIO_MMIO_DEVICE_STRIDE;
        if let Some(transport) = MmioNetTransport::new(base) {
            return Some(transport);
        }
    }
    None
}
