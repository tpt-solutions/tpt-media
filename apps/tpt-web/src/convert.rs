//! Sample and pixel conversions for browser output.
//!
//! WebAudio wants planar `f32` in `[-1, 1]` (`AudioBuffer.copyToChannel`);
//! a canvas `ImageData` wants interleaved RGBA8. Doing both conversions in
//! Rust keeps the JavaScript side to "copy these bytes into that object"
//! and keeps the maths unit-testable on the host.
//!
//! Every function returns `None` rather than panicking when the input is
//! shorter than its stated geometry: decoder output is trusted less than
//! it should be, and a mismatch is a decoder bug worth surfacing as a
//! counted decode error, not an out-of-bounds slice.

use tpt_media_core::{PixelFormat, SampleFormat};

/// Convert interleaved little-endian samples to planar `f32`
/// (`channels` consecutive planes of `frames` samples each).
///
/// `None` if `channels == 0`, the byte count does not equal
/// `frames * channels * bytes_per_sample`, or the size overflows.
pub fn audio_to_planar_f32(
    format: SampleFormat,
    channels: usize,
    frames: usize,
    bytes: &[u8],
) -> Option<Vec<f32>> {
    if channels == 0 {
        return None;
    }
    let bps = format.bytes_per_sample();
    let total = frames.checked_mul(channels)?;
    if bytes.len() != total.checked_mul(bps)? {
        return None;
    }
    let mut out = vec![0.0f32; total];
    for (i, sample) in bytes.chunks_exact(bps).enumerate() {
        let v = match format {
            SampleFormat::U8 => (f32::from(sample[0]) - 128.0) / 128.0,
            SampleFormat::S16 => f32::from(i16::from_le_bytes([sample[0], sample[1]])) / 32_768.0,
            SampleFormat::S24 => {
                // Sign-extend a 24-bit value through the top of an i32.
                let raw = i32::from_le_bytes([0, sample[0], sample[1], sample[2]]) >> 8;
                raw as f32 / 8_388_608.0
            }
            SampleFormat::S32 => {
                i32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]) as f32
                    / 2_147_483_648.0
            }
            SampleFormat::F32 => f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]),
            SampleFormat::F64 => f64::from_le_bytes([
                sample[0], sample[1], sample[2], sample[3], sample[4], sample[5], sample[6],
                sample[7],
            ]) as f32,
        };
        // Non-finite floats would reach the audio graph as NaN and can
        // poison a whole AudioContext; clamp everything to the legal range.
        let v = if v.is_finite() {
            v.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let (frame, ch) = (i / channels, i % channels);
        out[ch * frames + frame] = v;
    }
    Some(out)
}

#[inline]
fn clamp_u8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Full-range BT.601 YCbCr to RGB, 16.16 fixed point: the colourimetry
/// Thrum documents for its `Yuv420` planes ("BT.601-family, stored
/// full-range 8-bit unsigned").
#[inline]
fn yuv_to_rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
    let y = i32::from(y);
    let u = i32::from(u) - 128;
    let v = i32::from(v) - 128;
    let r = y + ((91_881 * v + 32_768) >> 16);
    let g = y - ((22_554 * u + 46_802 * v + 32_768) >> 16);
    let b = y + ((116_130 * u + 32_768) >> 16);
    [clamp_u8(r), clamp_u8(g), clamp_u8(b)]
}

