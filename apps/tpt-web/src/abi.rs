//! The C-ABI export surface a browser calls through `WebAssembly.instantiate`.
//!
//! Everything here is a thin veneer over [`crate::Player`]: integer handles
//! in, integers / `f64` / raw pointers out. There is no `wasm-bindgen`;
//! `web/tpt-web.js` is the hand-written counterpart of this file, and the two
//! must change together ([`ABI_VERSION`] is bumped when a signature or
//! meaning changes).
//!
//! # Ownership and memory model
//!
//! * A **handle** is a small non-zero integer naming a player owned by a
//!   thread-local table (a wasm instance is single-threaded, so the table
//!   needs no lock). `0` is never a valid handle, so a failed `open` cannot
//!   be mistaken for one. At most [`MAX_PLAYERS`] exist at once.
//! * **Input** is staged in a per-player buffer *owned by the module*: JS
//!   asks for `len` bytes with [`tpt_web_input_ptr`], writes into that
//!   region of linear memory, then calls [`tpt_web_input_commit`]. No
//!   pointer ever flows from JS into Rust, which is what lets the parser and
//!   player stay entirely safe code.
//! * **Output** is the same in reverse: [`tpt_web_audio_next`] /
//!   [`tpt_web_video_next`] move the next chunk into a per-player "current"
//!   slot and return its size; [`tpt_web_audio_ptr`] / [`tpt_web_video_ptr`]
//!   give the address JS reads it from. The pointer is valid until the next
//!   call that takes the same handle, and **every** call that can allocate
//!   may grow (and so detach) `memory.buffer`, so JS must construct its typed
//!   array views fresh after each call rather than caching them.
//! * **Errors**: functions returning `i32` return a negative
//!   [`WebError::code`] on failure (`-1` is "invalid handle"), and
//!   [`tpt_web_error_ptr`] / [`tpt_web_error_len`] expose the UTF-8 message
//!   of the most recent failure.
//!
//! # Panics
//!
//! No export panics on any input the parser and codecs are tested against
//! (see the fuzz-style tests in `parser.rs` and `player/tests.rs`, and
//! `web/check.mjs`, which repeats them against the built wasm). The
//! workspace release profile sets `panic = "abort"`, which on wasm is a
//! trap: if a bug did slip through, the instance stops instead of running on
//! with corrupt state, and the page can create a fresh one.

#![allow(unsafe_code)]

use std::cell::RefCell;

use crate::error::WebError;
use crate::limits::{WebLimits, STAGING_MAX_BYTES};
use crate::player::{AudioChunk, Player, Status, VideoPicture};

/// Version of this export surface. Bumped on any signature or semantic
/// change; `tpt-web.js` refuses a module with a different value.
pub const ABI_VERSION: u32 = 1;

/// Maximum simultaneously open players per module instance.
pub const MAX_PLAYERS: usize = 4;

/// Returned by `i32` functions for a handle that is not open.
pub const ERR_BAD_HANDLE: i32 = -1;

struct Slot {
    player: Player,
    /// Staging area JS writes input into (see the module docs).
    input: Vec<u8>,
    audio: Option<AudioChunk>,
    video: Option<VideoPicture>,
}

#[derive(Default)]
struct Host {
    slots: Vec<Option<Slot>>,
    error: String,
}

impl Host {
    fn set_error(&mut self, e: &WebError) -> i32 {
        self.error = e.to_string();
        e.code()
    }

    fn bad_handle(&mut self) -> i32 {
        self.error = "invalid handle".to_string();
        ERR_BAD_HANDLE
    }

    fn slot(&mut self, handle: u32) -> Option<&mut Slot> {
        let idx = (handle as usize).checked_sub(1)?;
        self.slots.get_mut(idx)?.as_mut()
    }
}

thread_local! {
    static HOST: RefCell<Host> = RefCell::new(Host::default());
}

fn with_host<R>(f: impl FnOnce(&mut Host) -> R) -> R {
    HOST.with(|h| f(&mut h.borrow_mut()))
}

fn status_code(s: Status) -> i32 {
    match s {
        Status::NeedInput => 0,
        Status::Ready => 1,
        Status::Ended => 2,
        Status::Failed => 3,
    }
}

/// The ABI version this module was built with.
#[no_mangle]
pub extern "C" fn tpt_web_abi_version() -> u32 {
    ABI_VERSION
}

