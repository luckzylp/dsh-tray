#!/usr/bin/env python3
"""Generate dsh-tray icons from the committed DeepSeek whale sources.

- \`assets/icons/whale-base-32.png\` is the official DeepSeek whale silhouette
  (alpha mask; RGB is irrelevant). It is recoloured for the three tray states.
- \`assets/app/whale.svg\` is the official DeepSeek whale logo (blue #4D6BFE);
  the application icon is rendered from it at 1024px and packed into an ICO.

Run from the repository root: python3 scripts/gen-icons.py
Requires Pillow plus Python GI (Rsvg) and pycairo for the SVG application
icon; if the GI renderer is unavailable the existing \`icon.png\` is reused to
rebuild the ICO. The generated files are committed; rerun only when the
artwork or colours change.
"""
from __future__ import annotations

import os
from io import BytesIO

from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ICONS = os.path.join(ROOT, "assets", "icons")
APP = os.path.join(ROOT, "assets", "app")
BASE_WHALE = os.path.join(ICONS, "whale-base-32.png")
APP_SVG = os.path.join(APP, "whale.svg")
APP_PNG = os.path.join(APP, "icon.png")

# Tray state colours: stopped = neutral gray, running = green, error = red.
STATE_COLORS = {
    "stopped": (138, 143, 152, 255),
    "running": (34, 197, 94, 255),
    "error": (239, 68, 68, 255),
}

APP_ICON_SIZE = 1024
ICO_SIZES = [(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]


def recolour(alpha: Image.Image, colour: tuple[int, int, int, int]) -> Image.Image:
    """Apply a solid colour to an alpha mask."""
    image = Image.new("RGBA", alpha.size, colour)
    image.putalpha(alpha)
    return image


def render_app_icon() -> Image.Image:
    """Rasterise the official blue whale SVG with the system Rsvg renderer."""
    try:
        import gi

        gi.require_version("Rsvg", "2.0")
        import cairo
        from gi.repository import Rsvg
    except (ImportError, ValueError) as error:
        if os.path.exists(APP_PNG):
            print(
                "warning: Rsvg/pycairo unavailable ("
                + str(error)
                + "); reusing existing assets/app/icon.png for the ICO"
            )
            return Image.open(APP_PNG).convert("RGBA")
        raise SystemExit(
            "rendering assets/app/whale.svg needs Python GI (Rsvg) and pycairo; "
            "install python3-gi, gir1.2-rsvg and python3-cairo"
        ) from error

    handle = Rsvg.Handle.new_from_file(APP_SVG)
    surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, APP_ICON_SIZE, APP_ICON_SIZE)
    context = cairo.Context(surface)
    rect = Rsvg.Rectangle()
    rect.x = 0
    rect.y = 0
    rect.width = APP_ICON_SIZE
    rect.height = APP_ICON_SIZE
    handle.render_document(context, rect)
    surface.flush()
    png = BytesIO()
    surface.write_to_png(png)
    png.seek(0)
    return Image.open(png).convert("RGBA")


def main() -> None:
    os.makedirs(ICONS, exist_ok=True)
    os.makedirs(APP, exist_ok=True)

    base = Image.open(BASE_WHALE).convert("RGBA")
    alpha32 = base.getchannel("A")
    alpha64 = alpha32.resize((64, 64), Image.LANCZOS)
    for state, colour in STATE_COLORS.items():
        recolour(alpha32, colour).save(os.path.join(ICONS, f"tray-{state}.png"))
        recolour(alpha64, colour).save(os.path.join(ICONS, f"tray-{state}@2x.png"))

    app_icon = render_app_icon()
    app_icon.save(APP_PNG)
    app_icon.save(os.path.join(APP, "icon.ico"), sizes=ICO_SIZES)
    print("icons written to", ICONS, "and", APP)


if __name__ == "__main__":
    main()
