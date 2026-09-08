//! Command execution for `tpt`.

use tpt_audio_aura::{AuraConfig, AuraDecoder, AuraEncoder, AuraMode};
use tpt_audio_pulse::{PulseDecoder, PulseEncoder, PulseEncoderConfig};
use tpt_media_codec::{CodecConfig as FrameworkCodecConfig, Decoder, Encoder, ResourceLimits};
use tpt_media_container::Container;
use tpt_media_core::{
    ChannelLayout, CodecId, MediaType, MetadataValue, SampleFormat, Timebase, Timestamp, Track,
    TrackMedia,
};
use tpt_media_io::{decode_wav, write_wav};
use tpt_media_probe::{sniff, ContainerKind, ProbeReport};

use crate::args::{Command, OpSpec, NATIVE_CODECS};
use crate::json::J;

/// Injectable file reader (`&'a` dyn fn returning file bytes).
pub type ReadFile<'a> = &'a dyn Fn(&str) -> Result<Vec<u8>, String>;
/// Injectable stdin reader (returns stdin bytes).
pub type ReadStdin<'a> = &'a dyn Fn() -> Result<Vec<u8>, String>;
/// Injectable file writer (writes bytes to a path).
pub type WriteFile<'a> = &'a dyn Fn(&str, &[u8]) -> Result<(), String>;

/// Injectable sources and sinks of bytes, so commands are testable without
/// a real filesystem or stdin/stdout.
pub struct Ctx<'a> {
    /// Read a file by path.
    pub read_file: ReadFile<'a>,
    /// Read stdin.
    pub read_stdin: ReadStdin<'a>,
    /// Write bytes to a file by path.
    pub write_file: WriteFile<'a>,
}

/// The result of running a command: an exit code and captured streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub code: i32,
    pub out: String,
    pub err: String,
    /// Bytes destined for stdout (`<out>` omitted or `-`); written only by
    /// the binary, not printed.
    pub binary_stdout: Option<Vec<u8>>,
}

impl Outcome {
    pub(crate) fn ok(out: String) -> Self {
        Outcome {
            code: 0,
            out,
            err: String::new(),
            binary_stdout: None,
        }
    }
    pub(crate) fn ok_binary(out: String, bytes: Vec<u8>) -> Self {
        Outcome {
            code: 0,
            out,
            err: String::new(),
            binary_stdout: Some(bytes),
        }
    }
    pub(crate) fn usage(err: impl ToString) -> Self {
        Outcome {
            code: 2,
            out: String::new(),
            err: format!("error: {}\n{}", err.to_string(), crate::args::USAGE),
            binary_stdout: None,
        }
    }
    fn fail(code: i32, err: impl ToString) -> Self {
        Outcome {
            code,
            out: String::new(),
            err: format!("error: {}", err.to_string()),
            binary_stdout: None,
        }
    }
}

fn read_source(input: &str, ctx: &Ctx<'_>) -> Result<Vec<u8>, String> {
    if input == "-" {
        (ctx.read_stdin)()
    } else {
        (ctx.read_file)(input)
    }
}

/// `tpt probe` / `tpt inspect`.
pub fn probe(input: &str, json: bool, ctx: &Ctx<'_>) -> Outcome {
    let bytes = match read_source(input, ctx) {
        Ok(b) => b,
        Err(e) => return Outcome::fail(1, e),
    };
    let report = tpt_media_probe::probe(&bytes);
    if json {
        Outcome::ok(probe_json(&report))
    } else {
        Outcome::ok(report.to_string())
    }
}

