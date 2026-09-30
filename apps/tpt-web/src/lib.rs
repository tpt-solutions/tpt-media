//! `tpt-web`: the browser player core for `.tpt` media (todo.md section 45).
//!
//! The crate has three layers, thinnest last:
//!
//! 1. **Plain Rust, host-testable** ([`StreamParser`], [`DecoderTable`],
//!    [`Player`]): an incremental `.tpt` parser with a bounded buffer, a
//!    codec table over the decoders that exist (Cinder, Aura, Reed, Thrum),
//!    and a push/pull streaming player that produces planar `f32` audio and
//!    RGBA8 pictures under hard memory limits ([`WebLimits`]). No
//!    JavaScript, no wasm, no clock; every behaviour is a native unit test.
//! 2. **A C-ABI export surface** ([`abi`]): eighteen `tpt_web_*`
//!    functions over integer handles, meant to be compiled to
//!    `wasm32-unknown-unknown`. There is no `wasm-bindgen`: the surface is
//!    integers and pointers into linear memory, so the dependency (and the
//!    generated glue, and the build tool) is not needed. See
//!    `docs/web-design.md`.
//! 3. **Hand-written JavaScript** (`web/tpt-web.js`, not part of the Rust
//!    crate): loads the module, moves bytes across, and schedules WebAudio
//!    and canvas output. No bundler.
//!
//! Vault and Mote (todo.md sections 33 and 34) are not dependencies: the
//! player only speaks the codec-agnostic `DynDecoder` interface, and
//! [`DecoderTable`] is the single place a new codec is registered.
//!
//! # Example
//!
//! ```no_run
//! use tpt_web::{Player, Status, WebError, WebLimits};
//!
//! fn play(file: &[u8]) -> Result<(), WebError> {
//!     let mut player = Player::new(WebLimits::default());
//!     for chunk in file.chunks(16 * 1024) {
//!         loop {
//!             match player.push(chunk) {
//!                 Ok(()) => break,
//!                 // Back-pressure: drain, then retry the same bytes.
//!                 Err(WebError::InputFull) => {
//!                     player.pump()?;
//!                     while let Some(_audio) = player.pop_audio() {}
//!                     while let Some(_frame) = player.pop_video() {}
//!                 }
//!                 Err(e) => return Err(e),
//!             }
//!         }
//!     }
//!     player.finish_input();
//!     while player.pump()? != Status::Ended {
//!         while let Some(_audio) = player.pop_audio() { /* schedule */ }
//!         while let Some(_frame) = player.pop_video() { /* draw */ }
//!     }
//!     Ok(())
//! }
//! ```

// `unsafe` is confined to `abi`, which must export `#[no_mangle]` functions
// (the `unsafe_code` lint flags that attribute in edition 2021) and hand
// JavaScript raw pointers into its own buffers. Everything else is safe Rust.
#![deny(unsafe_code)]

pub mod abi;
mod codecs;
mod convert;
mod error;
mod limits;
mod parser;
mod player;
#[cfg(feature = "synth")]
pub mod synth;

pub use codecs::{DecoderBuilder, DecoderTable};
pub use convert::{audio_to_planar_f32, video_to_rgba};
pub use error::{ContainerFault, WebError};
pub use limits::WebLimits;
pub use parser::{ParseEvent, ParserStats, StreamParser};
pub use player::{
    AudioChunk, AudioInfo, Player, PlayerStats, Status, StreamInfo, VideoInfo, VideoPicture,
    SAMPLE_COUNT_KEY,
};
