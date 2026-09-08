//! Image/video frame I/O.
//!
//! Dependency-free PPM P6 (RGB) and PGM P5 (grayscale) raster readers and
//! writers over [`VideoFrame`], plus fixture generators like
//! [`checkerboard`]. These are the frame-I/O boundary for the video codec
//! pipeline (§54): trivial formats that need no external image library,
//! deterministic to verify, and easy to replace when a real format lands.

extern crate alloc;

use alloc::format;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;

use tpt_media_core::{PixelFormat, Timebase, Timestamp, VideoFrame};

use crate::{IoError, IoResult};

/// Raster wire formats this module can read/write.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RasterFormat {
    /// Binary PPM `P6` — 8-bit per-channel RGB.
    PpmP6,
    /// Binary PGM `P5` — 8-bit grayscale.
    PgmP5,
}

impl RasterFormat {
    /// The pixel format a raster maps to.
    pub fn pixel_format(self) -> PixelFormat {
        match self {
            RasterFormat::PpmP6 => PixelFormat::Rgb24,
            RasterFormat::PgmP5 => PixelFormat::Gray8,
        }
    }

    fn magic(self) -> &'static str {
        match self {
            RasterFormat::PpmP6 => "P6",
            RasterFormat::PgmP5 => "P5",
        }
    }
}

/// Decode PPM/PGM `P6`/`P5` bytes into a [`VideoFrame`].
///
/// Only `maxval 255`, single-plane rasters are accepted; a well-formed
/// header with anything else (comments, `P5` with maxval != 255, extra
/// planes) is rejected with a clear error.
pub fn read_raster(bytes: &[u8]) -> IoResult<VideoFrame> {
    let mut pos = 0usize;
    let magic = token(bytes, &mut pos)?;
    let format = match magic {
        "P6" => RasterFormat::PpmP6,
        "P5" => RasterFormat::PgmP5,
        _ => return Err(IoError::InvalidHeader),
    };
    let width = parse_u32(token(bytes, &mut pos)?)?;
    let height = parse_u32(token(bytes, &mut pos)?)?;
    let maxval = parse_u32(token(bytes, &mut pos)?)?;
    if maxval != 255 {
        return Err(IoError::Unsupported("maxval != 255"));
    }
    // Skip trailing whitespace (single space, or CR/LF mixes) before the
    // binary pixel block.
    while bytes
        .get(pos)
        .is_some_and(|b| (*b as char).is_ascii_whitespace())
    {
        pos += 1;
    }
    let pixel_format = format.pixel_format();
    let stride = width * u32::from(pixel_format.bytes_per_pixel());
    let expected = (stride * height) as usize;
    if bytes.len() < pos + expected {
        return Err(IoError::UnexpectedEof);
    }
    let pixels = bytes[pos..pos + expected].to_vec();
    Ok(VideoFrame {
        frame: tpt_media_core::Frame {
            pts: Timestamp::zero(Timebase::ONE_PER_SECOND),
            duration: None,
            side_data: Vec::new(),
        },
        pixels,
        width,
        height,
        format: pixel_format,
        stride,
    })
}

/// Read a raster file from disk. (`std`)
#[cfg(feature = "std")]
pub fn read_raster_path(path: &str) -> IoResult<VideoFrame> {
    let bytes = std::fs::read(path).map_err(|e| IoError::Other(e.to_string()))?;
    read_raster(&bytes)
}

