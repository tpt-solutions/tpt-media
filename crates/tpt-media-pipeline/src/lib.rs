//! `tpt-media-pipeline`: the stream-processing core of the TPT media
//! toolchain.
//!
//! A media pipeline is a **linear, typed** composition of [`Node`]
//! stages:
//!
//! ```text
//!          +--------+   +--------+   +----------+   +-------+
//!  pull () -> Source ->  demux   ->  transform  ->  sink  ->
//!          +--------+   +--------+   +----------+   +-------+
//!            packets       packets      media          (collected)
//! ```
//!
//! [`Node`] is the processing unit (one input consumed, at most one output
//! per call). [`chain::Chain`] composes stages with a lockstep, bounded-by-
//! one coupling, and [`Pipeline`] drives a composed pullable graph.
//!
//! The crate defines the twelve core pipeline concepts, each mapped to code:
//!
//! 1. **Node abstraction** — [`Node`], [`NodeEvent`].
//! 2. **Stream negotiation** — [`negotiation::StreamSpec`],
//!    [`negotiation::negotiate_stream`].
//! 3. **Format negotiation** — [`negotiation::MediaFormat`],
//!    [`negotiation::FormatAware`], [`negotiation::negotiate_formats`].
//! 4. **Backpressure** — [`NodeEvent::NeedMore`] as the soft signal, plus
//!    a stall bound in [`Pipeline::pull`] that reports
//!    [`NodeError::Backpressure`].
//! 5. **Buffering** — [`nodes::Bounded`] (batching + hard cap).
//! 6. **Cancellation** — [`Node::cancel`] + [`nodes::Cancellable`],
//!    surfaced as [`NodeError::Cancelled`].
//! 7. **Error propagation** — [`NodeError`]; a node error aborts the run
//!    and is surfaced to the caller unchanged.
//! 8. **Flushing** — [`Node::flush`] drains internal buffers after EOS
//!    (see [`Pipeline::pull`]'s tail handling).
//! 9. **End-of-stream** — [`NodeEvent::Eos`] is one-shot; subsequent pulls
//!    return `Ok(None)`.
//! 10. **Timestamps through the pipeline** — [`nodes::Timestamped`]
//!     (`pts`/`dts`/`duration` survive payload transforms); container
//!     `Packet`s flow through unchanged.
//! 11. **Threading model** — [`Executor`]: [`Inline`] (deterministic
//!     default) and [`ThreadedExecutor`] (std-only worker thread). The
//!     full stop-free multi-stage model is described in
//!     `docs/pipeline-design.md`.
//! 12. **Realtime scheduling profile** — [`RealtimeProfile`],
//!     [`Pipeline::drive_scheduled`], [`LatencyObserver`],
//!     [`LatencyLog`], [`ScheduledRun`].
//!
//! The crate is `no_std`-compatible (with `alloc`); the `std` feature
//! (default) additionally enables the threaded executor.

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
extern crate alloc;

mod chain;
mod negotiation;
mod node;
mod nodes;
mod pipeline;
mod sched;

pub use chain::{Chain, NodeOps};
pub use negotiation::{negotiate_formats, negotiate_stream, FormatAware, MediaFormat, StreamSpec};
pub use node::{Node, NodeError, NodeEvent};
pub use nodes::{Bounded, Cancellable, Map, Sink, Source, Timestamped};
pub use pipeline::{Executor, Inline, Pipeline, STALL_BOUND};
pub use sched::{Clock, LatencyLog, LatencyObserver, RealtimeProfile, ScheduledRun};

