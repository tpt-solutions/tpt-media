//! Reusable separable 2D block DCT-II engine (single frame).
//!
//! Avenue A of the research plan (`todo.md` §56): a *separable* 2D DCT-II
//! over a configurable block size B, built purely from `tpt-dsp-core`'s 1D
//! [`tpt_dsp_core::dct_ii`]/[`tpt_dsp_core::dct_iii`] — the workspace rule
//! that `tpt-dsp` is never modified for research (`todo.md` §56) is what
//! makes this a row+column pass rather than a native 2D call.
//!
//! Convention and scaling:
//!
//! * Forward: DCT-II along each row, then DCT-II along each column of every
//!   B×B tile (each 1D transform unnormalized, `X[k] = Σ x[n]·cos((π/B)(n+½)k)`).
//! * Inverse: DCT-III along each column, then DCT-III along each row, then
//!   scaled by `4/B²`. This is exact because the 1D DCT-II followed by the
//!   1D DCT-III recovers `(B/2)·x`, so the 2D round trip has the scalar
//!   `(2/B)²` (see `dct_ii_then_iii_roundtrips` in `tpt-dsp-core`).
//!
//! The output layout keeps every B×B tile's coefficients in the tile's own
//! spatial location (the block's DC in its top-left corner, AC coefficients
//! to the right and below), which is what makes `block_dct`'s coefficient
//! stream spatiotemporally aligned and directly comparable to `baseline`.

/// A single-frame separable 2D block DCT-II / inverse, with reusable scratch.
pub struct BlockDct2D {
    block: usize,
    // Reused scratch, sized once to the largest frame seen (offline profile —
    // allocation here is fine, and it is *not* in the hot path of anything
    // real-time; this crate is research code per its crate-level docs).
    tmp: Vec<f32>,
    line: Vec<f32>,
    col: Vec<f32>,
    block_buf: Vec<f32>,
    frame_len: usize,
}

#[allow(clippy::len_without_is_empty)]
impl BlockDct2D {
    /// Build the engine for `block`-sized square tiles.
    ///
    /// # Panics
    ///
    /// Panics if `block == 0`.
    pub fn new(block: usize) -> Self {
        assert!(block > 0, "block size must be positive");
        BlockDct2D {
            block,
            tmp: Vec::new(),
            line: Vec::new(),
            col: Vec::new(),
            block_buf: Vec::new(),
            frame_len: 0,
        }
    }

    fn ensure_scratch(&mut self, frame_len: usize, w: usize, h: usize) {
        if self.frame_len != frame_len {
            self.tmp = vec![0.0; frame_len];
            self.line = vec![0.0; self.block];
            self.col = vec![0.0; self.block];
            self.block_buf = vec![0.0; self.block];
            self.frame_len = frame_len;
        }
        assert!(
            w % self.block == 0 && h % self.block == 0,
            "frame dimensions {w}x{h} must be divisible by block size {}",
            self.block
        );
        assert!(
            frame_len == w * h,
            "frame length {frame_len} must equal width*height {w}*{h}"
        );
    }

    /// 2D DCT-II of a single `w`×`h` frame into `out` (same length).
    ///
    /// # Panics
    ///
    /// Panics if `frame.len() != w*h`, `out.len() != w*h`, or `w`/`h` are not
    /// divisible by the block size.
    pub fn forward(&mut self, frame: &[f32], w: usize, h: usize, out: &mut [f32]) {
        let frame_len = w * h;
        assert_eq!(
            frame.len(),
            frame_len,
            "frame length must equal width*height"
        );
        assert_eq!(
            out.len(),
            frame_len,
            "output length must equal width*height"
        );
        self.ensure_scratch(frame_len, w, h);
        let b = self.block;

        // Rows: dct_ii along each row segment of each tile.
        for by in 0..(h / b) {
            for bx in 0..(w / b) {
                for j in 0..b {
                    let dst_row = by * b + j;
                    let src_base = (by * b + j) * w + bx * b;
                    self.line.copy_from_slice(&frame[src_base..src_base + b]);
                    tpt_dsp_core::dct_ii(&self.line, &mut self.block_buf);
                    for (i, c) in self.block_buf.iter().enumerate() {
                        self.tmp[dst_row * w + bx * b + i] = *c;
                    }
                }
            }
        }

        // Columns: dct_ii along each column segment of each tile.
        for by in 0..(h / b) {
            for bx in 0..(w / b) {
                for i in 0..b {
                    for j in 0..b {
                        self.col[j] = self.tmp[(by * b + j) * w + bx * b + i];
                    }
                    tpt_dsp_core::dct_ii(&self.col, &mut self.block_buf);
                    for (j, c) in self.block_buf.iter().enumerate() {
                        out[(by * b + j) * w + bx * b + i] = *c;
                    }
                }
            }
        }
    }

