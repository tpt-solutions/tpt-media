# Changelog — tpt-media-research

Scoped record of changes to `tpt-media-research`. The workspace-wide release
log — including cross-cutting changes and the lockstep version bump that ships
every crate together — is the root [`CHANGELOG.md`](../../CHANGELOG.md). This
file is a filtered view of it: a change touching only this crate is recorded
here *and* summarised in the root entry; a change touching several crates is
recorded in the root, which remains the record (CONTRIBUTING §9.1).

> **Experimental.** Per CONTRIBUTING §9.2 this crate's API may churn without
> an entry here, and it must not be depended on outside the workspace.

Format follows [Keep a Changelog](https://keepachangelog.com/). Versions are
lockstep with the workspace.

## [Unreleased]

### Added

- **§38 unit-test sweep over `tpt-media-research`'s public API** (2026-10-01):
  26 new inline tests (51 → 77) plus `tests/harness_contract.rs`, the crate's
  **first integration test** (7 tests), covering the §38 categories that had
  no coverage — empty input, maximum values, overflow, edge cases, malformed
  input, and the documented panics. The inline tests can reach private items,
  so `tests/harness_contract.rs` is what actually proves the platform
  composes *from outside* the crate, which is the whole claim: it drives
  `baseline` and `block_dct` through the same `rd_curve` machinery, checks
  every reported value is finite (the crate's stated validity contract),
  checks a matched rate is interpolated but never extrapolated, that
  `rd_point` and `rd_curve` agree on the same step, and that repeated
  measurements are byte-identical — the property the published numbers in
  `docs/representation-research.md` rest on.

### Fixed

- **`rd_point`/`rd_curve` reported `NaN` bits-per-sample for an empty volume**
  (2026-10-01, found during that sweep). The rate is `total_bits /
  signal.data().len()`, which is `0/0` when the volume has no samples — and
  an empty `SignalBuffer` is constructible from outside the crate. That
  broke the crate's own documented validity contract ("every reported value
  is finite"), and it was worse than a bad number: `partial_cmp` on `NaN` is
  `None`, so the sort in `rd_curve` and the dedup in
  `interpolated_distortion` both degrade to treating every comparison as
  equal, silently corrupting the whole curve. An empty volume transmits and
  distorts nothing, so the honest rate is 0 bits/sample.

- **`codebook_mse` returned `NaN` for an empty training set** (2026-10-01,
  same sweep). It asserts `entries > 0` but never `n > 0`, so the final
  `total / n` was `0/0`. This function exists precisely to *compare* two
  codebooks, and a `NaN` there orders as "less than" any real codebook —
  quietly picking a winner. Now 0, and `dim == 0` is rejected explicitly
  rather than dividing by it.

## [0.1.0] — 2026-09-04

### Added

- Initial crate (todo.md §56): the representation-research harness — synthetic
  signal generators (`signals::spatial`, `signals::spatiotemporal`),
  deterministic seeded PRNG (`det_rng`), empirical entropy estimation
  (`entropy`), the matched-rate distortion harness, the concrete
  representations under test (`block_dct`, `motion_aligned`, `patch_vq`,
  `temporal_dct`), quantizer construction helpers (`kmeans`, `uniform`),
  diagnostics, and the oracle experiment and methodology phases
  (`experiments`). Determinism is part of the contract: no `rand`, no
  thread-local state, no I/O — same inputs and same seed produce
  byte-identical output.
