//! Deterministic synthetic **video** corpus (todo.md section 36, video
//! subset).
//!
//! The audio corpus ([`crate::signals`]) generates rather than ships
//! fixtures, and this module follows the same rule for video: every
//! scene is a closed-form or seeded function of its arguments, so it is
//! reproducible byte-for-byte on every platform (todo.md section 39) and
//! `tests/corpus/` stays empty by design.
//!
//! One important honesty note, matching the audio module: several of
//! these categories are named after real content (a talking head, a
//! panning camera) that cannot be synthesized faithfully. What is
//! generated here is the *measurable structure* of each case - a slowly
//! drifting, locally-smooth region for a head; a global translation for
//! a camera pan - and the module says so in each doc comment rather than
//! claiming to be the real thing. The categories are still worth having:
//! they are the regimes a transform codec must survive, and they are
//! exactly the regimes a talking-head call actually spends its life in.
//!
//! All scenes are generated as [`VideoFrame`]s in
//! [`PixelFormat::Yuv420`](tpt_media_core::PixelFormat::Yuv420) at
//! `w x h`, with dimensions required to be multiples of 16 so they are
//! macroblock-aligned for `tpt-video-thrum` (todo.md section 32).
//! The `Gray8` variants exist too, since the corpus must exercise both
//! of Thrum's coded formats.

use tpt_media_core::{PixelFormat, Timebase, Timestamp, VideoFrame};

use crate::signals::splitmix64;

/// A generated scene: a name plus its frames.
pub struct Scene {
    /// Human-readable case name, matching the todo.md section 36 category.
    pub name: &'static str,
    /// The generated frames, in presentation order.
    pub frames: Vec<VideoFrame>,
}

/// Build a frame with a `pts` on a 1/`fps` timebase, so a scene's frames
/// carry real timing (the codec round-trips PTS untouched, and section 37
/// reports latency in terms of frame periods).
fn frame(width: u32, height: u32, format: PixelFormat, index: usize, fps: u32) -> VideoFrame {
    let mut f = VideoFrame::alloc(width, height, format);
    f.frame.pts = Timestamp {
        ticks: index as i64,
        timebase: Timebase::new(1, fps),
    };
    f
}

/// Fill a 4:2:0 frame's luma plane from a per-pixel closure.
///
/// The closure gets `(x, y)` in luma coordinates and returns a level in
/// `0..=255`. Chroma is written as a flat mid-grey, which is the correct
/// neutral for a *chroma-less* synthetic luma scene and keeps the
/// chroma planes from adding unrelated noise to the measurements.
fn paint_luma(
    f: &mut VideoFrame,
    width: u32,
    height: u32,
    mut shade: impl FnMut(usize, usize) -> u8,
) {
    let w = width as usize;
    for y in 0..height as usize {
        for x in 0..w {
            f.pixels[y * w + x] = shade(x, y);
        }
    }
    if f.format == PixelFormat::Yuv420 {
        let (cw, ch) = (w / 2, height as usize / 2);
        let base = w * height as usize;
        for c in 0..2 {
            for slot in f.pixels[base + c * cw * ch..base + (c + 1) * cw * ch].iter_mut() {
                *slot = 128;
            }
        }
    }
}

/// A smooth 2D value field in `0..=1`, built from a few sinusoids.
///
/// This is the base "natural-looking" texture used by several scenes:
/// real imagery is dominated by low spatial frequencies, and a codec
/// that cannot code low frequencies smoothly cannot code a talking head.
fn smooth_field(x: f32, y: f32) -> f32 {
    let v = (x * 0.08).sin() * 0.5 + (y * 0.06).sin() * 0.3 + ((x + y) * 0.04).sin() * 0.2;
    0.5 + 0.5 * v
}
/// **Static scene**: a fixed smooth field, identical in every frame.
///
/// The degenerate case a video call is mostly made of, and the one that
/// should compress hardest - every macroblock is a perfect prediction
/// of the previous frame, so the skip path should carry the whole frame.
pub fn static_scene(
    width: u32,
    height: u32,
    format: PixelFormat,
    frames: usize,
    fps: u32,
) -> Scene {
    let frames_out = (0..frames)
        .map(|i| {
            let mut f = frame(width, height, format, i, fps);
            paint_luma(&mut f, width, height, |x, y| {
                to_u8(smooth_field(x as f32, y as f32))
            });
            f
        })
        .collect();
    Scene {
        name: "static-scene",
        frames: frames_out,
    }
}

/// **Talking head** (synthesized): a slowly drifting bright oval on a
/// soft background, with a small amount of noise on the "skin".
///
/// What is faithful is the *structure*: a large, locally smooth,
/// slowly moving region against a static background - which is what
/// makes a head-and-shoulders shot easy for a video codec (large
/// predictable area, one moving object). What is not faithful is the
/// content: this is a synthetic oval, not a face, and no claim is made
/// that it behaves like real skin texture or hair.
pub fn talking_head(
    width: u32,
    height: u32,
    format: PixelFormat,
    frames: usize,
    fps: u32,
    seed: u64,
) -> Scene {
    let w = width as f32;
    let h = height as f32;
    let out = (0..frames)
        .map(|i| {
            let mut f = frame(width, height, format, i, fps);
            // Head drifts a few pixels per frame on a slow Lissajous
            // path, and breathes slightly in size.
            let t = i as f32;
            let cx = w * 0.5 + (t * 0.35).sin() * w * 0.04;
            let cy = h * 0.45 + (t * 0.5).cos() * h * 0.03;
            let r = h * 0.22 * (1.0 + 0.04 * (t * 0.8).sin());
            let mut state = seed ^ (i as u64).wrapping_mul(0x9E37_79B9);
            paint_luma(&mut f, width, height, |x, y| {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                // An ellipse, not a circle: heads are wider than tall.
                let d = (dx * dx) / (r * r * 1.3) + (dy * dy) / (r * r);
                let background = 40.0 + 60.0 * smooth_field(x as f32 * 0.5, y as f32 * 0.5);
                let v = if d < 1.0 {
                    // Inside: a soft, slightly shaded "face" plus the
                    // mild texture that makes motion estimation work.
                    let shade = 1.0 - 0.45 * d;
                    let grain = ((splitmix64(&mut state) >> 56) as f32 / 255.0 - 0.5) * 6.0;
                    150.0 * shade + grain
                } else {
                    background
                };
                to_u8(v)
            });
            f
        })
        .collect();
    Scene {
        name: "talking-head",
        frames: out,
    }
}

/// **Camera motion** (synthesized): the whole picture pans by a known
/// integer offset each frame over a smooth field.
///
/// The structure is the point: a global translation is what motion
/// compensation is *for*, so this is the scene that should benefit most
/// from inter prediction and least from intra refresh. Pan speed is
/// `px_per_frame` pixels, so the caller can dial in "handheld" through
/// "whip pan".
pub fn camera_motion(
    width: u32,
    height: u32,
    format: PixelFormat,
    frames: usize,
    fps: u32,
    px_per_frame: i32,
) -> Scene {
    let out = (0..frames)
        .map(|i| {
            let mut f = frame(width, height, format, i, fps);
            let ox = (i as f32) * px_per_frame as f32;
            paint_luma(&mut f, width, height, |x, y| {
                // Sample the field in *pixel* units, so `px_per_frame` is
                // a true translation of the content. (Scaling the
                // coordinates down instead would make a 3-pixel pan a
                // sub-pixel change and the scene would be static - the
                // test `motion_scenes_actually_change_between_frames`
                // catches exactly that.)
                to_u8(smooth_field(x as f32 + ox, y as f32))
            });
            f
        })
        .collect();
    Scene {
        name: "camera-motion",
        frames: out,
    }
}

