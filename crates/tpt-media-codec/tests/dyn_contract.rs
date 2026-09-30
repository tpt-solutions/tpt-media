//! The type-erased `Dyn*` contract, exercised through the public API only.
//!
//! The inline unit tests can reach private items, so they cannot prove that
//! the framework is usable from *outside* the crate — which is how both
//! `tpt-media-sys` (the C ABI) and `tpt-cli` consume it. Every codec crate
//! implements these traits, so a change that breaks the contract here
//! compiles fine and fails at the boundary.

extern crate alloc;

use tpt_media_codec::{
    Capabilities, CodecConfig, CodecError, CodecInfo, CodecResult, DynDecoder, DynDecoderFactory,
    DynEncoder, DynEncoderFactory, DynFrame, Registry, SupportedFormat,
};
use tpt_media_core::{
    ChannelLayout, Packet, PixelFormat, SampleFormat, Timebase, Timestamp, VideoFrame,
};

const CODEC_ID: [u8; 4] = *b"STUB";

fn ts(ticks: i64) -> Timestamp {
    Timestamp {
        ticks,
        timebase: Timebase::new(1, 1000),
    }
}

fn packet(ticks: i64, payload: &[u8]) -> Packet {
    let mut p = Packet::empty(0, ts(ticks));
    p.data.extend_from_slice(payload);
    p
}

fn info() -> CodecInfo {
    CodecInfo {
        name: "Stub",
        codec_id: CODEC_ID,
        version: 1,
        capabilities: Capabilities {
            real_time_decoder: true,
            lossy: true,
            supports_drain: true,
            ..Capabilities::default()
        },
        supported_formats: alloc::vec![SupportedFormat::Video {
            pixel_format: PixelFormat::Yuv420,
            width: 16,
            height: 16,
        }],
    }
}

/// A trivial "codec": each packet's last byte becomes one 16x16 grey frame.
struct StubDecoder;

impl DynDecoder for StubDecoder {
    fn decode(&mut self, packet: &Packet) -> CodecResult<Vec<DynFrame>> {
        let level = *packet.data.last().ok_or(CodecError::MalformedPacket)?;
        Ok(alloc::vec![frame(level)])
    }

    fn flush(&mut self) -> CodecResult<Vec<DynFrame>> {
        Ok(Vec::new())
    }

    fn drain(&mut self) -> CodecResult<Vec<DynFrame>> {
        // Graceful EOS: one trailing frame, distinct from a hard flush.
        Ok(alloc::vec![frame(0xEE)])
    }

    fn info(&self) -> CodecInfo {
        info()
    }
}

fn frame(level: u8) -> DynFrame {
    DynFrame::Video {
        pixel_format: PixelFormat::Gray8,
        width: 16,
        height: 16,
        stride: 16,
        pixels: alloc::vec![level; 16 * 16],
    }
}

struct StubEncoder;

impl DynEncoder for StubEncoder {
    fn encode(&mut self, frame: &DynFrame) -> CodecResult<Vec<Packet>> {
        let pixels = match frame {
            DynFrame::Video { pixels, .. } => pixels,
            DynFrame::Audio { .. } => return Err(CodecError::Unsupported("audio frame")),
        };
        Ok(alloc::vec![packet(0, &[*pixels.last().unwrap_or(&0)])])
    }

    fn flush(&mut self) -> CodecResult<Vec<Packet>> {
        Ok(Vec::new())
    }

    fn drain(&mut self) -> CodecResult<Vec<Packet>> {
        Ok(alloc::vec![packet(1, &[0xDD])])
    }

    fn info(&self) -> CodecInfo {
        info()
    }
}

struct StubFactory;

impl DynDecoderFactory for StubFactory {
    fn codec_id(&self) -> &[u8; 4] {
        &CODEC_ID
    }
    fn info(&self) -> CodecInfo {
        info()
    }
    fn build(&self, _cfg: &CodecConfig) -> CodecResult<Box<dyn DynDecoder>> {
        Ok(Box::new(StubDecoder))
    }
}

impl DynEncoderFactory for StubFactory {
    fn codec_id(&self) -> &[u8; 4] {
        &CODEC_ID
    }
    fn info(&self) -> CodecInfo {
        info()
    }
    fn build(&self, _cfg: &CodecConfig) -> CodecResult<Box<dyn DynEncoder>> {
        Ok(Box::new(StubEncoder))
    }
}

fn registry() -> Registry {
    let mut reg = Registry::new();
    reg.register_decoder(StubFactory);
    reg.register_encoder(StubFactory);
    reg
}

