//! Command execution for `tpt`.

use tpt_audio_aura::{AuraConfig, AuraDecoder, AuraEncoder, AuraMode};
use tpt_audio_cinder::{CinderDecoder, CinderEncoder, CinderEncoderConfig};
use tpt_audio_reed::{ReedConfig, ReedDecoder, ReedEncoder};
use tpt_media_codec::{CodecConfig as FrameworkCodecConfig, Decoder, Encoder, ResourceLimits};
use tpt_media_container::Container;
use tpt_media_core::{
    ChannelLayout, CodecId, MediaType, MetadataValue, PixelFormat, SampleFormat, Timebase,
    Timestamp, Track, TrackMedia, VideoFrame,
};
use tpt_media_io::{decode_wav, write_wav};
use tpt_media_probe::{sniff, ContainerKind, ProbeReport};
use tpt_video_mote::{MoteConfig, MoteDecoder, MoteEncoder};
use tpt_video_thrum::{ThrumConfig, ThrumDecoder, ThrumEncoder};
use tpt_video_vault::{VaultConfig, VaultDecoder, VaultEncoder};

use crate::args::{Command, OpSpec, NATIVE_CODECS};
use crate::json::J;
use crate::y4m;

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

/// The codec registry `probe`/`inspect` resolve capabilities against:
/// every native codec this binary can actually encode *and* decode.
///
/// `tpt_media_probe::probe` answers from an *empty* registry, which is the
/// right default for a library that cannot know what its caller linked in —
/// but from this binary it made `tpt probe` print
/// `encoder=false, decoder=false` for codecs the same program encodes and
/// decodes one subcommand later. A capability report that contradicts the
/// tool reporting it is worse than no report.
///
/// Thrum's factories are geometry-parameterised (a video codec's
/// configuration *is* its geometry), so they are built from
/// [`ThrumConfig::default`]. That placeholder never reaches the report —
/// `CodecCapabilityReport` carries no geometry, and `build()` takes the
/// real dimensions from the track's codec-private bytes — but it is why
/// this function exists rather than a plain list of unit structs.
fn probe_registry() -> tpt_media_codec::Registry {
    use tpt_audio_aura::{AuraDecoderFactory, AuraEncoderFactory};
    use tpt_audio_cinder::{CinderDecoderFactory, CinderEncoderFactory};
    use tpt_audio_reed::{ReedDecoderFactory, ReedEncoderFactory};
    use tpt_video_thrum::{ThrumDecoderFactory, ThrumEncoderFactory};

    let mut r = tpt_media_codec::Registry::new();
    r.register_decoder(CinderDecoderFactory);
    r.register_encoder(CinderEncoderFactory);
    r.register_decoder(AuraDecoderFactory);
    r.register_encoder(AuraEncoderFactory);
    r.register_decoder(ReedDecoderFactory);
    r.register_encoder(ReedEncoderFactory);
    // Geometry placeholders only; see the doc comment. A factory that
    // refuses to construct would silently under-report the codec, so the
    // defaults are validated here and a failure leaves the video codec
    // unregistered rather than panicking in a reporting path.
    if let (Ok(dec), Ok(enc)) = (
        ThrumDecoderFactory::new(ThrumConfig::default()),
        ThrumEncoderFactory::new(ThrumConfig::default()),
    ) {
        r.register_decoder(dec);
        r.register_encoder(enc);
    }
    // Vault: same placeholder-geometry rule as Thrum. Registered so a
    // `.tpt` carrying a VAUL track probes as decodable. `encode`/`decode`
    // drive it through the y4m bridge; these factories are what `probe`
    // consults, not what the encode path builds (that constructs a
    // geometry-carrying config directly).
    if let (Ok(dec), Ok(enc)) = (
        tpt_video_vault::VaultDecoderFactory::new(tpt_video_vault::VaultConfig::default()),
        tpt_video_vault::VaultEncoderFactory::new(tpt_video_vault::VaultConfig::default()),
    ) {
        r.register_decoder(dec);
        r.register_encoder(enc);
    }
    // Mote: same placeholder-geometry rule as Thrum, and the same split
    // between the probe registry here and the y4m bridge in
    // `VideoCodec::new_encoder`/`new_decoder`.
    if let (Ok(dec), Ok(enc)) = (
        tpt_video_mote::MoteDecoderFactory::new(tpt_video_mote::MoteConfig::default()),
        tpt_video_mote::MoteEncoderFactory::new(tpt_video_mote::MoteConfig::default()),
    ) {
        r.register_decoder(dec);
        r.register_encoder(enc);
    }
    r
}

/// `tpt probe` / `tpt inspect`.
pub fn probe(input: &str, json: bool, ctx: &Ctx<'_>) -> Outcome {
    let bytes = match read_source(input, ctx) {
        Ok(b) => b,
        Err(e) => return Outcome::fail(1, e),
    };
    let report = tpt_media_probe::probe_with_registry(&bytes, &probe_registry());
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
    Cinder,
    Aura,
    Reed,
}

impl AudioCodec {
    fn from_id(id: CodecId) -> Option<AudioCodec> {
        if id == CodecId::new(*b"CIND") {
            Some(AudioCodec::Cinder)
        } else if id == CodecId::new(*b"AURA") {
            Some(AudioCodec::Aura)
        } else if id == CodecId::new(*b"REED") {
            Some(AudioCodec::Reed)
        } else {
            None
        }
    }

    fn codec_id(self) -> CodecId {
        match self {
            AudioCodec::Cinder => CodecId::new(*b"CIND"),
            AudioCodec::Aura => CodecId::new(*b"AURA"),
            AudioCodec::Reed => CodecId::new(*b"REED"),
        }
    }

    /// Lower-case display name used in human output.
    fn name(self) -> &'static str {
        match self {
            AudioCodec::Cinder => "cinder",
            AudioCodec::Aura => "aura",
            AudioCodec::Reed => "reed",
        }
    }
}

/// The native video codecs. A video codec ID reaching a *non*-video
/// path — an `encode`/`benchmark` input that is not y4m, a `.tpt` whose
/// track is audio — is rejected with an explicit message rather than a
/// generic "no encoder registered" that would read as if the codec did
/// not exist.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    Thrum,
    Vault,
    Mote,
}

impl VideoCodec {
    fn from_id(id: CodecId) -> Option<VideoCodec> {
        if id == CodecId::new(*b"THRM") {
            Some(VideoCodec::Thrum)
        } else if id == CodecId::new(*b"VAUL") {
            Some(VideoCodec::Vault)
        } else if id == CodecId::new(*b"MOTE") {
            Some(VideoCodec::Mote)
        } else {
            None
        }
    }

    fn codec_id(self) -> CodecId {
        match self {
            VideoCodec::Thrum => CodecId::new(*b"THRM"),
            VideoCodec::Vault => CodecId::new(*b"VAUL"),
            VideoCodec::Mote => CodecId::new(*b"MOTE"),
        }
    }

