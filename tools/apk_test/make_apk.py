#!/usr/bin/env python3
"""Builds a minimal, valid ZIP archive containing one STORED entry,
AndroidManifest.xml, whose contents are a real Android Binary XML (AXML)
ResChunk_header (type=RES_XML_TYPE, headerSize=8) followed by a little
padding -- enough to exercise kernel/src/apk.rs's ZIP + AXML-header parsing
end to end. This is a fixture for testing the parser, not a real Android
manifest: the string pool and element tree a real AndroidManifest.xml has
after this header are not included (decoding those is future work, see
kernel/src/apk.rs's doc comment).

No `zipfile`-module shortcuts: the point is a from-scratch, byte-exact ZIP
so the test genuinely exercises apk.rs's own parsing rather than round
-tripping through Python's ZIP implementation.
"""
import struct
import sys
import zlib

ENTRY_NAME = b"AndroidManifest.xml"

RES_XML_TYPE = 0x0003
HEADER_SIZE = 8
PADDING = b"\x00" * 8
CHUNK_SIZE = HEADER_SIZE + len(PADDING)
axml_data = struct.pack("<HHI", RES_XML_TYPE, HEADER_SIZE, CHUNK_SIZE) + PADDING


def build() -> bytes:
    crc = zlib.crc32(axml_data) & 0xFFFFFFFF

    local_header = struct.pack(
        "<IHHHHHIIIHH",
        0x04034B50,
        20,  # version needed to extract
        0,  # flags
        0,  # compression method: STORED
        0, 0,  # mod time/date
        crc,
        len(axml_data),  # compressed size (== uncompressed, STORED)
        len(axml_data),
        len(ENTRY_NAME),
        0,  # extra field length
    )
    local_offset = 0
    local_entry = local_header + ENTRY_NAME + axml_data

    central_header = struct.pack(
        "<IHHHHHHIIIHHHHHII",
        0x02014B50,
        20,  # version made by
        20,  # version needed to extract
        0,  # flags
        0,  # compression method: STORED
        0, 0,  # mod time/date
        crc,
        len(axml_data),
        len(axml_data),
        len(ENTRY_NAME),
        0,  # extra field length
        0,  # comment length
        0,  # disk number start
        0,  # internal attributes
        0,  # external attributes
        local_offset,
    )
    central_entry = central_header + ENTRY_NAME

    central_dir_offset = len(local_entry)
    eocd = struct.pack(
        "<IHHHHIIH",
        0x06054B50,
        0, 0,  # this disk / disk with central dir
        1, 1,  # entries on this disk / total entries
        len(central_entry),
        central_dir_offset,
        0,  # comment length
    )

    return local_entry + central_entry + eocd


def main():
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} <output.apk>", file=sys.stderr)
        sys.exit(1)
    image = build()
    with open(sys.argv[1], "wb") as f:
        f.write(image)
    print(f"wrote {sys.argv[1]}: {len(image)} bytes")


if __name__ == "__main__":
    main()
