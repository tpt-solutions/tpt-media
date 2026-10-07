//! The long-term reference (todo.md §33's v0.3 stage 3): one picture
//! pinned outside the last-N pool, addressable by P-frame leaves until
//! replaced, surviving intra pool resets. The value case is content
//! that revisits — an A/B/A/B cut where the short-term pool has
//! forgotten the first A by the time the second A arrives.

use tpt_media_codec::{CodecConfig, Decoder, Encoder, ResourceLimits};
use tpt_media_core::{PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_vault::{VaultConfig, VaultDecoder, VaultEncoder};

const W: usize = 176;
const H: usize = 144;

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

/// One frame of a two-scene clip: scene 0 a dark rising gradient with a
/// panning textured block, scene 1 a bright falling one — neighbouring
/// frames within a scene differ only by the pan, the scenes differ
/// almost everywhere. `frames_per_scene` frames per scene, alternating
/// A/B/A/B.
fn abab_clip(frames_per_scene: usize) -> Vec<VideoFrame> {
    (0..4 * frames_per_scene)
        .map(|i| {
            let scene_a = (i / frames_per_scene) % 2 == 0;
            let mut f = VideoFrame::alloc(W as u32, H as u32, PixelFormat::Yuv420);
            let mut state = 0x5EED_A11A_u64 ^ (i as u64).wrapping_mul(0x9E37_79B9);
            let pan = (i * 2) as i32;
            for y in 0..H {
                for x in 0..W {
                    let g = (x * 120 / W + y * 70 / H) as i32;
                    let stripe = (x as i32 + pan) % 24;
                    let v = if scene_a {
                        40 + (g + stripe) / 2
                    } else {
                        210 - (g + stripe) / 2
                    };
                    f.pixels[y * W + x] = v.clamp(0, 255) as u8;
                }
            }
            let (cw, ch) = (W / 2, H / 2);
            for c in 0..2 {
                for y in 0..ch {
                    for x in 0..cw {
                        let v = if scene_a {
                            90 + ((lcg(&mut state) >> 60) as i32 % 8)
                        } else {
                            150 - ((lcg(&mut state) >> 60) as i32 % 8)
                        };
                        f.pixels[W * H + c * cw * ch + y * cw + x] = v.clamp(0, 255) as u8;
                    }
                }
            }
            f.frame.pts = Timestamp {
                ticks: i as i64,
                timebase: Timebase::new(1, 30),
            };
            f
        })
        .collect()
}

fn fw() -> CodecConfig {
    CodecConfig {
        limits: ResourceLimits::permissive(),
        ..CodecConfig::default()
    }
}

fn cfg(long_term_interval: u32) -> VaultConfig {
    VaultConfig {
        width: W as u32,
        height: H as u32,
        format: PixelFormat::Yuv420,
        long_term_interval,
        ..VaultConfig::default()
    }
}

/// The keyframe schedule parked far beyond the clip (an `interval` of 0
/// would force every frame intra and wash the LTR comparison out).
fn encoder(interval: u32) -> VaultEncoder {
    VaultEncoder::with_keyframe_interval(fw(), cfg(interval), 10_000).unwrap()
}

/// The value case, as the design demanded: an A/B/A/B clip whose
/// scenes are longer than the short-term pool. With the LTR pinned to
/// the first A (interval = one full A/B round trip), the second A's
/// frames predict from their own scene again and the stream is
/// measurably cheaper; the decoder-side census proves the LTR is not
/// merely held but *used*.
#[test]
fn ltr_pays_off_on_revisiting_content() {
    let clip = abab_clip(6);

    let run = |interval: u32| {
        let mut enc = encoder(interval);
        let mut dec = VaultDecoder::new(fw(), cfg(interval)).unwrap();
        let mut bits = 0u64;
        let mut ltr_leaves_in_second_a = 0u32;
        for f in &clip {
            for p in Encoder::encode(&mut enc, f).unwrap() {
                bits += p.data.len() as u64 * 8;
                let out = Decoder::decode(&mut dec, &p).unwrap();
                let st = dec.last_stats();
                for fr in out {
                    // The second scene-A run: displays 12..18.
                    if (12..18).contains(&fr.frame.pts.ticks) {
                        ltr_leaves_in_second_a += st.ltr;
                    }
                }
            }
        }
        (bits, ltr_leaves_in_second_a)
    };

    let (bits_off, ltr_off) = run(0);
    let (bits_on, ltr_on) = run(12);
    assert_eq!(ltr_off, 0, "no knob, no LTR leaves");
    assert!(
        bits_on < bits_off,
        "the revisiting second A must be cheaper with the LTR ({bits_on} bits) than without ({bits_off} bits)"
    );
    assert!(
        ltr_on > 0,
        "the decoder census must show P leaves naming the long-term reference"
    );
}

/// The closed loop stays exact with the LTR in play and a scene cut
/// landing mid-stream: every decoded frame is bit-identical to the
/// reconstruction the encoder kept, including across the LTR update
/// and the intra reset that deliberately does *not* clear it.
#[test]
fn ltr_stream_stays_drift_free_through_a_scene_cut() {
    let cfg = cfg(4);
    let mut enc = VaultEncoder::with_keyframe_interval(fw(), cfg.clone(), 10_000).unwrap();
    let mut dec = VaultDecoder::new(fw(), cfg).unwrap();
    // The cut detector is off; the clip's own hard cut at frame 12 is
    // survived by prediction, and the LTR updates every 4 reference
    // frames — several times across the cut.
    let clip = abab_clip(6);
    for f in &clip {
        for p in Encoder::encode(&mut enc, f).unwrap() {
            let recon = enc.reconstruction().unwrap();
            let out = Decoder::decode(&mut dec, &p).unwrap();
            assert_eq!(out.len(), 1);
            assert_eq!(
                out[0].pixels, recon.pixels,
                "decoder must match the encoder's kept reconstruction"
            );
        }
    }
}

/// LTR, B frames and the scene-cut detector compose: the full stream
/// decodes exactly once in display order, and the whole path is a pure
/// function of its input.
#[test]
fn ltr_composes_with_b_frames_and_scene_cut() {
    let cfg = VaultConfig {
        width: W as u32,
        height: H as u32,
        format: PixelFormat::Yuv420,
        b_frames: 2,
        scene_cut: true,
        long_term_interval: 5,
        ..VaultConfig::default()
    };
    let clip = abab_clip(6);
    let run = || {
        let mut enc = VaultEncoder::new(fw(), cfg.clone()).unwrap();
        let mut dec = VaultDecoder::new(fw(), cfg.clone()).unwrap();
        let mut packets = Vec::new();
        for f in &clip {
            packets.extend(Encoder::encode(&mut enc, f).unwrap());
        }
        packets.extend(Encoder::flush(&mut enc).unwrap());
        let mut shown = Vec::new();
        for p in &packets {
            shown.extend(Decoder::decode(&mut dec, p).unwrap());
        }
        shown.extend(Decoder::flush(&mut dec).unwrap());
        (packets, shown)
    };
    let (packets, shown) = run();
    assert_eq!(shown.len(), clip.len(), "every frame emitted exactly once");
    for (i, f) in shown.iter().enumerate() {
        assert_eq!(f.frame.pts.ticks, i as i64, "display order");
    }
    let (packets2, _) = run();
    for (a, b) in packets.iter().zip(&packets2) {
        assert_eq!(a.data, b.data, "deterministic");
    }
}
