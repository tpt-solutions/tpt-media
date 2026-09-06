# TPT Media — Master Implementation TODO

**Repository:** `tpt-media`
**Organization:** TPT Solutions
**Language:** Rust
**Status:** Architecture / Implementation Planning
**Document version:** 2.0
**Last updated:** 2026-09-07

---

# 0. Purpose

`tpt-media` is the native TPT media framework and codec ecosystem.

It provides:

* media data models
* timestamps and synchronization
* packet/frame abstractions
* codec interfaces
* media pipelines
* bitstream primitives
* container formats
* native TPT codecs
* media probing
* CLI tooling
* WebAssembly playback
* stable FFI

`tpt-media` does **not** attempt to become a general-purpose DSP library.

General signal-processing and mathematical primitives belong in `tpt-dsp`.

`tpt-kinetix` remains the standards-oriented media implementation project.

The intended ecosystem is:

```text
                         TPT APPLICATIONS
                               │
              ┌────────────────┼────────────────┐
              │                │                │
       tpt-government      tpt-kinetix      tpt-media
                                             │
                                      Native TPT Media
                                             │
                                             ▼
                                          tpt-dsp
```

More specifically:

```text
tpt-media
│
├── Media representation
├── Codecs
├── Containers
├── Bitstreams
├── Pipelines
├── Synchronization
└── I/O integration
        │
        ▼
    tpt-dsp
        │
        ├── transforms
        ├── filters
        ├── signal analysis
        ├── codec DSP
        └── SIMD
```

---

# 1. Core Design Principles

* [ ] Rust-first
* [ ] Safe Rust wherever practical
* [ ] `unsafe` isolated and documented
* [ ] `no_std` capability for appropriate components
* [ ] deterministic behavior where practical
* [ ] zero allocation in real-time hot paths
* [ ] caller-owned buffers where practical
* [ ] bounded resource usage
* [ ] explicit numerical contracts
* [ ] fuzz-resistant parsing
* [ ] corruption-tolerant decoding where practical
* [ ] forward-compatible formats
* [ ] clean architectural boundaries
* [ ] reusable crates
* [ ] WASM-compatible core components
* [ ] stable FFI boundary
* [ ] reproducible benchmarks
* [ ] provenance tracking
* [ ] patent-conscious design

Avoid:

* [ ] unnecessary dependencies
* [ ] hidden heap allocation
* [ ] global mutable state
* [ ] codec-specific DSP duplicated across codecs
* [ ] application-specific logic in core crates
* [ ] undocumented numerical assumptions
* [ ] claims of absolute patent-free status

Preferred terminology:

> **Patent-conscious / royalty-free design objective**

rather than:

> 100% patent-free

Patent status is jurisdictional and must be independently reviewed.

---

# 2. Repository Structure

Target structure:

```text
tpt-media/
│
├── Cargo.toml
├── README.md
├── LICENSE
├── LICENSE-APACHE
├── CHANGELOG.md
├── CONTRIBUTING.md
├── SECURITY.md
├── ARCHITECTURE.md
├── TODO.md
│
├── docs/
│   ├── architecture.md
│   ├── codec-design.md
│   ├── container-design.md
│   ├── pipeline-design.md
│   ├── performance.md
│   ├── security.md
│   └── provenance.md
│
├── crates/
│   ├── tpt-media-core/
│   ├── tpt-media-bitstream/
│   ├── tpt-media-codec/
│   ├── tpt-media-container/
│   ├── tpt-media-pipeline/
│   ├── tpt-media-io/
│   └── tpt-media-probe/
│
├── codecs/
│   ├── pulse/
│   ├── aura/
│   ├── vox/
│   ├── livelink/
│   ├── vault/
│   └── whisper/
│
├── containers/
│   └── tpt-container/
│
├── apps/
│   ├── tpt-cli/
│   └── tpt-web/
│
├── ffi/
│   └── tpt-media-sys/
│
├── tests/
│   ├── integration/
│   ├── corpus/
│   ├── corruption/
│   └── interoperability/
│
├── fuzz/
│   ├── bitstream/
│   ├── container/
│   ├── codecs/
│   └── probe/
│
└── benches/
```

