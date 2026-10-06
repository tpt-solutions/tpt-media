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

Three video codecs are implemented (Thrum, todo.md §32; Vault, §33; Mote,
§34); they are what the representation research (§56,
[`representation-research.md`](representation-research.md)) is to be
measured against.

| Codec     | Niche                    | Key technique             | Resolution | Bitrate target | Status |
| --------- | ------------------------ | ------------------------- | ---------- | -------------- | ------ |
| Thrum  | Real-time communication  | I/P + intra-refresh       | up to 1080p | adaptive      | implemented (v0.1, 2026-09-28) |
| Vault     | High-compression archival | Superblocks + RDO         | multiples of 8 (measured at QCIF/CIF) | offline       | implemented (v0.1, 2026-09-30) |
| Mote   | Edge / IoT               | Dictionary VQ + temporal skip | ≤480p   | benchmark-defined (see below) | implemented (v0.1, 2026-09-30) |

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

### Mote v0.1 — implemented design (2026-09-30)

`codecs/mote/src/`. The design goal is *a decoder a microcontroller-class
device can afford, on a feed that barely changes*, so the codec is a
dictionary lookup plus a change mask.

- **Supported dimensions** (todo.md §34): `Gray8` or planar `Yuv420`;
  width and height non-zero multiples of 16; long edge ≤ 848, short edge
  ≤ 480 (480p is 848×480 rounded down to a multiple of 16; portrait is
  allowed). Multiples of 16 make every plane, including 4:2:0 chroma,
  tile exactly into 8×8 patches, so there is no padding path.
- **Patch representation.** An 8×8 block of 8-bit samples from one plane
  (VQ dimension 64), numbered plane by plane (Y, U, V), raster order.
  Chroma patches are ordinary patches: all planes share one dictionary.
- **Dictionary.** 1–4096 entries of 64 bytes, stored raw. The index width
  is `ceil(log2(entries))` bits, 12 at the 4096-entry target, so a key
  frame is exactly `patches × 12` bits plus a 34-bit header. The
  dictionary is *stream state*: trained offline on the deployment scene
  and provisioned out of band, or sent once as a dictionary packet
  (`type 0`, `entries × 64` bytes, byte-aligned). Every frame packet
  carries the dictionary's 16-bit id (low 16 bits of the CRC-32 of the
  entry count and entries), so a decoder holding a different dictionary
  refuses the stream (`Unsupported`) instead of rendering garbage. A
  decoder without a dictionary answers `NeedMoreInput`.
- **VQ.** Nearest entry under squared error via
  `tpt_dsp_core::nearest_vector_accelerated` (todo.md §8). Samples are
  integers, so every distance is an exact integer below 2²⁴ in `f32`:
  the search is bit-identical on every target.
- **Training** (`Dictionary::train_patches`): seeded partial
  Fisher–Yates initialisation from distinct training patches
  (SplitMix64), assignment with lowest-index tie-break, centroid update to
  the *rounded integer mean* in pure integer arithmetic, empty clusters
  re-seated on the farthest patch (ascending centroid order, first patch
  wins ties), stop on a stable assignment or the iteration cap. Optional
  strided subsample above `max_vectors`. No `HashMap`, no clock. Rounding
  centroids each iteration costs a little against real-valued Lloyd and
  buys exactness.
- **Reconstruction model.** The decoded picture is the concatenation of
  dictionary entries; there is no post-filter (a deblocking pass would be
  a deliberate later addition, and would extend the provenance table). A
  skipped patch keeps the previous decoded patch bit for bit. The encoder
  builds its reference with the same copies, so it cannot drift from the
  decoder; the test suite asserts decoder output equals the encoder's own
  reconstruction on every frame it checks.
- **Frame packets.** *Key* (`type 1`): `dict_id`, then one index per patch.
  *Delta* (`type 2`): `dict_id`, a 3-bit Rice parameter `k`, a 16-bit
  count of coded patches, then per coded patch a Rice-coded skip run and
  an index; trailing skips are implicit. The encoder picks the `k` in
  `0..=7` minimising the frame's run cost. An unchanged frame is a
  53-bit header: 11 bytes with the CRC.
- **Provable bound.** `MoteConfig::max_packet_bits`: key is exactly
  `34 + T·b` bits; delta is `53 + T·(run + b)` where a run is at most
  `T−1` and the chosen `k` never costs more than `k = 7`. The encoder's
  reusable buffer is sized by it and a test drives noise against
  2/3/100/4096-entry dictionaries to confirm no packet exceeds it.