/// Convert a decoded picture to interleaved RGBA8 (`width * height * 4`
/// bytes, alpha 255 unless the source has alpha).
///
/// `stride` is the luma / packed row stride in bytes, as in
/// [`tpt_media_core::VideoFrame`]. Planar 4:2:0 chroma planes use
/// `stride / 2` and `(width + 1) / 2` by `(height + 1) / 2` samples.
pub fn video_to_rgba(
    format: PixelFormat,
    width: u32,
    height: u32,
    stride: u32,
    pixels: &[u8],
) -> Option<Vec<u8>> {
    let (w, h, stride) = (width as usize, height as usize, stride as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let out_len = w.checked_mul(h)?.checked_mul(4)?;
    // Validate the input geometry against the bytes actually supplied
    // *before* allocating, so a hostile width/height cannot make this
    // allocate more than 4x the bytes the caller really holds.
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    let planar = format == PixelFormat::Yuv420;
    let (min_stride, cstride, chroma_bytes) = match format {
        PixelFormat::Gray8 => (w, 0, 0),
        PixelFormat::Rgb24 => (w.checked_mul(3)?, 0, 0),
        PixelFormat::Rgba32 => (w.checked_mul(4)?, 0, 0),
        PixelFormat::Yuv420 => {
            let cs = stride / 2;
            if cs < cw {
                return None;
            }
            (w, cs, cs.checked_mul(ch)?.checked_mul(2)?)
        }
        PixelFormat::Nv12 => {
            if stride < cw.checked_mul(2)? {
                return None;
            }
            (w, stride, stride.checked_mul(ch)?)
        }
    };
    let luma = stride.checked_mul(h)?;
    if stride < min_stride || pixels.len() < luma.checked_add(chroma_bytes)? {
        return None;
    }
    let mut out = vec![0u8; out_len];
    match format {
        PixelFormat::Gray8 => {
            for y in 0..h {
                let row = &pixels[y * stride..y * stride + w];
                for (x, &g) in row.iter().enumerate() {
                    out[(y * w + x) * 4..][..4].copy_from_slice(&[g, g, g, 255]);
                }
            }
        }
        PixelFormat::Rgb24 => {
            for y in 0..h {
                let row = &pixels[y * stride..y * stride + w * 3];
                for (x, px) in row.chunks_exact(3).enumerate() {
                    out[(y * w + x) * 4..][..4].copy_from_slice(&[px[0], px[1], px[2], 255]);
                }
            }
        }
        PixelFormat::Rgba32 => {
            for y in 0..h {
                out[y * w * 4..(y + 1) * w * 4]
                    .copy_from_slice(&pixels[y * stride..y * stride + w * 4]);
            }
        }
        PixelFormat::Yuv420 | PixelFormat::Nv12 => {
            let (yp, rest) = pixels.split_at(luma);
            for y in 0..h {
                for x in 0..w {
                    let (cx, cy) = (x / 2, y / 2);
                    let (u, v) = if planar {
                        let plane = cstride * ch;
                        (rest[cy * cstride + cx], rest[plane + cy * cstride + cx])
                    } else {
                        (rest[cy * cstride + cx * 2], rest[cy * cstride + cx * 2 + 1])
                    };
                    let rgb = yuv_to_rgb(yp[y * stride + x], u, v);
                    out[(y * w + x) * 4..][..4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s16_mono_maps_to_unit_range() {
        let b: Vec<u8> = [0i16, 16_384, -32_768, 32_767]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let f = audio_to_planar_f32(SampleFormat::S16, 1, 4, &b).unwrap();
        assert_eq!(f[0], 0.0);
        assert_eq!(f[1], 0.5);
        assert_eq!(f[2], -1.0);
        assert!((f[3] - 0.999_969).abs() < 1e-5);
    }

    #[test]
    fn stereo_is_deinterleaved_into_planes() {
        // L R L R = 1 2 3 4 (U8 offset 128).
        let b = [129u8, 130, 131, 132];
        let f = audio_to_planar_f32(SampleFormat::U8, 2, 2, &b).unwrap();
        let s = 1.0 / 128.0;
        assert_eq!(f, vec![s, 3.0 * s, 2.0 * s, 4.0 * s]);
    }

    #[test]
    fn s24_sign_extends() {
        let neg1 = [0xFFu8, 0xFF, 0xFF];
        let f = audio_to_planar_f32(SampleFormat::S24, 1, 1, &neg1).unwrap();
        assert!((f[0] + 1.0 / 8_388_608.0).abs() < 1e-9);
        let min = [0x00u8, 0x00, 0x80];
        assert_eq!(
            audio_to_planar_f32(SampleFormat::S24, 1, 1, &min).unwrap()[0],
            -1.0
        );
    }

    #[test]
    fn non_finite_and_out_of_range_floats_are_neutralised() {
        let vals = [f32::NAN, f32::INFINITY, 3.0, -3.0];
        let b: Vec<u8> = vals.iter().flat_map(|v| v.to_le_bytes()).collect();
        let f = audio_to_planar_f32(SampleFormat::F32, 1, 4, &b).unwrap();
        assert_eq!(f, vec![0.0, 0.0, 1.0, -1.0]);
    }

    #[test]
    fn audio_geometry_mismatch_is_none_not_a_panic() {
        assert!(audio_to_planar_f32(SampleFormat::S16, 1, 4, &[0; 7]).is_none());
        assert!(audio_to_planar_f32(SampleFormat::S16, 0, 4, &[]).is_none());
        assert!(audio_to_planar_f32(SampleFormat::S16, usize::MAX, 2, &[]).is_none());
    }

    #[test]
    fn yuv420_known_colours() {
        // 2x2 picture, one chroma sample: mid grey, then pure-ish red.
        let grey = [128u8, 128, 128, 128, 128, 128];
        let rgba = video_to_rgba(PixelFormat::Yuv420, 2, 2, 2, &grey).unwrap();
        assert_eq!(&rgba[..4], &[128, 128, 128, 255]);
        // BT.601 full range red: Y=76, U=85, V=255.
        let red = [76u8, 76, 76, 76, 85, 255];
        let rgba = video_to_rgba(PixelFormat::Yuv420, 2, 2, 2, &red).unwrap();
        assert!(rgba[0] >= 250 && rgba[1] <= 5 && rgba[2] <= 5, "{rgba:?}");
        assert_eq!(rgba[3], 255);
    }

    #[test]
    fn nv12_matches_yuv420_for_the_same_picture() {
        let planar = [76u8, 200, 30, 90, 85, 255];
        // Same picture, interleaved UV.
        let nv12 = [76u8, 200, 30, 90, 85, 255];
        let a = video_to_rgba(PixelFormat::Yuv420, 2, 2, 2, &planar).unwrap();
        let b = video_to_rgba(PixelFormat::Nv12, 2, 2, 2, &nv12).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn short_or_degenerate_pictures_are_none() {
        assert!(video_to_rgba(PixelFormat::Yuv420, 4, 4, 4, &[0; 23]).is_none());
        assert!(video_to_rgba(PixelFormat::Yuv420, 0, 4, 4, &[]).is_none());
        assert!(video_to_rgba(PixelFormat::Rgba32, 2, 2, 4, &[0; 15]).is_none());
        assert!(video_to_rgba(PixelFormat::Gray8, u32::MAX, u32::MAX, 1, &[0]).is_none());
    }

    #[test]
    fn strided_gray_and_rgb_copy_only_visible_columns() {
        // width 2, stride 4 (2 bytes of padding per row).
        let g = [10u8, 20, 99, 99, 30, 40, 99, 99];
        let rgba = video_to_rgba(PixelFormat::Gray8, 2, 2, 4, &g).unwrap();
        assert_eq!(rgba[0], 10);
        assert_eq!(rgba[4], 20);
        assert_eq!(rgba[8], 30);
        assert_eq!(rgba[12], 40);
    }
}
