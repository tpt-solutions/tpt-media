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

## [0.1.0] — 2026-09-04

### Added

- Initial crate (todo.md §56): the swappable media representation contracts —
  `Representation` (analysis/synthesis), `Quantizer`, `RateEstimator`,
  `DistortionMetric` with `Mse`/`Rmse`/`psnr_db`, the spatiotemporal signal
  type `SignalView`/`SignalViewMut`/`SignalBuffer`/`SignalDims`, and
  sparsity reporting (`SparsityReport`, `CoeffMagnitudes`). `T` is never
  assumed to be a DCT, wavelet, block transform or motion-compensated
  residual; the point is to be able to swap all four and compare honestly.