- **Hostile input.** CRC first; then version (`Unsupported`), type
  (`3` is `Malformed`), and every count against what the packet can
  hold *before* anything is allocated (a dictionary packet claiming 4096
  entries with 100 bytes is rejected without allocating 256 KiB). Loops
  are bounded by the patch count; the unary run prefix is capped by the
  frame's patch count; every index is checked against the dictionary
  length (which matters when the entry count is not a power of two);
  state is committed only after a packet decodes fully, so a rejected
  packet never corrupts the reference. `ResourceLimits` packet size,
  frame bytes, allocation and reference frames are all honoured.
- **Quality metric.** Luma PSNR primary, all-plane PSNR and mean absolute
  error secondary (`src/quality.rs`). A fidelity yardstick only.

**Temporal prediction — investigation.** The question was whether a
dictionary codec should predict across frames, and in what form. Built
and measured: *conditional replenishment* (a delta frame re-codes a patch
only if the previous reconstruction's error exceeds `skip_mse` **and**
exceeds the best dictionary entry's error; coding a patch that is no
better than the copy would spend bits for nothing). Measured by
`codecs/mote/tests/temporal.rs` (320×240 mono, 192-entry dictionary,
fixed camera with ±1 sensor noise and a 24×24 object; mean over 11
frames after the first key frame; sizes are deterministic):

| object | mode | bytes/frame | luma PSNR |
| ------ | ---- | ----------: | --------: |
| still  | independent (temporal off) | 1 209 | 40.1 dB |
| still  | replenishment, `skip_mse` 0 / 2 / 4 | 37 | 40.1 dB |
| still  | `skip_mse` 16 | 13 | 40.1 dB |
| still  | `skip_mse` 64 | 11 | 40.1 dB |
| moving | independent | 1 209 | 38.6 dB |
| moving | `skip_mse` 0 / 2 / 4 | 52 / 52 / 51 | 38.6 dB |
| moving | `skip_mse` 16 | 29 | 38.6 dB |
| moving | `skip_mse` 64 | 27 | 38.6 dB |

Findings, stated plainly: replenishment is worth roughly 25–100× on
these scenes, and a looser threshold monotonically lowers the rate. The
PSNR column not moving is a limit of the test scene, not a result: the
noise is ±1 grey level, so dictionary error dominates skip error; a
noisier sensor would separate the rows. **Not investigated:**
motion-compensated candidates (a shifted previous-frame patch), and
context/delta coding of the index stream. A moving object currently costs
its patches twice (arrival and departure), which is why the moving rows
are larger than the still rows. Neither is built; both remain open.

Known sharp edges, stated plainly:

- **The dictionary is not free.** 4096 entries × 64 B = 256 KiB, sent once
  or provisioned. The bandwidth figures exclude it; at 0.37 kbps it is
  ~1.6 hours of steady-state stream. Entries are stored raw.
- **A 4096-entry dictionary trained on the frames it is tested on
  flatters the codec**: a 320×240 frame only has 1 200 patches. The
  benchmark also prints a held-out-site key-frame PSNR
  (35.3–37.8 dB versus 38–43 dB in-scene).
- **No rate control.** Rate is set by scene change, `skip_mse` and
  dictionary size, not by a target.
- **The encoder is not real-time** (`real_time_encoder` is false): a key
  frame is one accelerated 4096-entry search per patch.
- **Wired into the `tpt-web` player** (2026-10-05, with Vault: both took
  the §8 route — dependency + `DecoderTable::native()` line + a builder
  reconstructing the decoder from the track). It *is* driven by `tpt-cli`:
  `--codec mote` works for `encode`, `decode`, `transcode` and `benchmark`
  through the y4m bridge, it is in the probe capability table, and
  `tpt video-corpus --codec mote` measures it over the §36 corpus. The
  measured cross-codec figures are in `todo.md` §33 — on a static-camera
  codec like this the honest summary is that its rate is dominated by the
  one-off dictionary packet and its quality collapses on high motion, which
  is what it was built for.
- Tree/product quantisation for larger dictionaries (todo.md §8) is not
  implemented; the search is the accelerated linear scan.

### Vault v0.1 — implemented design (2026-09-30)

`codecs/vault/src/`. The design goal is *bits per unit of quality*, so the
encoder spends work freely and the decoder stays simple and bounded. Sources
are in [`provenance.md`](provenance.md); measured numbers in
[`performance.md`](performance.md).

