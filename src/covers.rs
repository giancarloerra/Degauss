//! Cover art: decode once, scale once, keep the result.
//!
//! Scroll has to stay smooth, so no PNG is decoded while the list is
//! moving. Art is decoded on demand, immediately reduced to the size it
//! will actually be drawn at, and cached. What the cache
//! cannot hold is reported rather than silently re-decoded every frame.
//!
//! Downscaling takes one source pixel per destination pixel. It is the
//! cheapest thing that fits the frame budget on this hardware, which is what
//! keeps artwork arriving while the list is moving.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::error::{DegaussError, Result};

/// Decoded, scaled artwork: 8-bit RGB, tightly packed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbImage {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl RgbImage {
    pub fn new(width: u32, height: u32, rgb: Vec<u8>) -> Result<Self> {
        let expected = width as usize * height as usize * 3;
        if rgb.len() != expected {
            return Err(DegaussError::unsupported(
                "image buffer",
                format!(
                    "{}x{} needs {expected} bytes, got {}",
                    width,
                    height,
                    rgb.len()
                ),
            ));
        }
        Ok(RgbImage { width, height, rgb })
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * self.width + x) * 3) as usize;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CoverStats {
    pub decoded: u64,
    pub cache_hits: u64,
    pub evictions: u64,
    pub failures: u64,
    pub decode_us_total: u64,
    pub scale_us_total: u64,
    pub worst_decode_us: u64,
    pub bytes_held: usize,
}

impl CoverStats {
    pub fn avg_decode_us(&self) -> u64 {
        self.decode_us_total.checked_div(self.decoded).unwrap_or(0)
    }
}

/// Decode artwork and reduce it so its longest edge is at most `max_edge`.
///
/// `ground` is the colour transparent pixels are composited onto, which is
/// whatever the picture is about to be drawn on.
/// The largest artwork file worth opening. Scraped screenshots and covers
/// run to a few hundred kilobytes; this is far above that and far below what
/// would exhaust the board.
const MAX_COVER_BYTES: u64 = 32 * 1024 * 1024;