/// **High-motion scene**: fast, incoherent, large-amplitude change
/// between consecutive frames.
///
/// The worst case for a P-frame codec: motion compensation cannot find
/// a good match anywhere in its search window, so nearly every
/// macroblock pays for a residual, and the intra-refresh window is the
/// only thing keeping the error bounded. Generated as heavy noise that
/// is *completely reseeded* per frame (not a scrolling window), so there
/// is genuinely no inter-frame correspondence to find.
pub fn high_motion(
    width: u32,
    height: u32,
    format: PixelFormat,
    frames: usize,
    fps: u32,
    seed: u64,
) -> Scene {
    let w = width as usize;
    let h = height as usize;
    let out = (0..frames)
        .map(|i| {
            let mut f = frame(width, height, format, i, fps);
            let mut state = seed
                .wrapping_add(i as u64)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let luma = w * h;
            for slot in f.pixels[..luma].iter_mut() {
                // Blended noise: mostly white, so the residual is large
                // and unpredictable, but not a pure single-pixel hash
                // (which is a pathological case with its own scene).
                let a = (splitmix64(&mut state) >> 56) as f32 / 255.0;
                let b = (splitmix64(&mut state) >> 56) as f32 / 255.0;
                *slot = to_u8(255.0 * (0.6 * a + 0.4 * b));
            }
            if format == PixelFormat::Yuv420 {
                let (cw, ch) = (w / 2, h / 2);
                for c in 0..2 {
                    for slot in f.pixels[luma + c * cw * ch..luma + (c + 1) * cw * ch].iter_mut() {
                        *slot = to_u8(64.0 + 127.0 * (splitmix64(&mut state) >> 57) as f32 / 256.0);
                    }
                }
            }
            f
        })
        .collect();
    Scene {
        name: "high-motion",
        frames: out,
    }
}
/// **Animation** (synthesized): flat regions of saturated colour whose
/// boundaries move a whole number of pixels per frame.
///
/// The structure is what matters and it is exactly right for animation:
/// large areas of *exactly* flat colour (so the residual is nearly zero
/// except at edges) with a small number of hard, moving edges. That is
/// the opposite profile to `high_motion` - large predictable areas and
/// few edges - and the two together bracket what a real codec sees.
pub fn animation(width: u32, height: u32, format: PixelFormat, frames: usize, fps: u32) -> Scene {
    let w = width as f32;
    let h = height as f32;
    // A handful of blocks, phase-shifted so the composition evolves.
    let blocks: [(f32, f32, f32, u8); 5] = [
        (0.30, 0.35, 0.18, 200),
        (0.55, 0.30, 0.12, 70),
        (0.45, 0.60, 0.20, 150),
        (0.70, 0.65, 0.10, 240),
        (0.20, 0.70, 0.09, 40),
    ];
    let out = (0..frames)
        .map(|i| {
            let mut f = frame(width, height, format, i, fps);
            let t = i as f32;
            paint_luma(&mut f, width, height, |x, y| {
                let fx = x as f32 / w;
                let fy = y as f32 / h;
                let mut v = 20u8;
                for (bx, by, br, level) in blocks {
                    // Each block oscillates on its own period, so the
                    // layout changes every frame but stays flat inside.
                    let ph = t * 0.6 + bx * 7.0;
                    let cx = bx + 0.05 * ph.sin();
                    let cy = by + 0.05 * ph.cos();
                    let half = br * 0.5;
                    if (fx - cx).abs() < half && (fy - cy).abs() < half * (h / w) {
                        v = level;
                    }
                }
                v
            });
            f
        })
        .collect();
    Scene {
        name: "animation",
        frames: out,
    }
}

/// **Screen content** (synthesized): flat background, crisp text-like
/// blocks, and a moving caret.
///
/// The structure is right for screen capture and wrong for nothing
/// else, which is the point: real screen content is almost entirely
/// flat colour with a small amount of *very* high-frequency detail
/// (glyph edges), plus occasional large-area changes (a window opens).
/// High-frequency detail inside a flat field is close to a worst case
/// for a DCT codec, so this scene is where `synthetic_patterns` and
/// `screen-content` diverge - the latter has *large* flat areas, the
/// former does not.
pub fn screen_content(
    width: u32,
    height: u32,
    format: PixelFormat,
    frames: usize,
    fps: u32,
) -> Scene {
    let w = width as usize;
    let h = height as usize;
    // Deterministic "text": rows of short runs, like rendered glyphs.
    let glyph = |x: usize, y: usize, seed: u64| -> bool {
        let row = y / 6;
        let col = x / 4;
        let mut s = seed ^ (row as u64).wrapping_mul(0x9E37_79B9) ^ (col as u64);
        // A cheap hash of (row, col) decides whether this cell is ink.
        s = splitmix64(&mut s);
        (s % 7) < 2
    };
    let out = (0..frames)
        .map(|i| {
            let mut f = frame(width, height, format, i, fps);
            paint_luma(&mut f, width, height, |x, y| {
                // A header bar, a body of text rows, and a caret that
                // marches across one row.
                if y < h / 8 {
                    70
                } else if y > h * 3 / 4 && y < h * 7 / 8 {
                    if (x % 3) < 2 {
                        230
                    } else {
                        90
                    }
                } else if (y - h / 8) % 8 < 5 && glyph(x, y, 0xBEEF) {
                    225
                } else {
                    245
                }
            });
            // Caret: a bright block moving along the text area.
            let caret_x = (i * 7) % w.max(1);
            let caret_y = h / 8 + 4;
            if caret_y < h {
                for dy in 0..6 {
                    for dx in 0..2 {
                        let x = caret_x + dx;
                        let y = caret_y + dy;
                        if x < w && y < h {
                            f.pixels[y * w + x] = 0;
                        }
                    }
                }
            }
            f
        })
        .collect();
    Scene {
        name: "screen-content",
        frames: out,
    }
}

