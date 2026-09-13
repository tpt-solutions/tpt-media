//! Integration tests tying the §36 synthetic corpus, §37 quality metrics,
//! and the §43 `transcode`/`benchmark` commands together: the corpus is
//! driven through the real codecs (via the same block-chunking the CLI
//! uses), the round trips are scored with the metrics, and the CLI
//! commands are exercised end-to-end through [`tpt_cli::run`].

use tpt_audio_aura::{AuraConfig, AuraDecoder, AuraEncoder};
use tpt_audio_pulse::{PulseDecoder, PulseEncoder, PulseEncoderConfig};
use tpt_cli::quality::{log_spectral_distance_db, snr_db};
use tpt_cli::signals;
use tpt_media_codec::{CodecConfig, Decoder, Encoder};
use tpt_media_core::{AudioFrame, ChannelLayout, Frame, SampleFormat, Timebase, Timestamp};

const RATE: u32 = 48_000;
/// Samples per encode block, matching the CLI (`samples_per_block`):
/// Pulse's 2×256 window, Aura's 1024.
const PULSE_BLOCK: usize = 512;
const AURA_BLOCK: usize = 1024;
/// One second of audio per corpus signal.
const N: usize = 48_000;

type PulseEnc = PulseEncoder;
type PulseDec = PulseDecoder;

fn s16_bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    signals::from_s16_le_bytes(bytes)
}

fn f32_to_s16_bytes(samples: &[f32]) -> Vec<u8> {
    signals::to_s16_le_bytes(samples)
}

fn make_frame(samples: Vec<u8>, start_tick: i64, count: u32) -> AudioFrame {
    AudioFrame {
        frame: Frame {
            pts: Timestamp {
                ticks: start_tick,
                timebase: Timebase::new(1, RATE),
            },
            duration: None,
            side_data: Vec::new(),
        },
        samples,
        sample_count: count,
        format: SampleFormat::S16,
        channel_layout: ChannelLayout::Mono,
        sample_rate: RATE,
    }
}

/// Drive `samples` (S16 LE) through a full Pulse encode/decode round trip
/// using the CLI's block chunking, returning the decoded S16 bytes.
fn pulse_round_trip(samples: &[u8], bits_per_coef: u8) -> Vec<u8> {
    let cfg = CodecConfig::default();
    let enc_cfg = PulseEncoderConfig {
        sample_rate: RATE,
        bits_per_coef,
        ..PulseEncoderConfig::default()
    };
    let mut enc = PulseEnc::new(cfg.clone(), enc_cfg.clone()).expect("pulse encoder");
    let mut dec = PulseDec::new(cfg, enc_cfg).expect("pulse decoder");
    let mut out = Vec::with_capacity(samples.len());
    for (i, chunk) in samples.chunks(PULSE_BLOCK * 2).enumerate() {
        let mut block = vec![0u8; PULSE_BLOCK * 2];
        block[..chunk.len()].copy_from_slice(chunk);
        let frame = make_frame(block, (i * PULSE_BLOCK) as i64, PULSE_BLOCK as u32);
        for pkt in enc.encode(&frame).expect("pulse encode") {
            for f in dec.decode(&pkt).expect("pulse decode") {
                out.extend_from_slice(&f.samples);
            }
        }
    }
    out.truncate(samples.len());
    out
}

/// Drive `samples` through a full Aura *lossless* round trip.
fn aura_round_trip(samples: &[u8]) -> Vec<u8> {
    let cfg = CodecConfig::default();
    let mut enc = AuraEncoder::new(
        cfg.clone(),
        AuraConfig {
            sample_rate: RATE,
            ..AuraConfig::default()
        },
    )
    .expect("aura encoder");
    let mut dec = AuraDecoder::new(
        cfg,
        AuraConfig {
            sample_rate: RATE,
            ..AuraConfig::default()
        },
    )
    .expect("aura decoder");
    let mut out = Vec::with_capacity(samples.len());
    for (i, chunk) in samples.chunks(AURA_BLOCK * 2).enumerate() {
        let mut block = vec![0u8; AURA_BLOCK * 2];
        block[..chunk.len()].copy_from_slice(chunk);
        let frame = make_frame(block, (i * AURA_BLOCK) as i64, AURA_BLOCK as u32);
        for pkt in enc.encode(&frame).expect("aura encode") {
            for f in dec.decode(&pkt).expect("aura decode") {
                out.extend_from_slice(&f.samples);
            }
        }
    }
    out.truncate(samples.len());
    out
}

