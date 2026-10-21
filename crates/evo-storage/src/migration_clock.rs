//! Same-connection admission around SQLx's original compiled, upward driver.
//!
//! This does not open a connection or acquire directory authority. The caller
//! must have completed the zero-write entry checks and hold its OS guard until
//! connection shutdown. Every transaction here is tracked by SQLx; no raw BEGIN
//! or pool acquisition is used. Failed phases await rollback, while failures
//! after a committed migration retain that fact in MigrationFailure.

use crate::owned_occurrence::{PreparePreimages, SchemaRead, collect_floor};
use crate::write_clock::OperationClock;
use evo_core::{Error, Result};
use sqlx::{
    Connection, Executor, Sqlite, SqliteConnection, Transaction, TransactionManager,
    migrate::{AppliedMigration, Migrate, MigrateError, Migration, Migrator},
};
use std::{
    fmt,
    future::Future,
    pin::Pin,
    time::{Duration, Instant},
};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

type MigrateResult<T> = std::result::Result<T, MigrateError>;
type MigrateFuture<'a, T> = Pin<Box<dyn Future<Output = MigrateResult<T>> + Send + 'a>>;

/// Transient failure details, never a persistent recovery receipt or authority.
///
/// A failed duration-update phase cannot undo an earlier migration commit.
/// rollback_error is an actually awaited rollback failure, never an inference
/// from Drop. The caller must close the connection/pool before releasing its
/// directory guard, including when a driver commit itself fails.
#[derive(Debug)]
pub(super) struct MigrationFailure {
    pub(super) error: Error,
    pub(super) migration_error: MigrateError,
    pub(super) rollback_error: Option<sqlx::Error>,
    /// Newly applied versions with an acknowledged successful commit.
    pub(super) committed_versions: Vec<i64>,
    /// A failed commit acknowledgement does not prove that it never committed.
    pub(super) commit_outcome_unknown: bool,
    pub(super) migrations_table_phase_committed: bool,
}

#[derive(Debug, Default)]
struct RunState {
    clock: OperationClock,
    source_failure: Option<Error>,
    rollback_error: Option<sqlx::Error>,
    committed_versions: Vec<i64>,
    commit_outcome_unknown: bool,
    migrations_table_phase_committed: bool,
}

/// The typed primary source failure stays in RunState. This merely bridges the
/// public SQLx trait's error type; neither side classifies errors by text.
#[derive(Debug)]
struct AdmissionRejected;

impl fmt::Display for AdmissionRejected {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("migration occurrence admission rejected")
    }
}

impl std::error::Error for AdmissionRejected {}

impl RunState {
    fn reject(&mut self, error: Error) -> MigrateError {
        if matches!(&error, Error::ClockRegressed) {
            self.clock.fail();
        }
        if self.source_failure.is_none() {
            self.source_failure = Some(error);
        }
        MigrateError::Source(Box::new(AdmissionRejected))
    }

    fn check(&mut self) -> MigrateResult<()> {
        if let Err(error) = self.clock.check() {
            return Err(self.reject(error));
        }
        Ok(())
    }

    fn finish(
        &mut self,
        result: MigrateResult<()>,
    ) -> std::result::Result<(), Box<MigrationFailure>> {
        let migration_error = match result {
            Ok(()) => match self.check() {
                Ok(()) => return Ok(()),
                Err(error) => error,
            },
            Err(error) => error,
        };
        let error = if self.clock.check().is_err() {
            // Neither a rollback failure nor a later driver error replaces the
            // operation's first Clock failure with Internal.
            Error::ClockRegressed
        } else {
            self.source_failure
                .take()
                .unwrap_or_else(|| super::internal(&migration_error))
        };
        Err(Box::new(MigrationFailure {
            error,
            migration_error,
            rollback_error: self.rollback_error.take(),
            committed_versions: std::mem::take(&mut self.committed_versions),
            commit_outcome_unknown: self.commit_outcome_unknown,
            migrations_table_phase_committed: self.migrations_table_phase_committed,
        }))
    }
}

/// Run exactly the crate's compiled migrations through SQLx's existing driver.
/// The adapter is private, so this exposes no caller-selected migration or undo.
pub(super) async fn run_migrations(
    conn: &mut SqliteConnection,
    inputs: &PreparePreimages,
    clock: &mut OperationClock,
) -> std::result::Result<(), Box<MigrationFailure>> {
    let mut adapter = ClockedMigrations {
        conn,
        inputs,
        state: RunState {
            clock: std::mem::take(clock),
            ..Default::default()
        },
    };
    let result = MIGRATOR.run_direct(&mut adapter).await;
    let result = adapter.state.finish(result);
    *clock = std::mem::take(&mut adapter.state.clock);
    result
}