/// Open a player. Each argument overrides one default limit; `0` keeps the
/// default. All are clamped to [`WebLimits::ceiling`]. Returns the handle,
/// or `0` if [`MAX_PLAYERS`] are already open.
#[no_mangle]
pub extern "C" fn tpt_web_open(
    max_input_bytes: u32,
    max_frame_bytes: u32,
    max_audio_queue_ms: u32,
    max_video_queue_frames: u32,
) -> u32 {
    let mut limits = WebLimits::default();
    if max_input_bytes != 0 {
        limits.max_input_buffer_bytes = max_input_bytes;
    }
    if max_frame_bytes != 0 {
        limits.codec.max_frame_bytes = max_frame_bytes;
    }
    if max_audio_queue_ms != 0 {
        limits.max_audio_queue_ms = max_audio_queue_ms;
    }
    if max_video_queue_frames != 0 {
        limits.max_video_queue_frames = max_video_queue_frames;
    }
    with_host(|h| {
        let slot = Slot {
            player: Player::new(limits),
            input: Vec::new(),
            audio: None,
            video: None,
        };
        // Reuse a free slot before growing the table.
        if let Some(i) = h.slots.iter().position(Option::is_none) {
            h.slots[i] = Some(slot);
            return (i + 1) as u32;
        }
        if h.slots.len() >= MAX_PLAYERS {
            h.error = "too many open players".to_string();
            return 0;
        }
        h.slots.push(Some(slot));
        h.slots.len() as u32
    })
}

/// Close a player and free everything it owns. Closing an unknown handle is
/// a no-op.
#[no_mangle]
pub extern "C" fn tpt_web_close(handle: u32) {
    with_host(|h| {
        if let Some(idx) = (handle as usize).checked_sub(1) {
            if let Some(s) = h.slots.get_mut(idx) {
                *s = None;
            }
        }
    });
}

/// Reserve `len` bytes of input staging and return their address, or null on
/// failure (bad handle, or `len` above the staging cap: push in slices).
/// The region is zero-filled and stays valid until the next call with this
/// handle.
#[no_mangle]
pub extern "C" fn tpt_web_input_ptr(handle: u32, len: u32) -> *mut u8 {
    with_host(|h| {
        if len as usize > STAGING_MAX_BYTES {
            h.set_error(&WebError::BadArgument(
                "input slice larger than the staging limit",
            ));
            return core::ptr::null_mut();
        }
        let Some(slot) = h.slot(handle) else {
            h.bad_handle();
            return core::ptr::null_mut();
        };
        slot.input.clear();
        if slot.input.try_reserve(len as usize).is_ok() {
            slot.input.resize(len as usize, 0);
            return slot.input.as_mut_ptr();
        }
        h.set_error(&WebError::Limit("input staging allocation"));
        core::ptr::null_mut()
    })
}

/// Push the first `len` bytes of the staging region to the player. Returns
/// `0`, or `-2` (`InputFull`: pump, drain output, and commit the *same*
/// staged bytes again) or another negative error.
#[no_mangle]
pub extern "C" fn tpt_web_input_commit(handle: u32, len: u32) -> i32 {
    with_host(|h| {
        let Some(slot) = h.slot(handle) else {
            return h.bad_handle();
        };
        let Slot { player, input, .. } = slot;
        let len = len as usize;
        if len > input.len() {
            return h.set_error(&WebError::BadArgument(
                "commit longer than the staged input",
            ));
        }
        match player.push(&input[..len]) {
            Ok(()) => 0,
            Err(e) => h.set_error(&e),
        }
    })
}

/// Declare end of input. Returns `0` or a negative error.
#[no_mangle]
pub extern "C" fn tpt_web_finish(handle: u32) -> i32 {
    with_host(|h| match h.slot(handle) {
        Some(slot) => {
            slot.player.finish_input();
            0
        }
        None => h.bad_handle(),
    })
}

/// Decode until the output queues are full or input runs out. Returns the
/// status (`0` need input, `1` ready, `2` ended) or a negative error.
#[no_mangle]
pub extern "C" fn tpt_web_pump(handle: u32) -> i32 {
    with_host(|h| {
        let Some(slot) = h.slot(handle) else {
            return h.bad_handle();
        };
        match slot.player.pump() {
            Ok(s) => status_code(s),
            Err(e) => h.set_error(&e),
        }
    })
}

