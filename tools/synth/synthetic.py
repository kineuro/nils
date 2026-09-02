#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""A synthetic DICOM corpus: no library, no real data.

Written for the language spike's smoke test (spikes/lang/smoke.sh) and kept as the
seed of the engine's synthetic generator, which grows to a one-million-instance
corpus generated at CI time from a fixed seed (docs/specs/wave1-parse-and-digest.md,
§12.6; C10: nothing real goes public). Writes a handful of files into a directory:

  part10.dcm        Part 10 file, preamble + meta group, explicit VR little endian
  nopreamble.dcm    the same, without the 128-byte preamble
  raw               the dataset alone, implicit VR little endian, no meta group
  truncated.dcm     part10.dcm cut in the middle of an element
  nosop.dcm         a Part 10 file whose dataset has no SOP Instance UID
  DS_Store          64 bytes of a Finder file
  empty             zero bytes
  readme.txt        text

Values are invented; UIDs are under the example root 1.2.826.0.1.3680043.8.498
(the "any UID" root used in the DICOM standard's own examples).
"""
import struct
import sys
from pathlib import Path

EXPLICIT_LE = "1.2.840.10008.1.2.1"
MR_IMAGE = "1.2.840.10008.5.1.4.1.1.4"
ROOT = "1.2.826.0.1.3680043.8.498"


def pad(b: bytes, char: bytes = b"\x00") -> bytes:
    return b + char if len(b) % 2 else b


def explicit(group: int, elem: int, vr: bytes, value: bytes) -> bytes:
    value = pad(value, b" " if vr in (b"CS", b"LO", b"DS", b"IS", b"SH") else b"\x00")
    if vr in (b"OB", b"OW", b"SQ", b"UN", b"UT"):
        return struct.pack("<HH2sHI", group, elem, vr, 0, len(value)) + value
    return struct.pack("<HH2sH", group, elem, vr, len(value)) + value


def implicit(group: int, elem: int, value: bytes, textual: bool = True) -> bytes:
    value = pad(value, b" " if textual else b"\x00")
    return struct.pack("<HHI", group, elem, len(value)) + value


def dataset(explicit_vr: bool, with_sop: bool = True) -> bytes:
    rows = struct.pack("<H", 4)
    cols = struct.pack("<H", 4)
    pixels = bytes(range(32))
    fields = [
        (0x0008, 0x0016, b"UI", MR_IMAGE.encode()),
        (0x0008, 0x0018, b"UI", f"{ROOT}.1.2".encode()),
        (0x0008, 0x0060, b"CS", b"MR"),
        (0x0008, 0x0070, b"LO", b"Synthetic"),
        (0x0008, 0x103E, b"LO", b"t1_spike"),
        (0x0018, 0x0080, b"DS", b"2000"),
        (0x0018, 0x0081, b"DS", b"3.5"),
        (0x0020, 0x000D, b"UI", f"{ROOT}.1".encode()),
        (0x0020, 0x000E, b"UI", f"{ROOT}.1.1".encode()),
        (0x0020, 0x0011, b"IS", b"3"),
        (0x0020, 0x0013, b"IS", b"7"),
        (0x0020, 0x0032, b"DS", b"-1.5\\-2.5\\3"),
        (0x0020, 0x0037, b"DS", b"1\\0\\0\\0\\1\\0"),
        (0x0028, 0x0010, b"US", rows),
        (0x0028, 0x0011, b"US", cols),
        (0x0028, 0x0030, b"DS", b"0.5\\0.5"),
        (0x7FE0, 0x0010, b"OW", pixels),
    ]
    if not with_sop:
        fields = [f for f in fields if f[:2] != (0x0008, 0x0018)]
    out = b""
    for g, e, vr, v in fields:
        if explicit_vr:
            out += explicit(g, e, vr, v)
        else:
            out += implicit(g, e, v, textual=vr not in (b"US", b"OW"))
    return out


def meta_group() -> bytes:
    body = b"".join(
        [
            explicit(0x0002, 0x0001, b"OB", b"\x00\x01"),
            explicit(0x0002, 0x0002, b"UI", MR_IMAGE.encode()),
            explicit(0x0002, 0x0003, b"UI", f"{ROOT}.1.2".encode()),
            explicit(0x0002, 0x0010, b"UI", EXPLICIT_LE.encode()),
            explicit(0x0002, 0x0012, b"UI", f"{ROOT}.9".encode()),
        ]
    )
    return explicit(0x0002, 0x0000, b"UL", struct.pack("<I", len(body))) + body


def main(out: Path) -> None:
    out.mkdir(parents=True, exist_ok=True)
    part10 = b"\x00" * 128 + b"DICM" + meta_group() + dataset(True)
    (out / "part10.dcm").write_bytes(part10)
    (out / "nopreamble.dcm").write_bytes(b"DICM" + meta_group() + dataset(True))
    (out / "raw").write_bytes(dataset(False))
    (out / "truncated.dcm").write_bytes(part10[: 128 + 4 + len(meta_group()) + 40])
    (out / "nosop.dcm").write_bytes(b"\x00" * 128 + b"DICM" + meta_group() + dataset(True, with_sop=False))
    (out / "DS_Store").write_bytes(b"\x00\x00\x00\x01Bud1" + b"\x00" * 56)
    (out / "empty").write_bytes(b"")
    (out / "readme.txt").write_bytes(b"not a DICOM file\n")
    sub = out / "sub" / "deeper"
    sub.mkdir(parents=True, exist_ok=True)
    (sub / "part10-copy.dcm").write_bytes(part10)
    print(f"synthetic corpus: {out} (9 files)")


if __name__ == "__main__":
    main(Path(sys.argv[1] if len(sys.argv) > 1 else "synthetic"))
