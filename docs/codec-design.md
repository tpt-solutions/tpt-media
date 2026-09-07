# Codec Design Notes

The native TPT codecs follow a **3-niche paradigm**: instead of one
general-purpose codec, we ship three distinct codecs per medium (audio,
video), each tuned for a specific use case.

## Audio

| Codec  | Niche                  | Key technique             | Sample rate  | Channels  |
| ------ | ---------------------- | ------------------------- | ------------ | --------- |
| Pulse  | Real-time, low latency | MDCT + quantization       | 8–48 kHz     | mono–stereo |
| Aura   | High-fidelity archival | LPC lossless / MDCT lossy | up to 96 kHz  | up to 8   |
| Vox    | Edge / IoT speech      | LPC + VQ                  | 8–16 kHz     | mono      |

## Video

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

Pulse, Livelink and Vox have two execution profiles:

- **Real-time**: zero allocation, bounded work, caller-owned buffers,
  deterministic execution time.
- **Offline**: allocation permitted, larger scratch buffers, parallel
  search, expensive RDO (Livelink is offline-only on the encoder side).

The decoder side of every codec is real-time safe by default; the encoder
side of Pulse, Livelink, and Vox is also real-time safe; the encoder
side of Aura, Vault, and Whisper is offline.

## Determinism

Decoding is always deterministic — any conformant bitstream produces
identical samples across machines. Encoding is deterministic only when
explicitly configured; in offline mode we allow non-deterministic
choices (parallel search, hash-based tie-breaking).

## Bitstream Versioning

Every codec embeds a 4-byte `CodecId` and a `BitstreamVersion` in its
configuration. A decoder encountering a higher version may attempt to
decode in compatibility mode but must surface a warning. Forward
extensions are reserved via tag bits; backward-incompatible changes
require a new `CodecId`.

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

Each codec crate follows the same internal layout:

```text
crates/<codec>/
├── Cargo.toml
├── README.md
├── PROVENANCE.md
├── src/
│   ├── lib.rs           — public API, Encoder/Decoder wrappers
│   ├── config.rs        — CodecConfig + Capabilities
│   ├── bitstream.rs     — codec-specific bitstream helpers (optional)
│   ├── tables.rs        — codec-specific constant tables (optional)
│   └── <algorithm>.rs   — internal algorithms
└── tests/
    ├── round_trip.rs
    └── corpus.rs
```

## Codec Testing

Every codec crate ships:

- Unit tests for every internal algorithm.
- A round-trip test (synthetic input → encode → decode → compare).
- A resource-limit test (oversized input must error, not crash).
- A determinism test (same input → identical bitstream on repeat).
- A real-time-safety test for codecs that claim it (compile-time
  assertion + runtime allocation tracker).