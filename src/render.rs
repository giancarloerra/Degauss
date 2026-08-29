//! Getting Slint's software renderer onto a raw framebuffer.
//!
//! Slint has no backend here: no winit, no GPU, no windowing system.
//! Degauss supplies its own [`slint::platform::Platform`], owns the frame
//! loop, and hands the renderer the framebuffer memory a line at a time.
//!
//! Two paths, chosen from what the device reports:
//!
//! * 16bpp: the renderer writes `Rgb565Pixel` values, which are a
//!   transparent wrapper around `u16` in exactly the layout fbdev expects,
//!   so rows of the mapped framebuffer are handed over directly and nothing
//!   is copied.
//! * 32bpp: the renderer writes into a one-line scratch buffer which is
//!   then copied out with red and blue swapped, because Slint's 32-bit pixel
//!   is R,G,B,A in memory and fbdev's is B,G,R,X.

use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::{
    LineBufferProvider, MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
    Rgb565Pixel, SoftwareRenderer,
};
use slint::platform::{Platform, PlatformError, WindowAdapter};

use crate::error::{DegaussError, Result};
use crate::surface::{Geometry, PixelFormat, Surface};

/// Slint platform with a single window and no event loop of its own.
pub struct DegaussPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
}

impl DegaussPlatform {
    pub fn new(repaint: RepaintBufferType) -> Self {
        DegaussPlatform {
            window: MinimalSoftwareWindow::new(repaint),
            start: Instant::now(),
        }
    }

    pub fn window(&self) -> Rc<MinimalSoftwareWindow> {
        self.window.clone()
    }
}

impl Platform for DegaussPlatform {
    fn create_window_adapter(&self) -> std::result::Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> core::time::Duration {
        self.start.elapsed()
    }

    // run_event_loop is intentionally left at its default (unsupported):
    // Degauss drives frames itself so it can time each one.
}

/// Install the platform and return the window handle. Slint permits this
/// once per process; a second call is a programming error rather than
/// something to paper over.
pub fn install_platform(repaint: RepaintBufferType) -> Result<Rc<MinimalSoftwareWindow>> {
    let platform = DegaussPlatform::new(repaint);
    let window = platform.window();
    slint::platform::set_platform(Box::new(platform))
        .map_err(|e| DegaussError::unsupported("slint platform", e.to_string()))?;
    Ok(window)
}

/// Hands the renderer rows of a 16bpp framebuffer directly.
struct Rgb565Lines<'a> {
    bytes: &'a mut [u8],
    line_length: usize,
}

impl LineBufferProvider for Rgb565Lines<'_> {
    type TargetPixel = Rgb565Pixel;

    fn process_line(
        &mut self,
        line: usize,
        range: core::ops::Range<usize>,
        render_fn: impl FnOnce(&mut [Self::TargetPixel]),
    ) {
        let start = line * self.line_length;
        let row = &mut self.bytes[start..start + self.line_length];
        // Checked rather than assumed: a framebuffer row that is not u16
        // aligned would otherwise be undefined behaviour.
        let pixels: &mut [Rgb565Pixel] =
            bytemuck::try_cast_slice_mut(row).expect("framebuffer row is u16 aligned");
        render_fn(&mut pixels[range]);
    }
}

/// Renders into a scratch line, then copies out in the byte order fbdev
/// wants.
struct Xrgb8888Lines<'a> {
    bytes: &'a mut [u8],
    line_length: usize,
    scratch: Vec<PremultipliedRgbaColor>,
}

impl LineBufferProvider for Xrgb8888Lines<'_> {
    type TargetPixel = PremultipliedRgbaColor;

    fn process_line(
        &mut self,
        line: usize,
        range: core::ops::Range<usize>,
        render_fn: impl FnOnce(&mut [Self::TargetPixel]),
    ) {
        let span = range.len();
        let slice = &mut self.scratch[..span];
        render_fn(slice);

        let row_start = line * self.line_length + range.start * 4;
        let row = &mut self.bytes[row_start..row_start + span * 4];
        for (out, px) in row.as_chunks_mut::<4>().0.iter_mut().zip(slice.iter()) {
            out[0] = px.blue;
            out[1] = px.green;
            out[2] = px.red;
            out[3] = 0xff;
        }
    }
}

/// How a finished frame reaches the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentMode {
    /// Slint draws straight into the mapped framebuffer.
    ///
    /// One copy fewer, but that memory is write-combined: writes stream out
    /// cheaply while reads are uncached. Anything the renderer blends
    /// (text edges, translucent fills) reads the destination pixel back, so
    /// this can be slower than it looks despite doing less work.
    Direct,
    /// Slint draws into ordinary cached RAM; only the rectangles it reports
    /// as changed are copied to the framebuffer afterwards.
    ///
    /// One extra copy, but every blend read hits cache and an unchanged
    /// screen copies nothing at all. The right answer on hardware whose
    /// framebuffer reads are slow; on this one it is not the default.
    Staged,
}

