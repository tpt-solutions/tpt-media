//! Unit tests for the primitives: entropy, transform, prediction, filter,
//! config. Codec-level behaviour is covered by `tests/`.

use super::*;
use crate::bits::{
    fold, read_rice_unsigned, rice_len_signed, rice_len_unsigned, write_rice_unsigned, BitCounter,
    PacketBits, RICE_K_MAX,
};
use crate::filter::{loop_filter, Field, Unit};
use crate::picture::{Picture, Plane};
use crate::pred::{chroma_mv, intra_predict, mc_block};
use crate::syntax::ref_index_bits;
use crate::transform::{build_scan, read_coefs, tile_size, write_coefs, Scans, Xform};
use tpt_media_bitstream::rice::WriteLike;
use tpt_media_bitstream::BitReader;

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

#[test]
fn crc32_matches_the_reference_vector() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b""), 0);
    assert_eq!(packet_crc(b"123456789"), 0xCBF4_3926);
}

#[test]
fn quant_step_is_monotonic_and_clamped() {
    for q in MIN_QP..MAX_QP {
        assert!(quant_step(q) < quant_step(q + 1));
    }
    assert_eq!(quant_step(0), quant_step(MIN_QP));
    assert_eq!(quant_step(255), quant_step(MAX_QP));
    let mut prev = u8::MAX;
    for q in 0..=100u8 {
        let qp = quality_to_qp(q);
        assert!(qp <= prev && (MIN_QP..=MAX_QP).contains(&qp));
        prev = qp;
    }
    assert_eq!(quality_to_qp(100), MIN_QP);
    assert_eq!(quality_to_qp(0), MAX_QP);
}

#[test]
fn scans_are_bijections_starting_at_dc() {
    for n in [4usize, 8, 16, 32] {
        let s = build_scan(n);
        assert_eq!(s.len(), n * n);
        assert_eq!(s[0], 0);
        let mut seen = vec![false; n * n];
        for &r in &s {
            assert!(!seen[r as usize], "n={n}: {r} repeated");
            seen[r as usize] = true;
        }
        // The scan visits anti-diagonals in order.
        let diag = |r: u16| (r as usize / n) + (r as usize % n);
        assert!(s.windows(2).all(|w| diag(w[0]) <= diag(w[1])));
    }
    // Known answer: the classic 4x4 zig-zag.
    assert_eq!(
        build_scan(4),
        [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15]
    );
}

#[test]
fn rice_lengths_match_what_is_written() {
    let mut state = 7u64;
    for _ in 0..500 {
        let k = (lcg(&mut state) % 8) as u32;
        let u = (lcg(&mut state) % 5000) as u32;
        let v = (lcg(&mut state) % 20001) as i32 - 10000;
        let mut c = BitCounter::default();
        write_rice_unsigned(&mut c, k, u).unwrap();
        assert_eq!(c.bits, u64::from(rice_len_unsigned(u, k)));
        let mut c = BitCounter::default();
        tpt_media_bitstream::rice::encode_riced(&mut c, k, i64::from(v)).unwrap();
        assert_eq!(c.bits, u64::from(rice_len_signed(v, k)), "v={v} k={k}");
    }
    assert_eq!(fold(0), 0);
    assert_eq!(fold(-1), 1);
    assert_eq!(fold(1), 2);
}

#[test]
fn unsigned_rice_round_trips_through_the_packet_writer() {
    let mut w = PacketBits::new();
    let vals = [(0u32, 0u32), (1, 0), (37, 0), (0, 3), (255, 3), (1000, 7)];
    for &(u, k) in &vals {
        write_rice_unsigned(&mut w, k, u).unwrap();
    }
    let bits = w.bits();
    let bytes = w.finish();
    assert_eq!(bytes.len() as u64, bits.div_ceil(8));
    let mut r = BitReader::new(&bytes);
    for &(u, k) in &vals {
        assert_eq!(read_rice_unsigned(&mut r, k).unwrap(), u);
    }
}

