# Web Player Design (`tpt-web`)

Design notes for `apps/tpt-web` (todo.md §45): a browser player for `.tpt`
media whose decoding runs in WebAssembly. The user-facing overview is
[`apps/tpt-web/README.md`](../apps/tpt-web/README.md); this document records
*why* it is built the way it is, what is verified and how, and what is
deliberately not there yet.

## 1. Scope

In scope: the four decoders that exist today — Cinder, Aura, Reed (audio) and
Thrum (video) — behind an incremental `.tpt` parser, a streaming player, a C
ABI for `wasm32-unknown-unknown`, hand-written JavaScript glue, and a
no-bundler demo page.

Out of scope, on purpose:

- **Vault and Mote** (todo.md §33/§34) are implemented separately and are not
  dependencies of this crate. §8 below is the extension point that lets them
  in without touching the parser, player or ABI.
- **Encoding, muxing, seeking, subtitles, DRM, adaptive streaming.** This is a
  decode-and-present path for a single `.tpt` byte stream.

## 2. Layering

```text
 browser page          index.html ── tpt-web.js ────────────┐  WebAudio, canvas, fetch,
 (JavaScript)                        (clock, scheduling)    │  rAF, pause: browser APIs
                                     tpt-decoder.js ────────┤  no DOM: also runs in Node
                                     (ABI marshalling)      │
 ─────────────────────────────────────────────────────────────────────────────────────
 wasm C ABI            abi.rs        tpt_web_* over integer handles      18 exports
 ─────────────────────────────────────────────────────────────────────────────────────
 plain Rust            player.rs     Player: push / pump / pop, queues, limits, timing
 (host-testable)       parser.rs     StreamParser: bounded push parser -> events
                       codecs.rs     DecoderTable: codec ID -> DynDecoder builder
                       convert.rs    S16/S24/F32.. -> planar f32; YUV/NV12/RGB -> RGBA8
                       limits.rs     WebLimits: hard bounds, ceiling, memory estimate
                       synth.rs      (feature `synth`) deterministic media via real encoders
 ─────────────────────────────────────────────────────────────────────────────────────
 workspace crates      tpt-container (format decoders), tpt-media-codec (DynDecoder,
                       ResourceLimits), tpt-audio-{cinder,aura,reed}, tpt-video-thrum
```

Everything from `player.rs` down is ordinary Rust with no `cfg(target_arch)`.
It builds and is unit-tested natively, and the same source is what the wasm
module contains. The ABI layer holds no logic beyond argument checking; the
JavaScript layer holds no decoding logic at all.

## 3. Why a plain C ABI and not `wasm-bindgen`

`ARCHITECTURE.md` originally expected `wasm-bindgen`. The implementation does
not use it, for three reasons:

1. **The surface is integers and pointers.** Bytes go in through a staging
   region, decoded audio/video come out as a pointer plus a length, and every
   other value is a number. `wasm-bindgen` earns its keep when a Rust API has
   to exchange strings, structs, closures and JS objects; this one does not.
2. **Dependencies (todo.md §1).** `wasm-bindgen` is a dependency, a
   build-time CLI that must be version-matched to it, and a generated JS/TS
   layer that owns the module's instantiation. The C ABI is zero dependencies
   and one `cargo build`.
3. **The JS is the documentation of the ABI.** `tpt-decoder.js` is ~200 lines
   and states every memory rule in one place. A generated binding would hide
   exactly the parts (view invalidation on `memory.grow`) that callers most
   need to understand.

The cost is that the JS side is hand-maintained; `ABI_VERSION` (checked by
`TptModule.load`) and `web/check.mjs` (which runs the JS against the built
module in CI) keep the two halves honest. If the API ever needs richer types,
`wasm-bindgen` can be added behind a feature without changing `player.rs`.

## 4. ABI and memory model

Full reference: the module docs of [`src/abi.rs`](../apps/tpt-web/src/abi.rs).
The rules that matter:

