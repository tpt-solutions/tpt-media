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
    pub fn frame_len(&self) -> usize {
        self.width * self.height
    }

    /// Total samples in the volume (`width * height * frames`).
    pub fn samples(&self) -> usize {
        self.frame_len() * self.frames
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
        assert_eq!(data.len(), dims.samples(), "signal data length must equal width*height*frames");
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
        assert_eq!(data.len(), dims.samples(), "signal data length must equal width*height*frames");
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
        SignalView { dims: self.dims, data: self.data }
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
        SignalBuffer { dims, data: vec![0.0; dims.samples()] }
    }

    /// A buffer filled with `value`.
    pub fn filled(dims: SignalDims, value: f32) -> Self {
        SignalBuffer { dims, data: vec![value; dims.samples()] }
    }

    /// Wrap caller-owned data directly.
    ///
    /// # Panics
    ///
    /// Panics if `data.len() != dims.samples()`.
    pub fn from_vec(dims: SignalDims, data: Vec<f32>) -> Self {
        assert_eq!(data.len(), dims.samples(), "signal data length must equal width*height*frames");
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
    pub fn sample_bilinear_toroidal(&self, x: f32, y: f32, t: usize) -> f32 {
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
        let dims = SignalDims { width: 4, height: 3, frames: 2 };
        let mut buf = SignalBuffer::zeroed(dims);
        buf.view_mut().set_sample(2, 1, 1, 7.5);
        assert_eq!(buf.view().sample(2, 1, 1), 7.5);
        assert_eq!(buf.view().sample(0, 0, 0), 0.0);
    }

    #[test]
    fn frame_slices_are_contiguous_and_ordered() {
        let dims = SignalDims { width: 2, height: 2, frames: 2 };
        let data: Vec<f32> = (0..8).map(|i| i as f32).collect();
        let buf = SignalBuffer::from_vec(dims, data);
        assert_eq!(buf.view().frame(0), &[0.0, 1.0, 2.0, 3.0]);
        assert_eq!(buf.view().frame(1), &[4.0, 5.0, 6.0, 7.0]);
    }

    #[test]
    fn bilinear_sample_at_integer_coords_matches_exact_sample() {
        let dims = SignalDims { width: 4, height: 4, frames: 1 };
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
        let dims = SignalDims { width: 4, height: 1, frames: 1 };
        let buf = SignalBuffer::from_vec(dims, vec![10.0, 20.0, 30.0, 40.0]);
        // x = -0.5 wraps to halfway between sample 3 (40.0) and sample 0 (10.0).
        let v = buf.sample_bilinear_toroidal(-0.5, 0.0, 0);
        assert!((v - 25.0).abs() < 1e-4, "v={v}");
    }
}
