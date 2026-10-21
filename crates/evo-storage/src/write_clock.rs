//! Operation-local wall-clock admission. No stamp is a deserializable input.
//! The caller must first qualify the complete floor on its original connection.
//! Stamps are consumed once through their current, still-healthy operation.

use crate::owned_occurrence::{OccurrenceFloor, PreparePreimages, SchemaRead, collect_floor};
use evo_core::{Error, Result};
use sqlx::{
    Connection, Sqlite, SqliteConnection, Transaction, TransactionManager,
    sqlite::SqliteTransactionManager,
};
use std::{
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};

#[derive(Debug, Default)]
pub(super) struct OperationClock {
    failed: bool,
    source: evo_core::clock::WallClock,
    identity: Arc<()>,
    serial: u64,
    rollback_error: Option<sqlx::Error>,
}

#[derive(Debug)]
struct Sampled<T> {
    value: T,
    #[cfg(test)]
    observed_nanos: i128,
    owner: Arc<()>,
    serial: u64,
}

/// A seconds value from one accepted system-wall-clock observation.
#[derive(Debug)]
pub(super) struct SecondsStamp(Sampled<i64>);

/// One accepted source observation, with no second read or rounded future time.
#[derive(Debug)]
pub(super) struct QualifiedEventTime {
    seconds: i64,
    millis: i64,
}
impl QualifiedEventTime {
    pub(super) fn seconds(&self) -> i64 {
        self.seconds
    }
    pub(super) fn millis(&self) -> i64 {
        self.millis
    }
}
#[derive(Debug)]
pub(super) struct EventStamp(Sampled<QualifiedEventTime>);

/// Only this clock module can create a binding to the accepted event sample.
/// It is provenance, not a replacement for a fresh admission sample.
pub(super) struct NativeEventBinding {
    owner: Arc<()>,
    serial: u64,
}

fn event_time(elapsed: Duration) -> Option<QualifiedEventTime> {
    Some(QualifiedEventTime {
        seconds: seconds(elapsed)?,
        millis: millis(elapsed)?,
    })
}

/// A milliseconds value from one accepted system-wall-clock observation.
#[derive(Debug)]
pub(super) struct MillisStamp(Sampled<i64>);

/// Used for a temporary name only after occurrence admission.
#[derive(Debug)]
pub(super) struct NanosStamp(Sampled<u128>);

fn seconds(elapsed: Duration) -> Option<i64> {
    i64::try_from(elapsed.as_secs()).ok()
}

fn millis(elapsed: Duration) -> Option<i64> {
    i64::try_from(elapsed.as_millis()).ok()
}

fn nanos(elapsed: Duration) -> Option<u128> {
    Some(elapsed.as_nanos())
}

/// Facts from finishing one real transaction. A rollback error does not erase
/// the primary Clock refusal, and an uncertain commit is never called unwritten.
#[derive(Debug)]
pub struct TransactionFailure {
    pub error: Error,
    pub rollback_error: Option<sqlx::Error>,
    pub commit_outcome_unknown: bool,
    pub commit_confirmed: bool,
}

impl TransactionFailure {
    pub(crate) fn into_error(self) -> Error {
        if self.rollback_error.is_some() || self.commit_outcome_unknown || self.commit_confirmed {
            tracing::error!(
                rollback_error = ?self.rollback_error,
                commit_outcome_unknown = self.commit_outcome_unknown,
                commit_confirmed = self.commit_confirmed,
                "transaction completion requires explicit reconciliation"
            );
        }
        self.error
    }
}

impl OperationClock {
    pub(super) fn with_source(source: evo_core::clock::WallClock) -> Self {
        Self {
            source,
            ..Default::default()
        }
    }

    pub(super) fn record_rollback_error(&mut self, error: sqlx::Error) {
        if self.rollback_error.is_none() {
            self.rollback_error = Some(error);
        }
    }

    pub(super) fn failure(&mut self, error: Error) -> TransactionFailure {
        TransactionFailure {
            error,
            rollback_error: self.rollback_error.take(),
            commit_outcome_unknown: false,
            commit_confirmed: false,
        }
    }