    /// Inverse 2D DCT of `coeffs` (a `forward` output) back into `out`.
    ///
    /// # Panics
    ///
    /// Same as [`Self::forward`].
    pub fn inverse(&mut self, coeffs: &[f32], w: usize, h: usize, out: &mut [f32]) {
        let frame_len = w * h;
        assert_eq!(
            coeffs.len(),
            frame_len,
            "coeffs length must equal width*height"
        );
        assert_eq!(
            out.len(),
            frame_len,
            "output length must equal width*height"
        );
        self.ensure_scratch(frame_len, w, h);
        let b = self.block;

        // Columns first (inverse of the forward's column pass).
        for by in 0..(h / b) {
            for bx in 0..(w / b) {
                for i in 0..b {
                    for j in 0..b {
                        self.col[j] = coeffs[(by * b + j) * w + bx * b + i];
                    }
                    tpt_dsp_core::dct_iii(&self.col, &mut self.block_buf);
                    for (j, c) in self.block_buf.iter().enumerate() {
                        self.tmp[(by * b + j) * w + bx * b + i] = *c;
                    }
                }
            }
        }

        // Rows, then the 2D scale factor (2/B)² = 4/B².
        let scale = 4.0 / (b * b) as f32;
        for by in 0..(h / b) {
            for bx in 0..(w / b) {
                for j in 0..b {
                    let dst_row = by * b + j;
                    let base = (by * b + j) * w + bx * b;
                    self.line.copy_from_slice(&self.tmp[base..base + b]);
                    tpt_dsp_core::dct_iii(&self.line, &mut self.block_buf);
                    for (i, c) in self.block_buf.iter().enumerate() {
                        out[dst_row * w + bx * b + i] = c * scale;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_data(w: usize, h: usize) -> Vec<f32> {
        // A smooth-but-not-trivial field: vertical ramp + sinusoid texture.
        (0..w * h)
            .map(|i| {
                let x = (i % w) as f32;
                let y = (i / w) as f32;
                let ramp = x / w as f32;
                let wave = (0.31 * x + 0.17 * y).sin();
                0.5 * ramp + 0.5 * (0.5 * wave + 0.5)
            })
            .collect()
    }

    #[test]
    fn block_dct_round_trips_single_frame() {
        let w = 16;
        let h = 16;
        for b in [4usize, 8] {
            let x = frame_data(w, h);
            let mut c = vec![0.0f32; w * h];
            let mut y = vec![0.0f32; w * h];
            let mut dct = BlockDct2D::new(b);
            dct.forward(&x, w, h, &mut c);
            dct.inverse(&c, w, h, &mut y);
            for (a, z) in x.iter().zip(y.iter()) {
                let err = (a - z).abs();
                assert!(err < 1e-3, "err={err} at b={b}");
            }
        }
    }

    #[test]
    fn block_size_one_round_trips() {
        let w = 8;
        let h = 8;
        let x = frame_data(w, h);
        let mut c = vec![0.0f32; w * h];
        let mut y = vec![0.0f32; w * h];
        let mut dct = BlockDct2D::new(1);
        dct.forward(&x, w, h, &mut c);
        dct.inverse(&c, w, h, &mut y);
        for (a, z) in x.iter().zip(y.iter()) {
            assert!((a - z).abs() < 1e-6);
        }
    }

    #[test]
    fn dc_only_signal_concentrates_energy_in_dc_coefficient() {
        let w = 8;
        let h = 8;
        let x = vec![0.37f32; w * h];
        let mut c = vec![0.0f32; w * h];
        let mut dct = BlockDct2D::new(4);
        dct.forward(&x, w, h, &mut c);
        // Every 4x4 tile has one dominant DC coefficient.
        for by in 0..2 {
            for bx in 0..2 {
                let dc = c[by * 4 * w + bx * 4];
                assert!((dc - 16.0 * 0.37).abs() < 1e-3, "dc={dc}");
                for j in 0..4 {
                    for i in 0..4 {
                        let v = c[(by * 4 + j) * w + bx * 4 + i];
                        if j == 0 && i == 0 {
                            continue;
                        }
                        assert!(v.abs() < 1e-3, "ac={v}");
                    }
                }
            }
        }
    }
}
