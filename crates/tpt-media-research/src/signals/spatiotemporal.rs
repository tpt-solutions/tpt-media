//! Deterministic multi-frame synthetic spatiotemporal signals `V(x,y,t)` —
//! chosen to expose specific mathematical behaviour (motion, periodicity,
//! scene change), not visual demo quality. See
//! `docs/representation-research.md` and `todo.md` §56.
//!
//! `moving_edge`/`moving_texture`/`acceleration` return the exact ground
//! truth they were generated with alongside the signal, so callers (notably
//! the oracle experiment) never have to re-derive it.

use tpt_media_representation::{SignalBuffer, SignalDims};

use super::spatial;

/// Warp a `(x,y) -> f32` `pattern` by a per-frame `displacement`, sampling
/// toroidally (wrapping at the edges) so every generator here is a total,
/// well-defined function of `(x, y, t)` with no boundary special-casing.
fn translate(
    width: usize,
    height: usize,
    frames: usize,
    displacement: impl Fn(usize) -> (f32, f32),
    pattern: impl Fn(f32, f32) -> f32,
) -> SignalBuffer {
    let dims = SignalDims {
        width,
        height,
        frames,
    };
    let mut buf = SignalBuffer::zeroed(dims);
    for t in 0..frames {
        let (dx, dy) = displacement(t);
        for y in 0..height {
            for x in 0..width {
                let xf = (x as f32 - dx).rem_euclid(width as f32);
                let yf = (y as f32 - dy).rem_euclid(height as f32);
                buf.view_mut().set_sample(x, y, t, pattern(xf, yf));
            }
        }
    }
    buf
}

/// A triangle wave in `[-1, 1]` with period `1.0` (in units of `phase`).
fn triangle_wave(phase: f32) -> f32 {
    let p = phase - phase.floor();
    if p < 0.5 {
        4.0 * p - 1.0
    } else {
        3.0 - 4.0 * p
    }
}

/// A single-frame `pattern`, held unchanged across `frames` — the "nothing
/// moves" control case.
///
/// # Panics
///
/// Panics if `pattern.dims().frames != 1`.
pub fn stationary(pattern: &SignalBuffer, frames: usize) -> SignalBuffer {
    assert_eq!(
        pattern.dims().frames,
        1,
        "stationary() takes a single-frame spatial pattern"
    );
    let dims = SignalDims {
        width: pattern.dims().width,
        height: pattern.dims().height,
        frames,
    };
    let mut buf = SignalBuffer::zeroed(dims);
    for t in 0..frames {
        buf.view_mut().frame_mut(t).copy_from_slice(pattern.data());
    }
    buf
}

/// A vertical step edge translating at constant velocity `(vx, vy)`:
/// `I(x,y,t) = edge(x - vx·t, y - vy·t)`. Returns the signal alongside the
/// exact velocity it was generated with.
pub fn moving_edge(
    width: usize,
    height: usize,
    frames: usize,
    vx: f32,
    vy: f32,
) -> (SignalBuffer, [f32; 2]) {
    let half = width as f32 / 2.0;
    let buf = translate(
        width,
        height,
        frames,
        move |t| (vx * t as f32, vy * t as f32),
        move |x, _y| {
            if x >= half {
                1.0
            } else {
                0.0
            }
        },
    );
    (buf, [vx, vy])
}

/// A deterministic texture pattern translating at constant velocity
/// `(vx, vy)`. Returns the signal alongside the exact velocity.
pub fn moving_texture(
    width: usize,
    height: usize,
    frames: usize,
    vx: f32,
    vy: f32,
    seed: u64,
) -> (SignalBuffer, [f32; 2]) {
    let tex = spatial::texture(width, height, seed);
    let buf = translate(
        width,
        height,
        frames,
        move |t| (vx * t as f32, vy * t as f32),
        move |x, y| tex.sample_bilinear_toroidal(x, y, 0),
    );
    (buf, [vx, vy])
}

