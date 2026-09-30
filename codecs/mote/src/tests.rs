//! Unit tests. Frames are tiny and dictionaries small so these stay fast
//! in the unoptimised test profile.

use super::*;
use tpt_media_codec::{DynDecoderFactory, DynEncoderFactory, ResourceLimitKind};
use tpt_media_core::Timebase;

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state >> 33
}

fn pts(i: i64) -> Timestamp {
    Timestamp {
        ticks: i,
        timebase: Timebase::new(1, 10),
    }
}

/// A deterministic scene: gradient + block texture + a bright square that
/// moves with `i`. Chroma is a slow ramp.
fn scene(w: u32, h: u32, format: PixelFormat, i: i64) -> VideoFrame {
    let mut f = VideoFrame::alloc(w, h, format);
    let (wu, hu) = (w as usize, h as usize);
    let sq = (i as usize * 8) % (wu - 16);
    for y in 0..hu {
        for x in 0..wu {
            let mut v = ((x * 3 + y * 2) % 180 + 20 + ((x / 8 + y / 8) % 3) * 12) as u8;
            if (sq..sq + 16).contains(&x) && (8..24).contains(&y) {
                v = 240;
            }
            f.pixels[y * wu + x] = v;
        }
    }
    if format == PixelFormat::Yuv420 {
        let (cw, ch) = (wu / 2, hu / 2);
        for c in 0..2 {
            for y in 0..ch {
                for x in 0..cw {
                    f.pixels[wu * hu + c * cw * ch + y * cw + x] = (100 + x + y * (c + 1)) as u8;
                }
            }
        }
    }
    f.frame.pts = pts(i);
    f
}

fn noise_frame(w: u32, h: u32, format: PixelFormat, seed: u64) -> VideoFrame {
    let mut f = VideoFrame::alloc(w, h, format);
    let mut s = seed;
    for p in f.pixels.iter_mut() {
        *p = lcg(&mut s) as u8;
    }
    f
}

fn cfg(w: u32, h: u32, format: PixelFormat) -> MoteConfig {
    MoteConfig {
        width: w,
        height: h,
        format,
        ..MoteConfig::default()
    }
}

fn train(frames: &[VideoFrame], entries: usize) -> Dictionary {
    Dictionary::train_from_frames(
        frames,
        &TrainConfig {
            entries,
            iterations: 4,
            ..TrainConfig::default()
        },
    )
    .unwrap()
}

/// Encoder + decoder over a shared, embedded, trained dictionary.
fn pair(w: u32, h: u32, format: PixelFormat, entries: usize) -> (MoteEncoder, MoteDecoder) {
    let train_frames: Vec<_> = (0..4).map(|i| scene(w, h, format, i)).collect();
    let dict = train(&train_frames, entries);
    let c = cfg(w, h, format);
    let enc = MoteEncoder::with_dictionary(CodecConfig::default(), c.clone(), dict.clone(), false)
        .unwrap();
    let dec = MoteDecoder::with_dictionary(CodecConfig::default(), c, dict).unwrap();
    (enc, dec)
}

/// Build a raw packet body with `f`, then append the CRC.
fn packet_from(f: impl FnOnce(&mut BitWriter)) -> Packet {
    let mut buf = vec![0u8; 4096];
    let n = {
        let mut w = BitWriter::new(&mut buf);
        f(&mut w);
        w.finish().unwrap()
    };
    let mut data = buf[..n].to_vec();
    let crc = crc32(&data);
    data.extend_from_slice(&crc.to_le_bytes());
    Packet {
        data,
        pts: pts(0),
        dts: pts(0),
        flags: PacketFlags::empty(),
        track_id: 0,
        duration: None,
    }
}

fn three_entry_dictionary() -> Dictionary {
    let mut e = Vec::new();
    for v in [10u8, 100, 200] {
        e.extend(core::iter::repeat_n(v, PATCH_DIM));
    }
    Dictionary::from_entries(e).unwrap()
}

#[test]
fn crc32_matches_the_reference_vector() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(packet_crc(b"123456789"), 0xCBF4_3926);
}

#[test]
fn index_bits_tracks_dictionary_size() {
    assert_eq!(index_bits(1), 1);
    assert_eq!(index_bits(2), 1);
    assert_eq!(index_bits(3), 2);
    assert_eq!(index_bits(4), 2);
    assert_eq!(index_bits(5), 3);
    assert_eq!(index_bits(4095), 12);
    assert_eq!(index_bits(4096), 12);
    assert_eq!(MAX_INDEX_BITS, index_bits(MAX_DICTIONARY_ENTRIES));
}