- **No pointer flows from JS into Rust.** Input is staged in a buffer the
  *module* owns (`tpt_web_input_ptr(h, len)` returns its address, JS writes,
  `tpt_web_input_commit(h, len)` pushes it). This is what lets the parser and
  player be safe Rust; only `abi.rs` has `unsafe` (`#[no_mangle]` trips the
  `unsafe_code` lint in edition 2021, and it hands out raw pointers), and even
  there the only pointers are `Vec::as_ptr()` of buffers it owns.
- **Output is pull-based and single-slot.** `tpt_web_audio_next` /
  `tpt_web_video_next` move the next item into a per-handle "current" slot;
  `*_ptr` and `*_info` describe it. The pointer is valid until the next call
  taking the same handle.
- **Views are ephemeral.** Any call that allocates can grow linear memory and
  detach `memory.buffer`. `tpt-decoder.js` builds a fresh view after every
  call and documents that returned `planar` / `rgba` views must be consumed
  immediately.
- **Errors** are negative codes (`WebError::code`) plus a message readable via
  `tpt_web_error_ptr/len`. `-2` (`InputFull`) is the only retryable one.
- **Handles** are small non-zero integers in a thread-local table, at most 4
  open at once. A closed or invalid handle returns `-1`; it is never
  undefined behaviour.

`panic = "abort"` (the workspace release profile) makes a panic a wasm trap:
the instance stops rather than running on with broken state. No known input
reaches one (see §7), and a page can simply instantiate a fresh module.

## 5. The `.tpt` parser

`tpt_container::TptDemuxer` borrows one contiguous `&[u8]` for its lifetime.
A browser delivers bytes in `fetch()` chunks and cannot hold the whole file
(or a self-referential buffer-plus-demuxer). So `StreamParser` is a small push
parser that owns a *bounded* buffer and drops bytes as they are consumed.

It reuses the container crate's own decoders (`FileHeader::from_bytes`,
`decode_trak`, `decode_tmta`, `decode_clus`, `read_chunk_header`), so the wire
format still has exactly one implementation. What is new is the framing state
machine and the limits:

- A chunk *claiming* a size above its kind's limit is rejected from the 8-byte
  chunk header alone, before any of its body is buffered.
- `IDX `, `CRC1`, reserved and unknown chunks are skipped **without
  buffering**: a claimed 200 MiB unknown chunk costs no memory (tested: 200
  MiB streamed through, capacity stays under 1 KiB).
- A malformed `CLUS`, or one for an undeclared track, is skipped and counted;
  a `CLUS` whose embedded CRC-32 mismatches is dropped and counted, not
  decoded. A malformed `TRAK`/`TMTA` is fatal, since a wrong track table would
  mis-decode everything after it.
- Streaming is native to the design: no end-of-file index is needed, exactly
  as todo.md §25 requires of the container (`IDX `/`CRC1` are optional and
  ignored). Output is byte-for-byte identical whether the file arrives whole
  or one byte at a time (tested).

## 6. The player: flow control and timing

`Player::pump` decodes until the next packet's output queue is full
(`max_audio_queue_ms`, `max_video_queue_frames`), holds that one packet, and
returns. With the parser's bounded input buffer this is end-to-end
back-pressure: `push` returns `InputFull` once the backlog is full, so a fast
network cannot outrun playback.

Timestamps: audio is stamped from cumulative sample counts (drift-free) and
re-anchored to packet timestamps at the start, after a `DISCONTINUITY` flag
and after a dropped packet. Video takes the producing packet's timestamp,
because the codec-agnostic `DynFrame` carries none. Codec block padding is
trimmed from the tail using the `tpt.io.sample_count` track metadata that
`tpt encode` writes, so a 4.000 s source plays as exactly 192 000 samples
(Aura lossless reproduces the source bit-exactly; tested).

A packet that fails to *decode* is skipped, counted, and remembered; one
corrupt packet costs a glitch, not the stream. `max_decode_errors` bounds the
tolerance so hostile input still terminates.

## 7. Resource limits (todo.md §19)

