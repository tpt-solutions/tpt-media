//! `tpt-media-pipeline` — node-based media pipeline.

extern crate alloc;
use alloc::vec::Vec;

/// Result emitted by a pipeline node.
#[derive(Debug)]
pub enum NodeEvent<T> {
    /// Node produced an output item.
    Item(T),
    /// Node needs more input.
    NeedMore,
    /// End of stream reached.
    Eos,
    /// Node wants to be flushed.
    Flush,
}

/// Pipeline node trait.
pub trait Node {
    type Input;
    type Output;

    /// Process exactly one input item.
    fn process_one(&mut self, input: Self::Input) -> NodeEvent<Self::Output>;
    /// Signal the node to flush any buffered state.
    fn flush(&mut self) -> NodeEvent<Self::Output> {
        NodeEvent::Eos
    }
    /// Cancel all in-flight work.
    fn cancel(&mut self) {}
}

/// Simple linear pipeline of nodes.
pub struct Pipeline<N: Node> {
    nodes: Vec<N>,
}

impl<N: Node> Pipeline<N> {
    /// Create a new pipeline.
    pub fn new(nodes: Vec<N>) -> Self {
        Pipeline { nodes }
    }

    /// Run the pipeline's first node with an initial input.
    ///
    /// This does not yet chain output through the remaining nodes — see
    /// todo.md §26 (stream/format negotiation, backpressure, flushing,
    /// threading model) for the work that turns this into a real
    /// multi-node pipeline.
    pub fn run(&mut self, input: N::Input) -> Vec<N::Output> {
        match self.nodes[0].process_one(input) {
            NodeEvent::Item(o) => alloc::vec![o],
            _ => Vec::new(),
        }
    }
}

/// A trivial pass-through source node.
pub struct Source<T> {
    items: Vec<T>,
    cursor: usize,
}

impl<T> Source<T> {
    pub fn new(items: Vec<T>) -> Self {
        Source { items, cursor: 0 }
    }
}

impl<T: Clone> Node for Source<T> {
    type Input = ();
    type Output = T;
    fn process_one(&mut self, _input: ()) -> NodeEvent<T> {
        if self.cursor < self.items.len() {
            let item = self.items[self.cursor].clone();
            self.cursor += 1;
            NodeEvent::Item(item)
        } else {
            NodeEvent::Eos
        }
    }
}

/// A trivial sink node.
pub struct Sink<T> {
    pub last: Option<T>,
}

impl<T> Sink<T> {
    pub fn new() -> Self {
        Sink { last: None }
    }
}

impl<T> Default for Sink<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> Node for Sink<T> {
    type Input = T;
    type Output = ();
    fn process_one(&mut self, input: T) -> NodeEvent<()> {
        self.last = Some(input);
        NodeEvent::Item(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_media_core::{Frame, Packet};

    #[test]
    fn source_emits_items_then_eos() {
        let mut src = Source::new(alloc::vec![1, 2, 3]);
        match src.process_one(()) {
            NodeEvent::Item(1) => {}
            _ => panic!("expected 1"),
        }
        match src.process_one(()) {
            NodeEvent::Item(2) => {}
            _ => panic!("expected 2"),
        }
        match src.process_one(()) {
            NodeEvent::Item(3) => {}
            _ => panic!("expected 3"),
        }
        match src.process_one(()) {
            NodeEvent::Eos => {}
            _ => panic!("expected EOS"),
        }
    }

    #[test]
    fn sink_stores_last() {
        let mut sink = Sink::<i32>::new();
        sink.process_one(42);
        assert_eq!(sink.last, Some(42));
    }

    #[test]
    fn packet_and_frame_types() {
        // Smoke test that the dependencies are wired correctly.
        let _: Option<Packet> = None;
        let _: Option<Frame> = None;
    }
}