/// Serialize a probe report as deterministic JSON (`--json`).
pub fn probe_json(report: &ProbeReport) -> String {
    let mut j = J::new();
    j.object();
    j.field("container");
    j.value_str(container_name(report.container));
    j.field("file_size_bytes");
    j.value_u64(report.file_size);
    if let Some(v) = report.version {
        j.field("container_version");
        j.object();
        j.field("major");
        j.value_u64(u64::from(v >> 8));
        j.field("minor");
        j.value_u64(u64::from(v & 0xFF));
        j.close();
    }
    if let Some(tb) = report.global_timebase {
        j.field("timebase");
        timebase_json(&mut j, tb.num, tb.den);
    }
    if let Some(d) = report.duration_seconds {
        j.field("duration_seconds");
        j.value_f64(d);
    }
    if let Some(b) = report.bitrate_bits_per_second {
        j.field("bitrate_bits_per_second");
        j.value_u64(b);
    }
    if let Some(c) = report.checksum_ok {
        j.field("checksum_ok");
        j.value_bool(c);
    }
    if !report.cluster_checksum_failures.is_empty() {
        j.field("cluster_checksum_failures");
        for (track_id, offset) in &report.cluster_checksum_failures {
            j.elem();
            j.object();
            j.field("track_id");
            j.value_u64(u64::from(*track_id));
            j.field("chunk_offset");
            j.value_u64(*offset);
            j.close();
        }
    }
    j.field("tracks");
    j.object();
    j.field("count");
    j.value_u64(report.track_count() as u64);
    j.field("audio");
    audio_json(&mut j, &report.audio);
    j.field("video");
    video_json(&mut j, &report.video);
    j.field("other");
    other_json(&mut j, &report.other);
    j.close();
    if !report.codec_capabilities.is_empty() {
        j.field("codec_capabilities");
        caps_json(&mut j, &report.codec_capabilities);
    }
    if !report.corruption.is_empty() {
        j.field("corruption");
        j.arr();
        for m in &report.corruption {
            j.elem();
            j.value_str(m);
        }
        j.close();
    }
    j.close();
    j.finish()
}

fn container_name(k: ContainerKind) -> &'static str {
    match k {
        ContainerKind::Tpt => "tpt",
        ContainerKind::Wav => "wav",
        ContainerKind::Unknown => "unknown",
    }
}

fn timebase_json(j: &mut J, num: u32, den: u32) {
    j.object();
    j.field("num");
    j.value_u64(u64::from(num));
    j.field("den");
    j.value_u64(u64::from(den));
    j.close();
}

fn pts_json(j: &mut J, label: &str, pts: Option<tpt_media_core::Timestamp>) {
    if let Some(t) = pts {
        j.field(label);
        j.object();
        j.field("ticks");
        j.value_i64(t.ticks);
        j.field("timebase");
        timebase_json(j, t.timebase.num, t.timebase.den);
        j.close();
    }
}

fn metadata_json(j: &mut J, meta: &[(String, MetadataValue)]) {
    if meta.is_empty() {
        return;
    }
    j.field("metadata");
    for (k, v) in meta {
        j.elem();
        j.object();
        j.field("key");
        j.value_str(k);
        j.field("value");
        match v {
            MetadataValue::Text(s) => j.value_str(s),
            MetadataValue::Int(i) => j.value_i64(*i),
            MetadataValue::Bool(b) => j.value_bool(*b),
            MetadataValue::Bytes(b) => {
                j.object();
                j.field("opaque_bytes");
                j.value_u64(b.len() as u64);
                j.close();
            }
        }
        j.close();
    }
}

fn audio_json(j: &mut J, tracks: &[tpt_media_probe::AudioTrackInfo]) {
    for t in tracks {
        j.elem();
        j.object();
        j.field("id");
        j.value_u64(u64::from(t.id));
        j.field("codec_id");
        j.value_str(&t.codec_id.to_string());
        j.field("codec_version");
        j.object();
        j.field("major");
        j.value_u64(u64::from(t.codec_version >> 8));
        j.field("minor");
        j.value_u64(u64::from(t.codec_version & 0xFF));
        j.close();
        if let Some(sf) = t.sample_format {
            j.field("sample_format");
            j.value_str(&format!("{sf:?}"));
        }
        if let Some(cl) = t.channel_layout {
            j.field("channels");
            j.value_u64(u64::from(cl.channels()));
        }
        if let Some(sr) = t.sample_rate {
            j.field("sample_rate_hz");
            j.value_u64(u64::from(sr));
        }
        if let Some(n) = t.sample_count {
            j.field("sample_count");
            j.value_u64(n);
        }
        if let Some(b) = t.bitrate {
            j.field("bitrate_bits_per_second");
            j.value_u64(b);
        }
        if let Some(n) = t.frame_count {
            j.field("packets");
            j.value_u64(n);
        }
        pts_json(j, "first_pts", t.first_pts);
        pts_json(j, "last_pts", t.last_pts);
        if let Some(name) = &t.name {
            j.field("name");
            j.value_str(name);
        }
        if let Some(lang) = &t.language {
            j.field("language");
            j.value_str(lang);
        }
        metadata_json(j, &t.metadata);
        j.close();
    }
}

