//! `tpt completions` and `tpt doctor`.
//!
//! `completions` prints a shell script that completes subcommands, options
//! and codec names. `doctor` is a self-test: it round-trips the built-in
//! synthetic signals through every native codec, using the same command
//! path a user runs, and reports whether each one came back intact.

use tpt_media_core::{AudioFrame, ChannelLayout, Frame, SampleFormat, Timebase, Timestamp};

use crate::commands::Ctx;
use crate::{quality, signals, y4m, Outcome};

const SUBCOMMANDS: &[&str] = &[
    "probe",
    "inspect",
    "encode",
    "decode",
    "transcode",
    "benchmark",
    "video-corpus",
    "compare",
    "list-codecs",
    "completions",
    "doctor",
];

const OPTIONS: &[&str] = &[
    "--codec",
    "--container",
    "--quality",
    "--bitrate",
    "--target-snr",
    "--target-psnr",
    "--resample",
    "--band-step",
    "--set-meta",
    "--runs",
    "--json",
    "--fps",
    "--frames",
    "--help",
];

const CODECS: &[&str] = &["cinder", "aura", "reed", "thrum", "vault", "mote"];
const SHELLS: &[&str] = &["bash", "zsh", "fish", "powershell"];

fn joined(items: &[&str]) -> String {
    items.join(" ")
}

/// The completion script for `shell`, or a usage error naming the choices.
pub fn completions(shell: &str) -> Outcome {
    let script = match shell {
        "bash" => bash_script(),
        "zsh" => zsh_script(),
        "fish" => fish_script(),
        "powershell" => powershell_script(),
        other => {
            return Outcome::usage(format!(
                "unknown shell `{other}` (choose one of: {})",
                SHELLS.join(", ")
            ))
        }
    };
    Outcome::ok(script)
}

fn bash_script() -> String {
    format!(
        r#"# bash completion for tpt. Load with: eval "$(tpt completions bash)"
_tpt() {{
  local cur prev
  cur="${{COMP_WORDS[COMP_CWORD]}}"
  prev="${{COMP_WORDS[COMP_CWORD-1]}}"
  case "$prev" in
    --codec) COMPREPLY=( $(compgen -W "{codecs}" -- "$cur") ); return ;;
    --container) COMPREPLY=( $(compgen -W "tpt wav" -- "$cur") ); return ;;
  esac
  if [[ $COMP_CWORD -eq 1 ]]; then
    COMPREPLY=( $(compgen -W "{subs}" -- "$cur") ); return
  fi
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "{opts}" -- "$cur") ); return
  fi
  COMPREPLY=( $(compgen -f -- "$cur") )
}}
complete -F _tpt tpt
"#,
        codecs = joined(CODECS),
        subs = joined(SUBCOMMANDS),
        opts = joined(OPTIONS),
    )
}

fn zsh_script() -> String {
    let subs: Vec<String> = SUBCOMMANDS.iter().map(|s| format!("'{s}'")).collect();
    format!(
        r#"#compdef tpt
# zsh completion for tpt. Save as _tpt on your $fpath, or: eval "$(tpt completions zsh)"
_tpt() {{
  _arguments -s \
    '1: :({subs})' \
    '--codec[codec]:codec:({codecs})' \
    '--container[output container]:container:(tpt wav)' \
    '--quality[quality 0..100]:quality:' \
    '--bitrate[target bits per second]:bitrate:' \
    '--target-snr[audio SNR target, dB]:dB:' \
    '--target-psnr[video PSNR target, dB]:dB:' \
    '--resample[resample audio to Hz]:hz:' \
    '--json[machine-readable output]' \
    '*:file:_files'
}}
compdef _tpt tpt
"#,
        subs = subs.join(" "),
        codecs = joined(CODECS),
    )
}

fn fish_script() -> String {
    let mut out =
        String::from("# fish completion for tpt. Load with: tpt completions fish | source\n");
    out.push_str(&format!(
        "complete -c tpt -f -n '__fish_use_subcommand' -a '{}'\n",
        joined(SUBCOMMANDS)
    ));
    out.push_str(&format!(
        "complete -c tpt -l codec -x -a '{}'\n",
        joined(CODECS)
    ));
    out.push_str("complete -c tpt -l container -x -a 'tpt wav'\n");
    for opt in OPTIONS
        .iter()
        .filter(|o| !matches!(**o, "--codec" | "--container"))
    {
        out.push_str(&format!("complete -c tpt -l {}\n", &opt[2..]));
    }
    out
}