/// Current status without doing work: `0` need input, `1` ready, `2` ended,
/// `3` failed.
#[no_mangle]
pub extern "C" fn tpt_web_status(handle: u32) -> i32 {
    with_host(|h| match h.slot(handle) {
        Some(slot) => status_code(slot.player.status()),
        None => h.bad_handle(),
    })
}

/// Stream and player facts by numeric key (an `f64` so every counter fits
/// exactly). Unknown keys and bad handles return NaN.
///
/// | key | value |
/// | --: | --- |
/// | 0 | has audio (0/1) |
/// | 1 | audio sample rate (Hz) |
/// | 2 | audio channels (0 = not declared) |
/// | 3 | has video (0/1) |
/// | 4 | video width |
/// | 5 | video height |
/// | 6 | frame rate numerator |
/// | 7 | frame rate denominator |
/// | 8 | audio queued (ms) |
/// | 9 | video frames queued |
/// | 10 | bytes the player holds now |
/// | 11 | worst-case bytes under the limits |
/// | 12 | container bytes accepted |
/// | 13 | packets parsed |
/// | 14 | packets that failed to decode |
/// | 15 | clusters dropped (malformed or bad CRC) |
/// | 16 | outputs dropped by queue hard stops |
/// | 17 | audio samples per channel produced |
/// | 18 | video frames produced |
/// | 19 | input ended mid-chunk (0/1) |
/// | 20 | tracks ignored |
#[no_mangle]
pub extern "C" fn tpt_web_info(handle: u32, key: u32) -> f64 {
    with_host(|h| {
        let Some(slot) = h.slot(handle) else {
            h.bad_handle();
            return f64::NAN;
        };
        let p = &slot.player;
        let info = p.info();
        let st = p.stats();
        match key {
            0 => f64::from(u8::from(info.audio.is_some())),
            1 => info
                .audio
                .as_ref()
                .map_or(0.0, |a| f64::from(a.sample_rate)),
            2 => info
                .audio
                .as_ref()
                .and_then(|a| a.channels)
                .map_or(0.0, f64::from),
            3 => f64::from(u8::from(info.video.is_some())),
            4 => info.video.as_ref().map_or(0.0, |v| f64::from(v.width)),
            5 => info.video.as_ref().map_or(0.0, |v| f64::from(v.height)),
            6 => info.video.as_ref().map_or(0.0, |v| f64::from(v.fps_num)),
            7 => info.video.as_ref().map_or(0.0, |v| f64::from(v.fps_den)),
            8 => p.audio_queued_ms(),
            9 => p.video_queued() as f64,
            10 => (p.memory_in_use() + slot.input.capacity()) as f64,
            11 => p.limits().memory_ceiling_bytes() as f64,
            12 => st.parser.bytes_in as f64,
            13 => st.parser.packets as f64,
            14 => f64::from(st.decode_errors),
            15 => st.parser.dropped_clusters as f64,
            16 => st.dropped_outputs as f64,
            17 => st.audio_samples as f64,
            18 => st.video_frames as f64,
            19 => f64::from(u8::from(st.truncated_input)),
            20 => f64::from(info.ignored_tracks),
            _ => f64::NAN,
        }
    })
}

/// Move the next decoded audio chunk into the handle's "current audio" slot
/// and return its length in samples per channel (`0` = none queued).
#[no_mangle]
pub extern "C" fn tpt_web_audio_next(handle: u32) -> u32 {
    with_host(|h| {
        let Some(slot) = h.slot(handle) else {
            h.bad_handle();
            return 0;
        };
        slot.audio = slot.player.pop_audio();
        slot.audio.as_ref().map_or(0, |c| c.frames)
    })
}

/// Address of the current audio chunk's planar `f32` samples
/// (`channels * frames` values, channel-major), or null if none.
#[no_mangle]
pub extern "C" fn tpt_web_audio_ptr(handle: u32) -> *const f32 {
    with_host(|h| match h.slot(handle).and_then(|s| s.audio.as_ref()) {
        Some(c) => c.planar.as_ptr(),
        None => core::ptr::null(),
    })
}

