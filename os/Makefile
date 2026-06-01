ARCH ?=  riscv64
# KERNEL ENTRY
KERNEL_ENTRY_PA ?= 0x80200000


ifeq ($(ARCH), riscv64)
    TARGET := riscv64gc-unknown-none-elf
    QEMU_NAME := qemu-system-riscv64
    BOARD := qemu
    SBI ?= rustsbi
    BOOTLOADER := ../bootloader/$(SBI)-$(BOARD).bin
    KERNEL_ENTRY_PA := 0x80200000
    LINKER_SCRIPT := src/linker-riscv64.ld
    # 用户程序目标
    USER_TARGET := riscv64gc-unknown-none-elf
else ifeq ($(ARCH), loongarch64)
    TARGET := loongarch64-unknown-none
    QEMU_NAME := qemu-system-loongarch64
    BOARD := qemu
    # 龙芯使用 OpenSBI 或其它 bootloader
    SBI ?= opensbi
    BOOTLOADER := ../bootloader/$(SBI)-$(BOARD).bin
    KERNEL_ENTRY_PA := 0x80000000  # 龙芯虚拟地址
    LINKER_SCRIPT := src/linker-loongarch64.ld
    USER_TARGET := loongarch64-unknown-none
else
    $(error Unsupported ARCH: $(ARCH))
endif

