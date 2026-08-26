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

    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);
    slint_build::compile_with_config("ui/degauss.slint", config)
        .expect("compiling ui/degauss.slint");
}
