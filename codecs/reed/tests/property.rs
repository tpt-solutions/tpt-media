//! §38/§39: seeded-random round-trip properties and determinism pins for
//! Reed. Reed is *lossy by design* (a ~4.5 kbps vocoder), so the properties
//! pinned here are the ones its contract actually claims: full-length,
//! finite, non-silent decodes of arbitrary non-silent input; silence
//! round-trips to exact silence; and encoding/decoding are pure functions
//! of (bytes, config) — no RNG, wall-clock, or thread state.

use tpt_media_reed::{ReedConfig, ReedDecoder, ReedEncoder};
use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{AudioFrame, ChannelLayout, Frame, Packet, SampleFormat, Timebase, Timestamp};

const RATE: u32 = 16_000;
const BLOCK: usize = 320;

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

fn random_block(state: &mut u64, amplitude: i16) -> Vec<u8> {
    (0..BLOCK)
        .flat_map(|_| {
            let v = ((lcg(state) >> 33) as i32 % (2 * amplitude as i32)) - amplitude as i32;
            (v as i16).to_le_bytes()
        })
        .collect()
}

fn rms_of(frame: &AudioFrame) -> f64 {
    let n = frame.sample_count as usize;
    let sum: f64 = (0..n)
        .map(|i| {
            let v = frame.s16(0, i).unwrap() as f64;
            v * v
        })
        .sum();
    (sum / n as f64).sqrt()
}

#[test]
fn random_audio_round_trips_full_length_finite_and_audible() {
    let fw = CodecConfig::default();
    let mut enc = ReedEncoder::new(fw.clone(), ReedConfig::default()).unwrap();
    let mut dec = ReedDecoder::new(fw, ReedConfig::default()).unwrap();
    let mut state = 0xA5A5_5A5A_0000_0001u64;

    for i in 0..8i64 {
        let block = random_block(&mut state, i16::MAX);
        let frame = frame_from_s16(block, i * BLOCK as i64);
        let in_rms = rms_of(&frame);
        for pkt in enc.encode(&frame).unwrap() {
            let out = dec.decode(&pkt).unwrap();
            assert_eq!(out.len(), 1);
            assert_eq!(
                out[0].sample_count, BLOCK as u32,
                "length must be preserved"
            );
            let samples: Vec<i64> = (0..BLOCK)
                .map(|s| out[0].s16(0, s).unwrap() as i64)
                .collect();
            assert!(
                samples.iter().any(|&v| v != 0),
                "decoded full-scale noise must not come back silent"
            );
            let out_rms = rms_of(&out[0]);
            // Full-scale random noise through a vocoder: the envelope is
            // flattened to the quantized LPC fit and the gain is
            // dB-quantized, but the decode must stay loud — within ~8 dB
            // of the input. (Measured headroom: ~2-4 dB either way; the
            // bound is deliberately generous against encoder drift.)
            let level_db = 20.0 * (out_rms / in_rms).log10();
            assert!(
                level_db > -8.0 && level_db < 8.0,
                "block {i}: decoded level {level_db:.1} dB off input"
            );
        }
    }
}

#[test]
fn silence_stays_silent_across_frames() {
    let fw = CodecConfig::default();
    let mut enc = ReedEncoder::new(fw.clone(), ReedConfig::default()).unwrap();
    let mut dec = ReedDecoder::new(fw, ReedConfig::default()).unwrap();
    for i in 0..3i64 {
        let frame = frame_from_s16(vec![0u8; BLOCK * 2], i * BLOCK as i64);
        let pkt = enc.encode(&frame).unwrap().into_iter().next().unwrap();
        let out = dec.decode(&pkt).unwrap().into_iter().next().unwrap();
        assert!(
            out.samples.iter().all(|&b| b == 0),
            "frame {i}: silence must decode to exact digital silence"
        );
    }
}

#[test]
fn quiet_input_does_not_blow_up() {
    // A near-silent (but not exactly silent) frame takes the gain floor;
    // the decode must stay near-silent and finite, not amplify noise.
    let fw = CodecConfig::default();
    let mut enc = ReedEncoder::new(fw.clone(), ReedConfig::default()).unwrap();
    let mut dec = ReedDecoder::new(fw, ReedConfig::default()).unwrap();
    let mut state = 0xBEEF_CAFE_0000_0002u64;
    let block = random_block(&mut state, 2); // ±2 LSB dither
    let frame = frame_from_s16(block, 0);
    for pkt in enc.encode(&frame).unwrap() {
        let out = dec.decode(&pkt).unwrap().into_iter().next().unwrap();
        assert!(
            rms_of(&out) < rms_of(&frame) * 4.0 + 64.0,
            "near-silent input must not amplify"
        );
    }
}

#[test]
fn encoding_and_decoding_are_deterministic() {
    // §39: independent encoder/decoder instances must produce
    // byte-identical packets and byte-identical decodes.
    let mk = || {
        let fw = CodecConfig::default();
        (
            ReedEncoder::new(fw.clone(), ReedConfig::default()).unwrap(),
            ReedDecoder::new(fw, ReedConfig::default()).unwrap(),
        )
    };
    let (mut enc_a, mut dec_a) = mk();
    let (mut enc_b, mut dec_b) = mk();
    let mut state = 0x1234_5678_9ABC_DEF0u64;
    for i in 0..4i64 {
        let frame = frame_from_s16(random_block(&mut state, 16_000), i * BLOCK as i64);
        let pa = enc_a.encode(&frame).unwrap();
        let pb = enc_b.encode(&frame).unwrap();
        assert_eq!(pa.len(), pb.len());
        for (x, y) in pa.iter().zip(pb.iter()) {
            assert_eq!(x.data, y.data, "packets must be byte-identical");
            let da = dec_a.decode(x).unwrap();
            let db = dec_b.decode(y).unwrap();
            assert_eq!(
                da[0].samples, db[0].samples,
                "decodes must be byte-identical"
            );
        }
    }
}

