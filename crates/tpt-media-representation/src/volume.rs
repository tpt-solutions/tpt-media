//! The spatiotemporal signal type `V(x, y, t)`.
//!
//! A dense `f32` volume, stored frame-major (all of frame 0, then frame 1,
//! ...), each frame row-major. Distinct from `tpt-media-core::VideoFrame`
//! (a packed-pixel production type) on purpose: the research question is
//! mathematical (real-valued signals, arbitrary transforms), not about
//! production pixel formats.

/// Dimensions of a `V(x, y, t)` volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignalDims {
    pub width: usize,
    pub height: usize,
    pub frames: usize,
}

impl SignalDims {
    /// Samples in one frame (`width * height`).
    ///
    /// # Panics
    ///
    /// Panics if `width * height` overflows `usize`. The product is *checked*
    /// rather than left to a plain `*`, because that is a debug-build panic
    /// and a silent wrap in release: the two profiles would then disagree
    /// about a volume's size, and the wrapped value is what every caller
    /// (the `Vec` allocations, the `frame()` slices, the sample indices) is
    /// derived from.
    pub fn frame_len(&self) -> usize {
        self.width
            .checked_mul(self.height)
            .expect("width * height must not overflow usize")
    }

    /// Total samples in the volume (`width * height * frames`).
    ///
    /// # Panics
    ///
    /// Panics if the product overflows `usize` (see [`SignalDims::frame_len`]
    /// for why this is checked rather than left to a plain `*`).
    pub fn samples(&self) -> usize {
        self.frame_len()
            .checked_mul(self.frames)
            .expect("width * height * frames must not overflow usize")
    }
}

/// Borrowed read-only view over a caller-owned `V(x, y, t)` buffer.
#[derive(Clone, Copy)]
pub struct SignalView<'a> {
    dims: SignalDims,
    data: &'a [f32],
}

impl<'a> SignalView<'a> {
    /// # Panics
    ///
    /// Panics if `data.len() != dims.samples()`.
    pub fn new(dims: SignalDims, data: &'a [f32]) -> Self {
        assert_eq!(
            data.len(),
            dims.samples(),
            "signal data length must equal width*height*frames"
        );
        SignalView { dims, data }
    }

    pub fn dims(&self) -> SignalDims {
        self.dims
    }

    pub fn data(&self) -> &'a [f32] {
        self.data
    }

    /// The samples of frame `t`, row-major.
    pub fn frame(&self, t: usize) -> &'a [f32] {
        let len = self.dims.frame_len();
        &self.data[t * len..(t + 1) * len]
    }

    pub fn sample(&self, x: usize, y: usize, t: usize) -> f32 {
        self.frame(t)[y * self.dims.width + x]
    }
}

/// Borrowed mutable view over a caller-owned `V(x, y, t)` buffer.
pub struct SignalViewMut<'a> {
    dims: SignalDims,
    data: &'a mut [f32],
}

impl<'a> SignalViewMut<'a> {
    /// # Panics
    ///
    /// Panics if `data.len() != dims.samples()`.
    pub fn new(dims: SignalDims, data: &'a mut [f32]) -> Self {
        assert_eq!(
            data.len(),
            dims.samples(),
            "signal data length must equal width*height*frames"
        );
        SignalViewMut { dims, data }
    }

    pub fn dims(&self) -> SignalDims {
        self.dims
    }

    pub fn data_mut(&mut self) -> &mut [f32] {
        self.data
    }

    /// The samples of frame `t`, row-major, mutable.
    pub fn frame_mut(&mut self, t: usize) -> &mut [f32] {
        let len = self.dims.frame_len();
        &mut self.data[t * len..(t + 1) * len]
    }

    pub fn set_sample(&mut self, x: usize, y: usize, t: usize, value: f32) {
        let width = self.dims.width;
        self.frame_mut(t)[y * width + x] = value;
    }

    /// Borrow this mutable view as a read-only [`SignalView`].
    pub fn as_view(&self) -> SignalView<'_> {
        SignalView {
            dims: self.dims,
            data: self.data,
        }
    }
}

