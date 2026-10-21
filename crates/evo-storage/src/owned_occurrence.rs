//! Pure decoding and reduction of occurrence values already qualified by a caller.
//!
//! This module neither identifies owned records nor samples a clock. Keep the
//! original JSON bytes for witnesses and hashes; parsed values are read views.
#[path = "e05_issue.rs"]
pub(crate) mod e05_issue;
use evo_core::{Error, Result};
use serde::{
    Deserialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Number, Value};
use std::fmt;

const NANOS_PER_SECOND: i128 = 1_000_000_000;
const NANOS_PER_MILLISECOND: i128 = 1_000_000;

/// Maximum qualified occurrence, with no invented epoch for an empty scan.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub(super) struct OccurrenceFloor {
    max_nanos: Option<i128>,
}

impl OccurrenceFloor {
    pub(super) fn observe_seconds(&mut self, value: i64) -> Result<()> {
        self.observe_units(value, NANOS_PER_SECOND)
    }

    pub(super) fn observe_millis(&mut self, value: i64) -> Result<()> {
        self.observe_units(value, NANOS_PER_MILLISECOND)
    }

    pub(super) fn max_nanos(&self) -> Option<i128> {
        self.max_nanos
    }

    fn observe_units(&mut self, value: i64, nanos_per_unit: i128) -> Result<()> {
        if value < 0 {
            return Err(Error::ClockRegressed);
        }
        let nanos = i128::from(value)
            .checked_mul(nanos_per_unit)
            .ok_or(Error::ClockRegressed)?;
        // Assign only after every check, so a rejected value leaves the floor intact.
        self.max_nanos = Some(self.max_nanos.map_or(nanos, |current| current.max(nanos)));
        Ok(())
    }
}

/// Parse one complete JSON value, rejecting duplicate decoded object keys.
///
/// `Deserializer` retains serde_json's default recursion limit. No caller input
/// can select another limit, and the returned value never replaces raw evidence.
pub(super) fn parse_owned_json(raw: &str) -> Result<Value> {
    let mut deserializer = serde_json::Deserializer::from_str(raw);
    let value = StrictValue::deserialize(&mut deserializer).map_err(|_| Error::ClockRegressed)?;
    deserializer.end().map_err(|_| Error::ClockRegressed)?;
    Ok(value.0)
}

/// Require a genuine nonnegative JSON integer fitting SQLite's signed INTEGER.
/// Floating-point numbers, including `1.0`, cannot pass through `as_i64`.
pub(super) fn owned_nonnegative_integer(value: &Value) -> Result<i64> {
    value
        .as_i64()
        .filter(|integer| *integer >= 0)
        .ok_or(Error::ClockRegressed)
}

struct StrictValue(Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictValueVisitor)
    }
}

struct StrictValueVisitor;

impl<'de> Visitor<'de> for StrictValueVisitor {
    type Value = StrictValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object keys")
    }

    fn visit_bool<E>(self, value: bool) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(Number::from(value))))
    }

    fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(Number::from(value))))
    }

    fn visit_f64<E>(self, value: f64) -> std::result::Result<Self::Value, E>
    where
        E: de::Error,
    {
        Number::from_f64(value)
            .map(|number| StrictValue(Value::Number(number)))
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::String(value)))
    }

    fn visit_unit<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_none<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<StrictValue>()? {
            values.push(value.0);
        }
        Ok(StrictValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            // Compare decoded names, so escaped spellings cannot conceal a duplicate.
            if values.contains_key(&key) {
                return Err(de::Error::custom("duplicate JSON object key"));
            }
            let value = object.next_value::<StrictValue>()?;
            values.insert(key, value.0);
        }
        Ok(StrictValue(Value::Object(values)))
    }
}

// The collector is a read-only operation on a caller-owned tracked transaction.
use evo_core::prepare_contract::{HostPrepareConfig, PrepareOperation};
use evo_core::{canonical_fingerprint, fingerprint, hash, identifier};
use serde::de::DeserializeOwned;
use sqlx::{Column, Connection, Row, SqliteConnection, TypeInfo, ValueRef};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default, Debug, Clone)]
pub(super) struct PreparePreimages {
    pub(super) candidates: Vec<HostPrepareConfig>,
    pub(super) requests: Vec<evo_core::Prepare>,
    pub(super) import_requests: Vec<Value>,
}

#[derive(Clone, Copy)]
pub(super) enum SchemaRead<'a> {
    Stable,
    Applying(&'a sqlx::migrate::Migration),
}

fn clock<T>(result: impl Into<std::result::Result<T, sqlx::Error>>) -> Result<T> {
    result.into().map_err(|_| Error::ClockRegressed)
}
fn require(condition: bool) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Error::ClockRegressed)
    }
}
fn decode<T: DeserializeOwned>(value: &Value) -> Result<T> {
    serde_json::from_value(value.clone()).map_err(|_| Error::ClockRegressed)
}
fn digest(value: &str) -> Result<()> {
    require(
        value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
    )
}
fn exact(value: &Value, fields: &[&str], optional: &[&str]) -> Result<()> {
    let object = value.as_object().ok_or(Error::ClockRegressed)?;
    require(
        fields.iter().all(|field| object.contains_key(*field))
            && object
                .keys()
                .all(|key| fields.contains(&key.as_str()) || optional.contains(&key.as_str())),
    )
}
fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or(Error::ClockRegressed)
}
fn array<'a>(value: &'a Value, field: &str) -> Result<&'a [Value]> {
    value
        .get(field)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or(Error::ClockRegressed)
}
fn nonnegative(value: &Value, field: &str) -> Result<i64> {
    owned_nonnegative_integer(value.get(field).ok_or(Error::ClockRegressed)?)
}

fn typed_fingerprint<T: serde::Serialize>(value: &T) -> Result<String> {
    fingerprint(value).map_err(|_| Error::ClockRegressed)
}

/// SQLite's migration timestamp format, with no locale, timezone or float coercion.
pub(super) fn parse_sqlite_utc_seconds(raw: &str) -> Result<i64> {
    let bytes = raw.as_bytes();
    require(bytes.len() == 19)?;
    for (index, byte) in bytes.iter().copied().enumerate() {
        match index {
            4 | 7 => require(byte == b'-')?,
            10 => require(byte == b' ')?,
            13 | 16 => require(byte == b':')?,
            _ => require(byte.is_ascii_digit())?,
        }
    }
    let part = |start: usize, end: usize| -> Result<i128> {
        bytes[start..end].iter().try_fold(0_i128, |n, byte| {
            n.checked_mul(10)
                .and_then(|n| n.checked_add(i128::from(byte - b'0')))
                .ok_or(Error::ClockRegressed)
        })
    };
    let year = part(0, 4)?;
    let month = part(5, 7)?;
    let day = part(8, 10)?;
    let hour = part(11, 13)?;
    let minute = part(14, 16)?;
    let second = part(17, 19)?;
    require(
        (1970..=9999).contains(&year)
            && (1..=12).contains(&month)
            && hour < 24
            && minute < 60
            && second < 60,
    )?;
    let leap = |year: i128| year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let months = [
        31_i128,
        if leap(year) { 29 } else { 28 },
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
    let month_index = usize::try_from(month - 1).map_err(|_| Error::ClockRegressed)?;
    require(day >= 1 && day <= months[month_index])?;
    let prior = year - 1;
    let days_to_year = prior
        .checked_mul(365)
        .and_then(|n| n.checked_add(prior / 4 - prior / 100 + prior / 400))
        .ok_or(Error::ClockRegressed)?;
    let epoch_prior = 1969_i128;
    let epoch_days = epoch_prior * 365 + epoch_prior / 4 - epoch_prior / 100 + epoch_prior / 400;
    let days = days_to_year
        .checked_sub(epoch_days)
        .and_then(|n| n.checked_add(months[..month_index].iter().sum::<i128>()))
        .and_then(|n| n.checked_add(day - 1))
        .ok_or(Error::ClockRegressed)?;
    let seconds = days
        .checked_mul(86_400)
        .and_then(|n| n.checked_add(hour * 3_600 + minute * 60 + second))
        .ok_or(Error::ClockRegressed)?;
    i64::try_from(seconds).map_err(|_| Error::ClockRegressed)
}

fn compiled_migrations() -> Vec<sqlx::migrate::Migration> {
    use sqlx::migrate::{Migration, MigrationType};
    [
        (1, "runtime", include_str!("../migrations/0001_runtime.sql")),
        (
            2,
            "revoke graph",
            include_str!("../migrations/0002_revoke_graph.sql"),
        ),
        (
            4,
            "replay worlds",
            include_str!("../migrations/0004_replay_worlds.sql"),
        ),
        (
            5,
            "root budget",
            include_str!("../migrations/0005_root_budget.sql"),
        ),
        (
            6,
            "revoke cleanup",
            include_str!("../migrations/0006_revoke_cleanup.sql"),
        ),
        (
            7,
            "formal parent kind",
            include_str!("../migrations/0007_formal_parent_kind.sql"),
        ),
    ]
    .into_iter()
    .map(|(version, description, sql)| {
        Migration::new(
            version,
            description.into(),
            MigrationType::Simple,
            sql.into(),
            false,
        )
    })
    .collect()
}

// Token comparison preserves string literals and CHECK/UNIQUE/foreign-key clauses.
// It ignores only SQL whitespace/comments, identifier quoting/case and IF NOT EXISTS.
fn sql_tokens(sql: &str) -> Result<Vec<String>> {
    let chars: Vec<char> = sql.chars().collect();
    let mut index = 0;
    let mut tokens = Vec::new();
    while index < chars.len() {
        let c = chars[index];
        if c.is_whitespace() {
            index += 1;
            continue;
        }
        if c == '-' && chars.get(index + 1) == Some(&'-') {
            while index < chars.len() && chars[index] != '\n' {
                index += 1;
            }
            continue;
        }
        if c == '/' && chars.get(index + 1) == Some(&'*') {
            index += 2;
            while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                index += 1;
            }
            require(index + 1 < chars.len())?;
            index += 2;
            continue;
        }
        if matches!(c, '\'' | '"' | '`' | '[') {
            let end = if c == '[' { ']' } else { c };
            index += 1;
            let mut token = String::new();
            loop {
                let current = *chars.get(index).ok_or(Error::ClockRegressed)?;
                index += 1;
                if current == end {
                    if chars.get(index) == Some(&end) && end != ']' {
                        token.push(end);
                        index += 1;
                        continue;
                    }
                    break;
                }
                token.push(current);
            }
            tokens.push(if c == '\'' {
                format!("'{}'", token.replace('\'', "''"))
            } else {
                token.to_ascii_uppercase()
            });
        } else if c.is_ascii_alphanumeric() || c == '_' {
            let start = index;
            while index < chars.len()
                && (chars[index].is_ascii_alphanumeric() || chars[index] == '_')
            {
                index += 1;
            }
            tokens.push(
                chars[start..index]
                    .iter()
                    .collect::<String>()
                    .to_ascii_uppercase(),
            );
        } else {
            tokens.push(c.to_string());
            index += 1;
        }
    }
    let mut clean = Vec::new();
    let mut at = 0;
    while at < tokens.len() {
        if tokens.get(at).map(String::as_str) == Some("IF")
            && tokens.get(at + 1).map(String::as_str) == Some("NOT")
            && tokens.get(at + 2).map(String::as_str) == Some("EXISTS")
        {
            at += 3;
        } else {
            clean.push(tokens[at].clone());
            at += 1;
        }
    }
    while clean.last().map(String::as_str) == Some(";") {
        clean.pop();
    }
    Ok(clean)
}

fn expected_schema(
    migrations: &[sqlx::migrate::Migration],
) -> Result<BTreeMap<String, Vec<String>>> {
    let mut result = BTreeMap::new();
    for migration in migrations {
        let tokens = sql_tokens(&migration.sql)?;
        for statement in tokens.split(|token| token == ";") {
            if statement.starts_with(&["CREATE".into(), "TABLE".into()]) {
                let mut statement = statement.to_vec();
                let name = statement.get(2).ok_or(Error::ClockRegressed)?.clone();
                let name = if name == "OBJECTS_FORMAL_PARENT" {
                    statement[2] = "OBJECTS".into();
                    "OBJECTS".into()
                } else {
                    name
                };
                result.insert(name, statement);
            } else if statement.starts_with(&["CREATE".into(), "INDEX".into()]) {
                result.insert(
                    statement.get(2).ok_or(Error::ClockRegressed)?.clone(),
                    statement.to_vec(),
                );
            } else if statement.starts_with(&["CREATE".into(), "VIRTUAL".into(), "TABLE".into()]) {
                result.insert(
                    statement.get(3).ok_or(Error::ClockRegressed)?.clone(),
                    statement.to_vec(),
                );
            }
        }
    }
    if !migrations.is_empty() {
        for sql in [
            "CREATE TABLE sqlite_sequence(name,seq)",
            "CREATE TABLE 'skill_fts_data'(id INTEGER PRIMARY KEY, block BLOB)",
            "CREATE TABLE 'skill_fts_idx'(segid, term, pgno, PRIMARY KEY(segid, term)) WITHOUT ROWID",
            "CREATE TABLE 'skill_fts_content'(id INTEGER PRIMARY KEY, c0, c1, c2)",
            "CREATE TABLE 'skill_fts_docsize'(id INTEGER PRIMARY KEY, sz BLOB)",
            "CREATE TABLE 'skill_fts_config'(k PRIMARY KEY, v) WITHOUT ROWID",
        ] {
            let mut tokens = sql_tokens(sql)?;
            // SQLite uses single-quoted identifiers in generated FTS table DDL.
            tokens[2] = tokens[2].trim_matches('\'').to_ascii_uppercase();
            result.insert(tokens[2].clone(), tokens);
        }
    }
    Ok(result)
}

fn rows(database: &RawDatabase, table: &str, columns: &[&str]) -> Result<Vec<OwnedRow>> {
    let mut rows = database
        .tables
        .get(table)
        .ok_or(Error::ClockRegressed)?
        .clone();
    for row in &mut rows {
        for column in columns {
            let cell = row.cells.get(*column).ok_or(Error::ClockRegressed)?;
            row.cells.insert(
                format!("__clock_type_{column}"),
                OwnedCell::Text(cell.sqlite_type().into()),
            );
        }
    }
    Ok(rows)
}
fn sql_type(row: &OwnedRow, column: &str) -> Result<String> {
    clock(row.try_get(format!("__clock_type_{column}").as_str()))
}
fn sql_string(row: &OwnedRow, column: &str) -> Result<String> {
    require(sql_type(row, column)? == "text")?;
    clock(row.try_get(column))
}
fn sql_integer(row: &OwnedRow, column: &str) -> Result<i64> {
    require(sql_type(row, column)? == "integer")?;
    clock(row.try_get(column))
}
fn sql_optional_integer(row: &OwnedRow, column: &str) -> Result<Option<i64>> {
    match sql_type(row, column)?.as_str() {
        "null" => Ok(None),
        "integer" => Ok(Some(clock(row.try_get(column))?)),
        _ => Err(Error::ClockRegressed),
    }
}
fn sql_optional_string(row: &OwnedRow, column: &str) -> Result<Option<String>> {
    match sql_type(row, column)?.as_str() {
        "null" => Ok(None),
        "text" => Ok(Some(clock(row.try_get(column))?)),
        _ => Err(Error::ClockRegressed),
    }
}
fn sql_bool(row: &OwnedRow, column: &str) -> Result<bool> {
    match sql_integer(row, column)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Error::ClockRegressed),
    }
}
fn observe_sql(
    floor: &mut OccurrenceFloor,
    row: &OwnedRow,
    column: &str,
    nullable: bool,
) -> Result<Option<i64>> {
    let value = sql_optional_integer(row, column)?;
    if let Some(value) = value {
        floor.observe_seconds(value)?;
    } else {
        require(nullable)?;
    }
    Ok(value)
}

fn schema_prefix(
    conn: &RawDatabase,
    view: SchemaRead<'_>,
    floor: &mut OccurrenceFloor,
) -> Result<usize> {
    let schema = rows(conn, "sqlite_schema", &[])?;
    let has_ledger = schema
        .iter()
        .any(|row| row.try_get::<String, _>("name").ok().as_deref() == Some("_sqlx_migrations"));
    let compiled = compiled_migrations();
    let mut prefix = 0;
    if has_ledger {
        let ledger = rows(
            conn,
            "_sqlx_migrations",
            &[
                "version",
                "description",
                "installed_on",
                "success",
                "checksum",
                "execution_time",
            ],
        )?;
        let mut ledger: Vec<_> = ledger.iter().collect();
        ledger.sort_by_key(|row| row.try_get::<i64, _>("version").unwrap_or(i64::MIN));
        for row in ledger {
            let migration = compiled.get(prefix).ok_or(Error::ClockRegressed)?;
            require(
                sql_integer(row, "version")? == migration.version
                    && sql_string(row, "description")? == migration.description
                    && sql_bool(row, "success")?
                    && sql_type(row, "checksum")? == "blob"
                    && clock(row.try_get::<Vec<u8>, _>("checksum"))? == migration.checksum.as_ref(),
            )?;
            // SQLx commits -1 when duration has not yet been recorded; a
            // disconnect can leave that exact marker in an otherwise valid ledger.
            // This duration is never an occurrence and does not raise the floor.
            require(sql_integer(row, "execution_time")? >= -1)?;
            floor.observe_seconds(parse_sqlite_utc_seconds(&sql_string(row, "installed_on")?)?)?;
            prefix += 1;
        }
    }
    let schema_prefix = match view {
        SchemaRead::Stable => prefix,
        SchemaRead::Applying(actual) => {
            let expected = compiled.get(prefix).ok_or(Error::ClockRegressed)?;
            require(
                has_ledger
                    && actual.version == expected.version
                    && actual.description == expected.description
                    && actual.sql == expected.sql
                    && actual.checksum == expected.checksum
                    && actual.migration_type == expected.migration_type
                    && actual.no_tx == expected.no_tx,
            )?;
            prefix.checked_add(1).ok_or(Error::ClockRegressed)?
        }
    };
    let mut expected = expected_schema(&compiled[..schema_prefix])?;
    if has_ledger {
        expected.insert("_SQLX_MIGRATIONS".into(), sql_tokens("CREATE TABLE _sqlx_migrations (version BIGINT PRIMARY KEY, description TEXT NOT NULL, installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, success BOOLEAN NOT NULL, checksum BLOB NOT NULL, execution_time BIGINT NOT NULL)")?);
    }
    let mut autoindexes = expected_autoindexes(schema_prefix, has_ledger);
    for row in schema {
        let name: String = clock(row.try_get("name"))?;
        let kind: String = clock(row.try_get("type"))?;
        let source: Option<String> = clock(row.try_get("sql"))?;
        if name.starts_with("sqlite_autoindex_") {
            let table = autoindexes.remove(&name).ok_or(Error::ClockRegressed)?;
            require(
                kind == "index"
                    && source.is_none()
                    && clock(row.try_get::<String, _>("tbl_name"))? == table,
            )?;
            continue;
        }
        let expected = expected
            .remove(&name.to_ascii_uppercase())
            .ok_or(Error::ClockRegressed)?;
        let (expected_kind, table) = if expected.get(1).map(String::as_str) == Some("INDEX") {
            (
                "index",
                expected.get(4).ok_or(Error::ClockRegressed)?.as_str(),
            )
        } else {
            ("table", name.as_str())
        };
        require(
            kind == expected_kind
                && clock(row.try_get::<String, _>("tbl_name"))?.eq_ignore_ascii_case(table),
        )?;
        let mut actual = sql_tokens(source.as_deref().ok_or(Error::ClockRegressed)?)?;
        if name.starts_with("skill_fts_") && actual.get(2).is_some() {
            actual[2] = actual[2].trim_matches('\'').to_ascii_uppercase();
        }
        require(actual == expected)?;
    }
    require(expected.is_empty() && autoindexes.is_empty())?;
    Ok(schema_prefix)
}

#[derive(Clone)]
struct OccurrenceObject {
    namespace: String,
    kind: String,
    id: String,
    owner: String,
    body: Value,
    // Private pure Value bytes digest, never a witness/role/qualification result.
    // Only the two fresh actual-raw snapshot builders populate Some; all
    // projections and body/context changes explicitly keep or reset None.
    body_value_digest_r26: Option<String>,
}
impl OccurrenceObject {
    fn value_body_digest_r26(&self) -> Result<String> {
        match &self.body_value_digest_r26 {
            Some(digest) => Ok(digest.clone()),
            // Preserve the original fallible codec and Error::Internal here.
            None => canonical_fingerprint(&self.body),
        }
    }
    fn replace_body_r26(&mut self, body: Value) {
        self.body_value_digest_r26 = None;
        self.body = body;
    }
}
#[derive(Default)]
struct OccurrenceSnapshot {
    // Ephemeral sealed evidence from one native writer's exact acknowledged prefix.
    // Stable readers and ordinary Sessions always leave this None.
    e05_phase: Option<e05_issue::E05PhaseEvidence>,
    native_consumer_origins_r49:
        BTreeMap<(String, String, String), e05_issue::NativeConsumerOriginR49>,
    complete_origins_r24: BTreeMap<(String, String), e05_issue::CompleteOriginR24>,
    complete_pending_r24: Option<e05_issue::CompletePendingR24>,
    objects: BTreeMap<(String, String, String), OccurrenceObject>,
    witnesses: BTreeMap<(String, String, String), (String, String)>,
    replay_owners: BTreeSet<(String, String, String, String)>,
    roots: BTreeMap<String, crate::budget::RootBudgetRecord>,
    calls: BTreeMap<(String, String), crate::budget::BudgetCallRecord>,
    groups: BTreeMap<(String, String), DispatchGroupRecord>,
    caches: Vec<OccurrenceCache>,
    watermarks: BTreeMap<String, (u64, String)>,
    edges: BTreeSet<(String, String, String, String, String)>,
    replay_worlds: BTreeMap<(String, String), evo_core::replay::ReplayWorldV2>,
    redacted_replay_worlds: BTreeMap<(String, String), (bool, Value)>,
}

// One immutable qualification object pass only. The shared borrow fixes every
// source, namespace lookup and phase for this set's entire lifetime. Never
// serialized, stored on a snapshot/session, or reused by another capture/phase/prepare.
struct TicketRelationScan<'snapshot> {
    snapshot: &'snapshot OccurrenceSnapshot,
    checked_relations: BTreeSet<(String, String)>,
    successful_tickets: BTreeMap<(String, String), MirrorEvaluationTicketV2>,
}

impl OccurrenceSnapshot {
    fn precompute_actual_body_digests_r26(&mut self) {
        // Called only after a fresh raw DB's original object/edge/budget/cache/
        // origin capture succeeded, before the unchanged qualification pass.
        // This caches NO validation or authority. Value->Value canonicalization
        // is byte-identical to fingerprint(Value) under the existing codec.
        // Do not introduce an earlier refusal: any error leaves None so the
        // original canonical call at its original use site reports it.
        for object in self.objects.values_mut() {
            object.body_value_digest_r26 = fingerprint(&object.body).ok();
        }
    }
    fn object(&self, namespace: &str, kind: &str, id: &str) -> Result<&OccurrenceObject> {
        self.objects
            .get(&(namespace.into(), kind.into(), id.into()))
            .ok_or(Error::ClockRegressed)
    }
    fn witness(&self, object: &OccurrenceObject, role: &str) -> Result<()> {
        let role_prefix = match role {
            "host" => "formal.host.content.v1",
            "admin" => "formal.admin.content.v1",
            "evaluator" => "formal.evaluator.content.v1",
            _ => return Err(Error::ClockRegressed),
        };
        let subject = typed_fingerprint(&(&object.namespace, &object.kind, &object.id))?;
        let operation = format!("{role_prefix}.{subject}");
        let content = object
            .value_body_digest_r26()
            .map_err(|_| Error::ClockRegressed)?;
        let entity = typed_fingerprint(&(&object.namespace, &object.kind, &object.id, content))?;
        require(
            self.witnesses
                .get(&(object.namespace.clone(), role.into(), operation))
                .is_some_and(|(actual, _)| actual == &entity),
        )
    }
}

fn collect_audit(
    conn: &RawDatabase,
    floor: &mut OccurrenceFloor,
    snapshot: &mut OccurrenceSnapshot,
) -> Result<()> {
    let audit = rows(
        conn,
        "audit",
        &["seq", "namespace", "payload", "previous_hash", "digest"],
    )?;
    let mut audit: Vec<_> = audit.iter().collect();
    audit.sort_by_key(|row| row.try_get::<i64, _>("seq").unwrap_or(i64::MIN));
    let mut previous = BTreeMap::<String, (i64, String)>::new();
    for row in audit {
        let seq = sql_integer(row, "seq")?;
        let namespace = sql_string(row, "namespace")?;
        identifier(&namespace).map_err(|_| Error::ClockRegressed)?;
        let raw = sql_string(row, "payload")?;
        let prior = sql_string(row, "previous_hash")?;
        let current = sql_string(row, "digest")?;
        digest(&current)?;
        let before = previous
            .entry(namespace.clone())
            .or_insert((0, String::new()));
        require(
            seq > before.0
                && prior == before.1
                && hash(format!("{prior}\n{raw}").as_bytes()) == current,
        )?;
        let value = parse_owned_json(&raw)?;
        exact(
            &value,
            &["actor", "role", "operation", "entity", "timestamp"],
            &[],
        )?;
        let actor = string(&value, "actor")?;
        identifier(actor).map_err(|_| Error::ClockRegressed)?;
        let role = string(&value, "role")?;
        let _: evo_core::Role = decode(&value["role"])?;
        let operation = string(&value, "operation")?;
        let entity = string(&value, "entity")?;
        floor.observe_seconds(nonnegative(&value, "timestamp")?)?;
        if matches!(role, "worker" | "admin")
            && matches!(operation, "replay.pool.register" | "replay.report.store")
        {
            snapshot.replay_owners.insert((
                namespace.clone(),
                operation.into(),
                entity.into(),
                actor.into(),
            ));
        }
        snapshot.witnesses.insert(
            (namespace, role.into(), operation.into()),
            (entity.into(), actor.into()),
        );
        *before = (seq, current);
    }
    e05_issue::load_native_consumer_origins_r49(conn, snapshot)?;
    Ok(())
}

fn collect_objects(conn: &RawDatabase, snapshot: &mut OccurrenceSnapshot) -> Result<()> {
    for row in rows(
        conn,
        "objects",
        &["namespace", "kind", "id", "owner", "body", "revision"],
    )? {
        let namespace = sql_string(&row, "namespace")?;
        let kind = sql_string(&row, "kind")?;
        let id = sql_string(&row, "id")?;
        let owner = sql_string(&row, "owner")?;
        for name in [&namespace, &kind, &id, &owner] {
            identifier(name).map_err(|_| Error::ClockRegressed)?;
        }
        require(sql_integer(&row, "revision")? > 0)?;
        let raw = sql_string(&row, "body")?;
        require(raw.len() <= 4 * 1024 * 1024)?;
        let body = parse_owned_json(&raw)?;
        let object = OccurrenceObject {
            namespace: namespace.clone(),
            kind: kind.clone(),
            id: id.clone(),
            owner,
            body,
            body_value_digest_r26: None,
        };
        require(
            snapshot
                .objects
                .insert((namespace, kind, id), object)
                .is_none(),
        )?;
    }
    Ok(())
}

/// Every row and namespace is read from this one already-open snapshot.
/// No pool acquisition, SQL mutation, clock sampling or source-authority grant occurs here.
pub(super) async fn collect_floor(
    conn: &mut SqliteConnection,
    inputs: &PreparePreimages,
    view: SchemaRead<'_>,
) -> Result<OccurrenceFloor> {
    require(conn.is_in_transaction())?;
    let database = load_sqlx_database(conn).await?;
    require(conn.is_in_transaction())?;
    collect_raw(&database, inputs, view).map_err(|_| Error::ClockRegressed)
}

pub(super) fn collect_floor_rusqlite(
    conn: &rusqlite::Connection,
    inputs: &PreparePreimages,
) -> Result<OccurrenceFloor> {
    require(!conn.is_autocommit())?;
    let database = load_rusqlite_database(conn)?;
    require(!conn.is_autocommit())?;
    collect_raw(&database, inputs, SchemaRead::Stable).map_err(|_| Error::ClockRegressed)
}

fn collect_raw(
    conn: &RawDatabase,
    inputs: &PreparePreimages,
    view: SchemaRead<'_>,
) -> Result<OccurrenceFloor> {
    let mut floor = OccurrenceFloor::default();
    let prefix = schema_prefix(conn, view, &mut floor)?;
    if prefix == 0 {
        return Ok(floor);
    }
    let mut snapshot = OccurrenceSnapshot::default();
    collect_audit(conn, &mut floor, &mut snapshot)?;
    collect_objects(conn, &mut snapshot)?;
    if prefix >= 2 {
        load_dependencies(conn, &mut snapshot)?;
    }
    if prefix >= 3 {
        collect_replay_worlds(conn, &mut snapshot)?;
    }
    if prefix >= 4 {
        collect_budget(conn, &mut floor, &mut snapshot)?;
    }
    if prefix >= 5 {
        collect_cleanup(conn, &mut floor, &snapshot)?;
    }
    load_caches(conn, &mut snapshot)?;
    e05_issue::load_complete_origins_r24(conn, &mut snapshot)?;
    snapshot.precompute_actual_body_digests_r26();
    {
        let mut ticket_scan = TicketRelationScan {
            snapshot: &snapshot,
            checked_relations: BTreeSet::new(),
            successful_tickets: BTreeMap::new(),
        };
        for object in snapshot.objects.values() {
            qualify_object_with_ticket_scan(
                object,
                &snapshot,
                inputs,
                &mut floor,
                Some(&mut ticket_scan),
            )?;
        }
    }
    collect_caches(conn, &snapshot, inputs, &mut floor)?;
    Ok(floor)
}

fn collect_budget(
    conn: &RawDatabase,
    floor: &mut OccurrenceFloor,
    snapshot: &mut OccurrenceSnapshot,
) -> Result<()> {
    let memberships = rows(
        conn,
        "root_budget_namespaces",
        &["billing_scope", "namespace"],
    )?;
    let mut namespaces = BTreeMap::<String, BTreeSet<String>>::new();
    for row in memberships {
        let scope = sql_string(&row, "billing_scope")?;
        let namespace = sql_string(&row, "namespace")?;
        identifier(&scope).map_err(|_| Error::ClockRegressed)?;
        identifier(&namespace).map_err(|_| Error::ClockRegressed)?;
        require(namespaces.entry(scope).or_default().insert(namespace))?;
    }
    for row in rows(
        conn,
        "root_budgets",
        &[
            "billing_scope",
            "root_budget_id",
            "authorizing_namespace",
            "currency",
            "pricing_version",
            "payment_subject",
            "authorization_receipt_digest",
            "per_call_cap_micros",
            "total_limit_micros",
            "spent_micros",
            "reserved_micros",
            "concurrency_limit",
            "stopped",
            "stop_reason",
            "stop_committed_at",
            "created_at",
        ],
    )? {
        let scope = sql_string(&row, "billing_scope")?;
        for field in [
            "root_budget_id",
            "authorizing_namespace",
            "currency",
            "pricing_version",
            "payment_subject",
        ] {
            identifier(&sql_string(&row, field)?).map_err(|_| Error::ClockRegressed)?;
        }
        digest(&sql_string(&row, "authorization_receipt_digest")?)?;
        let per_call = sql_integer(&row, "per_call_cap_micros")?;
        let total = sql_integer(&row, "total_limit_micros")?;
        require(
            per_call > 0
                && total >= per_call
                && sql_integer(&row, "spent_micros")? >= 0
                && sql_integer(&row, "reserved_micros")? >= 0
                && sql_integer(&row, "concurrency_limit")? == 1,
        )?;
        let stopped = sql_bool(&row, "stopped")?;
        let reason = sql_optional_string(&row, "stop_reason")?;
        let stop = observe_sql(floor, &row, "stop_committed_at", true)?;
        require(if stopped {
            reason.is_some() && stop.is_some()
        } else {
            reason.is_none() && stop.is_none()
        })?;
        observe_sql(floor, &row, "created_at", false)?;
        let allowed = namespaces
            .remove(&scope)
            .ok_or(Error::ClockRegressed)?
            .into_iter()
            .collect::<Vec<_>>();
        require(
            !allowed.is_empty()
                && allowed.len() <= 32
                && allowed.contains(&sql_string(&row, "authorizing_namespace")?),
        )?;
        let root = owned_root_from_row(&row, allowed).map_err(|_| Error::ClockRegressed)?;
        require(snapshot.roots.insert(scope, root).is_none())?;
    }
    require(namespaces.is_empty())?;
    let mut groups = BTreeMap::new();
    for row in rows(
        conn,
        "root_budget_dispatch_groups",
        &[
            "billing_scope",
            "dispatch_group_id",
            "owner_namespace",
            "stopped",
            "stop_reason",
            "stop_committed_at",
            "created_at",
        ],
    )? {
        let scope = sql_string(&row, "billing_scope")?;
        let id = sql_string(&row, "dispatch_group_id")?;
        identifier(&id).map_err(|_| Error::ClockRegressed)?;
        let root = snapshot.roots.get(&scope).ok_or(Error::ClockRegressed)?;
        require(
            root.allowed_namespaces
                .contains(&sql_string(&row, "owner_namespace")?),
        )?;
        let stopped = sql_bool(&row, "stopped")?;
        let reason = sql_optional_string(&row, "stop_reason")?;
        let stop = observe_sql(floor, &row, "stop_committed_at", true)?;
        require(if stopped {
            reason.is_some() && stop.is_some()
        } else {
            reason.is_none() && stop.is_none()
        })?;
        observe_sql(floor, &row, "created_at", false)?;
        let group = owned_dispatch_group_from_row(&row).map_err(|_| Error::ClockRegressed)?;
        require(groups.insert((scope, id), group).is_none())?;
    }
    let mut dispatch_ids = BTreeSet::new();
    let mut usage_ids = BTreeSet::new();
    for row in rows(
        conn,
        "root_budget_calls",
        &[
            "billing_scope",
            "call_id",
            "dispatch_group_id",
            "namespace",
            "stage",
            "actual_input_digest",
            "request_artifact_schema",
            "request_artifact_digest",
            "request_artifact_body",
            "reserved_micros",
            "state",
            "lease_token",
            "lease_epoch",
            "lease_until",
            "dispatch_id",
            "provider_request_id",
            "usage_record_id",
            "output_digest",
            "actual_cost_micros",
            "actual_currency",
            "actual_pricing_version",
            "execution_provenance",
            "actual_model_digest",
            "transport_artifact_schema",
            "transport_artifact_digest",
            "transport_artifact_body",
            "response_artifact_schema",
            "response_artifact_digest",
            "response_artifact_body",
            "response_usable",
            "response_block_reason",
            "execution_closed",
            "execution_closed_at",
            "execution_close_reason",
            "terminal_reason",
            "created_at",
            "dispatched_at",
            "finalized_at",
        ],
    )? {
        let scope = sql_string(&row, "billing_scope")?;
        let id = sql_string(&row, "call_id")?;
        let namespace = sql_string(&row, "namespace")?;
        let root = snapshot.roots.get(&scope).ok_or(Error::ClockRegressed)?;
        let group_id = sql_string(&row, "dispatch_group_id")?;
        let group = groups
            .get(&(scope.clone(), group_id))
            .ok_or(Error::ClockRegressed)?;
        require(
            root.allowed_namespaces.contains(&namespace) && group.owner_namespace == namespace,
        )?;
        for field in ["call_id", "namespace", "stage", "lease_token"] {
            identifier(&sql_string(&row, field)?).map_err(|_| Error::ClockRegressed)?;
        }
        digest(&sql_string(&row, "actual_input_digest")?)?;
        require(
            sql_integer(&row, "reserved_micros")? > 0
                && sql_integer(&row, "lease_epoch")? > 0
                && sql_integer(&row, "lease_until")? >= 0,
        )?;
        for field in [
            "dispatch_id",
            "provider_request_id",
            "usage_record_id",
            "actual_currency",
            "actual_pricing_version",
            "execution_provenance",
            "response_block_reason",
            "execution_close_reason",
            "terminal_reason",
        ] {
            sql_optional_string(&row, field)?;
        }
        for field in ["actual_model_digest", "output_digest"] {
            if let Some(value) = sql_optional_string(&row, field)? {
                digest(&value)?;
            }
        }
        if let Some(actual) = sql_optional_integer(&row, "actual_cost_micros")? {
            require(actual >= 0)?;
        }
        if let Some(usable) = sql_optional_integer(&row, "response_usable")? {
            require(usable == 0 || usable == 1)?;
        }
        let closed = sql_bool(&row, "execution_closed")?;
        let closed_at = observe_sql(floor, &row, "execution_closed_at", true)?;
        let close_reason = sql_optional_string(&row, "execution_close_reason")?;
        let dispatch = sql_optional_string(&row, "dispatch_id")?;
        let dispatched_at = observe_sql(floor, &row, "dispatched_at", true)?;
        let finalized_at = observe_sql(floor, &row, "finalized_at", true)?;
        observe_sql(floor, &row, "created_at", false)?;
        require(if !closed {
            closed_at.is_none() && close_reason.is_none()
        } else {
            dispatch.is_none() || (closed_at.is_some() && close_reason.is_some())
        })?;
        if let Some(dispatch) = &dispatch {
            identifier(dispatch).map_err(|_| Error::ClockRegressed)?;
            require(dispatch_ids.insert(dispatch.clone()))?;
        }
        if let Some(usage) = sql_optional_string(&row, "usage_record_id")? {
            identifier(&usage).map_err(|_| Error::ClockRegressed)?;
            require(usage_ids.insert(usage))?;
        }
        let state = sql_string(&row, "state")?;
        match state.as_str() {
            "reserved" => require(dispatch.is_none() && dispatched_at.is_none() && closed)?,
            "dispatched" | "uncertain" => require(dispatch.is_some() && dispatched_at.is_some())?,
            "finalized" => {
                require(dispatch.is_some() && dispatched_at.is_some() && finalized_at.is_some())?
            }
            "released" | "cancelled" => require(dispatch.is_none() && dispatched_at.is_none())?,
            _ => return Err(Error::ClockRegressed),
        }
        // Every non-NULL finalized_at above contributes, including non-Finalized rows.
        for prefix in ["request", "transport", "response"] {
            let schema = sql_optional_string(&row, &format!("{prefix}_artifact_schema"))?;
            let body_digest = sql_optional_string(&row, &format!("{prefix}_artifact_digest"))?;
            let body = sql_optional_string(&row, &format!("{prefix}_artifact_body"))?;
            match (schema, body_digest, body) {
                (None, None, None) => {}
                (Some(schema_version), Some(digest), Some(body)) => {
                    parse_owned_json(&body)?;
                    crate::budget::BudgetArtifact {
                        schema_version,
                        digest,
                        body,
                    }
                    .validate()
                    .map_err(|_| Error::ClockRegressed)?;
                }
                _ => return Err(Error::ClockRegressed),
            }
        }
        let call = owned_call_from_row(&row).map_err(|_| Error::ClockRegressed)?;
        require(snapshot.calls.insert((scope, id), call).is_none())?;
    }
    let mut events = BTreeMap::<String, Vec<crate::budget::RootBudgetLedgerEvent>>::new();
    let mut seals = Vec::new();
    let mut event_rows = rows(
        conn,
        "root_budget_events",
        &[
            "seq",
            "billing_scope",
            "call_id",
            "event_kind",
            "event_at",
            "details",
        ],
    )?;
    event_rows.sort_by_key(|row| row.try_get::<i64, _>("seq").unwrap_or(i64::MIN));
    for row in event_rows {
        let scope = sql_string(&row, "billing_scope")?;
        require(snapshot.roots.contains_key(&scope))?;
        let seq = sql_integer(&row, "seq")?;
        require(seq > 0)?;
        let call_id = sql_optional_string(&row, "call_id")?;
        if let Some(id) = &call_id {
            require(snapshot.calls.contains_key(&(scope.clone(), id.clone())))?;
        }
        let event_kind = sql_string(&row, "event_kind")?;
        let event_at = observe_sql(floor, &row, "event_at", false)?.ok_or(Error::ClockRegressed)?;
        let details = sql_string(&row, "details")?;
        let value = parse_owned_json(&details)?;
        if event_kind == "root_monetary_sealed_v1" {
            require(call_id.is_none())?;
            let evidence: crate::budget::SealedRootBudgetEvidence = decode(&value)?;
            require(
                evidence.schema_version == "rsia.root_budget_monetary_seal.v1"
                    && evidence.sealed_at == event_at,
            )?;
            floor.observe_seconds(evidence.sealed_at)?;
            seals.push((scope, seq, evidence));
        } else {
            events
                .entry(scope)
                .or_default()
                .push(crate::budget::RootBudgetLedgerEvent {
                    sequence: seq,
                    call_id,
                    event_kind,
                    event_at,
                    details,
                });
        }
    }
    for (scope, sequence, evidence) in seals {
        let root = snapshot.roots.get(&scope).ok_or(Error::ClockRegressed)?;
        let preceding_events = events
            .get(&scope)
            .into_iter()
            .flatten()
            .filter(|event| event.sequence < sequence)
            .cloned()
            .collect::<Vec<_>>();
        qualify_historical_root_seal_r42(&evidence, root, &preceding_events)?;
    }
    snapshot.groups = groups;
    Ok(())
}

fn collect_cleanup(
    conn: &RawDatabase,
    floor: &mut OccurrenceFloor,
    snapshot: &OccurrenceSnapshot,
) -> Result<()> {
    let mut jobs = BTreeSet::new();
    for row in rows(
        conn,
        "revoke_cleanup_jobs",
        &[
            "namespace",
            "job_id",
            "source_kind",
            "source_id",
            "watermark_seq",
            "watermark_digest",
            "state",
            "processed_nodes",
            "last_error",
            "created_at",
            "updated_at",
        ],
    )? {
        let namespace = sql_string(&row, "namespace")?;
        let id = sql_string(&row, "job_id")?;
        let kind = sql_string(&row, "source_kind")?;
        let source = sql_string(&row, "source_id")?;
        for value in [&namespace, &id, &kind, &source] {
            identifier(value).map_err(|_| Error::ClockRegressed)?;
        }
        let seq = sql_integer(&row, "watermark_seq")?;
        require(seq > 0 && sql_integer(&row, "processed_nodes")? >= 0)?;
        let watermark = sql_string(&row, "watermark_digest")?;
        digest(&watermark)?;
        require(matches!(
            sql_string(&row, "state")?.as_str(),
            "pending" | "running" | "complete" | "failed"
        ))?;
        sql_optional_string(&row, "last_error")?;
        observe_sql(floor, &row, "created_at", false)?;
        observe_sql(floor, &row, "updated_at", false)?;
        let tombstone = snapshot.object(&namespace, "tombstone", &source)?;
        let body: crate::lifecycle::RevokeTombstone = decode(&tombstone.body)?;
        require(
            body.source_kind == kind
                && body.watermark_seq == u64::try_from(seq).map_err(|_| Error::ClockRegressed)?
                && body.watermark_digest == watermark,
        )?;
        let expected =
            typed_fingerprint(&(&namespace, &kind, &source, body.watermark_seq, &watermark))?;
        require(id == format!("revoke-{}", &expected[..32]) && jobs.insert((namespace, id)))?;
    }
    for row in rows(
        conn,
        "revoke_cleanup_frontier",
        &[
            "seq",
            "namespace",
            "job_id",
            "node_kind",
            "node_id",
            "cursor_src_kind",
            "cursor_src_id",
            "expanded",
        ],
    )? {
        require(sql_integer(&row, "seq")? > 0)?;
        let namespace = sql_string(&row, "namespace")?;
        let job = sql_string(&row, "job_id")?;
        require(jobs.contains(&(namespace, job)))?;
        for field in ["node_kind", "node_id"] {
            identifier(&sql_string(&row, field)?).map_err(|_| Error::ClockRegressed)?;
        }
        require(
            sql_optional_string(&row, "cursor_src_kind")?.is_some()
                == sql_optional_string(&row, "cursor_src_id")?.is_some(),
        )?;
        sql_bool(&row, "expanded")?;
    }
    for row in rows(
        conn,
        "revoke_cleanup_events",
        &[
            "seq",
            "namespace",
            "job_id",
            "node_kind",
            "node_id",
            "event_kind",
            "event_at",
            "details",
        ],
    )? {
        require(sql_integer(&row, "seq")? > 0)?;
        require(jobs.contains(&(sql_string(&row, "namespace")?, sql_string(&row, "job_id")?)))?;
        require(
            sql_optional_string(&row, "node_kind")?.is_some()
                == sql_optional_string(&row, "node_id")?.is_some(),
        )?;
        identifier(&sql_string(&row, "event_kind")?).map_err(|_| Error::ClockRegressed)?;
        parse_owned_json(&sql_string(&row, "details")?)?;
        observe_sql(floor, &row, "event_at", false)?;
    }
    Ok(())
}

use crate::budget::{
    BudgetArtifact, BudgetCallRecord, BudgetCallState, BudgetStage, DispatchGroupRecord,
    RootBudgetLedgerEvent, RootBudgetMonetarySnapshot, RootBudgetRecord,
    RootBudgetStageMonetaryTotal, RootBudgetUnresolvedReason, RootBudgetUnresolvedRecord,
    SealedRootBudgetEvidence,
};
fn monetary_snapshot(
    root: RootBudgetRecord,
    dispatch_groups: Vec<DispatchGroupRecord>,
    calls: Vec<BudgetCallRecord>,
    ledger_events: Vec<RootBudgetLedgerEvent>,
) -> Result<RootBudgetMonetarySnapshot> {
    let groups: BTreeMap<_, _> = dispatch_groups
        .iter()
        .map(|group| (group.dispatch_group_id.as_str(), group))
        .collect();
    let mut totals: BTreeMap<_, (usize, i128, i128, i128, usize)> = BudgetStage::ALL
        .into_iter()
        .map(|stage| (stage, (0, 0, 0, 0, 0)))
        .collect();
    let mut unresolved = Vec::new();
    if !root.stopped {
        unresolved.push(root_unresolved(
            None,
            RootBudgetUnresolvedReason::RootNotStopped,
        ));
    }
    for group in &dispatch_groups {
        if !root.allowed_namespaces.contains(&group.owner_namespace) {
            let mut issue =
                root_unresolved(None, RootBudgetUnresolvedReason::GroupNamespaceMismatch);
            issue.namespace = Some(group.owner_namespace.clone());
            issue.dispatch_group_id = Some(group.dispatch_group_id.clone());
            unresolved.push(issue);
        }
    }
    for call in &calls {
        let total = totals.get_mut(&call.stage).ok_or(Error::ClockRegressed)?;
        total.0 = total.0.checked_add(1).ok_or(Error::ClockRegressed)?;
        if !root.allowed_namespaces.contains(&call.namespace) {
            unresolved.push(root_unresolved(
                Some(call),
                RootBudgetUnresolvedReason::CallNamespaceMismatch,
            ));
        }
        match groups.get(call.dispatch_group_id.as_str()) {
            None => unresolved.push(root_unresolved(
                Some(call),
                RootBudgetUnresolvedReason::MissingDispatchGroup,
            )),
            Some(group) if group.owner_namespace != call.namespace => {
                unresolved.push(root_unresolved(
                    Some(call),
                    RootBudgetUnresolvedReason::GroupNamespaceMismatch,
                ));
            }
            Some(_) => {}
        }
        let invalid_state = match call.state {
            BudgetCallState::Reserved => call.dispatch_id.is_some() || !call.execution_closed,
            BudgetCallState::Dispatched | BudgetCallState::Uncertain => {
                call.dispatch_id.is_none() || call.dispatched_at.is_none()
            }
            BudgetCallState::Finalized => {
                call.dispatch_id.is_none()
                    || call.dispatched_at.is_none()
                    || call.finalized_at.is_none()
                    || call.provider_request_id.is_none()
                    || call.usage_record_id.is_none()
                    || call.output_digest.is_none()
            }
            BudgetCallState::Released | BudgetCallState::Cancelled => call.dispatch_id.is_some(),
        };
        if invalid_state {
            unresolved.push(root_unresolved(
                Some(call),
                RootBudgetUnresolvedReason::InvalidCallState,
            ));
        }
        let pending = match call.state {
            BudgetCallState::Reserved => Some(RootBudgetUnresolvedReason::Reserved),
            BudgetCallState::Dispatched => Some(RootBudgetUnresolvedReason::Dispatched),
            BudgetCallState::Uncertain => Some(RootBudgetUnresolvedReason::Uncertain),
            BudgetCallState::Finalized | BudgetCallState::Released | BudgetCallState::Cancelled => {
                None
            }
        };
        if let Some(reason) = pending {
            add_exact_micros(&mut total.3, call.reserved_micros)?;
            unresolved.push(root_unresolved(Some(call), reason));
        }
        if !call.execution_closed {
            unresolved.push(root_unresolved(
                Some(call),
                RootBudgetUnresolvedReason::ExecutionOpen,
            ));
        }
        let any_charge = call.actual_cost_micros.is_some()
            || call.actual_currency.is_some()
            || call.actual_pricing_version.is_some();
        let complete_charge = call.actual_cost_micros.is_some()
            && call.actual_currency.is_some()
            && call.actual_pricing_version.is_some();
        let charge_matches = complete_charge
            && call.actual_currency.as_deref() == Some(root.currency.as_str())
            && call.actual_pricing_version.as_deref() == Some(root.pricing_version.as_str());
        if any_charge && !complete_charge {
            unresolved.push(root_unresolved(
                Some(call),
                RootBudgetUnresolvedReason::PartialActualCharge,
            ));
        } else if complete_charge && !charge_matches {
            unresolved.push(root_unresolved(
                Some(call),
                RootBudgetUnresolvedReason::ChargeIdentityMismatch,
            ));
        }
        if charge_matches {
            let actual = call.actual_cost_micros.ok_or(Error::ClockRegressed)?;
            add_exact_micros(&mut total.1, actual)?;
            if call.state == BudgetCallState::Finalized {
                add_exact_micros(&mut total.2, actual)?;
            }
            if actual > call.reserved_micros || actual > root.per_call_cap_micros {
                unresolved.push(root_unresolved(
                    Some(call),
                    RootBudgetUnresolvedReason::CallCostOverrun,
                ));
            }
        }
        let undispatched_terminal = matches!(
            call.state,
            BudgetCallState::Released | BudgetCallState::Cancelled
        ) && call.dispatch_id.is_none();
        if undispatched_terminal && any_charge {
            unresolved.push(root_unresolved(
                Some(call),
                RootBudgetUnresolvedReason::UnexpectedTerminalCharge,
            ));
        }
        if !charge_matches && !undispatched_terminal {
            total.4 = total.4.checked_add(1).ok_or(Error::ClockRegressed)?;
            if !any_charge && call.state != BudgetCallState::Reserved {
                unresolved.push(root_unresolved(
                    Some(call),
                    RootBudgetUnresolvedReason::MissingActualCharge,
                ));
            }
        }
    }
    let mut known_actual = 0_i128;
    let mut finalized_actual = 0_i128;
    let mut held_reservation = 0_i128;
    let mut stage_totals = Vec::with_capacity(BudgetStage::ALL.len());
    for (stage, (call_count, known, finalized, held, unknown_actual_call_count)) in totals {
        known_actual = known_actual
            .checked_add(known)
            .ok_or(Error::ClockRegressed)?;
        finalized_actual = finalized_actual
            .checked_add(finalized)
            .ok_or(Error::ClockRegressed)?;
        held_reservation = held_reservation
            .checked_add(held)
            .ok_or(Error::ClockRegressed)?;
        stage_totals.push(RootBudgetStageMonetaryTotal {
            stage,
            call_count,
            known_actual_micros: known.to_string(),
            finalized_actual_micros: finalized.to_string(),
            held_reservation_micros: held.to_string(),
            unknown_actual_call_count,
        });
    }
    if finalized_actual != i128::from(root.spent_micros) {
        unresolved.push(root_unresolved(
            None,
            RootBudgetUnresolvedReason::SpentLedgerMismatch,
        ));
    }
    if held_reservation != i128::from(root.reserved_micros) {
        unresolved.push(root_unresolved(
            None,
            RootBudgetUnresolvedReason::ReservationLedgerMismatch,
        ));
    }
    if known_actual > i128::from(root.total_limit_micros)
        || finalized_actual
            .checked_add(held_reservation)
            .ok_or(Error::ClockRegressed)?
            > i128::from(root.total_limit_micros)
    {
        unresolved.push(root_unresolved(
            None,
            RootBudgetUnresolvedReason::AuthorizationLimitExceeded,
        ));
    }
    let snapshot_digest = fingerprint(&(
        "rsia.root_budget_monetary_snapshot.v1",
        &root,
        &dispatch_groups,
        &calls,
        &ledger_events,
    ))?;
    Ok(RootBudgetMonetarySnapshot {
        schema_version: "rsia.root_budget_monetary_snapshot.v1".into(),
        root,
        stage_totals,
        dispatch_groups,
        calls,
        ledger_events,
        known_actual_micros: known_actual.to_string(),
        finalized_actual_micros: finalized_actual.to_string(),
        held_reservation_micros: held_reservation.to_string(),
        monetary_complete: unresolved.is_empty(),
        unresolved,
        complete_episode_cost: false,
        ordinary_p_eligible: false,
        snapshot_digest,
    })
}

fn seal_summary(
    snapshot: &RootBudgetMonetarySnapshot,
    namespace: &str,
    actor: &str,
    now: i64,
) -> SealedRootBudgetEvidence {
    SealedRootBudgetEvidence {
        schema_version: "rsia.root_budget_monetary_seal.v1".into(),
        root: snapshot.root.clone(),
        stage_totals: snapshot.stage_totals.clone(),
        call_count: snapshot.calls.len(),
        dispatch_group_count: snapshot.dispatch_groups.len(),
        ledger_event_count: snapshot.ledger_events.len(),
        known_actual_micros: snapshot.known_actual_micros.clone(),
        finalized_actual_micros: snapshot.finalized_actual_micros.clone(),
        held_reservation_micros: snapshot.held_reservation_micros.clone(),
        unresolved: snapshot.unresolved.clone(),
        monetary_complete: snapshot.monetary_complete,
        complete_episode_cost: false,
        ordinary_p_eligible: false,
        snapshot_digest: snapshot.snapshot_digest.clone(),
        sealed_by_namespace: namespace.into(),
        sealed_by_actor: actor.into(),
        sealed_at: now,
    }
}

// Time qualification of a retained seal is not a claim that its ledger is
// still current. Only spent/reserved and subsequent ledger state may differ.
fn qualify_historical_root_seal_r42(
    evidence: &SealedRootBudgetEvidence,
    current: &RootBudgetRecord,
    preceding_events: &[RootBudgetLedgerEvent],
) -> Result<()> {
    let saved = &evidence.root;
    // Validate historical amounts BEFORE excluding them from static identity.
    require(saved.spent_micros >= 0 && saved.reserved_micros >= 0)?;
    require(
        evidence.schema_version == "rsia.root_budget_monetary_seal.v1"
            && saved.root_budget_id == current.root_budget_id
            && saved.billing_scope == current.billing_scope
            && saved.authorizing_namespace == current.authorizing_namespace
            && saved.allowed_namespaces == current.allowed_namespaces
            && saved.currency == current.currency
            && saved.pricing_version == current.pricing_version
            && saved.payment_subject == current.payment_subject
            && saved.authorization_receipt_digest == current.authorization_receipt_digest
            && saved.per_call_cap_micros == current.per_call_cap_micros
            && saved.total_limit_micros == current.total_limit_micros
            && saved.created_at == current.created_at
            && saved.stopped
            && current.stopped
            && saved.stop_reason == current.stop_reason
            && saved.stop_committed_at == current.stop_committed_at
            && saved
                .allowed_namespaces
                .contains(&evidence.sealed_by_namespace)
            && evidence.sealed_at >= saved.created_at
            && evidence.sealed_at >= saved.stop_committed_at.ok_or(Error::ClockRegressed)?,
    )?;
    identifier(&evidence.sealed_by_actor).map_err(|_| Error::ClockRegressed)?;
    identifier(&evidence.sealed_by_namespace).map_err(|_| Error::ClockRegressed)?;
    digest(&evidence.snapshot_digest)?;
    require(
        evidence.ledger_event_count == preceding_events.len()
            && preceding_events
                .iter()
                .all(|event| event.event_at <= evidence.sealed_at)
            && !evidence.complete_episode_cost
            && !evidence.ordinary_p_eligible
            && evidence.monetary_complete == evidence.unresolved.is_empty()
            && evidence.stage_totals.len() == BudgetStage::ALL.len(),
    )?;
    fn amount(value: &str) -> Result<i128> {
        let parsed = value.parse::<i128>().map_err(|_| Error::ClockRegressed)?;
        require(parsed >= 0 && parsed.to_string() == value)?;
        Ok(parsed)
    }
    let mut call_count = 0_usize;
    let mut totals = [0_i128; 3];
    for (stage, expected_stage) in evidence.stage_totals.iter().zip(BudgetStage::ALL) {
        require(
            stage.stage == expected_stage && stage.unknown_actual_call_count <= stage.call_count,
        )?;
        call_count = call_count
            .checked_add(stage.call_count)
            .ok_or(Error::ClockRegressed)?;
        let values = [
            amount(&stage.known_actual_micros)?,
            amount(&stage.finalized_actual_micros)?,
            amount(&stage.held_reservation_micros)?,
        ];
        require(values[1] <= values[0])?;
        for (total, value) in totals.iter_mut().zip(values) {
            *total = total.checked_add(value).ok_or(Error::ClockRegressed)?;
        }
    }
    require(
        call_count == evidence.call_count
            && totals
                == [
                    amount(&evidence.known_actual_micros)?,
                    amount(&evidence.finalized_actual_micros)?,
                    amount(&evidence.held_reservation_micros)?,
                ],
    )?;
    for issue in &evidence.unresolved {
        if let Some(amount) = issue.actual_cost_micros {
            require(amount >= 0)?;
        }
        for value in [
            issue.namespace.as_deref(),
            issue.call_id.as_deref(),
            issue.dispatch_group_id.as_deref(),
            issue.actual_currency.as_deref(),
            issue.actual_pricing_version.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            identifier(value).map_err(|_| Error::ClockRegressed)?;
        }
    }
    Ok(())
}

// No constructor or wire codec grants authority. This private state is exposed
// only through the selected formal snapshot after its real native close ACK.
enum ScopedLatestRootSealR42 {
    Absent,
    Current(Box<SealedRootBudgetEvidence>),
    Stale,
}
impl ScopedLatestRootSealR42 {
    fn evidence(&self) -> Result<Option<&SealedRootBudgetEvidence>> {
        match self {
            Self::Absent => Ok(None),
            Self::Current(evidence) => Ok(Some(evidence.as_ref())),
            Self::Stale => Err(Error::Conflict(
                "root monetary seal is stale or invalid".into(),
            )),
        }
    }
}

fn strict_latest_root_seal_r42(
    raw: &RawDatabase,
    snapshot: &OccurrenceSnapshot,
    ctx: &evo_core::Context,
    billing_scope: &str,
) -> Result<ScopedLatestRootSealR42> {
    ctx.require(&[
        evo_core::Role::Admin,
        evo_core::Role::Host,
        evo_core::Role::Evaluator,
        evo_core::Role::Worker,
    ])?;
    identifier(billing_scope).map_err(|_| Error::ClockRegressed)?;
    let Some(root) = snapshot.roots.get(billing_scope) else {
        return Ok(ScopedLatestRootSealR42::Absent);
    };
    if !root
        .allowed_namespaces
        .iter()
        .any(|namespace| namespace == ctx.namespace())
    {
        return Err(Error::NotFound);
    }
    let mut latest = None;
    let mut ledger_events = Vec::new();
    for row in rows(
        raw,
        "root_budget_events",
        &[
            "seq",
            "billing_scope",
            "call_id",
            "event_kind",
            "event_at",
            "details",
        ],
    )? {
        if sql_string(&row, "billing_scope")? != billing_scope {
            continue;
        }
        let sequence = sql_integer(&row, "seq")?;
        require(sequence > 0)?;
        let call_id = sql_optional_string(&row, "call_id")?;
        let event_kind = sql_string(&row, "event_kind")?;
        let event_at = sql_integer(&row, "event_at")?;
        let details = sql_string(&row, "details")?;
        let value = parse_owned_json(&details)?;
        if event_kind == "root_monetary_sealed_v1" {
            require(call_id.is_none())?;
            let evidence: SealedRootBudgetEvidence = decode(&value)?;
            require(evidence.sealed_at == event_at)?;
            if latest
                .as_ref()
                .is_none_or(|(previous, _)| sequence > *previous)
            {
                latest = Some((sequence, evidence));
            }
        } else {
            ledger_events.push(RootBudgetLedgerEvent {
                sequence,
                call_id,
                event_kind,
                event_at,
                details,
            });
        }
    }
    let Some((_sequence, evidence)) = latest else {
        return Ok(ScopedLatestRootSealR42::Absent);
    };
    // A stale seal remains readable historical evidence but cannot prove the
    // current ledger. Never return an older valid seal instead of the latest.
    if evidence.schema_version != "rsia.root_budget_monetary_seal.v1"
        || evidence.root != *root
        || !root.stopped
        || !root
            .allowed_namespaces
            .contains(&evidence.sealed_by_namespace)
        || evidence.sealed_at < root.stop_committed_at.ok_or(Error::ClockRegressed)?
    {
        return Ok(ScopedLatestRootSealR42::Stale);
    }
    identifier(&evidence.sealed_by_actor).map_err(|_| Error::ClockRegressed)?;
    ledger_events.sort_by_key(|event| event.sequence);
    let current = monetary_snapshot(
        root.clone(),
        snapshot
            .groups
            .iter()
            .filter(|((scope, _), _)| scope == billing_scope)
            .map(|(_, group)| group.clone())
            .collect(),
        snapshot
            .calls
            .iter()
            .filter(|((scope, _), _)| scope == billing_scope)
            .map(|(_, call)| call.clone())
            .collect(),
        ledger_events,
    )?;
    let summary = seal_summary(
        &current,
        &evidence.sealed_by_namespace,
        &evidence.sealed_by_actor,
        evidence.sealed_at,
    );
    if summary != evidence {
        return Ok(ScopedLatestRootSealR42::Stale);
    }
    Ok(ScopedLatestRootSealR42::Current(Box::new(evidence)))
}

fn root_unresolved(
    call: Option<&BudgetCallRecord>,
    reason: RootBudgetUnresolvedReason,
) -> RootBudgetUnresolvedRecord {
    RootBudgetUnresolvedRecord {
        namespace: call.map(|call| call.namespace.clone()),
        call_id: call.map(|call| call.call_id.clone()),
        dispatch_group_id: call.map(|call| call.dispatch_group_id.clone()),
        reason,
        actual_cost_micros: call.and_then(|call| call.actual_cost_micros),
        actual_currency: call.and_then(|call| call.actual_currency.clone()),
        actual_pricing_version: call.and_then(|call| call.actual_pricing_version.clone()),
    }
}

fn add_exact_micros(total: &mut i128, amount: i64) -> Result<()> {
    if amount < 0 {
        return Err(Error::ClockRegressed);
    }
    *total = total
        .checked_add(i128::from(amount))
        .ok_or(Error::ClockRegressed)?;
    Ok(())
}

// Strict wire mirrors of actual M29 engine declarations; no engine dependency.
use evo_core::Role;
use evo_core::contract::CapabilityLevel;
use evo_core::contract::HostSurfaceManifest;
use evo_core::contract::RunProjection;
use evo_core::contract::SystemSnapshot;
use evo_core::curriculum::ProbeTerminal;
use evo_core::evaluation::ExperimentPlan;
use evo_core::evaluation::FormalExperimentPlanV41;
use evo_core::evaluation::OptimizationBudgetPlan;
use evo_core::evaluation::OptimizerComparisonContract;
use evo_core::evaluation::Verdict;
use evo_core::evidence::AggregateSummary;
use evo_core::evidence::EvidenceLocator;
use evo_core::evidence::EvidenceSet;
use evo_core::evidence::ExecutionAttestation;
use evo_core::evidence::Purpose;
use evo_core::evidence::TaskOrigin;
use evo_core::holdout::AnchorCoverageMatrix;
use evo_core::holdout::AnchorResult;
use evo_core::holdout::FrozenCandidatePair;
use evo_core::holdout::HoldoutManifest;
use evo_core::improver::ImproverContentV2;
use evo_core::replay::ReplaySimulationProfile;
use evo_core::replay::WorldPartition;
use evo_core::sequential::EarlyStopCertificate;
use evo_core::sequential::EarlyStopPlan;
use evo_core::sequential::ResearchFamilyAlphaPlan;
use evo_core::strategy::BatchActionV1;
use evo_core::strategy::ElasticPolicyV1;
use evo_core::strategy::ExplorationCapsV1;
use evo_core::strategy::OpportunityWait;
use evo_core::strategy::PracticePlan;
use evo_core::strategy::SimulationContext;
use serde::Serialize;

// M29 crates/evo-engine/src/streaming_evaluator.rs: RegisteredEvaluationControl.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorRegisteredEvaluationControl {
    schema_version: String,
    id: String,
    v1_plan_snapshot: ExperimentPlan,
    formal_plan: FormalExperimentPlanV41,
    alpha_plan: ResearchFamilyAlphaPlan,
    optimizer_comparison: OptimizerComparisonContract,
    optimization_budget: OptimizationBudgetPlan,
    early_stop_plan: Option<EarlyStopPlan>,
    anchor_matrix: AnchorCoverageMatrix,
    attempt_id: String,
    fixed_sample_claim_id: String,
    billing_scope: String,
    executor_actor: String,
    evaluator_actor: String,
    proposer_actor: String,
    approver_actor: String,
    oracle_digest: String,
    grader_digest: String,
    fixed_grader: MirrorFixedGraderSpec,
    evidence_scope: MirrorEvaluationEvidenceScope,
    created_at_unix_seconds: i64,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: ProtectedHoldoutRecord.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorProtectedHoldoutRecord {
    schema_version: String,
    id: String,
    registration_id: String,
    pair: FrozenCandidatePair,
    manifest: HoldoutManifest,
    candidate_bundle_digest: String,
    baseline_bundle_digest: String,
    rotating_inputs: Vec<MirrorProtectedInputRef>,
    anchor_inputs: Vec<MirrorProtectedInputRef>,
    oracle_entries: Vec<MirrorFrozenOracleEntry>,
    oracle_payload_digest: String,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: EvaluationTicketV2.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorEvaluationTicketV2 {
    schema_version: String,
    id: String,
    registration_id: String,
    registration_digest: String,
    holdout_id: String,
    manifest_digest: String,
    v1_plan_snapshot_digest: String,
    bound_v1_plan_digest: String,
    formal_plan_digest: String,
    profile: evo_core::evaluation::ProfileKind,
    n_planned: u32,
    research_family_id: String,
    alpha_plan_digest: String,
    candidate_digest: String,
    baseline_digest: String,
    candidate_bundle_digest: String,
    baseline_bundle_digest: String,
    environment_digest: String,
    proposer_actor: String,
    evaluator_actor: String,
    executor_actor: String,
    approver_actor: String,
    issue_receipt_digest: String,
    evidence_scope: MirrorEvaluationEvidenceScope,
    attempt_id: String,
    targets: Vec<MirrorEvaluationTarget>,
    lifecycle: MirrorTicketLifecycle,
    next_sequence: u64,
    pending_certificate: Option<EarlyStopCertificate>,
    stop_member_terminal_digest: Option<String>,
    late_execution_receipt_ids: Vec<String>,
    late_grader_receipt_ids: Vec<String>,
    unattributed_budget_call_ids: Vec<String>,
    invalid_reasons: Vec<String>,
    issued_at_unix_seconds: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    candidate_dependency_binding: Option<MirrorFormalCandidateDependencyBinding>,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: TicketIssueReceipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorTicketIssueReceipt {
    schema_version: String,
    ticket_id: String,
    registration_digest: String,
    manifest_digest: String,
    evaluator_actor: String,
    issued_at_unix_seconds: i64,
}

// Explicit new protocol2; the original v1 mirror above remains byte-for-byte.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorTicketIssueReceiptV2 {
    schema_version: String,
    ticket_id: String,
    registration_digest: String,
    manifest_digest: String,
    evaluator_actor: String,
    issued_at_unix_seconds: i64,
    issued_at_unix_ms: i64,
}

// This is a private read projection of an actually decoded static protocol.
// Its digest is derived from the corresponding original typed wire, not Value order.
struct E05IssueReceiptIdentity {
    ticket_id: String,
    registration_digest: String,
    manifest_digest: String,
    evaluator_actor: String,
    issued_at_unix_seconds: i64,
    issued_at_unix_ms: Option<i64>,
    actual_digest: String,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: ExecutionReceiptV2.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorExecutionReceiptV2 {
    schema_version: String,
    receipt_id: String,
    ticket_id: String,
    target_id: String,
    side: MirrorExecutionSide,
    request_digest: String,
    bundle_digest: String,
    environment_digest: String,
    output_digest: String,
    output_artifact_id: String,
    budget_call_id: String,
    executor_actor: String,
    latency_micros: u64,
    issued_at_unix_seconds: i64,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: IndependentGraderReceiptV2.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorIndependentGraderReceiptV2 {
    schema_version: String,
    receipt_id: String,
    ticket_id: String,
    target_id: String,
    candidate_execution_receipt_id: String,
    baseline_execution_receipt_id: String,
    candidate_output_digest: String,
    baseline_output_digest: String,
    oracle_digest: String,
    grader_digest: String,
    scored_output_pair_digest: String,
    candidate_score_micros: u32,
    baseline_score_micros: u32,
    anchor_result: Option<AnchorResult>,
    evaluator_actor: String,
    issued_at_unix_seconds: i64,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: FormalEvaluationV2.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "variant", deny_unknown_fields)]
enum MirrorFormalEvaluationV2 {
    CompleteBatch {
        evidence_scope: MirrorEvaluationEvidenceScope,
        cost_evidence_scope: MirrorCostEvidenceScope,
        ticket_id: String,
        ticket_digest: String,
        manifest_digest: String,
        candidate_bundle_digest: String,
        baseline_bundle_digest: String,
        environment_digest: String,
        verdict: Verdict,
        report: MirrorBernsteinReportWire,
        anchor_evidence_digest: String,
        resource_evidence_digest: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stored_dependency_binding: Option<MirrorFormalStoredDependencyBinding>,
    },
    EarlyStopped {
        evidence_scope: MirrorEvaluationEvidenceScope,
        ticket_id: String,
        ticket_digest: String,
        certificate: EarlyStopCertificate,
    },
    Invalid {
        evidence_scope: MirrorEvaluationEvidenceScope,
        ticket_id: String,
        ticket_digest: String,
        reasons: Vec<String>,
    },
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: ResourceEvidenceRecord.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorResourceEvidenceRecord {
    schema_version: String,
    ticket_id: String,
    billing_scope: String,
    cost_scope: MirrorCostEvidenceScope,
    calls: Vec<BudgetCallRecord>,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: ExecutionOutputArtifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorExecutionOutputArtifact {
    schema_version: String,
    id: String,
    ticket_id: String,
    target_id: String,
    side: MirrorExecutionSide,
    output_utf8: String,
    output_digest: String,
}

// M29 crates/evo-engine/src/development.rs: DevelopmentControlV1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorDevelopmentControlV1 {
    schema_version: String,
    id: String,
    namespace: String,
    billing_scope: String,
    root_budget_id: String,
    executor_actor: String,
    grader_actor: String,
    proposer_actor: String,
    manifest_id: String,
    manifest_digest: String,
    tasks: Vec<MirrorDevelopmentTaskSpecV1>,
    environment_digest: String,
    fixed_grader: MirrorFixedGraderSpec,
    grader_digest: String,
    oracle_digest: String,
    target_digest: String,
    runner_digest: String,
    rules_digest: String,
    tools_digest: String,
    source_ids: Vec<String>,
    evidence_scope: MirrorDevelopmentEvidenceScope,
    created_at_unix_seconds: i64,
}

// M29 crates/evo-engine/src/development.rs: DevelopmentExecutionReceiptV1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorDevelopmentExecutionReceiptV1 {
    schema_version: String,
    receipt_id: String,
    control_id: String,
    namespace: String,
    episode_id: String,
    step: u32,
    attempt: u32,
    request_id: String,
    task_id: String,
    side: MirrorDevelopmentSide,
    input_digest: String,
    bundle_digest: String,
    environment_digest: String,
    request_digest: String,
    budget_call_id: String,
    dispatch_id: String,
    usage_record_id: String,
    output_digest: String,
    output_artifact_id: String,
    cost_state: MirrorDevelopmentCostState,
    execution_provenance: MirrorDevelopmentExecutionProvenance,
    revoke_watermark: u64,
    source_ids: Vec<String>,
    executor_actor: String,
    executor_role: Role,
    issued_at_unix_seconds: i64,
}

// M29 crates/evo-engine/src/development.rs: DevelopmentGraderReceiptV1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorDevelopmentGraderReceiptV1 {
    schema_version: String,
    receipt_id: String,
    control_id: String,
    namespace: String,
    episode_id: String,
    step: u32,
    attempt: u32,
    request_id: String,
    task_id: String,
    parent_execution_receipt_id: String,
    candidate_execution_receipt_id: String,
    parent_output_digest: String,
    candidate_output_digest: String,
    oracle_digest: String,
    grader_digest: String,
    grader_version: String,
    parent_score_micros: u32,
    candidate_score_micros: u32,
    parent_passed: bool,
    candidate_passed: bool,
    scored_output_pair_digest: String,
    scoring_budget_call_id: Option<String>,
    revoke_watermark: u64,
    source_ids: Vec<String>,
    grader_actor: String,
    issued_at_unix_seconds: i64,
}

// M29 crates/evo-engine/src/development.rs: DevelopmentRunReceiptV1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorDevelopmentRunReceiptV1 {
    schema_version: String,
    id: String,
    control_id: String,
    namespace: String,
    request_id: String,
    episode_id: String,
    step: u32,
    attempt: u32,
    execution_receipt_ids: Vec<String>,
    grader_receipt_ids: Vec<String>,
    budget_call_ids: Vec<String>,
    usage_record_ids: Vec<String>,
    executor_actor: String,
    issued_at_unix_seconds: i64,
}

// M29 crates/evo-engine/src/development.rs: DevelopmentExecutionOutputV1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorDevelopmentExecutionOutputV1 {
    schema_version: String,
    id: String,
    control_id: String,
    request_id: String,
    task_id: String,
    side: MirrorDevelopmentSide,
    output_utf8: String,
    output_digest: String,
}

// M29 crates/evo-engine/src/development.rs: RegisteredExecutionRequestV1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorRegisteredExecutionRequestV1 {
    schema_version: String,
    control_id: String,
    request_id: String,
    episode_id: String,
    task_id: String,
    side: MirrorDevelopmentSide,
    input: MirrorRegisteredTargetInputV1,
    input_digest: String,
    bundle_digest: String,
    environment_digest: String,
    request_digest: String,
    target_id: String,
    target_digest: String,
    runner_digest: String,
    /// Trusted-source closure of the control; source revocation scans it.
    source_ids: Vec<String>,
}

// M29 crates/evo-engine/src/dispatch.rs: ManagementJob.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorManagementJob {
    id: String,
    schema_version: String,
    operation: String,
    request_key: String,
    payload_digest: String,
    owner_actor: String,
    owner_role: Role,
    state: MirrorManagementJobState,
    step: String,
    private_input_ref: String,
    result: Option<MirrorManagementResult>,
    error_code: Option<String>,
    cancel_requested: bool,
    lease_token: Option<String>,
    lease_until: i64,
    generation: u64,
    #[serde(default)]
    diagnostics: Vec<MirrorManagementDiagnostic>,
    created_at: i64,
}

// M29 crates/evo-engine/src/dispatch.rs: PrivateManagementInput.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorPrivateManagementInput {
    id: String,
    schema_version: String,
    operation: String,
    payload_digest: String,
    payload: Value,
}

// M29 crates/evo-engine/src/dispatch.rs: ExperimentRegisterRequest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorExperimentRegisterRequest {
    schema_version: String,
    request_key: String,
    control: MirrorRegisteredEvaluationControl,
    holdout: MirrorProtectedHoldoutRecord,
}

// M29 crates/evo-engine/src/dispatch.rs: EvaluationStartRequest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorEvaluationStartRequest {
    schema_version: String,
    request_key: String,
    ticket: MirrorIssueTicketRequestWire,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorEvaluationStartRequestV2 {
    schema_version: String,
    request_key: String,
    ticket: MirrorIssueTicketIntentV2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorIssueTicketIntentV2 {
    ticket_id: String,
    registration_id: String,
    holdout_id: String,
}

// M29 crates/evo-engine/src/dispatch.rs: EvaluationStatusRequest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorEvaluationStatusRequest {
    schema_version: String,
    request_key: String,
    ticket_id: String,
}

// M29 crates/evo-engine/src/dispatch.rs: ReplayRunRequest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorReplayRunRequest {
    schema_version: String,
    request_key: String,
    pool_digest: String,
    partition: WorldPartition,
    policy: ElasticPolicyV1,
    profile: ReplaySimulationProfile,
    caps: ExplorationCapsV1,
}

// M29 crates/evo-engine/src/dispatch.rs: CurriculumStepRequest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorCurriculumStepRequest {
    schema_version: String,
    request_key: String,
    profile_id: String,
    state_id: String,
    root_budget_limit_micros: u64,
}

// M29 crates/evo-engine/src/dispatch.rs: ExplorationStartRequest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorExplorationStartRequest {
    schema_version: String,
    request_key: String,
    world: MirrorExplorationWorldV1,
}

// M29 crates/evo-engine/src/dispatch.rs: BlockedManagementRequest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorBlockedManagementRequest {
    schema_version: String,
    request_key: String,
}

// M29 crates/evo-engine/src/practice.rs: PracticeRegistrationV1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorPracticeRegistrationV1 {
    schema_version: String,
    id: String,
    /// `fingerprint(PracticePlan)` of the authorized plan.
    plan_digest: String,
    task_ids: Vec<String>,
    k: u8,
    parent_cluster_id: String,
    registered_by: String,
    created_at: i64,
}

// M29 crates/evo-engine/src/practice.rs: PracticeRegistrationBindingV1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorPracticeRegistrationBindingV1 {
    schema_version: String,
    /// Storage id, `practice_registration_binding_storage_id(registration_id)`.
    id: String,
    registration_id: String,
    set_id: String,
}

// M29 crates/evo-engine/src/practice.rs: PracticeAttemptSetV1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorPracticeAttemptSetV1 {
    schema_version: String,
    /// Storage id, `practice_attempt_set_storage_id(set_id)`.
    id: String,
    set_id: String,
    namespace: String,
    control_id: String,
    episode_id: String,
    step: u32,
    bundle_digest: String,
    environment_digest: String,
    revoke_watermark: u64,
    /// `PracticeRunRequest::digest`: the same request is the same set.
    request_digest: String,
    plan: PracticePlan,
    /// The registration a K=3 plan cited and that was verified; `None` for K=1.
    registration_id: Option<String>,
    attempts: Vec<MirrorPracticeAttemptV1>,
    outcome: MirrorPracticeOutcomeV1,
    /// Distinct task parent families. K is never part of it (V093.c).
    independent_clusters: u32,
    /// Always false: K attempts stay in one original cluster (V093.c).
    counts_as_independent_samples: bool,
    created_at: i64,
}

// M29 crates/evo-engine/src/meta.rs: MetaTrialV1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorMetaTrialV1 {
    schema_version: String,
    trial_id: String,
    /// Digest of the S0 template's registration facts that no stream may
    /// change (see `s0_registration_digest`), including the declared limits.
    s0_template_digest: String,
    /// The old improver. This increment fixes it to the built-in I0.
    old_content: ImproverContentV2,
    old_content_digest: String,
    new_content: ImproverContentV2,
    new_content_digest: String,
    old_world_id: String,
    new_world_id: String,
    old_policy_digest: String,
    new_policy_digest: String,
    /// The root budget both streams are billed against. No world carries it: it
    /// is the trial's own declaration, and what binds it afterwards is the
    /// ledger. A stream whose dispatch group has calls under any other scope
    /// makes the trial unverifiable (see [`verified_meta_trial`]).
    billing_scope: String,
    declared_stream_limits: MirrorStreamLimitsV1,
    /// The trusted runs of the S0 source closure, in the template's order.
    source_closure: Vec<String>,
    revoke_watermark: u64,
    created_at: i64,
}

// M29 crates/evo-engine/src/packages.rs: E16Envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorE16Envelope<T> {
    schema_version: String,
    id: String,
    namespace: String,
    owner_actor: String,
    request_key: String,
    input_digest: String,
    created_at: i64,
    updated_at: i64,
    source_refs: Vec<MirrorE16SourceRef>,
    revoke_watermark: u64,
    payload: T,
}

// M29 crates/evo-engine/src/packages.rs: StagedAssetPayload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorStagedAssetPayload {
    publisher: String,
    asset_id: String,
    package_kind: String,
    manifest_digest: String,
    content_blob_digest: String,
    content_bytes: u64,
    baseline_digest: String,
    local_digest: String,
    upstream_digest: Option<String>,
    environment_digest: String,
    package_schema_version: String,
    compiler_version: String,
    state: MirrorStagedAssetState,
    quarantine_reason: Option<String>,
    candidate_material: Option<MirrorCandidateMaterialBinding>,
    candidate_ref: Option<String>,
}

// M29 crates/evo-engine/src/packages.rs: ExportAttemptPayload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorExportAttemptPayload {
    publisher: String,
    asset_id: String,
    staged_asset_id: String,
    manifest_digest: String,
    projection_blob_digest: String,
    projection_blob_bytes: u64,
    package_tree_digest: String,
    delivered_bytes: u64,
    delivery_kind: String,
    local_export_ref: String,
    state: MirrorExportAttemptState,
    destination_scope: String,
    delivery_audit_id: Option<String>,
    completion_receipt_digest: Option<String>,
    budget_ref: Option<MirrorE04BudgetRef>,
    error: Option<String>,
}

// M29 crates/evo-engine/src/packages.rs: DeliveryAuditPayload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorDeliveryAuditPayload {
    export_attempt_id: String,
    package_id: String,
    projection_blob_digest: String,
    delivery_kind: String,
    local_export_ref: String,
    package_tree_digest: String,
    delivered_bytes: u64,
    completion_receipt_digest: String,
    delivered_to: String,
    delivered_at: i64,
    watermark_at_delivery: u64,
    revoked: bool,
    revocation_notice_sent: bool,
    remote_erasure_disclaimer: String,
}

// M29 crates/evo-engine/src/seeds.rs: SeedInstallPayloadV1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorSeedInstallPayloadV1 {
    publisher: String,
    asset_id: String,
    kind: String,
    baseline_blob_digest: String,
    baseline_digest: String,
    local_blob_digest: String,
    local_digest: String,
    upstream_blob_digest: Option<String>,
    upstream_digest: Option<String>,
    environment_digest: String,
    status: MirrorSeedStatus,
    quarantine_reason: Option<String>,
}

// M29 crates/evo-engine/src/import.rs: ImportEnvelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorImportEnvelope<T> {
    schema_version: String,
    id: String,
    namespace: String,
    owner_actor: String,
    request_key: String,
    input_digest: String,
    created_at: i64,
    updated_at: i64,
    source_refs: Vec<MirrorImportTypedRef>,
    revoke_watermark: u64,
    payload: T,
}

// M29 crates/evo-engine/src/import.rs: SourceSelectionPayload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorSourceSelectionPayload {
    purpose: evo_core::evidence::Purpose,
    allow_model_excerpts: bool,
    outbound_authorized: bool,
    retention_scope: MirrorImportRetentionScope,
    roots: Vec<String>,
    import_source_ids: Vec<String>,
}

// M29 crates/evo-engine/src/import.rs: ImportSourcePayload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorImportSourcePayload {
    selection_id: String,
    logical_source_id: String,
    authorized_path: String,
    reader: MirrorSourceFormat,
    reader_version: String,
    raw_digest: String,
    observed_digest: Option<String>,
    raw_blob_digest: String,
    blob_published: bool,
    byte_len: u64,
    status: MirrorImportSourceReadStatus,
    task_origin: TaskOrigin,
    execution_attestation: ExecutionAttestation,
    purpose: evo_core::evidence::Purpose,
}

// M29 crates/evo-engine/src/import.rs: ImportResultPayload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorImportResultPayload {
    selection_id: String,
    state: MirrorImportResultState,
    evidence_set: Option<EvidenceSet>,
    aggregate_summary: AggregateSummary,
    locators: Vec<EvidenceLocator>,
    event_records: Vec<MirrorPersistedImportEvent>,
    generation_status: String,
}

// M29 crates/evo-engine/src/import.rs: ImportRegistrationRequest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorImportRegistrationRequest {
    schema_version: String,
    request_key: String,
    roots: Vec<String>,
    purpose: evo_core::evidence::Purpose,
    allow_model_excerpts: bool,
    outbound_authorized: bool,
    retention_scope: MirrorImportRetentionScope,
    sources: Vec<MirrorImportSourceSpec>,
}

// M29 crates/evo-engine/src/release_store.rs: RunApplicationSnapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorRunApplicationSnapshot {
    id: String,
    schema_version: String,
    run_id: String,
    profile_id: String,
    pointer_epoch: u64,
    release_id: Option<String>,
    bundle_digest: Option<String>,
    environment_digest: String,
    system_snapshot: SystemSnapshot,
    host_surface_id: String,
    host_surface_digest: String,
    host_capabilities_digest: String,
    projection: RunProjection,
    request_material: MirrorAppliedRequestMaterial,
    request_digest: String,
    capability_level: CapabilityLevel,
    evolution_enabled: bool,
}

// M29 crates/evo-engine/src/release_store.rs: HostSurfaceRecord.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorHostSurfaceRecord {
    id: String,
    schema_version: String,
    manifest: HostSurfaceManifest,
    manifest_digest: String,
    extracted: Vec<String>,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: FixedGraderSpec.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorFixedGraderSpec {
    schema_version: String,
    version: String,
    method: MirrorFixedGraderMethod,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: EvaluationEvidenceScope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorEvaluationEvidenceScope {
    /// Local deterministic program fixture. It cannot authorize production.
    ProgramFixture,
    RegisteredExternalHost,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: FrozenOracleEntry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorFrozenOracleEntry {
    target_id: String,
    expected_answer_json: String,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: ProtectedInputRef.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorProtectedInputRef {
    target_id: String,
    cluster_or_anchor_id: String,
    input_artifact_id: String,
    input_digest: String,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: TicketLifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorTicketLifecycle {
    Reserved,
    Running,
    ReadyForFinal,
    StopRequested,
    Draining,
    EarlyStopped,
    CompleteBatch,
    Invalid,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: EvaluationTarget.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorEvaluationTarget {
    target_id: String,
    kind: MirrorEvaluationTargetKind,
    input_artifact_id: String,
    input_digest: String,
    state: MirrorTargetState,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: FormalCandidateDependencyBinding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorFormalCandidateDependencyBinding {
    schema_version: String,
    namespace: String,
    candidate_id: String,
    candidate_record_digest: String,
    bundle_digest: String,
    parent_digest: String,
    profile_id: String,
    environment_digest: String,
    proposer_actor: String,
    sources: Vec<MirrorTypedSourceRef>,
    revoke_watermark_seq: i64,
    revoke_watermark_digest: String,
    nodes: Vec<MirrorFormalDependencyNode>,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: ExecutionSide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorExecutionSide {
    Candidate,
    Baseline,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: BernsteinReportWire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorBernsteinReportWire {
    stats_version: String,
    verdict: Verdict,
    n: usize,
    mean: String,
    variance: String,
    radius: String,
    lcb: String,
    alpha_i: String,
    reasons: Vec<String>,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: CostEvidenceScope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorCostEvidenceScope {
    /// Only calls directly attached to this acceptance ticket were reconciled.
    TicketExecutionOnly,
    CompleteEpisodeRoot,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: FormalStoredDependencyBinding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorFormalStoredDependencyBinding {
    schema_version: String,
    namespace: String,
    candidate_binding_digest: String,
    ticket_id: String,
    ticket_physical_id: String,
    ticket_digest: String,
    resource_physical_id: String,
    /// Complete actual fact slice, excluding the report itself. The final ticket
    /// has already been written when this is derived; no future placeholders or
    /// whole shared ExposureLedger are part of the snapshot.
    nodes: Vec<MirrorFormalDependencyNode>,
}

// M29 crates/evo-engine/src/development.rs: DevelopmentTaskSpecV1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorDevelopmentTaskSpecV1 {
    task_id: String,
    parent_family: String,
    profile_id: String,
    target_id: String,
    input: MirrorRegisteredTargetInputV1,
    input_digest: String,
    expected_answer_json: String,
}

// M29 crates/evo-engine/src/development.rs: DevelopmentEvidenceScope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorDevelopmentEvidenceScope {
    /// Fixture-only control. Receipts issued under it can never verify a cycle.
    ProgramFixture,
    /// Registered pure-function adapters executed in-process by the fixed
    /// trusted runner. No sandbox, no provider dispatch, no model.
    RegisteredPureFunctionExecution,
}

// M29 crates/evo-engine/src/optimization.rs: DevelopmentExecutionProvenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorDevelopmentExecutionProvenance {
    Fixture,
    /// Reserved for a future sandboxed runner; nothing issues it today.
    IsolatedRunner,
    /// Registered pure-function adapters executed in-process by the fixed
    /// trusted runner (`crate::development`). Not isolated, not a sandbox.
    RegisteredPureFunction,
}

// M29 crates/evo-engine/src/development.rs: DevelopmentCostState.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum MirrorDevelopmentCostState {
    Known {
        micros: i64,
        currency: String,
        pricing_version: String,
    },
    Uncertain,
}

// M29 crates/evo-engine/src/development.rs: DevelopmentSide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorDevelopmentSide {
    Parent,
    Candidate,
}

// M29 crates/evo-engine/src/development.rs: RegisteredTargetInputV1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "snake_case", deny_unknown_fields)]
enum MirrorRegisteredTargetInputV1 {
    ClampI64 { value: i64, min: i64, max: i64 },
}

// M29 crates/evo-engine/src/dispatch.rs: ManagementResult.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "result", deny_unknown_fields)]
enum MirrorManagementResult {
    ExperimentRegistered {
        control_id: String,
        control_digest: String,
        holdout_id: String,
        holdout_digest: String,
    },
    EvaluationTicketBlocked {
        ticket_id: String,
    },
    EvaluationStatus {
        ticket_id: String,
        lifecycle: String,
        planned_targets: usize,
        has_formal_terminal: bool,
    },
    ReplayStored {
        report_id: String,
        pool_digest: String,
        semantic_reports_digest: String,
    },
    ProbeScheduled {
        probe_job_id: String,
        trigger_digest: String,
        terminal: Option<ProbeTerminal>,
    },
    ExplorationStarted {
        world_id: String,
        context_signature: String,
        prefix_digest: String,
        legal_actions_digest: String,
        /// Digest of the policy the first decision was taken with (E14). A
        /// result stored before E14 has none and reads as an empty string:
        /// the job still loads (startup recovery must not depend on it), and
        /// `status` then fails that job closed with a `Conflict`, because the
        /// re-run decision never has an empty digest.
        #[serde(default)]
        policy_digest: String,
        /// Digest of the caps the first decision was taken with (E14); empty
        /// for a pre-E14 result, like `policy_digest`.
        #[serde(default)]
        caps_digest: String,
        action: BatchActionV1,
    },
}

// M29 crates/evo-engine/src/dispatch.rs: ManagementJobState.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorManagementJobState {
    Queued,
    Running,
    Draining,
    Succeeded,
    Failed,
    Cancelled,
    Blocked,
}

// M29 crates/evo-engine/src/dispatch.rs: ManagementDiagnostic.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorManagementDiagnostic {
    code: String,
    observed_generation: u64,
    recorded_at: i64,
}

// M29 crates/evo-engine/src/dispatch.rs: IssueTicketRequestWire.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorIssueTicketRequestWire {
    ticket_id: String,
    registration_id: String,
    holdout_id: String,
    issued_at_unix_seconds: i64,
}

// M29 crates/evo-engine/src/exploration.rs: ExplorationWorldV1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorExplorationWorldV1 {
    schema_version: String,
    id: String,
    approved_parent_digest: String,
    context_signature: String,
    parent_skill_digest: String,
    parent_bundle_digest: String,
    environment_digest: String,
    model_digest: String,
    tools_digest: String,
    grader_digest: String,
    rules_digest: String,
    source_watermark: u64,
    caps: ExplorationCapsV1,
    policy: ElasticPolicyV1,
    simulation: SimulationContext,
    root_opportunities: Vec<MirrorRootOpportunity>,
    dependencies: Vec<MirrorExplorationDependency>,
    successor_cost_upper_micros: u64,
    initial_baseline_quality_micros: u32,
    remaining_root_micros: u64,
    remaining_recovery_dispatches: u8,
    state: MirrorWorldState,
    node_ids: Vec<String>,
    dispatch_ids: Vec<String>,
    history_ids: Vec<String>,
    current_branch_seq: Option<u32>,
    current_branch_focus_actions: u8,
    decision_round: u32,
    waits: Vec<OpportunityWait>,
}

// M29 crates/evo-engine/src/practice.rs: PracticeOutcomeV1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum MirrorPracticeOutcomeV1 {
    /// `task_id` has a better and a worse attempt under the frozen development
    /// goal (higher score, then pass). Never evidence, never a formal n.
    Contrast {
        better_attempt: u32,
        worse_attempt: u32,
        task_id: String,
    },
    /// No valid difference. K=1 is always this; K is never raised to find one.
    NoContrast,
    /// An attempt was uncertain, the budget ran out or an error stopped the
    /// set. Later attempts were not run; finished attempts are kept.
    Incomplete { reason: String },
}

// M29 crates/evo-engine/src/practice.rs: PracticeAttemptV1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorPracticeAttemptV1 {
    /// 1-based position in the set.
    attempt: u32,
    request_id: String,
    /// The attempt's E03 run receipt (`DevelopmentRunReport::execution_receipt_id`).
    execution_receipt_id: String,
    tasks: Vec<MirrorPracticeTaskResultV1>,
    /// Cost of every execution of the attempt (both runner sides).
    /// `Uncertain` as soon as any execution's cost is. For a cache hit it
    /// describes the original execution and is not new spend.
    cost_state: MirrorDevelopmentCostState,
    /// The attempt's run receipt already existed before the runner was called:
    /// the historical evidence is reused and this is neither a new practice nor
    /// a new paid call.
    cache_hit: bool,
}

// M29 crates/evo-engine/src/meta.rs: StreamLimitsV1.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorStreamLimitsV1 {
    remaining_root_micros: u64,
    root_opportunities: Vec<MirrorRootOpportunity>,
    successor_cost_upper_micros: u64,
    remaining_recovery_dispatches: u8,
}

// M29 crates/evo-engine/src/packages.rs: E16SourceRef.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorE16SourceRef {
    kind: String,
    id: String,
    digest: String,
}

// M29 crates/evo-engine/src/packages.rs: StagedAssetState.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorStagedAssetState {
    Prepared,
    Staged,
    Quarantined,
    Aborted,
    Failed,
}

// M29 crates/evo-engine/src/packages.rs: CandidateMaterialBinding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorCandidateMaterialBinding {
    skill_snapshot_entry_path: String,
    strategy_entry_path: String,
    strategy_authority: MirrorCandidateStrategyAuthority,
}

// M29 crates/evo-engine/src/packages.rs: ExportAttemptState.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorExportAttemptState {
    Prepared,
    Completed,
    Aborted,
    Revoked,
    Failed,
}

// M29 crates/evo-engine/src/packages.rs: E04BudgetRef.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorE04BudgetRef {
    root_budget_id: String,
    billing_scope: String,
}

// M29 crates/evo-engine/src/seeds.rs: SeedStatus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorSeedStatus {
    Prepared,
    Installed,
    Staged,
    Quarantined,
    Unknown,
}

// M29 crates/evo-engine/src/import.rs: ImportTypedRef.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorImportTypedRef {
    kind: String,
    id: String,
    content_digest: String,
}

// M29 crates/evo-engine/src/import.rs: ImportRetentionScope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorImportRetentionScope {
    LocalPrivate,
    LocalWithAuthorizedExcerpts,
}

// M29 crates/evo-engine/src/import.rs: SourceFormat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorSourceFormat {
    RsiaTraceV1,
    RsihPiFixture,
    ClaudeFixture,
}

// M29 crates/evo-engine/src/import.rs: ImportSourceReadStatus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorImportSourceReadStatus {
    Prepared,
    Ready,
    Missing,
    PermissionDenied,
    SourceChanged,
    ZeroRecords,
    InvalidFile,
    TooLarge,
}

// M29 crates/evo-engine/src/import.rs: PersistedImportEvent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorPersistedImportEvent {
    source_id: String,
    event_index: usize,
    byte_start: usize,
    byte_end: usize,
    raw_fragment_digest: String,
    normalized_content_digest: String,
    role: String,
    kind: String,
}

// M29 crates/evo-engine/src/import.rs: ImportResultState.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorImportResultState {
    Complete,
    Partial,
    Failed,
}

// M29 crates/evo-engine/src/import.rs: ImportSourceSpec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorImportSourceSpec {
    source_id: String,
    path: String,
    reader: MirrorSourceFormat,
    expected_digest: String,
}

// M29 crates/evo-engine/src/release_store.rs: AppliedRequestMaterial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorAppliedRequestMaterial {
    mandatory_context_digest: String,
    task_input_digest: String,
    instructions: Vec<String>,
    tools: Vec<String>,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: FixedGraderMethod.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorFixedGraderMethod {
    ExactJsonAnswerV1,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: EvaluationTargetKind.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
enum MirrorEvaluationTargetKind {
    Rotating { cluster_id: String },
    Anchor { anchor_id: String },
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: TargetState.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state", deny_unknown_fields)]
enum MirrorTargetState {
    Planned,
    Running {
        candidate: Option<MirrorExecutionSlotBinding>,
        baseline: Option<MirrorExecutionSlotBinding>,
    },
    Completed {
        candidate_score_micros: u32,
        baseline_score_micros: u32,
        anchor_result: Option<AnchorResult>,
        candidate_receipt_id: String,
        baseline_receipt_id: String,
        grader_receipt_id: String,
        completion_sequence: u64,
    },
    PlannedFailure {
        reason: String,
    },
    EnvironmentInvalid {
        reason: String,
    },
    NotDispatched {
        released_call_ids: Vec<String>,
    },
    CancelledAfterDispatch {
        candidate: Option<MirrorExecutionSlotBinding>,
        baseline: Option<MirrorExecutionSlotBinding>,
    },
    RunningUsageUnknown {
        candidate: Option<MirrorExecutionSlotBinding>,
        baseline: Option<MirrorExecutionSlotBinding>,
    },
}

// M29 crates/evo-engine/src/release_store.rs: TypedSourceRef.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorTypedSourceRef {
    kind: String,
    id: String,
    content_digest: String,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: FormalDependencyNode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorFormalDependencyNode {
    kind: String,
    /// Actual objects key, including e05-<fingerprint((record_kind,logical_id))>.
    id: String,
    schema_version: String,
    record_kind: Option<String>,
    logical_id: String,
    /// Full typed record fingerprint; for a run this pins StoredTraceAuthority,
    /// separately from MirrorTypedSourceRef.content_digest (the trace body digest).
    content_digest: String,
    outgoing_edge_digest: String,
    outgoing_edge_count: usize,
}

// M29 crates/evo-engine/src/exploration.rs: WorldState.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorWorldState {
    Collecting,
    Sealed,
    Stopped,
}

// M29 crates/evo-engine/src/exploration.rs: ExplorationDependency.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorExplorationDependency {
    kind: String,
    id: String,
}

// M29 crates/evo-engine/src/exploration.rs: RootOpportunity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorRootOpportunity {
    root_slot: u32,
    branch_seq: u32,
    action_seq: u32,
    estimated_cost_upper_micros: u64,
}

// M29 crates/evo-engine/src/practice.rs: PracticeTaskResultV1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorPracticeTaskResultV1 {
    task_id: String,
    parent_family: String,
    passed: bool,
    score_micros: u32,
    output_digest: String,
}

// M29 crates/evo-engine/src/packages.rs: CandidateStrategyAuthority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorCandidateStrategyAuthority {
    BuiltinDefaultV1,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: ExecutionSlotBinding.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorExecutionSlotBinding {
    side: MirrorExecutionSide,
    budget_call_id: String,
    request_digest: String,
    phase: MirrorExecutionSlotPhase,
    receipt_id: Option<String>,
}

// M29 crates/evo-engine/src/streaming_evaluator.rs: ExecutionSlotPhase.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "phase", deny_unknown_fields)]
enum MirrorExecutionSlotPhase {
    ReservedBound,
    Dispatched {
        dispatch_id: String,
        dispatched_at_unix_seconds: i64,
    },
}

use evo_core::holdout::{ExposureLedger, ExposureState, SequentialMode};
use evo_core::sequential::FormalClaimKind;
fn wire_digest(value: &str, _name: &str) -> Result<()> {
    digest(value)
}
fn valid_time(value: i64) -> Result<()> {
    require(value >= 0)
}
fn canonical_ids(ids: &[String], _name: &str) -> Result<()> {
    require(!ids.is_empty() && ids.windows(2).all(|pair| pair[0] < pair[1]))?;
    for id in ids {
        identifier(id).map_err(|_| Error::ClockRegressed)?;
    }
    Ok(())
}

impl MirrorFixedGraderSpec {
    pub const SCHEMA: &'static str = "rsia.fixed_grader.v1";

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != Self::SCHEMA {
            return Err(Error::Invalid("unsupported fixed grader schema".into()));
        }
        identifier(&self.version)
    }
}

impl MirrorProtectedInputRef {
    fn validate(&self) -> Result<()> {
        identifier(&self.target_id)?;
        identifier(&self.cluster_or_anchor_id)?;
        identifier(&self.input_artifact_id)?;
        wire_digest(&self.input_digest, "input_digest")
    }
}

impl MirrorRegisteredEvaluationControl {
    /// Frozen controls retain the original blanket-inconclusive postprocessing.
    pub const SCHEMA_V1: &'static str = "rsia.registered_evaluation_control.v1";
    /// Explicitly selects verdict preservation without changing the core algorithm.
    pub const SCHEMA: &'static str = "rsia.registered_evaluation_control.v2";

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != Self::SCHEMA_V1 && self.schema_version != Self::SCHEMA {
            return Err(Error::Invalid(
                "unsupported registered-control schema".into(),
            ));
        }
        for value in [
            &self.id,
            &self.attempt_id,
            &self.fixed_sample_claim_id,
            &self.billing_scope,
            &self.executor_actor,
            &self.evaluator_actor,
            &self.proposer_actor,
            &self.approver_actor,
        ] {
            identifier(value)?;
        }
        for (value, name) in [
            (&self.oracle_digest, "oracle_digest"),
            (&self.grader_digest, "grader_digest"),
        ] {
            wire_digest(value, name)?;
        }
        self.fixed_grader.validate()?;
        if self.grader_digest != fingerprint(&self.fixed_grader)? {
            return Err(Error::Invalid(
                "grader_digest does not identify the frozen grader".into(),
            ));
        }
        valid_time(self.created_at_unix_seconds)?;
        let actors: BTreeSet<_> = [
            self.executor_actor.as_str(),
            self.evaluator_actor.as_str(),
            self.proposer_actor.as_str(),
            self.approver_actor.as_str(),
        ]
        .into_iter()
        .collect();
        if actors.len() != 4 {
            return Err(Error::Invalid(
                "executor, evaluator, proposer, and approver must be distinct".into(),
            ));
        }
        self.formal_plan.validate_against(
            &self.v1_plan_snapshot,
            &self.alpha_plan,
            &self.optimizer_comparison,
            &self.optimization_budget,
        )?;
        self.alpha_plan.validate()?;
        self.anchor_matrix.validate()?;
        if self.formal_plan.v1_plan_snapshot_digest != self.v1_plan_snapshot.digest()?
            || self.formal_plan.alpha_plan_digest != self.alpha_plan.digest()?
            || self.formal_plan.anchor_coverage_digest != self.anchor_matrix.digest()?
            || self.formal_plan.research_family_id != self.alpha_plan.research_family_id
        {
            return Err(Error::Invalid(
                "registered control has inconsistent plan dependencies".into(),
            ));
        }
        self.alpha_plan.allocation(
            &self.fixed_sample_claim_id,
            &self.attempt_id,
            FormalClaimKind::FixedSampleGain,
        )?;
        if let Some(early) = &self.early_stop_plan {
            early.validate_against(&self.alpha_plan)?;
            if early.v1_plan_snapshot_digest != self.formal_plan.v1_plan_snapshot_digest
                || early.anchor_coverage_digest != self.formal_plan.anchor_coverage_digest
                || early.attempt_id != self.attempt_id
                || early.n_planned != self.formal_plan.n_planned
                || early.profile != self.formal_plan.profile
            {
                return Err(Error::Invalid(
                    "early-stop plan differs from the registered formal plan".into(),
                ));
            }
        }
        Ok(())
    }
}

impl MirrorProtectedHoldoutRecord {
    pub const SCHEMA: &'static str = "rsia.protected_holdout_record.v1";

    pub fn validate_against(&self, control: &MirrorRegisteredEvaluationControl) -> Result<()> {
        if self.schema_version != Self::SCHEMA {
            return Err(Error::Invalid(
                "unsupported protected-holdout schema".into(),
            ));
        }
        identifier(&self.id)?;
        identifier(&self.registration_id)?;
        if self.registration_id != control.id {
            return Err(Error::Conflict(
                "holdout belongs to another registration".into(),
            ));
        }
        self.pair.validate()?;
        self.manifest.validate()?;
        for (value, name) in [
            (&self.candidate_bundle_digest, "candidate_bundle_digest"),
            (&self.baseline_bundle_digest, "baseline_bundle_digest"),
            (&self.oracle_payload_digest, "oracle_payload_digest"),
        ] {
            wire_digest(value, name)?;
        }
        if self.pair.v1_plan_snapshot_digest != control.formal_plan.v1_plan_snapshot_digest
            || self.manifest.v1_plan_snapshot_digest != control.formal_plan.v1_plan_snapshot_digest
            || self.manifest.alpha_plan_digest != control.alpha_plan.digest()?
            || self.manifest.research_family_id != control.alpha_plan.research_family_id
            || self.manifest.candidate_pair_digest != self.pair.digest()?
            || self.manifest.anchor_coverage_digest != control.anchor_matrix.digest()?
            || self.manifest.oracle_version != control.oracle_digest
            || self.manifest.grader_digest != control.grader_digest
        {
            return Err(Error::Invalid(
                "holdout manifest does not bind the registered control".into(),
            ));
        }
        match (&control.early_stop_plan, &self.manifest.sequential_mode) {
            (None, SequentialMode::Disabled) => {}
            (Some(plan), SequentialMode::RejectOnly { .. }) => {
                self.manifest.validate_sequential_binding(plan)?;
            }
            _ => {
                return Err(Error::Invalid(
                    "holdout sequential mode differs from the registered plan".into(),
                ));
            }
        }
        if self.rotating_inputs.len() != self.manifest.ordered_task_cluster_ids.len() {
            return Err(Error::Invalid("rotating input list is incomplete".into()));
        }
        let mut target_ids = BTreeSet::new();
        for (index, input) in self.rotating_inputs.iter().enumerate() {
            input.validate()?;
            if input.cluster_or_anchor_id != self.manifest.ordered_task_cluster_ids[index]
                || !target_ids.insert(input.target_id.as_str())
            {
                return Err(Error::Invalid(
                    "rotating inputs must follow the frozen cluster order".into(),
                ));
            }
        }
        let required_anchors = control.anchor_matrix.anchor_ids();
        let mut actual_anchors = BTreeSet::new();
        for input in &self.anchor_inputs {
            input.validate()?;
            if !target_ids.insert(input.target_id.as_str())
                || !actual_anchors.insert(input.cluster_or_anchor_id.as_str())
            {
                return Err(Error::Invalid("duplicate protected input target".into()));
            }
        }
        if actual_anchors != required_anchors {
            return Err(Error::Invalid("anchor input coverage is incomplete".into()));
        }
        let expected_targets: BTreeSet<_> = self
            .rotating_inputs
            .iter()
            .chain(self.anchor_inputs.iter())
            .map(|input| input.target_id.as_str())
            .collect();
        let mut oracle_targets = BTreeSet::new();
        for entry in &self.oracle_entries {
            identifier(&entry.target_id)?;
            parse_owned_json(&entry.expected_answer_json)?;
            if !oracle_targets.insert(entry.target_id.as_str()) {
                return Err(Error::Invalid("duplicate frozen oracle entry".into()));
            }
        }
        if oracle_targets != expected_targets
            || self.oracle_payload_digest != fingerprint(&self.oracle_entries)?
        {
            return Err(Error::Invalid(
                "frozen oracle does not cover the exact ticket targets".into(),
            ));
        }
        Ok(())
    }
}

impl MirrorExecutionSlotBinding {
    fn validate(&self) -> Result<()> {
        identifier(&self.budget_call_id)?;
        wire_digest(&self.request_digest, "slot request_digest")?;
        if let Some(receipt_id) = &self.receipt_id {
            identifier(receipt_id)?;
            if !matches!(self.phase, MirrorExecutionSlotPhase::Dispatched { .. }) {
                return Err(Error::Invalid(
                    "receipt cannot bind an undispatched execution slot".into(),
                ));
            }
        }
        if let MirrorExecutionSlotPhase::Dispatched {
            dispatch_id,
            dispatched_at_unix_seconds,
        } = &self.phase
        {
            identifier(dispatch_id)?;
            valid_time(*dispatched_at_unix_seconds)?;
        }
        Ok(())
    }
}

impl MirrorEvaluationTicketV2 {
    const SCHEMA: &'static str = "rsia.evaluation_ticket.v2";
    fn validate(&self) -> Result<()> {
        if self.schema_version != Self::SCHEMA {
            return Err(Error::Invalid(
                "unsupported evaluation-ticket schema".into(),
            ));
        }
        for value in [
            &self.id,
            &self.registration_id,
            &self.holdout_id,
            &self.research_family_id,
            &self.proposer_actor,
            &self.evaluator_actor,
            &self.executor_actor,
            &self.approver_actor,
            &self.attempt_id,
        ] {
            identifier(value)?;
        }
        for (value, name) in [
            (&self.registration_digest, "registration_digest"),
            (&self.manifest_digest, "manifest_digest"),
            (&self.v1_plan_snapshot_digest, "v1_plan_snapshot_digest"),
            (&self.bound_v1_plan_digest, "bound_v1_plan_digest"),
            (&self.formal_plan_digest, "formal_plan_digest"),
            (&self.alpha_plan_digest, "alpha_plan_digest"),
            (&self.candidate_digest, "candidate_digest"),
            (&self.baseline_digest, "baseline_digest"),
            (&self.candidate_bundle_digest, "candidate_bundle_digest"),
            (&self.baseline_bundle_digest, "baseline_bundle_digest"),
            (&self.environment_digest, "environment_digest"),
            (&self.issue_receipt_digest, "issue_receipt_digest"),
        ] {
            wire_digest(value, name)?;
        }
        if let Some(binding) = &self.candidate_dependency_binding {
            binding.validate()?;
        }
        valid_time(self.issued_at_unix_seconds)?;
        if self.targets.is_empty() || self.next_sequence == 0 {
            return Err(Error::Invalid("ticket has no planned targets".into()));
        }
        let mut ids = BTreeSet::new();
        for target in &self.targets {
            identifier(&target.target_id)?;
            identifier(&target.input_artifact_id)?;
            wire_digest(&target.input_digest, "target input_digest")?;
            if !ids.insert(&target.target_id) {
                return Err(Error::Invalid("duplicate ticket target".into()));
            }
            match &target.state {
                MirrorTargetState::Running {
                    candidate,
                    baseline,
                }
                | MirrorTargetState::RunningUsageUnknown {
                    candidate,
                    baseline,
                }
                | MirrorTargetState::CancelledAfterDispatch {
                    candidate,
                    baseline,
                } => {
                    for slot in [candidate, baseline].into_iter().flatten() {
                        slot.validate()?;
                    }
                }
                MirrorTargetState::Completed {
                    candidate_score_micros,
                    baseline_score_micros,
                    candidate_receipt_id,
                    baseline_receipt_id,
                    grader_receipt_id,
                    completion_sequence,
                    ..
                } => {
                    if *candidate_score_micros > 1_000_000
                        || *baseline_score_micros > 1_000_000
                        || *completion_sequence == 0
                    {
                        return Err(Error::Invalid("invalid completed target state".into()));
                    }
                    for receipt_id in [candidate_receipt_id, baseline_receipt_id, grader_receipt_id]
                    {
                        identifier(receipt_id)?;
                    }
                }
                MirrorTargetState::NotDispatched { released_call_ids } => {
                    for call_id in released_call_ids {
                        identifier(call_id)?;
                    }
                }
                _ => {}
            }
        }
        for call_id in &self.unattributed_budget_call_ids {
            identifier(call_id)?;
        }
        if let Some(stop_digest) = &self.stop_member_terminal_digest {
            wire_digest(stop_digest, "stop_member_terminal_digest")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorProfileRuntimeStatus {
    DisabledSandboxUnavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorRegisteredPureFunctionProfileV1 {
    pub schema_version: String,
    pub profile_id: String,
    pub target_id: String,
    pub target_source_id: String,
    pub target_digest: String,
    pub oracle_source_id: String,
    pub oracle_digest: String,
    pub runner_source_id: String,
    pub runner_digest: String,
    pub input_schema_digest: String,
    pub output_schema_digest: String,
    pub min_value: i64,
    pub max_value: i64,
    pub max_concrete_inputs: u8,
    pub max_normalized_json_bytes: usize,
    pub declared_cpu_limit_ms: u32,
    pub declared_output_limit_bytes: usize,
    pub runtime_status: MirrorProfileRuntimeStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorClampInput {
    pub value: i64,
    pub min: i64,
    pub max: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorClampOutput {
    pub clamped: i64,
}

const PURE_FUNCTION_PROFILE_ID: &str = "pure_function_test_proposal.v1";
const CLAMP_TARGET_ID: &str = "reference_host.clamp_i64.v1";
const CLAMP_TARGET_SOURCE_ID: &str = "reference-host-clamp-i64-source-v1";
const CLAMP_ORACLE_SOURCE_ID: &str = "engine-clamp-i64-oracle-v1";
const DISABLED_RUNNER_SOURCE_ID: &str = "engine-disabled-curriculum-runner-v1";
const DECLARED_CPU_LIMIT_MS: u32 = 1000;
const DECLARED_OUTPUT_LIMIT_BYTES: usize = 16 * 1024;
const REGISTERED_RUNNER_CONTRACT_SCHEMA: &str = "rsia.registered_development_runner.v1";
const DEVELOPMENT_CONTROL_SCHEMA: &str = "rsia.development_control.v1";

fn target_source_body() -> serde_json::Value {
    serde_json::json!({
        "kind":"rust_source",
        "path":"examples/reference-host/src/curriculum_target.rs",
        "utf8":include_str!("../../../examples/reference-host/src/curriculum_target.rs")
    })
}

fn oracle_source_body() -> serde_json::Value {
    serde_json::json!({
        "kind":"fixed_branch_oracle",
        "version":"v1",
        "algorithm":"if value < min { min } else if value > max { max } else { value }"
    })
}

fn disabled_runner_source_body() -> serde_json::Value {
    serde_json::json!({
        "kind":"runner_contract",
        "version":"v1",
        "runtime":"disabled",
        "isolation":"sandbox_unavailable",
        "provider_dispatch":false
    })
}

impl MirrorRegisteredPureFunctionProfileV1 {
    pub fn clamp_i64() -> Self {
        Self {
            schema_version: "rsia.registered_pure_function_profile.v1".into(),
            profile_id: PURE_FUNCTION_PROFILE_ID.into(),
            target_id: CLAMP_TARGET_ID.into(),
            target_source_id: CLAMP_TARGET_SOURCE_ID.into(),
            target_digest: fingerprint(&target_source_body()).expect("fixed target source"),
            oracle_source_id: CLAMP_ORACLE_SOURCE_ID.into(),
            oracle_digest: fingerprint(&oracle_source_body()).expect("fixed oracle source"),
            runner_source_id: DISABLED_RUNNER_SOURCE_ID.into(),
            runner_digest: fingerprint(&disabled_runner_source_body())
                .expect("fixed runner contract"),
            input_schema_digest: hash(b"{value:i64,min:i64,max:i64}:domain+-1000000"),
            output_schema_digest: hash(b"{clamped:i64}"),
            min_value: -1_000_000,
            max_value: 1_000_000,
            max_concrete_inputs: 32,
            max_normalized_json_bytes: 16 * 1024,
            declared_cpu_limit_ms: DECLARED_CPU_LIMIT_MS,
            declared_output_limit_bytes: DECLARED_OUTPUT_LIMIT_BYTES,
            runtime_status: MirrorProfileRuntimeStatus::DisabledSandboxUnavailable,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != "rsia.registered_pure_function_profile.v1"
            || self.profile_id != PURE_FUNCTION_PROFILE_ID
            || self.target_id != CLAMP_TARGET_ID
            || self.target_source_id != CLAMP_TARGET_SOURCE_ID
            || self.oracle_source_id != CLAMP_ORACLE_SOURCE_ID
            || self.runner_source_id != DISABLED_RUNNER_SOURCE_ID
            || self.target_digest != fingerprint(&target_source_body())?
            || self.oracle_digest != fingerprint(&oracle_source_body())?
            || self.runner_digest != fingerprint(&disabled_runner_source_body())?
            || self.target_digest == self.oracle_digest
            || self.runner_digest == self.target_digest
            || self.runner_digest == self.oracle_digest
            || self.min_value != -1_000_000
            || self.max_value != 1_000_000
            || self.max_concrete_inputs != 32
            || self.max_normalized_json_bytes != 16 * 1024
            || self.declared_cpu_limit_ms != DECLARED_CPU_LIMIT_MS
            || self.declared_output_limit_bytes != DECLARED_OUTPUT_LIMIT_BYTES
            || self.runtime_status != MirrorProfileRuntimeStatus::DisabledSandboxUnavailable
        {
            return Err(Error::Invalid("unsupported pure-function profile".into()));
        }
        for source_id in [
            &self.target_source_id,
            &self.oracle_source_id,
            &self.runner_source_id,
        ] {
            identifier(source_id)?;
        }
        for value in [
            &self.target_digest,
            &self.oracle_digest,
            &self.runner_digest,
            &self.input_schema_digest,
            &self.output_schema_digest,
        ] {
            if value.len() != 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(Error::Invalid("profile digest is invalid".into()));
            }
        }
        Ok(())
    }
}

fn clamp_oracle(input: MirrorClampInput) -> Result<MirrorClampOutput> {
    validate_input(input)?;
    let clamped = if input.value < input.min {
        input.min
    } else if input.value > input.max {
        input.max
    } else {
        input.value
    };
    Ok(MirrorClampOutput { clamped })
}

fn validate_input(input: MirrorClampInput) -> Result<()> {
    if input.min < -1_000_000
        || input.max > 1_000_000
        || input.value < -1_000_000
        || input.value > 1_000_000
        || input.min > input.max
    {
        return Err(Error::Invalid(
            "clamp input outside registered domain".into(),
        ));
    }
    Ok(())
}

fn registered_runner_contract_body() -> serde_json::Value {
    serde_json::json!({
        "schema_version": REGISTERED_RUNNER_CONTRACT_SCHEMA,
        "runtime": "in_process_registered_pure_function",
        "isolation": "none",
        "provider_dispatch": false,
        "model_execution": false,
        "targets": [CLAMP_TARGET_ID],
    })
}

fn registered_runner_digest() -> Result<String> {
    fingerprint(&registered_runner_contract_body())
}

impl MirrorRegisteredTargetInputV1 {
    fn target_id(&self) -> &'static str {
        match self {
            Self::ClampI64 { .. } => CLAMP_TARGET_ID,
        }
    }
    fn oracle_answer_json(&self) -> Result<String> {
        match *self {
            Self::ClampI64 { value, min, max } => {
                let output = clamp_oracle(MirrorClampInput { value, min, max })?;
                serde_json::to_string(&output).map_err(|_| Error::Internal)
            }
        }
    }
}

impl MirrorDevelopmentTaskSpecV1 {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.task_id)?;
        identifier(&self.parent_family)?;
        if self.profile_id != PURE_FUNCTION_PROFILE_ID
            || self.target_id != self.input.target_id()
            || self.target_id != CLAMP_TARGET_ID
        {
            return Err(Error::Invalid(
                "development task targets an unregistered pure function".into(),
            ));
        }
        wire_digest(&self.input_digest, "task input_digest")?;
        if self.input_digest != fingerprint(&self.input)? {
            return Err(Error::Conflict(
                "development task input digest differs from its typed input".into(),
            ));
        }
        if self.expected_answer_json != self.input.oracle_answer_json()? {
            return Err(Error::Conflict(
                "development task expected answer differs from the registered oracle".into(),
            ));
        }
        Ok(())
    }

    fn manifest_task(&self) -> MirrorDevelopmentTask {
        MirrorDevelopmentTask {
            id: self.task_id.clone(),
            parent_family: self.parent_family.clone(),
            input_digest: self.input_digest.clone(),
        }
    }
}

impl MirrorDevelopmentControlV1 {
    pub const SCHEMA: &'static str = DEVELOPMENT_CONTROL_SCHEMA;

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != Self::SCHEMA {
            return Err(Error::Invalid(
                "unsupported development control schema".into(),
            ));
        }
        for value in [
            &self.id,
            &self.namespace,
            &self.billing_scope,
            &self.root_budget_id,
            &self.executor_actor,
            &self.grader_actor,
            &self.proposer_actor,
            &self.manifest_id,
        ] {
            identifier(value)?;
        }
        for (value, name) in [
            (&self.manifest_digest, "manifest_digest"),
            (&self.environment_digest, "environment_digest"),
            (&self.grader_digest, "grader_digest"),
            (&self.oracle_digest, "oracle_digest"),
            (&self.target_digest, "target_digest"),
            (&self.runner_digest, "runner_digest"),
            (&self.rules_digest, "rules_digest"),
            (&self.tools_digest, "tools_digest"),
        ] {
            wire_digest(value, name)?;
        }
        let actors: BTreeSet<&str> = [
            self.executor_actor.as_str(),
            self.grader_actor.as_str(),
            self.proposer_actor.as_str(),
        ]
        .into_iter()
        .collect();
        if actors.len() != 3 {
            return Err(Error::Invalid(
                "executor, grader, and proposer must be distinct actors".into(),
            ));
        }
        self.fixed_grader.validate()?;
        if self.grader_digest != fingerprint(&self.fixed_grader)? {
            return Err(Error::Invalid(
                "grader_digest does not identify the frozen grader".into(),
            ));
        }
        let profile = MirrorRegisteredPureFunctionProfileV1::clamp_i64();
        profile.validate()?;
        if self.oracle_digest != profile.oracle_digest
            || self.target_digest != profile.target_digest
            || self.runner_digest != registered_runner_digest()?
        {
            return Err(Error::Invalid(
                "control does not bind the registered oracle, target, and runner".into(),
            ));
        }
        if self.tasks.is_empty() {
            return Err(Error::Invalid("development control has no tasks".into()));
        }
        for task in &self.tasks {
            task.validate()?;
        }
        if self.manifest()?.digest != self.manifest_digest {
            return Err(Error::Conflict(
                "control manifest digest differs from its frozen tasks".into(),
            ));
        }
        canonical_ids(&self.source_ids, "control source_ids")?;
        valid_time(self.created_at_unix_seconds)?;
        Ok(())
    }

    pub fn manifest(&self) -> Result<MirrorDevelopmentManifest> {
        MirrorDevelopmentManifest::build(
            self.manifest_id.clone(),
            self.tasks
                .iter()
                .map(MirrorDevelopmentTaskSpecV1::manifest_task)
                .collect(),
        )
    }

    pub fn task(&self, task_id: &str) -> Result<&MirrorDevelopmentTaskSpecV1> {
        self.tasks
            .iter()
            .find(|task| task.task_id == task_id)
            .ok_or_else(|| Error::Conflict("development task is not registered".into()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorDevelopmentTask {
    pub id: String,
    pub parent_family: String,
    pub input_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorDevelopmentManifest {
    pub id: String,
    pub tasks: Vec<MirrorDevelopmentTask>,
    pub digest: String,
}

impl MirrorDevelopmentManifest {
    pub fn build(id: impl Into<String>, tasks: Vec<MirrorDevelopmentTask>) -> Result<Self> {
        let id = id.into();
        identifier(&id)?;
        if tasks.is_empty() {
            return Err(Error::Invalid("development manifest is empty".into()));
        }
        let mut ids = BTreeSet::new();
        for task in &tasks {
            identifier(&task.id)?;
            identifier(&task.parent_family)?;
            wire_digest(&task.input_digest, "development task input digest")?;
            if !ids.insert(task.id.as_str()) {
                return Err(Error::Conflict("duplicate development task".into()));
            }
        }
        let digest = fingerprint(&(id.as_str(), tasks.as_slice()))?;
        Ok(Self { id, tasks, digest })
    }
}

const PRACTICE_REGISTRATION_SCHEMA: &str = "rsia.practice_registration.v1";
const PRACTICE_ATTEMPT_SET_SCHEMA: &str = "rsia.practice_attempt_set.v1";
const REGISTERED_PRACTICE_ATTEMPTS: u8 = 3;
const MAX_SCORE_MICROS: u32 = 1000000;

impl MirrorPracticeRegistrationV1 {
    pub const SCHEMA: &'static str = PRACTICE_REGISTRATION_SCHEMA;

    /// The plan this record authorizes, rebuilt from its own fields.
    fn registered_plan(&self) -> Result<PracticePlan> {
        if self.k != REGISTERED_PRACTICE_ATTEMPTS {
            return Err(Error::Invalid(
                "a practice registration authorizes K=3 only".into(),
            ));
        }
        PracticePlan::new(
            self.parent_cluster_id.clone(),
            self.task_ids.clone(),
            self.k,
            Some(self.id.clone()),
        )
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != Self::SCHEMA {
            return Err(Error::Invalid(
                "unsupported practice registration schema".into(),
            ));
        }
        wire_digest(&self.id, "practice registration id")?;
        wire_digest(&self.plan_digest, "practice registration plan_digest")?;
        identifier(&self.registered_by)?;
        valid_time(self.created_at)?;
        if fingerprint(&self.registered_plan()?)? != self.plan_digest {
            return Err(Error::Conflict(
                "practice registration plan digest differs from its own plan".into(),
            ));
        }
        Ok(())
    }

    /// Whether this record authorizes exactly `plan`.
    pub fn binds_plan(&self, plan: &PracticePlan) -> Result<()> {
        if plan.authorization_digest.as_deref() != Some(self.id.as_str())
            || plan.attempts_per_task != self.k
            || plan.parent_cluster_id != self.parent_cluster_id
            || plan.task_ids != self.task_ids
            || fingerprint(plan)? != self.plan_digest
        {
            return Err(Error::Conflict(
                "practice registration does not authorize this plan".into(),
            ));
        }
        Ok(())
    }
}

impl MirrorPracticeTaskResultV1 {
    fn validate(&self) -> Result<()> {
        identifier(&self.task_id)?;
        identifier(&self.parent_family)?;
        wire_digest(&self.output_digest, "practice task output_digest")?;
        if self.score_micros > MAX_SCORE_MICROS {
            return Err(Error::Invalid(
                "practice score micros must be within 0..=1000000".into(),
            ));
        }
        Ok(())
    }
}

impl MirrorPracticeAttemptV1 {
    fn validate(&self, set_id: &str, position: usize, task_ids: &BTreeSet<&str>) -> Result<()> {
        if usize::try_from(self.attempt).ok() != Some(position) {
            return Err(Error::Conflict(
                "practice attempts must be numbered 1.. in order".into(),
            ));
        }
        if self.request_id != practice_attempt_request_id(set_id, self.attempt)?
            || self.execution_receipt_id != run_receipt_id(&self.request_id)?
        {
            return Err(Error::Conflict(
                "practice attempt ids are not derived from its set".into(),
            ));
        }
        let mut seen = BTreeSet::new();
        for task in &self.tasks {
            task.validate()?;
            if !seen.insert(task.task_id.as_str()) {
                return Err(Error::Conflict("practice attempt repeats a task".into()));
            }
        }
        if &seen != task_ids {
            return Err(Error::Conflict(
                "practice attempt tasks differ from the plan".into(),
            ));
        }
        if let MirrorDevelopmentCostState::Known {
            micros,
            currency,
            pricing_version,
        } = &self.cost_state
        {
            identifier(currency)?;
            identifier(pricing_version)?;
            if *micros < 0 {
                return Err(Error::Invalid("practice cost must be nonnegative".into()));
            }
        }
        Ok(())
    }
}

impl MirrorPracticeAttemptSetV1 {
    pub const SCHEMA: &'static str = PRACTICE_ATTEMPT_SET_SCHEMA;

    /// Attempts that really ran; a cache hit is reused history, not a new
    /// practice.
    /// Structural and semantic checks of a stored or freshly built set,
    /// including that the outcome is exactly what its attempts imply.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != Self::SCHEMA {
            return Err(Error::Invalid(
                "unsupported practice attempt set schema".into(),
            ));
        }
        if self.counts_as_independent_samples {
            return Err(Error::Conflict(
                "a practice attempt set never counts as independent samples".into(),
            ));
        }
        for value in [
            &self.id,
            &self.set_id,
            &self.namespace,
            &self.control_id,
            &self.episode_id,
        ] {
            identifier(value)?;
        }
        if self.id != practice_attempt_set_storage_id(&self.set_id)? {
            return Err(Error::Conflict(
                "practice attempt set id is not derived from its set id".into(),
            ));
        }
        for (value, name) in [
            (&self.bundle_digest, "practice bundle_digest"),
            (&self.environment_digest, "practice environment_digest"),
            (&self.request_digest, "practice request_digest"),
        ] {
            wire_digest(value, name)?;
        }
        valid_time(self.created_at)?;
        self.plan.validate()?;
        let k = usize::from(self.plan.attempts_per_task);
        match (&self.registration_id, &self.plan.authorization_digest) {
            (Some(registration), Some(authorization))
                if k == usize::from(REGISTERED_PRACTICE_ATTEMPTS)
                    && registration == authorization => {}
            (None, None) if k == 1 => {}
            _ => {
                return Err(Error::Conflict(
                    "practice set registration differs from its plan".into(),
                ));
            }
        }
        if self.attempts.len() > k {
            return Err(Error::Conflict(
                "practice set holds more attempts than its K".into(),
            ));
        }
        let task_ids: BTreeSet<&str> = self.plan.task_ids.iter().map(String::as_str).collect();
        for (index, attempt) in self.attempts.iter().enumerate() {
            attempt.validate(&self.set_id, index + 1, &task_ids)?;
        }
        let clusters = usize::try_from(self.independent_clusters).map_err(|_| Error::Internal)?;
        let order = |attempt: &MirrorPracticeAttemptV1| -> Vec<(String, String)> {
            attempt
                .tasks
                .iter()
                .map(|task| (task.task_id.clone(), task.parent_family.clone()))
                .collect()
        };
        if let Some(first) = self.attempts.first() {
            let expected = order(first);
            if self
                .attempts
                .iter()
                .any(|attempt| order(attempt) != expected)
            {
                return Err(Error::Conflict(
                    "practice attempts disagree on their tasks or families".into(),
                ));
            }
            let families: BTreeSet<&str> = first
                .tasks
                .iter()
                .map(|task| task.parent_family.as_str())
                .collect();
            if clusters != families.len() {
                return Err(Error::Conflict(
                    "practice independent clusters differ from the task families".into(),
                ));
            }
        } else if clusters == 0 || clusters > self.plan.task_ids.len() {
            return Err(Error::Conflict(
                "practice independent clusters are outside the task count".into(),
            ));
        }
        let uncertain = self
            .attempts
            .iter()
            .position(|attempt| attempt.cost_state == MirrorDevelopmentCostState::Uncertain);
        if uncertain.is_some_and(|index| index + 1 != self.attempts.len()) {
            return Err(Error::Conflict(
                "a practice set continued after an uncertain attempt".into(),
            ));
        }
        match &self.outcome {
            MirrorPracticeOutcomeV1::Incomplete { reason } => {
                identifier(reason)?;
                if self.attempts.len() == k && uncertain.is_none() {
                    return Err(Error::Conflict(
                        "an incomplete practice set ran all of its attempts".into(),
                    ));
                }
            }
            outcome => {
                if self.attempts.len() != k || uncertain.is_some() {
                    return Err(Error::Conflict(
                        "a finished practice set holds all K certain attempts".into(),
                    ));
                }
                if *outcome != select_contrast(&self.attempts) {
                    return Err(Error::Conflict(
                        "practice outcome differs from its attempts".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

fn select_contrast(attempts: &[MirrorPracticeAttemptV1]) -> MirrorPracticeOutcomeV1 {
    let Some(first) = attempts.first() else {
        return MirrorPracticeOutcomeV1::NoContrast;
    };
    for task in &first.tasks {
        let mut better: Option<(u32, (u32, bool))> = None;
        let mut worse: Option<(u32, (u32, bool))> = None;
        for attempt in attempts {
            let Some(result) = attempt
                .tasks
                .iter()
                .find(|result| result.task_id == task.task_id)
            else {
                continue;
            };
            let key = (result.score_micros, result.passed);
            if better.is_none_or(|(_, best)| key > best) {
                better = Some((attempt.attempt, key));
            }
            if worse.is_none_or(|(_, worst)| key < worst) {
                worse = Some((attempt.attempt, key));
            }
        }
        if let (Some((better_attempt, best)), Some((worse_attempt, worst))) = (better, worse)
            && best > worst
        {
            return MirrorPracticeOutcomeV1::Contrast {
                better_attempt,
                worse_attempt,
                task_id: task.task_id.clone(),
            };
        }
    }
    MirrorPracticeOutcomeV1::NoContrast
}

fn practice_attempt_set_storage_id(set_id: &str) -> Result<String> {
    identifier(set_id)?;
    Ok(format!(
        "practice-set-{}",
        fingerprint(&(PRACTICE_ATTEMPT_SET_SCHEMA, set_id))?
    ))
}

fn practice_attempt_request_id(set_id: &str, attempt: u32) -> Result<String> {
    identifier(set_id)?;
    if attempt == 0 || attempt > u32::from(REGISTERED_PRACTICE_ATTEMPTS) {
        return Err(Error::Invalid("practice attempt is outside 1..=3".into()));
    }
    Ok(format!(
        "practice-{}",
        fingerprint(&("rsia.practice_attempt_request.v1", set_id, attempt))?
    ))
}

fn run_receipt_id(request_id: &str) -> Result<String> {
    Ok(format!("devrun-{}", fingerprint(&request_id)?))
}

fn development_storage_id(record_kind: &str, id: &str) -> Result<String> {
    identifier(record_kind)?;
    identifier(id)?;
    Ok(format!("e03dev-{}", fingerprint(&(record_kind, id))?))
}

#[derive(Clone)]
struct OccurrenceCache {
    namespace: String,
    actor: String,
    operation: String,
    request_key: String,
    payload_hash: String,
    subject_id: String,
    response: Value,
    redacted: bool,
}
fn load_caches(conn: &RawDatabase, snapshot: &mut OccurrenceSnapshot) -> Result<()> {
    for row in rows(
        conn,
        "idempotency",
        &[
            "namespace",
            "actor",
            "operation",
            "request_key",
            "payload_hash",
            "subject_id",
            "response",
            "redacted",
        ],
    )? {
        let entry = OccurrenceCache {
            namespace: sql_string(&row, "namespace")?,
            actor: sql_string(&row, "actor")?,
            operation: sql_string(&row, "operation")?,
            request_key: sql_string(&row, "request_key")?,
            payload_hash: sql_string(&row, "payload_hash")?,
            subject_id: sql_string(&row, "subject_id")?,
            response: parse_owned_json(&sql_string(&row, "response")?)?,
            redacted: sql_bool(&row, "redacted")?,
        };
        for id in [
            &entry.namespace,
            &entry.actor,
            &entry.operation,
            &entry.request_key,
            &entry.subject_id,
        ] {
            identifier(id).map_err(|_| Error::ClockRegressed)?;
        }
        digest(&entry.payload_hash)?;
        if entry.redacted {
            require(entry.response.is_null())?;
        }
        snapshot.caches.push(entry);
    }
    Ok(())
}
fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let digest = hash(parts.join("\0").as_bytes());
    format!("{prefix}-{}", &digest[..24])
}
fn evaluation_storage_id(kind: &str, id: &str) -> Result<String> {
    Ok(format!("e05-{}", typed_fingerprint(&(kind, id))?))
}
fn envelope<'a>(object: &'a OccurrenceObject, schema: &str, kind: &str) -> Result<&'a Value> {
    if object.body.get("schema_version").and_then(Value::as_str)
        == Some(evo_core::formal_version::NATIVE_TICKET_ENVELOPE)
    {
        require(
            matches!(
                schema,
                "rsia.typed_artifact_envelope.v1"
                    | evo_core::formal_version::NATIVE_TICKET_ENVELOPE
            ) && kind == "evaluation_ticket_v2"
                && object.kind == "artifact",
        )?;
        return evo_core::formal_version::native_ticket_payload(
            &object.body,
            &object.namespace,
            &object.id,
        )
        .map_err(|_| Error::ClockRegressed);
    }
    exact(
        &object.body,
        &["schema_version", "id", "record_kind", "payload"],
        &[],
    )?;
    require(
        object.kind == "artifact"
            && string(&object.body, "schema_version")? == schema
            && string(&object.body, "id")? == object.id
            && string(&object.body, "record_kind")? == kind,
    )?;
    let payload = object.body.get("payload").ok_or(Error::ClockRegressed)?;
    require(payload.is_object())?;
    Ok(payload)
}
fn development_control(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    id: &str,
) -> Result<MirrorDevelopmentControlV1> {
    let physical = development_storage_id("development_control_v1", id)?;
    let object = snapshot.object(namespace, "artifact", &physical)?;
    let payload = envelope(
        object,
        "rsia.typed_artifact_envelope.v1",
        "development_control_v1",
    )?;
    let control: MirrorDevelopmentControlV1 = decode(payload)?;
    control.validate().map_err(|_| Error::ClockRegressed)?;
    require(
        control.id == id
            && control.namespace == namespace
            && object.owner != control.executor_actor
            && object.owner != control.grader_actor,
    )?;
    let root = snapshot
        .roots
        .get(&control.billing_scope)
        .ok_or(Error::ClockRegressed)?;
    require(
        root.root_budget_id == control.root_budget_id
            && root
                .allowed_namespaces
                .iter()
                .any(|value| value == namespace),
    )?;
    for source in &control.source_ids {
        snapshot.object(namespace, "run", source)?;
    }
    Ok(control)
}
fn evaluation_control(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    id: &str,
) -> Result<MirrorRegisteredEvaluationControl> {
    let physical = evaluation_storage_id("registered_evaluation_control_v41", id)?;
    let object = snapshot.object(namespace, "artifact", &physical)?;
    let payload = envelope(
        object,
        "rsia.typed_artifact_envelope.v1",
        "registered_evaluation_control_v41",
    )?;
    let control: MirrorRegisteredEvaluationControl = decode(payload)?;
    control.validate().map_err(|_| Error::ClockRegressed)?;
    require(control.id == id && object.owner == control.evaluator_actor)?;
    Ok(control)
}
fn evaluation_holdout(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    id: &str,
    control: &MirrorRegisteredEvaluationControl,
) -> Result<MirrorProtectedHoldoutRecord> {
    let object = snapshot.object(
        namespace,
        "artifact",
        &evaluation_storage_id("protected_holdout_v41", id)?,
    )?;
    let payload = envelope(
        object,
        "rsia.typed_artifact_envelope.v1",
        "protected_holdout_v41",
    )?;
    let holdout: MirrorProtectedHoldoutRecord = decode(payload)?;
    holdout
        .validate_against(control)
        .map_err(|_| Error::ClockRegressed)?;
    require(holdout.id == id && object.owner == control.evaluator_actor)?;
    Ok(holdout)
}
fn evaluation_ticket(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    id: &str,
) -> Result<MirrorEvaluationTicketV2> {
    evaluation_ticket_with_relations(snapshot, namespace, id, None)
}

fn evaluation_ticket_with_relations(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    id: &str,
    checked_relations: Option<&mut BTreeSet<(String, String)>>,
) -> Result<MirrorEvaluationTicketV2> {
    let object = snapshot.object(
        namespace,
        "artifact",
        &evaluation_storage_id("evaluation_ticket_v2", id)?,
    )?;
    let payload = envelope(
        object,
        "rsia.typed_artifact_envelope.v1",
        "evaluation_ticket_v2",
    )?;
    // The original decoder permits an absent historical binding, never present null.
    if let Some(binding) = payload.get("candidate_dependency_binding") {
        require(!binding.is_null())?;
        let binding: MirrorFormalCandidateDependencyBinding = decode(binding)?;
        binding.validate().map_err(|_| Error::ClockRegressed)?;
        require(binding.namespace == namespace)?;
    }
    let ticket: MirrorEvaluationTicketV2 = decode(payload)?;
    ticket.validate().map_err(|_| Error::ClockRegressed)?;
    let control = evaluation_control(snapshot, namespace, &ticket.registration_id)?;
    let holdout = evaluation_holdout(snapshot, namespace, &ticket.holdout_id, &control)?;
    require(
        ticket.id == id
            && ticket.registration_digest == typed_fingerprint(&control)?
            && ticket.manifest_digest
                == holdout
                    .manifest
                    .digest()
                    .map_err(|_| Error::ClockRegressed)?
            && ticket.evaluator_actor == control.evaluator_actor
            && ticket.executor_actor == control.executor_actor
            && ticket.proposer_actor == control.proposer_actor
            && ticket.approver_actor == control.approver_actor
            && [
                control.evaluator_actor.as_str(),
                control.executor_actor.as_str(),
            ]
            .contains(&object.owner.as_str())
            && ticket.candidate_digest == holdout.pair.candidate_digest
            && ticket.baseline_digest == holdout.pair.baseline_digest,
    )?;
    if let Some(binding) = &ticket.candidate_dependency_binding {
        if let Some(checked) = checked_relations {
            let key = (namespace.to_owned(), id.to_owned());
            if !checked.contains(&key) {
                e05_candidate_binding(snapshot, namespace, &ticket, &control, &holdout, binding)?;
                e05_bound_fact_edges(snapshot, namespace, &ticket, &control)?;
                // Insert only after BOTH complete original relation checks returned Ok.
                checked.insert(key);
            }
        } else {
            e05_candidate_binding(snapshot, namespace, &ticket, &control, &holdout, binding)?;
            e05_bound_fact_edges(snapshot, namespace, &ticket, &control)?;
        }
    }
    Ok(ticket)
}

fn evaluation_ticket_for_qualification(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    id: &str,
    ticket_scan: Option<&mut TicketRelationScan<'_>>,
) -> Result<MirrorEvaluationTicketV2> {
    match ticket_scan {
        Some(scan) => {
            // A private scan cannot lend successful IDs to a different snapshot.
            require(std::ptr::eq(snapshot, scan.snapshot))?;
            let key = (namespace.to_owned(), id.to_owned());
            if let Some(ticket) = scan.successful_tickets.get(&key) {
                return Ok(ticket.clone());
            }
            let ticket = evaluation_ticket_with_relations(
                scan.snapshot,
                namespace,
                id,
                Some(&mut scan.checked_relations),
            )?;
            scan.successful_tickets.insert(key, ticket.clone());
            Ok(ticket)
        }
        None => evaluation_ticket(snapshot, namespace, id),
    }
}

fn qualified_call<'a>(
    snapshot: &'a OccurrenceSnapshot,
    namespace: &str,
    scope: &str,
    id: &str,
) -> Result<&'a BudgetCallRecord> {
    let call = snapshot
        .calls
        .get(&(scope.into(), id.into()))
        .ok_or(Error::ClockRegressed)?;
    require(call.namespace == namespace)?;
    Ok(call)
}
fn observe_seconds_field(floor: &mut OccurrenceFloor, value: &Value, field: &str) -> Result<()> {
    floor.observe_seconds(nonnegative(value, field)?)
}
fn observe_millis_field(floor: &mut OccurrenceFloor, value: &Value, field: &str) -> Result<()> {
    floor.observe_millis(nonnegative(value, field)?)
}
fn qualify_evaluation(
    object: &OccurrenceObject,
    snapshot: &OccurrenceSnapshot,
    floor: &mut OccurrenceFloor,
) -> Result<()> {
    qualify_evaluation_with_ticket_scan(object, snapshot, floor, None)
}

fn qualify_evaluation_with_ticket_scan(
    object: &OccurrenceObject,
    snapshot: &OccurrenceSnapshot,
    floor: &mut OccurrenceFloor,
    ticket_scan: Option<&mut TicketRelationScan<'_>>,
) -> Result<()> {
    let kind = string(&object.body, "record_kind")?;
    let payload = envelope(object, "rsia.typed_artifact_envelope.v1", kind)?;
    let id_field = match kind {
        "evaluation_ticket_issue_receipt_v1"
        | "evaluation_ticket_issue_receipt_v2"
        | "formal_evaluation_v2"
        | "evaluation_resource_evidence_v1" => "ticket_id",
        "execution_receipt_v2" | "independent_grader_receipt_v2" => "receipt_id",
        "exposure_ledger_v1" => "research_family_id",
        _ => "id",
    };
    let alpha_id;
    let logical_id = if kind == "alpha_claim_reservation_v1" {
        alpha_id = format!(
            "{}:{}",
            string(payload, "research_family_id")?,
            string(payload, "claim_id")?
        );
        alpha_id.as_str()
    } else {
        string(payload, id_field)?
    };
    require(object.id == evaluation_storage_id(kind, logical_id)?)?;
    match kind {
        "registered_evaluation_control_v41" => {
            evaluation_control(snapshot, &object.namespace, logical_id)?;
            observe_seconds_field(floor, payload, "created_at_unix_seconds")?;
            let plan = payload
                .get("v1_plan_snapshot")
                .ok_or(Error::ClockRegressed)?;
            observe_seconds_field(floor, plan, "frozen_at")?;
        }
        "protected_holdout_v41" => {
            let control = evaluation_control(
                snapshot,
                &object.namespace,
                string(payload, "registration_id")?,
            )?;
            evaluation_holdout(snapshot, &object.namespace, logical_id, &control)?;
            observe_millis_field(floor, &payload["pair"], "frozen_at_unix_ms")?;
            observe_millis_field(floor, &payload["manifest"], "issued_at_unix_ms")?;
        }
        "evaluation_ticket_v2" => {
            let ticket = evaluation_ticket_for_qualification(
                snapshot,
                &object.namespace,
                logical_id,
                ticket_scan,
            )?;
            floor.observe_seconds(ticket.issued_at_unix_seconds)?;
            e05_issue::validate_native_ticket_stop_r44(object, &ticket)?;
            if let Some(certificate) = &ticket.pending_certificate {
                certificate
                    .validate_shape()
                    .map_err(|_| Error::ClockRegressed)?;
                observe_millis_field(floor, &payload["pending_certificate"], "stopped_at_unix_ms")?;
            }
            for target in &ticket.targets {
                let slots = match &target.state {
                    MirrorTargetState::Running {
                        candidate,
                        baseline,
                    }
                    | MirrorTargetState::CancelledAfterDispatch {
                        candidate,
                        baseline,
                    }
                    | MirrorTargetState::RunningUsageUnknown {
                        candidate,
                        baseline,
                    } => Some([candidate, baseline]),
                    _ => None,
                };
                if let Some(slots) = slots {
                    for slot in slots.into_iter().flatten() {
                        slot.validate().map_err(|_| Error::ClockRegressed)?;
                        let control = evaluation_control(
                            snapshot,
                            &object.namespace,
                            &ticket.registration_id,
                        )?;
                        let call = qualified_call(
                            snapshot,
                            &object.namespace,
                            &control.billing_scope,
                            &slot.budget_call_id,
                        )?;
                        require(
                            call.dispatch_group_id == ticket.id
                                && call.actual_input_digest == slot.request_digest,
                        )?;
                        match &slot.phase {
                            MirrorExecutionSlotPhase::ReservedBound => {}
                            MirrorExecutionSlotPhase::Dispatched {
                                dispatch_id,
                                dispatched_at_unix_seconds,
                            } => {
                                require(
                                    call.dispatch_id.as_deref() == Some(dispatch_id.as_str())
                                        && call.dispatched_at == Some(*dispatched_at_unix_seconds),
                                )?;
                                floor.observe_seconds(*dispatched_at_unix_seconds)?;
                            }
                        }
                    }
                }
            }
        }
        "evaluation_ticket_issue_receipt_v1" => {
            let receipt: MirrorTicketIssueReceipt = decode(payload)?;
            require(receipt.schema_version == "rsia.evaluation_ticket_issue_receipt.v1")?;
            let ticket = evaluation_ticket_for_qualification(
                snapshot,
                &object.namespace,
                logical_id,
                ticket_scan,
            )?;
            require(
                receipt.registration_digest == ticket.registration_digest
                    && receipt.manifest_digest == ticket.manifest_digest
                    && receipt.evaluator_actor == ticket.evaluator_actor
                    && object.owner == receipt.evaluator_actor,
            )?;
            floor.observe_seconds(receipt.issued_at_unix_seconds)?;
        }
        "evaluation_ticket_issue_receipt_v2" => {
            let ticket = evaluation_ticket_for_qualification(
                snapshot,
                &object.namespace,
                logical_id,
                ticket_scan,
            )?;
            e05_issue_receipt_binding(
                "evaluation_ticket_issue_receipt_v2",
                ticket.candidate_dependency_binding.as_ref(),
            )?;
            let (kind, receipt) = e05_issue_receipt(snapshot, &object.namespace, &ticket)?;
            require(kind == "evaluation_ticket_issue_receipt_v2")?;
            floor.observe_seconds(receipt.issued_at_unix_seconds)?;
            floor.observe_millis(receipt.issued_at_unix_ms.ok_or(Error::ClockRegressed)?)?;
        }
        "execution_receipt_v2" => {
            let receipt: MirrorExecutionReceiptV2 = decode(payload)?;
            require(
                receipt.schema_version == "rsia.execution_receipt.v2"
                    && object.owner == receipt.executor_actor,
            )?;
            let ticket = evaluation_ticket_for_qualification(
                snapshot,
                &object.namespace,
                &receipt.ticket_id,
                ticket_scan,
            )?;
            let control = evaluation_control(snapshot, &object.namespace, &ticket.registration_id)?;
            let target = ticket
                .targets
                .iter()
                .find(|target| target.target_id == receipt.target_id)
                .ok_or(Error::ClockRegressed)?;
            require(
                receipt.executor_actor == control.executor_actor
                    && receipt.environment_digest == ticket.environment_digest,
            )?;
            let bundle = match receipt.side {
                MirrorExecutionSide::Candidate => &ticket.candidate_bundle_digest,
                MirrorExecutionSide::Baseline => &ticket.baseline_bundle_digest,
            };
            require(
                &receipt.bundle_digest == bundle
                    && receipt.request_digest
                        == typed_fingerprint(&(
                            "rsia.evaluation_execution_request.v1",
                            &ticket.id,
                            &target.target_id,
                            &target.input_digest,
                            receipt.side,
                            bundle,
                            &ticket.environment_digest,
                            &ticket.manifest_digest,
                        ))?,
            )?;
            let call = qualified_call(
                snapshot,
                &object.namespace,
                &control.billing_scope,
                &receipt.budget_call_id,
            )?;
            // The real output receipt can precede financial settlement.
            // Pending calls prove dispatch identity; CompleteBatch below still
            // requires finalized usage for every execution.
            require(
                call.stage == BudgetStage::TaskExecution
                    && matches!(
                        call.state,
                        BudgetCallState::Dispatched
                            | BudgetCallState::Uncertain
                            | BudgetCallState::Finalized
                    )
                    && call.actual_input_digest == receipt.request_digest
                    && call.dispatch_group_id == ticket.id
                    && (call.state != BudgetCallState::Finalized
                        || call.output_digest.as_deref() == Some(receipt.output_digest.as_str())),
            )?;
            let output = snapshot.object(
                &object.namespace,
                "artifact",
                &evaluation_storage_id(
                    "evaluation_execution_output_v1",
                    &receipt.output_artifact_id,
                )?,
            )?;
            let output: MirrorExecutionOutputArtifact = decode(envelope(
                output,
                "rsia.typed_artifact_envelope.v1",
                "evaluation_execution_output_v1",
            )?)?;
            require(
                output.id == receipt.output_artifact_id
                    && output.ticket_id == ticket.id
                    && output.target_id == target.target_id
                    && output.side == receipt.side
                    && hash(output.output_utf8.as_bytes()) == receipt.output_digest
                    && output.output_digest == receipt.output_digest,
            )?;
            floor.observe_seconds(receipt.issued_at_unix_seconds)?;
        }
        "independent_grader_receipt_v2" => {
            let receipt: MirrorIndependentGraderReceiptV2 = decode(payload)?;
            require(
                receipt.schema_version == "rsia.independent_grader_receipt.v2"
                    && object.owner == receipt.evaluator_actor
                    && receipt.candidate_score_micros <= 1_000_000
                    && receipt.baseline_score_micros <= 1_000_000,
            )?;
            let ticket = evaluation_ticket_for_qualification(
                snapshot,
                &object.namespace,
                &receipt.ticket_id,
                ticket_scan,
            )?;
            let control = evaluation_control(snapshot, &object.namespace, &ticket.registration_id)?;
            require(
                receipt.evaluator_actor == control.evaluator_actor
                    && receipt.grader_digest == control.grader_digest
                    && receipt.oracle_digest == control.oracle_digest,
            )?;
            for (id, output, side) in [
                (
                    &receipt.candidate_execution_receipt_id,
                    &receipt.candidate_output_digest,
                    MirrorExecutionSide::Candidate,
                ),
                (
                    &receipt.baseline_execution_receipt_id,
                    &receipt.baseline_output_digest,
                    MirrorExecutionSide::Baseline,
                ),
            ] {
                let object = snapshot.object(
                    &object.namespace,
                    "artifact",
                    &evaluation_storage_id("execution_receipt_v2", id)?,
                )?;
                let execution: MirrorExecutionReceiptV2 = decode(envelope(
                    object,
                    "rsia.typed_artifact_envelope.v1",
                    "execution_receipt_v2",
                )?)?;
                require(
                    execution.receipt_id == *id
                        && execution.ticket_id == ticket.id
                        && execution.target_id == receipt.target_id
                        && execution.output_digest == *output
                        && execution.side == side,
                )?;
            }
            floor.observe_seconds(receipt.issued_at_unix_seconds)?;
        }
        "formal_evaluation_v2" => {
            if let Some(binding) = payload.get("stored_dependency_binding") {
                require(!binding.is_null())?;
                decode::<MirrorFormalStoredDependencyBinding>(binding)?
                    .validate()
                    .map_err(|_| Error::ClockRegressed)?;
            }
            let formal: MirrorFormalEvaluationV2 = decode(payload)?;
            let ticket = evaluation_ticket_for_qualification(
                snapshot,
                &object.namespace,
                logical_id,
                ticket_scan,
            )?;
            require(object.owner == ticket.evaluator_actor)?;
            match formal {
                MirrorFormalEvaluationV2::EarlyStopped {
                    evidence_scope,
                    ticket_id,
                    ticket_digest,
                    certificate,
                } => {
                    certificate
                        .validate_shape()
                        .map_err(|_| Error::ClockRegressed)?;
                    require(
                        ticket_digest == typed_fingerprint(&ticket)?
                            && certificate.ticket_id() == ticket.id,
                    )?;
                    require(ticket_id == ticket.id)?;
                    e05_early_identity(
                        snapshot,
                        &object.namespace,
                        &ticket,
                        &certificate,
                        evidence_scope,
                    )?;
                    observe_millis_field(floor, &payload["certificate"], "stopped_at_unix_ms")?;
                }
                complete @ MirrorFormalEvaluationV2::CompleteBatch { .. } => {
                    let _read_class = e05_complete_batch(snapshot, object, &ticket, &complete)?;
                }
                MirrorFormalEvaluationV2::Invalid {
                    ticket_id,
                    ticket_digest,
                    ..
                } => {
                    require(
                        ticket_id == ticket.id && ticket_digest == typed_fingerprint(&ticket)?,
                    )?;
                }
            }
        }
        "exposure_ledger_v1" => {
            let ledger: ExposureLedger = decode(payload)?;
            ledger
                .validate_restore()
                .map_err(|_| Error::ClockRegressed)?;
            for entry in &ledger.entries {
                match entry.state {
                    ExposureState::Reserved => {}
                    ExposureState::Dispatched {
                        dispatched_at_unix_ms,
                    } => floor.observe_millis(dispatched_at_unix_ms)?,
                    ExposureState::FeedbackUsed { used_at_unix_ms } => {
                        floor.observe_millis(used_at_unix_ms)?
                    }
                }
            }
        }
        "evaluation_resource_evidence_v1" => {
            let resource: MirrorResourceEvidenceRecord = decode(payload)?;
            require(resource.schema_version == "rsia.evaluation_resource_evidence.v1")?;
            let mut ids = BTreeSet::new();
            for copy in &resource.calls {
                require(ids.insert(&copy.call_id))?;
                let actual = qualified_call(
                    snapshot,
                    &object.namespace,
                    &resource.billing_scope,
                    &copy.call_id,
                )?;
                require(actual == copy)?;
            }
        }
        "evaluation_execution_output_v1" => {
            let output: MirrorExecutionOutputArtifact = decode(payload)?;
            require(
                output.schema_version == "rsia.execution_output.v1"
                    && hash(output.output_utf8.as_bytes()) == output.output_digest,
            )?;
        }
        "alpha_claim_reservation_v1" => {
            exact(
                payload,
                &[
                    "schema_version",
                    "research_family_id",
                    "claim_id",
                    "attempt_id",
                    "ticket_id",
                ],
                &[],
            )?;
            require(string(payload, "schema_version")? == "rsia.alpha_claim_reservation.v1")?;
        }
        _ => return Err(Error::ClockRegressed),
    }
    Ok(())
}

fn qualify_development(
    object: &OccurrenceObject,
    snapshot: &OccurrenceSnapshot,
    floor: &mut OccurrenceFloor,
) -> Result<()> {
    let kind = string(&object.body, "record_kind")?;
    let payload = envelope(object, "rsia.typed_artifact_envelope.v1", kind)?;
    let field = match kind {
        "development_execution_receipt_v1" | "development_grader_receipt_v1" => "receipt_id",
        _ => "id",
    };
    let id = string(payload, field)?;
    require(object.id == development_storage_id(kind, id)?)?;
    match kind {
        "development_control_v1" => {
            development_control(snapshot, &object.namespace, id)?;
            observe_seconds_field(floor, payload, "created_at_unix_seconds")?;
        }
        "development_execution_receipt_v1" => {
            let receipt: MirrorDevelopmentExecutionReceiptV1 = decode(payload)?;
            require(
                receipt.schema_version == "rsia.development_execution_receipt.v1"
                    && receipt.namespace == object.namespace
                    && receipt.executor_actor == object.owner
                    && matches!(receipt.executor_role, Role::Host | Role::Worker),
            )?;
            let control = development_control(snapshot, &object.namespace, &receipt.control_id)?;
            require(
                receipt.executor_actor == control.executor_actor
                    && receipt.environment_digest == control.environment_digest,
            )?;
            let task = control
                .task(&receipt.task_id)
                .map_err(|_| Error::ClockRegressed)?;
            require(task.input_digest == receipt.input_digest)?;
            let call = qualified_call(
                snapshot,
                &object.namespace,
                &control.billing_scope,
                &receipt.budget_call_id,
            )?;
            require(
                call.dispatch_id.as_deref() == Some(receipt.dispatch_id.as_str())
                    && call.usage_record_id.as_deref() == Some(receipt.usage_record_id.as_str())
                    && call.output_digest.as_deref() == Some(receipt.output_digest.as_str())
                    && call.actual_input_digest == receipt.request_digest,
            )?;
            let output = snapshot.object(
                &object.namespace,
                "artifact",
                &development_storage_id(
                    "development_execution_output_v1",
                    &receipt.output_artifact_id,
                )?,
            )?;
            let output: MirrorDevelopmentExecutionOutputV1 = decode(envelope(
                output,
                "rsia.typed_artifact_envelope.v1",
                "development_execution_output_v1",
            )?)?;
            require(
                output.output_digest == receipt.output_digest
                    && hash(output.output_utf8.as_bytes()) == output.output_digest,
            )?;
            validate_development_execution(&receipt, &control, task, call, &output)?;
            floor.observe_seconds(receipt.issued_at_unix_seconds)?;
        }
        "development_grader_receipt_v1" => {
            let receipt: MirrorDevelopmentGraderReceiptV1 = decode(payload)?;
            require(
                receipt.schema_version == "rsia.development_grader_receipt.v1"
                    && receipt.namespace == object.namespace
                    && receipt.grader_actor == object.owner
                    && receipt.parent_score_micros <= 1_000_000
                    && receipt.candidate_score_micros <= 1_000_000,
            )?;
            let control = development_control(snapshot, &object.namespace, &receipt.control_id)?;
            require(
                receipt.grader_actor == control.grader_actor
                    && receipt.oracle_digest == control.oracle_digest
                    && receipt.grader_digest == control.grader_digest,
            )?;
            for (id, digest) in [
                (
                    &receipt.parent_execution_receipt_id,
                    &receipt.parent_output_digest,
                ),
                (
                    &receipt.candidate_execution_receipt_id,
                    &receipt.candidate_output_digest,
                ),
            ] {
                let source = snapshot.object(
                    &object.namespace,
                    "artifact",
                    &development_storage_id("development_execution_receipt_v1", id)?,
                )?;
                let execution: MirrorDevelopmentExecutionReceiptV1 = decode(envelope(
                    source,
                    "rsia.typed_artifact_envelope.v1",
                    "development_execution_receipt_v1",
                )?)?;
                require(
                    execution.receipt_id == *id
                        && execution.control_id == control.id
                        && execution.task_id == receipt.task_id
                        && execution.output_digest == *digest,
                )?;
            }
            validate_development_grader(&receipt, &control, snapshot, &object.namespace)?;
            floor.observe_seconds(receipt.issued_at_unix_seconds)?;
        }
        "development_run_receipt_v1" => {
            let receipt: MirrorDevelopmentRunReceiptV1 = decode(payload)?;
            require(
                receipt.schema_version == "rsia.development_run_receipt.v1"
                    && receipt.namespace == object.namespace
                    && receipt.executor_actor == object.owner
                    && receipt.id == run_receipt_id(&receipt.request_id)?,
            )?;
            let control = development_control(snapshot, &object.namespace, &receipt.control_id)?;
            require(receipt.executor_actor == control.executor_actor)?;
            for id in &receipt.execution_receipt_ids {
                snapshot.object(
                    &object.namespace,
                    "artifact",
                    &development_storage_id("development_execution_receipt_v1", id)?,
                )?;
            }
            for id in &receipt.grader_receipt_ids {
                snapshot.object(
                    &object.namespace,
                    "artifact",
                    &development_storage_id("development_grader_receipt_v1", id)?,
                )?;
            }
            for id in &receipt.budget_call_ids {
                qualified_call(snapshot, &object.namespace, &control.billing_scope, id)?;
            }
            floor.observe_seconds(receipt.issued_at_unix_seconds)?;
        }
        "development_execution_output_v1" => {
            let output: MirrorDevelopmentExecutionOutputV1 = decode(payload)?;
            require(
                output.schema_version == "rsia.development_execution_output.v1"
                    && hash(output.output_utf8.as_bytes()) == output.output_digest,
            )?;
        }
        _ => return Err(Error::ClockRegressed),
    }
    Ok(())
}

fn qualify_service_run(
    object: &OccurrenceObject,
    snapshot: &OccurrenceSnapshot,
    inputs: &PreparePreimages,
    floor: &mut OccurrenceFloor,
) -> Result<()> {
    exact(
        &object.body,
        &[
            "id",
            "owner",
            "goal",
            "snapshot_id",
            "improver_version",
            "capabilities",
            "status",
            "source",
            "created_at",
        ],
        &[],
    )?;
    let run: evo_core::Run = decode(&object.body)?;
    require(
        object.kind == "artifact"
            && run.id == object.id
            && run.owner == object.owner
            && run.source == "trusted_host_snapshot"
            && run.snapshot_id == format!("run-snapshot-{}", run.id),
    )?;
    let source = snapshot.object(&object.namespace, "artifact", &run.snapshot_id)?;
    let frozen: MirrorRunApplicationSnapshot = decode(&source.body)?;
    validate_run_snapshot(source, snapshot, &frozen)?;
    require(
        frozen.run_id == run.id
            && frozen.request_material.task_input_digest == hash(run.goal.as_bytes()),
    )?;
    let mut cache_seen = false;
    for cache in &snapshot.caches {
        if cache.namespace != object.namespace
            || cache.subject_id != run.id
            || cache.operation != "service.prepare"
        {
            continue;
        }
        cache_seen = true;
        require(
            !cache.redacted
                && cache.actor == run.owner
                && run.id
                    == stable_id(
                        "run",
                        &[
                            &object.namespace,
                            &cache.actor,
                            "prepare",
                            &cache.request_key,
                        ],
                    ),
        )?;
        let request = evo_core::Prepare {
            request_key: cache.request_key.clone(),
            goal: run.goal.clone(),
            capabilities: run.capabilities.clone(),
        };
        request.validate().map_err(|_| Error::ClockRegressed)?;
        let mut recovered = false;
        for config in &inputs.candidates {
            if !prepare_config_matches(config, &frozen)? {
                continue;
            }
            let operation = PrepareOperation {
                request: &request,
                config,
            };
            if typed_fingerprint(&operation)? == cache.payload_hash {
                recovered = true;
            }
        }
        // Each existing cache binds its own complete original operation. A second
        // healthy cache or current request cannot conceal a corrupt cache row.
        require(recovered)?;
    }
    if !cache_seen {
        // Only the current Session's actual typed request can close the interval
        // between putting its Run and putting its cache. It carries no authority.
        let mut pending = false;
        for request in &inputs.requests {
            request.validate().map_err(|_| Error::ClockRegressed)?;
            if request.goal != run.goal
                || request.capabilities != run.capabilities
                || stable_id(
                    "run",
                    &[
                        &object.namespace,
                        &run.owner,
                        "prepare",
                        &request.request_key,
                    ],
                ) != run.id
            {
                continue;
            }
            for config in &inputs.candidates {
                if prepare_config_matches(config, &frozen)? {
                    // Reconstruct the real serializer preimage, not a response or
                    // truncated capabilities fingerprint standing in for one.
                    let _ = typed_fingerprint(&PrepareOperation { request, config })?;
                    pending = true;
                }
            }
        }
        require(pending)?;
    }
    floor.observe_seconds(run.created_at)
}
fn prepare_config_matches(
    config: &HostPrepareConfig,
    frozen: &MirrorRunApplicationSnapshot,
) -> Result<bool> {
    config.validate().map_err(|_| Error::ClockRegressed)?;
    Ok(config.profile_id == frozen.profile_id
        && config.host_surface_id == frozen.host_surface_id
        && config
            .system_snapshot
            .digest()
            .map_err(|_| Error::ClockRegressed)?
            == frozen.environment_digest
        && typed_fingerprint(&config.host_capabilities)? == frozen.host_capabilities_digest
        && config.evolution_enabled == frozen.evolution_enabled
        && config.capability_level == frozen.capability_level)
}
fn validate_run_snapshot(
    object: &OccurrenceObject,
    snapshot: &OccurrenceSnapshot,
    frozen: &MirrorRunApplicationSnapshot,
) -> Result<()> {
    require(
        frozen.schema_version == "rsia.run_application.v1"
            && frozen.id == object.id
            && frozen.id == format!("run-snapshot-{}", frozen.run_id)
            && frozen.profile_id == frozen.system_snapshot.profile_id
            && frozen.environment_digest
                == frozen
                    .system_snapshot
                    .digest()
                    .map_err(|_| Error::ClockRegressed)?
            && frozen.request_digest == typed_fingerprint(&frozen.request_material)?
            && frozen.request_material.instructions == frozen.projection.instructions
            && frozen.request_material.tools == frozen.projection.tools
            && frozen.request_material.mandatory_context_digest
                == frozen.system_snapshot.mandatory_context_digest,
    )?;
    snapshot.witness(object, "host")?;
    let surface = snapshot.object(&object.namespace, "artifact", &frozen.host_surface_id)?;
    let record: MirrorHostSurfaceRecord = decode(&surface.body)?;
    require(
        record.schema_version == "rsia.host_surface_record.v1"
            && record.id == surface.id
            && record.manifest_digest == typed_fingerprint(&record.manifest)?
            && record.manifest_digest == frozen.host_surface_digest
            && record.manifest.host == frozen.system_snapshot.host_id
            && record.manifest.host_version == frozen.system_snapshot.host_version,
    )?;
    record
        .manifest
        .validate_against_extraction(&record.extracted)
        .map_err(|_| Error::ClockRegressed)?;
    snapshot.witness(surface, "admin")?;
    if let Some(release) = &frozen.release_id {
        snapshot.object(&object.namespace, "release", release)?;
    }
    Ok(())
}
fn qualify_candidate(
    object: &OccurrenceObject,
    snapshot: &OccurrenceSnapshot,
    floor: &mut OccurrenceFloor,
) -> Result<()> {
    exact(
        &object.body,
        &[
            "id",
            "owner",
            "proposal",
            "digest",
            "state",
            "improver_version",
            "evaluation_id",
            "approved_by",
            "created_at",
        ],
        &[],
    )?;
    let candidate: evo_core::Candidate = decode(&object.body)?;
    evo_core::Validate::validate(&candidate.proposal).map_err(|_| Error::ClockRegressed)?;
    require(
        object.kind == "candidate"
            && candidate.id == object.id
            && candidate.owner == object.owner
            && candidate.id
                == stable_id(
                    "candidate",
                    &[
                        &object.namespace,
                        &object.owner,
                        &candidate.proposal.request_key,
                    ],
                )
            && candidate.digest == typed_fingerprint(&candidate.proposal)?,
    )?;
    let run = snapshot.object(&object.namespace, "artifact", &candidate.proposal.run_id)?;
    require(string(&run.body, "source")? == "trusted_host_snapshot" && run.owner == object.owner)?;
    floor.observe_seconds(candidate.created_at)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "operation")]
enum MirrorParsedRequest {
    ExperimentRegister(Box<MirrorExperimentRegisterRequest>),
    EvaluationStart(MirrorEvaluationStartRequest),
    #[serde(rename = "evaluation_start_v2")]
    EvaluationStartV2(MirrorEvaluationStartRequestV2),
    EvaluationStatus(MirrorEvaluationStatusRequest),
    ReplayRun(Box<MirrorReplayRunRequest>),
    CurriculumStep(MirrorCurriculumStepRequest),
    ExplorationStart(Box<MirrorExplorationStartRequest>),
    Blocked(MirrorBlockedManagementRequest),
}
fn management_request(
    value: &Value,
    operation: &str,
    request_key: &str,
) -> Result<MirrorParsedRequest> {
    let request: MirrorParsedRequest = decode(value)?;
    let (actual_operation, schema, key) = match &request {
        MirrorParsedRequest::ExperimentRegister(v) => {
            ("experiment.register", &v.schema_version, &v.request_key)
        }
        MirrorParsedRequest::EvaluationStart(v) => {
            ("evaluation.start", &v.schema_version, &v.request_key)
        }
        MirrorParsedRequest::EvaluationStartV2(v) => {
            ("evaluation.start", &v.schema_version, &v.request_key)
        }
        MirrorParsedRequest::EvaluationStatus(v) => {
            ("evaluation.status", &v.schema_version, &v.request_key)
        }
        MirrorParsedRequest::ReplayRun(v) => ("replay.run", &v.schema_version, &v.request_key),
        MirrorParsedRequest::CurriculumStep(v) => {
            ("curriculum.step", &v.schema_version, &v.request_key)
        }
        MirrorParsedRequest::ExplorationStart(v) => {
            ("exploration.start", &v.schema_version, &v.request_key)
        }
        MirrorParsedRequest::Blocked(v) => ("meta.start", &v.schema_version, &v.request_key),
    };
    let version = if matches!(&request, MirrorParsedRequest::EvaluationStartV2(_)) {
        "v2"
    } else {
        "v1"
    };
    require(
        operation == actual_operation
            && key == request_key
            && *schema == format!("rsia.management.{}.{version}", operation.replace('.', "_")),
    )?;
    Ok(request)
}
fn management_id(prefix: &str, namespace: &str, actor: &str, operation: &str, key: &str) -> String {
    stable_id(prefix, &[namespace, actor, operation, key])
}
fn qualify_management(
    object: &OccurrenceObject,
    snapshot: &OccurrenceSnapshot,
    floor: &mut OccurrenceFloor,
) -> Result<()> {
    let job: MirrorManagementJob = decode(&object.body)?;
    require(
        object.kind == "job"
            && job.schema_version == "rsia.management_job.v1"
            && job.id == object.id
            && job.owner_actor == object.owner
            && job.id
                == management_id(
                    "management-job",
                    &object.namespace,
                    &job.owner_actor,
                    &job.operation,
                    &job.request_key,
                )
            && job.private_input_ref
                == management_id(
                    "management-input",
                    &object.namespace,
                    &job.owner_actor,
                    &job.operation,
                    &job.request_key,
                ),
    )?;
    let private = snapshot.object(&object.namespace, "artifact", &job.private_input_ref)?;
    let input: MirrorPrivateManagementInput = decode(&private.body)?;
    require(
        input.schema_version == "rsia.management_private_input.v1"
            && input.id == private.id
            && private.owner == job.owner_actor
            && input.operation == job.operation
            && input.payload_digest == job.payload_digest,
    )?;
    let request = management_request(&input.payload, &job.operation, &job.request_key)?;
    require(typed_fingerprint(&request)? == job.payload_digest)?;
    match job.operation.as_str() {
        "experiment.register" | "evaluation.start" | "evaluation.status" => {
            require(job.owner_role == Role::Evaluator)?
        }
        "exploration.start" | "replay.run" | "curriculum.step" | "meta.start" => {
            require(job.owner_role == Role::Admin)?
        }
        _ => return Err(Error::ClockRegressed),
    }
    floor.observe_seconds(job.created_at)?;
    for diagnostic in &job.diagnostics {
        identifier(&diagnostic.code).map_err(|_| Error::ClockRegressed)?;
        floor.observe_seconds(diagnostic.recorded_at)?;
    }
    Ok(())
}
fn qualify_practice(
    object: &OccurrenceObject,
    snapshot: &OccurrenceSnapshot,
    floor: &mut OccurrenceFloor,
) -> Result<()> {
    require(object.kind == "artifact" && string(&object.body, "id")? == object.id)?;
    match string(&object.body, "schema_version")? {
        "rsia.practice_registration.v1" => {
            let registration: MirrorPracticeRegistrationV1 = decode(&object.body)?;
            registration.validate().map_err(|_| Error::ClockRegressed)?;
            require(registration.registered_by == object.owner)?;
            floor.observe_seconds(registration.created_at)?;
        }
        "rsia.practice_attempt_set.v1" => {
            let set: MirrorPracticeAttemptSetV1 = decode(&object.body)?;
            set.validate().map_err(|_| Error::ClockRegressed)?;
            require(set.namespace == object.namespace)?;
            let control = development_control(snapshot, &object.namespace, &set.control_id)?;
            require(
                object.owner == control.executor_actor
                    && set.environment_digest == control.environment_digest,
            )?;
            if let Some(id) = &set.registration_id {
                let source = snapshot.object(&object.namespace, "artifact", id)?;
                let registration: MirrorPracticeRegistrationV1 = decode(&source.body)?;
                registration.validate().map_err(|_| Error::ClockRegressed)?;
                registration
                    .binds_plan(&set.plan)
                    .map_err(|_| Error::ClockRegressed)?;
            }
            for attempt in &set.attempts {
                let source = snapshot.object(
                    &object.namespace,
                    "artifact",
                    &development_storage_id(
                        "development_run_receipt_v1",
                        &attempt.execution_receipt_id,
                    )?,
                )?;
                let receipt: MirrorDevelopmentRunReceiptV1 = decode(envelope(
                    source,
                    "rsia.typed_artifact_envelope.v1",
                    "development_run_receipt_v1",
                )?)?;
                require(
                    receipt.control_id == control.id && receipt.request_id == attempt.request_id,
                )?;
            }
            floor.observe_seconds(set.created_at)?;
        }
        _ => return Err(Error::ClockRegressed),
    }
    Ok(())
}
fn qualify_meta(
    object: &OccurrenceObject,
    snapshot: &OccurrenceSnapshot,
    floor: &mut OccurrenceFloor,
) -> Result<()> {
    let payload = envelope(
        object,
        "rsia.exploration_artifact_envelope.v1",
        "meta_trial_v1",
    )?;
    let trial: MirrorMetaTrialV1 = decode(payload)?;
    require(
        trial.schema_version == "rsia.meta_trial.v1"
            && !trial.trial_id.is_empty()
            && trial.trial_id.len() <= 64
            && object.id
                == format!(
                    "e09-{}",
                    typed_fingerprint(&("meta_trial_v1", &trial.trial_id))?
                )
            && trial.old_world_id == format!("{}-old", trial.trial_id)
            && trial.new_world_id == format!("{}-new", trial.trial_id),
    )?;
    identifier(&trial.trial_id).map_err(|_| Error::ClockRegressed)?;
    trial
        .old_content
        .validate()
        .map_err(|_| Error::ClockRegressed)?;
    trial
        .new_content
        .validate()
        .map_err(|_| Error::ClockRegressed)?;
    require(
        trial.old_content_digest
            == trial
                .old_content
                .content_digest()
                .map_err(|_| Error::ClockRegressed)?
            && trial.old_content_digest
                == ImproverContentV2::builtin_default()
                    .content_digest()
                    .map_err(|_| Error::ClockRegressed)?
            && trial.new_content_digest
                == trial
                    .new_content
                    .content_digest()
                    .map_err(|_| Error::ClockRegressed)?
            && trial.new_content_digest != trial.old_content_digest
            && trial.old_policy_digest
                == trial
                    .old_content
                    .exploration_policy()
                    .digest()
                    .map_err(|_| Error::ClockRegressed)?
            && trial.new_policy_digest
                == trial
                    .new_content
                    .exploration_policy()
                    .digest()
                    .map_err(|_| Error::ClockRegressed)?
            && !trial.source_closure.is_empty(),
    )?;
    let mut sources = BTreeSet::new();
    for source in &trial.source_closure {
        identifier(source).map_err(|_| Error::ClockRegressed)?;
        require(sources.insert(source))?;
        snapshot.object(&object.namespace, "run", source)?;
    }
    require(
        snapshot
            .roots
            .get(&trial.billing_scope)
            .is_some_and(|root| root.allowed_namespaces.contains(&object.namespace)),
    )?;
    digest(&trial.s0_template_digest)?;
    for (id, policy) in [
        (&trial.old_world_id, &trial.old_policy_digest),
        (&trial.new_world_id, &trial.new_policy_digest),
    ] {
        let physical = format!("e09-{}", typed_fingerprint(&("exploration_world_v1", id))?);
        let row = snapshot.object(&object.namespace, "artifact", &physical)?;
        let world: MirrorExplorationWorldV1 = decode(envelope(
            row,
            "rsia.exploration_artifact_envelope.v1",
            "exploration_world_v1",
        )?)?;
        world.validate().map_err(|_| Error::ClockRegressed)?;
        let limits = &trial.declared_stream_limits;
        require(
            world.id == *id
                && world.source_watermark == trial.revoke_watermark
                && world.policy.digest().map_err(|_| Error::ClockRegressed)? == *policy
                && world.dependencies.len() == trial.source_closure.len()
                && world
                    .dependencies
                    .iter()
                    .zip(&trial.source_closure)
                    .all(|(dependency, run)| dependency.kind == "run" && dependency.id == *run)
                && typed_fingerprint(&world.root_opportunities)?
                    == typed_fingerprint(&limits.root_opportunities)?
                && world.successor_cost_upper_micros == limits.successor_cost_upper_micros
                && meta_s0_digest(&world, limits)? == trial.s0_template_digest,
        )?;
        if world.node_ids.is_empty() && world.dispatch_ids.is_empty() {
            require(
                world.remaining_root_micros == limits.remaining_root_micros
                    && world.remaining_recovery_dispatches == limits.remaining_recovery_dispatches
                    && world.decision_round == 0
                    && world.current_branch_seq.is_none()
                    && world.current_branch_focus_actions == 0
                    && world.waits.is_empty(),
            )?;
        } else {
            require(
                world.remaining_root_micros <= limits.remaining_root_micros
                    && world.remaining_recovery_dispatches <= limits.remaining_recovery_dispatches,
            )?;
        }
    }
    floor.observe_seconds(trial.created_at)
}

fn qualify_tombstone(
    object: &OccurrenceObject,
    snapshot: &OccurrenceSnapshot,
    floor: &mut OccurrenceFloor,
) -> Result<()> {
    let tombstone: crate::lifecycle::RevokeTombstone = decode(&object.body)?;
    require(
        object.kind == "tombstone"
            && tombstone.schema_version == "rsia.revoke_tombstone.v1"
            && tombstone.id == object.id
            && tombstone.watermark_seq > 0,
    )?;
    identifier(&tombstone.source_kind).map_err(|_| Error::ClockRegressed)?;
    require(
        tombstone.watermark_digest
            == typed_fingerprint(&(
                "rsia.revoke_watermark.v1",
                &object.namespace,
                &tombstone.source_kind,
                &tombstone.id,
                &tombstone.reason,
                tombstone.created_at,
            ))?,
    )?;
    let (sequence, digest) = snapshot
        .watermarks
        .get(&object.namespace)
        .ok_or(Error::ClockRegressed)?;
    require(
        *sequence >= tombstone.watermark_seq
            && (*sequence != tombstone.watermark_seq || *digest == tombstone.watermark_digest),
    )?;
    floor.observe_seconds(tombstone.created_at)
}
fn load_dependencies(conn: &RawDatabase, snapshot: &mut OccurrenceSnapshot) -> Result<()> {
    for row in rows(conn, "revoke_watermark", &["namespace", "seq", "digest"])? {
        let namespace = sql_string(&row, "namespace")?;
        identifier(&namespace).map_err(|_| Error::ClockRegressed)?;
        let sequence = sql_integer(&row, "seq")?;
        require(sequence >= 0)?;
        let digest_value = sql_string(&row, "digest")?;
        digest(&digest_value)?;
        require(
            snapshot
                .watermarks
                .insert(
                    namespace,
                    (
                        u64::try_from(sequence).map_err(|_| Error::ClockRegressed)?,
                        digest_value,
                    ),
                )
                .is_none(),
        )?;
    }
    for row in rows(
        conn,
        "dependencies",
        &["namespace", "src_kind", "src_id", "dst_kind", "dst_id"],
    )? {
        let namespace = sql_string(&row, "namespace")?;
        let source_kind = sql_string(&row, "src_kind")?;
        let source = sql_string(&row, "src_id")?;
        let target_kind = sql_string(&row, "dst_kind")?;
        let target = sql_string(&row, "dst_id")?;
        for id in [&namespace, &source_kind, &source, &target_kind, &target] {
            identifier(id).map_err(|_| Error::ClockRegressed)?;
        }
        require(
            snapshot
                .edges
                .insert((namespace, source_kind, source, target_kind, target)),
        )?;
    }
    Ok(())
}

fn e16_kind(schema: &str) -> Option<&'static str> {
    match schema {
        "rsia.e16.source_selection.v1" => Some("source_selection"),
        "rsia.e16.import_source.v1" => Some("import_source"),
        "rsia.e16.import_result.v1" => Some("import_result"),
        "rsia.e16.staged_asset.v1" => Some("staged_asset"),
        "rsia.e16.seed_install.v1" => Some("seed_install"),
        "rsia.e16.export_attempt.v1" | "rsia.e16.export_attempt.v2" => Some("export_attempt"),
        "rsia.e16.delivery_audit.v1" | "rsia.e16.delivery_audit.v2" => Some("delivery_audit"),
        _ => None,
    }
}
fn e16_base(object: &OccurrenceObject, floor: &mut OccurrenceFloor) -> Result<()> {
    let value = &object.body;
    exact(
        value,
        &[
            "schema_version",
            "id",
            "namespace",
            "owner_actor",
            "request_key",
            "input_digest",
            "created_at",
            "updated_at",
            "source_refs",
            "revoke_watermark",
            "payload",
        ],
        &[],
    )?;
    let kind = e16_kind(string(value, "schema_version")?).ok_or(Error::ClockRegressed)?;
    require(
        object.kind == "artifact"
            && string(value, "id")? == object.id
            && string(value, "namespace")? == object.namespace
            && string(value, "owner_actor")? == object.owner,
    )?;
    identifier(string(value, "request_key")?).map_err(|_| Error::ClockRegressed)?;
    digest(string(value, "input_digest")?)?;
    require(value["payload"].is_object() && value["revoke_watermark"].as_u64().is_some())?;
    let digest_field = if matches!(kind, "source_selection" | "import_source" | "import_result") {
        "content_digest"
    } else {
        "digest"
    };
    let mut seen = BTreeSet::new();
    for source in array(value, "source_refs")? {
        exact(source, &["kind", "id", digest_field], &[])?;
        let source_kind = string(source, "kind")?;
        let id = string(source, "id")?;
        identifier(source_kind).map_err(|_| Error::ClockRegressed)?;
        identifier(id).map_err(|_| Error::ClockRegressed)?;
        digest(string(source, digest_field)?)?;
        require(seen.insert((source_kind, id)))?;
    }
    let created = nonnegative(value, "created_at")?;
    let updated = nonnegative(value, "updated_at")?;
    require(updated >= created)?;
    if matches!(kind, "source_selection" | "import_source" | "import_result") {
        require(created > 0)?;
    }
    floor.observe_seconds(created)?;
    floor.observe_seconds(updated)?;
    Ok(())
}
fn validate_import_source(record: &MirrorImportEnvelope<MirrorImportSourcePayload>) -> Result<()> {
    let body = &record.payload;
    let reader_label = match body.reader {
        MirrorSourceFormat::RsiaTraceV1 => "rsia.trace.v1",
        MirrorSourceFormat::RsihPiFixture => "rsih.pi.fixture",
        MirrorSourceFormat::ClaudeFixture => "claude.fixture",
    };
    require(body.reader_version == reader_label)?;

    require(
        record.schema_version == "rsia.e16.import_source.v1"
            && body.task_origin == TaskOrigin::ImportedHistory
            && body.execution_attestation == ExecutionAttestation::UnverifiedImport
            && body.raw_blob_digest == body.raw_digest
            && record.source_refs.len() == 1,
    )?;
    digest(&body.raw_digest)?;
    digest(&body.raw_blob_digest)?;
    if let Some(observed) = &body.observed_digest {
        digest(observed)?;
    }
    let source = &record.source_refs[0];
    require(
        source.kind == "blob"
            && source.id == body.raw_blob_digest
            && source.content_digest == body.raw_digest,
    )?;
    match body.status {
        MirrorImportSourceReadStatus::Prepared => {
            require(!body.blob_published && body.observed_digest.is_none() && body.byte_len == 0)?
        }
        MirrorImportSourceReadStatus::Ready => require(
            body.observed_digest.as_deref() == Some(body.raw_digest.as_str())
                && body.byte_len > 0
                && body.byte_len <= evo_core::evidence::MAX_TOTAL_READ_BYTES as u64
                && body.blob_published,
        )?,
        MirrorImportSourceReadStatus::SourceChanged => require(
            !body.blob_published
                && body.observed_digest.as_deref() != Some(body.raw_digest.as_str()),
        )?,
        MirrorImportSourceReadStatus::ZeroRecords => require(
            !body.blob_published
                && body.byte_len == 0
                && body.observed_digest.as_deref() == Some(hash(b"").as_str()),
        )?,
        _ => require(!body.blob_published)?,
    }
    Ok(())
}
fn selection_request(
    record: &MirrorImportEnvelope<MirrorSourceSelectionPayload>,
    snapshot: &OccurrenceSnapshot,
) -> Result<MirrorImportRegistrationRequest> {
    require(
        record.schema_version == "rsia.e16.source_selection.v1"
            && !record.payload.import_source_ids.is_empty()
            && record.payload.import_source_ids.len() == record.source_refs.len()
            && record.id == format!("e16sel-{}", &record.input_digest[..24]),
    )?;
    let mut sources = Vec::new();
    let mut ids = BTreeSet::new();
    for (id, source_ref) in record
        .payload
        .import_source_ids
        .iter()
        .zip(&record.source_refs)
    {
        require(source_ref.kind == "import_source" && source_ref.id == *id && ids.insert(id))?;
        let source = snapshot.object(&record.namespace, "artifact", id)?;
        let source: MirrorImportEnvelope<MirrorImportSourcePayload> = decode(&source.body)?;
        validate_import_source(&source)?;
        require(
            source.namespace == record.namespace
                && source.owner_actor == record.owner_actor
                && source.payload.selection_id == record.id
                && source.request_key == record.request_key
                && source.payload.purpose == record.payload.purpose,
        )?;
        if typed_fingerprint(&source)? != source_ref.content_digest {
            // register commits Prepared source refs, then finalizes each source
            // in a separate real transaction before the final Selection/cache.
            // Only its exact initial Prepared preimage can explain this interval.
            require(
                record.updated_at == record.created_at
                    && !snapshot.caches.iter().any(|cache| {
                        cache.namespace == record.namespace
                            && cache.operation == "e16.import.register"
                            && cache.subject_id == record.id
                    }),
            )?;
            let mut prepared = source.clone();
            prepared.updated_at = prepared.created_at;
            prepared.payload.status = MirrorImportSourceReadStatus::Prepared;
            prepared.payload.observed_digest = None;
            prepared.payload.blob_published = false;
            prepared.payload.byte_len = 0;
            validate_import_source(&prepared)?;
            require(typed_fingerprint(&prepared)? == source_ref.content_digest)?;
        }
        let expected = format!(
            "e16src-{}",
            &hash(format!("{}\n{}", record.id, source.payload.logical_source_id).as_bytes())[..24]
        );
        require(source.id == expected)?;
        sources.push(MirrorImportSourceSpec {
            source_id: source.payload.logical_source_id.clone(),
            path: source.payload.authorized_path.clone(),
            reader: source.payload.reader,
            expected_digest: source.payload.raw_digest.clone(),
        });
    }
    let request = MirrorImportRegistrationRequest {
        schema_version: "rsia.e16.import_registration.v1".into(),
        request_key: record.request_key.clone(),
        roots: record.payload.roots.clone(),
        purpose: record.payload.purpose,
        allow_model_excerpts: record.payload.allow_model_excerpts,
        outbound_authorized: record.payload.outbound_authorized,
        retention_scope: record.payload.retention_scope,
        sources,
    };
    require(
        !request.sources.is_empty()
            && request.sources.len() <= evo_core::evidence::MAX_DISCOVERED_FILES,
    )?;
    require(
        !request.allow_model_excerpts
            || (request.outbound_authorized
                && request.retention_scope
                    == MirrorImportRetentionScope::LocalWithAuthorizedExcerpts),
    )?;
    let selection = evo_core::evidence::SourceSelection {
        roots: request.roots.clone(),
        run_ids: request
            .sources
            .iter()
            .map(|source| source.source_id.clone())
            .collect(),
        purpose: request.purpose,
        allow_model_excerpts: request.allow_model_excerpts,
    };
    selection.validate().map_err(|_| Error::ClockRegressed)?;
    for source in &request.sources {
        evo_core::evidence::assert_authorized_path(&source.path, &request.roots)
            .map_err(|_| Error::ClockRegressed)?;
    }
    require(typed_fingerprint(&request)? == record.input_digest)?;
    Ok(request)
}
fn qualify_e16(
    object: &OccurrenceObject,
    snapshot: &OccurrenceSnapshot,
    inputs: &PreparePreimages,
    floor: &mut OccurrenceFloor,
) -> Result<()> {
    e16_base(object, floor)?;
    let schema = string(&object.body, "schema_version")?;
    let namespace = &object.namespace;
    let owner = &object.owner;
    let key = string(&object.body, "request_key")?;
    let payload = &object.body["payload"];
    match schema {
        "rsia.e16.source_selection.v1" => {
            let record: MirrorImportEnvelope<MirrorSourceSelectionPayload> = decode(&object.body)?;
            selection_request(&record, snapshot)?;
        }
        "rsia.e16.import_source.v1" => {
            let record: MirrorImportEnvelope<MirrorImportSourcePayload> = decode(&object.body)?;
            validate_import_source(&record)?;
            require(
                record.id
                    == format!(
                        "e16src-{}",
                        &hash(
                            format!(
                                "{}\n{}",
                                record.payload.selection_id, record.payload.logical_source_id
                            )
                            .as_bytes()
                        )[..24]
                    ),
            )?;
            let spec = MirrorImportSourceSpec {
                source_id: record.payload.logical_source_id.clone(),
                path: record.payload.authorized_path.clone(),
                reader: record.payload.reader,
                expected_digest: record.payload.raw_digest.clone(),
            };
            require(
                record.input_digest == typed_fingerprint(&(&spec, spec.expected_digest.as_str()))?,
            )?;
            if let Some(selection) = snapshot.objects.get(&(
                record.namespace.clone(),
                "artifact".into(),
                record.payload.selection_id.clone(),
            )) {
                let selection: MirrorImportEnvelope<MirrorSourceSelectionPayload> =
                    decode(&selection.body)?;
                require(
                    selection.schema_version == "rsia.e16.source_selection.v1"
                        && selection.namespace == record.namespace
                        && selection.owner_actor == record.owner_actor
                        && selection.request_key == record.request_key
                        && selection.revoke_watermark == record.revoke_watermark
                        && selection.payload.purpose == record.payload.purpose
                        && selection
                            .payload
                            .import_source_ids
                            .iter()
                            .any(|id| id == &record.id),
                )?;
            } else {
                require(pending_import_source(&record, snapshot, inputs)?)?;
            }
        }
        "rsia.e16.import_result.v1" => {
            let record: MirrorImportEnvelope<MirrorImportResultPayload> = decode(&object.body)?;
            require(
                record.id
                    == format!(
                        "e16res-{}",
                        &hash(record.payload.selection_id.as_bytes())[..24]
                    )
                    && record.payload.generation_status
                        == "blocked_external_generation_conditions_unavailable"
                    && (record.payload.state == MirrorImportResultState::Failed)
                        == record.payload.evidence_set.is_none()
                    && record.input_digest == typed_fingerprint(&record.source_refs)?,
            )?;
            if let Some(set) = &record.payload.evidence_set {
                require(set.members.iter().all(|member| {
                    member.task_origin == TaskOrigin::ImportedHistory
                        && member.execution_attestation == ExecutionAttestation::UnverifiedImport
                }))?;
            }
            snapshot.object(namespace, "artifact", &record.payload.selection_id)?;
        }
        "rsia.e16.staged_asset.v1" => {
            let record: MirrorE16Envelope<MirrorStagedAssetPayload> = decode(&object.body)?;
            require(
                record.id
                    == format!(
                        "e16-staged-asset-{}",
                        typed_fingerprint(&(namespace.as_str(), owner.as_str(), key))?
                    ),
            )?;
            for value in [
                &record.payload.manifest_digest,
                &record.payload.content_blob_digest,
                &record.payload.baseline_digest,
                &record.payload.local_digest,
                &record.payload.environment_digest,
            ] {
                digest(value)?;
            }
            if let Some(value) = &record.payload.upstream_digest {
                digest(value)?;
            }
            require(matches!(
                record.payload.package_kind.as_str(),
                "skill" | "bundle" | "improver" | "template"
            ))?;
        }
        "rsia.e16.export_attempt.v2" => {
            let record: MirrorE16Envelope<MirrorExportAttemptPayload> = decode(&object.body)?;
            require(
                record.id
                    == format!(
                        "e16-export-attempt-{}",
                        typed_fingerprint(&(namespace.as_str(), owner.as_str(), key))?
                    ),
            )?;
            for value in [
                &record.payload.manifest_digest,
                &record.payload.projection_blob_digest,
                &record.payload.package_tree_digest,
            ] {
                digest(value)?;
            }
            let staged = snapshot.object(namespace, "artifact", &record.payload.staged_asset_id)?;
            require(string(&staged.body, "schema_version")? == "rsia.e16.staged_asset.v1")?;
            if let Some(id) = &record.payload.delivery_audit_id {
                let delivered = snapshot.object(namespace, "artifact", id)?;
                require(string(&delivered.body["payload"], "export_attempt_id")? == record.id)?;
            }
            if let Some(budget) = &record.payload.budget_ref {
                require(
                    snapshot
                        .roots
                        .get(&budget.billing_scope)
                        .is_some_and(|root| root.root_budget_id == budget.root_budget_id),
                )?;
            }
        }
        "rsia.e16.delivery_audit.v2" => {
            let record: MirrorE16Envelope<MirrorDeliveryAuditPayload> = decode(&object.body)?;
            require(
                record.id
                    == format!(
                        "e16-delivery-{}",
                        typed_fingerprint(&record.payload.export_attempt_id)?
                    ),
            )?;
            let export =
                snapshot.object(namespace, "artifact", &record.payload.export_attempt_id)?;
            require(string(&export.body, "schema_version")? == "rsia.e16.export_attempt.v2")?;
            for value in [
                &record.payload.projection_blob_digest,
                &record.payload.package_tree_digest,
                &record.payload.completion_receipt_digest,
            ] {
                digest(value)?;
            }
            floor.observe_seconds(record.payload.delivered_at)?;
        }
        "rsia.e16.seed_install.v1" => {
            let record: MirrorE16Envelope<MirrorSeedInstallPayloadV1> = decode(&object.body)?;
            require(
                record.id
                    == format!(
                        "e16-seed-install-{}",
                        typed_fingerprint(&(
                            namespace.as_str(),
                            record.payload.publisher.as_str(),
                            record.payload.asset_id.as_str(),
                            record.payload.kind.as_str()
                        ))?
                    ),
            )?;
            for value in [
                &record.payload.baseline_blob_digest,
                &record.payload.baseline_digest,
                &record.payload.local_blob_digest,
                &record.payload.local_digest,
                &record.payload.environment_digest,
            ] {
                digest(value)?;
            }
            require(
                record.payload.baseline_blob_digest == record.payload.baseline_digest
                    && record.payload.local_blob_digest == record.payload.local_digest,
            )?;
        }
        // Their outer is known; their old payload is not silently reclassified opaque.
        "rsia.e16.export_attempt.v1" | "rsia.e16.delivery_audit.v1" => {
            return Err(Error::ClockRegressed);
        }
        _ => return Err(Error::ClockRegressed),
    }
    require(payload.is_object())
}
fn qualify_e16_redacted(object: &OccurrenceObject, floor: &mut OccurrenceFloor) -> Result<()> {
    let value = &object.body;
    exact(
        value,
        &[
            "id",
            "schema_version",
            "state",
            "original_kind",
            "original_schema",
            "original_digest",
            "metadata",
        ],
        &[],
    )?;
    require(
        string(value, "id")? == object.id && string(value, "schema_version")? == "rsia.redacted.v1",
    )?;
    let kind = e16_kind(string(value, "original_schema")?).ok_or(Error::ClockRegressed)?;
    require(string(value, "original_kind")? == kind)?;
    require(string(value, "state")? == "source_revoked")?;
    require(!matches!(
        string(value, "original_schema")?,
        "rsia.e16.export_attempt.v1" | "rsia.e16.delivery_audit.v1"
    ))?;
    digest(string(value, "original_digest")?)?;
    let metadata = &value["metadata"];
    exact(
        metadata,
        &[
            "namespace",
            "owner_actor",
            "request_key",
            "input_digest",
            "created_at",
            "updated_at",
            "source_refs",
            "revoke_watermark",
            "payload",
        ],
        &[],
    )?;
    require(
        string(metadata, "namespace")? == object.namespace
            && string(metadata, "owner_actor")? == object.owner
            && metadata["revoke_watermark"].as_u64().is_some(),
    )?;
    identifier(string(metadata, "request_key")?).map_err(|_| Error::ClockRegressed)?;
    digest(string(metadata, "input_digest")?)?;
    let digest_field = if matches!(kind, "source_selection" | "import_source" | "import_result") {
        "content_digest"
    } else {
        "digest"
    };
    for source in array(metadata, "source_refs")? {
        exact(source, &["kind", "id", digest_field], &[])?;
        identifier(string(source, "kind")?).map_err(|_| Error::ClockRegressed)?;
        identifier(string(source, "id")?).map_err(|_| Error::ClockRegressed)?;
        digest(string(source, digest_field)?)?;
    }
    validate_e16_retained_payload(kind, &metadata["payload"])?;

    let created = nonnegative(metadata, "created_at")?;
    let updated = nonnegative(metadata, "updated_at")?;
    require(updated >= created)?;
    floor.observe_seconds(created)?;
    floor.observe_seconds(updated)
}

#[derive(Clone)]
enum OwnedCell {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}
impl OwnedCell {
    fn sqlite_type(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Integer(_) => "integer",
            Self::Real(value) => {
                let _ = value.to_bits();
                "real"
            }
            Self::Text(_) => "text",
            Self::Blob(_) => "blob",
        }
    }
}
#[derive(Clone)]
struct OwnedRow {
    cells: BTreeMap<String, OwnedCell>,
}
trait FromOwnedCell: Sized {
    fn from_cell(cell: &OwnedCell) -> std::result::Result<Self, sqlx::Error>;
}
fn cell_error() -> sqlx::Error {
    sqlx::Error::Protocol("owned occurrence native cell type mismatch".into())
}
impl FromOwnedCell for i64 {
    fn from_cell(cell: &OwnedCell) -> std::result::Result<Self, sqlx::Error> {
        if let OwnedCell::Integer(value) = cell {
            Ok(*value)
        } else {
            Err(cell_error())
        }
    }
}
impl FromOwnedCell for String {
    fn from_cell(cell: &OwnedCell) -> std::result::Result<Self, sqlx::Error> {
        if let OwnedCell::Text(value) = cell {
            Ok(value.clone())
        } else {
            Err(cell_error())
        }
    }
}
impl FromOwnedCell for Vec<u8> {
    fn from_cell(cell: &OwnedCell) -> std::result::Result<Self, sqlx::Error> {
        if let OwnedCell::Blob(value) = cell {
            Ok(value.clone())
        } else {
            Err(cell_error())
        }
    }
}
impl<T: FromOwnedCell> FromOwnedCell for Option<T> {
    fn from_cell(cell: &OwnedCell) -> std::result::Result<Self, sqlx::Error> {
        if matches!(cell, OwnedCell::Null) {
            Ok(None)
        } else {
            T::from_cell(cell).map(Some)
        }
    }
}
impl OwnedRow {
    fn try_get<T: FromOwnedCell, I: AsRef<str>>(
        &self,
        column: I,
    ) -> std::result::Result<T, sqlx::Error> {
        T::from_cell(self.cells.get(column.as_ref()).ok_or_else(cell_error)?)
    }
}
#[derive(Default)]
pub(super) struct RawDatabase {
    tables: BTreeMap<String, Vec<OwnedRow>>,
}
const DATABASE_TABLES: &[&str] = &[
    "_sqlx_migrations",
    "objects",
    "audit",
    "idempotency",
    "dependencies",
    "revoke_watermark",
    "replay_worlds",
    "root_budgets",
    "root_budget_namespaces",
    "root_budget_dispatch_groups",
    "root_budget_calls",
    "root_budget_events",
    "revoke_cleanup_jobs",
    "revoke_cleanup_frontier",
    "revoke_cleanup_events",
];
fn existing_tables(schema: &[OwnedRow]) -> Result<BTreeSet<String>> {
    schema
        .iter()
        .filter_map(|row| match row.try_get::<String, _>("type") {
            Ok(kind) if kind == "table" => Some(clock(row.try_get::<String, _>("name"))),
            Ok(_) => None,
            Err(_) => Some(Err(Error::ClockRegressed)),
        })
        .collect()
}
fn sqlx_native_rows(rows: Vec<sqlx::sqlite::SqliteRow>) -> Result<Vec<OwnedRow>> {
    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let mut cells = BTreeMap::new();
        for (index, column) in row.columns().iter().enumerate() {
            let value = clock(row.try_get_raw(index))?;
            let cell = if value.is_null() {
                OwnedCell::Null
            } else {
                match value.type_info().name() {
                    "INTEGER" => OwnedCell::Integer(clock(row.try_get::<i64, _>(index))?),
                    "REAL" => OwnedCell::Real(clock(row.try_get::<f64, _>(index))?),
                    "TEXT" => OwnedCell::Text(clock(row.try_get::<String, _>(index))?),
                    "BLOB" => OwnedCell::Blob(clock(row.try_get::<Vec<u8>, _>(index))?),
                    _ => return Err(Error::ClockRegressed),
                }
            };
            require(cells.insert(column.name().to_owned(), cell).is_none())?;
        }
        result.push(OwnedRow { cells });
    }
    Ok(result)
}
async fn load_sqlx_database(conn: &mut SqliteConnection) -> Result<RawDatabase> {
    let schema = sqlx_native_rows(clock(
        sqlx::query("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY name")
            .fetch_all(&mut *conn)
            .await,
    )?)?;
    let existing = existing_tables(&schema)?;
    let mut database = RawDatabase::default();
    database.tables.insert("sqlite_schema".into(), schema);
    for table in DATABASE_TABLES {
        if existing.contains(*table) {
            let query = format!("SELECT * FROM {table}");
            let values = clock(sqlx::query(&query).fetch_all(&mut *conn).await)?;
            database
                .tables
                .insert((*table).into(), sqlx_native_rows(values)?);
        }
    }
    Ok(database)
}
fn rusqlite_native_rows(conn: &rusqlite::Connection, query: &str) -> Result<Vec<OwnedRow>> {
    let mut statement = conn.prepare(query).map_err(|_| Error::ClockRegressed)?;
    let names = statement
        .column_names()
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<Vec<_>>();
    let mut rows = statement.query([]).map_err(|_| Error::ClockRegressed)?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().map_err(|_| Error::ClockRegressed)? {
        let mut cells = BTreeMap::new();
        for (index, name) in names.iter().enumerate() {
            use rusqlite::types::ValueRef as Native;
            let cell = match row.get_ref(index).map_err(|_| Error::ClockRegressed)? {
                Native::Null => OwnedCell::Null,
                Native::Integer(value) => OwnedCell::Integer(value),
                Native::Real(value) => OwnedCell::Real(value),
                Native::Text(value) => OwnedCell::Text(
                    std::str::from_utf8(value)
                        .map_err(|_| Error::ClockRegressed)?
                        .to_owned(),
                ),
                Native::Blob(value) => OwnedCell::Blob(value.to_vec()),
            };
            require(cells.insert(name.clone(), cell).is_none())?;
        }
        result.push(OwnedRow { cells });
    }
    Ok(result)
}
pub(super) fn load_rusqlite_database(conn: &rusqlite::Connection) -> Result<RawDatabase> {
    let schema = rusqlite_native_rows(
        conn,
        "SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY name",
    )?;
    let existing = existing_tables(&schema)?;
    let mut database = RawDatabase::default();
    database.tables.insert("sqlite_schema".into(), schema);
    for table in DATABASE_TABLES {
        if existing.contains(*table) {
            database.tables.insert(
                (*table).into(),
                rusqlite_native_rows(conn, &format!("SELECT * FROM {table}"))?,
            );
        }
    }
    Ok(database)
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum WorkerCellR50 {
    Null,
    Integer(i64),
    RealBits(u64),
    Text(String),
    Blob(Vec<u8>),
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerRowsR50 {
    tables: Vec<(String, Vec<WorkerRowR50>)>,
}
type WorkerRowR50 = Vec<(String, WorkerCellR50)>;

pub(super) fn raw_worker_value_r50(connection: &rusqlite::Connection) -> Result<Value> {
    require(!connection.is_autocommit())?;
    let database = load_rusqlite_database(connection)?;
    require(!connection.is_autocommit())?;
    let wire = WorkerRowsR50 {
        tables: database
            .tables
            .into_iter()
            .map(|(name, rows)| {
                (
                    name,
                    rows.into_iter()
                        .map(|row| {
                            row.cells
                                .into_iter()
                                .map(|(name, cell)| {
                                    let cell = match cell {
                                        OwnedCell::Null => WorkerCellR50::Null,
                                        OwnedCell::Integer(value) => WorkerCellR50::Integer(value),
                                        OwnedCell::Real(value) => {
                                            WorkerCellR50::RealBits(value.to_bits())
                                        }
                                        OwnedCell::Text(value) => WorkerCellR50::Text(value),
                                        OwnedCell::Blob(value) => WorkerCellR50::Blob(value),
                                    };
                                    (name, cell)
                                })
                                .collect()
                        })
                        .collect(),
                )
            })
            .collect(),
    };
    serde_json::to_value(wire).map_err(|_| Error::Internal)
}

pub(super) fn raw_from_worker_r50(value: Value) -> Result<RawDatabase> {
    let wire: WorkerRowsR50 = serde_json::from_value(value).map_err(|_| Error::ClockRegressed)?;
    let mut database = RawDatabase::default();
    for (name, rows) in wire.tables {
        require(name == "sqlite_schema" || DATABASE_TABLES.contains(&name.as_str()))?;
        let mut output = Vec::with_capacity(rows.len());
        for row in rows {
            let mut cells = BTreeMap::new();
            for (column, cell) in row {
                let cell = match cell {
                    WorkerCellR50::Null => OwnedCell::Null,
                    WorkerCellR50::Integer(value) => OwnedCell::Integer(value),
                    WorkerCellR50::RealBits(bits) => OwnedCell::Real(f64::from_bits(bits)),
                    WorkerCellR50::Text(value) => OwnedCell::Text(value),
                    WorkerCellR50::Blob(value) => OwnedCell::Blob(value),
                };
                require(cells.insert(column, cell).is_none())?;
            }
            output.push(OwnedRow { cells });
        }
        require(database.tables.insert(name, output).is_none())?;
    }
    require(database.tables.contains_key("sqlite_schema"))?;
    Ok(database)
}

pub(super) fn collect_floor_from_raw_r50(
    database: &RawDatabase,
    inputs: &PreparePreimages,
) -> Result<OccurrenceFloor> {
    collect_raw(database, inputs, SchemaRead::Stable).map_err(|_| Error::ClockRegressed)
}

fn owned_root_from_row(row: &OwnedRow, namespaces: Vec<String>) -> Result<RootBudgetRecord> {
    Ok(RootBudgetRecord {
        root_budget_id: row
            .try_get("root_budget_id")
            .map_err(|_| Error::ClockRegressed)?,
        billing_scope: row
            .try_get("billing_scope")
            .map_err(|_| Error::ClockRegressed)?,
        authorizing_namespace: row
            .try_get("authorizing_namespace")
            .map_err(|_| Error::ClockRegressed)?,
        allowed_namespaces: namespaces,
        currency: row.try_get("currency").map_err(|_| Error::ClockRegressed)?,
        pricing_version: row
            .try_get("pricing_version")
            .map_err(|_| Error::ClockRegressed)?,
        payment_subject: row
            .try_get("payment_subject")
            .map_err(|_| Error::ClockRegressed)?,
        authorization_receipt_digest: row
            .try_get("authorization_receipt_digest")
            .map_err(|_| Error::ClockRegressed)?,
        per_call_cap_micros: row
            .try_get("per_call_cap_micros")
            .map_err(|_| Error::ClockRegressed)?,
        total_limit_micros: row
            .try_get("total_limit_micros")
            .map_err(|_| Error::ClockRegressed)?,
        spent_micros: row
            .try_get("spent_micros")
            .map_err(|_| Error::ClockRegressed)?,
        reserved_micros: row
            .try_get("reserved_micros")
            .map_err(|_| Error::ClockRegressed)?,
        stopped: row
            .try_get::<i64, _>("stopped")
            .map_err(|_| Error::ClockRegressed)?
            != 0,
        stop_reason: row
            .try_get("stop_reason")
            .map_err(|_| Error::ClockRegressed)?,
        stop_committed_at: row
            .try_get("stop_committed_at")
            .map_err(|_| Error::ClockRegressed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| Error::ClockRegressed)?,
    })
}

fn owned_call_from_row(row: &OwnedRow) -> Result<BudgetCallRecord> {
    let stage: String = row.try_get("stage").map_err(|_| Error::ClockRegressed)?;
    let state: String = row.try_get("state").map_err(|_| Error::ClockRegressed)?;
    Ok(BudgetCallRecord {
        billing_scope: row
            .try_get("billing_scope")
            .map_err(|_| Error::ClockRegressed)?,
        call_id: row.try_get("call_id").map_err(|_| Error::ClockRegressed)?,
        dispatch_group_id: row
            .try_get("dispatch_group_id")
            .map_err(|_| Error::ClockRegressed)?,
        namespace: row
            .try_get("namespace")
            .map_err(|_| Error::ClockRegressed)?,
        stage: decode(&Value::String(stage))?,
        actual_input_digest: row
            .try_get("actual_input_digest")
            .map_err(|_| Error::ClockRegressed)?,
        request_artifact: owned_artifact_from_row(row, "request")?,
        reserved_micros: row
            .try_get("reserved_micros")
            .map_err(|_| Error::ClockRegressed)?,
        state: decode(&Value::String(state))?,
        lease_token: row
            .try_get("lease_token")
            .map_err(|_| Error::ClockRegressed)?,
        lease_epoch: row
            .try_get("lease_epoch")
            .map_err(|_| Error::ClockRegressed)?,
        lease_until: row
            .try_get("lease_until")
            .map_err(|_| Error::ClockRegressed)?,
        dispatch_id: row
            .try_get("dispatch_id")
            .map_err(|_| Error::ClockRegressed)?,
        provider_request_id: row
            .try_get("provider_request_id")
            .map_err(|_| Error::ClockRegressed)?,
        usage_record_id: row
            .try_get("usage_record_id")
            .map_err(|_| Error::ClockRegressed)?,
        output_digest: row
            .try_get("output_digest")
            .map_err(|_| Error::ClockRegressed)?,
        actual_cost_micros: row
            .try_get("actual_cost_micros")
            .map_err(|_| Error::ClockRegressed)?,
        actual_currency: row
            .try_get("actual_currency")
            .map_err(|_| Error::ClockRegressed)?,
        actual_pricing_version: row
            .try_get("actual_pricing_version")
            .map_err(|_| Error::ClockRegressed)?,
        execution_provenance: row
            .try_get::<Option<String>, _>("execution_provenance")
            .map_err(|_| Error::ClockRegressed)?
            .map(|value| decode(&Value::String(value)))
            .transpose()?,
        actual_model_digest: row
            .try_get("actual_model_digest")
            .map_err(|_| Error::ClockRegressed)?,
        transport_artifact: owned_artifact_from_row(row, "transport")?,
        response_artifact: owned_artifact_from_row(row, "response")?,
        response_usable: row
            .try_get::<Option<i64>, _>("response_usable")
            .map_err(|_| Error::ClockRegressed)?
            .map(|value| value != 0),
        response_block_reason: row
            .try_get("response_block_reason")
            .map_err(|_| Error::ClockRegressed)?,
        execution_closed: row
            .try_get::<i64, _>("execution_closed")
            .map_err(|_| Error::ClockRegressed)?
            != 0,
        execution_closed_at: row
            .try_get("execution_closed_at")
            .map_err(|_| Error::ClockRegressed)?,
        execution_close_reason: row
            .try_get("execution_close_reason")
            .map_err(|_| Error::ClockRegressed)?,
        terminal_reason: row
            .try_get("terminal_reason")
            .map_err(|_| Error::ClockRegressed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| Error::ClockRegressed)?,
        dispatched_at: row
            .try_get("dispatched_at")
            .map_err(|_| Error::ClockRegressed)?,
        finalized_at: row
            .try_get("finalized_at")
            .map_err(|_| Error::ClockRegressed)?,
    })
}

fn owned_artifact_from_row(row: &OwnedRow, prefix: &str) -> Result<Option<BudgetArtifact>> {
    let schema: Option<String> = row
        .try_get(format!("{prefix}_artifact_schema").as_str())
        .map_err(|_| Error::ClockRegressed)?;
    let digest: Option<String> = row
        .try_get(format!("{prefix}_artifact_digest").as_str())
        .map_err(|_| Error::ClockRegressed)?;
    let body: Option<String> = row
        .try_get(format!("{prefix}_artifact_body").as_str())
        .map_err(|_| Error::ClockRegressed)?;
    match (schema, digest, body) {
        (Some(schema_version), Some(digest), Some(body)) => {
            let artifact = BudgetArtifact {
                schema_version,
                digest,
                body,
            };
            artifact.validate()?;
            Ok(Some(artifact))
        }
        (None, None, None) => Ok(None),
        _ => Err(Error::Internal),
    }
}

fn owned_dispatch_group_from_row(row: &OwnedRow) -> Result<DispatchGroupRecord> {
    Ok(DispatchGroupRecord {
        billing_scope: row
            .try_get("billing_scope")
            .map_err(|_| Error::ClockRegressed)?,
        dispatch_group_id: row
            .try_get("dispatch_group_id")
            .map_err(|_| Error::ClockRegressed)?,
        owner_namespace: row
            .try_get("owner_namespace")
            .map_err(|_| Error::ClockRegressed)?,
        stopped: row
            .try_get::<i64, _>("stopped")
            .map_err(|_| Error::ClockRegressed)?
            != 0,
        stop_reason: row
            .try_get("stop_reason")
            .map_err(|_| Error::ClockRegressed)?,
        stop_committed_at: row
            .try_get("stop_committed_at")
            .map_err(|_| Error::ClockRegressed)?,
        created_at: row
            .try_get("created_at")
            .map_err(|_| Error::ClockRegressed)?,
    })
}

// Time-free record wires from the actual M29 producers; opaque bodies are not walked.
use evo_core::contract::{AppliedReceipt, ResolvedBundle};

// release_store.rs:ReleaseCandidateRecord
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorReleaseCandidateRecord {
    id: String,
    schema_version: String,
    bundle: ResolvedBundle,
    bundle_digest: String,
    environment_digest: String,
    profile_id: String,
    parent_digest: String,
    proposer_actor: String,
    sources: Vec<MirrorTypedSourceRef>,
    revoke_watermark: u64,
}

// release_store.rs:PersistentReleaseState
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorPersistentReleaseState {
    Approved,
    Canary,
    Active,
    Superseded,
    RolledBack,
    Revoked,
}

// release_store.rs:PersistentRelease
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorPersistentRelease {
    id: String,
    schema_version: String,
    candidate_id: String,
    bundle_digest: String,
    environment_digest: String,
    profile_id: String,
    parent_digest: String,
    baseline_bundle_digest: String,
    report_id: String,
    report_digest: String,
    proposer_actor: String,
    evaluator_actor: String,
    approved_by: String,
    expected_parent_release_id: Option<String>,
    expected_pointer_epoch: u64,
    state: MirrorPersistentReleaseState,
}

// release_store.rs:PersistentProfilePointer
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorPersistentProfilePointer {
    id: String,
    schema_version: String,
    profile_id: String,
    active_release_id: Option<String>,
    active_bundle_digest: Option<String>,
    epoch: u64,
}

// release_store.rs:HostApplicationRecord
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorHostApplicationRecord {
    id: String,
    schema_version: String,
    run_id: String,
    release_id: Option<String>,
    actual_request_digest: String,
    execution_receipt_id: Option<String>,
    receipt: Option<AppliedReceipt>,
}

// release_store.rs:TrustedHostExecutionReceipt
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorTrustedHostExecutionReceipt {
    id: String,
    schema_version: String,
    run_id: String,
    request_digest: String,
    environment_digest: String,
    host_surface_digest: String,
    output_digest: String,
    used_ids: Vec<String>,
}

// evidence.rs:StoredRunRecord
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorStoredRunRecord {
    id: String,
    body: Vec<u8>,
    parent_family: String,
    task_origin: TaskOrigin,
    execution_attestation: ExecutionAttestation,
    purpose: Purpose,
}

// evidence.rs:StoredTraceAuthority
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorStoredTraceAuthority {
    schema_version: String,
    record: MirrorStoredRunRecord,
    trace: evo_core::optimization::OptimizationTrace,
    excerpt_start: usize,
    excerpt_end: usize,
}

// optimization.rs:StageFactKind
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorStageFactKind {
    RequestPrepared,
    ResponseObserved,
    EditCompiled,
    DevelopmentRequestPrepared,
    DevelopmentObserved,
    TerminalNoChange,
    TerminalRejected,
    TerminalCandidate,
    StepPrepared,
    StepCompleted,
    DispatchPrepared,
    DispatchObserved,
    TerminalUncertain,
}

// optimization.rs:OptimizationJournalStage
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorOptimizationJournalStage {
    ReflectFailure,
    ReflectSuccess,
    Merge,
    Rank,
    EditCompile,
    Development,
    /// The model-call facts of a consolidation step (`ModelStage::Consolidate`).
    /// The step's own `StepPrepared`/`StepCompleted` facts stay under `Merge`.
    Consolidate,
}

// optimization.rs:StageDependency
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorStageDependency {
    kind: String,
    id: String,
}

// optimization.rs:StageFact
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorStageFact {
    schema_version: String,
    artifact_id: String,
    namespace: String,
    episode_id: String,
    step: u32,
    attempt: u32,
    stage: MirrorOptimizationJournalStage,
    kind: MirrorStageFactKind,
    request_id: String,
    input_digest: String,
    output_digest: Option<String>,
    dependencies: Vec<MirrorStageDependency>,
    payload: serde_json::Value,
}

impl MirrorFormalCandidateDependencyBinding {
    fn validate(&self) -> Result<()> {
        require(
            self.schema_version == "rsia.formal_candidate_dependency_binding.v1"
                && self.revoke_watermark_seq >= 0
                && !self.sources.is_empty(),
        )?;
        for id in [
            &self.namespace,
            &self.candidate_id,
            &self.profile_id,
            &self.proposer_actor,
        ] {
            identifier(id).map_err(|_| Error::ClockRegressed)?;
        }
        for value in [
            &self.candidate_record_digest,
            &self.bundle_digest,
            &self.parent_digest,
            &self.environment_digest,
            &self.revoke_watermark_digest,
        ] {
            digest(value)?;
        }
        let mut previous = None;
        for source in &self.sources {
            identifier(&source.id).map_err(|_| Error::ClockRegressed)?;
            digest(&source.content_digest)?;
            let key = (&source.kind, &source.id);
            require(source.kind == "run" && previous.is_none_or(|prior| prior < key))?;
            previous = Some(key);
        }
        validate_dependency_nodes(&self.nodes)?;
        require(
            self.nodes.iter().any(|node| {
                node.kind == "artifact"
                    && node.id == self.candidate_id
                    && node.schema_version == "rsia.release_candidate.v1"
                    && node.content_digest == self.candidate_record_digest
            }) && self.sources.iter().all(|source| {
                self.nodes
                    .iter()
                    .any(|node| node.kind == source.kind && node.id == source.id)
            }),
        )
    }
}
impl MirrorFormalStoredDependencyBinding {
    fn validate(&self) -> Result<()> {
        require(self.schema_version == "rsia.formal_stored_dependency_binding.v1")?;
        identifier(&self.namespace).map_err(|_| Error::ClockRegressed)?;
        identifier(&self.ticket_id).map_err(|_| Error::ClockRegressed)?;
        digest(&self.candidate_binding_digest)?;
        digest(&self.ticket_digest)?;
        require(
            self.ticket_physical_id
                == evaluation_storage_id("evaluation_ticket_v2", &self.ticket_id)?
                && self.resource_physical_id
                    == evaluation_storage_id("evaluation_resource_evidence_v1", &self.ticket_id)?,
        )?;
        validate_dependency_nodes(&self.nodes)?;
        for (id, kind) in [
            (&self.ticket_physical_id, "evaluation_ticket_v2"),
            (
                &self.resource_physical_id,
                "evaluation_resource_evidence_v1",
            ),
        ] {
            require(self.nodes.iter().any(|node| {
                node.kind == "artifact"
                    && node.id == *id
                    && node.record_kind.as_deref() == Some(kind)
            }))?;
        }
        Ok(())
    }
}
fn dependency_payload_schema(kind: &str) -> Option<&'static str> {
    match kind {
        "registered_evaluation_control_v41" => Some("rsia.registered_evaluation_control.v2"),
        "protected_holdout_v41" => Some("rsia.protected_holdout_record.v1"),
        "evaluation_ticket_v2" => Some("rsia.evaluation_ticket.v2"),
        "evaluation_ticket_issue_receipt_v1" => Some("rsia.evaluation_ticket_issue_receipt.v1"),
        "evaluation_ticket_issue_receipt_v2" => Some("rsia.evaluation_ticket_issue_receipt.v2"),
        "alpha_claim_reservation_v1" => Some("rsia.alpha_claim_reservation.v1"),
        "execution_receipt_v2" => Some("rsia.execution_receipt.v2"),
        "independent_grader_receipt_v2" => Some("rsia.independent_grader_receipt.v2"),
        "evaluation_execution_output_v1" => Some("rsia.execution_output.v1"),
        "evaluation_resource_evidence_v1" => Some("rsia.evaluation_resource_evidence.v1"),
        _ => None,
    }
}
fn validate_dependency_nodes(nodes: &[MirrorFormalDependencyNode]) -> Result<()> {
    require(!nodes.is_empty() && nodes.len() <= 10000)?;
    let mut previous = None;
    for node in nodes {
        for id in [&node.id, &node.logical_id, &node.schema_version] {
            identifier(id).map_err(|_| Error::ClockRegressed)?;
        }
        digest(&node.content_digest)?;
        digest(&node.outgoing_edge_digest)?;
        require(
            matches!(
                node.kind.as_str(),
                "run" | "artifact" | "formal_parent" | "release" | "pointer" | "receipt"
            ) && node.outgoing_edge_count <= 10000,
        )?;
        match (&node.record_kind, node.schema_version.as_str()) {
            (None, "rsia.optimization.source.v1") if node.kind == "run" => {}
            (None, "rsia.release_candidate.v1") if node.kind == "artifact" => {}
            (None, schema) if compiler_node_schema(&node.kind, schema) => {}
            (Some(kind), evo_core::formal_version::NATIVE_TICKET_ENVELOPE)
                if node.kind == "artifact" && kind == "evaluation_ticket_v2" =>
            {
                require(evaluation_storage_id(kind, &node.logical_id)? == node.id)?;
            }
            (Some(kind), "rsia.typed_artifact_envelope.v1")
                if node.kind == "artifact" && dependency_payload_schema(kind).is_some() =>
            {
                require(evaluation_storage_id(kind, &node.logical_id)? == node.id)?;
            }
            _ => return Err(Error::ClockRegressed),
        }
        if node.record_kind.is_none() {
            require(node.logical_id == node.id)?;
        }
        let key = (&node.kind, &node.id);
        require(previous.is_none_or(|prior| prior < key))?;
        previous = Some(key);
    }
    Ok(())
}
fn validate_native_authority(object: &OccurrenceObject) -> Result<()> {
    let authority: MirrorStoredTraceAuthority = decode(&object.body)?;
    let r = &authority.record;
    let t = &authority.trace;
    require(
        object.kind == "run"
            && authority.schema_version == "rsia.optimization.source.v1"
            && object.id == r.id
            && r.task_origin == TaskOrigin::TrustedRun
            && r.execution_attestation == ExecutionAttestation::TrustedHost
            && r.purpose == Purpose::Development
            && t.purpose == r.purpose
            && t.run_id == r.id
            && t.parent_family == r.parent_family
            && t.source_digest == hash(&r.body)
            && r.body.get(authority.excerpt_start..authority.excerpt_end)
                == Some(t.excerpt.as_bytes()),
    )?;
    identifier(&r.id).map_err(|_| Error::ClockRegressed)?;
    identifier(&r.parent_family).map_err(|_| Error::ClockRegressed)
}
fn qualify_stage(object: &OccurrenceObject) -> Result<()> {
    let fact: MirrorStageFact = decode(&object.body)?;
    require(
        object.kind == "artifact"
            && fact.schema_version == "rsia.optimization.stage_fact.v1"
            && fact.namespace == object.namespace
            && fact.artifact_id == object.id
            && !fact.payload.is_null()
            && fact.output_digest.as_deref() == Some(typed_fingerprint(&fact.payload)?.as_str())
            && fact.artifact_id
                == format!(
                    "optstage-{}",
                    typed_fingerprint(&(
                        &fact.namespace,
                        &fact.episode_id,
                        fact.step,
                        fact.attempt,
                        fact.stage,
                        fact.kind,
                        &fact.request_id
                    ))?
                ),
    )?;
    for id in [
        &fact.artifact_id,
        &fact.namespace,
        &fact.episode_id,
        &fact.request_id,
    ] {
        identifier(id).map_err(|_| Error::ClockRegressed)?;
    }
    digest(&fact.input_digest)?;
    for dependency in &fact.dependencies {
        identifier(&dependency.kind).map_err(|_| Error::ClockRegressed)?;
        identifier(&dependency.id).map_err(|_| Error::ClockRegressed)?;
    }
    // This producer's payload is an optimization observation, not an independently
    // owned occurrence source. Its raw content is retained and is never recursed.
    Ok(())
}

use evo_core::contract::{HostCapabilities, SkillSnapshot};
use evo_core::evaluation::CompleteEpisodeCostMeasurements;
use evo_core::{Strategy, Validate};

// M29 formal authority wire: TypedContentRef
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorTypedContentRef {
    kind: String,
    id: String,
    schema_version: String,
    content_digest: String,
}

// M29 formal authority wire: TypedContentEdge
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorTypedContentEdge {
    from_kind: String,
    from_id: String,
    to_kind: String,
    to_id: String,
}

// M29 formal authority wire: TypedContentClosure
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorTypedContentClosure {
    nodes: Vec<MirrorTypedContentRef>,
    edges: Vec<MirrorTypedContentEdge>,
    digest: String,
}

// M29 formal authority wire: RegisteredFormalHost
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorRegisteredFormalHost {
    id: String,
    schema_version: String,
    namespace: String,
    host_id: String,
    host_version: String,
    code_digest: String,
    grader_code_digest: String,
    host_uid: u32,
    grader_uid: u32,
    administrator_uid: u32,
    host_workspace: String,
    grader_workspace: String,
    grader_credential_environment_key: String,
    /// Authenticated credential/principal identities, not Context actor aliases.
    host_principal: String,
    grader_principal: String,
    administrator_principal: String,
    worker_principal: String,
    credential_binding_digest: String,
    isolated_workspace_digest: String,
    grader_workspace_digest: String,
    snapshot: SystemSnapshot,
    capabilities: HostCapabilities,
    host_surface_digest: String,
    adapter_digest: String,
    authorized_by: String,
}

// M29 formal authority wire: RegisteredPrincipal
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorRegisteredPrincipal {
    uid: u32,
    actor: String,
    principal: String,
    /// SHA256 commitment to a distinct private bearer credential. The bearer
    /// itself is accepted only over a kernel-authenticated local connection.
    credential_digest: String,
}

// M29 formal authority wire: RegisteredPrincipalSet
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorRegisteredPrincipalSet {
    id: String,
    schema_version: String,
    namespace: String,
    host_registration_id: String,
    version: u64,
    host: MirrorRegisteredPrincipal,
    grader: MirrorRegisteredPrincipal,
    administrator: MirrorRegisteredPrincipal,
    worker: MirrorRegisteredPrincipal,
}

// M29 formal authority wire: PrincipalSetObservation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorPrincipalSetObservation {
    id: String,
    schema_version: String,
    namespace: String,
    host_digest: String,
    principals: MirrorRegisteredPrincipalSet,
    grader_pid: u32,
    grader_uid: u32,
    grader_executable_digest: String,
    grader_workspace: String,
}

// M29 formal authority wire: PrincipalAction
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorPrincipalAction {
    id: String,
    schema_version: String,
    namespace: String,
    host_registration_id: String,
    host_digest: String,
    principal_set_digest: String,
    role: Role,
    actor: String,
    uid: u32,
    principal: String,
    credential_digest: String,
    subject_kind: String,
    subject_id: String,
    subject_content_digest: String,
    operation: String,
    authenticated_request_digest: String,
    peer_uid: u32,
    peer_gid: Option<u32>,
    peer_pid: Option<u32>,
    authority_pid: u32,
    authority_uid: u32,
    authority_executable_digest: String,
}

// M29 formal authority wire: RegisteredExternalProvider
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorRegisteredExternalProvider {
    id: String,
    schema_version: String,
    namespace: String,
    host_registration_id: String,
    endpoint: String,
    provider_model: String,
    model_digest: String,
    transport_code_digest: String,
    credential_environment_key: String,
    credential_principal: String,
    currency: String,
    pricing_version: String,
    input_micros_per_million_tokens: u64,
    output_micros_per_million_tokens: u64,
    max_output_tokens: u32,
    authorized_by: String,
}

// M29 formal authority wire: ExternalServiceCategory
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum MirrorExternalServiceCategory {
    #[serde(rename = "model")]
    Model,
}

// M29 formal authority wire: RegisteredExternalProviderV2
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorRegisteredExternalProviderV2 {
    id: String,
    schema_version: String,
    namespace: String,
    host_registration_id: String,
    endpoint: String,
    provider_model: String,
    model_digest: String,
    transport_code_digest: String,
    credential_environment_key: String,
    credential_principal: String,
    currency: String,
    pricing_version: String,
    input_micros_per_million_tokens: u64,
    output_micros_per_million_tokens: u64,
    max_output_tokens: u32,
    authorized_by: String,
    service_category: MirrorExternalServiceCategory,
    protocol_id: evo_core::model_protocol::ModelProtocolId,
    sampling_profile: evo_core::model_protocol::SamplingProfileId,
    sampling_digest: String,
    request_timeout_seconds: u32,
}

// M29 formal authority wire: FrozenCompilerProfile
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorFrozenCompilerProfile {
    id: String,
    schema_version: String,
    namespace: String,
    profile_id: String,
    version: u64,
    host_registration_id: String,
    provider_registration_id: String,
    resource_instrumentation_id: Option<String>,
    system_snapshot: SystemSnapshot,
    capabilities: HostCapabilities,
    environment_digest: String,
    baseline_skill: SkillSnapshot,
    baseline_strategy: Strategy,
    dependencies: Vec<MirrorTypedContentRef>,
    purpose: Purpose,
    application_order: Vec<String>,
    authorized_by: String,
}

// M29 formal authority wire: EmptyRootAuthorization
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorEmptyRootAuthorization {
    id: String,
    schema_version: String,
    namespace: String,
    profile_registration_id: String,
    profile_digest: String,
    host_registration_id: String,
    skill: SkillSnapshot,
    strategy: Strategy,
    authorized_by: String,
}

// M29 formal authority wire: ProfileSelection
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorProfileSelection {
    id: String,
    schema_version: String,
    profile_id: String,
    registration_id: String,
    registration_digest: String,
}

// M29 formal authority wire: ParentAuthority
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum MirrorParentAuthority {
    EmptyRoot { id: String },
    ApprovedRelease { release_id: String },
}

// M29 formal authority wire: ParentSelection
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorParentSelection {
    id: String,
    schema_version: String,
    authority: MirrorParentAuthority,
}

// M29 formal authority wire: FormalCompilerInputBinding
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorFormalCompilerInputBinding {
    schema_version: String,
    namespace: String,
    profile_registration_id: String,
    profile_digest: String,
    parent_digest: String,
    baseline_digest: String,
    parent_skill_digest: String,
    parent_strategy_digest: String,
    search_parent_skill_digest: String,
    search_parent_strategy_digest: String,
    baseline_skill_digest: String,
    baseline_strategy_digest: String,
    environment_digest: String,
    capabilities_digest: String,
    purpose: Purpose,
    application_order: Vec<String>,
    watermark: Option<(i64, String)>,
    live_profile_pointer_digest: Option<String>,
    closure: MirrorTypedContentClosure,
    binding_digest: String,
}

// M29 formal authority wire: FormalProviderCaptureRecord
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorFormalProviderCaptureRecord {
    id: String,
    schema_version: String,
    provider_registration_id: String,
    provider_registration_digest: String,
    host_registration_id: String,
    host_registration_digest: String,
    host_principal: String,
    host_pid: u32,
    host_uid: u32,
    host_code_digest: String,
    host_workspace: String,
    dispatch_id: String,
    request_digest: String,
    application_digest: String,
    bundle_digest: String,
    input_digest: String,
    output_digest: String,
    provider_request_id: String,
    usage_record_id: String,
    provider_response_digest: String,
    provider_response_utf8: String,
    actual_model_digest: String,
    input_tokens: u64,
    output_tokens: u64,
    actual_cost_micros: i64,
    currency: String,
    pricing_version: String,
}

// M29 formal authority wire: EpisodeCostRecord
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorEpisodeCostRecord {
    id: String,
    schema_version: String,
    ticket_id: String,
    profile_registration_id: String,
    measurements: CompleteEpisodeCostMeasurements,
    measurement_sources: Vec<MirrorTypedContentRef>,
    observed_by: String,
    observed_pid: u32,
    observed_uid: u32,
}

// M29 formal authority wire: IndependentGradeCapture
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorIndependentGradeCapture {
    id: String,
    schema_version: String,
    ticket_id: String,
    target_id: String,
    grader_receipt_digest: String,
    host_registration_id: String,
    grader_principal: String,
    grader_pid: u32,
    grader_uid: u32,
    grader_code_digest: String,
    grader_workspace: String,
}

// M29 formal authority wire: IssuedFormalAuthority
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorIssuedFormalAuthority {
    id: String,
    schema_version: String,
    report_id: String,
    raw_report_digest: String,
    ticket_digest: String,
    candidate_id: String,
    candidate_bundle_digest: String,
    environment_digest: String,
    compiler_binding: MirrorFormalCompilerInputBinding,
    billing_scope: String,
    root_seal_digest: String,
    episode_cost_digest: String,
    host_registration_id: String,
    principal_set_digest: String,
    report_principal_action_id: String,
    provider_captures: Vec<MirrorTypedContentRef>,
    grader_captures: Vec<MirrorTypedContentRef>,
    closure_roots: Vec<(String, String)>,
    closure: MirrorTypedContentClosure,
    watermark: Option<(i64, String)>,
    issued_by: String,
}

// M29 formal authority wire: RegisteredResourceInstrumentation
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorRegisteredResourceInstrumentation {
    id: String,
    schema_version: String,
    namespace: String,
    host_registration_id: String,
    endpoint: String,
    credential_environment_key: String,
    credential_principal: String,
    instrumentation_code_digest: String,
    authorized_by: String,
}

// M29 formal authority wire: ResourceInstrumentationResponse
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorResourceInstrumentationResponse {
    schema_version: String,
    id: String,
    request_digest: String,
    episode_id: String,
    root_budget_id: String,
    root_snapshot_digest: String,
    instrumentation_code_digest: String,
    complete_episode_root: bool,
    cpu_nanoseconds: Option<u64>,
    storage_byte_seconds: Option<u64>,
    retained_history_bytes: Option<u64>,
    human_nanoseconds: Option<u64>,
    gray_release_nanoseconds: Option<u64>,
}

// M29 formal authority wire: ResourceMeasurementRecord
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorResourceMeasurementRecord {
    id: String,
    schema_version: String,
    episode_id: String,
    ticket_id: String,
    billing_scope: String,
    root_snapshot_digest: String,
    host_registration_id: String,
    meter_registration_id: String,
    meter_registration_digest: String,
    observed_pid: u32,
    observed_uid: u32,
    dimension: String,
    unit: String,
    amount: u64,
    response_utf8: String,
    response_digest: String,
    request_digest: String,
}

// M29 formal authority wire: PersistedCompilerBinding
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorPersistedCompilerBinding {
    id: String,
    schema_version: String,
    candidate_id: String,
    binding: MirrorFormalCompilerInputBinding,
}

fn known_time_free_schema(kind: &str, schema: &str) -> bool {
    match kind {
        "release" => matches!(
            schema,
            "rsia.persistent_release.v1" | "rsia.persistent_release.v2"
        ),
        "pointer" => schema == "rsia.profile_pointer.v1",
        "receipt" => schema == "rsia.host_application.v1",
        "formal_parent" => schema == "rsia.formal_parent_selection.v1",
        "artifact" => matches!(
            schema,
            "rsia.release_candidate.v1"
                | "rsia.host_surface_record.v1"
                | "rsia.run_application.v1"
                | "rsia.host_execution_receipt.v1"
                | "rsia.optimization.stage_fact.v1"
                | "rsia.practice_registration_binding.v1"
                | "rsia.frozen_compiler_profile.v1"
                | "rsia.empty_root_authorization.v1"
                | "rsia.issued_formal_authority.v2"
                | "rsia.issued_formal_authority.v3"
                | "rsia.independent_grade_capture.v2"
                | "rsia.registered_formal_host.v1"
                | "rsia.registered_external_provider.v1"
                | "rsia.registered_external_provider.v2"
                | "rsia.registered_resource_instrumentation.v1"
                | "rsia.registered_formal_principals.v1"
                | "rsia.observed_principal_action.v1"
                | "rsia.observed_control_principal_action.v2"
                | "rsia.release_action.v2"
                | "rsia.formal_principal_set_observation.v1"
                | "rsia.formal_profile_selection.v1"
                | "rsia.persisted_compiler_binding.v1"
                | "rsia.actual_formal_provider_capture.v1"
                | "rsia.independent_grade_capture.v1"
                | "rsia.complete_episode_cost.v1"
                | "rsia.trusted_resource_meter.v1"
                | "rsia.monitoring.environment.v1"
                | "rsia.monitoring.observation.v1"
                | "rsia.monitoring.development_cycle.v2"
                | "rsia.monitoring.development_report_binding.v1"
                | "rsia.monitoring.consolidation_scope.v1"
                | "rsia.monitoring.consolidation_claim.v1"
                | "rsia.monitoring.consolidation_proposal.v1"
                | "rsia.monitoring.consolidation_staging.v1"
                | "rsia.monitoring.consolidation_run.v1"
                | "rsia.monitoring.environment_drift.v1"
                | "rsia.optimization.compiler_use.v1"
                | "rsia.authenticated_bundle.v1"
                | "rsia.curriculum_source.v1"
                | "rsia.curriculum_artifact_envelope.v1"
                | "rsia.replay_economic_artifact_envelope.v1"
                | "rsia.budget_call_ref.v1"
                | "rsia.budget_call_e16_ref.v1"
        ),
        _ => false,
    }
}
fn validate_bundle(bundle: &ResolvedBundle) -> Result<()> {
    let mut preimage = bundle.clone();
    preimage.digest.clear();
    require(typed_fingerprint(&preimage)? == bundle.digest)?;
    bundle.skill.validate().map_err(|_| Error::ClockRegressed)?;
    bundle
        .improver
        .validate()
        .map_err(|_| Error::ClockRegressed)
}
// This function item supplies the existing public URL type without calling
// connection-option construction or its conversion. FromStr is Url::parse for
// the same locked URL package re-exported by reqwest.
fn parse_provider_url<U: std::str::FromStr>(
    raw: &str,
    _type_witness: fn(&sqlx::sqlite::SqliteConnectOptions) -> U,
) -> Result<U> {
    raw.parse::<U>().map_err(|_| Error::ClockRegressed)
}

// Only public data validity is established here. This is not registration or
// an attestation of a provider's source, host, credentials or execution.
fn validate_current_provider_public_fields(
    record: &MirrorRegisteredExternalProviderV2,
) -> Result<()> {
    require(
        record.schema_version == "rsia.registered_external_provider.v2"
            && record.sampling_digest
                == evo_core::model_protocol::sampling_digest_v1(
                    record.protocol_id,
                    record.sampling_profile,
                )
                .map_err(|_| Error::ClockRegressed)?
            && (1..=120).contains(&record.request_timeout_seconds),
    )?;
    let endpoint = parse_provider_url(
        &record.endpoint,
        <sqlx::sqlite::SqliteConnectOptions as sqlx::ConnectOptions>::to_url_lossy,
    )?;
    require(
        endpoint.scheme() == "https"
            && endpoint.host_str().is_some()
            && endpoint.username().is_empty()
            && endpoint.password().is_none()
            && endpoint.query().is_none()
            && endpoint.fragment().is_none()
            && !record.endpoint.chars().any(char::is_whitespace)
            && record.endpoint.starts_with("https://")
            && !record.endpoint.contains('@')
            && (1..=32768).contains(&record.max_output_tokens),
    )?;
    for value in [
        &record.id,
        &record.host_registration_id,
        &record.provider_model,
        &record.credential_environment_key,
        &record.credential_principal,
        &record.currency,
        &record.pricing_version,
    ] {
        identifier(value).map_err(|_| Error::ClockRegressed)?;
    }
    digest(&record.model_digest)?;
    digest(&record.transport_code_digest)
}

fn time_free_record(object: &OccurrenceObject, snapshot: &OccurrenceSnapshot) -> Result<()> {
    let value = &object.body;
    let schema = string(value, "schema_version")?;
    require(known_time_free_schema(&object.kind, schema))?;
    if let Some(id) = value.get("id") {
        require(id.as_str() == Some(object.id.as_str()))?;
    }
    if let Some(namespace) = value.get("namespace") {
        require(namespace.as_str() == Some(object.namespace.as_str()))?;
    }
    macro_rules! wire {
        ($ty:ty) => {{
            let _: $ty = decode(value)?;
        }};
    }
    match schema {
        "rsia.budget_call_ref.v1" => {
            let reference: crate::budget::BudgetCallRef = decode(value)?;
            let identity = typed_fingerprint(&(
                "rsia.budget_call_ref.v1",
                &reference.namespace,
                &reference.billing_scope,
                &reference.call_id,
            ))?;
            require(
                reference.id == format!("budget-ref-{}", &identity[..32])
                    && reference
                        .source_ids
                        .windows(2)
                        .all(|pair| pair[0] < pair[1]),
            )?;
            for id in [&reference.billing_scope, &reference.call_id]
                .into_iter()
                .chain(reference.source_ids.iter())
            {
                identifier(id).map_err(|_| Error::ClockRegressed)?;
            }
            let call = snapshot
                .calls
                .get(&(reference.billing_scope, reference.call_id))
                .ok_or(Error::ClockRegressed)?;
            require(call.namespace == object.namespace)?;
            let e16_id = crate::typed_budget::e16_budget_ref_id(
                &call.namespace,
                &call.billing_scope,
                &call.call_id,
            )
            .map_err(|_| Error::ClockRegressed)?;
            require(
                !call
                    .request_artifact
                    .as_ref()
                    .is_some_and(crate::typed_budget::request_marks_e16)
                    && !snapshot.objects.contains_key(&(
                        call.namespace.clone(),
                        "artifact".into(),
                        e16_id,
                    )),
            )?;
            let expected: Vec<_> = reference
                .source_ids
                .into_iter()
                .map(|id| ("run".to_owned(), id))
                .collect();
            // Accounting retains these exact edges and identifiers on revoke.
            // Source liveness is not new-use authority and is not inferred here.
            require(
                e05_outgoing(snapshot, &object.namespace, &object.kind, &object.id)? == expected,
            )?;
        }
        "rsia.budget_call_e16_ref.v1" => {
            let scope = string(value, "billing_scope")?;
            let id = string(value, "call_id")?;
            let call = snapshot
                .calls
                .get(&(scope.into(), id.into()))
                .ok_or(Error::ClockRegressed)?;
            let body = serde_json::to_string(value).map_err(|_| Error::ClockRegressed)?;
            let reference = crate::typed_budget::history_ref_from_body(call, &body)
                .ok_or(Error::ClockRegressed)?;
            require(call.namespace == object.namespace)?;
            let legacy = typed_fingerprint(&(
                "rsia.budget_call_ref.v1",
                &call.namespace,
                &call.billing_scope,
                &call.call_id,
            ))?;
            require(!snapshot.objects.contains_key(&(
                call.namespace.clone(),
                "artifact".into(),
                format!("budget-ref-{}", &legacy[..32]),
            )))?;
            let expected: Vec<_> = reference
                .object_refs
                .iter()
                .map(|r| (r.kind.clone(), r.id.clone()))
                .collect();
            // The existing request/ref domain preserves accounting on revoke;
            // it grants neither fresh content use nor an occurrence timestamp.
            require(
                e05_outgoing(snapshot, &object.namespace, &object.kind, &object.id)? == expected,
            )?;
        }
        schema if monitoring_schema(schema) => qualify_monitoring(object)?,
        "rsia.optimization.compiler_use.v1" => {
            let record: MirrorCompilerInputUse = decode(value)?;
            require(
                record.schema_version == "rsia.optimization.compiler_use.v1"
                    && object.id == format!("compiler-use-{}", typed_fingerprint(&record)?)
                    && record.binding.namespace == object.namespace,
            )?;
        }
        "rsia.authenticated_bundle.v1" => {
            let record: MirrorAuthenticatedBundleRecord = decode(value)?;
            validate_bundle(&record.bundle)?;
            require(
                record.id == format!("formal-bundle-{}", record.bundle.digest)
                    && record.binding.namespace == object.namespace,
            )?;
        }
        "rsia.curriculum_source.v1" => qualify_curriculum_source(object)?,
        "rsia.release_candidate.v1" => {
            let record: MirrorReleaseCandidateRecord = decode(value)?;
            validate_bundle(&record.bundle)?;
            require(
                record.profile_id == record.bundle.profile_id
                    && record.bundle_digest == record.bundle.digest
                    && record.parent_digest == record.bundle.parent_digest
                    && !record.sources.is_empty(),
            )?;
            digest(&record.environment_digest)?;
            identifier(&record.proposer_actor).map_err(|_| Error::ClockRegressed)?;
            require(record.sources.windows(2).all(|pair| pair[0] < pair[1]))?;
            for source in &record.sources {
                require(source.kind == "run")?;
                digest(&source.content_digest)?;
                let native = snapshot.object(&object.namespace, "run", &source.id)?;
                validate_native_authority(native)?;
                let native: MirrorStoredTraceAuthority = decode(&native.body)?;
                require(native.trace.source_digest == source.content_digest)?;
            }
        }
        "rsia.persistent_release.v1" => wire!(MirrorPersistentRelease),
        "rsia.profile_pointer.v1" => wire!(MirrorPersistentProfilePointer),
        "rsia.host_application.v1" => wire!(MirrorHostApplicationRecord),
        "rsia.host_execution_receipt.v1" => wire!(MirrorTrustedHostExecutionReceipt),
        "rsia.host_surface_record.v1" => {
            let surface: MirrorHostSurfaceRecord = decode(value)?;
            require(surface.manifest_digest == typed_fingerprint(&surface.manifest)?)?;
            surface
                .manifest
                .validate_against_extraction(&surface.extracted)
                .map_err(|_| Error::ClockRegressed)?;
        }
        "rsia.run_application.v1" => {
            let frozen: MirrorRunApplicationSnapshot = decode(value)?;
            require(
                frozen.id == format!("run-snapshot-{}", frozen.run_id)
                    && frozen.environment_digest
                        == frozen
                            .system_snapshot
                            .digest()
                            .map_err(|_| Error::ClockRegressed)?
                    && frozen.profile_id == frozen.system_snapshot.profile_id
                    && frozen.request_digest == typed_fingerprint(&frozen.request_material)?
                    && frozen.request_material.instructions == frozen.projection.instructions
                    && frozen.request_material.tools == frozen.projection.tools,
            )?;
        }
        "rsia.optimization.stage_fact.v1" => qualify_stage(object)?,
        "rsia.practice_registration_binding.v1" => {
            let binding: MirrorPracticeRegistrationBindingV1 = decode(value)?;
            digest(&binding.registration_id)?;
            identifier(&binding.set_id).map_err(|_| Error::ClockRegressed)?;
            require(binding.id == format!("practice-binding-{}", binding.registration_id))?;
        }
        "rsia.frozen_compiler_profile.v1" => {
            let record: MirrorFrozenCompilerProfile = decode(value)?;
            require(
                record.profile_id == record.system_snapshot.profile_id
                    && record.version > 0
                    && record.application_order == ["mandatory_context", "skill", "task"],
            )?;
            record
                .baseline_skill
                .validate()
                .map_err(|_| Error::ClockRegressed)?;
            record
                .baseline_strategy
                .validate()
                .map_err(|_| Error::ClockRegressed)?;
            digest(&record.environment_digest)?;
        }
        "rsia.empty_root_authorization.v1" => wire!(MirrorEmptyRootAuthorization),
        "rsia.issued_formal_authority.v2" => wire!(MirrorIssuedFormalAuthority),
        "rsia.issued_formal_authority.v3" => {
            let record: evo_core::formal_version::FormalAuthorityV3<MirrorIssuedFormalAuthority> =
                decode(value)?;
            record.validate_known().map_err(|_| Error::ClockRegressed)?;
            require(
                record.report_ref.namespace == object.namespace
                    && record.report_ref.owner == object.owner
                    && record.id == object.id
                    && record.authority.issued_by == object.owner,
            )?;
        }
        "rsia.persistent_release.v2" => {
            let record: evo_core::formal_version::PersistentReleaseV2<MirrorPersistentRelease> =
                decode(value)?;
            record.validate_known().map_err(|_| Error::ClockRegressed)?;
            require(
                record.report_ref.namespace == object.namespace
                    && record.id == object.id
                    && record.release.approved_by == object.owner,
            )?;
        }
        "rsia.independent_grade_capture.v2" => {
            let record: MirrorIndependentGradeCapture = decode(value)?;
            require(
                record.schema_version == "rsia.independent_grade_capture.v2"
                    && record.id
                        == format!(
                            "formal-grade-v2-{}",
                            &typed_fingerprint(&(
                                record.ticket_id.as_str(),
                                record.target_id.as_str()
                            ))?[..32]
                        )
                    && record.grader_pid > 0,
            )?;
        }
        "rsia.registered_formal_host.v1" => wire!(MirrorRegisteredFormalHost),
        "rsia.registered_external_provider.v1" => wire!(MirrorRegisteredExternalProvider),
        "rsia.registered_external_provider.v2" => {
            let record: MirrorRegisteredExternalProviderV2 = decode(value)?;
            validate_current_provider_public_fields(&record)?;
        }
        "rsia.registered_resource_instrumentation.v1" => {
            wire!(MirrorRegisteredResourceInstrumentation)
        }
        "rsia.registered_formal_principals.v1" => wire!(MirrorRegisteredPrincipalSet),
        "rsia.release_action.v2" => e05_issue::native_release_action_static_r49(object)?,
        "rsia.observed_principal_action.v1" | "rsia.observed_control_principal_action.v2" => {
            wire!(MirrorPrincipalAction)
        }
        "rsia.formal_principal_set_observation.v1" => wire!(MirrorPrincipalSetObservation),
        "rsia.formal_profile_selection.v1" => wire!(MirrorProfileSelection),
        "rsia.formal_parent_selection.v1" => wire!(MirrorParentSelection),
        "rsia.persisted_compiler_binding.v1" => wire!(MirrorPersistedCompilerBinding),
        "rsia.actual_formal_provider_capture.v1" => {
            let record: MirrorFormalProviderCaptureRecord = decode(value)?;
            require(
                hash(record.provider_response_utf8.as_bytes()) == record.provider_response_digest
                    && record.actual_cost_micros >= 0,
            )?;
            require(record.provider_response_utf8.len() <= 2 * 1024 * 1024)?;
            // This is the producer's opaque provider body reader. Its nested
            // JSON is not an owned timestamp namespace and keeps that reader's
            // Value decoding contract; no recursive date-field classification.
            let response: Value = serde_json::from_str(&record.provider_response_utf8)
                .map_err(|_| Error::ClockRegressed)?;
            let output = response["choices"][0]["message"]["content"]
                .as_str()
                .ok_or(Error::ClockRegressed)?;
            require(
                response["id"].as_str() == Some(record.provider_request_id.as_str())
                    && response["usage"]["prompt_tokens"].as_u64() == Some(record.input_tokens)
                    && response["usage"]["completion_tokens"].as_u64()
                        == Some(record.output_tokens)
                    && hash(output.as_bytes()) == record.output_digest
                    && record.usage_record_id
                        == format!(
                            "usage-{}",
                            &typed_fingerprint(&(
                                record.provider_request_id.as_str(),
                                &response["usage"]
                            ))?[..32]
                        ),
            )?;
        }
        "rsia.independent_grade_capture.v1" => wire!(MirrorIndependentGradeCapture),
        "rsia.complete_episode_cost.v1" => {
            let record: MirrorEpisodeCostRecord = decode(value)?;
            record
                .measurements
                .validate_complete()
                .map_err(|_| Error::ClockRegressed)?;
        }
        "rsia.trusted_resource_meter.v1" => {
            let record: MirrorResourceMeasurementRecord = decode(value)?;
            require(hash(record.response_utf8.as_bytes()) == record.response_digest)?;
            let response: MirrorResourceInstrumentationResponse =
                decode(&parse_owned_json(&record.response_utf8)?)?;
            require(
                record.response_utf8.len() <= 512 * 1024
                    && response.schema_version == "rsia.episode_resource_measurements.v1"
                    && response.complete_episode_root
                    && response.episode_id == record.episode_id
                    && response.root_snapshot_digest == record.root_snapshot_digest
                    && response.request_digest == record.request_digest,
            )?;
        }
        _ => return Err(Error::ClockRegressed),
    }
    Ok(())
}

fn qualify_object(
    object: &OccurrenceObject,
    snapshot: &OccurrenceSnapshot,
    inputs: &PreparePreimages,
    floor: &mut OccurrenceFloor,
) -> Result<()> {
    qualify_object_with_ticket_scan(object, snapshot, inputs, floor, None)
}

fn qualify_object_with_ticket_scan(
    object: &OccurrenceObject,
    snapshot: &OccurrenceSnapshot,
    inputs: &PreparePreimages,
    floor: &mut OccurrenceFloor,
    ticket_scan: Option<&mut TicketRelationScan<'_>>,
) -> Result<()> {
    let schema = object.body.get("schema_version").and_then(Value::as_str);
    match schema {
        Some("rsia.e05.formal_report.v3") => {
            e05_issue::qualify_frozen_complete_r24(object, snapshot, floor)
        }
        Some(evo_core::formal_version::NATIVE_TICKET_ENVELOPE) => {
            require(string(&object.body, "record_kind")? == "evaluation_ticket_v2")?;
            match ticket_scan {
                Some(scan) => {
                    qualify_evaluation_with_ticket_scan(object, snapshot, floor, Some(scan))
                }
                None => qualify_evaluation(object, snapshot, floor),
            }
        }
        Some("rsia.typed_artifact_envelope.v1") => {
            let kind = string(&object.body, "record_kind")?;
            if kind.starts_with("development_") {
                qualify_development(object, snapshot, floor)
            } else {
                match ticket_scan {
                    Some(scan) => {
                        qualify_evaluation_with_ticket_scan(object, snapshot, floor, Some(scan))
                    }
                    None => qualify_evaluation(object, snapshot, floor),
                }
            }
        }
        Some("rsia.curriculum_artifact_envelope.v1") => qualify_curriculum(object, snapshot),
        Some("rsia.replay_economic_artifact_envelope.v1") => qualify_economic(object, snapshot),
        Some("rsia.management_job.v1") => qualify_management(object, snapshot, floor),
        Some("rsia.management_private_input.v1") => {
            let private: MirrorPrivateManagementInput = decode(&object.body)?;
            require(
                object.kind == "artifact"
                    && private.id == object.id
                    && typed_fingerprint(&decode::<MirrorParsedRequest>(&private.payload)?)?
                        == private.payload_digest,
            )
        }
        Some("rsia.practice_registration.v1" | "rsia.practice_attempt_set.v1") => {
            qualify_practice(object, snapshot, floor)
        }
        Some("rsia.exploration_artifact_envelope.v1") => {
            if string(&object.body, "record_kind")? == "meta_trial_v1" {
                qualify_meta(object, snapshot, floor)
            } else {
                qualify_exploration(object)
            }
        }
        Some("rsia.revoke_tombstone.v1") => qualify_tombstone(object, snapshot, floor),
        Some("rsia.optimization.source.v1") => validate_native_authority(object),
        Some("rsia.replay.store_envelope.v1") => qualify_replay_envelope(object, snapshot),
        Some("rsia.redacted.v1") => {
            if object.body.get("original_schema").and_then(Value::as_str)
                == Some(crate::replay::REPLAY_STORE_ENVELOPE_SCHEMA)
            {
                qualify_retained_replay(object, snapshot)
            } else if object
                .body
                .get("original_schema")
                .and_then(Value::as_str)
                .and_then(e16_kind)
                .is_some()
            {
                qualify_e16_redacted(object, floor)
            } else {
                qualify_redacted(object)
            }
        }
        Some(schema) if e16_kind(schema).is_some() => qualify_e16(object, snapshot, inputs, floor),
        Some(schema) if known_time_free_schema(&object.kind, schema) => {
            time_free_record(object, snapshot)
        }
        Some(schema) if schema.starts_with("rsia.") => Err(Error::ClockRegressed),
        Some(_) => opaque_object(object),
        None => {
            if object.kind == "artifact" && object.id.starts_with("optgrant-") {
                let selection: evo_core::evidence::SourceSelection = decode(&object.body)?;
                selection.validate().map_err(|_| Error::ClockRegressed)?;
                return require(
                    object.id == format!("optgrant-{}", typed_fingerprint(&selection)?),
                );
            }

            if object.kind == "artifact"
                && object.body.get("source").and_then(Value::as_str)
                    == Some("trusted_host_snapshot")
            {
                qualify_service_run(object, snapshot, inputs, floor)
            } else if object.kind == "artifact"
                && object.body.as_object().is_some_and(|value| {
                    [
                        "id",
                        "owner",
                        "goal",
                        "snapshot_id",
                        "improver_version",
                        "capabilities",
                        "status",
                        "source",
                        "created_at",
                    ]
                    .iter()
                    .all(|key| value.contains_key(*key))
                })
            {
                Err(Error::ClockRegressed)
            } else if object.kind == "candidate" {
                qualify_candidate(object, snapshot, floor)
            } else if matches!(
                object.kind.as_str(),
                "run" | "release" | "receipt" | "event" | "evaluation"
            ) {
                Err(Error::ClockRegressed)
            } else {
                opaque_object(object)
            }
        }
    }
}
fn opaque_object(object: &OccurrenceObject) -> Result<()> {
    require(
        ![
            "e11-",
            "e12-",
            "optgrant-",
            "compiler-use-",
            "formal-bundle-",
            "monitor-env-",
            "e03dev-",
            "e05-",
            "e09-",
            "e16-",
            "e16sel-",
            "e16src-",
            "e16res-",
            "run-snapshot-",
            "run-",
            "optstage-",
            "management-input-",
            "management-job-",
            "practice-set-",
        ]
        .iter()
        .any(|prefix| object.id.starts_with(prefix)),
    )
}
fn management_operation(operation: &str) -> bool {
    matches!(
        operation,
        "experiment.register"
            | "evaluation.start"
            | "evaluation.status"
            | "replay.run"
            | "curriculum.step"
            | "exploration.start"
            | "meta.start"
    )
}
fn collect_caches(
    _conn: &RawDatabase,
    snapshot: &OccurrenceSnapshot,
    inputs: &PreparePreimages,
    floor: &mut OccurrenceFloor,
) -> Result<()> {
    for cache in &snapshot.caches {
        let operation = cache.operation.as_str();
        if cache.redacted {
            require(cache.response.is_null())?;
            match operation {
                "service.prepare" => require(
                    cache.subject_id
                        == stable_id(
                            "run",
                            &[
                                &cache.namespace,
                                &cache.actor,
                                "prepare",
                                &cache.request_key,
                            ],
                        ),
                )?,
                "service.propose" => require(
                    cache.subject_id
                        == stable_id(
                            "candidate",
                            &[&cache.namespace, &cache.actor, &cache.request_key],
                        ),
                )?,
                "service.feedback" => require(
                    cache.subject_id
                        == stable_id(
                            "feedback",
                            &[&cache.namespace, &cache.actor, &cache.request_key],
                        ),
                )?,
                "e16.import.register" => {
                    require(cache.subject_id == format!("e16sel-{}", &cache.payload_hash[..24]))?
                }
                operation if management_operation(operation) => require(
                    cache.subject_id
                        == management_id(
                            "management-job",
                            &cache.namespace,
                            &cache.actor,
                            operation,
                            &cache.request_key,
                        ),
                )?,
                "optimization.source" => require(cache.subject_id == cache.request_key)?,
                "optimization.stage" => require(cache.subject_id == cache.request_key)?,
                operation if !claimed_owned_operation(operation) => {}
                _ => return Err(Error::ClockRegressed),
            }
            continue;
        }
        let object = OccurrenceObject {
            namespace: cache.namespace.clone(),
            kind: "artifact".into(),
            id: cache.subject_id.clone(),
            owner: cache.actor.clone(),
            body: cache.response.clone(),
            body_value_digest_r26: None,
        };
        match operation {
            "service.prepare" => {
                exact(&cache.response, &["run", "skills", "limitations"], &[])?;
                let _: evo_core::Prepared = decode(&cache.response)?;
                let mut object = object;
                object.replace_body_r26(cache.response["run"].clone());
                require(
                    string(&object.body, "id")? == cache.subject_id
                        && string(&object.body, "owner")? == cache.actor,
                )?;
                qualify_service_run(&object, snapshot, inputs, floor)?;
            }
            "service.propose" => {
                let candidate: evo_core::Candidate = decode(&cache.response)?;
                require(
                    candidate.id == cache.subject_id
                        && candidate.owner == cache.actor
                        && candidate.proposal.request_key == cache.request_key
                        && candidate.digest == cache.payload_hash,
                )?;
                let object = OccurrenceObject {
                    kind: "candidate".into(),
                    body_value_digest_r26: None,
                    ..object
                };
                qualify_candidate(&object, snapshot, floor)?;
            }
            "e16.import.register" => {
                require(
                    string(&object.body, "id")? == cache.subject_id
                        && string(&object.body, "request_key")? == cache.request_key
                        && string(&object.body, "input_digest")? == cache.payload_hash,
                )?;
                qualify_e16(&object, snapshot, inputs, floor)?;
            }
            operation if management_operation(operation) => {
                let job: MirrorManagementJob = decode(&object.body)?;
                require(
                    job.id == cache.subject_id
                        && job.operation == operation
                        && job.request_key == cache.request_key
                        && job.payload_digest == cache.payload_hash
                        && job.owner_actor == cache.actor,
                )?;
                let object = OccurrenceObject {
                    kind: "job".into(),
                    body_value_digest_r26: None,
                    ..object
                };
                qualify_management(&object, snapshot, floor)?;
            }
            "service.feedback" => {
                exact(
                    &object.body,
                    &["id", "owner", "request", "verification"],
                    &[],
                )?;
                let feedback: evo_core::FeedbackRecord = decode(&object.body)?;
                require(
                    feedback.id == cache.subject_id
                        && feedback.owner == cache.actor
                        && feedback.request.request_key == cache.request_key
                        && typed_fingerprint(&feedback.request)? == cache.payload_hash
                        && feedback.id
                            == stable_id(
                                "feedback",
                                &[&cache.namespace, &cache.actor, &cache.request_key],
                            ),
                )?;
            }
            "optimization.source" => {
                let object = OccurrenceObject {
                    kind: "run".into(),
                    body_value_digest_r26: None,
                    ..object
                };
                validate_native_authority(&object)?;
                require(
                    cache.subject_id == cache.request_key
                        && typed_fingerprint(&decode::<MirrorStoredTraceAuthority>(&object.body)?)?
                            == cache.payload_hash,
                )?;
            }
            "optimization.stage" => {
                qualify_stage(&object)?;
                require(
                    cache.subject_id == cache.request_key
                        && typed_fingerprint(&decode::<MirrorStageFact>(&object.body)?)?
                            == cache.payload_hash,
                )?;
            }
            operation if !claimed_owned_operation(operation) => {
                require(
                    !cache
                        .response
                        .get("schema_version")
                        .and_then(Value::as_str)
                        .is_some_and(|s| s.starts_with("rsia.")),
                )?;
            }
            _ => return Err(Error::ClockRegressed),
        }
    }
    Ok(())
}
fn claimed_owned_operation(operation: &str) -> bool {
    [
        "service.",
        "optimization.",
        "e16.",
        "experiment.",
        "evaluation.",
        "replay.",
        "curriculum.",
        "exploration.",
        "meta.",
    ]
    .iter()
    .any(|prefix| operation.starts_with(prefix))
}

use evo_core::evaluation::DataUse;
use evo_core::strategy::{LegalActionV1, OptimizationHistoryEntry, PrefixNodeV2};

// M29 exploration.rs: ExplorationEvidenceV1
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorExplorationEvidenceV1 {
    /// The development report passed the observation gate: its receipts, budget
    /// rows and scores were reloaded and the node's quality is the server-recomputed
    /// one. The only label a `Valid` node written by `run_next` can carry.
    Trusted,
    /// The report declared Fixture provenance. That is honest, and it can never be
    /// a trusted observation; the gate was not asked.
    FixtureDeclared,
    /// The report claimed an execution the gate refused: no receipts, scores that
    /// differ from the recomputed ones, or any other mismatch.
    EvidenceRejected,
    /// No gate verdict exists: the outcome was no candidate, the gate could not be
    /// reached, or the record was written before the gate existed (the default a
    /// record without the field reads as).
    #[default]
    NotObserved,
}

// M29 exploration.rs: ExplorationDispatchState
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorExplorationDispatchState {
    Claimed,
    Observed,
    Uncertain,
}

// M29 exploration.rs: PersistentSearchNode
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorPersistentSearchNode {
    schema_version: String,
    world_id: String,
    node: PrefixNodeV2,
    candidate_bundle_digest: Option<String>,
    candidate_skill_digest: Option<String>,
    development_selection_digest: Option<String>,
    intermediate_only: bool,
    /// The gate's verdict on the evidence behind this node. A node stored without
    /// the field is `NotObserved`.
    #[serde(default)]
    evidence: MirrorExplorationEvidenceV1,
    /// How the step that produced this node ended, as the closed class the optimization
    /// step reports (a no change, an incumbent kept for either of its two reasons, any
    /// kind of rejection, an uncertain dispatch, or the verdict on a candidate). It is a
    /// record, not an input: no decision reads it, so the node's status stays what the
    /// strategy knows (`HardFailure` for every outcome that is no candidate, `Valid`
    /// for a verified one, `UsageUncertain` for an unknown one). A node stored without
    /// the field, which every node written before the classes is, has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal_class: Option<MirrorStepTerminalClass>,
}

// M29 exploration.rs: ExplorationDispatchFact
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorExplorationDispatchFact {
    schema_version: String,
    id: String,
    world_id: String,
    action_id: String,
    action_seq: u32,
    request_digest: String,
    idempotency_key: String,
    decision: MirrorCoordinatorDecision,
    selected_action: LegalActionV1,
    expected_parent_skill_digest: String,
    expected_parent_bundle_digest: String,
    context_signature: String,
    state: MirrorExplorationDispatchState,
    node_id: Option<String>,
    /// The fixed code of the outcome ([`MirrorStepTerminalClass::code`]) once the dispatch is
    /// completed. A fact written before the classes existed holds the free text its
    /// step reported, and is read as it is.
    outcome_reason: Option<String>,
    /// The gate's verdict on the evidence of the dispatched step, the same label
    /// its node carries. `NotObserved` while the fact is claimed, for an outcome
    /// that is no candidate, and for a fact written before the gate existed (the
    /// schema is unchanged: a v2 fact without the field reads as `NotObserved`).
    #[serde(default)]
    evidence: MirrorExplorationEvidenceV1,
    /// The class the dispatch ended with, the same value its node carries: the typed
    /// form of `outcome_reason` and the fields it cannot hold. `None` while the fact is
    /// claimed and for a fact written before the classes existed (the schema is
    /// unchanged: a v2 fact without the field has none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal_class: Option<MirrorStepTerminalClass>,
}

// M29 exploration.rs: CoordinatorDecision
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorCoordinatorDecision {
    world_id: String,
    prefix_digest: String,
    legal_actions_digest: String,
    /// Digest of the `ElasticPolicyV1` actually passed to `decide_elastic`.
    policy_digest: String,
    /// Digest of the `ExplorationCapsV1` actually passed to `decide_elastic`.
    caps_digest: String,
    action: BatchActionV1,
}

// M29 optimization.rs: StepErrorKind
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorStepErrorKind {
    Invalid,
    Forbidden,
    NotFound,
    Budget,
    Conflict,
    Cancelled,
    Internal,
}

// M29 optimization.rs: StepTerminalClass
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "class", rename_all = "snake_case")]
enum MirrorStepTerminalClass {
    // ----- no change: the step ended on purpose, with nothing to propose (V088.b, V089.c)
    /// No reflection batch could be built from the traces of the step.
    NoEligibleReflectionBatch,
    /// Every reflection call answered with no suggestion.
    NoEditSuggestions,
    /// The atomic edit compiled and changed nothing.
    EditProducedNoChange,

    // ----- the incumbent is kept: the candidate was run and not accepted (V090.a)
    /// The development selection kept the incumbent: the candidate did not strictly
    /// improve the total of the same full manifest, and no incumbent pass was lost.
    KeepIncumbentNotImproved {
        parent_total_micros: u64,
        candidate_total_micros: u64,
    },
    /// The development selection kept the incumbent because the candidate lost a task
    /// the incumbent passed, whatever its total (a higher total included).
    KeepIncumbentRetentionBroken {
        parent_total_micros: u64,
        candidate_total_micros: u64,
    },

    // ----- rejected: the step ended on a refusal
    /// The source grant does not allow a model excerpt, or is not the one of the step.
    GrantUnavailable,
    /// The model's provider rejected a request. The kind is the typed answer; the
    /// words that came with it are never kept.
    ModelRejected { kind: MirrorModelRejectionKind },
    /// The model's answer is not a suggestion list: it does not parse as one, or it is
    /// longer than the request allowed.
    SuggestionShapeInvalid,
    /// The pool holds more suggestions than the final edit limit and no ranking call
    /// was preregistered.
    SuggestionPoolUnranked,
    /// The atomic skill edit did not compile.
    EditCompileFailed,
    /// The complete bundle did not compile.
    BundleCompileFailed,
    /// The development selection refused the report of the development runner.
    DevelopmentReportRejected,
    /// Any other refusal of the step, by the kind of the error that ended it.
    StepError { error: MirrorStepErrorKind },

    // ----- uncertain: the dispatch was paid for and its outcome cannot be recorded
    /// The usage of a model call is unknown.
    ModelUsageUnknown,
    /// A model dispatch was prepared and has no durable response: its usage is unknown.
    ModelDispatchUnrecorded,
    /// Another worker holds the model dispatch.
    ModelDispatchConcurrent,
    /// The model transport failed after the dispatch.
    ModelTransportOutcomeUnknown,
    /// A development execution was prepared and has no durable result.
    DevelopmentExecutionUnrecorded,
    /// Another worker holds the development execution.
    DevelopmentExecutionConcurrent,
    /// The development runner failed after the dispatch.
    DevelopmentExecutionOutcomeUnknown,
    /// The dispatch was cancelled, or its outcome cannot be told.
    CancelledOrUncertain,
    /// The optimization step ended in an error the coordinator cannot classify.
    OptimizationDispatchOutcomeUncertain,
    /// The source closure of the world changed while the step ran.
    ExplorationSourceClosureChangedAfterDispatch,
    /// The prefix the step was decided on is not the one the world holds when it ends.
    ExplorationPrefixChangedAfterDispatch,
    /// A claim was resumed on a world that no longer carries the inputs it was claimed with.
    ClaimInputsChangedAfterDispatch,
    /// The node a `Recover` repairs is not where the world lists it.
    RecoverTargetMissing,
    /// The observation gate could not answer for the evidence of a candidate.
    DevelopmentEvidenceUnverified,
    /// The digests of a candidate that compiled and was selected cannot be computed.
    CandidateMaterialUnavailable,

    // ----- a candidate, as the exploration coordinator records the verdict on it
    /// The observation gate verified the candidate's evidence.
    CandidateObserved,
    /// The report declared Fixture provenance, which can never be trusted evidence.
    FixtureEvidenceNotAccepted,
    /// The observation gate refused the report.
    DevelopmentEvidenceRejected,

    // ----- a record written before the classes existed
    /// The terminal of a step journaled as free text. The text is not carried forward.
    LegacyUnclassified,
}

// M29 model.rs: ModelRejectionKind
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorModelRejectionKind {
    Unauthorized,
    BudgetUnavailable,
    InvalidRequest,
    ProviderRejected,
    CancelledBeforeDispatch,
    CancelledAfterDispatch,
}

impl MirrorExplorationWorldV1 {
    const SCHEMA: &'static str = "rsia.exploration_world.v1";
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != Self::SCHEMA {
            return Err(Error::Invalid(
                "unsupported exploration world schema".into(),
            ));
        }
        identifier(&self.id)?;
        for value in [
            &self.approved_parent_digest,
            &self.context_signature,
            &self.parent_skill_digest,
            &self.parent_bundle_digest,
            &self.environment_digest,
            &self.model_digest,
            &self.tools_digest,
            &self.grader_digest,
            &self.rules_digest,
        ] {
            digest(value)?;
        }
        if self.context_signature
            != fingerprint(&(
                &self.parent_skill_digest,
                &self.parent_bundle_digest,
                &self.environment_digest,
                &self.model_digest,
                &self.tools_digest,
                &self.grader_digest,
                &self.rules_digest,
                self.source_watermark,
            ))?
        {
            return Err(Error::Conflict("world context signature mismatch".into()));
        }
        self.caps.validate()?;
        self.policy.validate()?;
        self.simulation.width()?;
        if self.initial_baseline_quality_micros > 1_000_000
            || self.node_ids.len() > usize::from(self.caps.max_nodes)
            || self.successor_cost_upper_micros == 0
        {
            return Err(Error::Invalid("invalid exploration world limits".into()));
        }
        let mut roots = BTreeSet::new();
        for root in &self.root_opportunities {
            if root.root_slot == 0
                || root.branch_seq == 0
                || root.action_seq == 0
                || root.action_seq >= 1_000_000
                || root.estimated_cost_upper_micros == 0
                || !roots.insert(root.action_seq)
            {
                return Err(Error::Invalid("invalid root opportunity".into()));
            }
        }
        let mut dependencies = BTreeSet::new();
        if self.dependencies.is_empty() {
            return Err(Error::Invalid(
                "exploration world requires a typed source closure".into(),
            ));
        }
        for dependency in &self.dependencies {
            identifier(&dependency.kind)?;
            identifier(&dependency.id)?;
            if dependency.kind != "run"
                || !dependencies.insert((dependency.kind.as_str(), dependency.id.as_str()))
            {
                return Err(Error::Invalid(
                    "exploration source closure must contain unique trusted runs".into(),
                ));
            }
        }
        for id in self
            .node_ids
            .iter()
            .chain(self.dispatch_ids.iter())
            .chain(self.history_ids.iter())
        {
            identifier(id)?;
        }
        Ok(())
    }
}

fn qualify_exploration(object: &OccurrenceObject) -> Result<()> {
    let kind = string(&object.body, "record_kind")?;
    let payload = envelope(object, "rsia.exploration_artifact_envelope.v1", kind)?;
    let logical = match kind {
        "exploration_world_v1" => {
            let world: MirrorExplorationWorldV1 = decode(payload)?;
            world.validate().map_err(|_| Error::ClockRegressed)?;
            world.id
        }
        "exploration_node_v1" => {
            let node: MirrorPersistentSearchNode = decode(payload)?;
            require(node.schema_version == "rsia.exploration_node.v1")?;
            format!("node-{}-{}", node.world_id, node.node.node_seq)
        }
        "exploration_dispatch_v1" => {
            let dispatch: MirrorExplorationDispatchFact = decode(payload)?;
            require(dispatch.schema_version == "rsia.exploration_dispatch.v2")?;
            dispatch.id
        }
        "optimization_history_v1" => {
            let history: OptimizationHistoryEntry = decode(payload)?;
            require(history.data_use == DataUse::Development)?;
            history.entry_id
        }
        _ => return Err(Error::ClockRegressed),
    };
    require(object.id == format!("e09-{}", typed_fingerprint(&(kind, logical))?))
}
fn qualify_redacted(object: &OccurrenceObject) -> Result<()> {
    let value = &object.body;
    exact(
        value,
        &[
            "id",
            "schema_version",
            "state",
            "original_kind",
            "original_schema",
            "original_digest",
            "metadata",
        ],
        &[],
    )?;
    require(
        string(value, "id")? == object.id
            && string(value, "schema_version")? == "rsia.redacted.v1"
            && string(value, "state")? == "source_revoked"
            && string(value, "original_kind")? == object.kind,
    )?;
    digest(string(value, "original_digest")?)?;
    if let Some(schema) = value["original_schema"].as_str() {
        require(
            known_time_free_schema(&object.kind, schema)
                || matches!(
                    schema,
                    "rsia.optimization.source.v1"
                        | evo_core::formal_version::NATIVE_TICKET_ENVELOPE
                        | "rsia.typed_artifact_envelope.v1"
                        | "rsia.exploration_artifact_envelope.v1"
                        | "rsia.management_private_input.v1"
                        | "rsia.replay.store_envelope.v1"
                        | "rsia.curriculum_artifact_envelope.v1"
                        | "rsia.practice_registration.v1"
                        | "rsia.practice_attempt_set.v1"
                ),
        )?;
    } else {
        require(value["original_schema"].is_null())?;
    }
    retained_metadata(&value["metadata"])
}
const RETAINED_METADATA_KEYS: &[&str] = &[
    "artifact_id",
    "kind",
    "state",
    "stage",
    "episode_id",
    "step",
    "attempt",
    "request_id",
    "call_id",
    "dispatch_id",
    "root_budget_id",
    "provider_request_id",
    "usage_record_id",
    "ticket_id",
    "report_id",
    "run_id",
    "release_id",
    "candidate_id",
    "profile_id",
    "bundle_digest",
    "parent_digest",
    "pointer_epoch",
    "input_digest",
    "output_digest",
    "request_digest",
    "actual_request_digest",
    "environment_digest",
    "host_surface_digest",
    "host_capabilities_digest",
    "revoke_watermark",
    "namespace",
    "purpose",
    "failure_kind",
    "quality_micros",
    "dispatched_repairs",
    "environment_reset",
    "world_id",
    "sealed_digest",
    "status",
    "decision",
    "record_kind",
    "provenance",
    "actual_cost_micros",
    "cost_micros",
    "input_tokens",
    "output_tokens",
    "latency_millis",
    "usage_record_ids",
    "parent_total_micros",
    "candidate_total_micros",
    "execution_receipt_id",
    "used_ids",
    "capability_level",
    "evolution_enabled",
    "lease_epoch",
    "dispatch_state",
    "opportunity_consumed",
    "remaining_root_micros",
    "remaining_recovery_dispatches",
    "dispatch_ids",
    "node_ids",
    "decision_round",
    "source_watermark",
    "root_slot",
    "branch_seq",
    "action_seq",
    "estimated_cost_upper_micros",
    "historical_facts",
];
fn retained_metadata(value: &Value) -> Result<()> {
    let map = value.as_object().ok_or(Error::ClockRegressed)?;
    require(
        map.keys()
            .all(|key| RETAINED_METADATA_KEYS.contains(&key.as_str())),
    )?;
    if let Some(history) = map.get("historical_facts") {
        retained_history(history)?;
    }
    Ok(())
}
fn retained_history(value: &Value) -> Result<()> {
    match value {
        Value::Null => Ok(()),
        Value::Array(items) => {
            for item in items {
                retained_history(item)?;
            }
            Ok(())
        }
        Value::Object(map) => {
            // historical_stage_facts preserves only minimal scalar metadata and
            // nested nonempty projections; it never retains an occurrence field.
            for (key, item) in map {
                if item.is_array() || item.is_object() {
                    retained_history(item)?;
                } else {
                    require(RETAINED_METADATA_KEYS.contains(&key.as_str()))?;
                }
            }
            Ok(())
        }
        _ => Err(Error::ClockRegressed),
    }
}

fn meta_s0_digest(
    world: &MirrorExplorationWorldV1,
    limits: &MirrorStreamLimitsV1,
) -> Result<String> {
    typed_fingerprint(&(
        "rsia.meta_trial.s0.v1",
        &world.schema_version,
        &world.approved_parent_digest,
        &world.context_signature,
        (
            &world.parent_skill_digest,
            &world.parent_bundle_digest,
            &world.environment_digest,
            &world.model_digest,
            &world.tools_digest,
            &world.grader_digest,
            &world.rules_digest,
        ),
        world.source_watermark,
        &world.caps,
        world.simulation,
        &world.root_opportunities,
        &world.dependencies,
        (
            world.successor_cost_upper_micros,
            world.initial_baseline_quality_micros,
            limits.remaining_root_micros,
            limits.remaining_recovery_dispatches,
        ),
    ))
}

fn development_cost_state(call: &BudgetCallRecord) -> Result<MirrorDevelopmentCostState> {
    match call.state {
        BudgetCallState::Finalized => Ok(MirrorDevelopmentCostState::Known {
            micros: call.actual_cost_micros.ok_or(Error::ClockRegressed)?,
            currency: call.actual_currency.clone().ok_or(Error::ClockRegressed)?,
            pricing_version: call
                .actual_pricing_version
                .clone()
                .ok_or(Error::ClockRegressed)?,
        }),
        BudgetCallState::Uncertain => Ok(MirrorDevelopmentCostState::Uncertain),
        _ => Err(Error::ClockRegressed),
    }
}
fn validate_development_execution(
    receipt: &MirrorDevelopmentExecutionReceiptV1,
    control: &MirrorDevelopmentControlV1,
    task: &MirrorDevelopmentTaskSpecV1,
    call: &BudgetCallRecord,
    output: &MirrorDevelopmentExecutionOutputV1,
) -> Result<()> {
    let key = (&receipt.request_id, &receipt.task_id, receipt.side);
    require(
        receipt.receipt_id == format!("devexec-{}", typed_fingerprint(&key)?)
            && receipt.budget_call_id == format!("devcall-{}", typed_fingerprint(&key)?)
            && receipt.usage_record_id == format!("devusage-{}", typed_fingerprint(&key)?)
            && receipt.output_artifact_id == format!("devout-{}", typed_fingerprint(&key)?)
            && receipt.execution_provenance
                == MirrorDevelopmentExecutionProvenance::RegisteredPureFunction
            && receipt.source_ids == control.source_ids
            && receipt.request_digest
                == typed_fingerprint(&(
                    crate::budget::REGISTERED_EXECUTION_REQUEST_SCHEMA,
                    &control.id,
                    &receipt.request_id,
                    &receipt.episode_id,
                    receipt.step,
                    receipt.attempt,
                    &task.task_id,
                    &task.input_digest,
                    receipt.side,
                    &receipt.bundle_digest,
                    &receipt.environment_digest,
                    &control.manifest_digest,
                    &control.grader_digest,
                    &control.rules_digest,
                    &control.tools_digest,
                    receipt.revoke_watermark,
                ))?
            && call.namespace == control.namespace
            && call.dispatch_group_id == receipt.episode_id
            && call.stage == BudgetStage::DevelopmentExecution
            && matches!(
                call.state,
                BudgetCallState::Finalized | BudgetCallState::Uncertain
            )
            && call.execution_closed
            && call.dispatch_id.is_some()
            && call.response_usable == Some(true)
            && call.execution_provenance != Some(crate::budget::BudgetExecutionProvenance::Fixture)
            && development_cost_state(call)? == receipt.cost_state,
    )?;
    let settlement = crate::budget::RegisteredExecutionSettlement::from_call(call)
        .map_err(|_| Error::ClockRegressed)?;
    require(
        settlement.provenance
            == crate::budget::RegisteredExecutionProvenance::RegisteredPureFunction
            && settlement.target_id == task.target_id
            && settlement.target_digest == control.target_digest
            && settlement.runner_digest == control.runner_digest,
    )?;
    let artifact = call
        .request_artifact
        .as_ref()
        .ok_or(Error::ClockRegressed)?;
    require(artifact.schema_version == crate::budget::REGISTERED_EXECUTION_REQUEST_SCHEMA)?;
    let request: MirrorRegisteredExecutionRequestV1 = decode(&parse_owned_json(&artifact.body)?)?;
    require(
        request.schema_version == artifact.schema_version
            && request.control_id == control.id
            && request.request_id == receipt.request_id
            && request.episode_id == receipt.episode_id
            && request.task_id == task.task_id
            && request.side == receipt.side
            && typed_fingerprint(&request.input)? == typed_fingerprint(&task.input)?
            && request.input_digest == task.input_digest
            && request.bundle_digest == receipt.bundle_digest
            && request.environment_digest == receipt.environment_digest
            && request.request_digest == receipt.request_digest
            && request.target_id == task.target_id
            && request.target_digest == control.target_digest
            && request.runner_digest == control.runner_digest
            && request.source_ids == control.source_ids
            && output.schema_version == "rsia.development_execution_output.v1"
            && output.id == receipt.output_artifact_id
            && output.control_id == receipt.control_id
            && output.request_id == receipt.request_id
            && output.task_id == receipt.task_id
            && output.side == receipt.side
            && output.output_digest == receipt.output_digest
            && hash(output.output_utf8.as_bytes()) == receipt.output_digest,
    )
}
fn fixed_score(output: &str, expected: &str) -> Result<u32> {
    let expected = parse_owned_json(expected)?;
    let Ok(output) = parse_owned_json(output) else {
        return Ok(0);
    };
    let valid = output
        .as_object()
        .is_some_and(|object| object.len() == 1 && object.get("answer") == Some(&expected));
    Ok(if valid { 1000000 } else { 0 })
}
fn validate_development_grader(
    receipt: &MirrorDevelopmentGraderReceiptV1,
    control: &MirrorDevelopmentControlV1,
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
) -> Result<()> {
    require(
        receipt.receipt_id
            == format!(
                "devgrade-{}",
                typed_fingerprint(&(&receipt.request_id, &receipt.task_id))?
            )
            && receipt.source_ids == control.source_ids
            && receipt.grader_version == control.fixed_grader.version
            && receipt.parent_passed == (receipt.parent_score_micros == 1000000)
            && receipt.candidate_passed == (receipt.candidate_score_micros == 1000000),
    )?;
    let task = control
        .tasks
        .iter()
        .find(|task| task.task_id == receipt.task_id)
        .ok_or(Error::ClockRegressed)?;
    for (id, side, score) in [
        (
            &receipt.parent_execution_receipt_id,
            MirrorDevelopmentSide::Parent,
            receipt.parent_score_micros,
        ),
        (
            &receipt.candidate_execution_receipt_id,
            MirrorDevelopmentSide::Candidate,
            receipt.candidate_score_micros,
        ),
    ] {
        let row = snapshot.object(
            namespace,
            "artifact",
            &development_storage_id("development_execution_receipt_v1", id)?,
        )?;
        let execution: MirrorDevelopmentExecutionReceiptV1 = decode(envelope(
            row,
            "rsia.typed_artifact_envelope.v1",
            "development_execution_receipt_v1",
        )?)?;
        require(
            execution.request_id == receipt.request_id
                && execution.episode_id == receipt.episode_id
                && execution.step == receipt.step
                && execution.attempt == receipt.attempt
                && execution.side == side
                && execution.revoke_watermark == receipt.revoke_watermark,
        )?;
        let output = snapshot.object(
            namespace,
            "artifact",
            &development_storage_id(
                "development_execution_output_v1",
                &execution.output_artifact_id,
            )?,
        )?;
        let output: MirrorDevelopmentExecutionOutputV1 = decode(envelope(
            output,
            "rsia.typed_artifact_envelope.v1",
            "development_execution_output_v1",
        )?)?;
        require(fixed_score(&output.output_utf8, &task.expected_answer_json)? == score)?;
    }
    require(
        receipt.scored_output_pair_digest
            == typed_fingerprint(&(
                &receipt.parent_output_digest,
                &receipt.candidate_output_digest,
                &control.oracle_digest,
                &control.grader_digest,
                receipt.parent_score_micros,
                receipt.candidate_score_micros,
                receipt.parent_passed,
                receipt.candidate_passed,
            ))?,
    )?;
    if let Some(id) = &receipt.scoring_budget_call_id {
        let call = qualified_call(snapshot, namespace, &control.billing_scope, id)?;
        require(
            call.dispatch_group_id == receipt.episode_id
                && call.stage == BudgetStage::DevelopmentScoring
                && matches!(
                    call.state,
                    BudgetCallState::Finalized | BudgetCallState::Uncertain
                )
                && call.execution_closed
                && call.execution_provenance
                    != Some(crate::budget::BudgetExecutionProvenance::Fixture),
        )?;
    }
    Ok(())
}

fn collect_replay_worlds(conn: &RawDatabase, snapshot: &mut OccurrenceSnapshot) -> Result<()> {
    for row in rows(
        conn,
        "replay_worlds",
        &["namespace", "id", "sealed", "manifest"],
    )? {
        let namespace = sql_string(&row, "namespace")?;
        let id = sql_string(&row, "id")?;
        let sealed = sql_bool(&row, "sealed")?;
        identifier(&namespace).map_err(|_| Error::ClockRegressed)?;
        identifier(&id).map_err(|_| Error::ClockRegressed)?;
        let value = parse_owned_json(&sql_string(&row, "manifest")?)?;
        if value.get("schema_version").and_then(Value::as_str) == Some("rsia.redacted.v1") {
            exact(
                &value,
                &[
                    "schema_version",
                    "state",
                    "original_schema",
                    "original_digest",
                    "sealed_digest",
                    "metadata",
                    "historical_transitions",
                ],
                &[],
            )?;
            require(
                string(&value, "state")? == "source_revoked"
                    && string(&value, "original_schema")? == "rsia.replay_world.v2",
            )?;
            digest(string(&value, "original_digest")?)?;
            if let Some(d) = value["sealed_digest"].as_str() {
                digest(d)?;
                require(sealed)?;
            } else {
                require(value["sealed_digest"].is_null() && !sealed)?;
            }
            retained_metadata(&value["metadata"])?;
            require(value["historical_transitions"].is_array())?;
            require(
                snapshot
                    .redacted_replay_worlds
                    .insert((namespace, id), (sealed, value))
                    .is_none(),
            )?;
            continue;
        }
        let world: evo_core::replay::ReplayWorldV2 = decode(&value)?;
        require(world.manifest.world_id == id && world.sealed_digest.is_some() == sealed)?;
        if sealed {
            world.validate_sealed().map_err(|_| Error::ClockRegressed)?;
        } else {
            let mut validation = world.clone();
            validation.seal().map_err(|_| Error::ClockRegressed)?;
        }
        require(
            snapshot
                .replay_worlds
                .insert((namespace, id), world)
                .is_none(),
        )?;
    }
    Ok(())
}
fn replay_pool(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    digest_value: &str,
) -> Result<evo_core::replay::ReplayPoolManifestV1> {
    digest(digest_value)?;
    let row = snapshot.object(
        namespace,
        "artifact",
        &format!("replay-pool-{digest_value}"),
    )?;
    if row.body.get("schema_version").and_then(Value::as_str) == Some("rsia.redacted.v1") {
        return qualified_retained_replay_pool(row, snapshot);
    }
    let pool: evo_core::replay::ReplayPoolManifestV1 = decode(&row.body["payload"])?;
    pool.validate_identity()
        .map_err(|_| Error::ClockRegressed)?;
    require(pool.pool_digest == digest_value)?;
    if pool.members.iter().any(|member| {
        !snapshot
            .replay_worlds
            .contains_key(&(namespace.into(), member.world_id.clone()))
    }) {
        qualify_mixed_replay_pool(row, &pool, snapshot)?;
        return Ok(pool);
    }
    let mut worlds = Vec::new();
    for member in &pool.members {
        worlds.push(
            snapshot
                .replay_worlds
                .get(&(namespace.into(), member.world_id.clone()))
                .ok_or(Error::ClockRegressed)?
                .clone(),
        );
    }
    pool.validate_against(&worlds)
        .map_err(|_| Error::ClockRegressed)?;
    Ok(pool)
}
// Only cleanup-redacted world rows may replace missing live members here.
// This validates retained identity for the clock scan, never replay authority.
fn qualify_mixed_replay_pool(
    object: &OccurrenceObject,
    pool: &evo_core::replay::ReplayPoolManifestV1,
    snapshot: &OccurrenceSnapshot,
) -> Result<()> {
    let value = &object.body;
    exact(
        value,
        &[
            "schema_version",
            "id",
            "namespace",
            "record_kind",
            "payload",
        ],
        &[],
    )?;
    require(
        object.kind == "artifact"
            && string(value, "schema_version")? == crate::replay::REPLAY_STORE_ENVELOPE_SCHEMA
            && string(value, "id")? == object.id
            && string(value, "namespace")? == object.namespace
            && string(value, "record_kind")? == crate::replay::REPLAY_POOL_RECORD_KIND,
    )?;
    retained_replay_owner(object, snapshot, "replay.pool.register")?;
    let mut clusters = BTreeMap::new();
    let mut edges = Vec::new();
    for member in &pool.members {
        if let Some(partition) = clusters.insert(member.cluster_id.as_str(), member.partition) {
            require(partition == member.partition)?;
        }
        let key = (object.namespace.clone(), member.world_id.clone());
        if let Some(world) = snapshot.replay_worlds.get(&key) {
            world.validate_sealed().map_err(|_| Error::ClockRegressed)?;
            let manifest = &world.manifest;
            require(
                manifest.world_id == member.world_id
                    && world.sealed_digest.as_ref() == Some(&member.world_digest)
                    && manifest.cluster_id == member.cluster_id
                    && manifest.partition == member.partition,
            )?;
            let compatibility = evo_core::replay::ReplayPoolCompatibilityV1 {
                purpose: manifest.purpose,
                generation_signature: manifest.generation_signature.clone(),
                approved_parent_digest: manifest.approved_parent_digest.clone(),
                model_digest: manifest.model_digest.clone(),
                tools_digest: manifest.tools_digest.clone(),
                scorer_digest: manifest.scorer_digest.clone(),
                guidance_digest: manifest.guidance_digest.clone(),
                repair_template_digest: manifest.repair_template_digest.clone(),
                revoke_watermark: manifest.revoke_watermark,
            };
            require(compatibility == pool.compatibility)?;
        } else {
            let (sealed, value) = snapshot
                .redacted_replay_worlds
                .get(&key)
                .ok_or(Error::ClockRegressed)?;
            require(*sealed && string(value, "sealed_digest")? == member.world_digest)?;
            let metadata = &value["metadata"];
            exact(metadata, &["world_id", "purpose", "revoke_watermark"], &[])?;
            require(
                string(metadata, "world_id")? == member.world_id
                    && decode::<Purpose>(&metadata["purpose"])? == pool.compatibility.purpose
                    && decode::<u64>(&metadata["revoke_watermark"])?
                        == pool.compatibility.revoke_watermark,
            )?;
        }
        edges.push(("replay_world".to_owned(), member.world_id.clone()));
    }
    edges.sort();
    require(e05_outgoing(snapshot, &object.namespace, &object.kind, &object.id)? == edges)
}
// Existing replay cleanup keeps typed reconciliation data, not replay authority.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedReplayPoolMetadata {
    record_kind: String,
    pool_digest: String,
    compatibility: evo_core::replay::ReplayPoolCompatibilityV1,
    members: Vec<evo_core::replay::ReplayPoolMemberV1>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedReplayUsage {
    world_id: String,
    world_digest: String,
    historical_usage: evo_core::replay::HistoricalUsage,
    probes: u32,
    simulated_rounds: u32,
    replay_cpu_nanos: u64,
    coverage: evo_core::replay::ReplayCoverage,
    terminal: evo_core::replay::ReplayTerminal,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RetainedReplayReportMetadata {
    record_kind: String,
    report_id: String,
    pool_digest: String,
    partition: evo_core::replay::WorldPartition,
    policy_digest: String,
    profile_digest: String,
    caps_digest: String,
    reports_body_digest: String,
    semantic_reports_digest: String,
    irreversible_usage: Vec<RetainedReplayUsage>,
    report_invalidated: bool,
}
fn retained_replay_metadata(object: &OccurrenceObject) -> Result<&Value> {
    let value = &object.body;
    exact(
        value,
        &[
            "id",
            "schema_version",
            "state",
            "original_kind",
            "original_schema",
            "original_digest",
            "metadata",
        ],
        &[],
    )?;
    require(
        object.kind == "artifact"
            && string(value, "id")? == object.id
            && string(value, "schema_version")? == "rsia.redacted.v1"
            && string(value, "state")? == "source_revoked"
            && string(value, "original_kind")? == "artifact"
            && string(value, "original_schema")? == crate::replay::REPLAY_STORE_ENVELOPE_SCHEMA,
    )?;
    // The original bytes were removed. Preserve the existing digest-format guard;
    // neither reconstruction of deleted bytes nor current replay authority is claimed.
    digest(string(value, "original_digest")?)?;
    Ok(&value["metadata"])
}
fn retained_replay_owner(
    object: &OccurrenceObject,
    snapshot: &OccurrenceSnapshot,
    operation: &str,
) -> Result<()> {
    require(snapshot.replay_owners.contains(&(
        object.namespace.clone(),
        operation.into(),
        object.id.clone(),
        object.owner.clone(),
    )))
}
fn qualified_retained_replay_pool(
    object: &OccurrenceObject,
    snapshot: &OccurrenceSnapshot,
) -> Result<evo_core::replay::ReplayPoolManifestV1> {
    let metadata: RetainedReplayPoolMetadata = decode(retained_replay_metadata(object)?)?;
    require(metadata.record_kind == crate::replay::REPLAY_POOL_RECORD_KIND)?;
    let pool = evo_core::replay::ReplayPoolManifestV1 {
        schema_version: evo_core::replay::REPLAY_POOL_SCHEMA.into(),
        compiler_version: evo_core::replay::REPLAY_POOL_COMPILER_VERSION.into(),
        pool_digest: metadata.pool_digest,
        compatibility: metadata.compatibility,
        members: metadata.members,
    };
    pool.validate_identity()
        .map_err(|_| Error::ClockRegressed)?;
    require(
        object.id
            == crate::replay::replay_pool_storage_id(&pool.pool_digest)
                .map_err(|_| Error::ClockRegressed)?,
    )?;
    retained_replay_owner(object, snapshot, "replay.pool.register")?;
    let mut expected: Vec<_> = pool
        .members
        .iter()
        .map(|member| ("replay_world".to_owned(), member.world_id.clone()))
        .collect();
    expected.sort();
    require(e05_outgoing(snapshot, &object.namespace, &object.kind, &object.id)? == expected)?;
    Ok(pool)
}
fn qualify_retained_replay(object: &OccurrenceObject, snapshot: &OccurrenceSnapshot) -> Result<()> {
    let metadata = retained_replay_metadata(object)?;
    match string(metadata, "record_kind")? {
        crate::replay::REPLAY_POOL_RECORD_KIND => {
            qualified_retained_replay_pool(object, snapshot).map(|_| ())
        }
        crate::replay::REPLAY_REPORT_RECORD_KIND => {
            let metadata: RetainedReplayReportMetadata = decode(metadata)?;
            require(
                metadata.record_kind == crate::replay::REPLAY_REPORT_RECORD_KIND
                    && metadata.report_invalidated
                    && metadata.report_id == object.id,
            )?;
            for value in [
                &metadata.pool_digest,
                &metadata.policy_digest,
                &metadata.profile_digest,
                &metadata.caps_digest,
                &metadata.reports_body_digest,
                &metadata.semantic_reports_digest,
            ] {
                digest(value)?;
            }
            let id = evo_core::replay::replay_report_id(
                &object.namespace,
                metadata.partition,
                &metadata.pool_digest,
                &metadata.policy_digest,
                &metadata.profile_digest,
                &metadata.caps_digest,
            )
            .map_err(|_| Error::ClockRegressed)?;
            require(id == object.id)?;
            retained_replay_owner(object, snapshot, "replay.report.store")?;
            // A cleanup page may already have redacted the pool. This read checks
            // its retained identity; ordinary replay consumers still reject it.
            let pool = replay_pool(snapshot, &object.namespace, &metadata.pool_digest)?;
            let expected: Vec<_> = pool
                .members
                .iter()
                .filter(|member| member.partition == metadata.partition)
                .collect();
            require(expected.len() == metadata.irreversible_usage.len())?;
            let mut edges = vec![(
                "artifact".to_owned(),
                crate::replay::replay_pool_storage_id(&metadata.pool_digest)
                    .map_err(|_| Error::ClockRegressed)?,
            )];
            for (usage, member) in metadata.irreversible_usage.iter().zip(expected) {
                require(
                    usage.world_id == member.world_id && usage.world_digest == member.world_digest,
                )?;
                edges.push(("replay_world".to_owned(), member.world_id.clone()));
            }
            edges.sort();
            require(e05_outgoing(snapshot, &object.namespace, &object.kind, &object.id)? == edges)
        }
        _ => Err(Error::ClockRegressed),
    }
}
fn qualify_replay_envelope(object: &OccurrenceObject, snapshot: &OccurrenceSnapshot) -> Result<()> {
    let value = &object.body;
    exact(
        value,
        &[
            "schema_version",
            "id",
            "namespace",
            "record_kind",
            "payload",
        ],
        &[],
    )?;
    require(
        object.kind == "artifact"
            && string(value, "id")? == object.id
            && string(value, "namespace")? == object.namespace,
    )?;
    match string(value, "record_kind")? {
        "replay_pool_manifest_v1" => {
            let pool: evo_core::replay::ReplayPoolManifestV1 = decode(&value["payload"])?;
            require(object.id == format!("replay-pool-{}", pool.pool_digest))?;
            replay_pool(snapshot, &object.namespace, &pool.pool_digest)?;
        }
        "stored_replay_report_v1" => {
            let report: evo_core::replay::StoredReplayReportV1 = decode(&value["payload"])?;
            require(report.report_id == object.id && report.namespace == object.namespace)?;
            let pool = replay_pool(snapshot, &object.namespace, &report.pool_digest)?;
            report
                .validate_static(&pool)
                .map_err(|_| Error::ClockRegressed)?;
        }
        _ => return Err(Error::ClockRegressed),
    }
    Ok(())
}

fn validate_e16_retained_payload(kind: &str, value: &Value) -> Result<()> {
    let fields: &[&str] = match kind {
        "staged_asset" => &[
            "asset_id",
            "baseline_digest",
            "candidate_ref",
            "compiler_version",
            "content_blob_digest",
            "content_bytes",
            "environment_digest",
            "local_digest",
            "manifest_digest",
            "package_kind",
            "package_schema_version",
            "publisher",
            "upstream_digest",
        ],
        "export_attempt" => &[
            "asset_id",
            "budget_ref",
            "completion_receipt_digest",
            "delivered_bytes",
            "delivery_audit_id",
            "delivery_kind",
            "destination_scope",
            "local_export_ref",
            "manifest_digest",
            "package_tree_digest",
            "projection_blob_bytes",
            "projection_blob_digest",
            "publisher",
        ],
        "delivery_audit" => &[
            "completion_receipt_digest",
            "delivered_bytes",
            "delivery_kind",
            "local_export_ref",
            "package_tree_digest",
            "projection_blob_digest",
        ],
        "seed_install" => &[
            "asset_id",
            "baseline_blob_digest",
            "baseline_digest",
            "environment_digest",
            "kind",
            "local_blob_digest",
            "local_digest",
            "publisher",
            "upstream_blob_digest",
            "upstream_digest",
        ],
        "source_selection" => &[],
        "import_source" => &[],
        "import_result" => &[],
        _ => return Err(Error::ClockRegressed),
    };
    exact(value, fields, &[])?;
    for field in fields {
        let entry = &value[*field];
        match *field {
            "content_bytes" | "projection_blob_bytes" | "delivered_bytes" => {
                require(entry.as_u64().is_some())?;
            }
            "candidate_ref" | "delivery_audit_id" => {
                if !entry.is_null() {
                    identifier(entry.as_str().ok_or(Error::ClockRegressed)?)
                        .map_err(|_| Error::ClockRegressed)?;
                }
            }
            "upstream_digest" | "upstream_blob_digest" | "completion_receipt_digest" => {
                if !entry.is_null() {
                    digest(entry.as_str().ok_or(Error::ClockRegressed)?)?;
                }
            }
            "budget_ref" => {
                if !entry.is_null() {
                    let _: MirrorE04BudgetRef = decode(entry)?;
                }
            }
            field if field.ends_with("_digest") => {
                digest(entry.as_str().ok_or(Error::ClockRegressed)?)?
            }
            _ => {
                require(entry.is_string())?;
            }
        }
    }
    Ok(())
}

fn decode_import_request(value: &Value) -> Result<MirrorImportRegistrationRequest> {
    let request: MirrorImportRegistrationRequest = decode(value)?;
    require(
        request.schema_version == "rsia.e16.import_registration.v1"
            && !request.sources.is_empty()
            && request.sources.len() <= evo_core::evidence::MAX_DISCOVERED_FILES,
    )?;
    identifier(&request.request_key).map_err(|_| Error::ClockRegressed)?;
    require(
        !request.allow_model_excerpts
            || (request.outbound_authorized
                && request.retention_scope
                    == MirrorImportRetentionScope::LocalWithAuthorizedExcerpts),
    )?;
    let selection = evo_core::evidence::SourceSelection {
        roots: request.roots.clone(),
        run_ids: request
            .sources
            .iter()
            .map(|source| source.source_id.clone())
            .collect(),
        purpose: request.purpose,
        allow_model_excerpts: request.allow_model_excerpts,
    };
    selection.validate().map_err(|_| Error::ClockRegressed)?;
    let mut seen = BTreeSet::new();
    for source in &request.sources {
        identifier(&source.source_id).map_err(|_| Error::ClockRegressed)?;
        digest(&source.expected_digest)?;
        require(seen.insert(&source.source_id))?;
        evo_core::evidence::assert_authorized_path(&source.path, &request.roots)
            .map_err(|_| Error::ClockRegressed)?;
    }
    Ok(request)
}
/// Pure validation of the actual original request. The caller scopes its
/// transient three-key wrapper to its real Context; this is never authority.
pub(super) fn validate_import_preimage(
    namespace: &str,
    owner_actor: &str,
    request: &Value,
) -> Result<()> {
    identifier(namespace).map_err(|_| Error::ClockRegressed)?;
    identifier(owner_actor).map_err(|_| Error::ClockRegressed)?;
    decode_import_request(request)?;
    Ok(())
}
fn pending_import_source(
    record: &MirrorImportEnvelope<MirrorImportSourcePayload>,
    snapshot: &OccurrenceSnapshot,
    inputs: &PreparePreimages,
) -> Result<bool> {
    for candidate in &inputs.import_requests {
        exact(candidate, &["namespace", "owner_actor", "request"], &[])?;
        let namespace = string(candidate, "namespace")?;
        let owner = string(candidate, "owner_actor")?;
        validate_import_preimage(namespace, owner, &candidate["request"])?;
        if namespace != record.namespace || owner != record.owner_actor {
            continue;
        }
        let request = decode_import_request(&candidate["request"])?;
        let selection_id = format!("e16sel-{}", &typed_fingerprint(&request)?[..24]);
        if selection_id != record.payload.selection_id || request.request_key != record.request_key
        {
            continue;
        }
        let Some(spec) = request
            .sources
            .iter()
            .find(|spec| spec.source_id == record.payload.logical_source_id)
        else {
            continue;
        };
        let expected_id = format!(
            "e16src-{}",
            &hash(format!("{}\n{}", selection_id, spec.source_id).as_bytes())[..24]
        );
        let current_watermark = snapshot
            .watermarks
            .get(namespace)
            .map_or(0, |value| value.0);
        require(
            record.id == expected_id
                && record.namespace == namespace
                && record.owner_actor == owner
                && record.request_key == request.request_key
                && record.input_digest
                    == typed_fingerprint(&(spec, spec.expected_digest.as_str()))?
                && record.updated_at == record.created_at
                && record.revoke_watermark == current_watermark
                && record.payload.authorized_path == spec.path
                && record.payload.reader == spec.reader
                && record.payload.purpose == request.purpose
                && record.payload.raw_digest == spec.expected_digest
                && record.payload.raw_blob_digest == spec.expected_digest
                && record.payload.status == MirrorImportSourceReadStatus::Prepared
                && record.payload.observed_digest.is_none()
                && !record.payload.blob_published
                && record.payload.byte_len == 0
                && !snapshot.objects.contains_key(&(
                    namespace.into(),
                    "tombstone".into(),
                    selection_id,
                ))
                && !snapshot.objects.contains_key(&(
                    namespace.into(),
                    "tombstone".into(),
                    record.id.clone(),
                )),
        )?;
        validate_import_source(record)?;
        return Ok(true);
    }
    Ok(false)
}

// Exact original compiler-node registry; time-free classification never broadens it.
fn compiler_node_schema(kind: &str, schema: &str) -> bool {
    match kind {
        "formal_parent" => schema == "rsia.formal_parent_selection.v1",
        "release" => matches!(
            schema,
            "rsia.persistent_release.v1" | "rsia.persistent_release.v2"
        ),
        "pointer" => schema == "rsia.profile_pointer.v1",
        "receipt" => schema == "rsia.host_application.v1",
        "artifact" => matches!(
            schema,
            "rsia.frozen_compiler_profile.v1"
                | "rsia.empty_root_authorization.v1"
                | "rsia.issued_formal_authority.v2"
                | "rsia.issued_formal_authority.v3"
                | "rsia.independent_grade_capture.v2"
                | "rsia.registered_formal_host.v1"
                | "rsia.registered_external_provider.v1"
                | "rsia.registered_external_provider.v2"
                | "rsia.registered_resource_instrumentation.v1"
                | "rsia.observed_principal_action.v1"
                | "rsia.observed_control_principal_action.v2"
                | "rsia.release_action.v2"
                | "rsia.formal_principal_set_observation.v1"
                | "rsia.formal_profile_selection.v1"
                | "rsia.persisted_compiler_binding.v1"
                | "rsia.authenticated_bundle.v1"
                | "rsia.actual_formal_provider_capture.v1"
                | "rsia.independent_grade_capture.v1"
                | "rsia.complete_episode_cost.v1"
                | "rsia.trusted_resource_meter.v1"
                | "rsia.host_surface_record.v1"
                | "rsia.run_application.v1"
                | "rsia.host_execution_receipt.v1"
                | "rsia.exploration_artifact_envelope.v1"
                | "rsia.optimization.stage_fact.v1"
                | "rsia.optimization.compiler_use.v1"
                | "rsia.monitoring.environment.v1"
                | "rsia.monitoring.observation.v1"
                | "rsia.monitoring.development_cycle.v2"
                | "rsia.monitoring.development_report_binding.v1"
                | "rsia.monitoring.consolidation_scope.v1"
                | "rsia.monitoring.consolidation_claim.v1"
                | "rsia.monitoring.consolidation_proposal.v1"
                | "rsia.monitoring.consolidation_staging.v1"
                | "rsia.monitoring.consolidation_run.v1"
                | "rsia.monitoring.environment_drift.v1"
                | "rsia.legacy_source_selection.v1"
        ),
        _ => false,
    }
}

fn expected_autoindexes(prefix: usize, ledger: bool) -> BTreeMap<String, String> {
    // These are the exact PRIMARY KEY/UNIQUE indexes of the fixed compiled
    // migrations. INTEGER PRIMARY KEY and WITHOUT ROWID primary keys have no
    // separate sqlite_schema index entry. No sqlite_autoindex_* wildcard is used.
    let mut indexes = BTreeMap::new();
    let mut insert = |table: &str, count: u8| {
        for n in 1..=count {
            indexes.insert(format!("sqlite_autoindex_{table}_{n}"), table.to_owned());
        }
    };
    if ledger {
        insert("_sqlx_migrations", 1);
    }
    if prefix >= 1 {
        insert("objects", 1);
        insert("idempotency", 1);
    }
    if prefix >= 2 {
        insert("dependencies", 1);
        insert("revoke_watermark", 1);
    }
    if prefix >= 3 {
        insert("replay_worlds", 1);
    }
    if prefix >= 4 {
        insert("root_budgets", 2);
        insert("root_budget_namespaces", 1);
        insert("root_budget_dispatch_groups", 1);
        insert("root_budget_calls", 3);
    }
    if prefix >= 5 {
        insert("revoke_cleanup_jobs", 2);
        insert("revoke_cleanup_frontier", 1);
    }
    indexes
}

// Additional exact current M29 time-free producer wires. No inferred timestamp fields.
use evo_core::contract::Profile;
use evo_core::curriculum::ProposalAttemptOutcome;
use evo_core::strategy::{ConsolidationClass, classify_consolidation_pair};

// M29 monitoring.rs:91 EnvironmentIdentityRecord
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorEnvironmentIdentityRecord {
    id: String,
    schema_version: String,
    identity_digest: String,
    namespace: String,
    evidence_scope: MirrorEnvironmentEvidenceScope,
    profile_id: String,
    release_id: Option<String>,
    bundle_digest: Option<String>,
    environment_digest: String,
    model_identity: String,
    ordered_tools: Vec<String>,
    host_id: String,
    host_version: String,
    host_surface_digest: String,
    host_capabilities_digest: String,
    mandatory_context_digest: String,
    development_manifest_digest: String,
    grader_digest: String,
    generation_strategy_digest: String,
    revoke_watermark: u64,
}

// M29 monitoring.rs:116 EnvironmentEvidenceScope
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorEnvironmentEvidenceScope {
    TrustedExecution,
    ProgramFixture,
}

// M29 monitoring.rs:123 ApplicationStatus
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorApplicationStatus {
    NotSelected,
    OfferedNotAttached,
    AttachedNotUsed,
    Used,
    Unknown,
}

// M29 monitoring.rs:133 QualityStatus
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorQualityStatus {
    Unknown,
}

// M29 monitoring.rs:139 DeploymentObservation
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorDeploymentObservation {
    id: String,
    schema_version: String,
    run_id: String,
    environment_id: String,
    snapshot_id: String,
    application_id: String,
    request_digest: String,
    execution_receipt_id: Option<String>,
    application_status: MirrorApplicationStatus,
    quality_status: MirrorQualityStatus,
}

// M29 monitoring.rs:169 SourceDependency
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorSourceDependency {
    id: String,
    content_digest: String,
}

// M29 monitoring.rs:181 TaskPairV1
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorTaskPairV1 {
    task_id: String,
    parent_passed: bool,
    candidate_passed: bool,
    class: ConsolidationClass,
    parent_score_micros: u32,
    candidate_score_micros: u32,
}

// M29 monitoring.rs:192 ConsolidationScope
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorConsolidationScope {
    namespace: String,
    skill_id: String,
    profile_id: String,
    development_manifest_digest: String,
    environment_id: String,
    environment_identity_digest: String,
    environment_digest: String,
    grader_digest: String,
    generation_strategy_digest: String,
    parent_skill_digest: String,
    parent_bundle_digest: String,
    billing_scope: String,
    root_budget_id: String,
    revoke_watermark: u64,
}

// M29 monitoring.rs:211 DevelopmentCycleRecord
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorDevelopmentCycleRecord {
    id: String,
    schema_version: String,
    scope_id: String,
    ordinal: u64,
    report_fact_id: String,
    report_digest: String,
    request_id: String,
    candidate_bundle_digest: String,
    execution_receipt_id: String,
    usage_record_ids: Vec<String>,
    provenance: MirrorDevelopmentExecutionProvenance,
    /// One entry per reported task, in report order (`v1` kept a score-biased
    /// `pair_classes` list without task ids instead).
    pairs: Vec<MirrorTaskPairV1>,
    has_contrast: bool,
    sources: Vec<MirrorSourceDependency>,
}

// M29 monitoring.rs:232 DevelopmentReportBinding
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorDevelopmentReportBinding {
    schema_version: String,
    id: String,
    report_fact_id: String,
    scope_id: String,
    cycle_id: String,
}

// M29 monitoring.rs:242 ConsolidationScopeIndex
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorConsolidationScopeIndex {
    id: String,
    schema_version: String,
    scope: MirrorConsolidationScope,
    cycle_ids: Vec<String>,
    report_fact_ids: Vec<String>,
    claim_ids: BTreeMap<u64, String>,
    invalidated_by: Option<String>,
}

// M29 monitoring.rs:271 ConsolidationClaimState
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorConsolidationClaimState {
    Claimed,
    NoContrast,
    Running,
    CompletedNoChange,
    CompletedCandidate,
    CompletedRejected,
    CompletedUncertain,
    CompletedRevoked,
    BlockedBudget,
}

// M29 monitoring.rs:285 ConsolidationClaim
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorConsolidationClaim {
    id: String,
    schema_version: String,
    scope_id: String,
    generation: u64,
    cycle_ids: Vec<String>,
    sources: Vec<MirrorSourceDependency>,
    revoke_watermark: u64,
    state: MirrorConsolidationClaimState,
    execution_input_digest: Option<String>,
}

// M29 monitoring.rs:333 ConsolidationRunOutcome
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorConsolidationRunOutcome {
    NoChange,
    Candidate,
    Rejected,
    Uncertain,
    Revoked,
    BlockedBudget,
}

// M29 monitoring.rs:344 ConsolidationRunRecord
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorConsolidationRunRecord {
    id: String,
    schema_version: String,
    claim_id: String,
    input_digest: Option<String>,
    outcome: MirrorConsolidationRunOutcome,
    /// The bounded terminal category of the outcome: [`terminal_category`] of its
    /// reason. Since the classes of the optimization step (`StepTerminalClass`) the
    /// reason of a step outcome is always a fixed code, so the category is that code:
    /// no text of an error, a model or a source is kept here. A record written before
    /// the classes holds the bounded prefix of the free text its step reported, and is
    /// read as it is.
    reason: String,
    budget_call_ids: Vec<String>,
    /// Set exactly when `outcome` is `Candidate`: the durable proposal that was
    /// written before this terminal record. Records written before proposals
    /// existed deserialize as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    proposal_id: Option<String>,
}

// M29 monitoring.rs:422 ConsolidationProposal
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorConsolidationProposal {
    id: String,
    schema_version: String,
    claim_id: String,
    scope_id: String,
    cycle_ids: Vec<String>,
    /// Fingerprint of the per-task pairings of the claim's two cycles.
    pairs_digest: String,
    parent_skill_digest: String,
    parent_bundle_digest: String,
    candidate_skill_digest: String,
    candidate_bundle: ResolvedBundle,
    candidate_bundle_digest: String,
    selection_digest: String,
    /// The optimization journal's terminal `StepCompleted` fact this candidate
    /// was read from; it also holds the edit, patch and report.
    terminal_fact_id: String,
    sources: Vec<MirrorSourceDependency>,
    /// `ProgramFixture` when the environment or any backing cycle is fixture
    /// evidence. Fixture proposals may be staged; formal approval still refuses
    /// fixture evidence (`ReleaseStore::approve_verified`).
    provenance: MirrorEnvironmentEvidenceScope,
    environment_digest: String,
    revoke_watermark: u64,
}

// M29 monitoring.rs:465 ConsolidationStagingBinding
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorConsolidationStagingBinding {
    /// `consolidation-staging-{proposal id}`: one binding per proposal.
    id: String,
    schema_version: String,
    proposal_id: String,
    candidate_id: String,
}

// M29 monitoring.rs:475 EnvironmentDriftRecord
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorEnvironmentDriftRecord {
    id: String,
    schema_version: String,
    previous_environment_id: String,
    current_environment_id: String,
    previous_identity_digest: String,
    current_identity_digest: String,
    invalidated_scope_id: String,
    reason: String,
}

// M29 curriculum.rs:183 ProbeScheduleReceiptV1
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorProbeScheduleReceiptV1 {
    schema_version: String,
    id: String,
    idempotency_key: String,
    profile_id: String,
    state_id: String,
    root_budget_limit_micros: u64,
    probe_job_id: String,
    trigger_digest: String,
    revoke_watermark: u64,
}

// M29 curriculum.rs:210 CurriculumSourceKindV1
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorCurriculumSourceKindV1 {
    TaskSpace,
    TargetSpec,
    OracleSpec,
    RunnerSpec,
    DevelopmentCycleExecution,
    ProposalDiagnostic,
}

// M29 curriculum.rs:221 CurriculumSourceArtifactV1
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorCurriculumSourceArtifactV1 {
    schema_version: String,
    id: String,
    source_kind: MirrorCurriculumSourceKindV1,
    data_use: DataUse,
    subject_digest: String,
    body_digest: String,
    body: serde_json::Value,
    dependency_ids: Vec<String>,
}

// M29 curriculum.rs:261 DevelopmentCycleReceiptV1
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorDevelopmentCycleReceiptV1 {
    schema_version: String,
    id: String,
    state_id: String,
    development_request_fact_id: String,
    development_observed_fact_id: String,
}

// M29 curriculum.rs:271 ProposalAttemptReceiptV1
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorProposalAttemptReceiptV1 {
    schema_version: String,
    id: String,
    job_id: String,
    outcome: ProposalAttemptOutcome,
    source_artifact_ids: Vec<String>,
}

// M29 curriculum.rs:292 TaskSelectionDecisionV1
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorTaskSelectionDecisionV1 {
    schema_version: String,
    id: String,
    state_id: String,
    state_digest: String,
    chosen_task_id: String,
    proposal_id: String,
    stable_choice_seq: u32,
    reason: String,
    budget_status: String,
}

// M29 curriculum_profiles.rs:263 ValidityReportV1
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", deny_unknown_fields)]
enum MirrorValidityReportV1 {
    Quarantined {
        profile_id: String,
        proposal_id: String,
        proposal_digest: String,
        target_digest: String,
        oracle_digest: String,
        runner_digest: String,
        reason: String,
    },
    SandboxUnavailable {
        profile_id: String,
        proposal_id: String,
        proposal_digest: String,
        target_digest: String,
        oracle_digest: String,
        runner_digest: String,
    },
}

// M29 replay_experiment.rs:77 ReplayEconomicJobState
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MirrorReplayEconomicJobState {
    Registered,
    BlockedSupport,
    UsageUncertain,
    Cancelled,
}

// M29 replay_experiment.rs:86 ReplayEconomicJobV1
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorReplayEconomicJobV1 {
    schema_version: String,
    id: String,
    experiment_id: String,
    request_key: String,
    request_digest: String,
    paired_ticket_id: String,
    state: MirrorReplayEconomicJobState,
    blocked_reasons: Vec<String>,
    cost_receipt_ids: Vec<String>,
    report_id: Option<String>,
    cancel_reason: Option<String>,
    created_seq: u64,
}

// M29 formal_authority.rs:2885 CompilerInputRequest
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorCompilerInputRequest {
    profile: Profile,
    /// Search parent is distinct from the approved control-plane P.
    search_parent: SkillSnapshot,
    baseline: SkillSnapshot,
    /// Generation strategy of the search parent, not authority for approved P.
    search_parent_strategy: Strategy,
    expected_parent_strategy_digest: Option<String>,
    baseline_strategy: Strategy,
    capabilities: HostCapabilities,
    environment_digest: String,
    purpose: Purpose,
    revoke_watermark: u64,
}

// M29 model.rs:195 CompilerInputUse
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorCompilerInputUse {
    schema_version: String,
    request: MirrorCompilerInputRequest,
    binding: MirrorFormalCompilerInputBinding,
}

// M29 compiler.rs:23 AuthenticatedBundleRecord
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorAuthenticatedBundleRecord {
    id: String,
    schema_version: String,
    bundle: ResolvedBundle,
    input: MirrorCompilerInputRequest,
    binding: MirrorFormalCompilerInputBinding,
}

fn monitoring_schema(schema: &str) -> bool {
    matches!(
        schema,
        "rsia.monitoring.environment.v1"
            | "rsia.monitoring.observation.v1"
            | "rsia.monitoring.development_cycle.v2"
            | "rsia.monitoring.development_report_binding.v1"
            | "rsia.monitoring.consolidation_scope.v1"
            | "rsia.monitoring.consolidation_claim.v1"
            | "rsia.monitoring.consolidation_proposal.v1"
            | "rsia.monitoring.consolidation_staging.v1"
            | "rsia.monitoring.consolidation_run.v1"
            | "rsia.monitoring.environment_drift.v1"
    )
}
fn qualify_monitoring(object: &OccurrenceObject) -> Result<()> {
    let value = &object.body;
    require(object.kind == "artifact" && string(value, "id")? == object.id)?;
    macro_rules! wire {
        ($ty:ty) => {{
            let _: $ty = decode(value)?;
        }};
    }
    match string(value, "schema_version")? {
        "rsia.monitoring.environment.v1" => {
            let record: MirrorEnvironmentIdentityRecord = decode(value)?;
            require(
                record.namespace == object.namespace
                    && record.id
                        == format!(
                            "monitor-env-{}",
                            &record.identity_digest[..32.min(record.identity_digest.len())]
                        ),
            )?;
            for d in [&record.identity_digest, &record.environment_digest] {
                digest(d)?;
            }
        }
        "rsia.monitoring.observation.v1" => wire!(MirrorDeploymentObservation),
        "rsia.monitoring.development_cycle.v2" => {
            let cycle: MirrorDevelopmentCycleRecord = decode(value)?;
            require(
                !cycle.pairs.is_empty()
                    && cycle.id
                        == format!(
                            "development-cycle-{}",
                            &typed_fingerprint(&(&cycle.scope_id, &cycle.report_fact_id))?[..32]
                        ),
            )?;
            let mut seen = BTreeSet::new();
            let mut contrast = false;
            for pair in &cycle.pairs {
                identifier(&pair.task_id).map_err(|_| Error::ClockRegressed)?;
                require(
                    seen.insert(&pair.task_id)
                        && pair.parent_score_micros <= 1000000
                        && pair.candidate_score_micros <= 1000000
                        && pair.class
                            == classify_consolidation_pair(
                                pair.parent_passed,
                                pair.candidate_passed,
                            ),
                )?;
                contrast |= pair.parent_passed != pair.candidate_passed
                    || pair.parent_score_micros != pair.candidate_score_micros;
            }
            require(contrast == cycle.has_contrast)?;
        }
        "rsia.monitoring.development_report_binding.v1" => wire!(MirrorDevelopmentReportBinding),
        "rsia.monitoring.consolidation_scope.v1" => {
            let record: MirrorConsolidationScopeIndex = decode(value)?;
            require(
                record.scope.namespace == object.namespace
                    && record.id
                        == format!(
                            "consolidation-scope-{}",
                            &typed_fingerprint(&record.scope)?[..32]
                        )
                    && record.cycle_ids.len() == record.report_fact_ids.len(),
            )?;
        }
        "rsia.monitoring.consolidation_claim.v1" => {
            let record: MirrorConsolidationClaim = decode(value)?;
            require(
                record.id
                    == format!(
                        "consolidation-claim-{}",
                        &typed_fingerprint(&(&record.scope_id, record.generation))?[..32]
                    ),
            )?;
        }
        "rsia.monitoring.consolidation_proposal.v1" => {
            let record: MirrorConsolidationProposal = decode(value)?;
            validate_bundle(&record.candidate_bundle)?;
            require(
                record.id == format!("consolidation-proposal-{}", record.claim_id)
                    && record.candidate_bundle.digest == record.candidate_bundle_digest
                    && record.candidate_skill_digest
                        == evo_core::skill_edit::skill_snapshot_digest(
                            &record.candidate_bundle.skill,
                        )
                        .map_err(|_| Error::ClockRegressed)?,
            )?;
            for d in [
                &record.pairs_digest,
                &record.parent_skill_digest,
                &record.parent_bundle_digest,
                &record.candidate_skill_digest,
                &record.candidate_bundle_digest,
                &record.selection_digest,
                &record.environment_digest,
            ] {
                digest(d)?;
            }
            identifier(&record.terminal_fact_id).map_err(|_| Error::ClockRegressed)?;
        }
        "rsia.monitoring.consolidation_staging.v1" => {
            let record: MirrorConsolidationStagingBinding = decode(value)?;
            require(record.id == format!("consolidation-staging-{}", record.proposal_id))?;
        }
        "rsia.monitoring.consolidation_run.v1" => {
            let record: MirrorConsolidationRunRecord = decode(value)?;
            require(record.id == format!("consolidation-run-{}", record.claim_id))?;
        }
        "rsia.monitoring.environment_drift.v1" => wire!(MirrorEnvironmentDriftRecord),
        _ => return Err(Error::ClockRegressed),
    }
    Ok(())
}
fn qualify_curriculum_source(object: &OccurrenceObject) -> Result<()> {
    let record: MirrorCurriculumSourceArtifactV1 = decode(&object.body)?;
    require(
        object.kind == "artifact"
            && record.id == object.id
            && record.schema_version == "rsia.curriculum_source.v1"
            && record.data_use == DataUse::Development
            && !record.body.is_null()
            && record.body_digest == typed_fingerprint(&record.body)?,
    )?;
    digest(&record.subject_digest)?;
    digest(&record.body_digest)?;
    let mut seen = BTreeSet::new();
    for id in &record.dependency_ids {
        identifier(id).map_err(|_| Error::ClockRegressed)?;
        require(id != &record.id && seen.insert(id))?;
    }
    // Task/native/provider body keys remain opaque; no recursive date scan.
    Ok(())
}
fn curriculum_id(kind: &str, id: &str) -> Result<String> {
    Ok(format!("e12-{}", typed_fingerprint(&(kind, id))?))
}
fn qualify_curriculum(object: &OccurrenceObject, snapshot: &OccurrenceSnapshot) -> Result<()> {
    let kind = string(&object.body, "record_kind")?;
    let payload = envelope(object, "rsia.curriculum_artifact_envelope.v1", kind)?;
    let logical = match kind {
        "curriculum_profile_v1" => {
            let record: evo_core::curriculum::CurriculumControlProfileV1 = decode(payload)?;
            record.validate().map_err(|_| Error::ClockRegressed)?;
            record.profile_id
        }
        "learner_state_v2" => {
            let record: evo_core::curriculum::LearnerStateV2 = decode(payload)?;
            record.validate().map_err(|_| Error::ClockRegressed)?;
            record.id
        }
        "development_cycle_receipt_v1" => {
            let record: MirrorDevelopmentCycleReceiptV1 = decode(payload)?;
            require(record.schema_version == "rsia.development_cycle_receipt.v1")?;
            record.id
        }
        "coverage_probe_job_v1" => {
            let record: evo_core::curriculum::ProbeJobV1 = decode(payload)?;
            require(
                record.schema_version == "rsia.coverage_probe_job.v1"
                    && record.effective_monetary_limit_micros == 0
                    && record.provider_dispatch_count == 0,
            )?;
            let profile = snapshot.object(
                &object.namespace,
                "artifact",
                &curriculum_id("curriculum_profile_v1", &record.profile_id)?,
            )?;
            let profile: evo_core::curriculum::CurriculumControlProfileV1 = decode(envelope(
                profile,
                "rsia.curriculum_artifact_envelope.v1",
                "curriculum_profile_v1",
            )?)?;
            profile.validate().map_err(|_| Error::ClockRegressed)?;
            require(
                record.curriculum_share_limit_micros
                    == profile
                        .curriculum_limit(record.root_budget_limit_micros)
                        .map_err(|_| Error::ClockRegressed)?
                    && record.attempts.len() <= usize::from(profile.max_total_proposals),
            )?;
            record.id
        }
        "curriculum_proposal_attempt_v1" => {
            let record: MirrorProposalAttemptReceiptV1 = decode(payload)?;
            require(
                record.schema_version == "rsia.curriculum_proposal_attempt.v1"
                    && !record.source_artifact_ids.is_empty()
                    && record.outcome != ProposalAttemptOutcome::Valid,
            )?;
            record.id
        }
        "structured_test_proposal_v1" => {
            let record: evo_core::curriculum::StructuredTestProposalV1 = decode(payload)?;
            record.validate().map_err(|_| Error::ClockRegressed)?;
            record.id
        }
        "curriculum_validity_report_v1" => {
            let record: MirrorValidityReportV1 = decode(payload)?;
            let (profile, proposal, hashes) = match &record {
                MirrorValidityReportV1::Quarantined {
                    profile_id,
                    proposal_id,
                    proposal_digest,
                    target_digest,
                    oracle_digest,
                    runner_digest,
                    ..
                }
                | MirrorValidityReportV1::SandboxUnavailable {
                    profile_id,
                    proposal_id,
                    proposal_digest,
                    target_digest,
                    oracle_digest,
                    runner_digest,
                } => (
                    profile_id,
                    proposal_id,
                    [proposal_digest, target_digest, oracle_digest, runner_digest],
                ),
            };
            identifier(profile).map_err(|_| Error::ClockRegressed)?;
            for d in hashes {
                digest(d)?;
            }
            let proposal_row = snapshot.object(
                &object.namespace,
                "artifact",
                &curriculum_id("structured_test_proposal_v1", proposal)?,
            )?;
            let proposal_body: evo_core::curriculum::StructuredTestProposalV1 = decode(envelope(
                proposal_row,
                "rsia.curriculum_artifact_envelope.v1",
                "structured_test_proposal_v1",
            )?)?;
            proposal_body
                .validate()
                .map_err(|_| Error::ClockRegressed)?;
            let job_row = snapshot.object(
                &object.namespace,
                "artifact",
                &curriculum_id("coverage_probe_job_v1", &proposal_body.probe_job_id)?,
            )?;
            let job: evo_core::curriculum::ProbeJobV1 = decode(envelope(
                job_row,
                "rsia.curriculum_artifact_envelope.v1",
                "coverage_probe_job_v1",
            )?)?;
            let profile_row = snapshot.object(
                &object.namespace,
                "artifact",
                &curriculum_id("curriculum_profile_v1", profile)?,
            )?;
            let control: evo_core::curriculum::CurriculumControlProfileV1 = decode(envelope(
                profile_row,
                "rsia.curriculum_artifact_envelope.v1",
                "curriculum_profile_v1",
            )?)?;
            control.validate().map_err(|_| Error::ClockRegressed)?;
            let registered = MirrorRegisteredPureFunctionProfileV1::clamp_i64();
            require(
                job.profile_id == *profile
                    && hashes[0] == &typed_fingerprint(&proposal_body)?
                    && hashes[1] == &registered.target_digest
                    && hashes[2] == &control.oracle_digest
                    && hashes[3] == &control.runner_digest
                    && control.oracle_digest == registered.oracle_digest
                    && control.runner_digest == registered.runner_digest,
            )?;
            format!(
                "validity-{}",
                &typed_fingerprint(&(&job.state_id, proposal, &record))?[..24]
            )
        }
        "curriculum_selection_v1" => {
            let record: MirrorTaskSelectionDecisionV1 = decode(payload)?;
            require(record.schema_version == "rsia.curriculum_selection.v1")?;
            record.id
        }
        "probe_schedule_receipt_v1" => {
            let record: MirrorProbeScheduleReceiptV1 = decode(payload)?;
            require(
                record.schema_version == "rsia.probe_schedule_receipt.v1"
                    && record.id
                        == format!(
                            "probe-schedule-{}",
                            &typed_fingerprint(&(
                                "probe_schedule_receipt_v1",
                                &record.idempotency_key
                            ))?[..24]
                        ),
            )?;
            record.id
        }
        _ => return Err(Error::ClockRegressed),
    };
    require(object.id == curriculum_id(kind, &logical)?)
}
fn economic_id(kind: &str, id: &str) -> Result<String> {
    Ok(format!("e11-{}", typed_fingerprint(&(kind, id))?))
}
fn economic_experiment(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    id: &str,
) -> Result<evo_core::replay_economics::ReplayEconomicExperimentV1> {
    let row = snapshot.object(
        namespace,
        "artifact",
        &economic_id("replay_economic_experiment_v1", id)?,
    )?;
    let record: evo_core::replay_economics::ReplayEconomicExperimentV1 = decode(envelope(
        row,
        "rsia.replay_economic_artifact_envelope.v1",
        "replay_economic_experiment_v1",
    )?)?;
    record.validate().map_err(|_| Error::ClockRegressed)?;
    require(record.id == id)?;
    Ok(record)
}
fn qualify_economic(object: &OccurrenceObject, snapshot: &OccurrenceSnapshot) -> Result<()> {
    let kind = string(&object.body, "record_kind")?;
    let payload = envelope(object, "rsia.replay_economic_artifact_envelope.v1", kind)?;
    let logical = match kind {
        "replay_economic_experiment_v1" => {
            let record: evo_core::replay_economics::ReplayEconomicExperimentV1 = decode(payload)?;
            record.validate().map_err(|_| Error::ClockRegressed)?;
            record.id
        }
        "replay_economic_job_v1" => {
            let record: MirrorReplayEconomicJobV1 = decode(payload)?;
            require(record.schema_version == "rsia.replay_economic_job.v1")?;
            record.id
        }
        "replay_economic_cost_receipt_v1" => {
            let record: evo_core::replay_economics::CostComponentReceiptV1 = decode(payload)?;
            record.validate().map_err(|_| Error::ClockRegressed)?;
            record.id
        }
        "replay_economic_report_v1" => {
            let record: evo_core::replay_economics::ReplayEconomicReportV1 = decode(payload)?;
            let experiment =
                economic_experiment(snapshot, &object.namespace, &record.experiment_id)?;
            record
                .validate_against(&experiment)
                .map_err(|_| Error::ClockRegressed)?;
            record.id
        }
        _ => return Err(Error::ClockRegressed),
    };
    require(object.id == economic_id(kind, &logical)?)
}

// Source checking is distinct from semantic-version admission. This private
// result is never a current_verified capability or an externally accepted report.
enum E05CompleteBatchRead {
    LegacyUnverified,
    SourceCheckedVersionPending,
}

fn e05_complete_changed() -> Error {
    Error::Conflict("formal_complete_batch_statistics_changed".into())
}

fn e05_complete_group<'a>(
    snapshot: &'a OccurrenceSnapshot,
    namespace: &str,
    billing_scope: &str,
    ticket_id: &str,
) -> Result<&'a DispatchGroupRecord> {
    let group = snapshot
        .groups
        .get(&(billing_scope.into(), ticket_id.into()))
        .ok_or_else(|| {
            Error::Invalid("source_insufficient: complete dispatch group missing".into())
        })?;
    require(
        group.billing_scope == billing_scope
            && group.dispatch_group_id == ticket_id
            && group.owner_namespace == namespace
            && group.stopped
            && group
                .stop_reason
                .as_ref()
                .is_some_and(|reason| !reason.is_empty())
            && group.stop_committed_at.is_some(),
    )?;
    // stop_dispatch_group preserves the first stop reason. A healthy operator
    // stop, closed root, expired lease or late financial settlement is not banned.
    Ok(group)
}

fn e05_complete_ticket_identity(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
    control: &MirrorRegisteredEvaluationControl,
    holdout: &MirrorProtectedHoldoutRecord,
) -> Result<()> {
    ticket.validate()?;
    control.validate()?;
    holdout.validate_against(control)?;
    let (_, issue) = e05_issue_receipt(snapshot, namespace, ticket)?;
    let mut bound = control.v1_plan_snapshot.clone();
    bound.bind_candidate(ticket.candidate_digest.clone())?;
    require(
        ticket.registration_id == control.id
            && ticket.holdout_id == holdout.id
            && ticket.registration_digest == typed_fingerprint(control)?
            && ticket.manifest_digest == holdout.manifest.digest()?
            && ticket.v1_plan_snapshot_digest == control.v1_plan_snapshot.digest()?
            && ticket.bound_v1_plan_digest == bound.digest()?
            && ticket.formal_plan_digest == control.formal_plan.digest()?
            && ticket.alpha_plan_digest == control.alpha_plan.digest()?
            && ticket.research_family_id == control.alpha_plan.research_family_id
            && ticket.attempt_id == control.attempt_id
            && ticket.profile == control.formal_plan.profile
            && ticket.n_planned == control.formal_plan.n_planned
            && ticket.candidate_digest == holdout.pair.candidate_digest
            && ticket.baseline_digest == holdout.pair.baseline_digest
            && ticket.candidate_bundle_digest == holdout.candidate_bundle_digest
            && ticket.baseline_bundle_digest == holdout.baseline_bundle_digest
            && ticket.environment_digest == holdout.manifest.environment_digest
            && ticket.proposer_actor == control.proposer_actor
            && ticket.evaluator_actor == control.evaluator_actor
            && ticket.executor_actor == control.executor_actor
            && ticket.approver_actor == control.approver_actor
            && ticket.evidence_scope == control.evidence_scope
            && issue.ticket_id == ticket.id
            && issue.registration_digest == ticket.registration_digest
            && issue.manifest_digest == ticket.manifest_digest
            && issue.evaluator_actor == ticket.evaluator_actor
            && issue.issued_at_unix_seconds == ticket.issued_at_unix_seconds
            && ticket.issue_receipt_digest == issue.actual_digest,
    )?;
    let inputs = holdout
        .rotating_inputs
        .iter()
        .map(|input| (input, false))
        .chain(holdout.anchor_inputs.iter().map(|input| (input, true)))
        .collect::<Vec<_>>();
    require(
        ticket.targets.len() == inputs.len()
            && ticket
                .targets
                .iter()
                .zip(inputs)
                .all(|(target, (input, anchor))| {
                    target.target_id == input.target_id
                        && target.input_artifact_id == input.input_artifact_id
                        && target.input_digest == input.input_digest
                        && match &target.kind {
                            MirrorEvaluationTargetKind::Rotating { cluster_id } => {
                                !anchor && cluster_id == &input.cluster_or_anchor_id
                            }
                            MirrorEvaluationTargetKind::Anchor { anchor_id } => {
                                anchor && anchor_id == &input.cluster_or_anchor_id
                            }
                        }
                }),
    )
}

fn e05_complete_p95(values: &mut [u64]) -> Result<u64> {
    if values.is_empty() {
        return Err(Error::Invalid("p95 requires observations".into()));
    }
    values.sort_unstable();
    let rank = 95usize
        .checked_mul(values.len())
        .ok_or_else(|| Error::Invalid("p95 rank overflow".into()))?
        .div_ceil(100);
    Ok(values[rank.saturating_sub(1)])
}

fn e05_complete_wire(
    report: &evo_core::evaluation::BernsteinReport,
) -> Result<MirrorBernsteinReportWire> {
    use evo_core::formal_complete_statistics::finite_decimal;
    Ok(MirrorBernsteinReportWire {
        stats_version: report.stats_version.clone(),
        verdict: report.verdict,
        n: report.n,
        mean: finite_decimal(report.mean)?,
        variance: finite_decimal(report.variance)?,
        radius: finite_decimal(report.radius)?,
        lcb: finite_decimal(report.lcb)?,
        alpha_i: finite_decimal(report.alpha_i)?,
        reasons: report.reasons.clone(),
    })
}

// Private record interface shared by the original actual-snapshot reducer and
// the authenticated frozen reader. No implementation can be supplied on wire.
trait E05CompleteRecordsR39 {
    fn record<T: DeserializeOwned>(&self, namespace: &str, kind: &str, id: &str) -> Result<T>;
    fn exposure(
        &self,
        namespace: &str,
        ticket: &MirrorEvaluationTicketV2,
    ) -> Result<evo_core::formal_version::ExposureProjectionV2>;
}
struct ActualCompleteRecordsR39<'a>(&'a OccurrenceSnapshot);
impl E05CompleteRecordsR39 for ActualCompleteRecordsR39<'_> {
    fn record<T: DeserializeOwned>(&self, namespace: &str, kind: &str, id: &str) -> Result<T> {
        evaluation_record(self.0, namespace, kind, id)
    }
    fn exposure(
        &self,
        namespace: &str,
        ticket: &MirrorEvaluationTicketV2,
    ) -> Result<evo_core::formal_version::ExposureProjectionV2> {
        let ledger: ExposureLedger = evaluation_record(
            self.0,
            namespace,
            "exposure_ledger_v1",
            &ticket.research_family_id,
        )?;
        ledger.validate_restore()?;
        let entry = ledger
            .entries
            .iter()
            .find(|entry| entry.ticket_id == ticket.id)
            .ok_or_else(e05_complete_changed)?
            .clone();
        Ok(evo_core::formal_version::ExposureProjectionV2 {
            schema_version: ledger.schema_version,
            research_family_id: ledger.research_family_id,
            alpha_plan_digest: ledger.alpha_plan_digest,
            query_limit: ledger.query_limit,
            entry,
        })
    }
}

// Mathematical input only: not a reconstructed BudgetCallRecord, not a full32
// fingerprint claim, not a retained artifact body and never a native permit.
struct E05MathCallR39 {
    billing_scope: String,
    call_id: String,
    dispatch_group_id: String,
    namespace: String,
    stage: BudgetStage,
    actual_input_digest: String,
    state: BudgetCallState,
    output_digest: Option<String>,
    actual_cost_micros: Option<i64>,
    actual_currency: Option<String>,
    actual_pricing_version: Option<String>,
    execution_closed: bool,
    dispatch_id: Option<String>,
    dispatched_at: Option<i64>,
    finalized_at: Option<i64>,
    execution_closed_at: Option<i64>,
}
impl From<&BudgetCallRecord> for E05MathCallR39 {
    fn from(call: &BudgetCallRecord) -> Self {
        Self {
            billing_scope: call.billing_scope.clone(),
            call_id: call.call_id.clone(),
            dispatch_group_id: call.dispatch_group_id.clone(),
            namespace: call.namespace.clone(),
            stage: call.stage,
            actual_input_digest: call.actual_input_digest.clone(),
            state: call.state,
            output_digest: call.output_digest.clone(),
            actual_cost_micros: call.actual_cost_micros,
            actual_currency: call.actual_currency.clone(),
            actual_pricing_version: call.actual_pricing_version.clone(),
            execution_closed: call.execution_closed,
            dispatch_id: call.dispatch_id.clone(),
            dispatched_at: call.dispatched_at,
            finalized_at: call.finalized_at,
            execution_closed_at: call.execution_closed_at,
        }
    }
}

fn e05_recompute_complete_report(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
    control: &MirrorRegisteredEvaluationControl,
    holdout: &MirrorProtectedHoldoutRecord,
    calls: &[BudgetCallRecord],
) -> Result<MirrorBernsteinReportWire> {
    let math_calls: Vec<_> = calls.iter().map(E05MathCallR39::from).collect();
    e05_recompute_complete_report_r39(
        &ActualCompleteRecordsR39(snapshot),
        namespace,
        ticket,
        control,
        holdout,
        &math_calls,
    )
}

fn e05_recompute_complete_report_r39<R: E05CompleteRecordsR39>(
    records: &R,
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
    control: &MirrorRegisteredEvaluationControl,
    holdout: &MirrorProtectedHoldoutRecord,
    calls: &[E05MathCallR39],
) -> Result<MirrorBernsteinReportWire> {
    let changed = e05_complete_changed;
    let planned = usize::try_from(ticket.n_planned).map_err(|_| changed())?;
    require(
        ticket.lifecycle == MirrorTicketLifecycle::CompleteBatch
            && planned == control.v1_plan_snapshot.n_planned
            && planned == holdout.manifest.ordered_task_cluster_ids.len(),
    )?;
    let actual_by_id: BTreeMap<_, _> = calls
        .iter()
        .map(|call| (call.call_id.as_str(), call))
        .collect();
    require(actual_by_id.len() == calls.len())?;
    let mut sequences = BTreeSet::new();
    let mut clusters = BTreeSet::new();
    let mut receipt_ids = BTreeSet::new();
    let mut grader_ids = BTreeSet::new();
    let mut used_calls = BTreeSet::new();
    let mut rows = Vec::new();
    let mut events = Vec::new();
    let mut candidate_cost = 0_i128;
    let mut baseline_cost = 0_i128;
    let mut candidate_latencies = Vec::new();
    let mut baseline_latencies = Vec::new();
    for target in &ticket.targets {
        let MirrorTargetState::Completed {
            candidate_receipt_id,
            baseline_receipt_id,
            grader_receipt_id,
            completion_sequence,
            ..
        } = &target.state
        else {
            return Err(changed());
        };
        require(
            sequences.insert(*completion_sequence) && grader_ids.insert(grader_receipt_id.as_str()),
        )?;
        let grader = e05_completed_target_evidence_r39(
            records, namespace, ticket, control, holdout, target,
        )?;
        require(
            grader.schema_version == "rsia.independent_grader_receipt.v2"
                && grader.receipt_id == *grader_receipt_id,
        )?;
        match &target.kind {
            MirrorEvaluationTargetKind::Rotating { cluster_id } => {
                require(clusters.insert(cluster_id.as_str()))?;
                rows.push(evo_core::evaluation::ClusterObservation {
                    cluster_id: cluster_id.clone(),
                    d: (i64::from(grader.candidate_score_micros)
                        - i64::from(grader.baseline_score_micros)) as f64
                        / 1_000_000.0,
                    weight: 1.0,
                });
            }
            MirrorEvaluationTargetKind::Anchor { .. } => {
                require(grader.anchor_result == Some(AnchorResult::Passed))?;
            }
        }
        events.push((*completion_sequence, target, grader));
        for (receipt_id, side) in [
            (candidate_receipt_id, MirrorExecutionSide::Candidate),
            (baseline_receipt_id, MirrorExecutionSide::Baseline),
        ] {
            require(receipt_ids.insert(receipt_id.as_str()))?;
            let receipt: MirrorExecutionReceiptV2 =
                records.record(namespace, "execution_receipt_v2", receipt_id)?;
            let bundle = match side {
                MirrorExecutionSide::Candidate => &ticket.candidate_bundle_digest,
                MirrorExecutionSide::Baseline => &ticket.baseline_bundle_digest,
            };
            let request =
                e05_request_digest(ticket, target, side, bundle, &ticket.environment_digest)?;
            require(
                receipt.schema_version == "rsia.execution_receipt.v2"
                    && receipt.receipt_id == *receipt_id
                    && receipt.executor_actor == ticket.executor_actor
                    && receipt.request_digest == request,
            )?;
            let call = actual_by_id
                .get(receipt.budget_call_id.as_str())
                .ok_or_else(changed)?;
            let amount = call.actual_cost_micros.ok_or_else(changed)?;
            require(
                used_calls.insert(call.call_id.as_str())
                    && call.billing_scope == control.billing_scope
                    && call.namespace == namespace
                    && call.dispatch_group_id == ticket.id
                    && call.stage == BudgetStage::TaskExecution
                    && call.actual_input_digest == request
                    && call.output_digest.as_deref() == Some(receipt.output_digest.as_str())
                    && call.state == BudgetCallState::Finalized
                    && call.execution_closed
                    && call.dispatch_id.is_some()
                    && call.dispatched_at.is_some()
                    && call.finalized_at.is_some()
                    && call.execution_closed_at.is_some()
                    && amount >= 0
                    && call.actual_currency.as_deref()
                        == Some(control.optimization_budget.root_total.currency.as_str())
                    && call.actual_pricing_version.as_deref()
                        == Some(
                            control
                                .optimization_budget
                                .root_total
                                .pricing_version
                                .as_str(),
                        ),
            )?;
            // No live lease, open root or finalized-before-close requirement:
            // an actually dispatched execution may be financially settled later.
            match side {
                MirrorExecutionSide::Candidate => {
                    candidate_cost = candidate_cost
                        .checked_add(i128::from(amount))
                        .ok_or_else(changed)?;
                    candidate_latencies.push(receipt.latency_micros);
                }
                MirrorExecutionSide::Baseline => {
                    baseline_cost = baseline_cost
                        .checked_add(i128::from(amount))
                        .ok_or_else(changed)?;
                    baseline_latencies.push(receipt.latency_micros);
                }
            }
        }
    }
    let target_count = u64::try_from(ticket.targets.len()).map_err(|_| changed())?;
    require(
        sequences.iter().copied().eq(1..=target_count)
            && ticket.next_sequence == target_count.checked_add(1).ok_or_else(changed)?
            && rows.len() == planned
            && rows
                .iter()
                .map(|row| &row.cluster_id)
                .eq(holdout.manifest.ordered_task_cluster_ids.iter())
            && used_calls.len() == actual_by_id.len()
            && baseline_cost > 0
            && candidate_cost >= 0,
    )?;
    events.sort_by_key(|(sequence, _, _)| *sequence);
    let first = events.first().ok_or_else(changed)?;
    let ledger = records.exposure(namespace, ticket)?;
    let entry = &ledger.entry;
    let first_feedback = first
        .2
        .issued_at_unix_seconds
        .checked_mul(1_000)
        .ok_or_else(changed)?;
    require(
        ledger.research_family_id == ticket.research_family_id
            && ledger.alpha_plan_digest == ticket.alpha_plan_digest
            && entry.manifest_digest == ticket.manifest_digest
            && entry.candidate_pair_digest == holdout.manifest.candidate_pair_digest
            && entry.epoch_id == holdout.manifest.epoch_id
            && entry.slice_id == holdout.manifest.slice_id
            && entry.task_cluster_ids == holdout.manifest.ordered_task_cluster_ids
            && entry.monetary_state == MonetaryReservationState::Finalized
            && matches!(&entry.state, ExposureState::FeedbackUsed { used_at_unix_ms } if *used_at_unix_ms == first_feedback),
    )?;
    // Validate only this ticket's immutable entry, never pin the whole shared
    // ledger into the report: later legitimate trials may append other entries.
    if let Some(plan) = &control.early_stop_plan {
        use evo_core::sequential::{CompletePairedUnit, EarlyStopDecision, SequentialRejectOnly};
        plan.validate_against(&control.alpha_plan)?;
        require(plan.ordered_cluster_ids == holdout.manifest.ordered_task_cluster_ids)?;
        let mut stream = SequentialRejectOnly::open(plan.clone(), &control.alpha_plan)?;
        for (_, target, grader) in &events {
            if let MirrorEvaluationTargetKind::Rotating { cluster_id } = &target.kind {
                let decision = stream.record_complete_unit(CompletePairedUnit {
                    cluster_id: cluster_id.clone(),
                    baseline_score_micros: grader.baseline_score_micros,
                    candidate_score_micros: grader.candidate_score_micros,
                })?;
                if decision.is_terminal() {
                    require(
                        decision == EarlyStopDecision::FinalFixedSample
                            && stream.k() == ticket.n_planned,
                    )?;
                    break;
                }
            }
        }
        require(
            stream.decision()? == EarlyStopDecision::FinalFixedSample
                && stream.k() == ticket.n_planned,
        )?;
    }
    let baseline_p95 = e05_complete_p95(&mut baseline_latencies)?;
    let candidate_p95 = e05_complete_p95(&mut candidate_latencies)?;
    require(baseline_p95 > 0)?;
    let report = evo_core::formal_complete_statistics::derive_complete_report(
        &control.schema_version,
        ticket.profile,
        &control.v1_plan_snapshot,
        (&ticket.candidate_digest, &ticket.bound_v1_plan_digest),
        (
            &control.alpha_plan,
            &control.fixed_sample_claim_id,
            &control.attempt_id,
        ),
        &rows,
        (
            candidate_cost as f64 / baseline_cost as f64,
            candidate_p95 as f64 / baseline_p95 as f64,
        ),
    )?;
    require(report.n == planned)?;
    e05_complete_wire(&report)
}

fn e05_complete_batch(
    snapshot: &OccurrenceSnapshot,
    object: &OccurrenceObject,
    ticket: &MirrorEvaluationTicketV2,
    formal: &MirrorFormalEvaluationV2,
) -> Result<E05CompleteBatchRead> {
    let MirrorFormalEvaluationV2::CompleteBatch {
        evidence_scope,
        cost_evidence_scope,
        ticket_id,
        ticket_digest,
        manifest_digest,
        candidate_bundle_digest,
        baseline_bundle_digest,
        environment_digest,
        verdict,
        report,
        anchor_evidence_digest,
        resource_evidence_digest,
        stored_dependency_binding,
    } = formal
    else {
        return Err(e05_complete_changed());
    };
    e05_dependency_live(snapshot, object)?;
    require(
        ticket.lifecycle == MirrorTicketLifecycle::CompleteBatch
            && ticket_id == &ticket.id
            && ticket_digest == &typed_fingerprint(ticket)?
            && manifest_digest == &ticket.manifest_digest
            && candidate_bundle_digest == &ticket.candidate_bundle_digest
            && baseline_bundle_digest == &ticket.baseline_bundle_digest
            && environment_digest == &ticket.environment_digest,
    )?;
    let binding = match (
        &ticket.candidate_dependency_binding,
        stored_dependency_binding,
    ) {
        (Some(_), Some(binding)) => binding,
        (None, None) => {
            // Preserve the historical read branch without inventing missing raw
            // facts or granting current_verified. Versioned legacy dispatch and
            // actual old-record constants remain a separately unclosed gate.
            return Ok(E05CompleteBatchRead::LegacyUnverified);
        }
        _ => return Err(e05_complete_changed()),
    };
    let control = evaluation_control(snapshot, &object.namespace, &ticket.registration_id)?;
    let holdout = evaluation_holdout(snapshot, &object.namespace, &ticket.holdout_id, &control)?;
    e05_complete_ticket_identity(snapshot, &object.namespace, ticket, &control, &holdout)?;
    e05_complete_group(
        snapshot,
        &object.namespace,
        &control.billing_scope,
        &ticket.id,
    )?;
    let resource: MirrorResourceEvidenceRecord = evaluation_record(
        snapshot,
        &object.namespace,
        "evaluation_resource_evidence_v1",
        &ticket.id,
    )?;
    require(
        resource.schema_version == "rsia.evaluation_resource_evidence.v1"
            && resource.ticket_id == ticket.id
            && resource.billing_scope == control.billing_scope
            && resource.cost_scope == MirrorCostEvidenceScope::TicketExecutionOnly
            && cost_evidence_scope == &MirrorCostEvidenceScope::TicketExecutionOnly
            && evidence_scope == &ticket.evidence_scope
            && evidence_scope == &control.evidence_scope
            && typed_fingerprint(&resource)? == *resource_evidence_digest
            && resource.calls.len()
                == ticket
                    .targets
                    .len()
                    .checked_mul(2)
                    .ok_or_else(e05_complete_changed)?,
    )?;
    // The whole scope/group slice is used. Filtering namespace first could hide
    // an unattributed foreign call instead of refusing its presence.
    let actual: Vec<_> = snapshot
        .calls
        .values()
        .filter(|call| {
            call.billing_scope == control.billing_scope && call.dispatch_group_id == ticket.id
        })
        .cloned()
        .collect();
    let actual_by_id: BTreeMap<_, _> = actual
        .iter()
        .map(|call| (call.call_id.as_str(), call))
        .collect();
    let mut copied_ids = BTreeSet::new();
    require(actual_by_id.len() == actual.len() && actual.len() == resource.calls.len())?;
    for copy in &resource.calls {
        require(
            copied_ids.insert(copy.call_id.as_str())
                && actual_by_id
                    .get(copy.call_id.as_str())
                    .is_some_and(|call| *call == copy),
        )?;
    }
    require(copied_ids.len() == actual_by_id.len())?;
    let anchors: Vec<_> = ticket
        .targets
        .iter()
        .filter(|target| matches!(target.kind, MirrorEvaluationTargetKind::Anchor { .. }))
        .collect();
    require(typed_fingerprint(&anchors)? == *anchor_evidence_digest)?;
    let expected = e05_recompute_complete_report(
        snapshot,
        &object.namespace,
        ticket,
        &control,
        &holdout,
        &actual,
    )?;
    require(report == &expected && verdict == &expected.verdict)?;
    let ticket_physical = evaluation_storage_id("evaluation_ticket_v2", &ticket.id)?;
    let resource_physical = evaluation_storage_id("evaluation_resource_evidence_v1", &ticket.id)?;
    require(
        binding.ticket_physical_id == ticket_physical
            && binding.resource_physical_id == resource_physical,
    )?;
    e05_require_exact_edges(
        snapshot,
        &object.namespace,
        &resource_physical,
        vec![e05_artifact_edge("evaluation_ticket_v2", &ticket.id)?],
    )?;
    e05_require_exact_edges(
        snapshot,
        &object.namespace,
        &object.id,
        e05_report_edges(snapshot, &object.namespace, ticket, &control)?,
    )?;
    e05_stored_binding(snapshot, &object.namespace, ticket, binding)?;
    Ok(E05CompleteBatchRead::SourceCheckedVersionPending)
}

#[cfg(test)]
mod historical_seal_r42_tests {
    use super::*;
    fn root() -> RootBudgetRecord {
        RootBudgetRecord {
            root_budget_id: "r42-root".into(),
            billing_scope: "r42-billing".into(),
            authorizing_namespace: "r42-a".into(),
            allowed_namespaces: vec!["r42-a".into(), "r42-b".into()],
            currency: "USD".into(),
            pricing_version: "r42-price".into(),
            payment_subject: "r42-payer".into(),
            authorization_receipt_digest: evo_core::hash(b"authorization"),
            per_call_cap_micros: 10,
            total_limit_micros: 100,
            spent_micros: 0,
            reserved_micros: 0,
            stopped: true,
            stop_reason: Some("first stop".into()),
            stop_committed_at: Some(20),
            created_at: 1,
        }
    }
    fn empty_seal(root: RootBudgetRecord, at: i64) -> SealedRootBudgetEvidence {
        let snapshot = monetary_snapshot(root, Vec::new(), Vec::new(), Vec::new()).unwrap();
        seal_summary(&snapshot, "r42-a", "r42-admin", at)
    }
    fn seal_row(sequence: i64, evidence: &SealedRootBudgetEvidence) -> OwnedRow {
        OwnedRow {
            cells: BTreeMap::from([
                ("seq".into(), OwnedCell::Integer(sequence)),
                (
                    "billing_scope".into(),
                    OwnedCell::Text(evidence.root.billing_scope.clone()),
                ),
                ("call_id".into(), OwnedCell::Null),
                (
                    "event_kind".into(),
                    OwnedCell::Text("root_monetary_sealed_v1".into()),
                ),
                ("event_at".into(), OwnedCell::Integer(evidence.sealed_at)),
                (
                    "details".into(),
                    OwnedCell::Text(serde_json::to_string(evidence).unwrap()),
                ),
            ]),
        }
    }
    // Pure shape controls are not a native or business-authority fixture.
    #[test]
    fn historical_seal_keeps_nonnegative_static_auth_and_first_stop() {
        let original = empty_seal(root(), 20);
        let mut current = root();
        current.spent_micros = 6;
        current.reserved_micros = 2;
        qualify_historical_root_seal_r42(&original, &current, &[]).unwrap();
        for field in [
            "spent",
            "reserved",
            "authorization",
            "stop",
            "actor",
            "stage",
            "total",
        ] {
            let mut bad = original.clone();
            match field {
                "spent" => bad.root.spent_micros = -1,
                "reserved" => bad.root.reserved_micros = -1,
                "authorization" => {
                    bad.root.authorization_receipt_digest = evo_core::hash(b"changed authorization")
                }
                "stop" => bad.root.stop_reason = Some("changed first stop".into()),
                "actor" => bad.sealed_by_actor = "invalid actor".into(),
                "stage" => bad.stage_totals[0].known_actual_micros = "-1".into(),
                "total" => bad.known_actual_micros = "1".into(),
                _ => unreachable!(),
            }
            assert!(
                matches!(
                    qualify_historical_root_seal_r42(&bad, &current, &[]),
                    Err(Error::ClockRegressed)
                ),
                "{field}"
            );
        }
    }
    #[test]
    fn latest_seal_does_not_fall_back_and_scopes_namespace_and_role() {
        let current = root();
        let first = empty_seal(current.clone(), 20);
        let mut latest = empty_seal(current.clone(), 25);
        latest.snapshot_digest = evo_core::hash(b"self well formed but wrong current ledger");
        let mut raw = RawDatabase::default();
        raw.tables.insert(
            "root_budget_events".into(),
            vec![seal_row(1, &first), seal_row(2, &latest)],
        );
        let mut snapshot = OccurrenceSnapshot::default();
        snapshot
            .roots
            .insert(current.billing_scope.clone(), current.clone());
        let allowed = evo_core::Context::new("r42-b", "reader", evo_core::Role::Evaluator).unwrap();
        let stale = strict_latest_root_seal_r42(&raw, &snapshot, &allowed, "r42-billing").unwrap();
        assert!(matches!(stale.evidence(),Err(Error::Conflict(message))
            if message=="root monetary seal is stale or invalid"));
        latest = empty_seal(current.clone(), 25);
        raw.tables.insert(
            "root_budget_events".into(),
            vec![seal_row(1, &first), seal_row(2, &latest)],
        );
        let fresh = strict_latest_root_seal_r42(&raw, &snapshot, &allowed, "r42-billing").unwrap();
        assert_eq!(fresh.evidence().unwrap(), Some(&latest));
        assert!(!fresh.evidence().unwrap().unwrap().complete_episode_cost);
        assert!(!fresh.evidence().unwrap().unwrap().ordinary_p_eligible);
        let foreign =
            evo_core::Context::new("r42-foreign", "reader", evo_core::Role::Admin).unwrap();
        assert!(matches!(
            strict_latest_root_seal_r42(&raw, &snapshot, &foreign, "r42-billing"),
            Err(Error::NotFound)
        ));
        let agent = evo_core::Context::new("r42-a", "agent", evo_core::Role::Agent).unwrap();
        assert!(matches!(
            strict_latest_root_seal_r42(&raw, &snapshot, &agent, "r42-billing"),
            Err(Error::Forbidden)
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pure raw-row/parser/cache fixtures only: no SQL or qualification authority.
    fn value_digest_raw_database_r26(raw: &str) -> RawDatabase {
        RawDatabase {
            tables: BTreeMap::from([(
                "objects".into(),
                vec![OwnedRow {
                    cells: BTreeMap::from([
                        ("namespace".into(), OwnedCell::Text("digest-ns".into())),
                        ("kind".into(), OwnedCell::Text("artifact".into())),
                        ("id".into(), OwnedCell::Text("digest-object".into())),
                        ("owner".into(), OwnedCell::Text("digest-owner".into())),
                        ("revision".into(), OwnedCell::Integer(1)),
                        ("body".into(), OwnedCell::Text(raw.into())),
                    ]),
                }],
            )]),
        }
    }
    fn value_digest_snapshot_r26(raw: &str) -> OccurrenceSnapshot {
        let mut snapshot = OccurrenceSnapshot::default();
        collect_objects(&value_digest_raw_database_r26(raw), &mut snapshot).unwrap();
        // Same private population operation as the two actual raw builders.
        snapshot.precompute_actual_body_digests_r26();
        snapshot
    }

    #[test]
    fn value_body_digest_matches_original_canonical_bytes_for_json_domain() {
        let cases = [
            "null",
            "true",
            "false",
            "0",
            "-1",
            "9223372036854775807",
            "-9223372036854775808",
            "18446744073709551615",
            "0.0",
            "-0.0",
            "1.5",
            "1e-200",
            "1e200",
            r#""""#,
            r#""\u0000\n\\\"""#,
            r#""界🙂""#,
            "[]",
            "{}",
            r#"{"z":[true,null,{"b":-0.0,"a":"\u0000界"}],"a":18446744073709551615}"#,
            r#"{"a":18446744073709551615,"z":[true,null,{"a":"\u0000界","b":-0.0}]}"#,
        ];
        for raw in cases {
            let body = parse_owned_json(raw).unwrap();
            // The original canonical algorithm remains the independent oracle.
            let canonical = canonical_fingerprint(&body).unwrap();
            let original_canonical_value = serde_json::to_value(&body).unwrap();
            assert_eq!(
                serde_json::to_vec(&original_canonical_value).unwrap(),
                serde_json::to_vec(&body).unwrap(),
                "raw={raw}"
            );
            assert_eq!(canonical, fingerprint(&body).unwrap(), "raw={raw}");
            let snapshot = value_digest_snapshot_r26(raw);
            let actual = snapshot
                .object("digest-ns", "artifact", "digest-object")
                .unwrap();
            assert_eq!(actual.body, body);
            assert_eq!(
                actual.body_value_digest_r26.as_deref(),
                Some(canonical.as_str())
            );
            assert_eq!(actual.value_body_digest_r26().unwrap(), canonical);
            let projection = OccurrenceObject {
                namespace: "projected-ns".into(),
                kind: "artifact".into(),
                id: "projected".into(),
                owner: "projector".into(),
                body: body.clone(),
                body_value_digest_r26: None,
            };
            assert_eq!(projection.value_body_digest_r26().unwrap(), canonical);
            assert!(projection.body_value_digest_r26.is_none()); // no lazy mutation
        }
        let first = value_digest_snapshot_r26(cases[cases.len() - 2]);
        let second = value_digest_snapshot_r26(cases[cases.len() - 1]);
        assert_eq!(
            first
                .object("digest-ns", "artifact", "digest-object")
                .unwrap()
                .value_body_digest_r26()
                .unwrap(),
            second
                .object("digest-ns", "artifact", "digest-object")
                .unwrap()
                .value_body_digest_r26()
                .unwrap()
        );

        // Equality is Value-specific. A typed record's declaration-order digest
        // must never be replaced by this object's pure Value content digest.
        #[derive(Serialize)]
        struct TypedOrder {
            z: u8,
            a: u8,
        }
        let typed = TypedOrder { z: 1, a: 2 };
        assert_ne!(
            typed_fingerprint(&typed).unwrap(),
            canonical_fingerprint(&typed).unwrap()
        );
        assert_eq!(
            canonical_fingerprint(&typed).unwrap(),
            fingerprint(&serde_json::to_value(&typed).unwrap()).unwrap()
        );
    }

    #[test]
    fn value_body_digest_fresh_capture_and_replacement_cannot_reuse_old_body() {
        let old = value_digest_snapshot_r26(r#"{"revision_body":1,"run":{"id":"run-a"}}"#);
        let fresh = value_digest_snapshot_r26(r#"{"revision_body":2,"run":{"id":"run-b"}}"#);
        let old_object = old
            .object("digest-ns", "artifact", "digest-object")
            .unwrap();
        let fresh_object = fresh
            .object("digest-ns", "artifact", "digest-object")
            .unwrap();
        let old_digest = old_object.value_body_digest_r26().unwrap();
        let fresh_digest = fresh_object.value_body_digest_r26().unwrap();
        assert_ne!(old_digest, fresh_digest);
        assert_eq!(old_object.value_body_digest_r26().unwrap(), old_digest);
        assert_eq!(
            fresh_digest,
            canonical_fingerprint(&fresh_object.body).unwrap()
        );

        // Exercise the actual single body-replacement helper against a Some
        // from collect_objects, rather than constructing a test Some DTO.
        let mut replaced = old_object.clone();
        replaced.replace_body_r26(fresh_object.body.clone());
        assert!(replaced.body_value_digest_r26.is_none());
        assert_eq!(replaced.value_body_digest_r26().unwrap(), fresh_digest);
        assert_eq!(old_object.value_body_digest_r26().unwrap(), old_digest);
        for kind in ["candidate", "job", "run"] {
            let projected = OccurrenceObject {
                kind: kind.into(),
                body_value_digest_r26: None,
                ..old_object.clone()
            };
            assert!(projected.body_value_digest_r26.is_none());
            assert_eq!(projected.value_body_digest_r26().unwrap(), old_digest);
        }
        // Existing strict JSON/SQLite errors are still emitted before a body
        // can enter a snapshot, without cache-created permissiveness.
        for raw in [r#"{"dup":1,"dup":2}"#, "null trailing", "["] {
            let mut bad = OccurrenceSnapshot::default();
            assert!(matches!(
                collect_objects(&value_digest_raw_database_r26(raw), &mut bad),
                Err(Error::ClockRegressed)
            ));
            assert!(bad.objects.is_empty());
        }
    }

    #[test]
    fn worker_rows_preserve_native_cell_bytes_and_reject_duplicate_coordinates() {
        let mut connection = rusqlite::Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE objects(i INTEGER,r REAL,t TEXT,b BLOB,n);")
            .unwrap();
        connection
            .execute(
                "INSERT INTO objects VALUES(?1,?2,?3,?4,NULL)",
                rusqlite::params![i64::MAX, -0.0_f64, "raw \0 ?%#界", vec![0_u8, 255]],
            )
            .unwrap();
        let transaction = connection.transaction().unwrap();
        let native_bits = transaction
            .query_row("SELECT r FROM objects", [], |row| row.get::<_, f64>(0))
            .unwrap()
            .to_bits();
        let value = raw_worker_value_r50(&transaction).unwrap();
        let database = raw_from_worker_r50(value.clone()).unwrap();
        let cells = &database.tables["objects"][0].cells;
        assert!(matches!(cells["i"], OwnedCell::Integer(i64::MAX)));
        assert!(matches!(cells["r"],OwnedCell::Real(value) if value.to_bits()==native_bits));
        assert!(matches!(&cells["t"],OwnedCell::Text(value) if value=="raw \0 ?%#界"));
        assert!(matches!(&cells["b"],OwnedCell::Blob(value) if value==&vec![0,255]));
        assert!(matches!(cells["n"], OwnedCell::Null));
        let mut duplicate_table = value.clone();
        let tables = duplicate_table["tables"].as_array_mut().unwrap();
        tables.push(tables[0].clone());
        assert!(raw_from_worker_r50(duplicate_table).is_err());
        let mut duplicate_column = value.clone();
        let tables = duplicate_column["tables"].as_array_mut().unwrap();
        let object = tables
            .iter_mut()
            .find(|table| table[0] == "objects")
            .unwrap();
        let columns = object[1][0].as_array_mut().unwrap();
        columns.push(columns[0].clone());
        assert!(raw_from_worker_r50(duplicate_column).is_err());
        let mut unknown = value;
        unknown["tables"][0][0] = Value::String("not_a_captured_table".into());
        assert!(raw_from_worker_r50(unknown).is_err());
        transaction.rollback().unwrap();
        connection.close().unwrap();
    }

    fn outgoing_test_snapshot(
        edges: BTreeSet<(String, String, String, String, String)>,
    ) -> OccurrenceSnapshot {
        OccurrenceSnapshot {
            e05_phase: None,
            native_consumer_origins_r49: BTreeMap::new(),
            complete_origins_r24: BTreeMap::new(),
            complete_pending_r24: None,
            objects: BTreeMap::new(),
            witnesses: BTreeMap::new(),
            replay_owners: BTreeSet::new(),
            roots: BTreeMap::new(),
            calls: BTreeMap::new(),
            groups: BTreeMap::new(),
            caches: Vec::new(),
            watermarks: BTreeMap::new(),
            edges,
            replay_worlds: BTreeMap::new(),
            redacted_replay_worlds: BTreeMap::new(),
        }
    }

    #[test]
    fn outgoing_prefix_range_matches_full_scan_for_empty_unicode_and_nearby_sources() {
        let sources = ["", "a", "a\0", "a\u{10ffff}", "b", "界", "\u{10ffff}"];
        let destinations = ["", "z", "界"];
        let mut edges = BTreeSet::new();
        for ns in sources {
            for kind in sources {
                for id in sources {
                    for dst_kind in destinations {
                        for dst_id in destinations {
                            edges.insert((
                                ns.into(),
                                kind.into(),
                                id.into(),
                                dst_kind.into(),
                                dst_id.into(),
                            ));
                        }
                    }
                }
            }
        }
        let snapshot = outgoing_test_snapshot(edges);
        let queries = ["", "a", "a\0", "a\u{10ffff}", "b", "c", "界", "\u{10ffff}"];
        for ns in queries {
            for kind in queries {
                for id in queries {
                    // Independent whole-set oracle is the original query semantics.
                    let expected: Vec<_> = snapshot
                        .edges
                        .iter()
                        .filter(|(n, k, i, _, _)| n == ns && k == kind && i == id)
                        .map(|(_, _, _, k, i)| (k.clone(), i.clone()))
                        .collect();
                    assert_eq!(
                        e05_outgoing(&snapshot, ns, kind, id).unwrap(),
                        expected,
                        "source prefix {ns:?}/{kind:?}/{id:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn outgoing_prefix_range_preserves_ten_thousand_limit_without_truncation() {
        let mut edges = BTreeSet::new();
        for index in 0..12_000 {
            edges.insert((
                "other".into(),
                "artifact".into(),
                "source".into(),
                "artifact".into(),
                format!("target-{index:05}"),
            ));
        }
        let mut snapshot = outgoing_test_snapshot(edges);
        assert!(
            e05_outgoing(&snapshot, "owner", "artifact", "source")
                .unwrap()
                .is_empty()
        );
        for index in 0..10_000 {
            snapshot.edges.insert((
                "owner".into(),
                "artifact".into(),
                "source".into(),
                "artifact".into(),
                format!("target-{index:05}"),
            ));
        }
        assert_eq!(
            e05_outgoing(&snapshot, "owner", "artifact", "source")
                .unwrap()
                .len(),
            10_000
        );
        snapshot.edges.insert((
            "owner".into(),
            "artifact".into(),
            "source".into(),
            "artifact".into(),
            "extra".into(),
        ));
        assert!(matches!(
            e05_outgoing(&snapshot, "owner", "artifact", "source"),
            Err(Error::ClockRegressed)
        ));
        assert!(matches!(
            e05_outgoing(&snapshot, "other", "artifact", "source"),
            Err(Error::ClockRegressed)
        ));
    }

    async fn native_sqlx_duration_database() -> SqliteConnection {
        let mut connection = SqliteConnection::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations")
            .run_direct(&mut connection)
            .await
            .unwrap();
        let identity: (String, String) =
            sqlx::query_as("SELECT sqlite_version(), sqlite_source_id()")
                .fetch_one(&mut connection)
                .await
                .unwrap();
        assert!(!identity.0.is_empty() && !identity.1.is_empty());
        println!("actual-native-sqlx-duration-identity {identity:?}");
        connection
    }

    async fn native_sqlx_duration_floor(
        connection: &mut SqliteConnection,
    ) -> Result<OccurrenceFloor> {
        let mut tx = connection.begin().await.unwrap();
        let result = collect_floor(&mut tx, &PreparePreimages::default(), SchemaRead::Stable).await;
        tx.rollback().await.unwrap();
        result
    }

    #[tokio::test]
    async fn native_sqlx_duration_marker_and_large_duration_do_not_change_occurrence() {
        let mut connection = native_sqlx_duration_database().await;
        let baseline = native_sqlx_duration_floor(&mut connection).await.unwrap();
        assert!(baseline.max_nanos().is_some());
        for duration in [-1_i64, 0, i64::MAX] {
            let changed =
                sqlx::query("UPDATE _sqlx_migrations SET execution_time=? WHERE version=1")
                    .bind(duration)
                    .execute(&mut connection)
                    .await
                    .unwrap();
            assert_eq!(changed.rows_affected(), 1);
            let before: i64 = sqlx::query_scalar("SELECT total_changes()")
                .fetch_one(&mut connection)
                .await
                .unwrap();
            let observed = native_sqlx_duration_floor(&mut connection).await.unwrap();
            let after: i64 = sqlx::query_scalar("SELECT total_changes()")
                .fetch_one(&mut connection)
                .await
                .unwrap();
            assert_eq!(
                observed, baseline,
                "duration {duration} became an occurrence"
            );
            assert_eq!(after, before, "the collector wrote during a duration read");
        }
        connection.close().await.unwrap();
    }

    #[tokio::test]
    async fn native_sqlx_duration_bad_values_and_original_ledger_bindings_still_refuse() {
        let mut connection = native_sqlx_duration_database().await;
        let baseline = native_sqlx_duration_floor(&mut connection).await.unwrap();
        // Each mutation is confined to this real original-driver fixture and
        // rolled back before the next healthy read. No decoder or clock is mocked.
        for assignment in [
            "execution_time=-2",
            "execution_time=0.5",
            "execution_time='not-a-duration'",
            "installed_on='not-a-clock'",
            "checksum=X'00'",
        ] {
            let mut tx = connection.begin().await.unwrap();
            let changed = sqlx::query(&format!(
                "UPDATE _sqlx_migrations SET {assignment} WHERE version=1"
            ))
            .execute(&mut *tx)
            .await
            .unwrap();
            assert_eq!(changed.rows_affected(), 1);
            let before: i64 = sqlx::query_scalar("SELECT total_changes()")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            let result =
                collect_floor(&mut tx, &PreparePreimages::default(), SchemaRead::Stable).await;
            let after: i64 = sqlx::query_scalar("SELECT total_changes()")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            assert!(
                matches!(result, Err(Error::ClockRegressed)),
                "actual invalid ledger {assignment} was not refused"
            );
            assert_eq!(after, before, "the refusing collector wrote");
            tx.rollback().await.unwrap();
            assert_eq!(
                native_sqlx_duration_floor(&mut connection).await.unwrap(),
                baseline,
                "healthy original ledger did not survive rollback of {assignment}"
            );
        }
        connection.close().await.unwrap();
    }

    fn e05_complete_test_group() -> DispatchGroupRecord {
        DispatchGroupRecord {
            billing_scope: "scope".into(),
            dispatch_group_id: "ticket".into(),
            owner_namespace: "test".into(),
            stopped: true,
            stop_reason: Some("operator_stop".into()),
            stop_committed_at: Some(3),
            created_at: 1,
        }
    }

    #[test]
    fn e05_complete_group_keeps_first_stop_reason_without_a_live_root() {
        let mut snapshot = OccurrenceSnapshot::default();
        snapshot
            .groups
            .insert(("scope".into(), "ticket".into()), e05_complete_test_group());
        assert!(snapshot.roots.is_empty());
        let group = e05_complete_group(&snapshot, "test", "scope", "ticket").unwrap();
        assert_eq!(group.stop_reason.as_deref(), Some("operator_stop"));
    }

    #[test]
    fn e05_complete_group_refuses_missing_open_or_foreign_group() {
        let mut snapshot = OccurrenceSnapshot::default();
        assert!(
            matches!(e05_complete_group(&snapshot, "test", "scope", "ticket"), Err(Error::Invalid(message)) if message.starts_with("source_insufficient:"))
        );
        let mut group = e05_complete_test_group();
        group.stopped = false;
        group.stop_reason = None;
        group.stop_committed_at = None;
        snapshot
            .groups
            .insert(("scope".into(), "ticket".into()), group);
        assert!(e05_complete_group(&snapshot, "test", "scope", "ticket").is_err());
        snapshot
            .groups
            .insert(("scope".into(), "ticket".into()), e05_complete_test_group());
        assert!(e05_complete_group(&snapshot, "other", "scope", "ticket").is_err());
    }

    #[test]
    fn e05_complete_p95_keeps_original_ceiling_rank_and_empty_refusal() {
        let mut twenty = (1_u64..=20).rev().collect::<Vec<_>>();
        assert_eq!(e05_complete_p95(&mut twenty).unwrap(), 19);
        let mut twenty_one = (1_u64..=21).rev().collect::<Vec<_>>();
        assert_eq!(e05_complete_p95(&mut twenty_one).unwrap(), 20);
        assert!(e05_complete_p95(&mut []).is_err());
    }

    #[test]
    fn e05_complete_wire_preserves_finite_decimal_and_refuses_nan() {
        let mut report = evo_core::evaluation::empirical_bernstein(&[], 0.04).unwrap();
        report.mean = -0.0;
        assert_eq!(e05_complete_wire(&report).unwrap().mean, "0");
        report.mean = f64::NAN;
        assert!(e05_complete_wire(&report).is_err());
    }

    fn provider_current_public_fixture() -> MirrorRegisteredExternalProviderV2 {
        let protocol_id = evo_core::model_protocol::ModelProtocolId::ChatCompletionsNonstreamV1;
        let sampling_profile = evo_core::model_protocol::SamplingProfileId::TemperatureZeroV1;
        MirrorRegisteredExternalProviderV2 {
            id: "provider".into(),
            schema_version: "rsia.registered_external_provider.v2".into(),
            namespace: "private-test".into(),
            host_registration_id: "host".into(),
            endpoint: "https://example.test/v1/chat/completions".into(),
            provider_model: "model".into(),
            model_digest: "a".repeat(64),
            transport_code_digest: "b".repeat(64),
            credential_environment_key: "MODEL_KEY".into(),
            credential_principal: "principal".into(),
            currency: "USD".into(),
            pricing_version: "price-v1".into(),
            input_micros_per_million_tokens: 0,
            output_micros_per_million_tokens: 0,
            max_output_tokens: 32768,
            authorized_by: "admin".into(),
            service_category: MirrorExternalServiceCategory::Model,
            protocol_id,
            sampling_profile,
            sampling_digest: evo_core::model_protocol::sampling_digest_v1(
                protocol_id,
                sampling_profile,
            )
            .unwrap(),
            request_timeout_seconds: 120,
        }
    }

    #[test]
    fn provider_current_public_keeps_healthy_parser_and_numeric_boundaries() {
        let mut record = provider_current_public_fixture();
        for endpoint in [
            "https://example.test",
            "https://example.test:443/v1",
            "https://[::1]:8443/v1",
            "https://example.test/path%40ok%3Fdata",
        ] {
            record.endpoint = endpoint.into();
            for timeout in [1, 120] {
                record.request_timeout_seconds = timeout;
                for tokens in [1, 32768] {
                    record.max_output_tokens = tokens;
                    validate_current_provider_public_fields(&record).unwrap();
                }
            }
        }
    }

    #[test]
    fn provider_current_public_refuses_credentials_queries_fragments_and_whitespace() {
        for endpoint in [
            "http://example.test",
            "HTTPS://example.test",
            "https://user@example.test",
            "https://user:secret@example.test",
            "https://example.test/path@name",
            "https://example.test?",
            "https://example.test#",
            "https://example.test?q=data",
            "https://example.test/path#part",
            " https://example.test",
            "https://example.test/path with-space",
            "https://",
            "not-a-url",
        ] {
            let mut record = provider_current_public_fixture();
            record.endpoint = endpoint.into();
            assert!(
                matches!(
                    validate_current_provider_public_fields(&record),
                    Err(Error::ClockRegressed)
                ),
                "{endpoint}"
            );
        }
    }

    #[test]
    fn provider_current_public_refuses_invalid_limits_and_bound_sampling() {
        for timeout in [0, 121, u32::MAX] {
            let mut record = provider_current_public_fixture();
            record.request_timeout_seconds = timeout;
            assert!(validate_current_provider_public_fields(&record).is_err());
        }
        for tokens in [0, 32769, u32::MAX] {
            let mut record = provider_current_public_fixture();
            record.max_output_tokens = tokens;
            assert!(validate_current_provider_public_fields(&record).is_err());
        }
        let mut record = provider_current_public_fixture();
        record.sampling_digest = "c".repeat(64);
        assert!(validate_current_provider_public_fields(&record).is_err());
        let mut record = provider_current_public_fixture();
        record.model_digest = "not-a-digest".into();
        assert!(validate_current_provider_public_fields(&record).is_err());
        let mut record = provider_current_public_fixture();
        record.provider_model = "bad model".into();
        assert!(validate_current_provider_public_fields(&record).is_err());
    }

    fn integer(raw: &str) -> Result<i64> {
        owned_nonnegative_integer(&parse_owned_json(raw)?)
    }

    #[test]
    fn sqlite_utc_calendar_keeps_epoch_and_real_gregorian_boundaries() {
        assert_eq!(parse_sqlite_utc_seconds("1970-01-01 00:00:00").unwrap(), 0);
        assert_eq!(
            parse_sqlite_utc_seconds("2000-02-29 00:00:00").unwrap(),
            951782400
        );
        assert_eq!(
            parse_sqlite_utc_seconds("2000-03-01 00:00:00").unwrap(),
            951868800
        );
        assert_eq!(
            parse_sqlite_utc_seconds("9999-12-31 23:59:59").unwrap(),
            253402300799
        );
    }
    #[test]
    fn sqlite_utc_refuses_coercion_offsets_bad_calendars_and_pre_epoch() {
        for raw in [
            "not-a-clock",
            "1969-12-31 23:59:59",
            "0000-01-01 00:00:00",
            "1900-02-29 00:00:00",
            "2100-02-29 00:00:00",
            "2001-02-29 00:00:00",
            "2026-04-31 00:00:00",
            "2026-00-01 00:00:00",
            "2026-13-01 00:00:00",
            "2026-10-18 24:00:00",
            "2026-10-18 00:60:00",
            "2026-10-18 00:00:60",
            "2026-10-18T00:00:00",
            "2026-10-18 00:00:00Z",
            "2026-10-18 00:00:00+00:00",
            "2026-10-18 00:00:00.0",
            " 2026-10-18 00:00:00",
            "2026-10-18 00:00:00 ",
            "２０２６-10-18 00:00:00",
        ] {
            assert!(
                matches!(parse_sqlite_utc_seconds(raw), Err(Error::ClockRegressed)),
                "{raw}"
            );
        }
    }
    #[test]
    fn empty_scan_has_no_floor() {
        assert_eq!(OccurrenceFloor::default().max_nanos(), None);
    }

    #[test]
    fn actual_epoch_zero_is_distinct_from_an_empty_scan() {
        let mut floor = OccurrenceFloor::default();
        floor.observe_seconds(0).unwrap();
        assert_eq!(floor.max_nanos(), Some(0));
        assert_ne!(floor, OccurrenceFloor::default());
    }

    #[test]
    fn seconds_maximum_does_not_wrap() {
        let mut floor = OccurrenceFloor::default();
        floor.observe_seconds(i64::MAX).unwrap();
        assert_eq!(
            floor.max_nanos(),
            Some(i128::from(i64::MAX) * NANOS_PER_SECOND)
        );
    }

    #[test]
    fn milliseconds_maximum_does_not_wrap_or_truncate() {
        let mut floor = OccurrenceFloor::default();
        floor.observe_millis(i64::MAX).unwrap();
        assert_eq!(
            floor.max_nanos(),
            Some(i128::from(i64::MAX) * NANOS_PER_MILLISECOND)
        );
    }

    #[test]
    fn mixed_units_keep_the_actual_subsecond_maximum() {
        let mut floor = OccurrenceFloor::default();
        floor.observe_seconds(7).unwrap();
        floor.observe_millis(7_500).unwrap();
        assert_eq!(floor.max_nanos(), Some(7_500_000_000));
        floor.observe_millis(7_499).unwrap();
        floor.observe_seconds(7).unwrap();
        assert_eq!(floor.max_nanos(), Some(7_500_000_000));
        floor.observe_seconds(8).unwrap();
        assert_eq!(floor.max_nanos(), Some(8_000_000_000));
    }

    #[test]
    fn rejected_observations_never_change_the_floor() {
        for initial in [None, Some(7_500)] {
            let mut floor = OccurrenceFloor::default();
            if let Some(millis) = initial {
                floor.observe_millis(millis).unwrap();
            }
            let before = floor.clone();
            assert!(matches!(
                floor.observe_seconds(-1),
                Err(Error::ClockRegressed)
            ));
            assert_eq!(floor, before);
            assert!(matches!(
                floor.observe_millis(i64::MIN),
                Err(Error::ClockRegressed)
            ));
            assert_eq!(floor, before);
        }
    }

    #[test]
    fn integer_decoder_preserves_large_exact_integers() {
        for (raw, expected) in [
            ("0", 0),
            ("9007199254740993", 9_007_199_254_740_993),
            ("9223372036854775807", i64::MAX),
        ] {
            assert_eq!(integer(raw).unwrap(), expected);
        }
    }

    #[test]
    fn integer_decoder_refuses_coercion_negative_values_and_overflow() {
        for raw in [
            "1.0",
            "1e0",
            "-0.0",
            "\"1\"",
            "true",
            "false",
            "null",
            "-1",
            "-9223372036854775808",
            "9223372036854775808",
            "18446744073709551615",
            "18446744073709551616",
            "1e400",
            "[]",
            "{}",
        ] {
            assert!(matches!(integer(raw), Err(Error::ClockRegressed)), "{raw}");
        }
    }

    #[test]
    fn duplicate_keys_are_rejected_at_every_container_level() {
        for raw in [
            r#"{"x":1,"x":2}"#,
            r#"{"k":1,"\u006b":2}"#,
            r#"{"é":1,"\u00e9":2}"#,
            r#"{"outer":{"x":1,"x":2}}"#,
            r#"[{"outer":[{"x":1,"x":2}]}]"#,
        ] {
            assert!(matches!(parse_owned_json(raw), Err(Error::ClockRegressed)));
        }
    }

    #[test]
    fn trailing_values_and_malformed_json_are_rejected() {
        for raw in ["0 1", "{}[]", "null trailing", "", "[", r#"{"x":1,}"#] {
            assert!(matches!(parse_owned_json(raw), Err(Error::ClockRegressed)));
        }
        assert_eq!(parse_owned_json(" \n 0\t ").unwrap(), Value::from(0));
    }

    #[test]
    fn valid_utf8_arrays_and_distinct_decoded_names_remain_intact() {
        let raw =
            r#"{ "名字": "e\u0301\né", "array": [2,1,{"v":null},true,"尾"], "k":0,"\u006c":1 }"#;
        let original = raw.as_bytes().to_vec();
        let value = parse_owned_json(raw).unwrap();
        assert_eq!(value["名字"], "e\u{301}\né");
        assert_eq!(
            value["array"],
            serde_json::json!([2, 1, {"v": null}, true, "尾"])
        );
        assert_eq!(value["k"], 0);
        assert_eq!(value["l"], 1);
        assert_eq!(raw.as_bytes(), original);
    }

    #[test]
    fn default_serde_recursion_limit_is_neither_removed_nor_lowered() {
        for depth in [1, 16, 126, 127, 128, 129, 256] {
            let raw = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
            assert_eq!(
                parse_owned_json(&raw).is_ok(),
                serde_json::from_str::<Value>(&raw).is_ok(),
                "nesting depth {depth}"
            );
        }
    }
}

// M29 streaming_evaluator.rs:3337-3828. The reducer replays accepted receipts
// from this same database snapshot; neither certificate copy supplies a bound,
// score, completion prefix, stopping reason or issuer. Pending/draining tickets
// retain their original phase rules and are not forced to be terminal here.
use evo_core::holdout::MonetaryReservationState;
use evo_core::sequential::{
    CompletePairedUnit, EarlyStopCertificateParts, EarlyStopDecision, SequentialRejectOnly,
};
fn evaluation_record<T: DeserializeOwned>(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    kind: &str,
    id: &str,
) -> Result<T> {
    let object = snapshot.object(namespace, "artifact", &evaluation_storage_id(kind, id)?)?;
    decode(envelope(object, "rsia.typed_artifact_envelope.v1", kind)?)
}
fn e05_fixed_grade(grader: &MirrorFixedGraderSpec, output: &str, expected: &str) -> Result<u32> {
    grader.validate()?;
    let output = parse_owned_json(output)?;
    exact(&output, &["answer"], &[])?;
    let expected = parse_owned_json(expected)?;
    match grader.method {
        MirrorFixedGraderMethod::ExactJsonAnswerV1 => Ok(if output["answer"] == expected {
            1_000_000
        } else {
            0
        }),
    }
}
fn e05_request_digest(
    ticket: &MirrorEvaluationTicketV2,
    target: &MirrorEvaluationTarget,
    side: MirrorExecutionSide,
    bundle_digest: &str,
    environment_digest: &str,
) -> Result<String> {
    typed_fingerprint(&(
        "rsia.evaluation_execution_request.v1",
        &ticket.id,
        &target.target_id,
        &target.input_digest,
        side,
        bundle_digest,
        environment_digest,
        &ticket.manifest_digest,
    ))
}
fn e05_execution_pair(
    ticket: &MirrorEvaluationTicketV2,
    target_id: &str,
    candidate: &MirrorExecutionReceiptV2,
    baseline: &MirrorExecutionReceiptV2,
) -> Result<()> {
    if candidate.ticket_id != ticket.id
        || baseline.ticket_id != ticket.id
        || candidate.target_id != target_id
        || baseline.target_id != target_id
        || candidate.side != MirrorExecutionSide::Candidate
        || baseline.side != MirrorExecutionSide::Baseline
        || candidate.bundle_digest != ticket.candidate_bundle_digest
        || baseline.bundle_digest != ticket.baseline_bundle_digest
        || candidate.environment_digest != ticket.environment_digest
        || baseline.environment_digest != ticket.environment_digest
    {
        return Err(Error::Conflict(
            "execution receipts do not form the frozen candidate/baseline pair".into(),
        ));
    }
    Ok(())
}
fn e05_output_artifact(
    receipt: &MirrorExecutionReceiptV2,
    output: &MirrorExecutionOutputArtifact,
) -> Result<()> {
    if output.schema_version != "rsia.execution_output.v1"
        || output.id != receipt.output_artifact_id
        || output.ticket_id != receipt.ticket_id
        || output.target_id != receipt.target_id
        || output.side != receipt.side
        || output.output_digest != receipt.output_digest
        || hash(output.output_utf8.as_bytes()) != receipt.output_digest
    {
        return Err(Error::Conflict(
            "execution output artifact does not match its receipt".into(),
        ));
    }
    Ok(())
}
fn e05_completed_target_evidence(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
    control: &MirrorRegisteredEvaluationControl,
    holdout: &MirrorProtectedHoldoutRecord,
    target: &MirrorEvaluationTarget,
) -> Result<MirrorIndependentGraderReceiptV2> {
    e05_completed_target_evidence_r39(
        &ActualCompleteRecordsR39(snapshot),
        namespace,
        ticket,
        control,
        holdout,
        target,
    )
}

fn e05_completed_target_evidence_r39<R: E05CompleteRecordsR39>(
    records: &R,
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
    control: &MirrorRegisteredEvaluationControl,
    holdout: &MirrorProtectedHoldoutRecord,
    target: &MirrorEvaluationTarget,
) -> Result<MirrorIndependentGraderReceiptV2> {
    let MirrorTargetState::Completed {
        candidate_score_micros,
        baseline_score_micros,
        anchor_result,
        candidate_receipt_id,
        baseline_receipt_id,
        grader_receipt_id,
        ..
    } = &target.state
    else {
        return Err(Error::Invalid(
            "formal evidence target is not completed".into(),
        ));
    };
    let candidate: MirrorExecutionReceiptV2 =
        records.record(namespace, "execution_receipt_v2", candidate_receipt_id)?;
    let baseline: MirrorExecutionReceiptV2 =
        records.record(namespace, "execution_receipt_v2", baseline_receipt_id)?;
    e05_execution_pair(ticket, &target.target_id, &candidate, &baseline)?;
    let candidate_output: MirrorExecutionOutputArtifact = records.record(
        namespace,
        "evaluation_execution_output_v1",
        &candidate.output_artifact_id,
    )?;
    let baseline_output: MirrorExecutionOutputArtifact = records.record(
        namespace,
        "evaluation_execution_output_v1",
        &baseline.output_artifact_id,
    )?;
    e05_output_artifact(&candidate, &candidate_output)?;
    e05_output_artifact(&baseline, &baseline_output)?;
    let oracle = holdout
        .oracle_entries
        .iter()
        .find(|entry| entry.target_id == target.target_id)
        .ok_or_else(|| Error::Invalid("stored oracle target missing".into()))?;
    let expected_candidate = e05_fixed_grade(
        &control.fixed_grader,
        &candidate_output.output_utf8,
        &oracle.expected_answer_json,
    )?;
    let expected_baseline = e05_fixed_grade(
        &control.fixed_grader,
        &baseline_output.output_utf8,
        &oracle.expected_answer_json,
    )?;
    let expected_anchor = match &target.kind {
        MirrorEvaluationTargetKind::Rotating { .. } => None,
        MirrorEvaluationTargetKind::Anchor { .. }
            if expected_baseline == 1_000_000 && expected_candidate == 1_000_000 =>
        {
            Some(AnchorResult::Passed)
        }
        MirrorEvaluationTargetKind::Anchor { .. } if expected_baseline == 1_000_000 => {
            Some(AnchorResult::CriticalRegression)
        }
        MirrorEvaluationTargetKind::Anchor { .. } => {
            return Err(Error::Invalid("stored anchor baseline is invalid".into()));
        }
    };
    let grader: MirrorIndependentGraderReceiptV2 = records.record(
        namespace,
        "independent_grader_receipt_v2",
        grader_receipt_id,
    )?;
    let expected_pair_digest = typed_fingerprint(&(
        &candidate.output_digest,
        &baseline.output_digest,
        &control.oracle_digest,
        &control.grader_digest,
        expected_candidate,
        expected_baseline,
        expected_anchor,
    ))?;
    if *candidate_score_micros != expected_candidate
        || *baseline_score_micros != expected_baseline
        || *anchor_result != expected_anchor
        || grader.ticket_id != ticket.id
        || grader.target_id != target.target_id
        || grader.candidate_execution_receipt_id != candidate.receipt_id
        || grader.baseline_execution_receipt_id != baseline.receipt_id
        || grader.candidate_output_digest != candidate.output_digest
        || grader.baseline_output_digest != baseline.output_digest
        || grader.oracle_digest != control.oracle_digest
        || grader.grader_digest != control.grader_digest
        || grader.evaluator_actor != ticket.evaluator_actor
        || grader.scored_output_pair_digest != expected_pair_digest
        || grader.candidate_score_micros != expected_candidate
        || grader.baseline_score_micros != expected_baseline
        || grader.anchor_result != expected_anchor
    {
        return Err(Error::Conflict(
            "stored grader evidence closure mismatch".into(),
        ));
    }
    Ok(grader)
}
fn e05_bound_early_certificate(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
    control: &MirrorRegisteredEvaluationControl,
    holdout: &MirrorProtectedHoldoutRecord,
    certificate: &EarlyStopCertificate,
) -> Result<()> {
    let calls: Vec<_> = snapshot
        .calls
        .values()
        .filter(|call| call.namespace == namespace && call.dispatch_group_id == ticket.id)
        .map(E05MathCallR39::from)
        .collect();
    e05_bound_early_certificate_r39(
        &ActualCompleteRecordsR39(snapshot),
        namespace,
        ticket,
        control,
        holdout,
        certificate,
        &calls,
    )
}

fn e05_bound_early_certificate_r39<R: E05CompleteRecordsR39>(
    records: &R,
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
    control: &MirrorRegisteredEvaluationControl,
    holdout: &MirrorProtectedHoldoutRecord,
    certificate: &EarlyStopCertificate,
    original_calls: &[E05MathCallR39],
) -> Result<()> {
    let changed = || Error::Conflict("formal_early_stop_certificate_changed".into());
    if ticket.candidate_dependency_binding.is_none()
        || ticket.lifecycle != MirrorTicketLifecycle::EarlyStopped
        || certificate.usage_uncertain()
    {
        return Err(changed());
    }
    ticket.validate()?;
    control.validate()?;
    holdout.validate_against(control)?;
    let pending = ticket.pending_certificate.as_ref().ok_or_else(changed)?;
    let mut bound_plan = control.v1_plan_snapshot.clone();
    bound_plan.bind_candidate(ticket.candidate_digest.clone())?;
    pending.validate_shape()?;
    if typed_fingerprint(pending)? != typed_fingerprint(certificate)?
        || ticket.registration_id != control.id
        || ticket.holdout_id != holdout.id
        || ticket.registration_digest != typed_fingerprint(control)?
        || ticket.manifest_digest != holdout.manifest.digest()?
        || ticket.v1_plan_snapshot_digest != control.v1_plan_snapshot.digest()?
        || ticket.bound_v1_plan_digest != bound_plan.digest()?
        || ticket.formal_plan_digest != control.formal_plan.digest()?
        || ticket.alpha_plan_digest != control.alpha_plan.digest()?
        || ticket.research_family_id != control.alpha_plan.research_family_id
        || ticket.attempt_id != control.attempt_id
        || ticket.profile != control.formal_plan.profile
        || ticket.candidate_digest != holdout.pair.candidate_digest
        || ticket.baseline_digest != holdout.pair.baseline_digest
        || ticket.candidate_bundle_digest != holdout.candidate_bundle_digest
        || ticket.baseline_bundle_digest != holdout.baseline_bundle_digest
        || ticket.environment_digest != holdout.manifest.environment_digest
        || ticket.executor_actor != control.executor_actor
        || ticket.evaluator_actor != control.evaluator_actor
        || ticket.evidence_scope != control.evidence_scope
        || ticket.n_planned != control.formal_plan.n_planned
        || ticket.n_planned as usize != holdout.manifest.ordered_task_cluster_ids.len()
    {
        return Err(changed());
    }
    let inputs = holdout
        .rotating_inputs
        .iter()
        .map(|input| (input, false))
        .chain(holdout.anchor_inputs.iter().map(|input| (input, true)))
        .collect::<Vec<_>>();
    if ticket.targets.len() != inputs.len()
        || ticket
            .targets
            .iter()
            .zip(inputs)
            .any(|(target, (input, anchor))| {
                target.target_id != input.target_id
                    || target.input_artifact_id != input.input_artifact_id
                    || target.input_digest != input.input_digest
                    || match &target.kind {
                        MirrorEvaluationTargetKind::Rotating { cluster_id } => {
                            anchor || cluster_id != &input.cluster_or_anchor_id
                        }
                        MirrorEvaluationTargetKind::Anchor { anchor_id } => {
                            !anchor || anchor_id != &input.cluster_or_anchor_id
                        }
                    }
            })
    {
        return Err(changed());
    }
    let mut stream = match &control.early_stop_plan {
        Some(plan) => {
            plan.validate_against(&control.alpha_plan)?;
            if plan.ordered_cluster_ids != holdout.manifest.ordered_task_cluster_ids {
                return Err(changed());
            }
            Some(SequentialRejectOnly::open(
                plan.clone(),
                &control.alpha_plan,
            )?)
        }
        None => None,
    };
    let calls: Vec<_> = original_calls
        .iter()
        .filter(|call| call.namespace == namespace && call.dispatch_group_id == ticket.id)
        .collect();
    let scopes: BTreeSet<_> = calls
        .iter()
        .map(|call| call.billing_scope.as_str())
        .collect();
    require(scopes == BTreeSet::from([control.billing_scope.as_str()]))?;
    let by_id = calls
        .iter()
        .map(|call| (call.call_id.as_str(), call))
        .collect::<BTreeMap<_, _>>();
    if by_id.len() != calls.len() {
        return Err(changed());
    }
    for call in &calls {
        if call.billing_scope != control.billing_scope
            || call.namespace != namespace
            || call.dispatch_group_id != ticket.id
            || call.stage != BudgetStage::TaskExecution
        {
            return Err(changed());
        }
        match call.state {
            BudgetCallState::Finalized => {
                if !call.execution_closed
                    || call.dispatch_id.is_none()
                    || call.dispatched_at.is_none()
                    || call.finalized_at.is_none()
                    || call.execution_closed_at.is_none()
                    || !call.actual_cost_micros.is_some_and(|amount| amount >= 0)
                    || call.actual_currency.as_deref()
                        != Some(control.optimization_budget.root_total.currency.as_str())
                    || call.actual_pricing_version.as_deref()
                        != Some(
                            control
                                .optimization_budget
                                .root_total
                                .pricing_version
                                .as_str(),
                        )
                {
                    return Err(changed());
                }
            }
            BudgetCallState::Released | BudgetCallState::Cancelled => {
                if call.dispatch_id.is_some() || call.dispatched_at.is_some() {
                    return Err(changed());
                }
            }
            _ => return Err(changed()),
        }
    }
    let mut used_calls = BTreeSet::new();
    let mut receipt_ids = BTreeSet::new();
    let mut grader_ids = BTreeSet::new();
    let mut sequences = BTreeSet::new();
    let mut events = Vec::new();
    for target in &ticket.targets {
        match &target.state {
            MirrorTargetState::Completed {
                candidate_receipt_id,
                baseline_receipt_id,
                grader_receipt_id,
                completion_sequence,
                ..
            } => {
                if !sequences.insert(*completion_sequence)
                    || !grader_ids.insert(grader_receipt_id.as_str())
                {
                    return Err(changed());
                }
                let grader = e05_completed_target_evidence_r39(
                    records, namespace, ticket, control, holdout, target,
                )?;
                if grader.schema_version != "rsia.independent_grader_receipt.v2"
                    || grader.receipt_id != *grader_receipt_id
                {
                    return Err(changed());
                }
                valid_time(grader.issued_at_unix_seconds)?;
                for (id, side) in [
                    (candidate_receipt_id, MirrorExecutionSide::Candidate),
                    (baseline_receipt_id, MirrorExecutionSide::Baseline),
                ] {
                    if !receipt_ids.insert(id.as_str()) {
                        return Err(changed());
                    }
                    let receipt: MirrorExecutionReceiptV2 =
                        records.record(namespace, "execution_receipt_v2", id)?;
                    let bundle = match side {
                        MirrorExecutionSide::Candidate => &ticket.candidate_bundle_digest,
                        MirrorExecutionSide::Baseline => &ticket.baseline_bundle_digest,
                    };
                    let request = e05_request_digest(
                        ticket,
                        target,
                        side,
                        bundle,
                        &ticket.environment_digest,
                    )?;
                    let call = by_id
                        .get(receipt.budget_call_id.as_str())
                        .ok_or_else(changed)?;
                    if receipt.schema_version != "rsia.execution_receipt.v2"
                        || receipt.receipt_id != *id
                        || receipt.executor_actor != ticket.executor_actor
                        || receipt.request_digest != request
                        || !used_calls.insert(call.call_id.as_str())
                        || call.state != BudgetCallState::Finalized
                        || call.actual_input_digest != request
                        || call.output_digest.as_deref() != Some(receipt.output_digest.as_str())
                    {
                        return Err(changed());
                    }
                }
                events.push((*completion_sequence, target, grader));
            }
            MirrorTargetState::CancelledAfterDispatch {
                candidate,
                baseline,
            } => {
                for slot in [candidate, baseline].into_iter().flatten() {
                    let call = by_id
                        .get(slot.budget_call_id.as_str())
                        .ok_or_else(changed)?;
                    let bundle = match slot.side {
                        MirrorExecutionSide::Candidate => &ticket.candidate_bundle_digest,
                        MirrorExecutionSide::Baseline => &ticket.baseline_bundle_digest,
                    };
                    let request = e05_request_digest(
                        ticket,
                        target,
                        slot.side,
                        bundle,
                        &ticket.environment_digest,
                    )?;
                    if !used_calls.insert(call.call_id.as_str())
                        || slot.request_digest != request
                        || call.actual_input_digest != request
                    {
                        return Err(changed());
                    }
                    match &slot.phase {
                        MirrorExecutionSlotPhase::Dispatched {
                            dispatch_id,
                            dispatched_at_unix_seconds,
                        } if call.state == BudgetCallState::Finalized
                            && call.dispatch_id.as_deref() == Some(dispatch_id.as_str())
                            && call.dispatched_at == Some(*dispatched_at_unix_seconds) => {}
                        MirrorExecutionSlotPhase::ReservedBound
                            if matches!(
                                call.state,
                                BudgetCallState::Released | BudgetCallState::Cancelled
                            ) => {}
                        _ => return Err(changed()),
                    }
                }
            }
            MirrorTargetState::NotDispatched { released_call_ids } => {
                for id in released_call_ids {
                    let call = by_id.get(id.as_str()).ok_or_else(changed)?;
                    if !used_calls.insert(call.call_id.as_str())
                        || !matches!(
                            call.state,
                            BudgetCallState::Released | BudgetCallState::Cancelled
                        )
                    {
                        return Err(changed());
                    }
                }
            }
            _ => return Err(changed()),
        }
    }
    // Already recorded unattributed calls remain paid/audit facts; they do not
    // become accepted statistical units or disappear from terminal accounting.
    for id in &ticket.unattributed_budget_call_ids {
        if !by_id.contains_key(id.as_str()) || !used_calls.insert(id.as_str()) {
            return Err(changed());
        }
    }
    let count = u64::try_from(events.len()).map_err(|_| changed())?;
    if count == 0
        || !sequences.iter().copied().eq(1..=count)
        || ticket.next_sequence != count.checked_add(1).ok_or_else(changed)?
        || used_calls.len() != by_id.len()
    {
        return Err(changed());
    }
    events.sort_by_key(|(sequence, _, _)| *sequence);
    let ledger = records.exposure(namespace, ticket)?;
    let entry = &ledger.entry;
    let first_feedback_at = events[0]
        .2
        .issued_at_unix_seconds
        .checked_mul(1_000)
        .ok_or_else(changed)?;
    if ledger.research_family_id != ticket.research_family_id
        || ledger.alpha_plan_digest != ticket.alpha_plan_digest
        || entry.manifest_digest != ticket.manifest_digest
        || entry.candidate_pair_digest != holdout.manifest.candidate_pair_digest
        || entry.epoch_id != holdout.manifest.epoch_id
        || entry.slice_id != holdout.manifest.slice_id
        || entry.task_cluster_ids != holdout.manifest.ordered_task_cluster_ids
        || entry.monetary_state != MonetaryReservationState::Finalized
        || !matches!(
            &entry.state,
            ExposureState::FeedbackUsed { used_at_unix_ms } if *used_at_unix_ms == first_feedback_at
        )
    {
        return Err(changed());
    }
    let mut first_stop = None;
    for (position, (sequence, target, grader)) in events.iter().enumerate() {
        let decision = match &target.kind {
            MirrorEvaluationTargetKind::Anchor { .. }
                if grader.anchor_result == Some(AnchorResult::CriticalRegression) =>
            {
                EarlyStopDecision::CriticalRegression
            }
            MirrorEvaluationTargetKind::Rotating { cluster_id } => match stream.as_mut() {
                Some(stream) => stream.record_complete_unit(CompletePairedUnit {
                    cluster_id: cluster_id.clone(),
                    baseline_score_micros: grader.baseline_score_micros,
                    candidate_score_micros: grader.candidate_score_micros,
                })?,
                None => EarlyStopDecision::Continue,
            },
            _ => EarlyStopDecision::Continue,
        };
        if decision.is_early_stop() {
            if position + 1 != events.len() {
                return Err(changed());
            }
            let (k, lcb, ucb, prefix) = if decision == EarlyStopDecision::CriticalRegression {
                (0, None, None, typed_fingerprint(&Vec::<String>::new())?)
            } else {
                let stream = stream.as_ref().ok_or_else(changed)?;
                let bound = stream.boundary()?.ok_or_else(changed)?;
                let wire = bound.to_wire()?;
                (
                    bound.k,
                    Some(wire.lcb),
                    Some(wire.ucb),
                    stream.completed_prefix_digest()?,
                )
            };
            first_stop = Some((
                decision,
                k,
                lcb,
                ucb,
                prefix,
                grader,
                sequence.checked_add(1).ok_or_else(changed)?,
            ));
            break;
        }
    }
    let (decision, k, lcb, ucb, prefix, issuer, stop_sequence) = first_stop.ok_or_else(changed)?;
    let expected = EarlyStopCertificate::from_verified_parts(EarlyStopCertificateParts {
        ticket_id: ticket.id.clone(),
        v1_plan_snapshot_digest: ticket.v1_plan_snapshot_digest.clone(),
        early_stop_plan_digest: control
            .early_stop_plan
            .as_ref()
            .map(EarlyStopPlan::digest)
            .transpose()?
            .unwrap_or(control.formal_plan.digest()?),
        alpha_plan_digest: ticket.alpha_plan_digest.clone(),
        manifest_digest: ticket.manifest_digest.clone(),
        slice_id: holdout.manifest.slice_id.clone(),
        candidate_digest: ticket.candidate_digest.clone(),
        baseline_digest: ticket.baseline_digest.clone(),
        grader_digest: control.grader_digest.clone(),
        issuer_receipt_digest: typed_fingerprint(issuer)?,
        k,
        lcb,
        ucb,
        completed_prefix_digest: prefix,
        // This is the frozen stop snapshot, not the later reconciled targets.
        member_terminal_digest: ticket
            .stop_member_terminal_digest
            .clone()
            .ok_or_else(changed)?,
        stop_reason: decision,
        stopped_at_unix_ms: issuer
            .issued_at_unix_seconds
            .checked_mul(1_000)
            .ok_or_else(changed)?,
        stop_sequence,
        usage_uncertain: false,
    })?;
    if typed_fingerprint(&expected)? != typed_fingerprint(certificate)? {
        return Err(changed());
    }
    Ok(())
}

fn e05_native_early_certificate_r44<R: E05CompleteRecordsR39>(
    records: &R,
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
    control: &MirrorRegisteredEvaluationControl,
    holdout: &MirrorProtectedHoldoutRecord,
    certificate: &EarlyStopCertificate,
    original_calls: &[E05MathCallR39],
) -> Result<()> {
    let changed = || Error::Conflict("formal_early_stop_certificate_changed".into());
    if ticket.lifecycle != MirrorTicketLifecycle::EarlyStopped || certificate.usage_uncertain() {
        return Err(changed());
    }
    if ticket.candidate_dependency_binding.is_none()
        && ticket.evidence_scope != MirrorEvaluationEvidenceScope::ProgramFixture
    {
        return Err(Error::Forbidden);
    }
    ticket.validate()?;
    control.validate()?;
    holdout.validate_against(control)?;
    let pending = ticket.pending_certificate.as_ref().ok_or_else(changed)?;
    let mut bound_plan = control.v1_plan_snapshot.clone();
    bound_plan.bind_candidate(ticket.candidate_digest.clone())?;
    pending.validate_shape()?;
    if typed_fingerprint(pending)? != typed_fingerprint(certificate)?
        || ticket.registration_id != control.id
        || ticket.holdout_id != holdout.id
        || ticket.registration_digest != typed_fingerprint(control)?
        || ticket.manifest_digest != holdout.manifest.digest()?
        || ticket.v1_plan_snapshot_digest != control.v1_plan_snapshot.digest()?
        || ticket.bound_v1_plan_digest != bound_plan.digest()?
        || ticket.formal_plan_digest != control.formal_plan.digest()?
        || ticket.alpha_plan_digest != control.alpha_plan.digest()?
        || ticket.research_family_id != control.alpha_plan.research_family_id
        || ticket.attempt_id != control.attempt_id
        || ticket.profile != control.formal_plan.profile
        || ticket.candidate_digest != holdout.pair.candidate_digest
        || ticket.baseline_digest != holdout.pair.baseline_digest
        || ticket.candidate_bundle_digest != holdout.candidate_bundle_digest
        || ticket.baseline_bundle_digest != holdout.baseline_bundle_digest
        || ticket.environment_digest != holdout.manifest.environment_digest
        || ticket.executor_actor != control.executor_actor
        || ticket.evaluator_actor != control.evaluator_actor
        || ticket.evidence_scope != control.evidence_scope
        || ticket.n_planned != control.formal_plan.n_planned
        || ticket.n_planned as usize != holdout.manifest.ordered_task_cluster_ids.len()
    {
        return Err(changed());
    }
    let inputs = holdout
        .rotating_inputs
        .iter()
        .map(|input| (input, false))
        .chain(holdout.anchor_inputs.iter().map(|input| (input, true)))
        .collect::<Vec<_>>();
    if ticket.targets.len() != inputs.len()
        || ticket
            .targets
            .iter()
            .zip(inputs)
            .any(|(target, (input, anchor))| {
                target.target_id != input.target_id
                    || target.input_artifact_id != input.input_artifact_id
                    || target.input_digest != input.input_digest
                    || match &target.kind {
                        MirrorEvaluationTargetKind::Rotating { cluster_id } => {
                            anchor || cluster_id != &input.cluster_or_anchor_id
                        }
                        MirrorEvaluationTargetKind::Anchor { anchor_id } => {
                            !anchor || anchor_id != &input.cluster_or_anchor_id
                        }
                    }
            })
    {
        return Err(changed());
    }
    let mut stream = match &control.early_stop_plan {
        Some(plan) => {
            plan.validate_against(&control.alpha_plan)?;
            if plan.ordered_cluster_ids != holdout.manifest.ordered_task_cluster_ids {
                return Err(changed());
            }
            Some(SequentialRejectOnly::open(
                plan.clone(),
                &control.alpha_plan,
            )?)
        }
        None => None,
    };
    let calls: Vec<_> = original_calls
        .iter()
        .filter(|call| call.namespace == namespace && call.dispatch_group_id == ticket.id)
        .collect();
    let scopes: BTreeSet<_> = calls
        .iter()
        .map(|call| call.billing_scope.as_str())
        .collect();
    require(scopes == BTreeSet::from([control.billing_scope.as_str()]))?;
    let by_id = calls
        .iter()
        .map(|call| (call.call_id.as_str(), call))
        .collect::<BTreeMap<_, _>>();
    if by_id.len() != calls.len() {
        return Err(changed());
    }
    for call in &calls {
        if call.billing_scope != control.billing_scope
            || call.namespace != namespace
            || call.dispatch_group_id != ticket.id
            || call.stage != BudgetStage::TaskExecution
        {
            return Err(changed());
        }
        match call.state {
            BudgetCallState::Finalized => {
                if !call.execution_closed
                    || call.dispatch_id.is_none()
                    || call.dispatched_at.is_none()
                    || call.finalized_at.is_none()
                    || call.execution_closed_at.is_none()
                    || !call.actual_cost_micros.is_some_and(|amount| amount >= 0)
                    || call.actual_currency.as_deref()
                        != Some(control.optimization_budget.root_total.currency.as_str())
                    || call.actual_pricing_version.as_deref()
                        != Some(
                            control
                                .optimization_budget
                                .root_total
                                .pricing_version
                                .as_str(),
                        )
                {
                    return Err(changed());
                }
            }
            BudgetCallState::Released | BudgetCallState::Cancelled => {
                if call.dispatch_id.is_some() || call.dispatched_at.is_some() {
                    return Err(changed());
                }
            }
            _ => return Err(changed()),
        }
    }
    let mut used_calls = BTreeSet::new();
    let mut receipt_ids = BTreeSet::new();
    let mut grader_ids = BTreeSet::new();
    let mut sequences = BTreeSet::new();
    let mut events = Vec::new();
    for target in &ticket.targets {
        match &target.state {
            MirrorTargetState::Completed {
                candidate_receipt_id,
                baseline_receipt_id,
                grader_receipt_id,
                completion_sequence,
                ..
            } => {
                if !sequences.insert(*completion_sequence)
                    || !grader_ids.insert(grader_receipt_id.as_str())
                {
                    return Err(changed());
                }
                let grader = e05_completed_target_evidence_r39(
                    records, namespace, ticket, control, holdout, target,
                )?;
                if grader.schema_version != "rsia.independent_grader_receipt.v2"
                    || grader.receipt_id != *grader_receipt_id
                {
                    return Err(changed());
                }
                valid_time(grader.issued_at_unix_seconds)?;
                for (id, side) in [
                    (candidate_receipt_id, MirrorExecutionSide::Candidate),
                    (baseline_receipt_id, MirrorExecutionSide::Baseline),
                ] {
                    if !receipt_ids.insert(id.as_str()) {
                        return Err(changed());
                    }
                    let receipt: MirrorExecutionReceiptV2 =
                        records.record(namespace, "execution_receipt_v2", id)?;
                    let bundle = match side {
                        MirrorExecutionSide::Candidate => &ticket.candidate_bundle_digest,
                        MirrorExecutionSide::Baseline => &ticket.baseline_bundle_digest,
                    };
                    let request = e05_request_digest(
                        ticket,
                        target,
                        side,
                        bundle,
                        &ticket.environment_digest,
                    )?;
                    let call = by_id
                        .get(receipt.budget_call_id.as_str())
                        .ok_or_else(changed)?;
                    if receipt.schema_version != "rsia.execution_receipt.v2"
                        || receipt.receipt_id != *id
                        || receipt.executor_actor != ticket.executor_actor
                        || receipt.request_digest != request
                        || !used_calls.insert(call.call_id.as_str())
                        || call.state != BudgetCallState::Finalized
                        || call.actual_input_digest != request
                        || call.output_digest.as_deref() != Some(receipt.output_digest.as_str())
                    {
                        return Err(changed());
                    }
                }
                events.push((*completion_sequence, target, grader));
            }
            MirrorTargetState::CancelledAfterDispatch {
                candidate,
                baseline,
            } => {
                for slot in [candidate, baseline].into_iter().flatten() {
                    let call = by_id
                        .get(slot.budget_call_id.as_str())
                        .ok_or_else(changed)?;
                    let bundle = match slot.side {
                        MirrorExecutionSide::Candidate => &ticket.candidate_bundle_digest,
                        MirrorExecutionSide::Baseline => &ticket.baseline_bundle_digest,
                    };
                    let request = e05_request_digest(
                        ticket,
                        target,
                        slot.side,
                        bundle,
                        &ticket.environment_digest,
                    )?;
                    if !used_calls.insert(call.call_id.as_str())
                        || slot.request_digest != request
                        || call.actual_input_digest != request
                    {
                        return Err(changed());
                    }
                    match &slot.phase {
                        MirrorExecutionSlotPhase::Dispatched {
                            dispatch_id,
                            dispatched_at_unix_seconds,
                        } if call.state == BudgetCallState::Finalized
                            && call.dispatch_id.as_deref() == Some(dispatch_id.as_str())
                            && call.dispatched_at == Some(*dispatched_at_unix_seconds) => {}
                        MirrorExecutionSlotPhase::ReservedBound
                            if matches!(
                                call.state,
                                BudgetCallState::Released | BudgetCallState::Cancelled
                            ) => {}
                        _ => return Err(changed()),
                    }
                }
            }
            MirrorTargetState::NotDispatched { released_call_ids } => {
                for id in released_call_ids {
                    let call = by_id.get(id.as_str()).ok_or_else(changed)?;
                    if !used_calls.insert(call.call_id.as_str())
                        || !matches!(
                            call.state,
                            BudgetCallState::Released | BudgetCallState::Cancelled
                        )
                    {
                        return Err(changed());
                    }
                }
            }
            _ => return Err(changed()),
        }
    }
    // Already recorded unattributed calls remain paid/audit facts; they do not
    // become accepted statistical units or disappear from terminal accounting.
    for id in &ticket.unattributed_budget_call_ids {
        if !by_id.contains_key(id.as_str()) || !used_calls.insert(id.as_str()) {
            return Err(changed());
        }
    }
    let count = u64::try_from(events.len()).map_err(|_| changed())?;
    if count == 0
        || !sequences.iter().copied().eq(1..=count)
        || ticket.next_sequence != count.checked_add(1).ok_or_else(changed)?
        || used_calls.len() != by_id.len()
    {
        return Err(changed());
    }
    events.sort_by_key(|(sequence, _, _)| *sequence);
    let ledger = records.exposure(namespace, ticket)?;
    let entry = &ledger.entry;
    let first_feedback_at = events[0]
        .2
        .issued_at_unix_seconds
        .checked_mul(1_000)
        .ok_or_else(changed)?;
    if ledger.research_family_id != ticket.research_family_id
        || ledger.alpha_plan_digest != ticket.alpha_plan_digest
        || entry.manifest_digest != ticket.manifest_digest
        || entry.candidate_pair_digest != holdout.manifest.candidate_pair_digest
        || entry.epoch_id != holdout.manifest.epoch_id
        || entry.slice_id != holdout.manifest.slice_id
        || entry.task_cluster_ids != holdout.manifest.ordered_task_cluster_ids
        || entry.monetary_state != MonetaryReservationState::Finalized
        || !matches!(
            &entry.state,
            ExposureState::FeedbackUsed { used_at_unix_ms } if *used_at_unix_ms == first_feedback_at
        )
    {
        return Err(changed());
    }
    let mut first_stop = None;
    for (position, (sequence, target, grader)) in events.iter().enumerate() {
        let decision = match &target.kind {
            MirrorEvaluationTargetKind::Anchor { .. }
                if grader.anchor_result == Some(AnchorResult::CriticalRegression) =>
            {
                EarlyStopDecision::CriticalRegression
            }
            MirrorEvaluationTargetKind::Rotating { cluster_id } => match stream.as_mut() {
                Some(stream) => stream.record_complete_unit(CompletePairedUnit {
                    cluster_id: cluster_id.clone(),
                    baseline_score_micros: grader.baseline_score_micros,
                    candidate_score_micros: grader.candidate_score_micros,
                })?,
                None => EarlyStopDecision::Continue,
            },
            _ => EarlyStopDecision::Continue,
        };
        if decision.is_early_stop() {
            if position + 1 != events.len() {
                return Err(changed());
            }
            let (k, lcb, ucb, prefix) = if decision == EarlyStopDecision::CriticalRegression {
                (0, None, None, typed_fingerprint(&Vec::<String>::new())?)
            } else {
                let stream = stream.as_ref().ok_or_else(changed)?;
                let bound = stream.boundary()?.ok_or_else(changed)?;
                let wire = bound.to_wire()?;
                (
                    bound.k,
                    Some(wire.lcb),
                    Some(wire.ucb),
                    stream.completed_prefix_digest()?,
                )
            };
            first_stop = Some((
                decision,
                k,
                lcb,
                ucb,
                prefix,
                grader,
                sequence.checked_add(1).ok_or_else(changed)?,
            ));
            break;
        }
    }
    let (decision, k, lcb, ucb, prefix, issuer, stop_sequence) = first_stop.ok_or_else(changed)?;
    let expected = EarlyStopCertificate::from_verified_parts(EarlyStopCertificateParts {
        ticket_id: ticket.id.clone(),
        v1_plan_snapshot_digest: ticket.v1_plan_snapshot_digest.clone(),
        early_stop_plan_digest: control
            .early_stop_plan
            .as_ref()
            .map(EarlyStopPlan::digest)
            .transpose()?
            .unwrap_or(control.formal_plan.digest()?),
        alpha_plan_digest: ticket.alpha_plan_digest.clone(),
        manifest_digest: ticket.manifest_digest.clone(),
        slice_id: holdout.manifest.slice_id.clone(),
        candidate_digest: ticket.candidate_digest.clone(),
        baseline_digest: ticket.baseline_digest.clone(),
        grader_digest: control.grader_digest.clone(),
        issuer_receipt_digest: typed_fingerprint(issuer)?,
        k,
        lcb,
        ucb,
        completed_prefix_digest: prefix,
        // This is the frozen stop snapshot, not the later reconciled targets.
        member_terminal_digest: ticket
            .stop_member_terminal_digest
            .clone()
            .ok_or_else(changed)?,
        stop_reason: decision,
        stopped_at_unix_ms: issuer
            .issued_at_unix_seconds
            .checked_mul(1_000)
            .ok_or_else(changed)?,
        stop_sequence,
        usage_uncertain: false,
    })?;
    if typed_fingerprint(&expected)? != typed_fingerprint(certificate)? {
        return Err(changed());
    }
    Ok(())
}
fn e05_dependency_payload_identity(kind: &str, payload: &Value) -> Result<()> {
    if kind == "evaluation_ticket_issue_receipt_v2" {
        let seconds = nonnegative(payload, "issued_at_unix_seconds")?;
        let millis = nonnegative(payload, "issued_at_unix_ms")?;
        require(seconds == millis / 1000)?;
    }
    let (ids, digests, timestamp): (&[&str], &[&str], Option<&str>) = match kind {
        "evaluation_ticket_issue_receipt_v1" | "evaluation_ticket_issue_receipt_v2" => (
            &["ticket_id", "evaluator_actor"],
            &["registration_digest", "manifest_digest"],
            Some("issued_at_unix_seconds"),
        ),
        "alpha_claim_reservation_v1" => (
            &["research_family_id", "claim_id", "attempt_id", "ticket_id"],
            &[],
            None,
        ),
        "execution_receipt_v2" => (
            &[
                "receipt_id",
                "ticket_id",
                "target_id",
                "output_artifact_id",
                "budget_call_id",
                "executor_actor",
            ],
            &[
                "request_digest",
                "bundle_digest",
                "environment_digest",
                "output_digest",
            ],
            Some("issued_at_unix_seconds"),
        ),
        "independent_grader_receipt_v2" => (
            &[
                "receipt_id",
                "ticket_id",
                "target_id",
                "candidate_execution_receipt_id",
                "baseline_execution_receipt_id",
                "evaluator_actor",
            ],
            &[
                "candidate_output_digest",
                "baseline_output_digest",
                "oracle_digest",
                "grader_digest",
                "scored_output_pair_digest",
            ],
            Some("issued_at_unix_seconds"),
        ),
        "evaluation_execution_output_v1" => {
            (&["id", "ticket_id", "target_id"], &["output_digest"], None)
        }
        "evaluation_resource_evidence_v1" => (&["ticket_id", "billing_scope"], &[], None),
        // These full native validators also validate the rest of their contracts.
        "registered_evaluation_control_v41" | "protected_holdout_v41" | "evaluation_ticket_v2" => {
            (&[], &[], None)
        }
        _ => {
            return Err(Error::Invalid(
                "unsupported formal dependency record kind".into(),
            ));
        }
    };
    for field in ids {
        identifier(
            payload
                .get(field)
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Invalid(format!("formal dependency lacks {field}")))?,
        )?;
    }
    for field in digests {
        wire_digest(
            payload
                .get(field)
                .and_then(Value::as_str)
                .ok_or_else(|| Error::Invalid(format!("formal dependency lacks {field}")))?,
            field,
        )?;
    }
    if let Some(field) = timestamp {
        valid_time(
            payload
                .get(field)
                .and_then(Value::as_i64)
                .ok_or_else(|| Error::Invalid(format!("formal dependency lacks {field}")))?,
        )?;
    }
    if kind == "independent_grader_receipt_v2" {
        for field in ["candidate_score_micros", "baseline_score_micros"] {
            if payload
                .get(field)
                .and_then(Value::as_u64)
                .is_none_or(|value| value > 1_000_000)
            {
                return Err(Error::Invalid(
                    "formal dependency score is out of bounds".into(),
                ));
            }
        }
    }
    Ok(())
}

// This is the production generic envelope's field order, not Value's map order.
// FormalDependencyNode content pins the complete typed envelope for E05 facts.
#[derive(Serialize)]
struct E05FingerprintEnvelope<'a, T: Serialize> {
    schema_version: &'a str,
    id: &'a str,
    record_kind: &'a str,
    payload: &'a T,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MirrorAlphaClaimReservation {
    schema_version: String,
    research_family_id: String,
    claim_id: String,
    attempt_id: String,
    ticket_id: String,
}
fn e05_typed_envelope_fingerprint<T: DeserializeOwned + Serialize>(
    object: &OccurrenceObject,
    kind: &str,
    payload: &Value,
) -> Result<String> {
    let record: T = decode(payload)?;
    if object.body.get("schema_version").and_then(Value::as_str)
        == Some(evo_core::formal_version::NATIVE_TICKET_ENVELOPE)
    {
        require(kind == "evaluation_ticket_v2")?;
        evo_core::formal_version::native_ticket_payload(
            &object.body,
            &object.namespace,
            &object.id,
        )
        .map_err(|_| Error::ClockRegressed)?;
        let typed: evo_core::formal_version::NativeTicketEnvelopeV2<T> = decode(&object.body)?;
        return typed_fingerprint(&typed);
    }
    typed_fingerprint(&E05FingerprintEnvelope {
        schema_version: "rsia.typed_artifact_envelope.v1",
        id: &object.id,
        record_kind: kind,
        payload: &record,
    })
}
fn e05_outgoing(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    kind: &str,
    id: &str,
) -> Result<Vec<(String, String)>> {
    // All five fields already use the BTreeSet's lexicographic order. Empty
    // destinations are the exact lower bound for this complete source prefix.
    let start = (
        namespace.to_owned(),
        kind.to_owned(),
        id.to_owned(),
        String::new(),
        String::new(),
    );
    let edges: Vec<_> = snapshot
        .edges
        .range(start..)
        .take_while(|(ns, k, i, _, _)| ns == namespace && k == kind && i == id)
        .map(|(_, _, _, k, i)| (k.clone(), i.clone()))
        .collect();
    require(edges.len() <= 10000)?;
    Ok(edges)
}

fn e05_dependency_live(snapshot: &OccurrenceSnapshot, object: &OccurrenceObject) -> Result<()> {
    if matches!(object.kind.as_str(), "run" | "artifact")
        && let Some(tombstone) = snapshot.objects.get(&(
            object.namespace.clone(),
            "tombstone".into(),
            object.id.clone(),
        ))
    {
        let tombstone: crate::lifecycle::RevokeTombstone = decode(&tombstone.body)?;
        require(
            matches!(tombstone.source_kind.as_str(), "run" | "artifact")
                && tombstone.source_kind != object.kind,
        )?;
    }
    require(
        object.kind == "run"
            || object.body.get("schema_version").and_then(Value::as_str)
                != Some("rsia.redacted.v1"),
    )
}
fn e05_dependency_identity(
    snapshot: &OccurrenceSnapshot,
    object: &OccurrenceObject,
) -> Result<(String, Option<String>, String, String)> {
    e05_dependency_live(snapshot, object)?;
    require(
        serde_json::to_vec(&object.body)
            .map_err(|_| Error::ClockRegressed)?
            .len()
            <= 4 * 1024 * 1024,
    )?;
    if object.kind == "run" {
        validate_native_authority(object)?;
        let record: MirrorStoredTraceAuthority = decode(&object.body)?;
        return Ok((
            record.schema_version.clone(),
            None,
            object.id.clone(),
            typed_fingerprint(&record)?,
        ));
    }
    require(matches!(
        object.kind.as_str(),
        "artifact" | "formal_parent" | "release" | "pointer" | "receipt"
    ))?;
    if object.kind == "artifact"
        && object.id.starts_with("optgrant-")
        && object.body.get("schema_version").is_none()
    {
        let grant: evo_core::evidence::SourceSelection = decode(&object.body)?;
        grant.validate().map_err(|_| Error::ClockRegressed)?;
        require(object.id == format!("optgrant-{}", typed_fingerprint(&grant)?))?;
        return Ok((
            "rsia.legacy_source_selection.v1".into(),
            None,
            object.id.clone(),
            typed_fingerprint(&object.body)?,
        ));
    }
    let schema = string(&object.body, "schema_version")?;
    if compiler_node_schema(&object.kind, schema) {
        if schema == "rsia.exploration_artifact_envelope.v1" {
            qualify_exploration(object)?;
        } else {
            time_free_record(object, snapshot)?;
        }
        e05_compiler_authority(snapshot, object)?;
        return Ok((
            schema.into(),
            None,
            object.id.clone(),
            typed_fingerprint(&object.body)?,
        ));
    }
    require(object.kind == "artifact")?;
    if schema == "rsia.release_candidate.v1" {
        time_free_record(object, snapshot)?;
        let candidate: MirrorReleaseCandidateRecord = decode(&object.body)?;
        e05_builtin_parent_strategy(&candidate.bundle.improver)?;
        let watermark = snapshot
            .watermarks
            .get(&object.namespace)
            .ok_or(Error::ClockRegressed)?;
        require(candidate.revoke_watermark == watermark.0)?;
        for source in &candidate.sources {
            require(!snapshot.objects.contains_key(&(
                object.namespace.clone(),
                "tombstone".into(),
                source.id.clone(),
            )))?;
        }
        return Ok((
            schema.into(),
            None,
            object.id.clone(),
            typed_fingerprint(&candidate)?,
        ));
    }
    require(
        schema == "rsia.typed_artifact_envelope.v1"
            || (schema == evo_core::formal_version::NATIVE_TICKET_ENVELOPE
                && string(&object.body, "record_kind")? == "evaluation_ticket_v2"),
    )?;
    let kind = string(&object.body, "record_kind")?;
    let payload = envelope(object, schema, kind)?;
    let expected = dependency_payload_schema(kind).ok_or(Error::ClockRegressed)?;
    let actual = string(payload, "schema_version")?;
    require(
        actual == expected
            || (kind == "registered_evaluation_control_v41"
                && actual == "rsia.registered_evaluation_control.v1"),
    )?;
    e05_dependency_payload_identity(kind, payload)?;
    let logical = match kind {
        "registered_evaluation_control_v41"
        | "protected_holdout_v41"
        | "evaluation_ticket_v2"
        | "evaluation_execution_output_v1" => string(payload, "id")?.to_owned(),
        "execution_receipt_v2" | "independent_grader_receipt_v2" => {
            string(payload, "receipt_id")?.to_owned()
        }
        "alpha_claim_reservation_v1" => format!(
            "{}:{}",
            string(payload, "research_family_id")?,
            string(payload, "claim_id")?
        ),
        _ => string(payload, "ticket_id")?.to_owned(),
    };
    require(object.id == evaluation_storage_id(kind, &logical)?)?;
    macro_rules! fp {
        ($ty:ty) => {
            e05_typed_envelope_fingerprint::<$ty>(object, kind, payload)?
        };
    }
    let content = match kind {
        "registered_evaluation_control_v41" => {
            evaluation_control(snapshot, &object.namespace, &logical)?;
            fp!(MirrorRegisteredEvaluationControl)
        }
        "protected_holdout_v41" => {
            let control = evaluation_control(
                snapshot,
                &object.namespace,
                string(payload, "registration_id")?,
            )?;
            evaluation_holdout(snapshot, &object.namespace, &logical, &control)?;
            fp!(MirrorProtectedHoldoutRecord)
        }
        // No recursive binding verification while reading a ticket node from its own
        // persisted fact graph. The outer entry separately verifies both bindings.
        "evaluation_ticket_v2" => {
            let record: MirrorEvaluationTicketV2 = decode(payload)?;
            record.validate()?;
            fp!(MirrorEvaluationTicketV2)
        }
        "evaluation_ticket_issue_receipt_v1" => fp!(MirrorTicketIssueReceipt),
        "evaluation_ticket_issue_receipt_v2" => {
            let ticket: MirrorEvaluationTicketV2 = evaluation_record(
                snapshot,
                &object.namespace,
                "evaluation_ticket_v2",
                string(payload, "ticket_id")?,
            )?;
            require(
                e05_issue_receipt(snapshot, &object.namespace, &ticket)?.0
                    == "evaluation_ticket_issue_receipt_v2",
            )?;
            fp!(MirrorTicketIssueReceiptV2)
        }
        "alpha_claim_reservation_v1" => fp!(MirrorAlphaClaimReservation),
        "execution_receipt_v2" => fp!(MirrorExecutionReceiptV2),
        "independent_grader_receipt_v2" => fp!(MirrorIndependentGraderReceiptV2),
        "evaluation_execution_output_v1" => {
            let record: MirrorExecutionOutputArtifact = decode(payload)?;
            require(
                !record.output_utf8.is_empty()
                    && record.output_utf8.len() <= 256 * 1024
                    && hash(record.output_utf8.as_bytes()) == record.output_digest,
            )?;
            fp!(MirrorExecutionOutputArtifact)
        }
        "evaluation_resource_evidence_v1" => {
            let record: MirrorResourceEvidenceRecord = decode(payload)?;
            require(
                record.cost_scope == MirrorCostEvidenceScope::TicketExecutionOnly
                    && !record.calls.is_empty(),
            )?;
            let mut ids = BTreeSet::new();
            for call in &record.calls {
                require(
                    ids.insert(&call.call_id)
                        && call.dispatch_group_id == record.ticket_id
                        && call.billing_scope == record.billing_scope,
                )?;
                require(
                    qualified_call(
                        snapshot,
                        &object.namespace,
                        &record.billing_scope,
                        &call.call_id,
                    )? == call,
                )?;
            }
            fp!(MirrorResourceEvidenceRecord)
        }
        _ => return Err(Error::ClockRegressed),
    };
    Ok((schema.into(), Some(kind.into()), logical, content))
}
fn e05_compiler_closure(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    root: (String, String),
) -> Result<()> {
    let mut pending = BTreeSet::from([root]);
    let mut nodes = BTreeMap::new();
    let mut edges = BTreeSet::new();
    let mut bytes = 0usize;
    let mut tickets = BTreeSet::new();
    let mut attached = BTreeSet::new();
    while let Some((kind, id)) = pending.pop_first() {
        if nodes.contains_key(&(kind.clone(), id.clone())) {
            continue;
        }
        require(nodes.len() < 10000)?;
        let object = snapshot.object(namespace, &kind, &id)?;
        let (schema, record_kind, _, _) = e05_dependency_identity(snapshot, object)?;
        bytes = bytes
            .checked_add(
                serde_json::to_vec(&object.body)
                    .map_err(|_| Error::ClockRegressed)?
                    .len(),
            )
            .ok_or(Error::ClockRegressed)?;
        require(bytes <= 64 * 1024 * 1024)?;
        if matches!(
            schema.as_str(),
            "rsia.typed_artifact_envelope.v1" | evo_core::formal_version::NATIVE_TICKET_ENVELOPE
        ) {
            if record_kind.as_deref() == Some("evaluation_ticket_v2") {
                tickets.insert((kind.clone(), id.clone()));
            }
            if matches!(
                record_kind.as_deref(),
                Some("execution_receipt_v2" | "independent_grader_receipt_v2")
            ) {
                attached.insert((kind.clone(), id.clone()));
            }
            if matches!(
                record_kind.as_deref(),
                Some("registered_evaluation_control_v41" | "protected_holdout_v41")
            ) {
                snapshot.witness(object, "evaluator")?;
            }
        }
        for (k, i) in e05_outgoing(snapshot, namespace, &kind, &id)? {
            edges.insert((kind.clone(), id.clone(), k.clone(), i.clone()));
            require(edges.len() <= 100000)?;
            pending.insert((k, i));
        }
        nodes.insert((kind, id), schema);
    }
    let mut degrees: BTreeMap<_, usize> = nodes.keys().cloned().map(|key| (key, 0)).collect();
    let mut children = BTreeMap::<(String, String), Vec<(String, String)>>::new();
    for (k, i, to_k, to_i) in edges {
        let parent = (k, i);
        let child = (to_k, to_i);
        if tickets.contains(&parent) && attached.contains(&child) {
            continue;
        }
        *degrees.get_mut(&child).ok_or(Error::ClockRegressed)? += 1;
        children.entry(parent).or_default().push(child);
    }
    let mut ready: Vec<_> = degrees
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(key, _)| key.clone())
        .collect();
    let mut seen = 0usize;
    while let Some(key) = ready.pop() {
        seen += 1;
        if let Some(edges) = children.remove(&key) {
            for key in edges {
                let n = degrees.get_mut(&key).ok_or(Error::ClockRegressed)?;
                *n -= 1;
                if *n == 0 {
                    ready.push(key);
                }
            }
        }
    }
    require(seen == nodes.len())
}
fn e05_verify_dependency_nodes(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    roots: Vec<(String, String)>,
    expected: &[MirrorFormalDependencyNode],
) -> Result<()> {
    validate_dependency_nodes(expected)?;
    let mut pending: BTreeSet<_> = roots.into_iter().collect();
    let mut seen = BTreeSet::new();
    let mut actual = Vec::new();
    let mut compiler_roots = Vec::new();
    while let Some((kind, id)) = pending.pop_first() {
        if !seen.insert((kind.clone(), id.clone())) {
            continue;
        }
        require(seen.len() <= 10000)?;
        let outgoing = e05_outgoing(snapshot, namespace, &kind, &id)?;
        for edge in &outgoing {
            if !seen.contains(edge) {
                pending.insert(edge.clone());
            }
        }
        require(seen.len() + pending.len() <= 10000)?;
        let object = snapshot.object(namespace, &kind, &id)?;
        let (schema_version, record_kind, logical_id, content_digest) =
            e05_dependency_identity(snapshot, object)?;
        if compiler_node_schema(&kind, &schema_version) {
            compiler_roots.push((kind.clone(), id.clone()));
        }
        actual.push(MirrorFormalDependencyNode {
            kind,
            id,
            schema_version,
            record_kind,
            logical_id,
            content_digest,
            outgoing_edge_digest: typed_fingerprint(&outgoing)?,
            outgoing_edge_count: outgoing.len(),
        });
    }
    actual.sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
    require(actual == expected)?;
    for root in compiler_roots {
        e05_compiler_closure(snapshot, namespace, root)?;
    }
    Ok(())
}
fn e05_candidate_binding(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
    control: &MirrorRegisteredEvaluationControl,
    holdout: &MirrorProtectedHoldoutRecord,
    binding: &MirrorFormalCandidateDependencyBinding,
) -> Result<()> {
    binding.validate()?;
    let object = snapshot.object(namespace, "artifact", &binding.candidate_id)?;
    time_free_record(object, snapshot)?;
    let candidate: MirrorReleaseCandidateRecord = decode(&object.body)?;
    let watermark = snapshot
        .watermarks
        .get(namespace)
        .ok_or(Error::ClockRegressed)?;
    require(
        binding.namespace == namespace
            && binding.candidate_id == ticket.candidate_digest
            && candidate.id == holdout.pair.candidate_digest
            && binding.candidate_record_digest == typed_fingerprint(&candidate)?
            && binding.bundle_digest == candidate.bundle_digest
            && binding.bundle_digest == ticket.candidate_bundle_digest
            && binding.parent_digest == candidate.parent_digest
            && binding.parent_digest == ticket.baseline_digest
            && binding.profile_id == candidate.profile_id
            && binding.environment_digest == candidate.environment_digest
            && binding.environment_digest == ticket.environment_digest
            && binding.proposer_actor == candidate.proposer_actor
            && binding.proposer_actor == ticket.proposer_actor
            && candidate.proposer_actor == control.proposer_actor
            && binding.sources == candidate.sources
            && binding.revoke_watermark_seq
                == i64::try_from(watermark.0).map_err(|_| Error::ClockRegressed)?
            && binding.revoke_watermark_digest == watermark.1
            && candidate.bundle_digest == holdout.candidate_bundle_digest
            && candidate.parent_digest == holdout.pair.baseline_digest
            && candidate.environment_digest == holdout.manifest.environment_digest,
    )?;
    let edges = e05_outgoing(snapshot, namespace, "artifact", &candidate.id)?;
    for source in &candidate.sources {
        require(edges.contains(&(source.kind.clone(), source.id.clone())))?;
    }
    e05_verify_dependency_nodes(
        snapshot,
        namespace,
        vec![("artifact".into(), candidate.id)],
        &binding.nodes,
    )
}
fn e05_stored_binding(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
    binding: &MirrorFormalStoredDependencyBinding,
) -> Result<()> {
    binding.validate()?;
    let candidate = ticket
        .candidate_dependency_binding
        .as_ref()
        .ok_or(Error::ClockRegressed)?;
    require(
        binding.namespace == namespace
            && binding.ticket_id == ticket.id
            && binding.ticket_digest == typed_fingerprint(ticket)?
            && binding.candidate_binding_digest == typed_fingerprint(candidate)?,
    )?;
    e05_verify_dependency_nodes(
        snapshot,
        namespace,
        vec![
            ("artifact".into(), binding.ticket_physical_id.clone()),
            ("artifact".into(), binding.resource_physical_id.clone()),
        ],
        &binding.nodes,
    )
}

// Pure stored authority checks from formal_authority.rs. These consume only
// the already loaded same-snapshot objects and authenticated audit witnesses;
// no process, credential, provider or filesystem operation is performed here.
impl MirrorRegisteredFormalHost {
    fn validate(&self, namespace: &str) -> Result<()> {
        if self.schema_version != "rsia.registered_formal_host.v1"
            || self.namespace != namespace
            || self.snapshot.host_id != self.host_id
            || self.snapshot.host_version != self.host_version
        {
            return Err(Error::Conflict("host registration scope mismatch".into()));
        }
        for v in [
            &self.id,
            &self.host_id,
            &self.host_version,
            &self.host_principal,
            &self.grader_principal,
            &self.administrator_principal,
            &self.worker_principal,
        ] {
            identifier(v)?;
        }
        let principals: BTreeSet<_> = [
            &self.host_principal,
            &self.grader_principal,
            &self.administrator_principal,
            &self.worker_principal,
        ]
        .into_iter()
        .collect();
        if principals.len() != 4 || self.isolated_workspace_digest == self.grader_workspace_digest {
            return Err(Error::Forbidden);
        }
        if self.host_uid == self.grader_uid
            || self.host_uid == self.administrator_uid
            || self.grader_uid == self.administrator_uid
            || self.host_workspace == self.grader_workspace
        {
            return Err(Error::Forbidden);
        }
        digest(&self.grader_code_digest)?;
        for v in [
            &self.code_digest,
            &self.credential_binding_digest,
            &self.isolated_workspace_digest,
            &self.grader_workspace_digest,
            &self.host_surface_digest,
            &self.adapter_digest,
        ] {
            digest(v)?;
        }
        Ok(())
    }
}
impl MirrorRegisteredPrincipalSet {
    pub fn binding_digest(&self) -> Result<String> {
        typed_fingerprint(self)
    }
    pub fn validate(&self, host: &MirrorRegisteredFormalHost) -> Result<()> {
        if self.schema_version != "rsia.registered_formal_principals.v1"
            || self.version == 0
            || self.namespace != host.namespace
            || self.host_registration_id != host.id
            || self.id != format!("formal-principals-{}", host.id)
            || self.host.uid != host.host_uid
            || self.grader.uid != host.grader_uid
            || self.administrator.uid != host.administrator_uid
            || self.host.principal != host.host_principal
            || self.grader.principal != host.grader_principal
            || self.administrator.principal != host.administrator_principal
            || self.worker.principal != host.worker_principal
            || self.binding_digest()? != host.credential_binding_digest
        {
            return Err(Error::Forbidden);
        }
        let all = [&self.host, &self.grader, &self.administrator, &self.worker];
        for p in all {
            identifier(&p.actor)?;
            identifier(&p.principal)?;
            digest(&p.credential_digest)?;
        }
        if all.iter().map(|p| p.uid).collect::<BTreeSet<_>>().len() != 4
            || all.iter().map(|p| &p.actor).collect::<BTreeSet<_>>().len() != 4
            || all
                .iter()
                .map(|p| &p.principal)
                .collect::<BTreeSet<_>>()
                .len()
                != 4
            || all
                .iter()
                .map(|p| &p.credential_digest)
                .collect::<BTreeSet<_>>()
                .len()
                != 4
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
    fn principal(&self, role: Role) -> Result<&MirrorRegisteredPrincipal> {
        match role {
            Role::Host => Ok(&self.host),
            Role::Evaluator => Ok(&self.grader),
            Role::Admin => Ok(&self.administrator),
            Role::Worker => Ok(&self.worker),
            _ => Err(Error::Forbidden),
        }
    }
}
fn e05_admin_object<'a>(
    snapshot: &'a OccurrenceSnapshot,
    namespace: &str,
    kind: &str,
    id: &str,
) -> Result<&'a OccurrenceObject> {
    let object = snapshot.object(namespace, kind, id)?;
    snapshot.witness(object, "admin")?;
    Ok(object)
}
fn e05_profile_host_id(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    profile: &str,
) -> Result<String> {
    let selection = e05_admin_object(
        snapshot,
        namespace,
        "artifact",
        &format!("formal-profile-{profile}"),
    )?;
    let record = e05_admin_object(
        snapshot,
        namespace,
        "artifact",
        string(&selection.body, "registration_id")?,
    )?;
    Ok(string(&record.body, "host_registration_id")?.into())
}
fn e05_subject_host(
    snapshot: &OccurrenceSnapshot,
    object: &OccurrenceObject,
) -> Result<Option<MirrorRegisteredFormalHost>> {
    let namespace = &object.namespace;
    let body = &object.body;
    let host_id: Option<String> = match string(body, "schema_version")? {
        "rsia.registered_formal_host.v1" => Some(string(body, "id")?.into()),
        "rsia.frozen_compiler_profile.v1"
        | "rsia.empty_root_authorization.v1"
        | "rsia.registered_external_provider.v1"
        | "rsia.registered_external_provider.v2"
        | "rsia.registered_resource_instrumentation.v1" => {
            Some(string(body, "host_registration_id")?.into())
        }
        "rsia.formal_profile_selection.v1" => {
            let profile = e05_admin_object(
                snapshot,
                namespace,
                "artifact",
                string(body, "registration_id")?,
            )?;
            Some(string(&profile.body, "host_registration_id")?.into())
        }
        "rsia.formal_parent_selection.v1" => {
            let selection: MirrorParentSelection = decode(body)?;
            Some(match selection.authority {
                MirrorParentAuthority::EmptyRoot { id } => {
                    let root = e05_admin_object(snapshot, namespace, "artifact", &id)?;
                    string(&root.body, "host_registration_id")?.into()
                }
                MirrorParentAuthority::ApprovedRelease { release_id } => {
                    let release = e05_admin_object(snapshot, namespace, "release", &release_id)?;
                    e05_profile_host_id(
                        snapshot,
                        namespace,
                        string(
                            release.body.get("release").unwrap_or(&release.body),
                            "profile_id",
                        )?,
                    )?
                }
            })
        }
        "rsia.persistent_release.v2" => {
            let value: evo_core::formal_version::PersistentReleaseV2<MirrorPersistentRelease> =
                decode(body)?;
            value.validate_known().map_err(|_| Error::ClockRegressed)?;
            Some(e05_profile_host_id(
                snapshot,
                namespace,
                &value.release.profile_id,
            )?)
        }
        "rsia.profile_pointer.v1" | "rsia.persistent_release.v1" => Some(e05_profile_host_id(
            snapshot,
            namespace,
            string(body, "profile_id")?,
        )?),
        _ => None,
    };
    match host_id {
        None => Ok(None),
        Some(id) => {
            let object = e05_admin_object(snapshot, namespace, "artifact", &id)?;
            let host: MirrorRegisteredFormalHost = decode(&object.body)?;
            host.validate(namespace)?;
            Ok(Some(host))
        }
    }
}
fn e05_registered_admin_authority(
    snapshot: &OccurrenceSnapshot,
    object: &OccurrenceObject,
) -> Result<()> {
    if e05_issue::qualify_native_admin_r49(snapshot, object)? {
        return Ok(());
    }
    let Some(host) = e05_subject_host(snapshot, object)? else {
        return Ok(());
    };
    let id = format!("formal-principals-{}", host.id);
    let Some(observed) =
        snapshot
            .objects
            .get(&(object.namespace.clone(), "artifact".into(), id.clone()))
    else {
        return Ok(());
    };
    // The production compatibility rule is presence based: an actual current
    // principal set is verified in full; absence is not fabricated as a set.
    snapshot.witness(observed, "admin")?;
    snapshot.witness(observed, "evaluator")?;
    let observed: MirrorPrincipalSetObservation = decode(&observed.body)?;
    let set = &observed.principals;
    set.validate(&host)?;
    require(
        observed.id == id
            && observed.schema_version == "rsia.formal_principal_set_observation.v1"
            && observed.namespace == object.namespace
            && observed.host_digest == typed_fingerprint(&host)?
            && observed.grader_uid == host.grader_uid
            && observed.grader_executable_digest == host.grader_code_digest
            && observed.grader_workspace == host.grader_workspace
            && observed.grader_pid != 0,
    )?;
    let id = format!(
        "principal-action-{}",
        typed_fingerprint(&(&object.kind, &object.id, Role::Admin))?
    );
    let action = snapshot.object(&object.namespace, "artifact", &id)?;
    snapshot.witness(action, "evaluator")?;
    let action: MirrorPrincipalAction = decode(&action.body)?;
    let p = set.principal(Role::Admin)?;
    require(
        action.id == id
            && action.schema_version == "rsia.observed_principal_action.v1"
            && action.namespace == object.namespace
            && action.role == Role::Admin
            && action.actor == p.actor
            && action.uid == p.uid
            && action.peer_uid == p.uid
            && action.principal == p.principal
            && action.credential_digest == p.credential_digest
            && action.host_registration_id == host.id
            && action.host_digest == typed_fingerprint(&host)?
            && action.principal_set_digest == set.binding_digest()?
            && action.subject_kind == object.kind
            && action.subject_id == object.id
            && action.subject_content_digest
                == object
                    .value_body_digest_r26()
                    .map_err(|_| Error::ClockRegressed)?
            && action.authority_uid == set.grader.uid
            && action.authority_executable_digest == host.grader_code_digest
            && action.authority_pid != 0,
    )?;
    digest(&action.authenticated_request_digest)?;
    require(matches!(
        action.operation.as_str(),
        "bootstrap"
            | "register_provider"
            | "register_meter"
            | "freeze_profile"
            | "empty_root"
            | "approve"
            | "canary"
            | "activate"
            | "rollback"
    ))
}
fn e05_compiler_authority(snapshot: &OccurrenceSnapshot, object: &OccurrenceObject) -> Result<()> {
    let schema = string(&object.body, "schema_version")?;
    let role = match schema {
        "rsia.frozen_compiler_profile.v1"
        | "rsia.empty_root_authorization.v1"
        | "rsia.registered_formal_host.v1"
        | "rsia.registered_external_provider.v1"
        | "rsia.registered_external_provider.v2"
        | "rsia.registered_resource_instrumentation.v1"
        | "rsia.formal_profile_selection.v1"
        | "rsia.formal_parent_selection.v1"
        | "rsia.persistent_release.v1"
        | "rsia.persistent_release.v2"
        | "rsia.profile_pointer.v1"
        | "rsia.host_surface_record.v1" => Some("admin"),
        "rsia.issued_formal_authority.v2"
        | "rsia.issued_formal_authority.v3"
        | "rsia.independent_grade_capture.v1"
        | "rsia.independent_grade_capture.v2" => Some("evaluator"),
        "rsia.actual_formal_provider_capture.v1"
        | "rsia.complete_episode_cost.v1"
        | "rsia.trusted_resource_meter.v1"
        | "rsia.run_application.v1"
        | "rsia.host_application.v1"
        | "rsia.host_execution_receipt.v1" => Some("host"),
        _ => None,
    };
    if let Some(role) = role {
        snapshot.witness(object, role)?;
    }
    if role == Some("admin") {
        e05_registered_admin_authority(snapshot, object)?;
    }
    if matches!(
        schema,
        "rsia.observed_principal_action.v1"
            | "rsia.observed_control_principal_action.v2"
            | "rsia.release_action.v2"
    ) {
        snapshot.witness(object, "evaluator")?;
    }
    if schema == "rsia.formal_principal_set_observation.v1" {
        snapshot.witness(object, "admin")?;
        snapshot.witness(object, "evaluator")?;
    }
    Ok(())
}
fn e05_builtin_parent_strategy(strategy: &Strategy) -> Result<()> {
    strategy.validate().map_err(|_| Error::ClockRegressed)?;
    let empty_root = Strategy {
        schema_version: "evo.strategy.v1".into(),
        instruction: "Preserve the unchanged host baseline; no evolution instruction is applied."
            .into(),
        max_candidates: 1,
        max_rounds: 1,
        require_counterexample: true,
    };
    require(
        typed_fingerprint(strategy)? == typed_fingerprint(&Strategy::default())?
            || typed_fingerprint(strategy)? == typed_fingerprint(&empty_root)?,
    )
}

fn e05_artifact_edge(kind: &str, id: &str) -> Result<(String, String)> {
    Ok(("artifact".into(), evaluation_storage_id(kind, id)?))
}
fn e05_require_exact_edges(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    id: &str,
    mut expected: Vec<(String, String)>,
) -> Result<()> {
    if let Some(phase) = &snapshot.e05_phase
        && let Some(result) = phase.require_current_prefix(snapshot, namespace, id)
    {
        return result;
    }
    expected.sort();
    expected.dedup();
    require(e05_outgoing(snapshot, namespace, "artifact", id)? == expected)
}
fn e05_report_edges(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
    control: &MirrorRegisteredEvaluationControl,
) -> Result<Vec<(String, String)>> {
    let mut edges = vec![
        e05_artifact_edge("evaluation_ticket_v2", &ticket.id)?,
        e05_artifact_edge("evaluation_resource_evidence_v1", &ticket.id)?,
    ];
    if control.evidence_scope == MirrorEvaluationEvidenceScope::RegisteredExternalHost {
        let object = snapshot.object(
            namespace,
            "artifact",
            &format!("formal-authority-{}", ticket.id),
        )?;
        let authority: MirrorIssuedFormalAuthority = decode(&object.body)?;
        edges.push(("artifact".into(), authority.report_principal_action_id));
    }
    Ok(edges)
}
fn e05_early_identity(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
    certificate: &EarlyStopCertificate,
    evidence_scope: MirrorEvaluationEvidenceScope,
) -> Result<()> {
    let control = evaluation_control(snapshot, namespace, &ticket.registration_id)?;
    let holdout = evaluation_holdout(snapshot, namespace, &ticket.holdout_id, &control)?;
    require(
        ticket.lifecycle == MirrorTicketLifecycle::EarlyStopped
            && certificate.ticket_id() == ticket.id
            && certificate.manifest_digest() == ticket.manifest_digest
            && certificate.v1_plan_snapshot_digest() == ticket.v1_plan_snapshot_digest
            && certificate.alpha_plan_digest() == ticket.alpha_plan_digest
            && certificate.candidate_digest() == ticket.candidate_digest
            && certificate.baseline_digest() == ticket.baseline_digest
            && certificate.grader_digest() == control.grader_digest
            && certificate.slice_id() == holdout.manifest.slice_id
            && ticket.stop_member_terminal_digest.as_deref()
                == Some(certificate.member_terminal_digest()),
    )?;
    let mut issuer_found = false;
    for target in &ticket.targets {
        if matches!(target.state, MirrorTargetState::Completed { .. }) {
            let grader = e05_completed_target_evidence(
                snapshot, namespace, ticket, &control, &holdout, target,
            )?;
            if typed_fingerprint(&grader)? == certificate.issuer_receipt_digest() {
                issuer_found = grader.ticket_id == ticket.id
                    && grader.evaluator_actor == ticket.evaluator_actor
                    && grader.grader_digest == control.grader_digest;
            }
        }
    }
    require(issuer_found)?;
    if ticket.candidate_dependency_binding.is_some() {
        require(evidence_scope == ticket.evidence_scope)?;
        e05_bound_early_certificate(snapshot, namespace, ticket, &control, &holdout, certificate)?;
    }
    Ok(())
}
/// Resolve exactly one actually persisted issue protocol. This function never
/// creates a future receipt, chooses the first of two sources, or infers v2 from
/// the ticket's unchanged wire shape. The physical kind and payload version pair
/// are both fixed, and all issue identity bytes are bound by the ticket digest.
fn e05_issue_receipt_binding(
    kind: &str,
    binding: Option<&MirrorFormalCandidateDependencyBinding>,
) -> Result<()> {
    require(kind != "evaluation_ticket_issue_receipt_v2" || binding.is_some())
}

fn e05_issue_receipt_v2_time(
    seconds: i64,
    millis: i64,
    control_created: i64,
    manifest_issued: i64,
) -> Result<()> {
    require(
        seconds >= 0
            && millis >= 0
            && control_created >= 0
            && manifest_issued >= 0
            && seconds == millis / 1000
            && seconds >= control_created
            && millis >= manifest_issued,
    )
}

fn e05_issue_receipt(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
) -> Result<(&'static str, E05IssueReceiptIdentity)> {
    let mut selected = None;
    for (kind, schema) in [
        (
            "evaluation_ticket_issue_receipt_v1",
            "rsia.evaluation_ticket_issue_receipt.v1",
        ),
        (
            "evaluation_ticket_issue_receipt_v2",
            "rsia.evaluation_ticket_issue_receipt.v2",
        ),
    ] {
        let physical = evaluation_storage_id(kind, &ticket.id)?;
        let Some(object) = snapshot
            .objects
            .get(&(namespace.into(), "artifact".into(), physical))
        else {
            continue;
        };
        require(selected.is_none())?;
        e05_issue_receipt_binding(kind, ticket.candidate_dependency_binding.as_ref())?;
        let payload = envelope(object, "rsia.typed_artifact_envelope.v1", kind)?;
        let receipt = if kind == "evaluation_ticket_issue_receipt_v1" {
            let raw: MirrorTicketIssueReceipt = decode(payload)?;
            require(raw.schema_version == schema)?;
            let actual_digest = typed_fingerprint(&raw)?;
            E05IssueReceiptIdentity {
                ticket_id: raw.ticket_id,
                registration_digest: raw.registration_digest,
                manifest_digest: raw.manifest_digest,
                evaluator_actor: raw.evaluator_actor,
                issued_at_unix_seconds: raw.issued_at_unix_seconds,
                issued_at_unix_ms: None,
                actual_digest,
            }
        } else {
            let raw: MirrorTicketIssueReceiptV2 = decode(payload)?;
            require(raw.schema_version == schema)?;
            let control = evaluation_control(snapshot, namespace, &ticket.registration_id)?;
            let holdout = evaluation_holdout(snapshot, namespace, &ticket.holdout_id, &control)?;
            e05_issue_receipt_v2_time(
                raw.issued_at_unix_seconds,
                raw.issued_at_unix_ms,
                control.created_at_unix_seconds,
                holdout.manifest.issued_at_unix_ms,
            )?;
            let actual_digest = typed_fingerprint(&raw)?;
            E05IssueReceiptIdentity {
                ticket_id: raw.ticket_id,
                registration_digest: raw.registration_digest,
                manifest_digest: raw.manifest_digest,
                evaluator_actor: raw.evaluator_actor,
                issued_at_unix_seconds: raw.issued_at_unix_seconds,
                issued_at_unix_ms: Some(raw.issued_at_unix_ms),
                actual_digest,
            }
        };
        require(
            object.namespace == namespace
                && object.owner == ticket.evaluator_actor
                && receipt.ticket_id == ticket.id
                && receipt.registration_digest == ticket.registration_digest
                && receipt.manifest_digest == ticket.manifest_digest
                && receipt.evaluator_actor == ticket.evaluator_actor
                && receipt.issued_at_unix_seconds == ticket.issued_at_unix_seconds
                && receipt.actual_digest == ticket.issue_receipt_digest,
        )?;
        selected = Some((kind, receipt));
    }
    selected.ok_or(Error::ClockRegressed)
}

fn e05_required_ticket_edges(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
    control: &MirrorRegisteredEvaluationControl,
) -> Result<Vec<(String, String)>> {
    let mut edges = vec![
        ("artifact".into(), ticket.candidate_digest.clone()),
        e05_artifact_edge("protected_holdout_v41", &ticket.holdout_id)?,
        e05_artifact_edge(
            e05_issue_receipt(snapshot, namespace, ticket)?.0,
            &ticket.id,
        )?,
    ];
    for allocation in control
        .alpha_plan
        .allocations
        .iter()
        .filter(|item| item.attempt_id == ticket.attempt_id)
    {
        let id = format!(
            "{}:{}",
            control.alpha_plan.research_family_id, allocation.claim_id
        );
        let actual: MirrorAlphaClaimReservation =
            evaluation_record(snapshot, namespace, "alpha_claim_reservation_v1", &id)?;
        if actual.schema_version != "rsia.alpha_claim_reservation.v1"
            || actual.research_family_id != ticket.research_family_id
            || actual.claim_id != allocation.claim_id
            || actual.attempt_id != ticket.attempt_id
            || actual.ticket_id != ticket.id
        {
            return Err(Error::Conflict(
                "formal alpha reservation identity differs".into(),
            ));
        }
        edges.push(e05_artifact_edge("alpha_claim_reservation_v1", &id)?);
    }
    for target in &ticket.targets {
        match &target.state {
            MirrorTargetState::Completed {
                candidate_receipt_id,
                baseline_receipt_id,
                grader_receipt_id,
                ..
            } => {
                for id in [candidate_receipt_id, baseline_receipt_id] {
                    edges.push(e05_artifact_edge("execution_receipt_v2", id)?);
                }
                edges.push(e05_artifact_edge(
                    "independent_grader_receipt_v2",
                    grader_receipt_id,
                )?);
            }
            MirrorTargetState::Running {
                candidate,
                baseline,
            }
            | MirrorTargetState::RunningUsageUnknown {
                candidate,
                baseline,
            }
            | MirrorTargetState::CancelledAfterDispatch {
                candidate,
                baseline,
            } => {
                for slot in [candidate, baseline].into_iter().flatten() {
                    if let Some(id) = &slot.receipt_id {
                        edges.push(e05_artifact_edge("execution_receipt_v2", id)?);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(edges)
}

fn e05_bound_fact_edges(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
    control: &MirrorRegisteredEvaluationControl,
) -> Result<()> {
    if ticket.candidate_dependency_binding.is_none() {
        return Ok(());
    }
    let ticket_edges = e05_required_ticket_edges(snapshot, namespace, ticket, control)?;
    e05_require_exact_edges(
        snapshot,
        namespace,
        &evaluation_storage_id("evaluation_ticket_v2", &ticket.id)?,
        ticket_edges,
    )?;
    e05_require_exact_edges(
        snapshot,
        namespace,
        &evaluation_storage_id("protected_holdout_v41", &ticket.holdout_id)?,
        vec![e05_artifact_edge(
            "registered_evaluation_control_v41",
            &ticket.registration_id,
        )?],
    )?;
    for target in &ticket.targets {
        let mut receipt_ids = Vec::new();
        match &target.state {
            MirrorTargetState::Completed {
                candidate_receipt_id,
                baseline_receipt_id,
                grader_receipt_id,
                ..
            } => {
                receipt_ids.extend([candidate_receipt_id, baseline_receipt_id]);
                e05_require_exact_edges(
                    snapshot,
                    namespace,
                    &evaluation_storage_id("independent_grader_receipt_v2", grader_receipt_id)?,
                    vec![
                        e05_artifact_edge("execution_receipt_v2", candidate_receipt_id)?,
                        e05_artifact_edge("execution_receipt_v2", baseline_receipt_id)?,
                    ],
                )?;
            }
            MirrorTargetState::Running {
                candidate,
                baseline,
            }
            | MirrorTargetState::RunningUsageUnknown {
                candidate,
                baseline,
            }
            | MirrorTargetState::CancelledAfterDispatch {
                candidate,
                baseline,
            } => {
                receipt_ids.extend(
                    [candidate, baseline]
                        .into_iter()
                        .flatten()
                        .filter_map(|slot| slot.receipt_id.as_ref()),
                );
            }
            _ => {}
        }
        for id in receipt_ids {
            let receipt: MirrorExecutionReceiptV2 =
                evaluation_record(snapshot, namespace, "execution_receipt_v2", id)?;
            e05_require_exact_edges(
                snapshot,
                namespace,
                &evaluation_storage_id("execution_receipt_v2", id)?,
                vec![
                    e05_artifact_edge("evaluation_ticket_v2", &ticket.id)?,
                    e05_artifact_edge(
                        "evaluation_execution_output_v1",
                        &receipt.output_artifact_id,
                    )?,
                ],
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod single_capture_ticket_scope_r60 {
    use super::*;

    // Intentionally unqualified DTO: no DB rows and no targets. Manual seeding
    // tests only the private memo's scope; it is not native/Current evidence.
    fn seeded_scope_scan(snapshot: &OccurrenceSnapshot) -> TicketRelationScan<'_> {
        let d = "0".repeat(64);
        let ticket = MirrorEvaluationTicketV2 {
            schema_version: MirrorEvaluationTicketV2::SCHEMA.into(),
            id: "ticket".into(),
            registration_id: "control".into(),
            registration_digest: d.clone(),
            holdout_id: "holdout".into(),
            manifest_digest: d.clone(),
            v1_plan_snapshot_digest: d.clone(),
            bound_v1_plan_digest: d.clone(),
            formal_plan_digest: d.clone(),
            profile: evo_core::evaluation::ProfileKind::QualityGain,
            n_planned: 0,
            research_family_id: "family".into(),
            alpha_plan_digest: d.clone(),
            candidate_digest: d.clone(),
            baseline_digest: d.clone(),
            candidate_bundle_digest: d.clone(),
            baseline_bundle_digest: d.clone(),
            environment_digest: d.clone(),
            proposer_actor: "proposer".into(),
            evaluator_actor: "evaluator".into(),
            executor_actor: "executor".into(),
            approver_actor: "approver".into(),
            issue_receipt_digest: d,
            evidence_scope: MirrorEvaluationEvidenceScope::ProgramFixture,
            attempt_id: "attempt".into(),
            targets: Vec::new(),
            lifecycle: MirrorTicketLifecycle::Reserved,
            next_sequence: 0,
            pending_certificate: None,
            stop_member_terminal_digest: None,
            late_execution_receipt_ids: Vec::new(),
            late_grader_receipt_ids: Vec::new(),
            unattributed_budget_call_ids: Vec::new(),
            invalid_reasons: Vec::new(),
            issued_at_unix_seconds: 0,
            candidate_dependency_binding: None,
        };
        assert!(ticket.validate().is_err());
        TicketRelationScan {
            snapshot,
            checked_relations: BTreeSet::new(),
            successful_tickets: BTreeMap::from([(("scope-a".into(), "ticket".into()), ticket)]),
        }
    }

    #[test]
    fn seeded_success_cannot_cross_snapshot() {
        let first = OccurrenceSnapshot::default();
        let second = OccurrenceSnapshot::default();
        let mut scan = seeded_scope_scan(&first);
        assert!(matches!(
            evaluation_ticket_for_qualification(&second, "scope-a", "ticket", Some(&mut scan)),
            Err(Error::ClockRegressed)
        ));
        assert_eq!(scan.successful_tickets.len(), 1);
    }

    #[test]
    fn seeded_success_cannot_cross_namespace() {
        let snapshot = OccurrenceSnapshot::default();
        let mut scan = seeded_scope_scan(&snapshot);
        assert!(matches!(
            evaluation_ticket_for_qualification(&snapshot, "scope-b", "ticket", Some(&mut scan)),
            Err(Error::ClockRegressed)
        ));
        assert_eq!(scan.successful_tickets.len(), 1);
        assert!(
            !scan
                .successful_tickets
                .contains_key(&("scope-b".into(), "ticket".into()))
        );
    }
}

#[cfg(test)]
mod budget_call_ref_floor_tests {
    use super::*;

    fn fixture() -> (OccurrenceObject, OccurrenceSnapshot) {
        let reference = crate::budget::BudgetCallRef {
            id: format!(
                "budget-ref-{}",
                &typed_fingerprint(&("rsia.budget_call_ref.v1", "ref-ns", "scope", "call"))
                    .unwrap()[..32]
            ),
            schema_version: "rsia.budget_call_ref.v1".into(),
            namespace: "ref-ns".into(),
            billing_scope: "scope".into(),
            call_id: "call".into(),
            source_ids: vec!["source-a".into(), "source-b".into()],
        };
        let call: crate::budget::BudgetCallRecord = serde_json::from_value(serde_json::json!({
            "billing_scope":"scope", "call_id":"call", "dispatch_group_id":"group",
            "namespace":"ref-ns", "stage":"task_execution", "actual_input_digest":"a".repeat(64),
            "request_artifact":null, "reserved_micros":1, "state":"reserved",
            "lease_token":"lease", "lease_epoch":1, "lease_until":100,
            "dispatch_id":null, "provider_request_id":null, "usage_record_id":null,
            "output_digest":null, "actual_cost_micros":null, "actual_currency":null,
            "actual_pricing_version":null, "execution_provenance":null, "actual_model_digest":null,
            "transport_artifact":null, "response_artifact":null, "response_usable":null,
            "response_block_reason":null, "execution_closed":true, "execution_closed_at":null,
            "execution_close_reason":null, "terminal_reason":null,
            "created_at":7, "dispatched_at":null, "finalized_at":null
        }))
        .unwrap();
        let object = OccurrenceObject {
            namespace: reference.namespace.clone(),
            kind: "artifact".into(),
            id: reference.id.clone(),
            owner: "host".into(),
            body: serde_json::to_value(&reference).unwrap(),
            body_value_digest_r26: None,
        };
        let mut snapshot = OccurrenceSnapshot::default();
        snapshot.calls.insert(("scope".into(), "call".into()), call);
        for source in &reference.source_ids {
            snapshot.edges.insert((
                "ref-ns".into(),
                "artifact".into(),
                reference.id.clone(),
                "run".into(),
                source.clone(),
            ));
        }
        (object, snapshot)
    }

    #[test]
    fn budget_call_ref_is_time_free_accounting_without_live_source_authority() {
        let (object, snapshot) = fixture();
        let mut floor = OccurrenceFloor::default();
        qualify_object(&object, &snapshot, &PreparePreimages::default(), &mut floor).unwrap();
        assert_eq!(floor.max_nanos(), None);
        // Accounting may outlive revoked/redacted source content. Its original
        // identifiers and dependency edges remain required, not live source bodies.
        assert!(snapshot.objects.is_empty());
    }

    #[test]
    fn budget_call_ref_rejects_corrupt_identity_call_and_source_closure() {
        for case in 0..11 {
            let (mut object, mut snapshot) = fixture();
            match case {
                0 => object.body["id"] = serde_json::json!("wrong-id"),
                1 => object.body["namespace"] = serde_json::json!("foreign-ns"),
                2 => snapshot.calls.clear(),
                3 => {
                    snapshot
                        .calls
                        .get_mut(&("scope".into(), "call".into()))
                        .unwrap()
                        .namespace = "foreign-ns".into()
                }
                4 => object.body["source_ids"] = serde_json::json!(["source-a", "source-a"]),
                5 => object.body["source_ids"] = serde_json::json!(["source-b", "source-a"]),
                6 => snapshot.edges.clear(),
                7 => {
                    snapshot.edges.insert((
                        "ref-ns".into(),
                        "artifact".into(),
                        object.id.clone(),
                        "artifact".into(),
                        "source-a".into(),
                    ));
                }
                8 => object.body["created_at"] = serde_json::json!(i64::MAX),
                9 => object.kind = "run".into(),
                10 => object.body["schema_version"] = serde_json::json!("rsia.unknown_call_ref.v1"),
                _ => unreachable!(),
            }
            assert!(
                matches!(
                    qualify_object(
                        &object,
                        &snapshot,
                        &PreparePreimages::default(),
                        &mut OccurrenceFloor::default()
                    ),
                    Err(Error::ClockRegressed)
                ),
                "case {case}"
            );
        }
    }

    fn e16_fixture() -> (OccurrenceObject, OccurrenceSnapshot) {
        use crate::typed_budget::{
            E16_BUDGET_REF_SCHEMA, E16_BUDGET_REQUEST_SCHEMA, E16BudgetCallRef, E16BudgetRequest,
            E16BudgetSourceRef,
        };
        let (_, mut snapshot) = fixture();
        snapshot.edges.clear();
        let input = crate::budget::BudgetArtifact::from_serializable(
            "rsia.fixture.model_input.v1",
            &serde_json::json!({"input":"accounting-only"}),
        )
        .unwrap();
        let refs: Vec<_> = ["source-a", "source-b"]
            .into_iter()
            .map(|id| E16BudgetSourceRef {
                namespace: "ref-ns".into(),
                kind: "artifact".into(),
                id: id.into(),
                schema_version: "rsia.e16.import_source.v1".into(),
                owner_actor: "host".into(),
                storage_body_digest: "c".repeat(64),
            })
            .collect();
        let request = E16BudgetRequest {
            schema_version: E16_BUDGET_REQUEST_SCHEMA.into(),
            namespace: "ref-ns".into(),
            billing_scope: "scope".into(),
            call_id: "call".into(),
            object_refs: refs.clone(),
            input_artifact: input.clone(),
        }
        .artifact()
        .unwrap();
        let reference = E16BudgetCallRef {
            id: crate::typed_budget::e16_budget_ref_id("ref-ns", "scope", "call").unwrap(),
            schema_version: E16_BUDGET_REF_SCHEMA.into(),
            namespace: "ref-ns".into(),
            billing_scope: "scope".into(),
            call_id: "call".into(),
            request_schema: E16_BUDGET_REQUEST_SCHEMA.into(),
            request_digest: request.digest.clone(),
            actual_input_digest: input.digest.clone(),
            object_refs: refs,
        };
        let call = snapshot
            .calls
            .get_mut(&("scope".into(), "call".into()))
            .unwrap();
        call.actual_input_digest = input.digest;
        call.request_artifact = Some(request);
        for source in &reference.object_refs {
            snapshot.edges.insert((
                "ref-ns".into(),
                "artifact".into(),
                reference.id.clone(),
                source.kind.clone(),
                source.id.clone(),
            ));
        }
        (
            OccurrenceObject {
                namespace: "ref-ns".into(),
                kind: "artifact".into(),
                id: reference.id.clone(),
                owner: "host".into(),
                body: serde_json::to_value(reference).unwrap(),
                body_value_digest_r26: None,
            },
            snapshot,
        )
    }

    fn redact_e16_fixture_request(snapshot: &mut OccurrenceSnapshot, original_digest: &str) {
        snapshot.calls.get_mut(&("scope".into(), "call".into())).unwrap().request_artifact = Some(
            crate::budget::BudgetArtifact::from_serializable("rsia.redacted.v1", &serde_json::json!({
                "schema_version":"rsia.redacted.v1", "state":"source_revoked",
                "reason":"e16_source_unavailable", "original_schema":"rsia.e16.model_budget_request.v1",
                "original_digest":original_digest,
            })).unwrap()
        );
    }

    #[test]
    fn e16_budget_ref_keeps_bound_accounting_time_free_before_and_after_redaction() {
        let (object, mut snapshot) = e16_fixture();
        for redacted in [false, true] {
            if redacted {
                redact_e16_fixture_request(
                    &mut snapshot,
                    object.body["request_digest"].as_str().unwrap(),
                );
            }
            let mut floor = OccurrenceFloor::default();
            qualify_object(&object, &snapshot, &PreparePreimages::default(), &mut floor).unwrap();
            assert_eq!(floor.max_nanos(), None);
            assert!(
                snapshot.objects.is_empty(),
                "live source bodies are not accounting authority"
            );
        }
    }

    #[test]
    fn e16_budget_ref_rejects_binding_corruption_and_both_reference_domains() {
        for case in 0..15 {
            let (mut object, mut snapshot) = e16_fixture();
            match case {
                0 => object.body["request_digest"] = serde_json::json!("b".repeat(64)),
                1 => {
                    object.body["object_refs"][0]["storage_body_digest"] =
                        serde_json::json!("z".repeat(64))
                }
                2 => object.body["created_at"] = serde_json::json!(i64::MAX),
                3 => {
                    snapshot
                        .calls
                        .get_mut(&("scope".into(), "call".into()))
                        .unwrap()
                        .namespace = "foreign-ns".into()
                }
                4 => snapshot.edges.clear(),
                5 => {
                    snapshot.edges.insert((
                        "ref-ns".into(),
                        "artifact".into(),
                        object.id.clone(),
                        "run".into(),
                        "source-a".into(),
                    ));
                }
                6 => {
                    let (legacy, _) = fixture();
                    snapshot.objects.insert(
                        (
                            legacy.namespace.clone(),
                            legacy.kind.clone(),
                            legacy.id.clone(),
                        ),
                        legacy,
                    );
                }
                7 => {
                    let call = snapshot
                        .calls
                        .get_mut(&("scope".into(), "call".into()))
                        .unwrap();
                    let mut request: Value =
                        serde_json::from_str(&call.request_artifact.as_ref().unwrap().body)
                            .unwrap();
                    request["input_artifact"]["digest"] = serde_json::json!("b".repeat(64));
                    call.request_artifact = Some(
                        crate::budget::BudgetArtifact::from_serializable(
                            "rsia.e16.model_budget_request.v1",
                            &request,
                        )
                        .unwrap(),
                    );
                }
                8 => {
                    snapshot
                        .calls
                        .get_mut(&("scope".into(), "call".into()))
                        .unwrap()
                        .request_artifact = None
                }
                9 => {
                    snapshot
                        .calls
                        .get_mut(&("scope".into(), "call".into()))
                        .unwrap()
                        .request_artifact
                        .as_mut()
                        .unwrap()
                        .digest = "b".repeat(64)
                }
                10 => object.body["object_refs"].as_array_mut().unwrap().reverse(),
                11 => {
                    let duplicate = object.body["object_refs"][0].clone();
                    object.body["object_refs"]
                        .as_array_mut()
                        .unwrap()
                        .push(duplicate);
                }
                12 => object.body["id"] = serde_json::json!("wrong-id"),
                13 => object.body["namespace"] = serde_json::json!("foreign-ns"),
                14 => redact_e16_fixture_request(&mut snapshot, &"b".repeat(64)),
                _ => unreachable!(),
            }
            assert!(
                matches!(
                    qualify_object(
                        &object,
                        &snapshot,
                        &PreparePreimages::default(),
                        &mut OccurrenceFloor::default()
                    ),
                    Err(Error::ClockRegressed)
                ),
                "case {case}"
            );
        }
        for e16_request in [false, true] {
            let (legacy, mut snapshot) = fixture();
            let (e16, e16_snapshot) = e16_fixture();
            if e16_request {
                snapshot
                    .calls
                    .get_mut(&("scope".into(), "call".into()))
                    .unwrap()
                    .request_artifact = e16_snapshot.calls[&("scope".into(), "call".into())]
                    .request_artifact
                    .clone();
            } else {
                snapshot.objects.insert(
                    (e16.namespace.clone(), e16.kind.clone(), e16.id.clone()),
                    e16,
                );
            }
            assert!(matches!(
                qualify_object(
                    &legacy,
                    &snapshot,
                    &PreparePreimages::default(),
                    &mut OccurrenceFloor::default()
                ),
                Err(Error::ClockRegressed)
            ));
        }
    }
}

#[cfg(test)]
mod native_ticket_binding_node_tests {
    use super::*;

    fn ticket_node(schema: &str) -> MirrorFormalDependencyNode {
        MirrorFormalDependencyNode {
            kind: "artifact".into(),
            id: evaluation_storage_id("evaluation_ticket_v2", "ticket").unwrap(),
            schema_version: schema.into(),
            record_kind: Some("evaluation_ticket_v2".into()),
            logical_id: "ticket".into(),
            content_digest: "a".repeat(64),
            outgoing_edge_digest: typed_fingerprint(&Vec::<(String, String)>::new()).unwrap(),
            outgoing_edge_count: 0,
        }
    }

    #[test]
    fn binding_shape_preserves_native_and_legacy_ticket_domains() {
        for schema in [
            evo_core::formal_version::NATIVE_TICKET_ENVELOPE,
            "rsia.typed_artifact_envelope.v1",
        ] {
            validate_dependency_nodes(&[ticket_node(schema)]).unwrap();
        }
    }

    #[test]
    fn native_ticket_node_does_not_admit_other_kinds_id_or_schema() {
        for case in 0..7 {
            let mut node = ticket_node(evo_core::formal_version::NATIVE_TICKET_ENVELOPE);
            match case {
                0 => node.record_kind = Some("execution_receipt_v2".into()),
                1 => node.record_kind = None,
                2 => node.id = "wrong-physical-id".into(),
                3 => node.kind = "release".into(),
                4 => node.schema_version = "rsia.unknown_ticket_envelope.v1".into(),
                5 => node.content_digest = "z".repeat(64),
                6 => node.outgoing_edge_count = 10001,
                _ => unreachable!(),
            }
            assert!(
                matches!(
                    validate_dependency_nodes(&[node]),
                    Err(Error::ClockRegressed)
                ),
                "case {case}"
            );
        }
        let duplicate = ticket_node(evo_core::formal_version::NATIVE_TICKET_ENVELOPE);
        assert!(matches!(
            validate_dependency_nodes(&[duplicate.clone(), duplicate]),
            Err(Error::ClockRegressed)
        ));
    }
}

pub(super) struct RestoreNativeRowsR50 {
    pub(super) raw: RawDatabase,
    pub(super) facts: Value,
}

fn restore_cell_r50(cell: &OwnedCell) -> WorkerCellR50 {
    match cell {
        OwnedCell::Null => WorkerCellR50::Null,
        OwnedCell::Integer(value) => WorkerCellR50::Integer(*value),
        OwnedCell::Real(value) => WorkerCellR50::RealBits(value.to_bits()),
        OwnedCell::Text(value) => WorkerCellR50::Text(value.clone()),
        OwnedCell::Blob(value) => WorkerCellR50::Blob(value.clone()),
    }
}

fn restore_raw_value_r50(database: &RawDatabase) -> Result<Value> {
    let wire = WorkerRowsR50 {
        tables: database
            .tables
            .iter()
            .map(|(name, rows)| {
                (
                    name.clone(),
                    rows.iter()
                        .map(|row| {
                            row.cells
                                .iter()
                                .map(|(name, cell)| (name.clone(), restore_cell_r50(cell)))
                                .collect()
                        })
                        .collect(),
                )
            })
            .collect(),
    };
    serde_json::to_value(wire).map_err(|_| Error::Internal)
}

fn restore_projection_r50(
    database: &RawDatabase,
    root_columns: &BTreeMap<String, Vec<String>>,
    native_version: String,
    native_source_id: String,
    integrity: Vec<String>,
) -> Result<Value> {
    require(!native_version.is_empty() && !native_source_id.is_empty())?;
    require(integrity == ["ok"])?;
    let mut migrations = Vec::new();
    for row in rows(
        database,
        "_sqlx_migrations",
        &[
            "version",
            "description",
            "installed_on",
            "success",
            "checksum",
            "execution_time",
        ],
    )? {
        require(sql_type(&row, "checksum")? == "blob")?;
        let checksum: Vec<u8> = clock(row.try_get("checksum"))?;
        migrations.push(serde_json::json!({
            "version": sql_integer(&row, "version")?,
            "description": sql_string(&row, "description")?,
            "installed_on": sql_string(&row, "installed_on")?,
            "success": sql_integer(&row, "success")?,
            "checksum_hex": checksum.iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
            "execution_time": sql_integer(&row, "execution_time")?,
        }));
    }
    migrations.sort_by_key(|row| row["version"].as_i64());
    let mut watermarks = Vec::new();
    for row in rows(
        database,
        "revoke_watermark",
        &["namespace", "seq", "digest"],
    )? {
        watermarks.push((
            sql_string(&row, "namespace")?,
            sql_integer(&row, "seq")?,
            sql_string(&row, "digest")?,
        ));
    }
    watermarks.sort();
    let mut objects = Vec::new();
    for row in rows(database, "objects", &["namespace", "kind", "id", "body"])? {
        objects.push((
            sql_string(&row, "namespace")?,
            sql_string(&row, "kind")?,
            sql_string(&row, "id")?,
            // Preserve actual SQL UTF8; tombstone hashes do not reserialize JSON.
            sql_string(&row, "body")?,
        ));
    }
    objects.sort();
    let mut roots = Vec::new();
    for (name, columns) in root_columns {
        require(!columns.is_empty())?;
        let table = database.tables.get(name).ok_or(Error::ClockRegressed)?;
        let mut output = Vec::new();
        for row in table {
            output.push(
                columns
                    .iter()
                    .map(|column| {
                        row.cells
                            .get(column)
                            .map(restore_cell_r50)
                            .ok_or(Error::ClockRegressed)
                    })
                    .collect::<Result<Vec<_>>>()?,
            );
        }
        roots.push((name.clone(), output));
    }
    Ok(serde_json::json!({
        "raw": restore_raw_value_r50(database)?,
        "native_version": native_version,
        "native_source_id": native_source_id,
        "integrity": integrity,
        "migrations": migrations,
        "watermarks": watermarks,
        "objects": objects,
        "root_budget_tables": roots,
    }))
}

pub(super) fn restore_worker_value_r50(connection: &rusqlite::Connection) -> Result<Value> {
    require(!connection.is_autocommit())?;
    let database = load_rusqlite_database(connection)?;
    let (version, source_id): (String, String) = connection
        .query_row("SELECT sqlite_version(),sqlite_source_id()", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|_| Error::ClockRegressed)?;
    let integrity = connection
        .prepare("PRAGMA integrity_check")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|_| Error::ClockRegressed)?;
    let names = connection
        .prepare(crate::lifecycle::ROOT_BUDGET_TABLE_QUERY)
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|_| Error::ClockRegressed)?;
    let mut columns = BTreeMap::new();
    for name in names {
        let values = connection
            .prepare("SELECT name FROM pragma_table_info(?) ORDER BY cid")
            .and_then(|mut statement| {
                statement
                    .query_map([&name], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .map_err(|_| Error::ClockRegressed)?;
        require(columns.insert(name, values).is_none())?;
    }
    require(!connection.is_autocommit())?;
    restore_projection_r50(&database, &columns, version, source_id, integrity)
}

pub(super) async fn restore_sqlx_value_r50(connection: &mut SqliteConnection) -> Result<Value> {
    require(connection.is_in_transaction())?;
    let database = load_sqlx_database(connection).await?;
    let (version, source_id): (String, String) = clock(
        sqlx::query_as("SELECT sqlite_version(),sqlite_source_id()")
            .fetch_one(&mut *connection)
            .await,
    )?;
    let integrity: Vec<String> = clock(
        sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_all(&mut *connection)
            .await,
    )?;
    let names: Vec<String> = clock(
        sqlx::query_scalar(crate::lifecycle::ROOT_BUDGET_TABLE_QUERY)
            .fetch_all(&mut *connection)
            .await,
    )?;
    let mut columns = BTreeMap::new();
    for name in names {
        let values: Vec<String> = clock(
            sqlx::query_scalar("SELECT name FROM pragma_table_info(?) ORDER BY cid")
                .bind(&name)
                .fetch_all(&mut *connection)
                .await,
        )?;
        require(columns.insert(name, values).is_none())?;
    }
    require(connection.is_in_transaction())?;
    restore_projection_r50(&database, &columns, version, source_id, integrity)
}

pub(super) fn restore_value_from_worker_r50(mut value: Value) -> Result<RestoreNativeRowsR50> {
    exact(
        &value,
        &[
            "raw",
            "native_version",
            "native_source_id",
            "integrity",
            "migrations",
            "watermarks",
            "objects",
            "root_budget_tables",
        ],
        &[],
    )?;
    require(
        !string(&value, "native_version")?.is_empty()
            && !string(&value, "native_source_id")?.is_empty()
            && value["integrity"] == serde_json::json!(["ok"]),
    )?;
    let raw = raw_from_worker_r50(
        value
            .as_object_mut()
            .and_then(|object| object.remove("raw"))
            .ok_or(Error::ClockRegressed)?,
    )?;
    Ok(RestoreNativeRowsR50 { raw, facts: value })
}

pub(super) fn restore_union_floor_r50(
    first: &RawDatabase,
    second: &RawDatabase,
    inputs: &PreparePreimages,
    extra: &[i64],
) -> Result<OccurrenceFloor> {
    let mut floor = collect_floor_from_raw_r50(first, inputs)?;
    let other = collect_floor_from_raw_r50(second, inputs)?;
    if let Some(value) = other.max_nanos {
        floor.max_nanos = Some(floor.max_nanos.map_or(value, |old| old.max(value)));
    }
    for value in extra {
        floor.observe_seconds(*value)?;
    }
    Ok(floor)
}
