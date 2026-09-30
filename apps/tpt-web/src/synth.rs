//! Deterministic synthetic media, encoded with the real encoders.
//!
//! This module exists for the crate's own tests, the demo-file generator
//! (`examples/make_demo.rs`) and the benchmark: none of them can use
//! `tpt-cli` (this crate does not depend on an app), and hand-written
//! packets are structurally valid but are not real bitstreams, so anything
//! that needs decoding to succeed has to be built through the encoders. It
//! is behind the `synth` feature so a consumer of the player does not link
//! the encoders.
//!
//! The container layout matches what `tpt encode` writes: track 0 audio /
//! track 1 video, codec-private bytes as the CLI writes them, and the
//! [`SAMPLE_COUNT_KEY`](crate::SAMPLE_COUNT_KEY) metadata entry.

use crate::player::SAMPLE_COUNT_KEY;
use tpt_audio_aura::{AuraConfig, AuraEncoder, AuraMode};
use tpt_audio_cinder::{CinderEncoder, CinderEncoderConfig};
use tpt_audio_reed::{ReedConfig, ReedEncoder};
use tpt_container::TptMuxer;
use tpt_media_codec::{CodecConfig as FrameworkConfig, Encoder};
use tpt_media_core::{
    AudioFrame, ChannelLayout, CodecId, Frame, MediaType, MetadataValue, Packet, PixelFormat,
    SampleFormat, Timebase, Timestamp, Track, TrackMedia, VideoFrame,
};
use tpt_video_thrum::{ThrumConfig, ThrumEncoder};

/// Which audio codec to encode with.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AudioKind {
    /// Cinder (lossy MDCT), 44.1 or 48 kHz.
    Cinder,
    /// Aura, lossless mode, 44.1 or 48 kHz.
    Aura,
    /// Reed (speech), 8 or 16 kHz.
    Reed,
}

impl AudioKind {
    fn id(self) -> CodecId {
        CodecId::new(match self {
            AudioKind::Cinder => *b"CIND",
            AudioKind::Aura => *b"AURA",
            AudioKind::Reed => *b"REED",
        })
    }

    /// Samples per encode block, as `tpt encode` uses.
    fn block(self, rate: u32) -> usize {
        match self {
            AudioKind::Cinder => 512,
            AudioKind::Aura => 1024,
            AudioKind::Reed => rate as usize / 50,
        }
    }
}

/// `seconds` of deterministic mono S16 audio: two tones under a slow
/// tremolo plus a little LCG noise, so every codec has something real to
/// code.
pub fn tone(rate: u32, seconds: f32) -> Vec<i16> {
    let n = (rate as f32 * seconds) as usize;
    let mut state = 0x1234_5678u32;
    (0..n)
        .map(|i| {
            let t = i as f32 / rate as f32;
            let env = 0.6 + 0.4 * (2.0 * core::f32::consts::PI * 2.0 * t).sin();
            let s = 0.5 * (2.0 * core::f32::consts::PI * 440.0 * t).sin()
                + 0.25 * (2.0 * core::f32::consts::PI * 1_320.0 * t).sin();
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = ((state >> 16) as f32 / 65_535.0 - 0.5) * 0.02;
            ((s * env + noise) * 0.6 * 32_767.0) as i16
        })
        .collect()
}

struct EncodedAudio {
    track: Track,
    /// `(pts in seconds, packet)`.
    packets: Vec<(f64, Packet)>,
}