Do **not** create:

```text
tpt-media-math
tpt-media-signal
tpt-media-simd
```

Those responsibilities belong in `tpt-dsp`.

---

# 3. TPT-DSP Integration

## 3.1 Establish dependency boundary

* [ ] Define `tpt-dsp` as the reusable signal-processing foundation
* [ ] Define `tpt-media` as the media representation/codec/container layer
* [ ] Document dependency direction
* [ ] Prevent `tpt-dsp` from depending on `tpt-media`
* [ ] Prevent codec-specific APIs from leaking into `tpt-dsp`

Desired:

```text
tpt-media ───────► tpt-dsp
```

Never:

```text
tpt-dsp ───────► tpt-media
```

---

# 4. TPT-DSP Foundation Upgrade

This section is part of the master `tpt-media` plan.

Implementation may occur in the `tpt-dsp` repository.

## 4.1 Existing capabilities to retain

* [x] complex arithmetic
* [x] FFT
* [x] inverse FFT
* [x] DCT-II
* [x] DCT-III
* [x] DCT-IV
* [x] windows
* [x] filters
* [x] convolution
* [x] resampling
* [x] Hilbert transform
* [x] ring buffers
* [x] SPSC queues
* [x] SIMD abstraction
* [x] spectral analysis
* [x] audio I/O
* [x] WAV support

These should be extended rather than duplicated.

---

# 5. TPT-DSP — Transform Expansion

## 5.1 MDCT

Required by Pulse and Aura.

* [ ] Define MDCT mathematical convention
* [ ] Define IMDCT convention
* [ ] Define normalization
* [ ] Define windowing convention
* [ ] Define overlap convention
* [ ] Define supported block sizes
* [ ] Implement reference MDCT
* [ ] Implement reference IMDCT
* [ ] Add round-trip tests
* [ ] Add numerical error tests
* [ ] Add edge-case tests
* [ ] Add reusable `MdctPlan`
* [ ] Avoid allocation during processing
* [ ] Add optimized implementation
* [ ] Add SIMD implementation
* [ ] Benchmark scalar vs optimized
* [ ] Document numerical guarantees

Initial target sizes:

```text
128
256
512
1024
2048
4096
```

---

## 5.2 DCT optimization

Current DCT-IV implementation is suitable as a correctness reference but must be evaluated for codec workloads.

* [ ] Benchmark current DCT implementation
* [ ] Define performance targets
* [ ] Implement optimized DCT-IV
* [ ] Preserve reference implementation for verification
* [ ] Add scalar/SIMD equivalence tests
* [ ] Verify numerical error bounds

---

## 5.3 2D transforms

Required primarily by video codecs.

* [ ] Define 2D DCT API
* [ ] Implement separable 2D DCT
* [ ] Implement inverse 2D DCT
* [ ] Support 4×4
* [ ] Support 8×8
* [ ] Support 16×16
* [ ] Benchmark
* [ ] SIMD optimize common block sizes
* [ ] Verify against 1D decomposition

---

# 6. TPT-DSP — LPC

Required by Vox and Aura.

* [ ] Implement autocorrelation
* [ ] Define LPC coefficient convention
* [ ] Implement Levinson-Durbin
* [ ] Define stability behavior
* [ ] Handle singular/degenerate inputs
* [ ] Implement LPC analysis
* [ ] Implement prediction
* [ ] Implement residual generation
* [ ] Implement synthesis
* [ ] Add numerical tests
* [ ] Add stability tests
* [ ] Add deterministic tests
* [ ] Add benchmarks

Potential API:

```text
LpcAnalyzer
LpcCoefficients
autocorrelation()
levinson_durbin()
predict()
residual()
synthesize()
```

---

# 7. TPT-DSP — Pitch Analysis

Required primarily by Vox and potentially useful elsewhere.

* [ ] Define pitch-analysis API
* [ ] Implement autocorrelation pitch estimator
* [ ] Implement AMDF estimator
* [ ] Define pitch period range
* [ ] Define voiced/unvoiced detection
* [ ] Add confidence metric
* [ ] Add tests using synthetic signals
* [ ] Add speech corpus tests
* [ ] Benchmark

