# Changelog — tpt-video-vault

Scoped record of changes to `tpt-video-vault`. The workspace-wide release log —
including cross-cutting changes and the lockstep version bump that ships every
crate together — is the root [`CHANGELOG.md`](../../CHANGELOG.md). This file
is a filtered view of it: a change touching only this crate is recorded here
*and* summarised in the root entry; a change touching several crates is
recorded in the root, which remains the record (CONTRIBUTING §9.1).

Format follows [Keep a Changelog](https://keepachangelog.com/). Versions are
lockstep with the workspace; no API-stability guarantee pre-1.0
(CONTRIBUTING §9.2).

> **Wire-format changes are breaking regardless of the crate version.** This
> codec is fail-fast: an unsupported 16-bit bitstream version is an error, not
> a guess (todo.md §35).

## [Unreleased]

### Added

- **Edge-case tests (2026-11-05).** `tests/edge_cases.rs` covers wrong geometry
  and format, short pixel buffers, zero-length and truncated packets, invalid
  configs, extreme pixel content, and overflowing geometry. Each case is refused
  or handled without a panic. The memory test's saturating counter now uses a
  CAS loop, for MSRV 1.86.
- **Long-term reference: wire version 5 (breaking, 2026-10-07).** One long-term
  reference picture, which the encoder can pin and later predict from. Two
  per-frame bits in I/P headers: `ltr_used` widens the leaves' reference-index
  field by one value that names the long-term picture, and `ltr_update` pins the
  frame's reconstruction as the new long-term reference. Both are per-frame wire
  facts, so parsing never depends on decoder state. Pinned by `tests/ltr.rs`.
  Design: `docs/codec-design.md`, "Vault B-frames v0.3 remainder".
- **Per-leaf combine modes: wire version 4 (breaking, 2026-10-07).** A non-skip B
  leaf begins with a 2-bit mode: `0` weighted bi-prediction with both vectors,
  `1` forward-only, `2` backward-only, `3` reserved and rejected. A leaf then
  carries only that mode's vectors. Measured before landing: the straddling
  scene-cut group cost 42% fewer B bits at +0.54 dB, and an ordinary pan cost 24%
  fewer B bits at +1.4 dB. `FrameStats` gained `fwd` and `bwd` counters, pinned
  by `tests/b_frames.rs`.
- **Per-frame weighted bi-prediction: wire version 3 (breaking, 2026-10-07).** A
  B frame's header carries a 2-bit weight naming the past reference's share in
  quarters: plain average, 1/4, or 3/4. The encoder chooses per B frame by exact
  frame `J`, which costs up to three encodes per B frame. Pinned by
  `tests/weighted_b.rs`.
- **Scene-cut detection (`VaultConfig::scene_cut`, 2026-10-07).** Off by default.
  The first frame after a hard cut is coded intra when its mean absolute luma
  difference from the last reference spikes above an absolute floor and a factor
  of the previous frame's difference. The detector saves tail bytes, not
  distortion. A cut gets the rate controller's 4× intra allowance.
- **Loop filter rate–distortion decision (`VaultConfig::loop_filter_rdo`,
  2026-10-07).** Off by default. Each frame keeps whichever of the filtered and
  unfiltered reconstructions is closer to the source. The decision cannot see the
  filter's effect on later frames' prediction, so on a smooth panning clip it can
  keep the filter when the stream would decode better without it.
  `FrameStats` gained a `loop_filter` field.
- **Frame-level rate control (`VaultConfig::target_bits_per_frame`, 2026-10-05).**
  A bounded per-frame `qp` search of at most three encodes per frame, with a
  budget bucket that lets the first intra frame spend 4× and banks what earlier
  frames saved. `tpt encode --codec vault --bitrate` now means something on the
  video path. No wire change. Encode cost is up to 3× on adapting frames.
- **Coefficient-level RDOQ (`VaultConfig::rdoq`, 2026-10-05).** An exact-rate
  zeroing pass, measured and default off. At the codec's operating points it
  produced byte-identical output, so the knob waits for a `lambda` tuned on real
  content.
- **B-frames: bi-prediction with display-order reordering, wire version 2
  (breaking, 2026-10-05).** `VaultConfig::b_frames` (0 to 4, default 0) buffers a
  lookahead group and codes it in decode order. B leaves carry motion-vector
  deltas against the fixed past and future reference pair. B reconstructions
  never enter the reference store. The keyframe interval counts display frames,
  so a `b_frames = 2` stream no longer runs a GOP four times longer than
  configured (fixed 2026-10-07).
- **The fast transform (2026-10-05).** Vault's `Xform` runs on `tpt-dsp-core`'s
  FFT-backed DCT plans in std builds, with the direct path kept for bare metal.
  The encoder is 11 to 26× faster, and every corpus scene fits the 33.3 ms QCIF
  frame budget. Rate and PSNR are byte-identical to the direct transform.
- **Playable in the browser and bare-metal builds (2026-10-05).** Vault joined
  `tpt-web`'s `DecoderTable::native()`. The crate builds for
  `thumbv7em-none-eabi` with `--no-default-features`, and the CI `no_std` job
  covers it.
- **Initial implementation** (2026-09-30, todo.md §33): `Gray8` and planar
  `Yuv420` at any size that is a multiple of 8; 64×64 superblocks
  recursively partitioned to 8×8 with implicit splits at the picture edge;
  half-pel motion (full-pel pattern search plus 8-neighbour half-pel
  refinement, 2-tap bilinear interpolation, neighbour-predicted vector
  differences); up to 16 reference frames; four intra modes at every block
  size; 4×4–32×32 DCT from `tpt-dsp-core` in orthonormal quantiser units;
  RDO over partition, mode, reference and vector with the explicit objective
  `J = D + λR` (greedy over a pruned candidate set, not a global optimum);
  static Rice coefficient coding; boundary-strength-driven H.263 Annex J loop
  filter; per-packet CRC-32 trailer plus the 16-bit bitstream version;
  `max_packet_bits` derived bound; freeze-frame concealment; `DynEncoder` /
  `DynDecoder` factories with codec id `VAUL`.
- Hostile-input hardening and resource limits (§19/§38/§47): header,
  reference-index and motion-vector range checks, bounded coefficient walks,
  atomic failed decodes, packet/frame/allocation/reference/recursion limits.
- `VaultDecoder::last_stats()` (`FrameStats`): a per-frame census of the coding
  tools actually used.
- Tests: 15 unit, 18 property, 4 coding-tool census, 1 memory (38 total), and
  a criterion bench at QCIF and CIF.
- Provenance records in `docs/provenance.md` and design notes in
  `docs/codec-design.md`.

### Fixed

- **B frames coded no residual at all, and priced candidates the decoder never
  renders** (fixed in wire version 2, 2026-10-05).
- **`VaultConfig::max_packet_bits` under-charged B leaves and the header** (fixed
  2026-10-07). A B leaf writes four absolute vectors, not the two P deltas that
  were charged. The derivation also still used the version 1 header. The bound
  now charges the worst frame type per unit, and its test runs at `b_frames = 2`.
- **Keyframe interval counted group anchors, not display frames** (fixed
  2026-10-07). A `b_frames = 2` stream ran a GOP four times longer than
  configured.
