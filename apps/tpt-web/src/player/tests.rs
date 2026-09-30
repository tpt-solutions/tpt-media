//! Player tests: real encoders in, real decoders out (via `synth`).

#![cfg(feature = "synth")]

use super::*;
use crate::synth::{self, AudioKind};
use tpt_container::TptMuxer;
use tpt_media_codec::{Capabilities, CodecInfo, CodecResult, ResourceLimits};
use tpt_media_core::{ChannelLayout, CodecId, SampleFormat, Track, TrackMedia};

#[derive(Debug, Default)]
struct Collected {
    audio: Vec<AudioChunk>,
    video: Vec<VideoPicture>,
    peak_memory: usize,
    peak_audio_ms: f64,
    peak_video: usize,
    status: Option<Status>,
}

impl Collected {
    fn samples(&self) -> usize {
        self.audio.iter().map(|c| c.frames as usize).sum()
    }
    fn mono(&self) -> Vec<f32> {
        self.audio.iter().flat_map(|c| c.planar.clone()).collect()
    }
}

/// Drive a player over `file` in `slice`-byte pushes, honouring
/// back-pressure, popping output every round, until it ends.
fn run(player: &mut Player, file: &[u8], slice: usize) -> Result<Collected, WebError> {
    let mut out = Collected::default();
    let record = |p: &mut Player, out: &mut Collected| {
        out.peak_memory = out.peak_memory.max(p.memory_in_use());
        out.peak_audio_ms = out.peak_audio_ms.max(p.audio_queued_ms());
        out.peak_video = out.peak_video.max(p.video_queued());
        while let Some(a) = p.pop_audio() {
            out.audio.push(a);
        }
        while let Some(v) = p.pop_video() {
            out.video.push(v);
        }
    };
    for chunk in file.chunks(slice) {
        loop {
            match player.push(chunk) {
                Ok(()) => break,
                Err(WebError::InputFull) => {
                    player.pump()?;
                    record(player, &mut out);
                }
                Err(e) => return Err(e),
            }
        }
        player.pump()?;
        record(player, &mut out);
    }
    player.finish_input();
    for _ in 0..100_000 {
        let st = player.pump()?;
        record(player, &mut out);
        if st == Status::Ended {
            out.status = Some(st);
            return Ok(out);
        }
    }
    panic!("player did not terminate");
}

fn pcm_of(chunks: &Collected) -> Vec<i16> {
    chunks
        .mono()
        .iter()
        .map(|s| (s * 32_768.0).round() as i16)
        .collect()
}

#[test]
fn aura_lossless_reproduces_the_source_bit_exactly() {
    let file = synth::audio_tpt(AudioKind::Aura, 48_000, 0.5);
    let mut p = Player::new(WebLimits::default());
    let out = run(&mut p, &file, 4096).unwrap();
    let expected = synth::tone(48_000, 0.5);
    assert_eq!(out.samples(), expected.len(), "padding must be trimmed");
    assert_eq!(pcm_of(&out), expected);
    assert_eq!(
        p.info().audio,
        Some(AudioInfo {
            codec: "aura",
            sample_rate: 48_000,
            channels: Some(1)
        })
    );
    assert!(p.info().video.is_none());
    assert_eq!(p.stats().decode_errors, 0);
}