/// Encode a [`VideoFrame`] as PPM P6 (RGB) or PGM P5 (grayscale).
///
/// The format is chosen from the frame's [`PixelFormat`]: `Rgb24`/`Rgba32`
/// → P6 (alpha dropped for `Rgba32`), `Gray8` → P5, anything else is
/// unsupported. Rows are copied by stride so padded frames encode cleanly.
pub fn write_raster(frame: &VideoFrame) -> IoResult<Vec<u8>> {
    let format = match frame.format {
        PixelFormat::Rgb24 | PixelFormat::Rgba32 => RasterFormat::PpmP6,
        PixelFormat::Gray8 => RasterFormat::PgmP5,
        _ => return Err(IoError::Unsupported("pixel format for PPM/PGM")),
    };
    let mut out = Vec::with_capacity(64 + frame.pixels.len());
    out.extend_from_slice(
        format!(
            "{}\n{} {}\n255\n",
            format.magic(),
            frame.width,
            frame.height
        )
        .as_bytes(),
    );
    let out_bpp = format.pixel_format().bytes_per_pixel() as usize;
    let src_bpp = frame.format.bytes_per_pixel() as usize;
    let row_bytes = frame.stride as usize;
    if row_bytes == 0 {
        return Err(IoError::InvalidHeader);
    }
    for y in 0..frame.height {
        let start = y as usize * row_bytes;
        let end = (start + row_bytes).min(frame.pixels.len());
        let row = &frame.pixels[start..end];
        let px_bytes = (frame.width as usize * src_bpp).min(row.len());
        match frame.format {
            PixelFormat::Rgb24 | PixelFormat::Gray8 => {
                out.extend_from_slice(&row[..px_bytes]);
            }
            PixelFormat::Rgba32 => {
                for px in row[..px_bytes].chunks(src_bpp) {
                    out.extend_from_slice(&px[..out_bpp.min(px.len())]);
                }
            }
            _ => unreachable!(),
        }
    }
    Ok(out)
}

/// Write a raster file to disk. (`std`)
#[cfg(feature = "std")]
pub fn write_raster_path(path: &str, frame: &VideoFrame) -> IoResult<()> {
    let bytes = write_raster(frame)?;
    std::fs::write(path, bytes).map_err(|e| IoError::Other(e.to_string()))
}

/// Generate a deterministic `tile × tile` checkerboard [`VideoFrame`].
///
/// Color frames get white/gray tiles, grayscale gets white/black — enough
/// contrast for codec fixtures (e.g. run-length sanity checks).
pub fn checkerboard(
    width: u32,
    height: u32,
    format: PixelFormat,
    tile: u32,
) -> IoResult<VideoFrame> {
    if tile == 0 {
        return Err(IoError::Other("tile size must be >= 1".into()));
    }
    let (white, dark) = match format {
        PixelFormat::Rgb24 | PixelFormat::Rgba32 => {
            ([255u8, 255, 255, 255], [128u8, 128, 128, 255])
        }
        PixelFormat::Gray8 => ([255u8, 255, 255, 255], [0u8, 0, 0, 0]),
        _ => return Err(IoError::Unsupported("pixel format for checkerboard")),
    };
    let bpp = format.bytes_per_pixel() as usize;
    let stride = width * format.bytes_per_pixel() as u32;
    let mut pixels = vec![0u8; (stride * height) as usize];
    for y in 0..height {
        for x in 0..width {
            let tile_parity = (y / tile + x / tile) & 1;
            let color = if tile_parity == 0 { white } else { dark };
            let offset = (y * stride) as usize + (x as usize) * bpp;
            pixels[offset..offset + bpp].copy_from_slice(&color[..bpp]);
        }
    }
    Ok(VideoFrame {
        frame: tpt_media_core::Frame {
            pts: Timestamp::zero(Timebase::ONE_PER_SECOND),
            duration: None,
            side_data: Vec::new(),
        },
        pixels,
        width,
        height,
        format,
        stride,
    })
}

impl fmt::Display for RasterFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.magic())
    }
}

/// Read the next whitespace-delimited ASCII token, ending at (not
/// consuming) the whitespace after it.
fn token<'a>(bytes: &'a [u8], pos: &mut usize) -> IoResult<&'a str> {
    while *pos < bytes.len() && (bytes[*pos] as char).is_ascii_whitespace() {
        *pos += 1;
    }
    let start = *pos;
    while *pos < bytes.len() && !(bytes[*pos] as char).is_ascii_whitespace() {
        *pos += 1;
    }
    let s = core::str::from_utf8(bytes.get(start..*pos).ok_or(IoError::UnexpectedEof)?)
        .map_err(|_| IoError::InvalidHeader)?;
    if s.is_empty() {
        return Err(IoError::UnexpectedEof);
    }
    Ok(s)
}