#[test]
fn corruption_sweep_never_panics() {
    // §38: deterministic corruption/truncation/bit-flip sweep over a real
    // packet, in the project's no-external-harness style. Every decode
    // must return a `Result`, never panic.
    let fw = CodecConfig::default();
    let mut enc = ReedEncoder::new(fw.clone(), ReedConfig::default()).unwrap();
    let mut dec = ReedDecoder::new(fw, ReedConfig::default()).unwrap();
    let frame = frame_from_s16(random_block(&mut 0x5EED_0001u64, i16::MAX), 0);
    let valid: Vec<Packet> = enc.encode(&frame).unwrap();
    let pts = Timestamp::zero(Timebase::new(1, RATE));

    let mut state = 0x0F0F_0F0F_F0F0_F0F0u64;
    for _ in 0..300 {
        let len = (lcg(&mut state) % 64) as usize;
        let data: Vec<u8> = (0..len).map(|_| lcg(&mut state) as u8).collect();
        let pkt = Packet {
            data,
            pts,
            dts: pts,
            flags: tpt_media_core::PacketFlags::KEYFRAME,
            track_id: 0,
            duration: None,
        };
        let _ = dec.decode(&pkt);
    }

    for valid in &valid {
        for cut in 0..=valid.data.len() {
            let pkt = Packet {
                data: valid.data[..cut].to_vec(),
                ..valid.clone()
            };
            let _ = dec.decode(&pkt);
        }
        for _ in 0..256 {
            let pos = (lcg(&mut state) as usize) % valid.data.len();
            let bit = 1u8 << (lcg(&mut state) % 8);
            let mut corrupt = valid.clone();
            corrupt.data[pos] ^= bit;
            let _ = dec.decode(&corrupt);
        }
        assert!(
            dec.decode(valid).is_ok(),
            "pristine packet must still decode"
        );
    }
}

/// Voiced input must come back at its own level. The excitation gain is the
/// one thing that sets it, and it was wrong for voiced frames (a 200 Hz tone
/// came back up to 8 dB loud) until the gain was matched to the input power
/// through the quantized synthesis filter. Measured after warm-up, when the
/// envelope has settled.
#[test]
fn voiced_tone_keeps_its_level_after_warm_up() {
    let fw = CodecConfig::default();
    let mut enc = ReedEncoder::new(fw.clone(), ReedConfig::default()).unwrap();
    let mut dec = ReedDecoder::new(fw, ReedConfig::default()).unwrap();
    let blocks = 40i64;
    let mut in_sq = 0f64;
    let mut out_sq = 0f64;
    let mut n = 0usize;
    for i in 0..blocks {
        let samples: Vec<u8> = (0..BLOCK)
            .flat_map(|s| {
                let t = (i as usize * BLOCK + s) as f64;
                let v = (t * 2.0 * core::f64::consts::PI * 200.0 / RATE as f64).sin() * 6000.0;
                (v as i16).to_le_bytes()
            })
            .collect();
        let frame = frame_from_s16(samples, i * BLOCK as i64);
        for pkt in enc.encode(&frame).unwrap() {
            for out in dec.decode(&pkt).unwrap() {
                if i >= 10 {
                    for s in 0..BLOCK {
                        let want = frame.s16(0, s).unwrap() as f64;
                        let got = out.s16(0, s).unwrap() as f64;
                        in_sq += want * want;
                        out_sq += got * got;
                        n += 1;
                    }
                }
            }
        }
    }
    let level_db = 10.0 * (out_sq / in_sq).log10();
    assert!(
        level_db.abs() < 3.0,
        "decoded voiced level {level_db:.1} dB off input over {n} samples"
    );
}

/// A quiet voiced tone (about -52 dBFS) must not come back loud. Version 1
/// floored the gain at -62 dB, and a high-Q resonance needs about -78 dB, so
/// such a tone decoded about 16 dB too loud; version 2's 7-bit gain covers it.
#[test]
fn quiet_voiced_tone_keeps_its_level_after_warm_up() {
    let fw = CodecConfig::default();
    let mut enc = ReedEncoder::new(fw.clone(), ReedConfig::default()).unwrap();
    let mut dec = ReedDecoder::new(fw, ReedConfig::default()).unwrap();
    let mut in_sq = 0f64;
    let mut out_sq = 0f64;
    for i in 0..40i64 {
        let samples: Vec<u8> = (0..BLOCK)
            .flat_map(|s| {
                let t = (i as usize * BLOCK + s) as f64;
                let v = (t * 2.0 * core::f64::consts::PI * 200.0 / RATE as f64).sin() * 80.0;
                (v as i16).to_le_bytes()
            })
            .collect();
        let frame = frame_from_s16(samples, i * BLOCK as i64);
        for pkt in enc.encode(&frame).unwrap() {
            for out in dec.decode(&pkt).unwrap() {
                if i >= 10 {
                    for s in 0..BLOCK {
                        let want = frame.s16(0, s).unwrap() as f64;
                        let got = out.s16(0, s).unwrap() as f64;
                        in_sq += want * want;
                        out_sq += got * got;
                    }
                }
            }
        }
    }
    let level_db = 10.0 * (out_sq / in_sq).log10();
    assert!(
        level_db.abs() < 3.0,
        "quiet voiced level {level_db:.1} dB off input"
    );
}