struct ClockedMigrations<'connection, 'inputs> {
    conn: &'connection mut SqliteConnection,
    inputs: &'inputs PreparePreimages,
    state: RunState,
}

fn is_compiled_up(migration: &Migration) -> bool {
    migration.migration_type.is_up_migration()
        && MIGRATOR.iter().any(|registered| {
            registered.version == migration.version
                && registered.description == migration.description
                && registered.migration_type == migration.migration_type
                && registered.sql == migration.sql
                && registered.checksum == migration.checksum
                && registered.no_tx == migration.no_tx
        })
}

/// The collector borrows this exact tracked transaction's connection and does
/// not begin another transaction. Each successful collection is followed by a
/// fresh production wall-clock sample. Consume it before any later sampling.
async fn admit_utc(
    conn: &mut SqliteConnection,
    inputs: &PreparePreimages,
    state: &mut RunState,
    schema: SchemaRead<'_>,
) -> MigrateResult<String> {
    state.check()?;
    let floor = collect_floor(conn, inputs, schema)
        .await
        .map_err(|error| state.reject(error))?;
    let stamp = state
        .clock
        .seconds(&floor)
        .map_err(|error| state.reject(error))?;
    let seconds = state
        .clock
        .take_seconds(stamp)
        .map_err(|error| state.reject(error))?;
    format_sqlite_utc(seconds).map_err(|error| state.reject(error))
}

async fn abort_phase(
    tx: Transaction<'_, Sqlite>,
    state: &mut RunState,
    primary: MigrateError,
) -> MigrateError {
    if let Err(error) = tx.rollback().await
        && state.rollback_error.is_none()
    {
        state.rollback_error = Some(error);
    }
    // Keep the primary typed admission failure even when rollback failed.
    primary
}

/// commit consumes Transaction even on error. Its Drop only queues cleanup.
/// Request and await the original public transaction manager on the same
/// connection, preserving the primary driver error and the unknown commit
/// outcome. Even an acknowledged rollback here cannot undo a committed phase.
async fn failed_commit(
    conn: &mut SqliteConnection,
    state: &mut RunState,
    primary: MigrateError,
) -> MigrateError {
    state.commit_outcome_unknown = true;
    if let Err(error) = sqlx::sqlite::SqliteTransactionManager::rollback(conn).await
        && state.rollback_error.is_none()
    {
        state.rollback_error = Some(error);
    }
    primary
}

