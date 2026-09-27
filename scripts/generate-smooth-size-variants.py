#!/usr/bin/env python3
"""Give the existing smooth faces distinct names for embedded size presets.

This is a maintainer tool, not a release-build dependency. Font outlines and
licences are unchanged; the generated TTFs are checked in.
"""

from pathlib import Path

from fontTools.ttLib import TTFont


ROOT = Path(__file__).resolve().parents[1]
FONTS = ROOT / "assets" / "fonts"
SOURCES = (
    ("DejaVuSans.ttf", "DejaVu Sans", "DejaVuSans"),
    ("RobotoCondensed-Bold.ttf", "Roboto Condensed", "RobotoCondensed-Bold"),
)
PRESETS = ("Smaller", "Small", "Large", "Larger")


for source, family, stem in SOURCES:
    for preset in PRESETS:
        font = TTFont(FONTS / source)
        names = font["name"]
        variant_family = f"{family} {preset}"
        postscript = f"{stem}-{preset}"
        for record in names.names:
            if record.nameID in (1, 4):
                replacement = variant_family
            elif record.nameID in (3, 6):
                replacement = postscript
            else:
                continue
            names.setName(
                replacement,
                record.nameID,
                record.platformID,
                record.platEncID,
                record.langID,
            )
        font["head"].created = 0
        font["head"].modified = 0
        output = FONTS / f"{stem}-{preset.lower()}.ttf"
        font.save(output)
        print(output.relative_to(ROOT))