#[test]
fn dictionary_validation() {
    assert!(Dictionary::from_entries(vec![]).is_err());
    assert!(Dictionary::from_entries(vec![0; 63]).is_err());
    assert!(Dictionary::from_entries(vec![0; 64]).is_ok());
    assert!(Dictionary::from_entries(vec![0; 64 * 4096]).is_ok());
    assert!(Dictionary::from_entries(vec![0; 64 * 4097]).is_err());
    let d = three_entry_dictionary();
    assert_eq!(d.len(), 3);
    assert_eq!(d.index_bits(), 2);
    assert_eq!(d.entry(1), &[100u8; 64][..]);
    // The id depends on content.
    let mut e = d.as_bytes().to_vec();
    e[0] ^= 1;
    assert_ne!(Dictionary::from_entries(e).unwrap().id(), d.id());
}

#[test]
fn training_is_deterministic_and_beats_a_naive_dictionary() {
    let frames: Vec<_> = (0..3)
        .map(|i| scene(64, 48, PixelFormat::Gray8, i))
        .collect();
    let a = train(&frames, 24);
    let b = train(&frames, 24);
    assert_eq!(a, b, "same input and seed must give the same bytes");

    // Baseline: the first 24 training patches used directly.
    let pic = Picture::from_frame(&frames[0]).unwrap();
    let grid = PatchGrid::new(64, 48, PixelFormat::Gray8);
    let all = collect_patches(&pic, &grid);
    let naive = Dictionary::from_entries(all[..24 * PATCH_DIM].to_vec()).unwrap();
    let cost = |d: &Dictionary| -> u64 {
        let df = d.to_f32();
        all.chunks(PATCH_DIM)
            .map(|p| {
                let q: Vec<f32> = p.iter().map(|&x| f32::from(x)).collect();
                nearest_vector_accelerated(&df, PATCH_DIM, &q, DistanceMetric::SquaredEuclidean).1
                    as u64
            })
            .sum()
    };
    // Trained on all three frames, evaluated on the first.
    assert!(
        cost(&a) < cost(&naive),
        "trained {} vs naive {}",
        cost(&a),
        cost(&naive)
    );
}

#[test]
fn a_different_seed_changes_the_dictionary() {
    let frames = [scene(64, 48, PixelFormat::Gray8, 0)];
    let mk = |seed| {
        Dictionary::train_from_frames(
            &frames,
            &TrainConfig {
                entries: 16,
                iterations: 3,
                seed,
                ..TrainConfig::default()
            },
        )
        .unwrap()
    };
    assert_ne!(mk(1), mk(2));
    assert_eq!(mk(1), mk(1));
}

#[test]
fn training_clamps_entries_to_the_patches_available() {
    // 32x32 gray = 16 patches.
    let d = train(&[scene(32, 32, PixelFormat::Gray8, 0)], 4096);
    assert_eq!(d.len(), 16);
}

#[test]
fn training_rejects_bad_input() {
    let tc = TrainConfig::default();
    assert!(Dictionary::train_patches(&[], &tc).is_err());
    assert!(Dictionary::train_patches(&[0; 65], &tc).is_err());
    assert!(Dictionary::train_from_frames(&[], &tc).is_err());
    let bad = TrainConfig {
        entries: 0,
        ..TrainConfig::default()
    };
    assert!(Dictionary::train_patches(&[0; 64], &bad).is_err());
    let mixed = [
        scene(32, 32, PixelFormat::Gray8, 0),
        scene(48, 32, PixelFormat::Gray8, 0),
    ];
    assert!(Dictionary::train_from_frames(&mixed, &tc).is_err());
}

#[test]
fn training_subsamples_deterministically_above_max_vectors() {
    let f = [scene(64, 48, PixelFormat::Gray8, 0)];
    let tc = TrainConfig {
        entries: 8,
        iterations: 2,
        max_vectors: 20,
        ..TrainConfig::default()
    };
    let a = Dictionary::train_from_frames(&f, &tc).unwrap();
    assert_eq!(a, Dictionary::train_from_frames(&f, &tc).unwrap());
    assert_eq!(a.len(), 8);
}

#[test]
fn dimension_rules() {
    let ok = |w, h| cfg(w, h, PixelFormat::Gray8).validate().is_ok();
    assert!(ok(16, 16));
    assert!(ok(320, 240));
    assert!(ok(640, 480));
    assert!(ok(848, 480), "480p 16:9 rounded to a multiple of 16");
    assert!(ok(480, 848), "portrait orientation");
    assert!(!ok(0, 16));
    assert!(!ok(17, 16));
    assert!(!ok(24, 16), "multiples of 16, not just of 8");
    assert!(!ok(864, 480), "long edge over 848");
    assert!(!ok(640, 496), "short edge over 480");
    assert!(!ok(1920, 1080));
}

#[test]
fn config_validation_and_codec_private() {
    let mut c = cfg(64, 48, PixelFormat::Yuv420);
    assert!(c.validate().is_ok());
    c.dictionary_entries = 0;
    assert!(c.validate().is_err());
    c.dictionary_entries = 4097;
    assert!(c.validate().is_err());
    c.dictionary_entries = 4096;
    c.format = PixelFormat::Rgb24;
    assert!(c.validate().is_err());

    let c = cfg(64, 48, PixelFormat::Yuv420);
    let back = MoteConfig::from_codec_private(64, 48, &c.to_codec_private()).unwrap();
    assert_eq!(back.format, PixelFormat::Yuv420);
    assert!(MoteConfig::from_codec_private(64, 48, &[9]).is_err());
}