    pub(super) fn check(&self) -> Result<()> {
        if self.failed {
            Err(Error::ClockRegressed)
        } else {
            Ok(())
        }
    }

    pub(super) fn take_event(&mut self, stamp: EventStamp) -> Result<QualifiedEventTime> {
        self.consume(stamp.0)
    }

    pub(super) fn take_bound_event(
        &mut self,
        stamp: EventStamp,
    ) -> Result<(QualifiedEventTime, NativeEventBinding)> {
        let owner = Arc::clone(&stamp.0.owner);
        let serial = stamp.0.serial;
        let event = self.take_event(stamp)?;
        Ok((event, NativeEventBinding { owner, serial }))
    }

    pub(super) fn require_event_binding(&self, binding: &NativeEventBinding) -> Result<()> {
        self.check()?;
        if Arc::ptr_eq(&self.identity, &binding.owner)
            && binding.serial != 0
            && binding.serial <= self.serial
        {
            Ok(())
        } else {
            Err(Error::ClockRegressed)
        }
    }

    pub(super) fn take_seconds(&mut self, stamp: SecondsStamp) -> Result<i64> {
        self.consume(stamp.0)
    }

    pub(super) fn take_millis(&mut self, stamp: MillisStamp) -> Result<i64> {
        self.consume(stamp.0)
    }

    pub(super) fn take_nanos(&mut self, stamp: NanosStamp) -> Result<u128> {
        self.consume(stamp.0)
    }

    fn consume<T>(&mut self, stamp: Sampled<T>) -> Result<T> {
        if self.failed || !Arc::ptr_eq(&self.identity, &stamp.owner) || stamp.serial != self.serial
        {
            return Err(self.fail());
        }
        Ok(stamp.value)
    }

    /// A qualification failure belongs to the same operation as its writes.
    /// A later successful sample cannot clear this state.
    pub(super) fn fail(&mut self) -> Error {
        self.failed = true;
        Error::ClockRegressed
    }

    pub(super) fn event(&mut self, floor: &OccurrenceFloor) -> Result<EventStamp> {
        self.observe_now(floor, event_time).map(EventStamp)
    }

    pub(super) fn seconds(&mut self, floor: &OccurrenceFloor) -> Result<SecondsStamp> {
        self.observe_now(floor, seconds).map(SecondsStamp)
    }

    pub(super) fn millis(&mut self, floor: &OccurrenceFloor) -> Result<MillisStamp> {
        self.observe_now(floor, millis).map(MillisStamp)
    }

    pub(super) fn nanos(&mut self, floor: &OccurrenceFloor) -> Result<NanosStamp> {
        self.observe_now(floor, nanos).map(NanosStamp)
    }

    fn observe_now<T>(
        &mut self,
        floor: &OccurrenceFloor,
        convert: fn(Duration) -> Option<T>,
    ) -> Result<Sampled<T>> {
        if self.failed {
            return Err(Error::ClockRegressed);
        }
        let elapsed = self
            .source
            .sample()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok());
        self.accept_elapsed(floor, elapsed, convert)
    }

    // Only this module can supply a duration. Production entry points above
    // always take a fresh system sample; no DTO, environment or caller clock.
    fn accept_elapsed<T>(
        &mut self,
        floor: &OccurrenceFloor,
        elapsed: Option<Duration>,
        convert: fn(Duration) -> Option<T>,
    ) -> Result<Sampled<T>> {
        if self.failed {
            return Err(Error::ClockRegressed);
        }
        let accepted = (|| {
            let elapsed = elapsed.ok_or(Error::ClockRegressed)?;
            let observed_nanos =
                i128::try_from(elapsed.as_nanos()).map_err(|_| Error::ClockRegressed)?;
            if floor
                .max_nanos()
                .is_some_and(|maximum| observed_nanos < maximum)
            {
                return Err(Error::ClockRegressed);
            }
            let value = convert(elapsed).ok_or(Error::ClockRegressed)?;
            let serial = self.serial.checked_add(1).ok_or(Error::ClockRegressed)?;
            Ok(Sampled {
                value,
                #[cfg(test)]
                observed_nanos,
                owner: Arc::clone(&self.identity),
                serial,
            })
        })();
        match &accepted {
            Ok(stamp) => self.serial = stamp.serial,
            Err(_) => self.failed = true,
        }
        accepted
    }
}