# Building
# TARGET := riscv64gc-unknown-none-elf
MODE := release
KERNEL_ELF := target/$(TARGET)/$(MODE)/os
KERNEL_BIN := $(KERNEL_ELF).bin
DISASM_TMP := target/$(TARGET)/$(MODE)/asm
FS_IMG := ../user/target/$(TARGET)/$(MODE)/fs.img
SDCARD_IMG ?= ../sdcard-rv.img
SDCARD_IMG_noltp := ../sdcard-rv-noltp.img
USER_INITPROC := ../user/target/$(TARGET)/$(MODE)/initproc
# 增加一个变量来决定最终给 QEMU 用的镜像路径，默认是原来的 FS_IMG
SELECTED_IMG ?= $(FS_IMG)
APPS := ../user/src/bin/*
LOG ?= INFO
CPU := 1
NET_DUMP ?= 1
NET_DUMP_FILE ?= ./qemu-net.pcap
NETDEV_ID ?= n0
NET_MAC ?=
BRIDGE_IF ?= br0
BRIDGE_HOST_IP ?= 10.0.2.2/24
NET_BACKEND ?= user
NET_USER_ARGS ?=
export LOG
# BOARD
# BOARD := qemu
# SBI ?= rustsbi
# BOOTLOADER := ../bootloader/$(SBI)-$(BOARD).bin

# Building mode argument
ifeq ($(MODE), release)
	MODE_ARG := --release
endif

ifeq ($(ARCH), riscv64)
	SDCARD_IMG := ../sdcard-rv.img
else ifeq ($(ARCH), loongarch64)
	SDCARD_IMG := ../sdcard-la.img
endif



# Binutils
# OBJDUMP := rust-objdump --arch-name=riscv64
# OBJCOPY := rust-objcopy --binary-architecture=riscv64

ifeq ($(ARCH), riscv64)
    OBJDUMP := rust-objdump --arch-name=riscv64
    OBJCOPY := rust-objcopy --binary-architecture=riscv64
else ifeq ($(ARCH), loongarch64)
    OBJDUMP := rust-objdump --arch-name=loongarch64
    OBJCOPY := rust-objcopy --binary-architecture=loongarch64
endif
# Disassembly
DISASM ?= -x

# 镜像配置
FS_SIZE := 64  # 镜像大小 (单位: MB)
USER_TARGET_DIR := ../user/target/$(TARGET)/$(MODE)



# Run usertests or usershell
TEST ?=

build: env $(KERNEL_BIN)

env:
	@(rustup target list | grep "riscv64gc-unknown-none-elf (installed)") || rustup target add $(TARGET)
	@# 检查是否安装了 cargo-binutils，如果没安装才去 install
	@which rust-objcopy > /dev/null || cargo install cargo-binutils
	@rustup component add rust-src
	@rustup component add llvm-tools-preview

	
$(KERNEL_BIN): kernel
	@$(OBJCOPY) $(KERNEL_ELF) --strip-all -O binary $@

kernel:
	@echo Platform: $(BOARD)
	# @cp src/linker-$(BOARD).ld src/linker.ld
		@echo Architecture: $(ARCH)
	@if [ "$(ARCH)" = "loongarch64" ]; then \
		cp src/linker-loongarch64.ld src/linker.ld; \
	elif [ "$(ARCH)" = "riscv64" ]; then \
		cp src/linker-riscv64.ld src/linker.ld; \
	else \
		cp src/linker-$(BOARD).ld src/linker.ld; \
	fi
	@cargo build $(MODE_ARG) --target $(TARGET)
	@rm -f src/linker.ld


clean:
	@cargo clean
	@cd ../user && make clean

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

QEMU_NET_DEVICE_ARGS := -device virtio-net-pci,netdev=$(NETDEV_ID)
ifneq ($(strip $(NET_MAC)),)
QEMU_NET_DEVICE_ARGS += ,mac=$(NET_MAC)
endif

QEMU_ARGS ?= -machine virt \
			 -nographic \
			 -bios $(BOOTLOADER) \
			 -device loader,file=$(KERNEL_BIN),addr=$(KERNEL_ENTRY_PA) \
			 -drive file=$(SELECTED_IMG),if=none,format=raw,id=x0 \
			 -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
			 -m 4G \
			 $(QEMU_NETDEV_ARGS) \
			 $(QEMU_NET_DUMP_ARGS) \
			 $(QEMU_NET_DEVICE_ARGS) \
			 -smp $(CPU) 

ifeq ($(ARCH), riscv64)
	QEMU_ARGS := -machine virt \
			 -nographic \
			 -bios $(BOOTLOADER) \
			 -device loader,file=$(KERNEL_BIN),addr=$(KERNEL_ENTRY_PA) \
			 -drive file=$(SELECTED_IMG),if=none,format=raw,id=x0 \
			 -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
			 -m 4G \
			 $(QEMU_NETDEV_ARGS) \
			 $(QEMU_NET_DUMP_ARGS) \
			 $(QEMU_NET_DEVICE_ARGS) \
			 -smp $(CPU) 
else ifeq ($(ARCH), loongarch64)
    # 龙芯 QEMU 参数
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
                #  -smp $(CPU) \
				#  -d in_asm,exec,int -D qemu.log

				#      QEMU_ARGS := -kernel $(KERNEL_ELF) -m 1G -nographic -smp $(CPU) -drive file=$(SELECTED_IMG),if=none,format=raw,id=x0  \
                #  -device virtio-blk-pci,drive=x0,bus=virtio-mmio-bus.0 -no-reboot  -device virtio-net-pci,netdev=net0 \
                #  -netdev user,id=net0,hostfwd=tcp::5555-:5555,hostfwd=udp::5555-:5555  \
                #  -rtc base=utc \

    # 如果龙芯需要额外的 BIOS，可以添加：
    # -bios $(BOOTLOADER)
endif



# QEMU_NAME := qemu-system-riscv64
# qemu-version-check:
# 	@sh scripts/qemu-ver-check.sh $(QEMU_NAME)

# run-inner: qemu-version-check build
# 	@qemu-system-riscv64 $(QEMU_ARGS)
run-inner: qemu-version-check build
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

run-sdcard-rv-noltp: qemu-version-check env $(KERNEL_BIN) patch-sdcard-noltp
	@echo "Running with patched external image (RV-NOLTP): $(SDCARD_IMG_noltp)"
	$(MAKE) run-inner SELECTED_IMG=$(SDCARD_IMG_noltp)

patch-sdcard:
	@$(MAKE) do-patch-sdcard IMG=$(SDCARD_IMG)

patch-sdcard-noltp:
	@$(MAKE) do-patch-sdcard IMG=$(SDCARD_IMG_noltp)

do-patch-sdcard:
	@echo "Step 1: Building user apps..."
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

	@echo "Step 3b: Installing LTP mkfs.ext2/ext3/ext4 stubs..."
	@mkdir -p ./temp_mnt/bin ./temp_mnt/sbin ./temp_mnt/musl/ltp/testcases/bin
	@rm -f ./temp_mnt/bin/mkfs.ext2 ./temp_mnt/bin/mkfs.ext3 ./temp_mnt/bin/mkfs.ext4 ./temp_mnt/sbin/mkfs.ext2 ./temp_mnt/sbin/mkfs.ext3 ./temp_mnt/sbin/mkfs.ext4 ./temp_mnt/musl/ltp/testcases/bin/mkfs.ext2 ./temp_mnt/musl/ltp/testcases/bin/mkfs.ext3 ./temp_mnt/musl/ltp/testcases/bin/mkfs.ext4
	@cp $(USER_TARGET_DIR)/mkfs_stub ./temp_mnt/bin/mkfs.ext2
	@cp $(USER_TARGET_DIR)/mkfs_stub ./temp_mnt/bin/mkfs.ext3
	@cp $(USER_TARGET_DIR)/mkfs_stub ./temp_mnt/bin/mkfs.ext4
	@cp $(USER_TARGET_DIR)/mkfs_stub ./temp_mnt/sbin/mkfs.ext2
	@cp $(USER_TARGET_DIR)/mkfs_stub ./temp_mnt/sbin/mkfs.ext3
	@cp $(USER_TARGET_DIR)/mkfs_stub ./temp_mnt/sbin/mkfs.ext4
	@cp $(USER_TARGET_DIR)/mkfs_stub ./temp_mnt/musl/ltp/testcases/bin/mkfs.ext2
	@cp $(USER_TARGET_DIR)/mkfs_stub ./temp_mnt/musl/ltp/testcases/bin/mkfs.ext3
	@cp $(USER_TARGET_DIR)/mkfs_stub ./temp_mnt/musl/ltp/testcases/bin/mkfs.ext4
	@chmod +x ./temp_mnt/bin/mkfs.ext2 ./temp_mnt/bin/mkfs.ext3 ./temp_mnt/bin/mkfs.ext4 ./temp_mnt/sbin/mkfs.ext2 ./temp_mnt/sbin/mkfs.ext3 ./temp_mnt/sbin/mkfs.ext4 ./temp_mnt/musl/ltp/testcases/bin/mkfs.ext2 ./temp_mnt/musl/ltp/testcases/bin/mkfs.ext3 ./temp_mnt/musl/ltp/testcases/bin/mkfs.ext4

# 	@echo "Step 3b: Injecting mkfs tools..."
# 	@mkdir -p ./temp_mnt/sbin
# ifeq ($(ARCH),riscv64)
# 	@for tool in  mkfs.ext3 mkfs.ext4 ; do \
# 		src="/workspace/tools/mkfs-riscv64/sbin/$$tool"; \
# 		if [ -f "$$src" ]; then \
# 			cp "$$src" ./temp_mnt/sbin/$$tool; \
# 			chmod +x ./temp_mnt/sbin/$$tool; \
# 			echo "  Copied $$tool to /sbin/"; \
# 		else \
# 			echo "  Warning: $$src not found"; \
# 		fi; \
# 	done
# 	@ln -sf mkfs.fat ./temp_mnt/sbin/mkfs.vfat 2>/dev/null || true
# else ifeq ($(ARCH),loongarch64)
# 	@for tool in  mkfs.ext3 mkfs.ext4 ; do \
# 		src="/workspace/tools/mkfs-loongarch64/sbin/$$tool"; \
# 		if [ -f "$$src" ]; then \
# 			cp "$$src" ./temp_mnt/sbin/$$tool; \
# 			chmod +x ./temp_mnt/sbin/$$tool; \
# 			echo "  Copied $$tool to /sbin/"; \
# 		else \
# 			echo "  Warning: $$src not found"; \
# 		fi; \
# 	done
# 	@ln -sf mkfs.fat ./temp_mnt/sbin/mkfs.vfat 2>/dev/null || true
# endif

ifeq ($(ARCH), riscv64)
	@echo "Step 4: Setting up /lib for RISC-V dynamic linking..."
	@mkdir -p ./temp_mnt/lib
	@mkdir -p ./temp_mnt/lib/riscv64-linux-gnu
	@if [ -f ./temp_mnt/glibc/lib/ld-linux-riscv64-lp64d.so.1 ]; then \
		cp ./temp_mnt/glibc/lib/ld-linux-riscv64-lp64d.so.1 ./temp_mnt/lib/ld-linux-riscv64-lp64d.so.1; \
		echo "  Copied ld-linux-riscv64-lp64d.so.1 to /lib/"; \
	fi
	@if [ -f ./temp_mnt/glibc/lib/libc.so.6 ]; then \
		cp ./temp_mnt/glibc/lib/libc.so.6 ./temp_mnt/lib/libc.so.6; \
		cp ./temp_mnt/glibc/lib/libc.so.6 ./temp_mnt/lib/riscv64-linux-gnu/libc.so.6; \
		echo "  Copied libc.so.6 to /lib/ and /lib/riscv64-linux-gnu/"; \
	fi
	@if [ -f ./temp_mnt/glibc/lib/libm.so.6 ]; then \
		cp ./temp_mnt/glibc/lib/libm.so.6 ./temp_mnt/lib/libm.so.6; \
		cp ./temp_mnt/glibc/lib/libm.so.6 ./temp_mnt/lib/riscv64-linux-gnu/libm.so.6; \
		echo "  Copied libm.so.6 to /lib/ and /lib/riscv64-linux-gnu/"; \
	fi
	@if [ -f ./temp_mnt/glibc/lib/libc.so ]; then \
		cp ./temp_mnt/glibc/lib/libc.so ./temp_mnt/lib/libc.so; \
		cp ./temp_mnt/glibc/lib/libc.so ./temp_mnt/lib/riscv64-linux-gnu/libc.so; \
		echo "  Copied libc.so to /lib/ and /lib/riscv64-linux-gnu/"; \
	fi
	@if [ -f ./temp_mnt/glibc/lib/libm.so ]; then \
		cp ./temp_mnt/glibc/lib/libm.so ./temp_mnt/lib/libm.so; \
		cp ./temp_mnt/glibc/lib/libm.so ./temp_mnt/lib/riscv64-linux-gnu/libm.so; \
		echo "  Copied libm.so to /lib/ and /lib/riscv64-linux-gnu/"; \
	fi
	@if [ -f ./temp_mnt/musl/lib/ld-musl-riscv64-sf.so.1 ]; then \
		cp ./temp_mnt/musl/lib/ld-musl-riscv64-sf.so.1 ./temp_mnt/lib/ld-musl-riscv64-sf.so.1; \
		echo "  Copied musl loader ld-musl-riscv64-sf.so.1 to /lib/"; \
	elif [ -f ./temp_mnt/musl/lib/libc.so ]; then \
		cp ./temp_mnt/musl/lib/libc.so ./temp_mnt/lib/ld-musl-riscv64-sf.so.1; \
		echo "  Created /lib/ld-musl-riscv64-sf.so.1 from musl libc.so"; \
	else \
		echo "  Warning: musl loader not found under /musl/lib, dynamic musl binaries may fail"; \
	fi
	@echo "Step 4b: Setting up double-float musl loader for LTP..."
	@if [ -f /opt/riscv64-linux-musl-cross/riscv64-linux-musl/lib/libc.so ]; then \
		cp /opt/riscv64-linux-musl-cross/riscv64-linux-musl/lib/libc.so ./temp_mnt/lib/ld-musl-riscv64.so.1; \
		echo "  Copied host double-float musl libc.so to /lib/ld-musl-riscv64.so.1"; \
	else \
		echo "  Warning: host double-float musl libc.so not found"; \
	fi
else ifeq ($(ARCH), loongarch64)
	@echo "Step 4: Setting up /lib64 for LoongArch64 glibc..."
	@mkdir -p ./temp_mnt/lib64
	@if [ -f ./temp_mnt/glibc/lib/ld-linux-loongarch-lp64d.so.1 ]; then \
		cp ./temp_mnt/glibc/lib/ld-linux-loongarch-lp64d.so.1 ./temp_mnt/lib64/; \
		echo "  Copied ld-linux-loongarch-lp64d.so.1 to /lib64/"; \
	fi
	@if [ -f ./temp_mnt/glibc/lib/libc.so.6 ]; then \
		cp ./temp_mnt/glibc/lib/libc.so.6 ./temp_mnt/lib64/; \
		echo "  Copied libc.so.6 to /lib64/"; \
	fi
	@if [ -f ./temp_mnt/glibc/lib/libm.so.6 ]; then \
		cp ./temp_mnt/glibc/lib/libm.so.6 ./temp_mnt/lib64/; \
		echo "  Copied libm.so.6 to /lib64/"; \
	fi
	@if [ -f ./temp_mnt/glibc/lib/libdl.so.2 ]; then \
		cp ./temp_mnt/glibc/lib/libdl.so.2 ./temp_mnt/lib64/; \
		echo "  Copied libdl.so.2 to /lib64/"; \
	fi
	@if [ -f ./temp_mnt/glibc/lib/libpthread.so.0 ]; then \
		cp ./temp_mnt/glibc/lib/libpthread.so.0 ./temp_mnt/lib64/; \
		echo "  Copied libpthread.so.0 to /lib64/"; \
	fi

	@echo "Step 5: Setting up /lib for LoongArch64 musl..."
	@mkdir -p ./temp_mnt/lib
	@if [ -f ./temp_mnt/musl/lib/ld-musl-loongarch-lp64d.so.1 ]; then \
		cp ./temp_mnt/musl/lib/ld-musl-loongarch-lp64d.so.1 ./temp_mnt/lib/; \
		echo "  Copied musl loader to /lib/"; \
	elif [ -f ./temp_mnt/musl/lib/libc.so ]; then \
		cp ./temp_mnt/musl/lib/libc.so ./temp_mnt/lib/ld-musl-loongarch-lp64d.so.1; \
		echo "  Created /lib/ld-musl-loongarch-lp64d.so.1 from musl libc.so"; \
	fi
	@if [ -f ./temp_mnt/lib/ld-musl-loongarch-lp64d.so.1 ]; then \
		cp ./temp_mnt/lib/ld-musl-loongarch-lp64d.so.1 ./temp_mnt/lib64/ld-musl-loongarch-lp64d.so.1; \
		echo "  Copied musl loader to /lib64/ for PT_INTERP compatibility"; \
	fi
endif

	@sync && umount ./temp_mnt && rmdir ./temp_mnt
	@echo "Injection finished."
	


qemu-version-check:
	@echo "Using QEMU: $(QEMU_NAME)"


.PHONY: build env kernel clean disasm disasm-vim run-inner gdbserver gdbclient qemu-version-check run-sdcard run-sdcard-rv-noltp patch-sdcard patch-sdcard-noltp do-patch-sdcard
