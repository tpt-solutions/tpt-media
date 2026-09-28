//! Peak-memory measurement harness (todo.md §42, "allocation profiling"
//! and "peak memory measurements").
//!
//! Each codec's `realtime_alloc.rs` test carries its own counting global
//! allocator. That is the right tool for "did this path allocate too much
//! relative to a generous bound", but it is not reusable, it is copied
//! four times, and an allocation *count* says nothing about *how many
//! bytes* were live at the worst moment — which is the number a working-set
//! budget is actually written in. This module factors the mechanism out so
//! any test binary can install one allocator and ask the sharper question.
//!
//! # Usage
//!
//! A global allocator can only be declared once per binary, so a test
//! binary installs [`TrackingAlloc`] once at module scope and then takes a
//! [`MemoryProbe`] around the region it cares about:
//!
//! ```ignore
//! use tpt_cli::memprobe::{MemoryProbe, TrackingAlloc};
//!
//! #[global_allocator]
//! static GLOBAL: TrackingAlloc = TrackingAlloc;
//! ```
//!
//! [`crate::quality::MemoryReport`] is the result type, so a measurement
//! drops straight into the same report structure the quality metrics use.
//!
//! # What is measured
//!
//! `peak_bytes` is the high-water mark of *live* heap bytes (allocations
//! minus frees) during the window, and `total_bytes` is the cumulative
//! sum of every allocation. The peak is the working-set number; the total
//! is churn, and is what the per-frame allocation count was standing in
//! for before.
//!
//! Only [`System`]'s own allocate/deallocate/reallocate
//! entry points are intercepted. Memory obtained any other way (a `static`,
//! a stack array, an `mmap` behind some third-party crate) is invisible
//! here — which is the standard caveat for this kind of harness, not a
//! claim that it is exact.
//!
//! # Fidelity caveat
//!
//! Cargo runs the tests in a binary on parallel threads, and *their*
//! allocations are indistinguishable from the ones under measurement. The
//! probe therefore takes a process-wide lock ([`MemoryProbe::start`]
//! blocks until any other probe has finished) so that measurements never
//! interleave with each other, but a binary that mixes memory tests with
//! unrelated tests will still see foreign allocations inside its window.
//! Keep memory assertions in a dedicated test binary
//! (`apps/tpt-cli/tests/memory_budget.rs` does exactly this) and treat the
//! result as order-of-magnitude evidence, not a byte-exact figure.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cmp::Ordering as Cmp;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use crate::quality::MemoryReport;

/// Bytes live in the heap right now, as far as the harness can tell.
static CURRENT: AtomicU64 = AtomicU64::new(0);
/// High-water mark of [`CURRENT`] within the current window.
static PEAK: AtomicU64 = AtomicU64::new(0);
/// Cumulative bytes allocated within the current window.
static TOTAL: AtomicU64 = AtomicU64::new(0);
/// Whether the counters are live at all. Off means the allocator is a
/// plain pass-through, so an unprobed program pays only a relaxed load.
static TRACKING: AtomicBool = AtomicBool::new(false);

/// Serialises probes so two can never measure the same instant.
///
/// Held for the probe's whole lifetime. Because [`MutexGuard`] is `!Send`,
/// a [`MemoryProbe`] also cannot be moved to another thread, which is what
/// makes "one probe at a time" enforceable rather than merely
/// conventional.
static PROBE_LOCK: Mutex<()> = Mutex::new(());

/// Record `size` bytes that have just become live.
#[inline]
fn on_alloc(size: usize) {
    let size = size as u64;
    let live = CURRENT.fetch_add(size, Ordering::Relaxed) + size;
    TOTAL.fetch_add(size, Ordering::Relaxed);
    PEAK.fetch_max(live, Ordering::Relaxed);
}