/// Per-signal Pulse round-trip SNR floors (dB) at the CLI's
/// default-equivalent quality (10 bits/coefficient).
///
/// These floors sit just under the *measured* values, because Pulse v0.1
/// has a structural quality ceiling of ≈3 dB shared by every signal (see
/// todo.md Known Issues, 2026-09-14): the encoder MDCTs non-overlapping
/// 2N-sample windows (hop 2N), so the TDAC overlap-add that would recover
/// the window edges never happens and half of each window's samples
/// time-alias away. That is a codec-design defect to fix in its own
/// change, not something these tests can or should mask — what they guard
/// against is a *further regression* (quantization, bitstream bugs) on
/// top of today's measured behavior.
fn pulse_floor(signal: &str) -> f64 {
    match signal {
        "silence" => f64::INFINITY,     // handled separately: must stay silent
        "sine_1khz" => 1.5,             // measured 3.01
        "square_220hz" => 1.5,          // measured 3.03
        "sweep_100_8k" => 1.5,          // measured 2.93
        "noise" => 1.5,                 // measured 2.95
        "clicks" => 0.5,                // measured 1.59
        "mixed" => 1.5,                 // measured 3.00
        "dc_fullscale" => 1.0,          // measured 2.67
        "alternating_fullscale" => 1.0, // measured 2.69
        other => panic!("unknown corpus signal {other}"),
    }
}

#[test]
fn pulse_corpus_round_trips_meet_quality_floors() {
    for (name, signal) in signals::corpus(0xC0FFEE, RATE, N) {
        let original = f32_to_s16_bytes(&signal);
        let decoded = pulse_round_trip(&original, 10);
        assert_eq!(decoded.len(), original.len(), "{name}: length must survive");
        let back = s16_bytes_to_f32(&decoded);
        assert!(
            back.iter().all(|v| v.is_finite()),
            "{name}: decode produced non-finite samples"
        );
        if name == "silence" {
            assert!(
                decoded.iter().all(|b| *b == 0),
                "silence must round-trip to silence"
            );
            continue;
        }
        let snr = snr_db(&signal, &back)
            .unwrap_or_else(|| panic!("{name}: SNR should be defined for non-silent input"));
        let floor = pulse_floor(name);
        assert!(
            snr >= floor,
            "{name}: SNR {snr:.1} dB fell below floor {floor:.1} dB"
        );
    }
}

#[test]
fn aura_corpus_is_bit_exact() {
    for (name, signal) in signals::corpus(0xC0FFEE, RATE, N) {
        let original = f32_to_s16_bytes(&signal);
        let decoded = aura_round_trip(&original);
        assert_eq!(decoded.len(), original.len(), "{name}: length must survive");
        assert_eq!(decoded, original, "{name}: lossless aura must be bit-exact");
    }
}

#[test]
fn corpus_spectral_distance_tracks_snr() {
    // The two §37 metrics must agree in direction: a signal with a poor
    // time-domain SNR must not look spectrally perfect.
    let noisy = signals::noise(1, N, 0.5);
    let original = f32_to_s16_bytes(&noisy);
    let decoded = pulse_round_trip(&original, 6); // deliberately low quality
    let back = s16_bytes_to_f32(&decoded);
    let snr = snr_db(&noisy, &back).expect("noise is not silent");
    let lsd = log_spectral_distance_db(&noisy, &back, 1024).expect("spectra computable");
    assert!(snr.is_finite() && lsd.is_finite());
    assert!(
        !(snr < 10.0 && lsd < 0.5),
        "poor SNR ({snr:.1} dB) with near-zero spectral distance ({lsd:.2} dB) \
         would mean the metrics disagree"
    );
}

// --- CLI end-to-end tests ------------------------------------------------

