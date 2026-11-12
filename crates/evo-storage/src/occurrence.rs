//! The registered occurrence-time columns (§7.3.1, Q-41/Q-54). The clock floor
//! reads only these entries; any other column never moves it. Deliberately not
//! registered: `_sqlx_migrations.execution_time` (a duration),
//! `root_budget_calls.lease_until` and `Job.lease_until` (lease expiry),
//! `Job.deadline` (a deadline), retention expiry, and `ExperimentPlan.frozen_at`
//! (unit undefined). A new persisted type with an occurrence time registers it
//! here in the change that introduces it.
use evo_core::{Error, Result};
use sqlx::{Row, SqliteConnection};

/// Which rows of `table` an entry reads.
#[derive(Debug, Clone, Copy)]
pub enum OccurrenceSelector {
    Row,
    /// `objects` rows of this kind whose body carries this `schema_version`.
    Kind {
        kind: &'static str,
        schema: &'static str,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum OccurrencePath {
    Column(&'static str),
    Json {
        column: &'static str,
        path: &'static str,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum OccurrenceFormat {
    UnixSeconds,
    UnixMillis,
    /// SQLite `CURRENT_TIMESTAMP` text, `YYYY-MM-DD HH:MM:SS` in UTC.
    SqliteUtcText,
}

#[derive(Debug, Clone, Copy)]
pub struct OccurrenceTimeColumn {
    pub table: &'static str,
    pub selector: OccurrenceSelector,
    pub path: OccurrencePath,
    pub format: OccurrenceFormat,
    /// SQL NULL, a JSON null or an absent JSON field is allowed only here.
    pub nullable: bool,
}

const fn column(table: &'static str, name: &'static str, nullable: bool) -> OccurrenceTimeColumn {
    OccurrenceTimeColumn {
        table,
        selector: OccurrenceSelector::Row,
        path: OccurrencePath::Column(name),
        format: OccurrenceFormat::UnixSeconds,
        nullable,
    }
}

const fn body(
    kind: &'static str,
    schema: &'static str,
    path: &'static str,
) -> OccurrenceTimeColumn {
    OccurrenceTimeColumn {
        table: "objects",
        selector: OccurrenceSelector::Kind { kind, schema },
        path: OccurrencePath::Json {
            column: "body",
            path,
        },
        format: OccurrenceFormat::UnixSeconds,
        nullable: false,
    }
}

/// The one registry. Typed `objects` bodies are registered per verified
/// writer; the bodies not listed yet only leave the floor lower (AG-132b).
pub const OCCURRENCE_TIME_COLUMNS: &[OccurrenceTimeColumn] = &[
    OccurrenceTimeColumn {
        table: "_sqlx_migrations",
        selector: OccurrenceSelector::Row,
        path: OccurrencePath::Column("installed_on"),
        format: OccurrenceFormat::SqliteUtcText,
        nullable: false,
    },
    OccurrenceTimeColumn {
        table: "audit",
        selector: OccurrenceSelector::Row,
        path: OccurrencePath::Json {
            column: "payload",
            path: "$.timestamp",
        },
        format: OccurrenceFormat::UnixSeconds,
        nullable: false,
    },
    column("root_budgets", "created_at", false),
    column("root_budgets", "stop_committed_at", true),
    column("root_budget_dispatch_groups", "created_at", false),
    column("root_budget_dispatch_groups", "stop_committed_at", true),
    column("root_budget_calls", "created_at", false),
    column("root_budget_calls", "dispatched_at", true),
    column("root_budget_calls", "finalized_at", true),
    column("root_budget_calls", "execution_closed_at", true),
    column("root_budget_events", "event_at", false),
    // Only the sealed event's details carry it.
    OccurrenceTimeColumn {
        table: "root_budget_events",
        selector: OccurrenceSelector::Row,
        path: OccurrencePath::Json {
            column: "details",
            path: "$.sealed_at",
        },
        format: OccurrenceFormat::UnixSeconds,
        nullable: true,
    },
    column("revoke_cleanup_jobs", "created_at", false),
    column("revoke_cleanup_jobs", "updated_at", false),
    column("revoke_cleanup_events", "event_at", false),
    column("root_budget_stage_plan_bindings", "created_at", false),
    column("root_budget_stage_plan_controls", "created_at", false),
    column("root_budget_stage_plan_calls", "created_at", false),
    body(
        "tombstone",
        crate::lifecycle::REVOKE_TOMBSTONE_SCHEMA,
        "$.created_at",
    ),
    body("job", crate::MANAGEMENT_JOB_V1_SCHEMA, "$.created_at"),
    body("job", crate::MANAGEMENT_JOB_V2_SCHEMA, "$.created_at"),
];

/// Highest registered occurrence time in Unix ms, read whole. Any registered
/// value that is unparsable, before the epoch or out of range refuses instead
/// of counting as 0 or being skipped. Tables a migration has not created yet
/// have no entries to read.
pub async fn occurrence_floor_millis(conn: &mut SqliteConnection) -> Result<Option<i64>> {
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_schema WHERE type='table'")
            .fetch_all(&mut *conn)
            .await
            .map_err(crate::internal)?;
    let mut floor = None;
    for entry in OCCURRENCE_TIME_COLUMNS {
        if !tables.iter().any(|table| table == entry.table) {
            continue;
        }
        let (type_of, value) = match entry.path {
            OccurrencePath::Column(name) => (format!("typeof({name})"), name.to_owned()),
            OccurrencePath::Json { column, path } => (
                format!("json_type({column},'{path}')"),
                format!("json_extract({column},'{path}')"),
            ),
        };
        let selector = match entry.selector {
            OccurrenceSelector::Row => "1".to_owned(),
            OccurrenceSelector::Kind { kind, schema } => {
                format!("kind='{kind}' AND json_extract(body,'$.schema_version')='{schema}'")
            }
        };
        let (valid, millis) = match entry.format {
            // Seconds stay in range only while their millisecond value fits i64.
            OccurrenceFormat::UnixSeconds => (
                format!("t='integer' AND v BETWEEN 0 AND {}", i64::MAX / 1000),
                "v*1000",
            ),
            OccurrenceFormat::UnixMillis => ("t='integer' AND v >= 0".to_owned(), "v"),
            // The round trip through datetime() rejects impossible dates.
            OccurrenceFormat::SqliteUtcText => (
                "t='text' AND v GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9] [0-9][0-9]:[0-9][0-9]:[0-9][0-9]' \
                 AND datetime(v)=v AND v >= '1970-01-01 00:00:00'"
                    .to_owned(),
                "CAST(strftime('%s',v) AS INTEGER)*1000",
            ),
        };
        let row = sqlx::query(&format!(
            "SELECT COALESCE(SUM(CASE WHEN t IS NULL OR t='null' THEN {absent}
                                      WHEN {valid} THEN 0 ELSE 1 END),0) AS invalid,
                    MAX(CASE WHEN {valid} THEN {millis} END) AS floor
               FROM (SELECT {type_of} AS t, {value} AS v FROM {table} WHERE {selector})",
            absent = i64::from(!entry.nullable),
            table = entry.table,
        ))
        .fetch_one(&mut *conn)
        .await
        .map_err(crate::internal)?;
        if row.try_get::<i64, _>("invalid").map_err(crate::internal)? != 0 {
            return Err(Error::OccurrenceTimeColumnInvalid);
        }
        let entry_floor: Option<i64> = row.try_get("floor").map_err(crate::internal)?;
        floor = floor.max(entry_floor);
    }
    Ok(floor)
}
