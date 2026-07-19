TARGET ?= riscv64gc-unknown-none-elf

MODE := release
APP_DIR := src/bin
TARGET_DIR := target/$(TARGET)/$(MODE)
COMPETITION_APP_OVERRIDES := \
    gcc rustc \
    git gitadd gitbranch gitcheckout gitcheckoutref gitclone gitcommit gitconfig gitdiff \
    gitfetch gitinit gitlog gitls gitpack gitpkt_test gitpull gitpush gitremote gitstatus
EXCLUDED_APPS := $(addprefix $(APP_DIR)/,$(addsuffix .rs,$(COMPETITION_APP_OVERRIDES)))
APPS := $(filter-out $(EXCLUDED_APPS),$(wildcard $(APP_DIR)/*.rs))
APP_NAMES := $(patsubst $(APP_DIR)/%.rs,%,$(APPS))
CARGO_BIN_ARGS := $(addprefix --bin ,$(APP_NAMES))
ELFS := $(patsubst $(APP_DIR)/%.rs, $(TARGET_DIR)/%, $(APPS))
BINS := $(patsubst $(APP_DIR)/%.rs, $(TARGET_DIR)/%.bin, $(APPS))
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
    CARGO_CONFIG := --config ../tools/cargo-config-user.toml
endif
ifeq ($(CARGO_OFFLINE),1)
    CARGO_LOCK_ARGS += --offline
    export CARGO_NET_OFFLINE := true
endif

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

elf: $(APPS)
	@$(CARGO) build $(CARGO_CONFIG) --release $(CARGO_LOCK_ARGS) $(CARGO_ARGS) $(CARGO_BIN_ARGS)
ifeq ($(TEST), 1)
	@$(CP) $(TARGET_DIR)/usertests $(TARGET_DIR)/initproc
endif

binary: elf
	@$(foreach elf, $(ELFS), $(OBJCOPY) $(elf) --strip-all -O binary $(patsubst $(TARGET_DIR)/%, $(TARGET_DIR)/%.bin, $(elf));)

build: binary

clean:
	@$(CARGO) clean $(CARGO_CONFIG)

.PHONY: elf binary build clean