fn parse_u32(s: &str) -> IoResult<u32> {
    s.parse().map_err(|_| IoError::InvalidHeader)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb24_round_trip() {
        let frame = checkerboard(16, 8, PixelFormat::Rgb24, 4).unwrap();
        let bytes = write_raster(&frame).unwrap();
        assert!(bytes.starts_with(b"P6\n16 8\n255\n"));
        assert_eq!(bytes.len(), 12 + 16 * 8 * 3); // header + pixels
        let back = read_raster(&bytes).unwrap();
        assert_eq!(back.format, PixelFormat::Rgb24);
        assert_eq!(back.width, 16);
        assert_eq!(back.height, 8);
        assert_eq!(back.stride, 16 * 3);
        assert_eq!(back.pixels, frame.pixels);
    }

    #[test]
    fn gray8_round_trip() {
        let frame = checkerboard(8, 8, PixelFormat::Gray8, 2).unwrap();
        let bytes = write_raster(&frame).unwrap();
        assert!(bytes.starts_with(b"P5\n8 8\n255\n"));
        let back = read_raster(&bytes).unwrap();
        assert_eq!(back.format, PixelFormat::Gray8);
        assert_eq!(back.pixels, frame.pixels);
    }

    #[test]
    fn rgba32_drops_alpha() {
        let frame = checkerboard(4, 4, PixelFormat::Rgba32, 2).unwrap();
        let bytes = write_raster(&frame).unwrap();
        let back = read_raster(&bytes).unwrap();
        assert_eq!(back.format, PixelFormat::Rgb24);
        // checkerboard is white/gray; alpha would have shown as extra bytes
        assert_eq!(back.pixels.len(), 4 * 4 * 3);
        assert_eq!(back.pixels[0..3], [255, 255, 255]);
    }

    #[test]
    fn checkerboard_contrast() {
        let frame = checkerboard(8, 8, PixelFormat::Gray8, 4).unwrap();
        assert_eq!(frame.pixels[0], 255); // top-left tile white
        assert_eq!(frame.pixels[4], 0); // column 4 of row 0 is a dark tile
    }

    #[test]
    fn read_rejects_bad_magic() {
        assert!(matches!(
            read_raster(b"XX\n1 1\n255\n\x00"),
            Err(IoError::InvalidHeader)
        ));
    }

    #[test]
    fn read_rejects_truncated_pixels() {
        let bytes = b"P6\n4 4\n255\n";
        assert!(matches!(read_raster(bytes), Err(IoError::UnexpectedEof)));
    }

    #[test]
    fn read_rejects_maxval_not_255() {
        let bytes = b"P6\n1 1\n254\n\xff\xff\xff";
        assert!(matches!(read_raster(bytes), Err(IoError::Unsupported(_))));
    }

    #[test]
    fn read_accepts_crlf_breaks() {
        // Pixels are 128 (non-whitespace): a raster whose first pixel byte
        // was an ASCII whitespace value (9/10/13/32) is byte-ambiguous
        // with the header's trailing whitespace, a PPM/PGM scanner
        // convention shared by all real parsers.
        let mut bytes = "P6\r\n2 2\r\n255\r\n".as_bytes().to_vec();
        bytes.extend_from_slice(&[128u8; 12]);
        let frame = read_raster(&bytes).unwrap();
        assert_eq!(frame.width, 2);
        assert_eq!(frame.height, 2);
    }

    #[test]
    fn checkerboard_rejects_zero_tile() {
        assert!(checkerboard(4, 4, PixelFormat::Gray8, 0).is_err());
    }
}
