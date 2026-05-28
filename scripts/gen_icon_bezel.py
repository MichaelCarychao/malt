"""
Generate a black-background bezeled source icon for malt from the existing
artwork.

The current source.png is a transparent-background lowercase "m" with a small
page-tab. On light OS themes (Windows taskbar, macOS Finder list view,
favicons against white) it disappears or looks weak. This script:

1. Reads src-tauri/icons/source.png (transparent BG, yellow "m").
2. Creates a 1024x1024 black-fill canvas with a rounded-corner mask (~18%
   of the side, conservative enough to look right both as a Windows square
   and after macOS's squircle pass).
3. Composites the original artwork centered at 70% scale so it has visible
   breathing room from the bezel edge.
4. Writes the result back to src-tauri/icons/source.png in place.

After running, regenerate the per-platform variants with:

    npx tauri icon src-tauri/icons/source.png

That populates all the 32x32 / 128x128 / icon.ico / icon.icns / etc. from
the new source.
"""

from pathlib import Path
from PIL import Image, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "src-tauri" / "icons" / "source.png"
# Backup of the previous artwork — keep it around so this script is idempotent
# (rerunning won't compound the bezel) and so we can revert if the bezel
# look turns out worse in some environment.
SOURCE_BACKUP = ROOT / "src-tauri" / "icons" / "source-transparent.png"

CANVAS = 1024
RADIUS_PCT = 0.18        # ~184px corner radius on a 1024px canvas
ART_SCALE = 0.70         # Inner artwork covers 70% of the canvas
BG = (0, 0, 0, 255)      # Solid black; Tauri's platform shaders take care
                         # of any further rounding for iOS / macOS.


def main():
    if not SOURCE.exists():
        raise SystemExit(f"missing source: {SOURCE}")

    # Restore from backup if present so reruns produce identical output.
    if SOURCE_BACKUP.exists():
        original = Image.open(SOURCE_BACKUP).convert("RGBA")
    else:
        original = Image.open(SOURCE).convert("RGBA")
        original.save(SOURCE_BACKUP)
        print(f"backed up transparent original -> {SOURCE_BACKUP.name}")

    # Build the rounded-corner bezel layer. We composite onto a transparent
    # canvas using a rounded-rectangle mask so the output PNG still has
    # platform-friendly transparent corners (Windows can square them off,
    # macOS can squircle them further).
    bezel = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    mask = Image.new("L", (CANVAS, CANVAS), 0)
    draw = ImageDraw.Draw(mask)
    radius = int(CANVAS * RADIUS_PCT)
    draw.rounded_rectangle((0, 0, CANVAS, CANVAS), radius=radius, fill=255)
    # Smooth the mask edge so it doesn't alias at small sizes.
    mask = mask.filter(ImageFilter.SMOOTH_MORE)
    bezel.paste(Image.new("RGBA", (CANVAS, CANVAS), BG), mask=mask)

    # Resize the original artwork. Keep its aspect ratio square via padding.
    art_size = int(CANVAS * ART_SCALE)
    art = original.copy()
    art.thumbnail((art_size, art_size), Image.LANCZOS)
    art_layer = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    ax = (CANVAS - art.width) // 2
    ay = (CANVAS - art.height) // 2
    art_layer.paste(art, (ax, ay), art)

    # Composite: bezel first, artwork on top.
    out = Image.alpha_composite(bezel, art_layer)
    out.save(SOURCE)
    print(f"wrote bezeled source -> {SOURCE} ({CANVAS}x{CANVAS})")
    print("next: npx tauri icon src-tauri/icons/source.png")


if __name__ == "__main__":
    main()