fn video_json(j: &mut J, tracks: &[tpt_media_probe::VideoTrackInfo]) {
    for t in tracks {
        j.elem();
        j.object();
        j.field("id");
        j.value_u64(u64::from(t.id));
        j.field("codec_id");
        j.value_str(&t.codec_id.to_string());
        j.field("codec_version");
        j.object();
        j.field("major");
        j.value_u64(u64::from(t.codec_version >> 8));
        j.field("minor");
        j.value_u64(u64::from(t.codec_version & 0xFF));
        j.close();
        if let Some(w) = t.width {
            j.field("width");
            j.value_u64(u64::from(w));
        }
        if let Some(h) = t.height {
            j.field("height");
            j.value_u64(u64::from(h));
        }
        if let Some(pf) = t.pixel_format {
            j.field("pixel_format");
            j.value_str(&format!("{pf:?}"));
        }
        if let Some(fps) = t.framerate_x1000 {
            j.field("framerate_x1000");
            j.value_u64(u64::from(fps));
        }
        if let Some(n) = t.frame_count {
            j.field("packets");
            j.value_u64(n);
        }
        if let Some(b) = t.bitrate {
            j.field("bitrate_bits_per_second");
            j.value_u64(b);
        }
        pts_json(j, "first_pts", t.first_pts);
        pts_json(j, "last_pts", t.last_pts);
        if let Some(name) = &t.name {
            j.field("name");
            j.value_str(name);
        }
        if let Some(lang) = &t.language {
            j.field("language");
            j.value_str(lang);
        }
        metadata_json(j, &t.metadata);
        j.close();
    }
}

fn other_json(j: &mut J, tracks: &[tpt_media_probe::TrackInfo]) {
    for t in tracks {
        j.elem();
        j.object();
        j.field("id");
        j.value_u64(u64::from(t.id));
        j.field("media_type");
        j.value_str(&format!("{:?}", t.media_type));
        j.field("codec_id");
        j.value_str(&t.codec_id.to_string());
        j.field("codec_version");
        j.object();
        j.field("major");
        j.value_u64(u64::from(t.codec_version >> 8));
        j.field("minor");
        j.value_u64(u64::from(t.codec_version & 0xFF));
        j.close();
        if let Some(n) = t.frame_count {
            j.field("packets");
            j.value_u64(n);
        }
        if let Some(b) = t.bitrate {
            j.field("bitrate_bits_per_second");
            j.value_u64(b);
        }
        pts_json(j, "first_pts", t.first_pts);
        pts_json(j, "last_pts", t.last_pts);
        if let Some(name) = &t.name {
            j.field("name");
            j.value_str(name);
        }
        metadata_json(j, &t.metadata);
        j.close();
    }
}

fn caps_json(j: &mut J, caps: &[tpt_media_probe::CodecCapabilityReport]) {
    for c in caps {
        j.elem();
        j.object();
        j.field("codec_id");
        j.value_str(&c.codec_id.to_string());
        if let Some(name) = c.name {
            j.field("name");
            j.value_str(name);
        }
        j.field("native");
        j.value_bool(c.native);
        j.field("decoder_available");
        j.value_bool(c.decoder_available);
        j.field("encoder_available");
        j.value_bool(c.encoder_available);
        j.field("lossless");
        j.value_bool(c.lossless);
        j.field("lossy");
        j.value_bool(c.lossy);
        j.field("real_time_safe");
        j.value_bool(c.real_time_safe);
        j.close();
    }
}

