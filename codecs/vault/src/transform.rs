//! 2D transform, quantisation and coefficient entropy coding.
//!
//! Transform sizes are 4x4 (chroma of an 8x8 block) up to 32x32. A 64x64
//! block is transformed as four 32x32 tiles: the largest transform is a
//! stated limit, not the block size. The separable DCT-II/III pair comes
//! from `tpt-dsp-core` (`dct_2d`/`idct_2d`), as todo.md 3.1 requires.
//!
//! ## Units
//!
//! `dct_2d` is unnormalised and `idct_2d` carries the matching
//! `n*n/4` factor, so the pair is an identity on the *forward output
//! divided by `n*n/4`*. Quantising that directly would make one `qp`
//! mean a different step for each transform size. Instead the quantiser
//! works in **orthonormal units** (`O = C * n * c(u) * c(v) / 2` with
//! `c(0) = 1/sqrt(2)`, else 1), where Parseval holds, so a uniform step
//! gives the same per-coefficient distortion at every size and the RDO's
//! `D` is comparable across partition choices.

use alloc::vec;
use alloc::vec::Vec;

use tpt_dsp_core::{dct_2d, idct_2d};
use tpt_media_bitstream::rice::{decode_riced, encode_riced, WriteLike};
use tpt_media_bitstream::{BitReader, WriteError};
use tpt_media_codec::{CodecError, CodecResult};

use crate::bits::{
    read_rice_unsigned, rice_len_signed, rice_len_unsigned, write_rice_unsigned, RICE_K_MAX,
};
use crate::{LEVEL_CLAMP, MAX_LEVEL, MAX_TRANSFORM};

/// Rounding offset of the dead-zone quantiser: `level = trunc(|O|/step + 0.4)`.
/// Rounding at 0.6 rather than 0.5 biases towards zero, which is where the
/// rate saving is; the RDO's `lambda` decides whether that was a good trade
/// at the *block* level.
pub const ROUND_OFFSET: f32 = 0.4;

/// `1/sqrt(2)`.
const INV_SQRT2: f32 = core::f32::consts::FRAC_1_SQRT_2;

/// Orthonormal weight of coefficient `(u, v)` in an `n x n` transform.
#[inline]
fn weight(n: usize, u: usize, v: usize) -> f32 {
    let cu = if u == 0 { INV_SQRT2 } else { 1.0 };
    let cv = if v == 0 { INV_SQRT2 } else { 1.0 };
    0.5 * n as f32 * cu * cv
}

/// The transform tile edge for a plane block of edge `m`.
#[inline]
pub(crate) fn tile_size(m: usize) -> usize {
    m.min(MAX_TRANSFORM)
}

/// Reusable float scratch for the tile transforms.
pub(crate) struct Xform {
    buf: Vec<f32>,
    scratch: Vec<f32>,
}

impl Xform {
    pub fn new() -> Self {
        Xform {
            buf: vec![0.0; MAX_TRANSFORM * MAX_TRANSFORM],
            scratch: vec![0.0; 2 * MAX_TRANSFORM],
        }
    }

    /// Forward transform + quantise the residual `src - pred` of the
    /// `t x t` tile at `(tx, ty)` inside an `m x m` plane block. Writes
    /// `t*t` levels (raster order) into `out`.
    #[allow(clippy::too_many_arguments)]
    pub fn quantise_tile(
        &mut self,
        src: &[u8],
        pred: &[u8],
        m: usize,
        tx: usize,
        ty: usize,
        t: usize,
        step: f32,
        out: &mut [i32],
    ) {
        let mut any = false;
        for y in 0..t {
            for x in 0..t {
                let i = (ty + y) * m + tx + x;
                let r = i32::from(src[i]) - i32::from(pred[i]);
                any |= r != 0;
                self.buf[y * t + x] = r as f32;
            }
        }
        if !any {
            out[..t * t].fill(0);
            return;
        }
        dct_2d(&mut self.buf[..t * t], t, t, &mut self.scratch);
        let scale = 1.0 / ((t * t) as f32 / 4.0);
        let inv_step = 1.0 / step;
        for u in 0..t {
            for v in 0..t {
                let c = self.buf[u * t + v] * scale;
                let o = c * weight(t, u, v);
                out[u * t + v] = quantise(o * inv_step);
            }
        }
    }

    /// Dequantise + inverse transform the tile's levels and add them to
    /// `pred`, writing the clamped 8-bit reconstruction into `out` (same
    /// `m x m` layout as `pred`).
    #[allow(clippy::too_many_arguments)]
    pub fn reconstruct_tile(
        &mut self,
        levels: &[i32],
        pred: &[u8],
        m: usize,
        tx: usize,
        ty: usize,
        t: usize,
        step: f32,
        out: &mut [u8],
    ) {
        if levels[..t * t].iter().all(|&l| l == 0) {
            for y in 0..t {
                let i = (ty + y) * m + tx;
                out[i..i + t].copy_from_slice(&pred[i..i + t]);
            }
            return;
        }
        for u in 0..t {
            for v in 0..t {
                let o = levels[u * t + v] as f32 * step;
                self.buf[u * t + v] = o / weight(t, u, v);
            }
        }
        idct_2d(&mut self.buf[..t * t], t, t, &mut self.scratch);
        for y in 0..t {
            for x in 0..t {
                let i = (ty + y) * m + tx + x;
                out[i] = to_u8(f32::from(pred[i]) + self.buf[y * t + x]);
            }
        }
    }
}

