//! The oracle experiment and methodology phases (`todo.md` §56).
//!
//! Every experiment in this module is a *measurement*, not an assertion:
//!
//! 1. It runs the named representation(s) over a deterministic synthetic
//!    signal at a matched set of rate points (`bits/sample` vs `MSE`, measured
//!    under the harness's empirical-entropy accounting).
//! 2. Its test asserts only **measurement validity** — finite values, a duly
//!    sorted RD curve, every target rate actually reached (never
//!    extrapolated), identical inputs reproduce identical output — *not* the
//!    hypothesized direction of the result.
//! 3. The numeric results are printed (via `cargo test -- --nocapture`, or
//!    by calling the functions directly) and recorded verbatim in
//!    `docs/representation-research.md`. A run that contradicts a hypothesis
//!    is a *finding*, not a bug.
//!
//! The three "core" representations are `baseline` (raw), `block_dct`
//! (spatial), and `patch_vq` (dictionary); `temporal_dct` and
//! `motion_aligned` are the extra oracle-experiment conditions that exploit
//! *known* temporal structure.

use tpt_media_representation::{Quantizer, Representation, SignalBuffer, SignalView};

use crate::diagnostics::SecondaryDiagnostics;
use crate::harness::{
    interpolated_distortion, validate_curve, LabeledCurve, MatchedRateRow, RdPoint,
};
use crate::representations::baseline::BaselineCoeffs;
use crate::representations::block_dct::BlockDctCoeffs;
use crate::representations::motion_aligned::{
    MotionAligned, MotionAlignedConfig, MotionAlignedQuantizer, MotionAlignedRate,
};
use crate::representations::patch_vq::{
    extract_patches, patch_dim, PatchVq, PatchVqConfig, PatchVqQuantizer, PatchVqRate,
};
use crate::representations::temporal_dct::{
    TemporalDct, TemporalDctConfig, TemporalDctQuantizer, TemporalDctRate,
};
use crate::representations::{Baseline, BaselineConfig, BlockDct, BlockDctConfig};
use crate::signals::spatiotemporal::moving_edge;
use crate::uniform::{EntropyRate, LinearQuantizer};

/// Result of one matched-rate experiment.
#[derive(Clone, Debug)]
pub struct ExperimentReport {
    pub label: &'static str,
    pub curves: Vec<LabeledCurve>,
    /// Target rates (bits/sample) chosen **inside** every curve's sampled
    /// range, so every cell is reached by interpolation, never extrapolation.
    pub targets: Vec<f64>,
    /// Matched-rate distortion (MSE) per target, per representation. A valid
    /// experiment has no `None` cells.
    pub table: Vec<(f64, MatchedRateRow)>,
    /// Secondary diagnostics at one mid-sweep operating point (never primary;
    /// present only for linear-coefficient curves — `patch_vq` has no linear
    /// coefficient stream and is omitted).
    pub secondary: Vec<(String, SecondaryDiagnostics)>,
}

/// The moving-edge oracle experiment: raw pixels vs spatial DCT vs
/// DCT+previous-frame-prediction vs motion-aligned-under-true-velocity, on
/// `I(x,y,t) = edge(x - v·t)` with known `v`, at matched rate points.
pub fn oracle_moving_edge(
    width: usize,
    height: usize,
    frames: usize,
    steps: &[f32],
    out: &mut SignalBuffer,
) -> ExperimentReport {
    let (signal, v) = moving_edge(width, height, frames, 1.0, 0.0);
    let label = "oracle: moving edge (vx=1)";

    let curves: Vec<LabeledCurve> = vec![
        ("raw pixels", baseline_curve(signal.view(), steps, out)),
        ("spatial DCT", block_dct_curve(signal.view(), 8, steps, out)),
        (
            "DCT + previous-frame pred",
            temporal_dct_curve(signal.view(), 8, steps, out),
        ),
        (
            "motion-aligned (true v)",
            motion_aligned_curve(signal.view(), v, 8, steps, out),
        ),
    ];
    let secondary = vec![
        (
            "raw pixels",
            secondary_linear(signal.view(), &Baseline, &BaselineConfig, 0.125),
        ),
        (
            "spatial DCT",
            secondary_linear(
                signal.view(),
                &BlockDct,
                &BlockDctConfig { block_size: 8 },
                0.125,
            ),
        ),
        (
            "DCT + previous-frame pred",
            secondary_temporal(signal.view(), 0.125),
        ),
        (
            "motion-aligned (true v)",
            secondary_motion(signal.view(), v, 0.125),
        ),
    ];
    assemble_report(label, curves, secondary)
}

