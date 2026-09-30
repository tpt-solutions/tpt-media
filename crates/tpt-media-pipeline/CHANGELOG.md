# Changelog — tpt-media-pipeline

Scoped record of changes to `tpt-media-pipeline`. The workspace-wide release
log — including cross-cutting changes and the lockstep version bump that ships
every crate together — is the root [`CHANGELOG.md`](../../CHANGELOG.md). This
file is a filtered view of it: a change touching only this crate is recorded
here *and* summarised in the root entry; a change touching several crates is
recorded in the root, which remains the record (CONTRIBUTING §9.1).

Format follows [Keep a Changelog](https://keepachangelog.com/). Versions are
lockstep with the workspace; no API-stability guarantee pre-1.0
(CONTRIBUTING §9.2).

## [Unreleased]

### Added

- **Per-crate README, changelog, keywords and categories** (2026-09-30):
  crate-specific `README.md` and manifest `keywords`/`categories`.
- `benches/pipeline_bench.rs`: a `Source → map → Sink` drive over 1 000
  packets, recorded in `docs/performance.md` (60 µs median, 60 ns/packet).

### Changed

- Documentation truth pass (2026-09-28): `ARCHITECTURE.md` §8 previously
  described ring-buffer backpressure; the implemented model is
  `NodeEvent::NeedMore` plus the `Pipeline::pull` stall bound reported as
  `NodeError::Backpressure`, and the crate's own README now says so.

## [0.1.0] — 2026-09-04

### Added

- Initial crate, todo.md §26 (2026-09-08): the full pipeline core — `Node`
  abstraction with the `NodeEvent`/`NodeError` contract, lockstep typed
  `Chain<L, R>` composition with cascading flush, `Pipeline` pull/drive with
  a stall-bound backpressure guard, the built-in `Source`/`Map`/`Bounded`/
  `Cancellable`/`Sink` nodes, `Timestamped<T>` PTS/DTS/duration pass-through,
  stream and format negotiation, the `Executor` threading abstraction
  (`Inline` plus std-only `ThreadedExecutor`), and realtime scheduling
  (`RealtimeProfile`, clock-injected `drive_scheduled`, `LatencyObserver`/
  `LatencyLog`, `ScheduledRun`). 21 unit tests plus a container integration
  test. `no_std + alloc`.