    /// Lower-case display name used in human output.
    fn name(self) -> &'static str {
        match self {
            VideoCodec::Thrum => "thrum",
            VideoCodec::Vault => "vault",
            VideoCodec::Mote => "mote",
        }
    }

    /// Build an encoder for `geometry` at quantiser index `qp`, together
    /// with the codec-private bytes that describe it in a track header.
    ///
    /// Returned as a pair because the two must agree: the track records the
    /// configuration the encoder was actually built with, and a decoder
    /// reconstructs from those bytes alone. Building both here is what keeps
    /// a `.tpt` self-describing.
    ///
    /// This erases at `Frame = VideoFrame` and deliberately does *not* use
    /// the type-erased `DynEncoder` the FFI and the web player drive.
    /// `DynFrame` carries no PTS and the `Dyn*` wrappers substitute
    /// `Timestamp::zero` for it, so a stream muxed through them would lose
    /// every frame timestamp — which the y4m bridge sets and the container
    /// needs. Erasing at the concrete frame type keeps PTS intact and is
    /// still enough to drop the per-codec types from these call sites.
    fn new_encoder(
        self,
        width: u32,
        height: u32,
        format: PixelFormat,
        qp: u8,
        bits_per_frame: Option<u32>,
    ) -> Result<(VideoEncoderBox, Vec<u8>), String> {
        let fw = FrameworkCodecConfig::default();
        match self {
            VideoCodec::Thrum => {
                let cfg = ThrumConfig {
                    width,
                    height,
                    format,
                    qp,
                    ..ThrumConfig::default()
                };
                let enc = ThrumEncoder::new(fw, cfg.clone())
                    .map_err(|e| format!("thrum encoder: {e}"))?;
                Ok((Box::new(enc), cfg.to_codec_private()))
            }
            VideoCodec::Vault => {
                let cfg = VaultConfig {
                    width,
                    height,
                    format,
                    qp,
                    // `--bitrate` / fps: Vault's frame-level rate control
                    // (todo.md §33). Thrum has no rate control yet.
                    target_bits_per_frame: bits_per_frame,
                    ..VaultConfig::default()
                };
                let enc = VaultEncoder::new(fw, cfg.clone())
                    .map_err(|e| format!("vault encoder: {e}"))?;
                Ok((Box::new(enc), cfg.to_codec_private()))
            }
            VideoCodec::Mote => {
                let cfg = MoteConfig {
                    width,
                    height,
                    format,
                    ..MoteConfig::default()
                };
                let enc =
                    MoteEncoder::new(fw, cfg.clone()).map_err(|e| format!("mote encoder: {e}"))?;
                Ok((Box::new(enc), cfg.to_codec_private()))
            }
        }
    }

    /// Build a decoder for `geometry` from a track's header: the
    /// codec-private `bytes` and the `version` the track declared.
    ///
    /// A track whose codec-private data is absent or too short falls back to
    /// the default configuration for that geometry, which is what the y4m
    /// bridge relies on for a file it has just written and what keeps a
    /// hand-built `.tpt` decodable.
    fn new_decoder(
        self,
        width: u32,
        height: u32,
        format: PixelFormat,
        bytes: &[u8],
        version: u16,
    ) -> Result<VideoDecoderBox, String> {
        let fw = FrameworkCodecConfig {
            version,
            extra: bytes.to_vec(),
            limits: ResourceLimits::default(),
        };
        match self {
            VideoCodec::Thrum => {
                let cfg = if bytes.len() >= ThrumConfig::CODEC_PRIVATE_LEN {
                    ThrumConfig::from_codec_private(width, height, bytes)
                        .map_err(|e| format!("bad thrum codec-private data: {e}"))?
                } else {
                    ThrumConfig {
                        width,
                        height,
                        format,
                        ..ThrumConfig::default()
                    }
                };
                ThrumDecoder::new(fw, cfg)
                    .map(|d| Box::new(d) as VideoDecoderBox)
                    .map_err(|e| format!("thrum decoder: {e}"))
            }
            VideoCodec::Vault => {
                let cfg = if bytes.len() >= VaultConfig::CODEC_PRIVATE_LEN {
                    VaultConfig::from_codec_private(width, height, bytes)
                        .map_err(|e| format!("bad vault codec-private data: {e}"))?
                } else {
                    VaultConfig {
                        width,
                        height,
                        format,
                        ..VaultConfig::default()
                    }
                };
                VaultDecoder::new(fw, cfg)
                    .map(|d| Box::new(d) as VideoDecoderBox)
                    .map_err(|e| format!("vault decoder: {e}"))
            }
            VideoCodec::Mote => {
                let cfg = if bytes.len() >= MoteConfig::CODEC_PRIVATE_LEN {
                    MoteConfig::from_codec_private(width, height, bytes)
                        .map_err(|e| format!("bad mote codec-private data: {e}"))?
                } else {
                    MoteConfig {
                        width,
                        height,
                        format,
                        ..MoteConfig::default()
                    }
                };
                MoteDecoder::new(fw, cfg)
                    .map(|d| Box::new(d) as VideoDecoderBox)
                    .map_err(|e| format!("mote decoder: {e}"))
            }
        }
    }
}

