# Caller-owned buffers for real-time paths

Status: **step 1 implemented for Cinder decode (2026-11-04)**; the rest of
the plan is still open. Tracks the open items in `todo.md` §1 ("zero
allocation in real-time hot paths") and §54 ("real-time paths are
allocation-free").

## Step 1 result (Cinder decode)

Implemented option B with the scope set out below: the opt-in
`RealtimeDecoder` trait in `tpt-media-codec` (`max_output_bytes`,
`decode_into`), implemented by `CinderDecoder`.

Measured with `codecs/cinder/tests/decode_alloc_count.rs`, which counts
allocation calls rather than peak bytes:

| Path | Allocation calls per packet | Output |
| --- | ---: | --- |
| Owned `Decoder::decode`, before | 8 | `Vec<AudioFrame>` |
| Owned `Decoder::decode`, after | 2 | `Vec<AudioFrame>` |
| `RealtimeDecoder::decode_into`, after | **0** | caller buffer |

The count covers 191 steady-state packets at both 256 and 128 samples,
with the frame-size switch included. `decode_into` output is byte-identical
to the owned path on every packet. The test asserts both properties.

Implementation notes:

- One decode core (`decode_block`) fills preallocated scratch, and both
  paths read from it, so they cannot drift apart. The owned path copies
  its output into a `Vec`, which is why it still allocates twice.
- Scratch buffers are allocated once at construction, at their maximum size
  (256 samples), so a 128/256 switch reuses capacity and does not reallocate.
- Both checks in `decode_into` (output size, packet limit) run before any
  decoder state changes, so a refusal leaves the stream unchanged.

Deviations from the proposal above:

- **Sinks.** The proposal put `FrameSink`/`PacketSink` types in
  `tpt-media-core`. Step 1 uses a plain `&mut [u8]` instead, because one
  codec needed nothing more and the sink types would have been speculative.
  Revisit when Reed or an encoder needs them.
- **Error.** Output-too-small reports `CodecError::InvalidConfig`. Adding a
  variant to `CodecError` would break downstream exhaustive matches, and
  it is not a 1.0-safe change. A dedicated variant is worth adding with the
  next breaking change to that enum.

## Step 2 result (Cinder encode)

The encode side needed a different shape from decode. A frame may yield
zero packets (it is buffered) or several (a large frame), so one call cannot
express the contract. `RealtimeEncoder` therefore has three operations:

- `push_frame(&Frame)`: takes in samples and emits nothing.
- `pop_packet_into(&mut [u8]) -> Option<(len, pts)>`: emits one complete
  packet, or `None` when more samples are needed.
- `drain_packet_into(&mut [u8]) -> Option<(len, pts)>`: emits the
  end-of-stream tail one packet at a time, or `None` when drained.

Measured with `codecs/cinder/tests/encode_alloc_count.rs`, at 256 and 128
samples per block:

- Zero allocation calls in `push_frame`, `pop_packet_into` and
  `drain_packet_into` at steady state.
- Packets, including the drain tail, are byte-identical to the owned
  `Encoder::encode` followed by `drain`, with the same timestamps.

The owned encoder now shares one core (`encode_block_into`) with the
real-time path. Its input rules are shared too (`accept_frame`).

Two limits to know about:

- `pending` reserves two blocks. A push of more than two blocks grows it
  once. The capacity is kept afterwards, so steady state is allocation-free,
  but the first large push allocates. Callers wanting zero allocation from
  the first push should push at most two blocks at a time.
- The owned encode's allocation count before the change was not measured,
  so no before/after figure is given for encode, unlike decode.

## Step 3 result (Reed decode)

Scope follows Reed's own capability flags. Its decoder claims real-time
operation; its encoder does not, because of the unvoiced codebook search.
So only decode was in scope. The encoder's allocations are unchanged.

`RealtimeDecoder` is implemented for `ReedDecoder`. The owned decode and
`decode_into` share one core, and synthesis moved into a free function
(`synthesize_into`) that writes into caller slices. Concealment keeps its
owned path and produces the same samples.

Measured with `codecs/reed/tests/alloc_count.rs`, at the default 16 kHz
configuration (320-sample frames):

| Path | Allocation calls per packet |
| --- | ---: |
| Owned `Decoder::decode`, before | 11 |
| Owned `Decoder::decode`, after | 2 |
| `RealtimeDecoder::decode_into`, after | **0** |

The four calls that remained were inside `tpt-dsp-core`'s `lsp_to_lpc`, which
collected the even and odd LSP indices and allocated its polynomial scratch.
They are now gone. The sibling `tpt-dsp` repo gained
`lsp_to_lpc_with_scratch(lsp, out, scratch)`, which takes caller-owned scratch
(`2·(p+2)` values) and is bit-identical to `lsp_to_lpc`. `lsp_to_lpc` remains,
as a wrapper that allocates the scratch, so existing callers are unchanged.
Reed's decoder keeps the scratch in the decoder, and the test asserts zero
allocations per packet.

This change to `tpt-dsp` is uncommitted and unreleased. The change is in the
sibling working tree on master, which already held other uncommitted work.
Publishing it is a separate decision.

Still open: Reed's encoder is not in scope, the offline codecs are untouched,
and the §54 claim stays unchecked until the other codecs are covered. (Cinder
concealment was closed in step 4.)

## Step 4 result (Cinder concealment)

Concealment now runs in the decoder's preallocated scratch. The old path
allocated in `synthesize`, `overlap_add`, the spectrum clone, and the pitch
extension, which were replaced by one in-place helper (`inverse_windowed`)
plus the existing `block`, `spec`, `synth` and `last_spectrum` buffers.

- `CinderDecoder::conceal_into(&mut [u8]) -> CodecResult<usize>` is the
  real-time form. It is an inherent method, not a `RealtimeDecoder` trait
  change, so the trait surface is unchanged. It checks the output size before
  any state changes.
- `conceal` (owned) shares the same core, `conceal_block`, and differs only in
  the final copy into a `Vec`.

Measured with `codecs/cinder/tests/conceal_alloc_count.rs`: zero allocation
calls per concealed block across 11 losses, covering the stream-start
fallback, the pitch path on a tone, the spectrum fallback on noise, and a
frame-size switch. The test also asserts `conceal_into` bytes equal the owned
`conceal` bytes.

Output was checked byte-identical to the pre-change code: a temporary golden
capture recorded 106 concealed frames (both frame sizes, both paths, stream-
start and multi-packet losses, a frame-size switch) before the rewrite and
matched it exactly after. The golden test was removed before commit.

Written for: maintainers deciding whether to accept a breaking
change to the codec traits before 1.0.

## Problem

The codec traits return owned values on every call:

```rust
// crates/tpt-media-codec/src/lib.rs
fn decode(&mut self, packet: &Packet) -> CodecResult<Vec<Self::Frame>>;
fn encode(&mut self, frame: &Self::Frame) -> CodecResult<Vec<Packet>>;
```

Each call allocates a `Vec` for the result, and each `Packet` carries an
owned `data: Vec<u8>`. The codecs keep their scratch buffers internal and
reuse them, so steady-state work is already bounded. But the public
surface still allocates per call, so the strict reading of "allocation-free
real-time path" cannot be claimed.

What is measured today: `codecs/cinder/tests/realtime_alloc.rs` (and the
Reed and Thrum equivalents) assert that a steady-state loop allocates
under a generous budget (512 KiB across 32 blocks). That guards against
per-frame growth. It does not assert zero allocations, and it cannot,
because the owned return values allocate.

## Goal

For a codec that claims real-time operation, a steady-state
`encode`/`decode` loop performs **zero heap allocations** after
construction, measured by the existing counting allocator with the budget
set to 0. Offline codecs (Aura, Thrum, Vault, Mote) are out of scope.

## Proposed shape

Add an opt-in trait beside the existing ones. The owned-`Vec` traits stay
as the convenience API, so offline callers and the CLI are unaffected.

```rust
/// Real-time decoding into caller-owned storage. No allocation in steady state.
pub trait RealtimeDecoder {
    /// Decode one packet into `out`, reusing its capacity.
    /// Returns how many frames were written. Errors leave `out` unchanged.
    fn decode_into(&mut self, packet: &Packet, out: &mut FrameSink<'_, Self::Frame>)
        -> CodecResult<usize>;
}

pub trait RealtimeEncoder {
    /// Encode one frame, appending packet bytes into `out`.
    fn encode_into(&mut self, frame: &Self::Frame, out: &mut PacketSink<'_>)
        -> CodecResult<usize>;
}
```

`FrameSink` and `PacketSink` are caller-owned, fixed-capacity buffers
(for audio, a `&mut [f32]` with a sample count; for packets, a
`&mut [u8]` with a written length). The sink types live in
`tpt-media-core` so every codec shares them.

The owned-`Vec` `Decoder` and `Encoder` traits can be implemented on top
of the new ones, so there is one code path and the two cannot drift.

## Options considered

| Option | Description | Cost | Verdict |
| --- | --- | --- | --- |
| A. Inherent methods per codec | `Cinder::decode_into` with no trait | No shared abstraction; every consumer hard-codes Cinder | Rejected: `tpt-media-pipeline` cannot be generic over it |
| **B. Opt-in trait (proposed)** | `RealtimeDecoder`/`RealtimeEncoder` beside the existing traits | Breaking only for codecs that opt in; owned traits become wrappers | **Recommended** |
| C. Change existing traits | Replace `Vec` returns everywhere | Breaks every caller, including CLI, FFI, and wasm | Rejected for now; revisit at 1.0 |
| D. Defer | Keep the bounded-allocation claim | None | Fallback if B is rejected |

## Scope and order

1. **Cinder decode only.** It is the codec that claims real-time, and the
   allocation test already exists. Measure first: count allocations in the
   current steady-state decode, and identify each source before changing
   anything.
2. **Cinder encode.** Same treatment.
3. **Reed** (`no_std`, bare-metal target), which uses the sinks without
   `alloc` if the sinks take slices. Check that the `thumbv7em-none-eabi`
   CI job still passes.
4. Thrum and Mote only if a real-time consumer appears. Neither claims
   real-time today.

Each step lands with: the zero-allocation assertion (budget 0, not
512 KiB), bit-identical output against the owned-`Vec` path, and a
`CHANGELOG.md` entry under the wire-version rules in CONTRIBUTING §9.

## Breaking-change impact

- No wire-format change. Packets and bitstreams are untouched.
- Additive trait change: existing `Decoder`/`Encoder` users compile
  unchanged.
- Codecs that opt in gain a new trait impl. FFI (`tpt-media-sys`) and
  `tpt-web` do not need changes in step 1.
- The `Packet` struct does not change in step 1.

## Risks

- **Sink sizing.** Callers must size output buffers. A too-small sink
  must return a clear error, not truncate silently. Each codec documents
  its maximum frame size so callers can size once.
- **Allocator noise.** The test harness allocates on other threads. The
  zero-budget test must run single-threaded, or the counting must be
  scoped to the current thread (the existing test already has the
  `TRACKING` flag; check it isolates per thread before relying on 0).
- **Duplicated logic.** The owned wrappers must call the sink path, not
  duplicate it. A test that compares both paths guards this.

## Decisions needed

1. Accept option B (opt-in traits) over option D (defer)?
2. Start with Cinder decode only (step 1), or include encode in the first
   change?
3. Sink types in `tpt-media-core` (shared) or in each codec crate?
4. Is a 0-allocation claim for Cinder's real-time path a 1.0 gate, or a
   pre-1.0 goal?

Once these are answered, implementation starts with step 1 and the
allocation measurement in its first commit.
