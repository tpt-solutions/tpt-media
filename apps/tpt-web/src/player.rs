//! The streaming player: push bytes in, pull decoded audio and video out.
//!
//! [`Player`] is the whole browser-side pipeline in plain Rust, with no
//! JavaScript, no wasm and no clock in it, so every behaviour is unit-tested
//! on the host:
//!
//! ```text
//!  push(bytes) --> StreamParser --> packets --> DecoderTable decoders
//!                  (bounded buffer)               |            |
//!                                          planar f32     RGBA8 pictures
//!                                          audio queue    video queue
//!                                                |            |
//!                                     pop_audio()/pop_video() (caller paces)
//! ```
//!
//! ## Flow control
//!
//! Nothing is decoded until [`Player::pump`] runs, and `pump` stops as soon
//! as the next packet's output queue is full
//! ([`WebLimits::max_audio_queue_ms`], [`WebLimits::max_video_queue_frames`]).
//! The stalled packet is held (at most one, bounded by `max_packet_bytes`)
//! and retried on the next `pump`. Combined with the parser's bounded input
//! buffer this gives end-to-end back-pressure: a fast network cannot outrun a
//! slow consumer, because [`Player::push`] returns
//! [`WebError::InputFull`] once the backlog is full and the caller must wait
//! for playback to drain.
//!
//! ## Timing
//!
//! The player does not schedule playback; it labels output with timestamps in
//! seconds and lets the page's audio clock pace it (see `web/tpt-web.js`).
//! Audio timestamps are derived from cumulative sample counts (exact and
//! drift-free), re-synchronised to the container's packet timestamps at the
//! start, after a `DISCONTINUITY` flag and after a dropped packet. Video
//! timestamps come from the packet that produced each picture, because the
//! codec-agnostic `DynFrame` carries none of its own.
//!
//! ## Errors
//!
//! Container-level violations and limit breaches are fatal
//! ([`Status::Failed`]). A packet that fails to *decode* is skipped, counted
//! and remembered ([`Player::last_decode_error`]): one corrupt packet costs a
//! glitch, not the stream, and [`WebLimits::max_decode_errors`] bounds how
//! long that tolerance lasts.

use std::collections::VecDeque;

use crate::codecs::DecoderTable;
use crate::convert::{audio_to_planar_f32, video_to_rgba};
use crate::error::{ContainerFault, WebError};
use crate::limits::WebLimits;
use crate::parser::{ParseEvent, ParserStats, StreamParser};
use tpt_media_codec::{CodecError, DynDecoder, DynFrame};
use tpt_media_core::{MediaType, MetadataValue, Packet, PacketFlags, Timebase, Track};

/// Track-metadata key under which `tpt encode` records the exact number of
/// source samples, so the codecs' block padding can be trimmed from the last
/// frame. (Mirrors the constant of the same value in `tpt-cli`; the key is
/// part of the file convention, not of either program.)
pub const SAMPLE_COUNT_KEY: &str = "tpt.io.sample_count";

/// Largest channel count the player will convert. WebAudio's guaranteed
/// minimum is 32, but every codec here is mono or stereo; 8 covers 7.1.
const MAX_AUDIO_CHANNELS: usize = 8;

/// Where the player is in its life. See [`Player::status`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Nothing to pull; more input is needed (or the stream has not been
    /// finished).
    NeedInput,
    /// At least one decoded audio chunk or video picture can be popped.
    Ready,
    /// Input is finished and everything decoded has been popped.
    Ended,
    /// A fatal error occurred; see [`Player::error`].
    Failed,
}

/// A block of decoded audio: planar `f32`, `channels` planes of `frames`
/// samples (`planar[ch * frames + i]`).
#[derive(Clone, Debug, PartialEq)]
pub struct AudioChunk {
    /// Presentation time of the first sample, in seconds.
    pub pts_seconds: f64,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Number of channels (planes).
    pub channels: u8,
    /// Samples per channel.
    pub frames: u32,
    /// Sample data, planar.
    pub planar: Vec<f32>,
}

impl AudioChunk {
    /// Duration of the chunk in seconds.
    pub fn duration_seconds(&self) -> f64 {
        f64::from(self.frames) / f64::from(self.sample_rate.max(1))
    }
}

/// A decoded picture, converted to RGBA8 (`width * height * 4` bytes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoPicture {
    /// Presentation time in seconds (as `f64` bits are not `Eq`, this is
    /// stored in microseconds).
    pub pts_micros: i64,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// RGBA8 pixels, row-major, no padding.
    pub rgba: Vec<u8>,
}

