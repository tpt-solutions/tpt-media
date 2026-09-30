//! YUV4MPEG2 (`.y4m`) reading and writing — the video counterpart of
//! the WAV bridge the audio commands use (todo.md §32's CLI wiring).
//!
//! Y4M is the "WAV of raw video": a one-line self-describing header
//! followed by `FRAME\n`-delimited uncompressed frames. It is the
//! format every encoder tool pipes around, it needs no external
//! dependency, and being text-headed it fails loudly on the wrong
//! input instead of decoding garbage. Like the WAV bridge, this module
//! is byte-exact: what goes in is what comes out, so a lossless codec
//! round trip through the CLI is a byte-identical file.
//!
//! What is accepted, deliberately narrow:
//!
//! - progressive only (`Ip`) — Thrum has no field coding; interlaced
//!   headers are rejected with an explicit message rather than
//!   silently treating fields as frames;
//! - 4:2:0 (`C420jpeg`, `C420mpeg2`, `C420paldv`, and the ambiguous
//!   `C420`, all mapped to [`PixelFormat::Yuv420`] — the three named
//!   variants differ only in chroma siting, which this bridge neither
//!   alters nor preserves) and monochrome (`Cmono`, mapped to
//!   [`PixelFormat::Gray8`]);
//! - everything else (4:2:2, 4:4:4, 10-bit, unknown tags) is rejected
//!   by name.
//!
//! Aspect ratio is parsed and ignored: the `.tpt` track model has no
//! aspect field, so round-tripping one is out of scope, and `A0:0`
//! ("unspecified") is what this writer emits.
//!
//! Frames carry no timestamps in Y4M; the CLI assigns frame `i` PTS
//! `i` on the `fps_den/fps_num` timebase (frame `i` plays at
//! `i · den/num` seconds).

use tpt_media_core::{PixelFormat, VideoFrame};

/// First-line magic of every YUV4MPEG2 stream.
const MAGIC: &str = "YUV4MPEG2";
/// Frame delimiter line.
const FRAME_MAGIC: &str = "FRAME";

/// The stream-level parameters a parsed header yields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Y4mParams {
    /// Luma width in pixels.
    pub width: u32,
    /// Luma height in pixels.
    pub height: u32,
    /// Frame-rate numerator (`F<num>:<den>`).
    pub fps_num: u32,
    /// Frame-rate denominator (`F<num>:<den>`).
    pub fps_den: u32,
    /// Pixel layout; only [`PixelFormat::Yuv420`] and
    /// [`PixelFormat::Gray8`] are produced.
    pub format: PixelFormat,
}

impl Y4mParams {
    /// The `C<colorspace>` tag for these parameters.
    fn colorspace_tag(&self) -> &'static str {
        match self.format {
            PixelFormat::Yuv420 => "C420jpeg",
            PixelFormat::Gray8 => "Cmono",
            other => unreachable!("y4m bridge only handles yuv420/gray8, got {other:?}"),
        }
    }
}

/// A parse error: the message is user-facing (it names the offending
/// tag or count).
pub type ParseResult<T> = Result<T, String>;

fn plane_count(format: PixelFormat) -> usize {
    match format {
        PixelFormat::Yuv420 => 3,
        PixelFormat::Gray8 => 1,
        other => unreachable!("y4m bridge only handles yuv420/gray8, got {other:?}"),
    }
}

/// Chroma plane dimensions for 4:2:0 (half width, half height).
fn chroma_dims(width: u32, height: u32) -> (usize, usize) {
    ((width / 2) as usize, (height / 2) as usize)
}

/// Expected total payload bytes of one frame in these parameters.
fn frame_bytes(p: &Y4mParams) -> usize {
    let luma = p.width as usize * p.height as usize;
    match p.format {
        PixelFormat::Yuv420 => {
            let (cw, ch) = chroma_dims(p.width, p.height);
            luma + 2 * cw * ch
        }
        PixelFormat::Gray8 => luma,
        other => unreachable!("y4m bridge only handles yuv420/gray8, got {other:?}"),
    }
}