**Partitioning.** The picture is tiled with 64×64 superblocks in raster
order. Each is a quadtree: a node is one *leaf* (one prediction, one
residual) or four children in z-order, down to a minimum of 8×8, so the
recursion is 64→32→16→8 — structurally three levels below the root
(`MAX_PARTITION_DEPTH`). A 1-bit `split` flag is sent for a node that lies
entirely inside the picture and is larger than 8×8; a node that overhangs
the edge splits implicitly with no flag. Since width and height are
multiples of 8, an 8×8 node is always inside and no padding exists. The
encoder may cap the leaf size (`max_block_size`, encoder-only: forced splits
are still signalled with flags, so a stream stays decodable whatever the
cap was).

**Prediction.** A leaf is *skip*, *inter* or *intra* (skip and intra flags
appear in P-frames only; an I-frame is all intra).

- *Skip*: reference 0, the predicted motion vector, no residual.
- *Inter*: reference index (`ceil(log2(num_refs))` bits, none for a single
  reference), a motion-vector difference against the predictor (left
  neighbour's vector if inter, else the top neighbour's, else zero; both are
  always earlier in z-order), coded as signed Rice at `k = 1`. Vectors are in
  **half-pel** units, `|v| ≤ 65`. Prediction is a 2-tap average at a half-pel
  offset and a 4-sample average at the diagonal position, edge-clamped so any
  vector is safe. Chroma vectors are the luma vector halved (floor), in
  half-chroma-pel units, never signalled.
- *Intra*: 2-bit mode — DC, vertical, horizontal, and *plane* (`left[y] +
  top[x] − top_left`, clamped) — with the decoder degrading any mode the
  geometry cannot support to DC. Prediction reads only the row above and the
  column to the left, which are always reconstructed earlier in z-order.
  Chroma reuses the luma mode.

**References.** Up to 16 (`MAX_REFERENCE_FRAMES`, 5 bits on the wire): the
last `max_refs` loop-filtered reconstructions, most recent first. An I-frame
clears the store. The decoder rejects a frame that claims more references
than it holds with `NeedMoreInput` (a stream joined mid-way or after loss),
and more than `limits.max_reference_frames` with a resource-limit error.
Default `max_refs` is 4 so a decoder under the strict default limits (8)
works out of the box; 16 needs a raised limit.

**Transform and quantisation.** `dct_2d`/`idct_2d` from `tpt-dsp-core` at
4×4 (chroma of an 8×8 leaf) up to 32×32; a 64×64 luma leaf is four 32×32
tiles (chroma of a 64×64 leaf is one 32×32). Coefficients are converted to
**orthonormal units** before quantisation, where Parseval holds, so one step
means the same per-coefficient distortion at every transform size and `D` is
comparable across partition choices. One flat dead-zone quantiser, step from
the same 6-bit `qp` table as Thrum, rounding offset 0.4, level clamped to
±8192. That clamp is derivable (a residual sample is in [−255, 255], so an
orthonormal coefficient is at most 255·32 = 8160, and 8160 / 1.122 < 7273)
and it is what makes the packet bound a proof.

**Entropy coding.** Per transform tile: 1 `coded` bit; if coded, 3 bits of
Rice parameter `k` (0–7, chosen to minimise that tile), `log2(t²)` bits of
`last` (highest scan position with a non-zero level; it is its own
terminator), then per non-zero level in diagonal-scan order an *unsigned*
Rice run and a *signed* Rice level. Static codes only, no context modelling.
Runs use an unsigned form of the same Rice code because the signed form in
`tpt-media-bitstream` would waste a bit per run.

**Loop filter.** The H.263 Annex J deblocking filter (`d = (A − 4B + 4C −
D)/8`, an up-down ramp limiting `d` by strength `S`, and a secondary
correction of the outer samples). `S` grows with the quantiser step
(1–12); an intra edge uses the full `S`, an inter edge two thirds. Edges are
only real coding-structure edges: leaf-block boundaries plus the 32×32 tile
seams inside a 64×64 luma leaf. An edge between two skip/no-residual units
with the same reference and motion within a pel is not filtered. The whole
picture is filtered after reconstruction, and the filtered picture is the
next reference. It is enabled by a per-frame header bit.

**Rate–distortion optimisation.** For each node the encoder compares
`J = D + λR` between coding it as a leaf and splitting it, with
`λ = LAMBDA_SCALE · step²` (`LAMBDA_SCALE = 0.15`, empirical and untuned):

- `D` is the SSE (luma + chroma) between source and the reconstruction the
  decoder will produce — the real quantiser is inside the objective.
- `R` is the *exact* bit count: the same generic writer that emits the
  packet is run against a bit counter, so the price cannot drift from the
  syntax.
