//! The two typefaces the interface can be set in.
//!
//! Glyphs are bitmaps baked at build time, one set per pixel size, so a
//! typeface exists only at the sizes in [`font_sizes`](../font_sizes). The
//! renderer, asked for a size it does not have, draws the largest it does
//! have that is smaller, and says nothing. Every size handed to it is
//! therefore rounded here first, to a size that is known to exist: what is
//! asked for is what is drawn.

include!("font_sizes.rs");

/// Size of text in menu and game/system list rows. Other UI text and row
/// geometry remain unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextSize {
    Smaller,
    Small,
    #[default]
    Default,
    Large,
    Larger,
}

impl TextSize {
    pub const ALL: [Self; 5] = [
        Self::Smaller,
        Self::Small,
        Self::Default,
        Self::Large,
        Self::Larger,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Smaller => "smaller",
            Self::Small => "small",
            Self::Default => "default",
            Self::Large => "large",
            Self::Larger => "larger",
        }
    }

    pub fn shown(self) -> &'static str {
        match self {
            Self::Smaller => "Smaller",
            Self::Small => "Small",
            Self::Default => "Default",
            Self::Large => "Large",
            Self::Larger => "Larger",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|size| size.label().eq_ignore_ascii_case(text))
    }

    pub fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|size| *size == self).unwrap()
    }
}

/// Which typeface the interface is set in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Font {
    /// DejaVu Sans, anti-aliased.
    #[default]
    Smooth,
    /// Px437 DOS/V re. JPN12, one pixel a pixel.
    Pixel,
    /// Roboto Condensed Bold: bolder and condensed, still anti-aliased, and
    /// narrower than Smooth on every real title, so nothing that fits
    /// stops fitting.
    Smooth2,
    /// Tamzen 6x12 Bold: the same six pixel cell as Pixel with two pixel
    /// strokes where Pixel has one. The letterforms sit smaller in the
    /// cell, so it reads thicker but smaller.
    Pixel2,
}

impl Font {
    /// In the order the option cycles through them.
    pub const ALL: [Font; 4] = [Font::Smooth, Font::Pixel, Font::Smooth2, Font::Pixel2];