/// A texture pattern oscillating sinusoidally in `x`:
/// `displacement(t) = amplitude·sin(angular_freq·t)`.
pub fn sinusoidal_motion(
    width: usize,
    height: usize,
    frames: usize,
    amplitude: f32,
    angular_freq: f32,
    seed: u64,
) -> SignalBuffer {
    let tex = spatial::texture(width, height, seed);
    translate(
        width,
        height,
        frames,
        move |t| (amplitude * (angular_freq * t as f32).sin(), 0.0),
        move |x, y| tex.sample_bilinear_toroidal(x, y, 0),
    )
}

/// A texture pattern oscillating on a triangle wave (deliberately distinct
/// from [`sinusoidal_motion`]'s smooth motion): `displacement(t) =
/// amplitude·triangle_wave(t/period)`.
pub fn periodic_motion(
    width: usize,
    height: usize,
    frames: usize,
    amplitude: f32,
    period: f32,
    seed: u64,
) -> SignalBuffer {
    let tex = spatial::texture(width, height, seed);
    translate(
        width,
        height,
        frames,
        move |t| (amplitude * triangle_wave(t as f32 / period), 0.0),
        move |x, y| tex.sample_bilinear_toroidal(x, y, 0),
    )
}

/// A vertical step edge under constant acceleration `(ax, ay)`:
/// `displacement(t) = ½·a·t²`. Returns the signal alongside the exact
/// acceleration it was generated with.
pub fn acceleration(
    width: usize,
    height: usize,
    frames: usize,
    ax: f32,
    ay: f32,
) -> (SignalBuffer, [f32; 2]) {
    let half = width as f32 / 2.0;
    let buf = translate(
        width,
        height,
        frames,
        move |t| {
            let tf = t as f32;
            (0.5 * ax * tf * tf, 0.5 * ay * tf * tf)
        },
        move |x, _y| if x >= half { 1.0 } else { 0.0 },
    );
    (buf, [ax, ay])
}

/// `before` held for frames `0..change_at`, then `after` for
/// `change_at..frames` — an instantaneous scene cut, deliberately *not* a
/// smooth transition.
///
/// # Panics
///
/// Panics if `before`/`after` aren't single-frame or don't share dimensions,
/// or if `change_at > frames`.
pub fn abrupt_change(
    before: &SignalBuffer,
    after: &SignalBuffer,
    frames: usize,
    change_at: usize,
) -> SignalBuffer {
    assert_eq!(
        before.dims().frames,
        1,
        "abrupt_change() takes single-frame patterns"
    );
    assert_eq!(
        after.dims().frames,
        1,
        "abrupt_change() takes single-frame patterns"
    );
    assert_eq!(
        before.dims().width,
        after.dims().width,
        "before/after must share dimensions"
    );
    assert_eq!(
        before.dims().height,
        after.dims().height,
        "before/after must share dimensions"
    );
    assert!(change_at <= frames, "change_at must not exceed frame count");

    let dims = SignalDims {
        width: before.dims().width,
        height: before.dims().height,
        frames,
    };
    let mut buf = SignalBuffer::zeroed(dims);
    for t in 0..frames {
        let src = if t < change_at { before } else { after };
        buf.view_mut().frame_mut(t).copy_from_slice(src.data());
    }
    buf
}

/// A deterministic texture pattern rotating about the frame centre at
/// constant angular velocity `angular_velocity` (radians/frame).
pub fn rotating_pattern(
    width: usize,
    height: usize,
    frames: usize,
    angular_velocity: f32,
    seed: u64,
) -> SignalBuffer {
    let tex = spatial::texture(width, height, seed);
    let dims = SignalDims {
        width,
        height,
        frames,
    };
    let mut buf = SignalBuffer::zeroed(dims);
    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;
    for t in 0..frames {
        let theta = -angular_velocity * t as f32; // inverse rotation: sample where this pixel came from
        let (s, c) = theta.sin_cos();
        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let sx = c * dx - s * dy + cx;
                let sy = s * dx + c * dy + cy;
                let v = tex.sample_bilinear_toroidal(sx, sy, 0);
                buf.view_mut().set_sample(x, y, t, v);
            }
        }
    }
    buf
}

