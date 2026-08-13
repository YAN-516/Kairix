# Top-level Makefile for Kairix OS
# Delegates to os/Makefile for actual builds

.PHONY: all rkernel rkernel_test lkernel lkernel_test lkernel_board lkernel_board_small lkernel_board_gcc lkernel_board_sata help mkfs-tools clean-mkfs clean

LOG ?= OFF
BOARD ?= qemu
AUTO_TEST ?= final
RV_CPU ?= $(or $(CPU),8)
RV_MEM ?= $(or $(MEM),8G)
LA_CPU ?= $(or $(CPU),12)
LA_MEM ?= $(or $(MEM),36G)
FILE_RV ?= sdcard-rv.img
FILE_LA ?= sdcard-la.img
UIMAGE_REF ?= uImage
UIMAGE_OUT ?= kairix-uimage
BOARD_INITRD_MAX ?= 268435456
BOARD_ROOTFS_IMG ?= kairix-2k1000-rootfs.img
BOARD_ROOTFS_SIZE ?= 120M
BOARD_GCC_ROOTFS ?= tools/target/gcc-loongarch64/rootfs
RV_SDCARD_IMG = $(abspath $(FILE_RV))
LA_SDCARD_IMG = $(abspath $(FILE_LA))

BOARD_ROOTFS_IMAGE = $(abspath $(BOARD_ROOTFS_IMG))
RKERNEL_QEMU := qemu-system-riscv64 -machine virt -kernel kernel-rv -m $(RV_MEM) -nographic -smp $(RV_CPU) -bios default -drive file=$(RV_SDCARD_IMG),if=none,format=raw,id=x0 -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 -no-reboot -device virtio-net-device,netdev=net -netdev user,id=net -rtc base=utc
LKERNEL_QEMU := qemu-system-loongarch64 -kernel kernel-la -m $(LA_MEM) -nographic -smp $(LA_CPU) -drive file=$(LA_SDCARD_IMG),if=none,format=raw,id=x0 -device virtio-blk-pci,drive=x0 -no-reboot -device virtio-net-pci,netdev=net0 -netdev user,id=net0 -rtc base=utc

help:
	@echo "Available targets:"
	@echo "  make rkernel [LOG=INFO] [RV_MEM=8G] [RV_CPU=8] [BOARD=qemu] - Build/run RISC-V with auto tests disabled"
	@echo "  make rkernel_test [AUTO_TEST=final|preliminary|off] - Build/run RISC-V competition mode"
	@echo "  make lkernel [LOG=INFO] [LA_MEM=8G] [LA_CPU=12] [BOARD=qemu|2k1000] - Build/run LoongArch with auto tests disabled"
	@echo "  make lkernel_board FILE_LA=... [LOG=OFF] - Build LoongArch artifacts for 2k1000 USB boot"
	@echo "  make lkernel_board_small [LOG=OFF] - Create a small 2k1000 initrd rootfs and board uImage"
	@echo "  make lkernel_board_gcc [LOG=OFF] - Create a 2k1000 initrd with a minimal native GCC toolchain"
	@echo "  make lkernel_board_sata FILE_LA=... [LOG=INFO] - Build a 2k1000 kernel and prepare an ext4 SATA rootfs"
	@echo "  make lkernel_test - Build/run LoongArch competition mode with LOG=OFF and auto tests enabled"
	@echo "  make all      - Build both kernels and patch sdcard images when present"
	@echo "  make mkfs-tools - Build mkfs.ext2/ext3/ext4 tools for both architectures"

# Local RISC-V run: keep kernel logs visible and start the interactive shell.
rkernel:
	$(MAKE) -C os ARCH=riscv64 BOARD=$(BOARD) LOG=$(LOG) build
	cp os/target/riscv64gc-unknown-none-elf/release/os kernel-rv
	$(MAKE) -C os ARCH=riscv64 BOARD=$(BOARD) AUTO_TEST=0 SDCARD_IMG=$(RV_SDCARD_IMG) patch-sdcard
	$(RKERNEL_QEMU)

