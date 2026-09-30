# tpt-media-pipeline

Node-based media pipeline (Source → Demux → Decode → Filter → Encode → Mux → Sink).

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](../../LICENSE-MIT)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)
[![no_std](https://img.shields.io/badge/no__std-compatible-green.svg)](https://doc.rust-lang.org/1.86.0/core/)

The stream-processing core: a linear, typed composition of pullable `Node`
stages, driven either synchronously (`Pipeline::pull`, deterministic by
default) or against an injected clock for real-time work
(`drive_scheduled`). Part of the [`tpt-media`](../../README.md) workspace —
[workspace changelog](../../CHANGELOG.md) · [changelog](CHANGELOG.md) ·
[architecture](../../ARCHITECTURE.md) · [contributing](../../CONTRIBUTING.md).

## At a glance

| | |
|---|---|
| Role | Pipeline core (todo.md §26) |
| Status | Implemented (all twelve core concepts) |
| Version | `0.1.0`, lockstep with the workspace |
| MSRV | 1.86 — verified, enforced by CI's `msrv` job |
| License | MIT OR Apache-2.0 |
| `no_std` | Yes — builds for `wasm32-unknown-unknown` (CI job `wasm`) |
| Unsafe | `#![forbid(unsafe_code)]` |

## The twelve concepts, and where they live

| Concept | API |
|---|---|
| Node abstraction | `Node`, `NodeEvent` |
| Stream negotiation | `negotiation::StreamSpec`, `negotiate_stream` |
| Format negotiation | `negotiation::MediaFormat`, `FormatAware`, `negotiate_formats` |
| Backpressure | `NodeEvent::NeedMore` + the stall bound in `Pipeline::pull` → `NodeError::Backpressure` |
| Buffering | `nodes::Bounded` (batch window + hard cap) |
| Cancellation | `Node::cancel`, `nodes::Cancellable` → `NodeError::Cancelled` |
| Error propagation | `NodeError` — aborts the run, surfaced unchanged |
| Flushing | `Node::flush`, drained by `Pipeline::pull`'s tail handling |
| End-of-stream | `NodeEvent::Eos`, one-shot; later pulls are `Ok(None)` |
| Timestamps | `nodes::Timestamped` — `pts`/`dts`/`duration` survive payload transforms |
| Threading | `Executor`: `Inline` (deterministic) and std-only `ThreadedExecutor` |
| Real-time scheduling | `RealtimeProfile`, `Pipeline::drive_scheduled`, `LatencyObserver`, `LatencyLog`, `ScheduledRun` |

## Usage

```rust
use tpt_media_pipeline::{Chain, Map, Pipeline, Sink, Source};

let graph = Chain::new(
    Chain::new(Source::new(items), Map::new(analyse)),
    Sink::new(),
);

let mut pipeline = Pipeline::new(graph);
while let Some(item) = pipeline.pull(())? {
    // ...
}
```

Backpressure is explicit rather than implicit: a node that cannot produce yet
returns `NodeEvent::NeedMore`, and `Pipeline::pull` enforces `STALL_BOUND` so
a livelocked graph is reported as `NodeError::Backpressure` instead of
spinning forever.

## Feature flags

| Feature | Default | Effect |
|---|---|---|
| `std` | yes | `ThreadedExecutor` (worker-thread graph driving) |

`no_std + alloc` with `default-features = false`. `Inline` is the default
executor and is what makes pipeline behaviour reproducible.

## Design notes

`docs/pipeline-design.md` is the specification this implementation was written
against. The model is stop-free per stage, lockstep and bounded by one
coupling — `Chain<L, R>` flushes the left stage's internal buffers into the
right stage before the right stage flushes its own.

## Benchmarks & profiling

```bash
cargo bench -p tpt-media-pipeline
```

`benches/pipeline_bench.rs` drives a `Source → map → Sink` graph over 1 000
packets. Numbers are recorded in
[`docs/performance.md`](../../docs/performance.md).

## Testing

```bash
cargo test -p tpt-media-pipeline
```

21 unit tests plus a container integration test: negotiation, cascading
flush, backpressure stall bound, cancellation, EOS one-shot behaviour,
timestamp pass-through, and the real-time scheduling profile.

## Known limitations

- The graph is linear, not a DAG — fan-out/fan-in is not modelled.
- `ThreadedExecutor` exists but the multi-stage stop-free model described in
  `docs/pipeline-design.md` is aspirational beyond the current implementation.

## License

MIT OR Apache-2.0, at your option. See [LICENSE-MIT](../../LICENSE-MIT) and
[LICENSE-APACHE-2.0](../../LICENSE-APACHE-2.0).
