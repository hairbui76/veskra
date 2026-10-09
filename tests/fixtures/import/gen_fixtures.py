#!/usr/bin/env python3
"""Generate color-managed PNG import fixtures and LittleCMS expectations (COL-004/COL-005).

Run from anywhere:  python3 tests/fixtures/import/gen_fixtures.py
Requires the pinned oracle tools in scripts/requirements-oracles.txt (Pillow 12.3.0 / LittleCMS 2.19).

Outputs (next to this script):
  * *.png fixtures (small, < 200 KB total)
  * gray-gamma18-v4.icc (Gray TRC profile written by this script from first principles)
  * expected.json: decoded source values (Pillow) and LittleCMS transform outputs used by
    crates/veskra-io/tests/import_png.rs. Tool versions are recorded inside the file.

Everything is generated from numbers chosen here; nothing is copied from third-party assets.
The Display P3 / sRGB profiles are Veskra's own generated profiles in tests/fixtures/color/icc/.
"""

from __future__ import annotations

import io
import json
import platform
import struct
import sys
import zlib
from pathlib import Path

from PIL import Image, ImageCms, PngImagePlugin

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
COLOR_ICC = ROOT / "tests" / "fixtures" / "color" / "icc"

P3_ICC = (COLOR_ICC / "veskra-display-p3-v4.icc").read_bytes()
SRGB_ICC = (COLOR_ICC / "veskra-srgb-v4.icc").read_bytes()
LCMS_SRGB_ICC = (COLOR_ICC / "lcms2-builtin-srgb.icc").read_bytes()


# ------------------------------------------------------------------------------------------------
# Deterministic pixel content


def swatches_rgb(n: int = 16) -> list[tuple[int, int, int]]:
    """n*n swatches: primaries/secondaries at several levels, grays and a pseudo-random spread."""
    out: list[tuple[int, int, int]] = []
    levels = [0, 1, 17, 64, 128, 191, 238, 254, 255]
    for v in levels:
        out += [(v, 0, 0), (0, v, 0), (0, 0, v), (v, v, 0), (0, v, v), (v, 0, v), (v, v, v)]
    seed = 0x2545F491
    while len(out) < n * n:
        seed = (seed * 1103515245 + 12345) & 0x7FFFFFFF
        out.append(((seed >> 3) & 255, (seed >> 11) & 255, (seed >> 19) & 255))
    return out[: n * n]


def rgb_image(px: list[tuple[int, int, int]], w: int, h: int) -> Image.Image:
    im = Image.new("RGB", (w, h))
    im.putdata(px)
    return im


def rgba_image(px: list[tuple[int, int, int, int]], w: int, h: int) -> Image.Image:
    im = Image.new("RGBA", (w, h))
    im.putdata(px)
    return im


# ------------------------------------------------------------------------------------------------
# Gray ICC (v4, kTRC curv gamma 1.8, D50 white) written from the ICC.1:2010 structure.


def s15f16(v: float) -> bytes:
    return struct.pack(">i", round(v * 65536))


def mluc(text: str) -> bytes:
    s = text.encode("utf-16-be")
    b = b"mluc" + b"\0" * 4 + struct.pack(">II", 1, 12) + b"enUS" + struct.pack(">II", len(s), 28) + s
    return b + b"\0" * (-len(b) % 4)


def gray_icc(gamma: float, description: str) -> bytes:
    d50 = (0.9642, 1.0, 0.8249)
    tags = [
        (b"desc", mluc(description)),
        (b"cprt", mluc("No copyright, use freely")),
        (b"wtpt", b"XYZ " + b"\0" * 4 + b"".join(s15f16(v) for v in d50)),
        (b"kTRC", b"curv" + b"\0" * 4 + struct.pack(">I", 1) + struct.pack(">H", round(gamma * 256)) + b"\0\0"),
    ]
    table_len = 4 + 12 * len(tags)
    offset = 128 + table_len
    table = struct.pack(">I", len(tags))
    data = b""
    for sig, body in tags:
        table += sig + struct.pack(">II", offset + len(data), len(body))
        data += body + b"\0" * (-len(body) % 4)
    size = 128 + table_len + len(data)
    hdr = bytearray(128)
    struct.pack_into(">I", hdr, 0, size)
    hdr[8:12] = bytes([4, 0x30, 0, 0])
    hdr[12:16] = b"mntr"
    hdr[16:20] = b"GRAY"
    hdr[20:24] = b"XYZ "
    struct.pack_into(">6H", hdr, 24, 2026, 1, 1, 0, 0, 0)
    hdr[36:40] = b"acsp"
    hdr[68:80] = b"".join(s15f16(v) for v in d50)
    return bytes(hdr) + table + data


# ------------------------------------------------------------------------------------------------
# Minimal Adam7 PNG writer (Pillow cannot write interlaced PNG). Filter type 0 on every row.

ADAM7 = [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)]


def chunk(ty: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + ty + data + struct.pack(">I", zlib.crc32(ty + data) & 0xFFFFFFFF)


def write_adam7_rgba8(px: list[tuple[int, int, int, int]], w: int, h: int, extra: list[bytes]) -> bytes:
    raw = bytearray()
    for x0, y0, dx, dy in ADAM7:
        cols = range(x0, w, dx)
        if not cols:
            continue
        for y in range(y0, h, dy):
            raw.append(0)
            for x in cols:
                raw += bytes(px[y * w + x])
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 1)
    out = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
    for c in extra:
        out += c
    out += chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b"")
    return out


# ------------------------------------------------------------------------------------------------