impl VideoPicture {
    /// Presentation time in seconds.
    pub fn pts_seconds(&self) -> f64 {
        self.pts_micros as f64 / 1_000_000.0
    }
}

/// The audio track the player selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioInfo {
    /// Decoder name (`"cinder"`, ...).
    pub codec: &'static str,
    /// Sample rate declared by the track (Hz).
    pub sample_rate: u32,
    /// Channel count declared by the track, if it declared one.
    pub channels: Option<u8>,
}

/// The video track the player selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoInfo {
    /// Decoder name (`"thrum"`, ...).
    pub codec: &'static str,
    /// Picture width in pixels.
    pub width: u32,
    /// Picture height in pixels.
    pub height: u32,
    /// Frame rate numerator (frames per `fps_den` seconds); one container
    /// tick is one frame, as written by `tpt encode`.
    pub fps_num: u32,
    /// Frame rate denominator.
    pub fps_den: u32,
}

/// What the player found in the stream so far.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamInfo {
    /// Selected audio track (the first audio track with a registered
    /// decoder), if any has been seen.
    pub audio: Option<AudioInfo>,
    /// Selected video track, likewise.
    pub video: Option<VideoInfo>,
    /// Tracks the player cannot play (no registered decoder for the codec, or
    /// a second track of a kind already selected).
    pub ignored_tracks: u32,
}

/// Counters for diagnostics and the demo's stats overlay.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct PlayerStats {
    /// Parser counters.
    pub parser: ParserStats,
    /// Audio chunks produced (before any were dropped).
    pub audio_chunks: u64,
    /// Audio samples per channel delivered to the queue.
    pub audio_samples: u64,
    /// Video pictures delivered to the queue.
    pub video_frames: u64,
    /// Packets that failed to decode (skipped).
    pub decode_errors: u32,
    /// Outputs discarded because a queue was already past twice its limit.
    pub dropped_outputs: u64,
    /// The input ended in the middle of a chunk.
    pub truncated_input: bool,
}

struct Pipe {
    track_id: u8,
    name: &'static str,
    decoder: Option<Box<dyn DynDecoder>>,
    /// Timestamp (seconds) the next audio sample / video frame is expected
    /// to carry; `None` means "take it from the next packet".
    next_pts: Option<f64>,
    /// Audio samples delivered so far (for trimming to the exact count).
    emitted: u64,
    /// Last video timestamp delivered, for drained frames.
    last_video_pts: f64,
}

impl Pipe {
    fn new(track_id: u8, name: &'static str) -> Self {
        Pipe {
            track_id,
            name,
            decoder: None,
            next_pts: None,
            emitted: 0,
            last_video_pts: 0.0,
        }
    }
}

/// The streaming player. See the [module docs](self).
pub struct Player {
    limits: WebLimits,
    table: DecoderTable,
    parser: StreamParser,
    tracks: Vec<Track>,
    audio: Option<Pipe>,
    video: Option<Pipe>,
    info: StreamInfo,
    audio_q: VecDeque<AudioChunk>,
    video_q: VecDeque<VideoPicture>,
    stalled: Option<Packet>,
    finished: bool,
    drained: bool,
    error: Option<WebError>,
    last_decode_error: Option<String>,
    stats: PlayerStats,
}

impl Player {
    /// A player over the native codec set (Cinder, Aura, Reed, Thrum) with
    /// `limits` (clamped to [`WebLimits::ceiling`]).
    pub fn new(limits: WebLimits) -> Self {
        Player::with_table(limits, DecoderTable::native())
    }

    /// A player over a caller-supplied codec table: the extension point for
    /// codecs this crate does not yet depend on.
    pub fn with_table(limits: WebLimits, table: DecoderTable) -> Self {
        let limits = limits.clamped();
        Player {
            limits,
            table,
            parser: StreamParser::new(limits),
            tracks: Vec::new(),
            audio: None,
            video: None,
            info: StreamInfo::default(),
            audio_q: VecDeque::new(),
            video_q: VecDeque::new(),
            stalled: None,
            finished: false,
            drained: false,
            error: None,
            last_decode_error: None,
            stats: PlayerStats::default(),
        }
    }

    /// The (clamped) limits in force.
    pub fn limits(&self) -> &WebLimits {
        &self.limits
    }

