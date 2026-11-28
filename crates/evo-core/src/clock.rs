//! Fallible system wall-clock conversions; no clock read participates in a policy decision.
use crate::{Error, Result};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Highest wall-clock sample of this process, in Unix ms. `Store::open` admits
/// the data directory's registered occurrence floor through a sample, and a
/// data directory has one trusted writer (§7.3.1), so every later recorded
/// occurrence comes from a sample checked against this floor.
static FLOOR_MILLIS: Mutex<i64> = Mutex::new(0);

fn admit_sample(floor: &Mutex<i64>, read_clock: impl FnOnce() -> SystemTime) -> Result<i64> {
    let mut floor = floor.lock().map_err(|_| Error::Internal)?;
    let sample = unix_millis_at(read_clock())?;
    if sample < *floor {
        return Err(Error::ClockRegressed);
    }
    *floor = sample;
    Ok(sample)
}
/// The production wall-clock sample; below the process floor it is refused.
pub(crate) fn sample_millis() -> Result<i64> {
    admit_sample(&FLOOR_MILLIS, SystemTime::now)
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
        let floor = Mutex::new(0);
        let admit = |sample| admit_sample(&floor, || UNIX_EPOCH + Duration::from_millis(sample));
        assert_eq!(admit(5_000).unwrap(), 5_000);
        assert!(matches!(admit(4_999), Err(Error::ClockRegressed)));
        assert_eq!(*floor.lock().unwrap(), 5_000);
        assert_eq!(admit(5_000).unwrap(), 5_000);
        assert_eq!(admit(6_000).unwrap(), 6_000);
    }
    #[test]
    fn sampling_holds_the_floor_lock_against_other_threads() {
        let floor = Mutex::new(0);
        let sample = admit_sample(&floor, || {
            let time = UNIX_EPOCH + Duration::from_millis(1_001);
            let floor_is_locked = std::thread::scope(|scope| {
                scope
                    .spawn(|| matches!(floor.try_lock(), Err(std::sync::TryLockError::WouldBlock)))
                    .join()
                    .unwrap()
            });
            assert!(floor_is_locked);
            time
        });
        assert_eq!(sample.unwrap(), 1_001);
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
