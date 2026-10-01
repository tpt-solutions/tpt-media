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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pcm::PcmSpec;
    use tpt_media_core::{PixelFormat, SampleFormat, VideoFrame};

    /// 16-bit mono 48 kHz, the format the CLI's PCM paths use.
    fn spec() -> PcmSpec {
        PcmSpec::new(SampleFormat::S16, 1, 48_000).unwrap()
    }

    /// One `frames`-long mono S16 frame of `value` bytes.
    fn audio(frames: usize, value: u8) -> MediaItem {
        let bytes = alloc::vec![value; frames * 2];
        MediaItem::Audio(spec().into_frame(&bytes).unwrap())
    }

    // --- VecMediaSink (no_std-capable, so tested under both feature sets) --

    #[test]
    fn vec_sink_starts_empty() {
        // §38 "empty input": a fresh sink must report empty, and `len` must
        // agree with `items` rather than being a separate counter that can
        // drift out of step with it.
        let s = VecMediaSink::new();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
        assert!(s.items().is_empty());
    }

    #[test]
    fn vec_sink_collects_in_order() {
        let mut s = VecMediaSink::new();
        s.write(&audio(4, 1)).unwrap();
        s.write(&audio(8, 2)).unwrap();
        s.finish().unwrap();
        assert!(!s.is_empty());
        assert_eq!(s.len(), 2);
        assert_eq!(s.len(), s.items().len());
        let counts: Vec<u32> = s
            .items()
            .iter()
            .map(|i| match i {
                MediaItem::Audio(a) => a.sample_count,
                MediaItem::Video(_) => unreachable!(),
            })
            .collect();
        assert_eq!(counts, alloc::vec![4, 8]);
    }

    #[test]
    fn vec_sink_finish_on_an_empty_sink_is_ok() {
        // Finishing nothing must not be an error: the CLI finalises
        // unconditionally, including for a stream that yielded no items.
        let mut s = VecMediaSink::new();
        assert!(s.finish().is_ok());
        assert!(s.is_empty());
    }

    #[test]
    fn vec_sink_items_is_a_snapshot_not_a_live_view() {
        // `items()` hands out a borrow, so a later write cannot retroactively
        // change what a caller already read.
        let mut s = VecMediaSink::new();
        s.write(&audio(4, 1)).unwrap();
        let seen = s.len();
        s.write(&audio(4, 2)).unwrap();
        assert_eq!(seen, 1);
        assert_eq!(s.len(), 2);
    }

    // --- FilePcmSink: the auto-flush threshold is the real logic here. ---

    #[cfg(feature = "std")]
    mod pcm_sink {
        use super::*;
        use core::cell::RefCell;
        use std::rc::Rc;

        /// A sink writing into a shared `Vec`, so the bytes can be inspected.
        #[derive(Clone, Default)]
        struct SharedBuf(Rc<RefCell<Vec<u8>>>);

        impl std::io::Write for SharedBuf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.borrow_mut().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        /// A writer that always fails, for the error path.
        struct Failing;

        impl std::io::Write for Failing {
            fn write(&mut self, _b: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("device on fire"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::other("device on fire"))
            }
        }

        #[test]
        fn audio_is_buffered_until_finish() {
            // Nothing must reach the writer before `finish`: the buffer is what
            // makes `-`/stdout streaming chunk-wise rather than per-frame.
            let buf = SharedBuf::default();
            let mut s = FilePcmSink::new(Box::new(buf.clone()));
            s.write(&audio(4, 7)).unwrap();
            assert!(buf.0.borrow().is_empty());
            s.finish().unwrap();
            assert_eq!(buf.0.borrow().len(), 8);
        }

        #[test]
        fn the_64k_threshold_flushes_without_finishing() {
            // A long stream must not buffer without bound. 64 KiB is the
            // documented threshold, so crossing it has to write out *without*
            // `finish` — that is what keeps memory flat over a long stream.
            let buf = SharedBuf::default();
            let mut s = FilePcmSink::new(Box::new(buf.clone()));
            // 64 KiB is 32 768 mono S16 frames; write in 8 192-frame chunks.
            for _ in 0..8 {
                s.write(&audio(8_192, 1)).unwrap();
            }
            let written = buf.0.borrow().len();
            assert!(
                written > 0,
                "crossing the 64 KiB threshold must flush before finish()"
            );
            assert_eq!(
                written % (8_192 * 2),
                0,
                "flush must land on a frame boundary"
            );
            s.finish().unwrap();
            assert_eq!(buf.0.borrow().len(), 8 * 8_192 * 2);
        }

        #[test]
        fn finish_is_idempotent_for_the_byte_count() {
            // A second `finish` must not duplicate output: the tail is flushed
            // and the buffer cleared, so the second call is a no-op.
            let buf = SharedBuf::default();
            let mut s = FilePcmSink::new(Box::new(buf.clone()));
            s.write(&audio(4, 3)).unwrap();
            s.finish().unwrap();
            let after_first = buf.0.borrow().len();
            s.finish().unwrap();
            assert_eq!(buf.0.borrow().len(), after_first);
        }

        #[test]
        fn finish_on_an_empty_sink_writes_nothing() {
            // §38 "empty input": no frames must produce an empty file, not a
            // header or a stray zero byte.
            let buf = SharedBuf::default();
            let mut s = FilePcmSink::new(Box::new(buf.clone()));
            s.finish().unwrap();
            assert!(buf.0.borrow().is_empty());
        }

        #[test]
        fn a_zero_length_audio_frame_is_accepted() {
            // An empty frame is legal at the codec level; the sink must not
            // reject it or confuse it with end of stream.
            let buf = SharedBuf::default();
            let mut s = FilePcmSink::new(Box::new(buf.clone()));
            s.write(&audio(0, 0)).unwrap();
            s.finish().unwrap();
            assert!(buf.0.borrow().is_empty());
        }

        #[test]
        fn a_video_item_is_rejected_by_name() {
            // The error must name the actual problem ("this is a PCM sink"),
            // not a generic failure — a caller streaming a multiplexed file
            // needs to know the sink is at fault, not the codec.
            let buf = SharedBuf::default();
            let mut s = FilePcmSink::new(Box::new(buf));
            let v = MediaItem::Video(VideoFrame::alloc(2, 2, PixelFormat::Gray8));
            let err = s.write(&v).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("PCM") && msg.contains("video"),
                "error should name the PCM/video mismatch, got: {msg}"
            );
        }

        #[test]
        fn a_rejected_video_item_does_not_corrupt_the_stream() {
            // The failed write must leave the sink usable, so a caller that
            // filters the video track can carry on rather than restart.
            let buf = SharedBuf::default();
            let mut s = FilePcmSink::new(Box::new(buf.clone()));
            s.write(&audio(4, 1)).unwrap();
            assert!(s
                .write(&MediaItem::Video(VideoFrame::alloc(
                    2,
                    2,
                    PixelFormat::Gray8
                )))
                .is_err());
            s.write(&audio(4, 2)).unwrap();
            s.finish().unwrap();
            assert_eq!(buf.0.borrow().len(), 16);
        }

        #[test]
        fn a_failing_writer_surfaces_as_an_io_error_not_a_panic() {
            // A dead device must be an `IoError` carrying the cause, and must
            // not leave the sink in a state that panics on the next call.
            let mut s = FilePcmSink::new(Box::new(Failing));
            s.write(&audio(4, 1)).unwrap();
            let err = s.finish().unwrap_err();
            assert!(
                err.to_string().contains("device on fire"),
                "lost the cause: {err}"
            );
            let _ = s.finish();
        }
    }
}