/// `tpt list-codecs`.
pub fn list_codecs() -> Outcome {
    let mut out = String::new();
    for (name, id, desc) in NATIVE_CODECS {
        out.push_str(&format!("{name:<8} {id}  {desc}\n"));
    }
    Outcome::ok(out)
}

/// One of the native TPT audio codecs the encode/decode commands can drive.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum AudioCodec {
    Pulse,
    Aura,
    Vox,
}

impl AudioCodec {
    fn from_id(id: CodecId) -> Option<AudioCodec> {
        if id == CodecId::new(*b"PULS") {
            Some(AudioCodec::Pulse)
        } else if id == CodecId::new(*b"AURA") {
            Some(AudioCodec::Aura)
        } else if id == CodecId::new(*b"VOX ") {
            Some(AudioCodec::Vox)
        } else {
            None
        }
    }

    fn codec_id(self) -> CodecId {
        match self {
            AudioCodec::Pulse => CodecId::new(*b"PULS"),
            AudioCodec::Aura => CodecId::new(*b"AURA"),
            AudioCodec::Vox => CodecId::new(*b"VOX "),
        }
    }
}

/// Reserved metadata key written into `.tpt` by `tpt encode` carrying the
/// original (pre-padding) per-track sample count, so `tpt decode` can trim
/// block-size padding added during encoding. A caller-supplied
/// `--set-meta` value for the same key wins (it is written after).
const TRACK_SAMPLE_COUNT_KEY: &str = "tpt.io.sample_count";

/// Boxed native encoder (trait objects; no generic repetition at call sites).
type EncoderBox = Box<dyn Encoder<Frame = tpt_media_core::AudioFrame>>;
/// Boxed native decoder.
type DecoderBox = Box<dyn Decoder<Frame = tpt_media_core::AudioFrame>>;