    /// What `settings.toml` and theme files record. Lowercase in the file,
    /// like every other name written there.
    pub fn label(self) -> &'static str {
        match self {
            Font::Smooth => "smooth",
            Font::Pixel => "pixel",
            Font::Smooth2 => "smooth 2",
            Font::Pixel2 => "pixel 2",
        }
    }

    /// What a menu row shows. Kept beside the persisted label so theme
    /// editing and the main Text option cannot drift in spelling.
    pub fn shown(self) -> &'static str {
        match self {
            Font::Smooth => "Smooth",
            Font::Pixel => "Pixel",
            Font::Smooth2 => "Smooth 2",
            Font::Pixel2 => "Pixel 2",
        }
    }

    /// The name the typeface calls itself by, which is what the renderer
    /// matches on. Exactly, byte for byte: a name that does not match is not
    /// an error, it silently draws in the other font.
    pub fn family(self) -> &'static str {
        match self {
            Font::Smooth => "DejaVu Sans",
            Font::Pixel => "Px437 DOS/V re. JPN12",
            Font::Smooth2 => "Roboto Condensed",
            Font::Pixel2 => "Tamzen",
        }
    }

    /// The separately embedded family for a list/menu preset. The default
    /// keeps the released family and size exactly as before.
    pub fn text_family(self, size: TextSize) -> &'static str {
        match (self, size) {
            (Font::Smooth, TextSize::Smaller) => "DejaVu Sans Smaller",
            (Font::Smooth, TextSize::Small) => "DejaVu Sans Small",
            (Font::Smooth, TextSize::Large) => "DejaVu Sans Large",
            (Font::Smooth, TextSize::Larger) => "DejaVu Sans Larger",
            (Font::Smooth2, TextSize::Smaller) => "Roboto Condensed Smaller",
            (Font::Smooth2, TextSize::Small) => "Roboto Condensed Small",
            (Font::Smooth2, TextSize::Large) => "Roboto Condensed Large",
            (Font::Smooth2, TextSize::Larger) => "Roboto Condensed Larger",
            (Font::Pixel, TextSize::Smaller) => "Px437 DOS/V re. JPN12 8",
            (Font::Pixel, TextSize::Small) => "Px437 DOS/V re. JPN12 10",
            (Font::Pixel, TextSize::Large) => "Px437 DOS/V re. JPN12 14",
            (Font::Pixel, TextSize::Larger) => "Px437 DOS/V re. JPN12 16",
            (Font::Pixel2, TextSize::Smaller) => "Tamzen 8",
            (Font::Pixel2, TextSize::Small) => "Tamzen 10",
            (Font::Pixel2, TextSize::Large) => "Tamzen 14",
            (Font::Pixel2, TextSize::Larger) => "Tamzen 16",
            _ => self.family(),
        }
    }

    /// Quantise to the released rung first, then choose the corresponding
    /// preset size. This preserves the exact default and the existing
    /// resolution-dependent text scale without changing row geometry.
    pub fn text_glyph(self, request: f32, size: TextSize) -> f32 {
        let baseline = self.quantise(request);
        let rung = self
            .sizes()
            .iter()
            .position(|value| *value == baseline)
            .unwrap();
        match self {
            Font::Smooth | Font::Smooth2 => SMOOTH_TEXT_SIZES[rung][size.index()],
            Font::Pixel | Font::Pixel2 => PIXEL_TEXT_BASE_SIZES[size.index()] * (rung + 1) as f32,
        }
    }

    /// The sizes this typeface is baked at, ascending.
    pub fn sizes(self) -> &'static [f32] {
        match self {
            Font::Smooth | Font::Smooth2 => &SMOOTH_SIZES,
            Font::Pixel | Font::Pixel2 => &PIXEL_SIZES,
        }
    }

    /// Read back from `settings.toml`, where an older or hand-edited file
    /// can say anything.
    pub fn parse(text: &str) -> Option<Font> {
        Font::ALL
            .into_iter()
            .find(|font| font.label().eq_ignore_ascii_case(text))
    }

    /// The next one along, used by right and A in Options.
    pub fn next(self) -> Font {
        match self {
            Font::Smooth => Font::Pixel,
            Font::Pixel => Font::Smooth2,
            Font::Smooth2 => Font::Pixel2,
            Font::Pixel2 => Font::Smooth,
        }
    }

    /// The previous one, used by left in Options.
    pub fn prev(self) -> Font {
        match self {
            Font::Smooth => Font::Pixel2,
            Font::Pixel => Font::Smooth,
            Font::Smooth2 => Font::Pixel,
            Font::Pixel2 => Font::Smooth2,
        }
    }

    /// The size a request is drawn at: the largest baked size that does not
    /// exceed it, or the smallest baked size when the request is under all
    /// of them.
    ///
    /// This is the renderer's own rule, applied before the renderer sees the
    /// request rather than after. Doing it here is what lets the two
    /// typefaces have different ladders: the glyphs are baked from the union
    /// of both, so left to itself the renderer would answer a request of 26
    /// with 26 in either font, which is a size the pixel font has no whole
    /// pixels at.
    pub fn quantise(self, request: f32) -> f32 {
        let sizes = self.sizes();
        sizes
            .iter()
            .rev()
            .copied()
            .find(|&size| size <= request)
            .unwrap_or(sizes[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_is_rounded_down_to_one_that_exists() {
        assert_eq!(Font::Smooth.quantise(30.0), 26.0);
        assert_eq!(Font::Smooth.quantise(26.0), 26.0, "an exact size is kept");
        assert_eq!(Font::Pixel.quantise(30.0), 24.0);
        assert_eq!(Font::Pixel.quantise(1000.0), 48.0, "clamped to the largest");
    }

    #[test]
    fn a_size_under_the_ladder_gets_the_smallest_rather_than_nothing() {
        // The bar asks for 8 pixels on a 240 line screen and no typeface is
        // baked that small. Drawing nothing, or dividing by zero looking for
        // a smaller one, are both worse than a legible 12.
        assert_eq!(Font::Smooth.quantise(7.0), 12.0);
        assert_eq!(Font::Pixel.quantise(7.0), 12.0);
    }

    #[test]
    fn every_pixel_size_is_a_whole_number_of_cells() {
        // The font is drawn on a 6 by 12 grid at a 12 pixel em. At any size
        // that is not a multiple of 12 the grid falls between pixels and the
        // renderer fills the difference with grey, which is the whole of
        // what separates this typeface from the other one.
        for size in PIXEL_SIZES {
            assert_eq!(size % 12.0, 0.0, "{size} is not a whole cell");
        }
    }

    #[test]
    fn the_ladders_are_ascending_so_rounding_down_finds_the_nearest() {
        for font in Font::ALL {
            let sizes = font.sizes();
            assert!(
                sizes.windows(2).all(|pair| pair[0] < pair[1]),
                "{:?} sizes are out of order",
                font
            );
        }
    }

    #[test]
    fn what_is_written_to_settings_is_what_comes_back() {
        for font in Font::ALL {
            assert_eq!(Font::parse(font.label()), Some(font));
            assert_eq!(Font::parse(font.shown()), Some(font));
        }
        assert_eq!(Font::parse("Smooth"), Some(Font::Smooth), "case is ignored");
        assert_eq!(
            Font::parse("Courier"),
            None,
            "an unknown name is not a font"
        );
    }

    #[test]
    fn previous_and_next_are_exact_inverses_for_every_font() {
        for font in Font::ALL {
            assert_eq!(font.prev().next(), font);
            assert_eq!(font.next().prev(), font);
        }
        assert_eq!(Font::Smooth.prev(), Font::Pixel2, "left wraps backward");
        assert_eq!(Font::Pixel2.next(), Font::Smooth, "right wraps forward");
    }

    #[test]
    fn list_size_presets_preserve_default_and_order_for_every_typeface() {
        for font in Font::ALL {
            for &request in font.sizes() {
                let rendered: Vec<_> = TextSize::ALL
                    .into_iter()
                    .map(|size| font.text_glyph(request, size))
                    .collect();
                assert_eq!(rendered[2], font.quantise(request));
                assert!(rendered.windows(2).all(|pair| pair[0] < pair[1]));
                assert_eq!(font.text_family(TextSize::Default), font.family());
                for size in TextSize::ALL {
                    if size != TextSize::Default {
                        assert_ne!(font.text_family(size), font.family());
                    }
                }
            }
        }
        for size in TextSize::ALL {
            assert_eq!(TextSize::parse(size.shown()), Some(size));
            assert_eq!(size.next().prev(), size);
            assert_eq!(size.prev().next(), size);
        }
        assert_eq!(TextSize::parse("anything else"), None);
    }

    #[test]
    fn pixel_preset_requests_use_an_integer_pixel_grid() {
        for font in [Font::Pixel, Font::Pixel2] {
            for &request in font.sizes() {
                for size in TextSize::ALL {
                    assert_eq!(font.text_glyph(request, size).fract(), 0.0);
                }
            }
        }
    }

    #[test]
    fn the_two_typefaces_are_not_the_same_one() {
        assert_ne!(Font::Smooth.family(), Font::Pixel.family());
        assert_ne!(Font::Smooth.label(), Font::Pixel.label());
    }

    /// What the build actually baked into this binary.
    fn generated_ui() -> String {
        std::fs::read_to_string(concat!(env!("OUT_DIR"), "/degauss.rs"))
            .expect("the build writes the compiled interface here")
    }

    #[test]
    fn both_typefaces_are_in_the_binary_under_the_names_asked_for() {
        // A family name that does not match one the build embedded is not an
        // error and draws nothing unusual: the renderer silently falls back
        // to the first font it has, so the option would appear to do nothing
        // at all. One typo either side of this is invisible without it.
        let generated = generated_ui();
        for font in Font::ALL {
            assert!(
                generated.contains(&format!("{:?}", font.family())),
                "{:?} is not embedded under the name {:?}",
                font,
                font.family()
            );
        }
    }

    #[test]
    fn every_size_asked_for_was_baked() {
        // A size in a ladder that the build did not bake is drawn at the
        // largest smaller one instead, which for the pixel font means
        // off-grid glyphs and for either means the wrong size.
        let generated = generated_ui();
        for font in Font::ALL {
            for size in font.sizes() {
                assert!(
                    generated.contains(&format!("pixel_size : {size}i16")),
                    "{:?} asks for {size} and the build did not bake it",
                    font
                );
            }
        }
    }

    #[test]
    fn a_name_off_a_card_has_a_glyph_for_every_letter_in_it() {
        // Nothing is drawn for a character whose glyph was not baked: not a
        // box, not a question mark, nothing. A name loses the letter and
        // reads as though it had a space there. These are the accents and
        // marks that real releases and fan translations put in a filename.
        let generated = generated_ui();
        let names = [
            "Astérix - Le Défi",         // French
            "Märchen Adventure Cotton",  // German
            "Pokémon Café",              // the one everybody has
            "Zażółć gęślą jaźń",         // Polish
            "Příliš žluťoučký kůň",      // Czech
            "Árvíztűrő tükörfúrógép",    // Hungarian
            "Güneş Şafağı",              // Turkish
            "Ș Ț în România",            // Romanian
            "Tiếng Việt",                // Vietnamese
            "Контра",                    // Cyrillic, from a translation
            "Ελλάδα",                    // Greek
            "½ ¼ ⅓ ° ™ € № — ' ' \" \"", // what a title puts around a name
        ];
        for name in names {
            for character in name.chars() {
                assert!(
                    generated.contains(&format!("code_point : {character:?}")),
                    "{character:?} (U+{:04X}), in {name:?}, has no glyph and would draw as a gap",
                    character as u32
                );
            }
        }
    }

    #[test]
    fn the_glyphs_baked_by_the_build_cover_both_ladders() {
        // build.rs names the sizes to bake and this names the sizes to ask
        // for. They are the same list only because both come from
        // font_sizes.rs; if that ever stops being true, a size asked for
        // here is drawn at a smaller one with nothing said about it.
        let build = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/build.rs"))
            .expect("build.rs sits beside the crate it builds");
        assert!(
            build.contains("include!(\"src/font_sizes.rs\")"),
            "build.rs must take its sizes from font_sizes.rs, not a copy of them"
        );
    }
}