`WebLimits` wraps `tpt_media_codec::ResourceLimits` (handed to every decoder)
with the player-level bounds a browser needs. Every bound is checked *before*
allocating, because wasm linear memory only grows and a failed growth aborts:

| Bound | Default | Ceiling (`clamped`) | Enforced by |
| --- | ---: | ---: | --- |
| unparsed input buffered | 16 MiB | 64 MiB | `StreamParser::push` → `InputFull` |
| one packet (`CLUS` body) | 4 MiB | 16 MiB | parser, from the chunk header |
| one metadata chunk (`TMTA`) | 64 KiB | 1 MiB | parser, from the chunk header |
| one track descriptor (`TRAK`) | 256 KiB | 256 KiB | parser |
| decoded frame, native and RGBA | 8 MiB | 64 MiB | track geometry at declaration + each frame |
| decoder-internal allocation | 32 MiB | 128 MiB | the codec (`check_allocation`) |
| decoded audio queued | 2000 ms | 30 s | `pump` stalls; hard stop at 2x |
| decoded pictures queued | 4 | 32 | `pump` stalls; hard stop at 2x |
| tracks | 16 | 64 | parser |
| undecodable packets | 256 | 4096 | player → `TooManyDecodeErrors` |
| open players per module | 4 | 4 | ABI |
| slice staged per `input_ptr` | 4 MiB | 4 MiB | ABI |

`WebLimits::memory_ceiling_bytes()` is the documented worst case the player
itself can own under a given set of limits: 2x the input buffer (for `Vec`
growth overshoot) + staging + one packet + the codec budget + audio at its
hard stop + pictures at their hard stop (each RGBA picture is itself bounded
by `max_frame_bytes`). **For the defaults that is 223 MiB** (233 701 376 B) —
dominated by eleven pictures of up to 8 MiB — and a test pins the figure to
200–260 MiB so it cannot drift silently. Real
streams sit far below it: `web/check.mjs` measures 196 736 bytes held by the
player for the 4 s demo, and the wasm instance's total linear memory after
playing it is 1.8 MiB (7.5 MiB after the 640x352 benchmark inputs).

Wasm needs that ceiling **plus** about 1 MiB of static data and stack, plus
allocator fragmentation. `web/build.mjs [MiB]` links with
`-C link-arg=--max-memory=<bytes>` (default 512 MiB) so an instance can never
grow past a fixed cap regardless of input; `check.mjs` reports the declared
maximum it reads back from the built module.

Not yet used: `max_decode_micros` and cancellation (todo.md §19 leaves both
open — no codec checks them), and `max_recursion_depth` (no recursive decode
paths).

## 8. Extension point for Vault and Mote

`DecoderTable` maps `(codec ID, media type)` to a `DecoderBuilder`
(`fn(&Track, &ResourceLimits) -> Result<Box<dyn DynDecoder>, WebError>`). The
player, parser and ABI know nothing about concrete codecs; they speak
`DynDecoder` / `DynFrame`. To add a codec:

1. Write a builder that reads what it needs from the `Track` (geometry,
   sample rate, codec-private bytes), validates it, and returns the codec's
   `DynDecoder`.
2. Either `table.register(*b"XXXX", MediaType::Video, "name", build_xxx)` on a
   table given to `Player::with_table`, or add one line to
   `DecoderTable::native()` once the codec crate is a dependency of `tpt-web`.

`a_registered_codec_plays_without_touching_the_player` (a fake codec) proves
step 2 needs no other change. Two constraints a new codec meets:

- Video codecs' `DynFrame` output must be one of `Gray8`, `Rgb24`, `Rgba32`,
  `Yuv420`, `Nv12` (extend `convert::video_to_rgba` for more). YUV is
  converted as full-range BT.601, which is what Thrum documents; a codec with
  different colorimetry needs its own conversion.
- The audio path assumes at most 8 channels and converts any `SampleFormat`.

The audio builders do **not** go through `tpt_media_codec::Registry`: the
Cinder/Aura/Reed registry factories build from `Default` configs and ignore
the track, so a 44.1 kHz file or a non-default Cinder bit depth would decode
wrongly. The builders instead mirror `tpt-cli`'s track-to-decoder logic.
Thrum's factory is per-geometry and is built per track.