#[test]
fn audio_timestamps_are_contiguous_and_start_at_zero() {
    for (kind, rate) in [
        (AudioKind::Cinder, 44_100),
        (AudioKind::Aura, 48_000),
        (AudioKind::Reed, 16_000),
    ] {
        let file = synth::audio_tpt(kind, rate, 0.4);
        let mut p = Player::new(WebLimits::default());
        let out = run(&mut p, &file, 1000).unwrap();
        assert_eq!(out.samples(), (rate as f32 * 0.4) as usize, "{kind:?}");
        assert_eq!(out.audio[0].pts_seconds, 0.0, "{kind:?}");
        for w in out.audio.windows(2) {
            let end = w[0].pts_seconds + w[0].duration_seconds();
            assert!((w[1].pts_seconds - end).abs() < 1e-9, "{kind:?}");
            assert_eq!(w[0].sample_rate, rate);
            assert_eq!(w[0].channels, 1);
        }
        assert!(out.mono().iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        assert!(
            out.mono().iter().any(|s| s.abs() > 0.05),
            "{kind:?} is silent"
        );
    }
}

#[test]
fn cinder_matches_a_directly_driven_decoder() {
    use tpt_audio_cinder::{CinderDecoder, CinderEncoderConfig};
    use tpt_media_codec::{CodecConfig, Decoder};
    let file = synth::audio_tpt(AudioKind::Cinder, 48_000, 0.3);
    let (_h, tracks, packets) = tpt_container::parse_tpt(&file).unwrap();
    let mut dec = CinderDecoder::new(
        CodecConfig {
            version: tracks[0].codec_config.version,
            extra: tracks[0].codec_config.data.clone(),
            limits: ResourceLimits::default(),
        },
        CinderEncoderConfig {
            sample_rate: 48_000,
            bits_per_coef: tracks[0].codec_config.data[0],
            ..CinderEncoderConfig::default()
        },
    )
    .unwrap();
    let mut reference: Vec<f32> = Vec::new();
    for pkt in &packets {
        for f in dec.decode(pkt).unwrap() {
            reference.extend(
                f.samples
                    .chunks_exact(2)
                    .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32_768.0),
            );
        }
    }
    let mut p = Player::new(WebLimits::default());
    let out = run(&mut p, &file, 777).unwrap();
    let got = out.mono();
    assert_eq!(got.len(), (48_000.0f32 * 0.3) as usize);
    // The player trims the padding tail; everything before it is identical.
    assert_eq!(got[..], reference[..got.len()]);
}

#[test]
fn video_frames_arrive_with_geometry_timestamps_and_plausible_pixels() {
    let (w, h, fps, n) = (64u32, 48u32, 12u32, 8u32);
    let file = synth::video_tpt(w, h, fps, n);
    let mut p = Player::new(WebLimits::default());
    let out = run(&mut p, &file, 2048).unwrap();
    assert_eq!(out.video.len(), n as usize);
    assert!(out.audio.is_empty());
    let info = p.info().video.clone().unwrap();
    assert_eq!(
        (
            info.codec,
            info.width,
            info.height,
            info.fps_num,
            info.fps_den
        ),
        ("thrum", w, h, fps, 1)
    );
    for (i, pic) in out.video.iter().enumerate() {
        assert_eq!((pic.width, pic.height), (w, h));
        assert_eq!(pic.rgba.len(), (w * h * 4) as usize);
        let want = i as f64 / f64::from(fps);
        assert!((pic.pts_seconds() - want).abs() < 1e-5, "frame {i}");
        // Lossy but close to the source picture rendered the same way.
        let src = synth::test_picture(w, h, i as u32, fps);
        let src_rgba = video_to_rgba(PixelFormat::Yuv420, w, h, src.stride, &src.pixels).unwrap();
        let mae: f64 = pic
            .rgba
            .iter()
            .zip(&src_rgba)
            .map(|(a, b)| f64::from(a.abs_diff(*b)))
            .sum::<f64>()
            / pic.rgba.len() as f64;
        assert!(mae < 20.0, "frame {i}: mean abs error {mae}");
    }
}

use tpt_media_core::PixelFormat;

#[test]
fn audio_and_video_play_together() {
    let file = synth::av_tpt(AudioKind::Cinder, 48_000, 0.5, 48, 32, 10);
    let mut p = Player::new(WebLimits::default());
    let out = run(&mut p, &file, 3000).unwrap();
    assert_eq!(out.samples(), 24_000);
    assert_eq!(out.video.len(), 5);
    let info = p.info();
    assert!(info.audio.is_some() && info.video.is_some());
    assert_eq!(info.ignored_tracks, 0);
}

#[test]
fn byte_at_a_time_output_is_identical_to_one_big_push() {
    let file = synth::av_tpt(AudioKind::Aura, 44_100, 0.2, 32, 32, 10);
    let mut a = Player::new(WebLimits::default());
    let whole = run(&mut a, &file, file.len()).unwrap();
    let mut b = Player::new(WebLimits::default());
    let drip = run(&mut b, &file, 1).unwrap();
    assert_eq!(whole.audio, drip.audio);
    assert_eq!(whole.video, drip.video);
}