#[test]
fn a_built_decoder_round_trips_packets_to_frames() {
    let reg = registry();
    let factory = reg.find_decoder(&CODEC_ID).expect("registered");
    assert_eq!(factory.info().name, "Stub");
    let mut dec = factory.build(&CodecConfig::default()).expect("build");

    let frames = dec.decode(&packet(0, &[7])).expect("decode");
    assert_eq!(frames.len(), 1);
    match &frames[0] {
        DynFrame::Video {
            pixels,
            width,
            height,
            ..
        } => {
            assert_eq!((*width, *height), (16, 16));
            assert!(pixels.iter().all(|p| *p == 7));
        }
        other => panic!("expected a video frame, got {other:?}"),
    }

    // A second packet does not disturb the first decoder's output.
    let frames = dec.decode(&packet(1, &[9])).expect("decode");
    match &frames[0] {
        DynFrame::Video { pixels, .. } => assert!(pixels.iter().all(|p| *p == 9)),
        other => panic!("expected a video frame, got {other:?}"),
    }
}

#[test]
fn an_empty_packet_is_malformed_not_a_silent_frame() {
    // No payload means no level byte; the decoder must say so rather than
    // fabricate a frame from an unwrap.
    let reg = registry();
    let mut dec = reg
        .find_decoder(&CODEC_ID)
        .expect("registered")
        .build(&CodecConfig::default())
        .expect("build");
    assert_eq!(
        dec.decode(&packet(0, &[])).unwrap_err(),
        CodecError::MalformedPacket
    );
}

#[test]
fn drain_and_flush_are_distinguishable() {
    // `Capabilities::supports_drain` promises drain is a graceful EOS that
    // emits buffered output, distinct from a hard flush. A decoder that
    // returns the same thing for both makes that flag a lie.
    let reg = registry();
    let mut dec = reg
        .find_decoder(&CODEC_ID)
        .expect("registered")
        .build(&CodecConfig::default())
        .expect("build");
    let flushed = dec.flush().expect("flush");
    let drained = dec.drain().expect("drain");
    assert!(flushed.is_empty(), "flush should emit nothing here");
    assert_eq!(drained.len(), 1, "drain should emit the tail frame");
    assert!(dec.info().capabilities.supports_drain);
}

#[test]
fn an_encoder_can_reject_a_frame_it_does_not_support() {
    let reg = registry();
    let mut enc = reg
        .find_encoder(&CODEC_ID)
        .expect("registered")
        .build(&CodecConfig::default())
        .expect("build");

    let video = VideoFrame::alloc(16, 16, PixelFormat::Gray8);
    let ok = enc
        .encode(&DynFrame::Video {
            pixel_format: PixelFormat::Gray8,
            width: 16,
            height: 16,
            stride: video.stride,
            pixels: video.pixels,
        })
        .expect("encode a video frame");
    assert_eq!(ok.len(), 1);
    assert_eq!(ok[0].data, alloc::vec![0u8]);

    // Audio is outside this codec's declared support and must be refused
    // with `Unsupported`, not silently reinterpreted as video.
    let err = enc
        .encode(&DynFrame::Audio {
            sample_format: SampleFormat::S16,
            channel_layout: ChannelLayout::Mono,
            sample_rate: 48_000,
            sample_count: 16,
            samples: alloc::vec![0u8; 32],
        })
        .unwrap_err();
    assert!(matches!(err, CodecError::Unsupported(_)), "got {err:?}");
}

#[test]
fn a_built_codec_reports_the_info_its_factory_advertised() {
    // The factory's `info()` and the built object's `info()` are two paths
    // to the same claim, and callers use both.
    let reg = registry();
    let factory = reg.find_decoder(&CODEC_ID).expect("registered");
    let dec = factory.build(&CodecConfig::default()).expect("build");
    // `CodecInfo` deliberately derives only `Clone, Debug` (it holds a
    // `Vec<SupportedFormat>`), so the two claims are compared field by
    // field rather than with `assert_eq!`.
    let (a, b) = (dec.info(), factory.info());
    assert_eq!(a.name, b.name);
    assert_eq!(a.codec_id, b.codec_id);
    assert_eq!(a.version, b.version);
    assert_eq!(a.supported_formats, b.supported_formats);
    assert_eq!(a.capabilities, b.capabilities);

    let factory = reg.find_encoder(&CODEC_ID).expect("registered");
    let enc = factory.build(&CodecConfig::default()).expect("build");
    let (a, b) = (enc.info(), factory.info());
    assert_eq!(a.name, b.name);
    assert_eq!(a.supported_formats, b.supported_formats);
    // And the advertised format list survives the round trip through the
    // registry.
    assert!(enc.info().supports_format(&SupportedFormat::Video {
        pixel_format: PixelFormat::Yuv420,
        width: 16,
        height: 16,
    }));
}
