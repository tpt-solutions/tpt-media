//! Prediction: intra (four modes) and inter with bilinear half-pel motion
//! compensation.
//!
//! Sub-pel interpolation is deliberately the *simplest* filter there is: a
//! two-tap average for a half-pel offset and a four-sample average for the
//! diagonal position (CONTRIBUTING section 3 forbids the longer 6/8-tap
//! interpolation filters). It buys less than a longer filter would; that
//! is a stated trade, not an oversight.

use crate::picture::Plane;

/// Intra prediction modes, in wire order (2 bits).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum IntraMode {
    /// Mean of the available top row and left column (128 if none).
    Dc = 0,
    /// Copy the row above down (needs `y > 0`).
    Vertical = 1,
    /// Copy the column to the left across (needs `x > 0`).
    Horizontal = 2,
    /// The plane through the left, top and top-left neighbours:
    /// `left[y] + top[x] - top_left`, clamped (needs `x > 0 && y > 0`).
    Plane = 3,
}

impl IntraMode {
    /// Every mode in wire order.
    pub const ALL: [IntraMode; 4] = [
        IntraMode::Dc,
        IntraMode::Vertical,
        IntraMode::Horizontal,
        IntraMode::Plane,
    ];

    pub(crate) fn from_bits(v: u8) -> IntraMode {
        match v & 3 {
            0 => IntraMode::Dc,
            1 => IntraMode::Vertical,
            2 => IntraMode::Horizontal,
            _ => IntraMode::Plane,
        }
    }
}

/// Whether `mode` may be signalled for a block whose top-left sample is
/// `(x0, y0)` in its plane. The decoder re-checks on read, so an
/// unavailable mode in a hostile packet degrades to DC rather than
/// reading outside the picture.
#[inline]
pub fn intra_mode_available(mode: IntraMode, x0: usize, y0: usize) -> bool {
    match mode {
        IntraMode::Dc => true,
        IntraMode::Vertical => y0 > 0,
        IntraMode::Horizontal => x0 > 0,
        IntraMode::Plane => x0 > 0 && y0 > 0,
    }
}

/// Fill `out` (dense `n x n`) with the intra prediction for `mode` at
/// `(x0, y0)`, read from the *reconstructed* (pre-loop-filter) plane. Only
/// the row above and the column to the left are ever read, both of which
/// precede the block in z-order, so the prediction is closed-loop.
pub(crate) fn intra_predict(
    plane: &Plane,
    x0: usize,
    y0: usize,
    n: usize,
    mode: IntraMode,
    out: &mut [u8],
) {
    let mode = if intra_mode_available(mode, x0, y0) {
        mode
    } else {
        IntraMode::Dc
    };
    let top = |x: usize| u32::from(plane.at_clamped((x0 + x) as isize, y0 as isize - 1));
    let left = |y: usize| u32::from(plane.at_clamped(x0 as isize - 1, (y0 + y) as isize));
    match mode {
        IntraMode::Dc => {
            let mut sum = 0u32;
            let mut cnt = 0u32;
            if y0 > 0 {
                sum += (0..n).map(top).sum::<u32>();
                cnt += n as u32;
            }
            if x0 > 0 {
                sum += (0..n).map(left).sum::<u32>();
                cnt += n as u32;
            }
            let v = (sum + cnt / 2).checked_div(cnt).unwrap_or(128);
            out[..n * n].fill(v as u8);
        }
        IntraMode::Vertical => {
            for y in 0..n {
                for x in 0..n {
                    out[y * n + x] = top(x) as u8;
                }
            }
        }
        IntraMode::Horizontal => {
            for y in 0..n {
                let l = left(y) as u8;
                out[y * n..(y + 1) * n].fill(l);
            }
        }
        IntraMode::Plane => {
            let tl = i32::from(plane.at_clamped(x0 as isize - 1, y0 as isize - 1));
            for y in 0..n {
                let l = left(y) as i32;
                for x in 0..n {
                    let v = l + top(x) as i32 - tl;
                    out[y * n + x] = v.clamp(0, 255) as u8;
                }
            }
        }
    }
}

