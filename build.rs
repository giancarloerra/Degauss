include!("src/font_sizes.rs");

fn main() {
    // Resources and glyphs are baked into the binary: the software renderer
    // has no system font stack to fall back on, and the MiSTer has no fonts
    // installed that we would want to depend on anyway.
    //
    // The typeface is named, not inherited from whatever the machine doing
    // the build happens to have installed. Without this, Slint asks the host
    // for a default: a Mac produced Helvetica and the CI runner produced
    // DejaVu Sans, so the same commit shipped two different-looking builds
    // and a release never matched a local one.
    //
    // Naming it here also makes it the first font registered, which is the
    // one the renderer falls back to, so a build where something went wrong
    // is set in the right typeface rather than in whichever other one.
    println!("cargo:rerun-if-changed=src/font_sizes.rs");
    println!("cargo:rerun-if-changed=assets/fonts");
    std::env::set_var(
        "SLINT_DEFAULT_FONT",
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/fonts/DejaVuSans.ttf"),
    );

    // Glyphs are baked as bitmaps, one set per pixel size, and only the sizes
    // named here exist at runtime. There is one list for both typefaces, so
    // it is the union of the two ladders and each carries sizes it will never
    // be asked for; `Font::quantise` is what keeps each typeface to its own.
    let mut sizes: Vec<f32> = SMOOTH_SIZES
        .iter()
        .chain(PIXEL_SIZES.iter())
        .copied()
        .collect();
    sizes.sort_by(f32::total_cmp);
    sizes.dedup();
    let sizes: Vec<String> = sizes.iter().map(f32::to_string).collect();
    std::env::set_var("SLINT_FONT_SIZES", sizes.join(","));

    // Which characters get glyphs, written out for the compiler to read.
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    std::fs::write(out.join("charset.slint"), charset_slint()).expect("writing charset.slint");

    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer)
        .with_include_paths(vec![out]);
    slint_build::compile_with_config("ui/degauss.slint", config)
        .expect("compiling ui/degauss.slint");
}

/// The scripts a name on a card can be spelled in.
///
/// exFAT keeps names as UTF-16 and takes any character there is, so this is
/// not what the card permits. It is what a name is written in: the Latin
/// alphabet and every accent put on it, and the two other alphabets that
/// releases and fan translations come in.
///
/// Japanese and Chinese are the gap, and no list can close it: DejaVu Sans
/// has no kana and no han, so there is nothing to bake. A name in either
/// draws as a gap until a typeface that has them is added.
///
/// Offering a range does not cost anything for the parts of it neither font
/// has: the compiler drops a character no font can draw rather than carrying
/// an empty glyph for it.
const SCRIPTS: [(u32, u32); 16] = [
    (0x0020, 0x007E), // ASCII, which everything else is written around.
    (0x00A0, 0x00FF), // Latin-1: French, German, Spanish, Italian, Nordic.
    (0x0100, 0x017F), // Latin Extended-A: Polish, Czech, Hungarian, Turkish.
    (0x0180, 0x024F), // Latin Extended-B: Romanian, and the rest of Latin.
    (0x02B0, 0x02FF), // Modifier letters, which turn up inside names.
    (0x0300, 0x036F), // Combining accents: a name copied from a Mac is
    // written in them rather than in the single characters above.
    (0x0370, 0x03FF), // Greek.
    (0x0400, 0x04FF), // Cyrillic, which the Russian translations are in.
    (0x1E00, 0x1EFF), // Latin Extended Additional: Vietnamese.
    (0x2000, 0x206F), // Dashes, the quotation marks that are not typewriter
    // marks, and the ellipsis that ends an elided name.
    (0x2070, 0x209F), // Raised and lowered digits.
    (0x20A0, 0x20BF), // Currency, for the prices in a title.
    (0x2100, 0x214F), // Trademark, numero, and the rest.
    (0x2150, 0x218F), // Fractions and roman numerals.
    (0x2190, 0x21FF), // Arrows, which the key hints are drawn with.
    (0x2200, 0x22FF), // Mathematical signs, for the ones in a title.
];

/// A Slint file naming every character worth a glyph.
///
/// Glyphs are baked at build time and there is no font on the machine to
/// reach for at runtime: the renderer is built without the feature that
/// would load one, and a MiSTer has no font stack to load from anyway. A
/// character whose glyph was not baked is not drawn as a box or a question
/// mark. It is drawn as nothing, and a name with a gap in it looks like a
/// name with a space in it.
///
/// A name comes off the card, and the card is exFAT, which stores names as
/// UTF-16 and accepts every character there is. So rather than keep a list
/// of the ones worth having, which is a list somebody has to remember to
/// add to, the whole plane is offered here and the compiler keeps what the
/// typefaces actually have: a character no embedded font can draw is
/// dropped rather than carried as an empty glyph. The set follows the font
/// files, and replacing one moves it.
///
/// Both fonts map only the basic plane, so nothing above it is offered.
fn charset_slint() -> String {
    let mut characters = String::new();
    for (first, last) in SCRIPTS {
        for code in first..=last {
            let Some(character) = char::from_u32(code) else {
                continue; // A surrogate half is not a character.
            };
            // Control codes have no glyph. The two separators and the byte
            // order mark would end the string as far as the parser is
            // concerned.
            if character.is_control() || matches!(code, 0x2028 | 0x2029 | 0xFEFF) {
                continue;
            }
            // The two characters that would end the literal early.
            if character == '\\' || character == '"' {
                characters.push('\\');
            }
            characters.push(character);
        }
    }
    format!(
        "// Generated by build.rs. Every character the embedded typefaces can\n\
         // draw; the compiler drops the ones neither of them has.\n\
         export component Charset inherits Text {{\n\
        \x20   visible: false;\n\
        \x20   width: 0px;\n\
        \x20   height: 0px;\n\
        \x20   text: \"{characters}\";\n\
         }}\n"
    )
}