impl Migrate for ClockedMigrations<'_, '_> {
    fn ensure_migrations_table(&mut self) -> MigrateFuture<'_, ()> {
        let conn = &mut *self.conn;
        let inputs = self.inputs;
        let state = &mut self.state;
        Box::pin(async move {
            state.check()?;
            let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
            if let Err(error) = admit_utc(&mut tx, inputs, state, SchemaRead::Stable).await {
                return Err(abort_phase(tx, state, error).await);
            }
            // Delegate SQLx's exact original table DDL, including its DEFAULT.
            // Our own ledger INSERT below always binds installed_on explicitly.
            if let Err(error) = Migrate::ensure_migrations_table(&mut *tx).await {
                return Err(abort_phase(tx, state, error).await);
            }
            if let Err(error) = state.check() {
                return Err(abort_phase(tx, state, error).await);
            }
            if let Err(error) = tx.commit().await {
                return Err(failed_commit(conn, state, error.into()).await);
            }
            state.migrations_table_phase_committed = true;
            Ok(())
        })
    }

    fn dirty_version(&mut self) -> MigrateFuture<'_, Option<i64>> {
        Migrate::dirty_version(&mut *self.conn)
    }

    fn list_applied_migrations(&mut self) -> MigrateFuture<'_, Vec<AppliedMigration>> {
        Migrate::list_applied_migrations(&mut *self.conn)
    }

    fn lock(&mut self) -> MigrateFuture<'_, ()> {
        // SQLite's original implementation is a no-op, not directory authority.
        Migrate::lock(&mut *self.conn)
    }

    fn unlock(&mut self) -> MigrateFuture<'_, ()> {
        Migrate::unlock(&mut *self.conn)
    }

    fn apply<'e: 'm, 'm>(&'e mut self, migration: &'m Migration) -> MigrateFuture<'m, Duration> {
        let conn = &mut *self.conn;
        let inputs = self.inputs;
        let state = &mut self.state;
        Box::pin(async move {
            state.check()?;
            if !is_compiled_up(migration) {
                return Err(state.reject(Error::ClockRegressed));
            }
            let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
            let start = Instant::now();
            if let Err(error) = admit_utc(&mut tx, inputs, state, SchemaRead::Stable).await {
                return Err(abort_phase(tx, state, error).await);
            }

            // Keep the original migration SQL, ExecuteMigration version, and
            // transactional bookkeeping semantics. SQLite's original apply
            // always uses a transaction, including when migration.no_tx is set.
            if let Err(error) = tx.execute(&*migration.sql).await {
                return Err(abort_phase(
                    tx,
                    state,
                    MigrateError::ExecuteMigration(error, migration.version),
                )
                .await);
            }

            // Applying is not a caller "trusted" flag: the collector verifies
            // the complete compiled migration, the exact previous ledger
            // prefix, and the resulting actual schema in this same transaction.
            let installed_on =
                match admit_utc(&mut tx, inputs, state, SchemaRead::Applying(migration)).await {
                    Ok(value) => value,
                    Err(error) => return Err(abort_phase(tx, state, error).await),
                };
            if let Err(error) = sqlx::query(
                r#"
    INSERT INTO _sqlx_migrations
        ( version, description, installed_on, success, checksum, execution_time )
    VALUES ( ?1, ?2, ?3, TRUE, ?4, -1 )
                "#,
            )
            .bind(migration.version)
            .bind(&*migration.description)
            .bind(installed_on)
            .bind(&*migration.checksum)
            .execute(&mut *tx)
            .await
            {
                return Err(abort_phase(tx, state, error.into()).await);
            }
            if let Err(error) = state.check() {
                return Err(abort_phase(tx, state, error).await);
            }
            if let Err(error) = tx.commit().await {
                return Err(failed_commit(conn, state, error.into()).await);
            }
            state.committed_versions.push(migration.version);

            // SQLx measures the whole migration transaction, then records the
            // duration in a separate write. Admission of this later write must
            // not be inherited from the pre-commit phase.
            let elapsed = start.elapsed();
            let mut duration_tx = conn.begin_with("BEGIN IMMEDIATE").await?;
            if let Err(error) = admit_utc(&mut duration_tx, inputs, state, SchemaRead::Stable).await
            {
                return Err(abort_phase(duration_tx, state, error).await);
            }
            // This is SQLx's original duration representation, not occurrence
            // time. Do not overwrite installed_on or pretend this transaction
            // can roll back the preceding migration commit.
            #[allow(clippy::cast_possible_truncation)]
            let execution_time = elapsed.as_nanos() as i64;
            if let Err(error) = sqlx::query(
                r#"
    UPDATE _sqlx_migrations
    SET execution_time = ?1
    WHERE version = ?2
                "#,
            )
            .bind(execution_time)
            .bind(migration.version)
            .execute(&mut *duration_tx)
            .await
            {
                return Err(abort_phase(duration_tx, state, error.into()).await);
            }
            if let Err(error) = state.check() {
                return Err(abort_phase(duration_tx, state, error).await);
            }
            if let Err(error) = duration_tx.commit().await {
                return Err(failed_commit(conn, state, error.into()).await);
            }
            Ok(elapsed)
        })
    }

    fn revert<'e: 'm, 'm>(&'e mut self, _migration: &'m Migration) -> MigrateFuture<'m, Duration> {
        let state = &mut self.state;
        Box::pin(async move {
            state.check()?;
            // The only exposed driver is run_direct, which never calls revert.
            // A future undo entry needs its own full admission/schema contract;
            // it cannot silently inherit SQLite's unguarded DELETE/DDL method.
            Err(state.reject(Error::Forbidden))
        })
    }
}

const SECONDS_PER_DAY: i64 = 86_400;
const LAST_UTC_SECOND: i64 = 253_402_300_799;

