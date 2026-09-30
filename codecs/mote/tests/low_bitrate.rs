//! todo.md section 34 "test extremely low bitrate", and the bandwidth
//! figures the docs quote.
//!
//! **These are regression bounds derived from measurement, not a claim
//! that Mote achieves any bitrate on arbitrary content.** Mote's rate is
//! entirely a function of how much of the scene changes, and how far the
//! dictionary is from the content; the bandwidth target is defined by the
//! benchmark (`benches/codec_bench.rs`, recorded in `docs/performance.md`).
//!
//! What is *exact* rather than measured, and therefore asserted exactly:
//!
//! - a key frame costs `patches * index_bits` bits plus a 34-bit header
//!   and 4 CRC bytes, so with the 4096-entry dictionary (12-bit index)
//!   it is exactly `12/64 = 0.1875` bits per luma pixel;
//! - an unchanged frame costs a 53-bit header and the CRC (11 bytes).

use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{PacketFlags, PixelFormat, Timebase, Timestamp, VideoFrame};
use tpt_video_mote::{
    compare, Dictionary, MoteConfig, MoteDecoder, MoteEncoder, TrainConfig, CRC_TRAILER_BYTES,
};

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state >> 33
}

/// A fixed-camera scene: static textured background, a little sensor
/// noise that changes every frame, and a small object drifting across it.
fn camera(w: u32, h: u32, i: i64) -> VideoFrame {
    let mut f = VideoFrame::alloc(w, h, PixelFormat::Gray8);
    let (wu, hu) = (w as usize, h as usize);
    let mut s = 0x5EED_0000_0000u64 ^ (i as u64).wrapping_mul(0x9E37_79B9);
    let ox = (i as usize * 3) % (wu - 24);
    for y in 0..hu {
        for x in 0..wu {
            let bg = 60 + ((x * 5 + y * 3) % 90) + ((x / 16 + y / 16) % 4) * 15;
            let mut v = bg as i32 + (lcg(&mut s) % 3) as i32 - 1;
            if (ox..ox + 24).contains(&x) && (hu / 3..hu / 3 + 24).contains(&y) {
                v = 235;
            }
            f.pixels[y * wu + x] = v.clamp(0, 255) as u8;
        }
    }
    f.frame.pts = Timestamp {
        ticks: i,
        timebase: Timebase::new(1, 1),
    };
    f
}

fn cfg(w: u32, h: u32) -> MoteConfig {
    MoteConfig {
        width: w,
        height: h,
        format: PixelFormat::Gray8,
        ..MoteConfig::default()
    }
}

#[test]
fn key_frame_with_the_full_dictionary_is_exactly_twelve_bits_per_patch() {
    // Synthetic 4096-entry dictionary (content is irrelevant to the size).
    let mut s = 1u64;
    let bytes: Vec<u8> = (0..4096 * 64).map(|_| lcg(&mut s) as u8).collect();
    let dict = Dictionary::from_entries(bytes).unwrap();
    assert_eq!(dict.len(), 4096);
    assert_eq!(dict.index_bits(), 12);
    let (w, h) = (128u32, 96u32); // 192 patches
    let mut enc =
        MoteEncoder::with_dictionary(CodecConfig::default(), cfg(w, h), dict, false).unwrap();
    let p = enc.encode(&camera(w, h, 0)).unwrap().remove(0);
    let bits: usize = 16 + 2 + 16 + 192 * 12;
    assert_eq!(p.data.len(), bits.div_ceil(8) + CRC_TRAILER_BYTES);
    // Coded bits per luma pixel, header excluded: exactly 12/64.
    assert_eq!((192 * 12) as f64 / (w * h) as f64, 0.1875);
}