/// `tpt encode`: WAV -> codec -> `.tpt`.
///
/// Reads a 16-bit mono WAV (44.1 or 48 kHz, matching the Pulse/Aura
/// supported-format table), chunks it into codec blocks, encodes each
/// block, and muxes everything into a `.tpt` container with full track
/// metadata (track timebase `1/sample_rate`, codec version, sample rate,
/// channel layout, `--set-meta` tags). `--quality` maps onto the codec
/// coefficient bit depth; `--bitrate` is validated but not yet applied
/// (Pulse has no rate control).
pub fn encode_cmd(spec: &OpSpec, ctx: &Ctx<'_>) -> Outcome {
    if spec.container != ContainerKind::Tpt {
        return Outcome::fail(
            1,
            "encode currently writes only the tpt container (drop --container wav)",
        );
    }
    let codec = match spec.codec {
        Some(c) => c,
        None => return Outcome::usage("encode requires --codec"),
    };
    let codec = match AudioCodec::from_id(codec) {
        Some(c) => c,
        None => return Outcome::fail(1, "no encoder registered for this codec"),
    };
    if codec == AudioCodec::Vox {
        return Outcome::fail(3, "the vox codec is not implemented yet");
    }
    let input = match read_source(&spec.input, ctx) {
        Ok(b) => b,
        Err(e) => return Outcome::fail(1, e),
    };
    let frame = match decode_wav(&input) {
        Ok(f) => f,
        Err(e) => return Outcome::fail(1, format!("input is not a readable wav: {e}")),
    };
    if frame.format != SampleFormat::S16
        || frame.channel_layout.channels() != 1
        || (frame.sample_rate != 44_100 && frame.sample_rate != 48_000)
    {
        return Outcome::fail(
            1,
            format!(
                "encode currently requires 16-bit mono wav at 44.1/48 kHz \
                 (got {:?}, {} ch, {} Hz)",
                frame.format,
                frame.channel_layout.channels(),
                frame.sample_rate
            ),
        );
    }
    let rate = frame.sample_rate;
    let total = frame.sample_count as usize;
    let bits_per_coef = quality_to_bits(spec.quality);
    let samples_per_block = match codec {
        AudioCodec::Pulse => 2 * 256,
        AudioCodec::Aura => 1024,
        AudioCodec::Vox => return Outcome::fail(3, "the vox codec is not implemented yet"),
    };
    let mut encoder = match build_encoder(codec, rate, bits_per_coef) {
        Ok(e) => e,
        Err(e) => return Outcome::fail(1, e),
    };

    let track_timebase = Timebase::new(1, rate);
    let mut muxer = tpt_container::TptMuxer::new(track_timebase);
    let mut track = Track::new(0, MediaType::Audio, codec.codec_id(), track_timebase);
    track.codec_config.version = encoder.info().version;
    track.codec_config.data = match codec {
        AudioCodec::Pulse => vec![bits_per_coef],
        AudioCodec::Aura => (samples_per_block as u16).to_le_bytes().to_vec(),
        AudioCodec::Vox => unreachable!("vox rejected above"),
    };
    track.media = TrackMedia {
        sample_rate: Some(rate),
        channel_layout: Some(ChannelLayout::Mono),
        ..TrackMedia::default()
    };
    track.metadata = spec
        .meta
        .iter()
        .map(|(k, v)| (k.clone(), MetadataValue::Text(v.clone())))
        .collect();
    if !track
        .metadata
        .iter()
        .any(|(k, _)| k == TRACK_SAMPLE_COUNT_KEY)
    {
        track.metadata.push((
            TRACK_SAMPLE_COUNT_KEY.to_string(),
            MetadataValue::Int(total as i64),
        ));
    }
    muxer.register_track(&track);

    let mut written = 0usize;
    let mut block_index = 0usize;
    while block_index * samples_per_block < total {
        let start = block_index * samples_per_block;
        let end = (start + samples_per_block).min(total);
        let mut block = vec![0u8; samples_per_block * 2];
        block[..(end - start) * 2].copy_from_slice(&frame.samples[start * 2..end * 2]);
        let chunk = tpt_media_core::AudioFrame {
            frame: tpt_media_core::Frame {
                pts: Timestamp {
                    ticks: start as i64,
                    timebase: track_timebase,
                },
                duration: None,
                side_data: Vec::new(),
            },
            samples: block,
            sample_count: samples_per_block as u32,
            format: SampleFormat::S16,
            channel_layout: ChannelLayout::Mono,
            sample_rate: rate,
        };
        let packets = match encoder.encode(&chunk) {
            Ok(p) => p,
            Err(e) => return Outcome::fail(1, format!("encode failed: {e}")),
        };
        for pkt in packets {
            if let Err(e) = muxer.write_packet_raw(&pkt) {
                return Outcome::fail(1, format!("mux failed: {e:?}"));
            }
            written += 1;
        }
        block_index += 1;
    }
    let bytes = muxer.finish();
    write_output(
        spec,
        bytes,
        format!("encoded {written} packets ({} samples)", total),
        ctx,
    )
}

