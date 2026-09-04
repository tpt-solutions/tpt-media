//! tpt-web — WASM bindings exposing tpt decoders to JavaScript.
//!
//! Build with: `wasm-pack build --target web --features wasm`.
//! See `apps/tpt-web/index.html` for the drag-and-drop front-end.

#[cfg(feature = "wasm")]
mod wasm;
#[cfg(feature = "wasm")]
pub use wasm::*;