//! Streaming sinks.
//!
//! The push contract is [`FrameSink`]: [`FrameSink::write`] accepts one
//! [`MediaItem`] at a time and [`FrameSink::finish`] flushes/flushes any
//! buffered state. Concrete implementations:
//!
//! - [`VecMediaSink`] — accumulates items in memory (no_std-capable);
//!   ideal for tests and in-memory pipelines.
//! - [`FilePcmSink`] / [`StdoutPcmSink`] (`std`) — buffers interleaved
//!   audio frames as PCM bytes and writes them on `finish` (or flushed
//!   chunk-wise), so `-` stdin/stdout streaming in the CLI maps here.

extern crate alloc;

use alloc::vec::Vec;

use crate::{IoResult, MediaItem};

#[cfg(feature = "std")]
use crate::IoError;

/// Push-driven streaming sink.
pub trait FrameSink {
    /// Accept one item.
    fn write(&mut self, item: &MediaItem) -> IoResult<()>;
    /// Flush and finalise the sink.
    fn finish(&mut self) -> IoResult<()>;
}

/// Accumulates [`MediaItem`]s in memory.
#[derive(Debug, Default)]
pub struct VecMediaSink {
    items: Vec<MediaItem>,
}

impl VecMediaSink {
    /// An empty sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// The items accepted so far.
    pub fn items(&self) -> &[MediaItem] {
        &self.items
    }

    /// Number of items accepted so far.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// `true` if nothing has been written yet.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

impl FrameSink for VecMediaSink {
    fn write(&mut self, item: &MediaItem) -> IoResult<()> {
        self.items.push(item.clone());
        Ok(())
    }

    fn finish(&mut self) -> IoResult<()> {
        Ok(())
    }
}

/// Writes audio items as interleaved PCM bytes to any `std::io::Write`
/// target, buffering one chunk at a time. (`std`)
#[cfg(feature = "std")]
pub struct FilePcmSink {
    inner: Box<dyn std::io::Write>,
    buffer: Vec<u8>,
}

#[cfg(feature = "std")]
impl FilePcmSink {
    /// A PCM sink writing to the given writer. Audio frames will be
    /// re-encoded as raw interleaved bytes (never reformatted — frames
    /// must already share format/channels/rate; mixing is an error).
    pub fn new(writer: Box<dyn std::io::Write>) -> Self {
        FilePcmSink {
            inner: writer,
            buffer: Vec::new(),
        }
    }

    fn flush_buffer(&mut self) -> IoResult<()> {
        use std::io::Write;

        if self.buffer.is_empty() {
            return Ok(());
        }
        self.inner
            .write_all(&self.buffer)
            .map_err(|e| IoError::Other(e.to_string()))?;
        self.buffer.clear();
        Ok(())
    }
}

#[cfg(feature = "std")]
impl FrameSink for FilePcmSink {
    fn write(&mut self, item: &MediaItem) -> IoResult<()> {
        use crate::pcm::encode_pcm;

        match item {
            MediaItem::Audio(frame) => {
                let bytes = encode_pcm(core::slice::from_ref(frame))?;
                self.buffer.extend_from_slice(&bytes);
                if self.buffer.len() >= 1 << 16 {
                    self.flush_buffer()?;
                }
                Ok(())
            }
            MediaItem::Video(_) => {
                Err(IoError::Other("PCM sink cannot accept video frames".into()))
            }
        }
    }

    fn finish(&mut self) -> IoResult<()> {
        use std::io::Write;

        self.flush_buffer()?;
        self.inner
            .flush()
            .map_err(|e| IoError::Other(e.to_string()))
    }
}

/// Alias for [`FilePcmSink`] constructed over stdout. (`std`)
#[cfg(feature = "std")]
pub type StdoutPcmSink = FilePcmSink;
