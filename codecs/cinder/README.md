# tpt-audio-cinder

Cinder: TPT native real-time low-latency audio codec (MDCT + scalar quantisation + PLC).

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](../../LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)

The low-latency end of the native audio family, built for a live
conversation rather than for archival quality. Part of the
[`tpt-media`](../../README.md) workspace —
[workspace changelog](../../CHANGELOG.md) · [changelog](CHANGELOG.md) ·
[codec design](../../docs/codec-design.md) · [provenance](../../docs/provenance.md) ·
[contributing](../../CONTRIBUTING.md).

## At a glance

| | |
|---|---|
| Role | Native real-time audio codec (todo.md §29) |
| Codec ID | `CIND` · bitstream `VERSION = 1` |
| Status | Implemented |
| Version | `0.1.0`, lockstep with the workspace |
| MSRV | 1.86 — verified, enforced by CI's `msrv` job |
| License | MIT OR Apache-2.0 |
| Formats | 16-bit mono `S16` @ 44.1 kHz and 48 kHz |
| Frame sizes | 128 or 256 samples (default 256) |
| Real-time | **Encoder and decoder** both real-time-safe (bounded allocation) |
| Measured peak memory | 6 645 B against a documented < 32 KiB budget |

## Algorithm

- **Transform** — MDCT with a sine window (`tpt_dsp_core::mdct_sine_window`,
  shared with Aura).
- **Quantisation** — uniform scalar, per-band step size.
- **Entropy** — fixed-width bit fields for the uniform quantiser index. No
  adaptive coder: at these block sizes the coder was not the bottleneck.
- **PLC** — finds the pitch period of the recent output and repeats it across the gap,
  fading over consecutive losses (`spec.txt` 5.1). With no usable period (stream start,
  silence, noise) it falls back to the last spectrum with pseudo-random sign flips. On the
  speech benchmark clip at 10% random loss, concealed output scores 16.0 dB against the
  lossless decode, up from 6.8 dB with sign flips alone (`tests/diag_plc.rs`).
- **Optional perceptual weighting** (`CinderEncoderConfig::perceptual_weighting`,
  off by default) — scales each band's step by that band's absolute threshold
  of hearing relative to 1 kHz, computed **once at construction**. It only
  ever coarsens a step, never sharpens it, and costs nothing per frame.

## Usage

```rust
use tpt_audio_cinder::{CinderDecoder, CinderEncoder, CinderEncoderConfig};
use tpt_media_codec::{Decoder, Encoder};
use tpt_media_core::{CodecConfig, Packet};

let cfg = CodecConfig::new(*b"CIND", 1);
let enc_cfg = CinderEncoderConfig::default(); // 256-sample frames, 48 kHz

let mut enc = CinderEncoder::new(cfg, enc_cfg.clone())?;
let mut dec = CinderDecoder::new(cfg, enc_cfg)?;

// One packet per 256 input samples; the last block is held back until drain.
let mut packets: Vec<Packet> = enc.encode(&audio_frame)?;
packets.extend(enc.drain()?);

let mut frames = Vec::new();
for pkt in &packets {
    // The first packet completes no output: the decoder has one block of latency.
    frames.extend(dec.decode(pkt)?);
}
```

Quality maps to `CinderEncoderConfig::bits_per_coef` (4–12) through the CLI's
`--quality 0..=100`.

## Wire format and versioning

Every packet leads with a 16-bit bitstream version, rejected up front:
unsupported versions are an error, never a guess (todo.md §35, fail-fast).
Version bumps are breaking changes; the wire layout is specified field by
field in [`docs/codec-design.md`](../../docs/codec-design.md).

Wire-format changes that have shipped (recorded here because they change
what bytes are valid, not just the API):

- Version 2 (2026-10-14): overlap-add. The packet layout is unchanged, but
  each packet is now a hop-`frame_size` block that the decoder overlap-adds.
  Version 1 packets are rejected. Bit depths 4–7 were also corrected: levels
  now clamp to the width's signed range. See `CHANGELOG.md`.

- The CRC-32 trailer (`CRC_TRAILER_BYTES = 4`) was added over all preceding
  payload bytes; packets encoded before it are rejected.
- The 16-bit bitstream version field was added; pre-version packets are
  misread as such and rejected.

## Benchmarks & profiling

```bash
cargo bench -p tpt-audio-cinder
```

`benches/codec_bench.rs` — encode of 64 × 512-sample blocks, and decode of
the 128 packets they produce, at 48 kHz. Release profile, criterion `--quick`,
2026-10-14 (version 2). Version 1 measured 604 µs and 676 µs.

| bench | median | per-item | real-time factor |
|---|---:|---:|---:|
| `cinder/encode` (64 × 512 samples = 0.68 s) | 606 µs | 9.5 µs/block | ≥1000× |
| `cinder/decode/128_packets` | 673 µs | 5.3 µs/packet | ≥1000× |

Each packet now covers one 256-sample hop, so a single packet costs about
half of what a 512-sample packet did, but there are twice as many of them.
Encode and decode together are about 15 µs per hop against a 5.3 ms frame
budget at 256/48 kHz, which leaves roughly three orders of magnitude of
headroom. Canonical tables: [`docs/performance.md`](../../docs/performance.md).

Profiling recipe: release build, `cargo bench -p tpt-audio-cinder`, then
`cargo flamegraph --bench codec_bench -- -c <case>` on Linux,
`Instruments` on macOS, `VTune` on Windows.

## Testing

```bash
cargo test -p tpt-audio-cinder
```

- Round-trip encode/decode, silence stays silent, determinism at the
  packet-byte level (two independent encoder/decoder instances).
- Packet-loss and jitter tests, PLC behaviour.
- Seeded-random property tests over full-range audio.
- Corruption/truncation/bit-flip sweeps (todo.md §38) — this is what caught
  the decoder reading `n_bands`/`n_coeffs` from a hostile packet with no
  bound relative to `frame_size`, which indexed the spectrum buffer out of
  bounds and panicked instead of returning `MalformedPacket`.
- `tests/realtime_alloc.rs` — a counting global allocator pins steady-state
  allocations over 31 encode+decode passes at a 512 KiB budget.

## Known limitations

- **Noise-like content plateaus at the default step.** With `band_steps` of
  1.0, the ±127 level clamp limits noise and mixed material to about 21 dB
  SNR at any bit depth from 5 to 10. That is a tuning limit: a larger step
  lifts the ceiling, at the cost of noise on quiet material. Use
  [`tpt-audio-aura`](../aura) for archival fidelity.
- Mono only, 44.1/48 kHz, `S16` — anything else is refused with
  `CodecError::Unsupported`, not silently converted.
- Fixed-width entropy coding is not rate-adaptive; `--bitrate` has no effect
  because no codec here has rate control.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
