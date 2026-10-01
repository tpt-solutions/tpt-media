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

### Fixed

- **The crate could not actually be built without `std`** (2026-10-01, found
  by auditing every crate's `no_std` claim). The `std` feature, the
  `#![cfg_attr(not(feature = "std"), no_std)]` attribute, the README's
  `no_std: Yes` row and CI's `wasm` job all said the crate builds without std
  — but all three dependencies were pinned in `[dependencies]` with
  `features = ["std"]`, which turns `std` on for the whole graph whatever a
  consumer asks for. `--no-default-features` therefore could not switch it off,
  and the CI check could not have failed on a real `std` leak. Now
  `default-features = false`, with `std` still forwarded through this crate's
  own `std` feature. Verified with the exact CI command.
- **`Source::from_range` answered differently depending on the build
- **`Source::from_range` answered differently depending on the build
  profile.** Converting an exclusive end bound to an inclusive one formed
  `hi - 1`, which underflows at the ends of the type: `from_range(..0u8)` was
  a panic in debug and a silent wrap to 255 in release — 256 items where the
  range contains none. The bound arithmetic no longer forms `hi - 1` or
  `lo + 1` before the emptiness test.
- **Every empty range yielded one item.** The first value was pushed before
  the bounds were compared, so `from_range(1..1)` produced `[1]` and
  `from_range(5..3)` produced `[5]`. Empty and inverted ranges now yield no
  items.
- An unbounded end bound no longer means "the high value is zero", which made
  `from_range(0..)` produce `[0]` and `from_range(5..)` produce `[5]` — an
  arbitrary answer indistinguishable from a working pipeline. It now panics
  with a stated reason, matching the crate's uniform "panic on invalid input
  rather than answer wrongly" rule; an unbounded *start* still means zero.
- `Bounded`'s doc claimed `NodeError::BufferOverflow` is reported past `cap`.
  Through a `Chain` it cannot be: the node takes one input and emits at most
  one output, so the queue never exceeds `window`, and `cap >= window` is
  asserted in the constructor. The doc now says so and the invariant is
  pinned by a test.

### Added

- **§38 unit-test sweep over the crate's public API** (2026-09-30): 20 new
  tests (21 → 41) covering the categories that had no coverage — empty input
  (empty source, empty and inverted ranges, `Sink` before any item), maximum
  values (ranges ending at the largest representable value), overflow
  (`from_range` bound arithmetic, `Bounded`'s constructor panics), edge cases
  (`Source::consumed`, `Bounded::len`/`is_empty`, `pull` with a zero and an
  undersized stall bound, flush draining a partial buffer, cancellation
  reaching `flush`), every `MediaFormat` variant, multi-candidate format
  negotiation, `StreamSpec` merging with an undeclared timebase, and the
  `ThreadedExecutor`'s worker-panic mapping. Five of them fail against the
  previous `from_range` — verified by restoring the old body and re-running —
  which is the evidence that they test the defects above rather than the
  current behaviour. None of the tests changed behaviour except as listed
  under *Fixed*.
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
