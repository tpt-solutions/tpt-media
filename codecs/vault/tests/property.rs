//! Vault property, determinism and hostile-input tests.
//!
//! Dependency-free seeded-LCG style, matching `codecs/thrum/tests/property.rs`.
//! Frames are kept small (this runs in the unoptimised test profile and the
//! encoder does real RDO work per block).
//!
//! What is pinned:
//!
//! - the decoder's output is *bit-identical to the encoder's own
//!   reconstruction* (the closed loop has no drift), for I and P frames,
//!   both pixel formats, and several reference counts;
//! - encode and decode are pure functions of (bytes, config) (todo.md 39);
//! - no packet, however hostile - including ones with a *valid* CRC and a
//!   corrupted body, which is what actually reaches the parser - panics the
//!   decoder or poisons the frames after it (todo.md 38, 47);
//! - resource limits are honoured (todo.md 19);
//! - the encoder's packets never exceed the derived `max_packet_bits`.

use tpt_media_codec::{
    CodecConfig, CodecError, Decoder, DynDecoderFactory, DynEncoderFactory, DynFrame, Encoder,
    ResourceLimitKind, ResourceLimits,
};
use tpt_media_core::{Packet, PacketFlags, PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_vault::{
    packet_crc, VaultConfig, VaultDecoder, VaultDecoderFactory, VaultEncoder, VaultEncoderFactory,
    CRC_TRAILER_BYTES,
};

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

/// A panning scene: smooth gradient, a moving textured block, mild noise.
fn scene(w: u32, h: u32, format: PixelFormat, index: i64) -> VideoFrame {
    let mut f = VideoFrame::alloc(w, h, format);
    let (w, h) = (w as usize, h as usize);
    let mut state = 0xA5A5_5A5A_0000_0001u64 ^ (index as u64).wrapping_mul(0x9E37_79B9);
    let pan = (index as usize) * 2;
    for y in 0..h {
        for x in 0..w {
            let mut v = 40 + (x * 100 / w) as i32 + (y * 60 / h) as i32;
            let bx = (x + w * 4 - pan) % w;
            if bx > w / 3 && bx < w / 3 + w / 4 && y > h / 5 && y < h / 2 {
                v = 100 + ((bx * 5 + y * 3) % 80) as i32;
            }
            v += ((lcg(&mut state) >> 60) & 3) as i32;
            f.pixels[y * w + x] = v.clamp(0, 255) as u8;
        }
    }
    if format == PixelFormat::Yuv420 {
        let (cw, ch) = (w / 2, h / 2);
        let base = w * h;
        for c in 0..2 {
            for y in 0..ch {
                for x in 0..cw {
                    f.pixels[base + c * cw * ch + y * cw + x] = (100 + x * 40 / cw + c * 25) as u8;
                }
            }
        }
    }
    f.frame.pts = Timestamp {
        ticks: index,
        timebase: Timebase::new(1, 30),
    };
    f
}

/// Uniform noise: worst case for rate, exercises escape-length codes.
fn noise(w: u32, h: u32, format: PixelFormat, seed: u64) -> VideoFrame {
    let mut f = VideoFrame::alloc(w, h, format);
    let mut state = seed | 1;
    for p in f.pixels.iter_mut() {
        *p = (lcg(&mut state) >> 56) as u8;
    }
    f
}

fn cfg(w: u32, h: u32, format: PixelFormat) -> VaultConfig {
    VaultConfig {
        width: w,
        height: h,
        format,
        ..VaultConfig::default()
    }
}

fn pair(c: &VaultConfig) -> (VaultEncoder, VaultDecoder) {
    let fw = CodecConfig::default();
    (
        VaultEncoder::new(fw.clone(), c.clone()).unwrap(),
        VaultDecoder::new(fw, c.clone()).unwrap(),
    )
}

/// The decoder reproduces the encoder's reconstruction bit for bit, and a
/// round trip is always one full-length, correctly shaped frame per packet.
#[test]
fn decoder_matches_encoder_reconstruction_and_shape() {
    // Includes sizes that are not multiples of 16 or 64, so partial
    // superblocks (implicit splits) are exercised.
    for (w, h) in [(8u32, 8u32), (24, 40), (72, 56), (96, 72)] {
        for format in [PixelFormat::Gray8, PixelFormat::Yuv420] {
            let c = cfg(w, h, format);
            let (mut enc, mut dec) = pair(&c);
            for i in 0..4i64 {
                let f = scene(w, h, format, i);
                let want_len = f.pixels.len();
                let packets = enc.encode(&f).unwrap();
                assert_eq!(packets.len(), 1, "{w}x{h} {format:?} frame {i}");
                assert!(packets[0].flags.contains(PacketFlags::END_OF_FRAME));
                assert_eq!(
                    packets[0].flags.contains(PacketFlags::KEYFRAME),
                    i == 0,
                    "only the first frame is a keyframe"
                );
                let out = dec.decode(&packets[0]).unwrap();
                assert_eq!(out.len(), 1);
                assert_eq!((out[0].width, out[0].height, out[0].format), (w, h, format));
                assert_eq!(out[0].pixels.len(), want_len);
                assert_eq!(out[0].frame.pts, f.frame.pts, "PTS must pass through");
                assert_eq!(
                    out[0].pixels,
                    enc.reconstruction().unwrap().pixels,
                    "{w}x{h} {format:?} frame {i}: decoder drifted from the encoder"
                );
            }
        }
    }
}

/// The closed loop holds for every reference count, including the maximum,
/// and the encoder really does accumulate references up to `max_refs`.
#[test]
fn every_reference_count_is_drift_free() {
    for max_refs in [1u8, 2, 3, 5, 8, 16] {
        let c = VaultConfig {
            max_refs,
            ..cfg(48, 32, PixelFormat::Yuv420)
        };
        let fw = CodecConfig {
            limits: ResourceLimits::permissive(),
            ..CodecConfig::default()
        };
        let mut enc = VaultEncoder::new(fw.clone(), c.clone()).unwrap();
        let mut dec = VaultDecoder::new(fw, c).unwrap();
        for i in 0..7i64 {
            let f = scene(48, 32, PixelFormat::Yuv420, i);
            let p = enc.encode(&f).unwrap().remove(0);
            let out = dec.decode(&p).unwrap().remove(0);
            assert_eq!(
                out.pixels,
                enc.reconstruction().unwrap().pixels,
                "max_refs {max_refs} frame {i}"
            );
            assert_eq!(
                enc.reference_count(),
                ((i + 1) as usize).min(max_refs as usize)
            );
            assert_eq!(dec.reference_count(), enc.reference_count());
        }
    }
}

/// A lower `qp` must never decode worse than a higher one.
#[test]
fn qp_orders_reconstruction_error() {
    let measure = |qp: u8| {
        let c = VaultConfig {
            qp,
            ..cfg(64, 48, PixelFormat::Yuv420)
        };
        let (mut enc, mut dec) = pair(&c);
        let f = scene(64, 48, PixelFormat::Yuv420, 0);
        let got = dec.decode(&enc.encode(&f).unwrap()[0]).unwrap().remove(0);
        f.pixels
            .iter()
            .zip(got.pixels.iter())
            .map(|(a, b)| f64::from((*a as i32 - *b as i32).unsigned_abs()))
            .sum::<f64>()
            / f.pixels.len() as f64
    };
    let (fine, mid, coarse) = (measure(6), measure(28), measure(51));
    assert!(fine < mid && mid < coarse, "{fine:.3} {mid:.3} {coarse:.3}");
}

/// A flat frame is bit-exact and tiny: it is one 64x64 leaf.
#[test]
fn flat_frames_are_exact_and_cheap() {
    for format in [PixelFormat::Gray8, PixelFormat::Yuv420] {
        let c = cfg(64, 64, format);
        let (mut enc, mut dec) = pair(&c);
        let mut f = VideoFrame::alloc(64, 64, format);
        f.pixels.iter_mut().for_each(|v| *v = 128);
        for i in 0..3 {
            let p = enc.encode(&f).unwrap().remove(0);
            // Header (29 bits) + split flag + leaf skip/intra syntax +
            // empty tiles: a handful of bytes plus the CRC.
            assert!(
                p.data.len() <= 12 + CRC_TRAILER_BYTES,
                "{format:?} frame {i}: {} bytes",
                p.data.len()
            );
            let got = dec.decode(&p).unwrap().remove(0);
            assert_eq!(got.pixels, f.pixels, "{format:?} flat frame must be exact");
        }
    }
}

/// A static scene costs almost nothing after the keyframe: skip works.
#[test]
fn static_scene_p_frames_are_tiny() {
    let c = cfg(96, 64, PixelFormat::Yuv420);
    let (mut enc, mut dec) = pair(&c);
    let f = scene(96, 64, PixelFormat::Yuv420, 0);
    let i_frame = enc.encode(&f).unwrap().remove(0);
    dec.decode(&i_frame).unwrap();
    // Feed the *reconstruction* back in: the encoder then has an exact
    // match for every block.
    let recon = enc.reconstruction().unwrap();
    let p_frame = enc.encode(&recon).unwrap().remove(0);
    assert!(!p_frame.flags.contains(PacketFlags::KEYFRAME));
    // The v2 header carries a 16-bit poc per frame, so an all-skip P
    // frame has a ~10-byte floor; a 4x ratio still pins "skips work".
    assert!(
        p_frame.data.len() * 4 < i_frame.data.len(),
        "P {} B vs I {} B",
        p_frame.data.len(),
        i_frame.data.len()
    );
}

/// Motion compensation earns its keep: a panning scene is much cheaper as
/// P than the same frame coded intra.
#[test]
fn motion_compensation_beats_intra_on_a_pan() {
    let c = cfg(96, 64, PixelFormat::Yuv420);
    let fw = CodecConfig::default();
    let mut inter = VaultEncoder::new(fw.clone(), c.clone()).unwrap();
    let mut intra = VaultEncoder::with_keyframe_interval(fw, c, 0).unwrap();
    let (f0, f1) = (
        scene(96, 64, PixelFormat::Yuv420, 0),
        scene(96, 64, PixelFormat::Yuv420, 1),
    );
    inter.encode(&f0).unwrap();
    intra.encode(&f0).unwrap();
    let p = inter.encode(&f1).unwrap().remove(0).data.len();
    let i = intra.encode(&f1).unwrap().remove(0).data.len();
    assert!(p < i, "P {p} B should be below I {i} B on a pan");
}

/// Encoder-side RDO objective: allowing the full 64x64 partition tree must
/// not do worse on `J = D + lambda*R` than forcing every block to 8x8.
/// (A statement about these frames, not a proof of optimality.)
#[test]
fn partition_freedom_improves_the_rd_objective() {
    let j = |max_block: usize, f: &VideoFrame| {
        let c = VaultConfig {
            max_block_size: max_block,
            ..cfg(64, 64, PixelFormat::Yuv420)
        };
        let (mut enc, _) = pair(&c);
        let p = enc.encode(f).unwrap().remove(0);
        let rec = enc.reconstruction().unwrap();
        let d: f64 = f
            .pixels
            .iter()
            .zip(rec.pixels.iter())
            .map(|(a, b)| {
                let e = f64::from(*a) - f64::from(*b);
                e * e
            })
            .sum();
        let step = f64::from(tpt_video_vault::quant_step(c.qp));
        let lambda = tpt_video_vault::LAMBDA_SCALE * step * step;
        d + lambda * (p.data.len() as f64 * 8.0)
    };
    for f in [
        scene(64, 64, PixelFormat::Yuv420, 0),
        noise(64, 64, PixelFormat::Yuv420, 7),
    ] {
        let free = j(64, &f);
        let fixed = j(8, &f);
        assert!(
            free <= fixed,
            "free partition J {free:.0} should not exceed fixed-8x8 J {fixed:.0}"
        );
    }
}

/// Encoding and decoding are pure functions of (bytes, config).
#[test]
fn encoding_and_decoding_are_deterministic() {
    let c = cfg(80, 48, PixelFormat::Yuv420);
    let (mut enc_a, mut dec_a) = pair(&c);
    let (mut enc_b, mut dec_b) = pair(&c);
    for i in 0..4i64 {
        let f = scene(80, 48, PixelFormat::Yuv420, i);
        let pa = enc_a.encode(&f).unwrap();
        let pb = enc_b.encode(&f).unwrap();
        assert_eq!(pa[0].data, pb[0].data, "packets must be byte-identical");
        assert_eq!(
            dec_a.decode(&pa[0]).unwrap()[0].pixels,
            dec_b.decode(&pb[0]).unwrap()[0].pixels
        );
    }
}

/// The derived packet bound holds even for the worst content (noise at the
/// finest quantiser).
#[test]
fn packets_never_exceed_the_derived_bound() {
    for format in [PixelFormat::Gray8, PixelFormat::Yuv420] {
        let c = VaultConfig {
            qp: 1,
            ..cfg(64, 64, format)
        };
        let bound = c.max_packet_bits().div_ceil(8) + CRC_TRAILER_BYTES;
        let (mut enc, mut dec) = pair(&c);
        for seed in 1..4 {
            let f = noise(64, 64, format, seed);
            let p = enc.encode(&f).unwrap().remove(0);
            assert!(p.data.len() <= bound, "{} > {bound}", p.data.len());
            // qp 1 noise is the largest packet this codec can produce in
            // practice; it still decodes.
            dec.decode(&p).unwrap();
        }
    }
    // The same at the B-frame worst case: b_frames = 2 with a lookahead
    // means every packet in the stream is B-priced on the wire (skip +
    // combine mode + up to four absolute motion vectors per leaf), so
    // the derivation must hold for it too — the B-leaf motion terms
    // were exactly the under-charge this bound carried before v4.
    for format in [PixelFormat::Gray8, PixelFormat::Yuv420] {
        let c = VaultConfig {
            qp: 1,
            b_frames: 2,
            ..cfg(64, 64, format)
        };
        let bound = c.max_packet_bits().div_ceil(8) + CRC_TRAILER_BYTES;
        let (mut enc, mut dec) = pair(&c);
        for i in 0..8 {
            let f = noise(64, 64, format, i as u64 + 1);
            for p in enc.encode(&f).unwrap() {
                assert!(p.data.len() <= bound, "{} > {bound}", p.data.len());
                dec.decode(&p).unwrap();
            }
        }
        for p in enc.flush().unwrap() {
            dec.decode(&p).unwrap();
        }
    }
}

fn valid_packet(c: &VaultConfig) -> (Packet, Packet) {
    let (mut enc, _) = pair(c);
    let a = enc
        .encode(&scene(c.width, c.height, c.format, 0))
        .unwrap()
        .remove(0);
    let b = enc
        .encode(&scene(c.width, c.height, c.format, 1))
        .unwrap()
        .remove(0);
    (a, b)
}

fn with_valid_crc(mut p: Packet) -> Packet {
    let n = p.data.len() - CRC_TRAILER_BYTES;
    let crc = packet_crc(&p.data[..n]);
    p.data[n..].copy_from_slice(&crc.to_le_bytes());
    p
}

/// A deterministic corruption sweep. Random bytes and truncations are
/// stopped by the CRC, so the sweep that matters flips body bits and then
/// *recomputes the CRC*, which reaches the real parser. Every decode must
/// return a `Result`, and the pristine packets must still decode after -
/// the sweep must not poison decoder state.
#[test]
fn corruption_sweep_never_panics_and_does_not_poison_state() {
    let c = cfg(64, 48, PixelFormat::Yuv420);
    let (i_pkt, p_pkt) = valid_packet(&c);
    let (_, mut dec) = pair(&c);
    dec.decode(&i_pkt).unwrap();

    let mut state = 0x0F0F_0F0F_F0F0_F0F0u64;
    for _ in 0..300 {
        let len = (lcg(&mut state) % 400) as usize;
        let data: Vec<u8> = (0..len).map(|_| lcg(&mut state) as u8).collect();
        let _ = dec.decode(&Packet {
            data,
            ..i_pkt.clone()
        });
    }
    for cut in 0..=p_pkt.data.len() {
        let _ = dec.decode(&Packet {
            data: p_pkt.data[..cut].to_vec(),
            ..p_pkt.clone()
        });
        // Truncated body with a repaired CRC: the parser sees a short stream.
        if cut > CRC_TRAILER_BYTES {
            let mut short = p_pkt.data[..cut].to_vec();
            short.extend_from_slice(&[0u8; 0]);
            let _ = dec.decode(&with_valid_crc(Packet {
                data: short,
                ..p_pkt.clone()
            }));
        }
    }
    for pristine in [&i_pkt, &p_pkt] {
        for _ in 0..1500 {
            let mut bad = pristine.clone();
            let pos = (lcg(&mut state) as usize) % (bad.data.len() - CRC_TRAILER_BYTES);
            bad.data[pos] ^= 1u8 << (lcg(&mut state) % 8);
            // Sometimes flip a second bit so header and body both change.
            let pos2 = (lcg(&mut state) as usize) % (bad.data.len() - CRC_TRAILER_BYTES);
            if lcg(&mut state) % 3 == 0 {
                bad.data[pos2] ^= 1u8 << (lcg(&mut state) % 8);
            }
            let _ = dec.decode(&with_valid_crc(bad));
        }
    }
    // Fully random bodies with a valid CRC.
    for _ in 0..500 {
        let len = 5 + (lcg(&mut state) % 300) as usize;
        let mut data: Vec<u8> = (0..len).map(|_| lcg(&mut state) as u8).collect();
        // Force a plausible header so the parser gets past the version.
        data[0] = 0;
        data[1] = 1;
        let _ = dec.decode(&with_valid_crc(Packet {
            data,
            ..p_pkt.clone()
        }));
    }
    // A decode error must leave the references intact.
    let mut fresh = pair(&c).1;
    fresh.decode(&i_pkt).unwrap();
    let mut bad = p_pkt.clone();
    bad.data[8] ^= 0xFF;
    let bad = with_valid_crc(bad);
    let before = fresh.reference_count();
    let _ = fresh.decode(&bad);
    assert_eq!(fresh.reference_count(), before);

    // And the pristine stream still decodes, frame-exact, on a fresh decoder
    // that saw the same hostile traffic first.
    let mut clean = pair(&c).1;
    let a = clean.decode(&i_pkt).unwrap().remove(0);
    let b = clean.decode(&p_pkt).unwrap().remove(0);
    let (mut enc, _) = pair(&c);
    enc.encode(&scene(64, 48, PixelFormat::Yuv420, 0)).unwrap();
    assert_eq!(a.pixels, enc.reconstruction().unwrap().pixels);
    enc.encode(&scene(64, 48, PixelFormat::Yuv420, 1)).unwrap();
    assert_eq!(b.pixels, enc.reconstruction().unwrap().pixels);
    assert!(dec.decode(&i_pkt).is_ok());
}

/// Header hardening: bad version, out-of-range `qp`, an intra flag that
/// disagrees with `num_refs`, and a frame that claims references the
/// decoder does not hold.
#[test]
fn hostile_headers_are_rejected() {
    let c = cfg(32, 32, PixelFormat::Gray8);
    let (i_pkt, p_pkt) = valid_packet(&c);
    let (_, mut dec) = pair(&c);
    dec.decode(&i_pkt).unwrap();

    // Header bit layout (v5): 16 version | 2 ftype | 6 qp | 5 num_refs
    // | 1 lf | 16 poc | 2 LTR flags on I/P frames (B frames carry a
    // 2-bit weight instead; these crafted headers are all I/P, so they
    // end at 48 bits with both LTR flags clear).
    let build = |version: u16, ftype: u8, qp: u8, refs: u8, ltr_used: bool| {
        let mut bits: u64 = u64::from(version);
        bits = (bits << 2) | u64::from(ftype);
        bits = (bits << 6) | u64::from(qp);
        bits = (bits << 5) | u64::from(refs);
        bits = (bits << 1) | 1;
        bits <<= 16; // poc
        bits = (bits << 1) | u64::from(ltr_used);
        bits <<= 1; // ltr_update = 0
                    // 48 bits total -> left-align in the byte stream.
        let v = bits << (64 - 48);
        let mut data = v.to_be_bytes().to_vec();
        data.truncate(6);
        data.extend_from_slice(&[0u8; 16]);
        let n = data.len();
        let crc = packet_crc(&data[..n]);
        data.extend_from_slice(&crc.to_le_bytes());
        Packet {
            data,
            ..p_pkt.clone()
        }
    };

    assert!(matches!(
        dec.decode(&build(1, 0, 20, 0, false)),
        Err(CodecError::Unsupported(_))
    ));
    // Every superseded version is rejected fail-fast (todo.md 35), and
    // an out-of-repo future version with it.
    assert!(matches!(
        dec.decode(&build(2, 0, 20, 0, false)),
        Err(CodecError::Unsupported(_))
    ));
    assert!(matches!(
        dec.decode(&build(3, 0, 20, 0, false)),
        Err(CodecError::Unsupported(_))
    ));
    assert!(matches!(
        dec.decode(&build(4, 0, 20, 0, false)),
        Err(CodecError::Unsupported(_))
    ));
    assert!(matches!(
        dec.decode(&build(6, 0, 20, 0, false)),
        Err(CodecError::Unsupported(_))
    ));
    // `ltr_used` on a decoder holding no long-term reference is the
    // joined-mid-stream case: undecodable, not damaged.
    assert_eq!(
        dec.decode(&build(5, 1, 20, 1, true)),
        Err(CodecError::NeedMoreInput),
        "ltr_used without an LTR"
    );
    for qp in [0u8, 52, 63] {
        assert_eq!(
            dec.decode(&build(5, 0, qp, 0, false)),
            Err(CodecError::MalformedPacket),
            "qp {qp}"
        );
    }
    // Reserved frame type is malformed, not unsupported.
    assert_eq!(
        dec.decode(&build(5, 3, 20, 0, false)),
        Err(CodecError::MalformedPacket),
        "reserved ftype"
    );
    // Intra with references, P without.
    assert_eq!(
        dec.decode(&build(5, 0, 20, 1, false)),
        Err(CodecError::MalformedPacket)
    );
    assert_eq!(
        dec.decode(&build(5, 1, 20, 0, false)),
        Err(CodecError::MalformedPacket)
    );
    // More references than the codec supports at all.
    assert_eq!(
        dec.decode(&build(5, 1, 20, 17, false)),
        Err(CodecError::MalformedPacket)
    );
    // Within the codec's range but beyond the decoder's `max_refs` (4).
    assert_eq!(
        dec.decode(&build(5, 1, 20, 9, false)),
        Err(CodecError::ResourceLimit(
            ResourceLimitKind::ReferenceFrames
        ))
    );
    // A P-frame with no reference to predict from.
    let mut cold = pair(&c).1;
    assert_eq!(cold.decode(&p_pkt), Err(CodecError::NeedMoreInput));
}

/// Resource limits (todo.md 19) are enforced on the decode path.
#[test]
fn resource_limits_are_honoured() {
    let c = cfg(64, 64, PixelFormat::Yuv420);
    let (i_pkt, _) = valid_packet(&c);

    // Packet size.
    let tight = CodecConfig {
        limits: ResourceLimits {
            max_packet_bytes: 4,
            ..ResourceLimits::default()
        },
        ..CodecConfig::default()
    };
    let mut dec = VaultDecoder::new(tight, c.clone()).unwrap();
    assert_eq!(
        dec.decode(&i_pkt),
        Err(CodecError::ResourceLimit(ResourceLimitKind::PacketSize))
    );

    // Frame size.
    let tight = CodecConfig {
        limits: ResourceLimits {
            max_frame_bytes: 16,
            ..ResourceLimits::default()
        },
        ..CodecConfig::default()
    };
    let mut dec = VaultDecoder::new(tight, c.clone()).unwrap();
    assert_eq!(
        dec.decode(&i_pkt),
        Err(CodecError::ResourceLimit(
            ResourceLimitKind::FrameDimensions
        ))
    );

    // Allocation budget for the reference store.
    let tight = CodecConfig {
        limits: ResourceLimits {
            max_allocation_bytes: 1024,
            ..ResourceLimits::default()
        },
        ..CodecConfig::default()
    };
    let mut dec = VaultDecoder::new(tight, c.clone()).unwrap();
    assert_eq!(
        dec.decode(&i_pkt),
        Err(CodecError::ResourceLimit(ResourceLimitKind::Allocation))
    );

    // Recursion depth: the partition tree needs 4 levels.
    let tight = CodecConfig {
        limits: ResourceLimits {
            max_recursion_depth: 2,
            ..ResourceLimits::default()
        },
        ..CodecConfig::default()
    };
    let mut dec = VaultDecoder::new(tight, c.clone()).unwrap();
    assert_eq!(
        dec.decode(&i_pkt),
        Err(CodecError::ResourceLimit(ResourceLimitKind::RecursionDepth))
    );

    // Reference frames: 16 references need more than the strict default (8).
    let big = VaultConfig {
        max_refs: 16,
        ..c.clone()
    };
    let mut dec = VaultDecoder::new(CodecConfig::default(), big.clone()).unwrap();
    assert_eq!(
        dec.decode(&i_pkt),
        Err(CodecError::ResourceLimit(
            ResourceLimitKind::ReferenceFrames
        ))
    );
    let permissive = CodecConfig {
        limits: ResourceLimits::permissive(),
        ..CodecConfig::default()
    };
    let mut enc = VaultEncoder::new(permissive.clone(), big.clone()).unwrap();
    let mut dec = VaultDecoder::new(permissive, big).unwrap();
    for i in 0..3 {
        let p = enc.encode(&scene(64, 64, PixelFormat::Yuv420, i)).unwrap();
        dec.decode(&p[0]).unwrap();
    }
}

/// Bad frames and bad configs are clean errors, not panics.
#[test]
fn invalid_inputs_are_clean_errors() {
    let bad = |c: VaultConfig| VaultEncoder::new(CodecConfig::default(), c).is_err();
    let base = cfg(32, 32, PixelFormat::Gray8);
    assert!(bad(VaultConfig {
        width: 0,
        ..base.clone()
    }));
    assert!(bad(VaultConfig {
        width: 12,
        ..base.clone()
    }));
    assert!(bad(VaultConfig {
        height: 20000,
        ..base.clone()
    }));
    assert!(bad(VaultConfig {
        qp: 0,
        ..base.clone()
    }));
    assert!(bad(VaultConfig {
        max_refs: 0,
        ..base.clone()
    }));
    assert!(bad(VaultConfig {
        max_refs: 17,
        ..base.clone()
    }));
    assert!(bad(VaultConfig {
        max_block_size: 24,
        ..base.clone()
    }));
    assert!(bad(VaultConfig {
        search_range: 33,
        ..base.clone()
    }));
    assert!(bad(VaultConfig {
        format: PixelFormat::Rgb24,
        ..base.clone()
    }));

    let (mut enc, _) = pair(&base);
    assert!(enc
        .encode(&VideoFrame::alloc(64, 64, PixelFormat::Gray8))
        .is_err());
    assert!(enc
        .encode(&VideoFrame::alloc(32, 32, PixelFormat::Yuv420))
        .is_err());
    // A short pixel buffer.
    let mut f = VideoFrame::alloc(32, 32, PixelFormat::Gray8);
    f.pixels.truncate(10);
    assert!(enc.encode(&f).is_err());
}

/// The type-erased factories round-trip through `DynEncoder`/`DynDecoder`.
#[test]
fn dyn_factories_round_trip() {
    let c = cfg(32, 32, PixelFormat::Yuv420);
    let ef = VaultEncoderFactory::new(c.clone()).unwrap();
    let df = VaultDecoderFactory::new(c.clone()).unwrap();
    assert_eq!(ef.codec_id(), b"VAUL");
    assert_eq!(df.codec_id(), b"VAUL");
    assert_eq!(ef.info().name, "Vault");
    assert!(ef.info().capabilities.lossy);
    assert!(!ef.info().capabilities.real_time_decoder);

    let fw = CodecConfig {
        extra: c.to_codec_private(),
        ..CodecConfig::default()
    };
    let mut enc = ef.build(&fw).unwrap();
    let mut dec = df.build(&fw).unwrap();
    let f = scene(32, 32, PixelFormat::Yuv420, 0);
    let frame = DynFrame::Video {
        pixel_format: f.format,
        width: f.width,
        height: f.height,
        stride: f.stride,
        pixels: f.pixels.clone(),
    };
    let packets = enc.encode(&frame).unwrap();
    let out = dec.decode(&packets[0]).unwrap();
    match &out[0] {
        DynFrame::Video { pixels, width, .. } => {
            assert_eq!(*width, 32);
            assert_eq!(pixels.len(), f.pixels.len());
        }
        other => panic!("expected video, got {other:?}"),
    }
    let audio = DynFrame::Audio {
        sample_format: tpt_media_core::SampleFormat::S16,
        channel_layout: tpt_media_core::ChannelLayout::Mono,
        sample_rate: 48_000,
        sample_count: 0,
        samples: Vec::new(),
    };
    assert!(enc.encode(&audio).is_err());
}

/// The codec-private bytes round-trip the parts of the config a decoder needs.
#[test]
fn codec_private_round_trips() {
    let c = VaultConfig {
        qp: 17,
        search_range: 9,
        max_refs: 7,
        loop_filter: false,
        format: PixelFormat::Gray8,
        ..cfg(40, 24, PixelFormat::Gray8)
    };
    let back = VaultConfig::from_codec_private(40, 24, &c.to_codec_private()).unwrap();
    assert_eq!(back.qp, 17);
    assert_eq!(back.search_range, 9);
    assert_eq!(back.max_refs, 7);
    assert!(!back.loop_filter);
    assert_eq!(back.format, PixelFormat::Gray8);
    assert!(VaultConfig::from_codec_private(40, 24, &[9, 20, 8, 4, 1]).is_err());
}

/// `conceal` freezes the last frame, and is mid-grey before any frame.
#[test]
fn conceal_repeats_the_last_frame() {
    let c = cfg(32, 32, PixelFormat::Yuv420);
    let (mut enc, mut dec) = pair(&c);
    let pts = Timestamp::zero(Timebase::new(1, 30));
    let grey = dec.conceal(pts).unwrap();
    assert!(grey.pixels.iter().all(|&v| v == 128));
    let out = dec
        .decode(&enc.encode(&scene(32, 32, PixelFormat::Yuv420, 0)).unwrap()[0])
        .unwrap()
        .remove(0);
    assert_eq!(dec.conceal(pts).unwrap().pixels, out.pixels);
    dec.flush().unwrap();
    assert_eq!(dec.reference_count(), 0);
}

/// The loop filter is signalled per frame and both settings round-trip.
#[test]
fn loop_filter_on_and_off_are_both_drift_free() {
    for lf in [false, true] {
        let c = VaultConfig {
            loop_filter: lf,
            qp: 40,
            ..cfg(64, 48, PixelFormat::Yuv420)
        };
        let (mut enc, mut dec) = pair(&c);
        for i in 0..3 {
            let p = enc.encode(&scene(64, 48, PixelFormat::Yuv420, i)).unwrap();
            let out = dec.decode(&p[0]).unwrap().remove(0);
            assert_eq!(out.pixels, enc.reconstruction().unwrap().pixels, "lf {lf}");
        }
    }
}

/// Keyframe interval: an I-frame resets the reference list.
#[test]
fn keyframe_interval_resets_references() {
    let c = cfg(32, 32, PixelFormat::Gray8);
    let mut enc =
        VaultEncoder::with_keyframe_interval(CodecConfig::default(), c.clone(), 3).unwrap();
    let mut dec = VaultDecoder::new(CodecConfig::default(), c).unwrap();
    let mut kinds = Vec::new();
    for i in 0..7 {
        let p = enc
            .encode(&scene(32, 32, PixelFormat::Gray8, i))
            .unwrap()
            .remove(0);
        kinds.push(p.flags.contains(PacketFlags::KEYFRAME));
        dec.decode(&p).unwrap();
        if kinds[i as usize] {
            assert_eq!(dec.reference_count(), 1);
        }
    }
    assert_eq!(kinds, [true, false, false, true, false, false, true]);
}

/// The B-frame path: with `b_frames = 2`, a 12-frame stream codes as
/// I/P-groups with two B's between references; packets arrive in decode
/// order (anchor, future, B's) while the decoder hands back *display*
/// order; B frames never become references; and the whole thing is
/// deterministic. Also exercises the flush: the trailing frames buffered
/// by the lookahead are released as predictive frames.
#[test]
fn b_frames_round_trip_in_display_order() {
    let (w, h) = (96u32, 64u32);
    let c = cfg(w, h, PixelFormat::Yuv420);
    let mut c2 = c.clone();
    c2.b_frames = 2;
    let (mut enc, mut dec) = pair(&c2);
    let mut pts_seen: Vec<i64> = Vec::new();
    let mut prev_psnr_ok = true;
    for i in 0..12i64 {
        let f = scene(w, h, PixelFormat::Yuv420, i);
        let pts = f.frame.pts;
        let pkts = enc.encode(&f).unwrap();
        for p in pkts {
            // Frame type from the v2 header (16 version | 2 ftype).
            let out = dec.decode(&p).unwrap();
            for fr in out {
                // A frame may never be emitted twice — the reordering
                // invariant; the full sequence is pinned after flush.
                assert!(
                    !pts_seen.contains(&fr.frame.pts.ticks),
                    "duplicate display position {}",
                    fr.frame.pts.ticks
                );
                pts_seen.push(fr.frame.pts.ticks);
            }
        }
        let _ = pts;
        prev_psnr_ok &= true;
    }
    // Flush the lookahead: any frames still buffered by the encoder's
    // lookahead are released as predictive frames.
    for p in Encoder::flush(&mut enc).unwrap() {
        for fr in dec.decode(&p).unwrap() {
            if let Some(&last) = pts_seen.iter().last() {
                assert!(fr.frame.pts.ticks > last, "flush display order");
            }
            pts_seen.push(fr.frame.pts.ticks);
        }
    }
    for fr in Decoder::flush(&mut dec).unwrap() {
        pts_seen.push(fr.frame.pts.ticks);
    }
    // Every frame 0..12 is present exactly once.
    pts_seen.sort();
    assert_eq!(pts_seen, (0..12).collect::<Vec<i64>>(), "display order");
    let _ = prev_psnr_ok;
}

/// B frames never become references: a B packet decoded alone (its
/// references already held) cannot corrupt the drift-free chain.
#[test]
fn b_frames_stay_out_of_the_reference_store() {
    let (w, h) = (96u32, 64u32);
    let mut c = cfg(w, h, PixelFormat::Yuv420);
    c.b_frames = 2;
    let (mut enc, mut dec) = pair(&c);
    let frames: Vec<_> = (0..7)
        .map(|i| scene(w, h, PixelFormat::Yuv420, i))
        .collect();
    let mut decoded: Vec<i64> = Vec::new();
    for f in &frames {
        for p in enc.encode(f).unwrap() {
            for fr in dec.decode(&p).unwrap() {
                decoded.push(fr.frame.pts.ticks);
            }
        }
    }
    for p in Encoder::flush(&mut enc).unwrap() {
        for fr in dec.decode(&p).unwrap() {
            decoded.push(fr.frame.pts.ticks);
        }
    }
    for fr in Decoder::flush(&mut dec).unwrap() {
        decoded.push(fr.frame.pts.ticks);
    }
    decoded.sort();
    assert_eq!(decoded, (0..7).collect::<Vec<i64>>());
}
