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

- **Per-crate README, changelog, keywords and categories** (2026-09-30):
  crate-specific `README.md` and manifest `keywords`/`categories`.

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