#[test]
fn patch_counts_match_the_geometry() {
    assert_eq!(cfg(320, 240, PixelFormat::Gray8).patch_count(), 1200);
    assert_eq!(cfg(320, 240, PixelFormat::Yuv420).patch_count(), 1800);
    assert_eq!(cfg(848, 480, PixelFormat::Yuv420).patch_count(), 9540);
    const { assert!(9540 < (1usize << COUNT_BITS)) };
}

#[test]
fn picture_frame_round_trip_is_byte_exact() {
    for format in [PixelFormat::Gray8, PixelFormat::Yuv420] {
        let f = scene(64, 48, format, 3);
        let pic = Picture::from_frame(&f).unwrap();
        assert_eq!(pic.to_frame(f.frame.pts), f);
    }
}

#[test]
fn picture_rejects_bad_frames() {
    let mut f = scene(64, 48, PixelFormat::Yuv420, 0);
    f.pixels.truncate(f.pixels.len() - 1);
    assert!(Picture::from_frame(&f).is_err());
    let rgb = VideoFrame::alloc(64, 48, PixelFormat::Rgb24);
    assert!(matches!(
        Picture::from_frame(&rgb),
        Err(CodecError::Unsupported(_))
    ));
    assert!(Picture::from_frame(&VideoFrame::alloc(20, 16, PixelFormat::Gray8)).is_err());
}

#[test]
fn every_patch_loads_and_stores_back_exactly() {
    for format in [PixelFormat::Gray8, PixelFormat::Yuv420] {
        let src = Picture::from_frame(&scene(48, 32, format, 2)).unwrap();
        let grid = PatchGrid::new(48, 32, format);
        let mut dst = Picture::new(48, 32, format, 0);
        let mut p = [0u8; PATCH_DIM];
        for g in 0..grid.total() {
            grid.load(&src, g, &mut p);
            grid.store(&mut dst, g, &p);
        }
        assert_eq!(dst, src, "{format:?}");
    }
}

#[test]
fn key_frame_round_trip_matches_the_encoder_reconstruction() {
    for format in [PixelFormat::Gray8, PixelFormat::Yuv420] {
        let (mut enc, mut dec) = pair(64, 48, format, 96);
        let f = scene(64, 48, format, 0);
        let pkts = enc.encode(&f).unwrap();
        assert_eq!(pkts.len(), 1, "dictionary is out of band here");
        assert!(pkts[0].flags.contains(PacketFlags::KEYFRAME));
        assert!(pkts[0].flags.contains(PacketFlags::END_OF_FRAME));
        let out = dec.decode(&pkts[0]).unwrap();
        assert_eq!(out.len(), 1);
        let expect = enc.reference.as_ref().unwrap().to_frame(f.frame.pts);
        assert_eq!(out[0], expect, "{format:?}: decoder must mirror encoder");
        assert_eq!(out[0].frame.pts, f.frame.pts);
        #[cfg(feature = "std")]
        {
            let q = compare(&f, &out[0]).unwrap();
            assert!(q.luma_psnr_db() > 24.0, "{format:?} {}", q.luma_psnr_db());
        }
    }
}

#[test]
fn full_size_dictionary_uses_twelve_bit_indices() {
    // A synthetic 4096-entry dictionary: no training, just deterministic
    // content, so the test exercises the 12-bit path cheaply.
    let mut s = 0xC0FFEE_u64;
    let bytes: Vec<u8> = (0..4096 * PATCH_DIM).map(|_| lcg(&mut s) as u8).collect();
    let dict = Dictionary::from_entries(bytes).unwrap();
    assert_eq!(dict.index_bits(), 12);
    let c = cfg(32, 32, PixelFormat::Gray8);
    let mut enc =
        MoteEncoder::with_dictionary(CodecConfig::default(), c.clone(), dict.clone(), false)
            .unwrap();
    let mut dec = MoteDecoder::with_dictionary(CodecConfig::default(), c, dict).unwrap();
    let f = noise_frame(32, 32, PixelFormat::Gray8, 7);
    let pkt = enc.encode(&f).unwrap().remove(0);
    // 16 patches: 16 version + 2 type + 16 id + 16 * 12 = 226 bits.
    assert_eq!(pkt.data.len(), 226usize.div_ceil(8) + CRC_TRAILER_BYTES);
    let out = dec.decode(&pkt).unwrap();
    assert_eq!(
        out[0],
        enc.reference.as_ref().unwrap().to_frame(out[0].frame.pts)
    );
}

