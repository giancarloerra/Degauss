fn main() {
    // Resources and glyphs are baked into the binary: the software renderer
    // has no system font stack to fall back on, and the MiSTer has no fonts
    // installed that we would want to depend on anyway.
    // Glyphs are baked in as bitmaps, one set per pixel size, and only the
    // sizes named here exist at runtime, so the list has to cover the
    // framebuffers the layout scales text for.
    //
    // 12 is what a 352x240 tube resolves to and it is listed first on
    // purpose. The rest are for larger framebuffers. None of them is a
    // size the tube asks for, and the renderer takes the largest that does
    // not exceed the request, so adding them leaves a tube byte for byte
    // as it was.
    std::env::set_var("SLINT_FONT_SIZES", "12,26,34,44,56");

    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer);
    slint_build::compile_with_config("ui/degauss.slint", config)
        .expect("compiling ui/degauss.slint");
}