/// Phase 0 — establish the truth: raw vs spatial DCT on a *stationary*
/// synthetic image. Proves the harness is honest (measurement validity), and
/// records whether the naive spatial DCT wins here.
pub fn phase0_baseline_vs_dct(
    width: usize,
    height: usize,
    steps: &[f32],
    out: &mut SignalBuffer,
) -> ExperimentReport {
    let signal = crate::signals::spatial::gradient(width, height);
    let label = "phase 0: raw vs spatial DCT (static gradient)";

    let curves: Vec<LabeledCurve> = vec![
        ("raw pixels", baseline_curve(signal.view(), steps, out)),
        ("spatial DCT", block_dct_curve(signal.view(), 8, steps, out)),
    ];
    let secondary = vec![
        (
            "raw pixels",
            secondary_linear(signal.view(), &Baseline, &BaselineConfig, 0.125),
        ),
        (
            "spatial DCT",
            secondary_linear(
                signal.view(),
                &BlockDct,
                &BlockDctConfig { block_size: 8 },
                0.125,
            ),
        ),
    ];
    assemble_report(label, curves, secondary)
}

/// Phase 2 — challenge locality: deterministic noise texture, blocks vs
/// patches at the same partition size (8×8 tiles; `patch_vq` at patch 8).
pub fn phase2_blocks_vs_patches(
    width: usize,
    height: usize,
    steps: &[f32],
    out: &mut SignalBuffer,
) -> ExperimentReport {
    let signal = crate::signals::spatial::texture(width, height, 0xBEEF);
    let label = "phase 2: blocks (DCT) vs patches (VQ) on noise texture";

    let curves: Vec<LabeledCurve> = vec![
        (
            "spatial DCT, 8x8",
            block_dct_curve(signal.view(), 8, steps, out),
        ),
        (
            "patch VQ, 8x8, K=16",
            patch_vq_curve(signal.view(), vq_config(16), steps, out),
        ),
    ];
    let secondary = vec![(
        "spatial DCT, 8x8",
        secondary_linear(
            signal.view(),
            &BlockDct,
            &BlockDctConfig { block_size: 8 },
            0.125,
        ),
    )];
    assemble_report(label, curves, secondary)
}

/// Phase 4 — challenge coordinates: stationary vs motion-aligned on a moving
/// edge (the oracle experiment is the 4-condition version; this phase is the
/// coordinates-only 2-condition version).
pub fn phase4_stationary_vs_motion_aligned(
    width: usize,
    height: usize,
    frames: usize,
    steps: &[f32],
    out: &mut SignalBuffer,
) -> ExperimentReport {
    let (signal, v) = moving_edge(width, height, frames, 1.0, 0.0);
    let label = "phase 4: stationary vs motion-aligned (moving edge)";

    let curves: Vec<LabeledCurve> = vec![
        (
            "stationary (spatial DCT)",
            block_dct_curve(signal.view(), 8, steps, out),
        ),
        (
            "motion-aligned (true v)",
            motion_aligned_curve(signal.view(), v, 8, steps, out),
        ),
    ];
    let secondary = vec![
        (
            "stationary (spatial DCT)",
            secondary_linear(
                signal.view(),
                &BlockDct,
                &BlockDctConfig { block_size: 8 },
                0.125,
            ),
        ),
        (
            "motion-aligned (true v)",
            secondary_motion(signal.view(), v, 0.125),
        ),
    ];
    assemble_report(label, curves, secondary)
}

/// Phase 5 — challenge representation: linear coefficients vs dictionary/VQ
/// on a multi-frame signal that isn't a pure translation (so neither temporal
/// trick trivially wins).
pub fn phase5_coefficients_vs_dictionary(
    width: usize,
    height: usize,
    frames: usize,
    steps: &[f32],
    out: &mut SignalBuffer,
) -> ExperimentReport {
    let signal =
        crate::signals::spatiotemporal::sinusoidal_motion(width, height, frames, 2.0, 0.8, 0xCAFE);
    let label = "phase 5: coefficients (DCT) vs dictionary (VQ), oscillating texture";

    let curves: Vec<LabeledCurve> = vec![
        (
            "spatial DCT, 8x8",
            block_dct_curve(signal.view(), 8, steps, out),
        ),
        (
            "patch VQ, 8x8, K=32",
            patch_vq_curve(signal.view(), vq_config(32), steps, out),
        ),
    ];
    let secondary = vec![(
        "spatial DCT, 8x8",
        secondary_linear(
            signal.view(),
            &BlockDct,
            &BlockDctConfig { block_size: 8 },
            0.125,
        ),
    )];
    assemble_report(label, curves, secondary)
}

