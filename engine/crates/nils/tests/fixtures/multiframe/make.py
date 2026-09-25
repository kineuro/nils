# SPDX-License-Identifier: AGPL-3.0-only
"""The native sources of the multi-frame pyramid fixtures: synthetic
enhanced and classic multi-frame objects whose every pixel is a formula the
tests compute again, written as explicit VR little endian Part 10 files into
the folder given. make.sh then encodes them as files.txt says with DCMTK and
GDCM. Nothing here comes from a person: the patient is a phantom and the
UIDs are under a test root."""

import os
import struct
import sys

ROOT = "1.2.826.0.1.3680043.10.1234.53"
ROWS, COLS = 48, 64
ENHANCED_MR = "1.2.840.10008.5.1.4.1.1.4.1"
MR = "1.2.840.10008.5.1.4.1.1.4"
AXIAL = "1\\0\\0\\0\\1\\0"
SAGITTAL = "0\\1\\0\\0\\0\\-1"


def value(x, y, plane):
    """The pixel at column x, row y of plane `plane`, as the tests compute
    it: twelve bits unsigned, the compressed fixtures' formula."""
    return (x * 37 + y * 101 + plane * 613) % 4096


def elem(group, element, vr, data):
    if len(data) % 2:
        data += b"\0" if vr in (b"UI", b"OB") else b" "
    if vr in (b"OB", b"OW", b"UN", b"SQ", b"UT"):
        return struct.pack("<HH2sHI", group, element, vr, 0, len(data)) + data
    return struct.pack("<HH2sH", group, element, vr, len(data)) + data


def text(group, element, vr, s):
    return elem(group, element, vr, s.encode())


def us(group, element, v):
    return elem(group, element, b"US", struct.pack("<H", v))


def seq(group, element, items):
    """A sequence of defined length, each item a list of elements."""
    body = b""
    for item in items:
        data = b"".join(sorted(item, key=lambda e: struct.unpack("<HH", e[:4])))
        body += struct.pack("<HHI", 0xFFFE, 0xE000, len(data)) + data
    return elem(group, element, b"SQ", body)


def ds(values):
    return "\\".join(f"{v:g}" for v in values)


def position(p):
    return seq(0x0020, 0x9113, [[text(0x0020, 0x0032, b"DS", ds(p))]])


def orientation(iop):
    return seq(0x0020, 0x9116, [[text(0x0020, 0x0037, b"DS", iop)]])


def measures(spacing, thickness):
    return seq(
        0x0028,
        0x9110,
        [
            [
                text(0x0018, 0x0050, b"DS", ds([thickness])),
                text(0x0018, 0x0088, b"DS", ds([thickness])),
                text(0x0028, 0x0030, b"DS", ds([spacing, spacing])),
            ]
        ],
    )


def transform(intercept):
    return seq(
        0x0028,
        0x9145,
        [
            [
                text(0x0028, 0x1052, b"DS", ds([intercept])),
                text(0x0028, 0x1053, b"DS", "1"),
                text(0x0028, 0x1054, b"LO", "US"),
            ]
        ],
    )


def part10(sop_class, sop, elems):
    meta = b"".join(
        [
            elem(0x0002, 0x0001, b"OB", b"\0\1"),
            text(0x0002, 0x0002, b"UI", sop_class),
            text(0x0002, 0x0003, b"UI", sop),
            text(0x0002, 0x0010, b"UI", "1.2.840.10008.1.2.1"),
            text(0x0002, 0x0012, b"UI", ROOT + ".1"),
        ]
    )
    head = b"\0" * 128 + b"DICM" + elem(0x0002, 0x0000, b"UL", struct.pack("<I", len(meta)))
    return head + meta + b"".join(elems)


def pixels(planes):
    px = bytearray()
    for p in planes:
        for y in range(ROWS):
            for x in range(COLS):
                px += struct.pack("<H", value(x, y, p))
    return bytes(px)


