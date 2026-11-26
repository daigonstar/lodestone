"""Fake Windows binaries for the tests: version resources and SKSE plugins."""
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from kiln import pe  # noqa: E402


def version_blob(*v):
    ms, ls = (v[0] << 16) | v[1], (v[2] << 16) | v[3]
    return b"MZ" + b"\0" * 62 + struct.pack("<IIII", 0xFEEF04BD, 0x10000, ms, ls)


def skse_plugin(name, indep=0, indep_ex=0, compat=(), query_only=False, built=1760000000):
    """A minimal 64-bit DLL exporting SKSEPlugin_Version (or only SKSEPlugin_Query)."""
    base_rva, raw = 0x1000, 0x200
    sym = b"SKSEPlugin_Query" if query_only else b"SKSEPlugin_Version"
    data = bytearray(0x600)
    # export directory at 0, arrays after it, then the name, then the version struct
    funcs, names, ords, name_off, ver_off = 0x40, 0x48, 0x50, 0x60, 0x100
    struct.pack_into("<IIHHIIIIIII", data, 0, 0, 0, 0, 0, base_rva + name_off, 1, 1, 1,
                     base_rva + funcs, base_rva + names, base_rva + ords)
    struct.pack_into("<I", data, funcs, base_rva + ver_off)
    struct.pack_into("<I", data, names, base_rva + name_off)
    struct.pack_into("<H", data, ords, 0)
    data[name_off:name_off + len(sym)] = sym
    if not query_only:
        struct.pack_into("<II", data, ver_off, 1, 1)
        data[ver_off + 8:ver_off + 8 + len(name)] = name.encode()
        b = ver_off + 8 + 256 + 256 + 252
        struct.pack_into("<II", data, b, indep_ex, indep)
        for i, c in enumerate(compat):
            struct.pack_into("<I", data, b + 8 + 4 * i, pe.pack_version(c))
    hdr = bytearray(raw)
    hdr[:2] = b"MZ"; struct.pack_into("<I", hdr, 0x3C, 0x40)
    hdr[0x40:0x44] = b"PE\0\0"
    struct.pack_into("<HHIIIHH", hdr, 0x44, 0x8664, 1, built, 0, 0, 240, 0x2022)
    opt = 0x58
    struct.pack_into("<H", hdr, opt, 0x20B)
    struct.pack_into("<II", hdr, opt + 112, base_rva, 40)          # export directory
    sec = opt + 240
    struct.pack_into("<8sIIIIIIHHI", hdr, sec, b".rdata", len(data), base_rva, len(data), raw, 0, 0, 0, 0, 0x40000040)
    return bytes(hdr) + bytes(data)
