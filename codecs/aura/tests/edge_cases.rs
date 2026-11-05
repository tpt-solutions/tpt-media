//! Edge-case sweep for Aura's public surface: empty input, zero-length packets,
//! extreme sample values at the LPC-order and block-size limits, and invalid
//! configuration. Complements the unit tests in `src/lib.rs` and the random
//! sweeps in `property.rs` and `stereo.rs`.

use tpt_media_aura::{AuraConfig, AuraDecoder, AuraEncoder, AuraMode};
use tpt_media_codec::{CodecConfig, CodecError, Decoder, Encoder};
use tpt_media_core::{AudioFrame, ChannelLayout, Frame, Packet, SampleFormat, Timebase, Timestamp};

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

fn lossless(lpc_order: u8, block_size: u16) -> AuraConfig {
    AuraConfig {
        mode: AuraMode::Lossless,
        sample_rate: RATE,
        lpc_order,
        block_size,
        ..AuraConfig::default()
    }
}

/// Encode `samples` in blocks of `block`, then drain, and decode everything.
fn round_trip(cfg: &AuraConfig, samples: &[i16], block: usize) -> Vec<i16> {
    let mut enc = AuraEncoder::new(CodecConfig::default(), cfg.clone()).unwrap();
    let mut dec = AuraDecoder::new(CodecConfig::default(), cfg.clone()).unwrap();
    let mut packets = Vec::new();
    for (b, chunk) in samples.chunks(block).enumerate() {
        packets.extend(enc.encode(&s16_frame(chunk, (b * block) as i64)).unwrap());
    }
    packets.extend(enc.drain().unwrap());
    let mut out = Vec::new();
    for p in &packets {
        for f in dec.decode(p).expect("own packets must decode") {
            for ch in f.samples.chunks(2) {
                out.push(i16::from_le_bytes([ch[0], ch[1]]));
            }
        }
    }
    out
}

#[test]
fn empty_frame_produces_no_packets() {
    let mut enc = AuraEncoder::new(CodecConfig::default(), lossless(16, 1024)).unwrap();
    let out = enc
        .encode(&s16_frame(&[], 0))
        .expect("empty frame is valid input");
    assert!(out.is_empty(), "an empty frame must not emit a packet");
    assert!(enc.drain().expect("drain").is_empty(), "nothing to drain");
}

#[test]
fn zero_length_packet_is_malformed_not_a_panic() {
    let mut dec = AuraDecoder::new(CodecConfig::default(), lossless(16, 1024)).unwrap();
    let pkt = Packet::empty(0, Timestamp::zero(tb()));
    assert!(dec.decode(&pkt).is_err(), "an empty packet must be refused");
}

#[test]
fn alternating_full_scale_is_bit_exact_at_the_order_and_block_limits() {
    // Alternating i16::MIN and i16::MAX forces the largest residuals the
    // lossless path can see. Check the LPC-order limits (0, 1 and 31) and a
    // block size that is not a power of two. Input is whole blocks only.
    for (order, block) in [(0u8, 1024u16), (1, 1024), (31, 1024), (16, 257)] {
        let samples: Vec<i16> = (0..block as usize * 12)
            .map(|i| if i % 2 == 0 { i16::MAX } else { i16::MIN })
            .collect();
        let cfg = lossless(order, block);
        let out = round_trip(&cfg, &samples, block as usize);
        assert_eq!(
            out, samples,
            "order {order}, block {block}: lossless round trip changed samples"
        );
    }
}

#[test]
fn frames_that_are_not_exactly_one_block_are_refused() {
    // Longer frames used to be truncated to their first block with no error.
    let cfg = lossless(16, 1024);
    let mut enc = AuraEncoder::new(CodecConfig::default(), cfg).unwrap();
    for len in [1usize, 512, 1023, 1025, 3000] {
        let samples = vec![1000i16; len];
        assert!(
            matches!(
                enc.encode(&s16_frame(&samples, 0)),
                Err(CodecError::InvalidConfig(_))
            ),
            "a {len}-sample frame must be refused, not partially coded"
        );
    }
    // And a whole block is still accepted.
    let ok = vec![1000i16; 1024];
    assert!(enc.encode(&s16_frame(&ok, 0)).is_ok());
}

#[test]
fn invalid_lpc_orders_are_rejected_at_construction() {
    // 32 is the trap: it fits the config type, not the 5-bit header field.
    for order in [32u8, 33, 255] {
        assert!(
            AuraEncoder::new(CodecConfig::default(), lossless(order, 1024)).is_err(),
            "lpc_order {order} must be refused"
        );
    }
}

#[test]
fn lossy_full_scale_square_decodes_to_exact_length_and_finite_samples() {
    // Lossy output is i16 by construction, so "finite" means it is nonzero
    // and not clipped to silence; length must still match the input.
    let cfg = AuraConfig {
        mode: AuraMode::Lossy,
        sample_rate: RATE,
        block_size: 2048,
        ..AuraConfig::default()
    };
    let samples: Vec<i16> = (0..8192)
        .map(|i| {
            if (i / 32) % 2 == 0 {
                i16::MAX
            } else {
                i16::MIN
            }
        })
        .collect();
    let out = round_trip(&cfg, &samples, 2048);
    assert_eq!(out.len(), samples.len(), "lossy decode changed the length");
    let energy: f64 = out.iter().map(|&v| (v as f64) * (v as f64)).sum();
    assert!(energy > 0.0, "full-scale lossy input decoded to silence");
}

#[test]
fn decoder_rejects_a_packet_truncated_at_every_length() {
    // Every prefix of a valid packet must fail cleanly: no panic, no success
    // on a partial body.
    let cfg = lossless(16, 1024);
    let mut enc = AuraEncoder::new(CodecConfig::default(), cfg.clone()).unwrap();
    let tone: Vec<i16> = (0..1024)
        .map(|i| ((i as f32 * 0.03).sin() * 9000.0) as i16)
        .collect();
    let mut packets = enc.encode(&s16_frame(&tone, 0)).unwrap();
    packets.extend(enc.drain().unwrap());
    let valid = packets.into_iter().next().expect("a packet");
    for len in 0..valid.data.len() {
        let mut dec = AuraDecoder::new(CodecConfig::default(), cfg.clone()).unwrap();
        let cut = Packet {
            data: valid.data[..len].to_vec(),
            ..valid.clone()
        };
        assert!(
            dec.decode(&cut).is_err(),
            "prefix of length {len} of {} decoded as if complete",
            valid.data.len()
        );
    }
    // The untruncated packet still decodes, so the loop tested real cuts.
    let mut dec = AuraDecoder::new(CodecConfig::default(), cfg).unwrap();
    assert!(dec.decode(&valid).is_ok());
}
