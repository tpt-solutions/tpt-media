//! The picture model: 8-bit planes, any dimensions that are multiples of 8.

use alloc::vec;
use alloc::vec::Vec;

use tpt_media_codec::{CodecError, CodecResult};
use tpt_media_core::{PixelFormat, Timestamp, VideoFrame};

use crate::{is_coded_format, MIN_BLOCK};

/// One 8-bit plane, dense (`stride == w`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Plane {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>,
}

impl Plane {
    pub fn new(w: usize, h: usize, fill: u8) -> Self {
        Plane {
            w,
            h,
            data: vec![fill; w * h],
        }
    }

    /// Copy the `bw x bh` rectangle at `(x0, y0)` into a dense buffer. The
    /// caller guarantees the rectangle is inside the plane.
    pub fn load_block(&self, x0: usize, y0: usize, bw: usize, bh: usize, out: &mut [u8]) {
        for r in 0..bh {
            let src = (y0 + r) * self.w + x0;
            out[r * bw..(r + 1) * bw].copy_from_slice(&self.data[src..src + bw]);
        }
    }

    /// Inverse of [`Plane::load_block`].
    pub fn store_block(&mut self, x0: usize, y0: usize, bw: usize, bh: usize, src: &[u8]) {
        for r in 0..bh {
            let dst = (y0 + r) * self.w + x0;
            self.data[dst..dst + bw].copy_from_slice(&src[r * bw..(r + 1) * bw]);
        }
    }

    /// Sample clamped to the plane's edges. Motion vectors may legitimately
    /// point outside the picture; both sides clamp identically.
    #[inline]
    pub fn at_clamped(&self, x: isize, y: isize) -> u8 {
        if self.w == 0 || self.h == 0 {
            return 0;
        }
        let cx = x.clamp(0, self.w as isize - 1) as usize;
        let cy = y.clamp(0, self.h as isize - 1) as usize;
        self.data[cy * self.w + cx]
    }
}

/// A reconstructed or source picture: luma plus (4:2:0) two chroma planes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Picture {
    pub width: usize,
    pub height: usize,
    pub format: PixelFormat,
    /// `[Y, U, V]`; U and V are zero-sized for `Gray8`.
    pub planes: [Plane; 3],
}

impl Picture {
    pub fn new(width: usize, height: usize, format: PixelFormat, fill: u8) -> Self {
        let (cw, ch) = (width / 2, height / 2);
        let planes = if format == PixelFormat::Yuv420 {
            [
                Plane::new(width, height, fill),
                Plane::new(cw, ch, 128),
                Plane::new(cw, ch, 128),
            ]
        } else {
            [
                Plane::new(width, height, fill),
                Plane::new(0, 0, 0),
                Plane::new(0, 0, 0),
            ]
        };
        Picture {
            width,
            height,
            format,
            planes,
        }
    }

    /// Number of coded planes (1 or 3).
    #[inline]
    pub fn nplanes(&self) -> usize {
        if self.format == PixelFormat::Yuv420 {
            3
        } else {
            1
        }
    }
}

/// Validate frame geometry: non-zero multiples of [`MIN_BLOCK`].
pub(crate) fn valid_dims(w: usize, h: usize) -> bool {
    w != 0 && h != 0 && w % MIN_BLOCK == 0 && h % MIN_BLOCK == 0
}

/// Build a picture from a [`VideoFrame`], validating its buffer size.
pub(crate) fn picture_from_frame(f: &VideoFrame) -> CodecResult<Picture> {
    if !is_coded_format(f.format) {
        return Err(CodecError::Unsupported("vault pixel format"));
    }
    let (w, h) = (f.width as usize, f.height as usize);
    if !valid_dims(w, h) {
        return Err(CodecError::InvalidConfig(
            "vault frame dimensions must be non-zero multiples of 8",
        ));
    }
    let stride = f.stride as usize;
    if stride < w {
        return Err(CodecError::InvalidConfig("vault stride < width"));
    }
    let (cw, ch) = (w / 2, h / 2);
    let luma_len = stride
        .checked_mul(h)
        .ok_or(CodecError::InvalidConfig("vault frame too large"))?;
    let cstride = stride / 2;
    let chroma_len = if f.format == PixelFormat::Yuv420 {
        cstride * ch
    } else {
        0
    };
    if f.pixels.len() < luma_len + 2 * chroma_len {
        return Err(CodecError::InvalidConfig("vault frame buffer too small"));
    }
    if f.format == PixelFormat::Yuv420 && cstride < cw {
        return Err(CodecError::InvalidConfig("vault chroma stride < width / 2"));
    }
    let mut pic = Picture::new(w, h, f.format, 0);
    for y in 0..h {
        let s = y * stride;
        pic.planes[0].data[y * w..(y + 1) * w].copy_from_slice(&f.pixels[s..s + w]);
    }
    if f.format == PixelFormat::Yuv420 {
        for c in 0..2 {
            let base = luma_len + c * cstride * ch;
            for y in 0..ch {
                let s = base + y * cstride;
                pic.planes[1 + c].data[y * cw..(y + 1) * cw].copy_from_slice(&f.pixels[s..s + cw]);
            }
        }
    }
    Ok(pic)
}

/// Serialise back into a [`VideoFrame`] with the layout
/// [`VideoFrame::alloc`] produces.
pub(crate) fn picture_to_frame(pic: &Picture, pts: Timestamp) -> VideoFrame {
    let mut f = VideoFrame::alloc(pic.width as u32, pic.height as u32, pic.format);
    f.frame.pts = pts;
    let stride = f.stride as usize;
    for y in 0..pic.height {
        let d = y * stride;
        f.pixels[d..d + pic.width]
            .copy_from_slice(&pic.planes[0].data[y * pic.width..(y + 1) * pic.width]);
    }
    if pic.format == PixelFormat::Yuv420 {
        let (cw, ch) = (pic.width / 2, pic.height / 2);
        for c in 0..2 {
            let base = stride * pic.height + c * (stride / 2) * ch;
            for y in 0..ch {
                let d = base + y * (stride / 2);
                f.pixels[d..d + cw].copy_from_slice(&pic.planes[1 + c].data[y * cw..(y + 1) * cw]);
            }
        }
    }
    f
}
