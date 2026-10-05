# Performance Guide

Performance work in `tpt-media` follows three rules:

1. **Measure first.** No optimization without a benchmark.
2. **Scalar reference first.** Every optimized kernel ships with a scalar
   reference that the SIMD version must match within a documented
   tolerance.
3. **Real-time paths are sacred.** No allocation, no FMA ordering
   tricks that change results.

## Real-Time Profile

APIs marked `// SAFETY: real-time-safe` must:

- Not allocate (no `Vec::new`, no `Box::new`, no `String`).
- Not block on I/O or locks.
- Have bounded execution time.
- Use caller-owned buffers.

The following APIs are real-time-safe in `0.1.x`:

- `tpt-media-bitstream::BitReader` (pure, allocation-free)
- `tpt-media-bitstream::BitWriter` (allocation-free when given an
  external buffer)
- `tpt-media-codec::Decoder::decode` for codecs in real-time mode
  (cinder, thrum, reed)
- `tpt-audio-cinder::Decoder::decode`
- `tpt-audio-reed::Decoder::decode`

## Offline Profile

Offline APIs may allocate, parallelize, and perform expensive RDO. The
encoder side of `aura`, `vault`, and `mote` is offline-only.

## Benchmark Suite

We use `criterion` for benchmarks. The benchmark suite covers:

- MDCT / IMDCT (sizes 128, 256, 512, 1024, 2048, 4096)
- FFT (powers of 2 from 64 to 16384)
- DCT (sizes 4, 8, 16, 32, 64)
- LPC (orders 8, 16, 32)
- VQ search (codebooks 64, 256, 1024, 4096)
- Psychoacoustics (Bark, masking)
- Entropy coding (Huffman, Rice, range)
- Each codec's encode and decode loops
- Container mux and demux
- Pipeline throughput

**Where they live (2026-09-14):** the first seven bullet groups are
benched in the `tpt-dsp` repo (`tpt-dsp-core/benches/dsp_bench.rs`,
results in `tpt-dsp/BENCHMARKS.md`) — transforms and DSP primitives are
`tpt-dsp`'s responsibility (todo.md §3). The last three are benched in
this repo:

| crate | bench | what it measures |
| --- | --- | --- |
| `tpt-audio-cinder` | `benches/codec_bench.rs` | encode/decode of 64×512-sample blocks @ 48 kHz |
| `tpt-audio-aura` | `benches/codec_bench.rs` | lossless encode/decode of 32×1024-sample blocks @ 48 kHz |
| `tpt-audio-reed` | `benches/codec_bench.rs` | encode/decode of 64×320-sample (20 ms) frames @ 16 kHz |
| `tpt-video-thrum` | `benches/codec_bench.rs` | I-frame/P-frame encode + decode of 8-frame clips, 176×144 4:2:0 and 640×480 mono |
| `tpt-video-mote` | `benches/codec_bench.rs` | key/delta encode + decode of 8-frame fixed-camera clips at 320×240 (mono, 4:2:0) and 640×480 mono, with bandwidth and PSNR printed; dictionary training |
| `tpt-video-vault` | `benches/codec_bench.rs` | I-frame/P-frame encode + decode of 4-frame clips, 176×144 and 352×288 4:2:0; `tests/measure.rs` (`#[ignore]`d) prints the rate/quality/tool tables below |
| `tpt-container` | `benches/container_bench.rs` | mux + demux of a 200-packet stream |
| `tpt-media-bitstream` | `benches/bitstream_bench.rs` | bit I/O, Rice, range, Huffman at codec batch sizes |
| `tpt-media-pipeline` | `benches/pipeline_bench.rs` | `Source→map→Sink` drive over 1 000 packets |

Additionally, `tpt benchmark <in.wav> --codec <name>` (todo.md §43)
measures end-to-end encode/decode wall time and §37 quality (SNR +
log-spectral distance) on a real WAV, and reports a real-time factor.
`cargo bench` (release profile) is the source for the tables below; the
CLI's numbers from a debug build are ~40× slower and are not comparable.

### Measured results (2026-09-14; Reed added 2026-09-28; Thrum added 2026-09-28; Mote added 2026-09-30, own section below)

`cargo bench`, criterion (20 samples, 300 ms warm-up, 2 s measurement),
Windows x64 dev machine, release profile. Numbers are machine-relative —
compare only against other numbers from this table, and re-run on the
same machine when judging a change.

