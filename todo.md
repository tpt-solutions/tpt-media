# tpt-media — Project Checklist

Tracks implementation of the tpt-media codec suite (see `spec.txt`).
Dual-licensed MIT OR Apache-2.0. Copyright TPT Solutions.

## Phase 0 — Project Scaffolding & Governance

- [ ] Init git repo
- [ ] Cargo workspace root `Cargo.toml` with all crate/app members
- [ ] Empty crate/app skeletons matching the spec's directory tree
      (`tpt-core`, `tpt-audio-pulse`, `tpt-audio-aura`, `tpt-audio-vox`,
      `tpt-video-livelink`, `tpt-video-vault`, `tpt-video-whisper`,
      `tpt-cli`, `tpt-web`, `tpt-media-sys`, `tests/`)
- [ ] `LICENSE-MIT` and `LICENSE-APACHE-2.0` files
- [ ] `license = "MIT OR Apache-2.0"` in all Cargo manifests; copyright
      attribution to TPT Solutions
- [ ] `README.md` skeleton (project description, license badges)
- [ ] `CONTRIBUTING.md` with clean-room rules and patent-compliance rules
      (spec §8)
- [ ] Basic CI skeleton (build + test on push/PR)

## Phase 1 — tpt-core (Shared Foundation)

- [ ] Bitstream: `BitWriter` (u64 accumulator, zero-alloc, writes to `&mut [u8]`)
- [ ] Bitstream: `BitReader` (u64 accumulator, reads from `&[u8]`)
- [ ] Entropy coding: static Range Coder (public-domain, no CABAC)
- [ ] Entropy coding: simple Huffman
- [ ] `dct.rs`: 1D Type-II DCT + inverse
- [ ] `dct.rs`: 2D Type-II DCT + inverse, up to 8x8
- [ ] `mdct.rs`: MDCT with overlapping windows (256/1024/2048/4096 points)
- [ ] `lpc.rs`: autocorrelation
- [ ] `lpc.rs`: Levinson-Durbin recursion
- [ ] `.tpt` container: File Header read/write (magic, version, timebase)
- [ ] `.tpt` container: `TRAK` chunk read/write (track ID, codec ID, codec
      private data)
- [ ] `.tpt` container: `CLUS` chunk read/write (timestamp, keyframe flag,
      track ID, payload)
- [ ] `.tpt` container: keyframe-scan seeking
- [ ] Unit tests: bitstream (BitWriter/BitReader round-trip)
- [ ] Unit tests: math primitives (DCT, MDCT, LPC)
- [ ] Unit tests: container round-trip (write then parse back)

## Phase 2 — tpt-audio-vox (Edge / IoT)

- [ ] `#![no_std]` crate setup
- [ ] Fixed-point i16/i32 math (no FPU)
- [ ] CELP / pure-LPC prediction pipeline
- [ ] Hardcode 8kHz/16kHz sample rate, mono only
- [ ] Public-domain VQ codebook (1024 pre-generated speech shapes)
- [ ] Residual-to-codebook matching + 10-bit index encoding
- [ ] Round-trip encode/decode tests on no_std target

## Phase 3 — tpt-audio-pulse (Real-Time)

- [ ] MDCT with sine window, 128/256-sample frames (~2.5–5ms @ 48kHz)
- [ ] Pure transform coding pipeline (no LPC, no speech/music detection)
- [ ] Packet Loss Concealment: extract pitch period from previous frame
- [ ] Packet Loss Concealment: repeat/stretch waveform to fill gap
- [ ] Round-trip tests
- [ ] Algorithmic-latency verification tests

## Phase 4 — tpt-audio-aura (High-Fidelity / Archival)

- [ ] Mode A (Lossless): high-order LPC (up to order 32)
- [ ] Mode A (Lossless): Rice-coded residual, bit-perfect reconstruction
- [ ] Mode B (Lossy): large MDCT (2048/4096 points)
- [ ] Psychoacoustics: Bark-scale critical bands
- [ ] Psychoacoustics: simultaneous frequency masking
- [ ] Psychoacoustics: temporal masking
- [ ] Psychoacoustics: quantization noise shaping under masking threshold
- [ ] f64 internal precision throughout
- [ ] Round-trip tests for both modes
- [ ] Bit-perfect verification tests for lossless mode

## Phase 5 — tpt-video-livelink (Real-Time)

- [ ] I-frame and P-frame pipeline only (no B-frames)
- [ ] Intra-refresh: force 5% of macroblocks to intra-mode per frame
- [ ] Motion estimation: full-pixel block matching via SAD
- [ ] Transform: 4x4 DCT
- [ ] Transform: 8x8 DCT
- [ ] 3x3 deblocking filter on block edges
- [ ] Round-trip tests

## Phase 6 — tpt-video-whisper (Edge / IoT)

- [ ] Shared pre-trained dictionary: 4096 common 8x8 pixel patches
- [ ] Frame split into 8x8 blocks
- [ ] Nearest-patch matching + 12-bit index encoding
- [ ] Hardcode max resolution to 480p
- [ ] Client-side Lanczos upscale filter
- [ ] Tests validating <50kbps target bitrate

## Phase 7 — tpt-video-vault (Archival)

- [ ] Superblocks up to 64x64, recursive partition down to 8x8
- [ ] Motion estimation: full-pixel block matching
- [ ] Motion estimation: half-pixel via simple 2-tap bilinear filter
      (patent-safe, no 6/8-tap FIR)
- [ ] Brute-force RDO: exhaustive partition-size and prediction-mode search
- [ ] Lagrangian cost calculation (J = D + λR)
- [ ] Reference frame buffer: up to 16 previous frames
- [ ] Round-trip tests
- [ ] Compression-ratio validation tests

## Phase 8 — tpt-cli

- [ ] `clap`-based CLI skeleton
- [ ] `encode` subcommand (`-i input -o output -c <codec> [--lossless]`)
- [ ] `decode` subcommand (`-i input.tpt -o output`)
- [ ] `probe` subcommand (prints container + track metadata)
- [ ] Wire up all 6 codecs + container through the CLI
- [ ] GitHub Actions: cross-compile for Windows, macOS, Linux
- [ ] GitHub Actions: attach binaries to GitHub Releases

## Phase 9 — tpt-web (WASM Player)

- [ ] `wasm-bindgen` bindings exposing decoders to JavaScript
- [ ] HTML5/Canvas/WebAudio front-end
- [ ] Drag-and-drop `.tpt` file playback
- [ ] GitHub Pages hosting/deploy CI

## Phase 10 — tpt-media-sys (FFI Bridge)

- [ ] `extern "C"` wrappers around safe Rust APIs
- [ ] `cbindgen` config generating `tpt_media.h`
- [ ] Sample C/C++ consumer verifying the bridge
- [ ] Sample Python (cffi) consumer verifying the bridge
- [ ] Sample Node.js (napi-rs) consumer verifying the bridge

## Phase 11 — Legal, Docs & Release Polish

- [ ] Full pass on `CONTRIBUTING.md` against every rule in spec §8
- [ ] Complete `README.md` with usage examples for CLI and web player
- [ ] Cut v1.0 release
- [ ] Verify CLI release pipeline end-to-end
- [ ] Verify web player deploy end-to-end