/// Facts about the current audio chunk: key `0` = presentation time
/// (seconds), `1` = sample rate, `2` = channels. NaN if none.
#[no_mangle]
pub extern "C" fn tpt_web_audio_info(handle: u32, key: u32) -> f64 {
    with_host(|h| match h.slot(handle).and_then(|s| s.audio.as_ref()) {
        Some(c) => match key {
            0 => c.pts_seconds,
            1 => f64::from(c.sample_rate),
            2 => f64::from(c.channels),
            _ => f64::NAN,
        },
        None => f64::NAN,
    })
}

/// Presentation time (seconds) of the next queued picture, NaN if none.
/// Lets the page decide *whether* to take a frame before committing to it.
#[no_mangle]
pub extern "C" fn tpt_web_video_peek_pts(handle: u32) -> f64 {
    with_host(|h| {
        h.slot(handle)
            .and_then(|s| s.player.peek_video_pts())
            .unwrap_or(f64::NAN)
    })
}

/// Move the next picture into the "current video" slot; returns `1` if there
/// was one, `0` if not.
#[no_mangle]
pub extern "C" fn tpt_web_video_next(handle: u32) -> u32 {
    with_host(|h| {
        let Some(slot) = h.slot(handle) else {
            h.bad_handle();
            return 0;
        };
        slot.video = slot.player.pop_video();
        u32::from(slot.video.is_some())
    })
}

/// Address of the current picture's RGBA8 pixels (`width * height * 4`
/// bytes), or null if none.
#[no_mangle]
pub extern "C" fn tpt_web_video_ptr(handle: u32) -> *const u8 {
    with_host(|h| match h.slot(handle).and_then(|s| s.video.as_ref()) {
        Some(p) => p.rgba.as_ptr(),
        None => core::ptr::null(),
    })
}

/// Facts about the current picture: key `0` = presentation time (seconds),
/// `1` = width, `2` = height. NaN if none.
#[no_mangle]
pub extern "C" fn tpt_web_video_info(handle: u32, key: u32) -> f64 {
    with_host(|h| match h.slot(handle).and_then(|s| s.video.as_ref()) {
        Some(p) => match key {
            0 => p.pts_seconds(),
            1 => f64::from(p.width),
            2 => f64::from(p.height),
            _ => f64::NAN,
        },
        None => f64::NAN,
    })
}

/// Address of the UTF-8 message of the most recent failed call.
#[no_mangle]
pub extern "C" fn tpt_web_error_ptr() -> *const u8 {
    with_host(|h| h.error.as_ptr())
}

/// Length in bytes of the message at [`tpt_web_error_ptr`].
#[no_mangle]
pub extern "C" fn tpt_web_error_len() -> u32 {
    with_host(|h| h.error.len() as u32)
}

#[cfg(all(test, feature = "synth"))]
mod tests {
    use super::*;
    use crate::synth;

