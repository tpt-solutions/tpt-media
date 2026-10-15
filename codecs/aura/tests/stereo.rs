//! Stereo lossless round trips for Aura. The encoder picks one of FLAC's four
//! stereo modes per block (L/R, L/S, R/S, M/S), so each test shape is chosen
//! to make a particular mode the cheapest, and every case must come back
//! bit-exact in the original channel order.

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

fn random_i16(state: &mut u64) -> i16 {
    lcg(state) as u16 as i16
}

fn stereo_frame(samples: Vec<u8>, start_tick: i64) -> AudioFrame {
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
        channel_layout: ChannelLayout::Stereo,
        sample_rate: RATE,
    }
}

/// Encode and decode interleaved L/R samples (a whole number of blocks).
fn round_trip(interleaved: &[i16]) -> Vec<i16> {
    let cfg = AuraConfig {
        sample_rate: RATE,
        ..AuraConfig::default()
    };
    let mut enc = AuraEncoder::new(CodecConfig::default(), cfg.clone()).unwrap();
    let mut dec = AuraDecoder::new(CodecConfig::default(), cfg).unwrap();
    let bytes: Vec<u8> = interleaved.iter().flat_map(|s| s.to_le_bytes()).collect();
    let block_bytes = BLOCK * 2 * 2;
    assert_eq!(bytes.len() % block_bytes, 0, "test input must be whole blocks");
    let mut out = Vec::new();
    for (i, chunk) in bytes.chunks(block_bytes).enumerate() {
        for p in enc
            .encode(&stereo_frame(chunk.to_vec(), (i * BLOCK) as i64))
            .expect("encode")
        {
            let mut pkt = Packet::empty(0, Timestamp::zero(Timebase::new(1, RATE)));
            pkt.data = p.data;
            for f in dec.decode(&pkt).expect("decode") {
                assert_eq!(f.channel_layout, ChannelLayout::Stereo, "output stays stereo");
                out.extend_from_slice(&f.samples);
            }
        }
    }
    out.chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect()
}

fn interleave(left: &[i16], right: &[i16]) -> Vec<i16> {
    left.iter().zip(right).flat_map(|(&l, &r)| [l, r]).collect()
}

const FRAMES: usize = BLOCK * 4;

#[test]
fn full_range_random_stereo_is_bit_exact() {
    for seed in 1..=4u64 {
        let mut state = 0x5EED_1000 + seed;
        let input: Vec<i16> = (0..FRAMES * 2).map(|_| random_i16(&mut state)).collect();
        assert_eq!(round_trip(&input), input, "seed {seed}");
    }
}

#[test]
fn correlated_channels_use_mid_side_and_stay_exact() {
    // R is L plus a little noise: the side channel is small, so M/S is chosen.
    let mut state = 0x5EED_2000;
    let mut left = Vec::with_capacity(FRAMES);
    let mut right = Vec::with_capacity(FRAMES);
    for i in 0..FRAMES {
        let tone = ((i as f64 * 0.031).sin() * 12_000.0) as i16;
        let l = tone.wrapping_add((lcg(&mut state) % 64) as i16 - 32);
        let r = l.wrapping_add((lcg(&mut state) % 5) as i16 - 2);
        left.push(l);
        right.push(r);
    }
    let input = interleave(&left, &right);
    assert_eq!(round_trip(&input), input);
}

#[test]
fn silent_and_identical_channels_are_exact() {
    let mut state = 0x5EED_3000;
    let noise: Vec<i16> = (0..FRAMES).map(|_| random_i16(&mut state)).collect();
    let silence = vec![0i16; FRAMES];
    // Left silent, right loud: the side channel is the negated right channel.
    let a = interleave(&silence, &noise);
    assert_eq!(round_trip(&a), a, "left silent");
    // Right silent, left loud.
    let b = interleave(&noise, &silence);
    assert_eq!(round_trip(&b), b, "right silent");
    // Identical channels: the side channel is all zero.
    let c = interleave(&noise, &noise);
    assert_eq!(round_trip(&c), c, "identical channels");
}

#[test]
fn extreme_opposite_channels_need_the_side_bits() {
    // L - R reaches 65535, which is outside i16: the side channel must not be
    // clamped on the way back.
    let left: Vec<i16> = (0..FRAMES)
        .map(|i| if i % 2 == 0 { i16::MIN } else { i16::MAX })
        .collect();
    let right: Vec<i16> = (0..FRAMES)
        .map(|i| if i % 2 == 0 { i16::MAX } else { i16::MIN })
        .collect();
    let input = interleave(&left, &right);
    assert_eq!(round_trip(&input), input);
}