#[test]
fn coefficient_blocks_round_trip_at_every_size() {
    let scans = Scans::new();
    let mut state = 0xC0FFEE_u64;
    for n in [4usize, 8, 16, 32] {
        for density in [0u64, 1, 8, 64] {
            for _ in 0..8 {
                let mut levels = vec![0i32; n * n];
                for l in levels.iter_mut() {
                    if density > 0 && lcg(&mut state) % 100 < density {
                        let mag = 1 + (lcg(&mut state) % 40) as i32;
                        let big = if lcg(&mut state) % 50 == 0 {
                            MAX_LEVEL
                        } else {
                            mag
                        };
                        *l = if lcg(&mut state) % 2 == 0 { big } else { -big };
                    }
                }
                let mut w = PacketBits::new();
                write_coefs(&mut w, &levels, n, scans.get(n)).unwrap();
                let bytes = w.finish();
                let mut r = BitReader::new(&bytes);
                let mut back = vec![9i32; n * n];
                read_coefs(&mut r, &mut back, n, scans.get(n)).unwrap();
                assert_eq!(back, levels, "n={n} density={density}");
            }
        }
    }
}

#[test]
fn worst_case_coefficient_block_stays_within_the_bound() {
    // Every level at +/-MAX_LEVEL is the most expensive block there is.
    let scans = Scans::new();
    for n in [4usize, 8, 16, 32] {
        let levels: Vec<i32> = (0..n * n)
            .map(|i| if i % 2 == 0 { MAX_LEVEL } else { -MAX_LEVEL })
            .collect();
        let mut c = BitCounter::default();
        write_coefs(&mut c, &levels, n, scans.get(n)).unwrap();
        let bound = 1
            + 3
            + 2 * u64::from(n.trailing_zeros())
            + (n * n) as u64 * 2 * u64::from(transform::MAX_RICE_BITS);
        assert!(c.bits <= bound, "n={n}: {} > {bound}", c.bits);
    }
    assert_eq!(RICE_K_MAX, 7);
}

#[test]
fn hostile_coefficient_streams_never_panic() {
    let scans = Scans::new();
    let mut state = 0xBADC0DE_u64;
    for n in [4usize, 8, 16, 32] {
        for _ in 0..300 {
            let len = (lcg(&mut state) % 64) as usize;
            let bytes: Vec<u8> = (0..len).map(|_| lcg(&mut state) as u8).collect();
            let mut r = BitReader::new(&bytes);
            let mut levels = vec![0i32; n * n];
            let _ = read_coefs(&mut r, &mut levels, n, scans.get(n));
            assert!(levels.iter().all(|&l| l.abs() <= LEVEL_CLAMP));
        }
    }
    // All zeros: `coded = 0`, an empty block.
    let zeros = vec![0u8; 4096];
    let mut r = BitReader::new(&zeros);
    let mut levels = vec![0i32; 64];
    assert!(read_coefs(&mut r, &mut levels, 8, scans.get(8)).is_ok());
    // `coded = 1` followed by zeros: the unary walk must stop with an error.
    let mut ones_then_zeros = vec![0u8; 4096];
    ones_then_zeros[0] = 0x80;
    let mut r = BitReader::new(&ones_then_zeros);
    assert!(read_coefs(&mut r, &mut levels, 8, scans.get(8)).is_err());
}