    /// What has been learned about the stream so far.
    pub fn info(&self) -> &StreamInfo {
        &self.info
    }

    /// Counters.
    pub fn stats(&self) -> PlayerStats {
        PlayerStats {
            parser: self.parser.stats(),
            ..self.stats
        }
    }

    /// The fatal error, if the player has failed.
    pub fn error(&self) -> Option<&WebError> {
        self.error.as_ref()
    }

    /// Message of the most recent packet that failed to decode.
    pub fn last_decode_error(&self) -> Option<&str> {
        self.last_decode_error.as_deref()
    }

    /// Append container bytes.
    ///
    /// [`WebError::InputFull`] means the backlog is full and *nothing was
    /// consumed*: call [`Player::pump`], pop output, and push the same bytes
    /// again. Any other error is fatal.
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), WebError> {
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        if self.finished {
            return Err(WebError::InputClosed);
        }
        match self.parser.push(bytes) {
            Ok(()) => Ok(()),
            Err(WebError::InputFull) => Err(WebError::InputFull),
            Err(e) => Err(self.fail(e)),
        }
    }

    /// Declare that no more input will arrive. Remaining packets are decoded
    /// by later [`Player::pump`] calls and the decoders are drained, after
    /// which the status becomes [`Status::Ended`]. A stream cut off mid-chunk
    /// is not an error: it ends after the last complete packet
    /// ([`PlayerStats::truncated_input`]).
    pub fn finish_input(&mut self) {
        self.finished = true;
    }

    fn fail(&mut self, e: WebError) -> WebError {
        self.error = Some(e.clone());
        // Free everything: a failed player never produces output again.
        self.audio_q = VecDeque::new();
        self.video_q = VecDeque::new();
        self.stalled = None;
        self.audio = None;
        self.video = None;
        e
    }

    /// Current status. Cheap; does no work.
    pub fn status(&self) -> Status {
        if self.error.is_some() {
            Status::Failed
        } else if !self.audio_q.is_empty() || !self.video_q.is_empty() {
            Status::Ready
        } else if self.finished && self.drained {
            Status::Ended
        } else {
            Status::NeedInput
        }
    }

    /// Parse and decode buffered input until either the input runs out or
    /// the output queue for the next packet is full, then report
    /// [`Player::status`]. Fatal errors are returned (and remembered).
    pub fn pump(&mut self) -> Result<Status, WebError> {
        if let Some(e) = &self.error {
            return Err(e.clone());
        }
        loop {
            let pkt = match self.stalled.take() {
                Some(p) => p,
                None => match self.parser.poll() {
                    Ok(Some(ParseEvent::Track(t))) => {
                        self.on_track(t).map_err(|e| self.fail(e))?;
                        continue;
                    }
                    Ok(Some(ParseEvent::Packet(p))) => p,
                    Ok(None) => break,
                    Err(e) => return Err(self.fail(e)),
                },
            };
            if !self.can_accept(&pkt) {
                self.stalled = Some(pkt);
                return Ok(self.status());
            }
            self.handle_packet(pkt).map_err(|e| self.fail(e))?;
        }
        if self.finished && !self.drained {
            self.drain().map_err(|e| self.fail(e))?;
        }
        Ok(self.status())
    }

    /// Take the next decoded audio chunk, if any.
    pub fn pop_audio(&mut self) -> Option<AudioChunk> {
        self.audio_q.pop_front()
    }

    /// Take the next decoded picture, if any.
    pub fn pop_video(&mut self) -> Option<VideoPicture> {
        self.video_q.pop_front()
    }

    /// Timestamp (seconds) of the picture `pop_video` would return.
    pub fn peek_video_pts(&self) -> Option<f64> {
        self.video_q.front().map(VideoPicture::pts_seconds)
    }

    /// Milliseconds of decoded audio waiting to be popped.
    pub fn audio_queued_ms(&self) -> f64 {
        self.audio_q
            .iter()
            .map(|c| c.duration_seconds() * 1000.0)
            .sum()
    }

    /// Decoded pictures waiting to be popped.
    pub fn video_queued(&self) -> usize {
        self.video_q.len()
    }

    /// Bytes the player currently holds: input buffer capacity, the stalled
    /// packet, and both output queues. Codec-internal state is bounded
    /// separately by `limits.codec.max_allocation_bytes`. Never exceeds
    /// [`WebLimits::memory_ceiling_bytes`].
    pub fn memory_in_use(&self) -> usize {
        let audio: usize = self
            .audio_q
            .iter()
            .map(|c| c.planar.capacity() * core::mem::size_of::<f32>())
            .sum();
        let video: usize = self.video_q.iter().map(|p| p.rgba.capacity()).sum();
        let stalled = self.stalled.as_ref().map_or(0, |p| p.data.capacity());
        self.parser.capacity_bytes() + audio + video + stalled
    }

    // ---- internals -------------------------------------------------------

    fn on_track(&mut self, track: Track) -> Result<(), WebError> {
        if let Some(existing) = self.tracks.iter_mut().find(|t| t.id == track.id) {
            // A TMTA update for a known track: refresh metadata only. A
            // decoder that already exists keeps the parameters it was built
            // with.
            *existing = track;
            return Ok(());
        }
        let selected = match track.media_type {
            MediaType::Audio => self.audio.is_none(),
            MediaType::Video => self.video.is_none(),
            _ => false,
        };
        let name = self
            .table
            .name(track.codec_config.codec_id, track.media_type);
        match (selected, name) {
            (true, Some(name)) => match track.media_type {
                MediaType::Audio => {
                    self.info.audio = Some(AudioInfo {
                        codec: name,
                        sample_rate: track.media.sample_rate.unwrap_or(48_000),
                        channels: track.media.channel_layout.map(|l| l.channels()),
                    });
                    self.audio = Some(Pipe::new(track.id, name));
                }
                _ => {
                    let (w, h) = (
                        track.media.width.unwrap_or(0),
                        track.media.height.unwrap_or(0),
                    );
                    self.check_video_geometry(w, h)?;
                    let tb = track.timebase;
                    self.info.video = Some(VideoInfo {
                        codec: name,
                        width: w,
                        height: h,
                        fps_num: tb.den,
                        fps_den: tb.num,
                    });
                    self.video = Some(Pipe::new(track.id, name));
                }
            },
            _ => self.info.ignored_tracks += 1,
        }
        self.tracks.push(track);
        Ok(())
    }

    /// Reject pictures whose decoded (native) or RGBA form would exceed
    /// `max_frame_bytes`, before any decoder allocates for them.
    fn check_video_geometry(&self, w: u32, h: u32) -> Result<(), WebError> {
        let rgba = u64::from(w) * u64::from(h) * 4;
        if rgba > u64::from(self.limits.codec.max_frame_bytes) {
            return Err(WebError::Limit("frame dimensions"));
        }
        Ok(())
    }

    fn pipe_kind(&self, track_id: u8) -> Option<MediaType> {
        if self.audio.as_ref().is_some_and(|p| p.track_id == track_id) {
            Some(MediaType::Audio)
        } else if self.video.as_ref().is_some_and(|p| p.track_id == track_id) {
            Some(MediaType::Video)
        } else {
            None
        }
    }

    /// Would this packet's output fit in its queue?
    fn can_accept(&self, pkt: &Packet) -> bool {
        match self.pipe_kind(pkt.track_id) {
            Some(MediaType::Audio) => {
                self.audio_queued_ms() < f64::from(self.limits.max_audio_queue_ms)
            }
            Some(_) => self.video_q.len() < self.limits.max_video_queue_frames as usize,
            None => true,
        }
    }

    fn track(&self, id: u8) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    fn ensure_decoder(&mut self, kind: MediaType) -> Result<(), WebError> {
        let id = match kind {
            MediaType::Audio => self.audio.as_ref().map(|p| p.track_id),
            _ => self.video.as_ref().map(|p| p.track_id),
        };
        let Some(id) = id else { return Ok(()) };
        let already = match kind {
            MediaType::Audio => self.audio.as_ref().is_some_and(|p| p.decoder.is_some()),
            _ => self.video.as_ref().is_some_and(|p| p.decoder.is_some()),
        };
        if already {
            return Ok(());
        }
        let track = self
            .track(id)
            .ok_or_else(|| WebError::Unsupported("track vanished".into()))?
            .clone();
        let built = self.table.build(&track, &self.limits.codec)?;
        let Some(decoder) = built else {
            return Err(WebError::Unsupported(format!(
                "no decoder for codec {}",
                track.codec_config.codec_id
            )));
        };
        let pipe = match kind {
            MediaType::Audio => self.audio.as_mut(),
            _ => self.video.as_mut(),
        };
        if let Some(p) = pipe {
            p.decoder = Some(decoder);
        }
        Ok(())
    }

    fn handle_packet(&mut self, pkt: Packet) -> Result<(), WebError> {
        let Some(kind) = self.pipe_kind(pkt.track_id) else {
            return Ok(()); // a track we are not playing
        };
        self.ensure_decoder(kind)?;
        let pts = ticks_to_seconds(pkt.pts.ticks, pkt.pts.timebase);
        let discontinuity = pkt.flags.contains(PacketFlags::DISCONTINUITY);
        let pipe = match kind {
            MediaType::Audio => self.audio.as_mut(),
            _ => self.video.as_mut(),
        };
        let Some(pipe) = pipe else { return Ok(()) };
        if discontinuity {
            pipe.next_pts = None;
        }
        let Some(decoder) = pipe.decoder.as_mut() else {
            return Ok(());
        };
        match decoder.decode(&pkt) {
            Ok(frames) => {
                match kind {
                    MediaType::Audio => self.emit_audio(frames, pts),
                    _ => self.emit_video(frames, pts, pkt.pts.timebase),
                }
                Ok(())
            }
            Err(CodecError::NeedMoreInput) => Ok(()),
            Err(e) => {
                // Lose sync: the next good packet re-anchors the clock.
                if let Some(p) = match kind {
                    MediaType::Audio => self.audio.as_mut(),
                    _ => self.video.as_mut(),
                } {
                    p.next_pts = None;
                }
                let name = pipe_name(self, kind);
                self.decode_failed(format!("{name}: {e}"))
            }
        }
    }

    fn decode_failed(&mut self, msg: String) -> Result<(), WebError> {
        self.stats.decode_errors = self.stats.decode_errors.saturating_add(1);
        self.last_decode_error = Some(msg);
        if self.stats.decode_errors > self.limits.max_decode_errors {
            return Err(WebError::TooManyDecodeErrors);
        }
        Ok(())
    }

    /// Exact source sample count recorded in the track metadata, if any.
    fn exact_samples(&self, track_id: u8) -> Option<u64> {
        self.track(track_id)?
            .metadata
            .iter()
            .find(|(k, _)| k == SAMPLE_COUNT_KEY)
            .and_then(|(_, v)| match v {
                MetadataValue::Int(n) => u64::try_from(*n).ok(),
                _ => None,
            })
    }

    fn emit_audio(&mut self, frames: Vec<DynFrame>, packet_pts: f64) {
        let max_ms = f64::from(self.limits.max_audio_queue_ms);
        for frame in frames {
            let DynFrame::Audio {
                sample_format,
                channel_layout,
                sample_rate,
                sample_count,
                samples,
            } = frame
            else {
                let _ = self.decode_failed("audio decoder produced a non-audio frame".into());
                continue;
            };
            let channels = usize::from(channel_layout.channels());
            let rate = sample_rate;
            let count = sample_count as usize;
            if channels == 0 || channels > MAX_AUDIO_CHANNELS || rate == 0 {
                let _ = self.decode_failed("audio frame has an unusable format".into());
                continue;
            }
            // A single frame longer than the whole queue budget could never
            // be queued honestly.
            if f64::from(sample_count) * 1000.0 / f64::from(rate) > max_ms {
                let _ = self.decode_failed("audio frame longer than the queue limit".into());
                continue;
            }
            let Some(mut planar) = audio_to_planar_f32(sample_format, channels, count, &samples)
            else {
                let _ = self.decode_failed("audio frame size does not match its format".into());
                continue;
            };
            drop(samples);
            let Some(id) = self.audio.as_ref().map(|p| p.track_id) else {
                return;
            };
            let exact = self.exact_samples(id);
            let Some(pipe) = self.audio.as_mut() else {
                return;
            };
            let mut frames_out = count;
            if let Some(exact) = exact {
                let remaining = exact.saturating_sub(pipe.emitted);
                if remaining == 0 {
                    continue; // padding past the recorded end
                }
                if (frames_out as u64) > remaining {
                    frames_out = remaining as usize;
                    planar = truncate_planar(&planar, channels, count, frames_out);
                }
            }
            if frames_out == 0 {
                continue;
            }
            let pts = pipe.next_pts.unwrap_or(packet_pts);
            pipe.next_pts = Some(pts + frames_out as f64 / f64::from(rate));
            pipe.emitted += frames_out as u64;
            self.stats.audio_chunks += 1;
            self.stats.audio_samples += frames_out as u64;
            // Queue hard stop at twice the limit, so a decoder that returns
            // a burst of frames cannot grow the queue without bound.
            if self.audio_queued_ms() >= 2.0 * max_ms {
                self.stats.dropped_outputs += 1;
                continue;
            }
            self.audio_q.push_back(AudioChunk {
                pts_seconds: pts,
                sample_rate: rate,
                channels: channels as u8,
                frames: frames_out as u32,
                planar,
            });
        }
    }

    fn emit_video(&mut self, frames: Vec<DynFrame>, packet_pts: f64, tb: Timebase) {
        let tick = ticks_to_seconds(1, tb);
        let hard_stop = 2 * self.limits.max_video_queue_frames as usize;
        for (i, frame) in frames.into_iter().enumerate() {
            let DynFrame::Video {
                pixel_format,
                width,
                height,
                stride,
                pixels,
            } = frame
            else {
                let _ = self.decode_failed("video decoder produced a non-video frame".into());
                continue;
            };
            if self.check_video_geometry(width, height).is_err() {
                let _ = self.decode_failed("decoded picture exceeds the frame limit".into());
                continue;
            }
            let Some(rgba) = video_to_rgba(pixel_format, width, height, stride, &pixels) else {
                let _ = self.decode_failed("video frame size does not match its format".into());
                continue;
            };
            drop(pixels);
            let Some(pipe) = self.video.as_mut() else {
                return;
            };
            let pts = if i == 0 {
                packet_pts
            } else {
                pipe.last_video_pts + tick
            };
            pipe.last_video_pts = pts;
            self.stats.video_frames += 1;
            if self.video_q.len() >= hard_stop {
                self.stats.dropped_outputs += 1;
                continue;
            }
            self.video_q.push_back(VideoPicture {
                pts_micros: (pts * 1_000_000.0).round() as i64,
                width,
                height,
                rgba,
            });
        }
    }

    /// End of stream: ask every decoder for frames it held back.
    fn drain(&mut self) -> Result<(), WebError> {
        if self.parser.has_partial_tail() {
            self.stats.truncated_input = true;
        }
        if self.parser.header().is_none() {
            return Err(WebError::Container(ContainerFault::Truncated));
        }
        if self.audio.is_none() && self.video.is_none() {
            return Err(WebError::Unsupported(
                "the file has no audio or video track this player can decode".into(),
            ));
        }
        for kind in [MediaType::Audio, MediaType::Video] {
            let pipe = match kind {
                MediaType::Audio => self.audio.as_mut(),
                _ => self.video.as_mut(),
            };
            let Some(pipe) = pipe else { continue };
            let base = pipe.next_pts.unwrap_or(pipe.last_video_pts);
            let Some(dec) = pipe.decoder.as_mut() else {
                continue;
            };
            match dec.drain() {
                Ok(frames) => match kind {
                    MediaType::Audio => self.emit_audio(frames, base),
                    _ => {
                        let tb = self
                            .video
                            .as_ref()
                            .and_then(|p| self.track(p.track_id))
                            .map_or(Timebase::MILLISECONDS, |t| t.timebase);
                        let start = self.video.as_ref().map_or(0.0, |p| p.last_video_pts)
                            + ticks_to_seconds(1, tb);
                        self.emit_video(frames, start, tb)
                    }
                },
                Err(e) => {
                    let name = pipe_name(self, kind);
                    self.decode_failed(format!("{name}: drain: {e}"))?;
                }
            }
        }
        self.drained = true;
        Ok(())
    }
}

fn pipe_name(p: &Player, kind: MediaType) -> &'static str {
    match kind {
        MediaType::Audio => p.audio.as_ref().map_or("audio", |x| x.name),
        _ => p.video.as_ref().map_or("video", |x| x.name),
    }
}

/// `ticks * num / den` seconds; a zero denominator yields 0 rather than a
/// division by zero, and the result is always finite.
fn ticks_to_seconds(ticks: i64, tb: Timebase) -> f64 {
    if tb.den == 0 {
        return 0.0;
    }
    ticks as f64 * f64::from(tb.num) / f64::from(tb.den)
}

/// Keep the first `keep` samples of every plane of a planar buffer that has
/// `frames` samples per plane.
fn truncate_planar(planar: &[f32], channels: usize, frames: usize, keep: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(channels * keep);
    for ch in 0..channels {
        out.extend_from_slice(&planar[ch * frames..ch * frames + keep]);
    }
    out
}

#[cfg(test)]
mod tests;
