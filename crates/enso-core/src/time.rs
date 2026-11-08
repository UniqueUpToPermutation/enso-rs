//! One timeline for every thread: [`Clock`] and [`Timestamp`].
//!
//! The MIDI thread stamps events, the audio thread will turn stamps into
//! sample positions, and the simulation turns them into sim time. That only
//! works if they all measure from the same origin, so create **one** `Clock`
//! at startup and hand copies of it to everyone (it's `Copy`).

use std::{
    fmt,
    ops::{Add, Sub},
    time::{Duration, Instant},
};

/// A point in time: nanoseconds since a [`Clock`]'s epoch.
///
/// Why not `std::time::Instant`? An `Instant` can only come from the OS
/// clock, so tests couldn't construct "the event at 1.5 s". A plain integer
/// can be built anywhere, compared exactly, and is cheap to store and send
/// (`u64` nanoseconds lasts 584 years).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Timestamp(u64);

impl Timestamp {
    /// The clock's epoch.
    pub const ZERO: Self = Self(0);

    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    /// Negative or NaN seconds give [`ZERO`](Self::ZERO).
    pub fn from_secs_f64(secs: f64) -> Self {
        // `as` saturates float-to-int conversions (NaN becomes 0).
        Self((secs * 1e9) as u64)
    }

    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    pub fn as_secs_f64(self) -> f64 {
        self.0 as f64 * 1e-9
    }

    /// Time elapsed from `earlier` to `self`, zero if `earlier` is later
    /// (like [`Instant::duration_since`]).
    pub fn duration_since(self, earlier: Timestamp) -> Duration {
        Duration::from_nanos(self.0.saturating_sub(earlier.0))
    }
}

impl Add<Duration> for Timestamp {
    type Output = Timestamp;
    fn add(self, rhs: Duration) -> Timestamp {
        Timestamp(self.0.saturating_add(rhs.as_nanos().min(u64::MAX as u128) as u64))
    }
}

/// Saturates at [`Timestamp::ZERO`].
impl Sub<Duration> for Timestamp {
    type Output = Timestamp;
    fn sub(self, rhs: Duration) -> Timestamp {
        Timestamp(self.0.saturating_sub(rhs.as_nanos().min(u64::MAX as u128) as u64))
    }
}

impl fmt::Display for Timestamp {
    /// Seconds with millisecond precision, e.g. `12.345s`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3}s", self.as_secs_f64())
    }
}

/// Converts between the OS clock and [`Timestamp`]s.
///
/// `Copy`, so every thread can hold its own copy; all copies share the epoch.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    epoch: Instant,
}

impl Clock {
    /// A clock whose epoch is now.
    pub fn new() -> Self {
        Self { epoch: Instant::now() }
    }

    /// The current time.
    pub fn now(&self) -> Timestamp {
        self.timestamp(Instant::now())
    }

    /// `instant` on this clock's timeline ([`Timestamp::ZERO`] if before the epoch).
    pub fn timestamp(&self, instant: Instant) -> Timestamp {
        let nanos = instant.saturating_duration_since(self.epoch).as_nanos();
        Timestamp(nanos.min(u64::MAX as u128) as u64)
    }

    /// The OS-clock instant of `timestamp`.
    pub fn instant(&self, timestamp: Timestamp) -> Instant {
        self.epoch + Duration::from_nanos(timestamp.0)
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_round_trip() {
        let t = Timestamp::from_secs_f64(1.5);
        assert_eq!(t.as_nanos(), 1_500_000_000);
        assert_eq!(t.as_secs_f64(), 1.5);
        assert_eq!(Timestamp::from_secs_f64(-2.0), Timestamp::ZERO);
        assert_eq!(Timestamp::from_secs_f64(f64::NAN), Timestamp::ZERO);
    }

    #[test]
    fn arithmetic_saturates() {
        let t = Timestamp::from_nanos(10);
        assert_eq!(t - Duration::from_nanos(20), Timestamp::ZERO);
        assert_eq!(t + Duration::from_nanos(5), Timestamp::from_nanos(15));
        assert_eq!(t.duration_since(Timestamp::from_nanos(4)), Duration::from_nanos(6));
        assert_eq!(t.duration_since(Timestamp::from_nanos(40)), Duration::ZERO);
    }

    #[test]
    fn clock_maps_both_ways() {
        let clock = Clock::new();
        let later = Instant::now() + Duration::from_millis(250);
        let stamp = clock.timestamp(later);
        assert!(stamp >= Timestamp::from_secs_f64(0.25));
        assert_eq!(clock.instant(stamp), later);
        assert!(clock.now() <= clock.now());
    }

    #[test]
    fn display_in_seconds() {
        assert_eq!(Timestamp::from_secs_f64(12.3456).to_string(), "12.346s");
    }
}