/// Forward + inverse at the finest quantiser reproduces a residual to
/// within the quantiser's own error, at every transform size, and a
/// constant block puts all its energy in the DC coefficient with the
/// orthonormal value `n * c`.
#[test]
fn transform_round_trip_and_dc_known_answer() {
    let mut xf = Xform::new();
    let step = quant_step(MIN_QP);
    let mut state = 5u64;
    for n in [4usize, 8, 16, 32] {
        let t = tile_size(n);
        let pred: Vec<u8> = (0..n * n)
            .map(|_| 100 + (lcg(&mut state) % 20) as u8)
            .collect();
        let src: Vec<u8> = pred
            .iter()
            .map(|&p| (i32::from(p) + (lcg(&mut state) % 41) as i32 - 20) as u8)
            .collect();
        let mut levels = vec![0i32; t * t];
        xf.quantise_tile(&src, &pred, n, 0, 0, t, step, &mut levels);
        let mut out = vec![0u8; n * n];
        xf.reconstruct_tile(&levels, &pred, n, 0, 0, t, step, &mut out);
        let max_err = out
            .iter()
            .zip(src.iter())
            .map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs())
            .max()
            .unwrap();
        assert!(max_err <= 2, "n={n}: max error {max_err}");

        // Constant residual +8: DC level = trunc(8 * n / step + offset).
        let cpred = vec![100u8; n * n];
        let csrc = vec![108u8; n * n];
        xf.quantise_tile(&csrc, &cpred, n, 0, 0, t, step, &mut levels);
        let want = ((8.0 * n as f32 / step) + ROUND_OFFSET) as i32;
        assert_eq!(levels[0], want, "n={n}");
        assert!(levels[1..].iter().all(|&l| l == 0), "n={n}: AC leaked");
    }
}

#[test]
fn coarser_steps_never_increase_the_number_of_nonzero_levels() {
    let mut xf = Xform::new();
    let mut state = 21u64;
    let pred = vec![128u8; 64];
    let src: Vec<u8> = (0..64)
        .map(|_| (128 + (lcg(&mut state) % 61) as i32 - 30) as u8)
        .collect();
    let count = |qp: u8, xf: &mut Xform| {
        let mut l = vec![0i32; 64];
        xf.quantise_tile(&src, &pred, 8, 0, 0, 8, quant_step(qp), &mut l);
        l.iter().filter(|&&v| v != 0).count()
    };
    let mut prev = usize::MAX;
    for qp in [1u8, 10, 20, 30, 40, 51] {
        let c = count(qp, &mut xf);
        assert!(c <= prev, "qp {qp}: {c} > {prev}");
        prev = c;
    }
}

#[test]
fn half_pel_interpolation_known_answers_and_edge_safety() {
    // 4x4 plane, value = 10 * x + y.
    let mut p = Plane::new(4, 4, 0);
    for y in 0..4 {
        for x in 0..4 {
            p.data[y * 4 + x] = (10 * x + y) as u8;
        }
    }
    let mut out = [0u8; 4];
    // Integer vector: a copy of the row-1 window at x = 0..2.
    mc_block(&p, 0, 1, 2, 1, 0, 0, &mut out);
    assert_eq!(&out[..2], &[1, 11]);
    // Half-pel right: (a + b + 1) >> 1.
    mc_block(&p, 0, 0, 2, 1, 1, 0, &mut out);
    assert_eq!(&out[..2], &[5, 15]);
    // Half-pel down: (0 + 1 + 1) >> 1.
    mc_block(&p, 0, 0, 1, 1, 0, 1, &mut out);
    assert_eq!(out[0], 1);
    // Diagonal: (0 + 10 + 1 + 11 + 2) >> 2.
    mc_block(&p, 0, 0, 1, 1, 1, 1, &mut out);
    assert_eq!(out[0], 6);
    // -1 half-pel is "one full pel left plus a half": between x = 0 and 1.
    mc_block(&p, 1, 0, 1, 1, -1, 0, &mut out);
    assert_eq!(out[0], 5);
    // Wildly off-picture vectors clamp instead of panicking.
    for v in [-1000, -65, 65, 1001] {
        let mut big = [0u8; 16];
        mc_block(&p, 0, 0, 4, 4, v, -v, &mut big);
        mc_block(&p, 0, 0, 4, 4, -v, v, &mut big);
    }
    assert_eq!(chroma_mv(4), 2);
    assert_eq!(chroma_mv(-3), -2);
}

