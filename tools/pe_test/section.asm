; The entire code+data content of the one PE section our hand-built test
; binary has, assembled to a flat blob with NASM and wrapped in real PE32+
; headers by pack_pe.py. `org` is set to the exact runtime virtual address
; (ImageBase + section RVA) this blob loads at, so `label - IMAGE_BASE`
; expressions below compute correct RVAs for the PE structures (Import
; Directory Table, DLL/function name strings) at assembly time, while
; `default rel` makes the actual code addressing RIP-relative (required
; regardless, since ImageBase doesn't fit a 32-bit absolute displacement).
BITS 64
default rel
org 0x140001000

IMAGE_BASE equ 0x140000000

entry:
    ; WriteConsoleA(hConsoleOutput=0, lpBuffer=msg, nNumberOfCharsToWrite=msg_len,
    ;               lpNumberOfCharsWritten=NULL, lpReserved=NULL)
    sub rsp, 0x28
    xor ecx, ecx
    lea rdx, [msg]
    mov r8d, msg_len
    xor r9d, r9d
    call [iat_writeconsolea]
    add rsp, 0x28

    ; ExitProcess(0)
    sub rsp, 0x28
    xor ecx, ecx
    call [iat_exitprocess]
    add rsp, 0x28

.spin:
    jmp .spin

align 8
; The Import Lookup Table and Import Address Table share this one array:
; the loader reads each slot as an RVA-to-IMAGE_IMPORT_BY_NAME first (to
; resolve the name), then overwrites that same slot with the resolved
; thunk address -- safe because it processes strictly left to right.
iat_exitprocess:   dq (ibn_exitprocess - IMAGE_BASE)
iat_writeconsolea: dq (ibn_writeconsolea - IMAGE_BASE)
iat_null:          dq 0

align 8
ibn_exitprocess:
    dw 0
    db "ExitProcess", 0

align 8
ibn_writeconsolea:
    dw 0
    db "WriteConsoleA", 0

align 8
dll_name: db "KERNEL32.DLL", 0

align 8
import_descriptors:
    ; KERNEL32.DLL
    dd (iat_exitprocess - IMAGE_BASE)  ; OriginalFirstThunk (ILT RVA)
    dd 0                                 ; TimeDateStamp
    dd 0                                 ; ForwarderChain
    dd (dll_name - IMAGE_BASE)          ; Name RVA
    dd (iat_exitprocess - IMAGE_BASE)  ; FirstThunk (IAT RVA) -- same array
    ; null terminator descriptor
    dd 0
    dd 0
    dd 0
    dd 0
    dd 0

align 8
msg: db "Hello from moon OS PE loader (ring 3, Win32-ish)!", 10
msg_len equ $ - msg

blob_end:
