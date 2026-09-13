//! §38 property tests for the timestamp model: seeded-random samples
//! asserting the invariants `Timestamp::rescale` and the
//! saturating/checked duration arithmetic promise (§16), independent of
//! any specific hand-picked example.

use tpt_media_core::{Duration, Timebase, Timestamp};

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

/// A pool of realistic timebases (audio, video, milliseconds, seconds).
const TIMEBASES: [Timebase; 6] = [
    Timebase {
        num: 1,
        den: 48_000,
    },
    Timebase {
        num: 1,
        den: 44_100,
    },
    Timebase {
        num: 1,
        den: 90_000,
    },
    Timebase { num: 1, den: 1_000 },
    Timebase { num: 1, den: 1 },
    Timebase {
        num: 1_001,
        den: 48_000,
    },
];

fn random_timestamp(state: &mut u64) -> (Timestamp, Timebase) {
    let tb = TIMEBASES[(lcg(state) % TIMEBASES.len() as u64) as usize];
    let magnitude = lcg(state) % 1_000_000_000_000; // 1e12 ticks
    let ticks = if lcg(state) & 1 == 0 {
        magnitude as i64
    } else {
        -(magnitude as i64)
    };
    (
        Timestamp {
            ticks,
            timebase: tb,
        },
        tb,
    )
}

#[test]
fn rescale_to_the_same_timebase_is_identity() {
    let mut state = 0x5EED_0100u64;
    for _ in 0..256 {
        let (t, tb) = random_timestamp(&mut state);
        let out = t.rescale(tb).expect("identity rescale always succeeds");
        assert_eq!(out.ticks, t.ticks);
        assert_eq!(out.timebase, tb);
    }
}

#[test]
fn rescale_is_weakly_monotonic() {
    let mut state = 0x5EED_0101u64;
    for _ in 0..4096 {
        let (a, from) = random_timestamp(&mut state);
        let b_tick_delta = (lcg(&mut state) % 1_000_000) as i64 + 1;
        let b = Timestamp {
            ticks: a.ticks.saturating_add(b_tick_delta),
            timebase: from,
        };
        let to = TIMEBASES[(lcg(&mut state) % TIMEBASES.len() as u64) as usize];
        if let (Some(ra), Some(rb)) = (a.rescale(to), b.rescale(to)) {
            assert!(
                ra.ticks <= rb.ticks,
                "a = {} → {} , b = {} → {} : order must be preserved",
                a.ticks,
                ra.ticks,
                b.ticks,
                rb.ticks
            );
        }
        // Either both convert or overflow is reported consistently for
        // the smaller magnitude first: if `a` overflows, `b >= a` must
        // not silently succeed in the negative direction.
        if a.ticks < 0 {
            if let Some(ra) = a.rescale(to) {
                let rb = b.rescale(to).expect("b ≥ a must convert if a did");
                assert!(ra.ticks <= rb.ticks);
            }
        }
    }
}

#[test]
fn rescale_to_a_finer_timebase_and_back_is_off_by_at_most_two_ticks() {
    // `rescale` truncates, so a forth-and-back round trip can be off by
    // up to 2 ticks (one truncation each way) — never more. Only exercised
    // in the direction where the first hop *multiplies* the tick count
    // (1 ms / 1 s / 1/1.001 ms → 48 kHz), keeping the truncation bound
    // tight.
    let mut state = 0x5EED_0102u64;
    let coarse = [Timebase { num: 1, den: 1_000 }, Timebase { num: 1, den: 1 }];
    for _ in 0..2048 {
        let ticks = (lcg(&mut state) % 1_000_000_000) as i64;
        let t = Timestamp {
            ticks,
            timebase: coarse[(lcg(&mut state) % 2) as usize],
        };
        let fine = TIMEBASES[(lcg(&mut state) % 3) as usize]; // 48k/44.1k/90k
        let forth = t
            .rescale(fine)
            .expect("finer timebase cannot overflow here");
        let back = forth.rescale(t.timebase).expect("round trip within range");
        assert!(
            (back.ticks - t.ticks).abs() <= 2,
            "{} → {} → {} (timebase {:?})",
            t.ticks,
            forth.ticks,
            back.ticks,
            t.timebase
        );
    }
}

#[test]
fn checked_and_saturating_duration_agree() {
    let mut state = 0x5EED_0103u64;
    for _ in 0..4096 {
        let (t, tb) = random_timestamp(&mut state);
        let dticks = (lcg(&mut state) % 2_000_000_000_000) as i64;
        let d = if lcg(&mut state) & 1 == 0 {
            Duration {
                ticks: dticks,
                timebase: tb,
            }
        } else {
            Duration {
                ticks: -dticks,
                timebase: tb,
            }
        };
        let checked = t.checked_add_duration(d);
        let saturating = t + d;
        match checked {
            Some(c) => assert_eq!(c.ticks, saturating.ticks),
            None => {
                // Saturated in the direction of the duration's sign.
                if d.ticks > 0 {
                    assert_eq!(saturating.ticks, i64::MAX);
                } else {
                    assert_eq!(saturating.ticks, i64::MIN);
                }
            }
        }
        // When the add didn't saturate, subtracting the same duration
        // must recover exactly the timestamp we started from.
        if let Some(back) = saturating.checked_sub_duration(d) {
            assert_eq!(back.ticks, t.ticks);
        }
    }
}

#[test]
fn rescale_reports_overflow_instead_of_wrapping() {
    let mut state = 0x5EED_0104u64;
    for _ in 0..256 {
        // 1 Hz → 90 kHz multiplies the tick count by 90 000; anything
        // beyond ~1e14 ticks overflows i64 and must return None.
        let huge = (i64::MAX as u64 - (lcg(&mut state) % 1_000_000)) as i64;
        let t = Timestamp {
            ticks: huge,
            timebase: Timebase { num: 1, den: 1 },
        };
        assert!(
            t.rescale(Timebase {
                num: 1,
                den: 90_000
            })
            .is_none(),
            "1 Hz tick count {huge} must not rescale into 90 kHz"
        );
    }
}
