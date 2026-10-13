use evo_core::{
    Error,
    clock::{unix_millis_at, unix_nanos_at, unix_seconds_at},
};
use std::time::{Duration, UNIX_EPOCH};
#[test]
fn epoch_is_a_valid_zero_sample() {
    assert_eq!(unix_seconds_at(UNIX_EPOCH).unwrap(), 0);
    assert_eq!(unix_millis_at(UNIX_EPOCH).unwrap(), 0);
    assert_eq!(unix_nanos_at(UNIX_EPOCH).unwrap(), 0);
}
#[test]
fn pre_epoch_sample_is_a_named_refusal_at_every_precision() {
    let sample = UNIX_EPOCH.checked_sub(Duration::from_nanos(1)).unwrap();
    assert!(matches!(
        unix_seconds_at(sample),
        Err(Error::ClockRegressed)
    ));
    assert!(matches!(unix_millis_at(sample), Err(Error::ClockRegressed)));
    assert!(matches!(unix_nanos_at(sample), Err(Error::ClockRegressed)));
    assert_eq!(
        unix_seconds_at(sample).unwrap_err().to_string(),
        "clock_regressed"
    );
}
#[test]
fn positive_subsecond_sample_preserves_units() {
    let sample = UNIX_EPOCH + Duration::new(123, 456_789_000);
    assert_eq!(unix_seconds_at(sample).unwrap(), 123);
    assert_eq!(unix_millis_at(sample).unwrap(), 123_456);
    assert_eq!(unix_nanos_at(sample).unwrap(), 123_456_789_000);
}