---

# 8. TPT-DSP — Vector Quantisation

Required by Vox and Whisper.

* [ ] Define generic VQ API
* [ ] Implement nearest-vector search
* [ ] Implement distance metrics
* [ ] Implement fixed-size codebooks
* [ ] Implement encoder
* [ ] Implement decoder
* [ ] Support caller-owned buffers
* [ ] Add deterministic behavior
* [ ] Add optional accelerated search
* [ ] Benchmark linear search
* [ ] Investigate tree/product quantisation for larger codebooks

VQ itself belongs in DSP.

The codec-specific codebook belongs in the codec.

---

# 9. TPT-DSP — LSP / LSF

Useful for speech coding.

* [ ] Define LSP representation
* [ ] Define LSF representation
* [ ] LPC → LSP conversion
* [ ] LSP → LPC conversion
* [ ] Stability validation
* [ ] Interpolation
* [ ] Quantisation support
* [ ] Tests

---

# 10. TPT-DSP — Psychoacoustics

Required primarily by Aura.

## 10.1 Frequency scales

* [ ] Bark scale
* [ ] critical-band mapping
* [ ] ERB utilities if useful
* [ ] frequency ↔ Bark conversion
* [ ] band aggregation

## 10.2 Psychoacoustic primitives

* [ ] threshold-in-quiet model
* [ ] tonal/noise classification
* [ ] spectral spreading
* [ ] simultaneous masking
* [ ] masking threshold
* [ ] perceptual weighting
* [ ] model validation
* [ ] benchmark

These should be generic models, not Aura-specific implementations.

---

# 11. TPT-DSP — Noise Shaping

* [ ] Define noise-shaping filter API
* [ ] Implement generic noise-shaping filter
* [ ] Support configurable orders
* [ ] Validate stability
* [ ] Add tests
* [ ] Add benchmark

---

# 12. TPT-DSP — Numerical Contracts

For every transform and numerical algorithm:

* [ ] Define input domain
* [ ] Define output domain
* [ ] Define normalization
* [ ] Define precision
* [ ] Define acceptable error
* [ ] Define valid sizes
* [ ] Define failure behavior
* [ ] Define determinism expectations
* [ ] Add reference tests

Required for:

* [ ] FFT
* [ ] DCT
* [ ] MDCT
* [ ] LPC
* [ ] filters
* [ ] resampling
* [ ] VQ
* [ ] psychoacoustic calculations

---

# 13. TPT-DSP — SIMD

* [ ] Retain SIMD in `tpt-dsp`
* [ ] Remove planned `tpt-media-simd`
* [ ] Establish scalar reference implementations
* [ ] Establish SIMD equivalence tests
* [ ] Benchmark SIMD kernels
* [ ] Document supported architectures
* [ ] Ensure runtime feature detection where appropriate
* [ ] Verify fallback behavior

Potential targets:

```text
scalar
portable SIMD
x86/x86_64
ARM/AArch64
future RISC-V
```

---

# 14. TPT-DSP — Real-Time Contracts

Define explicit execution profiles.

## Real-time

* [ ] no allocation
* [ ] no blocking
* [ ] no I/O
* [ ] no unbounded work
* [ ] caller-owned memory
* [ ] deterministic execution bounds where possible

## Offline

* [ ] dynamic allocation permitted
* [ ] larger scratch buffers permitted
* [ ] parallel processing permitted
* [ ] expensive RDO permitted

Document which DSP APIs satisfy which profile.

---

# 15. TPT-MEDIA-CORE

Define fundamental media data types.

* [ ] `Timestamp`
* [ ] `Duration`
* [ ] `Timebase`
* [ ] `Packet`
* [ ] `Frame`
* [ ] `AudioFrame`
* [ ] `VideoFrame`
* [ ] `Track`
* [ ] `Stream`
* [ ] `CodecId`
* [ ] `CodecConfig`
* [ ] `PixelFormat`
* [ ] `SampleFormat`
* [ ] `ChannelLayout`
* [ ] `MediaType`
* [ ] `Metadata`

