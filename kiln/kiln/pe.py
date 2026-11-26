"""Just enough of the Windows PE format to read a program's version and an
SKSE plugin's compatibility data, the same data SKSE checks before loading it."""
from __future__ import annotations

import struct
from dataclasses import dataclass, field
from pathlib import Path


class PEError(ValueError):
    pass


def _headers(b: bytes):
    if b[:2] != b"MZ":
        raise PEError("not a Windows program")
    pe = struct.unpack_from("<I", b, 0x3C)[0]
    if b[pe:pe + 4] != b"PE\0\0":
        raise PEError("not a Windows program")
    machine, nsec = struct.unpack_from("<HH", b, pe + 4)
    timestamp = struct.unpack_from("<I", b, pe + 8)[0]
    optsz = struct.unpack_from("<H", b, pe + 20)[0]
    opt = pe + 24
    magic = struct.unpack_from("<H", b, opt)[0]
    dd = opt + (112 if magic == 0x20B else 96)
    secs = [struct.unpack_from("<8sIIII", b, opt + optsz + i * 40) for i in range(nsec)]

    def off(rva: int) -> int:
        for _n, vsize, va, rsize, raw in secs:
            if va <= rva < va + max(vsize, rsize):
                return rva - va + raw
        raise PEError("bad address")
    return {"machine": machine, "timestamp": timestamp, "is64": magic == 0x20B, "dd": dd, "off": off}


def exports(b: bytes, h=None) -> dict[str, int]:
    h = h or _headers(b)
    erva, _size = struct.unpack_from("<II", b, h["dd"])
    if not erva:
        return {}
    off = h["off"]
    e = off(erva)
    n_names = struct.unpack_from("<I", b, e + 24)[0]
    afun, anames, aord = struct.unpack_from("<III", b, e + 28)
    out = {}
    for i in range(min(n_names, 4096)):
        p = off(struct.unpack_from("<I", b, off(anames) + 4 * i)[0])
        name = b[p:p + 256].split(b"\0", 1)[0].decode("ascii", "replace")
        o = struct.unpack_from("<H", b, off(aord) + 2 * i)[0]
        out[name] = struct.unpack_from("<I", b, off(afun) + 4 * o)[0]
    return out


_VERSIONS: dict = {}


def file_version(path: Path) -> tuple[int, int, int, int] | None:
    """The version in a program's version resource, e.g. (1, 6, 1170, 0)."""
    try:
        st = path.stat()
    except OSError:
        return None
    key = (str(path), st.st_size, st.st_mtime_ns)
    if key not in _VERSIONS:
        _VERSIONS[key] = _file_version(path)
    return _VERSIONS[key]


def _file_version(path: Path):
    try:
        b = path.read_bytes()
    except OSError:
        return None
    i = b.find(struct.pack("<I", 0xFEEF04BD))
    if i < 0:
        return None
    ms, ls = struct.unpack_from("<II", b, i + 8)
    return ms >> 16, ms & 0xFFFF, ls >> 16, ls & 0xFFFF


def xse_version(path: Path) -> tuple[int, int, int] | None:
    """Script extender version from its DLL/loader (SKSE stores 2.3.1 as 0.2.3.1)."""
    v = file_version(path)
    if not v:
        return None
    return tuple(v[1:4]) if v[0] == 0 else tuple(v[:3])


# ------------------------------------------------------------------ SKSE plugins
ADDRLIB, SIGNATURES, STRUCTS_629 = 1, 2, 4            # versionIndependence
NO_STRUCT_USE, ADDRLIB_V5 = 1, 2                       # versionIndependenceEx
ADDRLIB_V5_CUTOFF = 1748217600                         # 2025-05-26: older builds predate the new format


def pack_version(v: tuple[int, ...]) -> int:
    major, minor, build = (tuple(v) + (0, 0, 0))[:3]
    return ((major & 0xFF) << 24) | ((minor & 0xFF) << 16) | ((build & 0xFFF) << 4)