#[test]
fn intra_modes_and_availability() {
    let mut p = Plane::new(16, 16, 0);
    for y in 0..16 {
        for x in 0..16 {
            p.data[y * 16 + x] = (x * 4 + y * 8) as u8;
        }
    }
    let mut out = vec![0u8; 64];
    // No neighbours: DC is mid-grey.
    intra_predict(&p, 0, 0, 8, IntraMode::Dc, &mut out);
    assert!(out.iter().all(|&v| v == 128));
    // Unavailable directional modes degrade to DC, never read outside.
    intra_predict(&p, 0, 0, 8, IntraMode::Plane, &mut out);
    assert!(out.iter().all(|&v| v == 128));
    intra_predict(&p, 0, 0, 8, IntraMode::Vertical, &mut out);
    assert!(out.iter().all(|&v| v == 128));
    assert!(!intra_mode_available(IntraMode::Horizontal, 0, 8));
    assert!(intra_mode_available(IntraMode::Horizontal, 8, 0));
    assert!(!intra_mode_available(IntraMode::Plane, 8, 0));
    // Vertical copies the row above.
    intra_predict(&p, 8, 8, 8, IntraMode::Vertical, &mut out);
    for x in 0..8 {
        assert_eq!(out[7 * 8 + x], p.data[7 * 16 + 8 + x]);
    }
    // Plane extrapolation reproduces a linear ramp exactly.
    intra_predict(&p, 8, 8, 8, IntraMode::Plane, &mut out);
    for y in 0..8 {
        for x in 0..8 {
            assert_eq!(out[y * 8 + x], p.data[(8 + y) * 16 + 8 + x], "({x},{y})");
        }
    }
}

#[test]
fn loop_filter_behaviour() {
    let unit = |intra: bool, coded: bool| Unit {
        intra,
        inter: !intra,
        coded,
        size: 8,
        ..Unit::default()
    };
    let mk = |left: u8, right: u8| {
        let mut pic = Picture::new(16, 8, PixelFormat::Gray8, 0);
        for y in 0..8 {
            for x in 0..16 {
                pic.planes[0].data[y * 16 + x] = if x < 8 { left } else { right };
            }
        }
        pic
    };
    let field = |a: Unit, b: Unit| {
        let mut f = Field::new(16, 8);
        f.units[0] = a;
        f.units[1] = b;
        f
    };
    // Flat: unchanged whatever the strength.
    let mut flat = mk(100, 100);
    loop_filter(&mut flat, &field(unit(true, true), unit(true, true)), 30);
    assert_eq!(flat, mk(100, 100));
    // A small step across an intra edge is smoothed.
    let mut a = mk(100, 108);
    loop_filter(&mut a, &field(unit(true, false), unit(true, false)), 30);
    assert_ne!(a, mk(100, 108));
    assert!(a.planes[0].data[7] > 100 && a.planes[0].data[8] < 108);
    // Two identical skip units: no edge to smooth.
    let mut b = mk(100, 108);
    loop_filter(&mut b, &field(unit(false, false), unit(false, false)), 30);
    assert_eq!(b, mk(100, 108));
    // A genuine large edge (>= alpha) is preserved even when intra.
    let mut c = mk(20, 220);
    loop_filter(&mut c, &field(unit(true, true), unit(true, true)), 10);
    assert_eq!(c, mk(20, 220));
}

