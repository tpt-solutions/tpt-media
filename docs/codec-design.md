# Codec Design Notes

The native TPT codecs follow a **3-niche paradigm**: instead of one
general-purpose codec, we ship three distinct codecs per medium (audio,
video), each tuned for a specific use case.

## Audio

The table below is the *design* matrix; the binding contract is each
codec's `supported_formats` table in code, which is deliberately
narrower today: every audio codec is S16 mono — 44.1/48 kHz for
Pulse/Aura, 8/16 kHz for Vox.

| Codec  | Niche                  | Key technique             | Sample rate  | Channels  | Status |
| ------ | ---------------------- | ------------------------- | ------------ | --------- | ------ |
| Pulse  | Real-time, low latency | MDCT + quantization       | 8–48 kHz     | mono–stereo | implemented (S16 mono, 44.1/48 kHz) |
| Aura   | High-fidelity archival | LPC lossless / MDCT lossy | up to 96 kHz  | up to 8   | implemented (S16 mono, 44.1/48 kHz) |
| Vox    | Edge / IoT speech      | LPC + VQ                  | 8–16 kHz     | mono      | implemented (S16 mono, 8/16 kHz) |

### Vox v0.1 — implemented design (2026-09-28)

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
  period (50 Hz–1 kHz searchable) and the decoder drives an impulse
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

## Video

No video codec is implemented yet (todo.md §32–34); this table is the
design target the representation research (§56,
[`representation-research.md`](representation-research.md)) is meant to
be measured against.

| Codec     | Niche                    | Key technique             | Resolution | Bitrate target |
| --------- | ------------------------ | ------------------------- | ---------- | -------------- |
| Livelink  | Real-time communication  | I/P + intra-refresh       | up to 1080p | adaptive      |
| Vault     | High-compression archival | Superblocks + RDO         | up to 4K    | offline       |
| Whisper   | Edge / IoT               | Dictionary VQ             | ≤480p       | <50 kbps      |

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

Current claims: Pulse is real-time on both the encoder and decoder
(pinned by `codecs/pulse/tests/realtime_alloc.rs`); Vox is real-time on
the decoder only — its encoder runs a documented 1024-entry codebook
search per unvoiced frame and deliberately does *not* claim real-time
safety (pinned by `codecs/vox/tests/realtime_alloc.rs`); Aura is
offline on both sides (archival profile). Livelink/Vault/Whisper do not
exist yet.

## Determinism

Decoding is always deterministic — any conformant bitstream produces
identical samples across machines (§39 pins this byte-for-byte for
container generation, packets, and decodes). *Encoding is also fully
deterministic for every implemented codec*: no RNG, wall-clock, thread
count, or hash-order dependence exists anywhere in Pulse, Aura, or Vox,
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

## Patent-Conscious Design Choices

The following choices are made specifically to reduce patent exposure
(see `CONTRIBUTING.md` §3):

- **No CABAC.** Pulse and Aura use static Huffman or a static range
  coder; Livelink uses simple VLC tables; Vault uses Huffman with
  context but no adaptive probability tracking.
- **No fractional-pixel motion estimation beyond half-pixel bilinear.**
  Livelink uses full-pixel only. Vault uses full-pixel + 2-tap bilinear
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