/// A filled disk centred in the frame, growing at constant rate:
/// `radius(t) = initial_radius + growth_rate·t`.
pub fn expanding_pattern(
    width: usize,
    height: usize,
    frames: usize,
    initial_radius: f32,
    growth_rate: f32,
) -> SignalBuffer {
    let dims = SignalDims {
        width,
        height,
        frames,
    };
    let mut buf = SignalBuffer::zeroed(dims);
    let cx = width as f32 / 2.0;
    let cy = height as f32 / 2.0;
    for t in 0..frames {
        let r = initial_radius + growth_rate * t as f32;
        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let d = (dx * dx + dy * dy).sqrt();
                buf.view_mut()
                    .set_sample(x, y, t, if d <= r { 1.0 } else { 0.0 });
            }
        }
    }
    buf
}

/// Two independently translating step edges (velocities `v1`, `v2`),
/// combined by taking the brighter of the two at each sample — "multiple
/// independently moving structures" from `todo.md` §56/§9.
pub fn multiple_moving_edges(
    width: usize,
    height: usize,
    frames: usize,
    v1: [f32; 2],
    v2: [f32; 2],
) -> (SignalBuffer, [[f32; 2]; 2]) {
    let (a, _) = moving_edge(width, height, frames, v1[0], v1[1]);
    let (b, _) = moving_edge(width, height, frames, v2[0], v2[1]);
    let dims = a.dims();
    let combined: Vec<f32> = a
        .data()
        .iter()
        .zip(b.data().iter())
        .map(|(&x, &y)| x.max(y))
        .collect();
    (SignalBuffer::from_vec(dims, combined), [v1, v2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stationary_replicates_pattern_across_frames() {
        let pattern = spatial::checkerboard(4, 4, 2);
        let vol = stationary(&pattern, 3);
        assert_eq!(vol.view().frame(0), pattern.data());
        assert_eq!(vol.view().frame(2), pattern.data());
    }

    #[test]
    fn moving_edge_shifts_by_exactly_v_pixels_per_frame() {
        let (vol, v) = moving_edge(8, 2, 3, 1.0, 0.0);
        assert_eq!(v, [1.0, 0.0]);
        // The edge sits at half = 4.0 in frame 0; frame t's edge is shifted
        // right by t (toroidally), so summing "on" samples per frame is
        // conserved (rotation of the same step pattern).
        for t in 0..3 {
            let ones = vol.view().frame(t).iter().filter(|&&s| s == 1.0).count();
            assert_eq!(ones, 8, "frame {t}: half of 2 rows * 8 cols = 8 samples on");
        }
    }

    #[test]
    fn moving_edge_at_zero_velocity_is_stationary() {
        let (vol, _) = moving_edge(6, 2, 4, 0.0, 0.0);
        for t in 1..4 {
            assert_eq!(vol.view().frame(t), vol.view().frame(0));
        }
    }

    #[test]
    fn abrupt_change_switches_exactly_at_change_at() {
        let before = spatial::constant(4, 4, 0.0);
        let after = spatial::constant(4, 4, 1.0);
        let vol = abrupt_change(&before, &after, 5, 2);
        assert!(vol.view().frame(0).iter().all(|&v| v == 0.0));
        assert!(vol.view().frame(1).iter().all(|&v| v == 0.0));
        assert!(vol.view().frame(2).iter().all(|&v| v == 1.0));
        assert!(vol.view().frame(4).iter().all(|&v| v == 1.0));
    }

    #[test]
    fn multiple_moving_edges_returns_both_velocities() {
        let (_, velocities) = multiple_moving_edges(8, 8, 4, [1.0, 0.0], [0.0, 1.0]);
        assert_eq!(velocities, [[1.0, 0.0], [0.0, 1.0]]);
    }
}
