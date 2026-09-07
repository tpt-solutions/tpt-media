//! Pixel format enumeration for video frames.

/// A pixel format.
///
/// Only formats we can implement from public-domain mathematics are
/// included. No proprietary formats.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PixelFormat {
    /// 8-bit grayscale.
    Gray8 = 0,
    /// 8-bit R, G, B (no alpha). 24 bpp.
    Rgb24 = 1,
    /// 8-bit R, G, B, A. 32 bpp.
    Rgba32 = 2,
    /// 8-bit Y, U, V 4:2:0 planar (Y full, V/U half). 12 bpp average.
    Yuv420 = 3,
    /// 8-bit Y, U, V 4:2:0 semi-planar (Y plane + interleaved UV). 12 bpp.
    Nv12 = 4,
}

impl PixelFormat {
    /// Bytes per pixel (rounded up for subsampled formats).
    pub const fn bytes_per_pixel(self) -> u8 {
        match self {
            PixelFormat::Gray8 => 1,
            PixelFormat::Rgb24 => 3,
            PixelFormat::Rgba32 => 4,
            PixelFormat::Yuv420 => 1, // avg, see bpp()
            PixelFormat::Nv12 => 1,
        }
    }

    /// Bits per pixel (rounded up for subsampled formats).
    pub const fn bits_per_pixel(self) -> u8 {
        match self {
            PixelFormat::Gray8 => 8,
            PixelFormat::Rgb24 => 24,
            PixelFormat::Rgba32 => 32,
            PixelFormat::Yuv420 => 12,
            PixelFormat::Nv12 => 12,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bpp_table() {
        assert_eq!(PixelFormat::Gray8.bytes_per_pixel(), 1);
        assert_eq!(PixelFormat::Rgb24.bytes_per_pixel(), 3);
        assert_eq!(PixelFormat::Rgba32.bytes_per_pixel(), 4);
        assert_eq!(PixelFormat::Yuv420.bits_per_pixel(), 12);
    }
}
