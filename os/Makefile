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
AUTO_TEST ?= final
VF2_SD_SMOKE ?= 0
VF2_ROOT_PART ?= 5
INITPROC_NO_AUTOTEST_FLAG ?= .initproc-no-autotest
INITPROC_PRELIMINARY_TEST_FLAG ?= .initproc-preliminary-tests
# Global BuildStorm attribution is always installed at the path selected by
# initproc's final test suite. Keep this fixed so `make rkernel_test` and
# `make lkernel_test` cannot accidentally run a stale image copy.
BUILDSTORM_TESTCODE := $(abspath ../user/buildstorm_1.sh)
BUILDSTORM_DIAG_VERSION := 2026-08-02.17
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
	@test -f "$(BUILDSTORM_TESTCODE)" || (echo "Error: missing hard-coded BuildStorm script: $(BUILDSTORM_TESTCODE)" >&2; exit 1)
	@grep -Fq 'BUILDSTORM_DIAG_VERSION $(BUILDSTORM_DIAG_VERSION)' "$(BUILDSTORM_TESTCODE)" || (echo "Error: BuildStorm diagnostic version mismatch: expected $(BUILDSTORM_DIAG_VERSION)" >&2; exit 1)
	@grep -Fq '/proc/kairix_perf' "$(BUILDSTORM_TESTCODE)" || (echo "Error: BuildStorm script does not sample /proc/kairix_perf" >&2; exit 1)
	@for metric in fault_perf block_perf ext4_perf; do \
		grep -Fq "/^$$metric:/" "$(BUILDSTORM_TESTCODE)" || { echo "Error: BuildStorm script does not parse $$metric" >&2; exit 1; }; \
	done
	@! grep -Fq '/^heap_perf:/' "$(BUILDSTORM_TESTCODE)" || (echo "Error: allocator heap diagnostics must not be emitted by BUILDSTORM_GLOBAL" >&2; exit 1)
	@grep -Fq 'ext4_block_group_wait_ns' "$(BUILDSTORM_TESTCODE)" || (echo "Error: BuildStorm script does not emit ext4 block-group wait time" >&2; exit 1)
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
	mkdir -p "$$mnt/glibc"; \
	echo "  Installing BuildStorm diagnostics $(BUILDSTORM_DIAG_VERSION)..."; \
	cp "$(BUILDSTORM_TESTCODE)" "$$mnt/glibc/buildstorm_testcode.sh"; \
	chmod +x "$$mnt/glibc/buildstorm_testcode.sh"; \
	cmp -s "$(BUILDSTORM_TESTCODE)" "$$mnt/glibc/buildstorm_testcode.sh" || { \
		echo "Error: BuildStorm script verification failed after copy" >&2; \
		exit 1; \
	}; \
	rm -f "$$mnt/$(INITPROC_NO_AUTOTEST_FLAG)" "$$mnt/$(INITPROC_PRELIMINARY_TEST_FLAG)"; \
	case "$(AUTO_TEST)" in \
		final|1|true|TRUE|yes|YES|on|ON) \
			echo "Final auto tests enabled"; \
			;; \
		preliminary|prelim) \
			echo "Preliminary auto tests enabled; creating /$(INITPROC_PRELIMINARY_TEST_FLAG)"; \
			: > "$$mnt/$(INITPROC_PRELIMINARY_TEST_FLAG)"; \
			;; \
		off|none|0|false|FALSE|no|NO|OFF) \
			echo "Auto test disabled; creating /$(INITPROC_NO_AUTOTEST_FLAG)"; \
			: > "$$mnt/$(INITPROC_NO_AUTOTEST_FLAG)"; \
			;; \
		*) \
			echo "Error: unsupported AUTO_TEST=$(AUTO_TEST); use final, preliminary, or off" >&2; \
			exit 2; \
			;; \
	esac; \
	for app in ext4_linker_sparse_write_test initproc user_shell ls basictests libctests_static libctests_dynamic fork_cow_pressure mmap_prot_sig lazy_tlb_pair_test mmap_fault_around_test sshtest smp_load_test fp_context_test realtime_precision_test shm_futex_test concurrent_fsync_test glibc_fork_select_test rseq_hwprobe_test procfs_compat_test buildstorm_compat_test futex_timeout_test iozone_regression shared_anon_fork_test eventfd_epoll_test proc_self_exe_test multithread_exec_test nonleader_exec_test multithread_exec_target multithread_fork_test fcntl_lock_test shebang_exec_test cargo_cache_integrity_test ext4_exec_coherence_test ext4_invalidation_exec_test ext4_sync_metadata_test ext4_lock_fairness_test ext4_linker_sparse_write_test ext4_exit_publish_test page_cache_shard_test ext4_parallel_read_test ext4_stage3_concurrency_test socket_user_pointer_test utime_realtime_test vfork_exec_failure_test; do \
		echo "  Copying $$app..."; \
		cp "$(USER_TARGET_DIR)/$$app" "$$mnt/$$app"; \
		chmod +x "$$mnt/$$app"; \
	done; \
	mkdir -p "$$mnt/bin"; \
	invalid_sh=0; \
	if [ -f "$$mnt/bin/sh" ]; then \
		for candidate in "$(USER_TARGET_DIR)/user_shell" "$$mnt/bin/user_shell" "$$mnt/user_shell"; do \
			if [ -f "$$candidate" ] && cmp -s "$$candidate" "$$mnt/bin/sh"; then \
				invalid_sh=1; \
				break; \
			fi; \
		done; \
	fi; \
	rm -f "$$mnt/bin/user_shell"; \
	cp "$(USER_TARGET_DIR)/user_shell" "$$mnt/bin/user_shell"; \
	chmod +x "$$mnt/bin/user_shell"; \
	if [ "$$invalid_sh" = "1" ]; then \
		echo "  Replacing invalid /bin/sh copy of user_shell..."; \
		rm -f "$$mnt/bin/sh"; \
	fi; \
	if [ -L "$$mnt/bin/sh" ] && [ ! -e "$$mnt/bin/sh" ]; then \
		rm -f "$$mnt/bin/sh"; \
	fi; \
	if [ ! -e "$$mnt/bin/sh" ]; then \
		if [ -x "$$mnt/bin/bash" ]; then \
			ln -s bash "$$mnt/bin/sh"; \
		elif [ -x "$$mnt/bin/busybox" ]; then \
			ln -s busybox "$$mnt/bin/sh"; \
		elif [ -x "$$mnt/musl/busybox" ]; then \
			ln -s ../musl/busybox "$$mnt/bin/sh"; \
		elif [ -x "$$mnt/glibc/busybox" ]; then \
			ln -s ../glibc/busybox "$$mnt/bin/sh"; \
		else \
			echo "Error: rootfs has no POSIX shell for /bin/sh" >&2; \
			exit 1; \
		fi; \
	fi; \
	if [ ! -x "$$mnt/bin/sh" ] || cmp -s "$(USER_TARGET_DIR)/user_shell" "$$mnt/bin/sh"; then \
		echo "Error: /bin/sh must be a POSIX shell and must not be user_shell" >&2; \
		exit 1; \
	fi; \
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
