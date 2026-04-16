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
USER_INITPROC := ../user/target/$(TARGET)/$(MODE)/initproc
# 增加一个变量来决定最终给 QEMU 用的镜像路径，默认是原来的 FS_IMG
SELECTED_IMG ?= $(FS_IMG)
APPS := ../user/src/bin/*
LOG := INFO
CPU := 1
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

build: env $(KERNEL_BIN) fs-img 

env:
	@(rustup target list | grep "riscv64gc-unknown-none-elf (installed)") || rustup target add $(TARGET)
	@# 检查是否安装了 cargo-binutils，如果没安装才去 install
	@which rust-objcopy > /dev/null || cargo install cargo-binutils
	@rustup component add rust-src
	@rustup component add llvm-tools-preview
	@# 检查系统工具是否存在
	@which mkfs.ext4 > /dev/null || (echo "Error: mkfs.ext4 not found. Run: apt install e2fsprogs" && exit 1)
	@which e2cp > /dev/null || (echo "Error: e2cp not found. Run: apt install e2tools" && exit 1)
	
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
	@cargo build $(MODE_ARG)
	@rm src/linker.ld

# 修改后的镜像构建逻辑
fs-img: $(APPS)
	@echo "Step 1: Building user apps..."
	@cd ../user && make build TEST=$(TEST) MODE=$(MODE)
	
	@echo "Step 2: Creating empty $(FS_SIZE)MB disk image..."
	@rm -f $(FS_IMG)
	@dd if=/dev/zero of=$(FS_IMG) bs=1M count=$(FS_SIZE) status=none
	
	@echo "Step 3: Formatting image to ext4..."
	@# 禁用 metadata_csum, 64bit, casefold 等特性以确保与 lwext4 的最大兼容性
	@mkfs.ext4 -F -O ^metadata_csum,^64bit,^extra_isize $(FS_IMG)
	
	@echo "Step 4: Copying user apps into $(FS_IMG) using e2tools..."
	@for app in $$(ls $(USER_TARGET_DIR) | grep -v "\."); do \
		if [ -f $(USER_TARGET_DIR)/$$app ]; then \
			e2cp -p $(USER_TARGET_DIR)/$$app $(FS_IMG):/$$app; \
			echo "  Added $$app"; \
		fi \
	done
	
	@echo "Step 5: Setting permissions..."
	@chmod 777 $(FS_IMG)
	@echo "Ext4 image build finished: $(FS_IMG)"

$(APPS):

clean:
	@cargo clean
	@cd ../user && make clean

disasm: kernel
	@$(OBJDUMP) $(DISASM) $(KERNEL_ELF) | less

disasm-vim: kernel
	@$(OBJDUMP) $(DISASM) $(KERNEL_ELF) > $(DISASM_TMP)
	@vim $(DISASM_TMP)
	@rm $(DISASM_TMP)

run: build
	@$(QEMU_NAME) $(QEMU_ARGS)

QEMU_ARGS ?= -machine virt \
			 -nographic \
			 -bios $(BOOTLOADER) \
			 -device loader,file=$(KERNEL_BIN),addr=$(KERNEL_ENTRY_PA) \
			 -drive file=$(SELECTED_IMG),if=none,format=raw,id=x0 \
			 -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
			 -smp $(CPU) 

ifeq ($(ARCH), riscv64)
	QEMU_ARGS := -machine virt \
			 -nographic \
			 -bios $(BOOTLOADER) \
			 -device loader,file=$(KERNEL_BIN),addr=$(KERNEL_ENTRY_PA) \
			 -drive file=$(SELECTED_IMG),if=none,format=raw,id=x0 \
			 -device virtio-blk-device,drive=x0,bus=virtio-mmio-bus.0 \
			 -smp $(CPU) 
else ifeq ($(ARCH), loongarch64)
    # 龙芯 QEMU 参数
    QEMU_ARGS := -machine virt \
                 -nographic \
				 -rtc base=utc \
				 -no-reboot \
				 -m 1G \
				 -kernel $(KERNEL_ELF) \
                 -drive file=$(SELECTED_IMG),if=none,format=raw,id=hd0 \
    			 -device virtio-blk-pci,drive=hd0 \
                 -smp $(CPU) \
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
	@echo "Running with patched external image: $(SDCARD_IMG)"
	$(MAKE) run-inner SELECTED_IMG=$(SDCARD_IMG)


patch-sdcard:
	@echo "Step 1: Building user apps..."
	@cd ../user && make build MODE=$(MODE)
	
	@echo "Step 2: Injecting specific apps into $(SDCARD_IMG)..."
	@# 建议在挂载前习惯性执行修复，防止损坏
	@e2fsck -f -y $(SDCARD_IMG) || true
	
	@mkdir -p ./temp_mnt
	@mount $(SDCARD_IMG) ./temp_mnt
	
	@# 只拷贝 initproc 和 user_shell 和ls 和basictests
	@echo "  Copying initproc..."
	@cp $(USER_TARGET_DIR)/initproc ./temp_mnt/initproc
	@chmod +x ./temp_mnt/initproc
	
	@echo "  Copying user_shell..."
	@cp $(USER_TARGET_DIR)/user_shell ./temp_mnt/user_shell
	@chmod +x ./temp_mnt/user_shell

	@echo "  Copying ls..."
	@cp $(USER_TARGET_DIR)/ls ./temp_mnt/ls
	@chmod +x ./temp_mnt/ls

	@echo "  Copying basictests..."
	@cp $(USER_TARGET_DIR)/basictests ./temp_mnt/basictests
	@chmod +x ./temp_mnt/basictests

	@sync
	@umount ./temp_mnt
	@rmdir ./temp_mnt
	@echo "Injection finished. Only initproc and user_shell were added."

.PHONY: build env kernel clean disasm disasm-vim run-inner fs-img gdbserver gdbclient qemu-version-check run-sdcard

qemu-version-check:
	@echo "Using QEMU: $(QEMU_NAME)"