    fn error_string() -> String {
        let (p, n) = (tpt_web_error_ptr(), tpt_web_error_len());
        unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(p, n as usize)).into_owned() }
    }

    /// Push `data` the way `tpt-web.js` does: stage, write, commit, and on
    /// `-2` pump and retry.
    fn feed(h: u32, data: &[u8], slice: usize) {
        for chunk in data.chunks(slice) {
            loop {
                let p = tpt_web_input_ptr(h, chunk.len() as u32);
                assert!(!p.is_null(), "{}", error_string());
                unsafe { std::slice::from_raw_parts_mut(p, chunk.len()) }.copy_from_slice(chunk);
                match tpt_web_input_commit(h, chunk.len() as u32) {
                    0 => break,
                    -2 => {
                        assert!(tpt_web_pump(h) >= 0);
                        while tpt_web_audio_next(h) != 0 {}
                        while tpt_web_video_next(h) != 0 {}
                    }
                    e => panic!("commit failed: {e} {}", error_string()),
                }
            }
        }
    }

    #[test]
    fn full_lifecycle_through_the_c_abi() {
        assert_eq!(tpt_web_abi_version(), ABI_VERSION);
        let file = synth::av_tpt(synth::AudioKind::Cinder, 48_000, 0.5, 32, 32, 10);
        let h = tpt_web_open(0, 0, 0, 0);
        assert_ne!(h, 0);
        feed(h, &file, 1000);
        assert_eq!(tpt_web_finish(h), 0);

        let (mut samples, mut pictures) = (0u64, 0u64);
        let mut first_pts = None;
        loop {
            let st = tpt_web_pump(h);
            assert!(st >= 0, "{}", error_string());
            loop {
                let n = tpt_web_audio_next(h);
                if n == 0 {
                    break;
                }
                let ch = tpt_web_audio_info(h, 2) as usize;
                assert_eq!(ch, 1);
                assert_eq!(tpt_web_audio_info(h, 1), 48_000.0);
                let ptr = tpt_web_audio_ptr(h);
                assert!(!ptr.is_null());
                let data = unsafe { std::slice::from_raw_parts(ptr, ch * n as usize) };
                assert!(data.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
                samples += u64::from(n);
            }
            loop {
                let pts = tpt_web_video_peek_pts(h);
                if tpt_web_video_next(h) == 0 {
                    assert!(pts.is_nan());
                    break;
                }
                assert_eq!(tpt_web_video_info(h, 1), 32.0);
                assert_eq!(tpt_web_video_info(h, 2), 32.0);
                let ptr = tpt_web_video_ptr(h);
                let px = unsafe { std::slice::from_raw_parts(ptr, 32 * 32 * 4) };
                assert!(px.chunks_exact(4).all(|p| p[3] == 255));
                first_pts.get_or_insert(tpt_web_video_info(h, 0));
                pictures += 1;
            }
            if st == 2 {
                break;
            }
        }
        // 0.5 s at 48 kHz is exactly 24 000 samples after padding trim, and
        // 5 frames at 10 fps.
        assert_eq!(samples, 24_000);
        assert_eq!(pictures, 5);
        assert_eq!(first_pts, Some(0.0));
        assert_eq!(tpt_web_info(h, 0), 1.0);
        assert_eq!(tpt_web_info(h, 3), 1.0);
        assert_eq!(tpt_web_info(h, 4), 32.0);
        assert!(tpt_web_info(h, 10) <= tpt_web_info(h, 11));
        assert!(tpt_web_info(h, 999).is_nan());
        tpt_web_close(h);
        // A closed handle is rejected, not undefined behaviour.
        assert_eq!(tpt_web_status(h), ERR_BAD_HANDLE);
        assert_eq!(error_string(), "invalid handle");
    }

    #[test]
    fn bad_handles_and_oversized_slices_are_errors_not_crashes() {
        assert_eq!(tpt_web_pump(0), ERR_BAD_HANDLE);
        assert_eq!(tpt_web_pump(9999), ERR_BAD_HANDLE);
        assert!(tpt_web_input_ptr(0, 10).is_null());
        assert_eq!(tpt_web_input_commit(0, 10), ERR_BAD_HANDLE);
        assert!(tpt_web_audio_ptr(0).is_null());
        assert!(tpt_web_video_ptr(0).is_null());
        assert!(tpt_web_info(0, 0).is_nan());
        tpt_web_close(0);
        tpt_web_close(u32::MAX);

        let h = tpt_web_open(0, 0, 0, 0);
        assert!(tpt_web_input_ptr(h, u32::MAX).is_null());
        assert_eq!(tpt_web_input_commit(h, 5), -7); // nothing staged
        tpt_web_close(h);
    }

    #[test]
    fn player_count_is_capped_and_slots_are_reused() {
        let hs: Vec<u32> = (0..MAX_PLAYERS).map(|_| tpt_web_open(0, 0, 0, 0)).collect();
        assert!(hs.iter().all(|&h| h != 0));
        assert_eq!(tpt_web_open(0, 0, 0, 0), 0);
        tpt_web_close(hs[1]);
        assert_eq!(tpt_web_open(0, 0, 0, 0), hs[1]);
        for h in hs {
            tpt_web_close(h);
        }
    }

    #[test]
    fn hostile_bytes_yield_an_error_code_and_message() {
        let h = tpt_web_open(0, 0, 0, 0);
        feed(h, b"this is definitely not a tpt file", 8);
        let code = tpt_web_pump(h);
        assert_eq!(code, -3, "{}", error_string());
        assert!(error_string().contains("container"));
        assert_eq!(tpt_web_status(h), 3);
        tpt_web_close(h);
    }

    #[test]
    fn open_limits_are_clamped_to_the_ceiling() {
        let h = tpt_web_open(u32::MAX, u32::MAX, u32::MAX, u32::MAX);
        assert_ne!(h, 0);
        assert!(
            tpt_web_info(h, 11) <= WebLimits::ceiling().memory_ceiling_bytes() as f64,
            "ceiling {}",
            tpt_web_info(h, 11)
        );
        tpt_web_close(h);
    }
}
