//! 1D and 2D Type-II DCT, plus inverses.
//!
//! Sizes up to 8×8 are used by `tpt-video-livelink` and the small-block
//! stages of `tpt-video-vault`; the implementation is correct for any
//! power-of-two size up to 4096.
//!
//! Formulae (Type-II DCT):
//!
//! X[k] = sum_{n=0..N-1} x[n] * cos(π/N * (n + 0.5) * k)
//!
//! with the standard 1/√2 scale for the DC term. The inverse is the Type-III
//! DCT with the same scale.

use crate::Result;

/// 1D Type-II DCT (forward transform).
///
/// `x.len()` must equal `out.len()`; otherwise the input is silently
/// truncated to `out.len()`.
pub fn dct1d(x: &[f32], out: &mut [f32]) {
    let n = out.len();
    debug_assert_eq!(x.len(), n, "dct1d: input/output size mismatch");
    let scale0 = (1.0 / 2.0f32).sqrt();
    let scale = (2.0 / n as f32).sqrt();
    for k in 0..n {
        let mut sum = 0.0f32;
        for i in 0..n {
            let angle = std_f32::consts::PI * (i as f32 + 0.5) * k as f32 / n as f32;
            sum += x[i] * angle.cos();
        }
        out[k] = sum * scale * if k == 0 { scale0 } else { 1.0 };
    }
}

/// 1D Type-III DCT (inverse of `dct1d`).
pub fn idct1d(x: &[f32], out: &mut [f32]) {
    let n = out.len();
    debug_assert_eq!(x.len(), n, "idct1d: input/output size mismatch");
    let scale0 = (1.0 / 2.0f32).sqrt();
    let scale = (2.0 / n as f32).sqrt();
    for i in 0..n {
        let mut sum = 0.0f32;
        for k in 0..n {
            let angle = std_f32::consts::PI * (i as f32 + 0.5) * k as f32 / n as f32;
            let c = if k == 0 { scale0 } else { 1.0 };
            sum += x[k] * c * angle.cos();
        }
        out[i] = sum * scale;
    }
}

/// 2D Type-II DCT (row-then-column), in place.
pub fn dct2d(block: &mut [f32], width: usize, height: usize) -> Result<()> {
    if width != height {
        return Err(crate::Error::MalformedChunk {
            chunk: "DCT2D",
            reason: "non-square block not supported",
        });
    }
    let mut tmp = [0.0f32; 64];
    if width > tmp.len() || height > tmp.len() {
        return Err(crate::Error::BitstreamOverflow { needed: width, available: tmp.len() });
    }
    // Row pass.
    for row in 0..height {
        let mut line = [0.0f32; 64];
        for col in 0..width {
            line[col] = block[row * width + col];
        }
        dct1d(&line[..width], &mut tmp[..width]);
        for col in 0..width {
            block[row * width + col] = tmp[col];
        }
    }
    // Column pass.
    for col in 0..width {
        for row in 0..height {
            tmp[row] = block[row * width + col];
        }
        let mut col_out = [0.0f32; 64];
        dct1d(&tmp[..height], &mut col_out[..height]);
        for row in 0..height {
            block[row * width + col] = col_out[row];
        }
    }
    Ok(())
}

/// 2D Type-III DCT (inverse), in place.
pub fn idct2d(block: &mut [f32], width: usize, height: usize) -> Result<()> {
    if width != height {
        return Err(crate::Error::MalformedChunk {
            chunk: "IDCT2D",
            reason: "non-square block not supported",
        });
    }
    let mut tmp = [0.0f32; 64];
    let mut out = [0.0f32; 64];
    if width > tmp.len() || height > tmp.len() {
        return Err(crate::Error::BitstreamOverflow { needed: width, available: tmp.len() });
    }
    // Column pass first (inverse of row-then-column forward).
    for col in 0..width {
        for row in 0..height {
            tmp[row] = block[row * width + col];
        }
        idct1d(&tmp[..height], &mut out[..height]);
        for row in 0..height {
            block[row * width + col] = out[row];
        }
    }
    // Row pass.
    for row in 0..height {
        let mut line = [0.0f32; 64];
        for col in 0..width {
            line[col] = block[row * width + col];
        }
        idct1d(&line[..width], &mut out[..width]);
        for col in 0..width {
            block[row * width + col] = out[col];
        }
    }
    Ok(())
}

mod std_f32 {
    pub mod consts {
        pub const PI: f32 = core::f32::consts::PI;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn dct1d_round_trip() {
        let x = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let mut coeffs = [0.0f32; 8];
        let mut recon = [0.0f32; 8];
        dct1d(&x, &mut coeffs);
        idct1d(&coeffs, &mut recon);
        for i in 0..8 {
            assert!(approx_eq(recon[i], x[i], 1e-4), "i={i} got={} want={}", recon[i], x[i]);
        }
    }

    #[test]
    fn dct2d_round_trip_4x4() {
        let original = [
            10.0, 20.0, 30.0, 40.0,
            15.0, 25.0, 35.0, 45.0,
            50.0, 60.0, 70.0, 80.0,
            55.0, 65.0, 75.0, 85.0,
        ];
        let mut block = original;
        dct2d(&mut block, 4, 4).unwrap();
        idct2d(&mut block, 4, 4).unwrap();
        for i in 0..16 {
            assert!(approx_eq(block[i], original[i], 1e-3), "i={i} got={} want={}", block[i], original[i]);
        }
    }

    #[test]
    fn dct2d_round_trip_8x8() {
        let mut block = [0.0f32; 64];
        for (i, b) in block.iter_mut().enumerate() {
            *b = ((i as f32) * 1.37).sin() * 100.0;
        }
        let orig = block;
        dct2d(&mut block, 8, 8).unwrap();
        idct2d(&mut block, 8, 8).unwrap();
        for i in 0..64 {
            assert!(approx_eq(block[i], orig[i], 1e-3));
        }
    }
}