Support:

* [ ] PTS
* [ ] DTS
* [ ] duration
* [ ] keyframe/discontinuity flags
* [ ] side data
* [ ] codec-private data
* [ ] per-track timebases

---

# 16. Timestamp Architecture

* [ ] Define rational timebase
* [ ] Avoid floating-point timestamps
* [ ] Define timestamp conversion
* [ ] Define overflow behavior
* [ ] Define negative timestamp handling
* [ ] Define discontinuities
* [ ] Define clock domains
* [ ] Define synchronization model
* [ ] Add audio/video sync tests
* [ ] Add long-duration timestamp tests

---

# 17. Bitstream Layer

Create `tpt-media-bitstream`.

## Bit reader

* [ ] arbitrary bit-width reads
* [ ] bounded reads
* [ ] alignment
* [ ] peek
* [ ] skip
* [ ] EOF handling
* [ ] malformed-input handling

## Bit writer

* [ ] arbitrary bit-width writes
* [ ] alignment
* [ ] flush
* [ ] bounded capacity
* [ ] deterministic output

## Compression primitives

* [ ] Huffman
* [ ] Rice coding
* [ ] range coder
* [ ] entropy utilities
* [ ] canonical representations
* [ ] malformed-input handling

Do not move these into `tpt-dsp`.

---

# 18. Codec Framework

Create `tpt-media-codec`.

Define common interfaces:

```text
Codec
Encoder
Decoder
CodecConfig
Packet
Frame
```

* [ ] define codec discovery
* [ ] define encoder lifecycle
* [ ] define decoder lifecycle
* [ ] define flush semantics
* [ ] define drain semantics
* [ ] define error model
* [ ] define configuration model
* [ ] define capability discovery
* [ ] define hardware/SIMD capability exposure
* [ ] define real-time capability
* [ ] define supported formats

---

# 19. Resource Limits

All decoders must support bounded resources.

* [ ] maximum frame dimensions
* [ ] maximum packet size
* [ ] maximum metadata size
* [ ] maximum allocation
* [ ] maximum reference frames
* [ ] maximum recursion depth
* [ ] maximum decode work where practical
* [ ] cancellation support
* [ ] malformed-input termination guarantees

---

# 20. TPT Container

Create the native `.tpt` media container.

## 20.1 File header

Initial structure:

```text
magic:       TPTM
version
header_size
global_timebase
```

* [ ] finalize binary representation
* [ ] define endianness
* [ ] define versioning
* [ ] define reserved fields
* [ ] define compatibility rules

---

# 21. Container Track Structure

Track metadata must include:

* [ ] track ID
* [ ] media type
* [ ] codec ID
* [ ] codec version
* [ ] codec-private data
* [ ] timebase
* [ ] sample rate where applicable
* [ ] channel layout where applicable
* [ ] dimensions where applicable
* [ ] pixel format where applicable
* [ ] metadata

---

# 22. Container Cluster Structure

Clusters should support:

* [ ] timestamp
* [ ] duration
* [ ] track ID
* [ ] PTS
* [ ] DTS
* [ ] keyframe flag
* [ ] discontinuity flag
* [ ] payload
* [ ] optional checksum

---

# 23. Container Index

Do not require scanning the entire file for normal seeking.

* [ ] define index structure
* [ ] index keyframes
* [ ] index cluster offsets
* [ ] support per-track indexes
* [ ] support time-based lookup
* [ ] support offset-based lookup
* [ ] define index versioning
* [ ] define recovery behavior

---

# 24. Container Robustness

* [ ] corruption detection
* [ ] truncated-file handling
* [ ] incomplete final cluster handling
* [ ] unknown chunk skipping
* [ ] forward-compatible chunk types
* [ ] checksum support
* [ ] append mode
* [ ] live-stream mode
* [ ] recovery/index rebuilding
* [ ] fuzz testing

---

# 25. Container Streaming

Support files that are not yet complete.

* [ ] fragmented/clustered writing
* [ ] incremental parsing
* [ ] live append
* [ ] missing final index
* [ ] streaming playback
* [ ] graceful termination
* [ ] optional footer/index reconstruction

