OUTPUT_FORMAT(elf64-x86-64)
ENTRY(kmain)

/* Higher-half kernel, matches the Limine higher-half-kernel default load address. */
KERNEL_VMA = 0xffffffff80000000;

PHDRS
{
    text    PT_LOAD    FLAGS((1 << 2) | (1 << 0)) ; /* R + X */
    rodata  PT_LOAD    FLAGS((1 << 2)) ;             /* R     */
    data    PT_LOAD    FLAGS((1 << 2) | (1 << 1)) ; /* R + W */
}

SECTIONS
{
    . = KERNEL_VMA;

    .text : {
        *(.text .text.*)
    } :text

    . += CONSTANT(MAXPAGESIZE);

    .requests_start_marker : {
        KEEP(*(.requests_start_marker))
    } :rodata

    .requests : {
        KEEP(*(.requests))
    } :rodata

    .requests_end_marker : {
        KEEP(*(.requests_end_marker))
    } :rodata

    .rodata : {
        *(.rodata .rodata.*)
    } :rodata

    . += CONSTANT(MAXPAGESIZE);

    .data : {
        *(.data .data.*)
    } :data

    .bss : {
        *(COMMON)
        *(.bss .bss.*)
    } :data

    /DISCARD/ : {
        *(.eh_frame)
        *(.note .note.*)
        *(.comment)
    }
}