/// Read a picture, decode it and shrink it to fit.
pub fn load_scaled(
    path: &Path,
    max_edge: u32,
    ground: [u8; 3],
    stats: &mut CoverStats,
) -> Result<RgbImage> {
    // Scraped artwork is tens of kilobytes. A file orders of magnitude
    // larger is a mistake or a stray download, and decoding it would take
    // memory this board does not have to spare. Refusing it costs one
    // missing picture; running out of memory costs the whole menu.
    let size = std::fs::metadata(path)
        .map_err(|e| DegaussError::io("reading cover", path, e))?
        .len();
    if size > MAX_COVER_BYTES {
        return Err(DegaussError::unsupported(
            "cover",
            format!(
                "{} is {size} bytes, past the {MAX_COVER_BYTES} limit",
                path.display()
            ),
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| DegaussError::io("reading cover", path, e))?;

    let started = Instant::now();
    let full = decode(&bytes, path, ground)?;
    let decode_us = started.elapsed().as_micros() as u64;

    let started = Instant::now();
    let scaled = scale_to_fit(&full, max_edge);
    let scale_us = started.elapsed().as_micros() as u64;

    stats.decoded += 1;
    stats.decode_us_total += decode_us;
    stats.scale_us_total += scale_us;
    stats.worst_decode_us = stats.worst_decode_us.max(decode_us);
    Ok(scaled)
}

/// Decode artwork into 8-bit RGB.
///
/// Scraped libraries are not all PNG: a real card holds hundreds of JPEGs
/// alongside them, and a frontend that only reads one format shows gaps
/// wherever the other one is used. The format is decided by what the file
/// actually starts with, not by its name, because a mislabelled extension
/// is common in scraped art.
pub fn decode(bytes: &[u8], origin: &Path, ground: [u8; 3]) -> Result<RgbImage> {
    const JPEG_MAGIC: [u8; 2] = [0xff, 0xd8];
    if bytes.starts_with(&JPEG_MAGIC) {
        // JPEG has no transparency, so there is nothing to composite.
        return decode_jpeg(bytes, origin);
    }
    decode_png(bytes, origin, ground)
}

/// Decode a JPEG into 8-bit RGB.
fn decode_jpeg(bytes: &[u8], origin: &Path) -> Result<RgbImage> {
    use zune_jpeg::zune_core::colorspace::ColorSpace;

    // Same reader trait as the PNG side: a plain slice cannot seek.
    let source = zune_jpeg::zune_core::bytestream::ZCursor::new(bytes);
    let mut decoder = zune_jpeg::JpegDecoder::new(source);
    let pixels = decoder
        .decode()
        .map_err(|e| DegaussError::malformed("cover JPEG", origin, e.to_string()))?;
    let (width, height) = decoder
        .dimensions()
        .ok_or_else(|| DegaussError::malformed("cover JPEG", origin, "no dimensions"))?;

    let rgb = match decoder.output_colorspace() {
        Some(ColorSpace::RGB) => pixels,
        Some(ColorSpace::Luma) => {
            let mut out = Vec::with_capacity(pixels.len() * 3);
            for &v in &pixels {
                out.extend_from_slice(&[v, v, v]);
            }
            out
        }
        other => {
            return Err(DegaussError::unsupported(
                "cover JPEG colourspace",
                format!("{other:?} in {}", origin.display()),
            ))
        }
    };

    RgbImage::new(width as u32, height as u32, rgb)
}

/// Source-over compositing of one pixel, in 8-bit integers.
///
/// Rounded rather than truncated: a fully opaque pixel must come out exactly
/// as it went in, or artwork drawn edge to edge loses a level of every
/// channel and photographic covers pick up a visible cast.
fn over(rgb: &[u8], alpha: u8, ground: [u8; 3]) -> [u8; 3] {
    match alpha {
        255 => [rgb[0], rgb[1], rgb[2]],
        0 => ground,
        a => {
            let a = a as u32;
            let inverse = 255 - a;
            let mix = |top: u8, bottom: u8| -> u8 {
                ((top as u32 * a + bottom as u32 * inverse + 127) / 255) as u8
            };
            [
                mix(rgb[0], ground[0]),
                mix(rgb[1], ground[1]),
                mix(rgb[2], ground[2]),
            ]
        }
    }
}

/// Decode a PNG into 8-bit RGB, whatever colour type it uses, compositing
/// any transparency onto `ground`.
///
/// Dropping alpha instead is a trap that looks harmless and is not. A
/// platform logo is typically drawn as one flat colour with the entire shape
/// carried in the alpha channel: every palette entry white, the letters and
/// the background separated only by opacity. Discard alpha there and every
/// logo on the screen becomes an identical solid white rectangle.
pub fn decode_png(bytes: &[u8], origin: &Path, ground: [u8; 3]) -> Result<RgbImage> {
    use zune_png::zune_core::colorspace::ColorSpace;
    use zune_png::zune_core::options::DecoderOptions;

    // 8-bit output keeps 16-bit PNGs from doubling every buffer.
    let options = DecoderOptions::default().png_set_strip_to_8bit(true);
    // ZCursor is zune's own no-std reader; a plain &[u8] does not satisfy
    // the reader trait because it cannot seek.
    let source = zune_png::zune_core::bytestream::ZCursor::new(bytes);
    let mut decoder = zune_png::PngDecoder::new_with_options(source, options);
    let pixels = decoder
        .decode()
        .map_err(|e| DegaussError::malformed("cover PNG", origin, e.to_string()))?;

    let (width, height) = decoder
        .dimensions()
        .ok_or_else(|| DegaussError::malformed("cover PNG", origin, "no dimensions"))?;
    let colorspace = decoder
        .colorspace()
        .ok_or_else(|| DegaussError::malformed("cover PNG", origin, "no colourspace"))?;

    let flat = match pixels {
        zune_png::zune_core::result::DecodingResult::U8(v) => v,
        _ => {
            return Err(DegaussError::unsupported(
                "cover PNG depth",
                format!("{} did not decode as 8-bit", origin.display()),
            ))
        }
    };

    let pixel_count = width * height;
    let rgb = match colorspace {
        ColorSpace::RGB => flat,
        ColorSpace::RGBA => {
            let mut out = Vec::with_capacity(pixel_count * 3);
            for px in flat.as_chunks::<4>().0 {
                out.extend_from_slice(&over(&px[..3], px[3], ground));
            }
            out
        }
        ColorSpace::Luma => {
            let mut out = Vec::with_capacity(pixel_count * 3);
            for &v in &flat {
                out.extend_from_slice(&[v, v, v]);
            }
            out
        }
        ColorSpace::LumaA => {
            let mut out = Vec::with_capacity(pixel_count * 3);
            for px in flat.as_chunks::<2>().0 {
                out.extend_from_slice(&over(&[px[0]; 3], px[1], ground));
            }
            out
        }
        other => {
            return Err(DegaussError::unsupported(
                "cover PNG colourspace",
                format!("{other:?} in {}", origin.display()),
            ))
        }
    };

    RgbImage::new(width as u32, height as u32, rgb)
}

/// Shrink to fit, taking one source pixel per destination pixel. Returns the
/// input untouched when it already fits, because upscaling art on a 240-line
/// display only wastes memory.
///
/// Sampling rather than averaging the block is deliberate: averaging costs
/// noticeably more per screenshot and shows up on the worst frame of a
/// scroll. On a 352 by 240 screen the harder edges it buys are not worth the
/// stutter.
pub fn scale_to_fit(source: &RgbImage, max_edge: u32) -> RgbImage {
    let longest = source.width.max(source.height);
    if longest <= max_edge || max_edge == 0 {
        return source.clone();
    }

    let scale = max_edge as f32 / longest as f32;
    let width = ((source.width as f32 * scale).round() as u32).max(1);
    let height = ((source.height as f32 * scale).round() as u32).max(1);

    let mut out = vec![0u8; (width * height * 3) as usize];
    for y in 0..height {
        let sy = ((y as u64 * source.height as u64 / height as u64) as u32).min(source.height - 1);
        for x in 0..width {
            let sx = ((x as u64 * source.width as u64 / width as u64) as u32).min(source.width - 1);
            let px = source.pixel(sx, sy);
            let i = ((y * width + x) * 3) as usize;
            out[i] = px[0];
            out[i + 1] = px[1];
            out[i + 2] = px[2];
        }
    }

    RgbImage {
        width,
        height,
        rgb: out,
    }
}

/// Fixed-capacity cover cache with least-recently-used eviction.
pub struct CoverCache {
    max_edge: u32,
    capacity: usize,
    /// What transparent artwork is composited onto: the colour it is drawn
    /// on. Part of the cache key by construction, since changing the theme
    /// rebuilds the cache.
    ground: [u8; 3],
    images: HashMap<PathBuf, RgbImage>,
    /// Most recently used last.
    order: Vec<PathBuf>,
    /// Paths that failed to load, so a broken file is attempted once and
    /// then reported, never retried every frame.
    failed: HashMap<PathBuf, String>,
    pub stats: CoverStats,
}

impl CoverCache {
    pub fn new(max_edge: u32, capacity: usize, ground: [u8; 3]) -> Self {
        CoverCache {
            max_edge,
            capacity: capacity.max(1),
            ground,
            images: HashMap::new(),
            order: Vec::new(),
            failed: HashMap::new(),
            stats: CoverStats::default(),
        }
    }

    /// Art for a path, decoding it the first time. `None` means it already
    /// failed once; the reason is kept in [`CoverCache::failure`].
    pub fn get(&mut self, path: &Path) -> Option<&RgbImage> {
        if self.failed.contains_key(path) {
            return None;
        }
        if self.images.contains_key(path) {
            self.stats.cache_hits += 1;
            self.touch(path);
            return self.images.get(path);
        }

        let mut stats = self.stats;
        match load_scaled(path, self.max_edge, self.ground, &mut stats) {
            Ok(image) => {
                self.stats = stats;
                self.insert(path.to_path_buf(), image);
                self.images.get(path)
            }
            Err(e) => {
                self.stats.failures += 1;
                self.failed.insert(path.to_path_buf(), e.to_string());
                None
            }
        }
    }

    /// Every cover that could not be loaded, with the reason. Reported at
    /// the end of a run: a card with broken art should say so by name
    /// rather than just showing gaps.
    pub fn failures(&self) -> impl Iterator<Item = (&Path, &str)> {
        self.failed
            .iter()
            .map(|(path, reason)| (path.as_path(), reason.as_str()))
    }

    fn touch(&mut self, path: &Path) {
        if let Some(pos) = self.order.iter().position(|p| p == path) {
            let owned = self.order.remove(pos);
            self.order.push(owned);
        }
    }

    fn insert(&mut self, path: PathBuf, image: RgbImage) {
        while self.images.len() >= self.capacity {
            if self.order.is_empty() {
                break;
            }
            let oldest = self.order.remove(0);
            if let Some(dropped) = self.images.remove(&oldest) {
                self.stats.bytes_held = self.stats.bytes_held.saturating_sub(dropped.rgb.len());
                self.stats.evictions += 1;
            }
        }
        self.stats.bytes_held += image.rgb.len();
        self.images.insert(path.clone(), image);
        self.order.push(path);
    }

    /// How many covers are currently held. Reported at the end of a run,
    /// alongside the eviction count, so the cache size can be judged.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn len(&self) -> usize {
        self.images.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ground colour no fixture uses, so any pixel that comes back as
    /// this one arrived through the transparent path.
    const OPAQUE: [u8; 3] = [0x6c, 0x6c, 0x6c];

    // 2x2 indexed PNG shaped exactly like a real platform logo: every
    // palette entry is white and the shape is carried entirely by tRNS.
    // Diagonal: transparent, opaque, opaque, transparent.
    const PNG_LOGO: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x03, 0x00, 0x00, 0x00, 0x45,
        0x68, 0xfd, 0x16, 0x00, 0x00, 0x00, 0x06, 0x50, 0x4c, 0x54, 0x45, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff, 0x55, 0x7c, 0xf5, 0x6c, 0x00, 0x00, 0x00, 0x02, 0x74, 0x52, 0x4e, 0x53, 0x00,
        0xff, 0x5b, 0x91, 0x22, 0xb5, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c,
        0x63, 0x60, 0x60, 0x04, 0x42, 0x00, 0x00, 0x0c, 0x00, 0x03, 0x2b, 0x63, 0xcb, 0x50, 0x00,
        0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn a_logo_whose_shape_is_only_in_its_transparency_is_not_a_white_square() {
        // This is what every platform logo on the card looks like: one flat
        // colour, all of it white, with the artwork existing only as alpha.
        // Discarding alpha turns the entire set into identical white
        // rectangles, which is exactly what it did.
        let image = decode(PNG_LOGO, Path::new("logo.png"), OPAQUE).expect("decodes");

        assert_eq!(
            image.pixel(0, 0),
            OPAQUE,
            "transparent corner takes the ground"
        );
        assert_eq!(
            image.pixel(1, 0),
            [0xff, 0xff, 0xff],
            "opaque corner is white"
        );
        assert_eq!(image.pixel(0, 1), [0xff, 0xff, 0xff]);
        assert_eq!(image.pixel(1, 1), OPAQUE);

        let corners = [
            image.pixel(0, 0),
            image.pixel(1, 0),
            image.pixel(0, 1),
            image.pixel(1, 1),
        ];
        assert!(
            corners.iter().any(|px| px != &[0xff, 0xff, 0xff]),
            "a logo that comes back all white is the bug this guards"
        );
    }

    #[test]
    fn compositing_leaves_opaque_artwork_untouched() {
        // Rounding matters: photographic covers are opaque edge to edge, and
        // a compositing path that loses a level per channel tints all of it.
        let plain = decode(PNG_2X2, Path::new("fixture.png"), OPAQUE).expect("decodes");
        let other = decode(PNG_2X2, Path::new("fixture.png"), [0, 0, 0]).expect("decodes");
        for y in 0..2 {
            for x in 0..2 {
                assert_eq!(
                    plain.pixel(x, y),
                    other.pixel(x, y),
                    "opaque art must not depend on what is behind it"
                );
            }
        }
    }

    // 2x2 RGB PNG: red, green / blue, white.
    const PNG_2X2: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x02, 0x00, 0x00, 0x00, 0xfd,
        0xd4, 0x9a, 0x73, 0x00, 0x00, 0x00, 0x12, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0xf8,
        0xcf, 0xc0, 0xc0, 0x00, 0xc2, 0x0c, 0xff, 0x81, 0x00, 0x00, 0x1f, 0xee, 0x05, 0xfb, 0xf1,
        0xab, 0xba, 0x77, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    /// A 16x16 JPEG: red top-left, blue bottom-right, green elsewhere.
    /// Big enough that chroma subsampling does not average the colour
    /// away, which a 2x2 fixture does.
    const JPEG_16: &[u8] = &[
        0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 0x4a, 0x46, 0x49, 0x46, 0x00, 0x01, 0x01, 0x00, 0x00,
        0x48, 0x00, 0x48, 0x00, 0x00, 0xff, 0xe1, 0x00, 0x4c, 0x45, 0x78, 0x69, 0x66, 0x00, 0x00,
        0x4d, 0x4d, 0x00, 0x2a, 0x00, 0x00, 0x00, 0x08, 0x00, 0x01, 0x87, 0x69, 0x00, 0x04, 0x00,
        0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x1a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xa0, 0x01,
        0x00, 0x03, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0xa0, 0x02, 0x00, 0x04, 0x00,
        0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x10, 0xa0, 0x03, 0x00, 0x04, 0x00, 0x00, 0x00, 0x01,
        0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0xff, 0xed, 0x00, 0x38, 0x50, 0x68, 0x6f,
        0x74, 0x6f, 0x73, 0x68, 0x6f, 0x70, 0x20, 0x33, 0x2e, 0x30, 0x00, 0x38, 0x42, 0x49, 0x4d,
        0x04, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x38, 0x42, 0x49, 0x4d, 0x04, 0x25, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x10, 0xd4, 0x1d, 0x8c, 0xd9, 0x8f, 0x00, 0xb2, 0x04, 0xe9, 0x80,
        0x09, 0x98, 0xec, 0xf8, 0x42, 0x7e, 0xff, 0xc0, 0x00, 0x11, 0x08, 0x00, 0x10, 0x00, 0x10,
        0x03, 0x01, 0x22, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01, 0xff, 0xc4, 0x00, 0x1f, 0x00,
        0x00, 0x01, 0x05, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0xff, 0xc4,
        0x00, 0xb5, 0x10, 0x00, 0x02, 0x01, 0x03, 0x03, 0x02, 0x04, 0x03, 0x05, 0x05, 0x04, 0x04,
        0x00, 0x00, 0x01, 0x7d, 0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41,
        0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42,
        0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0, 0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17,
        0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39,
        0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58,
        0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77,
        0x78, 0x79, 0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95,
        0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2,
        0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8,
        0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2, 0xe3, 0xe4,
        0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9,
        0xfa, 0xff, 0xc4, 0x00, 0x1f, 0x01, 0x00, 0x03, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
        0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07,
        0x08, 0x09, 0x0a, 0x0b, 0xff, 0xc4, 0x00, 0xb5, 0x11, 0x00, 0x02, 0x01, 0x02, 0x04, 0x04,
        0x03, 0x04, 0x07, 0x05, 0x04, 0x04, 0x00, 0x01, 0x02, 0x77, 0x00, 0x01, 0x02, 0x03, 0x11,
        0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71, 0x13, 0x22, 0x32, 0x81,
        0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0, 0x15, 0x62, 0x72,
        0xd1, 0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26, 0x27, 0x28,
        0x29, 0x2a, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
        0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
        0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86,
        0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3,
        0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9,
        0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6,
        0xd7, 0xd8, 0xd9, 0xda, 0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf2, 0xf3,
        0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa, 0xff, 0xdb, 0x00, 0x43, 0x00, 0x02, 0x02, 0x02,
        0x02, 0x02, 0x02, 0x03, 0x02, 0x02, 0x03, 0x05, 0x03, 0x03, 0x03, 0x05, 0x06, 0x05, 0x05,
        0x05, 0x05, 0x06, 0x08, 0x06, 0x06, 0x06, 0x06, 0x06, 0x08, 0x0a, 0x08, 0x08, 0x08, 0x08,
        0x08, 0x08, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0a, 0x0c, 0x0c, 0x0c, 0x0c, 0x0c,
        0x0c, 0x0e, 0x0e, 0x0e, 0x0e, 0x0e, 0x0f, 0x0f, 0x0f, 0x0f, 0x0f, 0x0f, 0x0f, 0x0f, 0x0f,
        0x0f, 0xff, 0xdb, 0x00, 0x43, 0x01, 0x02, 0x02, 0x02, 0x04, 0x04, 0x04, 0x07, 0x04, 0x04,
        0x07, 0x10, 0x0b, 0x09, 0x0b, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10,
        0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10,
        0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10,
        0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0xff, 0xdd, 0x00, 0x04, 0x00,
        0x01, 0xff, 0xda, 0x00, 0x0c, 0x03, 0x01, 0x00, 0x02, 0x11, 0x03, 0x11, 0x00, 0x3f, 0x00,
        0xf8, 0xbe, 0xbd, 0x22, 0x8a, 0xfc, 0xfb, 0xaf, 0x27, 0xe8, 0xcb, 0xf4, 0x65, 0xff, 0x00,
        0x88, 0x9d, 0xf5, 0xff, 0x00, 0xf6, 0xff, 0x00, 0xaa, 0xfd, 0x57, 0xd9, 0xff, 0x00, 0xcb,
        0xbf, 0x6b, 0xcd, 0xed, 0x7d, 0xa7, 0xfd, 0x3c, 0xa7, 0xcb, 0xcb, 0xec, 0xfc, 0xef, 0x7e,
        0x96, 0xd7, 0xc3, 0xfa, 0x43, 0x7d, 0x21, 0xbf, 0xe2, 0x31, 0x7d, 0x4b, 0xfd, 0x8b, 0xea,
        0x5f, 0x52, 0xf6, 0x9f, 0xf2, 0xf3, 0xdb, 0x73, 0xfb, 0x6e, 0x4f, 0xee, 0x52, 0xe5, 0xe5,
        0xf6, 0x5f, 0xde, 0xbf, 0x37, 0x4b, 0x6b, 0xff, 0xd9,
    ];

    fn solid(width: u32, height: u32, colour: [u8; 3]) -> RgbImage {
        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        for _ in 0..width * height {
            rgb.extend_from_slice(&colour);
        }
        RgbImage::new(width, height, rgb).unwrap()
    }

    #[test]
    fn a_real_png_decodes_to_the_expected_pixels() {
        let image = decode(PNG_2X2, Path::new("fixture.png"), OPAQUE).expect("decodes");
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(image.pixel(0, 0), [255, 0, 0]);
        assert_eq!(image.pixel(1, 0), [0, 255, 0]);
        assert_eq!(image.pixel(0, 1), [0, 0, 255]);
        assert_eq!(image.pixel(1, 1), [255, 255, 255]);
    }

    #[test]
    fn a_truncated_png_is_an_error_carrying_the_file_name() {
        let err = decode(&PNG_2X2[..40], Path::new("broken.png"), OPAQUE)
            .expect_err("truncated data must not decode");
        assert!(err.to_string().contains("broken.png"), "got: {err}");
    }

    #[test]
    fn a_real_jpeg_decodes_too() {
        // A scraped library holds hundreds of JPEGs alongside its PNGs;
        // reading only one format leaves gaps wherever the other is used.
        let image = decode(JPEG_16, Path::new("fixture.jpg"), OPAQUE).expect("decodes");
        assert_eq!((image.width, image.height), (16, 16));
        // JPEG is lossy, so colours are approximate rather than exact.
        let red = image.pixel(3, 3);
        assert!(
            red[0] > 150 && red[2] < 110,
            "top left should read as red: {red:?}"
        );
        let blue = image.pixel(12, 12);
        assert!(
            blue[2] > 150 && blue[0] < 110,
            "bottom right should read as blue: {blue:?}"
        );
    }

    #[test]
    fn the_format_comes_from_the_bytes_not_the_extension() {
        // Scraped art is routinely mislabelled.
        let image = decode(JPEG_16, Path::new("actually-a-jpeg.png"), OPAQUE).expect("decodes");
        assert_eq!((image.width, image.height), (16, 16));
    }

    #[test]
    fn downscaling_keeps_every_quadrant_of_the_picture() {
        // Four quadrants of solid colour reduced to 2x2: one pixel per
        // quadrant, so no part of the picture is dropped altogether.
        let mut rgb = Vec::new();
        for y in 0..4u32 {
            for x in 0..4u32 {
                let colour = match (x / 2, y / 2) {
                    (0, 0) => [200, 0, 0],
                    (1, 0) => [0, 200, 0],
                    (0, 1) => [0, 0, 200],
                    _ => [100, 100, 100],
                };
                rgb.extend_from_slice(&colour);
            }
        }
        let source = RgbImage::new(4, 4, rgb).unwrap();
        let small = scale_to_fit(&source, 2);

        assert_eq!((small.width, small.height), (2, 2));
        assert_eq!(small.pixel(0, 0), [200, 0, 0]);
        assert_eq!(small.pixel(1, 0), [0, 200, 0]);
        assert_eq!(small.pixel(0, 1), [0, 0, 200]);
        assert_eq!(small.pixel(1, 1), [100, 100, 100]);
    }

    #[test]
    fn a_mixed_block_takes_a_pixel_rather_than_the_mean() {
        // Deliberate, and measured: averaging cost a millisecond and a
        // third per screenshot and 22 ms on the worst frame of a scroll.
        // If this ever reads 150 again, that decision was undone by
        // accident.
        let mut rgb = vec![0u8; 6];
        rgb[0] = 100;
        rgb[3] = 200;
        let source = RgbImage::new(2, 1, rgb).unwrap();
        let small = scale_to_fit(&source, 1);
        assert_eq!(small.pixel(0, 0)[0], 100, "sampled, not averaged");
    }

    #[test]
    fn art_smaller_than_the_target_is_left_alone() {
        let source = solid(8, 8, [1, 2, 3]);
        let out = scale_to_fit(&source, 64);
        assert_eq!(out, source, "upscaling would only waste memory");
    }

    #[test]
    fn non_square_art_keeps_its_aspect_ratio() {
        let source = solid(400, 100, [9, 9, 9]);
        let out = scale_to_fit(&source, 100);
        assert_eq!((out.width, out.height), (100, 25));
    }

    #[test]
    fn the_cache_evicts_least_recently_used_and_counts_it() {
        let dir = std::env::temp_dir().join(format!("degauss-covers-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths: Vec<PathBuf> = (0..3)
            .map(|i| {
                let p = dir.join(format!("c{i}.png"));
                std::fs::write(&p, PNG_2X2).unwrap();
                p
            })
            .collect();

        let mut cache = CoverCache::new(64, 2, OPAQUE);
        assert!(cache.get(&paths[0]).is_some());
        assert!(cache.get(&paths[1]).is_some());
        // Touch 0 so 1 becomes the oldest.
        assert!(cache.get(&paths[0]).is_some());
        assert_eq!(cache.stats.cache_hits, 1);
        assert!(cache.get(&paths[2]).is_some());

        assert_eq!(cache.len(), 2);
        assert_eq!(cache.stats.evictions, 1);
        assert_eq!(cache.stats.decoded, 3);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_broken_cover_is_attempted_once_and_then_reported() {
        // Retrying a broken file on every frame would look like a scrolling
        // performance problem instead of a data problem.
        let dir = std::env::temp_dir().join(format!("degauss-broken-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("broken.png");
        std::fs::write(&path, b"not a png").unwrap();

        let mut cache = CoverCache::new(64, 4, OPAQUE);
        assert!(cache.get(&path).is_none());
        assert!(cache.get(&path).is_none());
        assert_eq!(cache.stats.failures, 1, "one attempt, then remembered");
        let reported: Vec<_> = cache.failures().collect();
        assert_eq!(reported.len(), 1);
        assert_eq!(
            reported[0].0,
            path.as_path(),
            "the failing file must be named"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