fn vq_config(entries: usize) -> PatchVqConfig {
    PatchVqConfig {
        patch_size: 8,
        codebook_entries: entries,
        train_iterations: 30,
        train_seed: 42,
    }
}

// ---------------------------------------------------------------------------
// Per-representation curve drivers (quantizer/rate wiring lives here, once).
// ---------------------------------------------------------------------------

fn baseline_curve(signal: SignalView<'_>, steps: &[f32], out: &mut SignalBuffer) -> Vec<RdPoint> {
    crate::harness::rd_curve(
        signal,
        &Baseline,
        &BaselineConfig,
        steps,
        LinearQuantizer::<BaselineCoeffs>::new,
        &EntropyRate,
        out,
    )
}

fn block_dct_curve(
    signal: SignalView<'_>,
    block: usize,
    steps: &[f32],
    out: &mut SignalBuffer,
) -> Vec<RdPoint> {
    crate::harness::rd_curve(
        signal,
        &BlockDct,
        &BlockDctConfig { block_size: block },
        steps,
        LinearQuantizer::<BlockDctCoeffs>::new,
        &EntropyRate,
        out,
    )
}

fn temporal_dct_curve(
    signal: SignalView<'_>,
    block: usize,
    steps: &[f32],
    out: &mut SignalBuffer,
) -> Vec<RdPoint> {
    crate::harness::rd_curve(
        signal,
        &TemporalDct,
        &TemporalDctConfig { block_size: block },
        steps,
        TemporalDctQuantizer::new,
        &TemporalDctRate,
        out,
    )
}

fn motion_aligned_curve(
    signal: SignalView<'_>,
    v: [f32; 2],
    block: usize,
    steps: &[f32],
    out: &mut SignalBuffer,
) -> Vec<RdPoint> {
    crate::harness::rd_curve(
        signal,
        &MotionAligned::with_velocity(v[0], v[1]),
        &MotionAlignedConfig { block_size: block },
        steps,
        MotionAlignedQuantizer::new,
        &MotionAlignedRate,
        out,
    )
}

fn patch_vq_curve(
    signal: SignalView<'_>,
    cfg: PatchVqConfig,
    steps: &[f32],
    out: &mut SignalBuffer,
) -> Vec<RdPoint> {
    let dim = patch_dim(&cfg);
    let training = extract_patches(signal, &cfg);
    crate::harness::rd_curve(
        signal,
        &PatchVq,
        &cfg,
        steps,
        move |s| PatchVqQuantizer::new(s, dim, training.clone()),
        &PatchVqRate,
        out,
    )
}

// ---------------------------------------------------------------------------
// Secondary diagnostics (mid-sweep operating point, Δ=0.125). Never primary.
// ---------------------------------------------------------------------------

fn secondary_linear<R>(
    signal: SignalView<'_>,
    rep: &R,
    config: &R::Config,
    step: f32,
) -> SecondaryDiagnostics
where
    R: Representation,
    R::Coeffs: crate::uniform::LinearCoeffs,
{
    let coeffs = rep.analyze(signal, config);
    let q = LinearQuantizer::<R::Coeffs>::new(step).quantize(&coeffs);
    crate::diagnostics::secondary_diagnostics(&q)
}

fn secondary_temporal(signal: SignalView<'_>, step: f32) -> SecondaryDiagnostics {
    let rep = TemporalDct;
    let config = TemporalDctConfig { block_size: 8 };
    let coeffs = rep.analyze(signal, &config);
    let q = TemporalDctQuantizer::new(step).quantize(&coeffs);
    let mut stream = q.frame0;
    stream.extend(q.residual);
    crate::diagnostics::secondary_diagnostics(&stream)
}

fn secondary_motion(signal: SignalView<'_>, v: [f32; 2], step: f32) -> SecondaryDiagnostics {
    let rep = MotionAligned::with_velocity(v[0], v[1]);
    let config = MotionAlignedConfig { block_size: 8 };
    let coeffs = rep.analyze(signal, &config);
    let q = MotionAlignedQuantizer::new(step).quantize(&coeffs);
    crate::diagnostics::secondary_diagnostics(&q.dct)
}

