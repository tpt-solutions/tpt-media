//! Streaming sources.
//!
//! The pull contract is [`FrameSource`] + [`MediaItem`]: the consumer
//! calls [`FrameSource::poll`] until it returns `Ok(None)` (end of
//! stream). This mirrors the pipeline's `NodeEvent::Eos` semantics and
//! keeps sources trivially composable with [`crate::sink::FrameSink`]
//! and the async adapters.
//!
//! Concrete implementations:
//!
//! - [`SlicePcmReader`] — chunks a borrowed PCM byte slice into
//!   [`AudioFrame`]s; allocation-free apart from the emitted frames,
//!   available in `no_std`.
//! - [`FilePcmReader`] / [`StdinPcmReader`] (`std`) — the same chunking
//!   over a file or stdin, reading whole chunks from the underlying
//!   reader so a chunk never splits mid-sample.

extern crate alloc;

#[cfg(feature = "std")]
use alloc::boxed::Box;
#[cfg(feature = "std")]
use alloc::vec::Vec;

use tpt_media_core::{AudioFrame, VideoFrame};

use crate::{IoError, IoResult, PcmSpec};

/// One item a streaming source can emit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaItem {
    /// A decoded/raw audio frame.
    Audio(AudioFrame),
    /// A decoded/raw video frame.
    Video(VideoFrame),
}

/// Pull-driven streaming source.
///
/// `poll` returns `Ok(Some(item))` while data is available, `Ok(None)`
/// once the stream is exhausted, or an error.
pub trait FrameSource {
    /// Produce the next item, or `None` at end of stream.
    fn poll(&mut self) -> IoResult<Option<MediaItem>>;
}

/// Chunk [`AudioFrame`]s of at most `frames_per_chunk` samples from a
/// borrowed PCM byte slice.
///
/// The slice is advanced in place, so a single reader can stream a buffer
/// that does not fit in memory. PTS is `1/sample_rate`-based and monotonic
/// (see [`crate::pcm::decode_pcm`]).
#[derive(Debug)]
pub struct SlicePcmReader<'a> {
    bytes: &'a [u8],
    spec: PcmSpec,
    frames_per_chunk: usize,
    offset: usize,
    index: u64,
}

impl<'a> SlicePcmReader<'a> {
    /// Build a chunked reader over the byte slice.
    pub fn new(bytes: &'a [u8], spec: PcmSpec, frames_per_chunk: usize) -> IoResult<Self> {
        if frames_per_chunk == 0 {
            return Err(IoError::Other("frames_per_chunk must be >= 1".into()));
        }
        let frame_bytes = spec.bytes_per_frame();
        if frame_bytes == 0 {
            return Err(IoError::Other("invalid PCM spec".into()));
        }
        if bytes.len() % frame_bytes != 0 {
            return Err(IoError::Other(
                "PCM data length must be a whole number of sample frames".into(),
            ));
        }
        Ok(SlicePcmReader {
            bytes,
            spec,
            frames_per_chunk,
            offset: 0,
            index: 0,
        })
    }

    /// Number of bytes consumed so far.
    pub fn bytes_consumed(&self) -> usize {
        self.offset
    }

    fn next_frame(&mut self) -> IoResult<AudioFrame> {
        let chunk_bytes = self.frames_per_chunk * self.spec.bytes_per_frame();
        let end = (self.offset + chunk_bytes).min(self.bytes.len());
        let mut frame = self.spec.into_frame(&self.bytes[self.offset..end])?;
        frame.frame.pts = tpt_media_core::Timestamp {
            ticks: (self.index * self.frames_per_chunk as u64) as i64,
            timebase: self.spec.timebase(),
        };
        self.offset = end;
        self.index += 1;
        Ok(frame)
    }
}

impl FrameSource for SlicePcmReader<'_> {
    fn poll(&mut self) -> IoResult<Option<MediaItem>> {
        if self.offset >= self.bytes.len() {
            return Ok(None);
        }
        Ok(Some(MediaItem::Audio(self.next_frame()?)))
    }
}

/// File-backed PCM chunk reader. (`std`)
#[cfg(feature = "std")]
pub struct FilePcmReader {
    inner: Box<dyn std::io::Read>,
    spec: PcmSpec,
    frames_per_chunk: usize,
    pending: Vec<u8>,
    eof: bool,
    truncated: bool,
    index: u64,
}

#[cfg(feature = "std")]
impl FilePcmReader {
    /// Read PCM from an open reader, emitting chunks of at most
    /// `frames_per_chunk` sample frames.
    pub fn new(
        reader: Box<dyn std::io::Read>,
        spec: PcmSpec,
        frames_per_chunk: usize,
    ) -> IoResult<Self> {
        if frames_per_chunk == 0 {
            return Err(IoError::Other("frames_per_chunk must be >= 1".into()));
        }
        let frame_bytes = spec.bytes_per_frame();
        if frame_bytes == 0 {
            return Err(IoError::Other("invalid PCM spec".into()));
        }
        frames_per_chunk
            .checked_mul(frame_bytes)
            .ok_or(IoError::Overflow)?;
        Ok(FilePcmReader {
            inner: reader,
            spec,
            frames_per_chunk,
            pending: Vec::new(),
            eof: false,
            truncated: false,
            index: 0,
        })
    }