#[test]
fn back_pressure_bounds_the_queues_and_memory() {
    let limits = WebLimits {
        max_audio_queue_ms: 100,
        max_video_queue_frames: 2,
        ..WebLimits::default()
    };
    let file = synth::av_tpt(AudioKind::Aura, 48_000, 2.0, 32, 32, 15);

    // Nobody pops: pump must stop with queues near their limits, not decode
    // the whole file into memory.
    let mut p = Player::new(limits);
    let mut fed = 0;
    for chunk in file.chunks(2000) {
        if p.push(chunk).is_err() {
            break;
        }
        fed += chunk.len();
        p.pump().unwrap();
    }
    assert!(fed > 0);
    assert_eq!(p.status(), Status::Ready);
    // Overshoot is at most the one packet that crossed the limit
    // (Aura: 1024 samples = 21.3 ms).
    assert!(
        p.audio_queued_ms() < 100.0 + 22.0,
        "{}",
        p.audio_queued_ms()
    );
    assert!(p.video_queued() <= 2 + 1);
    assert!((p.audio_queued_ms() > 0.0) || p.video_queued() > 0);

    // A full run with a consumer that pops everything each round stays
    // inside the same bounds and inside the documented memory ceiling.
    let mut p = Player::new(limits);
    let out = run(&mut p, &file, 2000).unwrap();
    assert_eq!(out.samples(), 96_000);
    assert_eq!(out.video.len(), 30);
    assert!(out.peak_audio_ms < 100.0 + 22.0, "{}", out.peak_audio_ms);
    assert!(out.peak_video <= 3);
    assert!(
        (out.peak_memory as u64) < p.limits().memory_ceiling_bytes(),
        "{} vs {}",
        out.peak_memory,
        p.limits().memory_ceiling_bytes()
    );
}

#[test]
fn a_full_input_buffer_refuses_bytes_until_output_is_drained() {
    let file = synth::audio_tpt(AudioKind::Aura, 48_000, 3.0);
    let limits = WebLimits {
        max_input_buffer_bytes: 1, // clamped up to the one-chunk floor
        max_audio_queue_ms: 50,
        ..WebLimits::default()
    };
    let mut p = Player::new(limits);
    let cap = p.limits().max_input_buffer_bytes as usize;
    let mut got_full = false;
    // Push everything without pumping: must hit InputFull instead of
    // growing without bound.
    for chunk in file.chunks(64 * 1024) {
        match p.push(chunk) {
            Ok(()) => {}
            Err(WebError::InputFull) => {
                got_full = true;
                break;
            }
            Err(e) => panic!("{e}"),
        }
    }
    if file.len() > cap {
        assert!(got_full, "file {} cap {}", file.len(), cap);
    }
    assert!(p.memory_in_use() <= 2 * cap + 64 * 1024);
}

#[test]
fn a_stream_cut_off_mid_chunk_ends_cleanly() {
    let file = synth::av_tpt(AudioKind::Cinder, 48_000, 0.5, 32, 32, 10);
    let mut saw_truncation = false;
    for cut in [file.len() / 3, file.len() / 2, file.len() - 3] {
        let mut p = Player::new(WebLimits::default());
        let out = run(&mut p, &file[..cut], 1500).unwrap();
        assert_eq!(out.status, Some(Status::Ended));
        assert!(out.samples() > 0 || !out.video.is_empty());
        assert!(p.error().is_none());
        saw_truncation |= p.stats().truncated_input;
    }
    assert!(saw_truncation, "no cut landed mid-chunk");
}

#[test]
fn empty_and_non_tpt_input_fail_cleanly_and_stickily() {
    let mut p = Player::new(WebLimits::default());
    p.finish_input();
    assert!(matches!(
        p.pump(),
        Err(WebError::Container(ContainerFault::Truncated))
    ));
    assert_eq!(p.status(), Status::Failed);
    assert!(p.pump().is_err());
    assert!(p.push(b"x").is_err());

    let mut p = Player::new(WebLimits::default());
    p.push(b"RIFF....WAVEfmt ").unwrap();
    assert!(p.pump().is_err());
    assert_eq!(p.status(), Status::Failed);
    assert_eq!(p.memory_in_use(), 0);
}

