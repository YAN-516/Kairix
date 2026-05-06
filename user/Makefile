TARGET ?= riscv64gc-unknown-none-elf

MODE := release
APP_DIR := src/bin
TARGET_DIR := target/$(TARGET)/$(MODE)
APPS := $(wildcard $(APP_DIR)/*.rs)
ELFS := $(patsubst $(APP_DIR)/%.rs, $(TARGET_DIR)/%, $(APPS))
BINS := $(patsubst $(APP_DIR)/%.rs, $(TARGET_DIR)/%.bin, $(APPS))

CP := cp

ifeq ($(TARGET), riscv64gc-unknown-none-elf)
    ARCH := riscv64
    CARGO_ARGS += --target riscv64gc-unknown-none-elf
    OBJDUMP := rust-objdump --arch-name=riscv64
    OBJCOPY := rust-objcopy --binary-architecture=riscv64
else ifeq ($(TARGET), loongarch64-unknown-none)
    ARCH := loongarch64
    CARGO_ARGS += --target loongarch64-unknown-none
    OBJDUMP := rust-objdump --arch-name=loongarch64
    OBJCOPY := rust-objcopy --binary-architecture=loongarch64
endif

TEST ?=
LIBCBENCH ?=

elf: $(APPS)
	@cargo build --release $(CARGO_ARGS)
ifeq ($(TEST), 1)
	@$(CP) $(TARGET_DIR)/usertests $(TARGET_DIR)/initproc
endif
ifeq ($(LIBCBENCH), 1)
	@$(CP) $(TARGET_DIR)/libcbench $(TARGET_DIR)/initproc
endif

binary: elf
	@$(foreach elf, $(ELFS), $(OBJCOPY) $(elf) --strip-all -O binary $(patsubst $(TARGET_DIR)/%, $(TARGET_DIR)/%.bin, $(elf));)

build: binary

clean:
	@cargo clean

.PHONY: elf binary build clean
