OUTPUT_ARCH(riscv)
ENTRY(_start_phys)
BASE_ADDRESS = 0xFFFFFFC080200000;
LOAD_ADDRESS = 0x80200000;
KERNEL_OFFSET = BASE_ADDRESS - LOAD_ADDRESS;

SECTIONS
{
    . = BASE_ADDRESS;
    _skernel = .;
    _start_phys = LOAD_ADDRESS;

    stext = .;
    .text : AT(ADDR(.text) - KERNEL_OFFSET) {
        KEEP(*(.text.entry))
        *(.text .text.*)
    }

    . = ALIGN(4K);
    etext = .;
    srodata = .;
    .rodata : AT(ADDR(.rodata) - KERNEL_OFFSET) {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    }

    . = ALIGN(4K);
    erodata = .;
    sdata = .;
    .data : AT(ADDR(.data) - KERNEL_OFFSET) {
        *(.data .data.*)
        *(.sdata .sdata.*)
    }

    . = ALIGN(4K);
    edata = .;
    _sbss = .;
    .bss : AT(ADDR(.bss) - KERNEL_OFFSET) {
        *(.bss.stack)
        sbss = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
    }

    . = ALIGN(4K);
    _ebss = .;
    ekernel = .;
    _end = .;

    /DISCARD/ : {
        *(.eh_frame)
        *(.debug*)
        *(.comment)
        *(.gnu*)
        *(.note*)
    }
}
