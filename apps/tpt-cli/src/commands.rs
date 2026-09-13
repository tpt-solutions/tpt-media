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
        j.arr();
        for (track_id, offset) in &report.cluster_checksum_failures {
            j.elem();
            j.object();
            j.field("track_id");
            j.value_u64(u64::from(*track_id));
            j.field("chunk_offset");
            j.value_u64(*offset);
            j.close();
        }
        j.close();
    }
    j.field("tracks");
    j.object();
    j.field("count");
    j.value_u64(report.track_count() as u64);
    j.field("audio");
    track_list_json(&mut j, &report.audio, audio_track_json);
    j.field("video");
    track_list_json(&mut j, &report.video, video_track_json);
    j.field("other");
    track_list_json(&mut j, &report.other, other_track_json);
    j.close();
    if !report.codec_capabilities.is_empty() {
        j.field("codec_capabilities");
        track_list_json(&mut j, &report.codec_capabilities, caps_item_json);
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
    j.arr();
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
    j.close();
}

/// A track list as a JSON array. The helper opens and closes the array
/// itself (its elements are `elem()`-separated), so the caller only does
/// `field("audio")` first — probe's JSON used to emit
/// `"audio":,{...}` here because nothing ever opened the array.
fn track_list_json<T>(j: &mut J, tracks: &[T], item: fn(&mut J, &T)) {
    j.arr();
    for t in tracks {
        j.elem();
        item(j, t);
    }
    j.close();
}

