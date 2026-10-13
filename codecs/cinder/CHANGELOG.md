# Changelog — tpt-audio-cinder

Scoped record of changes to `tpt-audio-cinder`. The workspace-wide release log
— including cross-cutting changes and the lockstep version bump that ships
every crate together — is the root [`CHANGELOG.md`](../../CHANGELOG.md). This
file is a filtered view of it: a change touching only this crate is recorded
here *and* summarised in the root entry; a change touching several crates is
recorded in the root, which remains the record (CONTRIBUTING §9.1).

Format follows [Keep a Changelog](https://keepachangelog.com/). Versions are
lockstep with the workspace; no API-stability guarantee pre-1.0
(CONTRIBUTING §9.2).

> **Wire-format changes are breaking regardless of the crate version.** This
> codec is fail-fast: an unsupported 16-bit bitstream version is an error, not
> a guess (todo.md §35). There is no compatibility mode.

## [Unreleased]

### Changed

- **Cinder v2: overlap-add MDCT (wire version 2, breaking) (2026-10-14).**
  The encoder now analyses hop-`frame_size` blocks (each window is the
  previous block plus the current one) and the decoder overlap-adds
  consecutive inverse blocks, so time-domain aliasing cancels. This removes
  the structural ≈3 dB SNR ceiling recorded in todo.md Known Issues. Measured
  on the §37 corpus at 8 bits per coefficient: sine 3.01 → 34.30 dB,
  square 3.03 → 26.79 dB, noise 2.95 → 21.08 dB. The packet layout is
  unchanged, but its meaning is not, so version 1 packets are rejected with
  `Unsupported`. Plan and decisions: `docs/codec-design.md`, "Cinder v2".
  - **Framing.** One packet per `frame_size` input samples. The first
    packet yields no samples, so the stream has one block of latency.
    `Encoder::drain` emits the zero-padded tail; `Encoder::flush` emits the
    same tail and then resets the encoder. `Decoder::flush` resets the
    overlap state.
  - **Timestamps.** A packet is stamped with the start of the output it
    yields, so a decoded frame's pts equals its packet's pts. The first
    packet takes the stream origin, and no timestamp is negative.
  - **Rate.** Packets arrive every `frame_size` samples, so bits per second
    double at a given bit depth and step table. At matched bitrate (about
    218 kbit/s, 4 bands, 4 bits, a per-signal step of 4 to 64) v2 measured
    8 to 31 dB across the corpus, against the old codec's flat ~3 dB at
    231 kbit/s. The per-signal steps were chosen after the fact, not tuned.
  - **Corpus floors** in `apps/tpt-cli/tests/quality.rs` are re-pinned to
    the measured v2 values, about 1 dB under each.
  - **FFI.** The C ABI's Cinder block size is now 256 (was 512), since the
    ABI carries one packet per encode call. This is a visible change to
    `tpt_encoder_block_size` for Cinder callers.

### Fixed

- **Bit depths below 8 produced garbage.** The offset-binary level was
  written as `q + 128` into `bits_per_coef` bits, so any depth below 8
  wrapped and decoded to negative SNR (measured −6 dB at 4 bits). Levels now
  clamp to the signed range the width can carry, with offset `2^(bits-1)`.
  At 8 bits this is identical to version 1. `bits_per_coef` is validated to
  `4..=12` at construction, as its doc always claimed.

### Added

- **Per-crate README, changelog, keywords and categories** (2026-09-30):
  crate-specific `README.md` and manifest `keywords`/`categories`. The README
  now carries the measured performance table, the profiling recipe
  `docs/performance.md` referred to but that no file actually held, and the
  structural quality ceiling stated as a limitation rather than buried.
- Seeded-random property tests plus `tests/realtime_alloc.rs` (2026-09-11):
  a counting global allocator asserts steady-state encode/decode allocations
  stay bounded (512 KiB over 31 blocks, ~25× headroom), which is what turns
  "real-time" from an intention into a tested claim.

### Fixed

- The decoder read `n_bands` and each band's `n_coeffs` straight from the
  packet with no bound relative to `frame_size`, so a malformed or adversarial
  packet claiming more bands/coefficients than fit in the decoded spectrum
  buffer indexed it out of bounds and **panicked** instead of returning
  `CodecError::MalformedPacket`. Found by the codec fuzz sweep (2026-09-11).
  Fixed by bounds-checking `start + n_coeffs <= spec.len()` and rejecting
  `n_bands == 0` before writing. Regression tests
  `decode_rejects_n_coeffs_that_would_overrun_the_spectrum` and
  `decode_rejects_zero_bands`.

## [0.1.0] — 2026-09-04

### Added

- Initial crate (todo.md §29): MDCT with a sine window, uniform scalar
  quantisation with per-band step sizes, fixed-width bit-field entropy
  coding, 128/256-sample frames, 44.1/48 kHz mono `S16`, and phase-randomised
  sign-perturbation PLC. Round-trip, packet-loss and jitter tests.
- `CinderEncoderConfig::perceptual_weighting` (default `false`): scales each
  band's step by that band's absolute threshold of hearing relative to 1 kHz,
  evaluated once at construction. It only ever coarsens a step, never
  sharpens it, costs nothing per frame, and changes no wire format, since
  Cinder already transmits the actual per-band step used.
- Per-packet CRC-32 trailer and the 16-bit bitstream version field — both
  **breaking wire additions**: packets encoded before them are rejected.
