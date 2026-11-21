"""Builds the NSIS installer header and sidebar bitmaps from the app icon.
Run after `npx tauri icon app-icon.svg -o src-tauri/icons`:  python scripts/installer-images.py
"""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
ICONS = ROOT / "src-tauri" / "icons"
INK = (22, 24, 28)
AMBER = (245, 166, 35)
FONTS = Path("C:/Windows/Fonts")


def font(name, size):
    try:
        return ImageFont.truetype(str(FONTS / name), size)
    except OSError:
        return ImageFont.load_default()


icon = Image.open(ICONS / "icon.png").convert("RGBA")

# Sidebar: 164x314, shown on the welcome and finish pages.
side = Image.new("RGB", (164, 314), INK)
logo = icon.resize((84, 84), Image.LANCZOS)
side.paste(logo, ((164 - 84) // 2, 70), logo)
d = ImageDraw.Draw(side)
title = font("segoeuib.ttf", 26)
sub = font("segoeui.ttf", 12)
for text, f, y, color in [("Utter", title, 168, (255, 255, 255)), ("Talk. It types.", sub, 206, AMBER), ("Offline.", sub, 222, AMBER)]:
    w = d.textlength(text, font=f)
    d.text(((164 - w) / 2, y), text, font=f, fill=color)
side.save(ICONS / "installer-sidebar.bmp")

# Header: 150x57, top-right of the inner pages.
head = Image.new("RGB", (150, 57), (255, 255, 255))
small = icon.resize((40, 40), Image.LANCZOS)
head.paste(small, (150 - 40 - 10, 8), small)
head.save(ICONS / "installer-header.bmp")
print("wrote installer-sidebar.bmp and installer-header.bmp")
