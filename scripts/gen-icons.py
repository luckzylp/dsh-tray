#!/usr/bin/env python3
"""Generate dsh-tray icons from the committed DeepSeek whale sources.

- `assets/icons/whale-base-32.png` is the official DeepSeek whale silhouette
  (alpha mask; RGB is irrelevant). It is recoloured for the three tray states.
- `assets/app/icon-source.png` is the official DSH application icon; the
  generated `assets/app/icon.png` / `icon.ico` are derived from it.

Run from the repository root: python3 scripts/gen-icons.py
Requires Pillow. The generated files are committed; rerun only when the
artwork or colours change.
"""
from __future__ import annotations

import os

from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ICONS = os.path.join(ROOT, "assets", "icons")
APP = os.path.join(ROOT, "assets", "app")
BASE_WHALE = os.path.join(ICONS, "whale-base-32.png")
APP_SOURCE = os.path.join(APP, "icon-source.png")

# Tray state colours: stopped = neutral gray, running = green, error = red.
STATE_COLORS = {
    "stopped": (138, 143, 152, 255),
    "running": (34, 197, 94, 255),
    "error": (239, 68, 68, 255),
}


def recolour(alpha: Image.Image, colour: tuple[int, int, int, int]) -> Image.Image:
    """Apply a solid colour to an alpha mask."""
    image = Image.new("RGBA", alpha.size, colour)
    image.putalpha(alpha)
    return image


def main() -> None:
    os.makedirs(ICONS, exist_ok=True)
    os.makedirs(APP, exist_ok=True)

    base = Image.open(BASE_WHALE).convert("RGBA")
    alpha32 = base.getchannel("A")
    alpha64 = alpha32.resize((64, 64), Image.LANCZOS)
    for state, colour in STATE_COLORS.items():
        recolour(alpha32, colour).save(os.path.join(ICONS, f"tray-{state}.png"))
        recolour(alpha64, colour).save(os.path.join(ICONS, f"tray-{state}@2x.png"))

    app_icon = Image.open(APP_SOURCE).convert("RGBA")
    app_icon.save(os.path.join(APP, "icon.png"))
    app_icon.save(
        os.path.join(APP, "icon.ico"),
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )
    print("icons written to", ICONS, "and", APP)


if __name__ == "__main__":
    main()
