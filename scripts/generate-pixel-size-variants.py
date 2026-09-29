#!/usr/bin/env python3
"""Build crisp, fixed-size variants of Degauss's two existing pixel fonts.

This is a maintainer tool, not a release-build dependency. The generated TTFs
are checked in. Install fonttools and Pillow to regenerate them.
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import TTFont
from fontTools.ttLib.scaleUpem import scale_upem


ROOT = Path(__file__).resolve().parents[1]
FONTS = ROOT / "assets" / "fonts"
SOURCES = (
    ("Px437_DOS-V_re_JPN12.ttf", "Px437 DOS/V re. JPN12", "Px437_JPN12"),
    ("Tamzen6x12b.ttf", "Tamzen", "Tamzen"),
)
SIZES = (8, 10, 14, 16)
# A multiple of every desired size keeps each pixel boundary exact in the TTF.
UNITS_PER_EM = 8400


def nearest_pixel(value: int, units_per_pixel: int) -> int:
    if value >= 0:
        return ((value + units_per_pixel // 2) // units_per_pixel) * units_per_pixel
    return -nearest_pixel(-value, units_per_pixel)


def smaller_glyphs(font: TTFont, source: Path, size: int, unit: int) -> None:
    """Reduce 6x12 cells by coverage, retaining one-pixel strokes and dots."""
    native = ImageFont.truetype(str(source), 12)
    source_unit = UNITS_PER_EM // 12
    ascent = round(font["hhea"].ascent * size / UNITS_PER_EM)
    glyphs = font["glyf"]
    done = set()
    for codepoint, name in font.getBestCmap().items():
        if name in done:
            continue
        done.add(name)
        advance, _ = font["hmtx"][name]
        source_advance = max(1, (advance + source_unit // 2) // source_unit)
        bounds = native.getbbox(chr(codepoint))
        source_left = min(0, bounds[0]) if bounds else 0
        source_right = max(source_advance, bounds[2]) if bounds else source_advance
        source_bitmap = Image.new("L", (source_right - source_left, 12))
        ImageDraw.Draw(source_bitmap).text(
            (-source_left, 0), chr(codepoint), font=native, fill=255
        )
        target_width = max(1, (advance + unit // 2) // unit)
        reduced = source_bitmap.resize((target_width, size), Image.Resampling.BOX)
        pen = TTGlyphPen(None)
        for row in range(size):
            column = 0
            while column < target_width:
                if reduced.getpixel((column, row)) == 0:
                    column += 1
                    continue
                start = column
                while column < target_width and reduced.getpixel((column, row)):
                    column += 1
                left, right = start * unit, column * unit
                bottom, top = (ascent - row - 1) * unit, (ascent - row) * unit
                pen.moveTo((left, bottom))
                pen.lineTo((right, bottom))
                pen.lineTo((right, top))
                pen.lineTo((left, top))
                pen.closePath()
        glyph = pen.glyph()
        glyphs[name] = glyph
        if glyph.numberOfContours > 0:
            glyph.recalcBounds(glyphs)
            font["hmtx"][name] = (advance, glyph.xMin)


def make_variant(source: str, family: str, stem: str, size: int) -> None:
    source_path = FONTS / source
    font = TTFont(source_path)
    if font["head"].unitsPerEm != 1200:
        raise ValueError(f"{source}: unexpected units per em")
    scale_upem(font, UNITS_PER_EM)
    unit = UNITS_PER_EM // size
    if unit * size != UNITS_PER_EM:
        raise ValueError(f"{size}: fractional pixel grid")

    if size < 12:
        smaller_glyphs(font, source_path, size, unit)
    else:
        glyphs = font["glyf"]
        for name in font.getGlyphOrder():
            glyph = glyphs[name]
            if glyph.isComposite():
                raise ValueError(f"{source}: composite glyph {name}")
            if glyph.numberOfContours <= 0:
                continue
            for index, (x, y) in enumerate(glyph.coordinates):
                glyph.coordinates[index] = (
                    nearest_pixel(x, unit),
                    nearest_pixel(y, unit),
                )
            glyph.recalcBounds(glyphs)
            advance, _ = font["hmtx"][name]
            font["hmtx"][name] = (advance, glyph.xMin)

    variant_family = f"{family} {size}"
    postscript = f"{stem}-{size}"
    names = font["name"]
    for record in names.names:
        if record.nameID == 1:
            replacement = variant_family
        elif record.nameID == 3:
            replacement = postscript
        elif record.nameID == 4:
            replacement = variant_family
        elif record.nameID == 6:
            replacement = postscript
        else:
            continue
        names.setName(replacement, record.nameID, record.platformID, record.platEncID, record.langID)

    # Fixed metadata makes regeneration reproducible.
    font["head"].created = 0
    font["head"].modified = 0
    output = FONTS / f"{stem}-{size}.ttf"
    font.save(output)
    print(output.relative_to(ROOT))


if __name__ == "__main__":
    for source, family, stem in SOURCES:
        for size in SIZES:
            make_variant(source, family, stem, size)
