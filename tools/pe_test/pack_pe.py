#!/usr/bin/env python3
"""Wraps section.bin (assembled by NASM from section.asm) in real PE32+
headers, producing a minimal, hand-built, non-relocatable x86_64 Windows
executable used to test kernel/src/pe.rs end to end. There's no Windows
cross-toolchain in this environment, so this script -- not `link.exe` or
mingw -- is what "compiles" the test binary; it constructs the DOS/NT/
Optional/section headers by hand from well-documented, fixed PE32+ struct
layouts (see e.g. the Microsoft PE/COFF specification).
"""
import struct
import sys

IMAGE_BASE = 0x140000000
SECTION_RVA = 0x1000
FILE_ALIGNMENT = 512
SECTION_ALIGNMENT = 0x1000

ENTRY_RVA = 0x1000          # `entry` label -- first byte of the blob
IMPORT_DIR_RVA = 0x1080     # `import_descriptors` label, from section.lst
IMPORT_DIR_SIZE = 40        # 2 IMAGE_IMPORT_DESCRIPTORs (one real, one null)


def round_up(value, align):
    return (value + align - 1) // align * align


def build(section_bytes: bytes) -> bytes:
    section_raw_size = round_up(len(section_bytes), FILE_ALIGNMENT)
    section_data = section_bytes + b"\x00" * (section_raw_size - len(section_bytes))

    num_sections = 1
    size_of_optional_header = 112 + 16 * 8  # PE32+ fixed fields + 16 data directories
    headers_size = 64 + 4 + 20 + size_of_optional_header + num_sections * 40
    size_of_headers = round_up(headers_size, FILE_ALIGNMENT)

    size_of_image = round_up(SECTION_RVA + len(section_bytes), SECTION_ALIGNMENT)

    # -- DOS header (64 bytes) --------------------------------------
    # Only e_magic ("MZ") and e_lfanew (offset to the PE signature) matter
    # to our loader; the rest of the fields are zeroed rather than filled
    # with a real DOS stub, since nothing ever executes them.
    pe_offset = 64
    dos_header = bytearray(64)
    dos_header[0:2] = b"MZ"
    struct.pack_into("<I", dos_header, 0x3C, pe_offset)

    # -- PE signature + COFF header (24 bytes) -----------------------
    IMAGE_FILE_MACHINE_AMD64 = 0x8664
    IMAGE_FILE_EXECUTABLE_IMAGE = 0x0002
    IMAGE_FILE_LARGE_ADDRESS_AWARE = 0x0020
    coff = struct.pack(
        "<4sHHIIIHH",
        b"PE\x00\x00",
        IMAGE_FILE_MACHINE_AMD64,
        num_sections,
        0,  # TimeDateStamp
        0,  # PointerToSymbolTable
        0,  # NumberOfSymbols
        size_of_optional_header,
        IMAGE_FILE_EXECUTABLE_IMAGE | IMAGE_FILE_LARGE_ADDRESS_AWARE,
    )

    # -- Optional header (PE32+) --------------------------------------
    IMAGE_NT_OPTIONAL_HDR64_MAGIC = 0x20B
    IMAGE_SUBSYSTEM_WINDOWS_CUI = 3
    opt = struct.pack(
        "<HBBIIIIIQIIHHHHHHIIIIHHQQQQII",
        IMAGE_NT_OPTIONAL_HDR64_MAGIC,
        1, 0,               # Major/MinorLinkerVersion
        len(section_bytes),  # SizeOfCode (approximation: whole section)
        0,                   # SizeOfInitializedData
        0,                   # SizeOfUninitializedData
        ENTRY_RVA,
        SECTION_RVA,         # BaseOfCode
        IMAGE_BASE,
        SECTION_ALIGNMENT,
        FILE_ALIGNMENT,
        0, 0,                # Major/MinorOperatingSystemVersion
        0, 0,                # Major/MinorImageVersion
        6, 0,                # Major/MinorSubsystemVersion (6.0, ~Vista+)
        0,                   # Win32VersionValue
        size_of_image,
        size_of_headers,
        0,                   # CheckSum
        IMAGE_SUBSYSTEM_WINDOWS_CUI,
        0,                   # DllCharacteristics
        0x100000, 0x1000,    # SizeOfStackReserve/Commit
        0x100000, 0x1000,    # SizeOfHeapReserve/Commit
        0,                   # LoaderFlags
        16,                  # NumberOfRvaAndSizes
    )
    data_directories = bytearray(16 * 8)
    # DataDirectory[1] = Import Table
    struct.pack_into("<II", data_directories, 1 * 8, IMPORT_DIR_RVA, IMPORT_DIR_SIZE)
    opt += bytes(data_directories)
    assert len(opt) == size_of_optional_header, (len(opt), size_of_optional_header)

    # -- Section header (40 bytes) -------------------------------------
    IMAGE_SCN_MEM_EXECUTE = 0x20000000
    IMAGE_SCN_MEM_READ = 0x40000000
    IMAGE_SCN_CNT_CODE = 0x00000020
    section_header = struct.pack(
        "<8sIIIIIIHHI",
        b".text\x00\x00\x00",
        len(section_bytes),  # VirtualSize
        SECTION_RVA,
        section_raw_size,    # SizeOfRawData
        size_of_headers,     # PointerToRawData
        0, 0,                # PointerToRelocations/Linenumbers
        0, 0,                # NumberOfRelocations/Linenumbers
        IMAGE_SCN_CNT_CODE | IMAGE_SCN_MEM_EXECUTE | IMAGE_SCN_MEM_READ,
    )

    headers = bytes(dos_header) + coff + opt + section_header
    headers += b"\x00" * (size_of_headers - len(headers))

    return headers + section_data


def main():
    if len(sys.argv) != 3:
        print(f"usage: {sys.argv[0]} <section.bin> <output.exe>", file=sys.stderr)
        sys.exit(1)
    with open(sys.argv[1], "rb") as f:
        section_bytes = f.read()
    image = build(section_bytes)
    with open(sys.argv[2], "wb") as f:
        f.write(image)
    print(f"wrote {sys.argv[2]}: {len(image)} bytes")


if __name__ == "__main__":
    main()
