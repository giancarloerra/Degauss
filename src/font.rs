//! The two typefaces the interface can be set in.
//!
//! Glyphs are bitmaps baked at build time, one set per pixel size, so a
//! typeface exists only at the sizes in [`font_sizes`](../font_sizes). The
//! renderer, asked for a size it does not have, draws the largest it does
//! have that is smaller, and says nothing. Every size handed to it is
//! therefore rounded here first, to a size that is known to exist: what is
//! asked for is what is drawn.

include!("font_sizes.rs");

/// Which typeface the interface is set in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Font {
    /// DejaVu Sans, anti-aliased.
    #[default]
    Smooth,
    /// Px437 DOS/V re. JPN12, one pixel a pixel.
    Pixel,
}

impl Font {
    /// In the order the option cycles through them.
    pub const ALL: [Font; 2] = [Font::Smooth, Font::Pixel];

    /// What `settings.toml` records, and what the options screen shows once
    /// capitalised. Lowercase in the file, like every other name written
    /// there.
    pub fn label(self) -> &'static str {
        match self {
            Font::Smooth => "smooth",
            Font::Pixel => "pixel",
        }
    }

    /// The name the typeface calls itself by, which is what the renderer
    /// matches on. Exactly, byte for byte: a name that does not match is not
    /// an error, it silently draws in the other font.
    pub fn family(self) -> &'static str {
        match self {
            Font::Smooth => "DejaVu Sans",
            Font::Pixel => "Px437 DOS/V re. JPN12",
        }
    }

    /// The sizes this typeface is baked at, ascending.
    pub fn sizes(self) -> &'static [f32] {
        match self {
            Font::Smooth => &SMOOTH_SIZES,
            Font::Pixel => &PIXEL_SIZES,
        }
    }

    /// Read back from `settings.toml`, where an older or hand-edited file
    /// can say anything.
    pub fn parse(text: &str) -> Option<Font> {
        Font::ALL
            .into_iter()
            .find(|font| font.label().eq_ignore_ascii_case(text))
    }

    pub fn next(self) -> Font {
        match self {
            Font::Smooth => Font::Pixel,
            Font::Pixel => Font::Smooth,
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
        }
        assert_eq!(Font::parse("Smooth"), Some(Font::Smooth), "case is ignored");
        assert_eq!(
            Font::parse("Courier"),
            None,
            "an unknown name is not a font"
        );
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
