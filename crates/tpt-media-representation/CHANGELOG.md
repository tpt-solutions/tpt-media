# Changelog — tpt-media-representation

Scoped record of changes to `tpt-media-representation`. The workspace-wide
release log — including cross-cutting changes and the lockstep version bump
that ships every crate together — is the root
[`CHANGELOG.md`](../../CHANGELOG.md). This file is a filtered view of it: a
change touching only this crate is recorded here *and* summarised in the root
entry; a change touching several crates is recorded in the root, which remains
the record (CONTRIBUTING §9.1).

> **Experimental.** Per CONTRIBUTING §9.2 this crate's API may churn without
> an entry here, and it must not be depended on outside the workspace.

Format follows [Keep a Changelog](https://keepachangelog.com/). Versions are
lockstep with the workspace.

## [Unreleased]

### Added

- **Per-crate README, changelog, keywords and categories** (2026-09-30):
  crate-specific `README.md` and manifest `keywords`/`categories`. The README
  states the crate's experimental status and its offline-only profile up
  front, rather than leaving a reader to infer either.
- **§38 unit-test sweep over `tpt-media-representation`'s public API**
  (2026-10-01): 26 new inline tests (10 → 36) plus `tests/contracts.rs`, the
  crate's **first integration test** (8 tests), covering the §38 categories
  that had no coverage — empty input, maximum values, overflow, edge cases,
  malformed input, and the documented panics. The three trait-only modules
  (`representation`, `quantizer`, `rate`) had **no tests at all** and still do
  not inline: `Representation`, `Quantizer`, `RateEstimator` and
  `CoeffMagnitudes` have no implementor inside this crate, so their contracts
  are only observable from outside it. `tests/contracts.rs` implements all
  four for real and drives the full analyse → quantise → charge → dequantise →
  synthesise → measure loop, pinning the `Quantized`-carries-everything rule,
  the `RateEstimator` "no free decoder-side information" rule, and a monotone
  rate/distortion frontier across a six-point sweep. Three of the new tests
  fail against the pre-change code, verified by restoring each old body and
  re-running.

### Fixed

- **`tpt-media-representation`'s `SignalDims::frame_len`/`samples` answered
  differently depending on the build profile** (2026-10-01, found while
  auditing the crate's public API against §38's unit-test gate). Both computed
  `width * height [* frames]` with a plain `*`, which is a **panic in debug and
  a silent wrap in release**: the same `SignalDims` described two different
  volumes depending on the profile, and the wrapped value is what every caller
  derives from — the `Vec` allocations, the `frame()` slices, the sample
  indices. `SignalDims { width: usize::MAX, height: 2, frames: 1 }` wrapped to
  18 446 744 073 709 551 614 samples. Both products are now `checked_mul` with
  a stated message, so the two profiles agree. This is the same class of defect
  as `tpt-media-pipeline`'s `Source::from_range` one release back.
- **`SignalBuffer::sample_bilinear_toroidal` divided by zero on a
  zero-extent volume** (2026-10-01, same audit). With `width == 0`,
  `x.rem_euclid(0.0)` is `NaN`, `NaN as usize` is `0`, and the following
  `% width` then panics with `attempt to calculate the remainder with a divisor
  of zero` — a message that names the arithmetic rather than the cause. A
  zero-extent volume is legally constructible from outside the crate
  (`SignalBuffer::zeroed` accepts it), so the panic was reachable through the
  public API. The precondition is now stated and checked.

## [0.1.0] — 2026-09-04

### Added

- Initial crate (todo.md §56): the swappable media representation contracts —
  `Representation` (analysis/synthesis), `Quantizer`, `RateEstimator`,
  `DistortionMetric` with `Mse`/`Rmse`/`psnr_db`, the spatiotemporal signal
  type `SignalView`/`SignalViewMut`/`SignalBuffer`/`SignalDims`, and
  sparsity reporting (`SparsityReport`, `CoeffMagnitudes`). `T` is never
  assumed to be a DCT, wavelet, block transform or motion-compensated
  residual; the point is to be able to swap all four and compare honestly.