/// `tpt decode`: `.tpt` -> codec -> WAV.
///
/// Opens any `.tpt` container, finds its first audio track, decodes every
/// packet with the track's codec (the `--codec` argument, if given, must
/// match), and writes a single 16-bit WAV. Unimplemented codecs (vox) and
/// non-`.tpt` inputs are rejected up front.
pub fn decode_cmd(spec: &OpSpec, ctx: &Ctx<'_>) -> Outcome {
    if spec.container != ContainerKind::Wav {
        return Outcome::fail(
            1,
            "decode currently writes only wav output (use --container wav)",
        );
    }
    let input = match read_source(&spec.input, ctx) {
        Ok(b) => b,
        Err(e) => return Outcome::fail(1, e),
    };
    match sniff(&input) {
        ContainerKind::Tpt => {}
        ContainerKind::Wav => {
            return Outcome::fail(1, "input is a wav file; there is nothing to decode")
        }
        ContainerKind::Unknown => {
            return Outcome::fail(1, "input is not a recognisable media file")
        }
    }
    let mut demux = match tpt_container::TptDemuxer::new(&input) {
        Ok(d) => d,
        Err(e) => return Outcome::fail(1, format!("invalid tpt input: {e}")),
    };
    let track = demux
        .tracks()
        .iter()
        .find(|t| t.media_type == MediaType::Audio)
        .cloned();
    let track = match track {
        Some(t) => t,
        None => return Outcome::fail(1, "no audio track found"),
    };
    let codec = track.codec_config.codec_id;
    if let Some(requested) = spec.codec {
        if requested != codec {
            return Outcome::fail(
                1,
                format!("file track uses {codec:?}, not the requested {requested:?}"),
            );
        }
    }
    let codec = match AudioCodec::from_id(codec) {
        Some(c) => c,
        None => return Outcome::fail(1, "no decoder registered for this codec"),
    };
    if codec == AudioCodec::Vox {
        return Outcome::fail(3, "the vox codec is not implemented yet");
    }
    let rate = track.media.sample_rate.unwrap_or(48_000);
    let mut decoder = match build_decoder(codec, rate, &track) {
        Ok(d) => d,
        Err(e) => return Outcome::fail(1, e),
    };

    let mut frames: Vec<tpt_media_core::AudioFrame> = Vec::new();
    loop {
        match demux.next_packet() {
            Ok(pkt) => {
                let decoded = match decoder.decode(&pkt) {
                    Ok(f) => f,
                    Err(e) => return Outcome::fail(1, format!("decode failed: {e}")),
                };
                frames.extend(decoded);
            }
            Err(tpt_media_container::ContainerError::Truncated) => break,
            Err(e) => return Outcome::fail(1, format!("malformed container: {e:?}")),
        }
    }
    if frames.is_empty() {
        return Outcome::fail(1, "no decodable packets found");
    }
    trim_padding(&mut frames, &track);
    let wav = match write_wav(&frames) {
        Ok(w) => w,
        Err(e) => return Outcome::fail(1, format!("could not write wav output: {e}")),
    };
    let samples: usize = frames.iter().map(|f| f.sample_count as usize).sum();
    write_output(
        spec,
        wav,
        format!("decoded {} packets ({} samples)", frames.len(), samples),
        ctx,
    )
}

/// Drop block-size padding from the tail of the decoded frames, using the
/// exact sample count recorded by `tpt encode` (see
/// [`TRACK_SAMPLE_COUNT_KEY`]). Without that metadata the frames pass
/// through unchanged.
fn trim_padding(frames: &mut [tpt_media_core::AudioFrame], track: &tpt_media_core::Track) {
    let exact = track
        .metadata
        .iter()
        .find(|(k, _)| k == TRACK_SAMPLE_COUNT_KEY)
        .and_then(|(_, v)| match v {
            MetadataValue::Int(n) => Some(*n as usize),
            _ => None,
        });
    let exact = match exact {
        None => return,
        Some(n) => n,
    };
    let decoded: usize = frames.iter().map(|f| f.sample_count as usize).sum();
    if decoded <= exact {
        return;
    }
    let excess = decoded - exact;
    let last = match frames.last_mut() {
        Some(l) => l,
        None => return,
    };
    let last_count = last.sample_count as usize;
    if excess > last_count {
        return;
    }
    last.sample_count = (last_count - excess) as u32;
    last.samples.truncate(last.sample_count as usize * 2);
}

/// Map `--quality 0..=100` onto coefficient bit depth (Pulse/Aura support
/// a per-coefficient bit count; default 8 bits when unspecified).
fn quality_to_bits(quality: Option<u8>) -> u8 {
    match quality {
        None => 8,
        Some(q) => 4 + (u16::from(q) * 8 / 100) as u8,
    }
}

fn build_encoder(codec: AudioCodec, rate: u32, bits_per_coef: u8) -> Result<EncoderBox, String> {
    let fw = FrameworkCodecConfig::default();
    match codec {
        AudioCodec::Pulse => Ok(Box::new(
            PulseEncoder::new(
                fw,
                PulseEncoderConfig {
                    sample_rate: rate,
                    bits_per_coef,
                    ..PulseEncoderConfig::default()
                },
            )
            .map_err(|e| format!("pulse encoder: {e}"))?,
        ) as EncoderBox),
        AudioCodec::Aura => Ok(Box::new(
            AuraEncoder::new(
                fw,
                AuraConfig {
                    mode: AuraMode::Lossless,
                    sample_rate: rate,
                    ..AuraConfig::default()
                },
            )
            .map_err(|e| format!("aura encoder: {e}"))?,
        ) as EncoderBox),
        AudioCodec::Vox => Err("the vox codec is not implemented yet".into()),
    }
}