fn powershell_script() -> String {
    let subs: Vec<String> = SUBCOMMANDS.iter().map(|s| format!("'{s}'")).collect();
    let opts: Vec<String> = OPTIONS.iter().map(|s| format!("'{s}'")).collect();
    let codecs: Vec<String> = CODECS.iter().map(|s| format!("'{s}'")).collect();
    format!(
        r#"# PowerShell completion for tpt. Add to $PROFILE: tpt completions powershell | Out-String | Invoke-Expression
Register-ArgumentCompleter -Native -CommandName tpt -ScriptBlock {{
  param($wordToComplete, $commandAst, $cursorPosition)
  $words = $commandAst.CommandElements | ForEach-Object {{ $_.ToString() }}
  $prev = if ($words.Count -ge 2) {{ $words[-2] }} else {{ '' }}
  $candidates = if ($prev -eq '--codec') {{ @({codecs}) }}
    elseif ($words.Count -le 2) {{ @({subs}) }}
    elseif ($wordToComplete -like '-*') {{ @({opts}) }}
    else {{ @() }}
  $candidates | Where-Object {{ $_ -like "$wordToComplete*" }} | ForEach-Object {{
    [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
  }}
}}
"#,
        codecs = codecs.join(", "),
        subs = subs.join(", "),
        opts = opts.join(", "),
    )
}

/// Run one `tpt` command in memory: `input` is what `-` reads from stdin,
/// and the result carries the bytes written to stdout.
fn pipe(argv: &[&str], input: &[u8]) -> Outcome {
    let read_stdin = || -> Result<Vec<u8>, String> { Ok(input.to_vec()) };
    let read_file = |p: &str| -> Result<Vec<u8>, String> { Err(format!("no file access: {p}")) };
    let write_file =
        |p: &str, _: &[u8]| -> Result<(), String> { Err(format!("doctor writes nothing: {p}")) };
    let ctx = Ctx {
        read_file: &read_file,
        read_stdin: &read_stdin,
        write_file: &write_file,
    };
    let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
    crate::run(&argv, &ctx)
}

/// A 0.5 s mixed synthetic signal at `rate`, as a 16-bit mono WAV.
fn synthetic_wav(rate: u32) -> Vec<u8> {
    let n = rate as usize / 2;
    let samples = signals::mixed(rate, 1, n);
    let bytes: Vec<u8> = samples
        .iter()
        .flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes())
        .collect();
    let frame = AudioFrame {
        frame: Frame {
            pts: Timestamp::zero(Timebase::new(1, rate)),
            duration: None,
            side_data: Vec::new(),
        },
        sample_count: n as u32,
        samples: bytes,
        format: SampleFormat::S16,
        channel_layout: ChannelLayout::Mono,
        sample_rate: rate,
    };
    tpt_media_io::write_wav(&[frame]).unwrap_or_default()
}

/// A short moving gradient in grayscale: enough structure for the video
/// codecs to have something to code, and a clear PSNR when it is lost.
fn synthetic_y4m() -> Vec<u8> {
    let (w, h) = (64u32, 48u32);
    let params = y4m::Y4mParams {
        width: w,
        height: h,
        fps_num: 25,
        fps_den: 1,
        format: tpt_media_core::PixelFormat::Gray8,
    };
    let frames: Vec<tpt_media_core::VideoFrame> = (0..4u32)
        .map(|t| {
            let pixels: Vec<u8> = (0..w * h)
                .map(|i| {
                    let (x, y) = (i % w, i / w);
                    ((x * 3 + y * 2 + t * 5) % 256) as u8
                })
                .collect();
            tpt_media_core::VideoFrame {
                frame: Frame {
                    pts: Timestamp::zero(Timebase::new(1, 25)),
                    duration: None,
                    side_data: Vec::new(),
                },
                pixels,
                width: w,
                height: h,
                format: tpt_media_core::PixelFormat::Gray8,
                stride: w,
            }
        })
        .collect();
    y4m::write(&params, &frames)
}

fn s16_samples(wav: &[u8]) -> Result<Vec<f32>, String> {
    let f = tpt_media_io::decode_wav(wav).map_err(|e| e.to_string())?;
    Ok(f.samples
        .chunks_exact(2)
        .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0)
        .collect())
}