/// Owning `V(x, y, t)` buffer — a convenience wrapper that allocates its own
/// storage rather than requiring the caller to pass one in. Offline-profile
/// only (this whole crate is; see the crate-level docs), so unconditional
/// allocation here is fine.
#[derive(Clone, Debug)]
pub struct SignalBuffer {
    dims: SignalDims,
    data: Vec<f32>,
}

impl SignalBuffer {
    /// A buffer of `dims.samples()` zeros.
    pub fn zeroed(dims: SignalDims) -> Self {
        SignalBuffer {
            dims,
            data: vec![0.0; dims.samples()],
        }
    }

    /// A buffer filled with `value`.
    pub fn filled(dims: SignalDims, value: f32) -> Self {
        SignalBuffer {
            dims,
            data: vec![value; dims.samples()],
        }
    }

    /// Wrap caller-owned data directly.
    ///
    /// # Panics
    ///
    /// Panics if `data.len() != dims.samples()`.
    pub fn from_vec(dims: SignalDims, data: Vec<f32>) -> Self {
        assert_eq!(
            data.len(),
            dims.samples(),
            "signal data length must equal width*height*frames"
        );
        SignalBuffer { dims, data }
    }

    pub fn dims(&self) -> SignalDims {
        self.dims
    }

    pub fn data(&self) -> &[f32] {
        &self.data
    }

    pub fn into_vec(self) -> Vec<f32> {
        self.data
    }

