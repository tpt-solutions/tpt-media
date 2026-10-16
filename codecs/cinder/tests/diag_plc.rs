//! Diagnostic (ignored by default): how well `conceal` fills lost packets.
//!
//! Each signal is encoded, then decoded twice: once with every packet, and once
//! with a fixed pseudo-random subset dropped and concealed. The score is the SNR
//! of the second against the first, so codec noise cancels and only the
//! concealment is measured. Run with
//! `cargo test -p tpt-audio-cinder --test diag_plc -- --ignored --nocapture`
//! after `python benchmarks/run.py fetch`.

use tpt_audio_cinder::{CinderDecoder, CinderEncoder, CinderEncoderConfig};
use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{AudioFrame, ChannelLayout, Frame, Packet, SampleFormat, Timebase, Timestamp};

const RATE: u32 = 48_000;
const FRAME: usize = 256;

fn config() -> CinderEncoderConfig {
    CinderEncoderConfig {
        frame_size: FRAME as u16,
        bits_per_coef: 10,
        band_steps: vec![0.25; 16],
        ..Default::default()
    }
}

fn audio(samples: &[i16], start: usize) -> AudioFrame {
    let mut f = AudioFrame::alloc(samples.len() as u32, SampleFormat::S16, ChannelLayout::Mono, RATE);
    for (dst, s) in f.samples.chunks_mut(2).zip(samples) {
        dst.copy_from_slice(&s.to_le_bytes());
    }
    f.frame = Frame {
        pts: Timestamp {
            ticks: start as i64,
            timebase: Timebase::new(1, RATE),
        },
        duration: None,
        side_data: Vec::new(),
    };
    f
}

fn encode(signal: &[i16]) -> Vec<Packet> {
    let mut enc = CinderEncoder::new(CodecConfig::default(), config()).unwrap();
    let mut packets = Vec::new();
    for (i, chunk) in signal.chunks_exact(2 * FRAME).enumerate() {
        packets.extend(enc.encode(&audio(chunk, i * 2 * FRAME)).unwrap());
    }
    packets
}

fn pcm(frames: &[AudioFrame]) -> Vec<f64> {
    frames
        .iter()
        .flat_map(|f| f.samples.chunks_exact(2).map(|c| f64::from(i16::from_le_bytes([c[0], c[1]]))))
        .collect()
}

fn decode(packets: &[Packet], lost: &[bool]) -> Vec<f64> {
    let mut dec = CinderDecoder::new(CodecConfig::default(), config()).unwrap();
    let mut out = Vec::new();
    for (p, &gone) in packets.iter().zip(lost) {
        if gone {
            out.push(dec.conceal(p.pts).unwrap());
        } else {
            out.extend(dec.decode(p).unwrap());
        }
    }
    pcm(&out)
}

fn snr(reference: &[f64], test: &[f64]) -> f64 {
    let n = reference.len().min(test.len());
    let signal: f64 = reference[..n].iter().map(|x| x * x).sum();
    let noise: f64 = reference[..n].iter().zip(&test[..n]).map(|(a, b)| (a - b).powi(2)).sum();
    10.0 * (signal / noise.max(1e-9)).log10()
}

fn loss_pattern(count: usize, percent: u64, seed: u64) -> Vec<bool> {
    let mut state = seed;
    (0..count)
        .map(|i| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            // Never lose the first few packets: the decoder needs history.
            i >= 8 && (state >> 33) % 100 < percent
        })
        .collect()
}

fn voiced(seconds: usize) -> Vec<i16> {
    // A voiced-speech stand-in: a 140 Hz pulse train's first 12 harmonics with a slow vibrato.
    (0..RATE as usize * seconds)
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            let f0 = 140.0 * (1.0 + 0.02 * (2.0 * std::f64::consts::PI * 5.0 * t).sin());
            let phase = 2.0 * std::f64::consts::PI * f0 * t;
            let v: f64 = (1..=12).map(|k| (phase * f64::from(k)).sin() / f64::from(k)).sum();
            (v * 6000.0) as i16
        })
        .collect()
}

fn speech_clip() -> Vec<i16> {
    let b = std::fs::read("../../benchmarks/corpus/speech48/OSR_us_000_0010_8k_48k.wav")
        .expect("run `python benchmarks/run.py fetch` first");
    let mut i = 12;
    while i + 8 <= b.len() {
        let size = u32::from_le_bytes([b[i + 4], b[i + 5], b[i + 6], b[i + 7]]) as usize;
        if &b[i..i + 4] == b"data" {
            return b[i + 8..i + 8 + size]
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect();
        }
        i += 8 + size + (size & 1);
    }
    panic!("no data chunk");
}

#[test]
#[ignore]
fn concealment_snr_under_random_loss() {
    for (name, signal) in [("voiced", voiced(3)), ("speech", speech_clip()[..RATE as usize * 5].to_vec())] {
        let packets = encode(&signal);
        let reference = decode(&packets, &vec![false; packets.len()]);
        for percent in [5u64, 10, 20] {
            let mut total = 0.0;
            let seeds = [1u64, 2, 3, 4];
            for seed in seeds {
                let lost = loss_pattern(packets.len(), percent, seed);
                total += snr(&reference, &decode(&packets, &lost));
            }
            println!(
                "{name:>7} {percent:>2}% loss: SNR vs lossless decode {:.2} dB (mean of {} patterns)",
                total / seeds.len() as f64,
                seeds.len()
            );
        }
    }
}
