//! Command execution for `tpt`.

use tpt_media_aura::{AuraConfig, AuraDecoder, AuraEncoder, AuraMode};
use tpt_media_cinder::{CinderDecoder, CinderEncoder, CinderEncoderConfig};
use tpt_media_codec::{CodecConfig as FrameworkCodecConfig, Decoder, Encoder, ResourceLimits};
use tpt_media_container::Container;
use tpt_media_core::{
    ChannelLayout, CodecId, MediaType, MetadataValue, PixelFormat, SampleFormat, Timebase,
    Timestamp, Track, TrackMedia, VideoFrame,
};
use tpt_media_io::{decode_wav, write_wav};
use tpt_media_reed::{ReedConfig, ReedDecoder, ReedEncoder};

use crate::audio_input;
use tpt_media_mote::{MoteConfig, MoteDecoder, MoteEncoder};
use tpt_media_probe::{sniff, ContainerKind, ProbeReport};
use tpt_media_thrum::{ThrumConfig, ThrumDecoder, ThrumEncoder};
use tpt_media_vault::{VaultConfig, VaultDecoder, VaultEncoder};

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
    use tpt_media_aura::{AuraDecoderFactory, AuraEncoderFactory};
    use tpt_media_cinder::{CinderDecoderFactory, CinderEncoderFactory};
    use tpt_media_reed::{ReedDecoderFactory, ReedEncoderFactory};
    use tpt_media_thrum::{ThrumDecoderFactory, ThrumEncoderFactory};

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
        tpt_media_vault::VaultDecoderFactory::new(tpt_media_vault::VaultConfig::default()),
        tpt_media_vault::VaultEncoderFactory::new(tpt_media_vault::VaultConfig::default()),
    ) {
        r.register_decoder(dec);
        r.register_encoder(enc);
    }
    // Mote: same placeholder-geometry rule as Thrum, and the same split
    // between the probe registry here and the y4m bridge in
    // `VideoCodec::new_encoder`/`new_decoder`.
    if let (Ok(dec), Ok(enc)) = (
        tpt_media_mote::MoteDecoderFactory::new(tpt_media_mote::MoteConfig::default()),
        tpt_media_mote::MoteEncoderFactory::new(tpt_media_mote::MoteConfig::default()),
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
        qp: Option<u8>,
        bits_per_frame: Option<u32>,
    ) -> Result<(VideoEncoderBox, Vec<u8>), String> {
        let fw = FrameworkCodecConfig::default();
        match self {
            VideoCodec::Thrum => {
                let cfg = ThrumConfig {
                    width,
                    height,
                    format,
                    qp: qp.unwrap_or(ThrumConfig::default().qp),
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
                    qp: qp.unwrap_or(VaultConfig::default().qp),
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
                // The dictionary is Mote's only rate-quality dial, and it is
                // encoder-only: the decoder reads whatever size the stream
                // carries. `None` keeps the codec's default dictionary.
                let cfg = MoteConfig {
                    width,
                    height,
                    format,
                    dictionary_entries: qp.map_or(
                        MoteConfig::default().dictionary_entries,
                        mote_dictionary_entries,
                    ),
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
    /// Interleaved channels in `samples`: 1, or 2 for Aura.
    channels: usize,
    /// S16 LE interleaved samples; the final partial block is zero-padded by
    /// [`run_encode_job`].
    samples: Vec<u8>,
    /// The original (pre-padding) sample count recorded in track metadata
    /// so `tpt decode` can trim the padding.
    sample_count: usize,
    /// Caller-supplied metadata pairs (`--set-meta`, or the source track's
    /// tags when transcoding), written ahead of the built-in sample count.
    extra_meta: Vec<(String, MetadataValue)>,
}

/// Aura's lossless block: one LPC analysis and one packet per 8192 samples
/// (about 170 ms at 48 kHz). Measured on the benchmark corpus, 8192 is no
/// worse than 4096 on any clip, and beats 1024 by about 7% on music: the LPC
/// side information is amortised over more samples.
const AURA_SAMPLES_PER_BLOCK: usize = 8192;

/// The frame layout for `channels` interleaved S16 channels.
fn layout_for(channels: usize) -> ChannelLayout {
    if channels == 2 {
        ChannelLayout::Stereo
    } else {
        ChannelLayout::Mono
    }
}

/// Whether `codec` accepts a WAV with `channels` channels: Aura takes mono or
/// stereo, the other audio codecs mono only.
fn audio_channels_ok(codec: AudioCodec, channels: u8) -> bool {
    match codec {
        AudioCodec::Aura => channels == 1 || channels == 2,
        _ => channels == 1,
    }
}

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
    let mut muxer = tpt_media_format::TptMuxer::new(track_timebase);
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
        channel_layout: Some(layout_for(job.channels)),
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

    let total = job.samples.len() / (2 * job.channels);
    let mut written = 0usize;
    let mut block_index = 0usize;
    while block_index * samples_per_block < total {
        let start = block_index * samples_per_block;
        let end = (start + samples_per_block).min(total);
        let mut block = vec![0u8; samples_per_block * 2 * job.channels];
        let (bs, be) = (start * 2 * job.channels, end * 2 * job.channels);
        block[..be - bs].copy_from_slice(&job.samples[bs..be]);
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
            channel_layout: layout_for(job.channels),
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
/// Mote's dictionary size for a quantiser index: log-spaced from 64 entries
/// at qp 51 (the coarsest, quality 0) to 4096 at qp 1 (the finest). Measured
/// on `akiyo_cif_100f`: 64 entries give 28.5 dB at 15 kB, 256 give 30.0 dB at
/// 34 kB, and 1024 give 31.1 dB at 60 kB.
fn mote_dictionary_entries(qp: u8) -> u16 {
    let t = f64::from(51 - qp.clamp(1, 51)) / 50.0;
    (64.0 * 64f64.powf(t)).round().clamp(64.0, 4096.0) as u16
}

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
    /// `None` for the codec default.
    qp: Option<u8>,
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
    let mut muxer = tpt_media_format::TptMuxer::new(tb);
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
    let mut demux = tpt_media_format::TptDemuxer::new(input)
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
    if spec.target_snr.is_some() {
        return Outcome::fail(
            1,
            "--target-snr applies to audio input; use --target-psnr for video",
        );
    }
    if spec.target_psnr.is_some() && spec.bitrate_bits_per_second.is_some() {
        return Outcome::usage("pick one of --bitrate, --target-psnr");
    }
    let mut qp = spec.quality.map(|q| quality_to_qp(Some(q)));
    let mut fit_note = String::new();
    if let Some(target) = spec.target_psnr {
        match fit_video_qp(codec, &params, &frames, target) {
            Ok((q, psnr)) => {
                qp = Some(q);
                fit_note = format!(", --target-psnr {target} dB met at qp {q} ({psnr:.2} dB)");
            }
            Err(e) => return Outcome::fail(1, e),
        }
    }
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
        qp,
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
            "encoded {written} packets ({count} frames, {}x{} @ {}/{} fps){fit_note}",
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
/// per-coefficient LSF bit depth; `--bitrate` searches that same depth for
/// the best lossy fit under the target (Aura refuses it).
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
    let (frame, notes) = match audio_input::prepare(frame, codec == AudioCodec::Aura, spec.resample)
    {
        Ok(p) => p,
        Err(e) => return Outcome::fail(1, e),
    };
    if frame.format != SampleFormat::S16
        || !audio_channels_ok(codec, frame.channel_layout.channels())
        || !supported_input_rates(codec).contains(&frame.sample_rate)
    {
        return Outcome::fail(
            1,
            format!(
                "encode requires a wav at {} Hz \
                 for this codec (got {:?}, {} ch, {} Hz); --resample converts the rate",
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
        band_step: band_step_for(spec, codec),
        channels: frame.channel_layout.channels() as usize,
        samples: frame.samples,
        sample_count: frame.sample_count as usize,
        extra_meta: spec
            .meta
            .iter()
            .map(|(k, v)| (k.clone(), MetadataValue::Text(v.clone())))
            .collect(),
    };
    let mut job = job;
    let prep = prep_suffix(&notes);
    let fit_note = match audio_goal(spec) {
        Ok(Some(goal)) => match fit_audio(&mut job, goal) {
            Ok(note) => note,
            Err(e) => return e,
        },
        Ok(None) => String::new(),
        Err(e) => return e,
    };
    let sample_count = job.sample_count;
    let (bytes, written) = match run_encode_job(&job) {
        Ok(r) => r,
        Err(e) => return Outcome::fail(1, e),
    };
    write_output(
        spec,
        bytes,
        format!("encoded {written} packets ({sample_count} samples){prep}{fit_note}"),
        ctx,
    )
}

// ---------------------------------------------------------------------------
// Quality-targeted encoding: `--bitrate`, `--target-snr`, `--target-psnr`
// ---------------------------------------------------------------------------

/// Cinder's band-step range. The top is just under 64 because the wire
/// stores `step * 1024` in a `u16`, so 65535/1024 is the largest step.
const STEP_MIN: f32 = 1.0 / 1024.0;
const STEP_MAX: f32 = 63.99;
/// The finest step the quality dial uses. Finer steps decode correctly since
/// Cinder version 6 (before it, the level clamp collapsed SNR below about
/// 0.25), but they produce very large files, so the dial stops here.
const QUALITY_FINEST_STEP: f32 = 0.0625;

/// What a quality-targeted audio encode must meet.
#[derive(Clone, Copy, Debug, PartialEq)]
enum AudioGoal {
    /// Average container bits/s at or under this target (`--bitrate`).
    Bitrate(u64),
    /// Decoded SNR at or above this many dB (`--target-snr`).
    Snr(f64),
}

/// The audio goal the options ask for, if any. The quality-target options
/// are mutually exclusive, and `--target-psnr` belongs to video input.
fn audio_goal(spec: &OpSpec) -> Result<Option<AudioGoal>, Outcome> {
    let picked = [
        spec.bitrate_bits_per_second.is_some(),
        spec.target_snr.is_some(),
        spec.target_psnr.is_some(),
    ];
    if picked.iter().filter(|p| **p).count() > 1 {
        return Err(Outcome::usage(
            "pick one of --bitrate, --target-snr, --target-psnr",
        ));
    }
    if spec.target_psnr.is_some() {
        return Err(Outcome::usage(
            "--target-psnr applies to video input; use --target-snr for audio",
        ));
    }
    Ok(match (spec.bitrate_bits_per_second, spec.target_snr) {
        (Some(b), _) => Some(AudioGoal::Bitrate(b)),
        (_, Some(s)) => Some(AudioGoal::Snr(s)),
        _ => None,
    })
}

fn goal_label(goal: AudioGoal) -> String {
    match goal {
        AudioGoal::Bitrate(t) => format!("--bitrate {t}"),
        AudioGoal::Snr(t) => format!("--target-snr {t} dB"),
    }
}

fn describe(value: f64, goal: AudioGoal) -> String {
    match goal {
        AudioGoal::Bitrate(_) => format!("{value:.0} bit/s"),
        AudioGoal::Snr(_) => format!("{value:.2} dB"),
    }
}

fn meets(value: f64, goal: AudioGoal) -> bool {
    match goal {
        AudioGoal::Bitrate(t) => value <= t as f64,
        AudioGoal::Snr(t) => value >= t,
    }
}

/// Average container bits/s of `job` at its current settings.
fn container_bitrate(job: &EncodeJob) -> Result<f64, String> {
    let total = (job.samples.len() / (2 * job.channels)) as u64;
    let (bytes, _) = run_encode_job(job)?;
    Ok((bytes.len() as u64 * 8 * u64::from(job.rate)).div_ceil(total) as f64)
}

/// Decoded SNR (dB) of `job` at its current settings: encode, decode with
/// the codec's own decoder, and compare with the input. NaN when the input
/// is silent, because SNR is undefined there.
fn roundtrip_snr(job: &EncodeJob) -> Result<f64, String> {
    let rate = job.rate;
    let block_samples = samples_per_block(job.codec, rate);
    let channels = job.channels;
    let mut encoder = build_encoder(job.codec, rate, job.bits_per_coef, job.band_step)?;
    let private = match job.codec {
        AudioCodec::Cinder => vec![job.bits_per_coef],
        AudioCodec::Aura => (block_samples as u16).to_le_bytes().to_vec(),
        AudioCodec::Reed => ReedConfig {
            sample_rate: rate,
            lsf_bits: job.bits_per_coef,
            ..ReedConfig::default()
        }
        .to_codec_private(),
    };
    let mut decoder = make_decoder(job.codec, rate, encoder.info().version, &private)?;
    let total = job.samples.len() / (2 * channels);
    let track_timebase = Timebase::new(1, rate);
    let mut packets = Vec::new();
    let mut start = 0usize;
    while start < total {
        let end = (start + block_samples).min(total);
        let mut block = vec![0u8; block_samples * 2 * channels];
        block[..(end - start) * 2 * channels]
            .copy_from_slice(&job.samples[start * 2 * channels..end * 2 * channels]);
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
            sample_count: block_samples as u32,
            format: SampleFormat::S16,
            channel_layout: layout_for(channels),
            sample_rate: rate,
        };
        packets.extend(
            encoder
                .encode(&chunk)
                .map_err(|e| format!("encode failed: {e}"))?,
        );
        start = end;
    }
    packets.extend(encoder.drain().map_err(|e| format!("drain failed: {e}"))?);
    let mut out = Vec::new();
    for pkt in &packets {
        for f in decoder
            .decode(pkt)
            .map_err(|e| format!("decode failed: {e}"))?
        {
            out.extend(s16_to_f32(&f.samples));
        }
    }
    let reference = s16_to_f32(&job.samples);
    let n = reference.len().min(out.len());
    Ok(crate::quality::snr_db(&reference[..n], &out[..n]).unwrap_or(f64::NAN))
}

/// The value `goal` is judged on, at the job's current settings.
fn measure(job: &EncodeJob, goal: AudioGoal) -> Result<f64, String> {
    match goal {
        AudioGoal::Bitrate(_) => container_bitrate(job),
        AudioGoal::Snr(_) => {
            let snr = roundtrip_snr(job)?;
            if snr.is_nan() {
                Err("--target-snr is undefined for silent input".into())
            } else {
                Ok(snr)
            }
        }
    }
}

/// How much of the input the quality-target searches run on. Each trial is
/// a full encode and decode, so trials on a prefix are cheap; the chosen
/// setting is then checked on the whole input, so the target is still met.
const FIT_SEARCH_FRAMES: usize = 16;
const FIT_SEARCH_SECONDS: usize = 8;

/// Cinder's coarsest band step whose decoded SNR meets an SNR target.
///
/// SNR is not monotone in the step. Below about 0.25 the 8-bit escape cannot
/// hold the large levels, so SNR collapses, and it peaks near a step of 1.
/// So the search scans a grid of steps and keeps the coarsest that passes
/// (coarser steps cost fewer bits), then bisects between that point and the
/// next coarser grid point, where SNR falls below the target.
///
/// The search runs on the first few seconds of the input. The caller checks
/// the step it returns on the whole input and steps finer if needed.
fn search_cinder_snr_step(search: &mut EncodeJob, goal: AudioGoal) -> Result<f32, String> {
    let mut grid: Vec<f32> = (-10..=5).map(|k: i32| 2f32.powi(k)).collect();
    grid.push(STEP_MAX);
    let mut points: Vec<(f32, f64)> = Vec::with_capacity(grid.len() + 4);
    for &step in &grid {
        search.band_step = Some(step);
        points.push((step, measure(search, goal)?));
    }
    let mut passing = points.iter().rposition(|(_, v)| meets(*v, goal));
    if passing.is_none() {
        // The grid is only octave-spaced, so the best SNR can sit between
        // two grid points and the target still be reachable. Probe either
        // side of the best grid point before reporting it out of reach.
        let peak = points
            .iter()
            .enumerate()
            .max_by(|a, b| a.1 .1.total_cmp(&b.1 .1))
            .map(|(i, _)| i)
            .unwrap_or(0);
        let centre = points[peak].0.log2();
        for offset in [-0.5f32, -0.25, 0.25, 0.5] {
            let step = (centre + offset).exp2().clamp(STEP_MIN, STEP_MAX);
            search.band_step = Some(step);
            points.push((step, measure(search, goal)?));
        }
        points.sort_by(|a, b| a.0.total_cmp(&b.0));
        passing = points.iter().rposition(|(_, v)| meets(*v, goal));
    }
    let Some(i) = passing else {
        let highest = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
        return Err(format!(
            "{} is out of reach: the best band step gives {highest:.2} dB",
            goal_label(goal)
        ));
    };
    let (mut g, mut b) = (
        points[i].0.log2(),
        points.get(i + 1).map_or(STEP_MAX, |p| p.0).log2(),
    );
    while (g - b).abs() > 0.01 {
        let mid = (g + b) / 2.0;
        search.band_step = Some(mid.exp2().clamp(STEP_MIN, STEP_MAX));
        if meets(measure(search, goal)?, goal) {
            g = mid;
        } else {
            b = mid;
        }
    }
    Ok(g.exp2().clamp(STEP_MIN, STEP_MAX))
}

/// `--target-snr` for Cinder: search on a prefix, then check the step on the
/// whole input and step finer (by a quarter octave) until it meets the target.
fn fit_cinder_snr(job: &mut EncodeJob, goal: AudioGoal) -> Result<String, Outcome> {
    let fail = |e: String| Outcome::fail(1, e);
    let frame_bytes = 2 * job.channels;
    let total = job.samples.len() / frame_bytes;
    let keep = total.min(FIT_SEARCH_SECONDS * job.rate as usize);
    let mut search = EncodeJob {
        codec: job.codec,
        rate: job.rate,
        bits_per_coef: job.bits_per_coef,
        band_step: None,
        channels: job.channels,
        samples: job.samples[..keep * frame_bytes].to_vec(),
        sample_count: keep,
        extra_meta: Vec::new(),
    };
    let mut step = search_cinder_snr_step(&mut search, goal).map_err(fail)?;
    loop {
        job.band_step = Some(step);
        let value = measure(job, goal).map_err(fail)?;
        if meets(value, goal) {
            return Ok(format!(
                ", {} met at band step {step:.4} ({})",
                goal_label(goal),
                describe(value, goal)
            ));
        }
        if step <= STEP_MIN {
            return Err(fail(format!(
                "{} is out of reach: the finest band step ({STEP_MIN}) gives {}",
                goal_label(goal),
                describe(value, goal)
            )));
        }
        step = (step.log2() - 0.25).exp2().clamp(STEP_MIN, STEP_MAX);
    }
}

/// Pick the setting an audio goal needs and leave `job` at it. Overrides
/// `--quality` and `--band-step`. Returns the note for the summary line.
///
/// Cinder's rate and SNR both follow its quantiser step (`bits_per_coef`
/// only sets an escape width), so the step is bisected on a log scale.
/// Reed follows its LSF bit depth, which is discrete, so it is scanned.
/// Lossless Aura has no quality dial to turn, so it is refused.
///
/// When a target cannot be met, `--bitrate` encodes at the codec's floor
/// (its smallest output) and warns. `--target-snr` is an error instead,
/// because the user asked for a quality the codec cannot reach.
fn fit_audio(job: &mut EncodeJob, goal: AudioGoal) -> Result<String, Outcome> {
    if job.codec == AudioCodec::Aura {
        return Err(Outcome::usage(
            "--bitrate and --target-snr need a lossy codec: aura is lossless \
             (use --codec cinder or reed)",
        ));
    }
    if job.samples.is_empty() {
        return Ok(", nothing to fit (no samples)".to_string());
    }
    let fail = |e: String| Outcome::fail(1, e);
    match job.codec {
        AudioCodec::Cinder if matches!(goal, AudioGoal::Snr(_)) => fit_cinder_snr(job, goal),
        AudioCodec::Cinder => {
            // Bitrate falls as the step coarsens, so bisect the step.
            let (good, bad) = match goal {
                AudioGoal::Bitrate(_) => (STEP_MAX, STEP_MIN),
                AudioGoal::Snr(_) => (STEP_MIN, STEP_MAX),
            };
            let at_step = |job: &mut EncodeJob, step: f32| -> Result<f64, Outcome> {
                job.band_step = Some(step);
                measure(job, goal).map_err(fail)
            };
            let good_value = at_step(job, good)?;
            if !meets(good_value, goal) {
                return match goal {
                    AudioGoal::Bitrate(_) => Ok(format!(
                        ", warning: {} is below this codec's floor ({} at band step {STEP_MAX})",
                        goal_label(goal),
                        describe(good_value, goal)
                    )),
                    AudioGoal::Snr(_) => Err(fail(format!(
                        "{} is out of reach: the finest band step ({STEP_MIN}) gives {}",
                        goal_label(goal),
                        describe(good_value, goal)
                    ))),
                };
            }
            let bad_value = at_step(job, bad)?;
            let step = if meets(bad_value, goal) {
                bad
            } else {
                let (mut g, mut b) = (good.log2(), bad.log2());
                while (g - b).abs() > 0.01 {
                    let mid = (g + b) / 2.0;
                    let step = mid.exp2().clamp(STEP_MIN, STEP_MAX);
                    if meets(at_step(job, step)?, goal) {
                        g = mid;
                    } else {
                        b = mid;
                    }
                }
                g.exp2().clamp(STEP_MIN, STEP_MAX)
            };
            let value = at_step(job, step)?;
            Ok(format!(
                ", {} met at band step {step:.4} ({})",
                goal_label(goal),
                describe(value, goal)
            ))
        }
        AudioCodec::Reed => {
            let lo = quality_to_lsf_bits(Some(0));
            let hi = quality_to_lsf_bits(Some(100));
            // Both rate and SNR grow with depth. Bitrate takes the finest
            // depth that fits; SNR takes the coarsest depth that reaches
            // the target. Either way the first hit in the scan wins.
            let order: Vec<u8> = match goal {
                AudioGoal::Bitrate(_) => (lo..=hi).rev().collect(),
                AudioGoal::Snr(_) => (lo..=hi).collect(),
            };
            let mut last = (lo, 0.0);
            for bits in order {
                job.bits_per_coef = bits;
                let value = measure(job, goal).map_err(fail)?;
                if meets(value, goal) {
                    return Ok(format!(
                        ", {} met at {bits} LSF bits ({})",
                        goal_label(goal),
                        describe(value, goal)
                    ));
                }
                last = (bits, value);
            }
            let (bits, value) = last;
            match goal {
                AudioGoal::Bitrate(_) => Ok(format!(
                    ", warning: {} is below this codec's floor ({} at {bits} LSF bits)",
                    goal_label(goal),
                    describe(value, goal)
                )),
                AudioGoal::Snr(_) => Err(fail(format!(
                    "{} is out of reach: the finest LSF depth ({bits}) gives {}",
                    goal_label(goal),
                    describe(value, goal)
                ))),
            }
        }
        AudioCodec::Aura => unreachable!("refused above"),
    }
}

/// Mean PSNR (dB) of `frames` decoded through `codec` at quantiser `qp`:
/// the video counterpart of [`roundtrip_snr`]. Bit-exact frames have no
/// finite PSNR and are left out; a clip where every frame is bit-exact
/// reports infinity.
fn video_roundtrip_psnr(
    codec: VideoCodec,
    params: &y4m::Y4mParams,
    frames: &[VideoFrame],
    qp: u8,
) -> Result<f64, String> {
    let (mut encoder, private) =
        codec.new_encoder(params.width, params.height, params.format, Some(qp), None)?;
    let mut decoder = codec.new_decoder(
        params.width,
        params.height,
        params.format,
        &private,
        encoder.info().version,
    )?;
    let mut packets = Vec::new();
    for f in frames {
        packets.extend(
            encoder
                .encode(f)
                .map_err(|e| format!("encode failed: {e}"))?,
        );
    }
    let mut decoded = Vec::new();
    for pkt in &packets {
        decoded.extend(
            decoder
                .decode(pkt)
                .map_err(|e| format!("decode failed: {e}"))?,
        );
    }
    let finite: Vec<f64> = frames
        .iter()
        .zip(&decoded)
        .filter_map(|(src, out)| crate::quality::psnr_db(src, out))
        .filter(|p| p.is_finite())
        .collect();
    Ok(if finite.is_empty() {
        f64::INFINITY
    } else {
        finite.iter().sum::<f64>() / finite.len() as f64
    })
}

/// The coarsest quantiser whose mean PSNR still meets `target` dB, by
/// bisection over the legal `1..=51`. PSNR falls as qp rises (the search
/// assumes that, which holds for these codecs' rate-quality curves), so the
/// coarsest passing qp is the smallest file the target allows.
///
/// The bisection runs on the first [`FIT_SEARCH_FRAMES`] frames. The chosen
/// qp is then checked on the whole clip, and stepped finer until the whole
/// clip meets the target. Returns the qp and the whole-clip PSNR it reaches.
fn fit_video_qp(
    codec: VideoCodec,
    params: &y4m::Y4mParams,
    frames: &[VideoFrame],
    target: f64,
) -> Result<(u8, f64), String> {
    let prefix = &frames[..frames.len().min(FIT_SEARCH_FRAMES)];
    // qp 1 is assumed to pass here; if it does not, the whole-clip check
    // below reports the target as out of reach.
    let (mut good, mut bad) = (1u8, 52u8);
    while bad - good > 1 {
        let mid = good + (bad - good) / 2;
        if video_roundtrip_psnr(codec, params, prefix, mid)? >= target {
            good = mid;
        } else {
            bad = mid;
        }
    }
    // Check the candidate on the whole clip. If it fails there, bisect
    // finer on the whole clip: a bounded search, so an unreachable target
    // costs a handful of encodes, not one per quantiser step.
    let psnr = video_roundtrip_psnr(codec, params, frames, good)?;
    if psnr >= target {
        return Ok((good, psnr));
    }
    let unreachable = |psnr: f64| {
        format!(
            "--target-psnr {target} dB is out of reach: the finest quantiser (qp 1) gives {psnr:.2} dB over the whole clip"
        )
    };
    let finest = video_roundtrip_psnr(codec, params, frames, 1)?;
    if finest < target {
        return Err(unreachable(finest));
    }
    let (mut ok, mut failing) = (1u8, good);
    let mut ok_psnr = finest;
    while failing - ok > 1 {
        let mid = ok + (failing - ok) / 2;
        let mid_psnr = video_roundtrip_psnr(codec, params, frames, mid)?;
        if mid_psnr >= target {
            ok = mid;
            ok_psnr = mid_psnr;
        } else {
            failing = mid;
        }
    }
    Ok((ok, ok_psnr))
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
            let width = 2 * last.channel_layout.channels() as usize;
            last.samples.truncate(last.sample_count as usize * width);
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

/// The `, notes…` tail appended to an encode summary when the input was
/// changed on the way in (empty when nothing was).
fn prep_suffix(notes: &[String]) -> String {
    notes.iter().map(|n| format!(", {n}")).collect()
}

/// `--quality 0..=100` mapped onto Cinder's band step: log-scaled from 64
/// (coarsest, quality 0) down to [`QUALITY_FINEST_STEP`] (finest, quality 100).
/// Aura and Reed have no band step.
fn quality_to_band_step(quality: Option<u8>) -> Option<f32> {
    quality.map(|q| {
        // 6 = log2(64) and -4 = log2(1/16): ten octaves across the dial.
        (6.0 - 10.0 * f32::from(q) / 100.0)
            .exp2()
            .clamp(QUALITY_FINEST_STEP, STEP_MAX)
    })
}

/// The band step for a run: an explicit `--band-step` wins, then, for
/// Cinder only, `--quality`; otherwise the codec default (unset).
fn band_step_for(spec: &OpSpec, codec: AudioCodec) -> Option<f32> {
    spec.band_step.or_else(|| match codec {
        AudioCodec::Cinder => quality_to_band_step(spec.quality),
        _ => None,
    })
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
            let channels = frames.first().map_or(1, |f| f.channel_layout.channels());
            if !audio_channels_ok(target, channels) {
                return Outcome::fail(
                    1,
                    format!(
                        "transcode to {} cannot take a {channels}-channel source",
                        target.name()
                    ),
                );
            }
            let mut job = EncodeJob {
                codec: target,
                channels: channels as usize,
                rate,
                bits_per_coef: quality_param(target, spec.quality),
                band_step: band_step_for(spec, target),
                samples,
                sample_count,
                extra_meta: carry_meta(&track),
            };
            let fit_note = match audio_goal(spec) {
                Ok(Some(goal)) => match fit_audio(&mut job, goal) {
                    Ok(note) => note,
                    Err(e) => return e,
                },
                Ok(None) => String::new(),
                Err(e) => return e,
            };
            let (bytes, written) = match run_encode_job(&job) {
                Ok(r) => r,
                Err(e) => return Outcome::fail(1, e),
            };
            write_output(
                spec,
                bytes,
                format!(
                    "transcoded {written} packets ({sample_count} samples, {} -> {}){fit_note}",
                    source.name(),
                    target.name()
                ),
                ctx,
            )
        }
        DemuxedMedia::Video(track, frames) => {
            if spec.target_snr.is_some() {
                return Outcome::usage(
                    "--target-snr applies to audio; use --target-psnr for video",
                );
            }
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
            let mut qp = spec.quality.map(|q| quality_to_qp(Some(q)));
            let mut fit_note = String::new();
            if let Some(target_db) = spec.target_psnr {
                match fit_video_qp(target, &params, &frames, target_db) {
                    Ok((q, psnr)) => {
                        qp = Some(q);
                        fit_note =
                            format!(", --target-psnr {target_db} dB met at qp {q} ({psnr:.2} dB)");
                    }
                    Err(e) => return Outcome::fail(1, e),
                }
            }
            let job = VideoEncodeJob {
                codec: target,
                fps_num: params.fps_num,
                fps_den: params.fps_den,
                qp,
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
                     {source} -> {} at --quality {}{fit_note})",
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
    let qp = spec.quality.map(|q| quality_to_qp(Some(q)));
    let qp_shown = qp.unwrap_or(quality_to_qp(None));
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
        j.value_u64(u64::from(qp_shown));
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
             {packet_count} packets, qp {qp_shown}\n",
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
    let (frame, notes) = match audio_input::prepare(frame, codec == AudioCodec::Aura, spec.resample)
    {
        Ok(p) => p,
        Err(e) => return Outcome::fail(1, e),
    };
    if frame.format != SampleFormat::S16
        || !audio_channels_ok(codec, frame.channel_layout.channels())
        || !supported_input_rates(codec).contains(&frame.sample_rate)
    {
        return Outcome::fail(
            1,
            format!(
                "benchmark requires a wav at {} Hz \
                 for this codec (got {:?}, {} ch, {} Hz); --resample converts the rate",
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
    let mut bits_per_coef = quality_param(codec, spec.quality);
    let mut band_step = band_step_for(spec, codec);
    let fit_note = match audio_goal(spec) {
        Ok(Some(goal)) => {
            let mut job = EncodeJob {
                codec,
                rate,
                bits_per_coef,
                band_step,
                channels: frame.channel_layout.channels() as usize,
                samples: frame.samples.clone(),
                sample_count: total,
                extra_meta: Vec::new(),
            };
            let note = match fit_audio(&mut job, goal) {
                Ok(note) => note,
                Err(e) => return e,
            };
            bits_per_coef = job.bits_per_coef;
            band_step = job.band_step;
            note
        }
        Ok(None) => String::new(),
        Err(e) => return e,
    };
    let runs = spec.runs.unwrap_or(5);
    let block = samples_per_block(codec, rate);
    let mut encoder = match build_encoder(codec, rate, bits_per_coef, band_step) {
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
    let channels = frame.channel_layout.channels() as usize;
    let mut blocks = Vec::new();
    let mut start = 0usize;
    while start < total {
        let end = (start + block).min(total);
        let mut samples = vec![0u8; block * 2 * channels];
        let (bs, be) = (start * 2 * channels, end * 2 * channels);
        samples[..be - bs].copy_from_slice(&frame.samples[bs..be]);
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
            channel_layout: layout_for(channels),
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
            encoder = match build_encoder(codec, rate, bits_per_coef, band_step) {
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
    let width = 2 * frame.channel_layout.channels() as usize;
    let mut decoded_samples: Vec<u8> = Vec::with_capacity(total * width);
    for f in &frames {
        decoded_samples.extend_from_slice(&f.samples);
    }
    decoded_samples.truncate(total * width);
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
        j.field("input_changes");
        j.arr();
        for n in &notes {
            j.elem();
            j.value_str(n);
        }
        j.close();
        j.field("fit");
        j.value_str(fit_note.trim_start_matches(", "));
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
        if !notes.is_empty() {
            out.push_str(&format!("input:    {}\n", notes.join(", ")));
        }
        if !fit_note.is_empty() {
            out.push_str(&format!(
                "fit:      {}\n",
                fit_note.trim_start_matches(", ")
            ));
        }
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
        let (mut enc, private) = match codec.new_encoder(w, h, format, Some(30), None) {
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

/// A JSON number, or `null` when the metric is undefined (or infinite,
/// which JSON cannot carry).
fn opt_f64(j: &mut J, v: Option<f64>) {
    match v {
        Some(v) if v.is_finite() => j.value_f64(v),
        _ => j.value_null(),
    }
}

/// 16-bit little-endian PCM bytes as `f32` in `-1.0..1.0`.
fn s16_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0)
        .collect()
}

/// `tpt compare`: score a decoded (or otherwise degraded) file against its
/// reference — SNR and log-spectral distance for two WAVs, mean PSNR and
/// SSIM for two y4m clips — with the same §37 metrics `benchmark` uses, so
/// a user can check any codec round trip (including other tools') without
/// writing a script. Exit 0 whenever the comparison could be made.
pub fn compare_cmd(reference: &str, test: &str, json: bool, ctx: &Ctx<'_>) -> Outcome {
    if reference == "-" && test == "-" {
        return Outcome::usage("compare reads at most one of its inputs from stdin");
    }
    let (a, b) = match (read_source(reference, ctx), read_source(test, ctx)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return Outcome::fail(1, e),
    };
    let video = (a.starts_with(b"YUV4MPEG2"), b.starts_with(b"YUV4MPEG2"));
    match video {
        (true, true) => compare_video(&a, &b, json),
        (false, false) => compare_audio(&a, &b, json),
        _ => Outcome::fail(
            1,
            "cannot compare an audio file with a video file (both must be wav, or both y4m)",
        ),
    }
}

fn compare_audio(a: &[u8], b: &[u8], json: bool) -> Outcome {
    let (ra, tb) = match (decode_wav(a), decode_wav(b)) {
        (Ok(x), Ok(y)) => (x, y),
        (Err(e), _) => return Outcome::fail(1, format!("reference is not a readable wav: {e}")),
        (_, Err(e)) => return Outcome::fail(1, format!("test is not a readable wav: {e}")),
    };
    if ra.format != SampleFormat::S16 || tb.format != SampleFormat::S16 {
        return Outcome::fail(1, "compare needs 16-bit wav files");
    }
    if ra.sample_rate != tb.sample_rate
        || ra.channel_layout.channels() != tb.channel_layout.channels()
    {
        return Outcome::fail(
            1,
            format!(
                "files differ in format ({} ch @ {} Hz vs {} ch @ {} Hz)",
                ra.channel_layout.channels(),
                ra.sample_rate,
                tb.channel_layout.channels(),
                tb.sample_rate
            ),
        );
    }
    let (x, y) = (s16_to_f32(&ra.samples), s16_to_f32(&tb.samples));
    let n = x.len().min(y.len());
    if n == 0 {
        return Outcome::fail(1, "nothing to compare: a file has no samples");
    }
    let (x, y) = (&x[..n], &y[..n]);
    let snr = crate::quality::snr_db(x, y);
    let lsd = crate::quality::log_spectral_distance_db(x, y, 1024);
    let length_note = (ra.samples.len() != tb.samples.len()).then(|| {
        format!(
            "lengths differ ({} vs {} bytes of audio); compared the first {} samples",
            ra.samples.len(),
            tb.samples.len(),
            n
        )
    });
    if json {
        let mut j = J::new();
        j.object();
        j.field("kind");
        j.value_str("audio");
        j.field("samples_compared");
        j.value_u64(n as u64);
        j.field("snr_db");
        opt_f64(&mut j, snr);
        j.field("log_spectral_distance_db");
        opt_f64(&mut j, lsd);
        j.field("length_mismatch");
        j.value_bool(length_note.is_some());
        j.close();
        return Outcome::ok(j.finish() + "\n");
    }
    let mut out = format!(
        "SNR: {}\nLog-spectral distance: {}\n",
        match snr {
            Some(v) if v.is_infinite() => "identical (bit-exact)".to_string(),
            Some(v) => format!("{v:.2} dB"),
            None => "n/a".to_string(),
        },
        lsd.map_or("n/a".to_string(), |v| format!("{v:.2} dB"))
    );
    if let Some(note) = length_note {
        out.push_str(&format!("note: {note}\n"));
    }
    Outcome::ok(out)
}

fn compare_video(a: &[u8], b: &[u8], json: bool) -> Outcome {
    let ((pa, fa), (pb, fb)) = match (y4m::parse(a), y4m::parse(b)) {
        (Ok(x), Ok(y)) => (x, y),
        (Err(e), _) => return Outcome::fail(1, format!("reference is not a readable y4m: {e}")),
        (_, Err(e)) => return Outcome::fail(1, format!("test is not a readable y4m: {e}")),
    };
    if (pa.width, pa.height, pa.format) != (pb.width, pb.height, pb.format) {
        return Outcome::fail(
            1,
            format!(
                "clips differ in geometry ({}x{} vs {}x{})",
                pa.width, pa.height, pb.width, pb.height
            ),
        );
    }
    let n = fa.len().min(fb.len());
    if n == 0 {
        return Outcome::fail(1, "nothing to compare: a clip has no frames");
    }
    // Mean over frames; a bit-exact frame has infinite PSNR, which would
    // swamp the mean, so identical frames are counted separately.
    let (mut psnr_sum, mut ssim_sum) = (0f64, 0f64);
    let (mut finite, mut identical) = (0usize, 0usize);
    let mut worst: Option<f64> = None;
    for (r, t) in fa.iter().zip(fb.iter()) {
        if let Some(p) = crate::quality::psnr_db(r, t) {
            if p.is_finite() {
                psnr_sum += p;
                finite += 1;
                worst = Some(worst.map_or(p, |w| w.min(p)));
            } else {
                identical += 1;
            }
        }
        ssim_sum += crate::quality::ssim(r, t).unwrap_or(1.0);
    }
    let mean_psnr = (finite > 0).then(|| psnr_sum / finite as f64);
    let mean_ssim = ssim_sum / n as f64;
    let length_mismatch = fa.len() != fb.len();
    if json {
        let mut j = J::new();
        j.object();
        j.field("kind");
        j.value_str("video");
        j.field("frames_compared");
        j.value_u64(n as u64);
        j.field("mean_psnr_db");
        opt_f64(&mut j, mean_psnr);
        j.field("worst_psnr_db");
        opt_f64(&mut j, worst);
        j.field("identical_frames");
        j.value_u64(identical as u64);
        j.field("mean_ssim");
        j.value_f64(mean_ssim);
        j.field("length_mismatch");
        j.value_bool(length_mismatch);
        j.close();
        return Outcome::ok(j.finish() + "\n");
    }
    let mut out =
        format!(
        "Frames compared: {n}\nMean PSNR: {}\nWorst-frame PSNR: {}\nMean SSIM: {mean_ssim:.4}\n",
        mean_psnr.map_or("identical (bit-exact)".to_string(), |v| format!("{v:.2} dB")),
        worst.map_or("n/a".to_string(), |v| format!("{v:.2} dB")),
    );
    if identical > 0 && finite > 0 {
        out.push_str(&format!(
            "({identical} frames bit-exact, excluded from PSNR)\n"
        ));
    }
    if length_mismatch {
        out.push_str(&format!(
            "note: lengths differ ({} vs {} frames); compared the first {n}\n",
            fa.len(),
            fb.len()
        ));
    }
    Outcome::ok(out)
}

/// Execute a parsed command.
pub fn execute(cmd: &Command, ctx: &Ctx<'_>) -> Outcome {
    match cmd {
        Command::Probe { input, json } => probe(input, *json, ctx),
        Command::Inspect { input } => probe(input, false, ctx),
        Command::ListCodecs => list_codecs(),
        Command::Completions { shell } => crate::tools::completions(shell),
        Command::Doctor => crate::tools::doctor(),
        Command::Compare {
            reference,
            test,
            json,
        } => compare_cmd(reference, test, *json, ctx),
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
