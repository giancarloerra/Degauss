//! Register the optional list-size glyphs on the existing software window.
//!
//! Each tiny Slint component embeds only its own sizes. Creating and dropping
//! it registers those fonts without adding a font loader to MiSTer.

mod smooth_smaller {
    include!(concat!(env!("OUT_DIR"), "/smooth_smaller.rs"));
}
mod pixel_smaller {
    include!(concat!(env!("OUT_DIR"), "/pixel_smaller.rs"));
}
mod smooth_small {
    include!(concat!(env!("OUT_DIR"), "/smooth_small.rs"));
}
mod pixel_small {
    include!(concat!(env!("OUT_DIR"), "/pixel_small.rs"));
}
mod smooth_large {
    include!(concat!(env!("OUT_DIR"), "/smooth_large.rs"));
}
mod pixel_large {
    include!(concat!(env!("OUT_DIR"), "/pixel_large.rs"));
}
mod smooth_larger {
    include!(concat!(env!("OUT_DIR"), "/smooth_larger.rs"));
}
mod pixel_larger {
    include!(concat!(env!("OUT_DIR"), "/pixel_larger.rs"));
}

pub fn register() -> Result<(), slint::PlatformError> {
    let _ = smooth_smaller::FontPack::new()?;
    let _ = pixel_smaller::FontPack::new()?;
    let _ = smooth_small::FontPack::new()?;
    let _ = pixel_small::FontPack::new()?;
    let _ = smooth_large::FontPack::new()?;
    let _ = pixel_large::FontPack::new()?;
    let _ = smooth_larger::FontPack::new()?;
    let _ = pixel_larger::FontPack::new()?;
    Ok(())
}
