//! Fallible system wall-clock conversions; no clock read participates in a policy decision.
use crate::{Error, Result};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
}
