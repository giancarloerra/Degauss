//! Where pixels go.
//!
//! Two implementations behind one shape:
//!
//! * [`Framebuffer`]: the real Linux `/dev/fb0`, discovered at runtime.
//!   Nothing about geometry or pixel format is hardcoded, because a MiSTer
//!   framebuffer is 640x240 on a 15 kHz tube and something else entirely on
//!   HDMI, and Degauss has to work on whatever it is handed.
//! * [`MemorySurface`]: the same pixel layout in RAM, used by tests and for
//!   dumping frames to an image on a development machine.
//!
//! Only two pixel formats are accepted: 16-bit RGB565 and 32-bit XRGB8888.
//! Anything else is an error rather than a guess, because guessing a channel
//! order produces a picture that looks plausible and is wrong.

use std::path::Path;

use crate::error::{DegaussError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 16 bits: rrrrrggg gggbbbbb, little-endian in memory.
    Rgb565,
    /// 32 bits: byte order B, G, R, X (the common Linux fbdev layout).
    Xrgb8888,
}

impl PixelFormat {
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            PixelFormat::Rgb565 => 2,
            PixelFormat::Xrgb8888 => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub width: u32,
    pub height: u32,
    /// Bytes per scanline. Can exceed `width * bytes_per_pixel` when the
    /// driver pads rows, which is why nothing may assume a tight buffer.
    pub line_length: usize,
    pub format: PixelFormat,
}

impl Geometry {
    /// Bytes for one visible frame.
    pub fn frame_bytes(&self) -> usize {
        self.line_length * self.height as usize
    }
}

/// A surface that can be drawn into and shown.
pub trait Surface {
    fn geometry(&self) -> Geometry;
    /// The bytes of the frame currently being drawn.
    fn back_buffer(&mut self) -> &mut [u8];
    /// Note that a frame was completed. MiSTer's framebuffer has exactly one
    /// buffer, so there is nothing to flip: whatever was written is already
    /// on screen. This exists so the memory surface used in tests can count
    /// frames, and so a future device with real buffers has a place to hook.
    fn present(&mut self) -> Result<()>;
    /// Block until the display starts its next frame, when the device can
    /// say. False when it cannot, and the caller paces itself instead.
    ///
    /// A single-buffer framebuffer is scanned out while it is being written,
    /// so drawing whenever a frame happens to be ready tears. Waiting here
    /// puts each frame between scans and stops the loop spinning through
    /// frames the tube will never show.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn wait_for_vsync(&mut self) -> bool {
        false
    }
}

/// Plain RAM surface. Used by tests, and on a development machine where
/// there is no framebuffer to draw on.
pub struct MemorySurface {
    geometry: Geometry,
    bytes: Vec<u8>,
    pub presents: u64,
}

impl MemorySurface {
    pub fn new(width: u32, height: u32, format: PixelFormat) -> Self {
        let line_length = width as usize * format.bytes_per_pixel();
        let geometry = Geometry {
            width,
            height,
            line_length,
            format,
        };
        MemorySurface {
            bytes: vec![0; geometry.frame_bytes()],
            geometry,
            presents: 0,
        }
    }