#[test]
fn a_static_camera_at_one_frame_per_second_is_a_few_kilobits() {
    let (w, h) = (320u32, 240u32);
    let frames: Vec<_> = (0..8).map(|i| camera(w, h, i)).collect();
    let dict = Dictionary::train_from_frames(
        &frames[..3],
        &TrainConfig {
            entries: 192,
            iterations: 3,
            ..TrainConfig::default()
        },
    )
    .unwrap();
    let c = cfg(w, h);
    let mut enc =
        MoteEncoder::with_dictionary(CodecConfig::default(), c.clone(), dict.clone(), false)
            .unwrap();
    let mut dec = MoteDecoder::with_dictionary(CodecConfig::default(), c, dict).unwrap();

    let mut key_bytes = 0usize;
    let mut delta_bytes = Vec::new();
    let mut worst_psnr = f64::MAX;
    for f in &frames {
        let p = enc.encode(f).unwrap().remove(0);
        if p.flags.contains(PacketFlags::KEYFRAME) {
            key_bytes = p.data.len();
        } else {
            delta_bytes.push(p.data.len());
        }
        let out = dec.decode(&p).unwrap().remove(0);
        worst_psnr = worst_psnr.min(compare(f, &out).unwrap().luma_psnr_db());
    }
    let mean_delta = delta_bytes.iter().sum::<usize>() as f64 / delta_bytes.len() as f64;
    let kbps_1fps = mean_delta * 8.0 / 1000.0;
    eprintln!(
        "static camera 320x240 @1 fps: key {key_bytes} B, mean delta {mean_delta:.0} B \
         = {kbps_1fps:.2} kbps, worst luma PSNR {worst_psnr:.1} dB"
    );

    // Regression bounds with headroom over the measured figures (see the
    // module docs: not a universal guarantee).
    assert!(
        (delta_bytes.iter().copied().max().unwrap() as f64) < key_bytes as f64 / 4.0,
        "a delta frame should be a small fraction of the key frame"
    );
    assert!(kbps_1fps < 8.0, "{kbps_1fps:.2} kbps at 1 fps");
    assert!(worst_psnr > 20.0, "worst PSNR {worst_psnr:.1} dB");
}

#[test]
fn an_unchanged_frame_is_a_heartbeat() {
    let (w, h) = (64u32, 48u32);
    let f = camera(w, h, 0);
    let c = MoteConfig {
        dictionary_entries: 16,
        train_iterations: 2,
        ..cfg(w, h)
    };
    let mut enc = MoteEncoder::new(CodecConfig::default(), c).unwrap();
    let _ = enc.encode(&f).unwrap();
    for _ in 0..5 {
        let p = enc.encode(&f).unwrap().remove(0);
        assert_eq!(
            p.data.len(),
            (16 + 2 + 16 + 3 + 16usize).div_ceil(8) + CRC_TRAILER_BYTES
        );
    }
}

/// The lowest-rate corner: a 2-entry dictionary (1-bit index) still
/// round-trips, at 1/64 bit per pixel for a key frame.
#[test]
fn a_two_entry_dictionary_is_the_floor() {
    let (w, h) = (64u32, 48u32);
    let f = camera(w, h, 0);
    let c = MoteConfig {
        dictionary_entries: 2,
        train_iterations: 3,
        ..cfg(w, h)
    };
    let mut enc = MoteEncoder::new(CodecConfig::default(), c.clone()).unwrap();
    let mut dec = MoteDecoder::new(CodecConfig::default(), c).unwrap();
    let pkts = enc.encode(&f).unwrap();
    let mut last = None;
    for p in &pkts {
        if let Some(o) = dec.decode(p).unwrap().pop() {
            last = Some(o);
        }
    }
    assert_eq!(enc.dictionary().unwrap().len(), 2);
    assert_eq!(enc.dictionary().unwrap().index_bits(), 1);
    let key = pkts.last().unwrap();
    // 48 patches * 1 bit + 34 header bits.
    assert_eq!(
        key.data.len(),
        (34usize + 48).div_ceil(8) + CRC_TRAILER_BYTES
    );
    assert_eq!(last.unwrap().pixels.len(), f.pixels.len());
}