/// Qualifies one snapshot on the transaction's actual connection, then samples
/// and immediately consumes a new system stamp. Call again after any await
/// that precedes another event; an old integer is not an admission token.
pub(super) async fn seconds_in_transaction(
    tx: &mut impl std::ops::DerefMut<Target = SqliteConnection>,
    inputs: &PreparePreimages,
    clock: &mut OperationClock,
) -> Result<i64> {
    clock.check()?;
    let floor = match collect_floor(tx, inputs, SchemaRead::Stable).await {
        Ok(floor) => floor,
        Err(_) => return Err(poison_and_rollback(tx, clock).await),
    };
    let stamp = match clock.seconds(&floor) {
        Ok(stamp) => stamp,
        Err(_) => return Err(poison_and_rollback(tx, clock).await),
    };
    match clock.take_seconds(stamp) {
        Ok(value) => Ok(value),
        Err(_) => Err(poison_and_rollback(tx, clock).await),
    }
}

pub(super) async fn millis_in_transaction(
    tx: &mut impl std::ops::DerefMut<Target = SqliteConnection>,
    inputs: &PreparePreimages,
    clock: &mut OperationClock,
) -> Result<i64> {
    clock.check()?;
    let floor = match collect_floor(tx, inputs, SchemaRead::Stable).await {
        Ok(floor) => floor,
        Err(_) => return Err(poison_and_rollback(tx, clock).await),
    };
    let stamp = match clock.millis(&floor) {
        Ok(stamp) => stamp,
        Err(_) => return Err(poison_and_rollback(tx, clock).await),
    };
    match clock.take_millis(stamp) {
        Ok(value) => Ok(value),
        Err(_) => Err(poison_and_rollback(tx, clock).await),
    }
}

/// SQLite's public manager updates its real worker transaction depth and
/// acknowledges rollback on this same connection. The Transaction remains
/// owned by its caller, but its later Drop queues only a depth-zero no-op.
/// A failed rollback is retained separately; ClockRegressed stays primary.
async fn poison_and_rollback(
    tx: &mut impl std::ops::DerefMut<Target = SqliteConnection>,
    clock: &mut OperationClock,
) -> Error {
    clock.fail();
    if let Err(error) = SqliteTransactionManager::rollback(&mut **tx).await {
        clock.record_rollback_error(error);
    }
    Error::ClockRegressed
}

/// Guard a non-transactional SQLite/FS phase through the same acquired
/// connection. The caller retains its writer scope and this connection through
/// the next effect; do not reacquire the single pool from inside a Session.
pub(super) async fn seconds_on_connection(
    connection: &mut SqliteConnection,
    inputs: &PreparePreimages,
    clock: &mut OperationClock,
) -> Result<i64> {
    clock.check()?;
    if SqliteTransactionManager::get_transaction_depth(connection) != 0 {
        // This helper starts a top-level read snapshot; it must not silently
        // create a nested savepoint or later roll back another operation.
        return Err(clock.fail());
    }
    let mut tx = connection.begin().await.map_err(crate::internal)?;
    let floor = match collect_floor(&mut tx, inputs, SchemaRead::Stable).await {
        Ok(floor) => floor,
        Err(_) => {
            clock.fail();
            return match finish_transaction(tx, inputs, clock, false).await {
                Err(failure) => Err(failure.into_error()),
                Ok(()) => Err(Error::ClockRegressed),
            };
        }
    };
    if clock
        .seconds(&floor)
        .and_then(|stamp| clock.take_seconds(stamp))
        .is_err()
    {
        return match finish_transaction(tx, inputs, clock, false).await {
            Err(failure) => Err(failure.into_error()),
            Ok(()) => Err(Error::ClockRegressed),
        };
    }
    finish_transaction(tx, inputs, clock, false)
        .await
        .map_err(TransactionFailure::into_error)?;
    // The commit/terminal acknowledgement awaited above invalidates the first
    // stamp for a new event. Re-sample after it, comparing before truncation.
    let stamp = clock.seconds(&floor)?;
    clock.take_seconds(stamp)
}

