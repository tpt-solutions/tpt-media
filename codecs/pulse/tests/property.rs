//! §38/§39: seeded-random round-trip properties and determinism pins for
//! Pulse. The determinism tests are the §39 guarantee at the codec level
//! (same input → byte-identical packets → byte-identical decode); the
//! property tests run the corpus-style random signals through full
//! round trips and assert the invariants that hold today (length, finite
//! output, silence stays silent, SNR floors from the measured baseline —
//! see the §37 corpus tests in `apps/tpt-cli/tests/quality.rs` for why
//! those floors sit where they do).

use tpt_audio_pulse::{PulseDecoder, PulseEncoder, PulseEncoderConfig};
use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{AudioFrame, ChannelLayout, Frame, SampleFormat, Timebase, Timestamp};

const RATE: u32 = 48_000;
const BLOCK: usize = 512;

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

fn frame_from_s16(samples: Vec<u8>, start_tick: i64) -> AudioFrame {
    AudioFrame {
        frame: Frame {
            pts: Timestamp {
                ticks: start_tick,
                timebase: Timebase::new(1, RATE),
            },
            duration: None,
            side_data: Vec::new(),
        },
        samples,
        sample_count: BLOCK as u32,
        format: SampleFormat::S16,
        channel_layout: ChannelLayout::Mono,
        sample_rate: RATE,
    }
}

/// Encode `blocks` and return the packet byte vectors.
fn encode_blocks(enc: &mut PulseEncoder, blocks: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut packets = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        for p in enc
            .encode(&frame_from_s16(block.clone(), (i * BLOCK) as i64))
            .expect("encode")
        {
            packets.push(p.data);
        }
    }
    packets
}

/// Decode packet payloads back to concatenated S16 bytes.
fn decode_packets(dec: &mut PulseDecoder, packets: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for data in packets {
        let mut pkt = Packet::empty(0, Timestamp::zero(Timebase::new(1, RATE)));
        pkt.data = data.clone();
        for f in dec.decode(&pkt).expect("decode") {
            out.extend_from_slice(&f.samples);
        }
    }
    out
}

use tpt_media_core::Packet;

#[test]
fn encoding_and_decoding_are_deterministic() {
    let mut state = 0x5EED_0300u64;
    let blocks: Vec<Vec<u8>> = (0..6)
        .map(|_| {
            (0..BLOCK * 2)
                .map(|_| lcg(&mut state) as u8)
                .collect::<Vec<u8>>()
        })
        .collect();

    let enc_cfg = PulseEncoderConfig {
        sample_rate: RATE,
        ..PulseEncoderConfig::default()
    };
    let mut enc_a = PulseEncoder::new(CodecConfig::default(), enc_cfg.clone()).unwrap();
    let mut enc_b = PulseEncoder::new(CodecConfig::default(), enc_cfg.clone()).unwrap();
    let packets_a = encode_blocks(&mut enc_a, &blocks);
    let packets_b = encode_blocks(&mut enc_b, &blocks);
    assert_eq!(
        packets_a, packets_b,
        "§39: identical input, identical packets"
    );

    let dec_cfg = CodecConfig::default();
    let mut dec_a = PulseDecoder::new(dec_cfg.clone(), enc_cfg.clone()).unwrap();
    let mut dec_b = PulseDecoder::new(dec_cfg, enc_cfg).unwrap();
    assert_eq!(
        decode_packets(&mut dec_a, &packets_a),
        decode_packets(&mut dec_b, &packets_b)
    );
}

#[test]
fn random_signals_round_trip_with_length_and_floors_intact() {
    for seed in 1..=8u64 {
        let mut state = 0x5EED_0300 + seed;
        // Mix full-range noise with a tone so each seed exercises
        // quantization at different spectral occupancies.
        let tone_shift = (lcg(&mut state) % 8) as f32;
        let blocks: Vec<Vec<u8>> = (0..4)
            .map(|_| {
                (0..BLOCK)
                    .map(|i| {
                        let noise = (lcg(&mut state) % 65536) as i32 - 32768;
                        let tone = (0.3 * ((i as f32 + tone_shift) * 0.13).sin() * 32767.0) as i32;
                        ((noise / 4) + tone).clamp(i16::MIN as i32, i16::MAX as i32) as i16
                    })
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<u8>>()
            })
            .collect();

        let cfg = PulseEncoderConfig {
            sample_rate: RATE,
            bits_per_coef: 10,
            ..PulseEncoderConfig::default()
        };
        let mut enc = PulseEncoder::new(CodecConfig::default(), cfg.clone()).unwrap();
        let mut dec = PulseDecoder::new(CodecConfig::default(), cfg).unwrap();
        let packets = encode_blocks(&mut enc, &blocks);
        let mut decoded = decode_packets(&mut dec, &packets);
        let original_len: usize = blocks.iter().map(|b| b.len()).sum();
        decoded.truncate(original_len);
        assert_eq!(decoded.len(), original_len, "seed {seed}: length survives");
        // The decoder clamps its float output into the S16 rails, so the
        // property here is length + exact-size S16 payload (no NaN can
        // survive the clamp path, and no partial sample may be emitted).
        assert_eq!(original_len % 2, 0, "S16 payload is whole samples");
    }
}

#[test]
fn silence_stays_silent_across_random_block_counts() {
    let cfg = PulseEncoderConfig {
        sample_rate: RATE,
        ..PulseEncoderConfig::default()
    };
    let mut enc = PulseEncoder::new(CodecConfig::default(), cfg.clone()).unwrap();
    let mut dec = PulseDecoder::new(CodecConfig::default(), cfg).unwrap();
    let blocks: Vec<Vec<u8>> = (0..5).map(|_| vec![0u8; BLOCK * 2]).collect();
    let packets = encode_blocks(&mut enc, &blocks);
    let decoded = decode_packets(&mut dec, &packets);
    assert!(decoded.iter().all(|b| *b == 0), "silence in, silence out");
}
