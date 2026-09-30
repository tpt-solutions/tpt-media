//! Hard resource limits for a browser player (todo.md sections 19 and 45,
//! "memory limits").
//!
//! A WebAssembly instance's linear memory can grow but never shrink, and a
//! failed growth aborts the module. So every buffer the player owns is
//! bounded up front, and the bounds are checked *before* allocating:
//!
//! | what | bounded by |
//! | --- | --- |
//! | bytes buffered but not yet parsed | [`WebLimits::max_input_buffer_bytes`] |
//! | one container chunk / packet | `codec.max_packet_bytes` (+ framing), `codec.max_metadata_bytes` |
//! | one decoded frame | `codec.max_frame_bytes` (native *and* RGBA size) |
//! | decoder-internal allocation | `codec.max_allocation_bytes` (enforced by the codec) |
//! | decoded audio waiting to play | [`WebLimits::max_audio_queue_ms`] |
//! | decoded video waiting to show | [`WebLimits::max_video_queue_frames`] |
//! | tracks | [`WebLimits::max_tracks`] |
//! | undecodable packets tolerated | [`WebLimits::max_decode_errors`] |
//!
//! [`WebLimits::default`] is deliberately conservative (it starts from
//! [`ResourceLimits::strict`]); a page that trusts its source can raise
//! individual fields, but [`WebLimits::clamped`] bounds every field by
//! [`WebLimits::ceiling`] so neither a caller nor a hostile script talking
//! to the C ABI can ask for an unbounded budget.

use tpt_media_codec::ResourceLimits;

/// Bytes of container framing around a cluster payload (`CLUS` fixed prefix,
/// optional duration and CRC) plus slack, added to `max_packet_bytes` to get
/// the largest acceptable `CLUS` chunk body.
pub(crate) const CLUS_OVERHEAD: usize = 64;
/// Largest single slice the C ABI will stage for JavaScript to fill
/// (`tpt_web_input_ptr`). Bounds the staging buffer independently of the
/// input limit; the page pushes larger files in slices.
pub(crate) const STAGING_MAX_BYTES: usize = 4 * 1024 * 1024;
/// Largest acceptable `TRAK` chunk body. A `TRAK` holds a fixed descriptor of
/// a few dozen bytes, `u16`-bounded codec-private data and two short strings.
pub(crate) const MAX_TRAK_BYTES: usize = 256 * 1024;

/// Player-level resource limits. See the [module docs](self).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WebLimits {
    /// Limits handed to every decoder and used for packet / metadata /
    /// frame-size checks.
    pub codec: ResourceLimits,
    /// Maximum bytes held in the parser's input buffer (received, not yet
    /// consumed). [`crate::Player::push`] refuses input that would exceed it.
    pub max_input_buffer_bytes: u32,
    /// Maximum decoded audio queued for playback, in milliseconds.
    pub max_audio_queue_ms: u32,
    /// Maximum decoded video frames queued for display.
    pub max_video_queue_frames: u32,
    /// Maximum number of tracks in the container.
    pub max_tracks: u8,
    /// Undecodable packets tolerated (skipped and counted) before the stream
    /// is failed. Guarantees termination on hostile input.
    pub max_decode_errors: u32,
}

impl WebLimits {
    /// Conservative defaults for untrusted content: strict codec limits with
    /// an 8 MiB frame cap, 16 MiB of input buffer, 2 s of audio, 4 video
    /// frames.
    pub const fn strict() -> Self {
        let base = ResourceLimits::strict();
        WebLimits {
            codec: ResourceLimits {
                // 8 MiB: a 1080p RGBA picture (8.29 MB) still fits.
                max_frame_bytes: 8 * 1024 * 1024,
                ..base
            },
            max_input_buffer_bytes: 16 * 1024 * 1024,
            max_audio_queue_ms: 2_000,
            max_video_queue_frames: 4,
            max_tracks: 16,
            max_decode_errors: 256,
        }
    }

    /// The absolute maximum any field may be raised to; [`clamped`] enforces
    /// it.
    ///
    /// [`clamped`]: WebLimits::clamped
    pub const fn ceiling() -> Self {
        WebLimits {
            codec: ResourceLimits {
                max_frame_bytes: 64 * 1024 * 1024,
                max_packet_bytes: 16 * 1024 * 1024,
                max_reference_frames: 16,
                max_allocation_bytes: 128 * 1024 * 1024,
                max_recursion_depth: 16,
                max_decode_micros: 0,
                max_metadata_bytes: 1024 * 1024,
            },
            max_input_buffer_bytes: 64 * 1024 * 1024,
            max_audio_queue_ms: 30_000,
            max_video_queue_frames: 32,
            max_tracks: 64,
            max_decode_errors: 4_096,
        }
    }