#[test]
fn push_after_finish_is_rejected() {
    let mut p = Player::new(WebLimits::default());
    p.finish_input();
    assert_eq!(p.push(b"TPTM"), Err(WebError::InputClosed));
}

fn audio_track(id: u8, codec: &[u8; 4], rate: u32, private: Vec<u8>) -> Track {
    let mut t = Track::new(
        id,
        MediaType::Audio,
        CodecId::new(*codec),
        Timebase::new(1, rate),
    );
    t.codec_config.data = private;
    t.media = TrackMedia {
        sample_rate: Some(rate),
        channel_layout: Some(ChannelLayout::Mono),
        ..TrackMedia::default()
    };
    t
}

fn packet(track: u8, pts: i64, tb: Timebase, data: Vec<u8>) -> Packet {
    let mut p = Packet::empty(
        track,
        tpt_media_core::Timestamp {
            ticks: pts,
            timebase: tb,
        },
    );
    p.data = data;
    p.flags |= PacketFlags::KEYFRAME;
    p
}

#[test]
fn a_codec_the_table_lacks_is_not_playable() {
    let tb = Timebase::new(1, 48_000);
    let track = audio_track(0, b"NOPE", 48_000, vec![]);
    let mut m = TptMuxer::new(tb);
    m.register_track(&track);
    m.write_packet_raw(&packet(0, 0, tb, vec![1, 2, 3]))
        .unwrap();
    let file = m.finish();
    let mut p = Player::new(WebLimits::default());
    let err = run(&mut p, &file, 100).unwrap_err();
    assert!(matches!(err, WebError::Unsupported(_)), "{err}");
    assert_eq!(p.info().ignored_tracks, 1);
}

/// A trivial codec used to prove the extension point: every packet decodes
/// to 100 samples of a constant.
struct FakeDecoder;

impl DynDecoder for FakeDecoder {
    fn decode(&mut self, packet: &Packet) -> CodecResult<Vec<DynFrame>> {
        let v = i16::from(*packet.data.first().unwrap_or(&0));
        Ok(vec![DynFrame::Audio {
            sample_format: SampleFormat::S16,
            channel_layout: ChannelLayout::Mono,
            sample_rate: 8_000,
            sample_count: 100,
            samples: std::iter::repeat_n(v.to_le_bytes(), 100)
                .flatten()
                .collect(),
        }])
    }
    fn flush(&mut self) -> CodecResult<Vec<DynFrame>> {
        Ok(Vec::new())
    }
    fn drain(&mut self) -> CodecResult<Vec<DynFrame>> {
        Ok(Vec::new())
    }
    fn info(&self) -> CodecInfo {
        CodecInfo {
            name: "Fake",
            codec_id: *b"TEST",
            version: 1,
            capabilities: Capabilities::default(),
            supported_formats: Vec::new(),
        }
    }
}

fn build_fake(_: &Track, _: &ResourceLimits) -> Result<Box<dyn DynDecoder>, WebError> {
    Ok(Box::new(FakeDecoder))
}

#[test]
fn a_registered_codec_plays_without_touching_the_player() {
    let tb = Timebase::new(1, 8_000);
    let track = audio_track(0, b"TEST", 8_000, vec![]);
    let mut m = TptMuxer::new(tb);
    m.register_track(&track);
    for i in 0..5 {
        m.write_packet_raw(&packet(0, i * 100, tb, vec![(i + 1) as u8]))
            .unwrap();
    }
    let file = m.finish();

    // Not in the native table...
    assert!(run(&mut Player::new(WebLimits::default()), &file, 64).is_err());
    // ...but one `register` call makes it playable.
    let mut table = DecoderTable::native();
    table.register(*b"TEST", MediaType::Audio, "fake", build_fake);
    let mut p = Player::with_table(WebLimits::default(), table);
    let out = run(&mut p, &file, 64).unwrap();
    assert_eq!(out.samples(), 500);
    assert_eq!(p.info().audio.as_ref().unwrap().codec, "fake");
    assert!((out.audio[3].pts_seconds - 300.0 / 8_000.0).abs() < 1e-9);
    assert_eq!(out.audio[4].planar[0], 5.0 / 32_768.0);
}

