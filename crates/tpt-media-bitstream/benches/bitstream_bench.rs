//! §41 bitstream/entropy benchmarks: raw bit I/O plus the three entropy
//! coders (Rice, range) at codec-realistic batch sizes.

use std::hint::black_box;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use tpt_media_bitstream::huffman::build_table;
use tpt_media_bitstream::range::{RangeDecoder, RangeEncoder};
use tpt_media_bitstream::rice::{decode_riced, encode_riced};
use tpt_media_bitstream::{BitReader, BitWriter};

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

fn bench_bitstream(c: &mut Criterion) {
    let mut group = c.benchmark_group("bitstream");
    group.sample_size(20);
    group.warm_up_time(Duration::from_millis(300));
    group.measurement_time(Duration::from_secs(2));

    // Deterministic operand set shared by all benches.
    let mut state = 0x5EED_B175u64;
    let ops: Vec<(u32, u64)> = (0..1024)
        .map(|_| {
            let width = 1 + (lcg(&mut state) % 32) as u32;
            (width, lcg(&mut state) & ((1u64 << width) - 1))
        })
        .collect();
    let mut buf = vec![0u8; 1 << 16];

    group.bench_function("bit_write/1024_ops", |b| {
        b.iter(|| {
            let mut w = BitWriter::new(black_box(&mut buf));
            for &(width, value) in black_box(&ops) {
                w.write(width, value).expect("fits");
            }
            black_box(w.finish().expect("finish"))
        })
    });

    // Pre-fill for the read bench.
    let filled = {
        let mut w = BitWriter::new(&mut buf);
        for &(width, value) in &ops {
            w.write(width, value).expect("fits");
        }
        let n = w.finish().expect("finish");
        buf[..n].to_vec()
    };
    group.bench_function("bit_read/1024_ops", |b| {
        b.iter(|| {
            let mut r = BitReader::new(black_box(&filled));
            for &(width, _) in black_box(&ops) {
                black_box(r.read(width).expect("read"));
            }
        })
    });

    // Rice: 256 residuals at k=2 (a codec-realistic skew).
    let rice_values: Vec<i64> = (0..256)
        .map(|_| {
            let m = lcg(&mut state) % (1 << 12);
            if lcg(&mut state) & 1 == 0 {
                m as i64
            } else {
                -(m as i64)
            }
        })
        .collect();
    group.bench_function("rice_encode/256_values_k2", |b| {
        b.iter(|| {
            let mut w = BitWriter::new(black_box(&mut buf));
            for &v in black_box(&rice_values) {
                encode_riced(&mut w, 2, v).expect("encode");
            }
            black_box(w.finish().expect("finish"))
        })
    });
    let rice_filled = {
        let mut w = BitWriter::new(&mut buf);
        for &v in &rice_values {
            encode_riced(&mut w, 2, v).expect("encode");
        }
        let n = w.finish().expect("finish");
        buf[..n].to_vec()
    };
    group.bench_function("rice_decode/256_values_k2", |b| {
        b.iter(|| {
            let mut r = BitReader::new(black_box(&rice_filled));
            for &_ in black_box(&rice_values) {
                black_box(decode_riced(&mut r, 2).expect("decode"));
            }
        })
    });

    // Range coder: 256 symbols over a skewed 4-symbol model, via the
    // decode_peek/update two-phase path (freq > 1).
    let freqs = [32u32, 16, 8, 8];
    let cums = [0u32, 32, 48, 56];
    let symbols: Vec<usize> = (0..256).map(|_| (lcg(&mut state) % 4) as usize).collect();
    group.bench_function("range_encode/256_symbols", |b| {
        b.iter(|| {
            let mut enc = RangeEncoder::new(black_box(&mut buf));
            for &s in black_box(&symbols) {
                enc.encode(cums[s], freqs[s], 64).expect("encode");
            }
            black_box(enc.finish().expect("finish"))
        })
    });
    let range_filled = {
        let mut enc = RangeEncoder::new(&mut buf);
        for &s in &symbols {
            enc.encode(cums[s], freqs[s], 64).expect("encode");
        }
        let n = enc.finish().expect("finish");
        buf[..n].to_vec()
    };
    group.bench_function("range_decode/256_symbols", |b| {
        b.iter(|| {
            let mut dec = RangeDecoder::new(black_box(&range_filled)).expect("new");
            for &s in black_box(&symbols) {
                let cum = dec.decode_peek(64).expect("peek");
                dec.update(cums[s], freqs[s], 64).expect("update");
                black_box(cum);
            }
        })
    });

    // Huffman: fixed skewed table, 512 symbols.
    let table = build_table(&[100u64, 40, 18, 7, 3, 1]);
    let huff_symbols: Vec<u16> = (0..512).map(|_| (lcg(&mut state) % 6) as u16).collect();
    group.bench_function("huffman_encode/512_symbols", |b| {
        b.iter(|| {
            let mut w = BitWriter::new(black_box(&mut buf));
            for &s in black_box(&huff_symbols) {
                table.encode(&mut w, s).expect("encode");
            }
            black_box(w.finish().expect("finish"))
        })
    });
    let huff_filled = {
        let mut w = BitWriter::new(&mut buf);
        for &s in &huff_symbols {
            table.encode(&mut w, s).expect("encode");
        }
        let n = w.finish().expect("finish");
        buf[..n].to_vec()
    };
    group.bench_function("huffman_decode/512_symbols", |b| {
        b.iter(|| {
            let mut r = BitReader::new(black_box(&huff_filled));
            for &_ in black_box(&huff_symbols) {
                black_box(table.decode(&mut r).expect("decode"));
            }
        })
    });

    group.finish();
}

criterion_group!(benches, bench_bitstream);
criterion_main!(benches);
