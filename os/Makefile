ARCH ?= riscv64
BOARD := qemu

ifeq ($(ARCH), riscv64)
    TARGET := riscv64gc-unknown-none-elf
    QEMU_NAME := qemu-system-riscv64
    SBI ?= rustsbi
    KERNEL_ENTRY_PA := 0x80200000
    LINKER_SCRIPT := src/linker-riscv64.ld
    SDCARD_IMG ?= ../sdcard-rv.img
else ifeq ($(ARCH), loongarch64)
    TARGET := loongarch64-unknown-none
    QEMU_NAME := qemu-system-loongarch64
    SBI ?= opensbi
    KERNEL_ENTRY_PA := 0x80000000
    LINKER_SCRIPT := src/linker-loongarch64.ld
    SDCARD_IMG ?= ../sdcard-la.img
else
    $(error Unsupported ARCH: $(ARCH))
endif

BOOTLOADER := ../bootloader/$(SBI)-$(BOARD).bin

MODE := release
KERNEL_ELF := target/$(TARGET)/$(MODE)/os
KERNEL_BIN := $(KERNEL_ELF).bin
DISASM_TMP := target/$(TARGET)/$(MODE)/asm
FS_IMG := ../user/target/$(TARGET)/$(MODE)/fs.img
SELECTED_IMG ?= $(FS_IMG)
USER_TARGET_DIR := ../user/target/$(TARGET)/$(MODE)
MKFS_TOOLS_DIR := ../tools/target/mkfs-$(ARCH)/sbin

LOG ?= OFF
CPU := 1
NET_DUMP ?= 1
NET_DUMP_FILE ?= ./qemu-net.pcap
NETDEV_ID ?= n0
NET_MAC ?=
BRIDGE_IF ?= br0
BRIDGE_HOST_IP ?= 10.0.2.2/24
NET_BACKEND ?= user
NET_USER_ARGS ?=
NET_BACKEND ?= user
NET_USER_ARGS ?=
export LOG
CARGO_OFFLINE ?= 1
CARGO_LOCK_ARGS := --locked
ifneq ($(wildcard .cargo/config.toml),)
	CARGO_CONFIG :=
else
	CARGO_CONFIG := --config ../tools/cargo-config-os.toml
endif
ifeq ($(CARGO_OFFLINE),1)
	CARGO_LOCK_ARGS += --offline
	export CARGO_NET_OFFLINE := true
endif

ifeq ($(MODE), release)
	MODE_ARG := --release
endif

ifeq ($(ARCH), riscv64)
    OBJDUMP := rust-objdump --arch-name=riscv64
    OBJCOPY := rust-objcopy --binary-architecture=riscv64
else ifeq ($(ARCH), loongarch64)
    OBJDUMP := rust-objdump --arch-name=loongarch64
    OBJCOPY := rust-objcopy --binary-architecture=loongarch64
endif

DISASM ?= -x

build: env prepare-embedded $(KERNEL_BIN)

env:
	@(rustup target list --installed | grep -qx "$(TARGET)") || (echo "Error: missing Rust target '$(TARGET)'. Install it before offline build." >&2; exit 1)
	@command -v rust-objcopy > /dev/null || (echo "Error: missing rust-objcopy. Install cargo-binutils before offline build." >&2; exit 1)
	@(rustup component list --installed | grep -q "^rust-src") || (echo "Error: missing rust-src component. Install it before offline build." >&2; exit 1)
	@(rustup component list --installed | grep -q "^llvm-tools") || (echo "Error: missing llvm-tools-preview component. Install it before offline build." >&2; exit 1)

	
$(KERNEL_BIN): kernel
	@$(OBJCOPY) $(KERNEL_ELF) --strip-all -O binary $@

prepare-embedded: env mkfs-tools
	@echo "Preparing embedded runtime files for $(ARCH)..."
	@cd ../user && make elf MODE=$(MODE) TARGET=$(TARGET)

kernel: prepare-embedded
	@echo Platform: $(BOARD)
	@echo Architecture: $(ARCH)
	@cargo build $(CARGO_CONFIG) $(MODE_ARG) $(CARGO_LOCK_ARGS) --target $(TARGET)


clean: clean-mkfs
	@cargo clean $(CARGO_CONFIG)
	@cd ../user && make clean

clean-mkfs:
	@bash ../tools/build-mkfs.sh clean

