//! Input preparation for `tpt encode` and `tpt benchmark`: any WAV sample
//! format in, the 16-bit PCM the audio codecs speak out, with an optional
//! downmix to mono and an optional resample.
//!
//! Every step is explicit about what it changed, so the command can say so
//! in its output. Nothing here is applied silently to the bits the codec
//! sees except what the user can read back from that output.

use tpt_media_core::{AudioFrame, Frame, SampleFormat, Timebase, Timestamp};
use tpt_media_io::normalize_to_f32;

/// Half-width, in input samples, of the windowed-sinc kernel at full
/// bandwidth. 16 taps per side is the usual trade between passband ripple
/// and cost; a downsample widens it (see [`resample_s16`]).
const KERNEL_HALF_WIDTH: f64 = 16.0;

/// Convert any WAV sample format to signed 16-bit, rounding to nearest.
///
/// Integer sources are scaled through `f32` (the same normalisation the
/// rest of the workspace uses), so 24-bit and 32-bit integer and float
/// inputs all land on the same 16-bit grid.
pub fn to_s16(frame: AudioFrame) -> Result<AudioFrame, String> {
    if frame.format == SampleFormat::S16 {
        return Ok(frame);
    }
    let format = frame.format;
    let floats = normalize_to_f32(&frame.samples, format).map_err(|e| e.to_string())?;
    let samples: Vec<u8> = floats
        .iter()
        .flat_map(|s| {
            let v = if s.is_nan() { 0.0 } else { s.clamp(-1.0, 1.0) };
            ((v * 32767.0).round() as i16).to_le_bytes()
        })
        .collect();
    Ok(AudioFrame {
        samples,
        format: SampleFormat::S16,
        ..frame
    })
}

/// Average all channels of a 16-bit frame into one. Callers pass S16 only
/// (run [`to_s16`] first).
pub fn downmix_to_mono(frame: AudioFrame) -> AudioFrame {
    let channels = frame.channel_layout.channels() as usize;
    if channels <= 1 {
        return frame;
    }
    let frames = frame.samples.len() / (2 * channels);
    let mut samples = Vec::with_capacity(frames * 2);
    for f in 0..frames {
        let sum: i32 = (0..channels)
            .map(|c| {
                let i = (f * channels + c) * 2;
                i32::from(i16::from_le_bytes([frame.samples[i], frame.samples[i + 1]]))
            })
            .sum();
        let mean = (sum as f64 / channels as f64).round() as i16;
        samples.extend_from_slice(&mean.to_le_bytes());
    }
    AudioFrame {
        sample_count: frames as u32,
        samples,
        channel_layout: tpt_media_core::ChannelLayout::Mono,
        ..frame
    }
}

/// Resample a 16-bit frame to `to_rate` with a Hann-windowed sinc.
///
/// When downsampling, the cutoff drops to the new Nyquist and the kernel
/// widens by the same ratio, so the anti-alias filter is not the weak
/// point. Each output sample's taps are normalised to unit DC gain, which
/// keeps level exact for constant signals even at the buffer edges, where
/// the input is held at its first and last sample.
pub fn resample_s16(frame: &AudioFrame, to_rate: u32) -> AudioFrame {
    let from_rate = frame.sample_rate;
    let channels = frame.channel_layout.channels() as usize;
    let frames_in = frame.samples.len() / (2 * channels.max(1));
    let ratio = f64::from(to_rate) / f64::from(from_rate);
    let cutoff = ratio.min(1.0);
    let half = KERNEL_HALF_WIDTH / cutoff;
    let frames_out = (frames_in as f64 * ratio).ceil() as usize;

    // Output sample `j` sits at input position `j * from / to`. Its taps depend
    // only on the fractional part of that position, which repeats every
    // `to / gcd(from, to)` outputs, so each phase's taps are computed once.
    let g = gcd(from_rate, to_rate);
    let mut kernels: Vec<Option<Vec<(i64, f64)>>> = vec![None; (to_rate / g) as usize];
    let inputs: Vec<Vec<f64>> = (0..channels)
        .map(|c| {
            (0..frames_in)
                .map(|f| {
                    let i = (f * channels + c) * 2;
                    f64::from(i16::from_le_bytes([frame.samples[i], frame.samples[i + 1]]))
                })
                .collect()
        })
        .collect();

    let mut out = vec![0u8; frames_out * 2 * channels];
    for j in 0..frames_out {
        let pos = j as u64 * u64::from(from_rate);
        let base = (pos / u64::from(to_rate)) as i64;
        let phase = ((pos % u64::from(to_rate)) / u64::from(g)) as usize;
        let taps = kernels[phase].get_or_insert_with(|| {
            let frac = (phase as u64 * u64::from(g)) as f64 / f64::from(to_rate);
            kernel_for_phase(frac, half, cutoff)
        });
        for (c, input) in inputs.iter().enumerate() {
            let v: f64 = taps
                .iter()
                .map(|&(d, w)| w * input[(base + d).clamp(0, frames_in as i64 - 1) as usize])
                .sum();
            let i = (j * channels + c) * 2;
            let s = v.round().clamp(-32768.0, 32767.0) as i16;
            out[i..i + 2].copy_from_slice(&s.to_le_bytes());
        }
    }
    AudioFrame {
        frame: Frame {
            pts: Timestamp::zero(Timebase::new(1, to_rate)),
            duration: None,
            side_data: Vec::new(),
        },
        samples: out,
        sample_count: frames_out as u32,
        format: SampleFormat::S16,
        channel_layout: frame.channel_layout,
        sample_rate: to_rate,
    }
}