def unpack_version(p: int) -> str:
    return f"{p >> 24}.{(p >> 16) & 0xFF}.{(p >> 4) & 0xFFF}"


@dataclass
class XsePlugin:
    path: Path
    kind: str = "plugin"         # "plugin", "se-only" (no version data), "not-plugin", "32bit", "unreadable"
    name: str = ""
    author: str = ""
    version: int = 0
    independence: int = 0
    independence_ex: int = 0
    compatible: list[int] = field(default_factory=list)
    xse_required: int = 0
    build_time: int = 0

    def verdict(self, runtime: tuple[int, ...], xse_version: tuple[int, ...] | None,
                addrlib_present: bool) -> tuple[str, str]:
        """('ok' | 'reason code', detail) using SKSE's own rules."""
        rt = pack_version(runtime)
        if self.kind == "32bit":
            return "le", "made for the old 32-bit Skyrim (Legendary Edition)"
        if self.kind == "se-only":
            if rt >= pack_version((1, 6, 0)):
                return "old_game", "made for Skyrim 1.5.97 (before the Anniversary update) and has no " \
                                   "version info, so SKSE refuses it"
            return "ok", ""
        if self.kind != "plugin":
            return "ok", ""
        if self.independence & ~(ADDRLIB | SIGNATURES | STRUCTS_629):
            return "bad_data", "has version data SKSE doesn't understand"
        independent = bool(self.independence & (ADDRLIB | SIGNATURES))
        why = ""
        if self.independence & ADDRLIB:
            if not addrlib_present:
                return "addrlib", "needs Address Library for your game version"
            if not (self.independence_ex & ADDRLIB_V5) and 520128000 <= self.build_time < ADDRLIB_V5_CUTOFF \
                    and rt >= pack_version((1, 7, 0)):          # SKSE for 1.7.x+ checks this
                why, independent = "built before the 2025 Address Library change", False
        if independent and rt >= pack_version((1, 6, 629)) and not (self.independence & STRUCTS_629) \
                and not (self.independence_ex & NO_STRUCT_USE):
            return "old_game", "only works with Skyrim versions before 1.6.629"
        if not independent and rt not in self.compatible:
            made = ", ".join(unpack_version(c) for c in self.compatible[:3] if c)
            return "old_game", (why + "; " if why else "") + (
                f"made for Skyrim {made}" if made else "not marked as working with any game version")
        if xse_version and self.xse_required > pack_version(xse_version):
            return "old_xse", f"needs SKSE {unpack_version(self.xse_required)} or newer"
        return "ok", ""


def xse_plugin(path: Path) -> XsePlugin:
    p = XsePlugin(path)
    try:
        b = path.read_bytes()
        h = _headers(b)
    except (OSError, PEError, struct.error):
        p.kind = "unreadable"
        return p
    p.build_time = h["timestamp"]
    if not h["is64"]:
        p.kind = "32bit"
        return p
    try:
        ex = exports(b, h)
    except (PEError, struct.error):
        p.kind = "unreadable"
        return p
    if "SKSEPlugin_Version" in ex:
        try:
            o = h["off"](ex["SKSEPlugin_Version"])
            data_version, p.version = struct.unpack_from("<II", b, o)
            p.name = b[o + 8:o + 264].split(b"\0", 1)[0].decode("utf-8", "replace")
            p.author = b[o + 264:o + 520].split(b"\0", 1)[0].decode("utf-8", "replace")
            base = o + 8 + 256 + 256 + 252
            p.independence_ex, p.independence = struct.unpack_from("<II", b, base)
            comp = struct.unpack_from("<16I", b, base + 8)
            p.compatible = []
            for c in comp:
                if not c:
                    break
                p.compatible.append(c)
            p.xse_required = struct.unpack_from("<I", b, base + 8 + 64)[0]
        except (PEError, struct.error):
            p.kind = "unreadable"
        return p
    p.kind = "se-only" if "SKSEPlugin_Query" in ex else "not-plugin"
    return p
