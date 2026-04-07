OUTPUT_ARCH("loongarch")
ENTRY(_start)
BASE_ADDRESS = 0x9000000080000000;

SECTIONS
{
    . = BASE_ADDRESS;
    _skernel = .;

    stext = .;
    .text : {
        *(.text.entry)
        *(.text .text.*)
    }

    . = ALIGN(4K);
    etext = .;
    srodata = .;
    .rodata : {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    }

    . = ALIGN(4K);
    erodata = .;
    sdata = .;
    .data : {
        *(.data .data.*)
        *(.sdata .sdata.*)
    }

    . = ALIGN(4K);
    edata = .;
    _sbss = .;
    kernel_stack_bottom = .;
    .bss : {
        *(.bss.stack)
        kernel_stack_top = .;
        . = ALIGN(4K);
        sbss = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
    }

    .bss.bstack (NOLOAD) : {
    bstack = .;
    . += 0x80000;
    bstack_top = .;    
    }

    . = ALIGN(4K);
    _ebss = .;
    ekernel = .;
    _end = .;
    /DISCARD/ : {
        *(.eh_frame)
    }
}

EXTERN(sbss);
EXTERN(ebss);