    /// Clamp every field to [`WebLimits::ceiling`], and raise
    /// `max_input_buffer_bytes` to at least one whole worst-case chunk so a
    /// legal chunk can always be buffered.
    pub fn clamped(mut self) -> Self {
        let c = Self::ceiling();
        self.codec.max_frame_bytes = self.codec.max_frame_bytes.min(c.codec.max_frame_bytes);
        self.codec.max_packet_bytes = self.codec.max_packet_bytes.min(c.codec.max_packet_bytes);
        self.codec.max_reference_frames = self
            .codec
            .max_reference_frames
            .min(c.codec.max_reference_frames);
        self.codec.max_allocation_bytes = self
            .codec
            .max_allocation_bytes
            .min(c.codec.max_allocation_bytes);
        self.codec.max_metadata_bytes = self
            .codec
            .max_metadata_bytes
            .min(c.codec.max_metadata_bytes);
        self.codec.max_recursion_depth = self
            .codec
            .max_recursion_depth
            .min(c.codec.max_recursion_depth);
        self.max_input_buffer_bytes = self.max_input_buffer_bytes.min(c.max_input_buffer_bytes);
        self.max_audio_queue_ms = self.max_audio_queue_ms.clamp(1, c.max_audio_queue_ms);
        self.max_video_queue_frames = self
            .max_video_queue_frames
            .clamp(1, c.max_video_queue_frames);
        self.max_tracks = self.max_tracks.clamp(1, c.max_tracks);
        self.max_decode_errors = self.max_decode_errors.min(c.max_decode_errors);
        let floor = self.min_input_buffer();
        if (self.max_input_buffer_bytes as usize) < floor {
            self.max_input_buffer_bytes = floor as u32;
        }
        self
    }

    /// Smallest input buffer that can hold the largest chunk the parser will
    /// accept (plus its 8-byte chunk header and slack).
    pub(crate) fn min_input_buffer(&self) -> usize {
        let clus = self.codec.max_packet_bytes as usize + CLUS_OVERHEAD;
        let tmta = self.codec.max_metadata_bytes as usize;
        clus.max(tmta).max(MAX_TRAK_BYTES) + 8 + 64
    }

    /// Upper bound, in bytes, on the memory the player itself owns under
    /// these limits, for a stream whose largest decoded picture is
    /// `codec.max_frame_bytes` and whose audio is at most 8 channels of
    /// `f32` at 192 kHz.
    ///
    /// It is the sum of: the input buffer (counted twice, because `Vec`
    /// growth can overshoot to 2x), the C ABI's input staging slice, one
    /// in-flight packet, the codec's own allocation budget, an audio queue at
    /// its hard stop (twice the soft limit) plus the frame that crosses it and
    /// the chunk being handed to the caller, and a video queue at its hard stop (twice the soft limit)
    /// plus the picture being handed out and the decoder's transient native
    /// frame, each bounded by `max_frame_bytes`. Wasm linear memory needs this **plus** the module's
    /// static data and stack (about a megabyte) plus allocator
    /// fragmentation.
    pub fn memory_ceiling_bytes(&self) -> u64 {
        let input = 2 * u64::from(self.max_input_buffer_bytes) + STAGING_MAX_BYTES as u64;
        let packet = u64::from(self.codec.max_packet_bytes);
        let codec = u64::from(self.codec.max_allocation_bytes);
        let audio_per_ms = 192 * 8 * 4;
        // Hard stop at 2x the soft limit, plus the frame that crosses it (a
        // frame longer than the limit is rejected, so <= 1x), plus the chunk
        // in the caller's hands (<= 1x), plus the decoder's transient
        // native-format frame and its f32 conversion.
        let audio = 4 * u64::from(self.max_audio_queue_ms) * audio_per_ms
            + 2 * u64::from(self.codec.max_frame_bytes);
        // An RGBA picture is itself bounded by `max_frame_bytes` (the player
        // rejects larger ones), and the native 4:2:0 frame it came from is
        // at most 3/8 of that; one extra RGBA-sized slot covers it.
        let rgba = u64::from(self.codec.max_frame_bytes);
        let video = (2 * u64::from(self.max_video_queue_frames) + 3).saturating_mul(rgba);
        input + packet + codec + audio + video
    }
}

impl Default for WebLimits {
    fn default() -> Self {
        WebLimits::strict().clamped()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamped_never_exceeds_ceiling_and_never_below_floor() {
        let wild = WebLimits {
            codec: ResourceLimits::permissive(),
            max_input_buffer_bytes: u32::MAX,
            max_audio_queue_ms: u32::MAX,
            max_video_queue_frames: u32::MAX,
            max_tracks: u8::MAX,
            max_decode_errors: u32::MAX,
        }
        .clamped();
        let c = WebLimits::ceiling();
        assert_eq!(wild.max_input_buffer_bytes, c.max_input_buffer_bytes);
        assert_eq!(wild.codec.max_frame_bytes, c.codec.max_frame_bytes);
        assert_eq!(wild.max_video_queue_frames, c.max_video_queue_frames);
        assert!(wild.memory_ceiling_bytes() <= c.memory_ceiling_bytes());

        let tiny = WebLimits {
            max_input_buffer_bytes: 0,
            max_audio_queue_ms: 0,
            max_video_queue_frames: 0,
            max_tracks: 0,
            ..WebLimits::strict()
        }
        .clamped();
        assert!(tiny.max_input_buffer_bytes as usize >= tiny.min_input_buffer());
        assert_eq!(tiny.max_audio_queue_ms, 1);
        assert_eq!(tiny.max_video_queue_frames, 1);
        assert_eq!(tiny.max_tracks, 1);
    }

    #[test]
    fn default_is_strict_and_fits_a_modest_budget() {
        let d = WebLimits::default();
        assert_eq!(d, WebLimits::strict().clamped());
        // The default worst case is a fixed, documented figure (about
        // 223 MiB, dominated by eleven worst-case 8 MiB pictures) that a
        // page can reason about; real streams sit far below it.
        let mib = d.memory_ceiling_bytes() / (1024 * 1024);
        assert!((200..260).contains(&mib), "{mib} MiB");
    }
}
