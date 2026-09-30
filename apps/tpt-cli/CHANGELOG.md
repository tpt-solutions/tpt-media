# Changelog — tpt-cli

Scoped record of changes to `tpt-cli`. The workspace-wide release log —
including cross-cutting changes and the lockstep version bump that ships every
crate together — is the root [`CHANGELOG.md`](../../CHANGELOG.md). This file
is a filtered view of it: a change touching only this crate is recorded here
*and* summarised in the root entry; a change touching several crates is
recorded in the root, which remains the record (CONTRIBUTING §9.1).

Format follows [Keep a Changelog](https://keepachangelog.com/). Versions are
lockstep with the workspace.

## [Unreleased]

### Added

- **Per-crate README, changelog, keywords and categories** (2026-09-30):
  crate-specific `README.md` and manifest `keywords`/`categories`. The README
  documents the full command surface, the option table, the exit codes, and
  the audio-only scope of the encode family.
- §36 video corpus (`video.rs`) and §37 video quality metrics (2026-09-28):
  eight deterministic seeded scenes in both coded formats; `psnr_db` (`None`
  for bit-exact frames — a real corpus case, not a corner), `ssim` over 8×8
  luma windows, `video_bitrate_bits_per_second`, `bits_per_pixel`, and a
  `MemoryReport` that is `None` unless the process installed the tracking
  allocator — an absent measurement and a measurement of zero are different
  facts. `tests/video_quality.rs` drives every scene through real Thrum round
  trips.
- §42 allocation profiling and peak-memory measurement (`memprobe.rs` +
  `tests/memory_budget.rs`, 2026-09-28): one reusable
  `TrackingAlloc`/`MemoryProbe` harness reporting the high-water mark of *live*
  heap bytes (the number a memory budget is written in) and cumulative churn,
  with a saturating free path so a dealloc predating the window cannot wrap
  the counters. It measures every implemented codec against the budget table
  in `docs/performance.md` and pins Thrum's working set length-independent.
- Y4M bridge for video (`video`/`y4m` modules, 2026-09-28).
- `--codec thrum` resolves and `list-codecs` advertises it, with an explicit
  "video codec, audio-only" rejection from the encode family rather than a
  misleading unknown-codec error.

### Removed

- Exit code 3 ("codec not wired yet"): with every listed codec implemented, no
  code path produced it, and the stale documentation claiming otherwise was
  removed.

### Fixed

- **The hand-rolled JSON writer produced invalid JSON** for probe/benchmark
  output (found 2026-09-14 while adding `benchmark --json`; three related
  defects): `J::object()`/`J::arr()` emitted a separator *after* `field()`
  had already emitted one, producing `"key":,{...}`; `J::close()` chose its
  closing bracket from the *parent* container, so an object inside an array
  closed with `]`; and the probe track-list serializers emitted
  `elem()`-separated objects without ever opening the array, producing
  `"audio":,{...}` for empty lists. Probe `--json` for any file with tracks was
  malformed. All three fixed, with regression tests pinning exact output
  shapes.

## [0.1.0] — 2026-09-04

### Added

- Initial crate, todo.md §43 (2026-09-08): the `tpt` command-line interface —
  `probe`/`inspect`/`list-codecs`/`--help`, plus live `encode` and `decode`
  (16-bit mono WAV at 44.1/48 kHz → `cinder`/`aura` → `.tpt` and back, with
  codec-private parameter bytes, full track metadata, `--set-meta` tags, and
  the exact sample count recorded under the reserved `tpt.io.sample_count`
  key so decode can trim padding). 27 unit tests including a lossless Aura
  round trip, driven entirely through injected `Ctx` I/O closures.
- `transcode` and `benchmark` went live on 2026-09-14 (decode → re-encode,
  carrying source tags forward; and an untimed warm-up plus `--runs` measured
  passes reporting median/min wall time, real-time factors, payload bytes,
  bits-per-sample, bitrate and the §37 quality metrics, with `"bit_exact":
  true` replacing an unrepresentable infinite `snr_db`).
- The §36 audio corpus (`signals.rs`): deterministic signals from closed-form
  math plus SplitMix64, so no binary fixtures are committed.