## 9. The browser side

`tpt-web.js` makes the `AudioContext` clock the master clock for audio-only,
video-only and audio+video alike: `t0` maps media time to context time and is
anchored on the first chunk or picture. Audio is scheduled ahead (0.5 s) with
sample-accurate `start(when)`, and popped from the module only when there is
room, so the wasm queues provide the back-pressure. Pictures are presented
from `requestAnimationFrame` when due, dropping stale ones if a newer one is
also due. An underrun re-anchors `t0` rather than racing to catch up. Pause is
`AudioContext.suspend()`, which stops the master clock and so video too.
Playback defaults to 8 queued pictures because packets decode in file order
and a full video queue also holds back the audio behind it.

## 10. What is verified, and how

| Claim | Evidence | Where |
| --- | --- | --- |
| Parser, player, conversions, limits, ABI logic | 49 native tests (45 unit + 3 public-API + 1 doctest) incl. bit-exact Aura round trip, Cinder vs. a directly driven decoder, byte-at-a-time = whole-file, deterministic fuzz | `cargo test -p tpt-web` |
| Builds as a real wasm module | `cargo build -p tpt-web --target wasm32-unknown-unknown --release` (328 KiB) | `web/build.mjs`, CI `wasm` job |
| The wasm + JS glue decode correctly and survive hostile input | 11 checks under Node: exact sample/frame counts, 300 mutated/truncated/noise files with zero traps, handle and limit behaviour | `web/check.mjs` |
| `tpt-web.js` control flow (push, back-pressure, scheduling, pacing, end, stop, errors) | executed against the real wasm with **fake** AudioContext/canvas | `web/mock-browser.mjs` |
| **Real WebAudio and real canvas output** | 29 checks in headless Chrome against the *unmodified* player: real `AudioContext` created and released (`state` `closed`, `baseLatency` 0.01 s), 375 real `AudioBufferSourceNode`s totalling 4.0000 s scheduled back to back, canvas read back with `getImageData` (2834 distinct colours, 15 360/15 360 opaque, 0 dropped, 0 decode errors), plus the shipped `index.html` driven in an iframe to end of stream. Repeated 3x, 29/29 each, ~8.8 s. | `web/browser-check.mjs` + `web/serve.mjs` |

`web/browser-check.mjs` is the counterpart to `mock-browser.mjs`, not a
replacement for it: the fake run can be a gate in CI, the real one needs a
browser and an audio device, so the two answer different questions and both
are kept. It needs a real browser to exist, and says so and exits non-zero
if it cannot find one, rather than skipping — a missing browser must never
read as a pass.

Two limits are deliberate and worth stating plainly. It proves the browser
*accepted* the audio and advanced its clock; it cannot prove anyone *heard*
anything, so "audible output" is still unproven in the literal sense. And on
a machine with no audio output device there is no clock to advance, so the
playback-dependent checks report `SKIP` ("not verifiable here") instead of
passing or failing — a pass there means the page loaded, the wasm decoded
and the canvas is real, not that playback ran.

## 11. Known limitations

- No seeking (the parser is forward-only; `tpt_container` seeking needs the
  whole file). One audio and one video track are played; others are counted
  in `ignored_tracks`.
- Decoding runs on the page's main thread. At 100x+ real time for audio and
  tens of fps for video that is comfortable for the fixtures measured, but a
  Worker (with the module instantiated there) is the right home for large
  video; the design allows it (`tpt-decoder.js` has no DOM dependency), but it
  is not built.
- Audio is scheduled as one `AudioBufferSourceNode` per decoded packet
  (93/s for Cinder). Correct, gapless by construction, but not the most
  efficient shape; coalescing packets into larger buffers is a straightforward
  follow-up.
- Wasm linear memory never shrinks; a long session's footprint is its peak.
- Performance numbers are in [`performance.md`](performance.md); the video
  fixtures are highly compressible synthetic content, so their MB/s says
  nothing about real footage.