/// The taps for an output sample whose position is `frac` past an input
/// sample: `(offset, weight)` pairs, offset relative to that input sample,
/// normalised to unit DC gain.
fn kernel_for_phase(frac: f64, half: f64, cutoff: f64) -> Vec<(i64, f64)> {
    let lo = (frac - half).ceil() as i64;
    let hi = (frac + half).floor() as i64;
    let mut taps: Vec<(i64, f64)> = (lo..=hi)
        .map(|d| {
            let x = frac - d as f64;
            let w = 0.5 + 0.5 * (core::f64::consts::PI * x / half).cos();
            (d, cutoff * sinc(cutoff * x) * w)
        })
        .collect();
    let gain: f64 = taps.iter().map(|t| t.1).sum();
    if gain.abs() > 1e-12 {
        for t in &mut taps {
            t.1 /= gain;
        }
    }
    taps
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let px = core::f64::consts::PI * x;
        px.sin() / px
    }
}

/// Bring a decoded WAV to what `codec` accepts, applying each step the
/// input needs and recording it. Returns the frame and the notes.
///
/// `resample` is the explicit `--resample` target; without it the input
/// rate is kept, and the caller's usual rate check applies. Stereo is
/// downmixed for every codec except `keep_stereo` (Aura takes stereo).
pub fn prepare(
    frame: AudioFrame,
    keep_stereo: bool,
    resample: Option<u32>,
) -> Result<(AudioFrame, Vec<String>), String> {
    let mut notes: Vec<String> = Vec::new();
    let format = frame.format;
    let mut frame = to_s16(frame)?;
    if format != SampleFormat::S16 {
        notes.push(format!("converted {format:?} to 16-bit"));
    }
    if !keep_stereo && frame.channel_layout.channels() > 1 {
        let from = frame.channel_layout.channels();
        frame = downmix_to_mono(frame);
        notes.push(format!("downmixed {from} channels to mono"));
    }
    if let Some(to) = resample {
        if frame.sample_rate == 0 {
            return Err("input wav has a sample rate of 0 Hz".to_string());
        }
        if to != frame.sample_rate {
            let from = frame.sample_rate;
            frame = resample_s16(&frame, to);
            notes.push(format!("resampled {from} to {to} Hz"));
        }
    }
    Ok((frame, notes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tpt_media_core::{ChannelLayout, SampleFormat};

    fn frame(
        rate: u32,
        layout: ChannelLayout,
        format: SampleFormat,
        samples: Vec<u8>,
    ) -> AudioFrame {
        let channels = layout.channels() as usize;
        AudioFrame {
            frame: Frame {
                pts: Timestamp::zero(Timebase::new(1, rate)),
                duration: None,
                side_data: Vec::new(),
            },
            sample_count: (samples.len() / (format.bytes_per_sample() * channels)) as u32,
            samples,
            format,
            channel_layout: layout,
            sample_rate: rate,
        }
    }

    fn s16_bytes(values: &[i16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn s16_values(f: &AudioFrame) -> Vec<i16> {
        f.samples
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect()
    }

    #[test]
    fn float_and_24_bit_inputs_land_on_the_16_bit_grid() {
        let f32s: Vec<u8> = [0.0f32, 0.5, -1.0, 2.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let out = to_s16(frame(48_000, ChannelLayout::Mono, SampleFormat::F32, f32s)).unwrap();
        assert_eq!(out.format, SampleFormat::S16);
        // 2.0 clamps to full scale; 0.5 is half of 32767 rounded.
        assert_eq!(s16_values(&out), vec![0, 16384, -32767, 32767]);

        // 24-bit: full-scale positive is 0x7FFFFF.
        let s24 = vec![0xFF, 0xFF, 0x7F, 0x00, 0x00, 0x00];
        let out = to_s16(frame(48_000, ChannelLayout::Mono, SampleFormat::S24, s24)).unwrap();
        assert_eq!(s16_values(&out), vec![32767, 0]);
    }

    #[test]
    fn downmix_averages_channels() {
        let f = frame(
            48_000,
            ChannelLayout::Stereo,
            SampleFormat::S16,
            s16_bytes(&[100, 300, -50, 50]),
        );
        let m = downmix_to_mono(f);
        assert_eq!(m.channel_layout.channels(), 1);
        assert_eq!(m.sample_count, 2);
        assert_eq!(s16_values(&m), vec![200, 0]);
    }

    #[test]
    fn resample_keeps_a_tone_at_its_frequency_and_its_level() {
        // 1 kHz at 48 kHz, down to 16 kHz: the tone must survive, not alias.
        let n = 4_800;
        let tone: Vec<i16> = (0..n)
            .map(|i| {
                ((i as f64 * 2.0 * core::f64::consts::PI * 1000.0 / 48_000.0).sin() * 10_000.0)
                    as i16
            })
            .collect();
        let f = frame(
            48_000,
            ChannelLayout::Mono,
            SampleFormat::S16,
            s16_bytes(&tone),
        );
        let out = resample_s16(&f, 16_000);
        assert_eq!(out.sample_rate, 16_000);
        assert_eq!(out.sample_count, 1_600);
        let got = s16_values(&out);
        // Peak in the steady middle, away from the edge hold.
        let peak = got[200..1_400].iter().map(|v| v.abs()).max().unwrap();
        assert!((9_500..=10_100).contains(&peak), "peak {peak}");
    }

    #[test]
    fn resample_is_unity_gain_for_dc() {
        let f = frame(
            44_100,
            ChannelLayout::Mono,
            SampleFormat::S16,
            s16_bytes(&vec![5_000; 2_000]),
        );
        let up = resample_s16(&f, 48_000);
        let mid = &s16_values(&up)[300..up.sample_count as usize - 300];
        assert!(mid.iter().all(|&v| (4_999..=5_001).contains(&v)), "{mid:?}");
    }

    #[test]
    fn prepare_reports_each_change() {
        let stereo_f32 = frame(
            44_100,
            ChannelLayout::Stereo,
            SampleFormat::F32,
            vec![0u8; 4 * 2 * 100],
        );
        let (out, notes) = prepare(stereo_f32, false, Some(48_000)).unwrap();
        assert_eq!(out.channel_layout.channels(), 1);
        assert_eq!(out.sample_rate, 48_000);
        assert_eq!(
            notes,
            vec![
                "converted F32 to 16-bit".to_string(),
                "downmixed 2 channels to mono".to_string(),
                "resampled 44100 to 48000 Hz".to_string(),
            ]
        );
    }

    /// The direct per-sample form the cached kernels must reproduce.
    fn resample_reference(input: &[f64], from: u32, to: u32) -> Vec<f64> {
        let ratio = f64::from(to) / f64::from(from);
        let cutoff = ratio.min(1.0);
        let half = KERNEL_HALF_WIDTH / cutoff;
        let frames_out = (input.len() as f64 * ratio).ceil() as usize;
        (0..frames_out)
            .map(|j| {
                let t = j as f64 / ratio;
                let (mut acc, mut gain) = (0f64, 0f64);
                for k in (t - half).ceil() as i64..=(t + half).floor() as i64 {
                    let x = t - k as f64;
                    let w = 0.5 + 0.5 * (core::f64::consts::PI * x / half).cos();
                    let h = cutoff * sinc(cutoff * x) * w;
                    acc += h * input[k.clamp(0, input.len() as i64 - 1) as usize];
                    gain += h;
                }
                if gain.abs() > 1e-12 {
                    acc / gain
                } else {
                    0.0
                }
            })
            .collect()
    }

    #[test]
    fn cached_kernels_match_the_direct_form() {
        for &(from, to) in &[(44_100u32, 48_000u32), (48_000, 16_000), (8_000, 48_000)] {
            let n = 3_000;
            let values: Vec<i16> = (0..n)
                .map(|i| {
                    let t = i as f64;
                    ((t * 0.07).sin() * 9_000.0 + (t * 0.61).cos() * 3_000.0) as i16
                })
                .collect();
            let f = frame(
                from,
                ChannelLayout::Mono,
                SampleFormat::S16,
                s16_bytes(&values),
            );
            let got = s16_values(&resample_s16(&f, to));
            let input: Vec<f64> = values.iter().map(|&v| f64::from(v)).collect();
            let want = resample_reference(&input, from, to);
            assert_eq!(got.len(), want.len(), "{from}->{to}");
            for (j, (g, w)) in got.iter().zip(&want).enumerate() {
                let w = w.round().clamp(-32768.0, 32767.0) as i64;
                assert!(
                    (i64::from(*g) - w).abs() <= 1,
                    "{from}->{to} sample {j}: {g} vs {w}"
                );
            }
        }
    }
}