/// Dead-zone quantiser on a value already divided by the step. Clamped to
/// `+/-MAX_LEVEL`, which is what makes the packet-size bound provable.
#[inline]
fn quantise(a: f32) -> i32 {
    let mag = if a < 0.0 { -a } else { a };
    // `as i32` truncates toward zero and saturates; NaN maps to 0.
    let level = ((mag + ROUND_OFFSET) as i32).min(MAX_LEVEL);
    if a < 0.0 {
        -level
    } else {
        level
    }
}

/// Round to nearest, clamp to `0..=255`. NaN maps to 0.
#[inline]
pub(crate) fn to_u8(x: f32) -> u8 {
    // `as i32` on an f32 saturates and maps NaN to 0.
    let r = (x + 0.5 + 256.0) as i32 - 256;
    if x.is_nan() {
        0
    } else {
        r.clamp(0, 255) as u8
    }
}

/// Raster index -> scan-order tables for each transform size.
pub(crate) struct Scans {
    tables: [Vec<u16>; 4],
}

impl Scans {
    pub fn new() -> Self {
        Scans {
            tables: [build_scan(4), build_scan(8), build_scan(16), build_scan(32)],
        }
    }

    /// The scan for an `n x n` block, `n` in `{4, 8, 16, 32}`.
    pub fn get(&self, n: usize) -> &[u16] {
        &self.tables[(n.trailing_zeros() as usize).clamp(2, 5) - 2]
    }
}

/// Diagonal zig-zag scan: scan position -> raster position.
pub(crate) fn build_scan(n: usize) -> Vec<u16> {
    let mut out = Vec::with_capacity(n * n);
    for d in 0..(2 * n - 1) {
        let lo = d.saturating_sub(n - 1);
        let hi = d.min(n - 1);
        if d % 2 == 0 {
            // Up-right: row descending.
            for row in (lo..=hi).rev() {
                out.push((row * n + (d - row)) as u16);
            }
        } else {
            for row in lo..=hi {
                out.push((row * n + (d - row)) as u16);
            }
        }
    }
    out
}

/// Bits used for the `last` field of an `n x n` block: `log2(n*n)`.
#[inline]
pub(crate) fn last_bits(n: usize) -> u32 {
    2 * n.trailing_zeros()
}

/// Coefficient block syntax, per `t x t` tile:
///
/// ```text
/// 1 bit    coded   (0: all levels are zero)
/// 3 bits   k       Rice parameter, 0..=7, chosen to minimise this block
/// log2(t*t) bits   last   highest scan position with a non-zero level
/// then for each non-zero level in scan order up to `last`:
///          run     unsigned Rice(k): zeros skipped since the previous level
///          level   signed Rice(k)
/// ```
///
/// `last` is its own terminator, so decoder work per block is bounded by
/// `t*t` regardless of the bits.
pub(crate) fn write_coefs<W: WriteLike>(
    w: &mut W,
    levels: &[i32],
    n: usize,
    scan: &[u16],
) -> Result<(), WriteError> {
    let mut last = 0usize;
    let mut any = false;
    for (p, &r) in scan.iter().enumerate() {
        if levels[r as usize] != 0 {
            last = p;
            any = true;
        }
    }
    w.write_bits(1, u64::from(any))?;
    if !any {
        return Ok(());
    }
    let mut best_k = 0u32;
    let mut best_cost = u64::MAX;
    for k in 0..=RICE_K_MAX {
        let mut cost = 0u64;
        let mut run = 0u32;
        for &r in &scan[..=last] {
            let v = levels[r as usize];
            if v == 0 {
                run += 1;
            } else {
                cost += u64::from(rice_len_unsigned(run, k) + rice_len_signed(v, k));
                run = 0;
            }
        }
        if cost < best_cost {
            best_cost = cost;
            best_k = k;
        }
    }
    w.write_bits(3, u64::from(best_k))?;
    w.write_bits(last_bits(n), last as u64)?;
    let mut run = 0u32;
    for &r in &scan[..=last] {
        let v = levels[r as usize];
        if v == 0 {
            run += 1;
        } else {
            write_rice_unsigned(w, best_k, run)?;
            encode_riced(w, best_k, i64::from(v))?;
            run = 0;
        }
    }
    Ok(())
}

/// Inverse of [`write_coefs`]. `levels` (`n*n`) is cleared first.
pub(crate) fn read_coefs(
    r: &mut BitReader,
    levels: &mut [i32],
    n: usize,
    scan: &[u16],
) -> CodecResult<()> {
    levels[..n * n].fill(0);
    let bad = |_| CodecError::MalformedPacket;
    if r.read(1).map_err(bad)? == 0 {
        return Ok(());
    }
    let k = r.read(3).map_err(bad)? as u32;
    let last = r.read(last_bits(n)).map_err(bad)? as usize;
    // `last < n*n` by construction of the field width.
    let mut pos = 0usize;
    loop {
        let run = read_rice_unsigned(r, k).map_err(bad)? as usize;
        pos += run;
        if pos > last {
            return Err(CodecError::MalformedPacket);
        }
        let level = decode_riced(r, k).map_err(bad)?;
        let level = i32::try_from(level).map_err(|_| CodecError::MalformedPacket)?;
        levels[scan[pos] as usize] = level.clamp(-LEVEL_CLAMP, LEVEL_CLAMP);
        pos += 1;
        if pos > last {
            return Ok(());
        }
    }
}

/// Worst-case bits one Rice code can cost under the block's cheapest
/// parameter: bounded by the cost at `k = RICE_K_MAX` for the largest
/// foldable value (`fold(MAX_LEVEL)`), which also covers any run length.
pub(crate) const MAX_RICE_BITS: u32 = ((2 * MAX_LEVEL as u32) >> RICE_K_MAX) + 1 + RICE_K_MAX;