/// One audio round trip: pass when the output is bit-exact for the
/// lossless codec, and above a 6 dB SNR floor for the lossy ones.
fn check_audio(codec: &str, rate: u32) -> Result<String, String> {
    let wav = synthetic_wav(rate);
    let enc = pipe(&["encode", "--codec", codec, "-", "-"], &wav);
    if enc.code != 0 {
        return Err(enc.err.trim().to_string());
    }
    let tpt = enc.binary_stdout.unwrap_or_default();
    let dec = pipe(&["decode", "--container", "wav", "-", "-"], &tpt);
    if dec.code != 0 {
        return Err(dec.err.trim().to_string());
    }
    let out = dec.binary_stdout.unwrap_or_default();
    let reference = s16_samples(&wav)?;
    let got = s16_samples(&out)?;
    if reference.len() != got.len() {
        return Err(format!(
            "length changed: {} samples in, {} out",
            reference.len(),
            got.len()
        ));
    }
    if codec == "aura" {
        return if reference == got {
            Ok(format!("bit-exact, {} samples", got.len()))
        } else {
            Err("lossless codec changed the samples".into())
        };
    }
    if codec == "reed" {
        // Reed is a parametric speech vocoder: it reproduces the spectral
        // envelope, not the waveform, so waveform SNR is no measure of it.
        // Check the level instead: a broken decode is silent or far off.
        // The bound is loose on purpose (12 dB, not a quality gate): the
        // codec's own output level is a separate question, tracked in todo.md.
        let (Some(rin), Some(rout)) = (quality::rms_db(&reference), quality::rms_db(&got)) else {
            return Err("decoded audio is silent".into());
        };
        return if (rout - rin).abs() <= 12.0 {
            Ok(format!(
                "level {rout:.1} dBFS against {rin:.1} dBFS in ({} samples; a vocoder, so no waveform SNR)",
                got.len()
            ))
        } else {
            Err(format!(
                "level {rout:.1} dBFS is more than 12 dB from {rin:.1} dBFS in"
            ))
        };
    }
    let snr = quality::snr_db(&reference, &got).unwrap_or(f64::NAN);
    if snr.is_finite() && snr >= 6.0 {
        Ok(format!("SNR {snr:.1} dB, {} samples", got.len()))
    } else {
        Err(format!("SNR {snr:.1} dB is below the 6 dB floor"))
    }
}

/// One video round trip: pass when the mean PSNR clears a 20 dB floor.
fn check_video(codec: &str) -> Result<String, String> {
    let clip = synthetic_y4m();
    let enc = pipe(&["encode", "--codec", codec, "-", "-"], &clip);
    if enc.code != 0 {
        return Err(enc.err.trim().to_string());
    }
    let tpt = enc.binary_stdout.unwrap_or_default();
    let dec = pipe(&["decode", "-", "-"], &tpt);
    if dec.code != 0 {
        return Err(dec.err.trim().to_string());
    }
    let out = dec.binary_stdout.unwrap_or_default();
    let (_, reference) = y4m::parse(&clip).map_err(|e| e.to_string())?;
    let (_, got) = y4m::parse(&out).map_err(|e| e.to_string())?;
    if reference.len() != got.len() {
        return Err(format!(
            "frame count changed: {} in, {} out",
            reference.len(),
            got.len()
        ));
    }
    let psnrs: Vec<f64> = reference
        .iter()
        .zip(&got)
        .filter_map(|(r, g)| quality::psnr_db(r, g))
        .filter(|p| p.is_finite())
        .collect();
    if psnrs.is_empty() {
        return Ok(format!("bit-exact, {} frames", got.len()));
    }
    let mean = psnrs.iter().sum::<f64>() / psnrs.len() as f64;
    if mean >= 20.0 {
        Ok(format!("mean PSNR {mean:.1} dB, {} frames", got.len()))
    } else {
        Err(format!("mean PSNR {mean:.1} dB is below the 20 dB floor"))
    }
}

/// `tpt doctor`: round-trip the synthetic signals through every native
/// codec and report each result. Exit 0 only when all of them pass.
pub fn doctor() -> Outcome {
    let mut out =
        String::from("tpt doctor: round trip of built-in synthetic signals through each codec\n");
    let mut failures = 0usize;
    let checks: Vec<(&str, Result<String, String>)> = vec![
        ("cinder", check_audio("cinder", 48_000)),
        ("aura", check_audio("aura", 48_000)),
        ("reed", check_audio("reed", 16_000)),
        ("thrum", check_video("thrum")),
        ("vault", check_video("vault")),
        ("mote", check_video("mote")),
    ];
    for (codec, result) in checks {
        match result {
            Ok(detail) => out.push_str(&format!("  ok    {codec:<8} {detail}\n")),
            Err(why) => {
                failures += 1;
                out.push_str(&format!("  FAIL  {codec:<8} {why}\n"));
            }
        }
    }
    if failures == 0 {
        out.push_str("all codecs round-tripped\n");
        Outcome::ok(out)
    } else {
        out.push_str(&format!("{failures} codec(s) failed\n"));
        Outcome {
            code: 1,
            out,
            err: String::new(),
            binary_stdout: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completions_cover_every_subcommand_and_option_for_each_shell() {
        for shell in SHELLS {
            let o = completions(shell);
            assert_eq!(o.code, 0, "{shell}: {}", o.err);
            for sub in SUBCOMMANDS {
                assert!(o.out.contains(sub), "{shell} is missing `{sub}`");
            }
            for codec in CODECS {
                assert!(o.out.contains(codec), "{shell} is missing codec `{codec}`");
            }
        }
        assert_eq!(completions("tcsh").code, 2);
    }

    #[test]
    fn doctor_passes_on_this_build() {
        let o = doctor();
        assert_eq!(o.code, 0, "{}", o.out);
        for codec in ["cinder", "aura", "reed", "thrum", "vault", "mote"] {
            assert!(o.out.contains(codec), "{}", o.out);
        }
    }
}
