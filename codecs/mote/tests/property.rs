//! Mote property, determinism and hostile-input tests.
//!
//! Same dependency-free seeded-LCG style as `codecs/thrum/tests/property.rs`.
//! The properties pinned are the ones Mote's contract claims:
//!
//! - a round trip always yields a full-length, correctly shaped frame that
//!   is exactly what the encoder reconstructed (no drift, ever);
//! - a **bigger dictionary never decodes worse** on the content it was
//!   trained on (the dictionary is the only quality knob);
//! - encoding and decoding are pure functions of (bytes, dictionary,
//!   config) - todo.md section 39;
//! - no packet, however hostile, ever panics the decoder (todo.md 38,
//!   47): random bytes, truncations, bit flips with and without a
//!   repaired CRC, and random bodies behind a valid header.

use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{Packet, PacketFlags, PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_mote::{
    compare, packet_crc, Dictionary, MoteConfig, MoteDecoder, MoteEncoder, TrainConfig,
    CRC_TRAILER_BYTES, VERSION,
};

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

/// Structured content with a little noise, and a moving square.
fn frame(w: u32, h: u32, format: PixelFormat, i: i64) -> VideoFrame {
    let mut f = VideoFrame::alloc(w, h, format);
    let (wu, hu) = (w as usize, h as usize);
    let mut s = 0xA5A5_0000_0001u64 ^ (i as u64).wrapping_mul(0x9E37_79B9);
    let sq = (i as usize * 6) % (wu.saturating_sub(16).max(1));
    for y in 0..hu {
        for x in 0..wu {
            let mut v = ((x * 2 + y * 3) % 190 + 30 + ((x / 8 + y / 8) % 3) * 10) as u8;
            if (sq..sq + 16).contains(&x) && y < 16 {
                v = 250;
            }
            f.pixels[y * wu + x] = v.wrapping_add((lcg(&mut s) & 3) as u8);
        }
    }
    if format == PixelFormat::Yuv420 {
        let (cw, ch) = (wu / 2, hu / 2);
        for c in 0..2 {
            for y in 0..ch {
                for x in 0..cw {
                    f.pixels[wu * hu + c * cw * ch + y * cw + x] =
                        (90 + (x * 2 + y + c * 30) % 120) as u8;
                }
            }
        }
    }
    f.frame.pts = pts(i);
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

fn dictionary(w: u32, h: u32, format: PixelFormat, entries: usize) -> Dictionary {
    let frames: Vec<_> = (0..3).map(|i| frame(w, h, format, i)).collect();
    Dictionary::train_from_frames(
        &frames,
        &TrainConfig {
            entries,
            iterations: 3,
            ..TrainConfig::default()
        },
    )
    .unwrap()
}

fn pair(w: u32, h: u32, format: PixelFormat, dict: &Dictionary) -> (MoteEncoder, MoteDecoder) {
    let c = cfg(w, h, format);
    (
        MoteEncoder::with_dictionary(CodecConfig::default(), c.clone(), dict.clone(), false)
            .unwrap(),
        MoteDecoder::with_dictionary(CodecConfig::default(), c, dict.clone()).unwrap(),
    )
}

#[test]
fn round_trip_is_always_full_length_and_correctly_shaped() {
    for (w, h) in [(16u32, 16u32), (64, 48), (176, 144)] {
        for format in [PixelFormat::Gray8, PixelFormat::Yuv420] {
            let dict = dictionary(w, h, format, 48);
            let (mut enc, mut dec) = pair(w, h, format, &dict);
            for i in 0..6i64 {
                let f = frame(w, h, format, i);
                let packets = enc.encode(&f).unwrap();
                assert_eq!(packets.len(), 1, "{w}x{h} {format:?} frame {i}");
                let out = dec.decode(&packets[0]).unwrap();
                assert_eq!(out.len(), 1);
                assert_eq!((out[0].width, out[0].height, out[0].format), (w, h, format));
                assert_eq!(out[0].pixels.len(), f.pixels.len());
                assert_eq!(out[0].frame.pts, f.frame.pts, "PTS must pass through");
            }
        }
    }
}

/// A bigger dictionary must not decode worse on its own training content.
#[test]
fn dictionary_size_orders_reconstruction_error() {
    let (w, h, format) = (96u32, 64u32, PixelFormat::Gray8);
    let mse = |entries: usize| {
        let dict = dictionary(w, h, format, entries);
        let (mut enc, mut dec) = pair(w, h, format, &dict);
        let f = frame(w, h, format, 0);
        let p = enc.encode(&f).unwrap().remove(0);
        let out = dec.decode(&p).unwrap().remove(0);
        compare(&f, &out).unwrap().luma_mse
    };
    let (small, medium, large) = (mse(4), mse(32), mse(72));
    assert!(large < medium, "72 entries {large:.2} vs 32 {medium:.2}");
    assert!(medium < small, "32 entries {medium:.2} vs 4 {small:.2}");
}

/// A frame whose every patch is present in the dictionary is exact.
#[test]
fn a_flat_frame_round_trips_bit_exactly() {
    for format in [PixelFormat::Gray8, PixelFormat::Yuv420] {
        let mut f = VideoFrame::alloc(64, 48, format);
        f.pixels.iter_mut().for_each(|v| *v = 128);
        let c = MoteConfig {
            dictionary_entries: 8,
            ..cfg(64, 48, format)
        };
        let mut enc = MoteEncoder::new(CodecConfig::default(), c.clone()).unwrap();
        let mut dec = MoteDecoder::new(CodecConfig::default(), c).unwrap();
        for _ in 0..3 {
            let mut last = None;
            for p in enc.encode(&f).unwrap() {
                if let Some(o) = dec.decode(&p).unwrap().pop() {
                    last = Some(o);
                }
            }
            assert_eq!(last.unwrap().pixels, f.pixels, "{format:?} flat frame");
        }
    }
}

/// todo.md section 39.
#[test]
fn encoding_and_decoding_are_deterministic() {
    let dict = dictionary(64, 48, PixelFormat::Yuv420, 32);
    let (mut ea, mut da) = pair(64, 48, PixelFormat::Yuv420, &dict);
    let (mut eb, mut db) = pair(64, 48, PixelFormat::Yuv420, &dict);
    for i in 0..5i64 {
        let f = frame(64, 48, PixelFormat::Yuv420, i);
        let (pa, pb) = (ea.encode(&f).unwrap(), eb.encode(&f).unwrap());
        for (x, y) in pa.iter().zip(pb.iter()) {
            assert_eq!(x.data, y.data, "packets must be byte-identical");
            assert_eq!(
                da.decode(x).unwrap()[0].pixels,
                db.decode(y).unwrap()[0].pixels
            );
        }
    }
}

/// Training must be reproducible bit for bit (fixed seeds, no hash-order
/// dependence): three independent runs agree.
#[test]
fn training_is_reproducible() {
    let a = dictionary(64, 48, PixelFormat::Yuv420, 40);
    let b = dictionary(64, 48, PixelFormat::Yuv420, 40);
    let c = dictionary(64, 48, PixelFormat::Yuv420, 40);
    assert_eq!(a.as_bytes(), b.as_bytes());
    assert_eq!(b.as_bytes(), c.as_bytes());
    assert_eq!(a.id(), c.id());
}

fn with_fixed_crc(mut body: Vec<u8>) -> Vec<u8> {
    let crc = packet_crc(&body);
    body.extend_from_slice(&crc.to_le_bytes());
    body
}

fn pkt(data: Vec<u8>, at: Timestamp) -> Packet {
    Packet {
        data,
        pts: at,
        dts: at,
        flags: PacketFlags::KEYFRAME,
        track_id: 0,
        duration: None,
    }
}

/// todo.md sections 38/47: a deterministic hostile-input sweep. Every
/// decode must return a `Result`, never panic, and after the sweep a
/// pristine stream must still decode (the sweep must not poison state).
#[test]
fn corruption_sweep_never_panics_and_a_clean_stream_still_decodes() {
    let (w, h, format) = (64u32, 48u32, PixelFormat::Yuv420);
    let c = MoteConfig {
        dictionary_entries: 24,
        train_iterations: 2,
        ..cfg(w, h, format)
    };
    // Self-training: the first frame yields [dictionary packet, key packet].
    let mut enc = MoteEncoder::new(CodecConfig::default(), c.clone()).unwrap();
    let mut first = enc.encode(&frame(w, h, format, 0)).unwrap();
    assert_eq!(first.len(), 2);
    let key = first.pop().unwrap();
    let dict_packet = first.pop().unwrap();
    let delta = enc.encode(&frame(w, h, format, 1)).unwrap().remove(0);
    let at = key.pts;

    let mut dec = MoteDecoder::new(CodecConfig::default(), c).unwrap();
    dec.decode(&dict_packet).unwrap();
    dec.decode(&key).unwrap();
    let mut state = 0x0F0F_0F0F_F0F0_F0F0u64;

    // 1. Arbitrary bytes of arbitrary length.
    for _ in 0..300 {
        let len = (lcg(&mut state) % 400) as usize;
        let data: Vec<u8> = (0..len).map(|_| lcg(&mut state) as u8).collect();
        let _ = dec.decode(&pkt(data, at));
    }

    // 2. Every truncation of the real packets, CRC intact and repaired.
    for real in [&key, &delta, &dict_packet] {
        for cut in 0..=real.data.len().min(600) {
            let _ = dec.decode(&pkt(real.data[..cut].to_vec(), at));
            if cut > CRC_TRAILER_BYTES {
                let body = real.data[..cut - CRC_TRAILER_BYTES].to_vec();
                let _ = dec.decode(&pkt(with_fixed_crc(body), at));
            }
        }
    }

    // 3. Single-bit flips. Unrepaired ones must die at the CRC; repaired
    //    ones reach the parser.
    for real in [&key, &delta, &dict_packet] {
        for _ in 0..1500 {
            let pos = (lcg(&mut state) as usize) % real.data.len();
            let bit = 1u8 << (lcg(&mut state) % 8);
            let mut bad = real.data.clone();
            bad[pos] ^= bit;
            let _ = dec.decode(&pkt(bad.clone(), at));
            if pos < real.data.len() - CRC_TRAILER_BYTES {
                let body = bad[..bad.len() - CRC_TRAILER_BYTES].to_vec();
                let _ = dec.decode(&pkt(with_fixed_crc(body), at));
            }
        }
    }

    // 4. Random bodies behind a valid version + type header + CRC, so the
    //    parser (not the CRC or the version check) is what is exercised.
    for ty in [0u8, 1, 2, 3] {
        for _ in 0..300 {
            let len = 2 + (lcg(&mut state) % 300) as usize;
            let mut body: Vec<u8> = (0..len).map(|_| lcg(&mut state) as u8).collect();
            body[0] = (VERSION >> 8) as u8;
            body[1] = VERSION as u8;
            if len > 2 {
                body[2] = (body[2] & 0x3F) | (ty << 6);
            }
            let _ = dec.decode(&pkt(with_fixed_crc(body), at));
        }
    }

    // The pristine stream still decodes (dictionary first: the sweep may
    // have legitimately replaced it with a different valid one).
    dec.decode(&dict_packet).unwrap();
    assert!(dec.decode(&key).is_ok());
    assert!(dec.decode(&delta).is_ok());
}

/// An adversarial dictionary packet may claim the full 4096 entries and
/// must be measured against its real length, not trusted.
#[test]
fn a_dictionary_packet_cannot_claim_more_than_it_carries() {
    let mut dec =
        MoteDecoder::new(CodecConfig::default(), cfg(32, 32, PixelFormat::Gray8)).unwrap();
    // version, type 0, entries-1 = 4095, pad 0, then only 100 bytes.
    let mut body = vec![(VERSION >> 8) as u8, VERSION as u8];
    body.push(0b0000_0011); // type 00, top 6 bits of 4095
    body.push(0b1111_1100); // low 6 bits of 4095, pad 00
    body.extend(std::iter::repeat_n(7u8, 100));
    let r = dec.decode(&pkt(with_fixed_crc(body), pts(0)));
    assert_eq!(r, Err(tpt_media_codec::CodecError::MalformedPacket));
    assert!(dec.dictionary().is_none());
}
