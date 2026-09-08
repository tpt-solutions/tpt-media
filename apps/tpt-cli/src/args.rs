//! Command-line and option parsing for `tpt`.
//!
//! Hand-rolled (no clap, per §1 "unnecessary dependencies"): a small,
//! deterministic parser with two-argument (`--flag value`) and positional
//! arguments. Every parse error carries a human message; every command
//! exit code is stable (2 = usage, 1 = I/O, 3 = component not wired yet).

use core::str::FromStr;

use tpt_media_core::CodecId;
use tpt_media_probe::ContainerKind;

/// The native TPT codec family (name -> 4-byte codec ID), shared by
/// `--codec` resolution and `tpt list-codecs`.
pub const NATIVE_CODECS: &[(&str, &str, &str)] = &[
    ("pulse", "PULS", "lossy audio (MDCT + noise shaping)"),
    ("aura", "AURA", "lossless audio (residual + range coding)"),
    ("vox", "VOX ", "narrowband speech"),
];

/// A requested pipeline operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `tpt probe [--json] <input>` — introspect a media file (see §44).
    Probe { input: String, json: bool },
    /// `tpt inspect <input>` — probe with human detail (alias of probe).
    Inspect { input: String },
    /// `tpt encode ...` — compress `<input>` into `<output>`.
    Encode(OpSpec),
    /// `tpt decode ...` — decompress `<input>` into `<output>`.
    Decode(OpSpec),
    /// `tpt transcode ...` — recompress between codecs.
    Transcode(OpSpec),
    /// `tpt benchmark ...` — measure encode/decode throughput.
    Benchmark(OpSpec),
    /// `tpt list-codecs` — list the native codec family.
    ListCodecs,
    /// `tpt --help`.
    Help,
}

/// Options shared by the encode family (codec + container + quality +
/// bitrate + metadata + machine-readable output).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpSpec {
    /// Selected codec ID (`--codec`).
    pub codec: Option<CodecId>,
    /// Selected container (`--container`), defaults to `tpt`.
    pub container: ContainerKind,
    /// Quality 0..=100 (`--quality`).
    pub quality: Option<u8>,
    /// Target bitrate in bits/second (`--bitrate`).
    pub bitrate_bits_per_second: Option<u64>,
    /// Metadata pairs to set (`--set-meta key=value`, repeatable).
    pub meta: Vec<(String, String)>,
    /// Emit machine-readable output (`--json`).
    pub json: bool,
    /// Benchmark repetitions (`--runs`).
    pub runs: Option<usize>,
    /// Input path, or `-` for stdin.
    pub input: String,
    /// Output path, or `-` for stdout.
    pub output: Option<String>,
}

impl Default for OpSpec {
    fn default() -> Self {
        OpSpec {
            codec: None,
            container: ContainerKind::Tpt,
            quality: None,
            bitrate_bits_per_second: None,
            meta: Vec::new(),
            json: false,
            runs: None,
            input: String::new(),
            output: None,
        }
    }
}

/// Resolve a `--codec` argument: a familiar name or a raw 4-byte codec ID.
pub fn resolve_codec(name: &str) -> Option<CodecId> {
    match name.to_ascii_lowercase().as_str() {
        "pulse" | "puls" => Some(CodecId::new(*b"PULS")),
        "aura" => Some(CodecId::new(*b"AURA")),
        "vox" => Some(CodecId::new(*b"VOX ")),
        _ => CodecId::from_str(name).ok(),
    }
}

/// Parse `--bitrate`: plain bits/s, or `k`/`m` suffixes.
pub fn parse_bitrate(s: &str) -> Result<u64, String> {
    let (digits, mult) = match s.as_bytes().last() {
        Some(b'k') | Some(b'K') => (&s[..s.len() - 1], 1_000u64),
        Some(b'm') | Some(b'M') => (&s[..s.len() - 1], 1_000_000u64),
        _ => (s, 1u64),
    };
    let n: u64 = digits
        .parse()
        .map_err(|_| format!("invalid bitrate `{s}` (expected bits/s, or k/m suffix)"))?;
    n.checked_mul(mult)
        .ok_or_else(|| format!("bitrate `{s}` overflows"))
}

