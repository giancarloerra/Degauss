// The pixel sizes each typeface is baked at. Included by both `build.rs`,
// which bakes the glyphs, and `font.rs`, which asks for them: a size asked
// for but never baked is drawn at whatever smaller size was, silently.

/// DejaVu Sans. An outline font is legible at any size, so these are only
/// far enough apart to cover a 240 line screen up to 1080p without carrying
/// glyphs nobody looks at.
///
/// Frozen. Every size here is one that shipped, and a build renders a screen
/// the same way it did the day it was made.
pub const SMOOTH_SIZES: [f32; 5] = [12.0, 26.0, 34.0, 44.0, 56.0];

/// Px437 DOS/V re. JPN12, whose cell is 6 by 12 on a 1200 unit em: 600 units
/// of advance is half an em, so a 6 pixel cell means a 12 pixel em, and every
/// coordinate in the font is a whole number of pixels at 12 and at multiples
/// of it.
///
/// Anything between those lands mid-pixel and the renderer resolves it with
/// grey, which is the one thing a pixel font exists to avoid: at 26 pixels
/// the same vertical stem is two pixels wide in one letter and three in the
/// next.
pub const PIXEL_SIZES: [f32; 4] = [12.0, 24.0, 36.0, 48.0];

/// List and menu text sizes for each existing smooth-font rung. The middle
/// column is the released size, so themes without a text-size choice render
/// exactly as before. Rows correspond to SMOOTH_SIZES above.
pub const SMOOTH_TEXT_SIZES: [[f32; 5]; 5] = [
    [8.0, 10.0, 12.0, 14.0, 16.0],
    [18.0, 22.0, 26.0, 30.0, 35.0],
    [23.0, 28.0, 34.0, 40.0, 45.0],
    [29.0, 37.0, 44.0, 51.0, 59.0],
    [37.0, 47.0, 56.0, 65.0, 75.0],
];

/// The pixel fonts have dedicated grid-aligned faces at these base sizes.
/// A larger screen uses the same face at two, three or four times its base.
pub const PIXEL_TEXT_BASE_SIZES: [f32; 5] = [8.0, 10.0, 12.0, 14.0, 16.0];
