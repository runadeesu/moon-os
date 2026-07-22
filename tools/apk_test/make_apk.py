#!/usr/bin/env python3
"""Builds a real (if minimal) Android Binary XML AndroidManifest.xml --
a real string pool plus a real namespace/element/attribute node tree, not
just a bare chunk header -- representing:

  <manifest xmlns:android="http://schemas.android.com/apk/res/android"
            package="com.example.moongame">
    <application android:label="Moon Game Demo" />
  </manifest>

then wraps it as a genuinely DEFLATE-compressed ZIP entry (real
zlib.compressobj raw-deflate output, same as real aapt/aapt2-built APKs
almost always use) so this fixture exercises the full real pipeline --
kernel/src/inflate.rs's DEFLATE decoder feeding kernel/src/apk.rs's string
pool + XML tree decoder -- end to end, not just the chunk-header check the
original version of this fixture tested.

No `zipfile`-module shortcuts: everything here is assembled byte-by-byte
against the real ZIP/AXML formats, so the test genuinely exercises our own
parsers rather than round-tripping through Python's ZIP implementation.
The one place Python's `zlib` *is* used deliberately is producing the
DEFLATE-compressed bytes themselves -- an independent, trusted encoder to
compress against, the same role it played validating kernel/src/inflate.rs's
self-test vectors.
"""
import struct
import sys
import zlib

RES_STRING_POOL_TYPE = 0x0001
RES_XML_TYPE = 0x0003
RES_XML_START_NAMESPACE_TYPE = 0x0100
RES_XML_END_NAMESPACE_TYPE = 0x0101
RES_XML_START_ELEMENT_TYPE = 0x0102
RES_XML_END_ELEMENT_TYPE = 0x0103
TYPE_STRING = 0x03
UTF8_FLAG = 1 << 8
NO_NS = 0xFFFFFFFF

# Index into this list is the AXML string-pool index used everywhere below.
STRINGS = [
    "android",                                      # 0: namespace prefix
    "http://schemas.android.com/apk/res/android",   # 1: namespace uri
    "manifest",                                      # 2: element name
    "package",                                        # 3: attribute name
    "com.example.moongame",                           # 4: attribute value
    "application",                                    # 5: element name
    "label",                                           # 6: attribute name
    "Moon Game Demo",                                  # 7: attribute value
]


def _len_byte(n: int) -> bytes:
    assert n <= 0x7F, "fixture strings are short enough for the 1-byte length form"
    return bytes([n])


def build_string_pool() -> bytes:
    string_data = bytearray()
    offsets = []
    for s in STRINGS:
        offsets.append(len(string_data))
        b = s.encode("utf-8")
        # UTF8_FLAG entries: char-length, byte-length, bytes, NUL terminator.
        string_data += _len_byte(len(s)) + _len_byte(len(b)) + b + b"\x00"
    while len(string_data) % 4 != 0:
        string_data += b"\x00"

    header_size = 28
    strings_start = header_size + 4 * len(STRINGS)
    offsets_bytes = b"".join(struct.pack("<I", o) for o in offsets)
    chunk_size = header_size + len(offsets_bytes) + len(string_data)
    header = struct.pack(
        "<HHIIIII",
        RES_STRING_POOL_TYPE,
        header_size,
        chunk_size,
        len(STRINGS),
        0,  # styleCount
        UTF8_FLAG,
        strings_start,
    ) + struct.pack("<I", 0)  # stylesStart (none)
    chunk = header + offsets_bytes + string_data
    assert len(chunk) == chunk_size
    return chunk


def _node(chunk_type: int, body: bytes) -> bytes:
    header_size = 16
    size = header_size + len(body)
    return struct.pack("<HHIII", chunk_type, header_size, size, 1, NO_NS) + body


def start_namespace(prefix_idx: int, uri_idx: int) -> bytes:
    return _node(RES_XML_START_NAMESPACE_TYPE, struct.pack("<II", prefix_idx, uri_idx))


def end_namespace(prefix_idx: int, uri_idx: int) -> bytes:
    return _node(RES_XML_END_NAMESPACE_TYPE, struct.pack("<II", prefix_idx, uri_idx))


def attribute(ns_idx: int, name_idx: int, raw_value_idx: int, data: int) -> bytes:
    return struct.pack("<IIIHBBI", ns_idx, name_idx, raw_value_idx, 8, 0, TYPE_STRING, data)


def start_element(name_idx: int, attrs: list) -> bytes:
    body = struct.pack(
        "<IIHHHHHH",
        NO_NS,
        name_idx,
        20,  # attributeStart
        20,  # attributeSize
        len(attrs),
        0, 0, 0,  # idIndex, classIndex, styleIndex
    ) + b"".join(attrs)
    return _node(RES_XML_START_ELEMENT_TYPE, body)


def end_element(name_idx: int) -> bytes:
    return _node(RES_XML_END_ELEMENT_TYPE, struct.pack("<II", NO_NS, name_idx))


def build_axml() -> bytes:
    string_pool = build_string_pool()

    package_attr = attribute(NO_NS, 3, 4, 4)  # package="com.example.moongame"
    label_attr = attribute(1, 6, 7, 7)  # android:label="Moon Game Demo"

    body = b"".join(
        [
            start_namespace(0, 1),
            start_element(2, [package_attr]),  # <manifest package=...>
            start_element(5, [label_attr]),  # <application android:label=...>
            end_element(5),  # </application>
            end_element(2),  # </manifest>
            end_namespace(0, 1),
        ]
    )

    payload = string_pool + body
    header_size = 8
    top_header = struct.pack("<HHI", RES_XML_TYPE, header_size, header_size + len(payload))
    return top_header + payload


def build() -> bytes:
    axml = build_axml()
    entry_name = b"AndroidManifest.xml"

    co = zlib.compressobj(9, zlib.DEFLATED, -15)
    compressed = co.compress(axml) + co.flush()
    crc = zlib.crc32(axml) & 0xFFFFFFFF

    # Independent sanity check before trusting this fixture: decompress the
    # bytes we're about to embed and make sure they reproduce the original
    # AXML exactly.
    do = zlib.decompressobj(-15)
    assert do.decompress(compressed) == axml, "DEFLATE round-trip mismatch while building the fixture"

    local_header = struct.pack(
        "<IHHHHHIIIHH",
        0x04034B50,
        20, 0,
        8,  # method: DEFLATE
        0, 0,
        crc,
        len(compressed),
        len(axml),
        len(entry_name),
        0,
    )
    local_entry = local_header + entry_name + compressed
    local_offset = 0

    central_header = struct.pack(
        "<IHHHHHHIIIHHHHHII",
        0x02014B50,
        20, 20,
        0, 8,
        0, 0,
        crc,
        len(compressed),
        len(axml),
        len(entry_name),
        0, 0, 0, 0, 0,
        local_offset,
    )
    central_entry = central_header + entry_name

    central_dir_offset = len(local_entry)
    eocd = struct.pack(
        "<IHHHHIIH",
        0x06054B50,
        0, 0,
        1, 1,
        len(central_entry),
        central_dir_offset,
        0,
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
    print("expected package: com.example.moongame")
    print("expected label: Moon Game Demo")


if __name__ == "__main__":
    main()