---

# 26. Media Pipeline

Create `tpt-media-pipeline`.

Architecture:

```text
Source
  ↓
Demux
  ↓
Decoder
  ↓
Filter / Transform
  ↓
Encoder
  ↓
Mux
  ↓
Sink
```

* [ ] define node abstraction
* [ ] define stream negotiation
* [ ] define format negotiation
* [ ] define backpressure
* [ ] define buffering
* [ ] define cancellation
* [ ] define error propagation
* [ ] define flushing
* [ ] define end-of-stream
* [ ] define timestamps through pipeline
* [ ] define threading model
* [ ] define realtime scheduling profile

---

# 27. Media I/O

Create `tpt-media-io`.

Reuse `tpt-dsp-io` rather than duplicating functionality.

* [ ] define PCM integration
* [ ] integrate WAV
* [ ] integrate raw audio
* [ ] define image/video frame I/O
* [ ] define streaming sources
* [ ] define streaming sinks
* [ ] define async integration where appropriate

---

# 28. Native Codec Family

All codecs must implement the generic codec framework.

---

# 29. Pulse

**Purpose:** low-latency real-time audio.

Target:

* low latency
* low complexity
* robust packet loss behavior
* real-time safe operation

## DSP

* [ ] MDCT via `tpt-dsp`
* [ ] windowing via `tpt-dsp`
* [ ] quantisation
* [ ] optional perceptual weighting

## Codec

* [ ] define frame sizes
* [ ] define bitstream
* [ ] define modes
* [ ] define quantisation
* [ ] implement encoder
* [ ] implement decoder
* [ ] implement PLC
* [ ] test packet loss
* [ ] test jitter
* [ ] benchmark latency
* [ ] benchmark CPU usage

Initial frame targets:

```text
128
256 samples
```

---

# 30. Aura

**Purpose:** high-fidelity / archival audio.

Modes:

```text
lossless
lossy
```

## Lossless

* [ ] LPC
* [ ] residual
* [ ] Rice coding
* [ ] block structure
* [ ] checksum/integrity
* [ ] bit-exact round trip

## Lossy

* [ ] MDCT
* [ ] psychoacoustic model
* [ ] Bark bands
* [ ] masking
* [ ] quantisation
* [ ] noise shaping
* [ ] stereo model if required
* [ ] encoder
* [ ] decoder

Use `f64` only where justified by measurable quality requirements.

---

# 31. Vox

**Purpose:** extremely low-bandwidth speech.

Initial target:

```text
8 kHz
16 kHz
mono
```

* [ ] LPC analysis
* [ ] pitch estimation
* [ ] voiced/unvoiced classification
* [ ] excitation model
* [ ] VQ
* [ ] LSP/LSF if beneficial
* [ ] fixed-point implementation
* [ ] `no_std` support
* [ ] encoder
* [ ] decoder
* [ ] packet-loss concealment
* [ ] deterministic decoding
* [ ] embedded benchmark

Initial codebook target:

```text
1024 entries
```

---

# 32. Livelink

**Purpose:** low-latency video communication.

Design:

```text
I/P coding
intra refresh
simple prediction
low-complexity transforms
```

* [ ] define pixel formats
* [ ] define colour model
* [ ] define frame structure
* [ ] define I-frame
* [ ] define P-frame
* [ ] implement intra prediction
* [ ] implement inter prediction
* [ ] motion estimation
* [ ] full-pixel SAD
* [ ] DCT
* [ ] quantisation
* [ ] entropy coding
* [ ] intra refresh
* [ ] deblocking
* [ ] decoder
* [ ] encoder
* [ ] latency benchmark

Initial intra-refresh target:

```text
~5% of frame per frame
```

---

# 33. Vault

**Purpose:** high-compression video.

* [ ] define superblock structure
* [ ] 64×64 top-level blocks
* [ ] recursive partitioning
* [ ] minimum 8×8 blocks
* [ ] motion vectors
* [ ] full-pixel search
* [ ] half-pixel interpolation
* [ ] reference frames
* [ ] 2D transforms
* [ ] quantisation
* [ ] RDO
* [ ] entropy coding
* [ ] loop filtering
* [ ] decoder
* [ ] encoder

