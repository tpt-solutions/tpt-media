# Changelog — tpt-web

Scoped record of changes to `tpt-web`. The workspace-wide release log —
including cross-cutting changes and the lockstep version bump that ships every
crate together — is the root [`CHANGELOG.md`](../../CHANGELOG.md). This file
is a filtered view of it: a change touching only this crate is recorded here
*and* summarised in the root entry; a change touching several crates is
recorded in the root, which remains the record (CONTRIBUTING §9.1).

Format follows [Keep a Changelog](https://keepachangelog.com/). Versions are
lockstep with the workspace.

## [Unreleased]

### Added

- **The crate** (2026-09-30, todo.md §45): a browser player core for `.tpt`
  media. Bounded incremental `.tpt` parser (`StreamParser`), a
  `DecoderTable` over Cinder, Aura, Reed and Thrum (the extension point for
  Vault and Mote, which are not dependencies), a push/pull streaming
  `Player` producing planar `f32` audio and RGBA8 video under hard memory
  limits (`WebLimits`), and an 18-function `extern "C"` surface over integer
  handles for `wasm32-unknown-unknown`. No `wasm-bindgen`: see
  [`docs/web-design.md`](../../docs/web-design.md).
- **Browser demo** (`web/`): `index.html`, `tpt-web.js` (fetch + WebAudio +
  canvas playback on an AudioContext master clock), `tpt-decoder.js` (ABI
  glue, no DOM dependency), `demo.tpt`, and `build.mjs`. No bundler.
  **Not run in a real browser.**
- **Verification**: 49 native tests (bit-exact Aura round trip, Cinder against
  a directly driven decoder, byte-at-a-time equals whole-file, deterministic
  mutation/truncation/noise sweeps, ABI lifecycle); `web/check.mjs` runs 11
  checks against the built wasm under Node (300 hostile files, zero traps);
  `web/mock-browser.mjs` runs `tpt-web.js` against fake WebAudio/canvas.
- **Benchmark**: `cargo bench -p tpt-web` (native) and `web/bench.mjs` (wasm
  under Node/V8) over byte-identical inputs from `synth::benchmark_fixtures`;
  numbers in [`docs/performance.md`](../../docs/performance.md).
- CI `wasm` job builds the module and runs the two Node scripts against it.
- Per-crate README, CHANGELOG, `keywords` and `categories` (CONTRIBUTING
  §9.4).
