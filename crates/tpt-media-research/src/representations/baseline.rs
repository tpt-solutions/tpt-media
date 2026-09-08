//! `baseline` — the raw / uncompressed identity representation.
//!
//! `z = x` with no transform at all: the coefficient stream *is* the signal
//! samples. This is the RD anchor (Phase 0 reference point, `todo.md` §56)`:
//! every other representation is measured *against* it, and the measurement
//! harness is only trusted once the raw-vs-DCT comparison behaves sanely.
//!
//! [`Representation`] contract: `T` is the identity, `T⁻¹` is the identity,
//! so `Coeffs` is just the copied sample stream.

use tpt_media_representation::{Representation, SignalView, SignalViewMut};

use crate::uniform::LinearCoeffs;

/// No parameters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BaselineConfig;

/// The raw sample stream (one entry per `V(x,y,t)` sample).
#[derive(Clone, Debug, PartialEq)]
pub struct BaselineCoeffs(pub Vec<f32>);

impl LinearCoeffs for BaselineCoeffs {
    fn stream(&self) -> &[f32] {
        &self.0
    }

    fn stream_mut(&mut self) -> &mut [f32] {
        &mut self.0
    }

    fn from_stream(stream: Vec<f32>) -> Self {
        BaselineCoeffs(stream)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Baseline;

impl Representation for Baseline {
    type Config = BaselineConfig;
    type Coeffs = BaselineCoeffs;

    fn analyze(&self, signal: SignalView<'_>, _config: &Self::Config) -> Self::Coeffs {
        BaselineCoeffs(signal.data().to_vec())
    }

    fn synthesize(
        &self,
        coeffs: &Self::Coeffs,
        _config: &Self::Config,
        mut out: SignalViewMut<'_>,
    ) {
        assert_eq!(
            coeffs.0.len(),
            out.dims().samples(),
            "baseline coefficient stream must be one sample per signal sample"
        );
        out.data_mut().copy_from_slice(&coeffs.0);
    }
}

impl tpt_media_representation::CoeffMagnitudes for BaselineCoeffs {
    fn coefficient_magnitudes(&self) -> Vec<f32> {
        self.0.iter().map(|s| s.abs()).collect()
    }

    fn retain_above_threshold(&mut self, threshold: f32) -> usize {
        let mut retained = 0;
        for v in self.0.iter_mut() {
            if v.abs() < threshold {
                *v = 0.0;
            } else {
                retained += 1;
            }
        }
        retained
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_media_representation::{CoeffMagnitudes, SignalBuffer, SignalDims};

    #[test]
    fn baseline_round_trips_exactly() {
        let dims = SignalDims {
            width: 4,
            height: 4,
            frames: 2,
        };
        let data: Vec<f32> = (0..dims.samples()).map(|i| (i as f32) * 0.25).collect();
        let sig = SignalBuffer::from_vec(dims, data.clone());
        let coeffs = Baseline.analyze(sig.view(), &BaselineConfig);
        assert_eq!(coeffs, BaselineCoeffs(data.clone()));

        let mut out = SignalBuffer::zeroed(dims);
        Baseline.synthesize(&coeffs, &BaselineConfig, out.view_mut());
        assert_eq!(out.data(), &data);
    }

    #[test]
    fn retain_above_threshold_zeroes_small_coefficients() {
        let mut c = BaselineCoeffs(vec![0.0, 0.5, -0.4, 0.0, 1.0]);
        let retained = c.retain_above_threshold(0.5);
        assert_eq!(retained, 2);
        assert_eq!(c.0, vec![0.0, 0.5, 0.0, 0.0, 1.0]);
    }
}