- The leaf is the cheapest of three candidates: intra (best mode by SAD),
  inter (best reference and vector by SAD plus a vector-cost term — a
  full-pel shrinking-step 8-neighbour pattern search around the zero and
  predicted vectors, then half-pel refinement), and skip. Ties resolve skip
  over inter over intra.
- A node whose best leaf codes no coefficient and whose distortion is within
  0.10·step² per sample is not split (early termination).

This is **not** a global optimum and is not claimed to be. Candidate
generation is heuristic and pruned; the motion search is a pattern search
that can settle in a local minimum; there is no coefficient-level RDO
(RDOQ) or trellis quantisation; decisions ignore their effect on later
blocks' vector predictors and intra neighbours; and `λ` is not tuned. What
holds, and is tested, is narrower: each decision is the cheapest among the
alternatives that were priced, and freeing the partition never does worse
on `J` than forcing 8×8 on the test frames.

**Wire format (version 2).** v2 (2026-10-05) replaces v1's 1-bit
`intra` flag with a 2-bit frame type and adds a 16-bit per-frame
`poc` — the groundwork for B-frames ("Vault B-frames — design for
v0.2" below); v1 streams are rejected fail-fast. The v2 layout:

| field | bits | meaning |
| ----- | ---- | ------- |
| `version` | 16 | bitstream version, `2`; rejected up front if not |
| `ftype` | 2 | `0` = intra, `1` = predictive, `2` = bi-predictive (reserved for v0.2; `3` rejected as malformed) |
| `qp` | 6 | `1..=51` |
| `num_refs` | 5 | 0 iff intra; else `1..=16`, at most the decoder's `max_refs` and the references it holds |
| `loop_filter` | 1 | run the loop filter |
| `poc` | 16 | display position within the current GOP (sequential today; the B-frame reordering key in v0.2) |
| payload | — | superblock partition tree + per-leaf syntax |
| `crc32` | 32 | IEEE 802.3 CRC-32 (little-endian) over all preceding bytes, verified before parsing |

The v1 layout differed only in the 1-bit `intra` flag and the absent
`poc`.

| field | bits | meaning |
| ----- | ---- | ------- |
| `version` | 16 | rejected up front if not 1 |
| `intra` | 1 | I-frame (clears the reference store) |
| `qp` | 6 | `1..=51` |
| `num_refs` | 5 | 0 iff intra; else `1..=16`, at most the decoder's `max_refs` and the references it holds |
| `loop_filter` | 1 | run the loop filter |
| superblocks | — | raster order; per node `split` flag, per leaf: skip/intra flags (P only), ref index, mvd or intra mode, coefficient tiles (Y, then U, V) |
| `crc32` | 32 | IEEE 802.3, little-endian, over all preceding bytes, verified before parsing |

`VaultConfig::max_packet_bits` derives the worst case: 2 bits of split flags,
2 of skip/intra, 4 of reference, two mvds of at most `2·MV_LIMIT`, 2 of mode
and the tile headers per 8×8 unit (larger blocks have strictly fewer headers
per area), plus one run and one level token per sample, each at most
`MAX_RICE_BITS = 136` (the encoder's cheapest `k` never exceeds the cost at
`k = 7`). The encoder checks each finished packet against it.

Known sharp edges, stated plainly:

- **Slow encoder, neither side real-time.** Every partition level prices an
  intra, an inter and a skip candidate with a real transform, and the
  `tpt-dsp-core` DCT is a direct `O(n³)` separable one. About 45-55 ms/frame at
  QCIF and 170-180 ms at CIF in release on the development machine (twice that under load).
- **No B-frames, weighted prediction, long-term references, reference
  reordering, rate control, scene-cut detection or per-block `qp`.** The
  reference list is simply the last N pictures.
- **The loop filter is not RD-decided.** It is a per-frame flag the encoder
  sets from configuration. Measured on synthetic clips it helped textured
  content and was neutral-to-harmful on smooth gradients (numbers in
  `performance.md`); it is on by default.
- **Vector predictor is left-else-top**, not a median, and ignores whether
  the neighbour used a different reference.
- **Intra is minimal**: four modes, no top-right samples, no angular modes,
  whole-block prediction even at 64×64.
- **Closed-loop exactness assumes the same build and target on both sides**;
  the transform is `f32` and the workspace makes no cross-target bit-exactness
  claim (todo.md §39).
- **Not wired into the `tpt-web` player.** It *is* driven by `tpt-cli`:
  `--codec vault` works for `encode`, `decode`, `transcode` and `benchmark`
  through the y4m bridge, it is in the probe capability table, and
  `tpt video-corpus --codec vault` measures it over the §36 corpus. Its
  quality over that corpus is now measured (`todo.md` §33): uniformly higher
  PSNR than Thrum at the same `qp`, and on the predictable scenes also
  *smaller* (117 B vs 390 B at camera-motion). Encode CPU — the 2026-09-30
  direct-transform build spent 17-104 ms/frame and missed the 33.3 ms
  frame budget on every scene but two; since 2026-10-05 the tile transform
  runs on `tpt-dsp-core`'s FFT-backed `FastDct2dPlan` and every scene fits
  the budget (2.9-22.2 ms/frame). Those are synthetic clips at a shared
  `qp`, so they support no cross-codec compression claim. Rate is no
  longer fixed either: `target_bits_per_frame` drives a bounded
  per-frame `qp` search against a budget bucket
  (`tpt encode --codec vault --bitrate`), measured at a 60 kbit/s
  target decoding to 61.7 kbit/s measured.

### Vault B-frames — implemented v0.2 (design written 2026-10-05, coded 2026-10-05)

The design below is what a B-frame implementation must change, worked
through so the next session starts from a plan. It is deliberately
conservative: bi-prediction by plain averaging, no weighted prediction,
no long-term references — those stay in the bullet above for v0.3+.

**Wire format (the v2 bump, shipped).** The v1 1-bit `intra` flag
became a 2-bit frame type (`I` / `P` / `B`, `3` reserved-rejected) and
every frame gained a 16-bit `poc` — see "Wire format (version 2)"
above. A B-frame leaf carries four Rice-coded motion-vector deltas
(past pair, then future pair) against a (0, 0) predictor; the
references are the fixed pair (past anchor, future reference), so no
per-leaf reference index is spent. Combine mode is plain averaging
(weighted prediction is v0.3+). Display order is recovered from the
explicit `poc` — "derive it from PTS" would couple the codec to
container behaviour that `.tpt` does not promise.

**Encoder.** A lookahead window of `b_frames` frames (config, default
2, encoder-only): frames are coded I/P/B/P/B... with the next P (or I)
coded *before* the B-frames between them, so its reconstruction is a
legal future reference. Each B leaf prices forward-only, backward-only
and bi-directional candidates with the existing `J = D + λ·R` — the
RDO machinery needs no new concept, only a second reference list.
B-frames never enter the reference store. Rate control composes
unchanged: the bucket budgets each frame at its measured cost, and the
4× intra allowance already handles the I-frame spike.

**Decoder.** Reordering: packets now arrive in *decode* order, and the
decoder must hold B-frames until their future reference decodes, then
emit in display order. That means a bounded reorder buffer (at most
`b_frames + 1` pictures — `max_packet_bits`-style worst-case argument
extends), an explicit end-of-stream flush that releases held frames
(the decoder's existing `drain`), and one decision for consumers:
`decode()` returns *display-order* frames with gaps the caller waits
through. `tpt-web`'s push player feeds packets one at a time and
presents what comes out, so it inherits the reorder delay unchanged —
but its "presented N pictures" counters and the demo timing shift by
the B-depth, and `real_time_decoder` gains a caveat (reorder latency
= `b_frames` frame periods).

**Tests to add before coding.** Display-order round trip through an
I/P/B/P/B sequence; a B-frame whose forward and backward references
disagree (the averaging path); `max_packet_bits` bound at `b_frames`
= 2; corruption sweeps over the new header fields; and the tpt-web
reorder-buffer drain at end of stream.


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
offline on both sides (archival profile). Vault claims neither side
real-time (an offline codec that allocates per RDO candidate); Thrum and
Mote are covered in their own sections.

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
| Vault | `tpt-video-vault` | 2 | above, "Vault v0.1 — implemented design" (v2: 2-bit frame type + 16-bit poc, the B-frames groundwork) | CRC-32 trailer |
| Mote | `tpt-video-mote` | 1 | above, "Mote v0.1 — implemented design" (frame packets + provable bound) | CRC-32 trailer |

A wire-format change bumps `VERSION`, fails fast against every older
decoder, and must land with its layout table extended here and its
provenance entry updated (`docs/provenance.md`) in the same change.

## Patent-Conscious Design Choices

The following choices are made specifically to reduce patent exposure
(see `CONTRIBUTING.md` §3):

- **No CABAC.** Cinder and Aura use static Huffman or a static range
  coder; Thrum uses simple VLC tables; Vault uses static Rice codes with
  a per-block parameter and no context modelling or adaptive probability
  tracking.
- **No fractional-pixel motion estimation beyond half-pixel bilinear.**
  Thrum uses full-pixel only. Vault uses full-pixel + 2-tap bilinear
  half-pixel (a 4-sample average on the diagonal position).
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