//! Pipeline driver: turns a composed node graph (with `Input = ()`) into a
//! pullable stream.
//!
//! A [`Pipeline`] owns a single composed node — typically a [`Chain`] of
//! `Source -> ... -> Sink` — and drives it with a pull loop. It applies
//! the backpressure contract: repeated [`NodeEvent::NeedMore`] with no
//! output and no `Eos` for more than [`STALL_BOUND`] steps is treated as a
//! stall and reported as [`NodeError::Backpressure`].

use alloc::vec::Vec;

use crate::node::{Node, NodeError, NodeEvent};
use crate::sched::{Clock, LatencyObserver, RealtimeProfile, ScheduledRun};

/// Default number of consecutive non-output steps before a run is
/// considered stalled.
pub const STALL_BOUND: usize = 4096;

/// A composed, pullable media pipeline.
pub struct Pipeline<N> {
    core: N,
    done: bool,
    tail_emitted: bool,
}

impl<N: Node<Input = ()>> Pipeline<N> {
    /// Wrap a fully composed (pullable) node graph.
    pub fn new(core: N) -> Self {
        Pipeline {
            core,
            done: false,
            tail_emitted: false,
        }
    }

    /// The composed core graph.
    pub fn core(&self) -> &N {
        &self.core
    }

    /// Mutable access to the composed core graph (e.g. to reach a mid
    /// node's handle and cancel or inspect it).
    pub fn core_mut(&mut self) -> &mut N {
        &mut self.core
    }

    /// Pull one output item from the pipeline.
    ///
    /// - `Ok(Some(o))`: another output item emitted.
    /// - `Ok(None)`: end of stream reached (the internal `flush` tail has
    ///   also been fully drained).
    /// - `Err(..)`: abort (overflow, cancellation, stall, malformed item).
    pub fn pull(&mut self, stall_bound: usize) -> Result<Option<N::Output>, NodeError> {
        let mut since = 0usize;
        loop {
            if self.done {
                if self.tail_emitted {
                    return Ok(None);
                }
                match self.core.flush() {
                    Ok(NodeEvent::Item(o)) => return Ok(Some(o)),
                    Ok(NodeEvent::NeedMore) => {
                        since += 1;
                        if since > stall_bound {
                            return Err(NodeError::Backpressure);
                        }
                    }
                    Ok(_) => {
                        self.tail_emitted = true;
                        return Ok(None);
                    }
                    Err(e) => return Err(e),
                }
            }
            match self.core.process(()) {
                Ok(NodeEvent::Item(o)) => {
                    return Ok(Some(o));
                }
                Ok(NodeEvent::NeedMore) => {
                    since += 1;
                    if since > stall_bound {
                        return Err(NodeError::Backpressure);
                    }
                }
                Ok(NodeEvent::Eos) => {
                    self.done = true;
                }
                Ok(NodeEvent::Cancelled) => return Err(NodeError::Cancelled),
                Err(e) => return Err(e),
            }
        }
    }

    /// Drive the pipeline to completion, collecting every output item
    /// (including flushed tail items).
    pub fn drive(&mut self, stall_bound: usize) -> Result<Vec<N::Output>, NodeError> {
        let mut out = Vec::new();
        while let Some(o) = self.pull(stall_bound)? {
            out.push(o);
        }
        Ok(out)
    }

    /// Run `self` on an executor; returns the collected outputs.
    pub fn execute<E: Executor<N>>(
        self,
        executor: &E,
        stall_bound: usize,
    ) -> Result<Vec<N::Output>, NodeError> {
        executor.execute(self.core, stall_bound)
    }

    /// Drive to completion under a realtime scheduling profile, measuring
    /// per-item inter-arrival latency against `clock` and reporting each
    /// measurement through `observer`.
    pub fn drive_scheduled(
        &mut self,
        profile: &RealtimeProfile,
        clock: &dyn Clock,
        observer: &mut dyn LatencyObserver,
    ) -> Result<ScheduledRun, NodeError> {
        let mut run = ScheduledRun::default();
        let mut prev = clock.now_ns();
        let mut since = 0usize;

        while !self.done {
            match self.core.process(()) {
                Ok(NodeEvent::Item(_)) => {
                    let now = clock.now_ns();
                    let latency = now.saturating_sub(prev);
                    prev = now;
                    observer.observe(latency);
                    if latency > profile.max_latency_ns {
                        run.deadline_misses += 1;
                    }
                    run.items += 1;
                    since = 0;
                }
                Ok(NodeEvent::NeedMore) => {
                    since += 1;
                    if since > profile.stall_bound {
                        run.stalls += 1;
                        return Ok(run);
                    }
                }
                Ok(NodeEvent::Eos) => {
                    self.done = true;
                }
                Ok(NodeEvent::Cancelled) => return Err(NodeError::Cancelled),
                Err(e) => return Err(e),
            }
        }

        while !self.tail_emitted {
            match self.core.flush() {
                Ok(NodeEvent::Item(_)) => {
                    run.items += 1;
                    since = 0;
                }
                Ok(NodeEvent::NeedMore) => {
                    since += 1;
                    if since > profile.stall_bound {
                        run.stalls += 1;
                        return Ok(run);
                    }
                }
                Ok(_) => {
                    self.tail_emitted = true;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(run)
    }
}

/// An executor abstracts *where* the driver loop runs: inline on the
/// calling thread (deterministic default) or on a dedicated worker thread
/// (see [`ThreadedExecutor`]).
pub trait Executor<N: Node<Input = ()>> {
    /// Execute the composed node and return collected outputs.
    fn execute(&self, node: N, stall_bound: usize) -> Result<Vec<N::Output>, NodeError> {
        Pipeline::new(node).drive(stall_bound)
    }
}

/// The inline executor: runs the driver on the current thread. Fully
/// deterministic, so it is the default for tests and embedded targets.
#[derive(Debug, Clone, Copy, Default)]
pub struct Inline;

impl<N: Node<Input = ()>> Executor<N> for Inline {}

/// A single dedicated worker thread runs the whole graph. Useful when the
/// caller thread must not block on the pipeline clock (realtime sources).
#[cfg(feature = "std")]
#[derive(Debug, Clone, Copy, Default)]
pub struct ThreadedExecutor;

#[cfg(feature = "std")]
impl<N> Executor<N> for ThreadedExecutor
where
    N: Node<Input = (), Output: Send> + Send + 'static,
{
    fn execute(&self, node: N, stall_bound: usize) -> Result<Vec<N::Output>, NodeError> {
        let handle = std::thread::Builder::new()
            .name("tpt-pipeline-worker".into())
            .spawn(move || Pipeline::new(node).drive(stall_bound))
            .map_err(|_| NodeError::Internal("thread spawn failed"))?;
        handle
            .join()
            .map_err(|_| NodeError::Internal("pipeline worker panicked"))?
    }
}
