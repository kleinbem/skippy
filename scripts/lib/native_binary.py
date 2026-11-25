"""Read native import tables without loading the inspected binary."""

import mmap
import struct
from typing import NamedTuple

class Image(NamedTuple):
    kind: str
    arch: str
    imports: list
    soname: str = None


def elf(data):
    if len(data) < 64 or data[4] != 2 or data[5] not in (1, 2):
        raise ValueError("invalid or unsupported ELF header")
    byte_order = "<" if data[5] == 1 else ">"

    def read(fmt, offset):
        return struct.unpack_from(byte_order + fmt, data, offset)

    machine, = read("H", 18)
    arch = {62: "x86_64", 183: "aarch64"}.get(machine)
    if not arch:
        raise ValueError("unsupported ELF architecture")
    program_offset, sections = read("QQ", 32)
    entry_size, count = read("HH", 54)
    if count == 0xFFFF:
        count, = read("I", sections + 44)
    if count and (entry_size < 56 or program_offset + entry_size * count > len(data)):
        raise ValueError("invalid ELF program header table")
    loads, dynamic = [], None
    for index in range(count):
        kind, _, offset, address, _, length = read("IIQQQQ", program_offset + index * entry_size)
        if kind in (1, 2) and offset + length > len(data):
            raise ValueError("ELF segment exceeds file")
        if kind == 1:
            loads.append((address, offset, length))
        elif kind == 2:
            if dynamic is not None:
                raise ValueError("duplicate ELF dynamic segments")
            dynamic = (offset, length)
    if dynamic is None:
        return Image("elf", arch, [])
    offset, length = dynamic
    needed, sonames = [], []
    string_table = string_size = None
    for cursor in range(offset, offset + length - 15, 16):
        tag, value = read("qQ", cursor)
        if tag == 0:
            break
        if tag == 1:
            needed.append(value)
        elif tag == 5:
            string_table = value
        elif tag == 10:
            string_size = value
        elif tag == 14:
            sonames.append(value)
    else:
        raise ValueError("unterminated ELF dynamic table")
    if not needed and not sonames:
        return Image("elf", arch, [])
    if string_table is None or not string_size:
        raise ValueError("missing ELF dynamic string table")
    for address, offset, length in loads:
        if address <= string_table and string_table + string_size <= address + length:
            start = offset + string_table - address
            end = start + string_size
            imports = sorted({text(data, start + index, end) for index in needed})
            soname = text(data, start + sonames[0], end) if sonames else None
            return Image("elf", arch, imports, soname)
    raise ValueError("ELF dynamic string table is outside the file")


def text(data, start, end):
    if not 0 <= start < end <= len(data):
        raise ValueError("invalid binary string offset")
    zero = data.find(b"\0", start, end)
    if zero < 0:
        raise ValueError("unterminated binary string")
    return data[start:zero].decode("utf-8")


def macho(data, magic):
    prefix = "<" if magic == b"\xcf\xfa\xed\xfe" else ">"
    machine, _, _, commands, size = struct.unpack_from(prefix + "5I", data, 4)
    arch = {0x01000007: "x86_64", 0x0100000C: "aarch64"}.get(machine)
    if not arch or 32 + size > len(data):
        raise ValueError("unsupported Mach-O header")
    offset = 32
    imports = set()
    soname = None
    for _ in range(commands):
        cmd, length = struct.unpack_from(prefix + "II", data, offset)
        end = offset + length
        if length < 8 or end > 32 + size:
            raise ValueError("invalid Mach-O load command")
        if cmd in {0xC, 0xD, 0x20, 0x80000018, 0x8000001F, 0x80000023}:
            name_offset, = struct.unpack_from(prefix + "I", data, offset + 8)
            if not 24 <= name_offset < length:
                raise ValueError("invalid Mach-O dylib name offset")
            name = text(data, offset + name_offset, end)
            if cmd == 0xD:
                soname = name
            else:
                imports.add(name)
        offset = end
    if offset != 32 + size:
        raise ValueError("incomplete Mach-O load command table")
    return Image("macho", arch, sorted(imports), soname)


def pe(data):
    def read(fmt, offset):
        return struct.unpack_from("<" + fmt, data, offset)

    start, = read("I", 60)
    if data[start:start + 4] != b"PE\0\0":
        raise ValueError("invalid PE signature")
    machine, count = read("HH", start + 4)
    arch = {0x8664: "x86_64", 0xAA64: "aarch64"}.get(machine)
    opt_size, = read("H", start + 20)
    optional = start + 24
    magic, = read("H", optional)
    if not arch or magic != 0x20B or opt_size < 112:
        raise ValueError("unsupported PE header")
    sections = []
    for index in range(count):
        offset = optional + opt_size + index * 40
        virtual_size, rva, file_size, file_offset = read("4I", offset + 8)
        if file_size > 0 and file_offset + file_size > len(data):
            raise ValueError("invalid PE section")
        sections.append((rva, min(virtual_size or file_size, file_size), file_offset))

    def locate(rva):
        for base, size, offset in sections:
            if base <= rva < base + size:
                return offset + rva - base, offset + size
        raise ValueError("PE import points outside a file section")

    directories, = read("I", optional + 108)
    if directories < 2:
        return Image("pe", arch, [])
    if opt_size < 128:
        raise ValueError("truncated PE import directory")
    rva, size = read("II", optional + 120)
    if not rva:
        return Image("pe", arch, [])
    offset, limit = locate(rva)
    if size:
        limit = min(limit, offset + size)
    imports = set()
    while offset + 20 <= limit:
        descriptor = read("5I", offset)
        if not any(descriptor):
            return Image("pe", arch, sorted(imports))
        name, end = locate(descriptor[3])
        imports.add(text(data, name, end))
        offset += 20
    raise ValueError("unterminated PE import table")


def inspect(path):
    with path.open("rb") as stream:
        header = stream.read(32)
        magic = header[:4]
        with mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as data:
            try:
                if magic == b"\x7fELF":
                    return elf(data)
                if magic[:2] == b"MZ":
                    return pe(data)
                if magic in {b"\xcf\xfa\xed\xfe", b"\xfe\xed\xfa\xcf"}:
                    return macho(data, magic)
            except struct.error as error:
                raise ValueError(f"truncated native binary: {path}") from error
    raise ValueError(f"unsupported native binary: {path}")