disasm: kernel
	@$(OBJDUMP) $(DISASM) $(KERNEL_ELF) | less

disasm-vim: kernel
	@$(OBJDUMP) $(DISASM) $(KERNEL_ELF) > $(DISASM_TMP)
	@vim $(DISASM_TMP)
	@rm $(DISASM_TMP)

stop-bridge:
	@$(MAKE) net-down-bridge BRIDGE_IF=$(BRIDGE_IF)

net-up-bridge:
	@echo "Preparing bridge $(BRIDGE_IF)"
	@ip link add name $(BRIDGE_IF) type bridge 2>/dev/null || true
	@ip link set $(BRIDGE_IF) up
	@ip addr replace $(BRIDGE_HOST_IP) dev $(BRIDGE_IF)

net-down-bridge:
	@echo "Tearing down bridge $(BRIDGE_IF)"
	@ip addr flush dev $(BRIDGE_IF) 2>/dev/null || true
	@ip link del $(BRIDGE_IF) 2>/dev/null || true

QEMU_NET_DUMP_ARGS :=
ifeq ($(NET_DUMP),1)
QEMU_NET_DUMP_ARGS := -object filter-dump,id=netdump,netdev=$(NETDEV_ID),file=$(NET_DUMP_FILE)
endif

NET_BACKEND_RESOLVED := $(NET_BACKEND)
ifeq ($(NET_BACKEND),auto)
NET_BACKEND_RESOLVED := $(shell if ip link show $(BRIDGE_IF) >/dev/null 2>&1; then echo bridge; else echo user; fi)
endif

QEMU_NETDEV_ARGS :=
ifeq ($(NET_BACKEND_RESOLVED),bridge)
QEMU_NETDEV_ARGS := -netdev bridge,id=$(NETDEV_ID),br=$(BRIDGE_IF)
else ifeq ($(NET_BACKEND_RESOLVED),user)
QEMU_NETDEV_ARGS := -netdev user,id=$(NETDEV_ID)$(NET_USER_ARGS)
else
$(error Unsupported NET_BACKEND='$(NET_BACKEND_RESOLVED)', expected bridge/user/auto)
endif

NET_BACKEND_RESOLVED := $(NET_BACKEND)
ifeq ($(NET_BACKEND),auto)
NET_BACKEND_RESOLVED := $(shell if ip link show $(BRIDGE_IF) >/dev/null 2>&1; then echo bridge; else echo user; fi)
endif

QEMU_NETDEV_ARGS :=
ifeq ($(NET_BACKEND_RESOLVED),bridge)
QEMU_NETDEV_ARGS := -netdev bridge,id=$(NETDEV_ID),br=$(BRIDGE_IF)
else ifeq ($(NET_BACKEND_RESOLVED),user)
QEMU_NETDEV_ARGS := -netdev user,id=$(NETDEV_ID)$(NET_USER_ARGS)
else
$(error Unsupported NET_BACKEND='$(NET_BACKEND_RESOLVED)', expected bridge/user/auto)
endif

QEMU_NET_DEVICE_ARGS := -device virtio-net-pci,netdev=$(NETDEV_ID)
ifneq ($(strip $(NET_MAC)),)
QEMU_NET_DEVICE_ARGS += ,mac=$(NET_MAC)
endif

ifeq ($(ARCH), riscv64)
	QEMU_ARGS := -machine virt \
			 -nographic \
			 -bios $(BOOTLOADER) \
			 -device loader,file=$(KERNEL_BIN),addr=$(KERNEL_ENTRY_PA) \
			 -drive file=$(SELECTED_IMG),if=none,format=raw,id=x0 \
			 -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
			 -m 4G \
			 $(QEMU_NETDEV_ARGS) \
			 -m 4G \
			 $(QEMU_NETDEV_ARGS) \
			 $(QEMU_NET_DUMP_ARGS) \
			 $(QEMU_NET_DEVICE_ARGS) \
			 -smp $(CPU) 
else ifeq ($(ARCH), loongarch64)
    QEMU_ARGS := -machine virt \
                 -nographic \
				 -rtc base=utc \
				 -no-reboot \
				 -m 4G \
				 -kernel $(KERNEL_ELF) \
                 -drive file=$(SELECTED_IMG),if=none,format=raw,id=hd0 \
    			 -device virtio-blk-pci,drive=hd0 \
				 $(QEMU_NETDEV_ARGS) \
				 $(QEMU_NET_DUMP_ARGS) \
				 $(QEMU_NET_DEVICE_ARGS) \
				 -smp $(CPU)
