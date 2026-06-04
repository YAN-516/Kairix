# Top-level Makefile for Kairix OS
# Delegates to os/Makefile for actual builds

.PHONY: all rkernel lkernel help mkfs-tools clean-mkfs clean

help:
	@echo "Available targets:"
	@echo "  make rkernel  - Build and run RISC-V kernel with sdcard-rv.img"
	@echo "  make lkernel  - Build and run LoongArch kernel with sdcard-la.img"
	@echo "  make all      - Build both RISC-V and LoongArch kernels and copy to main directory"
	@echo "  make mkfs-tools - Build mkfs.ext2/ext3/ext4 tools for both architectures"

# Build and run RISC-V kernel with competition disk image
rkernel:
	$(MAKE) -C os ARCH=riscv64 run-sdcard

# Build and run LoongArch kernel with competition disk image
lkernel:
	$(MAKE) -C os ARCH=loongarch64 run-sdcard

# Build mkfs.ext tools that are injected into test images.
mkfs-tools:
	@echo "Building mkfs.ext2/ext3/ext4 tools..."
	@bash ./tools/build-mkfs.sh all

# Build both architectures and copy kernel binaries to workspace root for evaluation
all: mkfs-tools
	@echo "Vendoring Rust dependencies for offline build..."
	@cd os && rm -rf vendor && cargo vendor
	@cd user && rm -rf vendor && cargo vendor
	@echo "Building RISC-V kernel..."
	$(MAKE) -C os ARCH=riscv64 build
	cp os/target/riscv64gc-unknown-none-elf/release/os.bin os-riscv64.bin
	cp os/target/riscv64gc-unknown-none-elf/release/os os-riscv64
	@echo "Building LoongArch kernel..."
	$(MAKE) -C os ARCH=loongarch64 build
	cp os/target/loongarch64-unknown-none/release/os.bin os-loongarch64.bin
	cp os/target/loongarch64-unknown-none/release/os os-loongarch64
	@echo "Done. Kernels copied to workspace root:"
	@echo "  os-riscv64.bin, os-riscv64"
	@echo "  os-loongarch64.bin, os-loongarch64"

clean-mkfs:
	@bash ./tools/build-mkfs.sh clean

clean:
	$(MAKE) -C os ARCH=riscv64 clean
	$(MAKE) -C os ARCH=loongarch64 clean