    /// Fill `pending` up to at least one chunk, trimming to whole frames.
    fn ensure(&mut self) -> IoResult<()> {
        use std::io::Read;

        let frame_bytes = self.spec.bytes_per_frame();
        let chunk_bytes = self.frames_per_chunk * frame_bytes;
        let mut buf = [0u8; 8192];
        while self.pending.len() < chunk_bytes && !self.eof {
            let n = self
                .inner
                .read(&mut buf)
                .map_err(|e| IoError::Other(e.to_string()))?;
            if n == 0 {
                self.eof = true;
                break;
            }
            self.pending.extend_from_slice(&buf[..n]);
        }
        if self.eof && self.pending.len() % frame_bytes != 0 {
            self.truncated = true;
            let whole = (self.pending.len() / frame_bytes) * frame_bytes;
            self.pending.truncate(whole);
        }
        Ok(())
    }
}

#[cfg(feature = "std")]
impl FrameSource for FilePcmReader {
    fn poll(&mut self) -> IoResult<Option<MediaItem>> {
        self.ensure()?;
        if !self.pending.is_empty() {
            let frame_bytes = self.spec.bytes_per_frame();
            let chunk_bytes = self.frames_per_chunk * frame_bytes;
            let take = self.pending.len().min(chunk_bytes);
            let chunk = self.pending.drain(..take).collect::<Vec<_>>();
            let mut frame = self.spec.into_frame(&chunk)?;
            frame.frame.pts = tpt_media_core::Timestamp {
                ticks: (self.index * self.frames_per_chunk as u64) as i64,
                timebase: self.spec.timebase(),
            };
            self.index += 1;
            return Ok(Some(MediaItem::Audio(frame)));
        }
        if self.truncated {
            self.truncated = false;
            return Err(IoError::Other("PCM stream ended mid-sample-frame".into()));
        }
        Ok(None)
    }
}

/// Alias for [`FilePcmReader`] constructed over stdin. (`std`)
#[cfg(feature = "std")]
pub type StdinPcmReader = FilePcmReader;

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_media_core::SampleFormat;

    fn mono_s16_data() -> (PcmSpec, Vec<u8>) {
        let spec = PcmSpec::new(SampleFormat::S16, 1, 48_000).unwrap();
        let mut data = Vec::new();
        for v in 0..10i16 {
            data.extend_from_slice(&v.to_le_bytes());
        }
        (spec, data)
    }

    #[test]
    fn slice_reader_chunks_with_pts() {
        let (spec, data) = mono_s16_data();
        let mut r = SlicePcmReader::new(&data, spec, 4).unwrap();
        let a = r.poll().unwrap().unwrap();
        let b = r.poll().unwrap().unwrap();
        let c = r.poll().unwrap().unwrap();
        assert_eq!(r.poll().unwrap(), None);
        match (a, b, c) {
            (MediaItem::Audio(a), MediaItem::Audio(b), MediaItem::Audio(c)) => {
                assert_eq!(a.sample_count, 4);
                assert_eq!(a.frame.pts.ticks, 0);
                assert_eq!(b.sample_count, 4);
                assert_eq!(b.frame.pts.ticks, 4);
                assert_eq!(c.sample_count, 2);
                assert_eq!(c.frame.pts.ticks, 8);
            }
            _ => panic!("expected audio items"),
        }
    }

    #[test]
    fn slice_reader_rejects_partial_frame() {
        let spec = PcmSpec::new(SampleFormat::S16, 2, 48_000).unwrap();
        assert!(SlicePcmReader::new(&[0u8; 6], spec, 1).is_err());
    }

    #[test]
    fn slice_reader_empty_is_none() {
        let spec = PcmSpec::new(SampleFormat::S16, 1, 48_000).unwrap();
        let mut r = SlicePcmReader::new(&[], spec, 4).unwrap();
        assert!(matches!(r.poll(), Ok(None)));
    }

    #[cfg(feature = "std")]
    #[test]
    fn file_reader_matches_slice_reader() {
        let (spec, data) = mono_s16_data();
        let file = std::io::Cursor::new(data.clone());
        let mut fr = FilePcmReader::new(Box::new(file), spec, 4).unwrap();
        let mut slice = SlicePcmReader::new(&data, spec, 4).unwrap();
        loop {
            let a = fr.poll().unwrap();
            let b = slice.poll().unwrap();
            match (a, b) {
                (Some(MediaItem::Audio(a)), Some(MediaItem::Audio(b))) => {
                    assert_eq!(a.samples, b.samples);
                    assert_eq!(a.frame.pts, b.frame.pts);
                }
                (None, None) => break,
                other => panic!("divergence: {other:?}"),
            }
        }
    }

    #[cfg(feature = "std")]
    #[test]
    fn file_reader_reports_truncated_stream() {
        let spec = PcmSpec::new(SampleFormat::S16, 1, 48_000).unwrap();
        let truncated = vec![0u8; 5]; // 2.5 frames
        let file = std::io::Cursor::new(truncated);
        let mut fr = FilePcmReader::new(Box::new(file), spec, 4).unwrap();
        let first = fr.poll();
        assert!(matches!(first, Ok(Some(_)))); // 2 whole frames emitted
        assert!(fr.poll().is_err()); // then the truncation error
    }
}
