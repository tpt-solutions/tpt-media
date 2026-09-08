//! Async integration.
//!
//! The async surface is deliberately tiny and dependency-free (no tokio,
//! no `async_trait`, no `futures`; `async fn` in traits is stable since
//! Rust 1.75). Two traits mirror the sync [`FrameSource`]/[`FrameSink`]
//! contracts one-to-one so a source/sink can be lifted into an async
//! graph or threaded through an executor:
//!
//! - [`AsyncFrameSource::poll_next`] — whole work unit per await, pending
//!   semantics on the underlying IO.
//! - [`AsyncFrameSink::write_item`] / [`AsyncFrameSink::finish`].
//!
//! [`BlockingAdapter`] lifts any synchronous [`FrameSource`] into an
//! [`AsyncFrameSource`] by driving its `poll()` inline. That is correct
//! for in-memory and non-blocking sources; blocking file/device reads
//! MUST be moved to a dedicated I/O thread (the pipeline's
//! `ThreadedExecutor` provides this) rather than awaited inline, and
//! concrete non-blocking file/network transports land with the streaming
//! milestone. Documentation, not implemented stubs, is the deliverable
//! here (todo.md §27 "define async integration where appropriate").

use crate::{FrameSource, IoResult, MediaItem};

/// Continuation-based asynchronous streaming source.
pub trait AsyncFrameSource {
    /// Produce the next item, or `None` at end of stream.
    fn poll_next(
        &mut self,
    ) -> impl core::future::Future<Output = IoResult<Option<MediaItem>>> + Send;
}

/// Continuation-based asynchronous streaming sink.
pub trait AsyncFrameSink {
    /// Accept one item.
    fn write_item(
        &mut self,
        item: &MediaItem,
    ) -> impl core::future::Future<Output = IoResult<()>> + Send;
    /// Flush and finalise.
    fn finish(&mut self) -> impl core::future::Future<Output = IoResult<()>> + Send;
}

/// Synchronous adapter: run a [`FrameSource`]'s `poll()` inside the async
/// future.
///
/// Safe for in-memory / non-blocking sources (e.g. [`crate::SlicePcmReader`]).
/// Do not use for blocking file/device reads inside an executor thread.
pub struct BlockingAdapter<S> {
    inner: S,
}

impl<S: FrameSource + Send> BlockingAdapter<S> {
    /// Wrap a synchronous source.
    pub fn new(inner: S) -> Self {
        BlockingAdapter { inner }
    }

    /// Recover the wrapped source.
    pub fn into_inner(self) -> S {
        self.inner
    }
}

impl<S: FrameSource + Send> AsyncFrameSource for BlockingAdapter<S> {
    async fn poll_next(&mut self) -> IoResult<Option<MediaItem>> {
        self.inner.poll()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use core::future::Future;
    use core::task::{Context, Poll};
    use std::sync::Arc;
    use tpt_media_core::SampleFormat;

    struct Noop;
    impl std::task::Wake for Noop {
        fn wake(self: Arc<Self>) {}
    }

    fn block_on<F: Future>(fut: F) -> F::Output {
        let waker = std::task::Waker::from(Arc::new(Noop));
        let mut cx = Context::from_waker(&waker);
        let mut fut = std::boxed::Box::pin(fut);
        loop {
            if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }

    #[test]
    fn blocking_adapter_lifts_sync_source() {
        let spec = crate::PcmSpec::new(SampleFormat::S16, 1, 48_000).unwrap();
        let mut data = Vec::new();
        for v in 0..8i16 {
            data.extend_from_slice(&v.to_le_bytes());
        }
        let mut src = BlockingAdapter::new(crate::SlicePcmReader::new(&data, spec, 4).unwrap());
        let a = block_on(src.poll_next()).unwrap().unwrap();
        let b = block_on(src.poll_next()).unwrap().unwrap();
        assert!(block_on(src.poll_next()).unwrap().is_none());
        match (a, b) {
            (crate::MediaItem::Audio(a), crate::MediaItem::Audio(b)) => {
                assert_eq!(a.sample_count, 4);
                assert_eq!(b.frame.pts.ticks, 4);
            }
            _ => panic!("expected audio"),
        }
    }
}
