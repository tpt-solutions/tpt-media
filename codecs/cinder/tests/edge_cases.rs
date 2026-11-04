//! Edge-case sweep for Cinder's public surface: empty input, zero-length and
//! truncated packets, buffers that are too small, concealment before any
//! packet, and extreme parameters. Complements the unit tests in `src/lib.rs`
//! and the random sweeps in `property.rs`.

use tpt_audio_cinder::{CinderDecoder, CinderEncoder, CinderEncoderConfig};
use tpt_media_codec::{CodecConfig, CodecError, Decoder, Encoder, RealtimeDecoder};
use tpt_media_core::{
    AudioFrame, ChannelLayout, Frame, Packet, PacketFlags, SampleFormat, Timebase, Timestamp,
};

const RATE: u32 = 48_000;

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

fn packet(data: Vec<u8>) -> Packet {
    Packet {
        data,
        pts: Timestamp::zero(tb()),
        dts: Timestamp::zero(tb()),
        flags: PacketFlags::KEYFRAME,
        track_id: 0,
        duration: None,
    }
}

fn enc_cfg(bits: u8, frame_size: u16) -> CinderEncoderConfig {
    CinderEncoderConfig {
        sample_rate: RATE,
        bits_per_coef: bits,
        frame_size,
        ..CinderEncoderConfig::default()
    }
}

#[test]
fn empty_frame_produces_no_packets() {
    let mut enc = CinderEncoder::new(CodecConfig::default(), enc_cfg(8, 256)).unwrap();
    let out = enc
        .encode(&s16_frame(&[], 0))
        .expect("empty frame is valid input");
    assert!(out.is_empty(), "an empty frame must not emit a packet");
    assert!(enc.drain().expect("drain").is_empty(), "nothing to drain");
}

#[test]
fn empty_and_tiny_packets_are_malformed_not_panics() {
    let mut dec = CinderDecoder::new(CodecConfig::default(), enc_cfg(8, 256)).unwrap();
    // Zero bytes: no version field at all.
    assert_eq!(
        dec.decode(&packet(Vec::new())).unwrap_err(),
        CodecError::MalformedPacket
    );
    // One byte: the version field is truncated.
    assert_eq!(
        dec.decode(&packet(vec![0x00])).unwrap_err(),
        CodecError::MalformedPacket
    );
    // The version alone, with no range-coder payload.
    let mut out = vec![0u8; dec.max_output_bytes()];
    assert_eq!(
        dec.decode_into(&packet(vec![0x00]), &mut out).unwrap_err(),
        CodecError::MalformedPacket
    );
}

#[test]
fn output_buffers_too_small_are_refused_without_touching_state() {
    let mut enc = CinderEncoder::new(CodecConfig::default(), enc_cfg(8, 256)).unwrap();
    let tone: Vec<i16> = (0..4096)
        .map(|i| ((i as f32 * 0.05).sin() * 12_000.0) as i16)
        .collect();
    let mut packets = Vec::new();
    for (b, chunk) in tone.chunks(512).enumerate() {
        packets.extend(enc.encode(&s16_frame(chunk, (b * 512) as i64)).unwrap());
    }
    packets.extend(enc.drain().unwrap());

    let mut a = CinderDecoder::new(CodecConfig::default(), enc_cfg(8, 256)).unwrap();
    let mut b = CinderDecoder::new(CodecConfig::default(), enc_cfg(8, 256)).unwrap();
    let mut out = vec![0u8; a.max_output_bytes()];
    for p in &packets {
        a.decode_into(p, &mut out).unwrap();
        b.decode_into(p, &mut out).unwrap();
    }

    // A refused call must leave the stream exactly where it was: the next
    // concealed block from `a` matches the block from `b`, which was never
    // asked for the failed calls.
    let mut tiny = [0u8; 4];
    assert!(matches!(
        a.decode_into(&packets[0], &mut tiny),
        Err(CodecError::InvalidConfig(_))
    ));
    assert!(matches!(
        a.conceal_into(&mut tiny),
        Err(CodecError::InvalidConfig(_))
    ));
    let mut out_a = vec![0u8; a.max_output_bytes()];
    let mut out_b = vec![0u8; b.max_output_bytes()];
    let n_a = a.conceal_into(&mut out_a).unwrap();
    let n_b = b.conceal_into(&mut out_b).unwrap();
    assert_eq!(n_a, n_b);
    assert_eq!(
        &out_a[..n_a],
        &out_b[..n_b],
        "refused calls changed decoder state"
    );
}

#[test]
fn concealment_before_any_packet_is_silence_of_one_frame() {
    // With no history and no spectrum, the fallback has nothing to repeat.
    for frame_size in [128u16, 256] {
        let mut dec = CinderDecoder::new(CodecConfig::default(), enc_cfg(8, frame_size)).unwrap();
        let frame = dec
            .conceal(Timestamp::zero(tb()))
            .expect("conceal at stream start");
        assert_eq!(frame.sample_count as usize, frame_size as usize);
        assert!(
            frame.samples.iter().all(|&b| b == 0),
            "concealing before any packet must be silent (frame_size {frame_size})"
        );
    }
}

#[test]
fn full_scale_input_round_trips_at_extreme_bit_depths_and_steps() {
    // Square waves at full scale stress the escape path and the level clamp.
    let square: Vec<i16> = (0..2048)
        .map(|i| {
            if (i / 16) % 2 == 0 {
                i16::MAX
            } else {
                i16::MIN
            }
        })
        .collect();
    let cases = [
        (4u8, 63.999_f32),    // coarsest step the wire can carry, fewest bits
        (12u8, 1.0 / 1024.0), // finest step, widest alphabet
    ];
    for (bits, step) in cases {
        let cfg = CinderEncoderConfig {
            band_steps: vec![step; 16],
            ..enc_cfg(bits, 256)
        };
        let mut enc = CinderEncoder::new(CodecConfig::default(), cfg.clone()).unwrap();
        let mut packets = Vec::new();
        for (b, chunk) in square.chunks(512).enumerate() {
            packets.extend(enc.encode(&s16_frame(chunk, (b * 512) as i64)).unwrap());
        }
        packets.extend(enc.drain().unwrap());

        let mut dec = CinderDecoder::new(CodecConfig::default(), cfg).unwrap();
        let mut decoded_samples = 0usize;
        let mut energy = 0f64;
        for p in &packets {
            for f in dec.decode(p).expect("own packets must decode") {
                decoded_samples += f.sample_count as usize;
                for ch in f.samples.chunks(2) {
                    let v = i16::from_le_bytes([ch[0], ch[1]]) as f64;
                    energy += v * v;
                }
            }
        }
        for f in dec.flush().unwrap() {
            decoded_samples += f.sample_count as usize;
        }
        // One packet of latency is consumed by priming, then the stream is
        // emitted in full; the total must cover the input with no loss.
        assert!(
            decoded_samples >= square.len() - 256,
            "bits {bits}: decoded {decoded_samples} of {} samples",
            square.len()
        );
        assert!(
            energy > 0.0,
            "bits {bits}: full-scale input decoded to silence"
        );
    }
}
