# Codec Design Notes

The native TPT codecs follow a **3-niche paradigm**: instead of one
general-purpose codec, we ship three distinct codecs per medium (audio,
video), each tuned for a specific use case.

## Audio

The table below is the *design* matrix; the binding contract is each
codec's `supported_formats` table in code, which is deliberately
narrower today: every audio codec is S16 mono — 44.1/48 kHz for
Cinder/Aura, 8/16 kHz for Reed.

| Codec  | Niche                  | Key technique             | Sample rate  | Channels  | Status |
| ------ | ---------------------- | ------------------------- | ------------ | --------- | ------ |
| Cinder  | Real-time, low latency | MDCT + quantization       | 8–48 kHz     | mono–stereo | implemented (S16 mono, 44.1/48 kHz) |
| Aura   | High-fidelity archival | LPC lossless / MDCT lossy | up to 96 kHz  | up to 8   | implemented (S16 mono, 44.1/48 kHz) |
| Reed    | Edge / IoT speech      | LPC + VQ                  | 8–16 kHz     | mono      | implemented (S16 mono, 8/16 kHz) |

### Reed v0.1 — implemented design (2026-09-28)

A source-filter (LPC vocoder) model in the LPC-10 tradition, at a fixed
20 ms frame (160/320 samples at 8/16 kHz). Per frame the encoder
transmits a 90-bit parameter payload (+4-byte CRC-32 trailer, the family
convention): 90 bits of coded parameters = 4.5 kbps; the wire-rounded
16-byte packet = 6.4 kbps at the container level.

- **Spectral envelope** — Hamming-windowed autocorrelation LPC (order 2–16,
  default 10) → line spectral pairs → uniform per-coefficient
  quantization. LSP representation is the safety story: stability under
  coarse quantization is verifiable (`lsp_is_stable`) and the config
  enforces `2^lsf_bits > lpc_order` so a strictly-increasing index set
  always exists; the decoder re-enforces the ordering and rejects any
  set that still fails the stability check.
- **Excitation** — voiced frames transmit a 10-bit pitch
  period (50 Hz–1 kHz searchable) and the decoder drives an imcinder
  train; unvoiced frames transmit a 10-bit index into a fixed 1024-entry
  stochastic codebook (shape-gain VQ, the CELP fixed-codebook idea).
  The codebook is *never transmitted*: both sides derive the identical
  unit-RMS entry from a SplitMix64 stream seeded by the index, so it
  costs zero wire bits and needs no training corpus.
- **Gain** — LPC-residual RMS in 1 dB steps over a 62 dB range; index 0
  is exact mute, so digital silence round-trips to digital silence.
- **Packet loss** — `conceal()` repeats the last good envelope/voicing/
  pitch with gain decayed 6 dB per concealed frame (fading to silence).
- **Known v0.1 simplifications** — no cross-frame pitch-phase
  continuity, no LSP interpolation across frames, no sign bit on the
  codebook gain (negative-correlation entries simply score badly in the
  search), and argmax pitch re-estimation on narrow-band synthetic
  vowels can lock onto formant ringing near half the true period (the
  tests assert periodicity at the transmitted lag instead).

### Cinder v1 — wire format

`codecs/cinder/src/lib.rs` (`VERSION = 1`). One packet = one frame;
fixed-width fields, bit-packed via `BitWriter` in this order:

| Field | Width | Contents |
| ----- | ----- | -------- |
| `version` | 16 bits | bitstream version, `1`; a mismatching decoder rejects with `CodecError::Unsupported` before any further parse |
| `frame_size` | 1 bit | `0` = 128 samples, `1` = 256 — self-described per packet, so a decoder configured for one size inverts the other correctly |
| `n_bands` | 8 bits | number of spectral bands that follow |
| per band: `step` | 16 bits | integer quantization step (the band's effective step after optional perceptual weighting; integer by wire definition, so weighting can only coarsen) |
| per band: `n_coeffs` | 8 bits | coefficient count for the band |
| per band: levels | `bits_per_coef` bits each | quantized MDCT levels, offset binary (wire value = level + 128, source clamped to ±127) |
| `keyframe` | 1 bit | always `1` in v1 — every Cinder packet is self-contained (`PacketFlags::KEYFRAME` is set on every emitted packet) |

No entropy coder and no integrity trailer in v1 — each level is stored
at its full configured width, and packet integrity is the container's
job at this layer. The per-packet bit budget is
`16 + n_bands·(24 + bits_per_coef·band_size) + 8 + 16`, which bounds
the encoder's allocation exactly.

### Aura v1 — wire format

`codecs/aura/src/lib.rs` (`VERSION = 1`). One packet = one block; a
1-bit mode selects one of two layouts after a shared 4-byte header:

| Field | Width | Contents |
| ----- | ----- | -------- |
| `version` | 16 bits | bitstream version, `1`; rejected up front on mismatch |
| `mode` | 1 bit | `1` = lossless (LPC), `0` = lossy (MDCT) |
| `lpc_order` | 5 bits | LPC order, 1–32 (used by both modes) |
| `mdct_size` | 8 bits | `size/256 − 1` (so 2048 → 7, 4096 → 15) |
| **lossless:** coefficients | 16 bits × order | Q15 fixed-point LPC coefficients |
| **lossless:** residuals | Exp-Golomb each | zigzag-mapped prediction residual per sample — exact integer arithmetic over the *quantized* coefficients is what makes the round trip bit-exact |
| **lossy:** step table | 32 bits × n_bands | f32 quantization step per Bark band, transmitted so the decoder needs no side information |
| **lossy:** spectrum | 16 bits per bin | quantized MDCT bin index (noise-shaped before quantization) |
| `crc32` | 32 bits | IEEE 802.3 CRC-32 (little-endian) over all preceding bytes, verified before parsing |

## Video

One video codec is implemented (todo.md §32); the other two are still
design targets (§33–34) for the representation research (§56,
[`representation-research.md`](representation-research.md)) to be
measured against.

| Codec     | Niche                    | Key technique             | Resolution | Bitrate target | Status |
| --------- | ------------------------ | ------------------------- | ---------- | -------------- | ------ |
| Thrum  | Real-time communication  | I/P + intra-refresh       | up to 1080p | adaptive      | implemented (v0.1, 2026-09-28) |
| Vault     | High-compression archival | Superblocks + RDO         | up to 4K    | offline       | not started (§33) |
| Mote   | Edge / IoT               | Dictionary VQ             | ≤480p       | <50 kbps      | not started (§34) |

### Thrum v0.1 — implemented design (2026-09-28)

`codecs/thrum/src/lib.rs`. The design goal is *latency and bounded
work*, so almost every choice is the cheap one:

- **No B-frames, no reordering, no look-ahead.** A frame encoded at
  wall-clock *t* is decodable at *t*. The whole one-way latency is the
  encode + decode cost of that one frame — measured in
  [`performance.md`](performance.md) (≈13.4 ms at 176×144 4:2:0 against
  a 33.3 ms budget at 30 fps, of which decode is ≈1.9 ms).
- **Macroblock = 16×16 luma** (four 8×8 luma blocks) + one 8×8 U and one
  8×8 V for 4:2:0, raster order. Frame dimensions must be non-zero
  multiples of 16, so the picture is exactly macroblock-aligned and
  there is no padding path to get wrong. One frame = one packet.
- **Four intra modes** (DC / vertical / horizontal / diagonal), chosen
  per luma block by SAD, chroma always DC. The diagonal mode is a shear
  of the top edge — deliberately simple, and it reads *only* the row
  above, which is always fully reconstructed before the block is
  predicted. (Reading inside the block's own footprint would make the
  prediction depend on not-yet-written pixels, and the encoder would
  drift from the decoder.)
- **Full-pixel integer motion compensation**, one reference frame,
  exhaustive `±search_range` search costed by SAD with an exact early
  exit. No sub-pel interpolation, no weighted prediction, no multiple
  references. Chroma vectors are *derived* (the luma vector halved),
  not signalled, because 4:2:0 makes that the natural mapping.
- **Rate-aware skip.** A macroblock whose mean absolute prediction error
  is below one quantiser step is signalled skipped. Tying the threshold
  to the step rather than to a fixed constant is what keeps skip alive
  at coarse `qp`, where the reconstruction error alone would exceed any
  fixed bound.
- **Intra refresh** on a sliding window: `intra_refresh_percent` of a
  P-frame's macroblocks are re-coded intra, and the window advances by
  that many each frame, so the whole picture is swept in
  `ceil(total / per_frame)` frames. A receiver resynchronises from
  packet loss without waiting for a full I-frame.
- **Transform/quant/entropy**: 8×8 separable DCT-II/III from
  `tpt-dsp-core` (§3.1 — the codec consumes, it does not reimplement);
  one flat dead-zone uniform quantiser driven by a 6-bit `qp` in every
  frame header, resolved through a fixed table so the step is identical
  on every target and needs no float `pow`; and **real Rice coding** with
  a per-block parameter chosen by the encoder.
- **Deblocking**: a 4×4-grid loop filter, applied to the whole
  reconstructed frame, with the filtered picture becoming the next
  reference. Encoder and decoder run the identical function on identical
  input, so it can never cause drift.

Wire format: 16-bit version (rejected up front), 1-bit frame type,
6-bit `qp`, then the macroblock stream, then a CRC-32 trailer verified
before parsing — §35's family convention. `ThrumConfig::max_packet_bits`
*derives* the worst case from the level clamp and the per-code bit bound
rather than estimating it, which is what makes the encoder's single
allocation provably safe.

Known sharp edges, stated plainly:

- The encoder is **not** claimed real-time (`real_time_encoder` is
  false): the exhaustive search costs `(2·search_range + 1)²` SAD
  evaluations per macroblock. Only the decoder is claimed real-time,
  pinned by `codecs/thrum/tests/realtime_alloc.rs`.
- **No rate control.** `qp` is a configuration constant, not
  adaptively chosen, so "adaptive" in the table above is a target and
  not yet a feature.
- **Chroma is DC-only intra** and uses halved motion vectors, so chroma
  detail at sub-macroblock scale is not preserved. Fine for a
  communication codec, not for archival use.
- **Wired into the CLI through the y4m bridge**, not through
  `tpt-media-io`: `tpt encode/benchmark` take a `.y4m` input and
  `tpt decode` writes one back, with `--codec thrum`. The bridge is
  progressive-only and `C420*`/`Cmono`-only, and lives in
  `apps/tpt-cli/src/y4m.rs`. A video codec on a non-video path still
  fails with an explicit message rather than a generic unknown-codec.

## Common Patterns

All codecs share:

- A `CodecConfig` describing decoder parameters.
- A `Capabilities` struct describing supported formats, frame sizes, etc.
- A bounded `ResourceLimits` enforcement layer.
- Round-trip tests on synthetic corpora.
- Provenance records for each non-trivial algorithm.

## Real-Time vs Offline

The two execution profiles, with the claims actually made per codec
(`Capabilities` in code is binding; "real-time" means bounded allocation
and bounded work per frame, not literal zero allocation):

- **Real-time**: bounded allocation, bounded work, deterministic
  execution time.
- **Offline**: allocation permitted, larger scratch buffers, parallel
  search, expensive RDO.

Current claims: Cinder is real-time on both the encoder and decoder
(pinned by `codecs/cinder/tests/realtime_alloc.rs`); Reed is real-time on
the decoder only — its encoder runs a documented 1024-entry codebook
search per unvoiced frame and deliberately does *not* claim real-time
safety (pinned by `codecs/reed/tests/realtime_alloc.rs`); Aura is
offline on both sides (archival profile). Thrum/Vault/Mote do not
exist yet.

## Determinism

Decoding is always deterministic — any conformant bitstream produces
identical samples across machines (§39 pins this byte-for-byte for
container generation, packets, and decodes). *Encoding is also fully
deterministic for every implemented codec*: no RNG, wall-clock, thread
count, or hash-order dependence exists anywhere in Cinder, Aura, or Reed,
and the property tests assert byte-identical packets from independent
encoder instances. The allowance for non-deterministic offline encoding
(parallel search, hash tie-breaking) remains a policy for future codecs
that may use it, not a license currently exercised.

## Bitstream Versioning

Every codec packet leads with a 16-bit wire-format version (§35) after
its 4-byte `CodecId` (carried in the container's `TRAK` chunk, not per
packet). A decoder encountering a version it does not understand fails
fast with `CodecError::Unsupported` *before* interpreting any further
bits — there is deliberately no compatibility-mode guesswork and no
warning channel. Note the honest gap (§35): the codecs have no
reserved/extension *bits* inside a version yet, so every wire-format
change so far has been a breaking version bump; the container side has
the forward-compatible mechanism (reserved chunk ranges, unknown-chunk
skipping).

The current versions, and where each layout is specified:

| Codec | Crate | `VERSION` | Layout specified | Integrity |
| ----- | ----- | --------- | ---------------- | --------- |
| Cinder | `tpt-audio-cinder` | 1 | above, "Cinder v1 — wire format" | none in v1 (documented limitation) |
| Aura | `tpt-audio-aura` | 1 | above, "Aura v1 — wire format" | CRC-32 trailer |
| Reed | `tpt-audio-reed` | 1 | above, "Reed v0.1 — implemented design" | CRC-32 trailer |
| Thrum | `tpt-video-thrum` | 1 | above, "Thrum v0.1 — implemented design" | CRC-32 trailer |

A wire-format change bumps `VERSION`, fails fast against every older
decoder, and must land with its layout table extended here and its
provenance entry updated (`docs/provenance.md`) in the same change.

## Patent-Conscious Design Choices

The following choices are made specifically to reduce patent exposure
(see `CONTRIBUTING.md` §3):

- **No CABAC.** Cinder and Aura use static Huffman or a static range
  coder; Thrum uses simple VLC tables; Vault uses Huffman with
  context but no adaptive probability tracking.
- **No fractional-pixel motion estimation beyond half-pixel bilinear.**
  Thrum uses full-pixel only. Vault uses full-pixel + 2-tap bilinear
  half-pixel.
- **No parametric stereo.** Each channel is encoded independently; the
  caller may downmix before encoding if desired.
- **No SBR / high-frequency synthesis.** Lossy modes of Aura encode the
  full spectrum.

## Codec Internal Layout

Each codec crate follows the same internal layout (provenance lives
centrally in [`provenance.md`](provenance.md), not per crate):

```text
codecs/<codec>/
├── Cargo.toml
├── src/
│   └── lib.rs           — public API, config, Encoder/Decoder + factories
├── benches/
│   └── codec_bench.rs   — criterion encode/decode bench (§41)
└── tests/
    ├── property.rs      — seeded-random round trips + determinism (§38/§39)
    └── realtime_alloc.rs — bounded-allocation pin (§42, codecs that claim it)
```

The wire format, quantizers, and packetization live in `lib.rs` with the
DSP itself delegated to `tpt-dsp-core` (§3 of `todo.md`); docs/design
notes live in this directory, not in the crate.

## Codec Testing

Every codec crate ships (all three audio codecs do today):

- Unit tests for the internal algorithms (quantizers, bitstream layout,
  concealment).
- Round-trip tests (synthetic input → encode → decode → compare).
- A resource-limit test (oversized/tight-budget input must error, not
  crash).
- A determinism test (same input → identical bitstream, across
  independent instances).
- The §38 deterministic corruption sweep (arbitrary bytes, truncation,
  bit flips — no panic, ever).
- A bounded-allocation test for codecs that claim real-time paths
  (counting global allocator, `realtime_alloc.rs`).