impl PresentMode {
    pub fn label(self) -> &'static str {
        match self {
            PresentMode::Direct => "direct",
            PresentMode::Staged => "staged",
        }
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn next(self) -> Self {
        match self {
            PresentMode::Direct => PresentMode::Staged,
            PresentMode::Staged => PresentMode::Direct,
        }
    }

    /// The inverse of [`PresentMode::label`], so a saved setting can be read
    /// back. Anything else is not a mode and the caller keeps its default.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "direct" => Some(PresentMode::Direct),
            "staged" => Some(PresentMode::Staged),
            _ => None,
        }
    }
}

/// Where a frame's time actually went. Rendering and copying are timed
/// separately because they have different cures.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameWork {
    pub render: Duration,
    pub blit: Duration,
    /// Pixels the renderer reported as changed.
    pub dirty_pixels: u64,
    pub dirty_rects: u32,
}

/// Owns the staging buffer and draws frames in the selected mode.
pub struct Presenter {
    mode: PresentMode,
    staging: Vec<u8>,
    geometry: Geometry,
    /// Set when the next frame must be drawn in full rather than as a
    /// difference against the previous one.
    force_full: bool,
}

impl Presenter {
    pub fn new(geometry: Geometry, mode: PresentMode) -> Self {
        Presenter {
            mode,
            staging: vec![0; geometry.frame_bytes()],
            geometry,
            // Whatever is on the screen at startup was not put there by us.
            force_full: true,
        }
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn mode(&self) -> PresentMode {
        self.mode
    }

    /// Switch modes. The next frame is drawn in full: partial rendering
    /// assumes the target already holds the previous frame, and after a
    /// switch it does not. Without this the screen keeps whatever stale
    /// pixels it had and only the parts that happen to change get updated.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn set_mode(&mut self, mode: PresentMode, window: &MinimalSoftwareWindow) {
        if mode != self.mode {
            self.mode = mode;
            self.staging.fill(0);
            self.force_full = true;
            window.request_redraw();
        }
    }

    /// Draw the next frame in full, whether or not anything is dirty. A
    /// theme change recolours pixels partial rendering considers untouched,
    /// so the difference against the previous frame is not the truth of
    /// what changed.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn force_repaint(&mut self, window: &MinimalSoftwareWindow) {
        self.force_full = true;
        window.request_redraw();
    }

    /// Draw a frame if one is needed. `Ok(None)` means Slint decided nothing
    /// changed, which is not an error and not a frame.
    pub fn draw(
        &mut self,
        window: &MinimalSoftwareWindow,
        surface: &mut dyn Surface,
    ) -> Result<Option<FrameWork>> {
        let geometry = self.geometry;
        let mut error: Option<DegaussError> = None;
        let mut work = FrameWork::default();

        let mode = self.mode;
        // Read, not taken. `draw_if_needed` may decide nothing needs
        // redrawing and never run the closure at all; clearing the request
        // here would drop it, and the next frame that does draw would be a
        // partial one over a screen that needed all of it.
        let force_full = self.force_full;
        let staging = &mut self.staging;

        let drawn = window.draw_if_needed(|renderer| {
            // Slint's own snapshot code uses this pair to force a complete
            // repaint: switching to NewBuffer clears the partial-render
            // caches, and the previous setting is put back afterwards.
            let previous_repaint = renderer.repaint_buffer_type();
            if force_full {
                renderer.set_repaint_buffer_type(RepaintBufferType::NewBuffer);
            }

            let started = Instant::now();
            let target: &mut [u8] = match mode {
                PresentMode::Direct => surface.back_buffer(),
                PresentMode::Staged => staging.as_mut_slice(),
            };

            let region = match render_into(renderer, target, geometry) {
                Ok(region) => region,
                Err(e) => {
                    error = Some(e);
                    return;
                }
            };
            work.render = started.elapsed();
            if force_full {
                renderer.set_repaint_buffer_type(previous_repaint);
            }

            for (origin, size) in region.iter() {
                work.dirty_rects += 1;
                work.dirty_pixels += size.width as u64 * size.height as u64;
                let _ = origin;
            }

            if mode == PresentMode::Staged {
                let started = Instant::now();
                let destination = surface.back_buffer();
                let bpp = geometry.format.bytes_per_pixel();
                for (origin, size) in region.iter() {
                    let x0 = origin.x as usize;
                    let width_bytes = size.width as usize * bpp;
                    for row in origin.y as usize..(origin.y as usize + size.height as usize) {
                        let start = row * geometry.line_length + x0 * bpp;
                        let end = start + width_bytes;
                        if end > destination.len() || end > staging.len() {
                            continue;
                        }
                        destination[start..end].copy_from_slice(&staging[start..end]);
                    }
                }
                work.blit = started.elapsed();
            }
        });

        // Cleared only now that a frame has actually been drawn with it.
        if drawn {
            self.force_full = false;
        }

        if let Some(e) = error {
            return Err(e);
        }
        Ok(drawn.then_some(work))
    }
}

