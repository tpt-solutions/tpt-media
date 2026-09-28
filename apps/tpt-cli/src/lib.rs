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
//! | machine-readable output | `--json` on `probe` and `benchmark` |
//! | error reporting | stable exit codes (0 ok, 1 I/O or unsupported, 2 usage) |
//!
//! `probe`/`inspect`/`list-codecs`/`encode`/`decode`/`transcode`/
//! `benchmark` are all implemented. Objective quality metrics (§37)
//! live in [`quality`]; the deterministic synthetic corpus they are
//! validated against (§36, audio subset) lives in [`signals`].
//!
//! **Scope: the `encode`/`decode`/`transcode`/`benchmark` family is
//! audio-only** (WAV in, `.tpt`/WAV out). The native *video* codec
//! `thrum` resolves by name (`--codec thrum`, `--codec LIVE`) and
//! is listed by `tpt list-codecs`, but passing it to one of those
//! commands fails with an explicit "this is a video codec and these
//! commands are audio-only" message rather than a misleading
//! unknown-codec error. Driving it means using the
//! `tpt-video-thrum` crate directly; wiring raw video through
//! `tpt-media-io` is its own change (todo.md §32).

pub mod args;
pub mod commands;
pub mod json;
pub mod quality;
pub mod signals;
pub mod video;

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
            &argv(&["encode", "--codec", "cinder", "--container", "wav", "-", "-"]),
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
            &stdin_ctx(make_tpt()),
        );
        assert_eq!(o.code, 1);
        assert!(o.err.contains("writes only wav output"));
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
        // and --codec LIVE both resolve - but the encode/decode family
        // is audio-only, so the failure must say *why* rather than
        // claiming no encoder is registered.
        assert_eq!(
            args::resolve_codec("thrum").unwrap(),
            tpt_media_core::CodecId::new(*b"THRM")
        );
        assert_eq!(
            args::resolve_codec("THRM").unwrap(),
            tpt_media_core::CodecId::new(*b"THRM")
        );
        for cmd in ["encode", "transcode", "benchmark"] {
            let o = run(
                &argv(&[cmd, "--codec", "thrum", "in.raw"]),
                &file_ctx(&[]),
            );
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