fn take_value(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

/// Parse the full argument list (excluding the program name).
pub fn parse(argv: &[String]) -> Result<Command, String> {
    match argv.first().map(String::as_str) {
        None | Some("--help") | Some("-h") | Some("help") => Ok(Command::Help),
        Some("list-codecs") => Ok(Command::ListCodecs),
        Some(cmd @ ("probe" | "inspect")) => {
            let mut json = false;
            let mut input: Option<String> = None;
            let mut i = 1;
            while i < argv.len() {
                match argv[i].as_str() {
                    "--json" => json = true,
                    "--help" | "-h" => return Ok(Command::Help),
                    flag if flag.starts_with('-') && flag != "-" => {
                        return Err(format!("unknown option `{flag}` for {cmd}"))
                    }
                    positional => {
                        if input.is_some() {
                            return Err(format!("unexpected extra argument `{positional}`"));
                        }
                        input = Some(positional.to_string());
                    }
                }
                i += 1;
            }
            let input =
                input.ok_or_else(|| format!("{cmd} requires an input path (or `-` for stdin)"))?;
            Ok(if cmd == "probe" {
                Command::Probe { input, json }
            } else {
                Command::Inspect { input }
            })
        }
        Some(cmd @ ("encode" | "decode" | "transcode" | "benchmark")) => {
            let mut spec = OpSpec::default();
            let mut positionals: Vec<String> = Vec::new();
            let mut i = 1;
            while i < argv.len() {
                match argv[i].as_str() {
                    "--codec" => {
                        spec.codec = Some(
                            resolve_codec(&take_value(argv, &mut i, "--codec")?).ok_or_else(
                                || "unknown codec; use `tpt list-codecs`".to_string(),
                            )?,
                        )
                    }
                    "--container" => {
                        let v = take_value(argv, &mut i, "--container")?;
                        spec.container = match v.as_str() {
                            "tpt" => ContainerKind::Tpt,
                            "wav" => ContainerKind::Wav,
                            other => {
                                return Err(format!("unsupported container `{other}` (tpt or wav)"))
                            }
                        };
                    }
                    "--quality" => {
                        let v = take_value(argv, &mut i, "--quality")?;
                        let q: u8 = v
                            .parse()
                            .map_err(|_| format!("invalid quality `{v}` (0..=100)"))?;
                        if q > 100 {
                            return Err(format!("quality out of range: {q} (0..=100)"));
                        }
                        spec.quality = Some(q);
                    }
                    "--bitrate" => {
                        let v = take_value(argv, &mut i, "--bitrate")?;
                        spec.bitrate_bits_per_second = Some(parse_bitrate(&v)?);
                    }
                    "--set-meta" => {
                        let v = take_value(argv, &mut i, "--set-meta")?;
                        let (k, val) = v
                            .split_once('=')
                            .ok_or_else(|| format!("--set-meta expects key=value, got `{v}`"))?;
                        if k.is_empty() {
                            return Err("--set-meta key must not be empty".into());
                        }
                        spec.meta.push((k.to_string(), val.to_string()));
                    }
                    "--json" => spec.json = true,
                    "--runs" => {
                        let v = take_value(argv, &mut i, "--runs")?;
                        let n: usize = v
                            .parse()
                            .map_err(|_| format!("invalid --runs value `{v}`"))?;
                        if n == 0 {
                            return Err("--runs must be at least 1".into());
                        }
                        spec.runs = Some(n);
                    }
                    "--help" | "-h" => return Ok(Command::Help),
                    flag if flag.starts_with('-') && flag != "-" => {
                        return Err(format!("unknown option `{flag}` for {cmd}"))
                    }
                    positional => positionals.push(positional.to_string()),
                }
                i += 1;
            }
            if positionals.is_empty() {
                return Err(format!("{cmd} requires an input path (or `-` for stdin)"));
            }
            spec.input = positionals.remove(0);
            if cmd != "benchmark" {
                spec.output = positionals.pop();
            }
            if !positionals.is_empty() {
                return Err(format!("unexpected extra argument `{}`", positionals[0]));
            }
            Ok(match cmd {
                "encode" => Command::Encode(spec),
                "decode" => Command::Decode(spec),
                "transcode" => Command::Transcode(spec),
                _ => Command::Benchmark(spec),
            })
        }
        Some(other) => Err(format!("unknown command `{other}`")),
    }
}

/// The full usage text printed for `--help` and usage errors.
pub const USAGE: &str = "\
tpt — TPT media command-line interface

USAGE:
  tpt probe [--json] <input>          introspect a media file
  tpt inspect <input>                 probe with human detail
  tpt encode --codec <name> [..] <in> [<out>]   compress
  tpt decode --codec <name> [..] <in> [<out>]   decompress
  tpt transcode --codec <name> [..] <in> [<out>]
  tpt benchmark --codec <name> [..] <in>
  tpt list-codecs
  tpt --help

OPTIONS:
  --codec <name>       codec: pulse | aura | vox | <4cc>
  --container <kind>   tpt (default) | wav
  --quality <0..=100>  quality setting
  --bitrate <n>[k|m]   target bitrate (bits/s)
  --set-meta <k=v>     set a metadata pair (repeatable)
  --json               machine-readable output (probe)
  --runs <n>           benchmark repetitions
  --                    nothing else; `-` means stdin/stdout

Use `-` as <input> to read from stdin. <out> defaults to stdout.";
