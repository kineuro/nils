# SPDX-License-Identifier: AGPL-3.0-only
"""The native sources of the compressed pyramid fixtures: synthetic planes
whose every pixel is a formula the tests compute again, written as explicit
VR little endian Part 10 files into the folder given. make.sh then encodes
them in each transfer syntax with DCMTK and GDCM. Nothing here comes from a
person: the patient is a phantom and the UIDs are under a test root."""

import os
import struct
import sys

ROOT = "1.2.826.0.1.3680043.10.1234.52"
ROWS, COLS = 48, 64


def value(kind, x, y, z):
    """The pixel at column x, row y of plane z, as the tests compute it."""
    wave = (x * 37 + y * 101 + z * 613) % 4096
    if kind == "u12":
        return wave
    if kind == "s16":
        return wave - 1024
    if kind == "s12":
        return wave - 2048
    if kind == "u8":
        return 20 + 2 * x + 2 * y + 5 * z
    raise ValueError(kind)


FORMS = {
    # kind: bits allocated, bits stored, pixel representation
    "u12": (16, 12, 0),
    "s16": (16, 16, 1),
    "s12": (16, 12, 1),
    "u8": (8, 8, 0),
}


def elem(group, element, vr, data):
    if len(data) % 2:
        data += b"\0" if vr in (b"UI", b"OB") else b" "
    if vr in (b"OB", b"OW", b"UN", b"SQ", b"UT"):
        return struct.pack("<HH2sHI", group, element, vr, 0, len(data)) + data
    return struct.pack("<HH2sH", group, element, vr, len(data)) + data


def us(v):
    return struct.pack("<H", v)


def part10(sop, elems):
    meta = b"".join(
        [
            elem(0x0002, 0x0001, b"OB", b"\0\1"),
            elem(0x0002, 0x0002, b"UI", b"1.2.840.10008.5.1.4.1.1.4"),
            elem(0x0002, 0x0003, b"UI", sop.encode()),
            elem(0x0002, 0x0010, b"UI", b"1.2.840.10008.1.2.1"),
            elem(0x0002, 0x0012, b"UI", ROOT.encode() + b".1"),
        ]
    )
    head = b"\0" * 128 + b"DICM" + elem(0x0002, 0x0000, b"UL", struct.pack("<I", len(meta)))
    return head + meta + b"".join(elems)


def plane(out, name, series, kind, z, intercept):
    bits, stored, signed = FORMS[kind]
    sop = f"{ROOT}.{series}.{z + 1}"
    px = bytearray()
    for y in range(ROWS):
        for x in range(COLS):
            v = value(kind, x, y, z)
            px += struct.pack("<B" if bits == 8 else ("<h" if signed else "<H"), v)
    elems = [
        elem(0x0008, 0x0008, b"CS", b"ORIGINAL\\PRIMARY"),
        elem(0x0008, 0x0016, b"UI", b"1.2.840.10008.5.1.4.1.1.4"),
        elem(0x0008, 0x0018, b"UI", sop.encode()),
        elem(0x0008, 0x0060, b"CS", b"MR"),
        elem(0x0008, 0x103E, b"LO", f"phantom {kind}".encode()),
        elem(0x0010, 0x0010, b"PN", b"Phantom^Synthetic"),
        elem(0x0010, 0x0020, b"LO", b"PHANTOM"),
        elem(0x0020, 0x000D, b"UI", f"{ROOT}.{series}".encode()),
        elem(0x0020, 0x000E, b"UI", f"{ROOT}.{series}.0".encode()),
        elem(0x0020, 0x0013, b"IS", str(z + 1).encode()),
        elem(0x0020, 0x0032, b"DS", f"0\\0\\{z * 2}".encode()),
        elem(0x0020, 0x0037, b"DS", b"1\\0\\0\\0\\1\\0"),
        elem(0x0028, 0x0002, b"US", us(1)),
        elem(0x0028, 0x0004, b"CS", b"MONOCHROME2"),
        elem(0x0028, 0x0010, b"US", us(ROWS)),
        elem(0x0028, 0x0011, b"US", us(COLS)),
        elem(0x0028, 0x0030, b"DS", b"1\\1"),
        elem(0x0028, 0x0100, b"US", us(bits)),
        elem(0x0028, 0x0101, b"US", us(stored)),
        elem(0x0028, 0x0102, b"US", us(stored - 1)),
        elem(0x0028, 0x0103, b"US", us(signed)),
        elem(0x0028, 0x1052, b"DS", str(intercept).encode()),
        elem(0x0028, 0x1053, b"DS", b"1"),
        elem(0x7FE0, 0x0010, b"OB" if bits == 8 else b"OW", bytes(px)),
    ]
    with open(os.path.join(out, name), "wb") as f:
        f.write(part10(sop, elems))


def main():
    out = sys.argv[1]
    # one line per plane: file name, series, kind, plane, rescale intercept,
    # and how make.sh encodes it
    for line in open(os.path.join(os.path.dirname(__file__), "planes.txt")):
        line = line.split("#")[0].split()
        if line:
            name, series, kind, z, intercept = line[:5]
            plane(out, name + ".native", int(series), kind, int(z), int(intercept))


if __name__ == "__main__":
    main()
