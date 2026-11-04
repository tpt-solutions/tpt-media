//! Edge-case sweep for Mote's public surface: wrong geometry and format,
//! short pixel buffers, empty and truncated packets, invalid configuration,
//! and extreme pixel content. Complements the unit tests and the random
//! sweeps in `property.rs`.

use tpt_media_codec::{CodecConfig, CodecError, Decoder, Encoder};
use tpt_media_core::{Packet, PacketFlags, PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_mote::{MoteConfig, MoteDecoder, MoteEncoder};

const W: u32 = 32;
const H: u32 = 32;

fn cfg(format: PixelFormat) -> MoteConfig {
    MoteConfig {
        width: W,
        height: H,
        format,
        dictionary_entries: 16,
        train_iterations: 2,
        ..MoteConfig::default()
    }
}

fn frame(w: u32, h: u32, format: PixelFormat, fill: impl Fn(usize) -> u8) -> VideoFrame {
    let mut f = VideoFrame::alloc(w, h, format);
    for (i, p) in f.pixels.iter_mut().enumerate() {
        *p = fill(i);
    }
    f
}

fn packet(data: Vec<u8>) -> Packet {
    Packet {
        data,
        pts: Timestamp::zero(Timebase::new(1, 30)),
        dts: Timestamp::zero(Timebase::new(1, 30)),
        flags: PacketFlags::KEYFRAME,
        track_id: 0,
        duration: None,
    }
}

#[test]
fn wrong_geometry_or_format_is_refused() {
    let mut enc = MoteEncoder::new(CodecConfig::default(), cfg(PixelFormat::Gray8)).unwrap();
    let wrong_w = frame(W + 16, H, PixelFormat::Gray8, |_| 128);
    assert!(matches!(
        enc.encode(&wrong_w),
        Err(CodecError::InvalidConfig(_))
    ));
    let wrong_h = frame(W, H + 16, PixelFormat::Gray8, |_| 128);
    assert!(matches!(
        enc.encode(&wrong_h),
        Err(CodecError::InvalidConfig(_))
    ));
    let wrong_fmt = frame(W, H, PixelFormat::Yuv420, |_| 128);
    assert!(matches!(
        enc.encode(&wrong_fmt),
        Err(CodecError::Unsupported(_))
    ));
}

#[test]
fn short_pixel_buffer_is_refused_not_a_panic() {
    let mut enc = MoteEncoder::new(CodecConfig::default(), cfg(PixelFormat::Gray8)).unwrap();
    let mut short = frame(W, H, PixelFormat::Gray8, |_| 128);
    short.pixels.truncate(short.pixels.len() - 1);
    assert!(
        enc.encode(&short).is_err(),
        "a short buffer must be refused"
    );
    let mut empty = frame(W, H, PixelFormat::Gray8, |_| 128);
    empty.pixels.clear();
    assert!(
        enc.encode(&empty).is_err(),
        "an empty buffer must be refused"
    );
}

#[test]
fn zero_length_and_truncated_packets_fail_cleanly() {
    let mut enc = MoteEncoder::new(CodecConfig::default(), cfg(PixelFormat::Gray8)).unwrap();
    let f = frame(W, H, PixelFormat::Gray8, |i| (i * 3) as u8);
    let mut packets = enc.encode(&f).unwrap();
    packets.extend(enc.drain().unwrap());
    assert!(!packets.is_empty(), "a frame must produce packets");
    let last = packets.pop().expect("frame packet");
    // Prime a decoder with everything before the frame packet (the dictionary,
    // when one is sent separately), then test the frame packet's prefixes.
    let prime = |d: &mut MoteDecoder| {
        for p in &packets {
            d.decode(p).expect("priming packets decode");
        }
    };
    let mut dec = MoteDecoder::new(CodecConfig::default(), cfg(PixelFormat::Gray8)).unwrap();
    prime(&mut dec);
    assert!(
        dec.decode(&packet(Vec::new())).is_err(),
        "empty packet must fail"
    );
    for len in 0..last.data.len() {
        let mut d = MoteDecoder::new(CodecConfig::default(), cfg(PixelFormat::Gray8)).unwrap();
        prime(&mut d);
        assert!(
            d.decode(&packet(last.data[..len].to_vec())).is_err(),
            "prefix of length {len} of {} decoded as complete",
            last.data.len()
        );
    }
    let mut d = MoteDecoder::new(CodecConfig::default(), cfg(PixelFormat::Gray8)).unwrap();
    prime(&mut d);
    assert!(d.decode(&last).is_ok(), "the intact packet decodes");
}

#[test]
fn invalid_configs_are_refused_at_construction() {
    let base = cfg(PixelFormat::Gray8);
    let bad = [
        MoteConfig {
            width: 0,
            ..base.clone()
        },
        MoteConfig {
            height: 0,
            ..base.clone()
        },
        MoteConfig {
            width: 17,
            ..base.clone()
        }, // not a multiple of 16
        MoteConfig {
            dictionary_entries: 0,
            ..base.clone()
        },
        MoteConfig {
            dictionary_entries: 4097,
            ..base.clone()
        },
        MoteConfig {
            format: PixelFormat::Rgb24,
            ..base.clone()
        },
    ];
    for c in bad {
        let desc = format!("{c:?}");
        assert!(
            MoteEncoder::new(CodecConfig::default(), c).is_err(),
            "encoder accepted {desc}"
        );
    }
}

#[test]
fn extreme_pixel_content_keeps_exact_geometry() {
    for format in [PixelFormat::Gray8, PixelFormat::Yuv420] {
        for fill in [0u8, 255] {
            let mut enc = MoteEncoder::new(CodecConfig::default(), cfg(format)).unwrap();
            let mut dec = MoteDecoder::new(CodecConfig::default(), cfg(format)).unwrap();
            let f = frame(W, H, format, |_| fill);
            let mut packets = enc.encode(&f).unwrap();
            packets.extend(enc.drain().unwrap());
            let mut out = Vec::new();
            for p in &packets {
                out.extend(dec.decode(p).unwrap());
            }
            assert_eq!(out.len(), 1, "one frame in, one frame out");
            assert_eq!((out[0].width, out[0].height, out[0].format), (W, H, format));
            assert_eq!(
                out[0].pixels.len(),
                VideoFrame::alloc(W, H, format).pixels.len()
            );
        }
    }
}

#[test]
fn overflowing_geometry_is_refused_not_a_panic() {
    let huge = 1u32 << 30;
    let c = MoteConfig {
        width: huge,
        height: huge,
        format: PixelFormat::Gray8,
        ..MoteConfig::default()
    };
    let refused = match MoteEncoder::new(CodecConfig::default(), c.clone()) {
        Err(_) => true,
        Ok(mut e) => e
            .encode(&VideoFrame::alloc(huge, huge, PixelFormat::Gray8))
            .is_err(),
    };
    assert!(refused, "a 2^30 x 2^30 frame must be refused");
}
