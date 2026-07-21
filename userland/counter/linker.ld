OUTPUT_FORMAT(elf64-x86-64)
ENTRY(_start)

/* Classic low, fixed load address for a static (non-PIE) executable --
   comfortably clear of the null page and of the kernel's own higher-half
   range, and well within the canonical lower-half address range. */
BASE = 0x400000;

PHDRS
{
    text   PT_LOAD FLAGS((1 << 2) | (1 << 0)); /* R + X */
    rodata PT_LOAD FLAGS((1 << 2)) ;           /* R     */
    data   PT_LOAD FLAGS((1 << 2) | (1 << 1)); /* R + W */
}

SECTIONS
{
    . = BASE;

    .text : {
        *(.text .text.*)
    } :text

    . += CONSTANT(MAXPAGESIZE);

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
        *(.eh_frame_hdr)
        *(.note .note.*)
        *(.comment)
    }
}