/// Motion-compensated fetch of a `w x h` block whose top-left sample in the
/// current picture is `(x0, y0)`, displaced by `(mvx, mvy)` in **half-pel**
/// units. Edge-clamped, so any vector is safe and encoder and decoder agree
/// on what an off-picture vector means.
///
/// - integer position: a copy;
/// - half-pel in one axis: `(a + b + 1) >> 1`;
/// - half-pel in both: `(a + b + c + d + 2) >> 2`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn mc_block(
    plane: &Plane,
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
    mvx: i32,
    mvy: i32,
    out: &mut [u8],
) {
    let ox = x0 as isize + (mvx >> 1) as isize;
    let oy = y0 as isize + (mvy >> 1) as isize;
    let fx = (mvx & 1) as usize;
    let fy = (mvy & 1) as usize;
    let inside = ox >= 0
        && oy >= 0
        && (ox as usize) + w + fx <= plane.w
        && (oy as usize) + h + fy <= plane.h;
    if inside {
        let (ox, oy) = (ox as usize, oy as usize);
        let pw = plane.w;
        let d = &plane.data;
        for r in 0..h {
            let row = (oy + r) * pw + ox;
            let orow = &mut out[r * w..(r + 1) * w];
            match (fx, fy) {
                (0, 0) => orow.copy_from_slice(&d[row..row + w]),
                (1, 0) => {
                    for c in 0..w {
                        orow[c] =
                            ((u16::from(d[row + c]) + u16::from(d[row + c + 1]) + 1) >> 1) as u8;
                    }
                }
                (0, _) => {
                    for c in 0..w {
                        orow[c] =
                            ((u16::from(d[row + c]) + u16::from(d[row + pw + c]) + 1) >> 1) as u8;
                    }
                }
                _ => {
                    for c in 0..w {
                        let s = u16::from(d[row + c])
                            + u16::from(d[row + c + 1])
                            + u16::from(d[row + pw + c])
                            + u16::from(d[row + pw + c + 1]);
                        orow[c] = ((s + 2) >> 2) as u8;
                    }
                }
            }
        }
        return;
    }
    for r in 0..h {
        let sy = oy + r as isize;
        for c in 0..w {
            let sx = ox + c as isize;
            let a = u16::from(plane.at_clamped(sx, sy));
            out[r * w + c] = match (fx, fy) {
                (0, 0) => a as u8,
                (1, 0) => ((a + u16::from(plane.at_clamped(sx + 1, sy)) + 1) >> 1) as u8,
                (0, _) => ((a + u16::from(plane.at_clamped(sx, sy + 1)) + 1) >> 1) as u8,
                _ => {
                    let s = a
                        + u16::from(plane.at_clamped(sx + 1, sy))
                        + u16::from(plane.at_clamped(sx, sy + 1))
                        + u16::from(plane.at_clamped(sx + 1, sy + 1));
                    ((s + 2) >> 2) as u8
                }
            };
        }
    }
}

/// Chroma motion vector, in half-*chroma*-pel units, from a luma vector in
/// half-luma-pel units: one chroma sample spans two luma samples, so the
/// displacement halves. Floor (arithmetic shift), never signalled.
#[inline]
pub(crate) fn chroma_mv(v: i32) -> i32 {
    v >> 1
}

/// Sum of absolute differences.
#[inline]
pub(crate) fn sad(a: &[u8], b: &[u8]) -> u32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (i32::from(*x) - i32::from(*y)).unsigned_abs())
        .sum()
}

/// Sum of squared differences (the RDO's distortion `D`).
#[inline]
pub(crate) fn ssd(a: &[u8], b: &[u8]) -> u64 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| {
            let d = i64::from(*x) - i64::from(*y);
            (d * d) as u64
        })
        .sum()
}
