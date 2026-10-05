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

- **Vault and Mote are playable in the browser** (2026-10-05, closing the
  route the 2026-10-01 entry documented): both codecs became dependencies of
  the crate and joined `DecoderTable::native()` with builders mirroring how
  `tpt-cli` reconstructs a video decoder from a track (geometry from
  `TrackMedia`, per-codec parameters from the codec-private bytes, the
  codec's own factory doing the validation). The rebuilt `tpt_web.wasm` is
  ~368 KB (from 328), and the Node checks (`check.mjs`, `mock-browser.mjs`)
  pass against it. The extension point itself is unchanged and remains
  Rust-only — this entry is the "one line in `native()`" route in action,
  including the honest footnote that the wasm binary is fixed at build time,
  so a browser gets exactly the codecs compiled in.
- **Recorded the `tpt-web` extension point's real reachability** (2026-10-01,
  found by asking who can actually call it rather than re-reading the claim).
  `DecoderTable` is described as "the extension point" for Vault and Mote, and
  `docs/web-design.md` §8 is titled "Extension point for Vault and Mote" — but
  `tpt_web_open` builds its player with `Player::new(limits)`, which is
  `DecoderTable::native()` exactly. None of the 18 ABI exports can accept a
  table, and a `DecoderBuilder` is a Rust `fn` pointer that could not cross the
  boundary in any case. So **the extension point is unreachable from a
  browser**: no amount of `register` makes Vault or Mote playable in one, and
  the only route is for a codec to become a dependency of `tpt-web` and get one
  line in `DecoderTable::native()`. That is the honest current state — the two
  video codecs are reachable by the CLI and not by the web player — and it was
  not stated anywhere, so the section read as though browser registration were
  available. Documented in `docs/web-design.md` §8, the crate README, and on
  `tpt_web_open` itself where a maintainer would look. No behaviour changed: an
  ABI function taking a table would mean shipping function pointers across the
  boundary, which §4's "no pointer flows from JS into Rust" rule rules out.
  Per-codec feature flags and per-codec `open_*` entry points are the
  alternatives; neither is built and neither is claimed.
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
  glue, no DOM dependency), `demo.tpt`, and `build.mjs`. No bundler. **Run in
  a real browser** (2026-10-01) — see `web/browser-check.mjs` below.
- **Real-browser check** (2026-10-01, todo.md §45): `web/browser-check.mjs` +
  `web/serve.mjs`, zero-dependency like the rest of `web/`. Serves the folder
  over http (ES modules and `WebAssembly` cannot load from `file://`), then
  drives the **unmodified** player in headless Chrome/Edge: a real
  `AudioContext` (observed by subclassing it, so every call still reaches the
  genuine object), a real 2D canvas read back through the browser's own
  `getImageData`, real `requestAnimationFrame`, the real wasm. 29 checks, all
  passing, repeated 3x at ~8.8 s. Two limits are deliberate: it cannot prove
  a human *heard* the audio, and on a machine with no audio output device the
  playback-dependent checks print `SKIP` ("not verifiable here") rather than
  passing, because there is no clock to advance. A missing browser is an
  error, never a silent skip. The page reports in by POSTing to `/__done`
  rather than being raced against `--dump-dom`.
- **Fixed: `browser-check.html` was a syntax error and never ran at all**
  (2026-10-01). The `if (!played) ... else {` block was never closed, so the
  inline module threw `SyntaxError: Unexpected end of input` before executing
  a single line — the page sat at "running…" and the driver reported a
  timeout with no output. Found by loading the page in Chrome and reading
  its console; the file had never been parsed by anything.
- **Verification**: 49 native tests (bit-exact Aura round trip, Cinder against
  a directly driven decoder, byte-at-a-time equals whole-file, deterministic
  mutation/truncation/noise sweeps, ABI lifecycle); `web/check.mjs` runs 11
  checks against the built wasm under Node (300 hostile files, zero traps);
  `web/mock-browser.mjs` runs `tpt-web.js` against fake WebAudio/canvas;
  `web/browser-check.mjs` runs 29 checks against the real browser APIs.
- **Benchmark**: `cargo bench -p tpt-web` (native) and `web/bench.mjs` (wasm
  under Node/V8) over byte-identical inputs from `synth::benchmark_fixtures`;
  numbers in [`docs/performance.md`](../../docs/performance.md).
- CI `wasm` job builds the module and runs the two Node scripts against it,
  plus `web/browser-check.mjs` in the runner's preinstalled Chrome (where, with
  no sound card, the playback-dependent checks report `SKIP` rather than a
  pass).
- Per-crate README, CHANGELOG, `keywords` and `categories` (CONTRIBUTING
  §9.4).