#[test]
fn an_unchanged_frame_costs_only_a_header() {
    let (mut enc, mut dec) = pair(64, 48, PixelFormat::Gray8, 64);
    let f = scene(64, 48, PixelFormat::Gray8, 0);
    let key = enc.encode(&f).unwrap().remove(0);
    dec.decode(&key).unwrap();
    let delta = enc.encode(&f).unwrap().remove(0);
    assert!(!delta.flags.contains(PacketFlags::KEYFRAME));
    // 16 + 2 + 16 + 3 + 16 = 53 bits -> 7 bytes + CRC.
    assert_eq!(delta.data.len(), 7 + CRC_TRAILER_BYTES);
    let out = dec.decode(&delta).unwrap();
    assert_eq!(out[0].pixels, dec.conceal(pts(0)).unwrap().pixels);
    assert_eq!(
        out[0],
        enc.reference.as_ref().unwrap().to_frame(out[0].frame.pts)
    );
}

#[test]
fn delta_frames_are_smaller_than_key_frames_on_a_mostly_static_scene() {
    let (mut enc, mut dec) = pair(64, 48, PixelFormat::Yuv420, 64);
    let key = enc.encode(&scene(64, 48, PixelFormat::Yuv420, 0)).unwrap();
    dec.decode(&key[0]).unwrap();
    let delta = enc.encode(&scene(64, 48, PixelFormat::Yuv420, 1)).unwrap();
    let out = dec.decode(&delta[0]).unwrap();
    assert!(
        delta[0].data.len() * 3 < key[0].data.len(),
        "delta {} vs key {}",
        delta[0].data.len(),
        key[0].data.len()
    );
    assert_eq!(
        out[0],
        enc.reference.as_ref().unwrap().to_frame(out[0].frame.pts)
    );
}

#[test]
fn a_delta_frame_tracks_a_moving_object() {
    let (mut enc, mut dec) = pair(64, 48, PixelFormat::Gray8, 128);
    for i in 0..6 {
        let f = scene(64, 48, PixelFormat::Gray8, i);
        for p in enc.encode(&f).unwrap() {
            let out = dec.decode(&p).unwrap();
            assert_eq!(
                out[0],
                enc.reference.as_ref().unwrap().to_frame(out[0].frame.pts),
                "frame {i}: decoder diverged from encoder"
            );
        }
    }
}

#[test]
fn disabling_temporal_prediction_makes_every_frame_a_key_frame() {
    let train_frames = [scene(32, 32, PixelFormat::Gray8, 0)];
    let dict = train(&train_frames, 8);
    let c = MoteConfig {
        temporal: false,
        ..cfg(32, 32, PixelFormat::Gray8)
    };
    let mut enc = MoteEncoder::with_dictionary(CodecConfig::default(), c, dict, false).unwrap();
    for i in 0..3 {
        let p = enc.encode(&scene(32, 32, PixelFormat::Gray8, i)).unwrap();
        assert!(p[0].flags.contains(PacketFlags::KEYFRAME));
    }
}

#[test]
fn the_keyframe_interval_renews_periodically() {
    let dict = train(&[scene(32, 32, PixelFormat::Gray8, 0)], 8);
    let mut enc = MoteEncoder::with_dictionary_and_keyframe_interval(
        CodecConfig::default(),
        cfg(32, 32, PixelFormat::Gray8),
        dict,
        false,
        3,
    )
    .unwrap();
    let f = scene(32, 32, PixelFormat::Gray8, 0);
    let keys: Vec<bool> = (0..7)
        .map(|_| {
            enc.encode(&f).unwrap()[0]
                .flags
                .contains(PacketFlags::KEYFRAME)
        })
        .collect();
    assert_eq!(keys, [true, false, false, true, false, false, true]);
}

#[test]
fn self_training_sends_the_dictionary_first_and_it_round_trips() {
    let c = MoteConfig {
        dictionary_entries: 32,
        train_iterations: 3,
        ..cfg(64, 48, PixelFormat::Gray8)
    };
    let mut enc = MoteEncoder::new(CodecConfig::default(), c.clone()).unwrap();
    let mut dec = MoteDecoder::new(CodecConfig::default(), c).unwrap();
    let f = scene(64, 48, PixelFormat::Gray8, 0);
    let pkts = enc.encode(&f).unwrap();
    assert_eq!(pkts.len(), 2, "dictionary packet then key packet");
    let n = enc.dictionary().unwrap().len();
    assert!((2..=32).contains(&n));
    assert_eq!(pkts[0].data.len(), MoteConfig::dictionary_packet_bytes(n));
    assert!(
        dec.decode(&pkts[0]).unwrap().is_empty(),
        "no frame from a dictionary"
    );
    assert_eq!(dec.dictionary(), enc.dictionary());
    let out = dec.decode(&pkts[1]).unwrap();
    assert_eq!(out.len(), 1);
    // Second frame: no dictionary packet again.
    assert_eq!(enc.encode(&f).unwrap().len(), 1);
}