def lcms_rgb(src_icc: bytes, dst_icc: bytes, im: Image.Image, mode_in: str, flags) -> list[int]:
    src = ImageCms.ImageCmsProfile(io.BytesIO(src_icc))
    dst = ImageCms.ImageCmsProfile(io.BytesIO(dst_icc))
    t = ImageCms.buildTransform(src, dst, mode_in, "RGB", renderingIntent=ImageCms.Intent.RELATIVE_COLORIMETRIC,
                                flags=flags)
    out = ImageCms.applyTransform(im, t)
    return list(out.tobytes())


def flat(im: Image.Image) -> list[int]:
    return list(im.convert("RGBA").tobytes())


def main() -> int:
    expected: dict = {
        "generator": "tests/fixtures/import/gen_fixtures.py",
        "tools": {
            "python": platform.python_version(),
            "pillow": Image.__version__,
            "littlecms": ImageCms.core.littlecms_version,
        },
        "intent": "relative-colorimetric",
        "fixtures": {},
    }
    fx = expected["fixtures"]
    W = H = 16
    sw = swatches_rgb(W)
    F = ImageCms.Flags

    def save(name: str, im: Image.Image, **kw) -> bytes:
        buf = io.BytesIO()
        im.save(buf, "PNG", **kw)
        data = buf.getvalue()
        (HERE / name).write_bytes(data)
        return data

    # 1. Display P3 tagged RGB8: Convert must equal LittleCMS P3 -> sRGB; Assign keeps the numbers.
    p3 = rgb_image(sw, W, H)
    save("p3_icc_rgb8.png", p3, icc_profile=P3_ICC)
    fx["p3_icc_rgb8.png"] = {
        "width": W, "height": H, "source_rgba8": flat(p3),
        "lcms_to_veskra_srgb_rgb8_nooptimize": lcms_rgb(P3_ICC, SRGB_ICC, p3, "RGB", F.NOOPTIMIZE),
        "lcms_to_veskra_srgb_rgb8_default": lcms_rgb(P3_ICC, SRGB_ICC, p3, "RGB", F.NONE),
    }

    # 2. sRGB chunk, RGBA8 with alpha ramp incl. alpha-0 pixels carrying non-zero color.
    rgba = [(r, g, b, (i * 37) % 256 if i % 5 else 0) for i, (r, g, b) in enumerate(sw)]
    im = rgba_image(rgba, W, H)
    info = PngImagePlugin.PngInfo()
    info.add(b"sRGB", b"\x00")
    save("srgb_chunk_rgba8.png", im, pnginfo=info)
    fx["srgb_chunk_rgba8.png"] = {"width": W, "height": H, "source_rgba8": flat(im)}

    # 3. Independent sRGB ICC (LittleCMS built-in) on RGB8: must be recognised as sRGB (identity).
    im = rgb_image(sw, W, H)
    save("lcms_srgb_icc_rgb8.png", im, icc_profile=LCMS_SRGB_ICC)
    fx["lcms_srgb_icc_rgb8.png"] = {"width": W, "height": H, "source_rgba8": flat(im)}

    # 4. Untagged RGB8 (no iCCP/sRGB/gAMA/cHRM): sRGB assumed, profile_assumed = true.
    save("untagged_rgb8.png", im)
    fx["untagged_rgb8.png"] = {"width": W, "height": H, "source_rgba8": flat(im)}

    # 5. Gray 8-bit tagged with a gamma-1.8 Gray profile; LittleCMS Gray -> Veskra sRGB.
    gicc = gray_icc(1.8, "Veskra test Gray gamma 1.8")
    (HERE / "gray-gamma18-v4.icc").write_bytes(gicc)
    gray = Image.new("L", (W, H))
    gray.putdata([i for i in range(256)])
    save("gray18_icc_gray8.png", gray, icc_profile=gicc)
    fx["gray18_icc_gray8.png"] = {
        "width": W, "height": H, "source_rgba8": flat(gray),
        "lcms_to_veskra_srgb_rgb8_nooptimize": lcms_rgb(gicc, SRGB_ICC, gray, "L", F.NOOPTIMIZE),
        "lcms_to_veskra_srgb_rgb8_default": lcms_rgb(gicc, SRGB_ICC, gray, "L", F.NONE),
    }

    # 6. Palette + tRNS (partial transparency), untagged.
    pal = Image.new("P", (W, H))
    palette = [c for rgb in sw[:64] for c in rgb]
    pal.putpalette(palette)
    pal.putdata([(i * 7) % 64 for i in range(W * H)])
    trns = bytes([(k * 4) % 256 for k in range(64)])
    save("palette_trns.png", pal, transparency=trns)
    decoded = Image.open(HERE / "palette_trns.png")
    fx["palette_trns.png"] = {"width": W, "height": H, "source_rgba8": flat(decoded)}

    # 7. Adam7 interlaced RGBA8, odd size (13x11) so some passes are short; sRGB chunk.
    iw, ih = 13, 11
    ipx = [(sw[i][0], sw[i][1], sw[i][2], 255 - (i * 3) % 256) for i in range(iw * ih)]
    data = write_adam7_rgba8(ipx, iw, ih, [chunk(b"sRGB", b"\x00")])
    (HERE / "interlaced_rgba8.png").write_bytes(data)
    dec = Image.open(io.BytesIO(data))
    assert dec.info.get("interlace") == 1, "fixture must be interlaced"
    assert flat(dec) == [v for p in ipx for v in p], "Pillow must decode the hand-written Adam7 file"
    fx["interlaced_rgba8.png"] = {"width": iw, "height": ih, "source_rgba8": flat(dec)}

    out = json.dumps(expected, separators=(",", ":"), sort_keys=True)
    (HERE / "expected.json").write_text(out + "\n")
    total = sum(p.stat().st_size for p in HERE.iterdir() if p.is_file())
    print(f"wrote fixtures, directory total {total} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
