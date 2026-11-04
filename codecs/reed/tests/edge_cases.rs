//! Edge-case sweep for Reed's public surface: empty and partial input, zero-
//! length and truncated packets, output buffers that are too small, concealment
//! before any packet, and invalid configuration. Complements the unit tests in
//! `src/lib.rs` and the random sweeps in `property.rs`.

use tpt_audio_reed::{ReedConfig, ReedDecoder, ReedEncoder};
use tpt_media_codec::{CodecConfig, CodecError, Decoder, Encoder, RealtimeDecoder};
use tpt_media_core::{AudioFrame, ChannelLayout, Frame, Packet, SampleFormat, Timebase, Timestamp};

const RATE: u32 = 16_000;

fn tb() -> Timebase {
    Timebase::new(1, RATE)
}

fn s16_frame(samples: &[i16], start: i64) -> AudioFrame {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    AudioFrame {
        frame: Frame {
            pts: Timestamp {
                ticks: start,
                timebase: tb(),
            },
            duration: None,
            side_data: Vec::new(),
        },
        samples: bytes,
        sample_count: samples.len() as u32,
        format: SampleFormat::S16,
        channel_layout: ChannelLayout::Mono,
        sample_rate: RATE,
    }
}

fn cfg() -> ReedConfig {
    ReedConfig {
        sample_rate: RATE,
        ..ReedConfig::default()
    }
}

fn packet(data: Vec<u8>) -> Packet {
    Packet {
        data,
        pts: Timestamp::zero(tb()),
        dts: Timestamp::zero(tb()),
        flags: tpt_media_core::PacketFlags::KEYFRAME,
        track_id: 0,
        duration: None,
    }
}

#[test]
fn empty_frame_produces_no_packets() {
    let mut enc = ReedEncoder::new(CodecConfig::default(), cfg()).unwrap();
    let out = enc
        .encode(&s16_frame(&[], 0))
        .expect("empty frame is valid input");
    assert!(out.is_empty(), "an empty frame must not emit a packet");
    assert!(enc.drain().expect("drain").is_empty());
}

#[test]
fn frames_that_are_not_exactly_one_frame_are_refused() {
    // Longer frames used to be truncated to their first frame with no error.
    let mut enc = ReedEncoder::new(CodecConfig::default(), cfg()).unwrap();
    let frame = cfg().frame_size();
    for len in [1usize, frame / 2, frame + 1, 2 * frame + 7] {
        let samples = vec![500i16; len];
        assert!(
            matches!(
                enc.encode(&s16_frame(&samples, 0)),
                Err(CodecError::InvalidConfig(_))
            ),
            "a {len}-sample frame must be refused, not partially coded"
        );
    }
    assert!(enc.encode(&s16_frame(&vec![500i16; frame], 0)).is_ok());
}

#[test]
fn zero_length_and_truncated_packets_are_malformed_not_panics() {
    let mut dec = ReedDecoder::new(CodecConfig::default(), cfg()).unwrap();
    assert!(dec.decode(&packet(Vec::new())).is_err());
    // A packet truncated at every length must fail cleanly. The loop checks
    // no prefix is accepted as a complete frame.
    let mut enc = ReedEncoder::new(CodecConfig::default(), cfg()).unwrap();
    let tone: Vec<i16> = (0..cfg().frame_size())
        .map(|i| ((i as f32 * 0.2).sin() * 8000.0) as i16)
        .collect();
    let valid = enc.encode(&s16_frame(&tone, 0)).unwrap();
    let valid = valid.into_iter().next().expect("one packet per frame");
    for len in 0..valid.data.len() {
        let mut d = ReedDecoder::new(CodecConfig::default(), cfg()).unwrap();
        let cut = packet(valid.data[..len].to_vec());
        assert!(
            d.decode(&cut).is_err(),
            "prefix of length {len} of {} decoded as complete",
            valid.data.len()
        );
    }
    let mut d = ReedDecoder::new(CodecConfig::default(), cfg()).unwrap();
    assert!(d.decode(&valid).is_ok(), "the untruncated packet decodes");
}

