//! §41 codec benchmarks: Reed encode/decode at its default operating point
//! (320-sample / 20 ms frames @ 16 kHz, LPC order 10, 5-bit LSFs, 1024-entry
//! stochastic codebook).

use std::hint::black_box;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use tpt_audio_reed::{ReedConfig, ReedDecoder, ReedEncoder};
use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{AudioFrame, ChannelLayout, Frame, SampleFormat, Timebase, Timestamp};

const RATE: u32 = 16_000;
const BLOCK: usize = 320;
const BLOCKS: usize = 64;

fn make_frame(samples: Vec<u8>, index: usize) -> AudioFrame {
    AudioFrame {
        frame: Frame {
            pts: Timestamp {
                ticks: (index * BLOCK) as i64,
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

/// Alternating vowel-like (periodic) and noise-like blocks — both halves
/// of the vocoder's search space should be exercised, not just one.
fn blocks() -> Vec<AudioFrame> {
    let mut state = 0x5EED_A11Au64;
    (0..BLOCKS)
        .map(|i| {
            let mut b = Vec::with_capacity(BLOCK * 2);
            for j in 0..BLOCK {
                let n = i * BLOCK + j;
                let v = if i % 2 == 0 {
                    // 200 Hz harmonic stack with formant-ish amplitudes.
                    let t = n as f64 / RATE as f64;
                    let mut s = 0.0f64;
                    for k in 1..=40usize {
                        let f = 200.0 * k as f64;
                        s += (-((f - 700.0) / 300.0).powi(2))
                            * (core::f64::consts::TAU * f * t).sin();
                    }
                    s * 0.04
                } else {
                    state = state
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    ((state >> 33) as f64 / 2147483648.0 - 1.0) * 0.05
                };
                let s = (v.clamp(-1.0, 1.0) * i16::MAX as f64) as i16;
                b.extend_from_slice(&s.to_le_bytes());
            }
            make_frame(b, i)
        })
        .collect()
}

fn bench_reed(c: &mut Criterion) {
    let mut group = c.benchmark_group("reed");
    group.sample_size(20);
    group.warm_up_time(Duration::from_millis(300));
    group.measurement_time(Duration::from_secs(2));

    let input = blocks();
    let fw = CodecConfig::default();
    let cfg = ReedConfig::default();
    let mut encoder = ReedEncoder::new(fw.clone(), cfg.clone()).unwrap();
    let mut decoder = ReedDecoder::new(fw, cfg).unwrap();

    group.bench_function("encode/16khz_320", |b| {
        b.iter(|| {
            for frame in black_box(&input) {
                black_box(encoder.encode(frame).unwrap());
            }
        })
    });

    let packets: Vec<_> = input
        .iter()
        .flat_map(|f| encoder.encode(f).unwrap())
        .collect();

    group.bench_function("decode/16khz_320", |b| {
        b.iter(|| {
            for pkt in black_box(&packets) {
                black_box(decoder.decode(pkt).unwrap());
            }
        })
    });

    group.finish();
}

criterion_group!(benches, bench_reed);
criterion_main!(benches);