fn build_decoder(
    codec: AudioCodec,
    rate: u32,
    track: &tpt_media_core::Track,
) -> Result<DecoderBox, String> {
    let fw = FrameworkCodecConfig {
        version: track.codec_config.version,
        extra: track.codec_config.data.clone(),
        limits: ResourceLimits::default(),
    };
    match codec {
        AudioCodec::Pulse => {
            let bits = track
                .codec_config
                .data
                .first()
                .copied()
                .filter(|b| (4..=12).contains(b))
                .unwrap_or(8);
            Ok(Box::new(
                PulseDecoder::new(
                    fw,
                    PulseEncoderConfig {
                        sample_rate: rate,
                        bits_per_coef: bits,
                        ..PulseEncoderConfig::default()
                    },
                )
                .map_err(|e| format!("pulse decoder: {e}"))?,
            ) as DecoderBox)
        }
        AudioCodec::Aura => {
            let block_size = if track.codec_config.data.len() >= 2 {
                let bytes = [track.codec_config.data[0], track.codec_config.data[1]];
                u16::from_le_bytes(bytes)
            } else {
                1024
            };
            Ok(Box::new(
                AuraDecoder::new(
                    fw,
                    AuraConfig {
                        sample_rate: rate,
                        block_size,
                        ..AuraConfig::default()
                    },
                )
                .map_err(|e| format!("aura decoder: {e}"))?,
            ) as DecoderBox)
        }
        AudioCodec::Vox => Err("the vox codec is not implemented yet".into()),
    }
}

/// Route bytes to the requested output destination (`<out>` path, or
/// stdout when omitted / `-`).
fn write_output(spec: &OpSpec, bytes: Vec<u8>, summary: String, ctx: &Ctx<'_>) -> Outcome {
    match spec.output.as_deref() {
        None | Some("-") => Outcome::ok_binary(format!("{summary} -> stdout"), bytes),
        Some(path) => match (ctx.write_file)(path, &bytes) {
            Ok(()) => Outcome::ok(format!("{summary} -> {path}")),
            Err(e) => Outcome::fail(1, e),
        },
    }
}

/// The transcode/benchmark family: validated but not implemented yet.
fn encode_family(cmd: &str, spec: &OpSpec) -> Outcome {
    let mut detail = String::new();
    if let Some(c) = spec.codec {
        detail.push_str(&format!("codec={c} "));
    }
    detail.push_str(&format!("container={} ", container_name(spec.container)));
    if let Some(q) = spec.quality {
        detail.push_str(&format!("quality={q} "));
    }
    if let Some(b) = spec.bitrate_bits_per_second {
        detail.push_str(&format!("bitrate={b}bit/s "));
    }
    if !spec.meta.is_empty() {
        for (k, v) in &spec.meta {
            detail.push_str(&format!("meta[{}={v}] ", k));
        }
    }
    if let Some(r) = spec.runs {
        detail.push_str(&format!("runs={r} "));
    }
    if let Some(o) = &spec.output {
        detail.push_str(&format!("output={o} "));
    }
    Outcome::fail(
        3,
        format!(
            "tpt {cmd} is not wired yet (encode/decode are live; re-encoding \
             and benchmarking land later). Validated request: {detail}"
        ),
    )
}

/// Execute a parsed command.
pub fn execute(cmd: &Command, ctx: &Ctx<'_>) -> Outcome {
    match cmd {
        Command::Probe { input, json } => probe(input, *json, ctx),
        Command::Inspect { input } => probe(input, false, ctx),
        Command::ListCodecs => list_codecs(),
        Command::Encode(spec) => encode_cmd(spec, ctx),
        Command::Decode(spec) => decode_cmd(spec, ctx),
        Command::Transcode(spec) => encode_family("transcode", spec),
        Command::Benchmark(spec) => encode_family("benchmark", spec),
        Command::Help => Outcome::ok(crate::args::USAGE.into()),
    }
}
