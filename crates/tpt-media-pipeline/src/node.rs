//! Core node contract, events, errors.

use core::fmt;

/// Events a node can emit from [`Node::process`].
///
/// The node **consumes** the input item it is given in every returned
/// event: `NeedMore` means "I consumed it and am ready for more", `Eos`
/// means "I consumed it and we are now at end of stream", and `Item`
/// means "I consumed it and produced an output".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeEvent<T> {
    /// A produced output item.
    Item(T),
    /// The node consumed the input and needs more input before it can
    /// produce further output. This is the soft backpressure signal: the
    /// caller keeps supplying input, and a stage that buffers internally
    /// paces itself on these.
    NeedMore,
    /// End of stream has been reached. Any internally buffered outputs
    /// must be drained through [`Node::flush`].
    Eos,
    /// The node aborted itself before producing.
    Cancelled,
}

/// Errors a node can return; these abort the run and are surfaced to the
/// caller unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeError {
    /// Stream/format negotiation failed between adjacent stages (see the
    /// private `negotiation` module).
    FormatMismatch,
    /// The caller fed the node faster than it can accept; a downstream
    /// stage signalled backpressure for too long.
    Backpressure,
    /// A bounded internal buffer overflowed its hard capacity.
    BufferOverflow,
    /// An input item was malformed for this node.
    MalformedItem,
    /// The node was cancelled before it could finish.
    Cancelled,
    /// An internal invariant was violated.
    Internal(&'static str),
}

impl fmt::Display for NodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodeError::FormatMismatch => write!(f, "stream/format negotiation failed"),
            NodeError::Backpressure => write!(f, "backpressure violation: node overdriven"),
            NodeError::BufferOverflow => write!(f, "bounded buffer overflowed"),
            NodeError::MalformedItem => write!(f, "malformed input item"),
            NodeError::Cancelled => write!(f, "node was cancelled"),
            NodeError::Internal(what) => write!(f, "internal error: {what}"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for NodeError {}

/// A processing stage in a media pipeline.
///
/// A node is a push-oriented, single-input/single-output stage: the driver
/// calls [`Node::process`] with one input item at a time. The driving unit
/// for pulling nodes is the unit type `()`; mid and rear stages receive
/// the upstream output type. Keep work bounded — a stage must not build an
/// unbounded backlog; use a bounded buffer node for smoothing.
pub trait Node {
    /// The item consumed by each [`Node::process`] call. For pipeline
    /// sources this is `()` (the driver pulls); mid/rear stages receive
    /// the upstream output type.
    type Input;

    /// The item produced by this stage.
    type Output;

    /// Process at most one input item and emit at most one output event.
    fn process(&mut self, input: Self::Input) -> Result<NodeEvent<Self::Output>, NodeError>;

    /// Drain any internally buffered state. Called after end-of-stream.
    /// Returns at most one output per call; the caller repeats until
    /// `Ok(NodeEvent::Eos)` is returned. Stages without internal buffers
    /// may use the default, which reports `Eos` immediately.
    fn flush(&mut self) -> Result<NodeEvent<Self::Output>, NodeError> {
        Ok(NodeEvent::Eos)
    }

    /// Cancel in-flight work. After cancellation, further [`Node::process`]
    /// and [`Node::flush`] calls return `Err(NodeError::Cancelled)`.
    fn cancel(&mut self) {}
}