#[test]
fn a_late_joiner_can_be_caught_up_with_dictionary_packet() {
    let (mut enc, _) = pair(32, 32, PixelFormat::Gray8, 8);
    let c = cfg(32, 32, PixelFormat::Gray8);
    let mut late = MoteDecoder::new(CodecConfig::default(), c).unwrap();
    let key = enc
        .encode(&scene(32, 32, PixelFormat::Gray8, 0))
        .unwrap()
        .remove(0);
    assert_eq!(late.decode(&key), Err(CodecError::NeedMoreInput));
    let dp = enc.dictionary_packet(pts(0)).unwrap();
    late.decode(&dp).unwrap();
    assert!(late.decode(&key).is_ok());
}

#[test]
fn self_trained_dictionary_size_rule() {
    assert_eq!(self_trained_entries(4096, 1200), 300);
    assert_eq!(self_trained_entries(64, 1200), 64);
    assert_eq!(self_trained_entries(4096, 3), 2);
    assert_eq!(self_trained_entries(4096, 1), 1);
}

#[test]
fn decode_rejects_an_unsupported_wire_version() {
    let (_, mut dec) = pair(32, 32, PixelFormat::Gray8, 8);
    let p = packet_from(|w| {
        w.write(16, u64::from(VERSION) + 1).unwrap();
        w.write(2, TYPE_KEY).unwrap();
    });
    assert!(matches!(dec.decode(&p), Err(CodecError::Unsupported(_))));
}

#[test]
fn decode_rejects_a_corrupted_checksum_and_short_packets() {
    let (mut enc, mut dec) = pair(32, 32, PixelFormat::Gray8, 8);
    let mut p = enc
        .encode(&scene(32, 32, PixelFormat::Gray8, 0))
        .unwrap()
        .remove(0);
    let last = p.data.len() - 1;
    p.data[last] ^= 0xFF;
    assert_eq!(dec.decode(&p), Err(CodecError::MalformedPacket));
    for len in 0..CRC_TRAILER_BYTES + 2 {
        let short = Packet {
            data: vec![0; len],
            ..p.clone()
        };
        assert_eq!(
            dec.decode(&short),
            Err(CodecError::MalformedPacket),
            "len {len}"
        );
    }
}

#[test]
fn delta_without_a_reference_or_a_frame_without_a_dictionary_needs_more_input() {
    let (mut enc, mut dec) = pair(32, 32, PixelFormat::Gray8, 8);
    let f = scene(32, 32, PixelFormat::Gray8, 0);
    let _key = enc.encode(&f).unwrap();
    let delta = enc.encode(&f).unwrap().remove(0);
    assert_eq!(dec.decode(&delta), Err(CodecError::NeedMoreInput));

    let mut bare =
        MoteDecoder::new(CodecConfig::default(), cfg(32, 32, PixelFormat::Gray8)).unwrap();
    assert_eq!(bare.decode(&delta), Err(CodecError::NeedMoreInput));
}

#[test]
fn a_frame_for_a_different_dictionary_is_refused() {
    let (mut enc, _) = pair(32, 32, PixelFormat::Gray8, 8);
    let (_, mut other) = pair(32, 32, PixelFormat::Gray8, 12);
    let key = enc
        .encode(&scene(32, 32, PixelFormat::Gray8, 0))
        .unwrap()
        .remove(0);
    assert!(matches!(
        other.decode(&key),
        Err(CodecError::Unsupported(_))
    ));
}

fn three_entry_decoder() -> (MoteDecoder, Dictionary) {
    let d = three_entry_dictionary();
    let dec = MoteDecoder::with_dictionary(
        CodecConfig::default(),
        cfg(16, 16, PixelFormat::Gray8),
        d.clone(),
    )
    .unwrap();
    (dec, d)
}

#[test]
fn an_out_of_range_index_is_malformed_and_does_not_disturb_the_reference() {
    // 16x16 gray = 4 patches; 3 entries -> 2-bit indices, value 3 invalid.
    let (mut dec, d) = three_entry_decoder();
    let key = |idx: [u64; 4]| {
        packet_from(|w| {
            w.write(16, u64::from(VERSION)).unwrap();
            w.write(2, TYPE_KEY).unwrap();
            w.write(16, u64::from(d.id())).unwrap();
            for i in idx {
                w.write(2, i).unwrap();
            }
        })
    };
    assert_eq!(
        dec.decode(&key([0, 1, 2, 3])),
        Err(CodecError::MalformedPacket)
    );
    assert!(dec.reference.is_none(), "a failed decode must not commit");
    let good = dec.decode(&key([0, 1, 2, 0])).unwrap().remove(0);
    assert_eq!(good.pixels[0], 10);
    assert_eq!(good.pixels[8], 100);

    // A bad delta leaves the previous reference intact.
    let bad_delta = packet_from(|w| {
        w.write(16, u64::from(VERSION)).unwrap();
        w.write(2, TYPE_DELTA).unwrap();
        w.write(16, u64::from(d.id())).unwrap();
        w.write(3, 0).unwrap();
        w.write(16, 1).unwrap();
        w.write(1, 1).unwrap(); // run 0
        w.write(2, 3).unwrap(); // invalid index
    });
    assert_eq!(dec.decode(&bad_delta), Err(CodecError::MalformedPacket));
    let ok_delta = packet_from(|w| {
        w.write(16, u64::from(VERSION)).unwrap();
        w.write(2, TYPE_DELTA).unwrap();
        w.write(16, u64::from(d.id())).unwrap();
        w.write(3, 0).unwrap();
        w.write(16, 1).unwrap();
        w.write(1, 1).unwrap();
        w.write(2, 2).unwrap();
    });
    let out = dec.decode(&ok_delta).unwrap().remove(0);
    assert_eq!(out.pixels[0], 200, "patch 0 replaced");
    assert_eq!(out.pixels[8], 100, "other patches kept from the reference");
}