#[cfg(feature = "std")]
pub use pipeline::ThreadedExecutor;

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use core::cell::Cell;
    use tpt_media_core::{
        ChannelLayout, CodecId, Duration, MediaType, PixelFormat, SampleFormat, Timebase, Timestamp,
    };

    fn timebase() -> Timebase {
        Timebase { num: 1, den: 1000 }
    }

    fn ts(ticks: i64) -> Timestamp {
        Timestamp {
            ticks,
            timebase: timebase(),
        }
    }

    mod negotiation_tests {
        use super::*;

        struct AudioProducer;
        struct AudioConsumer;
        struct VideoConsumer;
        struct Anything;
        struct Nothing;

        const AUDIO: MediaFormat = MediaFormat::Audio {
            sample_format: SampleFormat::F32,
            channel_layout: ChannelLayout::STEREO,
            sample_rate: 48_000,
        };
        const AUDIO_FMTS: &[MediaFormat] = &[AUDIO];
        const VIDEO: MediaFormat = MediaFormat::Video {
            pixel_format: PixelFormat::Rgb24,
            width: 640,
            height: 480,
        };
        const VIDEO_FMTS: &[MediaFormat] = &[VIDEO];

        fn audio_fmt() -> MediaFormat {
            AUDIO
        }

        impl FormatAware for AudioProducer {
            fn output_formats(&self) -> Option<&[MediaFormat]> {
                Some(AUDIO_FMTS)
            }
        }
        impl FormatAware for AudioConsumer {
            fn input_formats(&self) -> Option<&[MediaFormat]> {
                Some(AUDIO_FMTS)
            }
        }
        impl FormatAware for VideoConsumer {
            fn input_formats(&self) -> Option<&[MediaFormat]> {
                Some(VIDEO_FMTS)
            }
        }
        impl FormatAware for Anything {}
        impl FormatAware for Nothing {
            fn input_formats(&self) -> Option<&[MediaFormat]> {
                Some(&[])
            }
            fn output_formats(&self) -> Option<&[MediaFormat]> {
                Some(&[])
            }
        }

        #[test]
        fn matching_formats_negotiate() {
            assert_eq!(
                negotiate_formats(&AudioProducer, &AudioConsumer).unwrap(),
                audio_fmt()
            );
        }

        #[test]
        fn mismatched_formats_fail() {
            assert_eq!(
                negotiate_formats(&AudioProducer, &VideoConsumer).unwrap_err(),
                NodeError::FormatMismatch
            );
        }

        #[test]
        fn unconstrained_side_passes_through() {
            assert_eq!(
                negotiate_formats(&AudioProducer, &Anything).unwrap(),
                audio_fmt()
            );
            assert_eq!(
                negotiate_formats(&Anything, &AudioConsumer).unwrap(),
                audio_fmt()
            );
        }

        #[test]
        fn both_unknown_or_nothing_fail() {
            assert_eq!(
                negotiate_formats(&Anything, &Anything).unwrap_err(),
                NodeError::FormatMismatch
            );
            assert_eq!(
                negotiate_formats(&AudioProducer, &Nothing).unwrap_err(),
                NodeError::FormatMismatch
            );
        }

        #[test]
        fn stream_spec_merges_matching_timebases() {
            let a = StreamSpec {
                timebase: Some(timebase()),
                has_audio: true,
                has_video: false,
            };
            let merged = negotiate_stream(&a, &a).expect("same timebase");
            assert_eq!(merged, a);
        }

        #[test]
        fn stream_spec_rejects_conflicting_timebases() {
            let a = StreamSpec {
                timebase: Some(timebase()),
                has_audio: true,
                has_video: false,
            };
            let b = StreamSpec {
                timebase: Some(Timebase {
                    num: 1,
                    den: 90_000,
                }),
                has_audio: true,
                has_video: false,
            };
            assert_eq!(
                negotiate_stream(&a, &b).unwrap_err(),
                NodeError::FormatMismatch
            );
        }
    }

    // A never-ending source used to exercise the stall guard.
    struct Looping;
    impl Node for Looping {
        type Input = ();
        type Output = i32;
        fn process(&mut self, _: ()) -> Result<NodeEvent<i32>, NodeError> {
            Ok(NodeEvent::Item(1))
        }
    }

    // A test node that asks for more input without ever making progress.
    struct Blocker;
    impl Node for Blocker {
        type Input = i32;
        type Output = i32;
        fn process(&mut self, _: i32) -> Result<NodeEvent<i32>, NodeError> {
            Ok(NodeEvent::NeedMore)
        }
    }

    // A fault-injection node that rejects items over a threshold.
    struct OverTen;
    impl Node for OverTen {
        type Input = i32;
        type Output = i32;
        fn process(&mut self, input: i32) -> Result<NodeEvent<i32>, NodeError> {
            if input > 9 {
                Err(NodeError::BufferOverflow)
            } else {
                Ok(NodeEvent::Item(input))
            }
        }
    }

    #[test]
    fn pull_pump_transforms_and_drives() {
        let mut p = Source::new(vec![1, 2, 3, 4]).map(|x| x * x).into_pipeline();
        assert_eq!(p.drive(16).unwrap(), vec![1, 4, 9, 16]);
    }

    #[test]
    fn source_from_range() {
        let mut p = Source::from_range(3..=5i32).into_pipeline();
        assert_eq!(p.drive(16).unwrap(), vec![3, 4, 5]);
    }

    #[test]
    fn sink_collects_the_stream() {
        let mut p = Source::new(vec![1, 2, 3]).then(Sink::new()).into_pipeline();
        let _out = p.drive(16).expect("run completes");
        let c = p.core();
        assert_eq!(c.right().items(), &[1, 2, 3]);
    }

    #[test]
    fn buffered_stage_batches_then_drains_tail() {
        let mut p = Source::new(vec![1, 2, 3, 4, 5])
            .then(Bounded::with_window(3))
            .map(|x| x * 10)
            .into_pipeline();
        assert_eq!(p.drive(16).unwrap(), vec![10, 20, 30, 40, 50]);
    }

    #[test]
    fn backpressure_is_absorbed_by_the_driver() {
        let mut p = Source::new(vec![9, 8, 7, 6])
            .then(Bounded::with_window(2))
            .into_pipeline();
        // First pull pumps two source items (two NeedMore steps) then
        // emits; the stall is absorbed inside pull().
        assert_eq!(p.pull(16).unwrap(), Some(9));
        assert_eq!(p.pull(16).unwrap(), Some(8));
    }

    #[test]
    fn stalled_pipeline_is_reported_as_backpressure() {
        let mut p = Looping.then(Blocker).into_pipeline();
        assert_eq!(p.pull(3).unwrap_err(), NodeError::Backpressure);
    }

    #[test]
    fn overflow_propagates_as_an_error() {
        let mut p = Source::new(vec![1, 11, 2]).then(OverTen).into_pipeline();
        assert_eq!(p.drive(16).unwrap_err(), NodeError::BufferOverflow);
    }

    #[test]
    fn cancellation_of_a_mid_node_is_propagated() {
        let mut p = Source::new(vec![1, 2, 3, 4])
            .map(|x| x * 2)
            .then(Cancellable::new())
            .into_pipeline();
        assert_eq!(p.pull(16).unwrap(), Some(2));
        p.core_mut().right_mut().cancel();
        assert_eq!(p.pull(16).unwrap_err(), NodeError::Cancelled);
    }

    #[test]
    fn cancellation_of_the_whole_chain() {
        let mut p = Source::new(vec![1, 2, 3, 4]).map(|x| x * 2).into_pipeline();
        assert_eq!(p.pull(16).unwrap(), Some(2));
        p.core_mut().cancel();
        assert_eq!(p.pull(16).unwrap_err(), NodeError::Cancelled);
    }

    #[test]
    fn eos_is_one_shot_and_idempotent() {
        let mut p = Source::new(vec![1, 2]).into_pipeline();
        assert_eq!(p.pull(16).unwrap(), Some(1));
        assert_eq!(p.pull(16).unwrap(), Some(2));
        assert_eq!(p.pull(16).unwrap(), None);
        assert_eq!(p.pull(16).unwrap(), None);
    }

    #[test]
    fn timestamps_survive_transforms() {
        let input: Vec<Timestamped<i32>> = (0..4)
            .map(|i| Timestamped::new(ts(i * 100), i as i32))
            .collect();
        let mut p = Source::new(input).map(|t| t.map(|v| v * 3)).into_pipeline();
        let out = p.drive(16).unwrap();
        let pts: Vec<i64> = out.iter().map(|t| t.pts.ticks).collect();
        assert_eq!(pts, vec![0, 100, 200, 300]);
        let items: Vec<i32> = out.iter().map(|t| t.item).collect();
        assert_eq!(items, vec![0, 3, 6, 9]);
    }

    #[test]
    fn timestamps_with_duration_flow_through() {
        let mut t = Timestamped::new(ts(5), 7u32);
        t.duration = Some(Duration {
            ticks: 2,
            timebase: timebase(),
        });
        let mapped = t.map(|v| v + 1);
        assert_eq!(mapped.pts.ticks, 5);
        assert_eq!(mapped.duration.unwrap().ticks, 2);
        assert_eq!(mapped.item, 8);
    }

    #[test]
    fn inline_executor_matches_drive() {
        let mut p = Source::new(vec![1, 2, 3]).map(|x| x * x).into_pipeline();
        assert_eq!(p.drive(16).unwrap(), vec![1, 4, 9]);

        let q = Source::new(vec![1, 2, 3]).map(|x| x * x).into_pipeline();
        assert_eq!(q.execute(&Inline, 16).unwrap(), vec![1, 4, 9]);
    }

    #[test]
    fn scheduled_run_measures_latency_and_misses() {
        struct StepClock {
            base: u64,
            step: u64,
            count: Cell<u64>,
        }
        impl Clock for StepClock {
            fn now_ns(&self) -> u64 {
                let c = self.count.get();
                self.count.set(c + 1);
                self.base + c * self.step
            }
        }

        let mut p = Source::new(vec![1, 2, 3, 4]).into_pipeline();
        let mut log = LatencyLog::default();
        let profile = RealtimeProfile {
            max_latency_ns: 100,
            stall_bound: 16,
        };
        let clock = StepClock {
            base: 0,
            step: 100,
            count: Cell::new(0),
        };
        let run = p.drive_scheduled(&profile, &clock, &mut log).unwrap();
        assert_eq!(run.items, 4);
        assert_eq!(run.deadline_misses, 0);
        assert_eq!(log.samples(), &[100, 100, 100, 100]);

        // Tighter budget makes every item a miss.
        let mut p2 = Source::new(vec![1, 2, 3]).into_pipeline();
        let tight = RealtimeProfile {
            max_latency_ns: 99,
            stall_bound: 16,
        };
        let clock2 = StepClock {
            base: 0,
            step: 100,
            count: Cell::new(0),
        };
        let run2 = p2.drive_scheduled(&tight, &clock2, &mut log).unwrap();
        assert_eq!(run2.items, 3);
        assert_eq!(run2.deadline_misses, 3);
    }

    #[cfg(feature = "std")]
    #[test]
    fn threaded_executor_offloads_to_a_worker() {
        let p = Source::new(vec![1i32, 2, 3]).map(|x| x * x).into_pipeline();
        let q = p;
        assert_eq!(q.execute(&ThreadedExecutor, 16).unwrap(), vec![1, 4, 9]);
    }

    // --- §38 unit-test gate: the categories that had no coverage ----------

    #[test]
    // `5..3` is deliberately reversed: an inverted range is the case under
    // test, and clippy is right that it is empty — which is the point.
    #[allow(clippy::reversed_empty_ranges)]
    fn empty_range_yields_no_items() {
        // Every empty range used to yield exactly one item, because the
        // first value was pushed before the bounds were compared.
        assert_eq!(Source::from_range(1..1i32).consumed(), 0);
        let mut p = Source::from_range(1..1i32).into_pipeline();
        assert_eq!(p.drive(16).unwrap(), Vec::<i32>::new());

        // Inverted range: start above end.
        let mut q = Source::from_range(5..3i32).into_pipeline();
        assert_eq!(q.drive(16).unwrap(), Vec::<i32>::new());
    }

    #[test]
    fn exclusive_end_does_not_underflow_at_the_smallest_value() {
        // `..0u8` is `RangeTo`, i.e. an *excluded* end of 0. Forming the
        // inclusive bound as `0 - 1` underflowed: a debug panic, and a
        // silent wrap to 255 in release — 256 items where there are none.
        let mut p = Source::from_range(..0u8).into_pipeline();
        assert_eq!(p.drive(16).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn ranges_ending_at_the_largest_value_do_not_overflow() {
        // The inclusive loop must stop *before* stepping past the maximum,
        // or `255 + 1` panics in debug and wraps in release.
        let mut p = Source::from_range(0u8..=255u8).into_pipeline();
        let out = p.drive(STALL_BOUND).unwrap();
        assert_eq!(out.len(), 256);
        assert_eq!(out[0], 0);
        assert_eq!(out[255], 255);

        // And the exclusive form stops one short of it.
        let mut q = Source::from_range(0u8..255u8).into_pipeline();
        assert_eq!(q.drive(STALL_BOUND).unwrap().len(), 255);
    }

    #[test]
    fn exclusive_start_bound_is_honoured() {
        // `(Excluded(0), Included(2))` is `0..=2` minus the zero: [1, 2].
        let mut p = Source::from_range((
            core::ops::Bound::Excluded(0u8),
            core::ops::Bound::Included(2u8),
        ))
        .into_pipeline();
        assert_eq!(p.drive(16).unwrap(), vec![1, 2]);

        // An excluded start that excludes everything the end allows.
        let mut q = Source::from_range((
            core::ops::Bound::Excluded(2u8),
            core::ops::Bound::Included(2u8),
        ))
        .into_pipeline();
        assert_eq!(q.drive(16).unwrap(), Vec::<u8>::new());
    }

    #[test]
    #[should_panic(expected = "end bound must be bounded")]
    fn unbounded_end_is_rejected_rather_than_silently_short() {
        // A `Source` is finite by construction. An unbounded end used to
        // mean "hi = 0", so `from_range(0..)` produced the single item
        // `[0]` and `from_range(5..)` produced `[5]` — an arbitrary answer
        // that looks like a working pipeline until it under-delivers.
        let _ = Source::<i32>::from_range(..);
    }

    #[test]
    #[should_panic(expected = "end bound must be bounded")]
    fn unbounded_end_with_a_start_is_rejected_too() {
        let _ = Source::from_range(3i32..);
    }
    #[cfg(feature = "std")]
    #[test]
    fn threaded_executor_reports_a_worker_panic_as_an_error() {
        // A panicking node on the worker thread must come back as a
        // `NodeError`, not unwind into the caller. This is the only
        // mapping of its kind in the crate, so it is pinned here.
        let p = Source::new(vec![1i32, 2])
            .map(|_| -> i32 {
                panic!("node exploded");
            })
            .into_pipeline();
        assert_eq!(
            p.execute(&ThreadedExecutor, 16).unwrap_err(),
            NodeError::Internal("pipeline worker panicked")
        );
    }

    #[test]
    fn source_reports_what_it_has_consumed() {
        let mut p = Source::new(vec![7, 8, 9]).into_pipeline();
        assert_eq!(p.core().consumed(), 0);
        p.pull(16).unwrap();
        p.pull(16).unwrap();
        assert_eq!(p.core().consumed(), 2);
        // The cursor does not advance past exhaustion.
        assert_eq!(p.pull(16).unwrap(), Some(9));
        assert_eq!(p.pull(16).unwrap(), None);
        assert_eq!(p.core().consumed(), 3);
    }

    #[test]
    fn empty_source_is_immediately_end_of_stream() {
        let mut p = Source::new(Vec::<i32>::new()).into_pipeline();
        assert_eq!(p.pull(16).unwrap(), None);
        assert_eq!(p.core().consumed(), 0);
    }

    #[test]
    #[should_panic(expected = "window must be at least 1")]
    fn bounded_rejects_a_zero_window() {
        let _ = Bounded::<i32>::new(0, 4);
    }

    #[test]
    #[should_panic(expected = "cap must be >= window")]
    fn bounded_rejects_a_cap_below_its_window() {
        // Otherwise the node can never reach its window and every item
        // after the cap would overflow — a permanently broken stage that
        // still looks configured.
        let _ = Bounded::<i32>::new(4, 2);
    }

    #[test]
    fn bounded_cap_is_unreachable_in_a_linear_chain() {
        // The cap is a structural guard, not a live limit: one input in,
        // at most one output out, so the queue never exceeds `window`.
        let mut p = Source::new(vec![1, 2, 3, 4, 5, 6])
            .then(Bounded::new(2, 2))
            .map(|x| x * 10)
            .into_pipeline();
        assert_eq!(p.drive(16).unwrap(), vec![10, 20, 30, 40, 50, 60]);

        // Same window with a generous cap: identical output, no overflow.
        let mut q = Source::new(vec![1, 2, 3])
            .then(Bounded::new(1, 64))
            .into_pipeline();
        assert_eq!(q.drive(16).unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn bounded_reports_its_fill_level() {
        let mut b = Bounded::<i32>::new(2, 2);
        assert!(b.is_empty());
        assert_eq!(b.len(), 0);
        // One item: still filling the window, so it is retained.
        assert!(matches!(b.process(1), Ok(NodeEvent::NeedMore)));
        assert!(!b.is_empty());
        assert_eq!(b.len(), 1);
        // The second reaches the window and is emitted.
        assert!(matches!(b.process(2), Ok(NodeEvent::Item(1))));
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn flush_drains_a_partially_filled_buffer_then_reports_eos() {
        let mut b = Bounded::<i32>::with_window(4);
        b.process(1).unwrap();
        b.process(2).unwrap();
        assert_eq!(b.len(), 2);
        assert_eq!(b.flush().unwrap(), NodeEvent::Item(1));
        assert_eq!(b.flush().unwrap(), NodeEvent::Item(2));
        assert!(b.is_empty());
        assert_eq!(b.flush().unwrap(), NodeEvent::Eos);
    }

    #[test]
    fn cancelled_node_rejects_flush_as_well_as_process() {
        // Cancellation must survive the end-of-stream flush path, or a
        // cancelled pipeline keeps draining buffered items after EOS.
        let mut b = Bounded::<i32>::with_window(4);
        b.process(1).unwrap();
        b.cancel();
        assert_eq!(b.process(2).unwrap_err(), NodeError::Cancelled);
        assert_eq!(b.flush().unwrap_err(), NodeError::Cancelled);
    }

    #[test]
    fn media_format_compatibility_covers_every_variant() {
        let audio = MediaFormat::Audio {
            sample_format: SampleFormat::S16,
            channel_layout: ChannelLayout::MONO,
            sample_rate: 48_000,
        };
        let video = MediaFormat::Video {
            pixel_format: PixelFormat::Yuv420,
            width: 320,
            height: 240,
        };
        let packets = MediaFormat::Compressed {
            media_type: MediaType::Audio,
            codec_id: CodecId::new(*b"CIND"),
        };
        let opaque = MediaFormat::Opaque("app-defined");

        for (i, f) in [&audio, &video, &packets, &opaque].into_iter().enumerate() {
            assert!(f.compatible(f), "{f:?} must be compatible with itself");
            for (j, other) in [&audio, &video, &packets, &opaque].into_iter().enumerate() {
                if i == j {
                    continue;
                }
                assert!(!f.compatible(other), "{f:?} vs {other:?}");
            }
        }

        // A one-field difference is enough to make two video formats
        // incompatible — the comparison is structural, not "both video".
        let smaller = MediaFormat::Video {
            pixel_format: PixelFormat::Yuv420,
            width: 160,
            height: 240,
        };
        assert!(!video.compatible(&smaller));
    }

    struct TwoFormatProducer;
    struct WideConsumer;
    const WIDE: &[MediaFormat] = &[
        MediaFormat::Compressed {
            media_type: MediaType::Audio,
            codec_id: CodecId::new(*b"REED"),
        },
        MediaFormat::Compressed {
            media_type: MediaType::Audio,
            codec_id: CodecId::new(*b"CIND"),
        },
    ];
    impl FormatAware for TwoFormatProducer {
        fn output_formats(&self) -> Option<&[MediaFormat]> {
            Some(WIDE)
        }
    }
    impl FormatAware for WideConsumer {
        fn input_formats(&self) -> Option<&[MediaFormat]> {
            Some(&WIDE[1..])
        }
    }

    #[test]
    fn negotiation_picks_the_first_format_the_right_side_accepts() {
        // The producer offers REED then CIND; the consumer accepts only
        // CIND. Negotiation must search, not just take the head.
        assert_eq!(
            negotiate_formats(&TwoFormatProducer, &WideConsumer).unwrap(),
            WIDE[1]
        );

        // A consumer that accepts anything still gets the producer's
        // first choice, not the last.
        struct AcceptAll;
        impl FormatAware for AcceptAll {}
        assert_eq!(
            negotiate_formats(&TwoFormatProducer, &AcceptAll).unwrap(),
            WIDE[0]
        );
    }

    #[test]
    fn stream_spec_merges_when_only_one_side_declares_a_timebase() {
        let declared = StreamSpec {
            timebase: Some(timebase()),
            has_audio: true,
            has_video: false,
        };
        let undeclared = StreamSpec {
            timebase: None,
            has_audio: false,
            has_video: true,
        };
        let merged = negotiate_stream(&declared, &undeclared).expect("merge");
        assert_eq!(merged.timebase, Some(timebase()));
        // Media flags are a union: the merged edge carries both.
        assert!(merged.has_audio);
        assert!(merged.has_video);

        // Order does not matter, and no timebase anywhere stays `None`.
        assert_eq!(negotiate_stream(&undeclared, &declared).unwrap(), merged);
        let empty = StreamSpec {
            timebase: None,
            has_audio: false,
            has_video: false,
        };
        assert_eq!(negotiate_stream(&empty, &empty).unwrap(), empty);
    }

    #[test]
    fn a_zero_stall_bound_rejects_the_first_need_more() {
        // `stall_bound` is "how many non-output steps are tolerated", so
        // zero means none. Worth pinning: it is the boundary between a
        // working pipeline and an instant backpressure error.
        let mut p = Source::new(vec![1, 2, 3])
            .then(Bounded::with_window(2))
            .into_pipeline();
        assert_eq!(p.pull(0).unwrap_err(), NodeError::Backpressure);
    }

    #[test]
    fn pull_survives_a_stall_bound_below_the_batch_size() {
        // window 4 needs four NeedMore steps; a bound of 2 must abort
        // rather than loop forever or emit a partial batch.
        let mut p = Source::new(vec![1, 2, 3, 4])
            .then(Bounded::with_window(4))
            .into_pipeline();
        assert_eq!(p.pull(2).unwrap_err(), NodeError::Backpressure);
        // The default bound is comfortably above that.
        let mut q = Source::new(vec![1, 2, 3, 4])
            .then(Bounded::with_window(4))
            .into_pipeline();
        assert_eq!(q.pull(STALL_BOUND).unwrap(), Some(1));
    }
}
