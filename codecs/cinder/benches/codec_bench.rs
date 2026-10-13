//! §41 codec benchmarks: Cinder encode/decode throughput at the default
//! operating point (frame_size 256 @ 48 kHz, 8 bits/coefficient), one
//! 512-sample block per packet — the same shape `tpt benchmark` drives.

use std::hint::black_box;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use tpt_audio_cinder::{CinderDecoder, CinderEncoder, CinderEncoderConfig};
use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{AudioFrame, ChannelLayout, Frame, SampleFormat, Timebase, Timestamp};

const RATE: u32 = 48_000;
const BLOCK: usize = 512;
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

/// 64 blocks of a mixed tonal+noise signal, deterministic.
fn blocks() -> Vec<AudioFrame> {
    let mut state = 0x5EED_BEEFu64;
    (0..BLOCKS)
        .map(|i| {
            let mut b = Vec::with_capacity(BLOCK * 2);
            for j in 0..BLOCK {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                let noise = ((state >> 33) as i32 % 8192) - 4096;
                let tone = (0.3 * ((i * BLOCK + j) as f32 * 0.13).sin() * 12_000.0) as i32;
                let v = (noise + tone).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
                b.extend_from_slice(&v.to_le_bytes());
            }
            make_frame(b, i)
        })
        .collect()
}

fn bench_cinder(c: &mut Criterion) {
    let mut group = c.benchmark_group("cinder");
    group.sample_size(20);
    group.warm_up_time(Duration::from_millis(300));
    group.measurement_time(Duration::from_secs(2));

    let input = blocks();
    let enc_cfg = CinderEncoderConfig {
        sample_rate: RATE,
        ..CinderEncoderConfig::default()
    };

    group.bench_function("encode/64x512@48k", |b| {
        let mut enc = CinderEncoder::new(CodecConfig::default(), enc_cfg.clone()).unwrap();
        b.iter(|| {
            for frame in black_box(&input) {
                black_box(enc.encode(frame).expect("encode"));
            }
        })
    });

    // Decode: packets pre-encoded once outside the timed loop. Each 512-sample
    // block yields two hop-256 packets, so this is 128 packets.
    let packets: Vec<_> = {
        let mut enc = CinderEncoder::new(CodecConfig::default(), enc_cfg.clone()).unwrap();
        input
            .iter()
            .flat_map(|f| enc.encode(f).expect("encode"))
            .collect()
    };
    group.bench_function("decode/128_packets@48k", |b| {
        let mut dec = CinderDecoder::new(CodecConfig::default(), enc_cfg.clone()).unwrap();
        b.iter(|| {
            for pkt in black_box(&packets) {
                black_box(dec.decode(pkt).expect("decode"));
            }
        })
    });

    group.finish();
}

criterion_group!(benches, bench_cinder);
criterion_main!(benches);
