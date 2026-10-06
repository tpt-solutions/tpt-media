# Changelog

All notable changes to `tpt-media` are recorded here. Dates are ISO-8601
(YYYY-MM-DD). Versions follow [Semantic Versioning](https://semver.org/).

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

Added

- **Vault's B frames gain per-frame weighted prediction — the wire's
  v3 bump, v0.3 stage 1** (2026-10-07): a B frame's header carries a
  2-bit `bweight` naming the *past* reference's share of the
  bi-prediction in quarters (plain average, 1/4, or 3/4; the future
  takes the complement; `3` reserved-rejected), applied by one shared
  combine in `build_pred` so encoder and decoder weight identically by
  construction. The encoder chooses per B frame by exact frame `J` —
  three full encodes (average and both quarter weights), ties to the
  average, the same bounded-search spirit as the rate controller, up to
  3× B-frame encode cost. Measured
  (`measure.rs::b_weight_decisions`): the search fires readily — on an
  ordinary panning clip it picks quarter weights on most B frames at
  both qp 30 and 18, because the two motion searches land on different
  vectors and tilting the combine prices cheaper than the residual it
  saves — and keeps the plain average where that is genuinely cheapest.
  `tests/weighted_b.rs` pins the exact-reconstruction case (a B frame
  built as the decoder's own 3/4-past blend reconstructs perfectly),
  the weight census, determinism, and the reserved value's rejection.
  v1/v2 streams are rejected fail-fast under v3 (todo.md §35); nothing
  released ever emitted v2, whose B-leaf payload was corrected in the
  same unreleased window. Also fixed here: `VaultConfig::max_packet_bits`
  still charged the *v1* header (29 bits against the real 46) — the
  bound is a stated proof, and its header term had been stale since the
  v2 bump; it now charges the v3 header (48, with the B weight counted
  unconditionally so I/P frames are over-charged by 2 bits and the
  proof covers every frame type).
- **Vault's keyframe schedule counts every coded frame** (2026-10-07):
  `keyframe_interval` is now a display-frame cadence even with
  `b_frames > 0` — B frames and the lookahead's future reference advance
  it, and a due keyframe inside a buffered B-group fires through the
  degenerate path at the group's first display frame. The first
  implementation advanced the counter on group anchors only, so a
  `b_frames = 2` stream quietly ran a four-times-longer GOP than
  configured (240 display frames against a configured 60). I/P streams
  are unchanged — there every coded frame is a reference frame — pinned
  by `tests/b_frames.rs`'s cadence test (keyframes at displays 0 and 4
  for `b_frames = 1`, `keyframe_interval = 4`, where the old counter
  produced only the cold-start intra).
- **Vault's loop filter can be rate–distortion decided, and scene cuts
  break the keyframe schedule** (2026-10-07, two of todo.md §33's open
  encoder items): `VaultConfig::loop_filter_rdo` decides the per-frame
  loop-filter header bit after the frame is coded — keeping whichever of
  the filtered and unfiltered reconstructions is closer to the source,
  patching the finished packet's header bit (offset pinned against a
  decoder parse) — and `VaultConfig::scene_cut` codes the first frame
  after a hard cut intra when its mean absolute luma difference from the
  most recent reference spikes above both an absolute floor and a factor
  times the previous frame's difference. Both knobs are encoder-only and
  default **off**, `rdoq`-style, pending real content. The honest
  measurements (harnesses and tables in `measure.rs` /
  `docs/performance.md`): the loop-filter decision reproduces the
  stream-level fixed-flag preference on textured content and on pure
  noise, but on the smooth panning clip it keeps the filter on every
  frame while the stream decodes better without it — the filter's harm
  there is a *prediction* effect no per-frame decision can price; the
  scene-cut detector saves tail bytes, not distortion (the RDO's
  intra-leaf fallback already keeps a P-after-cut's quality), 288 vs
  297 B post-cut at equal PSNR on the synthetic cut. Supporting changes:
  a cut (or any intra) now gets the rate controller's 4× intra allowance
  rather than only the first frame, the keyframe schedule is a
  `since_intra` counter so a detected cut re-arms it (scheduled keyframes
  stay one interval apart after a cut; default-config streams are
  bit-identical to before), and `FrameStats` gained a `loop_filter` field
  so tests can observe the per-frame decision. Vault's README, whose
  Known-limitations list predated rate control, B-frames and the fast
  transform, was brought up to date in the same change.
- **§40's randomized SIMD-equivalence sweeps landed in `tpt-dsp`**
  (2026-10-05, upstream): five SplitMix64-seeded sweeps drive the SIMD
  and scalar paths with random values on every nightly `simd` build —
  element-wise mul/add/magnitude, the FFT butterfly bit-exactly against
  the hand expansion, and the full `fft_inplace_f32` against the
  scalar FFT at lengths 4..1024 — closing the "fixed vectors only"
  gap that §40 recorded. Also upstream: the `tpt-dsp-io` `audio_check`
  example, which had failed to compile for every workspace build
  (it imports `AudioError`, an `audio`-feature-only export), is now
  declared with `required-features` and the full `tpt-dsp` workspace
  test suite is green for the first time in this clone.

- **The fast transform: Vault's encoder is 11-26× faster and every
  corpus scene now fits the frame budget** (2026-10-05, §33's "faster
  encoder" item closed). Following the profile that put the transform
  at 88-97% of encode time, `tpt-dsp-core` gained
  `FastDct2Plan`/`FastDct2dPlan` — FFT-backed DCT-II/III sharing one
  length-`2n` inverse FFT with a half-twiddle extraction, no input
  reordering, verified against the direct pair at 4-128 points and
  4×16-32×32, measured 4.4-15× faster than direct at the video tile
  sizes — and Vault's `Xform` now runs on it in std builds (one plan
  per tile edge 4/8/16/32; bare metal keeps the direct `dct_2d` path,
  no_std build verified). Measured over the criterion bench's own
  clips: QCIF I-frame 180 → **15.8 ms** (45 → 3.9 ms/frame), CIF
  I-frame 690 → **51.3 ms**, decode 7.4 ms → **0.65 ms** — with rate
  and PSNR byte-identical to the direct transform's figures (202
  B/packet @ 37.11 dB). The §36 corpus re-measured across all three
  video codecs: every Vault scene fits the 33.3 ms budget at QCIF
  (worst: high-motion 22.2 ms encode), where the direct transform
  missed it on all but two. The web player's wasm was rebuilt with the
  fast path (371 KB) and its Node checks pass. Vault's provenance row
  moved to reviewed (the DCT and the FFT-based computation are
  textbook, both GREEN).
- **Vault's encoder cost is now profiled, not suspected** (2026-10-05,
  §33's "faster encoder" item): temporary phase timers over the
  criterion bench's own QCIF clip measured the transform (`dct_2d` +
  quantise + `idct_2d` reconstruction, priced per RDO candidate) at
  **97.2%** of intra-frame encode time (9 024 transform calls, 44.2
  ms/frame) and **88.2%** of a P-frame stream, with the motion
  search's 89 627 SAD evaluations at **8.6%**. The suspicion written
  into the item is confirmed with numbers, the instrumentation is
  reverted, and the levers are recorded where the work will happen: a
  fast transform path (the `FastDctIvPlan` shape from §5.2) first,
  fewer priced candidates per leaf second. Method and numbers in
  `docs/performance.md`'s Vault section.
- **The two 0.4 release gates assessed and closed** (2026-10-05):
  "advanced quality testing" and "security hardening" were bare lines
  from the original plan. Assessed against what exists, both are
  satisfied with their remainders honestly relocated: quality testing
  now means the §36 corpora driven through every implemented codec,
  the §37 metric set (SNR/LSD/PSNR/SSIM/bpp/latency), rate/quality
  curves via `benchmark --quality`, and the cross-codec `tpt
  video-corpus` comparison — with perceptual/subjective metrics and
  real speech/music corpora deliberately still out (tracked in §37).
  Security hardening's content landed across §19/§47 (limits
  everywhere, three real overflow/bomb bugs fixed, container
  zero-timebase hardening, wasm memory caps, deterministic sweeps in
  CI) — the open remainder is *continuous* coverage-guided fuzzing,
  which §54 tracks as its own item.