endif
run-inner: qemu-version-check build
	@echo "ARCH=$(ARCH)"
	@echo "TARGET=$(TARGET)"
	@echo "KERNEL_BIN=$(KERNEL_BIN)"
	@echo "KERNEL_ENTRY_PA=$(KERNEL_ENTRY_PA)"
	@echo "QEMU_NAME=$(QEMU_NAME)"
	@echo "NET_BACKEND=$(NET_BACKEND_RESOLVED)"
	@echo "Running QEMU with command: $(QEMU_NAME) $(QEMU_ARGS)"
	@$(QEMU_NAME) $(QEMU_ARGS)
	@echo "ARCH=$(ARCH)"
	@echo "TARGET=$(TARGET)"
	@echo "KERNEL_BIN=$(KERNEL_BIN)"
	@echo "KERNEL_ENTRY_PA=$(KERNEL_ENTRY_PA)"
	@echo "QEMU_NAME=$(QEMU_NAME)"
	@echo "NET_BACKEND=$(NET_BACKEND_RESOLVED)"
	@echo "Running QEMU with command: $(QEMU_NAME) $(QEMU_ARGS)"
	@$(QEMU_NAME) $(QEMU_ARGS)

debug: qemu-version-check build
	@tmux new-session -d \
		"qemu-system-riscv64 $(QEMU_ARGS) -s -S" && \
		tmux split-window -h "riscv64-unknown-elf-gdb -ex 'file $(KERNEL_ELF)' -ex 'set arch riscv:rv64' -ex 'target remote localhost:1234'" && \
		tmux -2 attach-session -d

gdbserver: qemu-version-check build
	@qemu-system-riscv64 $(QEMU_ARGS) -s -S

gdbclient:
	@riscv64-unknown-elf-gdb -ex 'file $(KERNEL_ELF)' -ex 'set arch riscv:rv64' -ex 'target remote localhost:1234'

run-sdcard: qemu-version-check env $(KERNEL_BIN) patch-sdcard
	@echo "Running with patched external image: $(SDCARD_IMG) on bridge $(BRIDGE_IF)"
	@$(MAKE) net-up-bridge BRIDGE_IF=$(BRIDGE_IF)
	@rm -f $(NET_DUMP_FILE)
	@$(MAKE) run-inner SELECTED_IMG=$(SDCARD_IMG) BRIDGE_IF=$(BRIDGE_IF)

sdcard: run-sdcard

patch-sdcard:
	@$(MAKE) do-patch-sdcard IMG=$(SDCARD_IMG)

mkfs-tools:
	@bash ../tools/build-mkfs.sh $(ARCH)

do-patch-sdcard: mkfs-tools
	@echo "Step 1: Building user apps..."
	@cd ../user && make build MODE=$(MODE) TARGET=$(TARGET)
	@cd ../user && make build MODE=$(MODE) TARGET=$(TARGET)
	
	@echo "Step 2: Injecting apps into $(IMG)..."
	@e2fsck -f -y $(IMG) || true
	@mkdir -p ./temp_mnt
	@mount $(IMG) ./temp_mnt
	
	@for app in initproc user_shell ls basictests libctests_static libctests_dynamic; do \
		echo "  Copying $$app..."; \
		cp $(USER_TARGET_DIR)/$$app ./temp_mnt/$$app; \
		chmod +x ./temp_mnt/$$app; \
	done
	
	@echo "Step 3: Skipping /bin symlinks (now handled by initproc at runtime)"

	@echo "Step 3b: Installing real mkfs.ext2/ext3/ext4 tools..."
	@bash ../tools/install-mkfs-ext.sh $(ARCH) ./temp_mnt $(MKFS_TOOLS_DIR)

	@bash ../tools/setup-sdcard-libs.sh $(ARCH) ./temp_mnt

	@sync && umount ./temp_mnt && rmdir ./temp_mnt
	@echo "Injection finished."
	


qemu-version-check:
	@echo "Using QEMU: $(QEMU_NAME)"


.PHONY: build env prepare-embedded kernel clean clean-mkfs disasm disasm-vim run-inner gdbserver gdbclient qemu-version-check run-sdcard patch-sdcard mkfs-tools do-patch-sdcard