/// Deterministic xorshift.
fn rng(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

#[test]
fn garbage_payloads_are_skipped_then_bounded_by_the_error_budget() {
    let tb = Timebase::new(1, 48_000);
    let track = audio_track(0, b"CIND", 48_000, vec![8]);
    let mut m = TptMuxer::new(tb);
    m.register_track(&track);
    let mut seed = 7u64;
    for i in 0..600 {
        let len = 1 + (rng(&mut seed) % 300) as usize;
        let data: Vec<u8> = (0..len).map(|_| rng(&mut seed) as u8).collect();
        m.write_packet_raw(&packet(0, i * 512, tb, data)).unwrap();
    }
    let file = m.finish();
    let mut p = Player::new(WebLimits::default());
    // Random bytes must never panic; they either decode to *something*
    // bounded or are refused. If enough refuse, the stream is failed with
    // the dedicated error rather than looping or continuing forever.
    match run(&mut p, &file, 4096) {
        Ok(out) => {
            assert!(out.samples() <= 600 * 512);
        }
        Err(e) => assert_eq!(e, WebError::TooManyDecodeErrors),
    }
    assert!(p.stats().decode_errors <= p.limits().max_decode_errors + 1);
}

#[test]
fn a_video_track_that_claims_an_absurd_picture_is_refused_before_decoding() {
    let tb = Timebase::new(1, 30);
    let mut t = Track::new(0, MediaType::Video, CodecId::new(*b"THRM"), tb);
    t.media = TrackMedia {
        width: Some(60_000),
        height: Some(60_000),
        pixel_format: Some(PixelFormat::Yuv420),
        ..TrackMedia::default()
    };
    let mut m = TptMuxer::new(tb);
    m.register_track(&t);
    m.write_packet_raw(&packet(0, 0, tb, vec![0; 16])).unwrap();
    let file = m.finish();
    let mut p = Player::new(WebLimits::default());
    let err = run(&mut p, &file, 1000).unwrap_err();
    assert_eq!(err, WebError::Limit("frame dimensions"));
}

#[test]
fn mutated_real_files_never_panic_and_always_terminate_within_the_ceiling() {
    let base = synth::av_tpt(AudioKind::Cinder, 48_000, 0.3, 32, 32, 10);
    let mut seed = 0xDEAD_BEEF_CAFE_F00Du64;
    let ceiling = WebLimits::default().memory_ceiling_bytes();
    for round in 0..150 {
        let mut data = base.clone();
        for _ in 0..1 + rng(&mut seed) % 12 {
            let i = (rng(&mut seed) as usize) % data.len();
            data[i] = rng(&mut seed) as u8;
        }
        if round % 5 == 0 {
            data.truncate((rng(&mut seed) as usize) % data.len());
        }
        let mut p = Player::new(WebLimits::default());
        let slice = 1 + (rng(&mut seed) as usize) % 900;
        // Any outcome is acceptable except a panic, a hang, or blowing the
        // memory ceiling.
        if let Ok(out) = run(&mut p, &data, slice) {
            assert!((out.peak_memory as u64) < ceiling);
            assert!(out.samples() < 10_000_000);
        }
        assert!((p.memory_in_use() as u64) < ceiling);
    }
}

#[test]
fn sample_count_metadata_trims_only_the_tail() {
    // Aura lossless at 1024-sample blocks and a length that is not a
    // multiple of it: trimmed length is exact, content unchanged.
    let file = synth::audio_tpt(AudioKind::Aura, 44_100, 0.37);
    let mut p = Player::new(WebLimits::default());
    let out = run(&mut p, &file, 5000).unwrap();
    let want = synth::tone(44_100, 0.37);
    assert!(want.len() % 1024 != 0);
    assert_eq!(pcm_of(&out), want);
}
