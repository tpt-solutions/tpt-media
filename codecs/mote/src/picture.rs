//! Picture model and the patch grid.
//!
//! A Mote picture is one (`Gray8`) or three (`Yuv420`) 8-bit planes, each
//! tiled exactly by [`PATCH`] x [`PATCH`] patches. Frame dimensions are
//! multiples of 16, so the *chroma* planes of a 4:2:0 picture (half
//! resolution) are multiples of 8 and tile exactly too: there is no
//! padding and no partial patch anywhere, hence no padding-related rate
//! or quality cliff.
//!
//! Patches are numbered by one global index `g`: every patch of plane 0
//! (raster order), then plane 1, then plane 2. That numbering is the
//! wire order, shared verbatim by encoder and decoder.

use alloc::vec;
use alloc::vec::Vec;

use tpt_media_codec::{CodecError, CodecResult};
use tpt_media_core::{PixelFormat, Timestamp, VideoFrame};

use crate::{MAX_LONG_EDGE, MAX_SHORT_EDGE, PATCH, PATCH_DIM};

/// Dimension alignment: luma multiples of 16 so 4:2:0 chroma is a
/// multiple of [`PATCH`].
pub(crate) const ALIGN: usize = 2 * PATCH;

/// True for the pixel formats Mote codes.
#[inline]
pub fn is_coded_format(format: PixelFormat) -> bool {
    matches!(format, PixelFormat::Gray8 | PixelFormat::Yuv420)
}

/// Validate a frame geometry against the supported-dimension rules
/// (todo.md 34 "define supported dimensions").
pub(crate) fn validate_dimensions(width: u32, height: u32) -> CodecResult<()> {
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 || w % ALIGN != 0 || h % ALIGN != 0 {
        return Err(CodecError::InvalidConfig(
            "mote width/height must be non-zero multiples of 16",
        ));
    }
    let (long, short) = if w >= h { (w, h) } else { (h, w) };
    if long > MAX_LONG_EDGE || short > MAX_SHORT_EDGE {
        return Err(CodecError::InvalidConfig(
            "mote supports at most 480p (long edge <= 848, short edge <= 480)",
        ));
    }
    Ok(())
}

/// One 8-bit plane with no padding (`stride == w`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Plane {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>,
}

/// A whole picture: one or three planes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Picture {
    pub width: usize,
    pub height: usize,
    pub format: PixelFormat,
    pub planes: Vec<Plane>,
}

impl Picture {
    pub fn new(width: usize, height: usize, format: PixelFormat, fill: u8) -> Self {
        let mut planes = vec![Plane {
            w: width,
            h: height,
            data: vec![fill; width * height],
        }];
        if format == PixelFormat::Yuv420 {
            for _ in 0..2 {
                planes.push(Plane {
                    w: width / 2,
                    h: height / 2,
                    data: vec![fill; (width / 2) * (height / 2)],
                });
            }
        }
        Picture {
            width,
            height,
            format,
            planes,
        }
    }

    /// Build a picture from a [`VideoFrame`], validating that its buffer
    /// is at least the size the format and dimensions imply.
    pub fn from_frame(f: &VideoFrame) -> CodecResult<Picture> {
        if !is_coded_format(f.format) {
            return Err(CodecError::Unsupported("mote pixel format"));
        }
        validate_dimensions(f.width, f.height)?;
        let (w, h) = (f.width as usize, f.height as usize);
        let stride = f.stride as usize;
        if stride < w {
            return Err(CodecError::InvalidConfig("mote stride < width"));
        }
        let luma_len = stride * h;
        let (cw, ch, cstride) = (w / 2, h / 2, stride / 2);
        let chroma_len = if f.format == PixelFormat::Yuv420 {
            cstride * ch
        } else {
            0
        };
        if f.pixels.len() < luma_len + 2 * chroma_len {
            return Err(CodecError::InvalidConfig("mote frame buffer too small"));
        }
        let mut pic = Picture::new(w, h, f.format, 0);
        for y in 0..h {
            pic.planes[0].data[y * w..(y + 1) * w]
                .copy_from_slice(&f.pixels[y * stride..y * stride + w]);
        }
        if f.format == PixelFormat::Yuv420 {
            for c in 0..2 {
                let base = luma_len + c * chroma_len;
                for y in 0..ch {
                    let src = base + y * cstride;
                    pic.planes[1 + c].data[y * cw..(y + 1) * cw]
                        .copy_from_slice(&f.pixels[src..src + cw]);
                }
            }
        }
        Ok(pic)
    }