# Competition-style RISC-V run: auto tests enabled and kernel logs compiled out.
rkernel_test:
	$(MAKE) -C os ARCH=riscv64 BOARD=$(BOARD) LOG=$(LOG) build
	cp os/target/riscv64gc-unknown-none-elf/release/os kernel-rv
	$(MAKE) -C os ARCH=riscv64 BOARD=$(BOARD) AUTO_TEST=$(AUTO_TEST) SDCARD_IMG=$(RV_SDCARD_IMG) patch-sdcard
	$(RKERNEL_QEMU)

# Local LoongArch run: keep kernel logs visible and start the interactive shell.
lkernel:
	$(MAKE) -C os ARCH=loongarch64 BOARD=$(BOARD) LOG=$(LOG) build
	cp os/target/loongarch64-unknown-none/release/os kernel-la
	$(MAKE) -C os ARCH=loongarch64 BOARD=$(BOARD) AUTO_TEST=0 SDCARD_IMG=$(LA_SDCARD_IMG) patch-sdcard
	$(LKERNEL_QEMU)

# Competition-style LoongArch run: auto tests enabled and kernel logs compiled out.
lkernel_test:
	$(MAKE) -C os ARCH=loongarch64 BOARD=$(BOARD) LOG=$(LOG) build
	cp os/target/loongarch64-unknown-none/release/os kernel-la
	$(MAKE) -C os ARCH=loongarch64 BOARD=$(BOARD) AUTO_TEST=$(AUTO_TEST) SDCARD_IMG=$(LA_SDCARD_IMG) patch-sdcard
	$(LKERNEL_QEMU)

# 2k1000 board artifacts. U-Boot loads /install/uImage and /install/ramdisk.gz;
# ramdisk.gz is only a filename here, keep the ext4 image uncompressed unless
# gzip initrd decompression is implemented in the kernel.
lkernel_board:
	$(MAKE) -C os ARCH=loongarch64 BOARD=2k1000 LOG=$(LOG) build
	cp os/target/loongarch64-unknown-none/release/os kernel-la
	$(MAKE) -C os ARCH=loongarch64 BOARD=2k1000 AUTO_TEST=0 SDCARD_IMG=$(LA_SDCARD_IMG) patch-sdcard
	@rootfs_size=$$(stat -c%s "$(LA_SDCARD_IMG)"); \
	if [ "$$rootfs_size" -gt "$(BOARD_INITRD_MAX)" ]; then \
		echo "Error: $(LA_SDCARD_IMG) is $$rootfs_size bytes, larger than BOARD_INITRD_MAX=$(BOARD_INITRD_MAX)." >&2; \
		echo "2k1000 USB-menu initrd boot needs a small uncompressed ext4 image; use block-device rootfs or a smaller image." >&2; \
		exit 1; \
	fi
	python3 tools/wrap-uimage.py --ref "$(abspath $(UIMAGE_REF))" --kernel "$(abspath os/target/loongarch64-unknown-none/release/os.bin)" --out "$(abspath $(UIMAGE_OUT))"
	@echo "2k1000 artifacts:"
	@echo "  kernel: $(abspath $(UIMAGE_OUT)) -> copy to USB /install/uImage"
	@echo "  rootfs: $(LA_SDCARD_IMG) -> copy to USB /install/ramdisk.gz without gzip compression"

lkernel_board_small:
	@echo "Creating small 2k1000 ext4 initrd rootfs: $(BOARD_ROOTFS_IMAGE) ($(BOARD_ROOTFS_SIZE))"
	@rm -f "$(BOARD_ROOTFS_IMAGE)"
	truncate -s "$(BOARD_ROOTFS_SIZE)" "$(BOARD_ROOTFS_IMAGE)"
	mkfs.ext4 -F -L kairix-root "$(BOARD_ROOTFS_IMAGE)"
	$(MAKE) lkernel_board FILE_LA="$(BOARD_ROOTFS_IMAGE)" LOG=$(LOG) UIMAGE_REF="$(UIMAGE_REF)" UIMAGE_OUT="$(UIMAGE_OUT)"

