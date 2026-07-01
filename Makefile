# Top-level Makefile for Kairix OS
# Delegates to os/Makefile for actual builds

.PHONY: all rkernel rkernel_test lkernel lkernel_test help mkfs-tools clean-mkfs clean

LOG ?= INFO
CPU ?= 1
RKERNEL_QEMU := qemu-system-riscv64 -machine virt -kernel kernel-rv -m 1G -nographic -smp $(CPU) -bios default -drive file=sdcard-rv.img,if=none,format=raw,id=x0 -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 -no-reboot -device virtio-net-device,netdev=net -netdev user,id=net -rtc base=utc
LKERNEL_QEMU := qemu-system-loongarch64 -kernel kernel-la -m 1G -nographic -smp $(CPU) -drive file=sdcard-la.img,if=none,format=raw,id=x0 -device virtio-blk-pci,drive=x0 -no-reboot -device virtio-net-pci,netdev=net0 -netdev user,id=net0 -rtc base=utc

help:
	@echo "Available targets:"
	@echo "  make rkernel [LOG=INFO] - Build/run RISC-V with auto tests disabled"
	@echo "  make rkernel_test - Build/run RISC-V competition mode with LOG=OFF and auto tests enabled"
	@echo "  make lkernel [LOG=INFO] - Build/run LoongArch with auto tests disabled"
	@echo "  make lkernel_test - Build/run LoongArch competition mode with LOG=OFF and auto tests enabled"
	@echo "  make all      - Build both kernels and patch sdcard images when present"
	@echo "  make mkfs-tools - Build mkfs.ext2/ext3/ext4 tools for both architectures"

# Local RISC-V run: keep kernel logs visible and start the interactive shell.
rkernel:
	$(MAKE) -C os ARCH=riscv64 LOG=$(LOG) build
	cp os/target/riscv64gc-unknown-none-elf/release/os kernel-rv
	$(MAKE) -C os ARCH=riscv64 AUTO_TEST=0 patch-sdcard
	$(RKERNEL_QEMU)

# Competition-style RISC-V run: auto tests enabled and kernel logs compiled out.
rkernel_test:
	$(MAKE) -C os ARCH=riscv64 LOG=$(LOG) build
	cp os/target/riscv64gc-unknown-none-elf/release/os kernel-rv
	$(MAKE) -C os ARCH=riscv64 AUTO_TEST=1 patch-sdcard
	$(RKERNEL_QEMU)

# Local LoongArch run: keep kernel logs visible and start the interactive shell.
lkernel:
	$(MAKE) -C os ARCH=loongarch64 LOG=$(LOG) build
	cp os/target/loongarch64-unknown-none/release/os kernel-la
	$(MAKE) -C os ARCH=loongarch64 AUTO_TEST=0 patch-sdcard
	$(LKERNEL_QEMU)

# Competition-style LoongArch run: auto tests enabled and kernel logs compiled out.
lkernel_test:
	$(MAKE) -C os ARCH=loongarch64 LOG=$(LOG) build
	cp os/target/loongarch64-unknown-none/release/os kernel-la
	$(MAKE) -C os ARCH=loongarch64 AUTO_TEST=1 patch-sdcard
	$(LKERNEL_QEMU)

# Build mkfs.ext tools that are injected into test images.
mkfs-tools:
	@echo "Building mkfs.ext2/ext3/ext4 tools..."
	@bash ./tools/build-mkfs.sh all

# Build both architectures and copy official kernel ELF files to workspace root.
all: mkfs-tools
	@echo "Using vendored Rust dependencies from os/vendor and user/vendor..."
	@echo "Building RISC-V kernel..."
	$(MAKE) -C os ARCH=riscv64 LOG=OFF build
	cp os/target/riscv64gc-unknown-none-elf/release/os kernel-rv
	@if [ -f sdcard-rv.img ]; then \
		echo "Preparing RISC-V sdcard image..."; \
		$(MAKE) -C os ARCH=riscv64 AUTO_TEST=1 patch-sdcard; \
	else \
		echo "sdcard-rv.img not found; skipping RISC-V sdcard patch"; \
	fi
	@echo "Building LoongArch kernel..."
	$(MAKE) -C os ARCH=loongarch64 LOG=OFF build
	cp os/target/loongarch64-unknown-none/release/os kernel-la
	@if [ -f sdcard-la.img ]; then \
		echo "Preparing LoongArch sdcard image..."; \
		$(MAKE) -C os ARCH=loongarch64 AUTO_TEST=1 patch-sdcard; \
	else \
		echo "sdcard-la.img not found; skipping LoongArch sdcard patch"; \
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