#[test]
fn delta_count_and_run_bounds_are_enforced() {
    let (mut dec, d) = three_entry_decoder();
    let key = packet_from(|w| {
        w.write(16, u64::from(VERSION)).unwrap();
        w.write(2, TYPE_KEY).unwrap();
        w.write(16, u64::from(d.id())).unwrap();
        w.write(8, 0).unwrap();
    });
    dec.decode(&key).unwrap();
    let delta = |k: u64, count: u64, body: &dyn Fn(&mut BitWriter)| {
        packet_from(|w| {
            w.write(16, u64::from(VERSION)).unwrap();
            w.write(2, TYPE_DELTA).unwrap();
            w.write(16, u64::from(d.id())).unwrap();
            w.write(3, k).unwrap();
            w.write(16, count).unwrap();
            body(w);
        })
    };
    // count > total patches (4).
    assert_eq!(
        dec.decode(&delta(0, 5, &|_| {})),
        Err(CodecError::MalformedPacket)
    );
    // count promises more than the packet holds.
    assert_eq!(
        dec.decode(&delta(0, 2, &|_| {})),
        Err(CodecError::MalformedPacket)
    );
    // A run that walks off the end: k=0, run = 4 (0000 1) then index.
    let off_end = delta(0, 1, &|w| {
        w.write(4, 0).unwrap();
        w.write(1, 1).unwrap();
        w.write(2, 0).unwrap();
    });
    assert_eq!(dec.decode(&off_end), Err(CodecError::MalformedPacket));
    // A hostile unary prefix (all zeros) is bounded, not spun on.
    let endless = delta(0, 1, &|w| {
        for _ in 0..8 {
            w.write(32, 0).unwrap();
        }
    });
    assert_eq!(dec.decode(&endless), Err(CodecError::MalformedPacket));
}

#[test]
fn malformed_dictionary_packets_are_rejected() {
    let mut dec =
        MoteDecoder::new(CodecConfig::default(), cfg(16, 16, PixelFormat::Gray8)).unwrap();
    let dict_packet = |n_minus_1: u64, pad: u64, payload: usize| {
        packet_from(|w| {
            w.write(16, u64::from(VERSION)).unwrap();
            w.write(2, TYPE_DICTIONARY).unwrap();
            w.write(12, n_minus_1).unwrap();
            w.write(2, pad).unwrap();
            for _ in 0..payload {
                w.write(8, 5).unwrap();
            }
        })
    };
    // Claims 2 entries, carries 1.
    assert_eq!(
        dec.decode(&dict_packet(1, 0, 64)),
        Err(CodecError::MalformedPacket)
    );
    // Carries too many bytes for the claim.
    assert_eq!(
        dec.decode(&dict_packet(0, 0, 128)),
        Err(CodecError::MalformedPacket)
    );
    // Non-zero pad.
    assert_eq!(
        dec.decode(&dict_packet(0, 1, 64)),
        Err(CodecError::MalformedPacket)
    );
    assert!(dec.dictionary().is_none());
    assert!(dec.decode(&dict_packet(0, 0, 64)).unwrap().is_empty());
    assert_eq!(dec.dictionary().unwrap().len(), 1);
}

#[test]
fn an_unknown_packet_type_is_malformed() {
    let (_, mut dec) = pair(32, 32, PixelFormat::Gray8, 8);
    let p = packet_from(|w| {
        w.write(16, u64::from(VERSION)).unwrap();
        w.write(2, 3).unwrap();
    });
    assert_eq!(dec.decode(&p), Err(CodecError::MalformedPacket));
}

#[test]
fn conceal_repeats_the_last_frame_and_mid_greys_without_one() {
    let (mut enc, mut dec) = pair(32, 32, PixelFormat::Yuv420, 8);
    let grey = dec.conceal(pts(5)).unwrap();
    assert!(grey.pixels.iter().all(|&v| v == 128));
    assert_eq!(grey.frame.pts, pts(5));
    let key = enc
        .encode(&scene(32, 32, PixelFormat::Yuv420, 0))
        .unwrap()
        .remove(0);
    let shown = dec.decode(&key).unwrap().remove(0);
    let repeated = dec.conceal(pts(9)).unwrap();
    assert_eq!(repeated.pixels, shown.pixels);
}

