//! Built-in reusable nodes: sources, transforms, buffers, sinks.
//!
//! Each node is intentionally tiny and deterministic so it can be composed
//! into a full pipeline for tests and demos: a `tpt` file is read by a
//! demux node (in the `tpt-container` integration test), packets flow
//! through a [`Map`] transform with timestamps intact, and a [`Sink`]
//! collects the output.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::marker::PhantomData;
use core::ops::RangeBounds;

use tpt_media_core::{Duration, Timestamp};

use crate::node::{Node, NodeError, NodeEvent};

/// Pushing, pull-driven source: emits one item per [`Node::process`] call
/// and reports `Eos` once exhausted.
#[derive(Debug)]
pub struct Source<T> {
    items: Vec<T>,
    cursor: usize,
}

impl<T: Clone> Source<T> {
    /// A source over a finite, pre-materialised item list.
    pub fn new(items: Vec<T>) -> Self {
        Source { items, cursor: 0 }
    }

    /// The number of items already emitted.
    pub fn consumed(&self) -> usize {
        self.cursor
    }
}

impl<T> Source<T>
where
    T: Clone
        + core::ops::AddAssign
        + core::ops::Add<Output = T>
        + core::ops::Sub<Output = T>
        + From<u8>
        + PartialOrd,
{
    /// A source over an inclusive/range of countable values.
    pub fn from_range<R>(range: R) -> Self
    where
        R: RangeBounds<T>,
    {
        let lo = match range.start_bound() {
            core::ops::Bound::Included(x) => x.clone(),
            core::ops::Bound::Excluded(x) => x.clone() + T::from(1u8),
            core::ops::Bound::Unbounded => T::from(0u8),
        };
        let hi = match range.end_bound() {
            core::ops::Bound::Included(x) => x.clone(),
            core::ops::Bound::Excluded(x) => x.clone() - T::from(1u8),
            core::ops::Bound::Unbounded => T::from(0u8),
        };
        let mut items = Vec::new();
        let mut x = lo.clone();
        items.push(lo);
        while x < hi {
            x += T::from(1u8);
            items.push(x.clone());
        }
        Source::new(items)
    }
}

impl<T: Clone> Node for Source<T> {
    type Input = ();
    type Output = T;

    fn process(&mut self, _: ()) -> Result<NodeEvent<T>, NodeError> {
        match self.items.get(self.cursor) {
            Some(item) => {
                self.cursor += 1;
                Ok(NodeEvent::Item(item.clone()))
            }
            None => Ok(NodeEvent::Eos),
        }
    }
}

/// Pure function transform that preserves the item stream shape.
pub struct Map<T, O, F> {
    f: F,
    _marker: PhantomData<fn(T) -> O>,
}

impl<T, O, F> Map<T, O, F> {
    /// A transform node applying `f` to every input.
    pub fn new(f: F) -> Self {
        Map {
            f,
            _marker: PhantomData,
        }
    }
}

impl<T, O, F: FnMut(T) -> O> Node for Map<T, O, F> {
    type Input = T;
    type Output = O;

    fn process(&mut self, input: T) -> Result<NodeEvent<O>, NodeError> {
        Ok(NodeEvent::Item((self.f)(input)))
    }
}

/// Bounded buffering node: waits until `window` items have accumulated
/// before emitting, then emits one item per input — smoothing bursts into
/// steady batches. While the window is still filling it returns
/// [`NodeEvent::NeedMore`], which is the chain's soft backpressure signal
/// to the driver. A `cap` (>= `window`) is the hard upper bound beyond
/// which [`NodeError::BufferOverflow`] is reported.
#[derive(Debug)]
pub struct Bounded<T> {
    queue: VecDeque<T>,
    window: usize,
    cap: usize,
    cancelled: bool,
}

impl<T> Bounded<T> {
    /// A buffering node that emits a batch once `window` items collected.
    ///
    /// # Panics
    /// Panics if `window == 0` or `cap < window`.
    pub fn new(window: usize, cap: usize) -> Self {
        assert!(window > 0, "window must be at least 1");
        assert!(cap >= window, "cap must be >= window");
        Bounded {
            queue: VecDeque::new(),
            window,
            cap,
            cancelled: false,
        }
    }

