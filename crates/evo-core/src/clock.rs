//! Fallible system wall-clock conversions; no clock read participates in a policy decision.
use crate::{Error, Result};
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// An explicitly wired Rust clock source shared by storage and its producers.
/// Production constructors use System. The fixture constructor is a trusted
/// test-harness dependency, never an HTTP/MCP/Serde/environment selection.
#[derive(Clone, Default)]
pub enum WallClock {
    #[default]
    System,
    #[doc(hidden)]
    FixtureSeconds(Arc<dyn Fn() -> i64 + Send + Sync>),
}

impl std::fmt::Debug for WallClock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::System => "SystemWallClock",
            Self::FixtureSeconds(_) => "FixtureWallClock",
        })
    }
}

impl WallClock {
    pub fn system() -> Self {
        Self::System
    }

    /// Rust fixture factories must clone this exact reader before opening their
    /// database. Replacing an already-open store's clock is not supported.
    #[doc(hidden)]
    pub fn fixture_seconds(reader: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        Self::FixtureSeconds(reader)
    }

    /// Source identity only, not provenance or a role/authorization decision.
    pub fn same_source(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::System, Self::System) => true,
            (Self::FixtureSeconds(left), Self::FixtureSeconds(right)) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }

    /// Every call reads the source again. Returned samples are ordinary data;
    /// only storage's private operation owner can admit/consume its own stamp.
    pub fn sample(&self) -> Result<SystemTime> {
        match self {
            Self::System => Ok(SystemTime::now()),
            Self::FixtureSeconds(reader) => {
                let reading = reader();
                let duration = Duration::from_secs(reading.unsigned_abs());
                if reading < 0 {
                    UNIX_EPOCH.checked_sub(duration)
                } else {
                    UNIX_EPOCH.checked_add(duration)
                }
                .ok_or(Error::ClockRegressed)
            }
        }
    }

    pub fn seconds(&self) -> Result<i64> {
        unix_seconds_at(self.sample()?)
    }
    pub fn millis(&self) -> Result<i64> {
        unix_millis_at(self.sample()?)
    }
}

fn elapsed(time: SystemTime) -> Result<Duration> {
    time.duration_since(UNIX_EPOCH)
        .map_err(|_| Error::ClockRegressed)
}
fn seconds(value: Duration) -> Result<i64> {
    i64::try_from(value.as_secs()).map_err(|_| Error::ClockRegressed)
}
fn millis(value: Duration) -> Result<i64> {
    i64::try_from(value.as_millis()).map_err(|_| Error::ClockRegressed)
}
/// Converts a trusted sample; production callers sample the system wall clock.
pub fn unix_seconds_at(time: SystemTime) -> Result<i64> {
    seconds(elapsed(time)?)
}
/// Millisecond precision for occurrence comparisons, without wrapping.
pub fn unix_millis_at(time: SystemTime) -> Result<i64> {
    millis(elapsed(time)?)
}
/// Used only for a temporary filename after admission has accepted its sample.
pub fn unix_nanos_at(time: SystemTime) -> Result<u128> {
    Ok(elapsed(time)?.as_nanos())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn seconds_outside_i64_refuse_instead_of_wrapping() {
        assert!(matches!(
            seconds(Duration::from_secs(i64::MAX as u64 + 1)),
            Err(Error::ClockRegressed)
        ));
        assert_eq!(
            seconds(Duration::from_secs(i64::MAX as u64)).unwrap(),
            i64::MAX
        );
    }
    #[test]
    fn milliseconds_outside_i64_refuse_instead_of_wrapping() {
        let boundary = i64::MAX as u64;
        let valid = Duration::from_millis(boundary);
        assert_eq!(millis(valid).unwrap(), i64::MAX);
        assert!(matches!(
            millis(Duration::from_millis(boundary + 1)),
            Err(Error::ClockRegressed)
        ));
    }
    #[test]
    fn one_fixture_source_is_shared_without_resampling_a_cached_number() {
        use std::sync::atomic::{AtomicI64, Ordering};
        let value = Arc::new(AtomicI64::new(100));
        let observed = Arc::clone(&value);
        let reader: Arc<dyn Fn() -> i64 + Send + Sync> =
            Arc::new(move || observed.load(Ordering::SeqCst));
        let left = WallClock::fixture_seconds(Arc::clone(&reader));
        let right = WallClock::fixture_seconds(reader);
        assert!(left.same_source(&right));
        assert_eq!(left.seconds().unwrap(), 100);
        value.store(101, Ordering::SeqCst);
        assert_eq!(right.millis().unwrap(), 101000);
        value.store(-1, Ordering::SeqCst);
        assert!(matches!(left.seconds(), Err(Error::ClockRegressed)));
    }

    #[test]
    fn equal_fixture_values_do_not_claim_shared_source_identity() {
        let left = WallClock::fixture_seconds(Arc::new(|| 100));
        let right = WallClock::fixture_seconds(Arc::new(|| 100));
        assert!(!left.same_source(&right));
        assert!(!left.same_source(&WallClock::system()));
        assert!(WallClock::system().same_source(&WallClock::default()));
    }
}