    pub fn view(&self) -> SignalView<'_> {
        SignalView::new(self.dims, &self.data)
    }

    pub fn view_mut(&mut self) -> SignalViewMut<'_> {
        SignalViewMut::new(self.dims, &mut self.data)
    }

    /// Bilinear sample at fractional coordinates, wrapping toroidally in `x`
    /// and `y` (used by `motion_aligned`'s coordinate warp and by synthetic
    /// spatiotemporal signal generators, both of which need a value at an
    /// arbitrary real-valued position on a periodic lattice).
    ///
    /// # Panics
    ///
    /// Panics if `width` or `height` is zero. There is no lattice to sample
    /// on — every position wraps onto a sample that does not exist — so
    /// there is no value this could return. The failure is stated rather than
    /// left to surface as an arithmetic one: with a zero extent,
    /// `x.rem_euclid(0.0)` is `NaN`, `NaN as usize` is `0`, and the
    /// following `% width` then divides by zero and panics with
    /// "attempt to calculate the remainder with a divisor of zero" — true,
    /// but it names the arithmetic rather than the cause.
    pub fn sample_bilinear_toroidal(&self, x: f32, y: f32, t: usize) -> f32 {
        assert!(
            self.dims.width > 0 && self.dims.height > 0,
            "sample_bilinear_toroidal needs a non-degenerate frame: \
             width and height must both be non-zero (got {}x{})",
            self.dims.width,
            self.dims.height
        );
        let w = self.dims.width as f32;
        let h = self.dims.height as f32;
        let xw = x.rem_euclid(w);
        let yw = y.rem_euclid(h);
        let x0 = xw.floor() as usize % self.dims.width;
        let y0 = yw.floor() as usize % self.dims.height;
        let x1 = (x0 + 1) % self.dims.width;
        let y1 = (y0 + 1) % self.dims.height;
        let fx = xw - xw.floor();
        let fy = yw - yw.floor();

        let v = self.view();
        let s00 = v.sample(x0, y0, t);
        let s10 = v.sample(x1, y0, t);
        let s01 = v.sample(x0, y1, t);
        let s11 = v.sample(x1, y1, t);
        let top = s00 * (1.0 - fx) + s10 * fx;
        let bottom = s01 * (1.0 - fx) + s11 * fx;
        top * (1.0 - fy) + bottom * fy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_get_sample_round_trips() {
        let dims = SignalDims {
            width: 4,
            height: 3,
            frames: 2,
        };
        let mut buf = SignalBuffer::zeroed(dims);
        buf.view_mut().set_sample(2, 1, 1, 7.5);
        assert_eq!(buf.view().sample(2, 1, 1), 7.5);
        assert_eq!(buf.view().sample(0, 0, 0), 0.0);
    }

    #[test]
    fn frame_slices_are_contiguous_and_ordered() {
        let dims = SignalDims {
            width: 2,
            height: 2,
            frames: 2,
        };
        let data: Vec<f32> = (0..8).map(|i| i as f32).collect();
        let buf = SignalBuffer::from_vec(dims, data);
        assert_eq!(buf.view().frame(0), &[0.0, 1.0, 2.0, 3.0]);
        assert_eq!(buf.view().frame(1), &[4.0, 5.0, 6.0, 7.0]);
    }

    #[test]
    fn bilinear_sample_at_integer_coords_matches_exact_sample() {
        let dims = SignalDims {
            width: 4,
            height: 4,
            frames: 1,
        };
        let data: Vec<f32> = (0..16).map(|i| i as f32).collect();
        let buf = SignalBuffer::from_vec(dims, data);
        for y in 0..4 {
            for x in 0..4 {
                let exact = buf.view().sample(x, y, 0);
                let bilinear = buf.sample_bilinear_toroidal(x as f32, y as f32, 0);
                assert!((exact - bilinear).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn bilinear_sample_wraps_toroidally() {
        let dims = SignalDims {
            width: 4,
            height: 1,
            frames: 1,
        };
        let buf = SignalBuffer::from_vec(dims, vec![10.0, 20.0, 30.0, 40.0]);
        // x = -0.5 wraps to halfway between sample 3 (40.0) and sample 0 (10.0).
        let v = buf.sample_bilinear_toroidal(-0.5, 0.0, 0);
        assert!((v - 25.0).abs() < 1e-4, "v={v}");
    }

    // --- SignalDims arithmetic (§38: empty input, overflow, maximum values) ---

    #[test]
    fn frame_len_and_samples_multiply_out_as_documented() {
        let d = SignalDims {
            width: 3,
            height: 4,
            frames: 5,
        };
        assert_eq!(d.frame_len(), 12);
        assert_eq!(d.samples(), 60);
    }

    #[test]
    fn a_zero_extent_volume_has_no_samples_at_all() {
        for d in [
            SignalDims {
                width: 0,
                height: 0,
                frames: 0,
            },
            SignalDims {
                width: 4,
                height: 0,
                frames: 2,
            },
            SignalDims {
                width: 0,
                height: 4,
                frames: 2,
            },
        ] {
            assert_eq!(d.samples(), 0);
            let buf = SignalBuffer::zeroed(d);
            assert!(buf.data().is_empty());
            assert_eq!(buf.dims(), d);
        }
    }

    #[test]
    #[should_panic(expected = "width * height must not overflow usize")]
    fn frame_len_rejects_a_frame_that_overflows() {
        SignalDims {
            width: usize::MAX,
            height: 2,
            frames: 1,
        }
        .frame_len();
    }

    #[test]
    #[should_panic(expected = "width * height * frames must not overflow usize")]
    fn samples_rejects_a_volume_that_overflows_even_when_a_frame_does_not() {
        // width * height fits (usize::MAX * 1), so only the frame-count
        // multiplication can catch this one.
        SignalDims {
            width: usize::MAX,
            height: 1,
            frames: 2,
        }
        .samples();
    }

    #[test]
    fn the_largest_volume_that_does_not_overflow_is_still_measurable() {
        // A single sample per frame is the only geometry a usize holds at
        // full width; the point is that the bound is now reached by a stated
        // limit rather than by wrapping into a small, plausible-looking number.
        let d = SignalDims {
            width: usize::MAX,
            height: 1,
            frames: 1,
        };
        assert_eq!(d.frame_len(), usize::MAX);
        assert_eq!(d.samples(), usize::MAX);
    }

    // --- The documented length precondition (§38: malformed input) ---

    #[test]
    #[should_panic(expected = "signal data length must equal width*height*frames")]
    fn a_view_over_mismatched_data_is_rejected_rather_than_read_out_of_bounds() {
        SignalView::new(
            SignalDims {
                width: 2,
                height: 2,
                frames: 1,
            },
            &[0.0f32],
        );
    }

    #[test]
    #[should_panic(expected = "signal data length must equal width*height*frames")]
    fn a_mutable_view_over_mismatched_data_is_rejected() {
        let mut data = vec![0.0f32; 5];
        SignalViewMut::new(
            SignalDims {
                width: 2,
                height: 2,
                frames: 1,
            },
            &mut data,
        );
    }

    #[test]
    #[should_panic(expected = "signal data length must equal width*height*frames")]
    fn from_vec_rejects_data_that_does_not_match_the_dims() {
        SignalBuffer::from_vec(
            SignalDims {
                width: 2,
                height: 2,
                frames: 1,
            },
            vec![0.0f32; 3],
        );
    }

    #[test]
    fn a_view_and_a_buffer_agree_about_the_same_data() {
        let dims = SignalDims {
            width: 2,
            height: 3,
            frames: 2,
        };
        let data: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let mut buf = SignalBuffer::from_vec(dims, data);
        assert_eq!(buf.dims(), dims);
        assert_eq!(buf.view().data(), buf.data());
        assert_eq!(buf.view().dims(), dims);

        // The view types exist to alias the caller's storage, so every write
        // has to land in the owned buffer rather than in a copy.
        buf.view_mut().set_sample(0, 0, 1, 99.0);
        assert_eq!(buf.data()[6], 99.0);
        buf.view_mut().data_mut()[0] = 42.0;
        assert_eq!(buf.data()[0], 42.0);
        buf.view_mut().frame_mut(1)[1] = 7.0;
        assert_eq!(buf.view().sample(1, 0, 1), 7.0);
    }

    #[test]
    fn into_vec_hands_the_underlying_storage_over() {
        let buf = SignalBuffer::from_vec(
            SignalDims {
                width: 1,
                height: 1,
                frames: 1,
            },
            vec![5.0],
        );
        assert_eq!(buf.into_vec(), vec![5.0]);
    }

    // --- sample_bilinear_toroidal's own precondition (the fixed defect) ---

    #[test]
    #[should_panic(expected = "sample_bilinear_toroidal needs a non-degenerate frame")]
    fn the_toroidal_sampler_states_that_a_zero_width_frame_has_no_lattice() {
        SignalBuffer::zeroed(SignalDims {
            width: 0,
            height: 4,
            frames: 1,
        })
        .sample_bilinear_toroidal(0.5, 0.0, 0);
    }

    #[test]
    #[should_panic(expected = "sample_bilinear_toroidal needs a non-degenerate frame")]
    fn the_toroidal_sampler_states_that_a_zero_height_frame_has_no_lattice() {
        SignalBuffer::zeroed(SignalDims {
            width: 4,
            height: 0,
            frames: 1,
        })
        .sample_bilinear_toroidal(0.0, 0.5, 0);
    }

    #[test]
    fn the_toroidal_sampler_addresses_each_frame_separately() {
        // Frame 1 must be read from frame 1's samples, not frame 0's: the
        // spatial and temporal axes are independent.
        let dims = SignalDims {
            width: 2,
            height: 1,
            frames: 2,
        };
        let buf = SignalBuffer::from_vec(dims, vec![0.0, 0.0, 10.0, 20.0]);
        assert!((buf.sample_bilinear_toroidal(0.5, 0.0, 0) - 0.0).abs() < 1e-6);
        assert!((buf.sample_bilinear_toroidal(0.5, 0.0, 1) - 15.0).abs() < 1e-4);
    }

    #[test]
    fn a_single_sample_frame_is_a_valid_lattice_that_always_returns_it() {
        let dims = SignalDims {
            width: 1,
            height: 1,
            frames: 1,
        };
        let buf = SignalBuffer::from_vec(dims, vec![3.5]);
        // Every position wraps onto the one and only sample.
        for (x, y) in [(0.0, 0.0), (0.5, 0.5), (-3.25, 7.75), (12.0, -0.5)] {
            assert!((buf.sample_bilinear_toroidal(x, y, 0) - 3.5).abs() < 1e-6);
        }
    }
}