#[test]
fn config_validation_and_bounds() {
    let ok = VaultConfig::default();
    assert!(ok.validate().is_ok());
    assert!(VaultConfig {
        width: 9,
        ..ok.clone()
    }
    .validate()
    .is_err());
    assert!(VaultConfig {
        max_refs: 17,
        ..ok.clone()
    }
    .validate()
    .is_err());
    // The bound grows with the frame and never undercuts a raw frame.
    let small = VaultConfig {
        width: 64,
        height: 64,
        ..ok.clone()
    };
    let big = VaultConfig {
        width: 128,
        height: 64,
        ..ok.clone()
    };
    assert!(big.max_packet_bits() > small.max_packet_bits());
    assert!(small.max_packet_bits() > 64 * 64 * 8);
    let gray = VaultConfig {
        format: PixelFormat::Gray8,
        ..small.clone()
    };
    assert!(gray.max_packet_bits() < small.max_packet_bits());
    assert_eq!(ref_index_bits(1), 0);
    assert_eq!(ref_index_bits(2), 1);
    assert_eq!(ref_index_bits(3), 2);
    assert_eq!(ref_index_bits(4), 2);
    assert_eq!(ref_index_bits(5), 3);
    assert_eq!(ref_index_bits(16), 4);
    assert_eq!(MV_LIMIT, 65);
}

#[test]
fn the_writer_and_counter_agree_on_length() {
    let mut w = PacketBits::new();
    let mut c = BitCounter::default();
    for n in 1..=32u32 {
        let v = (1u64 << n) - 1;
        w.write_bits(n, v).unwrap();
        c.write_bits(n, v).unwrap();
    }
    assert_eq!(w.bits(), c.bits);
    assert!(w.write_bits(33, 0).is_err());
}

/// RDOQ drops a cheap level whose rate saving buys more than the
/// distortion it adds, keeps an expensive one, and (checked implicitly by
/// the second half) never touches the block's largest level.
#[test]
fn rdoq_zeroes_a_cheap_level_and_keeps_an_expensive_one() {
    use crate::syntax::rdoq_tile;
    let scan = build_scan(4);
    let mut levels = vec![0i32; 16];
    levels[scan[5] as usize] = 1; // interior: merging its runs saves real bits
    levels[scan[15] as usize] = 40; // the block's last, and far too big to drop

    // Small step: the level-1 distortion is negligible, the run merge is
    // not \u2192 dropped.
    let mut small = levels.clone();
    rdoq_tile(&mut small, &scan, 0.3, crate::LAMBDA_SCALE);
    assert_eq!(small[scan[5] as usize], 0, "cheap level must be dropped");
    assert_eq!(small[scan[15] as usize], 40, "expensive level must survive");

    // Large step: the same level now carries distortion worth keeping.
    let mut large = levels.clone();
    rdoq_tile(&mut large, &scan, 3.0, crate::LAMBDA_SCALE);
    assert_eq!(large[scan[5] as usize], 1, "the level earns its rate back");
    assert_eq!(large[scan[15] as usize], 40);
}

/// RDOQ can zero a block entirely, which must leave all levels zero so
/// the writer emits the no-coded-bit path.
#[test]
fn rdoq_can_zero_a_whole_block() {
    use crate::syntax::rdoq_tile;
    let scan = build_scan(4);
    let mut levels = vec![0i32; 16];
    levels[scan[2] as usize] = 1;
    levels[scan[9] as usize] = 1;
    rdoq_tile(&mut levels, &scan, 0.1, crate::LAMBDA_SCALE);
    assert!(levels.iter().all(|&l| l == 0), "tiny levels at a tiny step");
}