    /// The raw frame, for tests that check what actually landed in memory.
    #[allow(dead_code)]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Write the frame as a 24-bit BMP, so a render can be inspected with an
    /// ordinary image viewer.
    pub fn write_bmp(&self, path: &Path) -> Result<()> {
        let w = self.geometry.width as usize;
        let h = self.geometry.height as usize;
        let row_padded = (w * 3 + 3) & !3;
        let pixel_bytes = row_padded * h;
        let file_size = 54 + pixel_bytes;

        let mut out = Vec::with_capacity(file_size);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(file_size as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&(w as i32).to_le_bytes());
        // Negative height: rows top-down, matching our buffer order.
        out.extend_from_slice(&(-(h as i32)).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(pixel_bytes as u32).to_le_bytes());
        out.extend_from_slice(&2835i32.to_le_bytes());
        out.extend_from_slice(&2835i32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());

        let bpp = self.geometry.format.bytes_per_pixel();
        for y in 0..h {
            let row = &self.bytes[y * self.geometry.line_length..];
            for x in 0..w {
                let px = &row[x * bpp..x * bpp + bpp];
                let (r, g, b) = match self.geometry.format {
                    PixelFormat::Rgb565 => {
                        let v = u16::from_le_bytes([px[0], px[1]]);
                        let r = ((v >> 11) & 0x1f) as u8;
                        let g = ((v >> 5) & 0x3f) as u8;
                        let b = (v & 0x1f) as u8;
                        // Replicate high bits so 0x1f maps to 0xff, not 0xf8.
                        (
                            (r << 3) | (r >> 2),
                            (g << 2) | (g >> 4),
                            (b << 3) | (b >> 2),
                        )
                    }
                    PixelFormat::Xrgb8888 => (px[2], px[1], px[0]),
                };
                out.push(b);
                out.push(g);
                out.push(r);
            }
            out.resize(out.len() + (row_padded - w * 3), 0);
        }

        std::fs::write(path, &out).map_err(|e| DegaussError::io("writing bmp", path, e))
    }
}

impl Surface for MemorySurface {
    fn geometry(&self) -> Geometry {
        self.geometry
    }

    fn back_buffer(&mut self) -> &mut [u8] {
        &mut self.bytes
    }