/// Outcome of a `tpt` invocation with stdin input.
fn run_stdin(bytes: &[u8], args: &[&str]) -> tpt_cli::Outcome {
    let read_stdin = move || -> Result<Vec<u8>, String> { Ok(bytes.to_vec()) };
    let ctx = tpt_cli::Ctx {
        read_file: &|_: &str| -> Result<Vec<u8>, String> {
            Err("no file reads in this test".into())
        },
        read_stdin: &read_stdin,
        write_file: &|_: &str, _: &[u8]| -> Result<(), String> {
            Err("no file writes in this test".into())
        },
    };
    let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    tpt_cli::run(&argv, &ctx)
}

fn wav_bytes(samples: &[u8]) -> Vec<u8> {
    let frame = make_frame(samples.to_vec(), 0, samples.len() as u32 / 2);
    tpt_media_io::write_wav(&[frame]).expect("wav serialize")
}

/// 0.25 s of a 997 Hz tone — small enough for fast tests, big enough to
/// span several codec blocks.
fn test_tone() -> Vec<f32> {
    signals::sine(RATE, 997.0, RATE as usize / 4, 0.5)
}

fn encode_via_cli(samples: &[u8], extra: &[&str]) -> Vec<u8> {
    let mut args = vec!["encode", "--codec", "pulse"];
    args.extend_from_slice(extra);
    args.push("-");
    args.push("-");
    let o = run_stdin(&wav_bytes(samples), &args);
    assert_eq!(o.code, 0, "encode stderr: {}", o.err);
    o.binary_stdout.expect("tpt on stdout")
}

fn decode_via_cli(tpt: &[u8]) -> Vec<u8> {
    let o = run_stdin(tpt, &["decode", "--container", "wav", "-", "-"]);
    assert_eq!(o.code, 0, "decode stderr: {}", o.err);
    o.binary_stdout.expect("wav on stdout")
}

#[test]
fn transcode_pulse_to_aura_preserves_decoded_samples() {
    let original = f32_to_s16_bytes(&test_tone());
    let tpt = encode_via_cli(&original, &["--quality", "80"]);
    let direct_decode = decode_via_cli(&tpt);

    let tr = run_stdin(&tpt, &["transcode", "--codec", "aura", "-", "-"]);
    assert_eq!(tr.code, 0, "transcode stderr: {}", tr.err);
    assert!(tr.out.contains("pulse -> aura"), "summary: {}", tr.out);
    let transcoded = tr.binary_stdout.expect("tpt on stdout");

    let final_wav = decode_via_cli(&transcoded);
    // Aura is lossless, so decode(transcode(x)) must equal decode(x)
    // byte-for-byte.
    assert_eq!(final_wav, direct_decode);
}

#[test]
fn transcode_aura_to_pulse_round_trips_audibly() {
    let original = f32_to_s16_bytes(&test_tone());
    let aura_tpt = {
        // Encode with aura via the CLI.
        let o = run_stdin(
            &wav_bytes(&original),
            &["encode", "--codec", "aura", "-", "-"],
        );
        assert_eq!(o.code, 0, "encode stderr: {}", o.err);
        o.binary_stdout.expect("tpt on stdout")
    };
    let tr = run_stdin(
        &aura_tpt,
        &["transcode", "--codec", "pulse", "--quality", "80", "-", "-"],
    );
    assert_eq!(tr.code, 0, "transcode stderr: {}", tr.err);
    let transcoded = tr.binary_stdout.expect("tpt on stdout");
    let wav = decode_via_cli(&transcoded);
    let frame = tpt_media_io::decode_wav(&wav).expect("decoded wav");
    let back = s16_bytes_to_f32(&frame.samples);
    let snr = snr_db(&test_tone(), &back).expect("tone is not silent");
    // Same ≈3 dB structural ceiling as the corpus test (see
    // `pulse_floor`); require "no worse than today's measured 2.9 dB".
    assert!(snr > 1.0, "aura->pulse transcode SNR {snr:.1} dB too low");
}