| bench | median | per-item | realtime factor |
| --- | ---: | ---: | ---: |
| `cinder/encode` (64×512 samples = 0.68 s audio) | 604 µs | 9.4 µs/block | ≥1000× |
| `cinder/decode` (64 packets) | 676 µs | 10.6 µs/packet | ≥1000× |
| `aura_lossless/encode` (32×1024 samples = 0.68 s audio) | 1.80 ms | 56 µs/block | ~380× |
| `aura_lossless/decode` (32 packets) | 2.37 ms | 74 µs/packet | ~290× |
| `reed/encode` (64×320 samples = 1.28 s audio, alternating voiced/noise) | 17.0 ms | 266 µs/frame | ~75× |
| `reed/decode` (64 packets) | 375 µs | 5.9 µs/packet | ~3400× |
| `tpt_container/mux` (200 packets) | 84 µs | 0.42 µs/packet | — |
| `tpt_container/demux` (200 packets) | 22 µs | 0.11 µs/packet | — |
| `bitstream/bit_write` (1024 ops) | 2.6 µs | 2.5 ns/op | — |
| `bitstream/bit_read` (1024 ops) | 3.9 µs | 3.9 ns/op | — |
| `bitstream/rice_encode` (256 values, k=2) | 232 µs | 0.9 µs/value | — |
| `bitstream/rice_decode` (256 values, k=2) | 587 µs | 2.3 µs/value | — |
| `bitstream/range_encode` (256 symbols, skewed) | 0.59 µs | 2.3 ns/symbol | — |
| `bitstream/range_decode` (256 symbols, peek+update) | 0.85 µs | 3.3 ns/symbol | — |
| `bitstream/huffman_encode` (512 symbols) | 2.4 µs | 4.7 ns/symbol | — |
| `bitstream/huffman_decode` (512 symbols) | 11.0 µs | 21 ns/symbol | — |
| `pipeline/drive` (1000 packets, source→map→sink) | 60 µs | 60 ns/packet | — |
| `thrum/qcif/encode_i_frame` (8 frames, 176×144 4:2:0) | 46.8 ms | 5.8 ms/frame | ~57× |
| `thrum/qcif/encode_p_frame` (8 frames) | 92.1 ms | 11.5 ms/frame | ~29× |
| `thrum/qcif/decode` (8 packets) | 15.1 ms | 1.9 ms/packet | ~530× |
| `thrum/vga/encode_i_frame` (8 frames, 640×480 mono) | 377 ms | 47.1 ms/frame | ~6.8× |
| `thrum/vga/encode_p_frame` (8 frames) | 959 ms | 120 ms/frame | ~2.7× |
| `thrum/vga/decode` (8 packets) | 84.3 ms | 10.5 ms/packet | ~95× |

Readings worth recording honestly:

* **Cinder clears its real-time frame budget by ~3 orders of magnitude**
  (encode+decode ≈ 20 µs against a 5.3 ms frame budget at 256/48 kHz) —
  consistent with §5.2's ≤10% target and the FFT-backed MDCT switch.
* **Aura lossless decode is ~4× more expensive than Cinder decode** —
  expected: LPC synthesis + Exp-Golomb per residual vs. band-quantized
  MDCT coefficients. Aura's §30 profile is archival, not real-time.