/// The message shown when a video codec is passed to the WAV-based
/// audio path (e.g. `encode --codec thrum` with a non-y4m input).
fn video_codec_unsupported_here(codec: VideoCodec) -> String {
    format!(
        "{} is a video codec and this path is audio-only (WAV in); \
         give encode/benchmark a y4m file with --codec {}, or decode a \
         .tpt that contains a video track",
        codec.name(),
        codec.name()
    )
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
/// Boxed video encoder.
///
/// Erased at `Frame = VideoFrame` rather than at `tpt_media_codec::DynFrame`
/// on purpose: `DynFrame` carries no PTS, and the video path has to keep the
/// y4m bridge's frame timestamps. See [`VideoCodec::new_encoder`].
type VideoEncoderBox = Box<dyn Encoder<Frame = VideoFrame>>;
/// Boxed video decoder. See [`VideoEncoderBox`] for why not `DynFrame`.
type VideoDecoderBox = Box<dyn Decoder<Frame = VideoFrame>>;

/// The unit of work shared by `tpt encode` and `tpt transcode`: mono S16
/// samples plus everything needed to mux them into a `.tpt` container.
struct EncodeJob {
    codec: AudioCodec,
    rate: u32,
    bits_per_coef: u8,
    band_step: Option<f32>,
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

/// Aura's lossless block: one LPC analysis and one packet per 4096 samples.
/// Measured on the benchmark corpus, 4096 beats 1024 by about 7% on music
/// (LPC side info is amortised, and the 16-tap predictor adapts less often).
const AURA_SAMPLES_PER_BLOCK: usize = 4096;

/// Samples per encode block per codec — Cinder's 512 = two 256-sample hops,
/// Aura's 4096, Reed's 20 ms frame (160/320 samples at 8/16 kHz, hence the
/// rate).
fn samples_per_block(codec: AudioCodec, rate: u32) -> usize {
    match codec {
        AudioCodec::Cinder => 2 * 256,
        AudioCodec::Aura => AURA_SAMPLES_PER_BLOCK,
        AudioCodec::Reed => rate as usize / 50,
    }
}

/// `--quality 0..=100` mapped onto Reed's per-coefficient LSF bit depth.
/// The floor is 4, not 2: the wire needs `2^lsf_bits > lpc_order` (10) for
/// a valid strictly-increasing index set to exist at all.
fn quality_to_lsf_bits(quality: Option<u8>) -> u8 {
    match quality {
        None => 5,
        Some(q) => 4 + (u16::from(q) * 4 / 100) as u8,
    }
}

/// Encode a job into a finished `.tpt` container; returns the container
/// bytes and the packet count.
fn run_encode_job(job: &EncodeJob) -> Result<(Vec<u8>, usize), String> {
    let samples_per_block = samples_per_block(job.codec, job.rate);
    let mut encoder = build_encoder(job.codec, job.rate, job.bits_per_coef, job.band_step)?;
    let track_timebase = Timebase::new(1, job.rate);
    let mut muxer = tpt_container::TptMuxer::new(track_timebase);
    let mut track = Track::new(0, MediaType::Audio, job.codec.codec_id(), track_timebase);
    track.codec_config.version = encoder.info().version;
    track.codec_config.data = match job.codec {
        AudioCodec::Cinder => vec![job.bits_per_coef],
        AudioCodec::Aura => (samples_per_block as u16).to_le_bytes().to_vec(),
        AudioCodec::Reed => ReedConfig {
            sample_rate: job.rate,
            lsf_bits: job.bits_per_coef,
            ..ReedConfig::default()
        }
        .to_codec_private(),
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
    // Codecs that hold back output (Cinder's overlap-add tail) release it
    // only at end-of-stream.
    let tail = encoder.drain().map_err(|e| format!("drain failed: {e}"))?;
    for pkt in tail {
        if let Err(e) = muxer.write_packet_raw(&pkt) {
            return Err(format!("mux failed: {e:?}"));
        }
        written += 1;
    }
    Ok((muxer.finish(), written))
}

/// Sample rates each codec accepts for `encode`/`benchmark` input
/// (mirroring the codecs' `supported_formats` tables; there is no CLI
/// resampler — feed the rate the codec speaks).
fn supported_input_rates(codec: AudioCodec) -> &'static [u32] {
    match codec {
        AudioCodec::Cinder | AudioCodec::Aura => &[44_100, 48_000],
        AudioCodec::Reed => &[8_000, 16_000],
    }
}

// ---------------------------------------------------------------------------
// Video (thrum): y4m in, `.tpt` out — the video counterpart of the audio job
// ---------------------------------------------------------------------------

/// `--quality 0..=100` mapped onto Thrum's quantiser index: higher
/// quality means a *lower* `qp`, and the full slider spans the codec's
/// legal `1..=51` (`51 - q/2`). Unspecified keeps Thrum's own default.
fn quality_to_qp(quality: Option<u8>) -> u8 {
    match quality {
        None => ThrumConfig::default().qp,
        Some(q) => (51 - (u16::from(q) * 50 / 100)).max(1) as u8,
    }
}

/// The unit of work for `tpt encode --codec thrum` and video
/// `tpt transcode`: y4m-parsed frames plus muxing parameters. Frames
/// carry their PTS already (the y4m bridge assigns frame `i` tick `i`
/// on the `fps_den/fps_num` timebase).
struct VideoEncodeJob {
    codec: VideoCodec,
    fps_num: u32,
    fps_den: u32,
    qp: u8,
    /// `--bitrate` / fps: the per-frame budget Vault's rate control
    /// encodes toward (`None` = the configured constant `qp`).
    bits_per_frame: Option<u32>,
    frames: Vec<VideoFrame>,
    extra_meta: Vec<(String, MetadataValue)>,
}

/// Encode a video job into a finished `.tpt`: a video `Track` carrying
/// the geometry in `TrackMedia` and the codec's private bytes, one packet
/// per frame.
fn run_video_encode_job(job: &VideoEncodeJob) -> Result<(Vec<u8>, usize), String> {
    let first = job.frames.first().ok_or("no frames to encode")?;
    let bits_per_frame = job.bits_per_frame;
    let (mut encoder, private) = job.codec.new_encoder(
        first.width,
        first.height,
        first.format,
        job.qp,
        bits_per_frame,
    )?;
    let version = encoder.info().version;
    let tb = Timebase::new(job.fps_den, job.fps_num);
    let mut muxer = tpt_container::TptMuxer::new(tb);
    let mut track = Track::new(0, MediaType::Video, job.codec.codec_id(), tb);
    track.codec_config.version = version;
    track.codec_config.data = private;
    track.media = TrackMedia {
        width: Some(first.width),
        height: Some(first.height),
        pixel_format: Some(first.format),
        ..TrackMedia::default()
    };
    track.metadata = job.extra_meta.clone();
    muxer.register_track(&track);

    let mut written = 0usize;
    for f in &job.frames {
        let packets = encoder
            .encode(f)
            .map_err(|e| format!("encode failed: {e}"))?;
        for pkt in packets {
            muxer
                .write_packet_raw(&pkt)
                .map_err(|e| format!("mux failed: {e:?}"))?;
            written += 1;
        }
    }
    // B-frame encoders hold a lookahead: the flush releases the trailing
    // frames the last group's B's were waiting on. Codecs without a
    // lookahead return nothing here.
    let tail = encoder.flush().map_err(|e| format!("flush failed: {e}"))?;
    for pkt in tail {
        muxer
            .write_packet_raw(&pkt)
            .map_err(|e| format!("mux failed: {e:?}"))?;
        written += 1;
    }
    Ok((muxer.finish(), written))
}

/// A fully decoded `.tpt` input: whichever track comes first and its
/// decoded frames. `tpt decode` and `tpt transcode` dispatch on this.
enum DemuxedMedia {
    Audio(Track, Vec<tpt_media_core::AudioFrame>),
    Video(Track, Vec<VideoFrame>),
}

/// Demux and fully decode a `.tpt` byte buffer's first track, audio or
/// video, with padding/metadata handling identical to the audio-only
/// [`demux_and_decode`].
fn demux_and_decode_any(
    input: &[u8],
    requested: Option<CodecId>,
) -> Result<DemuxedMedia, (i32, String)> {
    let mut demux = tpt_container::TptDemuxer::new(input)
        .map_err(|e| (1, format!("invalid tpt input: {e}")))?;
    let track = demux
        .tracks()
        .first()
        .cloned()
        .ok_or((1, "no tracks found".to_string()))?;
    let codec_id = track.codec_config.codec_id;
    if let Some(wanted) = requested {
        if wanted != codec_id {
            return Err((
                1,
                format!("file track uses {codec_id:?}, not the requested {wanted:?}"),
            ));
        }
    }
    match track.media_type {
        MediaType::Audio => {
            let codec = match AudioCodec::from_id(codec_id) {
                Some(c) => c,
                None => {
                    return Err((1, "no decoder registered for this codec".into()));
                }
            };
            let rate = track.media.sample_rate.unwrap_or(48_000);
            let mut decoder = build_decoder(codec, rate, &track).map_err(|e| (1, e))?;
            let mut frames: Vec<tpt_media_core::AudioFrame> = Vec::new();
            loop {
                match demux.next_packet() {
                    Ok(pkt) => frames.extend(
                        decoder
                            .decode(&pkt)
                            .map_err(|e| (1, format!("decode failed: {e}")))?,
                    ),
                    Err(tpt_media_container::ContainerError::Truncated) => break,
                    Err(e) => return Err((1, format!("malformed container: {e:?}"))),
                }
            }
            if frames.is_empty() {
                return Err((1, "no decodable packets found".into()));
            }
            trim_padding(&mut frames, &track);
            Ok(DemuxedMedia::Audio(track, frames))
        }
        MediaType::Video => {
            if let (Some(codec), true) = (
                VideoCodec::from_id(codec_id),
                AudioCodec::from_id(codec_id).is_none(),
            ) {
                let width = track
                    .media
                    .width
                    .ok_or((1, "video track is missing its width".to_string()))?;
                let height = track
                    .media
                    .height
                    .ok_or((1, "video track is missing its height".to_string()))?;
                let format = track.media.pixel_format.unwrap_or(PixelFormat::Yuv420);
                let mut decoder = codec
                    .new_decoder(
                        width,
                        height,
                        format,
                        &track.codec_config.data,
                        track.codec_config.version,
                    )
                    .map_err(|e| (1, e))?;
                let mut frames: Vec<VideoFrame> = Vec::new();
                loop {
                    match demux.next_packet() {
                        Ok(pkt) => frames.extend(
                            decoder
                                .decode(&pkt)
                                .map_err(|e| (1, format!("decode failed: {e}")))?,
                        ),
                        Err(tpt_media_container::ContainerError::Truncated) => break,
                        Err(e) => return Err((1, format!("malformed container: {e:?}"))),
                    }
                }
                if frames.is_empty() {
                    return Err((1, "no decodable packets found".into()));
                }
                Ok(DemuxedMedia::Video(track, frames))
            } else {
                Err((1, "no decoder registered for this video codec".into()))
            }
        }
        other => Err((1, format!("first track is {other:?}, which has no decoder"))),
    }
}

/// The Y4M stream parameters for a decoded video track: geometry and
/// pixel format from `TrackMedia`, the frame rate from the track
/// timebase (`fps_den/fps_num` — a frame plays at tick · num/den
/// seconds).
fn y4m_params_for_track(track: &Track, frames: &[VideoFrame]) -> Option<y4m::Y4mParams> {
    let first = frames.first()?;
    Some(y4m::Y4mParams {
        width: track.media.width.unwrap_or(first.width),
        height: track.media.height.unwrap_or(first.height),
        fps_num: track.timebase.den,
        fps_den: track.timebase.num,
        format: first.format,
    })
}

/// Encode y4m-parsed frames with `codec` (the `encode` video path).
///
/// Geometry is *not* validated here: each codec's own `validate()` already
/// rejects a geometry it cannot code, and its message names the real
/// constraint (Thrum and Mote want multiples of 16, Mote also caps the long
/// edge at 848 and the short at 480, Vault works in 8x8 units). Checking
/// here instead would mean keeping a second, drifting copy of three codecs'
/// rules in the CLI.
fn encode_video(spec: &OpSpec, input: &[u8], ctx: &Ctx<'_>, codec: VideoCodec) -> Outcome {
    let (params, frames) = match y4m::parse(input) {
        Ok(r) => r,
        Err(e) => return Outcome::fail(1, format!("input is not a readable y4m: {e}")),
    };
    // `--bitrate` / fps = the per-frame budget Vault's rate control
    // encodes toward; other video codecs ignore it.
    let bits_per_frame = spec
        .bitrate_bits_per_second
        .map(|b| b.div_ceil(u64::from(params.fps_num)) * u64::from(params.fps_den))
        .and_then(|v| u32::try_from(v).ok());
    let job = VideoEncodeJob {
        codec,
        fps_num: params.fps_num,
        fps_den: params.fps_den,
        qp: quality_to_qp(spec.quality),
        bits_per_frame,
        frames,
        extra_meta: spec
            .meta
            .iter()
            .map(|(k, v)| (k.clone(), MetadataValue::Text(v.clone())))
            .collect(),
    };
    let count = job.frames.len();
    let (bytes, written) = match run_video_encode_job(&job) {
        Ok(r) => r,
        Err(e) => return Outcome::fail(1, e),
    };
    write_output(
        spec,
        bytes,
        format!(
            "encoded {written} packets ({count} frames, {}x{} @ {}/{} fps)",
            params.width, params.height, params.fps_num, params.fps_den
        ),
        ctx,
    )
}

/// `tpt encode`: WAV or y4m -> codec -> `.tpt`.
///
/// Audio codecs read a 16-bit mono WAV at one of the codec's supported
/// rates (44.1/48 kHz for Cinder/Aura, 8/16 kHz for Reed), chunk it into
/// codec blocks, encode each block, and mux everything into a `.tpt`
/// container with full track metadata (track timebase `1/sample_rate`,
/// codec version, sample rate, channel layout, `--set-meta` tags).
/// `--quality` maps onto Cinder/Aura coefficient bit depth and Reed's
/// per-coefficient LSF bit depth; `--bitrate` is validated but not yet
/// applied (no codec here has rate control).
///
/// A YUV4MPEG2 input (`YUV4MPEG2` magic) takes the video path instead:
/// it requires a video codec (thrum) and encodes every frame as one
/// packet into a video track carrying the geometry, pixel format, and
/// frame rate. `--quality` maps onto Thrum's quantiser index.
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
    let input = match read_source(&spec.input, ctx) {
        Ok(b) => b,
        Err(e) => return Outcome::fail(1, e),
    };
    if input.starts_with(b"YUV4MPEG2") {
        if AudioCodec::from_id(codec).is_some() {
            let name = NATIVE_CODECS
                .iter()
                .find(|(_, id, _)| id.as_bytes() == codec.as_bytes())
                .map(|(n, _, _)| *n)
                .unwrap_or("this codec");
            return Outcome::fail(
                1,
                format!(
                    "{name} is an audio codec; y4m input needs a video codec \
                     (--codec thrum, vault or mote)"
                ),
            );
        }
        let video = match VideoCodec::from_id(codec) {
            Some(v) => v,
            None => return Outcome::fail(1, "no encoder registered for this codec"),
        };
        return encode_video(spec, &input, ctx, video);
    }
    let codec = match AudioCodec::from_id(codec) {
        Some(c) => c,
        None => match VideoCodec::from_id(codec) {
            Some(v) => return Outcome::fail(1, video_codec_unsupported_here(v)),
            None => return Outcome::fail(1, "no encoder registered for this codec"),
        },
    };
    let frame = match decode_wav(&input) {
        Ok(f) => f,
        Err(e) => return Outcome::fail(1, format!("input is not a readable wav: {e}")),
    };
    if frame.format != SampleFormat::S16
        || frame.channel_layout.channels() != 1
        || !supported_input_rates(codec).contains(&frame.sample_rate)
    {
        return Outcome::fail(
            1,
            format!(
                "encode requires 16-bit mono wav at {} Hz for this codec \
                 (got {:?}, {} ch, {} Hz)",
                supported_input_rates(codec)
                    .iter()
                    .map(|r| r.to_string())
                    .collect::<Vec<_>>()
                    .join(" or "),
                frame.format,
                frame.channel_layout.channels(),
                frame.sample_rate
            ),
        );
    }
    let job = EncodeJob {
        codec,
        rate: frame.sample_rate,
        bits_per_coef: quality_param(codec, spec.quality),
        band_step: spec.band_step,
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

/// `tpt decode`: `.tpt` -> codec -> WAV or y4m.
///
/// Opens any `.tpt` container and decodes its first track: an audio
/// track (the `--codec` argument, if given, must match) writes a 16-bit
/// WAV and requires `--container wav`; a video track writes YUV4MPEG2
/// and requires the default container. Non-`.tpt` inputs are rejected
/// up front.
pub fn decode_cmd(spec: &OpSpec, ctx: &Ctx<'_>) -> Outcome {
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
    match demux_and_decode_any(&input, spec.codec) {
        Ok(DemuxedMedia::Audio(_track, frames)) => {
            if spec.container != ContainerKind::Wav {
                return Outcome::fail(
                    1,
                    "decoding an audio track writes wav output (use --container wav)",
                );
            }
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
        Ok(DemuxedMedia::Video(track, frames)) => {
            if spec.container == ContainerKind::Wav {
                return Outcome::fail(
                    1,
                    "decoding a video track writes y4m output (drop --container wav)",
                );
            }
            let params = match y4m_params_for_track(&track, &frames) {
                Some(p) => p,
                None => return Outcome::fail(1, "video track decoded no frames"),
            };
            let bytes = y4m::write(&params, &frames);
            let count = frames.len();
            write_output(
                spec,
                bytes,
                format!(
                    "decoded {count} frames ({}x{} @ {}/{} fps)",
                    params.width, params.height, params.fps_num, params.fps_den
                ),
                ctx,
            )
        }
        Err((code, msg)) => Outcome::fail(code, msg),
    }
}

/// Drop block-size padding from the tail of the decoded frames, using the
/// exact sample count recorded by `tpt encode` (see
/// [`TRACK_SAMPLE_COUNT_KEY`]). Without that metadata the frames pass
/// through unchanged.
fn trim_padding(frames: &mut Vec<tpt_media_core::AudioFrame>, track: &tpt_media_core::Track) {
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
    // The excess can span several trailing frames (Cinder's hop-sized
    // frames are shorter than its padding), so drop whole frames from the
    // end and trim the last partial one.
    let mut excess = decoded - exact;
    while excess > 0 {
        let last = match frames.last_mut() {
            Some(l) => l,
            None => return,
        };
        let last_count = last.sample_count as usize;
        if excess >= last_count {
            frames.pop();
            excess -= last_count;
        } else {
            last.sample_count = (last_count - excess) as u32;
            last.samples.truncate(last.sample_count as usize * 2);
            excess = 0;
        }
    }
}

/// Map `--quality 0..=100` onto the codec's per-coefficient bit count —
/// Cinder/Aura coefficient bit depth, Reed LSF bit depth (default 8 and 5
/// bits respectively when unspecified).
fn quality_param(codec: AudioCodec, quality: Option<u8>) -> u8 {
    match codec {
        AudioCodec::Reed => quality_to_lsf_bits(quality),
        _ => quality_to_bits(quality),
    }
}

/// Map `--quality 0..=100` onto coefficient bit depth (Cinder/Aura support
/// a per-coefficient bit count; default 8 bits when unspecified).
fn quality_to_bits(quality: Option<u8>) -> u8 {
    match quality {
        None => 8,
        Some(q) => 4 + (u16::from(q) * 8 / 100) as u8,
    }
}

fn build_encoder(
    codec: AudioCodec,
    rate: u32,
    bits_per_coef: u8,
    band_step: Option<f32>,
) -> Result<EncoderBox, String> {
    let fw = FrameworkCodecConfig::default();
    if band_step.is_some() && codec != AudioCodec::Cinder {
        return Err("--band-step applies to cinder only".into());
    }
    match codec {
        AudioCodec::Cinder => Ok(Box::new(
            CinderEncoder::new(fw, {
                let mut cfg = CinderEncoderConfig {
                    sample_rate: rate,
                    bits_per_coef,
                    ..CinderEncoderConfig::default()
                };
                if let Some(s) = band_step {
                    cfg.band_steps = vec![s; cfg.n_bands as usize];
                }
                cfg
            })
            .map_err(|e| format!("cinder encoder: {e}"))?,
        ) as EncoderBox),
        AudioCodec::Aura => Ok(Box::new(
            AuraEncoder::new(
                fw,
                AuraConfig {
                    mode: AuraMode::Lossless,
                    sample_rate: rate,
                    block_size: AURA_SAMPLES_PER_BLOCK as u16,
                    ..AuraConfig::default()
                },
            )
            .map_err(|e| format!("aura encoder: {e}"))?,
        ) as EncoderBox),
        AudioCodec::Reed => Ok(Box::new(
            ReedEncoder::new(
                fw,
                ReedConfig {
                    sample_rate: rate,
                    lsf_bits: bits_per_coef,
                    ..ReedConfig::default()
                },
            )
            .map_err(|e| format!("reed encoder: {e}"))?,
        ) as EncoderBox),
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
        AudioCodec::Cinder => {
            let bits = private
                .first()
                .copied()
                .filter(|b| (4..=12).contains(b))
                .unwrap_or(8);
            Ok(Box::new(
                CinderDecoder::new(
                    fw,
                    CinderEncoderConfig {
                        sample_rate: rate,
                        bits_per_coef: bits,
                        ..CinderEncoderConfig::default()
                    },
                )
                .map_err(|e| format!("cinder decoder: {e}"))?,
            ) as DecoderBox)
        }
        AudioCodec::Aura => {
            let block_size = if private.len() >= 2 {
                u16::from_le_bytes([private[0], private[1]])
            } else {
                AURA_SAMPLES_PER_BLOCK as u16
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
        AudioCodec::Reed => {
            let cfg = ReedConfig::from_codec_private(rate, private)
                .map_err(|e| format!("reed decoder: {e}"))?;
            Ok(
                Box::new(ReedDecoder::new(fw, cfg).map_err(|e| format!("reed decoder: {e}"))?)
                    as DecoderBox,
            )
        }
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
/// Decodes the input container's first track — audio or video — then
/// re-encodes it with `--codec` and muxes a fresh `.tpt`. Audio targets
/// (cinder/aura/reed) require an audio track; a video track needs
/// `--codec thrum` (the only video codec), which re-quantises at
/// `--quality`. The source track's metadata tags are carried over (a
/// `--set-meta` pair overrides a tag of the same key), and audio
/// sample-count bookkeeping is refreshed so the output decodes back to
/// exactly the decoded sample count. Transcoding is always
/// lossy-with-respect-to-the-*source* when the target is Cinder (lossy
/// of lossy compounds); Aura targets re-mux the decoded samples
/// bit-exactly, and a thrum→thrum transcode re-quantises the decoded
/// frames.
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
    let target_audio = AudioCodec::from_id(target);
    let target_video = VideoCodec::from_id(target);
    if target_audio.is_none() && target_video.is_none() {
        return Outcome::fail(1, "no encoder registered for this codec");
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
            // A video-codec target with no .tpt at all reads as "video
            // codec addressed at an audio command" — say that instead
            // of the generic sniff failure.
            if let Some(v) = target_video {
                return Outcome::fail(
                    1,
                    format!(
                        "{} is a video codec; transcode needs a .tpt input \
                         containing a video track",
                        v.name()
                    ),
                );
            }
            return Outcome::fail(1, "input is not a recognisable media file");
        }
    }
    let decoded = match demux_and_decode_any(&input, None) {
        Ok(r) => r,
        Err((code, msg)) => return Outcome::fail(code, msg),
    };
    // Carry the source tags, minus any key the caller overrides with
    // `--set-meta` (later pairs win by construction).
    let carry_meta = |track: &Track| -> Vec<(String, MetadataValue)> {
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
        extra_meta
    };
    match decoded {
        DemuxedMedia::Audio(track, frames) => {
            let target = match target_audio {
                Some(c) => c,
                None => {
                    return Outcome::fail(
                        1,
                        format!(
                            "cannot transcode an audio track to {} (a video codec)",
                            target_video.expect("checked above").name()
                        ),
                    );
                }
            };
            let source = match AudioCodec::from_id(track.codec_config.codec_id) {
                Some(c) => c,
                None => AudioCodec::Reed, // unknown source: name is display-only
            };
            let rate = track.media.sample_rate.unwrap_or(48_000);
            if !supported_input_rates(target).contains(&rate) {
                return Outcome::fail(
                    1,
                    format!(
                        "transcode to {} requires a {} Hz source track (got {rate} Hz); \
                         there is no CLI resampler",
                        target.name(),
                        supported_input_rates(target)
                            .iter()
                            .map(|r| r.to_string())
                            .collect::<Vec<_>>()
                            .join(" or ")
                    ),
                );
            }
            let samples: Vec<u8> = frames
                .iter()
                .flat_map(|f| f.samples.iter().copied())
                .collect();
            let sample_count: usize = frames.iter().map(|f| f.sample_count as usize).sum();
            let job = EncodeJob {
                codec: target,
                rate,
                bits_per_coef: quality_param(target, spec.quality),
                band_step: spec.band_step,
                samples,
                sample_count,
                extra_meta: carry_meta(&track),
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
        DemuxedMedia::Video(track, frames) => {
            let target = match target_video {
                Some(c) => c,
                None => {
                    return Outcome::fail(
                        1,
                        format!(
                            "cannot transcode a video track to {} (an audio codec); \
                             a video track needs --codec thrum",
                            target_audio.expect("checked above").name()
                        ),
                    );
                }
            };
            let params = match y4m_params_for_track(&track, &frames) {
                Some(p) => p,
                None => return Outcome::fail(1, "video track decoded no frames"),
            };
            let count = frames.len();
            // Name the source codec from the track rather than assuming
            // thrum: a transcoded input may itself be VAUL or MOTE.
            let source = match VideoCodec::from_id(track.codec_config.codec_id) {
                Some(c) => c.name().to_string(),
                None => format!("{:?}", track.codec_config.codec_id),
            };
            let bits_per_frame = spec
                .bitrate_bits_per_second
                .map(|b| b.div_ceil(u64::from(params.fps_num)) * u64::from(params.fps_den))
                .and_then(|v| u32::try_from(v).ok());
            let job = VideoEncodeJob {
                codec: target,
                fps_num: params.fps_num,
                fps_den: params.fps_den,
                qp: quality_to_qp(spec.quality),
                bits_per_frame,
                frames,
                extra_meta: carry_meta(&track),
            };
            let (bytes, written) = match run_video_encode_job(&job) {
                Ok(r) => r,
                Err(e) => return Outcome::fail(1, e),
            };
            write_output(
                spec,
                bytes,
                format!(
                    "transcoded {written} packets ({count} frames, {}x{}, \
                     {source} -> {} at --quality {})",
                    params.width,
                    params.height,
                    target.name(),
                    spec.quality.map_or("-".into(), |q| q.to_string())
                ),
                ctx,
            )
        }
    }
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
    packets.extend(encoder.drain().map_err(|e| format!("drain failed: {e}"))?);
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
/// The video counterpart of the audio benchmark: y4m in, Thrum
/// round-tripped `--runs` times after an untimed warm-up, with the §37
/// video metrics (PSNR, SSIM, bits-per-pixel, bitrate) computed on the
/// final pass and per-frame latency reported against the clip's own
/// frame rate. One P-frame's encode+decode is the one-way latency
/// contribution — Thrum never holds a frame back.
fn benchmark_video(spec: &OpSpec, input: &[u8], codec: VideoCodec) -> Outcome {
    let (params, frames) = match y4m::parse(input) {
        Ok(r) => r,
        Err(e) => return Outcome::fail(1, format!("input is not a readable y4m: {e}")),
    };
    let qp = quality_to_qp(spec.quality);
    let (mut encoder, private) =
        match codec.new_encoder(params.width, params.height, params.format, qp, None) {
            Ok(v) => v,
            Err(e) => return Outcome::fail(1, e),
        };
    let version = encoder.info().version;
    let mut decoder = match codec.new_decoder(
        params.width,
        params.height,
        params.format,
        &private,
        version,
    ) {
        Ok(d) => d,
        Err(e) => return Outcome::fail(1, e),
    };
    let runs = spec.runs.unwrap_or(5);

    // Pass 0 is an untimed warm-up (plan construction, page-ins); the
    // measured passes reuse the same encoder/decoder, so the numbers
    // are steady-state exactly like the audio benchmark's.
    let mut encode_ms = Vec::new();
    let mut decode_ms = Vec::new();
    let mut last_pass: Option<(Vec<tpt_media_core::Packet>, Vec<VideoFrame>)> = None;
    for run in 0..=runs {
        let t0 = std::time::Instant::now();
        let mut packets = Vec::new();
        for f in &frames {
            match encoder.encode(f) {
                Ok(p) => packets.extend(p),
                Err(e) => return Outcome::fail(1, format!("encode failed: {e}")),
            }
        }
        let e = t0.elapsed().as_secs_f64() * 1e3;
        let t1 = std::time::Instant::now();
        let mut decoded = Vec::new();
        for pkt in &packets {
            match decoder.decode(pkt) {
                Ok(f) => decoded.extend(f),
                Err(e) => return Outcome::fail(1, format!("decode failed: {e}")),
            }
        }
        let d = t1.elapsed().as_secs_f64() * 1e3;
        if run > 0 {
            encode_ms.push(e);
            decode_ms.push(d);
            last_pass = Some((packets, decoded));
        }
    }
    let (packets, decoded) =
        last_pass.expect("at least one measured run is guaranteed by --runs >= 1");
    let packet_count = packets.len();
    let payload_bytes: u64 = packets.iter().map(|p| p.data.len() as u64).sum();
    let count = frames.len();
    let pixels = u64::from(params.width) * u64::from(params.height);

    // §37 video quality on the final pass: PSNR/SSIM averaged over the
    // frames that have a finite score (a bit-exact frame on the skip
    // path contributes none, exactly as the corpus tests define it).
    let mut psnr_sum = 0f64;
    let mut psnr_n = 0usize;
    let mut ssim_sum = 0f64;
    let mut ssim_n = 0usize;
    for (src, out) in frames.iter().zip(&decoded) {
        if let Some(p) = crate::quality::psnr_db(src, out).filter(|p| p.is_finite()) {
            psnr_sum += p;
            psnr_n += 1;
        }
        if let Some(s) = crate::quality::ssim(src, out) {
            ssim_sum += s;
            ssim_n += 1;
        }
    }
    let psnr = (psnr_n > 0).then(|| psnr_sum / psnr_n as f64);
    let ssim = (ssim_n > 0).then(|| ssim_sum / ssim_n as f64);

    let enc_med = median(&encode_ms).unwrap_or_default();
    let enc_min = encode_ms.iter().cloned().fold(f64::INFINITY, f64::min);
    let dec_med = median(&decode_ms).unwrap_or_default();
    let dec_min = decode_ms.iter().cloned().fold(f64::INFINITY, f64::min);
    let fps = params.fps_num as f64 / params.fps_den as f64;
    let frame_ms = 1e3 / fps;
    let duration_s = count as f64 / fps;
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
    let bpp = crate::quality::bits_per_pixel(payload_bytes, count as u64, pixels);
    let bitrate = crate::quality::video_bitrate_bits_per_second_rational(
        payload_bytes,
        count as u64,
        params.fps_num,
        params.fps_den,
    );

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
        j.field("qp");
        j.value_u64(u64::from(qp));
        j.field("width");
        j.value_u64(u64::from(params.width));
        j.field("height");
        j.value_u64(u64::from(params.height));
        j.field("fps");
        j.value_f64(fps);
        j.field("frames");
        j.value_u64(count as u64);
        j.field("duration_seconds");
        j.value_f64(duration_s);
        j.field("packets");
        j.value_u64(packet_count as u64);
        j.field("runs");
        j.value_u64(runs as u64);
        j.field("payload_bytes");
        j.value_u64(payload_bytes);
        if let Some(b) = bpp {
            j.field("bits_per_pixel");
            j.value_f64(b);
        }
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
        if let Some(p) = psnr {
            j.field("psnr_db");
            j.value_f64(p);
        }
        if let Some(s) = ssim {
            j.field("ssim");
            j.value_f64(s);
        }
        j.close();
        Outcome::ok(j.finish())
    } else {
        let mut out = String::new();
        out.push_str(&format!(
            "benchmark: {}, {}x{} @ {:.3} fps, {count} frames ({duration_s:.3} s), \
             {packet_count} packets, qp {qp}\n",
            codec.name(),
            params.width,
            params.height,
            fps,
        ));
        out.push_str(&format!(
            "encode:   median {enc_med:.3} ms, min {enc_min:.3} ms  ({enc_factor:.0}x realtime)\n"
        ));
        out.push_str(&format!(
            "decode:   median {dec_med:.3} ms, min {dec_min:.3} ms  ({dec_factor:.0}x realtime)\n"
        ));
        out.push_str(&format!(
            "latency:  {:.3} ms per-frame budget at this fps; one P-frame's \
             encode+decode min {:.3} ms\n",
            frame_ms,
            enc_min + dec_min
        ));
        out.push_str(&format!(
            "payload:  {payload_bytes} bytes, {} bits/pixel, {}\n",
            bpp.map_or("n/a".into(), |b| format!("{b:.4}")),
            bitrate.map_or("n/a".into(), |b| format!("{b} bit/s"))
        ));
        out.push_str(&format!(
            "quality:  {}\n",
            psnr.map_or("PSNR n/a (all frames bit-exact)".into(), |p| format!(
                "PSNR {p:.2} dB"
            ))
        ));
        if let Some(s) = ssim {
            out.push_str(&format!("          SSIM {s:.4}\n"));
        }
        Outcome::ok(out)
    }
}

pub fn benchmark_cmd(spec: &OpSpec, ctx: &Ctx<'_>) -> Outcome {
    let codec = match spec.codec {
        Some(c) => c,
        None => return Outcome::usage("benchmark requires --codec"),
    };
    let input = match read_source(&spec.input, ctx) {
        Ok(b) => b,
        Err(e) => return Outcome::fail(1, e),
    };
    if input.starts_with(b"YUV4MPEG2") {
        match VideoCodec::from_id(codec) {
            Some(v) => return benchmark_video(spec, &input, v),
            None => {
                let name = NATIVE_CODECS
                    .iter()
                    .find(|(_, id, _)| id.as_bytes() == codec.as_bytes())
                    .map(|(n, _, _)| *n)
                    .unwrap_or("this codec");
                return Outcome::fail(
                    1,
                    format!(
                        "{name} is an audio codec; y4m benchmarking needs \
                         --codec thrum, vault or mote"
                    ),
                );
            }
        }
    }
    let codec = match AudioCodec::from_id(codec) {
        Some(c) => c,
        None => match VideoCodec::from_id(codec) {
            Some(v) => return Outcome::fail(1, video_codec_unsupported_here(v)),
            None => return Outcome::fail(1, "no encoder registered for this codec"),
        },
    };
    let frame = match decode_wav(&input) {
        Ok(f) => f,
        Err(e) => return Outcome::fail(1, format!("input is not a readable wav: {e}")),
    };
    if frame.format != SampleFormat::S16
        || frame.channel_layout.channels() != 1
        || !supported_input_rates(codec).contains(&frame.sample_rate)
    {
        return Outcome::fail(
            1,
            format!(
                "benchmark requires 16-bit mono wav at {} Hz for this codec \
                 (got {:?}, {} ch, {} Hz)",
                supported_input_rates(codec)
                    .iter()
                    .map(|r| r.to_string())
                    .collect::<Vec<_>>()
                    .join(" or "),
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
    let bits_per_coef = quality_param(codec, spec.quality);
    let runs = spec.runs.unwrap_or(5);
    let block = samples_per_block(codec, rate);
    let mut encoder = match build_encoder(codec, rate, bits_per_coef, spec.band_step) {
        Ok(e) => e,
        Err(e) => return Outcome::fail(1, e),
    };
    let private = match codec {
        AudioCodec::Cinder => vec![bits_per_coef],
        AudioCodec::Aura => (block as u16).to_le_bytes().to_vec(),
        AudioCodec::Reed => ReedConfig {
            sample_rate: rate,
            lsf_bits: bits_per_coef,
            ..ReedConfig::default()
        }
        .to_codec_private(),
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
    let version = encoder.info().version;
    for run in 0..=runs {
        // Overlap-add codecs (Cinder v2) carry state across a pass, so a
        // reused pair would start every measured pass mid-stream and report
        // garbage quality. The warm-up pass keeps the pair built above;
        // each measured pass gets a fresh one.
        if run > 0 {
            encoder = match build_encoder(codec, rate, bits_per_coef, spec.band_step) {
                Ok(e) => e,
                Err(e) => return Outcome::fail(1, e),
            };
            decoder = match make_decoder(codec, rate, version, &private) {
                Ok(d) => d,
                Err(e) => return Outcome::fail(1, e),
            };
        }
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

/// `tpt video-corpus`: run a video codec over the synthetic corpus of
/// todo.md §36 and report the §37 metrics for every scene.
///
/// This is deliberately *self-contained*: it generates its own content
/// and never touches a file, because raw-video I/O (todo.md §43) is not
/// built yet. Everything it prints comes from a real encode/decode round
/// trip through the codec, not an estimate.
///
/// Output is a table by default and JSON with `--json`, matching `probe`.
/// Bitrate is reported next to quality on purpose: a PSNR with no bitrate
/// hides the cost that bought it.
pub fn video_corpus_cmd(json: bool, codec_id: CodecId, fps: u32, frames: usize) -> Outcome {
    use crate::quality::{psnr_db, ssim, Timing, VideoReport};
    use crate::video::corpus;
    use tpt_media_core::PixelFormat;

    let codec = match VideoCodec::from_id(codec_id) {
        Some(c) => c,
        None => {
            return Outcome::fail(
                1,
                format!(
                    "{codec_id} is not a video codec; video-corpus measures thrum, vault or mote"
                ),
            )
        }
    };

    // QCIF: the smallest resolution at which the macroblock structure and
    // the motion search are meaningfully exercised, and small enough
    // that a full corpus sweep stays interactive. 176x144 is a multiple of
    // 16 and inside every codec's limits, so one geometry serves all three.
    let (w, h) = (176u32, 144u32);
    let format = PixelFormat::Yuv420;

    let mut rows: Vec<(&'static str, VideoReport)> = Vec::new();
    for scene in corpus(w, h, format, frames, fps, 0x5EED_A11A) {
        let (mut enc, private) = match codec.new_encoder(w, h, format, 30, None) {
            Ok(v) => v,
            Err(e) => return Outcome::fail(1, e),
        };
        let version = enc.info().version;
        let mut dec = match codec.new_decoder(w, h, format, &private, version) {
            Ok(d) => d,
            Err(e) => return Outcome::fail(1, e),
        };

        // Index the source frames by PTS so a decoded picture is compared
        // against the frame it actually came from. Zipping packets against
        // frames in order would assume one packet per frame, which Mote
        // breaks: it prefixes the first frame with a dictionary packet that
        // decodes to nothing, shifting every later frame by one.
        let by_pts: std::collections::HashMap<i64, &VideoFrame> = scene
            .frames
            .iter()
            .map(|f| (f.frame.pts.ticks, f))
            .collect();

        let t0 = std::time::Instant::now();
        let mut packets = Vec::new();
        for f in &scene.frames {
            match enc.encode(f) {
                Ok(p) => packets.extend(p),
                Err(e) => return Outcome::fail(1, format!("{}: encode failed: {e}", scene.name)),
            }
        }
        let encode_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let t1 = std::time::Instant::now();
        let (mut psnr_sum, mut psnr_n) = (0f64, 0usize);
        let (mut ssim_sum, mut ssim_n) = (0f64, 0usize);
        // Decoded *pictures*, not packets: Mote sends a dictionary packet
        // that carries none, so `packets.len()` would be one too high and
        // would understate the rate and bpp derived from it.
        let mut decoded = 0usize;
        for pkt in &packets {
            let out = match dec.decode(pkt) {
                Ok(o) => o,
                Err(e) => return Outcome::fail(1, format!("{}: decode failed: {e}", scene.name)),
            };
            for got in &out {
                // A packet carrying no picture (Mote's dictionary packet)
                // has nothing to score.
                let Some(src) = by_pts.get(&got.frame.pts.ticks) else {
                    continue;
                };
                decoded += 1;
                // A bit-exact frame has no finite PSNR, so it is left out
                // of the mean rather than poisoning it with infinity.
                if let Some(p) = psnr_db(src, got) {
                    if p.is_finite() {
                        psnr_sum += p;
                        psnr_n += 1;
                    }
                }
                if let Some(s) = ssim(src, got) {
                    ssim_sum += s;
                    ssim_n += 1;
                }
            }
        }
        let decode_ms = t1.elapsed().as_secs_f64() * 1000.0;

        rows.push((
            scene.name,
            VideoReport {
                frames: decoded,
                encoded_bytes: packets.iter().map(|p| p.data.len() as u64).sum(),
                psnr_db: (psnr_n > 0).then(|| psnr_sum / psnr_n as f64),
                ssim: (ssim_n > 0).then(|| ssim_sum / ssim_n as f64),
                encode: Timing {
                    elapsed_ms: encode_ms,
                    frames: scene.frames.len(),
                },
                decode: Timing {
                    elapsed_ms: decode_ms,
                    frames: scene.frames.len(),
                },
                memory: None,
            },
        ));
    }
    let pixels = u64::from(w) * u64::from(h);
    if json {
        render_corpus_json(&rows, pixels, w, h, fps, codec)
    } else {
        render_corpus_table(&rows, pixels, w, h, fps, codec)
    }
}

/// Render `video-corpus` as JSON, one object per scene.
fn render_corpus_json(
    rows: &[(&'static str, crate::quality::VideoReport)],
    pixels: u64,
    w: u32,
    h: u32,
    fps: u32,
    codec: VideoCodec,
) -> Outcome {
    use crate::json::J;
    let mut j = J::new();
    j.object();
    j.field("codec");
    j.value_str(codec.name());
    j.field("width");
    j.value_u64(u64::from(w));
    j.field("height");
    j.value_u64(u64::from(h));
    j.field("fps");
    j.value_u64(u64::from(fps));
    j.field("scenes");
    j.arr();
    for (name, r) in rows {
        j.object();
        j.field("name");
        j.value_str(name);
        j.field("frames");
        j.value_u64(r.frames as u64);
        j.field("encoded_bytes");
        j.value_u64(r.encoded_bytes);
        j.field("bits_per_pixel");
        j.value_f64(r.bpp(pixels).unwrap_or(f64::NAN));
        j.field("bitrate_bps");
        j.value_u64(r.bitrate_bps(fps).unwrap_or(0));
        j.field("psnr_db");
        j.value_f64(r.psnr_db.unwrap_or(f64::INFINITY));
        j.field("ssim");
        j.value_f64(r.ssim.unwrap_or(f64::NAN));
        j.field("encode_ms_per_frame");
        j.value_f64(r.encode.ms_per_frame().unwrap_or(f64::NAN));
        j.field("decode_ms_per_frame");
        j.value_f64(r.decode.ms_per_frame().unwrap_or(f64::NAN));
        j.field("latency_ms");
        j.value_f64(r.latency_ms().unwrap_or(f64::NAN));
        j.field("fits_frame_budget");
        j.value_bool(r.fits_frame_budget(fps).unwrap_or(false));
        j.close();
    }
    j.close();
    j.close();
    Outcome::ok(j.finish())
}

/// Render the human-readable `video-corpus` table. Split out of
/// [`video_corpus_cmd`] so the JSON branch stays readable.
fn render_corpus_table(
    rows: &[(&'static str, crate::quality::VideoReport)],
    pixels: u64,
    w: u32,
    h: u32,
    fps: u32,
    codec: VideoCodec,
) -> Outcome {
    use crate::quality::Timing;
    let mut out = String::new();
    out.push_str(&format!(
        "video corpus: {} {w}x{h} 4:2:0, {fps} fps, {} frames/scene\n\n",
        codec.name(),
        rows.first().map_or(0, |(_, r)| r.frames)
    ));
    out.push_str(&format!(
        "{:<22} {:>8} {:>8} {:>8} {:>9} {:>7} {:>8} {:>8}\n",
        "scene", "PSNR dB", "SSIM", "bytes", "bit/s", "bpp", "enc ms", "dec ms"
    ));
    for (name, r) in rows {
        out.push_str(&format!(
            "{:<22} {:>8} {:>8} {:>8} {:>9} {:>7} {:>8} {:>8}\n",
            name,
            r.psnr_db
                .map_or_else(|| "exact".into(), |p| format!("{p:.2}")),
            r.ssim.map_or("n/a".into(), |s| format!("{s:.4}")),
            r.encoded_bytes,
            r.bitrate_bps(fps).unwrap_or(0),
            r.bpp(pixels).map_or("n/a".into(), |b| format!("{b:.3}")),
            r.encode
                .ms_per_frame()
                .map_or("n/a".into(), |e| format!("{e:.1}")),
            r.decode
                .ms_per_frame()
                .map_or("n/a".into(), |e| format!("{e:.1}")),
        ));
    }
    let budget = Timing::frame_budget_ms(fps).unwrap_or(0.0);
    out.push_str(&format!(
        "\nframe budget at {fps} fps: {budget:.1} ms per frame (encode + decode must fit inside it)\n"
    ));
    Outcome::ok(out)
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
        Command::VideoCorpus {
            json,
            codec,
            fps,
            frames,
        } => video_corpus_cmd(*json, *codec, *fps, *frames),
        Command::Help => Outcome::ok(crate::args::USAGE.into()),
    }
}