    /// Serialise into a [`VideoFrame`] using the exact layout
    /// [`VideoFrame::alloc`] produces.
    pub fn to_frame(&self, pts: Timestamp) -> VideoFrame {
        let mut f = VideoFrame::alloc(self.width as u32, self.height as u32, self.format);
        f.frame.pts = pts;
        let stride = f.stride as usize;
        for y in 0..self.height {
            f.pixels[y * stride..y * stride + self.width]
                .copy_from_slice(&self.planes[0].data[y * self.width..(y + 1) * self.width]);
        }
        if self.format == PixelFormat::Yuv420 {
            let (cw, ch, cstride) = (self.width / 2, self.height / 2, stride / 2);
            for c in 0..2 {
                let base = stride * self.height + c * cstride * ch;
                for y in 0..ch {
                    f.pixels[base + y * cstride..base + y * cstride + cw]
                        .copy_from_slice(&self.planes[1 + c].data[y * cw..(y + 1) * cw]);
                }
            }
        }
        f
    }
}

/// The patch tiling of a picture geometry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PatchGrid {
    /// Per plane: (patches across, patches down, first global index).
    planes: [(usize, usize, usize); 3],
    nplanes: usize,
    total: usize,
}

impl PatchGrid {
    pub fn new(width: usize, height: usize, format: PixelFormat) -> Self {
        let mut planes = [(0usize, 0usize, 0usize); 3];
        let (wp, hp) = (width / PATCH, height / PATCH);
        planes[0] = (wp, hp, 0);
        let mut total = wp * hp;
        let mut nplanes = 1;
        if format == PixelFormat::Yuv420 {
            let (cwp, chp) = (width / 2 / PATCH, height / 2 / PATCH);
            planes[1] = (cwp, chp, total);
            total += cwp * chp;
            planes[2] = (cwp, chp, total);
            total += cwp * chp;
            nplanes = 3;
        }
        PatchGrid {
            planes,
            nplanes,
            total,
        }
    }

    /// Number of patches in a frame.
    #[inline]
    pub fn total(&self) -> usize {
        self.total
    }

    /// `(plane, x0, y0)` of patch `g`, in samples. `g < total`.
    #[inline]
    fn locate(&self, g: usize) -> (usize, usize, usize) {
        let mut p = self.nplanes - 1;
        while p > 0 && g < self.planes[p].2 {
            p -= 1;
        }
        let (wp, _, first) = self.planes[p];
        let local = g - first;
        ((p), (local % wp) * PATCH, (local / wp) * PATCH)
    }

    /// Copy patch `g` out of `pic` into `out`.
    #[inline]
    pub fn load(&self, pic: &Picture, g: usize, out: &mut [u8; PATCH_DIM]) {
        let (p, x0, y0) = self.locate(g);
        let plane = &pic.planes[p];
        for r in 0..PATCH {
            let src = (y0 + r) * plane.w + x0;
            out[r * PATCH..(r + 1) * PATCH].copy_from_slice(&plane.data[src..src + PATCH]);
        }
    }

    /// Copy `patch` into `pic` at patch `g`.
    #[inline]
    pub fn store(&self, pic: &mut Picture, g: usize, patch: &[u8]) {
        let (p, x0, y0) = self.locate(g);
        let plane = &mut pic.planes[p];
        for r in 0..PATCH {
            let dst = (y0 + r) * plane.w + x0;
            plane.data[dst..dst + PATCH].copy_from_slice(&patch[r * PATCH..(r + 1) * PATCH]);
        }
    }
}