/// Parse one `TAG value` token of the header line.
fn parse_header_line(line: &str) -> ParseResult<Y4mParams> {
    let mut tokens = line.split_whitespace();
    if tokens.next() != Some(MAGIC) {
        return Err(format!(
            "not a y4m stream (expected leading {MAGIC} marker)"
        ));
    }
    let mut width = None;
    let mut height = None;
    let mut fps = None;
    let mut interlacing: Option<char> = None;
    let mut colorspace: Option<String> = None;
    for tok in tokens {
        let mut chars = tok.chars();
        let tag = chars.next().unwrap_or('\0');
        let value = chars.as_str();
        match tag {
            'W' => {
                width = Some(value.parse().map_err(|_| format!("bad width `{value}`"))?)
            }
            'H' => {
                height = Some(value.parse().map_err(|_| format!("bad height `{value}`"))?)
            }
            'F' => {
                let (n, d) = value.split_once(':').ok_or_else(|| {
                    format!("bad frame rate `{value}` (expected NUM:DEN)")
                })?;
                let n: u32 = n.parse().map_err(|_| format!("bad frame rate `{value}`"))?;
                let d: u32 = d.parse().map_err(|_| format!("bad frame rate `{value}`"))?;
                if n == 0 || d == 0 {
                    return Err(format!("frame rate `{value}` has a zero component"));
                }
                fps = Some((n, d));
            }
            'I' => interlacing = Some(value.chars().next().unwrap_or('?')),
            'C' => colorspace = Some(value.to_string()),
            // A<num>:<num> (aspect ratio) and X* (extension tags) are
            // parsed and ignored — see the module doc.
            'A' | 'X' => {}
            other => return Err(format!("unknown y4m header tag `{other}{value}`")),
        }
    }
    let width = width.ok_or("y4m header is missing W (width)")?;
    let height = height.ok_or("y4m header is missing H (height)")?;
    let (fps_num, fps_den) = fps.ok_or("y4m header is missing F (frame rate)")?;
    if let Some('p') = interlacing {
        // progressive: what we want
    } else if let Some(i) = interlacing {
        return Err(format!(
            "interlaced y4m input (I{i}) is not supported; deinterlace first"
        ));
    }
    let format = match colorspace.as_deref() {
        Some("C420jpeg") | Some("C420mpeg2") | Some("C420paldv") | Some("C420") => {
            PixelFormat::Yuv420
        }
        Some("Cmono") => PixelFormat::Gray8,
        Some(other) => {
            return Err(format!(
                "y4m colorspace `{other}` is not supported (only 4:2:0 and Cmono)"
            ))
        }
        None => {
            return Err(
                "y4m header is missing C (colorspace); ambiguous streams are rejected".into(),
            )
        }
    };
    if width == 0 || height == 0 {
        return Err("y4m dimensions must be non-zero".into());
    }
    Ok(Y4mParams {
        width,
        height,
        fps_num,
        fps_den,
        format,
    })
}

/// Parse a complete YUV4MPEG2 stream: header plus every `FRAME`-delimited
/// frame, assigned PTS `i` on the `fps_den/fps_num` timebase.
///
/// The plane bytes are copied verbatim — this bridge is byte-exact by
/// contract, so a lossless codec round trip reproduces the input file.
pub fn parse(bytes: &[u8]) -> ParseResult<(Y4mParams, Vec<VideoFrame>)> {
    let mut split = bytes.splitn(2, |&b| b == b'\n');
    let header = split.next().unwrap_or(&bytes[0..0]);
    let mut rest = split.next().unwrap_or(&bytes[0..0]);
    let params = parse_header_line(&String::from_utf8_lossy(header))?;
    let per_frame = frame_bytes(&params);

    let mut frames = Vec::new();
    let mut index: u64 = 0;
    while !rest.is_empty() {
        // Frame delimiter: "FRAME[ params]\n".
        let mut parts = rest.splitn(2, |&b| b == b'\n');
        let line = parts.next().unwrap_or(&rest[0..0]);
        rest = parts.next().unwrap_or(&rest[0..0]);
        let line_str = String::from_utf8_lossy(line);
        let line_tok = line_str.split_whitespace().next().unwrap_or("");
        if line_tok != FRAME_MAGIC {
            return Err(format!(
                "expected a {FRAME_MAGIC} delimiter, found `{}`",
                line_str.lines().next().unwrap_or("").chars().take(32).collect::<String>()
            ));
        }
        if rest.len() < per_frame {
            return Err(format!(
                "truncated frame {} (expected {per_frame} payload bytes, found {})",
                index,
                rest.len()
            ));
        }
        let mut f = VideoFrame::alloc(params.width, params.height, params.format);
        f.pixels.copy_from_slice(&rest[..per_frame]);
        f.frame.pts = tpt_media_core::Timestamp {
            ticks: index as i64,
            timebase: tpt_media_core::Timebase::new(params.fps_den, params.fps_num),
        };
        frames.push(f);
        rest = &rest[per_frame..];
        index += 1;
    }
    if frames.is_empty() {
        return Err("y4m stream contains no frames".into());
    }
    Ok((params, frames))
}