- **Every public API is now documented — and it stays that way by
  enforcement, not review** (2026-10-05, §54's "public APIs have
  documented contracts"): a `-W missing_docs` sweep measured the real
  coverage instead of assuming it and found 187 undocumented public
  items across the workspace — 39 in `tpt-media-codec`, 47 in
  `tpt-media-probe`, 35 in `tpt-media-research`, 22 in
  `tpt-media-representation`, and the rest scattered (the three newest
  codecs were at zero; the older foundation crates predated the
  standard). All 187 are documented, `#![warn(missing_docs)]` is now
  on in all seventeen library crates so the CI clippy gate
  (`-D warnings`) fails any future undocumented public item, and the
  sweep surfaced and fixed four broken/private intra-doc links plus
  two container workspace declarations whose `default-features` was
  being silently ignored (pipeline's `tpt-container` dependency now
  opts into `std` properly, and its no_std build still passes).
  `cargo doc --workspace` remains zero-warning; 68 test binaries stay
  green.
- **Vault and Mote are playable in the browser** (2026-10-05): both video
  codecs joined `tpt-web` as dependencies and entered
  `DecoderTable::native()` with builders that reconstruct a decoder from the
  track exactly as `tpt-cli` does (geometry from `TrackMedia`, parameters
  from the codec-private bytes, the codec's factory validating). This closes
  §33's "the web player does not include Vault" via the route
  `docs/web-design.md` §8 recorded on 2026-10-01 (the extension point is
  Rust-only; a codec becomes browser-playable by becoming a dependency and
  taking its `native()` line). The rebuilt `tpt_web.wasm` is ~368 KB (from
  328 KiB); `check.mjs`/`mock-browser.mjs` pass against it, and the crate
  README, `docs/web-design.md`, and `codecs.rs` docs no longer describe the
  codecs as absent.
- **Vault and Mote now build bare-metal; the CI `no_std` job covers
  every codec that claims it** (2026-10-05): §33's "bare-metal `no_std`
  verification" item was the claim-then-verify pattern again — the
  first `--target thumbv7em-none-eabi --no-default-features` build of
  `tpt-video-vault` failed inside `bitflags`, because both video
  codecs' dependency blocks forced `features = ["std"]` onto
  `tpt-media-core`/`-bitstream`/`-codec`/`tpt-dsp-core` unconditionally
  (reed, the one previously verified codec, correctly used
  `default-features = false`). Both manifests now follow reed's shape;
  no `libm` was needed — the codecs' float math stays inside `core` —
  so no new dependencies. Vault and Mote build bare-metal locally and
  in CI next to reed.
- **§5.3 (2D transforms) audited and closed; AArch64 cross-check in CI**
  (2026-10-05):
  - `tpt-dsp` shipped the separable 2D DCT-II/III pair
    (`dct_2d`/`idct_2d`) and Thrum and Vault consume it — 8 of §5.3's
    nine items are now checked against the actual code (API, both
    directions, 4×4/8×8/16×16 with Vault coding 4×4–32×32 and Thrum
    8×8 as live consumers, the `bench_dct_2d` benchmarks, and the
    naive-double-sum verification). The one deliberately-open item is
    SIMD-optimising the block sizes: the scalar transform is already
    cheap at 64-coefficient blocks and encode cost is dominated by
    motion search and RDO.
  - CI gained an `aarch64` job: `cargo check` of every Rust crate for
    `aarch64-unknown-linux-gnu` (no linker needed; `tpt-media-sys`
    excluded — its build script compiles the C integration test with a
    target-unknown compiler — and `--all-targets` off because
    criterion's `alloca` build script wants a cross C compiler). §40's
    architecture-CI item narrows accordingly; test *execution* on
    AArch64 and cross-`target-feature` builds remain its open part.
  - §53's "benchmark suite" gate flips: the Vault/Mote-era benches it
    waited on landed 2026-09-30 with recorded numbers; CI-side
    recording stays deliberately a release-time ritual per CONTRIBUTING
    §11.
- **`tpt-video-vault`: the Vault high-compression video codec (todo.md §33)**
  (2026-09-30): new workspace member `codecs/vault`, codec id `VAUL`. An I/P
  codec (no B-frames) with 64×64 superblocks recursively partitioned to 8×8,
  half-pel motion vectors (pattern search, 2-tap bilinear interpolation),
  up to 16 reference frames, four intra modes, 4×4–32×32 DCT from
  `tpt-dsp-core`, static Rice coding, an H.263 Annex J loop filter, a
  versioned bitstream with a CRC-32 trailer and a derived packet-size bound.
  The RDO compares `J = D + λR` per partition node (`R` is the real writer's
  exact bit count) and is a greedy search over a pruned candidate set —
  **no global optimum is claimed**. 38 tests, including a corruption sweep
  that repairs the CRC so the parser itself is exercised, resource-limit
  tests (the first codec to exercise `max_recursion_depth`, so §19/§47's
  recursion items are now checked), and a census test that proves
  partitioning, half-pel motion and multiple references are actually used.
  Measured (release, 176×144 4:2:0 synthetic clips): about 45–55 ms/frame to
  encode, 1.9 ms to decode; forbidding partitions above 8×8 costs 63% more
  bytes at equal quality; the loop filter helped a textured clip and hurt a
  smooth one. **Wired into the CLI** (2026-10-01): `--codec vault` drives
  `encode`, `decode`, `transcode` and `benchmark` through the y4m bridge, and
  `tpt video-corpus --codec vault` measures it over the §36 corpus
  (cross-codec figures in `todo.md` §33). Not yet a `tpt-web` dependency.
  Design notes in `docs/codec-design.md`, measurements in
  `docs/performance.md`, sources in `docs/provenance.md` (every row
  `under-review`: no maintainer patent review yet).
- **`tpt-web`: the WASM browser player (todo.md §45)** (2026-09-30): new
  workspace member `apps/tpt-web`. A bounded incremental `.tpt` parser
  (`StreamParser`, reusing `tpt-container`'s chunk decoders), a `DecoderTable`
  over Cinder/Aura/Reed/Thrum as the extension point for Vault and Mote (not
  dependencies), a push/pull `Player` that yields planar `f32` audio and RGBA8
  pictures with end-to-end back-pressure, and hard memory limits
  (`WebLimits`: input backlog, packet/metadata/track-descriptor sizes checked
  from the chunk header before buffering, RGBA frame size, output queues at a
  soft limit and a 2x hard stop, decode-error budget; worst case 223 MiB for
  the defaults, pinned by a test, plus a `--max-memory` link cap). Exposed
  through 18 `extern "C"` functions over integer handles with no pointer ever
  flowing JS to Rust; **deliberately not `wasm-bindgen`** (the surface is
  integers and pointers; rationale in `docs/web-design.md`, which supersedes
  `ARCHITECTURE.md`'s earlier plan, updated here). Builds for
  `wasm32-unknown-unknown` (328 KiB). `web/` is a no-bundler demo
  (`index.html`, `tpt-web.js`, `tpt-decoder.js`, `demo.tpt`).
  Verified: 49 native tests; `web/check.mjs` runs the built wasm under Node
  (exact 192 000-sample / 60-frame decode, 300 hostile files with zero traps);
  `web/mock-browser.mjs` runs the playback JS against *fake* WebAudio/canvas.
  **Real browser audio/video output was not verified when this was written
  (no browser was available) and is verified by the next entry.**
  Benchmarks (`cargo bench -p tpt-web`, `web/bench.mjs`) and their
  measured, noise-caveated numbers are in `docs/performance.md`; memory limits
  are in `docs/security.md`. CI's `wasm` job now builds the module and runs
  the Node scripts. Also fixed stale "planned" statements in `README.md` and
  `ARCHITECTURE.md`. Per-crate README, CHANGELOG, keywords and categories per
  CONTRIBUTING §9.4.
- **`tpt-media-io`, `tpt-media-pipeline` and `tpt-media-probe` could not
  actually be built without `std`** (2026-10-01, found by auditing every
  crate's `no_std` claim after finding the same defect in
  `tpt-media-container`). All three carry a `std` feature, a
  `#![cfg_attr(not(feature = "std"), no_std)]` attribute, a README claiming
  `no_std` with a CI `wasm` job backing it — and all three *also* pinned their
  own dependencies with `features = ["std"]` in `[dependencies]`. That
  unconditionally enables `std` across the dependency graph, so
  `--no-default-features` on the crate under test could never turn `std` off:
  **CI's own wasm check was not testing what it claimed to**, and could not
  have failed on a genuine `std` leak. The build still compiled because the
  leaked `std` was there, so the defect was invisible to any check that did
  not inspect the resolved graph. Dependencies now use
  `default-features = false`, matching `tpt-media-codec`/`-container`, with
  `std` still forwarded through each crate's own `std` feature. Verified by
  re-running the exact CI command: it now passes with `std` genuinely off,
  and the default builds and tests are unchanged.
- **`tpt-container` was why three of those crates still could not drop `std`,
  not just itself** (2026-10-01, found by tracing the resolved graph instead
  of stopping at the first fix that made the build pass). It is std-only by
  design and its README says so — but it pinned all four of its dependencies
  with `features = ["std"]`, forcing `std` into every graph that depends on it,
  `tpt-media-probe` included. Fixing the three leaf crates alone still left
  `std` resolved (confirmed with `cargo tree -e features`); only this manifest
  cleared it. Its own build is unaffected: it is std-only now because it
  *declares* that, rather than because it leaks it into its dependents.
- **The `no_std` CI gate was never capable of failing** (same audit; the part
  that matters). Injecting a real `std::` use into a
  `#![cfg(not(feature = "std"))]` item in `tpt-media-probe` and re-running the
  exact `wasm` job command now **fails** (exit 101), and passes again once
  removed — so the gate tests what it claims to. Before these fixes it would
  have passed either way, because the manifests force-enabled `std`: a genuine
  leak could not have been caught by the check written to catch it. The codec
  crates, `tpt-cli` and `tpt-web` still pin `features = ["std"]`; that is
  correct for them (std-only, or wasm-with-std by design) and left alone.
- **§38 unit-test sweep over `tpt-media-io`'s `sink.rs`, which had no tests
  at all** (2026-10-01): the crate's README listed "source/sink contracts"
  among its covered areas, and the source half had 5 tests — the sink half had
  **zero**, while containing the crate's only non-obvious logic: the 64 KiB
  auto-flush threshold that keeps a long stream's memory flat. 12 tests now
  cover it (32 → 44 crate-wide), pinning that crossing the threshold flushes
  *without* `finish` and on a frame boundary, that a video item is rejected
  with an error naming the PCM/video mismatch and leaves the sink usable, and
  that a failing writer becomes an `IoError` carrying the cause rather than a
  panic. The README's "32 unit tests … source/sink contracts" is corrected to
  44 and to say what the sink tests actually pin. This completes the §38
  sweep: every foundation crate has now been audited.
- **`tpt-media-container` claimed `no_std` in three places and was not
  `no_std`** (2026-10-01, found while auditing crates against §38's unit-test
  gate). The README's "At a glance" table said `no_std: Yes — builds for
  wasm32-unknown-unknown (CI job wasm)`, and the manifest listed `no-std` in
  both `keywords` and `categories` — but `src/lib.rs` had **no `#![no_std]`
  attribute at all**, and the manifest pinned its dependencies with
  `features = ["std"]` instead of `default-features = false`. So
  `--no-default-features` could not turn `std` off, and a build with it
  "passed" while still using `std` — the claim was untested rather than
  false, which is the harder failure to notice. The crate was also simply
  **absent from CI's `wasm` job**, so nothing had ever compiled it for
  `wasm32` at all. Fixed to match the pattern its sibling
  `tpt-media-codec` already uses (`#![cfg_attr(not(feature = "std"), no_std)]`
  + `#![forbid(unsafe_code)]` + `extern crate alloc`), and added to the `wasm`
  job's list so the claim is now enforced instead of asserted.
- **§38 unit-test sweep over `tpt-media-container`'s public API** (2026-10-01):
  the crate had **one** test, and it could not fail — it constructed a `Chunk`
  and read back the two fields it had just assigned. 16 tests now (1 → 16),
  driven through an in-crate `Fake` that actually implements `Container`, so
  the trait's *contract* is exercised and not just its syntax: write/read
  round trip, end of stream (`Truncated`, never a panic or a silent `Ok`),
  empty container and zero-length body/payload, `seek_to_keyframe` at
  `i64::MIN`/`i64::MAX`, `stream`/`tracks` on a shared borrow, and the derives
  (`Clone` is a deep copy; `Eq` compares both fields; every `ContainerError`
  variant is nameable with its payload).
- **Two release-gate checklist items in `todo.md` §53 were stale, not
  pending** (2026-10-01): `WASM decoder` and `Vault prototype` were both still
  `[ ]` — last touched 2026-09-07, before either existed. The WASM decoder is
  `apps/tpt-web` (§45, 49 native tests + 11 Node checks + 29 real-browser
  checks) and Vault is `codecs/vault` v0.1 (§33, in `tpt-cli`, corpus
  measured). Both verified against the built artifacts before ticking, not
  against the claim. A neighbouring item, `stable container specification`,
  is annotated rather than ticked, and the annotation is the point: the spec
  exists and is field-by-field, but *stability* is a pre-1.0 commitment that
  CONTRIBUTING §9-§11 explicitly does not make yet.
- **`tpt-web`'s documented extension point is unreachable from a browser**
  (2026-10-01, found by asking who can actually reach it). `DecoderTable` is
  billed as "the extension point" for Vault and Mote, and `docs/web-design.md`
  §8 is titled "Extension point for Vault and Mote" — but `tpt_web_open` builds
  its player with `Player::new(limits)`, which is `DecoderTable::native()`
  exactly, and none of the 18 ABI exports can accept a table. A
  `DecoderBuilder` is a Rust `fn` pointer, so it could not cross the wasm
  boundary in any case. **No amount of `register` makes a codec playable in a
  browser**; the only route is for it to become a dependency of `tpt-web` and
  get one line in `DecoderTable::native()`. That is why Vault and Mote are
  reachable by the CLI and not by the web player, and it was stated nowhere, so
  §8 read as though browser registration were available. Now recorded in
  `docs/web-design.md` §8, the crate README, and on `tpt_web_open` itself. No
  behaviour changed — an ABI function taking a table would mean shipping
  function pointers across the boundary, which §4's "no pointer flows from JS
  into Rust" rule rules out; per-codec feature flags or per-codec `open_*`
  entry points are the alternatives, neither built nor claimed.
- **Five stale "not wired into the CLI" claims, left behind by the commit that
  wired them in** (2026-10-01): `b6cf60d` made `--codec vault` and
  `--codec mote` drive `encode`, `decode`, `transcode`, `benchmark` and
  `video-corpus`, and updated `todo.md`, the two codec READMEs and the
  `tpt-cli` changelog — but not `docs/codec-design.md` or the root
  `CHANGELOG.md`, which still told a reader the opposite. Fixed: the Mote and
  Vault sections of `docs/codec-design.md` said "**Not wired into
  `tpt encode`/`decode`** ... registered in the probe registry only; a y4m
  bridge would touch five command paths", and Vault's added "Its quality has
  not been measured over the §36 video corpus" — all untrue, and Vault's
  corpus numbers have been measured and recorded in `todo.md` §33. Both now
  say what is actually true: the CLI drives them, and what genuinely is *not*
  wired is the `tpt-web` player, whose `DecoderTable` still has no Vault or
  Mote entry. `docs/cli-design.md` also still described `video-corpus` as
  measuring `thrum` alone. Re-verified against the built binary rather than
  the source: `list-codecs` lists all three, and a 64x48 `C420jpeg` y4m
  fixture encodes and decodes byte-exactly through both `--codec vault`
  (3 632 B) and `--codec mote` (1 806 B), 27 725 B in and out.
- **Two U+FFFD replacement characters in this file** (2026-10-01), in the Vault
  entry: an em dash and a section sign that had been written as mojibake, so
  `todo.md` §33 was cited as `todo.md` <FFFD>33. Both repaired. Found by
  scanning the repository's markdown for U+FFFD, not by reading the file —
  they render as a blank-ish glyph, and the paragraph around them reads
  fluently enough to pass over.
- **`tpt-web` now runs in a real browser** (2026-10-01, todo.md §45): the
  last three open §45 items — browser audio output, browser video output and
  the browser demo — were blocked on exactly one thing, *no browser existed in
  the environment where the player was written*. One does now, so
  `web/browser-check.mjs` + `web/serve.mjs` (zero-dependency, like the rest of
  `web/`) serve the folder over http and drive the **unmodified** player in
  headless Chrome/Edge: a real `AudioContext` observed by subclassing it (so
  every call still reaches the genuine object), a real 2D canvas read back
  through the browser's own `getImageData`, real `requestAnimationFrame`, the
  real wasm. 29 checks, all passing, repeated 3x at ~8.8 s: the context is
  created and released (`state` back to `closed`, `baseLatency` 0.01 s), 375
  real `AudioBufferSourceNode`s are handed 4.0000 s of non-silent mono 48 kHz
  PCM scheduled back to back against the real clock, the canvas holds a real
  decoded picture (2834 distinct colours, 15 360/15 360 opaque, 0 frames
  dropped, 0 decode errors), and the shipped `index.html` — loaded same-origin
  in an iframe with nothing patched, its own button clicked — reaches end of
  stream and presents 60 pictures. CI's `wasm` job now runs it too.
  **What it still does not establish: that a human has heard anything.** It
  shows the browser accepted the audio and its clock advanced, which is
  correctness, not audibility. And the check is built not to fake that gap: on
  a machine with no audio output device the clock cannot move, so the
  playback-dependent checks print `SKIP` ("not verifiable here") rather than
  passing, a missing browser exits non-zero instead of skipping, and the page
  POSTs its report rather than being raced against `--dump-dom`.
  `mock-browser.mjs` is kept alongside it — the fake run is the CI-portable
  one, the real one answers a different question.
- **`browser-check.html` had never run in any browser, because it was a
  syntax error** (2026-10-01). Its `if (!played) ... else {` block was never
  closed, so the inline module threw `SyntaxError: Unexpected end of input`
  before executing one line; the page sat at "running…" and the driver
  reported a bare 120 s timeout with no output at all. Found by loading the
  page in Chrome and reading its console — a file that no linter, no parser
  and no test had ever touched. The page is also now guarded against the
  tempting CI misreading: "context released at end of stream" is asserted only
  once playback has actually run, since a runner with no audio device never
  reaches that point and would otherwise have reported a false failure for
  the one machine the `SKIP` branch exists to accommodate.

- **Mote implemented — `tpt-video-mote` (`codecs/mote`, todo.md §34)**
  (2026-09-30): the extreme edge/IoT video codec, now a workspace member.
  8×8 patch dictionary VQ (up to 4096 entries, 12-bit index) over `Gray8` /
  `Yuv420` up to 480p (multiples of 16, long edge ≤ 848, short edge ≤ 480);
  key frames plus delta frames that code only changed patches
  (conditional replenishment, Rice-coded skip runs; an unchanged frame is
  an 11-byte heartbeat); dictionary trained offline (deterministic seeded
  integer Lloyd) and provisioned out of band, or sent once in-band and
  identified by a 16-bit id in every frame packet. Versioned bitstream
  (`VERSION = 1`) with a CRC-32 trailer, `CODEC_ID = MOTE`,
  `DynEncoder`/`DynDecoder` factories, `ResourceLimits` honoured, a provable
  packet-size bound, and a decoder that cannot panic or read out of bounds on
  corrupt packets (bit-flip sweeps with repaired CRCs, random bodies behind
  valid headers, dictionary packets that over-claim). VQ search reuses
  `tpt_dsp_core::nearest_vector_accelerated` (§8) rather than a private copy.
  55 tests, a criterion bench, and a counting-allocator memory test.
  **The bandwidth target is benchmark-defined, not a guarantee**:
  on the bench's fixed-camera scene a 320×240 mono stream with the
  4096-entry dictionary measures 46 B per delta frame (0.37 kbps at 1 fps)
  and a 1 809 B key frame, while a key-frame-only 640×480 stream at 1 fps
  is 57.7 kbps — over the old 50 kbps design figure — so sub-50 kbps at 480p
  rests entirely on temporal skip and a static scene. Also: the old
  memory-budget row for Mote (16 KiB scratch / 64 KiB total, written before
  any code existed) could not survive contact with a 256 KiB dictionary and
  was replaced by measured figures. Every todo.md §34 item is checked; the
  temporal-prediction item records that conditional replenishment was built
  and measured while motion-compensated candidates and index-stream context
  coding were not investigated. Design and the temporal-prediction investigation: `docs/codec-design.md`;
  measurements: `docs/performance.md`; sources: `docs/provenance.md` (rows
  are `pending` patent review, not signed off). Per-crate
  [README](codecs/mote/README.md) and [CHANGELOG](codecs/mote/CHANGELOG.md).
- Mote is registered in `tpt-cli`'s `probe` registry (so a `.tpt` with a
  `MOTE` track reports decoder/encoder support) but is **not** driven by
  `tpt encode`/`decode`/`benchmark`: that needs `VideoCodec` threaded through
  five command paths, left out to avoid colliding with concurrent CLI work.
  **Superseded 2026-10-01:** that threading is done — `--codec mote` now
  drives `encode`, `decode`, `transcode`, `benchmark` and `video-corpus` (see
  the `tpt-cli` entry below).
- **The y4m bridge could not parse any YUV4MPEG2 file** (2026-09-30): the
  header parser consumes `C` as the *tag* and stores only what follows
  (`"420jpeg"`), but the colorspace match compared the *full tag*
  (`"C420jpeg"`) — so no colorspace ever matched and every `.y4m` input
  was rejected as unsupported. Found by the three `y4m::tests::*` unit
  tests, which had never passed since the bridge landed in `22e365a`.
  Fixed to match the value, with the `C` re-added in the error message so
  it still names the tag the user wrote.
- `make_decodable_tpt()` in `tpt-cli`'s test module: a fixture built by
  running `tpt encode` and capturing the injected writer, because
  `make_tpt()`'s hand-written packets are structurally valid but are not
  real Cinder bitstreams — any test needing decoding to actually succeed
  has to build its fixture through the encoder.
- **Per-crate README, CHANGELOG, keywords and categories** (2026-09-30):
  all sixteen workspace members now carry their own `README.md`,
  `CHANGELOG.md`, and crate-specific `keywords`/`categories` in their
  manifests. Before this, **zero** of sixteen crates had a README or a
  changelog, and all sixteen inherited one identical keyword/category set
  from `[workspace.package]` — which made every crate in the workspace look
  the same in crates.io search, and asserted `no-std` for the std-only ones
  (`tpt-container`, Cinder, Aura, both research crates). Each README is
  user-facing (what it is, real usage, feature flags, API surface, testing,
  known limitations) and links to `docs/` rather than duplicating it; each
  codec's carries the measured performance table from `docs/performance.md`
  and the profiling recipe that document previously claimed existed but did
  not. Policy is CONTRIBUTING §9.4: the root changelog stays canonical, the
  per-crate files are filtered views of it. Two packaging blockers in
  `tpt-media-sys` were fixed along the way — see *Fixed*.
- A new CI `metadata` job enforces this (2026-09-30): every member must have a
  `README.md` and `CHANGELOG.md`, declare at most five keywords, declare only
  valid crates.io category slugs, and not duplicate another crate's keyword
  set. The same script resolves every relative markdown link in the
  repository — 189 of them — because a broken cross-reference between a
  crate README and a design document is invisible until someone clicks it.
  The script is `ci/check-metadata.py`, runnable locally.
- The obligation is now recorded per planned crate: `todo.md` §33 (Vault),
  §34 (Mote) and §45 (`tpt-web`) each carry an explicit
  "per-crate documentation" item that must land in the same change that adds
  the crate as a member. The CI job fails until it happens, so the four
  unimplemented crates cannot silently ship undocumented.

### Fixed

- **Vault's B frames coded no residual at all, and priced candidates the
  decoder never renders** (2026-10-07, found by a per-frame PSNR probe two
  days after the B-frames landed - not by the display-order tests that
  shipped with them). `write_leaf`/`read_leaf`'s B-frame branch returned
  after the skip bit and the four motion vectors, before the shared
  coefficient tail: every B frame on the wire was prediction-only, while
  the encoder's RDO reconstructed candidates with quantised residuals and
  priced them as if the coefficients cost nothing. The candidate set was
  the I/P one (intra, single-reference inter, predicted-vector skip)
  against a wire whose non-skip leaves the decoder renders as a
  past/future average and whose skip decodes as a zero-vector future
  prediction. Nothing propagated - B reconstructions never enter the
  reference store, so the I/P chain stayed drift-free and every
  display-order and schedule test passed; only the B frames' own quality
  was silently 7-12 dB below what the encoder believed it had bought.
  Fixed within v2 (nothing released ever emitted the other syntax, so the
  version keeps its number): B leaves fall through to the coefficient
  tail like every other non-skip leaf, a hostile B vector now gets the
  same `MV_LIMIT` range check the I/P path always had, and the encoder
  prices the zero-vector skip plus the searched bi pair and its two
  single-vector special cases - predictions identical to what the decoder
  renders, pinned by checksum equality. `codecs/vault/tests/b_frames.rs`
  pins decoded B frames within 3 dB of the same clip coded without B
  frames (observed worst gap under 1 dB; pre-fix 7-12 dB).

- **`tpt-media-research`'s RD harness reported `NaN` bits-per-sample for an
  empty volume, and `codebook_mse` returned `NaN` for an empty training set**
  (2026-10-01, found while auditing that crate's public API against section 38's
  unit-test gate - the last unswept crate with no `tests/` directory at all).
  Both were `0/0`, and both are worse than a bad number. `rd_point`/`rd_curve`
  divide by `signal.data().len()`, which an empty `SignalBuffer` - which is
  constructible from outside the crate - makes zero; the result broke the
  crate's own stated validity contract ("every reported value is finite"), and
  because `partial_cmp` on `NaN` is `None`, the sort in `rd_curve` and the
  dedup in `interpolated_distortion` both silently degrade to treating every
  comparison as equal, corrupting the whole curve rather than one point.
  `codebook_mse` asserts `entries > 0` but never `n > 0`, and since it exists
  to *compare* codebooks, a `NaN` there orders as "less than" any real
  codebook - quietly picking a winner. Both now report 0, and `dim == 0` is
  rejected explicitly instead of dividing by it.
- **§38 unit-test sweep over `tpt-media-representation`'s public API**
  (2026-10-01): the crate had 37 public items, 10 tests, all inline, and **no
  `tests/` directory at all**; `representation`, `quantizer` and `rate` — three
  of its six modules — had no tests whatsoever. 26 new inline tests (10 → 36)
  plus `tests/contracts.rs`, the crate's first integration test (8 tests),
  across the §38 categories that had no coverage: empty input, maximum values,
  overflow, edge cases, malformed input, and the documented panics. The three
  trait-only modules still have no *inline* tests by design —
  `Representation`, `Quantizer`, `RateEstimator` and `CoeffMagnitudes` have no
  implementor inside the crate, so their contracts are only observable from
  outside it, and `tests/contracts.rs` implements all four for real and drives
  the full analyse → quantise → charge → dequantise → synthesise → measure
  loop. That pins the `Quantized`-carries-everything contract (the quantiser
  reads `step` out of `Quantized`, not out of itself), the `RateEstimator`
  "no free decoder-side information" rule, a monotone rate/distortion frontier
  over a six-point sweep, and §39 determinism. Three new tests fail against
  the pre-change code, verified by restoring each old body and re-running.
- **`tpt-media-representation`'s `SignalDims::frame_len`/`samples` answered
  differently depending on the build profile** (2026-10-01, found while
  auditing that crate's public API against §38's unit-test gate). Both computed
  `width * height [* frames]` with a plain `*`, which is a **panic in debug and
  a silent wrap in release**: one `SignalDims` described two different volumes
  depending on the profile, and the wrapped value is what every caller derives
  from — the `Vec` allocations, the `frame()` slices, the sample indices.
  `SignalDims { width: usize::MAX, height: 2, frames: 1 }` wrapped to
  18 446 744 073 709 551 614 samples. Both products are now `checked_mul` with
  a stated message, so the profiles agree. The same class of defect as
  `tpt-media-pipeline`'s `Source::from_range` above.
- **`SignalBuffer::sample_bilinear_toroidal` divided by zero on a
  zero-extent volume** (2026-10-01, same audit). With `width == 0`,
  `x.rem_euclid(0.0)` is `NaN`, `NaN as usize` is `0`, and the following
  `% width` panics with `attempt to calculate the remainder with a divisor of
  zero` — true, but it names the arithmetic rather than the cause. A
  zero-extent volume is legally constructible from outside the crate
  (`SignalBuffer::zeroed` accepts it), so the panic was reachable through the
  public API. The precondition is now stated, documented and checked.
- **`tpt-media-pipeline`'s `Source::from_range` answered differently
  depending on the build profile** (2026-09-30, found while auditing the
  crate's public API against §38's unit-test gate). Converting an exclusive
  end bound to an inclusive one formed `hi - 1`, which underflows at the ends
  of the type: `from_range(..0u8)` was a **panic in debug and a silent wrap to
  255 in release** — 256 items where the range contains none. Separately,
  every empty range yielded exactly one item (the first value was pushed
  before the bounds were compared), so `from_range(1..1)` produced `[1]`, and
  an unbounded end meant "the high value is zero", so `from_range(0..)`
  produced `[0]`. Bound arithmetic no longer forms `hi - 1` or `lo + 1`
  before the emptiness test, empty ranges yield nothing, and an unbounded end
  panics with a stated reason — the crate's uniform "panic rather than answer
  wrongly" rule. `Bounded`'s doc also claimed a `BufferOverflow` past `cap`
  that a linear `Chain` cannot reach; the invariant is now documented and
  pinned.
- **§38 unit-test sweep over `tpt-media-pipeline`'s public API**
  (2026-09-30): 20 new tests (21 → 41) for the categories that had no
  coverage — empty input, maximum values, overflow, edge cases, every
  `MediaFormat` variant, multi-candidate format negotiation, and the
  `ThreadedExecutor`'s worker-panic mapping. Five fail against the old
  `from_range`, verified by restoring the old body and re-running.
- `tpt probe` claimed no codec was available — `encoder=false, decoder=false` —
  for the codecs the same binary encodes and decodes one subcommand later,
  because it used `tpt_media_probe::probe`, which answers from an empty
  registry. That default is right for a library (it cannot know what its caller
  linked in) and wrong for a CLI (it carries exactly four native codecs), so
  `probe`/`inspect` now resolve against a registry of all four and the flags
  come from each codec's own declared capabilities. A `THRM` file now reports
  `native=true, decoder=true, encoder=true, lossless=false, lossy=true,
  rt=true`; `probe --json`'s `encoder_available`/`decoder_available` change
  with it. Pinned by two new tests.
- `tpt-media-sys` could not be packaged at all (found 2026-09-30 while
  auditing crate metadata). Two independent causes, both reproduced:
  `readme.workspace = true` resolves relative to the *member* directory, so
  it pointed at a `README.md` that did not exist — `error: readme
  'README.md' does not appear to exist`; and the five path dependencies
  carried no `version` — `error: all dependencies must have a version
  requirement specified when packaging`. Fixed by declaring `readme`
  explicitly (as every other member now does) and adding `version =
  "0.1.0"` to each dependency.
- `docs/performance.md` stated "A profiling recipe for each codec is recorded
  in its README" while no codec had a README at all. The statement is now
  true, and it was reworded to name the benchmark harness rather than imply
  the recipes live anywhere they did not.
- Four broken relative markdown links, three of them introduced when the
  crate table was converted to links: the *(planned)* rows for Vault, Mote
  and `tpt-web` pointed at directories that do not exist, so they are plain
  code spans again. The fourth was pre-existing —
  `docs/pipeline-design.md` linked `crates/...` instead of `../crates/...`.
  `ci/check-metadata.py` now resolves all of them.
- `docs/architecture.md` described `ffi/tpt-media-sys` as "planned ... not yet
  on disk"; it shipped on 2026-09-29 and is the workspace's only `unsafe`
  home today.
- The root `README.md` ecosystem note listed "the video codecs, the WASM
  player, and the FFI bridge" as planned. The FFI is implemented and Thrum
  with it; only Vault, Mote and `tpt-web` remain.
- Two `tpt-cli` tests asserted the pre-y4m policy and now assert the real
  behaviour. `decode_rejects_wrong_container_flag` expected the old
  "writes only wav output" wording — decoding now writes WAV for an audio
  track and y4m for a video one, so the requirement depends on the track.
  `video_codecs_resolve_but_are_rejected_by_audio_commands` used
  `file_ctx(&[])` with a nonexistent `in.raw`, so it failed on the I/O error
  before reaching the codec dispatch at all; it now uses a real decodable
  fixture. The *intent* of both — that a video codec on an audio path is
  explained rather than reported as unregistered — is unchanged and still
  asserted.
- `cargo fmt --all --check` failed on ten hunks in `apps/tpt-cli/src/
  commands.rs` and `y4m.rs`, left by `22e365a`; CI's `check` job would have
  failed on it. Reformatted.
- `tpt-cli`'s module docs listed the command surface without `video-corpus`
  (which `USAGE`, the crate README, `docs/cli-design.md` and the
  implementation all list) and attributed `--json` to `probe` and `benchmark`
  only.
- `apps/tpt-cli/CHANGELOG.md` had two `### Fixed` blocks inside `[Unreleased]`,
  so one release's entries rendered as two separate lists; they are merged.

### Removed

- Dead code in `tpt-cli`, all three of which were `-D warnings` failures in
  CI: the audio-only `demux_and_decode` (superseded by
  `demux_and_decode_any`, which also handles video tracks), the unused
  `y4m::plane_count`, and an unreachable `Some(other)` arm over the
  single-variant `VideoCodec` enum.

### Changed

- **Documentation truth pass on the CLI's video support** (2026-09-30): the
  y4m bridge landed in `22e365a`, but every document still described the CLI
  as audio-only. Corrected in `tpt-cli`'s crate docs, `USAGE`, `args.rs`,
  `commands.rs`, `apps/tpt-cli/README.md`, `codecs/thrum/README.md`,
  `docs/cli-design.md`, `docs/codec-design.md`, and `todo.md` §32 — whose
  "CLI encode/decode" item is now `[x]` because the work is done. The claim
  "the encode family is audio-only" was wrong in both directions: it hid the
  video path that exists and described `--codec thrum` as rejected when it is
  accepted whenever the media matches.
- **Renamed four native codecs to avoid collisions with existing
  products** (2026-09-28): `Vox` → `Reed` (collided with the Dialogic/OKI
  ADPCM ".vox" telephony codec), `Pulse` → `Cinder` (collided with
  PulseAudio and "pulse-code modulation"), `Livelink` → `Thrum` (collided
  with Unreal Engine's Live Link), `Whisper` → `Mote` (collided with
  OpenAI Whisper — especially confusing since this codec is video, not
  audio). Crate directories/packages, `CodecId` 4CCs (`VOX `→`REED`,
  `PULS`→`CIND`, `LIVE`→`THRM`, `WHSP`→`MOTE`), types, CLI codec names,
  and docs were all updated project-wide; `Aura` and `Vault` were
  unaffected. See `todo.md` §57 for the full record.

### Added

- **Vault B-frames: bi-prediction with display-order reordering, wire
  v2** (2026-10-05, §33): `VaultConfig::b_frames` (0..=4, default 0)
  buffers a lookahead group — anchor, `b_frames` bi-predictive frames,
  future reference — coded in decode order (anchor, future, B's) so
  each B's references precede its packet. B leaves carry four
  Rice-coded motion-vector deltas against the fixed
  past/future reference pair (no per-leaf reference index); the
  prediction is the plain average of the two motion-compensated
  references, and B reconstructions never enter the reference store.
  The v2 wire (2-bit frame type + 16-bit per-frame poc, stage 1 of
  this change) lets the decoder hold B frames and emit in *display*
  order; the encoder's flush upgrades trailing B's to predictive
  frames, and the CLI video encoder flushes the lookahead before
  finalising the stream, so no frames are lost. Keyframes never land
  between an anchor and its B's (the degenerate group codes the
  buffered frames as P's — the decoder's intra reference reset would
  otherwise strand them). Display order, single-emission, and
  end-to-end round trips are pinned by two new property tests; the
  hostile-header test covers the v2 layout including the reserved
  frame type. Still open in the item: weighted prediction,
  long-term references, reference reordering, and combine modes
  beyond averaging.

- **B-frames design written** (2026-10-05, §33's remaining item): the
  full v0.2 plan now lives in `docs/codec-design.md` ("Vault B-frames
  — design for v0.2") — the 2-bit frame type and explicit picture
  order count for the wire, the encoder lookahead and
  excluded-from-references bookkeeping, the decoder's bounded reorder
  buffer and end-of-stream drain, the `tpt-web` presentation-delay
  caveat, and the tests to write before coding. Not started; rate
  control (landed above) composes with it unchanged.

- **Vault gained frame-level rate control — `--bitrate` now means
  something on the video path** (2026-10-05, half of §33's
  rate-control item): `VaultConfig::target_bits_per_frame` drives a
  bounded per-frame `qp` search (at most three encodes per frame) with
  a budget bucket that lets the first intra frame spend 4× and banks
  what earlier frames saved, spending that credit to converge onto the
  target instead of stalling one quantiser step short of it. The frame
  header already carried a per-frame `qp`, so there is no wire change
  and the decoder is untouched. Deterministic (byte-identical
  re-encodes, pinned) and measured end-to-end through
  `tpt encode --codec vault --bitrate`: a 60 kbit/s target decodes to
  61.7 kbit/s measured, 180 kbit/s → 143 (the bounded search stops
  short on the textured first I-frame), 24 kbit/s → 28.6 (the
  `qp = MAX_QP` floor is the codec's lowest rate). Encode cost when
  enabled: up to 3× on adapting frames. Still open in the item:
  scene-cut detection and per-block `qp`.

- **Vault gained coefficient-level RDOQ as a measured, default-off
  encoder knob** (2026-10-05, half of §33's RDOQ item): an exact-rate
  zeroing pass — for every non-zero level of a tile, from the block's
  last down, drop it when the Rice-coded bits it costs (own code + run
  merge + the shortened span when it is the last) buy more distortion
  than they save under the leaf RDO's own `lambda`; by Parseval on the
  orthonormal scaling the step cancels out of the decision. Unit tests
  pin it firing (cheap level dropped at a fine step, expensive level
  kept, whole blocks able to go all-zero). The measured verdict at the
  codec's operating points: **byte-identical output at `qp` 30 and 12,
  0-6% time cost** — the dead-zone quantiser leaves no marginal levels
  on the available content — so `VaultConfig::rdoq` defaults to
  `false`, the sweep is restricted to tiles ≤16×16, and the knob
  waits for a `lambda` tuned on real content. A re-run of the full
  corpus with the knob off confirmed the pre-RDOQ numbers unchanged.

- **`tpt-media-sys`, the stable C ABI — §46's first slice** (2026-09-29):
  the codec abstraction (packet, frame, encoder, decoder) exposed to C
  as opaque, exclusively-owned handles under `ffi/tpt-media-sys`,
  covering all ten §46 mechanics items: `extern "C"` functions returning
  frozen `enum tpt_error` codes; `tpt_abi_version()` with a stated
  stability rule; explicit per-handle free functions (freeing NULL is a
  no-op; packet payloads are copied in); an abstract `tpt_media_config`
  (sample rate / channels / quality 0–100) that maps to per-codec
  parameters exactly as the CLI's `--quality` does, so a C caller never
  sees per-codec structs; thread-safety and per-function `# Safety` /
  `// SAFETY:` documentation (the crate is the workspace's one
  sanctioned `unsafe` home); every export panic-guarded (`TPT_ERR_PANIC`
  instead of unwinding into C); a hand-written `include/tpt_media.h`
  kept in sync with the ABI by a two-directional test; and a real C
  integration test (`tests/integration.c` sees only the header) driving
  a bit-exact Aura lossless round trip through `cargo test`. Built as
  both `rlib` and `staticlib` (`tpt_media_sys.lib`). Supporting
  changes: the six `*Dyn{Encoder,Decoder}` wrappers gained public
  `new()` constructors (the FFI needs direct boxed construction; the
  factories stay for config-from-codec-private), and the ABI owns PTS
  association at the boundary because the type-erased `DynFrame`
  carries no timestamps. `Context`/`Container`/`Demuxer`/`Muxer`
  handles are the recorded v0.2 surface.
- **§1 Core Design Principles audited and enforced** (2026-09-29): the
  one remaining un-audited block of todo.md now carries the same
  evidence-first annotations as everything else. Two of its claims
  became enforced facts rather than intentions in this change:
  - every library crate (the six no_std foundation crates, all four
    codecs, `tpt-container`, and the two research crates — thirteen in
    total) now carries `#![forbid(unsafe_code)]`, so "safe Rust
    wherever practical" and "unsafe isolated" are compiler-checked;
    the workspace's only remaining `unsafe` is the CLI's documented
    `memprobe` allocator harness plus the per-codec test allocators;
  - "WASM-compatible core components" is now a tested fact: all six
    no_std foundation crates compile for `wasm32-unknown-unknown` with
    std off, and a new CI `wasm` job enforces it.
- **§48/§49 provenance completion and §51 engineering policies**
  (2026-09-29):
  - `docs/provenance.md` — the two missing per-codec records added
    (Cinder, Aura), so all four implemented codecs now carry complete
    GREEN-classified algorithm tables with references, derivation
    notes, dated patent reviews, and explicit "deliberately *not*
    used" boundaries; the Per-Crate Records section now states that
    records live in that one document — the per-crate
    `PROVENANCE.md` files it previously promised do not exist and
    never did (CONTRIBUTING's matching stale references fixed too,
    and its real-time section now distinguishes the zero-allocation
    rule for *new* APIs from the tested bounded-allocation claims of
    current ones).
  - `CONTRIBUTING.md` §9–§11 (todo.md §51): the versioning strategy
    (lockstep workspace versions, `0.y`-is-breaking SemVer,
    wire formats versioned separately from crates), API stability
    expectations (no pre-1.0 guarantee, the research crates are
    explicitly experimental, deprecation-not-removal), the feature-flag
    convention (`std` is the only default feature, additive-only
    features, no_std-first for new crates), benchmark ownership
    (per-crate benches, `docs/performance.md` as the canonical record,
    benches deliberately out of CI, a failed budget is a failed
    build), and the MSRV policy (§9.3). The document now also states
    the project's actual contribution model up front: development is
    **issues only** — external code contributions (pull requests) are
    not accepted — and it is framed as maintainer-facing development
    policy rather than a contributor handbook (matching updates to the
    README's Contributing section).
  - `docs/codec-design.md` — field-by-field wire-format
    specifications for Cinder v1 and Aura v1 (matching the existing
    Reed/Thrum sections) plus a crate/`VERSION`/layout/integrity table
    covering all four codecs and the change discipline for future
    version bumps; closes todo.md §54's "native codecs have explicit
    versioned specifications".
- **Thrum, the low-latency video communication codec** (§32
  prototype, 2026-09-28): `tpt-video-thrum` is a real implementation
  of the I/P hybrid design — every item on §32's list is now `[x]`.
  Built for *latency and bounded work*, not compression efficiency:
  - **Pixel formats** `Gray8` and planar 4:2:0 `Yuv420` (the only coded
    formats; anything else is refused rather than converted). Frames must
    be non-zero multiples of 16, so the picture is exactly
    macroblock-aligned — no padding, no partial macroblocks.
  - **Frame structure** — 16×16 macroblocks (four 8×8 luma blocks plus
    one 8×8 U and one 8×8 V for 4:2:0) in raster order. One frame is one
    packet; `END_OF_FRAME` is set on every packet and no reordering
    buffer exists anywhere in the codec.
  - **Intra prediction** — four modes (DC / vertical / horizontal /
    diagonal) chosen per 8×8 luma block by SAD, chroma always DC.
    `intra_mode_available` gates modes on geometry and the decoder
    re-checks, so an unavailable mode in a hostile packet degrades to DC
    rather than reading out of bounds.
  - **Inter prediction** — exhaustive full-pixel integer motion
    estimation over `±search_range`, costed by SAD with an exact early
    exit; single reference frame, edge-clamped compensation, chroma
    vectors derived (not signalled) as the luma vector halved.
  - **Skip** — a macroblock whose mean absolute prediction error is
    below one quantiser step is signalled skipped (copy the reference at
    `(0, 0)`). The threshold is *rate-aware* rather than a fixed SAD
    constant: a constant disabled skip entirely at coarse `qp`, exactly
    where it is worth the most.
  - **Intra refresh** — 5% of a P-frame's macroblocks re-coded intra on a
    sliding window that provably sweeps the whole picture in
    `ceil(total / per_frame)` frames. The first implementation used a
    strided cursor that was only exhaustive when the stride was coprime
    with the macroblock count; with 48 macroblocks and 9 per frame the
    stride 6 shared a factor and the sweep silently covered 1/6 of the
    frame (`intra_refresh_sweeps_every_macroblock` now pins it).
  - **Transform / quantisation / entropy** — 8×8 separable DCT-II/III
    from `tpt-dsp-core`'s `dct_2d`/`idct_2d` (§3.1: transforms live in
    `tpt-dsp`); one flat dead-zone uniform quantiser driven by a 6-bit
    `qp` in every frame header via a fixed table (no `powf`, so the step
    is bit-identical on every target and `no_std`-clean); **real Rice
    coding** (`tpt-media-bitstream::rice`) as a run/level stream over the
    zigzag scan, with a per-block Rice parameter chosen by the encoder
    from `{0,1,2,3}`.
  - **Deblocking** — a 4×4-grid loop filter in the classic H.263 shape,
    applied to the whole reconstructed frame, with the *filtered* picture
    becoming the next reference. Encoder and decoder call the identical
    function, so it cannot cause drift.
  - **Wire format** — 16-bit version (rejected up front, §35's
    convention) and a 4-byte CRC-32 trailer verified before parsing.
    `ThrumConfig::max_packet_bits` derives the worst-case packet size
    from the level clamp and the per-code bit bound, so the encoder's
    single allocation provably cannot overflow.
  - **Packet-loss concealment** — `ThrumDecoder::conceal` repeats the
    last decoded frame (a freeze), the right concealment for a codec
    with no temporal hierarchy; mid-grey before the first frame.
  - **Tests** — 45 unit tests (transform/quantiser/entropy round trips,
    intra-mode availability, deblocking behaviour, I/P frame structure,
    refresh coverage, skip path, concealment, flush, version and CRC
    rejection, `NeedMoreInput` without a reference, config validation,
    codec-private round trip); 6 property tests (full-length round trips
    across four geometries and both formats, `qp` ordering quality,
    bit-exact flat frames, determinism, out-of-range header rejection,
    and the §38 corruption/truncation/bit-flip sweep); §42
    bounded-allocation test; §41 criterion bench.
  - **Two real defects the tests caught and fixed**, both recorded in
    `todo.md` §32: (a) the decoder walked macroblocks **column-major**
    while the encoder wrote them row-major, which desynchronised every
    macroblock after the first against both the bitstream and the intra
    prediction context — visible as ~13 dB PSNR on a smooth gradient,
    now ~45 dB; (b) the encoder retained its worst-case packet
    *capacity* (2.5 MB per 176×144 packet) instead of the few kilobytes
    written, found by the allocation test and fixed with
    `shrink_to_fit` — superseded the same day by the §42 work below:
    the buffer is now allocated once at construction and reused, so the
    steady-state path does not allocate it at all.
- **§36 video corpus, §37 video quality metrics, and §42 measured
  memory budgets** (2026-09-28):
  - `apps/tpt-cli/src/video.rs` — the §36 video corpus: eight
    deterministic seeded scenes (static, talking head, camera motion,
    high motion, animation, screen content, synthetic patterns,
    pathological frames), each generated closed-form from SplitMix64
    like the audio corpus, in both coded formats, with the same honesty
    note (the real-world names describe the *measurable structure* each
    regime contributes, not photographic content).
  - `apps/tpt-cli/src/quality.rs` — the §37 video metrics: `psnr_db`
    (`None` for bit-exact frames — a real corpus case, not a corner),
    `ssim` (8×8 luma windows), `video_bitrate_bits_per_second`,
    `bits_per_pixel`, and `VideoReport` accessors for bitrate, latency,
    and real-time fit at a target fps. Memory rides in the same report
    as a `MemoryReport` that is `None` unless the process installed the
    tracking allocator — an absent measurement and a measurement of
    zero are different facts.
  - `apps/tpt-cli/tests/video_quality.rs` — every corpus scene through
    real Thrum round trips in both coded formats: per-scene rate and
    quality pins (static scenes ride the skip path at near-zero rate
    and bit-exact output; incompressible scenes cost orders of
    magnitude more) plus a corpus-wide bitrate ordering that tracks
    inter-frame predictability.
  - `apps/tpt-cli/src/memprobe.rs` +
    `apps/tpt-cli/tests/memory_budget.rs` (§42 "allocation profiling"
    and "peak memory measurements") — the per-codec counting-allocator
    mechanism factored into one reusable `TrackingAlloc`/`MemoryProbe`
    harness that reports the high-water mark of *live* heap bytes (the
    number a memory budget is written in) and cumulative churn, with a
    saturating free path so a dealloc predating the window cannot wrap
    the counters. The dedicated `memory_budget` test binary — the only
    one that installs the allocator — measures every implemented codec's
    steady-state streaming working set against `docs/performance.md`'s
    budget table (now a measured table) and pins Thrum's working set
    length-independent (8× the clip, same ~149 KiB peak). The
    measurement reshaped two things: Reed's documented budget rose from
    a pre-measurement "< 8 KiB" (the measured 8 140 B peak filled 99%
    of it — a flake in waiting) to "< 16 KiB", and the Thrum encoder
    above stopped allocating its ~2.4 MB worst-case packet buffer per
    frame (allocated once, reused, exact-size copies emitted).
  - `tpt-video-thrum` is wired into the CLI's codec table
    (`--codec thrum` / `--codec LIVE` resolve, and `tpt list-codecs`
    advertises it), but the `encode`/`decode`/`transcode`/`benchmark`
    commands remain audio-only (WAV in/out) and reject it with an
    explicit "this is a video codec and these commands are audio-only"
    message rather than a misleading unknown-codec error. Measured
    numbers in `docs/performance.md`, design in `docs/codec-design.md`,
    algorithm sources in `docs/provenance.md`.
- **Documentation baseline audit** (§50/§53 gate item, 2026-09-28):
  every top-level document was checked against the code it describes and
  the stale claims fixed in place:
  - `README.md`: the crate table marked the unimplemented video codecs,
    `tpt-web`, and `tpt-media-sys` as *(planned)*, and gained the
    previously-omitted `tpt-media-representation`/`tpt-media-research`
    rows.
  - `ARCHITECTURE.md`: §6's codec-trait snippet rewritten to the real
    API (`CodecResult<Vec<Self::Frame>>`, `CodecError`,
    `DynEncoder`/`Registry` — the old snippet's `DecodeError` and
    single-frame returns never existed); §2's topology now lists the
    research crates and marks planned crates; §7 documents the actual
    `IDX `/`CRC1`/streaming behavior instead of scan-only seeking; §8
    describes the real backpressure model (`NodeEvent::NeedMore` +
    stall bound, not ring buffers); §11 now says "bounded allocation"
    (pinned by tests) instead of "zero allocation" and drops the
    real-time thrum claim; §10 marked WASM/FFI as design intent.
  - `docs/architecture.md`: removed the false "reed uses fixed-point
    i16/i32" claim (it is `f64` + `libm`) and the nonexistent
    `process_one()` pipeline API (it is `process()`/`Chain`/
    `Pipeline::pull`); the 32-bit CI note now reflects the
    `thumbv7em-none-eabi` build job.
  - `docs/codec-design.md`: format tables gained honest status columns
    (the code's `supported_formats` — S16 mono everywhere — is binding,
    not the aspirational matrix); the Real-Time section now states the
    actual per-codec claims (Cinder both sides, Reed decoder only, Aura
    offline); Determinism documents that *encoding* is fully
    deterministic today (§39 pins it); Bitstream Versioning describes
    the implemented fail-fast policy instead of a compatibility mode
    that deliberately doesn't exist; the codec internal-layout section
    matches reality (no per-crate README/PROVENANCE files).
  - `cargo doc --workspace --no-deps` is now **zero-warning**: 13
    broken intra-doc links fixed across 7 crates (private items
    un-linked, cross-crate types fully qualified).
  - New example `crates/tpt-media-io/examples/io_tour.rs` — a runnable
    tour of the byte-exact WAV and PPM round trips (closes §50's
    "examples" item; the root-level `examples/` directory stays empty
    by design, examples live in the crate they demonstrate).
  - Stale "exit code 3 = codec not implemented" CLI documentation
    removed: with Reed wired, every listed codec is implemented and no
    code path produces exit code 3 any more.
  - The per-area design docs written alongside their implementations
    (`container-design`, `pipeline-design`, `media-io-design`,
    `cli-design`, `security`, `provenance`, `performance`,
    `representation-research`) were verified accurate as-is.

- **Reed, the extremely low-bandwidth speech codec** (§31 prototype,
  2026-09-28): `tpt-audio-reed` is now a real implementation — a
  source-filter LPC vocoder at 8/16 kHz mono, 20 ms frames, in the
  LPC-10 tradition:
  - Order-2..=16 (default 10) autocorrelation LPC +
    Levinson-Durbin from `tpt-dsp-core`, transmitted as uniformly
    quantized LSP indices (`lpc_to_lsp`/`lsp_to_lpc`/`lsp_is_stable`);
    the config enforces `2^lsf_bits > lpc_order` so a valid
    strictly-increasing index set always exists, and the decoder
    re-enforces ordering before the stability check, so no bit stream —
    hostile or corrupted — can synthesize through an unstable filter.
  - Voicing + 10-bit pitch period from `tpt-dsp-core`'s normalized
    autocorrelation pitch estimator; unvoiced frames carry a 10-bit
    index into a fixed 1024-entry stochastic codebook (shape-gain VQ,
    the CELP fixed-codebook idea) that is never transmitted — both
    sides derive identical unit-RMS entries from a SplitMix64 stream.
  - Excitation gain (residual RMS) quantized in 1 dB steps over 62 dB,
    with index 0 as exact mute so digital silence round-trips to
    digital silence.
  - Wire format: 16-bit version (§35 convention, rejected up front),
    order/lsf-bit-depth/voicing header, gain + pitch/codebook fields,
    LSF indices, CRC-32 trailer (the Aura/Cinder family convention).
    Default parameter payload is a fixed 90 bits/20 ms = 4.5 kbps;
    16-byte packets = 6.4 kbps at the container level.
  - Packet-loss concealment: `ReedDecoder::conceal` repeats the last
    good envelope/voicing/pitch with gain decayed 6 dB per concealed
    frame, fading gaps to silence.
  - Tests: vowel/pitch/envelope round trips on synthetic voiced frames,
    silence-exactness, resource limits, jittered PTS, determinism,
    seeded-random property round trips, and the §38 deterministic
    corruption/truncation/bit-flip sweep; §42 bounded-allocation test
    (`tests/realtime_alloc.rs`); §41 criterion bench
    (`benches/codec_bench.rs`). Builds `no_std` for
    `thumbv7em-none-eabi` (float math via `libm`, matching
    `tpt-media-core`; it is *not* a fixed-point implementation).
  - Wired into `tpt-cli`: `encode`/`decode`/`transcode`/`benchmark`
    all accept `--codec reed` (8/16 kHz inputs; `--quality` maps to LSF
    bit depth), and the codec-private track data round-trips the
    config. Provenance records for every algorithm live in
    `docs/provenance.md`; design notes in `docs/codec-design.md`;
    measured numbers in `docs/performance.md`.
- `tpt-cli` §43 completion + §36/§37 quality harness (2026-09-14):
  - `tpt transcode <in.tpt> <out.tpt> --codec <target>` is live: decodes
    the input container's first audio track, re-encodes with the target
    codec, and muxes a fresh `.tpt`, carrying the source track's
    metadata tags (`--set-meta` overrides by key). `cinder→aura` output
    decodes byte-identically to the direct decode; `aura→cinder` is
    lossy-of-lossy as expected.
  - `tpt benchmark <in.wav> --codec <name> [--quality q] [--runs n]
    [--json]` is live: measures encode/decode wall time (median + min
    over `--runs` measured passes after one untimed warm-up), payload
    bitrate, bits/sample, real-time factors, and §37 quality (SNR +
    log-spectral distance) of the round trip; `bit_exact: true` replaces
    the unrepresentable `snr_db: inf` in JSON.
  - New `quality` module (§37 audio metrics): `snr_db`,
    `log_spectral_distance_db` (Hann-windowed FFT magnitudes via
    `tpt-dsp-core`), `rms_db`, `bitrate_bits_per_second`; all pure
    functions with metric-direction self-tests.
  - New `signals` module (§36 audio corpus): deterministic SplitMix64
    seeded generators — silence, sine, square, sweep, noise, click
    trains (transient-heavy), a mixed "programme" signal, and the
    pathological full-scale DC / full-scale alternating inputs.
  - Integration tests (`apps/tpt-cli/tests/quality.rs`) drive the whole
    corpus through real Cinder and Aura round trips: Aura is bit-exact on
    every signal including full-range noise; Cinder floors are pinned at
    measured values (see the Fixed-section note on the ≈3 dB ceiling).
- Criterion benchmark suite (§41, 2026-09-14): `codec_bench.rs` in
  `tpt-audio-cinder` and `tpt-audio-aura` (encode/decode at each codec's
  default operating point), `container_bench.rs` in `tpt-container`
  (200-packet mux/demux), `bitstream_bench.rs` in `tpt-media-bitstream`
  (bit I/O, Rice, range, Huffman), `pipeline_bench.rs` in
  `tpt-media-pipeline` (1000-packet source→map→sink drive). Results
  recorded in `docs/performance.md`; `cargo bench` reproduces the
  configuration (fixed in code).
- Seeded-random property tests (§38) + determinism pins (§39) +
  memory assertions (§42):
  - `tpt-media-bitstream/tests/property.rs` — randomized write/read
    round trips through `BitWriter`/`BitReader`, Rice, Huffman, and the
    range coder, plus byte-identical re-encode determinism.
  - `tpt-media-core/tests/property.rs` — rescale identity/monotonicity/
    bounded-round-trip, checked-vs-saturating duration agreement,
    overflow rejection, over seeded-random samples.
  - `containers/tpt-container/tests/property.rs` — random packet
    streams round-trip exactly (PTS/DTS/flags/payload), muxing is
    byte-identical across runs, and extreme/duplicate PTS values
    (up to `i64::MAX`) round-trip without panicking (§47).
  - `codecs/{cinder,aura}/tests/property.rs` — full-range random audio
    round trips (Aura bit-exact; Cinder bounded by its measured floors),
    encoder/decoder determinism at the packet-byte level, silence
    stays silent.
  - `codecs/cinder/tests/realtime_alloc.rs` (§42) — a counting global
    allocator asserts steady-state encode/decode allocations stay
    bounded (512 KiB over 31 blocks, ~25× headroom), guarding the
    real-time no-unbounded-allocation claim.
  - `crates/tpt-media-probe/tests/timestamps.rs` (§47) — `probe()`
    survives extreme, duplicated, descending, and mixed-sign PTS
    containers with finite, non-negative reports; one unsigned-envelope
    quirk documented rather than hidden.
- Documentation truth pass (§47/§39/§41): `docs/security.md` now shows
  the real `ResourceLimits` fields, describes the deterministic
  sweep-based fuzzing strategy actually in CI (replacing the
  not-built `cargo-fuzz` target list), records the completed audits,
  and adds a Malformed Timestamps section; `docs/performance.md` gains
  measured benchmark tables, the determinism/SIMD-differences section,
  and the reproducible-benchmark configuration; `SECURITY.md`'s audit
  checklist reflects the completed audits.

### Fixed

- `tpt-media-probe` did not compile for `wasm32-unknown-unknown` with
  std off despite carrying the no_std switch — `format!`/`ToString`
  came from the std prelude and the `alloc` imports were missing
  (found 2026-09-29 by building the foundation crates for wasm32 for
  the first time; every std build masked it). Fixed with the two
  missing imports, plus two latent clippy failures surfaced when
  check caches invalidated: a hand-rolled no-op waker in
  `tpt-media-io`'s tests replaced with `Waker::noop()` (stable since
  the declared MSRV) and a `map_or` → `is_none_or` in probe's
  timestamps test.
- **The declared MSRV was never true** (2026-09-29, found by testing it
  for the first time): `rust-version = "1.75"` was falsified by the
  committed lockfile — Cargo 1.75 cannot parse `clap_builder` 4.6's
  edition-2024 manifest, and 1.85 falls one minor short because
  `criterion` 0.8.2 declares rustc 1.86. Verified that 1.86 builds the
  whole workspace clean (`cargo check --workspace --locked
  --all-targets`); the field now says 1.86 and a new CI `msrv` job
  enforces it on exactly that toolchain. README's badge and
  CONTRIBUTING §4 updated to match; the bump policy for the future is
  CONTRIBUTING §9.3.
- `tpt-media-probe`'s human-readable audio track line printed the
  first→last PTS envelope **in raw ticks under a "{:.3} s" label** —
  no timebase conversion — so a 1-second 16 kHz file reported
  "15680.000 s" (found 2026-09-28 while smoke-testing the new reed CLI
  path; a `.tpt` file with any track timebase other than 1/1 was
  affected, e.g. every file `tpt encode` produces). The line now
  converts through the track timebase; `probe --json` was already
  correct (it emits ticks and timebase explicitly).
- `tpt-media-bitstream::range::RangeDecoder::decode` **desynchronized
  from the encoder for any symbol with `freq > 1`** (found 2026-09-14
  by the new §38 property tests, not by review): the decoder narrowed
  its range to `range/total` and never multiplied by the decoded
  symbol's frequency, while the encoder narrows to
  `(range/total)·freq` — so every non-uniform model diverged after the
  first symbol. Every pre-existing test and fuzz sweep used `freq = 1`
  (or only fed corrupted bytes), which is why this survived. The API is
  now the standard two-phase scheme: `decode_peek(total)` returns the
  cumulative-frequency bin without touching state, and
  `update(cum, freq, total)` narrows by the chosen symbol's interval,
  mirroring `RangeEncoder::encode` exactly; `decode(total)` remains for
  the uniform `freq = 1` case it implemented correctly. Regression
  tests: `decode_peek_update_round_trips_skewed_frequencies` (in-crate)
  and `range_coder_round_trips_random_symbols` (property).
- `tpt-cli`'s hand-rolled JSON writer produced **invalid JSON for
  probe/benchmark output** (found 2026-09-14 while adding `benchmark
  --json`; three related defects):
  `J::object()`/`J::arr()` emitted a separator *after* `field()` had
  already emitted one, producing `"key":,{...}`;
  `J::close()` chose its closing bracket from the *parent* container,
  so an object inside an array closed with `]` (`[{...]`) — the shape
  every probe track list takes; and the probe track-list serializers
  emitted `elem()`-separated objects without ever opening the array,
  producing `"audio":,{...}` and `"video":,` for empty lists. Probe
  `--json` for any file with tracks was malformed. Fixed all three;
  regression tests pin exact output shapes and assert probe `--json`
  contains no stray-separator forms.

- Workspace scaffolding: `crates/`, `codecs/`, `containers/` per the v2
  architecture. (`apps/` and `ffi/` are listed in `todo.md` §2 as the
  target structure but have **not** been added yet — they will be
  enabled as their implementations land; a previous version of this
  entry listed them as added, which was inaccurate.)
- `tpt-media-core`: shared media data types (Timestamp, Duration,
  Timebase, Packet, Frame, AudioFrame, VideoFrame, Track, Stream,
  CodecId, CodecConfig, PixelFormat, SampleFormat, ChannelLayout,
  MediaType, Metadata).
- `tpt-media-bitstream`: zero-allocation `BitReader`/`BitWriter`,
  Huffman encoder/decoder, Rice coder, public-domain range coder.
- `tpt-media-codec`: generic `Encoder`/`Decoder`/`Codec` traits,
  resource-limit types, error model, capability discovery.
- `tpt-media-container`: container abstraction over the `.tpt` format.
- `tpt-container`: native `.tpt` container implementation (header,
  TRAK, CLUS, seeking, corruption tolerance, streaming).
- `tpt-media-pipeline` §26: full pipeline core — `Node` abstraction with
  `NodeEvent`/`NodeError` contract (one input consumed, at most one output
  emitted per call), lockstep typed `Chain<L,R>` composition with
  cascading flush (left-stage internal buffers drain into the right stage
  before the right stage flushes its own), `Pipeline` pull/drive with a
  stall-bound backpressure guard, built-in `Source`/`Map`/`Bounded`/
  `Cancellable`/`Sink` nodes, `Timestamped<T>` PTS/DTS/duration
  pass-through, `MediaFormat`/`StreamSpec`/`FormatAware` stream and
  format negotiation, `Executor` threading abstraction (`Inline`, plus
  std-only `ThreadedExecutor` offloading the graph to a worker thread),
  and realtime scheduling (`RealtimeProfile`, `Clock`-injected
  `drive_scheduled`, `LatencyObserver`/`LatencyLog`, `ScheduledRun`).
  Design documented in `docs/pipeline-design.md`. no_std + alloc; 21 unit
  + 1 container integration test (§26, 2026-09-08).
- `tpt-media-io` §27: media I/O crate (no_std + alloc; `std` default
  feature). New `IoError` model (UnexpectedEof / InvalidHeader /
  Unsupported / Overflow / Other). PCM: `PcmSpec`, `encode_pcm`/`decode_pcm`
  with chunked monotonic PTS, `normalize_to_f32`/`denormalize_from_f32`
  (f32 domain bit-exact for ≤24-bit integer formats). WAV: byte-exact
  `parse_wav`/`decode_wav`/`write_wav` for the codec pipeline plus a
  std-gated `f32` bridge `decode_wav_f32`/`write_wav_f32` that **reuses
  `tpt-dsp-io`** rather than duplicating it (§27 constraint). Raster frame
  I/O: dependency-free PPM P6 / PGM P5 readers/writers over `VideoFrame`,
  `checkerboard` fixtures, std path helpers. Streaming `FrameSource` +
  `MediaItem` (pull) and `FrameSink` (push) traits with concrete
  implementations over slices/vecs (no_std) and files/stdin/stdout (`std`);
  a stream that ends mid-sample-frame is reported as an error. Async
  integration: `AsyncFrameSource`/`AsyncFrameSink` (RPITIT + `Send`,
  dependency-free) and a sync `BlockingAdapter`. Design:
  `docs/media-io-design.md`. 32 unit tests (§27, 2026-09-08).
- `tpt-media-probe`: container and codec introspection.
- `tpt-media-probe` §44: full `.tpt` probing via `TptDemuxer` — container
  version and global timebase, per-track audio/video/subtitle/metadata
  inspection (codec ID + version, sample rate, channel layout,
  dimensions, pixel format, first/last PTS, frame/sample counts, name,
  language, typed `TMTA` metadata), duration (last PTS + duration),
  overall and per-track bitrate estimates, integrity reporting
  (container `CRC1` footer and per-cluster checksum failures), and codec
  capability reporting against the `tpt-media-codec` `Registry` with the
  native codec family table. `probe_file` reads from disk (`std`).
  Dependency edge inverted: `tpt-container` no longer depends on
  `tpt-media-probe` (was unused), so probe → container (§44, §52,
  2026-09-08).
- `tpt-audio-cinder`, `tpt-audio-aura`, `tpt-audio-reed`: native audio
  codec implementations. (`tpt-video-thrum`, `tpt-video-vault`,
  `tpt-video-mote` are listed in `todo.md` §2 as the target
  structure but have **not** been added yet.)
- `apps/tpt-cli` §43: `tpt` is now a working trans-Q codec CLI.
  `encode`/`decode` are live end-to-end (WAV → `cinder`/`aura` → `.tpt`
  and back): encode reads a 16-bit mono WAV (44.1/48 kHz), chunks and
  zero-pads to codec block sizes (Cinder 512, Aura 1024), muxes with full
  track metadata (timebase, codec version, sample rate, channels,
  `--set-meta` tags) and records the exact sample count under the
  reserved `tpt.io.sample_count` key; decode sniffs the container, picks
  the first audio track, decodes via the track's codec (explicit
  `--codec` must match), trims padding and writes a 16-bit WAV. Binary
  stdout flows through `Outcome.binary_stdout`; `Ctx` gained a
  `write_file` injector. `transcode`/`benchmark` still exit 3 (not wired);
  `reed` exits 3. 27 unit tests incl. a lossless Aura round trip
  (§43, 2026-09-08).
- `tpt-cli` §43: the `tpt` command-line interface (first `apps/`
  workspace member). Commands `probe` (with `--json` deterministic
  output), `inspect`, `list-codecs`, and `--help` are fully implemented;
  `encode`/`decode` were later wired end-to-end (see the `tpt-cli` §43
  entry above); `transcode`/`benchmark` fully validate their config
  (`--codec --container --quality --bitrate --set-meta [--runs]`,
  positionals, `-` for stdin/stdout) then exit 3. Hand-rolled arg parser
  and JSON writer (no clap, no serde). Injectable I/O (`Ctx`) so every
  command is tested in-process without a filesystem. Design:
  `docs/cli-design.md`. 16 unit tests (§43, 2026-09-08).
- Documentation: `README.md`, `CONTRIBUTING.md`, `ARCHITECTURE.md`,
  `SECURITY.md`, `docs/architecture.md`, `docs/codec-design.md`,
  `docs/container-design.md`, `docs/pipeline-design.md`,
  `docs/cli-design.md`, `docs/media-io-design.md`, `docs/performance.md`,
  `docs/security.md`, `docs/provenance.md`.
- `tpt-media-representation`: representation research contracts —
  `Representation`/`Quantizer`/`RateEstimator`/`DistortionMetric`,
  spatiotemporal `SignalView`/`SignalBuffer` (`V(x,y,t)`),
  MSE/distortion/sparsity diagnostics.
- `tpt-media-core`: `Track::media` gains optional `TrackMedia` parameters
  (sample rate, channel layout, width/height, pixel format); `ChannelMask`
  gains `from_bits` for deserializers.
- `tpt-container` §20–25 (container completeness, 2026-09-08):
  - Header: 20-byte layout with a reserved `u32`; real version checking
    (`UnsupportedVersion` for version 0 / future major / future minor);
    documented compatibility rules; 16-byte version 0.1 headers still parse.
  - `TRAK`: full track descriptor — media type, codec version, per-track
    timebase, sample rate, channel layout, dimensions, pixel format, name,
    language. New `TMTA` chunk for typed metadata pairs.
  - `CLUS`: distinct `PTS`/`DTS`/`duration`, dedicated `DISCONTINUITY` flag,
    optional per-cluster CRC-32 (flags bit 4).
  - Index: `IDX ` per-track chunk with absolute offsets and a version byte;
    unknown index versions are ignored (recovery); `seek_to_keyframe_on_track`
    and `seek_to_offset`; `rebuild_index()` and `reconstruct_trailer()`.
  - Robustness: incomplete-final-cluster handling, reserved/unknown chunk
    skipping, container `CRC1` footer, append mode, live-stream mode, and a
    deterministic fuzz-style corruption/truncation sweep that cannot panic.
  - Streaming: fragmented/clustered writing, incremental parsing via
    `new_streaming` + `feed()`, live append, missing-final-index playback,
    graceful termination via the `finalize()` trailer.
- Dependency-free no_std CRC-32 (`crc32` module) for container checksums.
- `tpt-media-research`: §56 representation research platform —
  five concrete representations (`baseline`, `block_dct`, `temporal_dct`,
  `patch_vq`, `motion_aligned`), deterministic entropy estimation and
  synthetic signal generators, the RD/sparsity measurement harness with
  primary/secondary/diagnostic metric hierarchy, `det_rng`, and trained
  VQ codebook construction (`kmeans.rs`).
- Representation experiments: four-condition oracle (moving edge) plus
  Phases 0/2/4/5, asserted for measurement validity (never hypothesis
  direction) and documented, results included, in
  `docs/representation-research.md`.

### Fixed

- `Timestamp::rescale`: the overflow-safe conversion was using the
  wrong factors (`self.timebase.num` twice instead of `target.num`),
  which both produced incorrect results for non-identity rescalings
  *and* masked cases that should overflow as `None`. Math re-derived
  from first principles, factors corrected, and the overflow test
  replaced with one whose inputs actually overflow `i64` (a previous
  `i64::MAX` µs → 90 kHz case fit in `i64` even with the bug, so it
  could not have been exercising the overflow branch).
- `tpt-audio-aura`: doc comment incorrectly described the lossless
  residual coder as Rice coding delegated to `tpt-media-bitstream`;
  the actual code (and v0.1 wire format) uses an in-crate
  `write_exp_golomb`/`read_exp_golomb` over zigzag-mapped residuals.
  Comments now match the code; switching the wire format to Rice
  would be a separate, breaking change and is tracked as the §974
  unchecked item.
- `Timestamp::add`/`sub` (Timestamp ± Duration): silently fell back
  to the unchanged left-hand side on overflow via `unwrap_or(self.ticks)`,
  which produces a result that's neither pre- nor post-overflow
  without any way to tell. Replaced with **signed-duration-aware
  saturating** arithmetic (`i64::MAX`/`i64::MIN` in the direction
  of the signed Duration), plus new fallible
  `Timestamp::checked_add_duration` / `checked_sub_duration` for
  callers that genuinely need to detect overflow.
- `PacketFlags::DISCONTINUITY` was present in `tpt-media-core` but
  undocumented and not round-tripped through the v1 `.tpt` wire
  format (`tpt-container` reserved only bits 0x01 KEYFRAME and
  0x02 END_OF_FRAME). Now: full semantic docs on all three flags,
  `Packet::is_discontinuity()` / `is_end_of_frame()` accessors, and
  bit 0x04 round-trips through both muxer and demuxer.
- `tpt-audio-cinder`: the per-packet header carried `frame_size` in an
  8-bit field, so the *default* `frame_size` of 256 overflowed to 0
  and every packet the default encoder produced was rejected as
  `MalformedPacket` by the decoder. The header now writes a 1-bit
  128/256 selector (both sides updated together; existing 128
  packets and the `plc` codec are unaffected). Regression test added.
- `tpt-audio-cinder`: `bits_per_coef` was not part of the bitstream,
  so a decoder configured with default 8 bits mis-parsed packets
  encoded at a different coefficient depth. The CLI now carries it
  (and Aura's `block_size`, which is likewise not self-described) in
  the track's codec-private `data`; defaults still apply when the
  field is absent.

### Added

- `tpt-media-core::sync`: a `no_std`-friendly clock-domain /
  synchronization module per `todo.md` §20 — `ClockDomain`
  (named `(String, Timebase)`, equality by name only),
  `SyncPoint` (a cross-domain pin), `SyncMap` (linear-interpolation
  translator with explicit *no extrapolation* to prevent A/V
  drift), and `SyncError`. Includes a realistic
  audio-video sync end-to-end test (48 kHz audio × 90 kHz video
  with two pins at t=0 and t=60 s).
- `tpt-media-bitstream`: `entropy` module (`shannon_entropy_bits` /
  `shannon_entropy_nats`, `normalize`, `histogram`) wired into `lib.rs`
  (§17, 2026-09-09).
- `tpt-audio-cinder` §29 tests: `plc_conceals_mid_stream_packet_loss`
  (real drop-and-conceal stream scenario — encode a 4-frame tone, drop
  the middle packet, substitute `conceal()`, assert sample-count/PTS
  continuity and non-silent concealed output with clean decode before
  and after the gap) and `jittered_pts_are_preserved_not_reordered`
  (deliver valid packets out of PTS order without panic or
  self-reordering; output frames echo their own packet's PTS)
  (2026-09-09).
- `tpt-audio-aura` §30: per-packet CRC-32 integrity. Every packet now
  ends with a 4-byte CRC-32 (`crc32`, `CRC_TRAILER_BYTES`) over all
  preceding payload bytes; `AuraDecoder` verifies the checksum before
  parsing and rejects a mismatch (or a packet shorter than the trailer)
  as `CodecError::MalformedPacket`. Tests: trailer round-trip,
  body-bit-flip rejection, checksum-bit-flip rejection, undersized
  packet rejection. **Wire-format impact:** this is a v0.1 packet-format
  addition — packets produced before this change carry no trailer and
  are rejected by the new decoder (2026-09-09).
- `tpt-audio-aura` §30 (lossy mode): quantization is now psychoacoustically
  driven instead of a flat 16-bit-per-coefficient scheme. Each MDCT bin is
  mapped to a Bark critical band (`tpt-dsp-core::hz_to_bark`); bands are
  classified tonal/noise (`classify_tonal_bins`) and spread across the Bark
  axis to model simultaneous masking (`simultaneous_masking`,
  `combined_threshold_db`); the resulting per-band masking threshold sets
  that band's quantization step (`step = sqrt(12 * threshold_linear)`,
  clamped), and coefficients are quantized in bin order with first-order
  spectral noise shaping (`noise_shape_step`) so residual quantization
  error is fed forward rather than left independent per bin. The per-band
  step table (one 32-bit float per Bark band) is transmitted so decode
  needs no side information beyond the packet itself; the Bark-band layout
  itself is *not* transmitted — both encoder and decoder derive it
  deterministically from `(sample_rate, n_half)` alone. New tests:
  `mdct_bin_bark_bands_is_deterministic_and_nondecreasing`,
  `lossy_step_table_reflects_masking_not_flat`. **Wire-format impact:**
  this changes the v0.1 lossy packet layout (adds the per-band step table
  before the coefficient array) — lossy packets produced before this
  change are not decodable by the new decoder and vice versa; lossless
  mode is unaffected (2026-09-11).
- `tpt-audio-cinder` §29: optional perceptual weighting.
  `CinderEncoderConfig::perceptual_weighting` (default `false`, existing
  callers unaffected) scales each band's configured quantization step by
  that band's absolute threshold of hearing (`tpt-dsp-core::threshold_in_quiet`)
  relative to a 1kHz reference, computed once at construction time (not
  per frame). Deliberately does not use Aura's full masking model —
  Cinder is real-time/low-complexity by design. Scaling only ever coarsens
  a step, never sharpens below the configured value (the wire format's
  step field is an integer). No wire-format change. Tests:
  `perceptual_weighting_off_by_default_leaves_step_unchanged`,
  `perceptual_weighting_coarsens_less_sensitive_bands_never_below_configured`,
  `perceptual_weighting_round_trip_carries_signal` (2026-09-11).
- `tpt-media-codec`/codecs §19: `ResourceLimits` are now enforced by
  real decoders. `tpt-audio-cinder` checks `check_allocation` (decoded
  spectrum buffer) and `check_frame_bytes` (output) during decode;
  `tpt-audio-aura` checks `check_frame_bytes` and per-mode
  `check_allocation` (lossless LPC coefficient/sample buffers, lossy
  MDCT spec/synthesis buffers) before allocating. Both codecs have a
  test proving a structurally valid packet plus a tight budget yields
  `CodecError::ResourceLimit(Allocation)` (2026-09-09).
- `tpt-media-codec`/§19: new `ResourceLimits::max_metadata_bytes` +
  `ResourceLimitKind::MetadataSize` + `check_metadata`, following the
  existing `check_packet`/`check_allocation` pattern (`permissive()`
  16MiB, `strict()`/`Default` 64KiB). Wired into `tpt-container`:
  `TptDemuxer` gained a `limits: ResourceLimits` field and
  `with_limits`/`new_streaming_with_limits` constructors; `index_tmta`
  now checks a `TMTA` chunk's raw body length against the limit *before*
  parsing it, returning the previously-unused `TptError::ResourceExceeded`
  on violation. Test:
  `oversized_tmta_metadata_is_rejected_by_default_but_allowed_with_permissive_limits`
  (2026-09-11).
- `tpt-audio-aura`/`tpt-audio-cinder` §35: versioned bitstreams. Both
  codecs' packets now lead with a 16-bit wire-format version field (the
  pre-existing `VERSION` constant, actually written to the stream for
  the first time); a decoder that reads a version it doesn't recognize
  returns `CodecError::Unsupported` immediately, before interpreting any
  further bits. **Wire-format impact:** breaking for both codecs —
  packets produced before this change have no version field and are
  misread by the new decoder (and vice versa). This is the last
  un-gracefully-handled breaking change either codec should need: every
  future version bump can now be rejected cleanly instead of silently
  misparsed. Tests: `decode_rejects_unsupported_wire_version` (Aura and
  Cinder). Also strengthened Aura's `lossy_step_table_reflects_masking_not_flat`
  test, which had silently stopped checking real data once this change
  shifted every subsequent bit offset in the packets it hand-parses — it
  now also asserts every recovered step is finite and positive
  (2026-09-11).
- §38 fuzz infrastructure: deterministic, dependency-free
  corruption/truncation sweeps (fixed-seed LCG, no `cargo-fuzz`/libFuzzer,
  same approach as `tpt-container`'s pre-existing §24 sweep) added to
  every remaining crate that parses untrusted input:
  `tpt-media-bitstream::fuzz_like` (`BitReader`, Huffman, Rice, range
  coder), `tpt-audio-aura`/`tpt-audio-cinder`
  (`decode_never_panics_on_corrupted_or_arbitrary_packets`), and
  `tpt-media-probe` (`probe_never_panics_on_corrupted_or_arbitrary_bytes`).
  Found two real bugs on first run — see `Fixed` below (2026-09-11).

### Fixed

- `tpt-media-bitstream`: `BitReader::skip` corrupted every read following
  a multi-byte, non-byte-aligned skip. It correctly computed how many
  whole bytes to advance past, but then reloaded the *wrong* byte (one
  past the skip region) to recover the last skipped byte's unconsumed
  trailing bits, and also spuriously reloaded a byte when the skip landed
  exactly on a byte boundary. Unnoticed until now because the only
  production caller (`tpt-audio-aura`'s `read_exp_golomb`) only ever
  calls `skip(1)` (never the multi-byte path), and the crate's own prior
  `skip_across_bytes` test happened to skip exactly the bits already
  buffered in the accumulator. Surfaced by a new `tpt-audio-cinder` test
  that skips over a real packet's per-band coefficients
  (`skip(n_coeffs * bits_per_coef)`) to inspect the transmitted step
  table. Fixed by rewriting `skip` to advance past whole bytes directly
  and only reload one byte for a genuine partial-byte remainder. New
  regression tests:
  `skip_past_a_fresh_reader_leaves_trailing_bits_of_the_last_byte_readable`,
  `skip_an_exact_multiple_of_8_bits_consumes_no_extra_byte`,
  `skip_beyond_available_bits_is_end_of_input` (2026-09-11).
- `tpt-media-bitstream::range::RangeDecoder::decode` divided by zero on a
  *second* call after an earlier call on the same decoder had already
  returned `EndOfInput`: running out of input partway through
  renormalization leaves `range` below `TOP` (and potentially below
  `total`) without ever restoring it, so `range/total` on a later call
  could be `0`. Found by the new `tpt-media-bitstream::fuzz_like`
  corruption sweep. Fixed by rejecting `total > self.range` up front
  (which also covers a caller genuinely passing a `total` larger than
  `range`, not just this poisoned-state case). New regression test:
  `decode_keeps_returning_err_after_running_out_of_input_instead_of_panicking`
  (2026-09-11).
- `tpt-audio-cinder`'s decoder read `n_bands` and each band's `n_coeffs`
  straight from the packet with no bound relative to `frame_size`, so a
  malformed/adversarial packet claiming more bands/coefficients than
  actually fit in the decoded spectrum buffer indexed it out of bounds
  and panicked instead of returning `CodecError::MalformedPacket`. Found
  by the new codec fuzz sweep (see `Added` above). Fixed by
  bounds-checking `start + n_coeffs <= spec.len()` (and rejecting
  `n_bands == 0`) before writing. New regression tests:
  `decode_rejects_n_coeffs_that_would_overrun_the_spectrum`,
  `decode_rejects_zero_bands` (2026-09-11).
- §47 security audit (integer overflow / bounds checks / allocation
  exhaustion / decompression bombs) over `tpt-container`'s untrusted-byte
  decoders found and fixed two more real issues: (1) `decode_idx`'s
  `entry_count` is a fully attacker-controlled `u32` (up to ~4.29
  billion) that was passed straight to `Vec::with_capacity` with no
  check against the actual input size — a malformed `IDX ` chunk of a
  few bytes could trigger a multi-gigabyte allocation attempt. Fixed by
  rejecting `entry_count` unless it could actually fit in the remaining
  body (each entry is a fixed 17 bytes). (2) `decode_tmta`'s `META_BYTES`
  branch bounds-checked a length field (`len`, also an attacker-controlled
  `u32`) with plain `p + len` `usize` addition — correct on 64-bit, but
  able to wrap around on a 32-bit target (this workspace explicitly
  targets 32-bit: `thumbv7em-none-eabi` for Reed, and wasm32 for the
  planned WASM target) and pass a check it should have failed. Fixed with
  `checked_add`. Tests:
  `decode_idx_rejects_entry_count_that_would_overrun_the_body`,
  `decode_tmta_rejects_bytes_value_length_that_would_overrun_the_body`
  (2026-09-11).
- CI's `no_std` Reed build (`cargo build -p tpt-audio-reed --target
  thumbv7em-none-eabi --no-default-features`) now compiles. The
  failure was hidden twice over: (1) member-level `default-features =
  false` on the workspace-shared `tpt-media-{core,bitstream,codec}`
  dependencies was silently *ignored* because Cargo only honours it
  when the `workspace.dependencies` entry declares it — so the `std`
  feature stayed on for the bare-metal target; and (2) `thiserror
  1.0.69` has no Cargo features and unconditionally requires `std`, so
  the `default-features = false` on it was a no-op. Fixed: workspace
  deps now declare `default-features = false` (std-consuming members
  opt back in via `features = ["std"]`, forwarding the feature
  transitively), `thiserror` bumped to 2.x (std optional), and
  `tpt-media-codec` gained the missing `#![cfg_attr(not(feature =
  "std"), no_std)]` gate plus an `alloc::string::String` import.
- `tpt-media-bitstream` / `tpt-media-core`: `f64` methods (`ln`,
  `log2`, `round`) are not available in `core` for `no_std` targets;
  the float math now routes through `libm` instead (2.13.1-era
  `bitflags` compiles clean for the bare-metal target once its `std`
  feature is not unified in).

## [0.0.0] — 2026-09-04

### Removed

- v1 workspace scaffolding deleted in favour of the v2 architectural plan.

## [0.1.0] — 2026-09-04

### Added

- Initial workspace scaffolding for the v1 plan (`tpt-core`,
  `tpt-audio-pulse`, `tpt-audio-aura`, `tpt-audio-vox`,
  `tpt-video-livelink`, `tpt-video-vault`, `tpt-video-whisper`,
  `tpt-cli`, `tpt-web`, `tpt-media-sys`).
- CI workflow (`fmt + clippy + test`, plus `no_std` vox build).
- Release workflow (Linux x86_64, macOS x86_64, macOS aarch64,
  Windows x86_64).

[Unreleased]: https://github.com/TPT-Solutions/tpt-media/compare/v0.0.0...HEAD
[0.0.0]: https://github.com/TPT-Solutions/tpt-media/releases/tag/v0.0.0
[0.1.0]: https://github.com/TPT-Solutions/tpt-media/releases/tag/v0.1.0
