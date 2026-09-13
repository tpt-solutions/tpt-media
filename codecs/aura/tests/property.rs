//! §38/§39: seeded-random round-trip properties and determinism pins for
//! Aura. The headline property is the lossless one: *any* S16 payload —
//! including full-range random noise, which is the hardest case — must
//! round-trip bit-exactly. Determinism is pinned at the packet-byte level.

use tpt_audio_aura::{AuraConfig, AuraDecoder, AuraEncoder};
use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{AudioFrame, ChannelLayout, Frame, Packet, SampleFormat, Timebase, Timestamp};

const RATE: u32 = 48_000;
const BLOCK: usize = 1024;

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

fn encode_blocks(enc: &mut AuraEncoder, blocks: &[Vec<u8>]) -> Vec<Vec<u8>> {
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

fn decode_packets(dec: &mut AuraDecoder, packets: &[Vec<u8>]) -> Vec<u8> {
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

fn aura_pair() -> (AuraEncoder, AuraDecoder) {
    let cfg = AuraConfig {
        sample_rate: RATE,
        ..AuraConfig::default()
    };
    let enc = AuraEncoder::new(CodecConfig::default(), cfg.clone()).unwrap();
    let dec = AuraDecoder::new(CodecConfig::default(), cfg).unwrap();
    (enc, dec)
}

#[test]
fn lossless_round_trips_full_range_random_audio() {
    // Full-range S16 noise is the worst possible input for a lossless
    // codec: maximal residual entropy, no structure to exploit. If *this*
    // is bit-exact, every easier signal is too.
    for seed in 1..=8u64 {
        let mut state = 0x5EED_0400 + seed;
        let blocks: Vec<Vec<u8>> = (0..4)
            .map(|_| {
                (0..BLOCK * 2)
                    .map(|_| lcg(&mut state) as u8)
                    .collect::<Vec<u8>>()
            })
            .collect();
        let (mut enc, mut dec) = aura_pair();
        let packets = encode_blocks(&mut enc, &blocks);
        let mut decoded = decode_packets(&mut dec, &packets);
        let original_len: usize = blocks.iter().map(|b| b.len()).sum();
        decoded.truncate(original_len);
        assert_eq!(decoded.len(), original_len, "seed {seed}: length survives");
        let mut original = Vec::with_capacity(original_len);
        for b in &blocks {
            original.extend_from_slice(b);
        }
        assert_eq!(decoded, original, "seed {seed}: lossless must be bit-exact");
    }
}

#[test]
fn encoding_and_decoding_are_deterministic() {
    let mut state = 0x5EED_0401u64;
    let blocks: Vec<Vec<u8>> = (0..4)
        .map(|_| {
            (0..BLOCK * 2)
                .map(|_| lcg(&mut state) as u8)
                .collect::<Vec<u8>>()
        })
        .collect();
    let (mut enc_a, mut dec_a) = aura_pair();
    let (mut enc_b, mut dec_b) = aura_pair();
    let packets_a = encode_blocks(&mut enc_a, &blocks);
    let packets_b = encode_blocks(&mut enc_b, &blocks);
    assert_eq!(
        packets_a, packets_b,
        "§39: identical input, identical packets"
    );
    assert_eq!(
        decode_packets(&mut dec_a, &packets_a),
        decode_packets(&mut dec_b, &packets_b)
    );
}

#[test]
fn silence_stays_silent() {
    let blocks: Vec<Vec<u8>> = (0..3).map(|_| vec![0u8; BLOCK * 2]).collect();
    let (mut enc, mut dec) = aura_pair();
    let packets = encode_blocks(&mut enc, &blocks);
    let decoded = decode_packets(&mut dec, &packets);
    assert!(decoded.iter().all(|b| *b == 0));
}