Initial target:

```text
up to 16 reference frames
```

RDO must use an explicit objective such as:

```text
J = D + λR
```

where:

* `D` = distortion
* `R` = bitrate
* `λ` = rate/distortion tradeoff

Do not claim an absolute global minimum.

---

# 34. Whisper

**Purpose:** extreme edge / IoT video.

* [ ] define supported dimensions
* [ ] define patch representation
* [ ] define dictionary
* [ ] implement VQ
* [ ] implement encoder
* [ ] implement decoder
* [ ] define reconstruction model
* [ ] investigate temporal prediction
* [ ] define quality metric
* [ ] benchmark bandwidth
* [ ] benchmark CPU
* [ ] benchmark memory
* [ ] test extremely low bitrate

Initial target:

```text
≤480p
4096-entry dictionary
12-bit codebook index
```

The bandwidth target must be benchmark-defined rather than treated as a universal guarantee.

---

# 35. Codec Interoperability

Native TPT codecs need:

* [ ] versioned bitstreams
* [ ] codec identification
* [ ] configuration serialization
* [ ] decoder capability negotiation
* [ ] graceful unsupported-mode handling
* [ ] forward-compatible extension mechanism

---

# 36. Codec Quality Testing

Create reference corpora.

## Audio

* [ ] speech
* [ ] music
* [ ] transient-heavy material
* [ ] synthetic signals
* [ ] silence
* [ ] pathological inputs

## Video

* [ ] talking head
* [ ] camera motion
* [ ] static scenes
* [ ] high-motion scenes
* [ ] animation
* [ ] screen content
* [ ] synthetic patterns
* [ ] pathological frames

---

# 37. Objective Quality Metrics

Audio:

* [ ] SNR
* [ ] spectral error
* [ ] perceptual metrics where practical
* [ ] bitrate/quality curves
* [ ] CPU/quality curves

Video:

* [ ] PSNR
* [ ] SSIM
* [ ] bitrate
* [ ] frame latency
* [ ] encode CPU
* [ ] decode CPU
* [ ] memory

---

# 38. Testing

## Unit tests

* [ ] every public primitive
* [ ] edge cases
* [ ] malformed input
* [ ] overflow
* [ ] empty input
* [ ] maximum values

## Property tests

* [ ] encode/decode invariants
* [ ] timestamp invariants
* [ ] bitstream invariants
* [ ] container invariants
* [ ] transform properties

## Fuzzing

* [ ] bitstream fuzzing
* [ ] container fuzzing
* [ ] codec fuzzing
* [ ] probe fuzzing
* [ ] malformed metadata
* [ ] truncated packets
* [ ] random packet ordering

---

# 39. Determinism

* [ ] deterministic decoding
* [ ] deterministic container generation where practical
* [ ] deterministic scalar DSP
* [ ] document SIMD differences
* [ ] deterministic test corpus
* [ ] reproducible benchmark configuration

---

# 40. Scalar / SIMD Equivalence

For all optimized DSP kernels:

* [ ] scalar reference
* [ ] SIMD implementation
* [ ] same-input comparison
* [ ] tolerance definition
* [ ] randomized testing
* [ ] architecture-specific CI

---

# 41. Performance

Benchmark:

* [ ] MDCT
* [ ] IMDCT
* [ ] FFT
* [ ] DCT
* [ ] LPC
* [ ] VQ
* [ ] psychoacoustics
* [ ] entropy coding
* [ ] Pulse encode/decode
* [ ] Aura encode/decode
* [ ] Vox encode/decode
* [ ] Livelink encode/decode
* [ ] Vault encode/decode
* [ ] Whisper encode/decode
* [ ] container mux/demux
* [ ] pipeline throughput

Record:

```text
throughput
latency
CPU
memory
allocations
```

---

# 42. Memory Testing

* [ ] allocation profiling
* [ ] real-time allocation assertions
* [ ] peak memory measurements
* [ ] hostile-input memory tests
* [ ] maximum-frame tests
* [ ] maximum-packet tests