def write(out, name, series, instance, sop_class, planes, top, shared, per_frame):
    """One object of len(planes) frames, frame k holding plane planes[k];
    `top` are its own elements beside the common ones, `shared` the shared
    functional groups and `per_frame` one list of groups per frame, both
    none for a classic object. Elements are sorted by tag before writing."""
    sop = f"{ROOT}.{series}.{instance}"
    elems = [
        text(0x0008, 0x0008, b"CS", "ORIGINAL\\PRIMARY"),
        text(0x0008, 0x0016, b"UI", sop_class),
        text(0x0008, 0x0018, b"UI", sop),
        text(0x0008, 0x0060, b"CS", "MR"),
        text(0x0008, 0x103E, b"LO", f"phantom multiframe {series}"),
        text(0x0010, 0x0010, b"PN", "Phantom^Synthetic"),
        text(0x0010, 0x0020, b"LO", "PHANTOM"),
        text(0x0020, 0x000D, b"UI", f"{ROOT}.{series}"),
        text(0x0020, 0x000E, b"UI", f"{ROOT}.{series}.0"),
        text(0x0020, 0x0013, b"IS", str(instance)),
        us(0x0028, 0x0002, 1),
        text(0x0028, 0x0004, b"CS", "MONOCHROME2"),
        us(0x0028, 0x0010, ROWS),
        us(0x0028, 0x0011, COLS),
        us(0x0028, 0x0100, 16),
        us(0x0028, 0x0101, 12),
        us(0x0028, 0x0102, 11),
        us(0x0028, 0x0103, 0),
        elem(0x7FE0, 0x0010, b"OW", pixels(planes)),
    ]
    if len(planes) > 1 or per_frame is not None:
        elems.append(text(0x0028, 0x0008, b"IS", str(len(planes))))
    if shared is not None:
        elems.append(seq(0x5200, 0x9229, [shared]))
    if per_frame is not None:
        elems.append(seq(0x5200, 0x9230, per_frame))
    elems += top
    elems.sort(key=lambda e: struct.unpack("<HH", e[:4]))
    with open(os.path.join(out, name), "wb") as f:
        f.write(part10(sop_class, sop, elems))


def enhanced(out, name, series, instance, planes, z_of, iop=AXIAL, per_frame_rescale=False):
    """An enhanced MR object: frame k holds plane planes[k] at z_of(planes[k])
    along the axial normal (or x for a sagittal frame)."""
    frames = []
    for k, p in enumerate(planes):
        o = iop[k] if isinstance(iop, list) else iop
        where = [0, 0, z_of(p)] if o == AXIAL else [z_of(p), 0, 0]
        groups = [position(where)]
        if isinstance(iop, list):
            groups.append(orientation(o))
        if per_frame_rescale:
            groups.append(transform(-1024))
        frames.append(groups)
    shared = [measures(0.8, 2)]
    if not isinstance(iop, list):
        shared.append(orientation(iop))
    if not per_frame_rescale:
        shared.append(transform(-1024))
    write(out, name, series, instance, ENHANCED_MR, planes, [], shared, frames)


def main():
    out = sys.argv[1]
    two = lambda p: 2.0 * p
    for line in open(os.path.join(os.path.dirname(__file__), "files.txt")):
        line = line.split("#")[0].split()
        if not line:
            continue
        name, series, layout = line[0], int(line[1]), line[2]
        native = name + ".native"
        if layout == "ordered":
            # five frames in order along the normal, groups shared
            enhanced(out, native, series, 1, [0, 1, 2, 3, 4], two)
        elif layout == "shuffled":
            # five frames whose positions run out of order, the rescale per frame
            enhanced(out, native, series, 1, [3, 0, 4, 1, 2], two, per_frame_rescale=True)
        elif layout == "classic":
            # four frames of a classic MR object and no functional groups, one
            # position for the file, as a cine is written (a secondary capture
            # is refused by the digest before it is a stack)
            top = [
                text(0x0018, 0x0050, b"DS", "2"),
                text(0x0018, 0x0088, b"DS", "3"),
                text(0x0020, 0x0032, b"DS", "0\\0\\0"),
                text(0x0020, 0x0037, b"DS", AXIAL),
                text(0x0028, 0x0030, b"DS", "0.8\\0.8"),
                text(0x0028, 0x1052, b"DS", "-1024"),
                text(0x0028, 0x1053, b"DS", "1"),
            ]
            write(out, native, series, 1, MR, [0, 1, 2, 3], top, None, None)
        elif layout == "first":
            enhanced(out, native, series, 1, [0, 1, 2], two)
        elif layout == "second":
            enhanced(out, native, series, 2, [3, 4, 5], two)
        elif layout == "single":
            # one classic single-frame plane after them, plane 6
            top = [
                text(0x0018, 0x0050, b"DS", "2"),
                text(0x0020, 0x0032, b"DS", "0\\0\\12"),
                text(0x0020, 0x0037, b"DS", AXIAL),
                text(0x0028, 0x0030, b"DS", "0.8\\0.8"),
                text(0x0028, 0x1052, b"DS", "-1024"),
                text(0x0028, 0x1053, b"DS", "1"),
            ]
            write(out, native, series, 3, MR, [6], top, None, None)
        elif layout == "split":
            # frames 1-3 axial, planes 0-2; frames 4-6 sagittal, planes 10-12
            enhanced(
                out, native, series, 1, [0, 1, 2, 10, 11, 12], lambda p: 2.0 * (p % 10),
                iop=[AXIAL] * 3 + [SAGITTAL] * 3,
            )
        else:
            raise ValueError(layout)


if __name__ == "__main__":
    main()