/// The loop-filter RD decision patches a finished packet: the bit it
/// flips must be exactly the one the decoder parses as `loop_filter`,
/// with every other header field untouched.
#[test]
fn set_loop_filter_bit_targets_the_bit_the_decoder_reads() {
    use crate::encoder::set_loop_filter_bit;

    let parse = |data: &[u8]| {
        let mut r = BitReader::new(data);
        let version = r.read(16).unwrap();
        let ftype = r.read(2).unwrap();
        let qp = r.read(QP_BITS).unwrap();
        let num_refs = r.read(5).unwrap();
        let lf = r.read(1).unwrap();
        let poc = r.read(16).unwrap();
        (version, ftype, qp, num_refs, lf, poc)
    };

    // A v2 header exactly as the encoder writes it: filter on.
    let mut w = PacketBits::new();
    w.write_bits(16, u64::from(VERSION)).unwrap();
    w.write_bits(2, u64::from(crate::ftype::P)).unwrap();
    w.write_bits(QP_BITS, 30).unwrap();
    w.write_bits(5, 4).unwrap();
    w.write_bits(1, 1).unwrap();
    w.write_bits(16, 0x1234).unwrap();
    let mut data = w.finish();

    set_loop_filter_bit(&mut data, false);
    let (v, t, q, n, lf, p) = parse(&data);
    assert_eq!(v, u64::from(VERSION));
    assert_eq!(t, u64::from(crate::ftype::P));
    assert_eq!(q, 30);
    assert_eq!(n, 4);
    assert_eq!(lf, 0, "the patched-off bit must be the one the decoder reads");
    assert_eq!(p, 0x1234);

    set_loop_filter_bit(&mut data, true);
    let (_, _, _, _, lf, _) = parse(&data);
    assert_eq!(lf, 1, "and back on");
}

/// A B leaf whose combine mode reads 3 (reserved) is malformed, and the
/// single-sided modes parse exactly the vectors the writer sent.
#[test]
fn b_leaf_combine_modes_parse_and_reject_reserved() {
    use crate::syntax::{read_leaf, LeafCtx, LeafKind};
    use crate::syntax::MVD_K;
    use crate::MV_LIMIT;
    use tpt_media_bitstream::rice::{encode_riced, WriteLike};

    let ctx = LeafCtx {
        intra_frame: false,
        b_frame: true,
        b_weight_past: 2,
        ref_bits: 0,
        nplanes: 1,
    };
    let scans = Scans::new();
    let write = |mode: u64, mv: (i32, i32), mv_b: (i32, i32)| {
        let mut w = PacketBits::new();
        w.write_bits(1, 0).unwrap(); // not skip
        w.write_bits(2, mode).unwrap();
        let vs: &[(i32, i32)] = match mode {
            0 => &[mv, mv_b],
            1 => &[mv],
            2 => &[mv_b],
            _ => &[],
        };
        for v in vs {
            encode_riced(&mut w, MVD_K, i64::from(v.0)).unwrap();
            encode_riced(&mut w, MVD_K, i64::from(v.1)).unwrap();
        }
        w.finish()
    };

    // Reserved mode 3 is rejected before any vector is read.
    let mut bad = PacketBits::new();
    bad.write_bits(1, 0).unwrap();
    bad.write_bits(2, 3).unwrap();
    let bad = bad.finish();
    let mut r = BitReader::new(&bad);
    assert!(read_leaf(&mut r, 8, &ctx, 2, (0, 0), &scans).is_err());

    // Forward-only: mode 1, one vector; decodes as Fwd with mv set and
    // the future vector zero.
    let data = write(1, (10, -4), (0, 0));
    let mut r = BitReader::new(&data);
    let leaf = read_leaf(&mut r, 8, &ctx, 2, (0, 0), &scans).unwrap();
    assert_eq!(leaf.kind, LeafKind::Fwd);
    assert_eq!(leaf.mv, (10, -4));
    assert_eq!(leaf.mv_b, (0, 0));

    // Backward-only: mode 2.
    let data = write(2, (0, 0), (-6, 8));
    let mut r = BitReader::new(&data);
    let leaf = read_leaf(&mut r, 8, &ctx, 2, (0, 0), &scans).unwrap();
    assert_eq!(leaf.kind, LeafKind::Bwd);
    assert_eq!(leaf.mv, (0, 0));
    assert_eq!(leaf.mv_b, (-6, 8));

    // A vector beyond MV_LIMIT is hostile, mode or no mode.
    let data = write(1, (MV_LIMIT + 2, 0), (0, 0));
    let mut r = BitReader::new(&data);
    assert!(read_leaf(&mut r, 8, &ctx, 2, (0, 0), &scans).is_err());
}
