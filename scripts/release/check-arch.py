"""Fail unless an executable's machine architecture is the expected one.

Usage: python3 check-arch.py <file> <x86_64|arm64>

Reads ELF, PE (Windows) and Mach-O headers directly, so it behaves the same on
every runner without `file`, `lipo` or `dumpbin`. Universal (fat) Mach-O
binaries are rejected: release assets are built per architecture.
"""

import struct
import sys

ELF = {0x3E: "x86_64", 0xB7: "arm64"}
PE = {0x8664: "x86_64", 0xAA64: "arm64"}
MACHO = {0x01000007: "x86_64", 0x0100000C: "arm64"}


def architecture(data: bytes) -> str:
    if data[:4] == b"\x7fELF":
        order = "<" if data[5] == 1 else ">"
        return ELF.get(struct.unpack(order + "H", data[18:20])[0], "unknown ELF")
    if data[:2] == b"MZ":
        offset = struct.unpack("<I", data[0x3C:0x40])[0]
        if data[offset : offset + 4] != b"PE\0\0":
            return "invalid PE"
        return PE.get(struct.unpack("<H", data[offset + 4 : offset + 6])[0], "unknown PE")
    magic = data[:4]
    if magic in (b"\xcf\xfa\xed\xfe", b"\xce\xfa\xed\xfe"):
        return MACHO.get(struct.unpack("<I", data[4:8])[0], "unknown Mach-O")
    if magic in (b"\xca\xfe\xba\xbe", b"\xca\xfe\xba\xbf"):
        return "universal Mach-O"
    return "unrecognized"


def main() -> int:
    path, expected = sys.argv[1], sys.argv[2]
    with open(path, "rb") as handle:
        found = architecture(handle.read(4096))
    print(f"{path}: {found}")
    if found != expected:
        print(f"::error::{path} is {found}, expected {expected}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
