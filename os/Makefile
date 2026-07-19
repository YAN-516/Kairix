ARCH ?= riscv64
BOARD ?= qemu

ifeq ($(ARCH), riscv64)
    TARGET := riscv64gc-unknown-none-elf
    LINKER_SCRIPT := src/linker-riscv64.ld
    SDCARD_IMG ?= ../sdcard-rv.img
else ifeq ($(ARCH), loongarch64)
    TARGET := loongarch64-unknown-none
    LINKER_SCRIPT := src/linker-loongarch64.ld
    SDCARD_IMG ?= ../sdcard-la.img
else
    $(error Unsupported ARCH: $(ARCH))
endif

MODE := release
KERNEL_ELF := target/$(TARGET)/$(MODE)/os
DISASM_TMP := target/$(TARGET)/$(MODE)/asm
USER_TARGET_DIR := ../user/target/$(TARGET)/$(MODE)
MKFS_TOOLS_DIR := ../tools/target/mkfs-$(ARCH)/sbin

LOG ?= OFF
AUTO_TEST ?= 1
VF2_SD_SMOKE ?= 0
VF2_ROOT_PART ?= 5
INITPROC_NO_AUTOTEST_FLAG ?= .initproc-no-autotest
export LOG
CARGO_OFFLINE ?= 1
CARGO_LOCK_ARGS := --locked
TOOLCHAIN ?= nightly-2025-01-18-x86_64-unknown-linux-gnu
RUSTUP_HOME ?= $(HOME)/.rustup
TOOLCHAIN_BIN := $(RUSTUP_HOME)/toolchains/$(TOOLCHAIN)/bin
ifneq ($(wildcard $(TOOLCHAIN_BIN)/cargo),)
	CARGO := $(TOOLCHAIN_BIN)/cargo
	export RUSTC := $(TOOLCHAIN_BIN)/rustc
else
	CARGO := cargo
endif
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

KERNEL_RUSTFLAGS := -C link-arg=-T$(LINKER_SCRIPT) -C force-frame-pointers=yes --cfg=board="$(BOARD)" --cfg=vf2_root_part="$(VF2_ROOT_PART)" --check-cfg=cfg(board,values("qemu","2k1000","visionfive2")) --check-cfg=cfg(vf2_sd_smoke) --check-cfg=cfg(vf2_root_part,values("4","5","6","7","8"))
ifeq ($(VF2_SD_SMOKE),1)
    KERNEL_RUSTFLAGS += --cfg=vf2_sd_smoke
endif

ifeq ($(ARCH), riscv64)
    OBJDUMP := rust-objdump --arch-name=riscv64
    OBJCOPY := rust-objcopy --binary-architecture=riscv64
else ifeq ($(ARCH), loongarch64)
    OBJDUMP := rust-objdump --arch-name=loongarch64
    OBJCOPY := rust-objcopy --binary-architecture=loongarch64
endif

DISASM ?= -x

build: kernel

env:
	@command -v rustc > /dev/null || (echo "Error: missing rustc." >&2; exit 1)
	@$(CARGO) --version > /dev/null || (echo "Error: missing cargo." >&2; exit 1)

prepare-embedded: env mkfs-tools
	@echo "Preparing embedded runtime files for $(ARCH)..."
	@cd ../user && make elf MODE=$(MODE) TARGET=$(TARGET)

kernel: prepare-embedded
	@echo Architecture: $(ARCH)
	@RUSTFLAGS='$(KERNEL_RUSTFLAGS)' $(CARGO) build $(CARGO_CONFIG) $(MODE_ARG) $(CARGO_LOCK_ARGS) --target $(TARGET)
	@$(OBJCOPY) -O binary $(KERNEL_ELF) $(KERNEL_ELF).bin


clean: clean-mkfs
	@$(CARGO) clean $(CARGO_CONFIG)
	@cd ../user && make clean

clean-mkfs:
	@bash ../tools/build-mkfs.sh clean

disasm: kernel
	@$(OBJDUMP) $(DISASM) $(KERNEL_ELF) | less

disasm-vim: kernel
	@$(OBJDUMP) $(DISASM) $(KERNEL_ELF) > $(DISASM_TMP)
	@vim $(DISASM_TMP)
	@rm $(DISASM_TMP)

patch-sdcard:
	@$(MAKE) do-patch-sdcard IMG=$(SDCARD_IMG)

mkfs-tools:
	@bash ../tools/build-mkfs.sh $(ARCH)

do-patch-sdcard: mkfs-tools
	@echo "Step 1: Building user apps..."
	@cd ../user && make build MODE=$(MODE) TARGET=$(TARGET)

	@echo "Step 2: Injecting apps into $(IMG)..."
	@e2fsck -f -y $(IMG) || true
	@set -e; \
	img=$$(realpath "$(IMG)"); \
	mnt=$$(mktemp -d "$${TMPDIR:-/tmp}/kairix-sdcard.XXXXXX"); \
	mounted=0; \
	cleanup() { \
		status=$$?; \
		if [ "$$mounted" = "1" ] && mountpoint -q "$$mnt"; then \
			if umount "$$mnt"; then \
				mounted=0; \
			else \
				status=$$?; \
			fi; \
		fi; \
		rmdir "$$mnt" 2>/dev/null || true; \
		exit $$status; \
	}; \
	trap cleanup EXIT INT TERM; \
	mount "$$img" "$$mnt"; \
	mounted=1; \
	rm -f "$$mnt/$(INITPROC_NO_AUTOTEST_FLAG)"; \
	if [ "$(AUTO_TEST)" = "0" ] || [ "$(AUTO_TEST)" = "false" ] || [ "$(AUTO_TEST)" = "FALSE" ] || [ "$(AUTO_TEST)" = "no" ] || [ "$(AUTO_TEST)" = "NO" ] || [ "$(AUTO_TEST)" = "off" ] || [ "$(AUTO_TEST)" = "OFF" ]; then \
		echo "Auto test disabled; creating /$(INITPROC_NO_AUTOTEST_FLAG)"; \
		: > "$$mnt/$(INITPROC_NO_AUTOTEST_FLAG)"; \
	else \
		echo "Auto test enabled; /$(INITPROC_NO_AUTOTEST_FLAG) removed"; \
	fi; \
	for app in initproc user_shell ls basictests libctests_static libctests_dynamic fork_cow_pressure mmap_prot_sig sshtest; do \
		echo "  Copying $$app..."; \
		cp "$(USER_TARGET_DIR)/$$app" "$$mnt/$$app"; \
		chmod +x "$$mnt/$$app"; \
	done; \
	echo "Step 3: Skipping /bin symlinks (now handled by initproc at runtime)"; \
	echo "Step 3b: Installing real mkfs.ext2/ext3/ext4 tools..."; \
	bash ../tools/install-mkfs-ext.sh $(ARCH) "$$mnt" $(MKFS_TOOLS_DIR); \
	bash ../tools/setup-sdcard-libs.sh $(ARCH) "$$mnt"; \
	sync; \
	umount "$$mnt"; \
	mounted=0; \
	[ ! -d "$$mnt" ] || rmdir "$$mnt"; \
	trap - EXIT INT TERM; \
	echo "Injection finished."

.PHONY: build env prepare-embedded kernel clean clean-mkfs disasm disasm-vim patch-sdcard mkfs-tools do-patch-sdcard