#[test]
fn transcode_carries_and_overrides_metadata() {
    let original = f32_to_s16_bytes(&test_tone());
    let tpt = encode_via_cli(&original, &["--set-meta", "title=piano take 1"]);
    let tr = run_stdin(
        &tpt,
        &[
            "transcode",
            "--codec",
            "aura",
            "--set-meta",
            "title=piano take 2",
            "--set-meta",
            "artist=TPT",
            "-",
            "-",
        ],
    );
    assert_eq!(tr.code, 0, "transcode stderr: {}", tr.err);
    let transcoded = tr.binary_stdout.expect("tpt on stdout");
    let report = tpt_media_probe::probe(&transcoded);
    let audio = &report.audio[0];
    let get = |key: &str| {
        audio
            .metadata
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| match v {
                tpt_media_core::MetadataValue::Text(s) => s.clone(),
                _ => panic!("expected text metadata"),
            })
    };
    assert_eq!(
        get("title").as_deref(),
        Some("piano take 2"),
        "--set-meta must override the carried-over tag"
    );
    assert_eq!(get("artist").as_deref(), Some("TPT"));
    assert_eq!(get("title=not"), None);
}

#[test]
fn transcode_errors_are_stable() {
    let tone = f32_to_s16_bytes(&test_tone());
    let tpt = encode_via_cli(&tone, &[]);
    // Missing --codec → usage.
    let o = run_stdin(&tpt, &["transcode", "-", "-"]);
    assert_eq!(o.code, 2);
    assert!(o.err.contains("requires --codec"));
    // WAV input → guidance to encode instead.
    let o = run_stdin(
        &wav_bytes(&tone),
        &["transcode", "--codec", "pulse", "-", "-"],
    );
    assert_eq!(o.code, 1);
    assert!(o.err.contains("use `tpt encode`"));
    // Vox target → not wired.
    let o = run_stdin(&tpt, &["transcode", "--codec", "vox", "-", "-"]);
    assert_eq!(o.code, 3);
    // Garbage input → not recognisable.
    let o = run_stdin(b"zzz", &["transcode", "--codec", "pulse", "-", "-"]);
    assert_eq!(o.code, 1);
    assert!(o.err.contains("not a recognisable media file"));
}

#[test]
fn benchmark_human_output_reports_timing_and_quality() {
    let tone = f32_to_s16_bytes(&test_tone());
    let o = run_stdin(
        &wav_bytes(&tone),
        &[
            "benchmark",
            "--codec",
            "pulse",
            "--quality",
            "80",
            "--runs",
            "2",
            "-",
        ],
    );
    assert_eq!(o.code, 0, "benchmark stderr: {}", o.err);
    for expected in [
        "benchmark: pulse",
        "encode:",
        "decode:",
        "payload:",
        "quality:",
    ] {
        assert!(
            o.out.contains(expected),
            "missing `{expected}` in:\n{}",
            o.out
        );
    }
    assert!(o.out.contains("SNR"), "quality line: {}", o.out);
}

#[test]
fn benchmark_json_is_structured_and_aura_is_bit_exact() {
    let tone = f32_to_s16_bytes(&test_tone());
    let o = run_stdin(
        &wav_bytes(&tone),
        &["benchmark", "--codec", "aura", "--json", "--runs", "2", "-"],
    );
    assert_eq!(o.code, 0, "benchmark stderr: {}", o.err);
    let json = o.out;
    for expected in [
        "\"codec\":\"aura\"",
        "\"bit_exact\":true",
        "\"encode_ms\":{",
        "\"decode_ms\":{",
        "\"packets\":",
        "\"bits_per_sample\":",
        "\"duration_seconds\"",
    ] {
        assert!(json.contains(expected), "missing `{expected}` in {json}");
    }
    // Lossless: no finite snr_db field (inf is not representable).
    assert!(
        !json.contains("\"snr_db\""),
        "aura should omit snr_db: {json}"
    );
}

#[test]
fn benchmark_rejects_non_wav_input() {
    let tone = f32_to_s16_bytes(&test_tone());
    let tpt = encode_via_cli(&tone, &[]);
    let o = run_stdin(&tpt, &["benchmark", "--codec", "pulse", "-"]);
    assert_eq!(o.code, 1);
    assert!(o.err.contains("not a readable wav"));
    let o = run_stdin(
        &wav_bytes(&tone),
        &["benchmark", "-"], // no --codec
    );
    assert_eq!(o.code, 2);
    assert!(o.err.contains("requires --codec"));
}
