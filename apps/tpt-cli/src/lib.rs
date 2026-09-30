//! `tpt-cli` — the TPT media command-line interface.
//!
//! Command surface (§43):
//!
//! ```text
//! tpt probe [--json] <input>          introspect a media file (§44)
//! tpt inspect <input>                 probe with human detail
//! tpt encode --codec <name> [..] <in> [<out>]
//! tpt decode --codec <name> [..] <in> [<out>]
//! tpt transcode --codec <name> [..] <in> [<out>]
//! tpt benchmark --codec <name> [..] <in>
//! tpt video-corpus [--json] [--fps <n>] [--frames <n>]
//! tpt list-codecs
//! tpt --help
//! ```
//!
//! The §§43 definitions map to code as follows (detailed in
//! `docs/cli-design.md`):
//!
//! | item | mapping |
//! |---|---|
//! | codec selection | `--codec` -> [`crate::args::resolve_codec`] |
//! | container selection | `--container` -> `tpt`/`wav` |
//! | quality configuration | `--quality 0..=100` (validated) |
//! | bitrate configuration | `--bitrate <n>[k\|m]` (parsed, `k`=`1e3`,`m`=`1e6`) |
//! | metadata manipulation | `--set-meta key=value` (repeatable) |
//! | streaming stdin/stdout | `-` as `<input>` / `<output>` |
//! | machine-readable output | `--json` on `probe`, `benchmark`, `video-corpus` |
//! | error reporting | stable exit codes (0 ok, 1 I/O or unsupported, 2 usage) |
//!
//! `probe`/`inspect`/`list-codecs`/`encode`/`decode`/`transcode`/
//! `benchmark`/`video-corpus` are all implemented. Objective quality
//! metrics (§37) live in [`quality`]; the deterministic synthetic corpora
//! they are validated against (§36) are [`signals`] for audio and
//! [`video`] for video — both generated closed-form from a seeded PRNG,
//! with no committed binary fixtures.
//!
//! **Media, not just audio.** The `encode`/`decode`/`transcode`/
//! `benchmark` family carries audio through the WAV bridge and video
//! through the YUV4MPEG2 bridge in [`y4m`]: a `.y4m` input to
//! `encode`/`benchmark` with `--codec thrum` is encoded one packet per
//! frame, and `decode` writes a video track back out as `.y4m`. The
//! bridge is byte-exact and deliberately narrow — progressive only,
//! `C420*` or `Cmono`, no aspect-ratio round trip (see [`y4m`]'s module
//! doc). Raw video never goes through `tpt-media-io`, which remains
//! audio/raster only.

pub mod args;
pub mod commands;
pub mod json;
pub mod memprobe;
pub mod quality;
pub mod signals;
pub mod video;
pub mod y4m;

use args::Command;

pub use commands::{Ctx, Outcome, ReadFile, ReadStdin, WriteFile};