/// Record `size` bytes that have just stopped being live.
///
/// Saturating, unlike the plain `fetch_sub` the older per-codec harnesses
/// use: a buffer allocated *before* the window opened can be freed inside
/// it, and an unsigned counter would wrap to near-`u64::MAX` and poison
/// every subsequent number in the window. Saturating keeps a stale free
/// from turning into a nonsense peak.
#[inline]
fn on_dealloc(size: usize) {
    let size = size as u64;
    let _ = CURRENT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
        Some(live.saturating_sub(size))
    });
}

/// A [`GlobalAlloc`] that mirrors live heap bytes into the counters above.
///
/// Zero-cost when no probe is open. Install it once per test binary:
///
/// ```ignore
/// tpt_cli::memprobe::TrackingAlloc
/// ```
///
/// This is the one type in `tpt-cli` that contains `unsafe` (a
/// `GlobalAlloc` impl is `unsafe` by definition). It is confined here on
/// purpose — see the module docs.
pub struct TrackingAlloc;

unsafe impl GlobalAlloc for TrackingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() && TRACKING.load(Ordering::Relaxed) {
            on_alloc(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if TRACKING.load(Ordering::Relaxed) {
            on_dealloc(layout.size());
        }
        System.dealloc(ptr, layout);
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc_zeroed(layout);
        if !ptr.is_null() && TRACKING.load(Ordering::Relaxed) {
            on_alloc(layout.size());
        }
        ptr
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let out = System.realloc(ptr, layout, new_size);
        if !out.is_null() && TRACKING.load(Ordering::Relaxed) {
            // Only the *delta* crosses the counter: the original bytes were
            // already counted when they were first allocated.
            match new_size.cmp(&layout.size()) {
                Cmp::Greater => on_alloc(new_size - layout.size()),
                Cmp::Less => on_dealloc(layout.size() - new_size),
                Cmp::Equal => {}
            }
        }
        out
    }
}

/// An open measurement window over heap allocation.
///
/// Created by [`MemoryProbe::start`], which blocks until any other probe
/// in the process has been dropped, and closed by [`MemoryProbe::stop`] or
/// by being dropped. A panicking test therefore still closes its window.
pub struct MemoryProbe {
    /// Held for the window's lifetime; see [`PROBE_LOCK`].
    _guard: MutexGuard<'static, ()>,
    /// Cleared by [`MemoryProbe::stop`] so `Drop` does not clear twice.
    armed: bool,
}

impl MemoryProbe {
    /// Open a window: wait for exclusive access, zero the counters, and
    /// start tracking.
    ///
    /// Blocks until any other [`MemoryProbe`] in the process is dropped, so
    /// measurements never interleave. A lock poisoned by an unrelated
    /// panicking test is recovered from rather than propagated — one
    /// failing test should not wedge every later measurement.
    pub fn start() -> Self {
        let guard = PROBE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        CURRENT.store(0, Ordering::Relaxed);
        PEAK.store(0, Ordering::Relaxed);
        TOTAL.store(0, Ordering::Relaxed);
        TRACKING.store(true, Ordering::Relaxed);
        Self {
            _guard: guard,
            armed: true,
        }
    }

    /// What the window has seen so far, without closing it.
    pub fn report(&self) -> MemoryReport {
        MemoryReport {
            peak_bytes: PEAK.load(Ordering::Relaxed),
            total_bytes: TOTAL.load(Ordering::Relaxed),
        }
    }

    /// Close the window and return what it saw.
    pub fn stop(mut self) -> MemoryReport {
        self.armed = false;
        TRACKING.store(false, Ordering::Relaxed);
        self.report()
    }

    /// Run `f` under a probe and return its value alongside the report.
    ///
    /// The common shape: measure one pass of a codec.
    pub fn measure<T>(f: impl FnOnce() -> T) -> (T, MemoryReport) {
        let probe = Self::start();
        let value = f();
        let report = probe.stop();
        (value, report)
    }
}

impl Drop for MemoryProbe {
    fn drop(&mut self) {
        // An open window must never outlive its probe, or the rest of the
        // process runs with its allocations counted. `armed` is only false
        // when `stop` already did this.
        if self.armed {
            TRACKING.store(false, Ordering::Relaxed);
        }
    }
}