    fn present(&mut self) -> Result<()> {
        self.presents += 1;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub use linux::Framebuffer;

#[cfg(target_os = "linux")]
mod linux {
    use std::fs::{File, OpenOptions};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;

    use super::{Geometry, PixelFormat, Surface};
    use crate::error::{DegaussError, Result};

    // fbdev ioctls, from linux/fb.h.
    const FBIOGET_VSCREENINFO: libc::Ioctl = 0x4600;
    const FBIOGET_FSCREENINFO: libc::Ioctl = 0x4602;
    // _IOW('F', 0x20, u32). MiSTer's driver implements this against a real
    // hardware interrupt and times out after 50 ms if the interrupt never
    // arrives, so a false return here is meaningful, not a stub.
    const FBIO_WAITFORVSYNC: libc::Ioctl = 0x4004_4620;

    #[repr(C)]
    #[derive(Debug, Default, Clone, Copy)]
    struct FbBitfield {
        offset: u32,
        length: u32,
        msb_right: u32,
    }

    #[repr(C)]
    #[derive(Debug, Default, Clone, Copy)]
    struct FbVarScreeninfo {
        xres: u32,
        yres: u32,
        xres_virtual: u32,
        yres_virtual: u32,
        xoffset: u32,
        yoffset: u32,
        bits_per_pixel: u32,
        grayscale: u32,
        red: FbBitfield,
        green: FbBitfield,
        blue: FbBitfield,
        transp: FbBitfield,
        nonstd: u32,
        activate: u32,
        height: u32,
        width: u32,
        accel_flags: u32,
        pixclock: u32,
        left_margin: u32,
        right_margin: u32,
        upper_margin: u32,
        lower_margin: u32,
        hsync_len: u32,
        vsync_len: u32,
        sync: u32,
        vmode: u32,
        rotate: u32,
        colorspace: u32,
        reserved: [u32; 4],
    }

    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    struct FbFixScreeninfo {
        id: [u8; 16],
        smem_start: libc::c_ulong,
        smem_len: u32,
        kind: u32,
        type_aux: u32,
        visual: u32,
        xpanstep: u16,
        ypanstep: u16,
        ywrapstep: u16,
        line_length: u32,
        mmio_start: libc::c_ulong,
        mmio_len: u32,
        accel: u32,
        capabilities: u16,
        reserved: [u16; 2],
    }

    impl Default for FbFixScreeninfo {
        fn default() -> Self {
            // SAFETY: every field is a plain integer or integer array, so an
            // all-zero value is a valid instance.
            unsafe { std::mem::zeroed() }
        }
    }

    fn checked_geometry(var: &FbVarScreeninfo, fix: &FbFixScreeninfo) -> Result<(Geometry, usize)> {
        let format = match var.bits_per_pixel {
            16 => PixelFormat::Rgb565,
            32 => PixelFormat::Xrgb8888,
            other => {
                return Err(DegaussError::unsupported(
                    "framebuffer pixel format",
                    format!("{other} bits per pixel (only 16 and 32 are handled)"),
                ))
            }
        };

        // Channel order is checked, not assumed: a 16bpp surface that is
        // actually BGR565 would render with red and blue swapped.
        let (r_off, g_off, b_off) = (var.red.offset, var.green.offset, var.blue.offset);
        let expected = match format {
            PixelFormat::Rgb565 => (11, 5, 0),
            PixelFormat::Xrgb8888 => (16, 8, 0),
        };
        if (r_off, g_off, b_off) != expected {
            return Err(DegaussError::unsupported(
                "framebuffer channel order",
                format!(
                    "red/green/blue offsets {r_off}/{g_off}/{b_off}, expected {}/{}/{}",
                    expected.0, expected.1, expected.2
                ),
            ));
        }

        let geometry = Geometry {
            width: var.xres,
            height: var.yres,
            line_length: fix.line_length as usize,
            format,
        };
        if geometry.width == 0 || geometry.height == 0 || geometry.line_length == 0 {
            return Err(DegaussError::unsupported(
                "framebuffer geometry",
                format!(
                    "{}x{} line_length {}",
                    geometry.width, geometry.height, geometry.line_length
                ),
            ));
        }

        // A row must fit the stride the driver reports, or every write
        // past the row's width lands in the next line.
        let row_bytes = (geometry.width as usize)
            .checked_mul(geometry.format.bytes_per_pixel())
            .ok_or_else(|| {
                DegaussError::unsupported("framebuffer geometry", "row size overflow")
            })?;
        if geometry.line_length < row_bytes {
            return Err(DegaussError::unsupported(
                "framebuffer geometry",
                format!(
                    "line_length {} is shorter than a {}px row of {} bytes",
                    geometry.line_length, geometry.width, row_bytes
                ),
            ));
        }

        // One visible frame is all this driver ever exposes, and the
        // driver has to actually have that much: mapping more than it
        // owns turns every later write into a SIGBUS, which on this
        // machine is the menu disappearing.
        let map_len = geometry
            .line_length
            .checked_mul(geometry.height as usize)
            .filter(|len| *len <= isize::MAX as usize)
            .ok_or_else(|| {
                DegaussError::unsupported("framebuffer memory", "frame size overflow")
            })?;
        if (fix.smem_len as usize) < map_len {
            return Err(DegaussError::unsupported(
                "framebuffer memory",
                format!(
                    "driver reports {} bytes, a {}x{} frame needs {map_len}",
                    fix.smem_len, geometry.width, geometry.height
                ),
            ));
        }
        Ok((geometry, map_len))
    }

    #[derive(Debug, PartialEq, Eq)]
    struct PhysicalRange {
        base: u64,
        delta: usize,
        len: usize,
    }

    fn physical_range(
        start: u64,
        available: usize,
        requested: usize,
        page_size: usize,
        pixel_bytes: usize,
    ) -> Result<PhysicalRange> {
        let invalid = || {
            DegaussError::unsupported(
                "framebuffer /dev/mem fallback after fbdev ENODEV",
                "invalid or overflowing physical framebuffer range",
            )
        };
        if start == 0 || requested == 0 || requested > available || !page_size.is_power_of_two() {
            return Err(invalid());
        }
        if !matches!(pixel_bytes, 2 | 4) || !start.is_multiple_of(pixel_bytes as u64) {
            return Err(DegaussError::unsupported(
                "framebuffer /dev/mem fallback after fbdev ENODEV",
                format!("physical framebuffer address is not aligned to {pixel_bytes}-byte pixels"),
            ));
        }
        start.checked_add(available as u64).ok_or_else(invalid)?;
        let delta = (start % page_size as u64) as usize;
        let base = start - delta as u64;
        let len = requested
            .checked_add(delta)
            .filter(|len| *len <= isize::MAX as usize)
            .ok_or_else(invalid)?;
        // Linux rounds mmap's length up to full pages. Check that rounding
        // cannot overflow; the extra tail is only the page containing the
        // requested framebuffer end, as on the native fbdev mapping.
        let page_span = len.checked_add(page_size - 1).ok_or_else(invalid)? & !(page_size - 1);
        base.checked_add(page_span as u64).ok_or_else(invalid)?;
        // mmap64 accepts a signed offset even on 32-bit ARM.
        i64::try_from(base).map_err(|_| invalid())?;
        Ok(PhysicalRange { base, delta, len })
    }

    fn fallback_error(framebuffer: &Path, operation: &str, error: std::io::Error) -> DegaussError {
        DegaussError::io(
            "mapping framebuffer after fbdev ENODEV",
            Path::new("/dev/mem"),
            std::io::Error::new(
                error.kind(),
                format!(
                    "{} returned ENODEV; {operation}: {error}",
                    framebuffer.display()
                ),
            ),
        )
    }

    fn with_enodev_fallback<T>(
        native: std::io::Result<T>,
        path: &Path,
        fallback: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        match native {
            Ok(mapping) => Ok(mapping),
            Err(error) if error.raw_os_error() == Some(libc::ENODEV) => fallback(),
            Err(error) => Err(DegaussError::io("mapping framebuffer", path, error)),
        }
    }

    fn map_device(file: &File, len: usize, offset: u64) -> std::io::Result<*mut libc::c_void> {
        // SAFETY: length and physical offset are validated before fallback;
        // the native mapping uses the checked visible-frame length and offset zero.
        let map = unsafe {
            libc::mmap64(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                offset as libc::off64_t,
            )
        };
        if map == libc::MAP_FAILED {
            // Capture errno before any other syscall can change it.
            Err(std::io::Error::last_os_error())
        } else {
            Ok(map)
        }
    }

    /// The real framebuffer device.
    pub struct Framebuffer {
        file: File,
        map: *mut libc::c_void,
        map_len: usize,
        pixel_offset: usize,
        mapping_source: &'static str,
        geometry: Geometry,
    }

    // The mapping is owned exclusively by this struct and never aliased.
    unsafe impl Send for Framebuffer {}

    impl Framebuffer {
        pub fn mapping_source(&self) -> &'static str {
            self.mapping_source
        }

        pub fn open(path: &Path) -> Result<Self> {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .map_err(|e| DegaussError::io("opening framebuffer", path, e))?;

            let mut var = FbVarScreeninfo::default();
            let mut fix = FbFixScreeninfo::default();
            // SAFETY: both pointers are valid, correctly sized and typed for
            // the ioctls being issued.
            let ok = unsafe {
                libc::ioctl(file.as_raw_fd(), FBIOGET_VSCREENINFO, &mut var) == 0
                    && libc::ioctl(file.as_raw_fd(), FBIOGET_FSCREENINFO, &mut fix) == 0
            };
            if !ok {
                return Err(DegaussError::io(
                    "querying framebuffer geometry",
                    path,
                    std::io::Error::last_os_error(),
                ));
            }

            let (geometry, frame_len) = checked_geometry(&var, &fix)?;
            let native = map_device(&file, frame_len, 0);
            let (map, map_len, pixel_offset, mapping_source) =
                with_enodev_fallback(native.map(|map| (map, frame_len, 0, "fbdev")), path, || {
                    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
                    let range = physical_range(
                        fix.smem_start as u64,
                        fix.smem_len as usize,
                        frame_len,
                        usize::try_from(page_size).unwrap_or(0),
                        geometry.format.bytes_per_pixel(),
                    )?;
                    let mem_path = Path::new("/dev/mem");
                    let memory = OpenOptions::new()
                        .read(true)
                        .write(true)
                        .custom_flags(libc::O_SYNC | libc::O_CLOEXEC)
                        .open(mem_path)
                        .map_err(|error| fallback_error(path, "opening /dev/mem", error))?;
                    let map = map_device(&memory, range.len, range.base)
                        .map_err(|error| fallback_error(path, "mapping /dev/mem", error))?;
                    Ok((map, range.len, range.delta, "/dev/mem fallback"))
                })?;

            Ok(Framebuffer {
                file,
                map,
                map_len,
                pixel_offset,
                mapping_source,
                geometry,
            })
        }
    }

    impl Surface for Framebuffer {
        fn geometry(&self) -> Geometry {
            self.geometry
        }

        fn back_buffer(&mut self) -> &mut [u8] {
            let len = self.geometry.frame_bytes();
            // SAFETY: the mapping is at least one frame long (checked at open
            // time) and this borrow is the only live view of those bytes.
            //
            // Note this memory is WRITE-COMBINED: writes stream out cheaply
            // but reads are uncached and slow, which is why the staged
            // presentation path exists.
            unsafe {
                std::slice::from_raw_parts_mut((self.map as *mut u8).add(self.pixel_offset), len)
            }
        }

        fn present(&mut self) -> Result<()> {
            // Single buffer: what was written is already being scanned out.
            Ok(())
        }

        fn wait_for_vsync(&mut self) -> bool {
            Framebuffer::wait_for_vsync(self).unwrap_or(false)
        }
    }

    impl Framebuffer {
        /// Ask the driver to wait for the start of the next displayed frame.
        ///
        /// Returns false when the driver cannot answer. On MiSTer this is a
        /// real hardware interrupt with a 50 ms timeout, so a false here
        /// means there is genuinely no pacing signal to synchronise with,
        /// not that the call is a stub.
        pub fn wait_for_vsync(&mut self) -> Result<bool> {
            let mut arg: u32 = 0;
            // SAFETY: the ioctl reads a single u32 through this pointer.
            let rc = unsafe { libc::ioctl(self.file.as_raw_fd(), FBIO_WAITFORVSYNC, &mut arg) };
            Ok(rc == 0)
        }
    }

    impl Drop for Framebuffer {
        fn drop(&mut self) {
            // SAFETY: unmapping exactly what was mapped in `open`. Nothing
            // else needs undoing: Degauss never changes the framebuffer's
            // mode, so there is no device state to restore.
            unsafe {
                libc::munmap(self.map, self.map_len);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::cell::Cell;

        #[test]
        fn only_enodev_uses_the_physical_mapping() {
            let calls = Cell::new(0);
            let fallback = || {
                calls.set(calls.get() + 1);
                Ok(42)
            };
            assert_eq!(
                with_enodev_fallback(Ok(7), Path::new("/dev/fb0"), fallback).unwrap(),
                7
            );
            assert_eq!(calls.get(), 0);
            for errno in [
                libc::EACCES,
                libc::EINVAL,
                libc::ENOMEM,
                libc::EIO,
                libc::EPERM,
            ] {
                let error = with_enodev_fallback(
                    Err(std::io::Error::from_raw_os_error(errno)),
                    Path::new("/dev/fb0"),
                    fallback,
                )
                .unwrap_err();
                assert!(error
                    .to_string()
                    .contains(&std::io::Error::from_raw_os_error(errno).to_string()));
            }
            assert_eq!(calls.get(), 0);
            assert_eq!(
                with_enodev_fallback(
                    Err(std::io::Error::from_raw_os_error(libc::ENODEV)),
                    Path::new("/dev/fb0"),
                    fallback
                )
                .unwrap(),
                42
            );
            assert_eq!(calls.get(), 1);
        }

        #[test]
        fn fallback_failures_name_both_device_boundaries() {
            for operation in ["opening /dev/mem", "mapping /dev/mem"] {
                let error = fallback_error(
                    Path::new("/dev/fb9"),
                    operation,
                    std::io::Error::from_raw_os_error(libc::EACCES),
                )
                .to_string();
                assert!(error.contains("/dev/fb9 returned ENODEV"));
                assert!(error.contains(operation));
                assert!(
                    error.contains(&std::io::Error::from_raw_os_error(libc::EACCES).to_string())
                );
            }
        }

        #[test]
        fn physical_mapping_preserves_driver_address_and_checks_bounds() {
            assert_eq!(
                physical_range(0x2000, 4096, 1000, 4096, 2).unwrap(),
                PhysicalRange {
                    base: 0x2000,
                    delta: 0,
                    len: 1000
                }
            );
            assert_eq!(
                physical_range(0x2082, 4096, 1000, 4096, 2).unwrap(),
                PhysicalRange {
                    base: 0x2000,
                    delta: 130,
                    len: 1130
                }
            );
            for pixel_bytes in [2, 4] {
                assert!(physical_range(0x2001, 4096, 1000, 4096, pixel_bytes).is_err());
                assert!(physical_range(0x2084, 4096, 1000, 4096, pixel_bytes).is_ok());
            }
            assert!(physical_range(0x2002, 4096, 1000, 4096, 4).is_err());
            // The queried ARM physical address may exceed a signed 32-bit offset.
            assert_eq!(
                physical_range(0xe0000000, 4096, 1000, 4096, 4)
                    .unwrap()
                    .base,
                0xe0000000
            );
            for (start, available, requested, page) in [
                (0, 4096, 1000, 4096),
                (0x2000, 999, 1000, 4096),
                (0x2000, 4096, 0, 4096),
                (0x2000, 4096, 1000, 0),
                (0x2000, 4096, 1000, 3),
                (u64::MAX - 3, 4, 4, 4096),
                (0x2002, usize::MAX, usize::MAX, 4096),
                // Requested range fits u64, but the enclosing page does not.
                (u64::MAX - 4095, 2, 2, 4096),
                // The adjusted slice would exceed isize::MAX on either target.
                (0x2002, isize::MAX as usize, isize::MAX as usize, 4096),
                (1u64 << 63, 4096, 1000, 4096),
            ] {
                assert!(physical_range(start, available, requested, page, 2).is_err());
            }
        }

        fn screen(format: PixelFormat) -> (FbVarScreeninfo, FbFixScreeninfo) {
            let (bits, red, green) = match format {
                PixelFormat::Rgb565 => (16, 11, 5),
                PixelFormat::Xrgb8888 => (32, 16, 8),
            };
            let var = FbVarScreeninfo {
                xres: 640,
                yres: 240,
                bits_per_pixel: bits,
                red: FbBitfield {
                    offset: red,
                    ..Default::default()
                },
                green: FbBitfield {
                    offset: green,
                    ..Default::default()
                },
                ..Default::default()
            };
            let stride = 640 * format.bytes_per_pixel() as u32 + 64;
            (
                var,
                FbFixScreeninfo {
                    line_length: stride,
                    smem_len: stride * 240,
                    ..Default::default()
                },
            )
        }

        #[test]
        fn both_pixel_formats_keep_geometry_stride_and_memory_validation() {
            for format in [PixelFormat::Rgb565, PixelFormat::Xrgb8888] {
                let (var, fix) = screen(format);
                let (geometry, len) = checked_geometry(&var, &fix).unwrap();
                assert_eq!(geometry.format, format);
                assert_eq!(geometry.width, 640);
                assert_eq!(len, fix.line_length as usize * 240);
                assert!(checked_geometry(&FbVarScreeninfo { xres: 0, ..var }, &fix).is_err());
                assert!(checked_geometry(
                    &FbVarScreeninfo {
                        red: FbBitfield::default(),
                        ..var
                    },
                    &fix
                )
                .is_err());
                assert!(checked_geometry(
                    &FbVarScreeninfo {
                        bits_per_pixel: 24,
                        ..var
                    },
                    &fix
                )
                .is_err());
                assert!(checked_geometry(
                    &var,
                    &FbFixScreeninfo {
                        line_length: 1,
                        ..fix
                    }
                )
                .is_err());
                assert!(checked_geometry(
                    &var,
                    &FbFixScreeninfo {
                        smem_len: fix.smem_len - 1,
                        ..fix
                    }
                )
                .is_err());
                assert!(checked_geometry(
                    &FbVarScreeninfo {
                        xres: u32::MAX,
                        yres: u32::MAX,
                        ..var
                    },
                    &fix
                )
                .is_err());
            }
        }

        #[test]
        fn adjusted_pixels_and_drop_use_different_addresses() {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/zero")
                .unwrap();
            let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
            let map = map_device(&file, page * 2, 0).unwrap();
            let mut fb = Framebuffer {
                file,
                map,
                map_len: page * 2,
                pixel_offset: 128,
                mapping_source: "/dev/mem fallback",
                geometry: Geometry {
                    width: 2,
                    height: 1,
                    line_length: 8,
                    format: PixelFormat::Xrgb8888,
                },
            };
            assert_eq!(fb.mapping_source(), "/dev/mem fallback");
            fb.back_buffer().fill(0xa5);
            // SAFETY: both regions lie inside this live, exclusively held mapping.
            unsafe {
                assert_eq!(*(map as *const u8), 0);
                assert_eq!(*((map as *const u8).add(128)), 0xa5);
                assert_eq!(*((map as *const u8).add(136)), 0);
            }
            drop(fb);
            // mincore reports ENOMEM for the now-unmapped original page range.
            let mut residency = [0u8; 2];
            assert_eq!(
                unsafe { libc::mincore(map, page * 2, residency.as_mut_ptr()) },
                -1
            );
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ENOMEM)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a pixel by hand, for the BMP fixture below. Production code
    /// never packs pixels itself: Slint's renderer writes them.
    fn pack(format: PixelFormat, r: u8, g: u8, b: u8, out: &mut [u8]) {
        match format {
            PixelFormat::Rgb565 => {
                let value =
                    (((r as u16) & 0xf8) << 8) | (((g as u16) & 0xfc) << 3) | ((b as u16) >> 3);
                out[0] = (value & 0xff) as u8;
                out[1] = (value >> 8) as u8;
            }
            PixelFormat::Xrgb8888 => {
                out[0] = b;
                out[1] = g;
                out[2] = r;
                out[3] = 0xff;
            }
        }
    }

    #[test]
    fn frame_size_follows_the_stride_not_the_width() {
        let tight = Geometry {
            width: 640,
            height: 240,
            line_length: 1280,
            format: PixelFormat::Rgb565,
        };
        assert_eq!(tight.frame_bytes(), 1280 * 240);

        // A driver may pad rows; frame size must follow the stride, not the
        // width, or every row after the first lands skewed.
        let padded = Geometry {
            line_length: 1408,
            ..tight
        };
        assert_eq!(padded.frame_bytes(), 1408 * 240);
    }

    #[test]
    fn a_memory_surface_round_trips_through_bmp_with_the_right_colours() {
        let mut surface = MemorySurface::new(2, 1, PixelFormat::Rgb565);
        let format = surface.geometry().format;
        {
            let buf = surface.back_buffer();
            pack(format, 0xff, 0x00, 0x00, &mut buf[0..2]);
            pack(format, 0x00, 0x00, 0xff, &mut buf[2..4]);
        }
        let path = std::env::temp_dir().join(format!("degauss-surface-{}.bmp", std::process::id()));
        surface.write_bmp(&path).expect("bmp written");

        let bytes = std::fs::read(&path).expect("bmp readable");
        assert_eq!(&bytes[0..2], b"BM");
        // 54-byte header, then BGR triples.
        assert_eq!(&bytes[54..57], &[0x00, 0x00, 0xff], "first pixel is red");
        assert_eq!(&bytes[57..60], &[0xff, 0x00, 0x00], "second pixel is blue");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn presenting_a_memory_surface_is_counted() {
        let mut surface = MemorySurface::new(4, 4, PixelFormat::Xrgb8888);
        surface.present().unwrap();
        assert_eq!(surface.presents, 1);
    }
}
