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

- **`vault` and `mote` are driven by the y4m bridge** (2026-10-01,
  todo.md §33's open item): `--codec vault` / `--codec mote` now work for
  `encode`, `decode`, `transcode` and `benchmark` exactly as `--codec thrum`
  does, and both appear in `list-codecs` and `--help`. `VideoCodec` grew the
  two variants plus `codec_id`/`new_encoder`/`new_decoder`, and the five
  video command paths (`run_video_encode_job`, the decode branch of
  `demux_and_decode_any`, `encode_video`, `benchmark_video`, the transcode
  re-encode) now go through them instead of naming Thrum's concrete types.

  The erasure point is `Encoder`/`Decoder<Frame = VideoFrame>`, **not** the
  type-erased `Dyn*` contracts the FFI and `tpt-web` use. `DynFrame` carries
  no PTS and the `Dyn*` wrappers substitute `Timestamp::zero` for it, so a
  `.tpt` muxed through them would lose every frame timestamp — and setting
  those timestamps is the y4m bridge's job. `new_encoder` returns the encoder
  *and* its codec-private bytes together, because the track records the
  configuration the encoder was built with and a decoder reconstructs from
  those bytes alone.

  Two behaviour changes worth calling out, both intentional:
  `benchmark` now names the codec it measured in its human and JSON output
  instead of a hard-coded "thrum", and `transcode` names the *source* codec
  from the track rather than assuming thrum (so `vault -> mote` reports
  truthfully). The Thrum-only `% 16` geometry check was removed in favour of
  each codec's own `validate()` — a rule copied into the CLI would have been
  wrong for at least two of the three codecs.

  Five new tests: a y4m round trip per codec through `decode` (which
  re-derives the decoder from the track header, so it covers the
  codec-private bytes too), PTS preservation, per-codec geometry refusal,
  `benchmark` naming its codec, and a vault→mote `transcode`.

- **`tpt video-corpus --codec` measures any of the three video codecs**
  (2026-10-01, todo.md §33's remaining open item): the same eight §36 scenes
  at the same geometry, reporting the codec it actually measured in both the
  table and the JSON. Two real fixes fell out of generalising it:

  - The packet↔frame pairing was a `zip` that assumed one packet per frame.
    Mote breaks that assumption — it prefixes the first frame with a
    dictionary packet that decodes to nothing — which would have compared
    every frame with its predecessor. Source frames are now indexed by PTS
    and a decoded picture is scored against the frame it came from, which is
    correct whatever packet-to-frame mapping a codec uses.
  - The reported `frames` was `packets.len()`, and since that is the divisor
    in the bitrate and bpp maths, Mote's out-of-band dictionary understated
    its rate by 25% (496 → 621 kbps on talking-head). It is now the number of
    decoded *pictures*.

  Measured cross-codec numbers are in `todo.md` §33, with the explicit
  caveat that they are synthetic clips at a shared `qp` and support no
  compression claim. Three new tests, including one that fails if the
  packet count is used for `frames` again.

- **Per-crate README, changelog, keywords and categories** (2026-09-30):
  crate-specific `README.md` and manifest `keywords`/`categories`. The README
  documents the full command surface, the option table, the exit codes, and
  both bridges — the WAV bridge for audio and the YUV4MPEG2 bridge for
  video.
- `make_decodable_tpt()` test fixture (2026-09-30): builds a real `.tpt` by
  running `tpt encode` and capturing the injected writer, for tests that
  need decoding to actually succeed rather than fail early.
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
- Y4M bridge for video (`video`/`y4m` modules, 2026-09-28): `encode` and
  `benchmark` take a `.y4m` input with `--codec thrum` and encode one packet
  per frame; `decode` writes a video track back out as `.y4m`; `transcode`
  re-encodes it. Byte-exact, progressive-only, `C420*`/`Cmono`-only, and
  aspect ratio is parsed then discarded (the `.tpt` track model has no aspect
  field).
- `--codec thrum` / `--codec LIVE` resolve and `tpt list-codecs` advertises
  them. On a *non*-video path — a WAV into `encode`, an audio track into
  `transcode` — the failure still names the video/audio mismatch rather than
  claiming the codec is unregistered.
- `tpt video-corpus`: measures `thrum` over the §36 synthetic video corpus
  (PSNR, SSIM, bitrate, bits-per-pixel, encode/decode CPU and frame latency),
  generating its own content so it reads no files.

### Fixed

- `tpt probe` reported that no codec was available, for codecs the same binary
  encodes and decodes one subcommand later. It called
  `tpt_media_probe::probe`, which answers from an *empty* registry — the right
  default for a library that cannot know what its caller linked in, the wrong
  one for a CLI that knows exactly which codecs it carries. `tpt probe out.tpt`
  on a `THRM` file printed `encoder=false, decoder=false` immediately after
  `tpt encode --codec thrum` had succeeded. It now resolves against a registry
  of all four native codecs, and the flags come from each codec's own declared
  capabilities:

  ```text
  Codec THRM: native=true, decoder=true, encoder=true, lossless=false, lossy=true, rt=true
  ```

  `probe --json` changes with it (`encoder_available`/`decoder_available` are
  now `true` for native codecs). Pinned by two new tests. Thrum's factories are
  geometry-parameterised, so they are built from `ThrumConfig::default()`; that
  placeholder never reaches the report, which carries no geometry, and
  `build()` takes the real dimensions from the track's codec-private bytes.
- **No `.y4m` file could be parsed at all.** The header parser takes `C` as
  the *tag* and stores only what follows (`"420jpeg"`), but the colorspace
  match compared the *full tag* (`"C420jpeg"`), so no colorspace ever
  matched and every y4m input was rejected as unsupported. Fixed to match
  the value, with the `C` re-added in the error message. Caught by the
  `y4m::tests::*` unit tests, which had been failing since the bridge landed.
- Two tests asserted the pre-y4m policy and now assert real behaviour:
  `decode_rejects_wrong_container_flag` (decoding writes WAV for an audio
  track and y4m for a video one, so the `--container` requirement depends on
  the track) and `video_codecs_resolve_but_are_rejected_by_audio_commands`
  (used a nonexistent input file, so it failed on I/O before reaching the
  codec dispatch; now uses a real decodable fixture).
- `cargo fmt --all --check` failed on ten hunks in `commands.rs` and
  `y4m.rs`; reformatted.
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

### Removed

- Dead code that was failing CI's `-D warnings`: the audio-only
  `demux_and_decode` (superseded by `demux_and_decode_any`, which also
  handles video tracks), the unused `y4m::plane_count`, and an unreachable
  `Some(other)` arm over the single-variant `VideoCodec` enum.
- Exit code 3 ("codec not wired yet"): with every listed codec implemented, no
  code path produced it, and the stale documentation claiming otherwise was
  removed.

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
