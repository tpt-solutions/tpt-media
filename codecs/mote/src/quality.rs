//! Mote's quality metric (todo.md 34 "define quality metric").
//!
//! The metric is defined over a *decoded frame versus its source frame*,
//! both 8-bit, both of the same geometry and format:
//!
//! - **Primary: luma PSNR** - `10 * log10(255^2 / MSE_luma)` in dB, where
//!   `MSE_luma` is the mean squared error over the `w x h` luma samples.
//!   Luma carries the structure a patch dictionary is being judged on,
//!   and it is the number the docs and tests quote unless stated.
//! - **Secondary: all-plane PSNR** - the same formula over every sample
//!   of every plane (luma plus both 4:2:0 chroma planes, weighted by
//!   sample count). For `Gray8` it equals the luma figure.
//! - **Mean absolute error** over all planes, in grey levels, because a
//!   PSNR average hides whether error is spread thin or concentrated.
//!
//! Identical frames have infinite PSNR; the report caps it at
//! [`PSNR_CAP_DB`] so it stays a finite, comparable number.
//!
//! This is a *fidelity* metric. It says nothing about how a patch
//! dictionary's blockiness reads to a viewer, and PSNR is known to
//! correlate poorly with perception on quantised-patch artefacts;
//! treat it as a regression yardstick, not a claim of visual quality.
//! The workspace-wide video metrics (todo.md 37) live in `tpt-cli`.

use tpt_media_codec::{CodecError, CodecResult};
use tpt_media_core::{PixelFormat, VideoFrame};

/// PSNR reported for a zero-error frame.
pub const PSNR_CAP_DB: f64 = 99.0;

/// Squared-error and absolute-error totals for one frame pair.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QualityReport {
    /// Mean squared error over the luma plane.
    pub luma_mse: f64,
    /// Mean squared error over every plane.
    pub all_planes_mse: f64,
    /// Mean absolute error over every plane, in grey levels.
    pub mean_abs_error: f64,
}

impl QualityReport {
    /// Luma PSNR in dB (the primary figure), capped at [`PSNR_CAP_DB`].
    #[cfg(feature = "std")]
    pub fn luma_psnr_db(&self) -> f64 {
        psnr_from_mse(self.luma_mse)
    }

    /// All-plane PSNR in dB, capped at [`PSNR_CAP_DB`].
    #[cfg(feature = "std")]
    pub fn all_planes_psnr_db(&self) -> f64 {
        psnr_from_mse(self.all_planes_mse)
    }
}

/// `10 log10(255^2 / mse)`, capped.
#[cfg(feature = "std")]
pub fn psnr_from_mse(mse: f64) -> f64 {
    if mse <= 0.0 {
        return PSNR_CAP_DB;
    }
    (10.0 * (255.0f64 * 255.0 / mse).log10()).min(PSNR_CAP_DB)
}

/// Compare a decoded frame against its source.
pub fn compare(source: &VideoFrame, decoded: &VideoFrame) -> CodecResult<QualityReport> {
    if source.format != decoded.format
        || source.width != decoded.width
        || source.height != decoded.height
    {
        return Err(CodecError::InvalidConfig(
            "mote quality: frames differ in geometry or format",
        ));
    }
    let (w, h) = (source.width as usize, source.height as usize);
    if w == 0 || h == 0 {
        return Err(CodecError::InvalidConfig("mote quality: empty frame"));
    }
    let mut planes: [(usize, usize, usize, usize); 3] = [(0, 0, 0, 0); 3];
    let luma_stride = source.stride as usize;
    if luma_stride != decoded.stride as usize || luma_stride < w {
        return Err(CodecError::InvalidConfig("mote quality: bad stride"));
    }
    planes[0] = (0, w, h, luma_stride);
    let mut n = 1;
    let mut needed = luma_stride * h;
    match source.format {
        PixelFormat::Gray8 => {}
        PixelFormat::Yuv420 => {
            let (cw, ch, cs) = (w / 2, h / 2, luma_stride / 2);
            planes[1] = (luma_stride * h, cw, ch, cs);
            planes[2] = (luma_stride * h + cs * ch, cw, ch, cs);
            needed += 2 * cs * ch;
            n = 3;
        }
        _ => return Err(CodecError::Unsupported("mote pixel format")),
    }
    if source.pixels.len() < needed || decoded.pixels.len() < needed {
        return Err(CodecError::InvalidConfig("mote quality: buffer too small"));
    }
    let (mut luma_sq, mut all_sq, mut all_abs, mut count) = (0u64, 0u64, 0u64, 0u64);
    for (p, &(base, pw, ph, stride)) in planes[..n].iter().enumerate() {
        for y in 0..ph {
            let row = base + y * stride;
            for x in 0..pw {
                let d = i64::from(source.pixels[row + x]) - i64::from(decoded.pixels[row + x]);
                let sq = (d * d) as u64;
                if p == 0 {
                    luma_sq += sq;
                }
                all_sq += sq;
                all_abs += d.unsigned_abs();
                count += 1;
            }
        }
    }
    Ok(QualityReport {
        luma_mse: luma_sq as f64 / (w * h) as f64,
        all_planes_mse: all_sq as f64 / count as f64,
        mean_abs_error: all_abs as f64 / count as f64,
    })
}