---

# 43. CLI

Create `tpt-cli`.

Commands:

```text
tpt probe
tpt encode
tpt decode
tpt transcode
tpt inspect
tpt benchmark
```

* [ ] codec selection
* [ ] container selection
* [ ] quality configuration
* [ ] bitrate configuration
* [ ] metadata manipulation
* [ ] streaming stdin/stdout
* [ ] machine-readable output
* [ ] error reporting

---

# 44. Probe

Create `tpt-media-probe`.

* [ ] identify container
* [ ] identify codec
* [ ] inspect tracks
* [ ] inspect duration
* [ ] inspect timestamps
* [ ] inspect dimensions
* [ ] inspect sample rate
* [ ] inspect bitrate
* [ ] inspect metadata
* [ ] report corruption
* [ ] report codec capabilities

---

# 45. WASM

Create `tpt-web`.

* [ ] WASM-compatible decoder
* [ ] `.tpt` parser
* [ ] streaming playback
* [ ] browser audio output
* [ ] browser video output
* [ ] JavaScript bindings
* [ ] memory limits
* [ ] browser demo
* [ ] performance benchmark

---

# 46. FFI

Create `tpt-media-sys`.

The FFI should expose the **stable media abstraction**, not internal implementation details.

Expose:

```text
Context
Packet
Frame
Codec
Encoder
Decoder
Container
Demuxer
Muxer
```

* [ ] C ABI
* [ ] opaque handles
* [ ] explicit ownership
* [ ] explicit allocation/free functions
* [ ] error codes
* [ ] ABI version
* [ ] thread-safety documentation
* [ ] safety documentation
* [ ] C header generation
* [ ] C integration tests

---

# 47. Security

* [ ] threat model
* [ ] hostile media-input model
* [ ] integer-overflow audit
* [ ] bounds-check audit
* [ ] allocation exhaustion protection
* [ ] recursion limits
* [ ] decompression-bomb protection
* [ ] malformed timestamp handling
* [ ] fuzzing CI
* [ ] security disclosure process

---

# 48. Provenance / IP

Maintain:

```text
algorithm
reference
source
derivation
implementation
license
patent review
status
```

For each non-trivial algorithm:

* [ ] identify mathematical source
* [ ] record public references
* [ ] record implementation derivation
* [ ] record contributor provenance
* [ ] review patent concerns
* [ ] document uncertainty

Source classifications:

```text
GREEN
Public-domain mathematics
Textbooks
Academic papers
Expired patents
Public standards where legally appropriate

YELLOW
Open-source implementations
Modern patents
Ambiguous licensing
Vendor documentation

RED
Proprietary implementation source
Confidential information
Restricted specifications
Unclear ownership
```

Do not copy implementation code from external projects.

---

# 49. Clean-Room Development

For native codec implementations:

* [ ] document algorithm sources
* [ ] maintain implementation provenance
* [ ] avoid copying implementation structure from reference codecs
* [ ] do not derive code from proprietary source
* [ ] document external references used
* [ ] maintain codec-specific design notes

The purpose is provenance and independent implementation, not an absolute legal guarantee.

---

# 50. Documentation

Create:

* [ ] README
* [ ] ARCHITECTURE
* [ ] codec design guide
* [ ] container specification
* [ ] pipeline specification
* [ ] DSP dependency guide
* [ ] performance guide
* [ ] security model
* [ ] provenance/IP policy
* [ ] contributor guide
* [ ] API documentation
* [ ] examples

---

# 51. Cross-Repository Integration

## `tpt-dsp`

`tpt-media` should consume:

```text
FFT
DCT
MDCT
LPC
pitch
VQ
psychoacoustics
filters
resampling
SIMD
```

* [ ] establish versioning strategy
* [ ] establish MSRV compatibility
* [ ] establish feature flags
* [ ] establish benchmark ownership
* [ ] establish API stability expectations

## `tpt-kinetix`

`tpt-media` should not duplicate standards implementation unnecessarily.

Potential future relationship:

```text
tpt-kinetix
    │
    └── standards codecs

tpt-media
    │
    └── TPT-native codecs
```

Shared generic DSP comes from:

```text
tpt-dsp
```

---

# 52. Dependency Graph

Target:

```text
                  tpt-media-core
                         │
          ┌──────────────┼──────────────┐
          │              │              │
   bitstream          codec        container
          │              │              │
          └──────────────┼──────────────┘
                         │
                      pipeline
                         │
                       apps
                         │
                         ▼
                      tpt-dsp
```

More accurately, DSP should be an independent lower-level dependency:

```text
              tpt-dsp
                 ▲
                 │
          ┌──────┴──────┐
          │             │
      tpt-media     tpt-kinetix
```

Avoid cyclic dependencies.

---

# 53. Release Gates

Before 0.1:

* [ ] workspace builds
* [ ] CI operational
* [ ] basic media model
* [ ] timestamp model
* [ ] bitstream primitives
* [ ] container prototype
* [ ] one working codec
* [ ] fuzz infrastructure
* [ ] documentation baseline

Before 0.2:

* [ ] MDCT
* [ ] LPC
* [ ] VQ
* [ ] improved DSP APIs
* [ ] Pulse prototype
* [ ] Vox prototype
* [ ] container streaming
* [ ] basic pipeline

Before 0.3:

* [ ] Aura prototype
* [ ] Livelink prototype
* [ ] WASM decoder
* [ ] stable container specification
* [ ] benchmark suite

Before 0.4:

* [ ] Vault prototype
* [ ] Whisper prototype
* [ ] advanced quality testing
* [ ] security hardening

Before 1.0:

* [ ] stable public APIs
* [ ] stable `.tpt` specification
* [ ] stable codec specifications
* [ ] compatibility policy
* [ ] comprehensive fuzzing
* [ ] reproducible benchmarks
* [ ] documented security model
* [ ] documented provenance
* [ ] FFI stability
* [ ] WASM stability
* [ ] production-quality implementations

---

# 54. Final Architectural Acceptance Criteria

`tpt-media` is architecturally complete when:

```text
                    TPT MEDIA
                        │
       ┌────────────────┼────────────────┐
       │                │                │
    Container         Codec           Pipeline
       │                │                │
       │         ┌──────┴──────┐         │
       │         │             │         │
       │      Native       Standards     │
       │       codecs        codecs      │
       │         │             │         │
       └─────────┴─────────────┴─────────┘
                         │
                      tpt-dsp
                         │
          ┌──────────────┼──────────────┐
          │              │              │
       Transforms      Signal       Analysis
```

The architecture must satisfy:

* [ ] no duplicated DSP implementations
* [ ] no codec-specific DSP in `tpt-dsp`
* [ ] no media/container abstractions in `tpt-dsp`
* [ ] no cyclic dependencies
* [ ] real-time paths are allocation-free
* [ ] malformed media cannot cause unbounded resource consumption
* [ ] native codecs have explicit versioned specifications
* [ ] `.tpt` is independently implementable
* [ ] public APIs have documented contracts
* [ ] numerical algorithms have documented conventions
* [ ] optimized implementations have reference implementations
* [ ] provenance is recorded
* [ ] security testing is continuous
* [ ] the resulting crates are reusable outside `tpt-media`

---

# 55. Guiding Principle

The ultimate separation is:

```text
tpt-dsp
    = transform and understand signals

tpt-media
    = represent, encode, transport, synchronize and store media

tpt-kinetix
    = implement established media standards
```

A future TPT application should therefore be able to depend on:

```text
tpt-dsp
```

without importing media infrastructure, and:

```text
tpt-media
```

without needing to know how its underlying DSP algorithms are implemented.

This is the intended long-term architecture of the TPT media stack.

This gives you a single master checklist, while still making it clear to an agent that some [ ] items belong to the tpt-dsp repository rather than being implemented inside tpt-media.

One thing I particularly like about this arrangement is that if you later decide to build something like RF codecs, scientific signal codecs, telemetry compression, or robotics signal processing, the DSP work we've done for media isn't trapped inside tpt-media; it becomes part of the reusable TPT foundation.