lkernel_board_gcc:
	bash tools/gcc-toolchain/build-loongarch64.sh
	$(MAKE) lkernel_board_small LOG=$(LOG) BOARD_ROOTFS_IMG="$(BOARD_ROOTFS_IMG)" BOARD_ROOTFS_SIZE="$(BOARD_ROOTFS_SIZE)" UIMAGE_REF="$(UIMAGE_REF)" UIMAGE_OUT="$(UIMAGE_OUT)"
	bash tools/install-board-gcc.sh "$(BOARD_GCC_ROOTFS)" "$(BOARD_ROOTFS_IMAGE)"
	@echo "Native GCC is available on the board at /usr/bin/gcc"

# Build the board kernel while keeping the full ext4 image on a SATA disk.
# The USB ramdisk remains a small recovery root and is not generated here.
lkernel_board_sata:
	@test -f "$(LA_SDCARD_IMG)" || (echo "Error: SATA rootfs image not found: $(LA_SDCARD_IMG)" >&2; exit 1)
	$(MAKE) -C os ARCH=loongarch64 BOARD=2k1000 LOG=$(LOG) build
	cp os/target/loongarch64-unknown-none/release/os kernel-la
	$(MAKE) -C os ARCH=loongarch64 BOARD=2k1000 AUTO_TEST=$(AUTO_TEST) SDCARD_IMG=$(LA_SDCARD_IMG) patch-sdcard
	python3 tools/wrap-uimage.py --ref "$(abspath $(UIMAGE_REF))" --kernel "$(abspath os/target/loongarch64-unknown-none/release/os.bin)" --out "$(abspath $(UIMAGE_OUT))"
	@echo "2k1000 SATA artifacts:"
	@echo "  kernel: $(abspath $(UIMAGE_OUT)) -> USB /install/uImage"
	@echo "  SATA rootfs image: $(LA_SDCARD_IMG) -> write to the whole SATA disk"
	@echo "  Keep the existing small ext4 image as USB /install/ramdisk.gz for fallback"

# Build mkfs.ext tools that are injected into test images.
mkfs-tools:
	@echo "Building mkfs.ext2/ext3/ext4 tools..."
	@bash ./tools/build-mkfs.sh all

# Build both architectures and copy official kernel ELF files to workspace root.
all: mkfs-tools
	@echo "Using vendored Rust dependencies from os/vendor and user/vendor..."
	@echo "Building RISC-V kernel..."
	$(MAKE) -C os ARCH=riscv64 BOARD=$(BOARD) LOG=OFF build
	cp os/target/riscv64gc-unknown-none-elf/release/os kernel-rv
	@if [ -f "$(RV_SDCARD_IMG)" ]; then \
		echo "Preparing RISC-V sdcard image..."; \
		$(MAKE) -C os ARCH=riscv64 BOARD=$(BOARD) AUTO_TEST=$(AUTO_TEST) SDCARD_IMG=$(RV_SDCARD_IMG) patch-sdcard; \
	else \
		echo "$(RV_SDCARD_IMG) not found; skipping RISC-V sdcard patch"; \
	fi
	@echo "Building LoongArch kernel..."
	$(MAKE) -C os ARCH=loongarch64 BOARD=$(BOARD) LOG=OFF build
	cp os/target/loongarch64-unknown-none/release/os kernel-la
	@if [ -f "$(LA_SDCARD_IMG)" ]; then \
		echo "Preparing LoongArch sdcard image..."; \
		$(MAKE) -C os ARCH=loongarch64 BOARD=$(BOARD) AUTO_TEST=$(AUTO_TEST) SDCARD_IMG=$(LA_SDCARD_IMG) patch-sdcard; \
	else \
		echo "$(LA_SDCARD_IMG) not found; skipping LoongArch sdcard patch"; \
	fi
	@echo "Done. Official kernel ELF files copied to workspace root:"
	@echo "  kernel-rv"
	@echo "  kernel-la"

clean-mkfs:
	@bash ./tools/build-mkfs.sh clean

clean:
	$(MAKE) -C os ARCH=riscv64 clean
	$(MAKE) -C os ARCH=loongarch64 clean
	rm -f kernel-rv kernel-la os-riscv64 os-loongarch64 os-riscv64.bin os-loongarch64.bin