/// Owner-held transactions and SQLx's locally borrowed top-level snapshots
/// retain the original connection through explicit rollback acknowledgement.
/// The terminal helper below deliberately does not accept nested savepoints.
pub(super) trait ClockTransaction: std::ops::DerefMut<Target = SqliteConnection> {
    fn rollback(
        self,
    ) -> impl std::future::Future<Output = std::result::Result<(), sqlx::Error>> + Send;
}
impl ClockTransaction for Transaction<'_, Sqlite> {
    async fn rollback(self) -> std::result::Result<(), sqlx::Error> {
        Transaction::rollback(self).await
    }
}
impl ClockTransaction for crate::NativeTransaction {
    async fn rollback(self) -> std::result::Result<(), sqlx::Error> {
        crate::NativeTransaction::rollback(self).await
    }
}

/// A read-only Session need not pass timed admission, so diagnostics remain
/// available for a malformed/future occurrence. A poisoned Session can never
/// commit, including after callers catch an earlier error.
pub(super) async fn finish_transaction(
    mut tx: impl ClockTransaction,
    inputs: &PreparePreimages,
    clock: &mut OperationClock,
    wrote: bool,
) -> std::result::Result<(), TransactionFailure> {
    let admission = if wrote {
        seconds_in_transaction(&mut tx, inputs, clock)
            .await
            .map(|_| ())
    } else {
        clock.check()
    };
    if let Err(error) = admission {
        if let Err(rollback) = tx.rollback().await {
            clock.record_rollback_error(rollback);
        }
        return Err(clock.failure(error));
    }
    if SqliteTransactionManager::get_transaction_depth(&tx) != 1 {
        // No generic "commit then rollback" is safe at savepoint depth: that
        // rollback could otherwise undo an outer transaction.
        return Err(clock.failure(Error::Internal));
    }
    // Keep ownership of this exact connection through a failed commit. The
    // public manager updates SQLx's worker depth, unlike an ad-hoc SQL COMMIT.
    let committed = SqliteTransactionManager::commit(&mut *tx).await;
    match committed {
        Err(error) => {
            if let Err(rollback) = tx.rollback().await {
                clock.record_rollback_error(rollback);
            }
            let mut failure = clock.failure(crate::internal(error));
            failure.commit_outcome_unknown = true;
            Err(failure)
        }
        Ok(()) => {
            // The successful manager commit acknowledged depth zero. Consume
            // the still-open Rust wrapper with its normal rollback method:
            // SQLite's worker performs no SQL at depth zero, acknowledges it,
            // and Transaction::rollback marks the wrapper closed. This is
            // terminal acknowledgement, never a claim to undo the commit.
            match tx.rollback().await {
                Ok(()) => Ok(()),
                Err(error) => {
                    clock.record_rollback_error(error);
                    let mut failure = clock.failure(Error::Internal);
                    failure.commit_confirmed = true;
                    Err(failure)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    #[test]
    fn native_event_binding_cannot_transfer_operations_or_repair_a_failed_clock() {
        let floor = OccurrenceFloor::default();
        let mut clock = OperationClock::default();
        let sample = clock
            .accept_elapsed(&floor, Some(Duration::from_millis(10_502)), event_time)
            .unwrap();
        let (event, binding) = clock.take_bound_event(EventStamp(sample)).unwrap();
        assert_eq!(event.seconds(), 10);
        assert_eq!(event.millis(), 10_502);
        assert!(clock.require_event_binding(&binding).is_ok());
        let _later = clock
            .accept_elapsed(&floor, Some(Duration::from_millis(10_503)), seconds)
            .unwrap();
        // Provenance survives fresh samples on the same operation; it does not
        // admit a write, which still needs the native plan's fresh admission.
        assert!(clock.require_event_binding(&binding).is_ok());
        let other = OperationClock::default();
        assert!(matches!(
            other.require_event_binding(&binding),
            Err(Error::ClockRegressed)
        ));
        clock.fail();
        assert!(matches!(
            clock.require_event_binding(&binding),
            Err(Error::ClockRegressed)
        ));
    }

    #[test]
    fn qualified_event_preserves_same_sample_millis_without_rounding_seconds() {
        let mut floor = OccurrenceFloor::default();
        floor.observe_millis(10_501).unwrap();
        let mut clock = OperationClock::default();
        let sample = clock
            .accept_elapsed(&floor, Some(Duration::from_millis(10_502)), event_time)
            .unwrap();
        let event = clock.take_event(EventStamp(sample)).unwrap();
        assert_eq!(event.seconds(), 10);
        assert_eq!(event.millis(), 10_502);
        assert_eq!(event.seconds(), event.millis() / 1000);
        assert!(event.seconds() * 1000 < 10_501);
        assert!(event.millis() >= 10_501);
        let mut bad = OperationClock::default();
        assert!(matches!(
            bad.accept_elapsed(&floor, Some(Duration::from_millis(10_500)), event_time),
            Err(Error::ClockRegressed)
        ));
    }

    fn rejected<T>(value: Result<T>) {
        assert!(matches!(value, Err(Error::ClockRegressed)));
    }

    #[test]
    fn an_empty_floor_does_not_invent_an_occurrence() {
        let floor = OccurrenceFloor::default();
        let mut clock = OperationClock::default();
        assert_eq!(floor.max_nanos(), None);
        let stamp = clock
            .accept_elapsed(&floor, Some(Duration::ZERO), seconds)
            .unwrap();
        assert_eq!(stamp.value, 0);
        assert_eq!(stamp.observed_nanos, 0);
        assert_eq!(floor.max_nanos(), None);
    }

    #[test]
    fn milliseconds_are_compared_before_seconds_are_truncated() {
        let mut floor = OccurrenceFloor::default();
        floor.observe_millis(10_500).unwrap();
        let before = floor.clone();
        let mut clock = OperationClock::default();
        rejected(clock.accept_elapsed(&floor, Some(Duration::from_millis(10_499)), seconds));
        assert_eq!(floor, before);
        rejected(clock.accept_elapsed(&floor, Some(Duration::from_millis(10_750)), millis));

        for (value, expected_seconds) in [(10_500, 10), (10_750, 10)] {
            let mut fresh = OperationClock::default();
            let stamp = fresh
                .accept_elapsed(&floor, Some(Duration::from_millis(value)), seconds)
                .unwrap();
            assert_eq!(stamp.value, expected_seconds);
            assert_eq!(stamp.observed_nanos, i128::from(value) * 1_000_000);
        }
    }

    #[test]
    fn an_earlier_epoch_sample_poison_is_not_repaired_by_healthy_time() {
        let floor = OccurrenceFloor::default();
        let mut clock = OperationClock::default();
        let before_epoch = UNIX_EPOCH
            .checked_sub(Duration::from_nanos(1))
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .ok();
        rejected(clock.accept_elapsed(&floor, before_epoch, seconds));
        rejected(clock.accept_elapsed(&floor, Some(Duration::from_secs(1_000)), seconds));
    }

    #[test]
    fn seconds_overflow_is_not_replaced_by_a_nanos_stamp() {
        let floor = OccurrenceFloor::default();
        let mut clock = OperationClock::default();
        rejected(clock.accept_elapsed(
            &floor,
            Some(Duration::from_secs(i64::MAX as u64 + 1)),
            seconds,
        ));
        rejected(clock.accept_elapsed(&floor, Some(Duration::from_secs(1)), nanos));
    }

    #[test]
    fn milliseconds_overflow_and_exact_boundary_are_distinct() {
        let floor = OccurrenceFloor::default();
        let mut clock = OperationClock::default();
        assert_eq!(
            clock
                .accept_elapsed(&floor, Some(Duration::from_millis(i64::MAX as u64)), millis,)
                .unwrap()
                .value,
            i64::MAX
        );
        rejected(clock.accept_elapsed(
            &floor,
            Some(Duration::from_millis(i64::MAX as u64 + 1)),
            millis,
        ));
        rejected(clock.accept_elapsed(&floor, Some(Duration::ZERO), millis));
    }

    #[test]
    fn source_qualification_failure_has_the_same_operation_lifetime() {
        let mut clock = OperationClock::default();
        assert!(matches!(clock.fail(), Error::ClockRegressed));
        let floor = OccurrenceFloor::default();
        rejected(clock.accept_elapsed(&floor, Some(Duration::from_secs(5)), seconds));
        rejected(clock.seconds(&floor));
        rejected(clock.millis(&floor));
        rejected(clock.nanos(&floor));
    }

    #[test]
    fn public_sampling_methods_only_use_the_system_clock() {
        let floor = OccurrenceFloor::default();
        let before = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let mut clock = OperationClock::default();
        let stamp = clock.seconds(&floor).unwrap();
        let seconds = clock.take_seconds(stamp).unwrap();
        let stamp = clock.millis(&floor).unwrap();
        let millis = clock.take_millis(stamp).unwrap();
        let stamp = clock.nanos(&floor).unwrap();
        let nanos = clock.take_nanos(stamp).unwrap();
        let after = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        assert!(i128::from(seconds) >= i128::from(before.as_secs()));
        assert!(i128::from(seconds) <= i128::from(after.as_secs()));
        assert!(millis as u128 >= before.as_millis());
        assert!(millis as u128 <= after.as_millis());
        assert!(nanos >= before.as_nanos());
        assert!(nanos <= after.as_nanos());
        assert_eq!(floor.max_nanos(), None);
    }
    #[test]
    fn a_stamp_cannot_escape_a_later_clock_failure() {
        let floor = OccurrenceFloor::default();
        let mut clock = OperationClock::default();
        let stamp = SecondsStamp(
            clock
                .accept_elapsed(&floor, Some(Duration::from_secs(1)), seconds)
                .unwrap(),
        );
        clock.fail();
        rejected(clock.take_seconds(stamp));
        rejected(clock.check());
    }

    #[test]
    fn another_operation_cannot_consume_a_stale_stamp() {
        let floor = OccurrenceFloor::default();
        let mut original = OperationClock::default();
        let stamp = SecondsStamp(
            original
                .accept_elapsed(&floor, Some(Duration::from_secs(1)), seconds)
                .unwrap(),
        );
        let mut other = OperationClock::default();
        let _other_stamp = other
            .accept_elapsed(&floor, Some(Duration::from_secs(1)), seconds)
            .unwrap();
        assert_eq!(original.serial, other.serial);
        rejected(other.take_seconds(stamp));
        rejected(other.check());
        original.check().unwrap();
    }

    #[test]
    fn a_new_sample_invalidates_the_prior_sample() {
        let floor = OccurrenceFloor::default();
        let mut clock = OperationClock::default();
        let older = SecondsStamp(
            clock
                .accept_elapsed(&floor, Some(Duration::from_secs(1)), seconds)
                .unwrap(),
        );
        let newer = SecondsStamp(
            clock
                .accept_elapsed(&floor, Some(Duration::from_secs(2)), seconds)
                .unwrap(),
        );
        rejected(clock.take_seconds(older));
        rejected(clock.take_seconds(newer));
    }

    #[test]
    fn serial_exhaustion_poison_refuses_instead_of_wrapping() {
        let floor = OccurrenceFloor::default();
        let mut clock = OperationClock {
            serial: u64::MAX,
            ..OperationClock::default()
        };
        rejected(clock.accept_elapsed(&floor, Some(Duration::ZERO), seconds));
        assert_eq!(clock.serial, u64::MAX);
        rejected(clock.check());
    }
}
