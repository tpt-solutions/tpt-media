//! §38 property tests and §39 determinism pins for the bitstream layer:
//! seeded-random write/read round trips through every coder this crate
//! exposes (`BitWriter`/`BitReader`, Rice, Huffman, range), plus
//! byte-for-byte reproducibility of the encode side. Same strategy as the
//! in-crate fuzz sweeps: fixed-seed LCG, no external harness.

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

#[test]
fn bit_writer_reader_round_trips_random_widths() {
    let mut state = 0x5EED_0001u64;
    for trial in 0..32 {
        let mut ops = Vec::new();
        for _ in 0..128 {
            let width = 1 + (lcg(&mut state) % 32) as u32;
            let mask = (1u64 << width) - 1;
            let value = lcg(&mut state) & mask;
            ops.push((width, value));
        }
        let mut buf = vec![0u8; 4096];
        let mut w = BitWriter::new(&mut buf);
        for &(width, value) in &ops {
            w.write(width, value).expect("buffer is large enough");
        }
        let written = w.finish().expect("finish succeeds");
        let mut r = BitReader::new(&buf[..written]);
        for (i, &(width, value)) in ops.iter().enumerate() {
            assert_eq!(
                r.read(width).expect("stream holds every written bit"),
                value,
                "trial {trial} op {i}: width {width}"
            );
        }
    }
}

#[test]
fn peek_predicts_read_for_random_widths() {
    let mut state = 0x5EED_0002u64;
    let mut buf = vec![0u8; 4096];
    let mut w = BitWriter::new(&mut buf);
    let mut widths = Vec::new();
    for _ in 0..256 {
        let width = 1 + (lcg(&mut state) % 16) as u32;
        w.write(width, lcg(&mut state) & ((1u64 << width) - 1))
            .expect("fits");
        widths.push(width);
    }
    let written = w.finish().expect("finish");
    let mut r = BitReader::new(&buf[..written]);
    for width in widths {
        let peeked = r.peek(width).expect("peek within stream");
        assert_eq!(r.read(width).expect("read within stream"), peeked);
    }
}

#[test]
fn rice_coding_round_trips_random_values() {
    let mut state = 0x5EED_0003u64;
    for _ in 0..64 {
        let k = (lcg(&mut state) % 9) as u32;
        // Unary-coded quotient: keep `magnitude >> k` small enough that
        // the worst case (k = 0) still fits the buffer comfortably.
        let magnitude = lcg(&mut state) % (1 << 13);
        let value = if lcg(&mut state) & 1 == 0 {
            magnitude as i64
        } else {
            -(magnitude as i64)
        };
        let mut buf = vec![0u8; 4096];
        let mut w = BitWriter::new(&mut buf);
        encode_riced(&mut w, k, value).expect("encode");
        let written = w.finish().expect("finish");
        let mut r = BitReader::new(&buf[..written]);
        assert_eq!(decode_riced(&mut r, k).expect("decode"), value, "k = {k}");
    }
}

#[test]
fn huffman_round_trips_random_symbols() {
    let mut state = 0x5EED_0004u64;
    // A fixed skewed distribution gives a non-degenerate code.
    let freqs = [100u64, 40, 18, 7, 3, 1];
    let table = build_table(&freqs);
    let mut symbols = Vec::new();
    for _ in 0..512 {
        symbols.push((lcg(&mut state) % freqs.len() as u64) as u16);
    }
    let mut buf = vec![0u8; 4096];
    let mut w = BitWriter::new(&mut buf);
    for &s in &symbols {
        table.encode(&mut w, s).expect("encode");
    }
    let written = w.finish().expect("finish");
    let mut r = BitReader::new(&buf[..written]);
    for &s in &symbols {
        assert_eq!(table.decode(&mut r).expect("decode"), s);
    }
}

#[test]
fn range_coder_round_trips_random_symbols() {
    let mut state = 0x5EED_0005u64;
    let total = 64u32;
    let freqs = [32u32, 16, 8, 8];
    let cums = [0u32, 32, 48, 56];
    let mut buf = vec![0u8; 8192];
    let mut symbols = Vec::new();
    let n = {
        let mut enc = RangeEncoder::new(&mut buf);
        for _ in 0..256 {
            let s = (lcg(&mut state) % freqs.len() as u64) as usize;
            enc.encode(cums[s], freqs[s], total).expect("encode");
            symbols.push(s);
        }
        enc.finish().expect("finish")
    };
    let mut dec = RangeDecoder::new(&buf[..n]).expect("decoder");
    for (i, s) in symbols.iter().enumerate() {
        let cum = dec.decode_peek(total).expect("peek");
        assert!(
            cum >= cums[*s] && cum < cums[*s] + freqs[*s],
            "symbol {i}: peeked {cum} outside symbol {s}'s interval [{}, {})",
            cums[*s],
            cums[*s] + freqs[*s]
        );
        dec.update(cums[*s], freqs[*s], total)
            .expect("update with the peeked symbol's interval");
    }
}

#[test]
fn encoding_is_deterministic() {
    // §39: identical inputs must produce byte-identical encodes.
    let mut state = 0x5EED_0006u64;
    let ops: Vec<(u32, u64)> = (0..256)
        .map(|_| {
            let width = 1 + (lcg(&mut state) % 24) as u32;
            (width, lcg(&mut state) & ((1u64 << width) - 1))
        })
        .collect();
    let encode = || {
        let mut buf = vec![0u8; 4096];
        let mut w = BitWriter::new(&mut buf);
        for &(width, value) in &ops {
            w.write(width, value).expect("fits");
        }
        let written = w.finish().expect("finish");
        buf[..written].to_vec()
    };
    assert_eq!(encode(), encode());

    let rice_encode = |k: u32| {
        let mut buf = vec![0u8; 1 << 17];
        let mut w = BitWriter::new(&mut buf);
        for i in 0..128 {
            encode_riced(&mut w, k, (i as i64) * 37 - 2000).expect("encode");
        }
        let written = w.finish().expect("finish");
        buf[..written].to_vec()
    };
    for k in 0..4 {
        assert_eq!(rice_encode(k), rice_encode(k), "rice k = {k}");
    }
}
