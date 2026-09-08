# Pipeline Design

`tpt-media-pipeline` defines the stream-processing core of the TPT media
toolchain: the twelve concepts a media pipeline must nail before any codec
or CLI work can promise realtime behaviour. Everything here is implemented
and tested in `crates/tpt-media-pipeline`.

## Architecture

A pipeline is a **linear, typed** composition of nodes:

```text
Source
   ↓        () pulled by the driver
Demux        packets
   ↓
Decoder      frames
   ↓
Filter       frames -> frames
   ↓
Encoder      packets
   ↓
Mux          container bytes
   ↓
Sink         (collected / emitted)
```

Nodes do not talk to each other directly. A `Chain<L, R>` couples one node
to the next; a `Pipeline` drives a whole composed graph with a single pull
loop.

## 1. Node abstraction

[`Node`](crates/tpt-media-pipeline/src/node.rs) is one processing stage:

```rust
pub trait Node {
    type Input;
    type Output;
    fn process(&mut self, input: Self::Input)
        -> Result<NodeEvent<Self::Output>, NodeError>;
    fn flush(&mut self) -> Result<NodeEvent<Self::Output>, NodeError>;  // default Eos
    fn cancel(&mut self) {}
}
```

Contract: `process` consumes **one** input and emits **at most one** output
per call. Compliance keeps every intermediate buffer in a chain bounded by
one item, which is what makes backpressure and buffering tractable.

`NodeEvent<T>` is the queue of control flow:

- `Item(T)` — an output item.
- `NeedMore` — *consumed the input, ready for more*. This is the soft
  backpressure signal.
- `Eos` — no more items will ever be produced by `process`.
- `Cancelled` — the node aborted itself.

Built-in nodes in `crates/tpt-media-pipeline/src/nodes.rs`: `Source`,
`Source::from_range`, `Map`, `Bounded`, `Cancellable`, `Sink`, and the
`Timestamped<T>` wrapper.

## 2. Stream negotiation

Before flow begins, adjacent stages share a *stream contract*:

```rust
pub struct StreamSpec {
    pub timebase: Option<Timebase>,
    pub has_audio: bool,
    pub has_video: bool,
}
```

`negotiate_stream(a, b)` merges two edges. The timebase must agree when
both sides declare one; media presence is unioned. A mismatch is a
`NodeError::FormatMismatch` before a single packet moves.

## 3. Format negotiation

```rust
pub enum MediaFormat {
    Audio { sample_format, channel_layout, sample_rate },
    Video { pixel_format, width, height },
    Compressed { media_type, codec_id },
    Opaque(&'static str),
}
```

A stage implements `FormatAware` with `input_formats()` / `output_formats()`.
`None` means "accepts/produces anything" (pass-through edge);
`Some(&[])` means "accepts/produces nothing". `negotiate_formats(left, right)`
resolves the exact format that will cross the edge, or fails the pipeline.

## 4. Backpressure

Backpressure is the *pace at which a stage can accept input*.

- **Soft**: `NodeEvent::NeedMore` flows upstream through `Chain` relay —
  the driver keeps feeding input, and buffering stages (see below) use it
  to slow the feed down instead of dropping data.
- **Hard**: if the driver sees `stall_bound` consecutive non-output steps
  (`NeedMore` with no `Item` and no `Eos`), it declares a stall and returns
  `NodeError::Backpressure`. This caps runaway feeds on misbehaved stages.

`Pipeline::pull(stall_bound)` implements both half of the contract: it
absorbs `NeedMore` churn internally and only surfaces stalled runs.

## 5. Buffering

`Bounded<T>` is a bounded batching buffer: it accumulates `window` items
before emitting, then emits one item per input. While filling it returns
`NeedMore` (producing the soft backpressure signal above); a `cap`
(`>= window`) is the hard ceiling beyond which the node fails with
`NodeError::BufferOverflow`. On EOS its contents are drained by `flush`.

Buffering is *always* a node concern; `Chain` itself holds at most one item
in flight.

## 6. Cancellation

