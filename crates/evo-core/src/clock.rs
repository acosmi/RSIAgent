//! Fallible system wall-clock conversions; no clock read participates in a policy decision.
use crate::{Error, Result};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Highest wall-clock sample of this process, in Unix ms. `Store::open` admits
/// the data directory's registered occurrence floor through a sample, and a
/// data directory has one trusted writer (§7.3.1), so every later recorded
/// occurrence comes from a sample checked against this floor.
static FLOOR_MILLIS: AtomicI64 = AtomicI64::new(0);

fn admit_sample(floor: &AtomicI64, sample: i64) -> Result<i64> {
    if sample < floor.fetch_max(sample, Ordering::SeqCst) {
        return Err(Error::ClockRegressed);
    }
    Ok(sample)
}
/// The production wall-clock sample; below the process floor it is refused.
pub(crate) fn sample_millis() -> Result<i64> {
    admit_sample(&FLOOR_MILLIS, unix_millis_at(SystemTime::now())?)
}
/// Refuses a clock that is behind a data directory's recorded occurrence floor.
pub fn admit_floor(floor_millis: i64) -> Result<()> {
    if sample_millis()? < floor_millis {
        return Err(Error::ClockRegressed);
    }
    Ok(())
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
    fn a_sample_behind_the_floor_refuses_and_keeps_the_floor() {
        let floor = AtomicI64::new(0);
        assert_eq!(admit_sample(&floor, 5_000).unwrap(), 5_000);
        assert!(matches!(
            admit_sample(&floor, 4_999),
            Err(Error::ClockRegressed)
        ));
        assert_eq!(floor.load(Ordering::SeqCst), 5_000);
        assert_eq!(admit_sample(&floor, 5_000).unwrap(), 5_000);
        assert_eq!(admit_sample(&floor, 6_000).unwrap(), 6_000);
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
}