fn audio_track_json(j: &mut J, t: &tpt_media_probe::AudioTrackInfo) {
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

fn video_track_json(j: &mut J, t: &tpt_media_probe::VideoTrackInfo) {
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

fn other_track_json(j: &mut J, t: &tpt_media_probe::TrackInfo) {
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

fn caps_item_json(j: &mut J, c: &tpt_media_probe::CodecCapabilityReport) {
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

    /// Lower-case display name used in human output.
    fn name(self) -> &'static str {
        match self {
            AudioCodec::Pulse => "pulse",
            AudioCodec::Aura => "aura",
            AudioCodec::Vox => "vox",
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

/// The unit of work shared by `tpt encode` and `tpt transcode`: mono S16
/// samples plus everything needed to mux them into a `.tpt` container.
struct EncodeJob {
    codec: AudioCodec,
    rate: u32,
    bits_per_coef: u8,
    /// S16 LE mono samples; the final partial block is zero-padded by
    /// [`run_encode_job`].
    samples: Vec<u8>,
    /// The original (pre-padding) sample count recorded in track metadata
    /// so `tpt decode` can trim the padding.
    sample_count: usize,
    /// Caller-supplied metadata pairs (`--set-meta`, or the source track's
    /// tags when transcoding), written ahead of the built-in sample count.
    extra_meta: Vec<(String, MetadataValue)>,
}

/// Samples per encode block per codec — the block sizes `tpt encode` has
/// always used (Pulse's 512 = its 2×256-sample window, Aura's 1024).
fn samples_per_block(codec: AudioCodec) -> usize {
    match codec {
        AudioCodec::Pulse => 2 * 256,
        AudioCodec::Aura => 1024,
        AudioCodec::Vox => unreachable!("vox jobs are rejected before encoding"),
    }
}

/// Encode a job into a finished `.tpt` container; returns the container
/// bytes and the packet count.
fn run_encode_job(job: &EncodeJob) -> Result<(Vec<u8>, usize), String> {
    let samples_per_block = samples_per_block(job.codec);
    let mut encoder = build_encoder(job.codec, job.rate, job.bits_per_coef)?;
    let track_timebase = Timebase::new(1, job.rate);
    let mut muxer = tpt_container::TptMuxer::new(track_timebase);
    let mut track = Track::new(0, MediaType::Audio, job.codec.codec_id(), track_timebase);
    track.codec_config.version = encoder.info().version;
    track.codec_config.data = match job.codec {
        AudioCodec::Pulse => vec![job.bits_per_coef],
        AudioCodec::Aura => (samples_per_block as u16).to_le_bytes().to_vec(),
        AudioCodec::Vox => return Err("the vox codec is not implemented yet".into()),
    };
    track.media = TrackMedia {
        sample_rate: Some(job.rate),
        channel_layout: Some(ChannelLayout::Mono),
        ..TrackMedia::default()
    };
    track.metadata = job.extra_meta.clone();
    if !track
        .metadata
        .iter()
        .any(|(k, _)| k == TRACK_SAMPLE_COUNT_KEY)
    {
        track.metadata.push((
            TRACK_SAMPLE_COUNT_KEY.to_string(),
            MetadataValue::Int(job.sample_count as i64),
        ));
    }
    muxer.register_track(&track);

    let total = job.samples.len() / 2;
    let mut written = 0usize;
    let mut block_index = 0usize;
    while block_index * samples_per_block < total {
        let start = block_index * samples_per_block;
        let end = (start + samples_per_block).min(total);
        let mut block = vec![0u8; samples_per_block * 2];
        block[..(end - start) * 2].copy_from_slice(&job.samples[start * 2..end * 2]);
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
            sample_rate: job.rate,
        };
        let packets = encoder
            .encode(&chunk)
            .map_err(|e| format!("encode failed: {e}"))?;
        for pkt in packets {
            if let Err(e) = muxer.write_packet_raw(&pkt) {
                return Err(format!("mux failed: {e:?}"));
            }
            written += 1;
        }
        block_index += 1;
    }
    Ok((muxer.finish(), written))
}

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
    let job = EncodeJob {
        codec,
        rate: frame.sample_rate,
        bits_per_coef: quality_to_bits(spec.quality),
        samples: frame.samples,
        sample_count: frame.sample_count as usize,
        extra_meta: spec
            .meta
            .iter()
            .map(|(k, v)| (k.clone(), MetadataValue::Text(v.clone())))
            .collect(),
    };
    let sample_count = job.sample_count;
    let (bytes, written) = match run_encode_job(&job) {
        Ok(r) => r,
        Err(e) => return Outcome::fail(1, e),
    };
    write_output(
        spec,
        bytes,
        format!("encoded {written} packets ({sample_count} samples)"),
        ctx,
    )
}

/// Demux and fully decode a `.tpt` byte buffer: returns the audio track
/// (with its metadata intact) and every decoded frame with block-size
/// padding already trimmed. Errors carry the CLI exit code they map to
/// (1 = I/O or malformed, 3 = codec not implemented).
fn demux_and_decode(
    input: &[u8],
    requested: Option<CodecId>,
) -> Result<(tpt_media_core::Track, Vec<tpt_media_core::AudioFrame>), (i32, String)> {
    let mut demux = tpt_container::TptDemuxer::new(input)
        .map_err(|e| (1, format!("invalid tpt input: {e}")))?;
    let track = demux
        .tracks()
        .iter()
        .find(|t| t.media_type == MediaType::Audio)
        .cloned();
    let track = match track {
        Some(t) => t,
        None => return Err((1, "no audio track found".into())),
    };
    let codec_id = track.codec_config.codec_id;
    if let Some(wanted) = requested {
        if wanted != codec_id {
            return Err((
                1,
                format!("file track uses {codec_id:?}, not the requested {wanted:?}"),
            ));
        }
    }
    let codec = match AudioCodec::from_id(codec_id) {
        Some(c) => c,
        None => return Err((1, "no decoder registered for this codec".into())),
    };
    if codec == AudioCodec::Vox {
        return Err((3, "the vox codec is not implemented yet".into()));
    }
    let rate = track.media.sample_rate.unwrap_or(48_000);
    let mut decoder = build_decoder(codec, rate, &track).map_err(|e| (1, e))?;

    let mut frames: Vec<tpt_media_core::AudioFrame> = Vec::new();
    loop {
        match demux.next_packet() {
            Ok(pkt) => {
                let decoded = decoder
                    .decode(&pkt)
                    .map_err(|e| (1, format!("decode failed: {e}")))?;
                frames.extend(decoded);
            }
            Err(tpt_media_container::ContainerError::Truncated) => break,
            Err(e) => return Err((1, format!("malformed container: {e:?}"))),
        }
    }
    if frames.is_empty() {
        return Err((1, "no decodable packets found".into()));
    }
    trim_padding(&mut frames, &track);
    Ok((track, frames))
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
    let (_track, frames) = match demux_and_decode(&input, spec.codec) {
        Ok(r) => r,
        Err((code, msg)) => return Outcome::fail(code, msg),
    };
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

/// Build a decoder from a track's codec configuration (version + private
/// data), the shape `tpt decode`/`transcode` encounter on disk.
fn build_decoder(
    codec: AudioCodec,
    rate: u32,
    track: &tpt_media_core::Track,
) -> Result<DecoderBox, String> {
    make_decoder(
        codec,
        rate,
        track.codec_config.version,
        &track.codec_config.data,
    )
}

/// Build a decoder from raw codec-private bytes — used by `benchmark`,
/// which has no track on disk, only the encoder it just drove (so the
/// private data mirrors what [`run_encode_job`] writes into the track).
fn make_decoder(
    codec: AudioCodec,
    rate: u32,
    codec_version: u16,
    private: &[u8],
) -> Result<DecoderBox, String> {
    let fw = FrameworkCodecConfig {
        version: codec_version,
        extra: private.to_vec(),
        limits: ResourceLimits::default(),
    };
    match codec {
        AudioCodec::Pulse => {
            let bits = private
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
            let block_size = if private.len() >= 2 {
                u16::from_le_bytes([private[0], private[1]])
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

/// `tpt transcode`: `.tpt` -> (decode) -> codec -> (re-mux) -> `.tpt`.
///
/// Decodes the input container's first audio track, then re-encodes the
/// recovered samples with `--codec` and muxes a fresh `.tpt`. The source
/// track's metadata tags are carried over (a `--set-meta` pair overrides
/// a tag of the same key), and the sample-count bookkeeping is refreshed
/// so the output decodes back to exactly the decoded sample count.
/// Transcoding is always lossy-with-respect-to-the-*source* when the
/// target is Pulse (lossy of lossy compounds); Aura targets re-mux the
/// decoded samples bit-exactly.
pub fn transcode_cmd(spec: &OpSpec, ctx: &Ctx<'_>) -> Outcome {
    if spec.container != ContainerKind::Tpt {
        return Outcome::fail(
            1,
            "transcode currently writes only the tpt container (drop --container wav)",
        );
    }
    let target = match spec.codec {
        Some(c) => c,
        None => return Outcome::usage("transcode requires --codec"),
    };
    let target = match AudioCodec::from_id(target) {
        Some(c) => c,
        None => return Outcome::fail(1, "no encoder registered for this codec"),
    };
    if target == AudioCodec::Vox {
        return Outcome::fail(3, "the vox codec is not implemented yet");
    }
    let input = match read_source(&spec.input, ctx) {
        Ok(b) => b,
        Err(e) => return Outcome::fail(1, e),
    };
    match sniff(&input) {
        ContainerKind::Tpt => {}
        ContainerKind::Wav => {
            return Outcome::fail(1, "input is a wav file; use `tpt encode` to compress it")
        }
        ContainerKind::Unknown => {
            return Outcome::fail(1, "input is not a recognisable media file")
        }
    }
    let (track, frames) = match demux_and_decode(&input, None) {
        Ok(r) => r,
        Err((code, msg)) => return Outcome::fail(code, msg),
    };
    let source = match AudioCodec::from_id(track.codec_config.codec_id) {
        Some(c) => c,
        None => AudioCodec::Vox, // unknown source: name is display-only
    };
    let rate = track.media.sample_rate.unwrap_or(48_000);
    if rate != 44_100 && rate != 48_000 {
        return Outcome::fail(
            1,
            format!("transcode requires a 44.1/48 kHz source track (got {rate} Hz)"),
        );
    }
    let samples: Vec<u8> = frames
        .iter()
        .flat_map(|f| f.samples.iter().copied())
        .collect();
    let sample_count: usize = frames.iter().map(|f| f.sample_count as usize).sum();
    // Carry the source tags, minus any key the caller overrides with
    // `--set-meta` (later pairs win by construction).
    let mut extra_meta: Vec<(String, MetadataValue)> = track
        .metadata
        .iter()
        .filter(|(k, _)| !spec.meta.iter().any(|(nk, _)| nk == k))
        .cloned()
        .collect();
    extra_meta.extend(
        spec.meta
            .iter()
            .map(|(k, v)| (k.clone(), MetadataValue::Text(v.clone()))),
    );
    let job = EncodeJob {
        codec: target,
        rate,
        bits_per_coef: quality_to_bits(spec.quality),
        samples,
        sample_count,
        extra_meta,
    };
    let (bytes, written) = match run_encode_job(&job) {
        Ok(r) => r,
        Err(e) => return Outcome::fail(1, e),
    };
    write_output(
        spec,
        bytes,
        format!(
            "transcoded {written} packets ({sample_count} samples, {} -> {})",
            source.name(),
            target.name()
        ),
        ctx,
    )
}

/// Median of a list (average of the two middle values for even lengths).
fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("durations are finite"));
    let mid = sorted.len() / 2;
    Some(if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    })
}

/// One measured benchmark pass over the whole input: returns
/// (encode ms, decode ms).
fn bench_pass(
    encoder: &mut dyn Encoder<Frame = tpt_media_core::AudioFrame>,
    decoder: &mut dyn Decoder<Frame = tpt_media_core::AudioFrame>,
    blocks: &[tpt_media_core::AudioFrame],
) -> Result<
    (
        f64,
        f64,
        Vec<tpt_media_core::Packet>,
        Vec<tpt_media_core::AudioFrame>,
    ),
    String,
> {
    let t0 = std::time::Instant::now();
    let mut packets = Vec::new();
    for block in blocks {
        packets.extend(
            encoder
                .encode(block)
                .map_err(|e| format!("encode failed: {e}"))?,
        );
    }
    let encode_ms = t0.elapsed().as_secs_f64() * 1e3;
    let t1 = std::time::Instant::now();
    let mut frames = Vec::new();
    for pkt in &packets {
        frames.extend(
            decoder
                .decode(pkt)
                .map_err(|e| format!("decode failed: {e}"))?,
        );
    }
    let decode_ms = t1.elapsed().as_secs_f64() * 1e3;
    // Sanity: a benchmark whose decode doesn't reproduce the input length
    // would silently report garbage quality numbers.
    let decoded: usize = frames.iter().map(|f| f.sample_count as usize).sum();
    let expected: usize = blocks.iter().map(|b| b.sample_count as usize).sum();
    if decoded != expected {
        return Err(format!(
            "decode produced {decoded} samples, expected {expected}"
        ));
    }
    Ok((encode_ms, decode_ms, packets, frames))
}

/// `tpt benchmark`: WAV -> codec -> WAV, measuring encode/decode wall
/// time and objective quality (§37 metrics: SNR + log-spectral distance)
/// of the round trip.
///
/// One untimed warm-up pass primes the MDCT/FFT plans and the allocator;
/// `--runs` (default 5) measured passes follow. Reported times are the
/// median and minimum across passes (minimum because a background
/// preemption can only ever make a pass slower, never faster). Timings
/// are machine-dependent by nature; the *quality* numbers and the packet
/// count are fully deterministic (§39).
pub fn benchmark_cmd(spec: &OpSpec, ctx: &Ctx<'_>) -> Outcome {
    let codec = match spec.codec {
        Some(c) => c,
        None => return Outcome::usage("benchmark requires --codec"),
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
                "benchmark currently requires 16-bit mono wav at 44.1/48 kHz \
                 (got {:?}, {} ch, {} Hz)",
                frame.format,
                frame.channel_layout.channels(),
                frame.sample_rate
            ),
        );
    }
    let rate = frame.sample_rate;
    let total = frame.sample_count as usize;
    if total == 0 {
        return Outcome::fail(1, "input wav has no samples");
    }
    let bits_per_coef = quality_to_bits(spec.quality);
    let runs = spec.runs.unwrap_or(5);
    let block = samples_per_block(codec);
    let mut encoder = match build_encoder(codec, rate, bits_per_coef) {
        Ok(e) => e,
        Err(e) => return Outcome::fail(1, e),
    };
    let private = match codec {
        AudioCodec::Pulse => vec![bits_per_coef],
        AudioCodec::Aura => (block as u16).to_le_bytes().to_vec(),
        AudioCodec::Vox => unreachable!("vox rejected above"),
    };
    let mut decoder = match make_decoder(codec, rate, encoder.info().version, &private) {
        Ok(d) => d,
        Err(e) => return Outcome::fail(1, e),
    };

    // Chunk the input into encode blocks exactly like `run_encode_job`
    // does, so the packet stream benchmarked here is the one `tpt encode`
    // would produce.
    let track_timebase = Timebase::new(1, rate);
    let mut blocks = Vec::new();
    let mut start = 0usize;
    while start < total {
        let end = (start + block).min(total);
        let mut samples = vec![0u8; block * 2];
        samples[..(end - start) * 2].copy_from_slice(&frame.samples[start * 2..end * 2]);
        blocks.push(tpt_media_core::AudioFrame {
            frame: tpt_media_core::Frame {
                pts: Timestamp {
                    ticks: start as i64,
                    timebase: track_timebase,
                },
                duration: None,
                side_data: Vec::new(),
            },
            samples,
            sample_count: block as u32,
            format: SampleFormat::S16,
            channel_layout: ChannelLayout::Mono,
            sample_rate: rate,
        });
        start += block;
    }

    // Pass 0 is an untimed warm-up (plan construction, page-ins).
    let mut encode_ms = Vec::new();
    let mut decode_ms = Vec::new();
    let mut last_pass: Option<(Vec<tpt_media_core::Packet>, Vec<tpt_media_core::AudioFrame>)> =
        None;
    for run in 0..=runs {
        match bench_pass(encoder.as_mut(), decoder.as_mut(), &blocks) {
            Ok((e, d, packets, frames)) => {
                if run > 0 {
                    encode_ms.push(e);
                    decode_ms.push(d);
                    last_pass = Some((packets, frames));
                }
            }
            Err(msg) => return Outcome::fail(1, msg),
        }
    }
    let (packets, frames) =
        last_pass.expect("at least one measured run is guaranteed by --runs >= 1");
    let packet_count = packets.len();
    let payload_bytes: u64 = packets.iter().map(|p| p.data.len() as u64).sum();

    // Quality of the round trip against the original samples.
    let mut decoded_samples: Vec<u8> = Vec::with_capacity(total * 2);
    for f in &frames {
        decoded_samples.extend_from_slice(&f.samples);
    }
    decoded_samples.truncate(total * 2);
    let reference = crate::signals::from_s16_le_bytes(&frame.samples);
    let candidate = crate::signals::from_s16_le_bytes(&decoded_samples);
    let snr = crate::quality::snr_db(&reference, &candidate);
    let lsd = crate::quality::log_spectral_distance_db(&reference, &candidate, 1024);
    let bit_exact = snr == Some(f64::INFINITY);

    let enc_med = median(&encode_ms).unwrap_or_default();
    let enc_min = encode_ms.iter().cloned().fold(f64::INFINITY, f64::min);
    let dec_med = median(&decode_ms).unwrap_or_default();
    let dec_min = decode_ms.iter().cloned().fold(f64::INFINITY, f64::min);
    let duration_s = total as f64 / rate as f64;
    let enc_factor = if enc_min > 0.0 {
        duration_s / (enc_min / 1e3)
    } else {
        0.0
    };
    let dec_factor = if dec_min > 0.0 {
        duration_s / (dec_min / 1e3)
    } else {
        0.0
    };
    let bits_per_sample = payload_bytes as f64 * 8.0 / total as f64;
    let bitrate = crate::quality::bitrate_bits_per_second(payload_bytes, total as u64, rate);

    if spec.json {
        let mut j = crate::json::J::new();
        j.object();
        j.field("codec");
        j.value_str(codec.name());
        j.field("quality");
        match spec.quality {
            Some(q) => j.value_u64(u64::from(q)),
            None => j.value_null(),
        }
        j.field("sample_rate");
        j.value_u64(u64::from(rate));
        j.field("samples");
        j.value_u64(total as u64);
        j.field("duration_seconds");
        j.value_f64(duration_s);
        j.field("packets");
        j.value_u64(packet_count as u64);
        j.field("runs");
        j.value_u64(runs as u64);
        j.field("payload_bytes");
        j.value_u64(payload_bytes);
        j.field("bits_per_sample");
        j.value_f64(bits_per_sample);
        if let Some(b) = bitrate {
            j.field("bitrate_bits_per_second");
            j.value_u64(b);
        }
        j.field("encode_ms");
        j.object();
        j.field("median");
        j.value_f64(enc_med);
        j.field("min");
        j.value_f64(enc_min);
        j.close();
        j.field("decode_ms");
        j.object();
        j.field("median");
        j.value_f64(dec_med);
        j.field("min");
        j.value_f64(dec_min);
        j.close();
        j.field("encode_realtime_factor_min");
        j.value_f64(enc_factor);
        j.field("decode_realtime_factor_min");
        j.value_f64(dec_factor);
        j.field("bit_exact");
        j.value_bool(bit_exact);
        // `inf` is not representable in JSON; a bit-exact round trip
        // reports `bit_exact: true` and omits `snr_db` instead.
        if let Some(s) = snr.filter(|s| s.is_finite()) {
            j.field("snr_db");
            j.value_f64(s);
        }
        if let Some(d) = lsd.filter(|d| d.is_finite()) {
            j.field("spectral_distance_db");
            j.value_f64(d);
        }
        j.close();
        Outcome::ok(j.finish())
    } else {
        let mut out = String::new();
        out.push_str(&format!(
            "benchmark: {} @ {} Hz, {} samples ({:.3} s), {} packets, quality {}\n",
            codec.name(),
            rate,
            total,
            duration_s,
            packet_count,
            spec.quality.map_or("-".into(), |q| q.to_string())
        ));
        out.push_str(&format!(
            "encode:   median {enc_med:.3} ms, min {enc_min:.3} ms  ({enc_factor:.0}x realtime)\n"
        ));
        out.push_str(&format!(
            "decode:   median {dec_med:.3} ms, min {dec_min:.3} ms  ({dec_factor:.0}x realtime)\n"
        ));
        out.push_str(&format!(
            "payload:  {payload_bytes} bytes, {bits_per_sample:.2} bits/sample, {}\n",
            bitrate.map_or("n/a".into(), |b| format!("{b} bit/s"))
        ));
        let snr_text = match snr {
            Some(s) if s.is_infinite() => "bit-exact (SNR = inf)".into(),
            Some(s) => format!("SNR {s:.1} dB"),
            None => "SNR undefined (silent input)".into(),
        };
        let lsd_text = lsd.map_or_else(|| "n/a".into(), |d| format!("spectral distance {d:.2} dB"));
        out.push_str(&format!("quality:  {snr_text}, {lsd_text}\n"));
        Outcome::ok(out)
    }
}

/// Execute a parsed command.
pub fn execute(cmd: &Command, ctx: &Ctx<'_>) -> Outcome {
    match cmd {
        Command::Probe { input, json } => probe(input, *json, ctx),
        Command::Inspect { input } => probe(input, false, ctx),
        Command::ListCodecs => list_codecs(),
        Command::Encode(spec) => encode_cmd(spec, ctx),
        Command::Decode(spec) => decode_cmd(spec, ctx),
        Command::Transcode(spec) => transcode_cmd(spec, ctx),
        Command::Benchmark(spec) => benchmark_cmd(spec, ctx),
        Command::Help => Outcome::ok(crate::args::USAGE.into()),
    }
}