/// Serialize `frames` as YUV4MPEG2 under `params`.
///
/// Panics if a frame does not match `params`' geometry/format — the
/// callers build frames from the same parameters they write, so a
/// mismatch is a programming error, and `expect` says so plainly.
pub fn write(params: &Y4mParams, frames: &[VideoFrame]) -> Vec<u8> {
    let per_frame = frame_bytes(params);
    let mut out = Vec::with_capacity(
        128 + frames.len() * (FRAME_MAGIC.len() + 1 + per_frame),
    );
    out.extend_from_slice(
        format!(
            "{MAGIC} W{} H{} F{}:{} Ip A0:0 {}\n",
            params.width,
            params.height,
            params.fps_num,
            params.fps_den,
            params.colorspace_tag()
        )
        .as_bytes(),
    );
    for f in frames {
        assert!(
            f.width == params.width
                && f.height == params.height
                && f.format == params.format
                && f.pixels.len() == per_frame,
            "frame does not match the y4m parameters being written"
        );
        out.extend_from_slice(b"FRAME\n");
        out.extend_from_slice(&f.pixels);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_media_core::Timebase;

    fn frame(w: u32, h: u32, format: PixelFormat, i: usize) -> VideoFrame {
        let mut f = VideoFrame::alloc(w, h, format);
        for (slot, idx) in f.pixels.iter_mut().zip(0..) {
            *slot = (idx % 251) as u8;
        }
        f.frame.pts = tpt_media_core::Timestamp {
            ticks: i as i64,
            timebase: Timebase::new(1, 30),
        };
        f
    }

    #[test]
    fn yuv420_round_trips_byte_exactly() {
        let frames: Vec<_> = (0..3).map(|i| frame(176, 144, PixelFormat::Yuv420, i)).collect();
        let params = Y4mParams {
            width: 176,
            height: 144,
            fps_num: 30,
            fps_den: 1,
            format: PixelFormat::Yuv420,
        };
        let bytes = write(&params, &frames);
        let (parsed, out) = parse(&bytes).expect("parse");
        assert_eq!(parsed, params);
        assert_eq!(out.len(), 3);
        for (a, b) in frames.iter().zip(&out) {
            assert_eq!(a.pixels, b.pixels, "plane bytes must survive verbatim");
            assert_eq!(a.frame.pts.ticks, b.frame.pts.ticks);
        }
    }

    #[test]
    fn gray8_round_trips_byte_exactly() {
        let frames: Vec<_> = (0..2).map(|i| frame(64, 48, PixelFormat::Gray8, i)).collect();
        let params = Y4mParams {
            width: 64,
            height: 48,
            fps_num: 25,
            fps_den: 1,
            format: PixelFormat::Gray8,
        };
        let bytes = write(&params, &frames);
        let (parsed, out) = parse(&bytes).expect("parse");
        assert_eq!(parsed.format, PixelFormat::Gray8);
        assert_eq!(out.len(), 2);
        assert_eq!(frames[0].pixels, out[0].pixels);
    }

    #[test]
    fn header_variants_parse() {
        let (p, _) = parse(b"YUV4MPEG2 W32 H16 F30000:1001 Ip A1:1 C420mpeg2\nFRAME\n\x00")
            .expect("C420mpeg2");
        assert_eq!((p.fps_num, p.fps_den), (30_000, 1_001));
        assert!(parse(b"YUV4MPEG2 W32 H16 F30:1 Ip A0:0 C420\nFRAME\n\x00").is_ok());
        assert!(parse(b"YUV4MPEG2 W32 H16 F30:1 It A0:0 C420jpeg\nFRAME\n").is_err());
        assert!(parse(b"YUV4MPEG2 W32 H16 F30:1 Ip A0:0 C422\nFRAME\n").is_err());
        assert!(parse(b"YUV4MPEG2 W32 H16 F30:1 Ip A0:0 C420p10\nFRAME\n").is_err());
        assert!(parse(b"RIFF....WAVEfmt ").is_err());
        assert!(parse(b"YUV4MPEG2 W32 H16 Ip A0:0 C420jpeg\nFRAME\n").is_err());
    }

    #[test]
    fn truncation_and_empty_streams_are_errors() {
        let full = write(
            &Y4mParams {
                width: 16,
                height: 16,
                fps_num: 30,
                fps_den: 1,
                format: PixelFormat::Yuv420,
            },
            &[frame(16, 16, PixelFormat::Yuv420, 0)],
        );
        // Cut into the plane payload.
        assert!(parse(&full[..full.len() - 10]).is_err());
        // A header with no frames at all.
        assert!(parse(b"YUV4MPEG2 W16 H16 F30:1 Ip A0:0 C420jpeg\n").is_err());
    }
}
