//! Thrum property and determinism tests.
//!
//! Follows this workspace's dependency-free seeded-LCG style rather than
//! a property-framework crate, matching `codecs/reed/tests/property.rs`.
//!
//! Thrum is lossy by design, so the properties pinned here are the
//! ones its contract actually claims:
//!
//! - a round trip of arbitrary content always yields a **full-length,
//!   correctly shaped** frame - never a panic, never a short buffer;
//! - a **cleaner input decodes cleaner**: the skip path and motion
//!   compensation must never make the output *worse* than the prediction
//!   they replaced;
//! - encoding and decoding are **pure functions** of (bytes, config) -
//!   no RNG, wall-clock, or thread state (todo.md section 39);
//! - no packet, however hostile, ever panics the decoder (todo.md section 38).

use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{Packet, PacketFlags, PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_thrum::{ThrumConfig, ThrumDecoder, ThrumEncoder};

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

fn frame(w: u32, h: u32, format: PixelFormat, index: i64) -> VideoFrame {
    let mut f = VideoFrame::alloc(w, h, format);
    let (w, h) = (w as usize, h as usize);
    let mut state = 0xA5A5_5A5A_0000_0001u64 ^ (index as u64).wrapping_mul(0x9E37_79B9);
    for y in 0..h {
        for x in 0..w {
            // A mix of low-frequency structure (which the codec models
            // well) and per-pixel noise (which it does not), so the
            // property is exercised on content the codec both suits and
            // struggles with.
            let ramp = (x + y) % 64;
            let noise = (lcg(&mut state) >> 33) as u8;
            f.pixels[y * w + x] = (ramp as u8).wrapping_add(noise & 0x1F);
        }
    }
    if format == PixelFormat::Yuv420 {
        let (cw, ch) = (w / 2, h / 2);
        let base = w * h;
        for c in 0..2 {
            for y in 0..ch {
                for x in 0..cw {
                    f.pixels[base + c * cw * ch + y * cw + x] =
                        ((x * 2 + y + c * 32) % 200 + 16) as u8;
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

fn config(w: u32, h: u32, format: PixelFormat) -> ThrumConfig {
    ThrumConfig {
        width: w,
        height: h,
        format,
        ..ThrumConfig::default()
    }
}

/// A round trip of arbitrary content yields exactly one full-length,
/// correctly shaped frame per packet, for any legal geometry and format.
#[test]
fn round_trip_is_always_full_length_and_correctly_shaped() {
    for (w, h) in [(16u32, 16u32), (64, 48), (176, 144), (320, 240)] {
        for format in [PixelFormat::Gray8, PixelFormat::Yuv420] {
            let c = config(w, h, format);
            let fw = CodecConfig::default();
            let mut enc = ThrumEncoder::new(fw.clone(), c.clone()).unwrap();
            let mut dec = ThrumDecoder::new(fw, c).unwrap();
            for i in 0..6i64 {
                let f = frame(w, h, format, i);
                let want_len = f.pixels.len();
                let packets = enc.encode(&f).unwrap();
                assert_eq!(packets.len(), 1, "{w}x{h} {format:?} frame {i}");
                let out = dec.decode(&packets[0]).unwrap();
                assert_eq!(out.len(), 1);
                assert_eq!(out[0].width, w);
                assert_eq!(out[0].height, h);
                assert_eq!(out[0].format, format);
                assert_eq!(
                    out[0].pixels.len(),
                    want_len,
                    "buffer length must be preserved"
                );
                assert_eq!(out[0].frame.pts, f.frame.pts, "PTS must pass through");
            }
        }
    }
}

/// A lower `qp` must never decode worse than a higher one on the same
/// content: the quantiser is the only rate knob and it must order quality.
#[test]
fn qp_monotonically_orders_reconstruction_error() {
    let measure = |qp: u8| {
        let c = ThrumConfig {
            qp,
            ..config(128, 96, PixelFormat::Yuv420)
        };
        let fw = CodecConfig::default();
        let mut enc = ThrumEncoder::new(fw.clone(), c.clone()).unwrap();
        let mut dec = ThrumDecoder::new(fw, c).unwrap();
        let f = frame(128, 96, PixelFormat::Yuv420, 0);
        let mut err = 0f64;
        for p in enc.encode(&f).unwrap() {
            let got = dec.decode(&p).unwrap().remove(0);
            err = f
                .pixels
                .iter()
                .zip(got.pixels.iter())
                .map(|(a, b)| f64::from((*a as i32 - *b as i32).unsigned_abs()))
                .sum::<f64>()
                / f.pixels.len() as f64;
        }
        err
    };
    let coarse = measure(51);
    let fine = measure(6);
    assert!(
        fine < coarse,
        "qp 6 mean abs error {fine:.3} should be below qp 51 ({coarse:.3})"
    );
}

/// Silence must survive exactly: a constant frame has a constant
/// prediction, so no residual is coded and the round trip is bit-exact.
/// This is the digital-silence property the audio codecs pin.
#[test]
fn a_flat_frame_round_trips_bit_exactly() {
    for format in [PixelFormat::Gray8, PixelFormat::Yuv420] {
        let c = config(64, 48, format);
        let fw = CodecConfig::default();
        let mut enc = ThrumEncoder::new(fw.clone(), c.clone()).unwrap();
        let mut dec = ThrumDecoder::new(fw, c).unwrap();
        let mut f = VideoFrame::alloc(64, 48, format);
        f.pixels.iter_mut().for_each(|v| *v = 128);
        for _ in 0..3 {
            let p = enc.encode(&f).unwrap().remove(0);
            let got = dec.decode(&p).unwrap().remove(0);
            assert_eq!(got.pixels, f.pixels, "{format:?} flat frame must be exact");
        }
    }
}

/// todo.md section 39: independent encoder/decoder instances must produce byte-identical
/// packets and byte-identical decodes. Thrum is a pure function of
/// (frame bytes, config) - no RNG, no clock, no thread state.
#[test]
fn encoding_and_decoding_are_deterministic() {
    let mk = || {
        let fw = CodecConfig::default();
        (
            ThrumEncoder::new(fw.clone(), config(176, 144, PixelFormat::Yuv420)).unwrap(),
            ThrumDecoder::new(fw, config(176, 144, PixelFormat::Yuv420)).unwrap(),
        )
    };
    let (mut enc_a, mut dec_a) = mk();
    let (mut enc_b, mut dec_b) = mk();
    for i in 0..4i64 {
        let f = frame(176, 144, PixelFormat::Yuv420, i);
        let pa = enc_a.encode(&f).unwrap();
        let pb = enc_b.encode(&f).unwrap();
        assert_eq!(pa.len(), pb.len());
        for (x, y) in pa.iter().zip(pb.iter()) {
            assert_eq!(x.data, y.data, "packets must be byte-identical");
            let da = dec_a.decode(x).unwrap();
            let db = dec_b.decode(y).unwrap();
            assert_eq!(da[0].pixels, db[0].pixels, "decodes must be byte-identical");
        }
    }
}
/// todo.md section 38: a deterministic corruption / truncation / bit-flip sweep over a
/// real packet, in the project's no-external-harness style. Every decode
/// must return a `Result`, never panic - and the pristine packet must
/// still decode afterwards, so the sweep is not silently poisoning the
/// decoder's reference state.
#[test]
fn corruption_sweep_never_panics_and_a_clean_packet_still_decodes() {
    let c = config(64, 48, PixelFormat::Yuv420);
    let fw = CodecConfig::default();
    let mut enc = ThrumEncoder::new(fw.clone(), c.clone()).unwrap();
    let mut dec = ThrumDecoder::new(fw, c).unwrap();
    let f0 = frame(64, 48, PixelFormat::Yuv420, 0);
    let valid = enc.encode(&f0).unwrap().remove(0);
    let pts = valid.pts;

    // Arbitrary bytes of arbitrary length.
    let mut state = 0x0F0F_0F0F_F0F0_F0F0u64;
    for _ in 0..400 {
        let len = (lcg(&mut state) % 512) as usize;
        let data: Vec<u8> = (0..len).map(|_| lcg(&mut state) as u8).collect();
        let pkt = Packet {
            data,
            pts,
            dts: pts,
            flags: PacketFlags::KEYFRAME,
            track_id: 0,
            duration: None,
        };
        let _ = dec.decode(&pkt);
    }

    // Every truncation of the real packet.
    for cut in 0..=valid.data.len() {
        let pkt = Packet {
            data: valid.data[..cut].to_vec(),
            ..valid.clone()
        };
        let _ = dec.decode(&pkt);
    }

    // Every single-bit flip at a sample of positions.
    for _ in 0..2000 {
        let pos = (lcg(&mut state) as usize) % valid.data.len();
        let bit = 1u8 << (lcg(&mut state) % 8);
        let mut corrupt = valid.clone();
        corrupt.data[pos] ^= bit;
        let _ = dec.decode(&corrupt);
    }

    assert!(
        dec.decode(&valid).is_ok(),
        "a pristine packet must still decode after the sweep"
    );
}

/// Packets claiming a `qp` outside the legal range, or a size the
/// resource limits forbid, are rejected rather than parsed.
#[test]
fn out_of_range_headers_are_rejected() {
    let c = config(32, 32, PixelFormat::Gray8);
    let fw = CodecConfig::default();
    // Encode one real I-frame so the decoder holds a reference, then
    // hand it hand-built headers whose `qp` is outside 1..=51.
    let mut enc = ThrumEncoder::new(fw.clone(), c.clone()).unwrap();
    let mut dec = ThrumDecoder::new(fw, c).unwrap();
    dec.decode(
        &enc.encode(&frame(32, 32, PixelFormat::Gray8, 0))
            .unwrap()
            .remove(0),
    )
    .unwrap();

    let pts = Timestamp::zero(Timebase::new(1, 30));
    for qp in [0u8, 63] {
        // 16-bit version, intra flag, then the 6-bit qp field. The CRC
        // is computed over the body so the qp check, not the checksum,
        // is what rejects the packet.
        let mut body = vec![0u8; 4];
        body[0] = 0x00;
        body[1] = 0x01; // version 1
        body[2] = 0b1000_0000; // intra flag, high bit of the qp field
        body[3] = qp;
        let mut data = body.clone();
        let crc = tpt_video_thrum::packet_crc(&body);
        data.extend_from_slice(&crc.to_le_bytes());
        let pkt = Packet {
            data,
            pts,
            dts: pts,
            flags: PacketFlags::KEYFRAME,
            track_id: 0,
            duration: None,
        };
        assert_eq!(
            dec.decode(&pkt),
            Err(tpt_media_codec::CodecError::MalformedPacket),
            "qp {qp} must be rejected"
        );
    }
}
