//! §41 codec benchmarks: Aura *lossless* encode/decode at its default
//! operating point (1024-sample blocks @ 48 kHz, LPC order 16).

use std::hint::black_box;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use tpt_audio_aura::{AuraConfig, AuraDecoder, AuraEncoder};
use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{AudioFrame, ChannelLayout, Frame, SampleFormat, Timebase, Timestamp};

const RATE: u32 = 48_000;
const BLOCK: usize = 1024;
const BLOCKS: usize = 32;

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

fn blocks() -> Vec<AudioFrame> {
    let mut state = 0x5EED_A11Au64;
    (0..BLOCKS)
        .map(|i| {
            let mut b = Vec::with_capacity(BLOCK * 2);
            for j in 0..BLOCK {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let noise = ((state >> 33) as i32 % 16_384) - 8192;
                let tone = (0.25 * ((i * BLOCK + j) as f32 * 0.07).sin() * 16_000.0) as i32;
                let v = (noise + tone).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                b.extend_from_slice(&v.to_le_bytes());
            }
            make_frame(b, i)
        })
        .collect()
}

fn bench_aura(c: &mut Criterion) {
    let mut group = c.benchmark_group("aura_lossless");
    group.sample_size(20);
    group.warm_up_time(Duration::from_millis(300));
    group.measurement_time(Duration::from_secs(2));

    let input = blocks();
    let cfg = AuraConfig {
        sample_rate: RATE,
        ..AuraConfig::default()
    };

    group.bench_function("encode/32x1024@48k", |b| {
        let mut enc = AuraEncoder::new(CodecConfig::default(), cfg.clone()).unwrap();
        b.iter(|| {
            for frame in black_box(&input) {
                black_box(enc.encode(frame).expect("encode"));
            }
        })
    });

    let packets: Vec<_> = {
        let mut enc = AuraEncoder::new(CodecConfig::default(), cfg.clone()).unwrap();
        input
            .iter()
            .flat_map(|f| enc.encode(f).expect("encode"))
            .collect()
    };
    group.bench_function("decode/32_packets@48k", |b| {
        let mut dec = AuraDecoder::new(CodecConfig::default(), cfg.clone()).unwrap();
        b.iter(|| {
            for pkt in black_box(&packets) {
                black_box(dec.decode(pkt).expect("decode"));
            }
        })
    });

    group.finish();
}

criterion_group!(benches, bench_aura);
criterion_main!(benches);