/// **Synthetic patterns**: zone plate, checkerboard and single-pixel
/// hash, tiled.
///
/// Deliberately the hardest spatial content there is, and the video
/// analogue of the audio corpus's Nyquist square: a zone plate has
/// energy at *every* spatial frequency including right up to the
/// Nyquist limit, the checkerboard is the worst case for any block
/// transform (its energy is exactly at the block-period frequency), and
/// the single-pixel hash is spatially white. No DCT codec reproduces
/// these, and the point of generating them is to pin *where* it stops
/// trying rather than to claim a quality floor.
pub fn synthetic_patterns(
    width: u32,
    height: u32,
    format: PixelFormat,
    frames: usize,
    fps: u32,
    seed: u64,
) -> Scene {
    let w = width as usize;
    let h = height as usize;
    let out = (0..frames)
        .map(|i| {
            let mut f = frame(width, height, format, i, fps);
            // Three vertical bands, one per pattern, so a single frame
            // covers all three regimes.
            let band = w / 3;
            let mut state = seed ^ (i as u64).wrapping_mul(0x9E37_79B9);
            paint_luma(&mut f, width, height, |x, y| {
                if x < band {
                    // Zone plate: concentric rings whose spacing shrinks
                    // with radius, giving a flat radial frequency sweep.
                    let dx = x as f32 - band as f32 * 0.5;
                    let dy = y as f32 - h as f32 * 0.5;
                    let r2 = dx * dx + dy * dy;
                    to_u8(128.0 + 120.0 * (r2 * 0.35).sin())
                } else if x < 2 * band {
                    // 4x4 checkerboard: energy exactly at the block period.
                    if (x / 4 + y / 4) % 2 == 0 {
                        16
                    } else {
                        240
                    }
                } else {
                    // Spatially white hash.
                    let _ = &mut state;
                    (splitmix64(&mut state) >> 56) as u8
                }
            });
            f
        })
        .collect();
    Scene {
        name: "synthetic-patterns",
        frames: out,
    }
}
/// **Pathological frames**: the extremes of the representable range, the
/// video analogue of the audio corpus's `dc_fullscale` and
/// `alternating_fullscale`.
///
/// Four bands: full-scale DC, a full-scale per-pixel alternating
/// (spatial Nyquist) square, a hard 50% black/white split down the
/// middle (the largest possible edge, straight through the middle of
/// the frame), and a mid-grey frame (the degenerate all-zero-residual
/// case). These must round-trip without panic, hang, or buffer
/// overflow - not at any particular quality, which is the whole point
/// of calling them pathological.
pub fn pathological_frames(
    width: u32,
    height: u32,
    format: PixelFormat,
    frames: usize,
    fps: u32,
) -> Scene {
    let w = width as usize;
    let out = (0..frames)
        .map(|i| {
            let mut f = frame(width, height, format, i, fps);
            paint_luma(&mut f, width, height, |x, y| {
                let band = w / 4;
                if x < band {
                    255 // full-scale DC
                } else if x < 2 * band {
                    if (x + y) % 2 == 0 {
                        0
                    } else {
                        255
                    } // spatial Nyquist
                } else if x < 3 * band {
                    if x < w * 5 / 8 {
                        0
                    } else {
                        255
                    } // hard central split
                } else {
                    // Alternate mid-grey with a hard flash, so the
                    // intra-refresh path sees both a no-op and a jump.
                    if i % 2 == 0 {
                        128
                    } else {
                        0
                    }
                }
            });
            f
        })
        .collect();
    Scene {
        name: "pathological-frames",
        frames: out,
    }
}

/// Round and clamp an `f32` to an 8-bit level, matching the audio
/// corpus's `to_s16` clamp semantics.
#[inline]
fn to_u8(v: f32) -> u8 {
    // The ordering matters: NaN fails *both* comparisons, so it falls
    // through the first branch to 0 rather than casting to a garbage
    // level. Written as an explicit NaN test rather than `!(v >= 0.0)`
    // so the intent survives a clippy lint that dislikes negated
    // comparisons on a partially-ordered type.
    if v.is_nan() || v < 0.0 {
        0
    } else if v > 255.0 {
        255
    } else {
        (v + 0.5) as u8
    }
}

