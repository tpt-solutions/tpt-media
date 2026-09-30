# tpt-media-sys

Stable C ABI for the tpt-media codec abstraction: opaque handles, explicit ownership, error codes.

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](../../LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)

The C-facing surface of the workspace: a hand-written header, opaque
exclusively-owned handles, integer error codes, and no Rust types leaking
across the boundary. Underneath, the same codec objects the CLI drives do the
work. Part of the [`tpt-media`](../../README.md) workspace —
[workspace changelog](../../CHANGELOG.md) · [changelog](CHANGELOG.md) ·
[contributing](../../CONTRIBUTING.md).

## At a glance

| | |
|---|---|
| Role | Stable C ABI (todo.md §46, first slice) |
| ABI version | `ABI_VERSION = 1`, queryable at runtime via `tpt_abi_version()` |
| Header | `include/tpt_media.h` (hand-written, test-enforced) |
| Status | Implemented for the audio family (Cinder, Aura, Reed) |
| Version | `0.1.0`, lockstep with the workspace |
| MSRV | 1.86 — verified, enforced by CI's `msrv` job |
| License | MIT OR Apache-2.0 |
| Builds as | `rlib` (Rust consumers and tests) **and** `staticlib` (`tpt_media_sys.lib` for C linkers) |
| `unsafe` | The workspace's one sanctioned `unsafe` home (CONTRIBUTING §4); every block carries a `// SAFETY:` note |

## Building and linking from C

```bash
cargo build -p tpt-media-sys
# staticlib: target/debug/tpt_media_sys.lib  (tpt_media_sys.a on Unix)
```

```c
#include "tpt_media.h"   /* include/tpt_media.h */

TptDecoder *dec = NULL;
tpt_decoder_new(TPT_CODEC_AURA, &cfg, &dec);

TptPacket *pkt = NULL;
tpt_packet_new(bytes, len, ts, TPT_PACKET_FLAG_KEYFRAME, &pkt);

TptFrame *frame = NULL;
int rc = tpt_decoder_decode(dec, pkt, &frame);
if (rc != TPT_OK) { /* handle tpt_error */ }

tpt_frame_samples(frame);   /* const view, valid until tpt_frame_free */
tpt_frame_free(frame);
tpt_packet_free(pkt);
tpt_decoder_free(dec);
```

The real C integration test is `tests/integration.c`: it sees **only** the
header and drives a bit-exact Aura lossless round trip through `cargo test`.

## ABI stability rule

`tpt_abi_version()` reports the ABI revision of the loaded library.

- **Compatible:** adding functions or error codes. The header grows, the
  version holds.
- **Breaking:** changing a signature, an error code's meaning, a struct
  layout, or removing anything. This bumps the version.

## Ownership

- Every allocating call returns its handle through a `tpt_xxx**` out-parameter;
  the caller owns it and releases it with the matching `tpt_xxx_free`.
  Freeing NULL is a no-op.
- `tpt_packet_new` **copies** the payload bytes; the caller keeps ownership of
  its own buffer.
- `tpt_frame_samples` returns a pointer into the frame's owned buffer, valid
  until `tpt_frame_free`.
- `const` handles are read-only; mutable handles are used only within the
  call.
- NULL is never dereferenced — argument validation returns
  `TPT_ERR_NULL_ARGUMENT`.

## Thread safety

Every handle is exclusively owned in the Rust `&mut` sense: two threads must
never call into the *same* handle concurrently, even for nominally read-only
accessors. Distinct handles may be used concurrently from distinct threads.
Handles are not guaranteed `Send` in v0.1 — create and use a handle on one
thread.

## Panics

No Rust panic unwinds across the ABI (undefined behaviour in `extern "C"`).
Every exported function body runs under `catch_unwind`; a caught panic becomes
`TPT_ERR_PANIC` and the handle involved should be considered poisoned.
Argument validation happens before construction, so invalid input is an error
code, not a panic.

## Configuration

`tpt_media_config` is abstract on purpose — sample rate, channels and
`quality 0..=100` — and maps onto per-codec parameters exactly as the CLI's
`--quality` does, so a C caller never sees a per-codec parameter struct.
`channels` must be 1: the native audio family is S16 mono.

## Scope

This is §46's first slice: the codec abstraction (packet, frame, encoder,
decoder) over the audio family. `Context`, `Container`, `Demuxer` and `Muxer`
handles are the recorded v0.2 surface. The video codecs gain the FFI with
their own abstraction slice.

PTS association lives at the boundary: the type-erased `DynFrame` carries no
timestamps, so the ABI owns them.

## Testing

```bash
cargo test -p tpt-media-sys
```

- `tests/round_trip.rs::header_matches_the_abi` — a two-directional check that
  every exported symbol appears in the header and vice versa, so the
  hand-written header cannot drift from the implementation.
- `tests/integration.c` — a real C program compiled by `build.rs`, seeing only
  the header, driving a bit-exact Aura lossless round trip.
- Rust-side round-trip and handle-ownership tests.

## Known limitations

- Audio family only; container/pipeline handles are v0.2.
- `staticlib` output means C callers link the archive themselves; no shared
  library or dynamic-loader story yet.
- Handles are not `Send`, so cross-thread pipelines need one handle per
  thread.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
