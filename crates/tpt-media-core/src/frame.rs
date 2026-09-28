//! Decoded sample/pixel frames.

use alloc::vec;
use alloc::vec::Vec;

use crate::channel_layout::ChannelLayout;
use crate::pixel_format::PixelFormat;
use crate::sample_format::SampleFormat;
use crate::timebase::Timestamp;

/// Common decoded-frame interface.
///
/// A `Frame` is the result of a successful decode call. It carries a
/// PTS, a duration, and codec-private side data. Concrete variants
/// ([`AudioFrame`], [`VideoFrame`]) add typed sample/pixel buffers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Presentation timestamp.
    pub pts: Timestamp,
    /// Duration of this frame.
    pub duration: Option<crate::timebase::Duration>,
    /// Codec-private side data (e.g. SEI-like messages).
    pub side_data: Vec<SideData>,
}

/// Opaque codec side-data block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SideData {
    /// Four-byte side-data type tag.
    pub kind: [u8; 4],
    /// Payload bytes.
    pub data: Vec<u8>,
}

/// A decoded audio frame.
///
/// Sample data is stored in a single `Vec<u8>` together with a
/// [`SampleFormat`] describing how to interpret it. Decoders that
/// produce planar audio store the samples channel-interleaved; planar
/// is the caller's responsibility to repack if desired.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioFrame {
    /// Common frame fields.
    pub frame: Frame,
    /// Interleaved (or planar-flattened) sample bytes.
    pub samples: Vec<u8>,
    /// Number of samples per channel.
    pub sample_count: u32,
    /// Sample format and channel layout.
    pub format: SampleFormat,
    /// Channel layout.
    pub channel_layout: ChannelLayout,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

impl AudioFrame {
    /// Allocate an empty audio frame for the given sample count.
    pub fn alloc(
        sample_count: u32,
        format: SampleFormat,
        channel_layout: ChannelLayout,
        sample_rate: u32,
    ) -> Self {
        let bytes_per_sample = format.bytes_per_sample();
        let channels = channel_layout.channels() as usize;
        let samples = vec![0u8; sample_count as usize * channels * bytes_per_sample];
        AudioFrame {
            frame: Frame {
                pts: Timestamp::zero(crate::timebase::Timebase::ONE_PER_SECOND),
                duration: None,
                side_data: Vec::new(),
            },
            samples,
            sample_count,
            format,
            channel_layout,
            sample_rate,
        }
    }

    /// Read an interleaved `i16` sample at `(channel, frame_index)`.
    ///
    /// Returns `None` if the frame is not in `S16` format or the
    /// indices are out of bounds.
    pub fn s16(&self, channel: usize, frame_index: usize) -> Option<i16> {
        if self.format != SampleFormat::S16 {
            return None;
        }
        let channels = self.channel_layout.channels() as usize;
        if channel >= channels || frame_index >= self.sample_count as usize {
            return None;
        }
        let offset = (frame_index * channels + channel) * 2;
        let lo = *self.samples.get(offset)? as u16;
        let hi = *self.samples.get(offset + 1)? as u16;
        Some(i16::from_le_bytes([lo as u8, hi as u8]))
    }
}

/// A decoded video frame.
///
/// Pixels are stored in a single `Vec<u8>` in the decoder's native
/// layout; the [`PixelFormat`] tells the caller how to interpret it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoFrame {
    /// Common frame fields.
    pub frame: Frame,
    /// Pixel bytes.
    pub pixels: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel format.
    pub format: PixelFormat,
    /// Row stride in bytes (>= width * bpp).
    pub stride: u32,
}

impl VideoFrame {
    /// Allocate an empty video frame for the given dimensions.
    ///
    /// For the subsampled planar formats ([`PixelFormat::Yuv420`] and
    /// [`PixelFormat::Nv12`]) the buffer is `1.5 × w × h`: `stride × h`
    /// luma bytes followed by `stride/2 × h/2` bytes for each of the two
    /// chroma planes (U then V — the standard I420 plane order).
    pub fn alloc(width: u32, height: u32, format: PixelFormat) -> Self {
        let stride = width * format.bytes_per_pixel() as u32;
        let luma = (stride * height) as usize;
        let extra = match format {
            PixelFormat::Yuv420 | PixelFormat::Nv12 => luma / 2,
            _ => 0,
        };
        let pixels = vec![0u8; luma + extra];
        VideoFrame {
            frame: Frame {
                pts: Timestamp::zero(crate::timebase::Timebase::ONE_PER_SECOND),
                duration: None,
                side_data: Vec::new(),
            },
            pixels,
            width,
            height,
            format,
            stride,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_frame_alloc_and_s16() {
        let f = AudioFrame::alloc(4, SampleFormat::S16, ChannelLayout::Stereo, 48_000);
        assert_eq!(f.samples.len(), 4 * 2 * 2);
        assert_eq!(f.s16(0, 0), Some(0));
        assert_eq!(f.s16(1, 3), Some(0));
    }

    #[test]
    fn video_frame_alloc() {
        let f = VideoFrame::alloc(16, 8, PixelFormat::Rgb24);
        assert_eq!(f.stride, 48);
        assert_eq!(f.pixels.len(), 48 * 8);
    }
}