    /// Buffering node with an equal hard cap (no overflow possible).
    pub fn with_window(window: usize) -> Self {
        Bounded::new(window, window)
    }

    /// Items currently held in the buffer.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

impl<T> Node for Bounded<T> {
    type Input = T;
    type Output = T;

    fn process(&mut self, input: T) -> Result<NodeEvent<T>, NodeError> {
        if self.cancelled {
            return Err(NodeError::Cancelled);
        }
        if self.queue.len() >= self.cap {
            return Err(NodeError::BufferOverflow);
        }
        self.queue.push_back(input);
        if self.queue.len() >= self.window {
            Ok(NodeEvent::Item(
                self.queue.pop_front().expect("len >= window > 0"),
            ))
        } else {
            Ok(NodeEvent::NeedMore)
        }
    }

    fn flush(&mut self) -> Result<NodeEvent<T>, NodeError> {
        if self.cancelled {
            return Err(NodeError::Cancelled);
        }
        Ok(self
            .queue
            .pop_front()
            .map_or(NodeEvent::Eos, NodeEvent::Item))
    }

    fn cancel(&mut self) {
        self.cancelled = true;
    }
}

/// Collecting sink: stores every item; emits `Item(())` per input so the
/// driver sees steady progress. Terminates with the default `Eos` flush.
#[derive(Debug)]
pub struct Sink<T> {
    items: Vec<T>,
    last: Option<T>,
}

impl<T> Sink<T> {
    /// A collecting sink.
    pub fn new() -> Self {
        Sink {
            items: Vec::new(),
            last: None,
        }
    }

    /// Items collected so far.
    pub fn items(&self) -> &[T] {
        &self.items
    }

    /// Number of items collected.
    pub fn count(&self) -> usize {
        self.items.len()
    }

    /// The most recently collected item.
    pub fn last(&self) -> Option<&T> {
        self.last.as_ref()
    }
}

impl<T> Default for Sink<T> {
    fn default() -> Self {
        Sink::new()
    }
}

impl<T: Clone> Node for Sink<T> {
    type Input = T;
    type Output = ();

    fn process(&mut self, input: T) -> Result<NodeEvent<()>, NodeError> {
        self.last = Some(input.clone());
        self.items.push(input);
        Ok(NodeEvent::Item(()))
    }
}

/// Pass-through node that can be told to cancel; further items are then
/// rejected with [`NodeError::Cancelled`]. Demonstrates cancellation
/// propagation through a chain.
#[derive(Debug)]
pub struct Cancellable<T> {
    cancelled: bool,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Cancellable<T> {
    /// A cancellable pass-through node.
    pub fn new() -> Self {
        Cancellable {
            cancelled: false,
            _marker: PhantomData,
        }
    }

    /// Cancel this node.
    pub fn cancel(&mut self) {
        self.cancelled = true;
    }
}

impl<T> Default for Cancellable<T> {
    fn default() -> Self {
        Cancellable::new()
    }
}

impl<T> Node for Cancellable<T> {
    type Input = T;
    type Output = T;

    fn process(&mut self, input: T) -> Result<NodeEvent<T>, NodeError> {
        if self.cancelled {
            return Err(NodeError::Cancelled);
        }
        Ok(NodeEvent::Item(input))
    }

    fn cancel(&mut self) {
        Cancellable::cancel(self);
    }
}

/// An item that carries presentation/decoding timestamps through the
/// pipeline, so transforms can keep the clock intact even while the
/// payload mutates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timestamped<T> {
    /// Presentation timestamp.
    pub pts: Timestamp,
    /// Decoding timestamp.
    pub dts: Timestamp,
    /// Playback duration of this item, if known.
    pub duration: Option<Duration>,
    /// The payload.
    pub item: T,
}

impl<T> Timestamped<T> {
    /// Timestamp an item; `dts` starts equal to `pts`.
    pub fn new(pts: Timestamp, item: T) -> Self {
        Timestamped {
            pts,
            dts: pts,
            duration: None,
            item,
        }
    }

    /// Map the payload, preserving timestamps — the guarantee transforms
    /// get for free.
    pub fn map<O>(self, f: impl FnOnce(T) -> O) -> Timestamped<O> {
        Timestamped {
            pts: self.pts,
            dts: self.dts,
            duration: self.duration,
            item: f(self.item),
        }
    }
}
