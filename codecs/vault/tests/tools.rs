//! Vault coding-tool census tests.
//!
//! The round-trip tests prove the closed loop is consistent; these prove the
//! *tools* are actually used when the content calls for them: recursive
//! partitioning, half-pel motion, multiple references and skip. Each test
//! decodes real packets and reads [`tpt_video_vault::FrameStats`].

use tpt_media_codec::{CodecConfig, Decoder, Encoder, ResourceLimits};
use tpt_media_core::{PixelFormat, VideoFrame};
use tpt_video_vault::{VaultConfig, VaultDecoder, VaultEncoder};

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

/// Smooth-ish textured luma (bilinearly interpolated noise on a coarse
/// grid), gray only. Smooth enough that half-pel interpolation predicts it
/// well, textured enough that copying a wrong block costs bits.
fn texture(w: usize, h: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    let (gw, gh) = (w / 6 + 2, h / 6 + 2);
    let grid: Vec<f32> = (0..gw * gh)
        .map(|_| 40.0 + (lcg(&mut state) >> 56) as f32 * 0.6)
        .collect();
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / 6.0, y as f32 / 6.0);
            let (ix, iy) = (fx as usize, fy as usize);
            let (ax, ay) = (fx - ix as f32, fy - iy as f32);
            let g = |gx: usize, gy: usize| grid[gy * gw + gx];
            let v = g(ix, iy) * (1.0 - ax) * (1.0 - ay)
                + g(ix + 1, iy) * ax * (1.0 - ay)
                + g(ix, iy + 1) * (1.0 - ax) * ay
                + g(ix + 1, iy + 1) * ax * ay;
            out[y * w + x] = v as u8;
        }
    }
    out
}

fn gray_frame(w: u32, h: u32, pixels: Vec<u8>) -> VideoFrame {
    let mut f = VideoFrame::alloc(w, h, PixelFormat::Gray8);
    f.pixels = pixels;
    f
}

fn cfg(w: u32, h: u32) -> VaultConfig {
    VaultConfig {
        width: w,
        height: h,
        format: PixelFormat::Gray8,
        qp: 20,
        ..VaultConfig::default()
    }
}

#[test]
fn recursive_partitioning_uses_large_blocks_where_flat_and_small_where_busy() {
    let (w, h) = (128usize, 64usize);
    let mut px = vec![128u8; w * h];
    let mut state = 99u64;
    // Right half: strong noise. Left half stays flat.
    for y in 0..h {
        for x in w / 2..w {
            px[y * w + x] = (lcg(&mut state) >> 56) as u8;
        }
    }
    let c = cfg(w as u32, h as u32);
    let mut enc = VaultEncoder::new(CodecConfig::default(), c.clone()).unwrap();
    let mut dec = VaultDecoder::new(CodecConfig::default(), c).unwrap();
    let p = enc
        .encode(&gray_frame(w as u32, h as u32, px))
        .unwrap()
        .remove(0);
    dec.decode(&p).unwrap();
    let s = dec.last_stats();
    assert!(
        s.leaves_by_size[3] >= 1,
        "flat 64x64 half must be one leaf: {s:?}"
    );
    assert!(
        s.leaves_by_size[0] + s.leaves_by_size[1] >= 4,
        "noisy half must be partitioned finely: {s:?}"
    );
    assert_eq!(
        s.intra,
        s.leaves_by_size.iter().sum::<u32>(),
        "I-frame is all intra"
    );
}

#[test]
fn half_pel_motion_is_used_and_predicts_well() {
    let (w, h) = (64usize, 64usize);
    let base = texture(w + 1, h, 5);
    // Frame 0: the texture; frame 1: the same texture displaced by exactly
    // half a pixel, i.e. the average of neighbouring columns.
    let f0: Vec<u8> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (y, x)))
        .map(|(y, x)| base[y * (w + 1) + x])
        .collect();
    let f1: Vec<u8> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (y, x)))
        .map(|(y, x)| {
            ((u16::from(base[y * (w + 1) + x]) + u16::from(base[y * (w + 1) + x + 1]) + 1) >> 1)
                as u8
        })
        .collect();
    let c = cfg(w as u32, h as u32);
    let mut enc = VaultEncoder::new(CodecConfig::default(), c.clone()).unwrap();
    let mut dec = VaultDecoder::new(CodecConfig::default(), c).unwrap();
    let a = enc.encode(&gray_frame(64, 64, f0)).unwrap().remove(0);
    dec.decode(&a).unwrap();
    let b = enc.encode(&gray_frame(64, 64, f1)).unwrap().remove(0);
    dec.decode(&b).unwrap();
    let s = dec.last_stats();
    assert!(s.half_pel > 0, "no half-pel vector was chosen: {s:?}");
    assert!(s.skip + s.inter > 0 && s.intra == 0, "{s:?}");
    assert!(
        b.data.len() * 3 < a.data.len(),
        "half-pel P {} B vs I {} B",
        b.data.len(),
        a.data.len()
    );
}

#[test]
fn multiple_references_are_used_and_pay_off() {
    let (w, h) = (64usize, 64usize);
    let a = gray_frame(64, 64, texture(w, h, 11));
    let b = gray_frame(64, 64, texture(w, h, 4242));
    let seq = [&a, &b, &a, &b, &a];
    let run = |max_refs: u8| {
        let c = VaultConfig {
            max_refs,
            ..cfg(64, 64)
        };
        let fw = CodecConfig {
            limits: ResourceLimits::permissive(),
            ..CodecConfig::default()
        };
        let mut enc = VaultEncoder::new(fw.clone(), c.clone()).unwrap();
        let mut dec = VaultDecoder::new(fw, c).unwrap();
        let mut bytes = 0usize;
        let mut used_older_ref = 0u32;
        for (i, f) in seq.iter().enumerate() {
            let p = enc.encode(f).unwrap().remove(0);
            dec.decode(&p).unwrap();
            if i >= 2 {
                bytes += p.data.len();
                used_older_ref += dec.last_stats().ref_hist[1..].iter().sum::<u32>();
            }
        }
        (bytes, used_older_ref)
    };
    let (bytes_multi, older) = run(4);
    let (bytes_single, none) = run(1);
    assert!(
        older > 0,
        "no leaf ever named a reference beyond the newest"
    );
    assert_eq!(none, 0, "one reference means only index 0");
    assert!(
        bytes_multi * 2 < bytes_single,
        "4 refs {bytes_multi} B should be far below 1 ref {bytes_single} B on an A/B/A/B cut"
    );
}

#[test]
fn a_repeated_frame_is_almost_all_skip() {
    let px = texture(96, 64, 3);
    let c = cfg(96, 64);
    let mut enc = VaultEncoder::new(CodecConfig::default(), c.clone()).unwrap();
    let mut dec = VaultDecoder::new(CodecConfig::default(), c).unwrap();
    let f = gray_frame(96, 64, px);
    dec.decode(&enc.encode(&f).unwrap()[0]).unwrap();
    let recon = enc.reconstruction().unwrap();
    dec.decode(&enc.encode(&recon).unwrap()[0]).unwrap();
    let s = dec.last_stats();
    assert!(s.skip >= 1 && s.inter == 0 && s.intra == 0, "{s:?}");
    // A skipped 64x64 needs no residual at all.
    assert_eq!(s.coded, 0, "{s:?}");
}