fn render_into(
    renderer: &SoftwareRenderer,
    bytes: &mut [u8],
    geometry: Geometry,
) -> Result<slint::platform::software_renderer::PhysicalRegion> {
    let line_length = geometry.line_length;
    let width = geometry.width as usize;

    Ok(match geometry.format {
        PixelFormat::Rgb565 => {
            if !line_length.is_multiple_of(2) {
                return Err(DegaussError::unsupported(
                    "framebuffer stride",
                    format!("{line_length} bytes is not a whole number of 16-bit pixels"),
                ));
            }
            renderer.render_by_line(Rgb565Lines { bytes, line_length })
        }
        PixelFormat::Xrgb8888 => renderer.render_by_line(Xrgb8888Lines {
            bytes,
            line_length,
            scratch: vec![PremultipliedRgbaColor::default(); width],
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::MemorySurface;
    use slint::{ComponentHandle, PhysicalSize};

    /// Slint accepts one platform per process, and this build is configured
    /// single-threaded, so every renderer assertion lives in one test on one
    /// thread. Splitting them would race on the platform installation rather
    /// than test anything extra.
    #[test]
    fn the_software_renderer_writes_the_bytes_each_framebuffer_format_expects() {
        let window = install_platform(RepaintBufferType::ReusedBuffer).expect("platform installs");
        let ui = crate::DegaussWindow::new().expect("component builds");
        ui.show().expect("shown");

        // 16bpp: the CRT path, rendered straight into framebuffer memory.
        ui.set_c_background(slint::Color::from_rgb_u8(0xff, 0x00, 0x00));
        window.set_size(PhysicalSize::new(16, 8));
        window.request_redraw();
        let mut rgb565 = MemorySurface::new(16, 8, PixelFormat::Rgb565);
        let mut presenter = Presenter::new(rgb565.geometry(), PresentMode::Direct);
        assert!(
            presenter
                .draw(&window, &mut rgb565)
                .expect("frame renders")
                .is_some(),
            "a window asked to redraw must produce a frame"
        );
        assert_eq!(
            u16::from_le_bytes([rgb565.bytes()[0], rgb565.bytes()[1]]),
            0xf800,
            "red must reach the buffer in RGB565 layout"
        );

        // Asking again with nothing changed must do no work: otherwise it
        // would be measuring frames nobody needed.
        assert!(
            presenter
                .draw(&window, &mut rgb565)
                .expect("no-op frame")
                .is_none(),
            "an unchanged window must not redraw"
        );

        // 32bpp: the HDMI path, converted to the byte order fbdev wants.
        ui.set_c_background(slint::Color::from_rgb_u8(0x12, 0x34, 0x56));
        window.set_size(PhysicalSize::new(8, 4));
        window.request_redraw();
        let mut xrgb = MemorySurface::new(8, 4, PixelFormat::Xrgb8888);
        let mut direct = Presenter::new(xrgb.geometry(), PresentMode::Direct);
        direct.draw(&window, &mut xrgb).expect("frame renders");
        assert_eq!(
            &xrgb.bytes()[0..4],
            &[0x56, 0x34, 0x12, 0xff],
            "blue, green, red, padding: fbdev order, not Slint's"
        );

        // The staged path must put exactly the same picture on the screen;
        // if the dirty-rectangle copy were wrong, parts of the frame would
        // simply never arrive.
        let expected = xrgb.bytes().to_vec();
        let mut staged_surface = MemorySurface::new(8, 4, PixelFormat::Xrgb8888);
        let mut staged = Presenter::new(staged_surface.geometry(), PresentMode::Direct);
        // Switching path mid-run is the hazard: partial rendering assumes the
        // target already holds the last frame, which a fresh target does not.
        staged.set_mode(PresentMode::Staged, &window);
        let work = staged
            .draw(&window, &mut staged_surface)
            .expect("staged frame renders")
            .expect("a frame was drawn");
        assert_eq!(
            staged_surface.bytes(),
            expected.as_slice(),
            "staged rendering must reach the screen byte for byte"
        );
        assert!(
            work.dirty_pixels > 0,
            "a full redraw must report dirty pixels"
        );

        // A theme change recolours pixels no property change touched, so
        // the presenter can be told to draw the next frame in full. Nothing
        // about the window changed here: without the forced repaint there
        // would be no frame at all, let alone a complete one.
        staged.force_repaint(&window);
        let work = staged
            .draw(&window, &mut staged_surface)
            .expect("forced frame renders")
            .expect("a forced repaint must produce a frame");
        assert_eq!(
            work.dirty_pixels,
            8 * 4,
            "a forced repaint must cover every pixel on the screen"
        );
    }
}
