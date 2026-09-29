#!/usr/bin/env python3
"""Regression checks for the maintainer's pixel-font generator."""

import importlib.util
import logging
from pathlib import Path
import unittest

from PIL import Image, ImageDraw, ImageFont
from fontTools.ttLib import TTFont
from fontTools.ttLib.scaleUpem import scale_upem


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "pixel_variants", ROOT / "scripts/generate-pixel-size-variants.py"
)
GENERATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GENERATOR)
logging.getLogger("fontTools").setLevel(logging.ERROR)


class PixelVariantTests(unittest.TestCase):
    def test_smaller_glyphs_retain_accents_outside_the_native_cell(self):
        for source, _, _ in GENERATOR.SOURCES:
            source_path = GENERATOR.FONTS / source
            native = ImageFont.truetype(str(source_path), 12)
            for size in (8, 10):
                with self.subTest(source=source, size=size):
                    font = TTFont(source_path)
                    scale_upem(font, GENERATOR.UNITS_PER_EM)
                    unit = GENERATOR.UNITS_PER_EM // size
                    advance = font["hmtx"][font.getBestCmap()[ord("É")]][0]
                    width = max(1, round(advance / unit))
                    bounds = native.getbbox("É")
                    left, top = min(0, bounds[0]), min(0, bounds[1])
                    right, bottom = max(6, bounds[2]), max(12, bounds[3])
                    bitmap = Image.new("L", (right - left, bottom - top))
                    ImageDraw.Draw(bitmap).text((-left, -top), "É", font=native, fill=255)
                    expected = bitmap.resize((width, size), Image.Resampling.BOX)

                    GENERATOR.smaller_glyphs(font, source_path, size, unit)
                    glyph = font["glyf"][font.getBestCmap()[ord("É")]]
                    ascent = round(font["hhea"].ascent * size / GENERATOR.UNITS_PER_EM)
                    actual = set()
                    start = 0
                    for end in glyph.endPtsOfContours:
                        coordinates = glyph.coordinates[start:end + 1]
                        x_min = min(x for x, _ in coordinates) // unit
                        x_max = max(x for x, _ in coordinates) // unit
                        y_max = max(y for _, y in coordinates) // unit
                        actual.update((column, ascent - y_max) for column in range(x_min, x_max))
                        start = end + 1
                    self.assertEqual(
                        actual,
                        {(x, y) for y in range(size) for x in range(width) if expected.getpixel((x, y))},
                        "The accented glyph must be reduced from its complete native bitmap",
                    )
                    font.close()


if __name__ == "__main__":
    unittest.main()