* **Reed encode is dominated by the unvoiced shape-gain codebook search**
  (1024 candidate excitations regenerated and correlated per noise
  frame; the 266 µs/frame median is over a half-voiced/half-noise bench
  input — pure voiced frames skip the search entirely). Decode is one
  codebook-entry regeneration + LPC synthesis per frame: 5.9 µs against
  a 20 ms frame budget. The encoder deliberately does *not* claim
  real-time safety (`Capabilities.real_time_encoder` is false); the
  decoder does, and `codecs/reed/tests/realtime_alloc.rs` pins its
  bounded allocation. `--quality` (Reed's LSF bit depth) trades envelope
  fidelity against packet size only — search cost is
  voicing-dependent, not quality-dependent.
* **Rice decode is ~5× slower than range decode here** — Rice's unary
  inner loop dominates for k=2-magnitude residuals; the range coder's
  peek+update is branch-free per symbol. If Rice ever becomes a hot path
  (Aura §30's unchecked "Rice coding" item), budget for a prefix-summed
  decode table.
* Huffman/Rice/Range relative standings match `tpt-dsp`'s entropy
  notes; nothing here contradicts them.
* **Thrum P-frame encoding is ~2× the cost of an I-frame**, and the
  exhaustive motion search is why: every macroblock costs
  `(2·search_range + 1)²` full-pixel SAD evaluations (289 at the default
  `search_range = 8`) on top of the intra path's own mode search. That
  is the deliberate trade in §32 — bounded work and no sub-pel
  interpolation, bought with encoder CPU. The **decoder is ~6× cheaper
  than the encoder** (1.9 ms vs 11.5 ms/frame at QCIF), which is the
  asymmetry that matters: only the decoder is claimed real-time
  (`Capabilities.real_time_encoder` is false, exactly as for Reed).
* **Thrum's latency figure is the P-frame encode + decode sum**, not
  any buffer or reorder delay: the encoder never holds a frame back and
  there is no reordering anywhere in the codec, so at QCIF one-way
  latency is ~13.4 ms against a 33.3 ms frame budget at 30 fps. Decode
  alone is ~1.9 ms. At 640×480 the P-frame encode alone (~120 ms) blows
  a 33.3 ms budget, so this configuration is offline/smaller-frame only
  until the search window shrinks — `search_range` is the knob, and the
  cost is quadratic in it.
* **Mean packet sizes** at the default `qp = 26`, from the bench's own
  synthetic clip (a panning diagonal ramp plus noise, 8 frames so ~1
  I-frame + 7 P-frames): 1 074 B/packet at 176×144 4:2:0 and 4 357 B at
  640×480 mono. These are *bench-input* rates, not a claimed bitrate
  target — a real scene, and especially a real talking head, codes
  considerably smaller, because the skip path and intra refresh both
  key on predicted residual being small.

### Mote (added 2026-09-30)

`cargo bench -p tpt-video-mote` (release profile, criterion, 10 samples,
300 ms warm-up, 3 s measurement). **Measured on a shared Windows x64 dev
machine while other agents were building concurrently, so treat every timing
as roughly ±2×**: two consecutive runs disagreed by up to 2× on the same
bench (both are shown where they differ). The size and PSNR figures are exact
and reproducible: the scene, training and encoder are all seeded and
integer-exact.

**The scene** is a fixed camera: a static textured background (a
site-specific seed), per-frame sensor noise of ±1 grey level, and a 32×32
object crossing the frame; 8 frames per pass (1 key + 7 delta), 4-iteration
dictionary training on frames earlier in the same site's stream. **The
bandwidth target is defined by this benchmark**: no figure below is a
guarantee for content with more motion, more noise, or a different site.

*Bandwidth and quality* (printed by the bench; `skip_mse` 4, key-frame
interval irrelevant to the delta rate):

| point | dictionary | key frame | vs raw | mean delta | delta-only rate | luma PSNR mean / min | held-out site (key) |
| ----- | ---------- | --------: | -----: | ---------: | --------------- | -------------------- | ------------------: |
| 320×240 mono | 4096 (12-bit) | 1 809 B | 42:1 | 46 B | 0.37 kbps @1 fps · 1.85 @5 · 5.55 @15 | 40.8 / 35.3 dB | 35.7 dB |
| 320×240 mono | 256 (8-bit) | 1 209 B | 64:1 | 37 B | 0.30 kbps @1 fps · 1.48 @5 · 4.44 @15 | 38.2 / 34.8 dB | 35.3 dB |
| 320×240 4:2:0 | 4096 (12-bit) | 2 709 B | 43:1 | 46 B | 0.37 kbps @1 fps · 1.85 @5 · 5.55 @15 | 40.9 / 35.3 dB | 35.8 dB |
| 640×480 mono | 4096 (12-bit) | 7 209 B | 43:1 | 57 B | 0.46 kbps @1 fps · 2.28 @5 · 6.84 @15 | 43.3 / 40.5 dB | 37.8 dB |

Dictionary packet (sent once, or provisioned): 262 152 B at 4096 entries,
16 392 B at 256. A key-frame-only stream is the no-temporal-prediction
baseline: 14.5 kbps at 1 fps for 320×240 mono, **57.7 kbps at 1 fps for
640×480 mono — over the 50 kbps design figure the old table quoted — so any
sub-50 kbps claim at 480p is carried entirely by temporal skip and a static
scene.** The in-scene PSNR flatters the 4096-entry rows (a 320×240 frame has
1 200 patches; training on ~4 800 of them nearly memorises the scene); the
held-out-site column is the fairer figure.

*CPU* (per frame; encode is not claimed real-time):

| bench | per frame (run 1 / run 2 where they differ) | notes |
| ----- | ------------------------------------------- | ----- |
| `320×240 mono d4096 / encode_key_frame` | 27 ms / 38 ms | one accelerated 4096-entry search per patch |
| `320×240 mono d4096 / encode_delta_frame` | 0.60 ms / 1.5 ms | skip test first; search only on changed patches |
| `320×240 mono d4096 / decode_delta_frame` | 11 µs | |
| `320×240 mono d4096 / decode_key_frame` | 47 µs per packet | includes constructing the decoder (256 KiB dictionary clone) |
| `320×240 mono d256 / encode_key_frame` | 5.3 ms / 6.2 ms | |
| `320×240 mono d256 / encode_delta_frame` | 3.4 ms / 2.7 ms | *slower* than d4096: a coarser dictionary leaves more patches above the skip threshold |
| `320×240 mono d256 / decode_delta_frame` | 16 µs / 18 µs | |
| `320×240 4:2:0 d4096 / encode_key_frame` | 63 ms | |
| `320×240 4:2:0 d4096 / encode_delta_frame` | 1.4 ms | |
| `320×240 4:2:0 d4096 / decode_delta_frame` | 28 µs | |
| `640×480 mono d4096 / encode_key_frame` | 192 ms / 215 ms | |
| `640×480 mono d4096 / encode_delta_frame` | 8.4 ms / 12.7 ms | |
| `640×480 mono d4096 / decode_delta_frame` | 62 µs | |
| `training (2 frames, 320×240 mono, 256 entries, 4 iterations)` | 47 ms | |

Full-size training (4 iterations, printed by the bench): 0.40 s for 320×240
mono / 4 frames, 0.98 s for 320×240 4:2:0, 3.54 s for 640×480 mono / 3
frames, all to 4096 entries. Read these as upper-bound-ish: training stops
early once the assignment is stable, which happens quickly when entries
approach the number of training patches.

Readings worth recording honestly:

* **Decode is a table lookup and a copy**: tens of microseconds per frame
  even at 640×480, against a 33 ms budget at 30 fps, dominated by the
  per-frame reference clone and output buffer rather than by the lookups.
* **Encode is asymmetric by two orders of magnitude between key and delta
  frames**: a key frame searches every patch (27–38 ms at QVGA, ~200 ms at
  480p), a delta frame searches only changed patches. A 30 fps stream is
  affordable on the encoder only for delta frames; the encoder is not
  claimed real-time.
* **Dictionary size is not monotone in encode cost**: 256 entries make key
  frames 5× cheaper but delta frames slower, because larger quantisation
  error leaves more patches above the skip threshold each frame.

*Memory* (`tests/memory.rs`, counting allocator; `cargo test --release -p
tpt-video-mote --test memory -- --nocapture`; 160×112 4:2:0, the full
4096-entry dictionary; exact figures depend on geometry, the dictionary term
does not):

| | constructed | resident after a stream | peak |
| - | ----------: | ----------------------: | ---: |
| decoder | 262 156 B (the dictionary) | 289 156 B | 53 920 B above resident, per pass with nothing retained |
| encoder | 1 311 951 B (dictionary + its `f32` search copy + packet buffer) | 1 343 047 B | 1 397 431 B |

One 160×112 4:2:0 frame is 26 880 B. The dictionary (256 KiB at 4096
entries) dominates the decoder; the encoder is ~5× that because it also holds
the dictionary as `f32` for the search. Both scale with frame size on top of
that: at 320×240 4:2:0 (115 200 B per frame) the decoder needs the
dictionary plus about three frames (reference, working copy, output).

### Vault (added 2026-09-30)

`cargo bench -p tpt-video-vault` (criterion, 10 samples, 300 ms warm-up,
5 s measurement, release profile, same Windows x64 dev machine). Each
iteration codes a 4-frame clip: `encode_i_frame` codes four intra frames
(`keyframe_interval = 0`), `encode_p_frame` codes one I-frame then three
P-frames at the default `max_refs = 4`, `encode_p_frame_refs1` is the same
with a single reference. The clip is a smooth gradient with a panning textured
block and 0–3 LSBs of noise, default `qp = 30`.

| bench | median | per frame |
| --- | ---: | ---: |
| `vault/qcif_176x144_yuv420/encode_i_frame` | 180 ms | 45 ms |
| `vault/qcif_176x144_yuv420/encode_p_frame` (1 I + 3 P) | 218 ms | 54 ms |
| `vault/qcif_176x144_yuv420/encode_p_frame_refs1` | 215 ms | 54 ms |
| `vault/qcif_176x144_yuv420/decode` | 7.4 ms | 1.9 ms |
| `vault/cif_352x288_yuv420/encode_i_frame` | 690 ms | 172 ms |
| `vault/cif_352x288_yuv420/encode_p_frame` (1 I + 3 P) | 712 ms | 178 ms |
| `vault/cif_352x288_yuv420/encode_p_frame_refs1` | 712 ms | 178 ms |
| `vault/cif_352x288_yuv420/decode` | 25.2 ms | 6.3 ms |

Mean packet size and luma PSNR over those clips: **202 B/packet at 37.11 dB
(QCIF), 498 B/packet at 37.87 dB (CIF)**. Machine caveat, measured: the
first run of this bench happened while other builds were running and came
out about twice as slow (QCIF `encode_i_frame` 285 ms, `decode` 14.4 ms);
the table above is the quieter run. The rates and PSNRs do not change between
runs (the codec is a pure function of its input); only the timings do.

**Where the encoder time goes** (profiled 2026-10-05, temporary phase
timers around the transform and motion-search entry points, run over
the same QCIF criterion clip): the transform (`dct_2d` + quantise +
`idct_2d` reconstruction, priced per candidate) is **97.2%** of
intra-frame encode time — 9 024 transform calls across a 4-frame
intra stream, 44.2 ms/frame — and **88.2%** of a P-frame stream,
where the motion search's 89 627 SAD evaluations cost **8.6%** and
candidate pricing sits at 89.2% (nested with the transform: pricing
is where the transforms are paid). This is the measured basis for
§33's "faster encoder" item: a fast transform path and fewer priced
candidates per leaf are the levers, in that order.

Everything below comes from `codecs/vault/tests/measure.rs`
(`cargo test --release -p tpt-video-vault --test measure -- --ignored
--nocapture --test-threads=1`): 8-frame 176×144 4:2:0 clips, one I-frame
then seven P-frames (`keyframe_interval` 60), `max_refs` 4 unless stated.
The clips are synthetic. Bytes are per frame averaged over the clip; PSNR is
mean luma PSNR. **These are properties of these clips, not general claims,
and no comparison with any other codec is made**: the clips are smooth and
highly predictable, and the repository has no real-video corpus.

Rate and quality against `qp` (clip "noisy" = 0–3 LSBs of noise, "clean" =
none); the last column is the *last frame's* census of leaf sizes
`[8, 16, 32, 64]`:

| `qp` | noisy B/frame | noisy PSNR | clean B/frame | clean PSNR | last-frame leaves (noisy) |
| ---: | ---: | ---: | ---: | ---: | --- |
| 14 | 798 | 45.45 dB | 573 | 52.27 dB | [52, 22, 8, 2] |
| 20 | 401 | 43.75 dB | 370 | 46.52 dB | [36, 26, 8, 2] |
| 26 | 243 | 39.97 dB | 194 | 41.23 dB | [12, 28, 9, 2] |
| 32 | 114 | 36.02 dB | 121 | 36.67 dB | [8, 25, 10, 2] |
| 38 | 67 | 32.78 dB | 66 | 33.12 dB | [0, 31, 9, 2] |
| 44 | 39 | 30.12 dB | 40 | 30.29 dB | [0, 19, 8, 3] |

Partition freedom (noisy clip, `qp` 26; `max_block_size` caps the largest
leaf the encoder may choose, ms/frame is encode + decode on the same busy-ish
machine, so machine-relative):

| `max_block_size` | B/frame | PSNR | ms/frame |
| ---: | ---: | ---: | ---: |
| 8 | 396 | 39.55 dB | 10 |
| 16 | 252 | 40.32 dB | 18 |
| 32 | 245 | 39.97 dB | 37 |
| 64 | 243 | 39.97 dB | 59 |

Allowing larger blocks cuts the rate by 39% against forcing 8×8 (396 → 243
B/frame) at essentially equal quality, and the cost is encoder time, which
grows roughly six-fold from 8×8 to 64×64. Almost all of the saving is
already there at 16×16; the 32→64 step buys 2 B/frame here. That is a fact
about this smooth clip.

Loop filter on/off (`loop_filter` per frame; PSNR against the *source*, luma):

| clip | `qp` | off | on |
| --- | ---: | --- | --- |
| panning, noisy | 26 | 221 B, 40.08 dB | 243 B, 39.97 dB |
| panning, noisy | 38 | 72 B, 32.85 dB | 67 B, 32.78 dB |
| panning, noisy | 46 | 36 B, 30.27 dB | 34 B, 29.92 dB |
| texture (interpolated noise) | 26 | 343 B, 38.04 dB | 342 B, 38.11 dB |
| texture | 38 | 110 B, 30.15 dB | 125 B, 30.59 dB |
| texture | 44 | 70 B, 26.07 dB | 71 B, 26.51 dB |

Recorded honestly: the filter improved PSNR on the textured clip
(+0.08 to +0.44 dB, at equal or somewhat higher rate — at `qp` 38 the rate
rose 14%) and was neutral-to-harmful on the smooth gradient clip
(−0.07 to −0.35 dB, rate +10% at `qp` 26). PSNR is not a measure of
blockiness, so this does not say the filter is useless visually, only that it
is not a free win on these clips. It is on by default, and an earlier
`d < beta` averaging variant was measurably worse (+24% rate at `qp` 26 on
the smooth clip), which is why it was replaced by the H.263 Annex J form.

References (panning noisy clip, default `qp`): `max_refs` 1, 4, 16 gave 126,
132 and 129 B/frame at 36.64, 36.98 and 36.83 dB. **A pan gains nothing from
extra references**, as expected; the value of references is on content that
revisits earlier frames, which the census test
`multiple_references_are_used_and_pay_off` covers (an A/B/A/B cut costs
far fewer bytes with 4 references than with 1). Encode time barely moved
with the reference count on this clip (55, 56, 56 ms/frame, and the bench's
`encode_p_frame` versus `encode_p_frame_refs1` are within noise); why was not
investigated, and a 4- or 8-frame clip holds few references yet, so this is not
evidence about the cost of 16 references on a long stream.

Readings worth recording:

* **Vault encodes about 5× slower than Thrum** (QCIF, 1 I + 3 P: 54 ms per
  frame, against Thrum's 11.5 ms per frame for its P-frame bench) and its
  **decoder costs about the same order** as Thrum's (1.9 ms/frame at QCIF
  in both tables, measured on different runs). That is the intended
  asymmetry for an archival codec; neither side is claimed real-time
  (`Capabilities` is lossy only).
* **A P-frame costs about 28% more than an I-frame**, derived from the
  bench: (218 ms − 45 ms) / 3 ≈ 58 ms per P-frame against 45 ms per intra
  frame at QCIF. In a P-frame the encoder prices intra, inter and skip at
  every partition level, so the P path is the intra path plus the inter
  candidate.
* **CIF costs about 4× QCIF** (172 ms vs 45 ms per intra frame): the
  encoder scales with picture area. It has not been measured above CIF.
* **Decode is cheap next to encode** (about 24× at QCIF: 45 ms per intra-frame encode vs 1.9 ms mean per
  decoded packet), since the decoder runs one prediction and one inverse
  transform per leaf and none of the search.

### Allocations

The per-frame allocation *counts* for Cinder's hot path are asserted
bounded by `codecs/cinder/tests/realtime_alloc.rs` (a counting global
allocator over 31 steady-state encode+decode passes, budget 512 KiB —
~25× the measured working set), and Reed's by
`codecs/reed/tests/realtime_alloc.rs` (same harness; 32 steady-state
passes over alternating voiced/noise frames, same 512 KiB bound — the
codebook search regenerates entries into one fixed scratch frame rather
than allocating per candidate). Thrum's is
`codecs/thrum/tests/realtime_alloc.rs` (same harness, 24 steady-state
encode+decode passes at 176×144 4:2:0, 4 MiB bound — deliberately looser
than the audio budgets because a *video* decoder must return a whole
frame buffer per call, which is inherently ~1.5 bytes per pixel; the
property guarded is still order-of-magnitude stability, not a tight
ceiling). That test is what caught the encoder retaining its
worst-case packet capacity (2.5 MB per packet for a 176×144 frame,
59 MB over 24 packets) instead of the few kilobytes actually written;
the initial `shrink_to_fit` fix was itself superseded — the encoder now
allocates that worst-case buffer once at construction and reuses it for
every frame, emitting an exact-size copy per packet, so the steady-state
path does not allocate it at all.

*Peak* working sets — the high-water mark of live heap bytes, which is
the number a memory budget is stated in — are measured by
`apps/tpt-cli/tests/memory_budget.rs` through `tpt-cli`'s `memprobe`
harness (a `TrackingAlloc` global allocator whose `MemoryProbe` windows
report live-heap peaks and cumulative churn). It measures Cinder, Aura,
Reed, and Thrum in a steady-state streaming pass where nothing is
retained, asserts each against the budget table below, and asserts
Thrum's working set is independent of clip length (8× the input does
not cost 8× the memory). Results: see the "Measured" column below.
The container, pipeline, and probe layers are not yet measured by that
harness; that remains `criterion`-profiler follow-up work.

## SIMD Equivalence (todo.md §40)

All optimized kernels in this ecosystem live in `tpt-dsp` (§3), so the
equivalence machinery lives there too — this section records where it
stands rather than duplicating it:

- A scalar reference (`simd_scalar.rs`) backs every `core::simd` kernel.
- Equivalence tests (`simd_mul_matches_scalar`, `simd_add_matches_scalar`,
  `simd_magnitude_matches_scalar`, `simd_fft_matches_scalar_fft`)
  compare the paths on identical inputs.
- Tolerances are documented per kernel family in each module's
  `# Numerical contract` section (`tpt-dsp` §12), e.g. ~1e-3/1e-4 in
  `f32` for the FFT-backed transforms.
- Randomized testing beyond the fixed equivalence vectors is the main
  remaining gap (the §40 item stays honest); `tpt-media`'s own
  seeded-random property tests (§38) exercise the *composed* scalar
  paths end-to-end.
- Architecture-specific CI (x86_64 + AArch64 matrix) is likewise still
  open; today's CI runs x86_64 Linux/Windows only.

## Determinism (todo.md §39)

* **Decoding is deterministic.** Both native codecs are pure functions
  of (packet bytes, config): no RNG, no wall-clock, no thread state.
  Pinned by `encoding_and_decoding_are_deterministic` in
  `codecs/{cinder,aura}/tests/property.rs` (byte-identical packets and
  decodes from independent encoder/decoder instances).
* **Container generation is deterministic.** The muxer writes no
  timestamps, counters, or padding entropy; the same packet stream
  produces byte-identical files (`muxing_is_deterministic`,
  `containers/tpt-container/tests/property.rs`).
* **Scalar DSP is deterministic** by construction in `tpt-dsp` (pure
  functions over fixed iteration orders); the codec-level determinism
  tests above pin that end-to-end at the media layer.
* **SIMD differences** are build-time, not runtime: `core::simd`
  kernels dispatch by `target-feature` at compile time (see
  `tpt-dsp/ARCHITECTURE.md` §10 for the support matrix and why runtime
  detection is deliberately absent). Same-binary results are
  deterministic; results *across* different `target-cpu` builds may
  differ within the documented per-kernel tolerances.
* **The §36 corpus is deterministic.** `apps/tpt-cli/src/signals.rs`
  generates every signal from closed-form math plus SplitMix64 — same
  seed, same bytes, no committed binary fixtures needed.

## Reproducibility

Benchmarks are reproducible when run with:

- The same `rustc` version (CI records this).
- The same CPU (CI uses a pinned runner).
- The same `RUSTFLAGS` and `cargo` flags.
- The same input data (committed to `tests/corpus/`).
- The same criterion invocation: the committed benches fix
  `sample_size(20)`, `warm_up_time(300ms)`, `measurement_time(2s)` in
  code, so plain `cargo bench` reproduces the configuration — no flags
  to remember.

We do not claim absolute cross-CPU reproducibility for SIMD paths;
results are tagged with the target architecture.

## Memory Budgets

Default memory budgets per codec, with the measured steady-state
streaming working set next to each (see "Allocations" above for how the
measurement is taken):

| Codec | Per-frame scratch | Reference frames | Total working set | Measured peak |
| ----- | ----------------- | ---------------- | ----------------- | ------------- |
| Cinder | 4 KiB            | 0                | < 32 KiB          | 6 645 B       |
| Aura   | 64 KiB           | 0                | < 256 KiB         | 25 330 B      |
| Reed   | 1 KiB            | 0                | < 16 KiB          | 8 140 B       |
| Thrum  | 64 KiB           | 1                | < 1 MiB           | 152 503 B (length-independent: 152 200 B over 6 frames vs 152 503 B over 48) |
| Vault  | 4 MiB            | 16               | < 128 MiB         | 482 784 B at 176×144 4:2:0 with 4 references (encoder + decoder together, `codecs/vault/tests/memory.rs`; identical over 6 and 10 frames, 483 088 B over 24); 253 536 B with 1 reference; 1 402 096 B with 16 references over 24 frames — about 76.4–76.6 KB per extra reference, i.e. two 38 016 B frames (one in each store). Not pinned by `tpt-cli`'s budget test; the budget column is a ceiling that has only been measured at QCIF |
| Mote   | frame-sized (26 880 B at 160×112 4:2:0) | 1 | decoder < 1 MiB at ≤ QVGA 4:2:0 (256 KiB dictionary + ~3 frames); encoder < 2 MiB (adds the `f32` search copy) | decoder 289 156 B resident + 53 920 B pass peak; encoder 1 343 047 B resident, 1 397 431 B peak (160×112 4:2:0, 4096 entries; `codecs/mote/tests/memory.rs`) |

The budget column is the ceiling each codec's streaming working set must
stay under; the measured column is what the current implementation
actually peaks at, over a steady-state encode+decode pass in which
nothing is retained (`apps/tpt-cli/tests/memory_budget.rs`, x86_64
Windows, `cargo test`'s unoptimized test profile — the numbers are
allocation-structure-dependent, not timing-dependent, so they are
stable across runs; platform struct-size differences can move them
slightly). The same test pins every budget row above to its documented
value, so the table cannot silently drift away from the code.

Two numbers moved because of the measurement, both worth recording:

* Reed's budget was originally "< 8 KiB"; the first measurement came in
  at 8 140 B — 99% of the ceiling, passing by 52 bytes. A budget the
  implementation fills is a flake in waiting, so it was raised to the
  next sane power of two rather than kept as a false precision point.
* Thrum's encoder initially allocated its worst-case packet buffer
  (~2.4 MB for a 176×144 frame) *per frame*; the buffer is now allocated
  once and reused, which is why the measured peak (~149 KiB, one frame
  plus one reference plus the reused scratch) is two orders of
  magnitude below what the per-frame allocation would have produced.

Cumulative per-frame *churn* is a separate figure from the peak and is
deliberately not budgeted to zero: Thrum's measured churn is
~263 KB/frame (exact-size payload copy + the decoded frame the decoder
must hand back), bounded by `thrum_per_frame_churn_is_bounded` to
proportionality with a frame rather than to nothing.

## Browser player (`tpt-web`, todo.md §45)

Two measurements of the same operation on byte-identical inputs: push
16 KiB slices into a `Player`, parse the container, decode with the real
codec, convert to planar `f32` / RGBA8, pop everything. Median of 15 runs
after 2 warm-up runs, whole-file.

- **Native**: `cargo bench -p tpt-web` (`benches/decode_throughput.rs`,
  bench profile = release + debug info).
- **WebAssembly**: `node apps/tpt-web/web/build.mjs`, then
  `cargo run -p tpt-web --example make_demo -- --bench-dir DIR`, then
  `node apps/tpt-web/web/bench.mjs DIR` (`web/bench.mjs`). This is the
  `wasm32-unknown-unknown` release build running in Node 22.18 / V8 12.4 —
  **not a browser tab**: no WebAudio, no canvas, no main-thread contention.
  A browser is this number plus the page's cost of presenting the output.

Machine: Intel Core i7-12700 (20 logical CPUs, hybrid), Windows 11 x64,
rustc 1.97.1. The machine was shared with other builds while measuring;
audio results were stable across three runs, video results were not (see the
ranges), so video numbers are given as the range of medians over three runs.

| input | file | native median | wasm median | native realtime | wasm realtime |
| --- | ---: | ---: | ---: | ---: | ---: |
| cinder 48 kHz mono, 30 s | 964 KiB | 18.1-19.3 ms | 25.8-27.1 ms | 1550-1650x | 1110-1160x |
| aura lossless 48 kHz mono, 10 s | 1156 KiB | 16.1-17.7 ms | 30.9-32.6 ms | 565-620x | 307-324x |
| reed 16 kHz mono, 30 s | 87 KiB | 5.3-5.8 ms | 6.9-7.4 ms | 5160-5630x | 4070-4380x |
| thrum 176x144 15 fps, 60 frames | 9.1 KiB | 19.9-20.5 ms | 22.3-53.6 ms | 195-207x (2920-3110 fps) | 75-179x (1120-2690 fps) |
| thrum 640x352 15 fps, 30 frames | 16.7 KiB | 91.6-219 ms | 97.2-101.6 ms | 9-22x (137-328 fps) | 20-21x (295-309 fps) |
| demo.tpt: cinder + thrum 160x96, 4 s | 137 KiB | 15.4-37.0 ms | 19.2-20.0 ms | 108-259x | 200-208x |

What the numbers say, and what they do not:

- **Audio**: wasm runs at roughly 1.4x (Cinder), 1.9x (Aura) and 1.3x
  (Reed) the native time, and is still 300x-4000x faster than real time. A
  browser will spend far more time in WebAudio scheduling than in decoding.
- **Video**: real-time budget at 30 fps is 33 ms per frame; the measured
  640x352 decode-and-convert is 3-7 ms per frame native and about 3.3 ms in
  wasm, and QCIF well under 1 ms. The native video range is wide because of
  machine noise (the same binary measured 91.6 ms and 219 ms for the
  640x352 file in different runs), so no native-vs-wasm ratio is claimed for
  video.
- **The video fixtures are synthetic and extremely compressible** (a
  gradient with one moving square: 9 KiB for 60 QCIF frames), so their
  "input MB/s" is near zero and says nothing about real footage. They
  measure per-frame reconstruction and YUV-to-RGBA conversion, which is
  real; they do not measure entropy-decoding load, which is understated.
- Nothing here measures a browser, a Worker, low-end phones, or WebAudio.
  Those numbers do not exist yet.

Memory (player-owned, from `web/check.mjs` on the 4 s demo): 196 736 B held
by the player at peak; wasm linear memory 1.8 MiB after playback and 7.5 MiB
after the benchmark inputs (including 640x352 video); module 328 KiB
(`wasm32-unknown-unknown` release, symbols stripped). The documented worst
case under default limits is 223 MiB (`docs/web-design.md` section 7).

## Profiling

- `cargo flamegraph` for coarse profiling.
- `perf stat` for hardware counters on Linux.
- `Instruments` on macOS.
- `VTune` on Windows / Linux.

A profiling recipe for each crate with benchmarks is recorded in its own
README (`benches/` names the harness, and the recipe says which tool to point
at it); the canonical measured numbers are the tables above.