#[test]
fn flush_drops_the_reference_but_keeps_the_dictionary() {
    let (mut enc, mut dec) = pair(32, 32, PixelFormat::Gray8, 8);
    let f = scene(32, 32, PixelFormat::Gray8, 0);
    let key = enc.encode(&f).unwrap().remove(0);
    let delta = enc.encode(&f).unwrap().remove(0);
    dec.decode(&key).unwrap();
    assert!(dec.flush().unwrap().is_empty());
    assert!(dec.dictionary().is_some());
    assert_eq!(dec.decode(&delta), Err(CodecError::NeedMoreInput));
    assert!(dec.decode(&key).is_ok());
}

#[test]
fn encoder_rejects_frames_that_do_not_match_the_config() {
    let (mut enc, _) = pair(32, 32, PixelFormat::Gray8, 8);
    assert!(matches!(
        enc.encode(&scene(32, 32, PixelFormat::Yuv420, 0)),
        Err(CodecError::Unsupported(_))
    ));
    assert!(matches!(
        enc.encode(&scene(48, 32, PixelFormat::Gray8, 0)),
        Err(CodecError::InvalidConfig(_))
    ));
}

#[test]
fn resource_limits_are_honoured() {
    let (mut enc, dec) = pair(32, 32, PixelFormat::Gray8, 8);
    let key = enc
        .encode(&scene(32, 32, PixelFormat::Gray8, 0))
        .unwrap()
        .remove(0);
    let c = cfg(32, 32, PixelFormat::Gray8);
    let dict = dec.dictionary().unwrap().clone();

    let mut tight = CodecConfig::default();
    tight.limits.max_packet_bytes = 4;
    let mut d = MoteDecoder::with_dictionary(tight, c.clone(), dict.clone()).unwrap();
    assert_eq!(
        d.decode(&key),
        Err(CodecError::ResourceLimit(ResourceLimitKind::PacketSize))
    );

    let mut tight = CodecConfig::default();
    tight.limits.max_frame_bytes = 16;
    let mut d = MoteDecoder::with_dictionary(tight, c.clone(), dict.clone()).unwrap();
    assert_eq!(
        d.decode(&key),
        Err(CodecError::ResourceLimit(
            ResourceLimitKind::FrameDimensions
        ))
    );

    let mut tight = CodecConfig::default();
    tight.limits.max_allocation_bytes = 16;
    let mut d = MoteDecoder::new(tight, c.clone()).unwrap();
    assert_eq!(
        d.decode(&key),
        Err(CodecError::ResourceLimit(ResourceLimitKind::Allocation))
    );

    let mut tight = CodecConfig::default();
    tight.limits.max_reference_frames = 0;
    let mut d = MoteDecoder::with_dictionary(tight, c, dict).unwrap();
    assert_eq!(
        d.decode(&key),
        Err(CodecError::ResourceLimit(
            ResourceLimitKind::ReferenceFrames
        ))
    );
}

#[test]
fn packets_never_exceed_the_provable_bound() {
    // Worst-ish cases: uncorrelated noise against a small and a full-size
    // dictionary, alternating between two noise frames so a delta frame
    // codes (nearly) every patch, and a sparse-change stream so runs are
    // long. Every packet must sit under `max_packet_bits`.
    for entries in [2usize, 3, 100, 4096] {
        let mut s = 99u64;
        let bytes: Vec<u8> = (0..entries * PATCH_DIM)
            .map(|_| lcg(&mut s) as u8)
            .collect();
        let dict = Dictionary::from_entries(bytes).unwrap();
        for format in [PixelFormat::Gray8, PixelFormat::Yuv420] {
            let c = MoteConfig {
                skip_mse: 0,
                ..cfg(32, 32, format)
            };
            let bound = c.max_packet_bits(entries).div_ceil(8) + CRC_TRAILER_BYTES;
            let mut enc =
                MoteEncoder::with_dictionary(CodecConfig::default(), c, dict.clone(), false)
                    .unwrap();
            for i in 0..6u64 {
                let f = noise_frame(32, 32, format, 1 + (i % 2));
                for p in enc.encode(&f).unwrap() {
                    assert!(
                        p.data.len() <= bound,
                        "{entries} entries {format:?}: {} > {bound}",
                        p.data.len()
                    );
                }
            }
        }
    }
}

#[test]
fn the_bound_is_tight_for_a_key_frame() {
    // A key frame is exactly header + T*b, so the bound must be reached.
    let c = cfg(32, 32, PixelFormat::Gray8);
    let key_bits = 16 + 2 + 16 + 16 * 3;
    assert!(c.max_packet_bits(8) >= key_bits);
}