fn encode_audio(kind: AudioKind, rate: u32, pcm: &[i16], track_id: u8) -> EncodedAudio {
    let block = kind.block(rate);
    let tb = Timebase::new(1, rate);
    let fw = FrameworkConfig::default();
    let bits = match kind {
        AudioKind::Reed => 5u8,
        _ => 8,
    };
    let mut enc: Box<dyn Encoder<Frame = AudioFrame>> = match kind {
        AudioKind::Cinder => Box::new(
            CinderEncoder::new(
                fw,
                CinderEncoderConfig {
                    sample_rate: rate,
                    bits_per_coef: bits,
                    ..CinderEncoderConfig::default()
                },
            )
            .expect("cinder encoder"),
        ),
        AudioKind::Aura => Box::new(
            AuraEncoder::new(
                fw,
                AuraConfig {
                    mode: AuraMode::Lossless,
                    sample_rate: rate,
                    ..AuraConfig::default()
                },
            )
            .expect("aura encoder"),
        ),
        AudioKind::Reed => Box::new(
            ReedEncoder::new(
                fw,
                ReedConfig {
                    sample_rate: rate,
                    lsf_bits: bits,
                    ..ReedConfig::default()
                },
            )
            .expect("reed encoder"),
        ),
    };
    let mut track = Track::new(track_id, MediaType::Audio, kind.id(), tb);
    track.codec_config.version = enc.info().version;
    track.codec_config.data = match kind {
        AudioKind::Cinder => vec![bits],
        AudioKind::Aura => (block as u16).to_le_bytes().to_vec(),
        AudioKind::Reed => ReedConfig {
            sample_rate: rate,
            lsf_bits: bits,
            ..ReedConfig::default()
        }
        .to_codec_private(),
    };
    track.media = TrackMedia {
        sample_rate: Some(rate),
        channel_layout: Some(ChannelLayout::Mono),
        ..TrackMedia::default()
    };
    track.metadata.push((
        SAMPLE_COUNT_KEY.to_string(),
        MetadataValue::Int(pcm.len() as i64),
    ));

    let mut packets = Vec::new();
    let mut start = 0usize;
    while start < pcm.len() {
        let end = (start + block).min(pcm.len());
        let mut bytes = vec![0u8; block * 2];
        for (i, s) in pcm[start..end].iter().enumerate() {
            bytes[i * 2..i * 2 + 2].copy_from_slice(&s.to_le_bytes());
        }
        let frame = AudioFrame {
            frame: Frame {
                pts: Timestamp {
                    ticks: start as i64,
                    timebase: tb,
                },
                duration: None,
                side_data: Vec::new(),
            },
            samples: bytes,
            sample_count: block as u32,
            format: SampleFormat::S16,
            channel_layout: ChannelLayout::Mono,
            sample_rate: rate,
        };
        for mut p in enc.encode(&frame).expect("audio encode") {
            p.track_id = track_id;
            packets.push((start as f64 / rate as f64, p));
        }
        start += block;
    }
    let tail = start as f64 / rate as f64;
    for mut p in enc.drain().expect("audio drain") {
        p.track_id = track_id;
        packets.push((tail, p));
    }
    EncodedAudio { track, packets }
}

/// One deterministic 4:2:0 test picture: a diagonal luma ramp, drifting
/// chroma, and a bright square that moves `4 px` per frame.
pub fn test_picture(width: u32, height: u32, index: u32, fps: u32) -> VideoFrame {
    let mut f = VideoFrame::alloc(width, height, PixelFormat::Yuv420);
    let (w, h) = (width as usize, height as usize);
    let sq = 16usize.min(w).min(h);
    let sx = (index as usize * 4) % (w - sq + 1);
    let sy = (h - sq) / 2;
    for y in 0..h {
        for x in 0..w {
            let ramp = ((x * 255 / w.max(1)) + (y * 255 / h.max(1))) / 2;
            let inside = x >= sx && x < sx + sq && y >= sy && y < sy + sq;
            f.pixels[y * w + x] = if inside { 235 } else { (ramp as u8) / 2 + 40 };
        }
    }
    let (cw, ch) = (w / 2, h / 2);
    let (u0, v0) = (w * h, w * h + cw * ch);
    for y in 0..ch {
        for x in 0..cw {
            f.pixels[u0 + y * cw + x] = (96 + x * 64 / cw.max(1) + index as usize % 32) as u8;
            f.pixels[v0 + y * cw + x] = (160 - y * 64 / ch.max(1)) as u8;
        }
    }
    f.frame.pts = Timestamp {
        ticks: i64::from(index),
        timebase: Timebase::new(1, fps),
    };
    f
}

struct EncodedVideo {
    track: Track,
    packets: Vec<(f64, Packet)>,
}

fn encode_video(width: u32, height: u32, fps: u32, frames: u32, track_id: u8) -> EncodedVideo {
    let cfg = ThrumConfig {
        width,
        height,
        format: PixelFormat::Yuv420,
        ..ThrumConfig::default()
    };
    let mut enc = ThrumEncoder::new(FrameworkConfig::default(), cfg.clone()).expect("thrum");
    let tb = Timebase::new(1, fps);
    let mut track = Track::new(track_id, MediaType::Video, CodecId::new(*b"THRM"), tb);
    track.codec_config.version = enc.info().version;
    track.codec_config.data = cfg.to_codec_private();
    track.media = TrackMedia {
        width: Some(width),
        height: Some(height),
        pixel_format: Some(PixelFormat::Yuv420),
        ..TrackMedia::default()
    };
    let mut packets = Vec::new();
    for i in 0..frames {
        let pic = test_picture(width, height, i, fps);
        for mut p in enc.encode(&pic).expect("video encode") {
            p.track_id = track_id;
            packets.push((f64::from(i) / f64::from(fps), p));
        }
    }
    EncodedVideo { track, packets }
}

