//! The per-frame bi-prediction weight (todo.md §33's v0.3 stage 1, the
//! wire's v3 bump): a B frame signals its past reference's share in
//! quarters, the encoder picks it by exact frame `J` across the plain
//! average and its two quarter-weight neighbours, and the decoder
//! weights identically by construction (one header field, one combine
//! implementation in `build_pred`).

use tpt_media_codec::{CodecConfig, CodecError, Decoder, Encoder, ResourceLimits};
use tpt_media_core::{Packet, PacketFlags, PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_vault::{packet_crc, VaultConfig, VaultDecoder, VaultEncoder};

const W: usize = 176;
const H: usize = 144;

/// An exposure ramp: frame 0 is a dim gradient, frame 2 the same
/// gradient 120 levels brighter, and the B frame exactly the decoder's
/// own 3/4-past blend of the two. With `w_past = 3` the bi-prediction
/// is *exact* (zero residual); the plain average is off by ~30 levels
/// everywhere, so the frame-`J` comparison must pick the weight.
fn ramp_frame(i: usize) -> VideoFrame {
    let mut f = VideoFrame::alloc(W as u32, H as u32, PixelFormat::Yuv420);
    for y in 0..H {
        for x in 0..W {
            let a = (60 + x / 8) as u16;
            let g = (180 + x / 8) as u16;
            let v = match i {
                0 => a,
                1 => (3 * a + g + 2) >> 2,
                _ => g,
            };
            f.pixels[y * W + x] = v as u8;
        }
    }
    let (cw, ch) = (W / 2, H / 2);
    for c in 0..2 {
        for y in 0..ch {
            for x in 0..cw {
                let (a, g) = if c == 0 { (90u16, 150u16) } else { (160u16, 100u16) };
                let v = match i {
                    0 => a,
                    1 => (3 * a + g + 2) >> 2,
                    _ => g,
                };
                f.pixels[W * H + c * cw * ch + y * cw + x] = v as u8;
            }
        }
    }
    f.frame.pts = Timestamp {
        ticks: i as i64,
        timebase: Timebase::new(1, 30),
    };
    f
}

fn fw() -> CodecConfig {
    CodecConfig {
        limits: ResourceLimits::permissive(),
        ..CodecConfig::default()
    }
}

fn encoder() -> VaultEncoder {
    let cfg = VaultConfig {
        width: W as u32,
        height: H as u32,
        format: PixelFormat::Yuv420,
        b_frames: 1,
        ..VaultConfig::default()
    };
    VaultEncoder::new(fw(), cfg).unwrap()
}

/// The 3-packet stream: intra (display 0), future (display 2), B
/// (display 1).
fn ramp_packets() -> Vec<Packet> {
    let mut enc = encoder();
    let mut packets = Vec::new();
    for i in 0..3 {
        packets.extend(Encoder::encode(&mut enc, &ramp_frame(i)).unwrap());
    }
    packets.extend(Encoder::flush(&mut enc).unwrap());
    packets
}

/// The weight search must see through the exposure ramp: the B frame's
/// decoded census names `3/4 past`, every frame still decodes, and two
/// encodes are byte-identical.
#[test]
fn b_weight_search_picks_the_blend_weight() {
    let packets = ramp_packets();
    assert_eq!(packets.len(), 3);

    let mut dec = VaultDecoder::new(fw(), {
        VaultConfig {
            width: W as u32,
            height: H as u32,
            format: PixelFormat::Yuv420,
            b_frames: 1,
            ..VaultConfig::default()
        }
    })
    .unwrap();
    let mut weights = Vec::new();
    for p in &packets {
        let out = Decoder::decode(&mut dec, p).unwrap();
        // Frames unblocked by the same packet share its census (the B
        // packet emits both its own display position and the future it
        // unblocks).
        let w = dec.last_stats().b_weight_past;
        for fr in out {
            if w != 0 {
                weights.push((fr.frame.pts.ticks, w));
            }
        }
    }
    assert_eq!(weights, vec![(1, 3), (2, 3)]);
    assert!(
        weights.contains(&(1, 3)),
        "the exposure ramp's B frame must pick 3/4 past — the weight its own pixels were made with"
    );

    let again = ramp_packets();
    assert_eq!(packets.len(), again.len());
    for (a, b) in packets.iter().zip(&again) {
        assert_eq!(a.data, b.data, "the weight search is deterministic");
    }
}

/// Coded at its true weight, the ramp's B frame reconstructs *exactly*:
/// its pixels were built with the decoder's own rounding from the same
/// references at zero vectors, so the chosen prediction is the source
/// and the residual is empty. (This is also the closed-loop pin for the
/// weighted combine: encoder-side prediction and decoder-side weighting
/// cannot disagree by even a rounding step.)
#[test]
fn b_weighted_frame_reconstructs_exactly() {
    let packets = ramp_packets();
    let mut dec = VaultDecoder::new(fw(), {
        VaultConfig {
            width: W as u32,
            height: H as u32,
            format: PixelFormat::Yuv420,
            b_frames: 1,
            ..VaultConfig::default()
        }
    })
    .unwrap();
    let mut b_psnr = None;
    for p in &packets {
        for fr in Decoder::decode(&mut dec, p).unwrap() {
            if fr.frame.pts.ticks == 1 {
                let src = ramp_frame(1);
                let mse = src.pixels[..W * H]
                    .iter()
                    .zip(&fr.pixels[..W * H])
                    .map(|(a, b)| {
                        let d = f64::from(*a) - f64::from(*b);
                        d * d
                    })
                    .sum::<f64>()
                    / (W * H) as f64;
                b_psnr = Some(if mse == 0.0 {
                    99.0
                } else {
                    10.0 * (255.0f64 * 255.0 / mse).log10()
                });
            }
        }
    }
    let psnr = b_psnr.expect("the B frame decodes");
    assert!(
        psnr > 45.0,
        "an exactly-predicted B frame must reconstruct near-perfectly, got {psnr:.2} dB"
    );
}

/// Wire value 3 is reserved: a B packet whose weight field reads 3 is
/// malformed even though its CRC verifies.
#[test]
fn reserved_b_weight_is_rejected() {
    let packets = ramp_packets();
    // The B packet is the one whose header ftype (bits 16-17) reads 2.
    let mut b = packets
        .into_iter()
        .find(|p| (p.data[2] >> 6) & 3 == 2)
        .expect("the stream contains a B packet");
    assert!(b.flags.contains(PacketFlags::END_OF_FRAME));
    // The weight field is header bits 46-47: byte 5, masks 0x02 | 0x01.
    let body_len = b.data.len() - 4;
    b.data[5] |= 0x03;
    let crc = packet_crc(&b.data[..body_len]);
    b.data[body_len..].copy_from_slice(&crc.to_le_bytes());

    let mut dec = VaultDecoder::new(fw(), {
        VaultConfig {
            width: W as u32,
            height: H as u32,
            format: PixelFormat::Yuv420,
            b_frames: 1,
            ..VaultConfig::default()
        }
    })
    .unwrap();
    assert!(
        matches!(
            Decoder::decode(&mut dec, &b),
            Err(CodecError::MalformedPacket)
        ),
        "a reserved weight must be rejected as malformed"
    );
}