fn leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Encode the exact existing SQLx/SQLite UTC text shape without a time library.
/// Range checks precede every conversion; no saturation, default, or old time.
fn format_sqlite_utc(seconds: i64) -> Result<String> {
    if !(0..=LAST_UTC_SECOND).contains(&seconds) {
        return Err(Error::ClockRegressed);
    }
    let mut days = seconds
        .checked_div(SECONDS_PER_DAY)
        .ok_or(Error::ClockRegressed)?;
    let within_day = seconds
        .checked_rem(SECONDS_PER_DAY)
        .ok_or(Error::ClockRegressed)?;
    let mut year = 1970_i64;
    loop {
        let days_in_year = if leap_year(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days = days
            .checked_sub(days_in_year)
            .ok_or(Error::ClockRegressed)?;
        year = year.checked_add(1).ok_or(Error::ClockRegressed)?;
        if year > 9999 {
            return Err(Error::ClockRegressed);
        }
    }
    let months = [
        31_i64,
        if leap_year(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1_i64;
    for days_in_month in months {
        if days < days_in_month {
            break;
        }
        days = days
            .checked_sub(days_in_month)
            .ok_or(Error::ClockRegressed)?;
        month = month.checked_add(1).ok_or(Error::ClockRegressed)?;
    }
    let day = days.checked_add(1).ok_or(Error::ClockRegressed)?;
    let hour = within_day.checked_div(3_600).ok_or(Error::ClockRegressed)?;
    let minute = within_day
        .checked_rem(3_600)
        .and_then(|value| value.checked_div(60))
        .ok_or(Error::ClockRegressed)?;
    let second = within_day.checked_rem(60).ok_or(Error::ClockRegressed)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(Error::ClockRegressed);
    }
    Ok(format!(
        "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

    #[test]
    fn utc_formatter_keeps_epoch_day_month_year_and_gregorian_boundaries() {
        for (seconds, expected) in [
            (0, "1970-01-01 00:00:00"),
            (86_399, "1970-01-01 23:59:59"),
            (86_400, "1970-01-02 00:00:00"),
            (31_535_999, "1970-12-31 23:59:59"),
            (31_536_000, "1971-01-01 00:00:00"),
            (63_072_000, "1972-01-01 00:00:00"),
            (68_169_600, "1972-02-29 00:00:00"),
            (68_256_000, "1972-03-01 00:00:00"),
            (951_782_400, "2000-02-29 00:00:00"),
            (951_868_800, "2000-03-01 00:00:00"),
            (4_107_456_000, "2100-02-28 00:00:00"),
            (4_107_542_400, "2100-03-01 00:00:00"),
            (253_402_214_400, "9999-12-31 00:00:00"),
            (LAST_UTC_SECOND, "9999-12-31 23:59:59"),
        ] {
            let actual = format_sqlite_utc(seconds).unwrap();
            assert_eq!(actual, expected);
            assert_eq!(actual.len(), 19);
        }
    }

    #[test]
    fn utc_formatter_refuses_unrepresentable_time_instead_of_filling_in() {
        for seconds in [i64::MIN, -1, LAST_UTC_SECOND + 1, i64::MAX] {
            assert!(matches!(
                format_sqlite_utc(seconds),
                Err(Error::ClockRegressed)
            ));
        }
    }

    #[test]
    fn utc_formatter_round_trips_the_independent_strict_parser() {
        for seconds in [0, 68_169_600, 951_782_400, 4_107_542_400, LAST_UTC_SECOND] {
            assert_eq!(
                crate::owned_occurrence::parse_sqlite_utc_seconds(
                    &format_sqlite_utc(seconds).unwrap()
                )
                .unwrap(),
                seconds
            );
        }
    }

    #[test]
    fn compiled_migration_identity_cannot_be_replaced_by_a_version_number() {
        assert_eq!(
            MIGRATOR
                .iter()
                .map(|migration| migration.version)
                .collect::<Vec<_>>(),
            [1, 2, 4, 5, 6, 7]
        );
        let original = MIGRATOR.iter().next().unwrap();
        assert!(is_compiled_up(original));

        let mut changed = original.clone();
        changed.sql = Cow::Borrowed("SELECT 1;");
        assert!(!is_compiled_up(&changed));
        let mut changed = original.clone();
        changed.checksum = Cow::Borrowed(&[]);
        assert!(!is_compiled_up(&changed));
        let mut changed = original.clone();
        changed.description = Cow::Borrowed("unregistered");
        assert!(!is_compiled_up(&changed));
        let mut changed = original.clone();
        changed.no_tx = !changed.no_tx;
        assert!(!is_compiled_up(&changed));
    }

    #[test]
    fn clock_failure_survives_driver_and_rollback_failures_with_commits_visible() {
        let mut state = RunState::default();
        let bridge = state.reject(Error::ClockRegressed);
        assert!(matches!(bridge, MigrateError::Source(_)));
        state.rollback_error = Some(sqlx::Error::RowNotFound);
        state.migrations_table_phase_committed = true;
        state.committed_versions.push(1);
        state.commit_outcome_unknown = true;
        let failure = state
            .finish(Err(MigrateError::Execute(sqlx::Error::RowNotFound)))
            .unwrap_err();
        assert!(matches!(failure.error, Error::ClockRegressed));
        assert!(matches!(
            failure.rollback_error,
            Some(sqlx::Error::RowNotFound)
        ));
        assert_eq!(failure.committed_versions, [1]);
        assert!(failure.commit_outcome_unknown);
        assert!(failure.migrations_table_phase_committed);
        assert!(matches!(failure.migration_error, MigrateError::Execute(_)));
    }

    #[test]
    fn non_clock_source_error_is_not_relabelled_as_clock_regression() {
        let mut state = RunState::default();
        let bridge = state.reject(Error::Forbidden);
        state.clock.check().unwrap();
        let failure = state.finish(Err(bridge)).unwrap_err();
        assert!(matches!(failure.error, Error::Forbidden));
        assert!(failure.rollback_error.is_none());
        assert!(failure.committed_versions.is_empty());
    }
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    const NOW: i64 = 2_000_000_000;

    struct AdapterFixture {
        directory: std::path::PathBuf,
        connection: Option<SqliteConnection>,
        state: RunState,
        samples: Arc<AtomicUsize>,
        fail_at: Arc<AtomicUsize>,
    }

    impl AdapterFixture {
        async fn close(mut self, expected: &serde_json::Value, conflict: bool) {
            use sqlx::ConnectOptions;
            self.connection.take().unwrap().close().await.unwrap();
            let members = || {
                std::fs::read_dir(&self.directory)
                    .unwrap()
                    .map(|entry| {
                        let entry = entry.unwrap();
                        let kind = entry.file_type().unwrap();
                        assert!(kind.is_file() && !kind.is_symlink());
                        (entry.file_name(), std::fs::read(entry.path()).unwrap())
                    })
                    .collect::<std::collections::BTreeMap<_, _>>()
            };
            let closed = members();
            assert_eq!(
                closed.keys().cloned().collect::<Vec<_>>(),
                [std::ffi::OsString::from("db.sqlite3")]
            );
            assert!(!self.directory.join("db.sqlite3-journal").exists());
            let data = self.directory.join("db.sqlite3");
            let mut reopened = sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&data)
                .create_if_missing(false)
                .read_only(true)
                .disable_statement_logging()
                .connect()
                .await
                .unwrap();
            assert!(!reopened.is_in_transaction());
            assert_eq!(adapter_image(&mut reopened).await, *expected);
            let journal: String = sqlx::query_scalar("PRAGMA journal_mode")
                .fetch_one(&mut reopened)
                .await
                .unwrap();
            assert_eq!(journal, "delete");
            if conflict {
                let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM objects_formal_parent")
                    .fetch_one(&mut reopened)
                    .await
                    .unwrap();
                assert_eq!(rows, 0);
            }
            reopened.close().await.unwrap();
            assert_eq!(members(), closed);
            // Kept private on failure; remove only after both actual closes.
            std::fs::remove_file(data).unwrap();
            std::fs::remove_dir(self.directory).unwrap();
        }
    }

    async fn adapter_prefix_five() -> AdapterFixture {
        use evo_core::clock::WallClock;
        use sqlx::ConnectOptions;
        let samples = Arc::new(AtomicUsize::new(0));
        let fail_at = Arc::new(AtomicUsize::new(usize::MAX));
        let observed = Arc::clone(&samples);
        let threshold = Arc::clone(&fail_at);
        let source = WallClock::fixture_seconds(Arc::new(move || {
            if observed.fetch_add(1, Ordering::SeqCst) >= threshold.load(Ordering::SeqCst) {
                NOW - 1
            } else {
                NOW
            }
        }));
        let mut clock = OperationClock::with_source(source);
        let stamp = clock.seconds(&Default::default()).unwrap();
        clock.take_seconds(stamp).unwrap();
        // File transaction controls only; no WAL/mount/publication credit.
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            assert_eq!(
                std::fs::metadata(directory.path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        // A failed test must not auto-remove a database with unknown native close.
        let directory = directory.keep();
        let mut connection = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(directory.join("db.sqlite3"))
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Delete)
            .foreign_keys(true)
            .disable_statement_logging()
            .connect()
            .await
            .unwrap();
        let inputs = PreparePreimages::default();
        let mut state = {
            let mut driver = ClockedMigrations {
                conn: &mut connection,
                inputs: &inputs,
                state: RunState {
                    clock,
                    ..Default::default()
                },
            };
            let start = samples.load(Ordering::SeqCst);
            driver.ensure_migrations_table().await.unwrap();
            assert_eq!(samples.load(Ordering::SeqCst) - start, 1);
            for migration in MIGRATOR.iter().filter(|migration| migration.version <= 5) {
                let start = samples.load(Ordering::SeqCst);
                driver.apply(migration).await.unwrap();
                assert_eq!(samples.load(Ordering::SeqCst) - start, 3);
            }
            assert_eq!(driver.state.committed_versions, [1, 2, 4, 5]);
            driver.state
        };
        let mut tx = connection.begin_with("BEGIN IMMEDIATE").await.unwrap();
        crate::write_clock::seconds_in_transaction(&mut tx, &inputs, &mut state.clock)
            .await
            .unwrap();
        sqlx::query("INSERT INTO objects(namespace,kind,id,owner,body,revision) VALUES('n','artifact','kept','fixture',' { \"id\" : \"kept\", \"v\" : 1 } ',19)")
            .execute(&mut *tx).await.unwrap();
        crate::write_clock::finish_transaction(tx, &inputs, &mut state.clock, true)
            .await
            .unwrap();
        AdapterFixture {
            directory,
            connection: Some(connection),
            state,
            samples,
            fail_at,
        }
    }

    async fn adapter_image(connection: &mut SqliteConnection) -> serde_json::Value {
        let schema: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name,tbl_name",
        )
        .fetch_all(&mut *connection)
        .await
        .unwrap();
        let ledger: Vec<(i64, String, String, i64, Vec<u8>, i64)> = sqlx::query_as(
            "SELECT version,description,installed_on,success,checksum,execution_time FROM _sqlx_migrations ORDER BY version",
        ).fetch_all(&mut *connection).await.unwrap();
        let objects: Vec<(i64, String, String, String, String, String, i64)> = sqlx::query_as(
            "SELECT rowid,namespace,kind,id,owner,body,revision FROM objects ORDER BY rowid",
        )
        .fetch_all(&mut *connection)
        .await
        .unwrap();
        serde_json::json!({"schema":schema,"ledger":ledger,"objects":objects})
    }

    #[tokio::test]
    async fn committed_six_survives_seven_post_ddl_admission_failure() {
        let mut fixture = adapter_prefix_five().await;
        let inputs = PreparePreimages::default();
        let expected = {
            let mut driver = ClockedMigrations {
                conn: fixture.connection.as_mut().unwrap(),
                inputs: &inputs,
                state: std::mem::take(&mut fixture.state),
            };
            let six = MIGRATOR
                .iter()
                .find(|migration| migration.version == 6)
                .unwrap();
            let start = fixture.samples.load(Ordering::SeqCst);
            driver.apply(six).await.unwrap();
            assert_eq!(fixture.samples.load(Ordering::SeqCst) - start, 3);
            let before = adapter_image(driver.conn).await;
            assert_eq!(before["ledger"].as_array().unwrap().last().unwrap()[0], 6);
            let start = fixture.samples.load(Ordering::SeqCst);
            fixture.fail_at.store(start + 1, Ordering::SeqCst);
            let seven = MIGRATOR
                .iter()
                .find(|migration| migration.version == 7)
                .unwrap();
            let result = driver.apply(seven).await.map(|_| ());
            // Sample two follows actual DDL and Applying qualification.
            assert_eq!(fixture.samples.load(Ordering::SeqCst) - start, 2);
            let failure = driver.state.finish(result).unwrap_err();
            assert!(matches!(failure.error, Error::ClockRegressed));
            assert!(failure.rollback_error.is_none());
            assert!(!failure.commit_outcome_unknown);
            assert_eq!(failure.committed_versions, [1, 2, 4, 5, 6]);
            assert!(!driver.conn.is_in_transaction());
            assert_eq!(adapter_image(driver.conn).await, before);
            before
        };
        fixture.close(&expected, false).await;
    }

    #[tokio::test]
    async fn rogue_sql_and_conflicting_prefix_have_no_effects() {
        for conflicting_prefix in [false, true] {
            let mut fixture = adapter_prefix_five().await;
            let inputs = PreparePreimages::default();
            let connection = fixture.connection.as_mut().unwrap();
            let migration = if conflicting_prefix {
                sqlx::query("CREATE TABLE objects_formal_parent(x)")
                    .execute(&mut *connection)
                    .await
                    .unwrap();
                MIGRATOR.iter().find(|m| m.version == 6).unwrap().clone()
            } else {
                let original = MIGRATOR.iter().find(|m| m.version == 7).unwrap().clone();
                let rogue = Migration::new(
                    original.version,
                    original.description.clone(),
                    original.migration_type,
                    Cow::Borrowed("CREATE TABLE rogue(x);\nCOMMIT;\n"),
                    original.no_tx,
                );
                assert_ne!(rogue.checksum, original.checksum);
                rogue
            };
            let before = adapter_image(connection).await;
            let changes: i64 = sqlx::query_scalar("SELECT total_changes()")
                .fetch_one(&mut *connection)
                .await
                .unwrap();
            let start = fixture.samples.load(Ordering::SeqCst);
            {
                let mut driver = ClockedMigrations {
                    conn: connection,
                    inputs: &inputs,
                    state: std::mem::take(&mut fixture.state),
                };
                let result = driver.apply(&migration).await.map(|_| ());
                let failure = driver.state.finish(result).unwrap_err();
                assert!(matches!(failure.error, Error::ClockRegressed));
                assert!(failure.rollback_error.is_none());
                assert!(!failure.commit_outcome_unknown);
                assert_eq!(failure.committed_versions, [1, 2, 4, 5]);
                assert_eq!(fixture.samples.load(Ordering::SeqCst), start);
                assert!(!driver.conn.is_in_transaction());
                assert_eq!(adapter_image(driver.conn).await, before);
                let after: i64 = sqlx::query_scalar("SELECT total_changes()")
                    .fetch_one(&mut *driver.conn)
                    .await
                    .unwrap();
                assert_eq!(after, changes);
            }
            fixture.close(&before, conflicting_prefix).await;
        }
        // The old Python rogue case began on a committed 0006 prefix too.
        let mut fixture = adapter_prefix_five().await;
        let inputs = PreparePreimages::default();
        let expected = {
            let mut driver = ClockedMigrations {
                conn: fixture.connection.as_mut().unwrap(),
                inputs: &inputs,
                state: std::mem::take(&mut fixture.state),
            };
            let six = MIGRATOR.iter().find(|m| m.version == 6).unwrap();
            driver.apply(six).await.unwrap();
            assert_eq!(driver.state.committed_versions, [1, 2, 4, 5, 6]);
            let original = MIGRATOR.iter().find(|m| m.version == 7).unwrap();
            let rogue = Migration::new(
                original.version,
                original.description.clone(),
                original.migration_type,
                Cow::Borrowed("CREATE TABLE rogue(x);\nCOMMIT;\n"),
                original.no_tx,
            );
            assert_ne!(rogue.checksum, original.checksum);
            let before = adapter_image(driver.conn).await;
            let changes: i64 = sqlx::query_scalar("SELECT total_changes()")
                .fetch_one(&mut *driver.conn)
                .await
                .unwrap();
            let start = fixture.samples.load(Ordering::SeqCst);
            let result = driver.apply(&rogue).await.map(|_| ());
            let failure = driver.state.finish(result).unwrap_err();
            assert!(matches!(failure.error, Error::ClockRegressed));
            assert!(failure.rollback_error.is_none());
            assert!(!failure.commit_outcome_unknown);
            assert_eq!(failure.committed_versions, [1, 2, 4, 5, 6]);
            assert_eq!(fixture.samples.load(Ordering::SeqCst), start);
            assert!(!driver.conn.is_in_transaction());
            assert_eq!(adapter_image(driver.conn).await, before);
            let after: i64 = sqlx::query_scalar("SELECT total_changes()")
                .fetch_one(&mut *driver.conn)
                .await
                .unwrap();
            assert_eq!(after, changes);
            before
        };
        fixture.close(&expected, false).await;
    }
}