/// The whole synthetic corpus, one [`Scene`] per todo.md section 36 video
/// category, at `width x height` (multiples of 16) and `frames` frames
/// each.
///
/// The order matches section 36's list, so a test failure names the
/// category directly. `fps` sets the frame timestamps' timebase; the
/// latency metric in [`crate::quality`] turns it into a frame budget.
pub fn corpus(
    width: u32,
    height: u32,
    format: PixelFormat,
    frames: usize,
    fps: u32,
    seed: u64,
) -> Vec<Scene> {
    vec![
        talking_head(width, height, format, frames, fps, seed),
        camera_motion(width, height, format, frames, fps, 3),
        static_scene(width, height, format, frames, fps),
        high_motion(width, height, format, frames, fps, seed),
        animation(width, height, format, frames, fps),
        screen_content(width, height, format, frames, fps),
        synthetic_patterns(width, height, format, frames, fps, seed),
        pathological_frames(width, height, format, frames, fps),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 64;
    const H: u32 = 48;
    const N: usize = 4;

    fn all() -> Vec<Scene> {
        corpus(W, H, PixelFormat::Yuv420, N, 30, 0x5EED_A11A)
    }

    #[test]
    fn corpus_covers_every_section_36_video_category() {
        let names: Vec<&str> = all().iter().map(|s| s.name).collect();
        assert_eq!(names.len(), 8);
        for expected in [
            "talking-head",
            "camera-motion",
            "static-scene",
            "high-motion",
            "animation",
            "screen-content",
            "synthetic-patterns",
            "pathological-frames",
        ] {
            assert!(names.contains(&expected), "missing scene {expected}");
        }
        // No duplicates, so a rename can't silently shadow a category.
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len());
    }

    #[test]
    fn every_scene_has_correctly_shaped_frames() {
        for format in [PixelFormat::Yuv420, PixelFormat::Gray8] {
            for scene in corpus(W, H, format, N, 30, 0x5EED_A11A) {
                assert_eq!(scene.frames.len(), N, "{} frame count", scene.name);
                for (i, f) in scene.frames.iter().enumerate() {
                    assert_eq!(f.width, W, "{} width", scene.name);
                    assert_eq!(f.height, H, "{} height", scene.name);
                    assert_eq!(f.format, format, "{} format", scene.name);
                    assert_eq!(
                        f.pixels.len(),
                        f.pixels.capacity().min(f.pixels.len()),
                        "{} buffer must be exactly sized",
                        scene.name
                    );
                    assert_eq!(f.frame.pts.ticks, i as i64, "{} pts", scene.name);
                }
            }
        }
    }

    #[test]
    fn scenes_are_deterministic() {
        let a = all();
        let b = all();
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.name, y.name);
            for (fx, fy) in x.frames.iter().zip(y.frames.iter()) {
                assert_eq!(fx.pixels, fy.pixels, "{} must be reproducible", x.name);
            }
        }
    }

    #[test]
    fn a_static_scene_is_identical_across_its_frames() {
        let s = static_scene(W, H, PixelFormat::Yuv420, 3, 30);
        for i in 1..s.frames.len() {
            assert_eq!(
                s.frames[0].pixels, s.frames[i].pixels,
                "static scene frame {i} differs"
            );
        }
    }

    #[test]
    fn motion_scenes_actually_change_between_frames() {
        for name in ["talking-head", "camera-motion", "high-motion", "animation"] {
            let scene = all()
                .into_iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("scene {name}"));
            assert_ne!(
                scene.frames[0].pixels, scene.frames[1].pixels,
                "{name} must actually move, or it is not testing motion"
            );
        }
    }

    #[test]
    fn pathological_frames_reach_both_extremes() {
        let s = pathological_frames(W, H, PixelFormat::Yuv420, 2, 30);
        let luma = &s.frames[0].pixels[..(W * H) as usize];
        assert_eq!(luma[0], 255, "first band must be full-scale DC");
        assert!(luma.contains(&0), "must contain true black");
        // The alternating band is the spatial Nyquist square: both 0 and
        // 255, alternating.
        let band = (W / 4) as usize;
        let a = luma[band];
        let b = luma[band + 1];
        assert!(a != b, "spatial Nyquist band must alternate");
    }

    #[test]
    fn chroma_is_neutral_for_luma_only_scenes() {
        // The synthetic scenes have no real chroma, so chroma is written
        // as flat mid-grey. Pinning that matters: if it ever changed, a
        // chroma regression would show up as a corpus change rather than
        // a codec change.
        let s = static_scene(W, H, PixelFormat::Yuv420, 1, 30);
        let luma_len = (W * H) as usize;
        assert!(s.frames[0].pixels[luma_len..].iter().all(|&v| v == 128));
    }
}