#[test]
fn encoding_is_deterministic() {
    let mk = || {
        let c = MoteConfig {
            dictionary_entries: 16,
            train_iterations: 3,
            ..cfg(64, 48, PixelFormat::Yuv420)
        };
        MoteEncoder::new(CodecConfig::default(), c).unwrap()
    };
    let (mut a, mut b) = (mk(), mk());
    for i in 0..4 {
        let f = scene(64, 48, PixelFormat::Yuv420, i);
        let (pa, pb) = (a.encode(&f).unwrap(), b.encode(&f).unwrap());
        assert_eq!(pa.len(), pb.len());
        for (x, y) in pa.iter().zip(pb.iter()) {
            assert_eq!(x.data, y.data);
        }
    }
}

#[test]
fn quality_metric_is_defined_as_documented() {
    let a = scene(32, 32, PixelFormat::Yuv420, 0);
    let same = compare(&a, &a).unwrap();
    assert_eq!(same.luma_mse, 0.0);
    assert_eq!(same.mean_abs_error, 0.0);
    #[cfg(feature = "std")]
    assert_eq!(same.luma_psnr_db(), PSNR_CAP_DB);

    // A constant +2 error on every luma sample only.
    let mut b = a.clone();
    for p in b.pixels[..32 * 32].iter_mut() {
        *p = p.saturating_add(2);
    }
    let r = compare(&a, &b).unwrap();
    let clipped = a.pixels[..32 * 32].iter().filter(|&&p| p > 253).count();
    assert!(clipped == 0, "test scene must not clip");
    assert!((r.luma_mse - 4.0).abs() < 1e-9);
    // Luma is two thirds of the samples of a 4:2:0 frame.
    assert!((r.all_planes_mse - 4.0 * 2.0 / 3.0).abs() < 1e-9);
    #[cfg(feature = "std")]
    assert!((r.luma_psnr_db() - 10.0 * (255.0f64 * 255.0 / 4.0).log10()).abs() < 1e-9);

    assert!(compare(&a, &scene(48, 32, PixelFormat::Yuv420, 0)).is_err());
    assert!(compare(&a, &scene(32, 32, PixelFormat::Gray8, 0)).is_err());
}

#[test]
fn factories_build_a_working_pair_through_the_dyn_interface() {
    let c = MoteConfig {
        dictionary_entries: 16,
        train_iterations: 2,
        ..cfg(32, 32, PixelFormat::Gray8)
    };
    let ef = MoteEncoderFactory::new(c.clone()).unwrap();
    let df = MoteDecoderFactory::new(c.clone()).unwrap();
    assert_eq!(ef.codec_id(), b"MOTE");
    assert_eq!(df.codec_id(), b"MOTE");
    assert_eq!(ef.info().name, "Mote");
    assert!(ef.info().capabilities.real_time_decoder);
    assert!(!ef.info().capabilities.real_time_encoder);
    let cc = CodecConfig {
        extra: c.to_codec_private(),
        ..CodecConfig::default()
    };
    let mut enc = ef.build(&cc).unwrap();
    let mut dec = df.build(&cc).unwrap();
    let f = scene(32, 32, PixelFormat::Gray8, 0);
    let frame = tpt_media_codec::DynFrame::Video {
        pixel_format: f.format,
        width: f.width,
        height: f.height,
        stride: f.stride,
        pixels: f.pixels.clone(),
    };
    let mut frames = 0;
    for p in enc.encode(&frame).unwrap() {
        frames += dec.decode(&p).unwrap().len();
    }
    assert_eq!(frames, 1);
    assert!(matches!(
        enc.encode(&tpt_media_codec::DynFrame::Audio {
            sample_format: tpt_media_core::SampleFormat::F32,
            channel_layout: tpt_media_core::ChannelLayout::Mono,
            sample_rate: 8000,
            sample_count: 0,
            samples: vec![],
        }),
        Err(CodecError::Unsupported(_))
    ));
}

#[test]
fn provisioned_dictionary_factories_agree() {
    let dict = train(&[scene(32, 32, PixelFormat::Gray8, 0)], 8);
    let c = cfg(32, 32, PixelFormat::Gray8);
    let ef = MoteEncoderFactory::with_dictionary(c.clone(), dict.clone(), false).unwrap();
    let df = MoteDecoderFactory::with_dictionary(c, dict).unwrap();
    let cc = CodecConfig::default();
    let mut enc = ef.build(&cc).unwrap();
    let mut dec = df.build(&cc).unwrap();
    let f = scene(32, 32, PixelFormat::Gray8, 0);
    let frame = tpt_media_codec::DynFrame::Video {
        pixel_format: f.format,
        width: f.width,
        height: f.height,
        stride: f.stride,
        pixels: f.pixels,
    };
    let pkts = enc.encode(&frame).unwrap();
    assert_eq!(pkts.len(), 1);
    assert_eq!(dec.decode(&pkts[0]).unwrap().len(), 1);
}
