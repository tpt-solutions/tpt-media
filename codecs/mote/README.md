# tpt-video-mote

Mote: TPT extreme edge/IoT video codec (8x8 patch dictionary VQ, 4096 entries, 12-bit index, temporal skip).

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](../../LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)

The smallest video codec in the native family: a **dictionary
(vector-quantisation) codec** for low-resolution, mostly-static camera feeds on
constrained devices. A frame is cut into 8×8 patches, and each patch is
replaced by the index of its nearest entry in a shared dictionary of up to 4096
patches. The decoder is a table lookup and a copy. Part of the
[`tpt-media`](../../README.md) workspace —
[workspace changelog](../../CHANGELOG.md) · [changelog](CHANGELOG.md) ·
[codec design](../../docs/codec-design.md) · [provenance](../../docs/provenance.md) ·
[performance](../../docs/performance.md) · [contributing](../../CONTRIBUTING.md).

## At a glance

| | |
|---|---|
| Role | Native extreme-edge video codec (todo.md §34) |
| Codec ID | `MOTE` · bitstream `VERSION = 1` |
| Status | Implemented (v0.1); driven by the CLI's y4m bridge |
| Version | `0.1.0`, lockstep with the workspace |
| MSRV | 1.86 |
| License | MIT OR Apache-2.0 |
| Coded formats | `Gray8` and planar `Yuv420` |
| Dimensions | multiples of 16; long edge ≤ 848, short edge ≤ 480 ("≤480p", either orientation) |
| Dictionary | 1–4096 entries of 8×8 bytes; index is `ceil(log2(entries))` bits, **12 bits at 4096** |
| Real-time | Decoder: bounded work, one table lookup per patch. Encoder is not (`real_time_encoder: false`) |
| `no_std` | Source is `no_std`-compatible (`--no-default-features` builds); like Thrum it links `tpt-dsp-core` with `std`, and no bare-metal target is CI-verified |

## Design (v0.1)

- **Patches.** 8×8 blocks of one plane, dimension 64, numbered plane by plane
  (Y, U, V) in raster order. Multiples of 16 make even 4:2:0 chroma tile
  exactly: no padding, no partial patch. All planes share one dictionary.
- **Dictionary + VQ.** Nearest-entry search is
  `tpt_dsp_core::nearest_vector_accelerated` (squared error) — VQ lives in
  `tpt-dsp` (todo.md §8), Mote owns only the codebook *contents*. Every
  distance is an exact integer in `f32`, so the encoder is bit-reproducible.
- **Training.** `Dictionary::train_from_frames` is a seeded, integer-centroid
  Lloyd iteration with deterministic empty-cluster re-seating. No `HashMap`,
  no clock, no thread state: same input and seed give the same bytes.
- **Dictionary is stream state.** Train it offline on the deployment scene and
  provision it out of band (`MoteDecoder::with_dictionary`), or send it once as
  a *dictionary packet*. Every frame packet carries the dictionary's 16-bit id,
  so a mismatched dictionary is rejected rather than rendered as garbage.
- **Reconstruction.** The decoded picture is dictionary entries, no post-filter.
  A skipped patch keeps the previous decoded patch bit for bit; encoder and
  decoder run the identical copy, so they cannot drift.
- **Temporal prediction.** Conditional replenishment against one reference
  frame: a delta frame codes only the patches the previous reconstruction no
  longer explains, as Rice-coded skip runs plus indices. An unchanged frame is
  an 11-byte heartbeat. No motion compensation (see Known limitations).

## Wire format

Every packet is `body ++ CRC-32(body)` (LE); the CRC is verified before any
parsing (todo.md §35). MSB-first bits:

```text
common:  version:16  type:2          0 dictionary, 1 key, 2 delta
dict:    entries-1:12  pad:2 (=0)  entries*64 bytes
key:     dict_id:16   patches * index_bits
delta:   dict_id:16  k:3  count:16   count * ( rice_k(skip run)  index )
```

`MoteConfig::max_packet_bits` derives the worst-case frame packet from the
geometry and dictionary size, and the encoder's single buffer is sized by it.
A key frame with the 4096-entry dictionary costs exactly 12/64 = 0.1875 bits
per luma pixel (plus a 34-bit header and the CRC).

