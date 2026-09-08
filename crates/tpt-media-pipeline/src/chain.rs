//! Typed linear composition of two nodes.

use crate::node::{Node, NodeError, NodeEvent};

/// Left-compose `left` into `right`: every output of the left node is fed
/// to the right node, lockstep (one item per [`Node::process`] call), so
/// intermediate buffering is always bounded by a single item.
///
/// Events are relayed with these semantics:
/// - `left` emits `Item(o)` -> `o` is handed to `right` and `right`'s
///   event is relayed (including its `NeedMore`, which is the chain's
///   backpressure signal: the driver should keep supplying input).
/// - `left` emits `Eos` -> the chain reports `Eos`; the driver must then
///   drain the tail through [`Node::flush`], which cascades `left` first
///   (so buffered left-stage items flow on), then drains `right`.
/// - `left` emits `NeedMore` -> relayed as-is (a source-driven mid stage
///   may need more driving input).
pub struct Chain<L, R> {
    left: L,
    right: R,
    left_done: bool,
    cancelled: bool,
    flush_phase: FlushPhase,
}

enum FlushPhase {
    /// Draining the left stage's internal buffer into the right stage.
    Left,
    /// Draining the right stage's own buffer.
    Right,
}

impl<L: Node, R: Node<Input = L::Output>> Chain<L, R> {
    /// Compose two nodes.
    pub fn new(left: L, right: R) -> Self {
        Chain {
            left,
            right,
            left_done: false,
            cancelled: false,
            flush_phase: FlushPhase::Left,
        }
    }

    /// The left (upstream) stage.
    pub fn left(&self) -> &L {
        &self.left
    }

    /// Mutable access to the left (upstream) stage.
    pub fn left_mut(&mut self) -> &mut L {
        &mut self.left
    }

    /// The right (downstream) stage.
    pub fn right(&self) -> &R {
        &self.right
    }

    /// Mutable access to the right (downstream) stage.
    pub fn right_mut(&mut self) -> &mut R {
        &mut self.right
    }
}

impl<L: Node, R: Node<Input = L::Output>> Node for Chain<L, R> {
    type Input = L::Input;
    type Output = R::Output;

    fn process(&mut self, input: Self::Input) -> Result<NodeEvent<Self::Output>, NodeError> {
        if self.cancelled {
            return Err(NodeError::Cancelled);
        }
        if self.left_done {
            return Err(NodeError::Backpressure);
        }
        match self.left.process(input) {
            Ok(NodeEvent::Item(o)) => {
                let result = self.right.process(o);
                if matches!(result, Ok(NodeEvent::Cancelled)) {
                    self.cancelled = true;
                }
                result
            }
            Ok(NodeEvent::NeedMore) => Ok(NodeEvent::NeedMore),
            Ok(NodeEvent::Eos) => {
                self.left_done = true;
                Ok(NodeEvent::Eos)
            }
            Ok(NodeEvent::Cancelled) => {
                self.cancelled = true;
                Ok(NodeEvent::Cancelled)
            }
            Err(e) => Err(e),
        }
    }

    fn flush(&mut self) -> Result<NodeEvent<Self::Output>, NodeError> {
        if self.cancelled {
            return Err(NodeError::Cancelled);
        }
        loop {
            match self.flush_phase {
                FlushPhase::Left => match self.left.flush() {
                    Ok(NodeEvent::Item(o)) => match self.right.process(o) {
                        Ok(NodeEvent::Item(out)) => return Ok(NodeEvent::Item(out)),
                        Ok(_) => continue,
                        Err(e) => return Err(e),
                    },
                    Ok(_) => {
                        self.flush_phase = FlushPhase::Right;
                        continue;
                    }
                    Err(e) => return Err(e),
                },
                FlushPhase::Right => return self.right.flush(),
            }
        }
    }

    fn cancel(&mut self) {
        self.cancelled = true;
        self.left.cancel();
        self.right.cancel();
    }
}

/// Builder-style helpers on any [`Node`].
pub trait NodeOps: Node + Sized {
    /// Compose `self` with a downstream node.
    fn then<R: Node<Input = Self::Output>>(self, right: R) -> Chain<Self, R> {
        Chain::new(self, right)
    }

    /// Compose with a [`crate::nodes::Map`] transform.
    fn map<O, F>(self, f: F) -> Chain<Self, crate::nodes::Map<Self::Output, O, F>>
    where
        F: FnMut(Self::Output) -> O,
    {
        self.then(crate::nodes::Map::new(f))
    }

    /// Drive `self` as a pipeline (only valid when it can be *pulled*).
    fn into_pipeline(self) -> crate::pipeline::Pipeline<Self>
    where
        Self: Node<Input = ()>,
    {
        crate::pipeline::Pipeline::new(self)
    }
}

impl<N: Node> NodeOps for N {}
