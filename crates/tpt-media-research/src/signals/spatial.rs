//! Deterministic single-frame (`frames == 1`) synthetic spatial signals —
//! chosen to expose specific mathematical behaviour, not visual demo
//! quality. See `docs/representation-research.md` and `todo.md` §56.

use tpt_media_representation::{SignalBuffer, SignalDims};

use crate::det_rng::SplitMix64;

fn dims1(width: usize, height: usize) -> SignalDims {
    SignalDims { width, height, frames: 1 }
}

/// A uniform field.
pub fn constant(width: usize, height: usize, value: f32) -> SignalBuffer {
    SignalBuffer::filled(dims1(width, height), value)
}

/// A linear ramp from `0.0` at `x=0` to `1.0` at `x=width-1`.
pub fn gradient(width: usize, height: usize) -> SignalBuffer {
    let dims = dims1(width, height);
    let mut buf = SignalBuffer::zeroed(dims);
    let denom = (width.max(2) - 1) as f32;
    for y in 0..height {
        for x in 0..width {
            buf.view_mut().set_sample(x, y, 0, x as f32 / denom);
        }
    }
    buf
}

/// A single vertical step edge at `x = width/2`.
pub fn edge(width: usize, height: usize) -> SignalBuffer {
    let dims = dims1(width, height);
    let mut buf = SignalBuffer::zeroed(dims);
    let half = width / 2;
    for y in 0..height {
        for x in 0..width {
            let v = if x >= half { 1.0 } else { 0.0 };
            buf.view_mut().set_sample(x, y, 0, v);
        }
    }
    buf
}

/// One quadrant (`x >= width/2 && y >= height/2`) set to `1.0`, else `0.0`.
pub fn corner(width: usize, height: usize) -> SignalBuffer {
    let dims = dims1(width, height);
    let mut buf = SignalBuffer::zeroed(dims);
    let hx = width / 2;
    let hy = height / 2;
    for y in 0..height {
        for x in 0..width {
            let v = if x >= hx && y >= hy { 1.0 } else { 0.0 };
            buf.view_mut().set_sample(x, y, 0, v);
        }
    }
    buf
}

/// `sin(2π·(freq_x·x/width + freq_y·y/height))`, rescaled to `[0,1]`.
pub fn sinusoid(width: usize, height: usize, freq_x: f32, freq_y: f32) -> SignalBuffer {
    let dims = dims1(width, height);
    let mut buf = SignalBuffer::zeroed(dims);
    for y in 0..height {
        for x in 0..width {
            let phase = std::f32::consts::TAU
                * (freq_x * x as f32 / width as f32 + freq_y * y as f32 / height as f32);
            let v = 0.5 * (phase.sin() + 1.0);
            buf.view_mut().set_sample(x, y, 0, v);
        }
    }
    buf
}

/// Alternating `cell`-sized squares of `0.0`/`1.0`.
pub fn checkerboard(width: usize, height: usize, cell: usize) -> SignalBuffer {
    assert!(cell > 0, "cell size must be positive");
    let dims = dims1(width, height);
    let mut buf = SignalBuffer::zeroed(dims);
    for y in 0..height {
        for x in 0..width {
            let v = if ((x / cell) + (y / cell)) % 2 == 0 { 0.0 } else { 1.0 };
            buf.view_mut().set_sample(x, y, 0, v);
        }
    }
    buf
}

/// A single `1.0` sample at the centre, `0.0` elsewhere.
pub fn impulse(width: usize, height: usize) -> SignalBuffer {
    let dims = dims1(width, height);
    let mut buf = SignalBuffer::zeroed(dims);
    buf.view_mut().set_sample(width / 2, height / 2, 0, 1.0);
    buf
}

/// Deterministic pseudo-random noise in `[0,1)`, seeded so the same `seed`
/// always reproduces the same texture.
pub fn texture(width: usize, height: usize, seed: u64) -> SignalBuffer {
    let dims = dims1(width, height);
    let mut rng = SplitMix64::new(seed);
    let data: Vec<f32> = (0..dims.samples()).map(|_| rng.next_unit_f32()).collect();
    SignalBuffer::from_vec(dims, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_has_no_variation() {
        let buf = constant(8, 8, 0.3);
        assert!(buf.data().iter().all(|&v| (v - 0.3).abs() < 1e-9));
    }

    #[test]
    fn edge_splits_field_in_half() {
        let buf = edge(8, 4);
        let ones = buf.data().iter().filter(|&&v| v == 1.0).count();
        assert_eq!(ones, 4 * 4);
    }

    #[test]
    fn texture_is_deterministic_for_a_given_seed() {
        let a = texture(4, 4, 123);
        let b = texture(4, 4, 123);
        assert_eq!(a.data(), b.data());
        let c = texture(4, 4, 124);
        assert_ne!(a.data(), c.data());
    }

    #[test]
    fn impulse_has_exactly_one_nonzero_sample() {
        let buf = impulse(5, 5);
        assert_eq!(buf.data().iter().filter(|&&v| v != 0.0).count(), 1);
    }
}
