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
- **PLC** — phase-randomised sign perturbation of the previous frame's MDCT.
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
let packets: Vec<Packet> = enc.encode(&audio_frame)?;

let mut dec = CinderDecoder::new(cfg, enc_cfg)?;
let frames = dec.decode(&packets[0])?;
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

- The CRC-32 trailer (`CRC_TRAILER_BYTES = 4`) was added over all preceding
  payload bytes; packets encoded before it are rejected.
- The 16-bit bitstream version field was added; pre-version packets are
  misread as such and rejected.

## Benchmarks & profiling

```bash
cargo bench -p tpt-audio-cinder
```

`benches/codec_bench.rs` — encode/decode of 64 × 512-sample blocks at 48 kHz.
On the recorded machine (2026-09-14, release profile, criterion):

| bench | median | per-item | real-time factor |
|---|---:|---:|---:|
| `cinder/encode` (64 × 512 samples = 0.68 s) | 604 µs | 9.4 µs/block | ≥1000× |
| `cinder/decode` (64 packets) | 676 µs | 10.6 µs/packet | ≥1000× |

Encode + decode ≈ 20 µs against a 5.3 ms frame budget at 256/48 kHz —
roughly three orders of magnitude of headroom. Canonical tables:
[`docs/performance.md`](../../docs/performance.md).

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

- **Structural quality ceiling.** Uniform scalar quantisation with a fixed
  bit depth tops out around 3 dB better than the input's own noise floor in
  the recorded experiments (todo.md Known Issues). Use
  [`tpt-audio-aura`](../aura) for archival fidelity.
- Mono only, 44.1/48 kHz, `S16` — anything else is refused with
  `CodecError::Unsupported`, not silently converted.
- Fixed-width entropy coding is not rate-adaptive; `--bitrate` has no effect
  because no codec here has rate control.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
