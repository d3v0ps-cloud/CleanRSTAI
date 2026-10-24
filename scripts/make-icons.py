#!/usr/bin/env python3
"""Build the app icons in assets/icon/ from assets/icon/glyph.png.

The glyph (black on transparent) sits on a rounded plate, so it stays visible on light and
dark docks and taskbars. Two variants use the app's own palette (ui/theme.rs):

  light: dark ink glyph on the light paper colour  (the default, bundled into the .app)
  dark:  light ink glyph on the dark raised colour (window icon while the app is dark)

Outputs:
  icon-{light,dark}.png          1024 px masters
  window-{light,dark}.png        256 px, embedded in the binary as the window icon
  macos/icon_<n>x<n>[@2x].png    the sizes cargo-bundle turns into the .icns
  icon.ico                       16-256 px, for Windows

Needs Pillow. Run from the repository root: python3 scripts/make-icons.py
"""

from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent / "assets" / "icon"

# (plate fill, plate outline, glyph) from theme::palette: paper/outline/ink (light) and
# raised/outline/ink (dark).
VARIANTS = {
    "light": ((0xFA, 0xF9, 0xF5), (0xD3, 0xCF, 0xC4), (0x1F, 0x1E, 0x1D)),
    "dark": ((0x20, 0x20, 0x1F), (0x4D, 0x4D, 0x4C), (0xF0, 0xEF, 0xED)),
}

SUPERSAMPLE = 4


def glyph_mask(size: int) -> Image.Image:
    """The glyph's coverage at `size` px, with crisp edges even when scaled up."""
    alpha = Image.open(ROOT / "glyph.png").convert("RGBA").getchannel("A")
    big = alpha.resize((size * SUPERSAMPLE,) * 2, Image.Resampling.BICUBIC)
    # Re-sharpen the interpolated edge (the art is two-tone), then antialias on the way down.
    big = big.point(lambda v: 0 if v < 96 else 255 if v > 160 else (v - 96) * 255 // 64)
    return big.resize((size, size), Image.Resampling.LANCZOS)


def render(size: int, variant: str) -> Image.Image:
    fill, outline, ink = VARIANTS[variant]
    if size >= 128:
        # macOS grid: an 824 px plate with ~185 px corners on a 1024 px canvas.
        plate, radius, glyph = size * 824 // 1024, size * 185 // 1024, size * 600 // 1024
    else:
        # Small sizes: use the whole square so the glyph stays legible.
        plate, radius, glyph = size, size * 22 // 100, size * 80 // 100

    s = SUPERSAMPLE
    canvas = Image.new("RGBA", (size * s, size * s), (0, 0, 0, 0))
    off = (size - plate) * s // 2
    border = max(1, size // 128) * s
    ImageDraw.Draw(canvas).rounded_rectangle(
        (off, off, off + plate * s - 1, off + plate * s - 1),
        radius=radius * s,
        fill=fill + (255,),
        outline=outline + (255,),
        width=border,
    )
    canvas = canvas.resize((size, size), Image.Resampling.LANCZOS)

    mask = glyph_mask(glyph)
    layer = Image.new("RGBA", (glyph, glyph), ink + (255,))
    pos = ((size - glyph) // 2,) * 2
    canvas.paste(layer, pos, mask)
    return canvas


def main() -> None:
    for variant in VARIANTS:
        render(1024, variant).save(ROOT / f"icon-{variant}.png", optimize=True)
        render(256, variant).save(ROOT / f"window-{variant}.png", optimize=True)

    mac = ROOT / "macos"
    mac.mkdir(exist_ok=True)
    for n in (16, 32, 128, 256, 512):
        render(n, "light").save(mac / f"icon_{n}x{n}.png", optimize=True)
        render(n * 2, "light").save(mac / f"icon_{n}x{n}@2x.png", optimize=True)

    sizes = (16, 24, 32, 48, 64, 128, 256)
    images = [render(n, "light") for n in sizes]
    images[-1].save(ROOT / "icon.ico", sizes=[(n, n) for n in sizes], append_images=images[:-1])


if __name__ == "__main__":
    main()