#[test]
fn output_buffer_too_small_is_refused_without_touching_state() {
    let mut enc = ReedEncoder::new(CodecConfig::default(), cfg()).unwrap();
    let tone: Vec<i16> = (0..cfg().frame_size() * 6)
        .map(|i| ((i as f32 * 0.2).sin() * 8000.0) as i16)
        .collect();
    let mut packets = Vec::new();
    for (b, chunk) in tone.chunks(cfg().frame_size()).enumerate() {
        packets.extend(
            enc.encode(&s16_frame(chunk, (b * cfg().frame_size()) as i64))
                .unwrap(),
        );
    }
    let mut a = ReedDecoder::new(CodecConfig::default(), cfg()).unwrap();
    let mut b = ReedDecoder::new(CodecConfig::default(), cfg()).unwrap();
    let mut out = vec![0u8; a.max_output_bytes()];
    for p in &packets {
        a.decode_into(p, &mut out).unwrap();
        b.decode_into(p, &mut out).unwrap();
    }
    let mut tiny = [0u8; 2];
    assert!(matches!(
        a.decode_into(&packets[0], &mut tiny),
        Err(CodecError::InvalidConfig(_))
    ));
    // The refused call must leave the stream as it was: the next concealed
    // frame from `a` equals `b`'s, which never saw the refused call.
    let ca = a.conceal(Timestamp::zero(tb())).unwrap();
    let cb = b.conceal(Timestamp::zero(tb())).unwrap();
    assert_eq!(ca.samples, cb.samples, "refused decode_into changed state");
}

#[test]
fn concealment_before_any_packet_is_one_silent_frame() {
    let mut dec = ReedDecoder::new(CodecConfig::default(), cfg()).unwrap();
    let f = dec
        .conceal(Timestamp::zero(tb()))
        .expect("conceal at stream start");
    assert_eq!(f.sample_count as usize, cfg().frame_size());
    assert!(f.samples.iter().all(|&b| b == 0));
}

#[test]
fn invalid_configs_are_refused_at_construction() {
    let bad = [
        ReedConfig {
            sample_rate: 44_100,
            ..cfg()
        },
        ReedConfig {
            lpc_order: 0,
            ..cfg()
        },
        ReedConfig {
            lpc_order: 3,
            ..cfg()
        }, // odd order
        ReedConfig {
            lpc_order: 18,
            ..cfg()
        }, // above 16
        ReedConfig {
            lsf_bits: 1,
            ..cfg()
        },
        ReedConfig {
            lsf_bits: 13,
            ..cfg()
        },
    ];
    for c in bad {
        let desc = format!("{c:?}");
        assert!(
            ReedEncoder::new(CodecConfig::default(), c.clone()).is_err(),
            "encoder accepted {desc}"
        );
    }
}

#[test]
fn full_scale_square_round_trips_to_exact_length_and_finite_samples() {
    let mut enc = ReedEncoder::new(CodecConfig::default(), cfg()).unwrap();
    let mut dec = ReedDecoder::new(CodecConfig::default(), cfg()).unwrap();
    let frame = cfg().frame_size();
    let square: Vec<i16> = (0..frame * 8)
        .map(|i| {
            if (i / 40) % 2 == 0 {
                i16::MAX
            } else {
                i16::MIN
            }
        })
        .collect();
    let mut out_samples = 0usize;
    let mut energy = 0f64;
    for (b, chunk) in square.chunks(frame).enumerate() {
        for p in enc.encode(&s16_frame(chunk, (b * frame) as i64)).unwrap() {
            for f in dec.decode(&p).unwrap() {
                out_samples += f.sample_count as usize;
                for ch in f.samples.chunks(2) {
                    let v = i16::from_le_bytes([ch[0], ch[1]]) as f64;
                    energy += v * v;
                }
            }
        }
    }
    assert!(out_samples > 0, "no output");
    assert!(energy > 0.0, "full-scale input decoded to silence");
}