fn mux(tracks: &[&Track], mut packets: Vec<(f64, Packet)>) -> Vec<u8> {
    let mut m = TptMuxer::new(Timebase::MILLISECONDS);
    for t in tracks {
        m.register_track(t);
    }
    // Stable sort: equal-time packets keep their per-track order.
    packets.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, p) in &packets {
        m.write_packet_raw(p).expect("registered track");
    }
    m.finish()
}

/// A finished audio-only `.tpt` of `seconds` of [`tone`].
pub fn audio_tpt(kind: AudioKind, rate: u32, seconds: f32) -> Vec<u8> {
    let pcm = tone(rate, seconds);
    let a = encode_audio(kind, rate, &pcm, 0);
    mux(&[&a.track], a.packets)
}

/// A finished video-only `.tpt` of `frames` [`test_picture`]s.
pub fn video_tpt(width: u32, height: u32, fps: u32, frames: u32) -> Vec<u8> {
    let v = encode_video(width, height, fps, frames, 0);
    mux(&[&v.track], v.packets)
}

/// A finished audio+video `.tpt` (audio track 0, video track 1), packets
/// interleaved in presentation order.
pub fn av_tpt(
    kind: AudioKind,
    rate: u32,
    seconds: f32,
    width: u32,
    height: u32,
    fps: u32,
) -> Vec<u8> {
    let pcm = tone(rate, seconds);
    let a = encode_audio(kind, rate, &pcm, 0);
    let frames = (seconds * fps as f32).round() as u32;
    let v = encode_video(width, height, fps, frames, 1);
    let mut all = a.packets;
    all.extend(v.packets);
    mux(&[&a.track, &v.track], all)
}

/// A named benchmark input with what it contains.
pub struct Fixture {
    /// Short file-name-safe identifier (also the `.tpt` file stem).
    pub id: &'static str,
    /// Human description for tables.
    pub name: &'static str,
    /// The finished `.tpt`.
    pub bytes: Vec<u8>,
    /// Duration of the media in seconds.
    pub media_seconds: f64,
    /// Video frames it contains (0 for audio-only).
    pub video_frames: u64,
}

/// The fixed set of inputs the native benchmark
/// (`benches/decode_throughput.rs`) and the wasm benchmark
/// (`web/bench.mjs`, fed by `make_demo --bench-dir`) both time, so the two
/// numbers are for byte-identical files. The last entry is byte-identical to
/// `web/demo.tpt`.
pub fn benchmark_fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            id: "cinder-30s",
            name: "cinder 48 kHz mono, 30 s",
            bytes: audio_tpt(AudioKind::Cinder, 48_000, 30.0),
            media_seconds: 30.0,
            video_frames: 0,
        },
        Fixture {
            id: "aura-10s",
            name: "aura lossless 48 kHz mono, 10 s",
            bytes: audio_tpt(AudioKind::Aura, 48_000, 10.0),
            media_seconds: 10.0,
            video_frames: 0,
        },
        Fixture {
            id: "reed-30s",
            name: "reed 16 kHz mono, 30 s",
            bytes: audio_tpt(AudioKind::Reed, 16_000, 30.0),
            media_seconds: 30.0,
            video_frames: 0,
        },
        Fixture {
            id: "thrum-qcif",
            name: "thrum 176x144 (QCIF) 15 fps, 60 frames",
            bytes: video_tpt(176, 144, 15, 60),
            media_seconds: 4.0,
            video_frames: 60,
        },
        Fixture {
            id: "thrum-640x352",
            name: "thrum 640x352 15 fps, 30 frames",
            bytes: video_tpt(640, 352, 15, 30),
            media_seconds: 2.0,
            video_frames: 30,
        },
        Fixture {
            id: "demo",
            name: "demo.tpt: cinder + thrum 160x96, 4 s",
            bytes: av_tpt(AudioKind::Cinder, 48_000, 4.0, 160, 96, 15),
            media_seconds: 4.0,
            video_frames: 60,
        },
    ]
}