## Usage

```rust
use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_video_mote::{Dictionary, MoteConfig, MoteDecoder, MoteEncoder, TrainConfig};

let cfg = MoteConfig { width: 320, height: 240, ..MoteConfig::default() };

// Offline: train the 4096-entry dictionary on frames from the deployment site.
let dict = Dictionary::train_from_frames(&training_frames, &TrainConfig::default())?;

// Camera side: `embed = false` because the decoder is provisioned out of band.
let mut enc = MoteEncoder::with_dictionary(CodecConfig::default(), cfg.clone(), dict.clone(), false)?;
let packets = enc.encode(&frame)?;          // key frame first, then small delta frames

// Receiver side.
let mut dec = MoteDecoder::with_dictionary(CodecConfig::default(), cfg, dict)?;
let frames = dec.decode(&packets[0])?;
```

`MoteEncoder::new` instead trains a dictionary on the first frame and sends it
in-band; that dictionary is capped well below the frame's patch count
(`self_trained_entries`), because a dictionary as large as its training set
just memorises the frame. For the full 4096-entry target, train offline.
`MoteDecoder::conceal` repeats the last frame (mid-grey before the first).
`MoteEncoder::dictionary_packet` re-emits the dictionary for a late joiner.

The quality metric (`compare`, `QualityReport`) is defined in
[`src/quality.rs`](src/quality.rs): luma PSNR is the primary figure, with
all-plane PSNR and mean absolute error alongside.

## Benchmarks

```bash
cargo bench -p tpt-video-mote
cargo test  --release -p tpt-video-mote --test memory -- --nocapture
```

**The bandwidth target is defined by these benchmarks, not promised for
arbitrary content.** Mote's rate is a function of how much of the scene
changes and how well the dictionary fits it. The measured tables — bandwidth,
CPU, memory, and the temporal-prediction investigation — are in
[`docs/performance.md`](../../docs/performance.md) and
[`docs/codec-design.md`](../../docs/codec-design.md).

Headline (synthetic fixed-camera scene, 320×240 mono, 4096-entry dictionary
trained on the scene): key frame 1 809 B (42:1 against raw), mean delta frame
46 B, i.e. 0.37 kbps at 1 fps and 5.55 kbps at 15 fps; luma PSNR 35–41 dB. A
key-frame-only 640×480 stream at 1 fps is 57.7 kbps, so any sub-50 kbps claim
at 480p rests on temporal skip.

## Testing

```bash
cargo test -p tpt-video-mote
```

42 unit tests (dictionary and training determinism, dimension rules, patch-grid
round trips, key/delta round trips that must equal the encoder's own
reconstruction, 12-bit index sizing, hostile dictionary/index/run/count
packets, state not corrupted by a rejected packet, resource limits, the
provable packet bound, factories), plus integration tests:
`property.rs` (shapes across geometries, dictionary size orders quality,
determinism, and a §38 hostile-input sweep including bit flips with a repaired
CRC and random bodies behind a valid header), `low_bitrate.rs` (exact
12-bit-per-patch key frame, heartbeat, 2-entry floor, static-camera bandwidth),
`temporal.rs` (the temporal-prediction investigation) and `memory.rs`
(counting allocator, full 4096-entry dictionary).

## Known limitations

- **The dictionary is not free.** 4096 entries is 256 KiB — a one-time cost the
  bandwidth figures exclude, and the reason a late joiner needs
  `dictionary_packet`. Entries are stored raw; compressing them is open work.
- **No motion compensation.** A moving object costs its patches twice (where it
  arrives, where it leaves). Built for static cameras.
- **PSNR only.** The quality metric is a fidelity yardstick; it says little
  about how patch blockiness reads to a viewer.
- **Self-trained dictionaries are small**, and figures for a dictionary trained
  on the same frames it is tested on flatter the codec; the benchmark also
  prints a held-out-site number.
- Tree/product quantisation for larger dictionaries (todo.md §8) is not
  implemented; search is the accelerated linear scan.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
