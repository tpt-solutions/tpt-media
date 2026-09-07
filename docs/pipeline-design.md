# Pipeline Design Notes

`tpt-media-pipeline` provides a small, node-based graph for media
processing. It is **not** a general dataflow framework; it is tuned for
the needs of media pipelines.

## Goals

- Composability: a node has one input type, one output type, and a
  `process_one()` method.
- Backpressure: when downstream nodes are slow, upstream nodes block.
- Cancellation: a `cancel()` call propagates through the graph.
- Real-time safety: the per-node processing must be lock-free and
  allocation-free on the documented real-time path.

## Non-Goals

- General DAG scheduling (use `tokio`, `rayon`, or similar for that).
- GPU acceleration (out of scope).
- Cross-process distribution (out of scope).

## Node Trait

```rust
pub trait Node {
    type Input;
    type Output;

    fn process_one(&mut self, input: Self::Input)
        -> Result<NodeEvent<Self::Output>, PipelineError>;

    fn cancel(&mut self);
}

pub enum NodeEvent<T> {
    Item(T),                  // produced an item
    NeedMore,                 // needs more input
    Flush,                    // please flush
    Eos,                      // end of stream reached
}
```

## Default Nodes

| Node             | Purpose                                  |
| ---------------- | ---------------------------------------- |
| `Source<T>`      | Wraps a user-supplied iterator / channel |
| `Sink<T>`        | Wraps a user-supplied consumer            |
| `Demux<Packet, Frame>` | Reads a `.tpt` stream, produces Frames |
| `Mux<Frame, Packet>`   | Writes a `.tpt` stream from Frames      |
| `Decoder<F>`     | Calls a `tpt-media-codec` decoder        |
| `Encoder<F>`     | Calls a `tpt-media-codec` encoder        |
| `Filter<F>`      | Generic sample/pixel transform            |

## Backpressure

Default backpressure uses a bounded SPSC ring buffer per edge. The
buffer size is configurable per-edge. Producers `try_push`; if the
buffer is full, they either spin, yield, or block (configurable).

For real-time paths, the producer **never** blocks and **never**
allocates; the buffer is sized large enough to absorb jitter.

## Cancellation

`cancel()` sets a flag on the node. The next `process_one()` call
returns `Err(PipelineError::Cancelled)`. The executor tears down the
graph and returns control to the caller.

## Timestamps

Timestamps are passed through the pipeline as part of the `Packet` and
`Frame` types. Timebase conversions are performed by the boundary nodes
(`Demux`, `Mux`). Internal nodes are timebase-agnostic.

## Threading

The default executor is single-threaded. A multi-threaded executor is
provided but is **not** used on real-time paths. The recommended pattern
for audio is:

```text
audio thread → Decoder → Sink (ring buffer)
```

For video decoding in a player:

```text
I/O thread → Demux → ring buffer → Decoder → ring buffer → Renderer
```

## Error Model

Errors propagate as `Result<NodeEvent<T>, PipelineError>`. Recoverable
errors (e.g. corrupted packet) are surfaced via `Item(Err(...))` or
logged and skipped; unrecoverable errors (e.g. resource exhaustion)
cancel the graph.