`Node::cancel()` interrupts a stage; after cancellation every subsequent
`process`/`flush` returns `Err(NodeError::Cancelled)`. `Chain::cancel()`
cascades to both children. `Cancellable` demonstrates cancellation that
propagates through a chain — including a mid-chain node reached via
`Pipeline::core_mut().right_mut().cancel()`.

## 7. Error propagation

`NodeError` is the small, closed set of failure modes: `FormatMismatch`,
`Backpressure`, `BufferOverflow`, `MalformedItem`, `Cancelled`,
`Internal(&'static str)`.

Errors are **abortive, not propagated along the stream**: a node error
stops the driver (`Pipeline::drive`/`pull` return `Err(..)`) and is
surfaced to the caller unchanged. The run can be restarted from a fresh
`Pipeline`; a cancelled or errored pipeline is not reused.

## 8. Flushing

`flush()` drains a node's internal buffers *after* EOS. Critical point:
flushing is **cascading**. `Chain::flush` first drains the left stage into
the right stage (`FlushPhase::Left`), then drains the right stage itself
(`FlushPhase::Right`), so a `Bounded` in the middle of `Source -> Bounded
-> Map` still emits its tail after the source ends. `Pipeline::pull`
exposes this as the "tail": after `Eos` it keeps returning flushed items
until the stage reports `Eos` again, after which it returns `None`.

`Timestamped` and container `Packet`s flow through `Map` transforms with
`pts`, `dts` and `duration` intact (container flow is exercised against a
real `.tpt` file in `tests/container_flow.rs`).

## 9. End-of-stream

`NodeEvent::Eos` is **one-shot**. After the driver observes it, it enters
the flush tail; once the tail is drained, repeated pulls return `Ok(None)`
(`eos_is_one_shot_and_idempotent`). `Eos` from a source is what triggers
the cascade; `Eos` from a mid-stage simply drains its buffer.

## 10. Timestamps through the pipeline

The `Timestamped<T>` wrapper (`pts`, `dts`, `duration`, payload) guarantees
that transform nodes cannot accidentally disturb the clock: `Timestamped::map`
maps the payload and leaves the timestamps untouched. Container packets
already carry `Timestamp`s (see `docs/container-design.md`) and pass through
the pipeline as-is; a `Reclock` adapter (rescale to a different timebase)
is future work.

## 11. Threading model

The driver loop is abstracted behind `Executor<N>`:

- `Inline` — runs the driver on the calling thread. Deterministic, the
  default for tests and embedded targets.
- `ThreadedExecutor` (std feature) — moves the whole composed graph to a
  dedicated worker thread and delivers the collected outputs (or the
  first error) back to the caller. Requires `N: Send + 'static`.

The full stop-free multi-stage model (the eventual target for §27 I/O and
the CLI) is: one worker per stage, single-producer/single-consumer queues
between stages for ordered delivery, `Send`-bound items, and a shared
`CancelToken` (an `Arc<AtomicBool>`) that `cancel()` sets so every worker
can observe it at the next item boundary. Frequency/pace mismatch between
stages is absorbed by the bounded queues, i.e. real backpressure at the
level of whole buffers rather than a runaway single node.

## 12. Realtime scheduling profile

`RealtimeProfile { max_latency_ns, stall_bound }` is the pipeline's realtime
commitment. `Pipeline::drive_scheduled(profile, clock, observer)` measures
each output item's inter-arrival latency against a caller-injected `Clock`
and feeds it to a `LatencyObserver` (`LatencyLog` records every sample). It
returns a `ScheduledRun { items, deadline_misses, stalls }`.

Clocks are injected and never assumed: embedded targets can drive with a
hardware clock, tests use a deterministic stepping clock, so the scheduled
run is fully reproducible.

## Coverage

- 21 unit tests + 1 container integration test in `tpt-media-pipeline`.
- Deterministic only: no `rand`, no wall-clock assumptions, `BTreeMap`/
  `Vec` only, `Cell`-based test clocks.
- `no_std` + `alloc`; `std` feature adds the threaded executor and
  `std::error::Error` impls.