#!/usr/bin/env python3
"""Generate dsh-tray icon assets (tray PNGs + app icon PNG/ICO).

Run from the repository root: python3 scripts/gen-icons.py
Requires Pillow. The generated files are committed; rebuilding them is only
needed when the artwork changes.
"""
from __future__ import annotations

import os
from PIL import Image, ImageDraw

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ICONS = os.path.join(ROOT, "assets", "icons")
APP = os.path.join(ROOT, "assets", "app")

BG_TOP = (15, 23, 42, 255)        # slate-900
BG_BOTTOM = (30, 41, 59, 255)     # slate-800
ACCENT = (56, 189, 248, 255)      # sky-400
ACCENT_2 = (34, 211, 238, 255)    # cyan-400


def rounded_mask(size: int, radius: int) -> Image.Image:
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, size - 1, size - 1), radius=radius, fill=255)
    return mask


def gradient(size: int) -> Image.Image:
    img = Image.new("RGBA", (size, size))
    for y in range(size):
        t = y / max(1, size - 1)
        color = tuple(round(BG_TOP[i] * (1 - t) + BG_BOTTOM[i] * t) for i in range(4))
        for x in range(size):
            img.putpixel((x, y), color)
    return img


def draw_glyph(draw: ImageDraw.ImageDraw, size: int, color, scale: float = 1.0):
    """Terminal-prompt chevron + underscore, centered."""
    s = size * scale
    cx, cy = size / 2, size / 2
    lw = max(1, round(size * 0.105 * scale))
    # chevron ">"
    left = cx - s * 0.20
    right = cx + s * 0.02
    top = cy - s * 0.19
    mid = cy
    bottom = cy + s * 0.19
    draw.line([(left, top), (right, mid), (left, bottom)], fill=color, width=lw, joint="curve")
    # underscore
    ux1, ux2 = cx + s * 0.10, cx + s * 0.30
    uy = cy + s * 0.19
    draw.line([(ux1, uy), (ux2, uy)], fill=color, width=lw)


def squared_icon(size: int, pad_ratio: float = 0.0) -> Image.Image:
    ss = 4
    base = size * ss
    canvas = Image.new("RGBA", (base, base), (0, 0, 0, 0))
    pad = round(base * pad_ratio)
    inner = base - pad * 2
    bg = gradient(inner)
    bg.putalpha(rounded_mask(inner, radius=round(inner * 0.22)))
    canvas.alpha_composite(bg, (pad, pad))
    glyph = Image.new("RGBA", (base, base), (0, 0, 0, 0))
    draw_glyph(ImageDraw.Draw(glyph), base, ACCENT, scale=inner / base)
    canvas.alpha_composite(glyph)
    return canvas.resize((size, size), Image.LANCZOS)


def template_icon(size: int) -> Image.Image:
    """macOS template image: black glyph with alpha, no background."""
    ss = 4
    base = size * ss
    canvas = Image.new("RGBA", (base, base), (0, 0, 0, 0))
    draw_glyph(ImageDraw.Draw(canvas), base, (0, 0, 0, 255), scale=0.92)
    return canvas.resize((size, size), Image.LANCZOS)


def main() -> None:
    os.makedirs(ICONS, exist_ok=True)
    os.makedirs(APP, exist_ok=True)

    squared_icon(32, pad_ratio=0.02).save(os.path.join(ICONS, "tray.png"))
    squared_icon(64, pad_ratio=0.02).save(os.path.join(ICONS, "tray@2x.png"))
    template_icon(32).save(os.path.join(ICONS, "tray-template.png"))
    template_icon(64).save(os.path.join(ICONS, "tray-template@2x.png"))

    app_icon = squared_icon(512)
    app_icon.save(os.path.join(APP, "icon.png"))
    app_icon.save(
        os.path.join(APP, "icon.ico"),
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )
    print("icons written to", ICONS, "and", APP)


if __name__ == "__main__":
    main()