fn assemble_report(
    label: &'static str,
    curves: Vec<LabeledCurve>,
    secondary: Vec<(&'static str, SecondaryDiagnostics)>,
) -> ExperimentReport {
    for (name, pts) in &curves {
        validate_curve(pts).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    // Secondary labels are &'static str in the callers; the report's public
    // field uses String (display/test-friendly).
    let secondary: Vec<(String, SecondaryDiagnostics)> = secondary
        .into_iter()
        .map(|(n, d)| (n.to_string(), d))
        .collect();
    let targets = match common_targets(&curves, 5) {
        Some(t) => t,
        None => {
            // No rate point reachable by every condition: an honest finding
            // (the curves simply don't overlap), recorded as an empty table.
            return ExperimentReport {
                label,
                curves,
                targets: Vec::new(),
                table: Vec::new(),
                secondary,
            };
        }
    };
    for (name, pts) in &curves {
        for &t in &targets {
            assert!(
                interpolated_distortion(pts, t).is_some(),
                "{name}: target {t} not reachable (harness config error)"
            );
        }
    }
    let table = crate::harness::matched_rate_table(&curves, &targets);
    assert!(
        crate::harness::missing_from_table(&table).is_empty(),
        "matched-rate table must not contain extrapolated cells"
    );
    ExperimentReport {
        label,
        curves,
        targets,
        table,
        secondary,
    }
}

/// Render an [`ExperimentReport`] for the record: the matched-rate table
/// (MSE and PSNR at peak 1.0) plus the full sampled curves and secondary
/// diagnostics. This is what gets transcribed into
/// `docs/representation-research.md`; a report with an empty table records
/// the honest "these conditions never operate at a common rate" finding.
pub fn format_report(report: &ExperimentReport) -> String {
    let mut s = String::new();
    s.push_str(&format!("==== {} ====\n", report.label));
    if report.targets.is_empty() {
        s.push_str("No common rate point: the conditions never operate at a common rate.\n");
    } else {
        s.push_str("Matched-rate table (rate in bits/sample; MSE; PSNR-dB @ peak 1.0):\n");
        for (target, row) in &report.table {
            s.push_str(&format!("  @ {target:.5} b/s\n"));
            for (name, mse) in row {
                match mse {
                    Some(mse) => {
                        let psnr = tpt_media_representation::psnr_db(*mse, 1.0);
                        let psnr_s = if psnr.is_infinite() {
                            "inf".to_string()
                        } else {
                            format!("{psnr:.2}")
                        };
                        s.push_str(&format!("    {name:<28} mse={mse:.6}  psnr={psnr_s}\n"));
                    }
                    None => s.push_str(&format!("    {name:<28} (not in range at this rate)\n")),
                }
            }
        }
    }
    s.push_str("Sampled curves (rate b/s, mse, psnr dB):\n");
    for (name, pts) in &report.curves {
        s.push_str(&format!("  [{name}]\n"));
        for p in pts {
            let psnr = p.psnr_db(1.0);
            let psnr_s = if psnr.is_infinite() {
                "inf".to_string()
            } else {
                format!("{psnr:.2}")
            };
            s.push_str(&format!(
                "    rate={:.6}  mse={:.6}  psnr={psnr_s}\n",
                p.bits_per_sample, p.mse
            ));
        }
    }
    s.push_str("Secondary diagnostics at Δ=0.125 (never primary):\n");
    for (name, d) in &report.secondary {
        s.push_str(&format!(
            "  [{name}] entropy={:.4} b/s  nonzero={}/{}  top-k-for-99%={}\n",
            d.entropy_bits, d.nonzero, d.total, d.coefficients_for_99pct_energy
        ));
    }
    s
}

/// Targets strictly inside the intersection of every curve's sampled rate
/// range: the only rates at which all conditions can be compared by
/// interpolation. `None` if the intersection is empty (the curves are
/// genuinely incommensurable — a valid finding on its own).
///
/// A degenerate single-point intersection (possible when one condition's
/// whole envelope is a point, e.g. `motion_aligned`'s near-flat curve on the
/// easy moving-edge signal) still yields that one target: every other curve
/// must *reach* it in range for the experiment to pass, so it is a real
/// measurement, not a fabricated one.
fn common_targets(curves: &[LabeledCurve], count: usize) -> Option<Vec<f64>> {
    if curves.is_empty() {
        return None;
    }
    let low = curves
        .iter()
        .map(|(_, pts)| {
            pts.iter()
                .map(|p| p.bits_per_sample)
                .fold(f64::INFINITY, f64::min)
        })
        .fold(0.0f64, f64::max);
    let high = curves
        .iter()
        .map(|(_, pts)| {
            pts.iter()
                .map(|p| p.bits_per_sample)
                .fold(f64::NEG_INFINITY, f64::max)
        })
        .fold(f64::INFINITY, f64::min);
    if !(low.is_finite() && high.is_finite()) || high + 1e-9 < low {
        return None;
    }
    if (high - low).abs() < 1e-6 || count <= 1 {
        return Some(vec![low]);
    }
    let log_low = low.log10().clamp(-6.0, 6.0);
    let log_high = high.log10().clamp(-6.0, 6.0);
    Some(
        (0..count)
            .map(|i| {
                let f = if count == 1 {
                    0.5
                } else {
                    i as f64 / (count - 1) as f64
                };
                // Stay just inside the range so the test exercises
                // interpolation (not a boundary exact match).
                let t = 0.05 + 0.90 * f;
                10f64.powf(log_low + t * (log_high - log_low))
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_media_representation::{SignalBuffer, SignalDims};

    fn dims(w: usize, h: usize, f: usize) -> SignalDims {
        SignalDims {
            width: w,
            height: h,
            frames: f,
        }
    }

    #[test]
    fn oracle_moving_edge_is_a_valid_measurement() {
        let steps = crate::harness::default_steps();
        let mut out = SignalBuffer::zeroed(dims(64, 64, 8));
        let report = oracle_moving_edge(64, 64, 8, &steps, &mut out);
        println!("{}", format_report(&report));
        assert_eq!(report.curves.len(), 4, "four conditions");
        assert!(!report.targets.is_empty(), "targets must be chosen");
        for (name, pts) in &report.curves {
            validate_curve(pts).expect(name);
            assert!(
                pts.iter()
                    .all(|p| p.mse.is_finite() && p.bits_per_sample.is_finite()),
                "{name}"
            );
        }
        assert!(
            crate::harness::missing_from_table(&report.table).is_empty(),
            "no extrapolation anywhere"
        );
    }

    #[test]
    fn oracle_is_deterministic_run_to_run() {
        let steps = crate::harness::default_steps();
        let mut oa = SignalBuffer::zeroed(dims(64, 64, 8));
        let mut ob = SignalBuffer::zeroed(dims(64, 64, 8));
        let a = oracle_moving_edge(64, 64, 8, &steps, &mut oa);
        let b = oracle_moving_edge(64, 64, 8, &steps, &mut ob);
        assert_eq!(
            a.curves, b.curves,
            "identical inputs must reproduce identical output"
        );
    }

    #[test]
    fn every_phase_is_a_valid_measurement() {
        let steps = crate::harness::default_steps();

        let mut out0 = SignalBuffer::zeroed(dims(64, 64, 1));
        let r0 = phase0_baseline_vs_dct(64, 64, &steps, &mut out0);
        println!("{}", format_report(&r0));
        assert_eq!(r0.curves.len(), 2);
        assert!(!r0.targets.is_empty());

        let mut out2 = SignalBuffer::zeroed(dims(64, 64, 1));
        let r2 = phase2_blocks_vs_patches(64, 64, &steps, &mut out2);
        println!("{}", format_report(&r2));
        assert_eq!(r2.curves.len(), 2);
        assert!(!r2.targets.is_empty());

        let mut out4 = SignalBuffer::zeroed(dims(64, 64, 8));
        let r4 = phase4_stationary_vs_motion_aligned(64, 64, 8, &steps, &mut out4);
        println!("{}", format_report(&r4));
        assert_eq!(r4.curves.len(), 2);
        assert!(!r4.targets.is_empty());

        let mut out5 = SignalBuffer::zeroed(dims(64, 64, 8));
        let r5 = phase5_coefficients_vs_dictionary(64, 64, 8, &steps, &mut out5);
        println!("{}", format_report(&r5));
        assert_eq!(r5.curves.len(), 2);
        assert!(!r5.targets.is_empty());
    }
}
