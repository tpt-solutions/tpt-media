# tpt-web

Browser player core for `.tpt` media: a WASM-compatible streaming decoder with hard memory limits and a plain C-ABI export surface.

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](../../LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)

Decodes `.tpt` audio (Cinder, Aura, Reed) and video (Thrum) inside
WebAssembly and hands planar-`f32` audio and RGBA8 pictures to a page that
plays them with WebAudio and a canvas. Part of the
[`tpt-media`](../../README.md) workspace —
[workspace changelog](../../CHANGELOG.md) · [changelog](CHANGELOG.md) ·
[web design](../../docs/web-design.md) · [contributing](../../CONTRIBUTING.md).

## At a glance

| | |
|---|---|
| Role | WASM browser player (todo.md §45) |
| Artifact | `cdylib` → `tpt_web.wasm` (328 KiB release) plus an `rlib` for native tests |
| Codecs | Cinder, Aura, Reed (audio); Thrum (video). Vault and Mote are **not** dependencies; see [Adding a codec](#adding-a-codec) |
| Bindings | Plain `extern "C"` over integer handles + hand-written JS. **No `wasm-bindgen`** ([why](../../docs/web-design.md#3-why-a-plain-c-abi-and-not-wasm-bindgen)) |
| Dependencies | Only workspace crates; nothing external |
| `no_std` | No: an app is exempt from the `no_std`-first rule (CONTRIBUTING §10), not from §9.4 |
| Version | `0.1.0`, lockstep with the workspace |
| MSRV | 1.86 |
| License | MIT OR Apache-2.0 |

## Try the demo

```bash
rustup target add wasm32-unknown-unknown        # once
node apps/tpt-web/web/build.mjs                 # builds web/tpt_web.wasm (512 MiB memory cap)
cd apps/tpt-web/web && python -m http.server    # any static server works
# open http://localhost:8000/ and press "Play demo.tpt"
```

`web/` is a static folder: `index.html`, `tpt-web.js` (playback),
`tpt-decoder.js` (ABI glue), `demo.tpt` (4 s of synthetic Cinder audio and
160x96 Thrum video, regenerable with
`cargo run -p tpt-web --example make_demo`). No bundler, no npm. The page can
also play a local file (file picker) or stream a URL.

> **Browser-tested, with one stated limit.** The decoder and JS glue are
> exercised end to end under Node against the real wasm (`web/check.mjs`), the
> playback logic of `tpt-web.js` is run against fake `AudioContext`/canvas
> objects (`web/mock-browser.mjs`), **and** the unmodified player is run in
> headless Chrome against the real browser APIs
> (`web/browser-check.mjs`): 29 checks covering a real `AudioContext`'s
> lifecycle and scheduling, a real canvas read back with `getImageData`, and
> the shipped `index.html` driven to end of stream.
> The limit: that run proves the browser accepted the audio and advanced its
> clock, not that a human heard it, and on a machine with no audio device the
> playback checks report "not verifiable here" rather than passing.

## Using it from JavaScript

```js
import { TptModule, Status } from "./tpt-decoder.js";

const mod = await TptModule.load("./tpt_web.wasm");
const dec = mod.open({ maxVideoQueueFrames: 8 });     // every limit optional, clamped by the module

for await (const chunk of fetchChunks("song.tpt")) {  // Uint8Arrays from fetch().body
  let rest = chunk;
  while (rest.length) {
    rest = rest.subarray(dec.push(rest));             // returns bytes consumed; short = buffer full
    if (rest.length) { dec.pump(); drain(); }         // back-pressure: drain, then retry
  }
  dec.pump();
  drain();
}
dec.finish();
while (dec.pump() !== Status.ENDED) drain();
drain();

function drain() {
  for (let a; (a = dec.nextAudio()); ) { /* a.planar: Float32Array, channel-major; a.pts seconds */ }
  for (let v; (v = dec.nextVideo()); ) { /* v.rgba: Uint8ClampedArray for ImageData; v.pts */ }
}
```

`planar` / `rgba` are views straight into wasm memory, valid only until the
next call: copy them out (`copyToChannel`, `putImageData`) immediately. Errors
arrive as `TptError` with the negative ABI `code`. `tpt-web.js` wraps all of
this — fetching, WebAudio scheduling ahead of the clock, canvas pacing,
pause, stats — as `new TptPlayer({ canvas, onEnded, onError, ... }).play(urlOrFile)`.

## Using it from Rust

The player is plain Rust; the wasm layer is a veneer. Everything below runs
natively and is what the unit tests cover.

```rust
use tpt_web::{Player, Status, WebError, WebLimits};

let mut player = Player::new(WebLimits::default());
for chunk in file.chunks(16 * 1024) {
    loop {
        match player.push(chunk) {
            Ok(()) => break,
            Err(WebError::InputFull) => {                 // retryable: drain, push again
                player.pump()?;
                while let Some(_a) = player.pop_audio() {}
                while let Some(_v) = player.pop_video() {}
            }
            Err(e) => return Err(e),
        }
    }
}
player.finish_input();
while player.pump()? != Status::Ended {
    while let Some(a) = player.pop_audio() { /* a.planar, a.pts_seconds, a.sample_rate */ }
    while let Some(v) = player.pop_video() { /* v.rgba, v.width, v.height */ }
}
```

## API surface

| Layer | Item | Purpose |
|---|---|---|
| Rust | `Player` | `push` / `finish_input` / `pump` / `pop_audio` / `pop_video`, `info`, `stats`, `memory_in_use` |
| Rust | `StreamParser` | bounded incremental `.tpt` parser: `push` bytes, `poll` `ParseEvent`s |
| Rust | `DecoderTable` | codec ID → decoder builder; **the extension point** |
| Rust | `WebLimits` | hard bounds, `clamped()`, `ceiling()`, `memory_ceiling_bytes()` |
| Rust | `audio_to_planar_f32`, `video_to_rgba` | sample/pixel conversion |
| Rust | `synth` (feature) | deterministic test media via the real encoders |
| C ABI | `tpt_web_*` (18 exports) | open/close, input staging, pump/status, info, audio/video pull, error message; see [`src/abi.rs`](src/abi.rs) |
| JS | `TptModule` / `TptDecoder` | ABI marshalling; runs in browser, Worker and Node |
| JS | `TptPlayer` | fetch + WebAudio + canvas playback |

## Memory limits

Every bound is checked before allocating (wasm memory never shrinks and a
failed growth aborts the module). Defaults are conservative (strict codec
limits, 16 MiB input backlog, 2 s of audio, 4 pictures, 8 MiB per frame);
callers may raise them but everything is clamped to a compiled-in ceiling.
Worst case player-owned memory under the defaults is 223 MiB
(`WebLimits::memory_ceiling_bytes`); the 4 s demo peaks at ~192 KiB. The full
table and rationale are in [`docs/web-design.md`](../../docs/web-design.md)
section 7 and [`docs/security.md`](../../docs/security.md).
`node web/build.mjs [MiB]` links with `--max-memory` to cap the instance
outright.

## Adding a codec

Vault and Mote are deliberately absent. When one lands, write a
`DecoderBuilder` (`fn(&Track, &ResourceLimits) -> Result<Box<dyn DynDecoder>,
WebError>`) and either `table.register(...)` it on a table passed to
`Player::with_table`, or add one line to `DecoderTable::native()`. Nothing
else changes; see [`docs/web-design.md`](../../docs/web-design.md) section 8.

> **The extension point is Rust-only.** `tpt_web_open` builds its player with
> `DecoderTable::native()` and no ABI function accepts a table (a
> `DecoderBuilder` is a Rust `fn` pointer, which cannot cross the wasm
> boundary). So from a browser the only route is: add the codec as a dependency
> of `tpt-web` and put one line in `DecoderTable::native()`. `register` is for
> Rust callers embedding the `rlib`. This is a real limitation, not a design
> intent — it is why the two video codecs play in the CLI and not in the web
> player.

## Testing

```bash
cargo test -p tpt-web                                    # 49 tests: parser, player, ABI, conversions, limits, fuzz sweeps
cargo clippy -p tpt-web --all-targets -- -D warnings
node apps/tpt-web/web/build.mjs                          # wasm32 build
node apps/tpt-web/web/check.mjs                          # built wasm + JS glue under Node (11 checks)
node apps/tpt-web/web/mock-browser.mjs                   # tpt-web.js logic vs fake WebAudio/canvas
node apps/tpt-web/web/browser-check.mjs                  # the real thing: 29 checks in headless Chrome
```

The last one is the only check that needs a real browser and a real audio
device; it finds Chrome or Edge itself (override with `TPT_BROWSER`), exits
non-zero if it cannot find one, and prints `SKIP` rather than a pass for
anything that needs an audio clock it cannot get.

The test inputs are made by the real encoders (`synth`, enabled for this
crate's own tests through a self dev-dependency): among them an Aura lossless
round trip that must come back bit-exact, a Cinder decode compared against a
directly driven decoder, byte-at-a-time = whole-file output, and deterministic
mutation/truncation/noise sweeps that must never panic or exceed the memory
ceiling.

## Benchmarks

```bash
cargo bench -p tpt-web                                   # native table
cargo run -p tpt-web --example make_demo -- --bench-dir bench-inputs
node apps/tpt-web/web/bench.mjs bench-inputs             # the same files as wasm, under Node/V8
```

Measured numbers (native and wasm, with their caveats) are in
[`docs/performance.md`](../../docs/performance.md). They are Node/V8 numbers,
not browser numbers.

## Known limitations

- **Never run in a real browser** (see above).
- No seeking; one audio and one video track are played (others are counted).
- Decodes on the page's main thread; a Worker deployment is possible
  (`tpt-decoder.js` has no DOM dependency) but not built.
- Audio is scheduled as one `AudioBufferSourceNode` per decoded packet:
  gapless, but chattier than necessary.
- Video pacing follows the audio clock; a video-only stream is paced by the
  same `AudioContext` clock.
- Wasm linear memory never shrinks; a long session's footprint is its peak.
- `max_decode_micros` and cancellation (§19) are not used; no codec checks
  them yet.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
