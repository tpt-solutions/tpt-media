# tpt-audio-reed

Reed: TPT extremely-low-bandwidth speech codec (no_std LPC vocoder with LSP quantization and stochastic-excitation VQ).

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](../../LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)
[![no_std](https://img.shields.io/badge/bare__metal-builds-green.svg)](https://doc.rust-lang.org/1.86.0/core/)

The bandwidth end of the native audio family: ~4.5 kbps of parameters,
honestly robotic, and the only crate in the workspace that CI proves builds
for bare metal. Part of the [`tpt-media`](../../README.md) workspace —
[workspace changelog](../../CHANGELOG.md) · [changelog](CHANGELOG.md) ·
[codec design](../../docs/codec-design.md) · [provenance](../../docs/provenance.md) ·
[contributing](../../CONTRIBUTING.md).

## At a glance

| | |
|---|---|
| Role | Native low-bitrate speech codec (todo.md §31) |
| Codec ID | `REED` · bitstream `VERSION = 1` |
| Status | Implemented |
| Version | `0.1.0`, lockstep with the workspace |
| MSRV | 1.86 — verified, enforced by CI's `msrv` job |
| License | MIT OR Apache-2.0 |
| Rates | 8 kHz and 16 kHz, mono, 20 ms frames |
| Bitrate | 4.5 kbps parameters; 6.4 kbps at container level (16 bytes/frame) |
| Real-time | **Decoder** real-time-safe; encoder is not (`real_time_encoder: false`) |
| `no_std` | Yes — CI builds it for `thumbv7em-none-eabi` |
| Measured peak memory | 8 140 B against a documented < 16 KiB budget |

## Design (v0.1)

A source-filter (LPC vocoder) model in the spirit of the classic 2.4 kbps
LPC-10 codecs. Per 20 ms frame the encoder transmits:

- **Spectral envelope** — order-p LPC (autocorrelation + Levinson-Durbin),
  converted to line spectral pairs and uniformly quantised. The decoder
  inverts after re-enforcing strictly-increasing LSP ordering, so an
  arbitrary bit stream can never synthesise through an unstable filter. LSP
  representation is what makes coarse quantisation + interpolation safe;
  raw LPC coefficients are neither.
- **Voicing + pitch** — normalised-autocorrelation pitch estimation
  classifies the frame and estimates its period; the decoder drives an
  impulse train at that period.
- **Excitation gain** — the residual's RMS, quantised in 1 dB steps over
  62 dB. Index 0 is **exact mute**, so all-zero input round-trips to digital
  silence rather than dither.
- **Unvoiced excitation shape** — a shape-gain VQ over a fixed stochastic
  codebook of 1024 unit-RMS noise shapes. The codebook is **not transmitted**:
  both sides derive the identical entry from the index deterministically via
  a SplitMix64 stream, so it costs 10 bits per unvoiced frame and nothing
  else, and no training corpus is required.

**Packet loss** — `ReedDecoder::conceal` repeats the last good frame's
envelope/voicing/pitch with the excitation gain decayed 6 dB per concealed
frame, so gaps fade out instead of buzzing.

## Usage

```rust
use tpt_audio_reed::{ReedConfig, ReedDecoder, ReedEncoder};
use tpt_media_codec::{Decoder, Encoder};
use tpt_media_core::CodecConfig;

let cfg = CodecConfig::new(*b"REED", 1);
let rc = ReedConfig::default(); // 16 kHz, order 10, lsf_bits 6

let mut enc = ReedEncoder::new(cfg, rc.clone())?;
let packets = enc.encode(&frame_320_samples)?;

let mut dec = ReedDecoder::new(cfg, rc)?;
let frames = dec.decode(&packets[0])?;
```

CLI: `--quality 0..=100` maps to Reed's LSF bit depth — a pure
envelope-fidelity versus packet-size trade. Search cost is
voicing-dependent, not quality-dependent.

## Wire format

Little-endian bit packing via `tpt-media-bitstream`: 16-bit version, 3-bit LPC
order code, 4-bit LSF bit depth, 1-bit voicing, 6-bit gain index, 10-bit pitch
period (voiced) or codebook index (unvoiced), then `order` LSF indices of
`lsf_bits` bits each, then a 4-byte CRC-32 trailer. Unsupported bitstream
versions are rejected up front (todo.md §35).

## Bare-metal build

```bash
cargo build -p tpt-audio-reed --target thumbv7em-none-eabi --no-default-features
```

That is what CI's `no_std` job runs. Float math is `libm`-backed there — this
is **not** a fixed-point implementation (todo.md §31).


## Benchmarks & profiling

```bash
cargo bench -p tpt-audio-reed
```

`benches/codec_bench.rs` — encode/decode of 64 × 320-sample (20 ms) frames at
16 kHz (2026-09-28, release profile, criterion):

| bench | median | per-item | real-time factor |
|---|---:|---:|---:|
| `reed/encode` (64 frames, alternating voiced/noise) | 17.0 ms | 266 µs/frame | ~75× |
| `reed/decode` (64 packets) | 375 µs | 5.9 µs/packet | ~3400× |

Encode is dominated by the unvoiced shape-gain codebook search — 1024
candidate excitations regenerated and correlated per noise frame. Pure voiced
frames skip the search entirely, so the 266 µs median over a half-voiced
input is a worst case, not a typical one. Decode is one codebook-entry
regeneration plus LPC synthesis: 5.9 µs against a 20 ms budget.
Tables: [`docs/performance.md`](../../docs/performance.md).

Profiling recipe: `cargo bench -p tpt-audio-reed`, then `cargo flamegraph
--bench codec_bench` (Linux) / `Instruments` (macOS) / `VTune` (Windows).

## Testing

```bash
cargo test -p tpt-audio-reed
```

Envelope and pitch preservation are pinned explicitly, alongside PLC
behaviour, property tests, the corruption/truncation sweep, codec-private
round trip, and `tests/realtime_alloc.rs` (32 steady-state passes over
alternating voiced/noise frames, 512 KiB budget — the search regenerates
entries into one fixed scratch frame rather than allocating per candidate).

## Known limitations

- **Robotic by design.** ~4.5 kbps cannot carry natural speech; that is the
  point of the codec, not a defect.
- The encoder is not real-time-safe and does not claim to be.
- Fixed 20 ms frames; no variable-rate operation mode.
- The synthetic codebook is defined by construction, not trained — there is
  no adaptation to a particular speaker.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