/// Entry point used by `main` and tests alike: parse `argv` and execute.
pub fn run(argv: &[String], ctx: &Ctx<'_>) -> Outcome {
    match args::parse(argv) {
        Err(e) => Outcome::usage(e),
        Ok(Command::Help) => Outcome::ok(args::USAGE.into()),
        Ok(cmd) => commands::execute(&cmd, ctx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::format;

    fn file_ctx(files: &'static [(String, Vec<u8>)]) -> Ctx<'static> {
        let rf: ReadFile<'static> =
            Box::leak(Box::new(move |path: &str| -> Result<Vec<u8>, String> {
                files
                    .iter()
                    .find(|(p, _)| p == path)
                    .map(|(_, bytes)| bytes.clone())
                    .ok_or_else(|| format!("no such file: {path}"))
            }));
        let rs: ReadStdin<'static> = Box::leak(Box::new(|| -> Result<Vec<u8>, String> {
            Err("stdin not provided in this test".into())
        }));
        let wf: WriteFile<'static> =
            Box::leak(Box::new(|path: &str, _: &[u8]| -> Result<(), String> {
                Err(format!("no file writes in this test: {path}"))
            }));
        Ctx {
            read_file: rf,
            read_stdin: rs,
            write_file: wf,
        }
    }

    fn stdin_ctx(bytes: Vec<u8>) -> Ctx<'static> {
        let leaked: &'static Vec<u8> = Box::leak(Box::new(bytes));
        let rf: ReadFile<'static> = Box::leak(Box::new(|path: &str| -> Result<Vec<u8>, String> {
            Err(format!("no such file: {path}"))
        }));
        let rs: ReadStdin<'static> = Box::leak(Box::new(move || -> Result<Vec<u8>, String> {
            Ok(leaked.clone())
        }));
        let wf: WriteFile<'static> =
            Box::leak(Box::new(|path: &str, _: &[u8]| -> Result<(), String> {
                Err(format!("no file writes in this test: {path}"))
            }));
        Ctx {
            read_file: rf,
            read_stdin: rs,
            write_file: wf,
        }
    }

    type WriteLog = std::sync::Mutex<Vec<(String, Vec<u8>)>>;

    /// Like [`file_ctx`] but records every file write into `log`.
    fn file_ctx_with_writes(
        files: &'static [(String, Vec<u8>)],
        log: &'static WriteLog,
    ) -> Ctx<'static> {
        let rf: ReadFile<'static> =
            Box::leak(Box::new(move |path: &str| -> Result<Vec<u8>, String> {
                files
                    .iter()
                    .find(|(p, _)| p == path)
                    .map(|(_, bytes)| bytes.clone())
                    .ok_or_else(|| format!("no such file: {path}"))
            }));
        let rs: ReadStdin<'static> = Box::leak(Box::new(|| -> Result<Vec<u8>, String> {
            Err("stdin not provided in this test".into())
        }));
        let wf: WriteFile<'static> = Box::leak(Box::new(
            move |path: &str, bytes: &[u8]| -> Result<(), String> {
                log.lock().unwrap().push((path.to_string(), bytes.to_vec()));
                Ok(())
            },
        ));
        Ctx {
            read_file: rf,
            read_stdin: rs,
            write_file: wf,
        }
    }

    /// Serialize a mono S16 WAV at `rate` Hz via `tpt_media_io::write_wav`.
    fn io_wav(rate: u32, samples: &[u8]) -> Vec<u8> {
        let frame = tpt_media_core::AudioFrame {
            frame: tpt_media_core::Frame {
                pts: tpt_media_core::Timestamp::zero(tpt_media_core::Timebase::new(1, rate)),
                duration: None,
                side_data: Vec::new(),
            },
            samples: samples.to_vec(),
            sample_count: (samples.len() / 2) as u32,
            format: tpt_media_core::SampleFormat::S16,
            channel_layout: tpt_media_core::ChannelLayout::Mono,
            sample_rate: rate,
        };
        tpt_media_io::write_wav(&[frame]).expect("wav serialize")
    }

    /// A deterministic 16-bit mono float ramp so lossy comparisons are
    /// still meaningful (output lengths, not exact samples).
    fn sine_samples(count: usize) -> Vec<u8> {
        (0..count)
            .flat_map(|i| {
                let v = ((i as f32 * 997.0).sin() * 12_000.0) as i16;
                v.to_le_bytes()
            })
            .collect()
    }

    fn argv(args: &[&str]) -> Vec<String> {
        // `run` expects args *without* the program name (see main.rs).
        args.iter().map(|s| s.to_string()).collect()
    }

    // A small valid `.tpt`: one CIND audio track, three packets.
    fn make_tpt() -> Vec<u8> {
        let tb = tpt_media_core::Timebase {
            num: 1,
            den: 48_000,
        };
        let packets = (0..3)
            .map(|i| {
                let mut p = tpt_media_core::Packet::empty(
                    1,
                    tpt_media_core::Timestamp {
                        ticks: (i as i64) * 480,
                        timebase: tb,
                    },
                );
                p.data.extend_from_slice(&[0; 8]);
                p
            })
            .collect::<Vec<_>>();
        tpt_container::build_tpt(tb, &[(1, b"CIND", &[][..])], &packets)
    }

    /// A real, *decodable* `.tpt`: `sine_samples` driven through `tpt
    /// encode`, captured from the injected writer.
    ///
    /// [`make_tpt`]'s hand-written packets are structurally valid but are
    /// not Cinder bitstreams, so any test that needs decoding to actually
    /// succeed (rather than fail earlier for an unrelated reason) has to
    /// build its fixture through the encoder.
    fn make_decodable_tpt() -> Vec<u8> {
        let log: &'static WriteLog = Box::leak(Box::new(std::sync::Mutex::new(Vec::new())));
        let files: &'static [(String, Vec<u8>)] = Box::leak(Box::new(vec![(
            "in.wav".to_string(),
            io_wav(48_000, &sine_samples(2_048)),
        )]));
        let o = run(
            &argv(&["encode", "--codec", "cinder", "in.wav", "out.tpt"]),
            &file_ctx_with_writes(files, log),
        );
        assert_eq!(o.code, 0, "fixture encode failed: {}", o.err);
        log.lock()
            .unwrap()
            .iter()
            .find(|(p, _)| p == "out.tpt")
            .map(|(_, bytes)| bytes.clone())
            .expect("encode wrote out.tpt")
    }

    /// A real *video* `.tpt`: a synthetic y4m stream driven through
    /// `tpt encode --codec thrum`. The video counterpart of
    /// [`make_decodable_tpt`], and the fixture `probe` needs to see a
    /// `THRM` track.
    fn make_video_tpt() -> Vec<u8> {
        let (w, h) = (32u32, 32u32);
        let frames: Vec<_> = (0..2)
            .map(|i| {
                let mut f =
                    tpt_media_core::VideoFrame::alloc(w, h, tpt_media_core::PixelFormat::Yuv420);
                // A luma ramp: distinguishable frames, still cheap to encode.
                for (n, px) in f.pixels.iter_mut().enumerate() {
                    *px = ((n + i * 8) % 256) as u8;
                }
                f
            })
            .collect();
        let params = y4m::Y4mParams {
            width: w,
            height: h,
            fps_num: 30,
            fps_den: 1,
            format: tpt_media_core::PixelFormat::Yuv420,
        };
        let y4m_bytes = y4m::write(&params, &frames);
        let log: &'static WriteLog = Box::leak(Box::new(std::sync::Mutex::new(Vec::new())));
        let files: &'static [(String, Vec<u8>)] =
            Box::leak(Box::new(vec![("in.y4m".to_string(), y4m_bytes)]));
        let o = run(
            &argv(&["encode", "--codec", "thrum", "in.y4m", "out.tpt"]),
            &file_ctx_with_writes(files, log),
        );
        assert_eq!(o.code, 0, "fixture encode failed: {}", o.err);
        log.lock()
            .unwrap()
            .iter()
            .find(|(p, _)| p == "out.tpt")
            .map(|(_, bytes)| bytes.clone())
            .expect("encode wrote out.tpt")
    }

    #[test]
    fn help_prints_usage() {
        let o = run(&argv(&["--help"]), &file_ctx(&[]));
        assert_eq!(o.code, 0);
        assert!(o.out.contains("tpt — TPT media command-line interface"));
    }

    #[test]
    fn unknown_command_is_a_usage_error() {
        let o = run(&argv(&["frobnicate"]), &file_ctx(&[]));
        assert_eq!(o.code, 2);
        assert!(o.err.contains("unknown command"));
    }

    #[test]
    fn unknown_option_is_a_usage_error() {
        let o = run(&argv(&["probe", "--bogus", "x.tpt"]), &file_ctx(&[]));
        assert_eq!(o.code, 2);
        assert!(o.err.contains("unknown option"));
    }

    #[test]
    fn probe_human_output_from_stdin() {
        let o = run(&argv(&["probe", "-"]), &stdin_ctx(make_tpt()));
        assert_eq!(o.code, 0);
        assert!(o.out.contains("Container: Tpt"));
        assert!(o.out.contains("Tracks: 1"));
        assert!(o.out.contains("CIND"));
    }

    #[test]
    fn probe_json_output() {
        let o = run(&argv(&["probe", "--json", "-"]), &stdin_ctx(make_tpt()));
        assert_eq!(o.code, 0);
        assert!(o.out.contains("\"container\":\"tpt\""));
        assert!(o.out.ends_with("}\n") || o.out.ends_with('}'));
        assert!(o.out.contains("\"codec_id\":\"CIND\""));
        assert!(o.out.contains("\"duration_seconds\""));
    }

    #[test]
    fn probe_reports_the_codecs_this_binary_actually_has() {
        // `tpt probe` used to resolve capabilities against an *empty*
        // registry, so it reported `encoder=false, decoder=false` for
        // codecs this same program encodes and decodes one subcommand
        // later. A capability report that contradicts the tool printing
        // it is worse than no report, so both the audio and the video
        // track must now report real availability.
        let o = run(&argv(&["probe", "-"]), &stdin_ctx(make_tpt()));
        assert_eq!(o.code, 0);
        let cind = o
            .out
            .lines()
            .find(|l| l.contains("Codec CIND"))
            .unwrap_or_else(|| panic!("no CIND capability line in:\n{}", o.out));
        assert!(cind.contains("encoder=true"), "got: {cind}");
        assert!(cind.contains("decoder=true"), "got: {cind}");

        let v = run(&argv(&["probe", "-"]), &stdin_ctx(make_video_tpt()));
        assert_eq!(v.code, 0);
        let thrm = v
            .out
            .lines()
            .find(|l| l.contains("Codec THRM"))
            .unwrap_or_else(|| panic!("no THRM capability line in:\n{}", v.out));
        assert!(thrm.contains("encoder=true"), "got: {thrm}");
        assert!(thrm.contains("decoder=true"), "got: {thrm}");
        // Thrum declares itself lossy and real-time; the report carries
        // the codec's own declaration, not a hard-coded guess.
        assert!(thrm.contains("lossy=true"), "got: {thrm}");
    }

    #[test]
    fn probe_json_reports_codec_availability_too() {
        let o = run(&argv(&["probe", "--json", "-"]), &stdin_ctx(make_tpt()));
        assert_eq!(o.code, 0);
        assert!(
            o.out.contains("\"decoder_available\":true"),
            "got: {}",
            o.out
        );
        assert!(
            o.out.contains("\"encoder_available\":true"),
            "got: {}",
            o.out
        );
    }

    #[test]
    fn probe_missing_input_is_a_usage_error() {
        let o = run(&argv(&["probe"]), &file_ctx(&[]));
        assert_eq!(o.code, 2);
        assert!(o.err.contains("requires an input path"));
    }

    #[test]
    fn probe_missing_file_is_an_io_error() {
        let o = run(&argv(&["probe", "nope.tpt"]), &file_ctx(&[]));
        assert_eq!(o.code, 1);
        assert!(o.err.contains("no such file"));
    }

    #[test]
    fn inspect_is_a_probe_alias() {
        let o = run(&argv(&["inspect", "-"]), &stdin_ctx(make_tpt()));
        assert_eq!(o.code, 0);
        assert!(o.out.contains("Container: Tpt"));
    }

    #[test]
    fn list_codecs_prints_the_native_family() {
        let o = run(&argv(&["list-codecs"]), &file_ctx(&[]));
        assert_eq!(o.code, 0);
        assert!(o.out.contains("CIND"));
        assert!(o.out.contains("AURA"));
        assert!(o.out.contains("REED"));
    }

    #[test]
    fn encode_without_codec_is_a_usage_error() {
        let o = run(&argv(&["encode", "in.wav", "out.tpt"]), &file_ctx(&[]));
        assert_eq!(o.code, 2);
        assert!(o.err.contains("requires --codec"));
    }

    #[test]
    fn encode_rejects_the_wav_container() {
        let o = run(
            &argv(&[
                "encode",
                "--codec",
                "cinder",
                "--container",
                "wav",
                "-",
                "-",
            ]),
            &stdin_ctx(vec![]),
        );
        assert_eq!(o.code, 1);
        assert!(o.err.contains("writes only the tpt container"));
    }

    #[test]
    fn encode_rejects_non_wav_input() {
        let o = run(
            &argv(&["encode", "--codec", "cinder", "-", "-"]),
            &stdin_ctx(b"not a wav at all".to_vec()),
        );
        assert_eq!(o.code, 1);
        assert!(o.err.contains("input is not a readable wav"));
    }

    #[test]
    fn encode_rejects_unsupported_rates() {
        let wav = io_wav(8_000, &[0; 128]);
        let o = run(
            &argv(&["encode", "--codec", "cinder", "-", "-"]),
            &stdin_ctx(wav),
        );
        assert_eq!(o.code, 1);
        assert!(
            o.err.contains("44100 or 48000"),
            "unexpected err: {}",
            o.err
        );
    }

    #[test]
    fn encode_rejects_reed_outside_its_rates() {
        // Reed speaks 8/16 kHz; a 48 kHz input is rejected with the
        // codec-specific rate list (no CLI resampler).
        let o = run(
            &argv(&["encode", "--codec", "reed", "-", "-"]),
            &stdin_ctx(io_wav(48_000, &[0; 1024])),
        );
        assert_eq!(o.code, 1);
        assert!(o.err.contains("8000 or 16000"), "unexpected err: {}", o.err);
    }

    #[test]
    fn encode_cinder_writes_a_tpt_to_stdout() {
        let samples = sine_samples(1000);
        let o = run(
            &argv(&["encode", "--codec", "cinder", "--quality", "80", "-", "-"]),
            &stdin_ctx(io_wav(48_000, &samples)),
        );
        assert_eq!(o.code, 0, "stderr was: {}", o.err);
        let tpt = o.binary_stdout.expect("binary stdout expected");
        assert_eq!(&tpt[..4], b"TPTM");
        assert!(o.out.starts_with("encoded"));
    }

    #[test]
    fn encode_cinder_round_trips_through_stdout() {
        let samples = sine_samples(1000);
        let enc = run(
            &argv(&["encode", "--codec", "cinder", "--quality", "80", "-", "-"]),
            &stdin_ctx(io_wav(48_000, &samples)),
        );
        assert_eq!(enc.code, 0, "encode stderr: {}", enc.err);
        let tpt = enc.binary_stdout.expect("tpt on stdout");

        let dec = run(
            &argv(&["decode", "--container", "wav", "-", "-"]),
            &stdin_ctx(tpt),
        );
        assert_eq!(dec.code, 0, "decode stderr: {}", dec.err);
        let wav_out = dec.binary_stdout.expect("wav on stdout");
        let frame = tpt_media_io::decode_wav(&wav_out).expect("decoded wav parses");
        assert_eq!(
            frame.sample_count as usize,
            samples.len() / 2,
            "padding must be trimmed back to the original length"
        );
    }

    #[test]
    fn encode_cinder_writes_a_container_to_a_file() {
        let samples = sine_samples(512);
        let wav = io_wav(48_000, &samples);
        let files: &'static [(String, Vec<u8>)] =
            Box::leak(Box::new(vec![("in.wav".to_string(), wav)]));
        let log: &'static WriteLog = Box::leak(Box::new(WriteLog::default()));
        let ctx = file_ctx_with_writes(files, log);
        let o = run(
            &argv(&["encode", "--codec", "cinder", "in.wav", "out.tpt"]),
            &ctx,
        );
        assert_eq!(o.code, 0, "stderr was: {}", o.err);
        assert_eq!(o.binary_stdout, None);
        let writes = log.lock().unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(&writes[0].0, "out.tpt");
        assert_eq!(&writes[0].1[..4], b"TPTM");
        drop(writes);
        let _ = &samples;
    }

    #[test]
    fn encode_aura_round_trips_through_stdout() {
        let samples = sine_samples(1000);
        let wav = io_wav(48_000, &samples);
        let enc = run(
            &argv(&["encode", "--codec", "aura", "-", "-"]),
            &stdin_ctx(wav),
        );
        assert_eq!(enc.code, 0, "encode stderr: {}", enc.err);
        let tpt = enc.binary_stdout.expect("tpt on stdout");
        assert_eq!(&tpt[..4], b"TPTM");

        let dec = run(
            &argv(&["decode", "--container", "wav", "-", "-"]),
            &stdin_ctx(tpt),
        );
        assert_eq!(dec.code, 0, "decode stderr: {}", dec.err);
        let wav_out = dec.binary_stdout.expect("wav on stdout");
        assert_eq!(&wav_out[..4], b"RIFF");
        let frame = tpt_media_io::decode_wav(&wav_out).expect("decoded wav parses");
        assert_eq!(frame.sample_count as usize, samples.len() / 2);
        assert_eq!(frame.samples, samples, "lossless aura round trip");
    }

    #[test]
    fn decode_rejects_wav_input() {
        let o = run(
            &argv(&["decode", "--container", "wav", "-", "-"]),
            &stdin_ctx(io_wav(48_000, &sine_samples(256))),
        );
        assert_eq!(o.code, 1);
        assert!(o.err.contains("nothing to decode"));
    }

    #[test]
    fn decode_rejects_unknown_input() {
        let o = run(
            &argv(&["decode", "--container", "wav", "-", "-"]),
            &stdin_ctx(b"zzz".to_vec()),
        );
        assert_eq!(o.code, 1);
        assert!(o.err.contains("not a recognisable media file"));
    }

    #[test]
    fn decode_rejects_wrong_container_flag() {
        let o = run(
            &argv(&["decode", "--container", "tpt", "-", "-"]),
            &stdin_ctx(make_decodable_tpt()),
        );
        assert_eq!(o.code, 1);
        // The wording tracks `decode_cmd`'s audio branch: decoding an audio
        // track writes WAV, decoding a video track writes y4m, so the
        // `--container` requirement depends on which track is present.
        assert!(o.err.contains("writes wav output"), "got: {}", o.err);
    }

    #[test]
    fn decode_rejects_codec_mismatch() {
        let o = run(
            &argv(&["decode", "--codec", "aura", "--container", "wav", "-", "-"]),
            &stdin_ctx(make_tpt()),
        );
        assert_eq!(o.code, 1);
        assert!(o.err.contains("track uses"), "unexpected err: {}", o.err);
        assert!(o.err.contains("requested"));
    }

    #[test]
    fn encode_reed_round_trips_through_stdout() {
        // 640 samples at 16 kHz = two 320-sample (20 ms) reed frames.
        let samples = sine_samples(640);
        let enc = run(
            &argv(&["encode", "--codec", "reed", "-", "-"]),
            &stdin_ctx(io_wav(16_000, &samples)),
        );
        assert_eq!(enc.code, 0, "encode stderr: {}", enc.err);
        let tpt = enc.binary_stdout.expect("tpt on stdout");
        assert_eq!(&tpt[..4], b"TPTM");

        let dec = run(
            &argv(&["decode", "--container", "wav", "-", "-"]),
            &stdin_ctx(tpt),
        );
        assert_eq!(dec.code, 0, "decode stderr: {}", dec.err);
        let wav_out = dec.binary_stdout.expect("wav on stdout");
        assert_eq!(&wav_out[..4], b"RIFF");
        let frame = tpt_media_io::decode_wav(&wav_out).expect("decoded wav parses");
        assert_eq!(
            frame.sample_count as usize,
            samples.len() / 2,
            "padding must be trimmed back to the original length"
        );
    }

    #[test]
    fn unknown_codec_is_a_usage_error() {
        let o = run(
            &argv(&["encode", "--codec", "notacodec", "in.raw"]),
            &file_ctx(&[]),
        );
        assert_eq!(o.code, 2);
        assert!(o.err.contains("unknown codec"));
    }

    #[test]
    fn quality_out_of_range_is_rejected() {
        let o = run(
            &argv(&["encode", "--codec", "cinder", "--quality", "101", "in.raw"]),
            &file_ctx(&[]),
        );
        assert_eq!(o.code, 2);
        assert!(o.err.contains("quality out of range"));
    }

    #[test]
    fn bitrate_suffix_and_bytes_are_validated() {
        assert_eq!(args::parse_bitrate("128k").unwrap(), 128_000);
        assert_eq!(args::parse_bitrate("2m").unwrap(), 2_000_000);
        assert_eq!(args::parse_bitrate("96").unwrap(), 96);
        assert!(args::parse_bitrate("fast").is_err());
        let o = run(
            &argv(&["encode", "--codec", "cinder", "--bitrate", "fast", "in.raw"]),
            &file_ctx(&[]),
        );
        assert_eq!(o.code, 2);
    }

    #[test]
    fn video_codecs_resolve_but_are_rejected_by_audio_commands() {
        // thrum is part of the native family, so --codec thrum
        // and --codec LIVE both resolve - but on a *non*-y4m
        // input the failure must say *why* rather than claiming
        // no encoder is registered.
        assert_eq!(
            args::resolve_codec("thrum").unwrap(),
            tpt_media_core::CodecId::new(*b"THRM")
        );
        assert_eq!(
            args::resolve_codec("THRM").unwrap(),
            tpt_media_core::CodecId::new(*b"THRM")
        );
        // A real, decodable `.tpt`, because every one of these commands reads
        // (and `transcode` decodes) its input before dispatching on the
        // codec: a missing file would fail first with an I/O error, and an
        // undecodable one with a decode error — neither of which says
        // anything about the codec. (The encode family is no longer
        // audio-only — a y4m input takes the video path — so this
        // deliberately exercises the other branch.)
        let files: &'static [(String, Vec<u8>)] =
            Box::leak(Box::new(vec![("in.tpt".to_string(), make_decodable_tpt())]));
        let ctx = file_ctx(files);
        for cmd in ["encode", "transcode", "benchmark"] {
            let o = run(&argv(&[cmd, "--codec", "thrum", "in.tpt"]), &ctx);
            assert_eq!(o.code, 1, "{cmd} exit code");
            assert!(
                o.err.contains("video codec"),
                "{cmd} should explain the video/audio mismatch, got: {}",
                o.err
            );
            assert!(
                !o.err.contains("no encoder registered"),
                "{cmd} should not claim the codec is unknown"
            );
        }
    }

    #[test]
    fn list_codecs_advertises_thrum() {
        let o = run(&argv(&["list-codecs"]), &file_ctx(&[]));
        assert_eq!(o.code, 0);
        assert!(o.out.contains("thrum"), "{}", o.out);
        assert!(o.out.contains("THRM"), "{}", o.out);
    }

    /// A synthetic y4m clip: a luma ramp that changes per frame, so a
    /// decoder that drops or reorders pictures is visible.
    fn ramp_y4m(w: u32, h: u32, frames: usize) -> Vec<u8> {
        let pics: Vec<_> = (0..frames)
            .map(|i| {
                let mut f =
                    tpt_media_core::VideoFrame::alloc(w, h, tpt_media_core::PixelFormat::Yuv420);
                for (n, px) in f.pixels.iter_mut().enumerate() {
                    *px = ((n + i * 8) % 256) as u8;
                }
                f
            })
            .collect();
        y4m::write(
            &y4m::Y4mParams {
                width: w,
                height: h,
                fps_num: 30,
                fps_den: 1,
                format: tpt_media_core::PixelFormat::Yuv420,
            },
            &pics,
        )
    }

    /// Run `encode --codec <name> in.y4m out.tpt` and return the written file.
    fn encode_video(name: &str, y4m_bytes: Vec<u8>) -> (Outcome, Vec<u8>) {
        let log: &'static WriteLog = Box::leak(Box::new(std::sync::Mutex::new(Vec::new())));
        let files: &'static [(String, Vec<u8>)] =
            Box::leak(Box::new(vec![("in.y4m".to_string(), y4m_bytes)]));
        let o = run(
            &argv(&["encode", "--codec", name, "in.y4m", "out.tpt"]),
            &file_ctx_with_writes(files, log),
        );
        let bytes = log
            .lock()
            .unwrap()
            .iter()
            .find(|(p, _)| p == "out.tpt")
            .map(|(_, b)| b.clone())
            .unwrap_or_default();
        (o, bytes)
    }

    /// Every video codec the y4m bridge claims to drive must actually round
    /// trip through `encode` and `decode` (todo.md §33's open item).
    ///
    /// The assertion is deliberately about the *file*, not the exit codes: a
    /// codec that encodes and then cannot decode its own output would pass a
    /// smoke test that only checked `code == 0`. `decode` also re-derives the
    /// decoder from the track header alone, so this exercises the
    /// codec-private bytes as well as the packet stream.
    #[test]
    fn every_video_codec_round_trips_through_the_y4m_bridge() {
        for (name, id) in [("thrum", *b"THRM"), ("vault", *b"VAUL"), ("mote", *b"MOTE")] {
            let (enc, tpt) = encode_video(name, ramp_y4m(32, 32, 3));
            assert_eq!(enc.code, 0, "{name} encode failed: {}", enc.err);
            assert!(!tpt.is_empty(), "{name} encode wrote no out.tpt");

            // The track must name the codec that produced it.
            let (_, tracks, _) = tpt_container::parse_tpt(&tpt).expect("parsable tpt");
            assert_eq!(
                tracks[0].codec_config.codec_id,
                tpt_media_core::CodecId::new(id),
                "{name} wrote the wrong codec id"
            );

            // Decode it back through the public command, from a .tpt input.
            let dlog: &'static WriteLog = Box::leak(Box::new(std::sync::Mutex::new(Vec::new())));
            let dfiles: &'static [(String, Vec<u8>)] =
                Box::leak(Box::new(vec![("in.tpt".to_string(), tpt)]));
            let dec = run(
                &argv(&["decode", "in.tpt", "out.y4m"]),
                &file_ctx_with_writes(dfiles, dlog),
            );
            assert_eq!(dec.code, 0, "{name} decode failed: {}", dec.err);
            let out = dlog
                .lock()
                .unwrap()
                .iter()
                .find(|(p, _)| p == "out.y4m")
                .map(|(_, b)| b.clone())
                .unwrap_or_default();
            assert!(
                out.starts_with(b"YUV4MPEG2"),
                "{name} decode did not produce a y4m stream"
            );
        }
    }

    /// Every encoded frame must keep the PTS the y4m bridge assigned it.
    ///
    /// This is the regression the generalisation could have introduced: the
    /// type-erased `Dyn*` contracts carry no PTS, so routing the video path
    /// through them would silently stamp every packet with
    /// `Timestamp::zero`. The CLI erases at `Frame = VideoFrame` instead, and
    /// this pins the difference where it is observable.
    #[test]
    fn video_packets_keep_the_pts_the_bridge_assigned() {
        for name in ["thrum", "vault", "mote"] {
            let (o, tpt) = encode_video(name, ramp_y4m(32, 32, 4));
            assert_eq!(o.code, 0, "{name}: {}", o.err);
            let (_, _, packets) = tpt_container::parse_tpt(&tpt).expect("parsable tpt");
            assert!(
                packets.len() >= 4,
                "{name}: at least one packet per frame, got {}",
                packets.len()
            );
            // Not "one packet per frame": Mote prefixes its first frame with
            // an out-of-band dictionary packet, so its count is higher. What
            // must hold for every codec is that each frame's PTS survived, so
            // the distinct timestamps are exactly the frame indices the y4m
            // bridge assigned (0,1,2,... on a 30/1 timebase).
            let mut ticks: Vec<i64> = packets.iter().map(|p| p.pts.ticks).collect();
            ticks.sort_unstable();
            ticks.dedup();
            assert_eq!(ticks, vec![0, 1, 2, 3], "{name}: frame PTS not preserved");
            for p in &packets {
                assert_eq!(p.pts.timebase.num, 1, "{name}: packet tb num");
                assert_eq!(p.pts.timebase.den, 30, "{name}: packet tb den");
            }
        }
    }

    /// A geometry the chosen codec cannot code must be refused by *its own*
    /// validation, with a message naming that codec — not a generic "no
    /// encoder registered", which would read as if the codec did not exist.
    ///
    /// 40x24 is a multiple of 8 but not of 16: Vault codes in 8x8 units so it
    /// accepts, Thrum and Mote want 16-pixel macroblocks so they do not. That
    /// the three differ is the point — a single hard-coded rule in the CLI
    /// would get at least two of them wrong.
    #[test]
    fn an_uncodable_geometry_names_the_codec_that_refused_it() {
        let files: &'static [(String, Vec<u8>)] =
            Box::leak(Box::new(vec![("in.y4m".to_string(), ramp_y4m(40, 24, 1))]));
        for name in ["thrum", "mote"] {
            let o = run(
                &argv(&["encode", "--codec", name, "in.y4m", "out.tpt"]),
                &file_ctx(files),
            );
            assert_eq!(o.code, 1, "{name} should refuse 40x24");
            assert!(
                o.err.contains(name) && o.err.contains("encoder"),
                "{name} error should name the codec and the failure, got: {}",
                o.err
            );
        }
        // Vault's coarser alignment accepts the same frame.
        let (o, bytes) = encode_video("vault", ramp_y4m(40, 24, 1));
        assert_eq!(o.code, 0, "vault should accept 40x24: {}", o.err);
        assert!(!bytes.is_empty());
    }

    /// `benchmark` must drive every video codec and report the one it
    /// actually measured, in both the human and JSON forms.
    #[test]
    fn benchmark_drives_every_video_codec_and_names_it() {
        for name in ["thrum", "vault", "mote"] {
            let files: &'static [(String, Vec<u8>)] =
                Box::leak(Box::new(vec![("in.y4m".to_string(), ramp_y4m(32, 32, 2))]));

            let human = run(
                &argv(&["benchmark", "--codec", name, "--runs", "1", "in.y4m"]),
                &file_ctx(files),
            );
            assert_eq!(human.code, 0, "{name} benchmark failed: {}", human.err);
            assert!(
                human.out.contains(&format!("benchmark: {name},")),
                "{name} benchmark should name itself, got:\n{}",
                human.out
            );

            let json = run(
                &argv(&[
                    "benchmark",
                    "--codec",
                    name,
                    "--json",
                    "--runs",
                    "1",
                    "in.y4m",
                ]),
                &file_ctx(files),
            );
            assert_eq!(json.code, 0, "{name} --json failed: {}", json.err);
            assert!(
                json.out.contains(&format!("\"codec\":\"{name}\"")),
                "{name} JSON should report its own name, got:\n{}",
                json.out
            );
        }
    }

    /// `transcode` between two *different* video codecs: encode as Vault,
    /// then decode that track and re-encode it as Mote. Exercises both the
    /// source and target halves of the generalisation, and the message must
    /// name both ends truthfully rather than assuming thrum.
    #[test]
    fn transcode_moves_between_different_video_codecs() {
        let (_, src) = encode_video("vault", ramp_y4m(32, 32, 2));
        assert!(!src.is_empty(), "vault encode wrote nothing");

        let tlog: &'static WriteLog = Box::leak(Box::new(std::sync::Mutex::new(Vec::new())));
        let tfiles: &'static [(String, Vec<u8>)] =
            Box::leak(Box::new(vec![("in.tpt".to_string(), src)]));
        let o = run(
            &argv(&["transcode", "--codec", "mote", "in.tpt", "dst.tpt"]),
            &file_ctx_with_writes(tfiles, tlog),
        );
        assert_eq!(o.code, 0, "transcode failed: {}", o.err);
        assert!(
            o.out.contains("vault -> mote"),
            "transcode should name both codecs, got:\n{}",
            o.out
        );

        let dst = tlog
            .lock()
            .unwrap()
            .iter()
            .find(|(p, _)| p == "dst.tpt")
            .map(|(_, b)| b.clone())
            .unwrap_or_default();
        let (_, tracks, packets) = tpt_container::parse_tpt(&dst).expect("parsable tpt");
        assert_eq!(
            tracks[0].codec_config.codec_id,
            tpt_media_core::CodecId::new(*b"MOTE")
        );
        assert!(!packets.is_empty(), "transcode produced no packets");
    }

    #[test]
    fn video_corpus_reports_every_scene_with_both_metrics() {
        let o = run(&argv(&["video-corpus", "--frames", "2"]), &file_ctx(&[]));
        assert_eq!(o.code, 0, "{}", o.err);
        // Every todo.md 36 video category appears by name.
        for name in [
            "talking-head",
            "camera-motion",
            "static-scene",
            "high-motion",
            "animation",
            "screen-content",
            "synthetic-patterns",
            "pathological-frames",
        ] {
            assert!(o.out.contains(name), "missing scene {name} in\n{}", o.out);
        }
        // Both quality metrics and the rate/CPU columns are present: a
        // quality number without a bitrate would be misleading.
        assert!(o.out.contains("PSNR"), "{}", o.out);
        assert!(o.out.contains("SSIM"), "{}", o.out);
        assert!(o.out.contains("bit/s"), "{}", o.out);
        assert!(o.out.contains("enc ms"), "{}", o.out);
        assert!(o.out.contains("dec ms"), "{}", o.out);
        assert!(o.out.contains("frame budget"), "{}", o.out);
    }

    #[test]
    fn video_corpus_json_is_wellformed_and_complete() {
        let o = run(
            &argv(&["video-corpus", "--json", "--frames", "2", "--fps", "15"]),
            &file_ctx(&[]),
        );
        assert_eq!(o.code, 0, "{}", o.err);
        let s = o.out.trim();
        assert!(s.starts_with('{') && s.ends_with('}'), "{}", s);
        // Braces must balance, which catches the stray-comma class of bug
        // the JSON writer has hit before.
        let opens = s.matches('{').count();
        let closes = s.matches('}').count();
        assert_eq!(opens, closes, "unbalanced braces in {s}");
        assert_eq!(opens, 9, "one wrapper plus eight scenes");
        // The fps override must be honoured, not ignored.
        assert!(s.contains("\"fps\":15"), "{s}");
        for key in [
            "bits_per_pixel",
            "bitrate_bps",
            "psnr_db",
            "ssim",
            "encode_ms_per_frame",
            "decode_ms_per_frame",
            "latency_ms",
            "fits_frame_budget",
        ] {
            assert!(s.contains(key), "missing {key} in {s}");
        }
    }

    /// The corpus sweep must run for every video codec and report the one it
    /// actually measured — todo.md §33's "§36/§37 video-corpus quality and a
    /// bitrate comparison against another codec".
    #[test]
    fn video_corpus_measures_every_video_codec_and_names_it() {
        for name in ["thrum", "vault", "mote"] {
            let o = run(
                &argv(&["video-corpus", "--codec", name, "--frames", "2"]),
                &file_ctx(&[]),
            );
            assert_eq!(o.code, 0, "{name} corpus failed: {}", o.err);
            assert!(
                o.out.contains(&format!("video corpus: {name} ")),
                "{name} corpus should name itself, got:\n{}",
                o.out
            );
            for scene in ["talking-head", "camera-motion", "static-scene"] {
                assert!(o.out.contains(scene), "{name} missing {scene}");
            }

            let j = run(
                &argv(&["video-corpus", "--codec", name, "--json", "--frames", "2"]),
                &file_ctx(&[]),
            );
            assert_eq!(j.code, 0, "{name} --json failed: {}", j.err);
            assert!(
                j.out.contains(&format!("\"codec\":\"{name}\"")),
                "{name} JSON should report its own name, got:\n{}",
                j.out
            );
        }
    }

    /// The reported frame count is decoded *pictures*, not packets.
    ///
    /// Mote prefixes its first frame with a dictionary packet that carries no
    /// picture, so counting packets would report one frame too many — and
    /// because `frames` is the divisor in the bitrate and bpp maths, would
    /// understate Mote's rate by a frame's worth. Pinned on the codec that
    /// actually emits the extra packet.
    #[test]
    fn corpus_counts_decoded_pictures_not_packets() {
        for (name, frames) in [("thrum", "3"), ("vault", "3"), ("mote", "3")] {
            let o = run(
                &argv(&["video-corpus", "--codec", name, "--frames", frames]),
                &file_ctx(&[]),
            );
            assert_eq!(o.code, 0, "{name}: {}", o.err);
            assert!(
                o.out.contains(&format!("{frames} frames/scene")),
                "{name} should report {frames} frames/scene, got:\n{}",
                o.out
            );
        }
    }

    /// A codec that cannot produce a corpus measurement must say so, rather
    /// than falling through to a confusing encode error.
    #[test]
    fn video_corpus_rejects_a_non_video_codec_by_name() {
        for bad in ["cinder", "aura", "reed"] {
            let o = run(
                &argv(&["video-corpus", "--codec", bad, "--frames", "1"]),
                &file_ctx(&[]),
            );
            assert_ne!(o.code, 0, "{bad} should be rejected");
            assert!(
                o.err.contains("not a video codec"),
                "{bad} error should explain, got: {}",
                o.err
            );
        }
        // A name that is neither in the table nor a well-formed 4-byte
        // codec ID is a usage error (exit 2), not a codec error. Note
        // `--codec nope` is *not* such a case: `resolve_codec` accepts any
        // four characters as a raw codec ID, so "nope" is a well-formed
        // `nope` codec that simply is not a video one.
        let o = run(
            &argv(&["video-corpus", "--codec", "notacodec", "--frames", "1"]),
            &file_ctx(&[]),
        );
        assert_eq!(o.code, 2, "unknown codec is a usage error: {}", o.err);

        // A four-character non-video codec ID reaches the video check.
        let o = run(
            &argv(&["video-corpus", "--codec", "nope", "--frames", "1"]),
            &file_ctx(&[]),
        );
        assert_eq!(o.code, 1, "a raw non-video 4cc is a codec error: {}", o.err);
    }

    #[test]
    fn video_corpus_options_are_validated() {
        // Zero is rejected rather than dividing by it downstream.
        assert!(run(&argv(&["video-corpus", "--fps", "0"]), &file_ctx(&[])).code != 0);
        assert!(run(&argv(&["video-corpus", "--frames", "0"]), &file_ctx(&[])).code != 0);
        assert!(run(&argv(&["video-corpus", "--fps", "abc"]), &file_ctx(&[])).code != 0);
        // A missing value is a usage error, not a panic.
        assert!(run(&argv(&["video-corpus", "--fps"]), &file_ctx(&[])).code != 0);
        // An unknown flag and a stray positional are both rejected.
        assert!(run(&argv(&["video-corpus", "--nope"]), &file_ctx(&[])).code != 0);
        assert!(run(&argv(&["video-corpus", "extra"]), &file_ctx(&[])).code != 0);
    }

    #[test]
    fn codec_resolution() {
        assert_eq!(
            args::resolve_codec("cinder").unwrap(),
            tpt_media_core::CodecId::new(*b"CIND")
        );
        assert_eq!(
            args::resolve_codec("AURA").unwrap(),
            tpt_media_core::CodecId::new(*b"AURA")
        );
        assert_eq!(
            args::resolve_codec("reed").unwrap(),
            tpt_media_core::CodecId::new(*b"REED")
        );
        assert_eq!(
            args::resolve_codec("RAWR").unwrap(),
            tpt_media_core::CodecId::new(*b"RAWR")
        );
        assert!(args::resolve_codec("noper").is_none());
    }

    #[test]
    fn json_string_escaping() {
        let mut j = json::J::new();
        j.object();
        j.field("a\"b");
        j.value_str("line\nbreak");
        j.close();
        assert_eq!(j.finish(), "{\"a\\\"b\":\"line\\nbreak\"}");
    }

    #[test]
    fn json_nested_containers_do_not_emit_stray_commas() {
        // Regression: `object()`/`arr()` used to call `sep()` themselves,
        // so a container appearing as a *field value* produced invalid
        // JSON like `"key":,{...}`. This is exactly the shape `probe
        // --json` (timebase, tracks) and `benchmark --json` (encode_ms)
        // emit.
        let mut j = json::J::new();
        j.object();
        j.field("first");
        j.value_u64(1);
        j.field("nested");
        j.object();
        j.field("deep");
        j.object();
        j.close();
        j.close();
        j.field("list");
        j.arr();
        j.elem();
        j.object();
        j.field("x");
        j.value_u64(2);
        j.close();
        j.elem();
        j.value_str("y");
        j.close();
        j.close();
        assert_eq!(
            j.finish(),
            "{\"first\":1,\"nested\":{\"deep\":{}},\"list\":[{\"x\":2},\"y\"]}"
        );
    }

    #[test]
    fn probe_json_is_wellformed_for_nested_fields() {
        // `probe --json` on a container with a timebase and a track hits
        // every nested-container path; assert none of the stray-separator
        // shapes survive.
        let o = run(&argv(&["probe", "--json", "-"]), &stdin_ctx(make_tpt()));
        assert_eq!(o.code, 0, "stderr: {}", o.err);
        for bad in [":,{", ":,[", ",,", "[,", ",]", "{,", ",}", "{\"}\""] {
            assert!(!o.out.contains(bad), "found `{bad}` in {}", o.out);
        }
        assert!(o.out.contains("\"timebase\":{\"num\":1,\"den\":48000}"));
    }
}
