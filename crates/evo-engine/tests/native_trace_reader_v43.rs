//! AG-148 (v4.5 §11.2.1, V117.b, §5.1.1, §3.3.2): the strict `rsia.trace.v1`
//! reader and the versioned import result. The reader golden is a software
//! fixture (`observed_exporter: false`), not output of a real exporter; the
//! legacy rows are a portable derivative of one real parent capture
//! (`synthetic_derivative_not_observed_producer`), not the observed bytes.
use evo_core::evidence::{
    ExecutionAttestation, IMPORT_NORMALIZATION_V2, IMPORT_READER_SET_V2, IMPORT_RESULT_SCHEMA_V2,
    IMPORT_RESULT_VERSIONS, ImportArtifactRef, ImportArtifactSchema, ImportEventKey,
    ImportMaterialReadRequest, LEGACY_IMPORT_RESULT_SCHEMA_V1, Purpose, SourceSelection,
    TaskOrigin, import_result_id_v2, legacy_import_result_id_v1,
};
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::dependency_integrity::{
    E16Wire, InspectionBudget, InspectionStatus, IssueCode, inspect_e16_content_edges,
};
use evo_engine::evidence::{
    load_stored_source, read_persisted_imported_development_material,
    read_persisted_imported_evidence,
};
use evo_engine::import::{
    IMPORT_REGISTRATION_SCHEMA, ImportRegistrationRequest, ImportResultReadState,
    ImportResultRecord, ImportResultState, ImportRetentionScope, ImportSourceRecord,
    ImportSourceSpec, LegacyImportResultRecordV1, PersistentImportService, PinnedSourceBytes,
    SourceFormat, SourceSelectionRecord, detect_format, detect_format_from_bytes,
    ingest_imported_sources, ingest_pinned_imported_sources, parse_fixture,
};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallReservation, BudgetStage, RootBudgetAuthorization,
};
use evo_storage::lifecycle::{LifecycleStore, TypedObjectRef};
use evo_storage::typed_budget::{E16_BUDGET_REQUEST_SCHEMA, E16BudgetRequest, E16BudgetSourceRef};
use serde_json::{Value, json};

const GOLDEN: &str = include_str!("../../../fixtures/imports/rsia.trace.v1.reader-golden.json");
const PORTABLE: &str =
    include_str!("../../../fixtures/imports/rsia.e16.import_result.v1.portable-derived.json");
const ORIGINAL_NATIVE: &[u8] = include_bytes!("../../../fixtures/imports/rsia.trace.v1.jsonl");
const ORIGINAL_PI: &[u8] = include_bytes!("../../../fixtures/imports/rsih.pi.fixture.json");
const ORIGINAL_CLAUDE: &[u8] = include_bytes!("../../../fixtures/imports/claude.fixture.jsonl");
const LEGACY_NAMESPACE: &str = "ag148-legacy";

fn golden() -> Value {
    serde_json::from_str(GOLDEN).unwrap()
}

/// The golden header as a record prefix (schema_version first), declaring
/// `events` exported events and nothing filtered or truncated.
fn header(events: usize) -> String {
    format!(
        r#""schema_version":"rsia.trace.v1","exporter":{{"name":"rsia","version":"reader-golden.v1","commit":"7d22edb5dfe6ec84f7a3167aba02ae683e9941be"}},"run_id":"ag148-reader-golden-run","redaction_report":{{"schema_version":"rsia.trace.redaction.v1","scan_status":"passed","review_status":"reviewed","removed_items":0,"redacted_items":0}},"coverage":{{"discovery_exhausted":true,"files_known":true,"bytes_read":22,"parsed_ok":1,"parsed_fail":0,"permission_denied":0,"missing":0,"unsupported_format":0,"zero_records":0,"events_seen":{events},"events_exported":{events},"events_filtered":0,"event_truncated":0,"char_truncated":0,"parameter_truncated_events":0,"parameter_truncated_bytes":0,"excerpt_truncated":0,"outbound_truncated":0}}"#
    )
}

fn envelope(events: &[&str]) -> String {
    format!(
        r#"{{{},"events":[{}]}}"#,
        header(events.len()),
        events.join(",")
    )
}

/// One complete record (header plus event fields) of a trace of `events`.
fn record(event: &str, events: usize) -> String {
    format!("{{{},{event}}}", header(events))
}

#[track_caller]
fn refused(body: &str, message: &str) {
    let outcome = parse_fixture(SourceFormat::RsiaTraceV1, body);
    assert!(
        matches!(&outcome, Err(Error::Invalid(actual)) if actual == message),
        "{message}: {outcome:?}"
    );
}

fn selection() -> SourceSelection {
    SourceSelection {
        roots: vec!["/selected".into()],
        run_ids: vec![],
        purpose: Purpose::Development,
        allow_model_excerpts: true,
    }
}

async fn open(namespace: &str) -> (tempfile::TempDir, Store, Context) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("rsia.sqlite3"))
        .await
        .unwrap();
    let admin = Context::new(namespace, "import-admin", Role::Admin).unwrap();
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&admin, &hash(b"ag148 watermark"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    (directory, store, admin)
}

/// Writes the present files under one canonical root and registers them all
/// (a `None` body stays missing).
async fn register(
    store: &Store,
    admin: &Context,
    root: &std::path::Path,
    key: &str,
    files: &[(&str, SourceFormat, Option<&[u8]>)],
) -> SourceSelectionRecord {
    let root = root.canonicalize().unwrap();
    let mut sources = Vec::new();
    for (name, reader, bytes) in files {
        let path = root.join(name);
        if let Some(bytes) = bytes {
            std::fs::write(&path, bytes).unwrap();
        }
        sources.push(ImportSourceSpec {
            source_id: (*name).into(),
            path: path.to_string_lossy().into_owned(),
            reader: *reader,
            expected_digest: hash(bytes.unwrap_or(b"never written")),
        });
    }
    PersistentImportService::new(store.clone())
        .register(
            admin,
            ImportRegistrationRequest {
                schema_version: IMPORT_REGISTRATION_SCHEMA.into(),
                request_key: key.into(),
                roots: vec![root.to_string_lossy().into_owned()],
                purpose: Purpose::Development,
                allow_model_excerpts: true,
                outbound_authorized: true,
                retention_scope: ImportRetentionScope::LocalWithAuthorizedExcerpts,
                sources,
            },
        )
        .await
        .unwrap()
}

struct Imported {
    directory: tempfile::TempDir,
    store: Store,
    admin: Context,
    service: PersistentImportService,
    selection: SourceSelectionRecord,
    result: ImportResultRecord,
}

/// A healthy current import of the golden and the original Pi/Claude fixtures.
async fn import_three() -> Imported {
    let (directory, store, admin) = open("ag148-current").await;
    let selection = register(
        &store,
        &admin,
        directory.path(),
        "three-readers",
        &[
            (
                "native.json",
                SourceFormat::RsiaTraceV1,
                Some(GOLDEN.as_bytes()),
            ),
            ("pi.json", SourceFormat::RsihPiFixture, Some(ORIGINAL_PI)),
            (
                "claude.jsonl",
                SourceFormat::ClaudeFixture,
                Some(ORIGINAL_CLAUDE),
            ),
        ],
    )
    .await;
    let service = PersistentImportService::new(store.clone());
    let result = service.execute(&admin, &selection.id).await.unwrap();
    assert_eq!(result.payload.state, ImportResultState::Complete);
    Imported {
        directory,
        store,
        admin,
        service,
        selection,
        result,
    }
}

async fn put(store: &Store, ctx: &Context, id: &str, body: &Value) {
    let mut session = store.session().await.unwrap();
    session
        .put(ctx, "artifact", id, ctx.actor(), body)
        .await
        .unwrap();
    session.commit().await.unwrap();
}

async fn stored_body(store: &Store, ctx: &Context, id: &str) -> String {
    let mut session = store.session().await.unwrap();
    let row = session.object_row(ctx, "artifact", id).await.unwrap();
    session.commit().await.unwrap();
    row.unwrap().body
}

async fn artifacts(store: &Store, ctx: &Context) -> Vec<Value> {
    let mut session = store.session().await.unwrap();
    let rows = session.list(ctx, "artifact").await.unwrap();
    session.commit().await.unwrap();
    rows
}

fn material_request(
    result: &ImportResultRecord,
    events: &[(&str, usize)],
) -> ImportMaterialReadRequest {
    ImportMaterialReadRequest {
        result: ImportArtifactRef {
            schema: ImportArtifactSchema::ImportResult,
            id: result.id.clone(),
            object_digest: fingerprint(result).unwrap(),
        },
        events: events
            .iter()
            .map(|(source_id, event_index)| ImportEventKey {
                source_id: (*source_id).into(),
                event_index: *event_index,
            })
            .collect(),
    }
}

struct Legacy {
    _directory: tempfile::TempDir,
    store: Store,
    admin: Context,
    fixture: Value,
    selection: SourceSelectionRecord,
    result: LegacyImportResultRecordV1,
}

/// Installs the portable v1 rows with the edges register/execute wrote, and
/// optionally the raw blobs of the three original inputs.
async fn install_legacy(with_blobs: bool) -> Legacy {
    let fixture: Value = serde_json::from_str(PORTABLE).unwrap();
    assert_eq!(
        fixture["label"],
        "synthetic_derivative_not_observed_producer"
    );
    let rows = &fixture["rows"];
    let selection: SourceSelectionRecord =
        serde_json::from_str(rows["selection"].as_str().unwrap()).unwrap();
    let sources: Vec<ImportSourceRecord> = rows["import_sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| serde_json::from_str(row.as_str().unwrap()).unwrap())
        .collect();
    let result: LegacyImportResultRecordV1 =
        serde_json::from_str(rows["result"].as_str().unwrap()).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("rsia.sqlite3"))
        .await
        .unwrap();
    let admin = Context::new(LEGACY_NAMESPACE, "legacy-admin", Role::Admin).unwrap();
    let mut session = store.session().await.unwrap();
    let watermark = session
        .bump_watermark(&admin, &hash(b"ag148 legacy capture watermark"))
        .await
        .unwrap();
    assert_eq!(
        watermark,
        fixture["observed_producer"]["watermark"]["sequence"]
    );
    for source in &sources {
        session
            .put(&admin, "artifact", &source.id, admin.actor(), source)
            .await
            .unwrap();
        session
            .put_edge(
                &admin,
                "artifact",
                &source.id,
                "blob",
                &source.payload.raw_blob_digest,
            )
            .await
            .unwrap();
    }
    session
        .put(&admin, "artifact", &selection.id, admin.actor(), &selection)
        .await
        .unwrap();
    session
        .put(&admin, "artifact", &result.id, admin.actor(), &result)
        .await
        .unwrap();
    for target in std::iter::once(&selection.id).chain(sources.iter().map(|source| &source.id)) {
        session
            .put_edge(&admin, "artifact", &result.id, "artifact", target)
            .await
            .unwrap();
    }
    for source in &sources {
        session
            .put_edge(&admin, "artifact", &selection.id, "artifact", &source.id)
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    if with_blobs {
        for raw in [ORIGINAL_NATIVE, ORIGINAL_PI, ORIGINAL_CLAUDE] {
            assert_eq!(store.put_blob(&admin, raw).await.unwrap(), hash(raw));
        }
    }
    Legacy {
        _directory: directory,
        store,
        admin,
        fixture,
        selection,
        result,
    }
}

// ---------------------------------------------------------------------------
// Reader (§11.2.1, V117.b)
// ---------------------------------------------------------------------------

#[test]
fn native_envelope_requires_schema_and_each_role() {
    assert_eq!(golden()["observed_exporter"], false);
    assert_eq!(
        detect_format_from_bytes(GOLDEN.as_bytes()).unwrap(),
        SourceFormat::RsiaTraceV1
    );
    let events = parse_fixture(SourceFormat::RsiaTraceV1, GOLDEN).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        (
            events[0].role.as_str(),
            events[0].kind.as_str(),
            events[0].content.as_str()
        ),
        ("user", "chat", "reader software golden")
    );
    // Every header and event field is explicit; a missing one is never defaulted.
    for field in [
        "schema_version",
        "exporter",
        "run_id",
        "redaction_report",
        "coverage",
    ] {
        let mut value = golden();
        value.as_object_mut().unwrap().remove(field);
        refused(
            &value.to_string(),
            &format!("missing {field} for rsia.trace"),
        );
    }
    let mut unlabeled = golden();
    unlabeled.as_object_mut().unwrap().remove("schema_version");
    assert!(detect_format_from_bytes(unlabeled.to_string().as_bytes()).is_err());
    for field in ["sequence", "role", "kind", "content"] {
        let mut value = golden();
        value["events"][0].as_object_mut().unwrap().remove(field);
        refused(
            &value.to_string(),
            &format!("missing {field} for rsia.trace"),
        );
    }
    // A later event without a role is refused too; it never becomes `user`.
    refused(
        &envelope(&[
            r#"{"sequence":0,"role":"user","kind":"chat","content":"first"}"#,
            r#"{"sequence":1,"kind":"chat","content":"second"}"#,
        ]),
        "missing role for rsia.trace",
    );
    for role in [json!(null), json!(7), json!(true), json!({"name":"user"})] {
        let mut value = golden();
        value["events"][0]["role"] = role;
        refused(&value.to_string(), "role must be a string");
    }
    // Header values outside the frozen contract.
    for (pointer, replacement, message) in [
        (
            "/exporter/name",
            json!("other"),
            "invalid exporter for rsia.trace",
        ),
        (
            "/exporter/commit",
            json!("ABCDEF"),
            "invalid exporter for rsia.trace",
        ),
        (
            "/exporter/version",
            json!("bad version"),
            "invalid exporter for rsia.trace",
        ),
        ("/run_id", json!(""), "invalid run_id for rsia.trace"),
        (
            "/redaction_report/scan_status",
            json!("skipped"),
            "invalid redaction_report for rsia.trace",
        ),
        (
            "/redaction_report/removed_items",
            json!(-1),
            "invalid redaction_report for rsia.trace",
        ),
        (
            "/coverage/events_exported",
            json!(2),
            "inconsistent coverage for rsia.trace",
        ),
        (
            "/coverage/events_filtered",
            json!(u64::MAX),
            "inconsistent coverage for rsia.trace",
        ),
        (
            "/coverage/parameter_truncated_bytes",
            json!(-1),
            "invalid coverage for rsia.trace",
        ),
    ] {
        let mut value = golden();
        *value.pointer_mut(pointer).unwrap() = replacement;
        refused(&value.to_string(), message);
    }
    let mut extended = golden();
    extended["coverage"]["guessed_total"] = json!(1);
    refused(&extended.to_string(), "invalid coverage for rsia.trace");
}

#[test]
fn native_single_record_requires_schema_and_role() {
    let single = record(
        r#""sequence":4,"role":"assistant","kind":"thought","content":"single record""#,
        1,
    );
    assert_eq!(
        detect_format_from_bytes(single.as_bytes()).unwrap(),
        SourceFormat::RsiaTraceV1
    );
    let events = parse_fixture(SourceFormat::RsiaTraceV1, &single).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].role, "assistant");
    assert_eq!(events[0].kind, "thought");
    assert_eq!(
        (events[0].byte_start, events[0].byte_end),
        (0, single.len())
    );
    let value: Value = serde_json::from_str(&single).unwrap();
    for field in [
        "schema_version",
        "run_id",
        "sequence",
        "role",
        "kind",
        "content",
    ] {
        let mut changed = value.clone();
        changed.as_object_mut().unwrap().remove(field);
        refused(
            &changed.to_string(),
            &format!("missing {field} for rsia.trace"),
        );
    }
    let mut null_role = value.clone();
    null_role["role"] = json!(null);
    refused(&null_role.to_string(), "role must be a string");
}

#[test]
fn native_jsonl_checks_every_nonempty_record() {
    let line = |sequence: u64, role: &str, content: &str| {
        record(
            &format!(
                r#""sequence":{sequence},"role":"{role}","kind":"chat","content":"{content}""#
            ),
            3,
        )
    };
    let (first, second, third) = (
        line(0, "user", "one"),
        line(2, "assistant", "two"),
        line(5, "user", "three"),
    );
    // Sequence gaps (filtered events) are allowed; blank lines are skipped.
    let body = format!("{first}\n\n{second}\n{third}\n");
    assert_eq!(
        detect_format_from_bytes(body.as_bytes()).unwrap(),
        SourceFormat::RsiaTraceV1
    );
    let events = parse_fixture(SourceFormat::RsiaTraceV1, &body).unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_index)
            .collect::<Vec<_>>(),
        [0, 2, 3]
    );
    for (event, line) in events.iter().zip([&first, &second, &third]) {
        assert_eq!(&body[event.byte_start..event.byte_end], line.as_str());
    }
    // The second record is checked as fully as the first; nothing is inherited.
    let second_value: Value = serde_json::from_str(&second).unwrap();
    let with_second = |changed: &Value| format!("{first}\n{changed}\n{third}\n");
    for field in [
        "schema_version",
        "exporter",
        "run_id",
        "redaction_report",
        "coverage",
        "role",
    ] {
        let mut changed = second_value.clone();
        changed.as_object_mut().unwrap().remove(field);
        refused(
            &with_second(&changed),
            &format!("missing {field} for rsia.trace"),
        );
    }
    for (pointer, replacement, message) in [
        (
            "/run_id",
            json!("another-run"),
            "header mismatch for rsia.trace",
        ),
        (
            "/coverage/events_exported",
            json!(2),
            "header mismatch for rsia.trace",
        ),
        (
            "/sequence",
            json!(0),
            "sequence must strictly increase for rsia.trace",
        ),
        (
            "/schema_version",
            json!("rsia.trace.v2"),
            "unsupported_format:rsia.trace.v2",
        ),
        (
            "/format",
            json!("claude.fixture"),
            "format discriminator mismatch",
        ),
    ] {
        let mut changed = second_value.clone();
        if let Some(target) = changed.pointer_mut(pointer) {
            *target = replacement;
        } else {
            changed[pointer.trim_start_matches('/')] = replacement;
        }
        let outcome = parse_fixture(SourceFormat::RsiaTraceV1, &with_second(&changed));
        assert!(
            matches!(&outcome, Err(Error::Invalid(actual) | Error::Conflict(actual)) if actual == message),
            "{pointer}: {outcome:?}"
        );
    }
    // The declared count must match the records actually present.
    refused(
        &format!("{first}\n{second}\n"),
        "inconsistent coverage for rsia.trace",
    );
}

#[test]
fn native_role_type_and_original_bytes_are_preserved() {
    let body = envelope(&[
        r#"{"sequence":0,"role":"User","kind":"Chat","content":"a"}"#,
        r#"{"sequence":1,"role":" assistant ","kind":"tool_result","content":"b"}"#,
        r#"{"sequence":7,"role":"tool","kind":"command_echo","content":"c"}"#,
    ]);
    let events = parse_fixture(SourceFormat::RsiaTraceV1, &body).unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.role.as_str())
            .collect::<Vec<_>>(),
        ["User", " assistant ", "tool"]
    );
    assert_eq!(
        events
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>(),
        ["Chat", "tool_result", "command_echo"]
    );
    let ingested =
        ingest_imported_sources(&selection(), &[("/selected/roles", body.as_bytes())], None)
            .unwrap();
    assert_eq!(
        &body[ingested.events[0].byte_start..ingested.events[0].byte_end],
        r#"{"sequence":0,"role":"User","kind":"Chat","content":"a"}"#
    );
    for (event, locator) in ingested.events.iter().zip(&ingested.locators) {
        let fragment = &body.as_bytes()[event.byte_start..event.byte_end];
        assert_eq!(event.raw_fragment_digest, hash(fragment));
        assert_eq!(
            locator.verify_and_extract(body.as_bytes()).unwrap(),
            fragment
        );
    }
    for (field, replacement, message) in [
        ("role", json!(7), "role must be a string"),
        ("role", json!(""), "invalid role for rsia.trace"),
        ("role", json!("us\u{0}er"), "invalid role for rsia.trace"),
        ("kind", json!(""), "invalid kind for rsia.trace"),
        ("content", json!(7), "content must be a string"),
        (
            "sequence",
            json!(-1),
            "sequence must be an unsigned integer",
        ),
        (
            "sequence",
            json!("0"),
            "sequence must be an unsigned integer",
        ),
    ] {
        let mut value = golden();
        value["events"][0][field] = replacement;
        refused(&value.to_string(), message);
    }
}

#[test]
fn internal_authority_alias_is_refused_by_auto_and_pinned_readers() {
    let alias = "rsia.optimization.source.v1";
    let message = format!("unsupported_format:{alias}");
    let mut renamed = golden();
    renamed["schema_version"] = json!(alias);
    let mut formatted = golden();
    formatted["format"] = json!(alias);
    for body in [renamed.to_string(), formatted.to_string()] {
        assert!(
            matches!(detect_format_from_bytes(body.as_bytes()), Err(Error::Invalid(actual)) if actual == message)
        );
        refused(&body, &message);
    }
    let healthy = record(
        r#""sequence":0,"role":"user","kind":"chat","content":"one""#,
        2,
    );
    let mut second: Value = serde_json::from_str(&record(
        r#""sequence":1,"role":"user","kind":"chat","content":"two""#,
        2,
    ))
    .unwrap();
    second["schema_version"] = json!(alias);
    let jsonl = format!("{healthy}\n{second}\n");
    assert_eq!(
        detect_format_from_bytes(jsonl.as_bytes()).unwrap(),
        SourceFormat::RsiaTraceV1
    );
    refused(&jsonl, &message);
    let ingested = ingest_imported_sources(
        &selection(),
        &[
            ("/selected/golden", GOLDEN.as_bytes()),
            ("/selected/alias", renamed.to_string().as_bytes()),
        ],
        None,
    )
    .unwrap();
    assert_eq!(ingested.aggregate_summary.coverage.unsupported_format, 1);
    assert_eq!(ingested.aggregate_summary.coverage.parsed_ok, 1);
    assert_eq!(ingested.evidence_set.members.len(), 1);
    assert_eq!(ingested.events.len(), 1);
}

#[test]
fn native_unknown_version_is_distinct_from_invalid_record() {
    let mut unknown = golden();
    unknown["schema_version"] = json!("rsia.trace.v2");
    let mut no_role = golden();
    no_role["events"][0].as_object_mut().unwrap().remove("role");
    let mut codex = golden();
    codex["schema_version"] = json!("codex.session.v1");
    let (unknown, no_role, codex) = (unknown.to_string(), no_role.to_string(), codex.to_string());
    refused(&unknown, "unsupported_format:rsia.trace.v2");
    refused(&codex, "unsupported_format:codex");
    refused(&no_role, "missing role for rsia.trace");
    assert!(detect_format("codex").is_err());
    let pinned: Vec<_> = [
        ("healthy", GOLDEN),
        ("unknown", unknown.as_str()),
        ("codex", codex.as_str()),
        ("no-role", no_role.as_str()),
    ]
    .into_iter()
    .map(|(name, body)| PinnedSourceBytes {
        source_name: name,
        raw_bytes: body.as_bytes(),
        reader: SourceFormat::RsiaTraceV1,
        cluster_hint: Some(name),
    })
    .collect();
    let ingested = ingest_pinned_imported_sources(&selection(), &pinned, None).unwrap();
    let coverage = &ingested.aggregate_summary.coverage;
    assert_eq!(
        (
            coverage.parsed_ok,
            coverage.unsupported_format,
            coverage.parsed_fail
        ),
        (1, 2, 1)
    );
    assert_eq!(ingested.evidence_set.members.len(), 1);
}

#[tokio::test]
async fn native_invalid_second_record_keeps_no_success_prefix() {
    let first = r#"{"sequence":0,"role":"user","kind":"chat","content":"prefix must not survive"}"#;
    let bad = envelope(&[first, r#"{"sequence":1,"role":"user","content":"no kind"}"#]);
    refused(&bad, "missing kind for rsia.trace");
    let jsonl = format!(
        "{}\n{}\n",
        record(
            r#""sequence":0,"role":"user","kind":"chat","content":"prefix must not survive""#,
            2
        ),
        record(r#""sequence":1,"role":"user","content":"no kind""#, 2)
    );
    refused(&jsonl, "missing kind for rsia.trace");
    let claude: &[u8] = br#"{"format":"claude.fixture","role":"user","content":"claude healthy"}"#;
    let ingested = ingest_imported_sources(
        &selection(),
        &[
            ("/selected/bad", bad.as_bytes()),
            ("/selected/jsonl", jsonl.as_bytes()),
            ("/selected/claude", claude),
        ],
        None,
    )
    .unwrap();
    assert_eq!(ingested.events.len(), 1);
    assert_eq!(ingested.events[0].content, "claude healthy");
    assert_eq!(ingested.evidence_set.members.len(), 1);
    assert_eq!(ingested.aggregate_summary.coverage.parsed_fail, 2);
    assert!(
        ingested
            .locators
            .iter()
            .all(|locator| locator.source_id == ingested.evidence_set.members[0].source_id)
    );
    // The persisted v2 result of the same sources keeps no prefix either.
    let (directory, store, admin) = open("ag148-prefix").await;
    let selection = register(
        &store,
        &admin,
        directory.path(),
        "no-prefix",
        &[
            ("bad.json", SourceFormat::RsiaTraceV1, Some(bad.as_bytes())),
            ("claude.jsonl", SourceFormat::ClaudeFixture, Some(claude)),
        ],
    )
    .await;
    let result = PersistentImportService::new(store.clone())
        .execute(&admin, &selection.id)
        .await
        .unwrap();
    assert_eq!(result.payload.state, ImportResultState::Partial);
    assert_eq!(result.payload.import_coverage.parsed_fail, 1);
    assert!(result.payload.native_traces.is_empty());
    let claude_source = &selection.payload.import_source_ids[1];
    assert!(
        result
            .payload
            .event_records
            .iter()
            .all(|event| event.source_id == *claude_source)
    );
    assert_eq!(result.payload.evidence_set.unwrap().members.len(), 1);
}

#[tokio::test]
async fn mixed_source_partial_coverage_keeps_precise_causes() {
    let mut unknown = golden();
    unknown["schema_version"] = json!("rsia.trace.v2");
    let mut no_role = golden();
    no_role["events"][0].as_object_mut().unwrap().remove("role");
    let (unknown, no_role) = (unknown.to_string(), no_role.to_string());
    let (directory, store, admin) = open("ag148-mixed").await;
    let selection = register(
        &store,
        &admin,
        directory.path(),
        "mixed",
        &[
            (
                "native.json",
                SourceFormat::RsiaTraceV1,
                Some(GOLDEN.as_bytes()),
            ),
            (
                "unknown.json",
                SourceFormat::RsiaTraceV1,
                Some(unknown.as_bytes()),
            ),
            (
                "no-role.json",
                SourceFormat::RsiaTraceV1,
                Some(no_role.as_bytes()),
            ),
            ("pi.json", SourceFormat::RsihPiFixture, Some(ORIGINAL_PI)),
            (
                "claude.jsonl",
                SourceFormat::ClaudeFixture,
                Some(ORIGINAL_CLAUDE),
            ),
            ("missing.json", SourceFormat::RsiaTraceV1, None),
        ],
    )
    .await;
    let result = PersistentImportService::new(store.clone())
        .execute(&admin, &selection.id)
        .await
        .unwrap();
    let payload = &result.payload;
    assert_eq!(payload.state, ImportResultState::Partial);
    let read = payload.aggregate_summary.coverage.bytes_read;
    assert_eq!(
        read,
        (GOLDEN.len() + unknown.len() + no_role.len() + ORIGINAL_PI.len() + ORIGINAL_CLAUDE.len())
            as u64
    );
    let own = &payload.import_coverage;
    assert_eq!(
        (
            own.parsed_ok,
            own.unsupported_format,
            own.parsed_fail,
            own.missing
        ),
        (3, 1, 1, 1)
    );
    assert_eq!(
        (own.events_seen, own.events_exported, own.events_filtered),
        (3, 3, 0)
    );
    assert_eq!(
        (
            own.parameter_truncated_events,
            own.parameter_truncated_bytes
        ),
        (0, 0)
    );
    assert_eq!(own.bytes_read, read);
    // The exporter's self-report is kept apart and never overwrites this import.
    assert_eq!(payload.native_traces.len(), 1);
    let native = &payload.native_traces[0];
    assert_eq!(native.source_id, selection.payload.import_source_ids[0]);
    assert_eq!(native.event_sequences, [(0, 0)]);
    assert_eq!(native.header.run_id, "ag148-reader-golden-run");
    assert_eq!(
        (
            native.header.coverage.bytes_read,
            native.header.coverage.parsed_ok
        ),
        (22, 1)
    );
    assert!(native.header.coverage.complete());
    // A clean import whose declaration is partial stays partial: a self-report
    // can lower the state, never raise it.
    for (pointer, replacement) in [
        ("/coverage/char_truncated", json!(1)),
        ("/coverage/discovery_exhausted", json!(false)),
        ("/coverage/parameter_truncated_events", json!(1)),
    ] {
        let mut declared = golden();
        *declared.pointer_mut(pointer).unwrap() = replacement;
        let declared = declared.to_string();
        let selection = register(
            &store,
            &admin,
            directory.path(),
            &format!("declared-{}", pointer.rsplit('/').next().unwrap()),
            &[(
                "declared.json",
                SourceFormat::RsiaTraceV1,
                Some(declared.as_bytes()),
            )],
        )
        .await;
        let result = PersistentImportService::new(store.clone())
            .execute(&admin, &selection.id)
            .await
            .unwrap();
        assert!(result.payload.import_coverage.complete(), "{pointer}");
        assert_eq!(
            result.payload.state,
            ImportResultState::Partial,
            "{pointer}"
        );
    }
}

#[tokio::test]
async fn native_import_never_becomes_trusted_execution() {
    let mut claiming = golden();
    claiming["task_origin"] = json!("trusted_run");
    claiming["execution_attestation"] = json!("trusted_host");
    claiming["events"][0]["attestation"] = json!("trusted_host");
    claiming["events"][0]["receipt"] = json!("applied");
    let claiming = claiming.to_string();
    let events = parse_fixture(SourceFormat::RsiaTraceV1, &claiming).unwrap();
    assert_eq!(events[0].task_origin, TaskOrigin::ImportedHistory);
    assert_eq!(
        events[0].attestation,
        ExecutionAttestation::UnverifiedImport
    );
    let (directory, store, admin) = open("ag148-trust").await;
    let selection = register(
        &store,
        &admin,
        directory.path(),
        "claims-trust",
        &[(
            "claims.json",
            SourceFormat::RsiaTraceV1,
            Some(claiming.as_bytes()),
        )],
    )
    .await;
    let result = PersistentImportService::new(store.clone())
        .execute(&admin, &selection.id)
        .await
        .unwrap();
    for member in &result.payload.evidence_set.as_ref().unwrap().members {
        assert_eq!(member.task_origin, TaskOrigin::ImportedHistory);
        assert_eq!(
            member.execution_attestation,
            ExecutionAttestation::UnverifiedImport
        );
    }
    let evaluator = Context::new("ag148-trust", "evaluator", Role::Evaluator).unwrap();
    let summary = read_persisted_imported_evidence(&evaluator, &store, &result.id)
        .await
        .unwrap();
    assert!(!summary.formal_evaluation_eligible);
    assert!(!summary.legacy);
    assert_eq!(
        summary.execution_attestation,
        ExecutionAttestation::UnverifiedImport
    );
    let host = Context::new("ag148-trust", "host", Role::Host).unwrap();
    let mut session = store.session().await.unwrap();
    let runs: Vec<Value> = session.list(&admin, "run").await.unwrap();
    assert!(runs.is_empty());
    for id in std::iter::once(&result.id).chain(&selection.payload.import_source_ids) {
        assert!(load_stored_source(&mut session, &host, id).await.is_err());
    }
    session.commit().await.unwrap();
}

#[test]
fn pinned_pi_and_claude_fixtures_keep_original_contracts() {
    assert_eq!(
        hash(ORIGINAL_NATIVE),
        "b8d7f5187a765212afd651c0265559a3d8b1a6c6b69d52dad869527043f340b1"
    );
    assert_eq!(
        hash(ORIGINAL_PI),
        "0925270c90b7b1ef2f84a601c15fad98401edac32adce87753366234d2a66887"
    );
    assert_eq!(
        hash(ORIGINAL_CLAUDE),
        "f1fb8ef66b698e63e88334e8a4bcbab7960198329fd9b1931a4a15c6082b8569"
    );
    let pi = parse_fixture(
        SourceFormat::RsihPiFixture,
        std::str::from_utf8(ORIGINAL_PI).unwrap(),
    )
    .unwrap();
    assert_eq!(
        (
            pi.len(),
            pi[0].role.as_str(),
            pi[0].kind.as_str(),
            pi[0].content.as_str()
        ),
        (1, "user", "chat", "hi")
    );
    let claude = parse_fixture(
        SourceFormat::ClaudeFixture,
        std::str::from_utf8(ORIGINAL_CLAUDE).unwrap(),
    )
    .unwrap();
    assert_eq!(
        (
            claude.len(),
            claude[0].role.as_str(),
            claude[0].content.as_str()
        ),
        (1, "user", "hello")
    );
    // Pi and Claude keep their own lenient role rule; AG-148 does not tighten them.
    let pi_without_role = parse_fixture(
        SourceFormat::RsihPiFixture,
        r#"{"format":"rsih.pi.fixture","prompts":[{"text":"no role"}]}"#,
    )
    .unwrap();
    assert_eq!(pi_without_role[0].role, "user");
    let claude_without_role =
        parse_fixture(SourceFormat::ClaudeFixture, r#"{"content":"no role"}"#).unwrap();
    assert_eq!(claude_without_role[0].role, "user");
    assert!(matches!(
        parse_fixture(SourceFormat::ClaudeFixture, r#"{"role":"user"}"#),
        Err(Error::Invalid(message)) if message == "content must be a string"
    ));
    for event in pi.iter().chain(&claude) {
        assert_eq!(event.task_origin, TaskOrigin::ImportedHistory);
        assert_eq!(event.attestation, ExecutionAttestation::UnverifiedImport);
    }
    // The original Native sample predates the frozen header: refused, unchanged.
    refused(
        std::str::from_utf8(ORIGINAL_NATIVE).unwrap(),
        "missing schema_version for rsia.trace",
    );
}

// ---------------------------------------------------------------------------
// Versioned result and its consumers (§5.1.1, E16.1)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn result_v2_identity_binds_reader_and_normalization_versions() {
    assert_eq!(
        IMPORT_RESULT_VERSIONS
            .iter()
            .map(|row| (row.schema_version, row.retains_judge))
            .collect::<Vec<_>>(),
        [
            (LEGACY_IMPORT_RESULT_SCHEMA_V1, false),
            (IMPORT_RESULT_SCHEMA_V2, true)
        ]
    );
    let imported = import_three().await;
    let (selection, result) = (&imported.selection, &imported.result);
    assert_eq!(result.schema_version, IMPORT_RESULT_SCHEMA_V2);
    assert_eq!(result.payload.reader_version, IMPORT_READER_SET_V2);
    assert_eq!(
        result.payload.normalization_version,
        IMPORT_NORMALIZATION_V2
    );
    assert_eq!(
        result.id,
        import_result_id_v2(&selection.id, &selection.input_digest).unwrap()
    );
    let tuple = |reader: &str, normalization: &str| {
        let digest = fingerprint(&(
            IMPORT_RESULT_SCHEMA_V2,
            reader,
            normalization,
            selection.id.as_str(),
            selection.input_digest.as_str(),
        ))
        .unwrap();
        format!("e16res-{}", &digest[..24])
    };
    assert_eq!(
        tuple(IMPORT_READER_SET_V2, IMPORT_NORMALIZATION_V2),
        result.id
    );
    assert_ne!(
        tuple("rsia.e16.reader_set.v1", IMPORT_NORMALIZATION_V2),
        result.id
    );
    assert_ne!(
        tuple(IMPORT_READER_SET_V2, "rsia.e16.import_normalization.v1"),
        result.id
    );
    assert_ne!(result.id, legacy_import_result_id_v1(&selection.id));
    // The source keeps its format label; the algorithm lives on the result.
    let mut session = imported.store.session().await.unwrap();
    for id in &selection.payload.import_source_ids {
        let source: ImportSourceRecord =
            session.need(&imported.admin, "artifact", id).await.unwrap();
        assert_eq!(
            source.payload.reader_version,
            source.payload.reader.as_label()
        );
    }
    session.commit().await.unwrap();
    let view = imported
        .service
        .read_result_version(&imported.admin, &result.id)
        .await
        .unwrap();
    assert_eq!(view.state, ImportResultReadState::CurrentVerified);
    assert_eq!(view.schema_version, IMPORT_RESULT_SCHEMA_V2);
    assert_eq!(view.current_v2_id, result.id);
    assert_eq!(view.legacy_v1_id, legacy_import_result_id_v1(&selection.id));
    assert_eq!(
        view.record,
        stored_body(&imported.store, &imported.admin, &result.id).await
    );
    // A stored v2 result claiming another algorithm version is invalid.
    for (pointer, replacement) in [
        ("/payload/reader_version", "rsia.e16.reader_set.v1"),
        (
            "/payload/normalization_version",
            "rsia.e16.import_normalization.v1",
        ),
    ] {
        let mut changed = serde_json::to_value(result).unwrap();
        *changed.pointer_mut(pointer).unwrap() = json!(replacement);
        put(&imported.store, &imported.admin, &result.id, &changed).await;
        assert!(
            matches!(
                imported.service.load_live_result(&imported.admin, &result.id).await,
                Err(Error::Conflict(message)) if message == "import_result_invalid"
            ),
            "{pointer}"
        );
    }
}

#[tokio::test]
async fn legacy_result_static_reads_keep_original_bytes() {
    assert_eq!(
        evo_storage::CAPACITY_LEGACY_IMPORT_RESULT_SCHEMA_V1,
        LEGACY_IMPORT_RESULT_SCHEMA_V1
    );
    assert_eq!(
        evo_storage::CAPACITY_IMPORT_RESULT_SCHEMA,
        IMPORT_RESULT_SCHEMA_V2
    );
    let legacy = install_legacy(true).await;
    let (store, admin, rows) = (&legacy.store, &legacy.admin, &legacy.fixture["rows"]);
    let result_text = rows["result"].as_str().unwrap();
    let mut texts = vec![
        (
            legacy.selection.id.clone(),
            rows["selection"].as_str().unwrap(),
        ),
        (legacy.result.id.clone(), result_text),
    ];
    for row in rows["import_sources"].as_array().unwrap() {
        let source: ImportSourceRecord = serde_json::from_str(row.as_str().unwrap()).unwrap();
        texts.push((source.id, row.as_str().unwrap()));
    }
    for (id, text) in &texts {
        assert_eq!(stored_body(store, admin, id).await, *text);
    }
    assert_eq!(
        hash(result_text.as_bytes()),
        legacy.fixture["row_sha256"]["result"]
    );
    let service = PersistentImportService::new(store.clone());
    let view = service
        .read_result_version(admin, &legacy.result.id)
        .await
        .unwrap();
    assert_eq!(view.state, ImportResultReadState::Legacy);
    assert_eq!(view.schema_version, LEGACY_IMPORT_RESULT_SCHEMA_V1);
    assert_eq!(view.record, result_text);
    assert_eq!(view.selection_id, legacy.selection.id);
    assert_eq!(view.legacy_v1_id, legacy.result.id);
    assert_eq!(
        view.current_v2_id,
        import_result_id_v2(&legacy.selection.id, &legacy.selection.input_digest).unwrap()
    );
    for event in &legacy.result.payload.event_records {
        let fragment = service
            .read_event_fragment(
                admin,
                &legacy.result.id,
                &event.source_id,
                event.event_index,
            )
            .await
            .unwrap();
        assert_eq!(hash(&fragment), event.raw_fragment_digest);
    }
    let evaluator = Context::new(LEGACY_NAMESPACE, "legacy-evaluator", Role::Evaluator).unwrap();
    let summary = read_persisted_imported_evidence(&evaluator, store, &legacy.result.id)
        .await
        .unwrap();
    assert!(summary.legacy);
    assert_eq!(summary.result_schema, LEGACY_IMPORT_RESULT_SCHEMA_V1);
    assert_eq!(summary.total_events, 5);
    assert!(!summary.formal_evaluation_eligible);
    assert!(matches!(
        service.load_live_result(admin, &legacy.result.id).await,
        Err(Error::Conflict(message)) if message == "import_result_legacy"
    ));
    let report =
        inspect_e16_content_edges(admin, store, &legacy.result.id, InspectionBudget::default())
            .await
            .unwrap();
    assert_eq!(report.status, InspectionStatus::Complete, "{report:#?}");
    // Reads, audits and inspections never rewrite the historical rows.
    for (id, text) in &texts {
        assert_eq!(stored_body(store, admin, id).await, *text);
    }
}

#[tokio::test]
async fn legacy_material_refusal_precedes_reconstruction() {
    // No raw blob is installed: any reconstruction would fail on the read.
    let legacy = install_legacy(false).await;
    let (store, admin) = (&legacy.store, &legacy.admin);
    let before = artifacts(store, admin).await;
    let current_ref = ImportArtifactRef {
        schema: ImportArtifactSchema::ImportResult,
        id: legacy.result.id.clone(),
        object_digest: fingerprint(&legacy.result).unwrap(),
    };
    let legacy_ref = ImportArtifactRef {
        schema: ImportArtifactSchema::LegacyImportResultV1,
        ..current_ref.clone()
    };
    for result in [current_ref, legacy_ref] {
        let request = ImportMaterialReadRequest {
            result,
            events: vec![ImportEventKey {
                source_id: legacy.selection.payload.import_source_ids[1].clone(),
                event_index: 0,
            }],
        };
        assert!(
            matches!(
                read_persisted_imported_development_material(admin, store, &request).await,
                Err(Error::Conflict(message)) if message == "import_result_legacy"
            ),
            "{:?}",
            request.result.schema
        );
    }
    let request = ImportMaterialReadRequest {
        result: ImportArtifactRef {
            schema: ImportArtifactSchema::LegacyImportResultV1,
            id: legacy.result.id.clone(),
            object_digest: fingerprint(&legacy.result).unwrap(),
        },
        events: vec![],
    };
    assert!(matches!(
        request.validate(),
        Err(Error::Conflict(message)) if message == "import_result_legacy"
    ));
    assert!(
        store
            .read_blob(admin, &hash(ORIGINAL_PI), 1024)
            .await
            .is_err()
    );
    assert_eq!(artifacts(store, admin).await, before);
}

#[tokio::test]
async fn current_material_recomputes_from_exact_raw() {
    let imported = import_three().await;
    let native_source = imported.selection.payload.import_source_ids[0].clone();
    let request = material_request(&imported.result, &[(native_source.as_str(), 0)]);
    let material =
        read_persisted_imported_development_material(&imported.admin, &imported.store, &request)
            .await
            .unwrap();
    assert_eq!(material.excerpts.len(), 1);
    assert_eq!(material.excerpts[0].content, "reader software golden");
    let original = serde_json::to_value(&imported.result).unwrap();
    for (pointer, replacement) in [
        ("/payload/import_coverage/bytes_read", json!(1)),
        (
            "/payload/import_coverage/parameter_truncated_events",
            json!(1),
        ),
        ("/payload/native_traces/0/event_sequences/0/1", json!(9)),
        (
            "/payload/native_traces/0/header/run_id",
            json!("another-run"),
        ),
        (
            "/payload/native_traces/0/header/coverage/bytes_read",
            json!(23),
        ),
        ("/payload/state", json!("partial")),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        put(
            &imported.store,
            &imported.admin,
            &imported.result.id,
            &changed,
        )
        .await;
        let typed: ImportResultRecord = serde_json::from_value(changed).unwrap();
        let mut rebound = request.clone();
        rebound.result.object_digest = fingerprint(&typed).unwrap();
        let mismatch = |outcome: &evo_core::Result<_>| matches!(outcome, Err(Error::Conflict(message)) if message == "persisted import derivation mismatch");
        let outcome = read_persisted_imported_development_material(
            &imported.admin,
            &imported.store,
            &rebound,
        )
        .await;
        assert!(mismatch(&outcome.map(|_| ())), "material {pointer}");
        let outcome = imported
            .service
            .read_result_version(&imported.admin, &imported.result.id)
            .await;
        assert!(mismatch(&outcome.map(|_| ())), "read view {pointer}");
    }
    put(
        &imported.store,
        &imported.admin,
        &imported.result.id,
        &original,
    )
    .await;
    read_persisted_imported_development_material(&imported.admin, &imported.store, &request)
        .await
        .unwrap();
}

#[tokio::test]
async fn unknown_result_version_is_not_legacy() {
    let imported = import_three().await;
    let (store, admin, service) = (&imported.store, &imported.admin, &imported.service);
    let id = imported.result.id.clone();
    let native_source = imported.selection.payload.import_source_ids[0].clone();
    let original = serde_json::to_value(&imported.result).unwrap();
    let evaluator = Context::new("ag148-current", "evaluator", Role::Evaluator).unwrap();
    let mut unknown = original.clone();
    unknown["schema_version"] = json!("rsia.e16.import_result.v3");
    let mut unlabeled = original.clone();
    unlabeled.as_object_mut().unwrap().remove("schema_version");
    for stored in [unknown, unlabeled] {
        put(store, admin, &id, &stored).await;
        let unsupported = |outcome: evo_core::Result<()>| matches!(outcome, Err(Error::Invalid(message)) if message == "import_result_version_unsupported");
        assert!(unsupported(
            service.load_live_result(admin, &id).await.map(|_| ())
        ));
        assert!(unsupported(
            service.read_result_version(admin, &id).await.map(|_| ())
        ));
        assert!(unsupported(
            service
                .read_event_fragment(admin, &id, &native_source, 0)
                .await
                .map(|_| ())
        ));
        assert!(unsupported(
            read_persisted_imported_evidence(&evaluator, store, &id)
                .await
                .map(|_| ())
        ));
        assert!(unsupported(
            read_persisted_imported_development_material(
                admin,
                store,
                &material_request(&imported.result, &[(native_source.as_str(), 0)]),
            )
            .await
            .map(|_| ())
        ));
    }
    let report = inspect_e16_content_edges(admin, store, &id, InspectionBudget::default())
        .await
        .unwrap();
    let object = report
        .objects
        .iter()
        .find(|object| object.id == id)
        .unwrap();
    assert_eq!(object.wire, E16Wire::Unknown);
    assert!(
        object
            .issues
            .iter()
            .any(|issue| issue.code == IssueCode::UnknownSchema)
    );
    // A known version that does not decode is invalid: never legacy, never unsupported.
    let mut extended = original.clone();
    extended["payload"]["legacy_hint"] = json!(true);
    let mut relabeled = original.clone();
    relabeled["schema_version"] = json!(LEGACY_IMPORT_RESULT_SCHEMA_V1);
    for stored in [extended, relabeled] {
        put(store, admin, &id, &stored).await;
        assert!(matches!(
            service.read_result_version(admin, &id).await,
            Err(Error::Conflict(message)) if message == "import_result_invalid"
        ));
    }
    put(store, admin, &id, &original).await;
    service.load_live_result(admin, &id).await.unwrap();
}

#[tokio::test]
async fn versioned_import_replay_is_idempotent() {
    let imported = import_three().await;
    let bytes = serde_json::to_string(&imported.result).unwrap();
    let replay = imported
        .service
        .execute(&imported.admin, &imported.selection.id)
        .await
        .unwrap();
    assert_eq!(serde_json::to_string(&replay).unwrap(), bytes);
    imported.store.close().await;
    let reopened = Store::open(&imported.directory.path().join("rsia.sqlite3"))
        .await
        .unwrap();
    let restarted = PersistentImportService::new(reopened)
        .execute(&imported.admin, &imported.selection.id)
        .await
        .unwrap();
    assert_eq!(serde_json::to_string(&restarted).unwrap(), bytes);
    // A selection with a legacy v1 result gets its own current id from the
    // same registered raw bytes; the v1 row stays byte for byte.
    let legacy = install_legacy(true).await;
    let v1_text = stored_body(&legacy.store, &legacy.admin, &legacy.result.id).await;
    let service = PersistentImportService::new(legacy.store.clone());
    let current = service
        .execute(&legacy.admin, &legacy.selection.id)
        .await
        .unwrap();
    assert_eq!(
        current.id,
        import_result_id_v2(&legacy.selection.id, &legacy.selection.input_digest).unwrap()
    );
    assert_ne!(current.id, legacy.result.id);
    // The pre-header Native sample fails the strict reader; Pi and Claude stay.
    assert_eq!(current.payload.state, ImportResultState::Partial);
    assert_eq!(current.payload.import_coverage.parsed_fail, 1);
    assert_eq!(current.payload.event_records.len(), 2);
    assert!(current.payload.native_traces.is_empty());
    let again = service
        .execute(&legacy.admin, &legacy.selection.id)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_string(&again).unwrap(),
        serde_json::to_string(&current).unwrap()
    );
    let view = service
        .read_result_version(&legacy.admin, &current.id)
        .await
        .unwrap();
    assert_eq!(view.state, ImportResultReadState::CurrentVerified);
    assert_eq!(view.legacy_v1_id, legacy.result.id);
    assert_eq!(
        stored_body(&legacy.store, &legacy.admin, &legacy.result.id).await,
        v1_text
    );
}

#[tokio::test]
async fn revoked_import_refuses_before_new_derivation() {
    let imported = import_three().await;
    let (store, admin, service) = (&imported.store, &imported.admin, &imported.service);
    let secondary = imported.selection.payload.import_source_ids[1].clone();
    LifecycleStore::begin_revoke(
        admin,
        store,
        TypedObjectRef {
            kind: "artifact".into(),
            id: secondary.clone(),
        },
        "ag148 non-primary revoke",
        10,
    )
    .await
    .unwrap();
    let before = artifacts(store, admin).await;
    let refused_live = |outcome: evo_core::Result<()>| match outcome {
        Err(Error::Forbidden) => true,
        Err(Error::Conflict(message)) => message != "persisted import derivation mismatch",
        _ => false,
    };
    let request = material_request(&imported.result, &[(secondary.as_str(), 0)]);
    assert!(refused_live(
        read_persisted_imported_development_material(admin, store, &request)
            .await
            .map(|_| ())
    ));
    assert!(refused_live(
        service
            .read_result_version(admin, &imported.result.id)
            .await
            .map(|_| ())
    ));
    assert!(refused_live(
        service
            .load_live_result(admin, &imported.result.id)
            .await
            .map(|_| ())
    ));
    assert!(refused_live(
        service
            .execute(admin, &imported.selection.id)
            .await
            .map(|_| ())
    ));
    assert_eq!(artifacts(store, admin).await, before);
}

#[tokio::test]
async fn legacy_result_cannot_become_new_e16_budget_source() {
    let legacy = install_legacy(true).await;
    let (store, admin) = (&legacy.store, &legacy.admin);
    let host = Context::new(LEGACY_NAMESPACE, "model-host", Role::Host).unwrap();
    store
        .authorize_root_budget(
            admin,
            &RootBudgetAuthorization {
                root_budget_id: "root".into(),
                billing_scope: "scope".into(),
                allowed_namespaces: vec![LEGACY_NAMESPACE.into()],
                currency: "USD".into(),
                pricing_version: "price".into(),
                payment_subject: "payer".into(),
                authorization_receipt_digest: hash(b"authorization"),
                per_call_cap_micros: 100,
                total_limit_micros: 1000,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    let key = |id: &str| TypedObjectRef {
        kind: "artifact".into(),
        id: id.into(),
    };
    let reservation = |refs: Vec<E16BudgetSourceRef>| {
        let input = BudgetArtifact::from_serializable(
            "test.future_content_input.v1",
            &json!({"content":"REQUEST-MARKER"}),
        )
        .unwrap();
        let request = E16BudgetRequest {
            schema_version: E16_BUDGET_REQUEST_SCHEMA.into(),
            namespace: LEGACY_NAMESPACE.into(),
            billing_scope: "scope".into(),
            call_id: "budget-call".into(),
            object_refs: refs,
            input_artifact: input.clone(),
        }
        .artifact()
        .unwrap();
        BudgetCallReservation {
            billing_scope: "scope".into(),
            call_id: "budget-call".into(),
            dispatch_group_id: "group-budget-call".into(),
            stage: BudgetStage::Reflection,
            actual_input_digest: input.digest,
            request_artifact: Some(request),
            max_cost_micros: 20,
            lease_token: "lease-budget-call".into(),
            lease_until: 1000,
            now: 2,
        }
    };
    let legacy_error = |outcome: evo_core::Result<()>| matches!(outcome, Err(Error::Conflict(message)) if message == "import_result_legacy");
    // Static metadata is never promoted to a new budget source.
    assert!(legacy_error(
        store
            .snapshot_e16_budget_sources(&host, &[key(&legacy.result.id)])
            .await
            .map(|_| ())
    ));
    // A hand-built binding to it is refused inside the reservation, which then
    // leaves no call behind.
    let mut refs = store
        .snapshot_e16_budget_sources(&host, &[key(&legacy.selection.id)])
        .await
        .unwrap();
    refs.push(E16BudgetSourceRef {
        namespace: LEGACY_NAMESPACE.into(),
        kind: "artifact".into(),
        id: legacy.result.id.clone(),
        schema_version: LEGACY_IMPORT_RESULT_SCHEMA_V1.into(),
        owner_actor: admin.actor().into(),
        storage_body_digest: hash(
            stored_body(store, admin, &legacy.result.id)
                .await
                .as_bytes(),
        ),
    });
    refs.sort();
    assert!(legacy_error(
        store
            .reserve_e16_budget_call(&host, &reservation(refs))
            .await
            .map(|_| ())
    ));
    assert!(
        store
            .budget_call(admin, "scope", "budget-call")
            .await
            .unwrap()
            .is_none()
    );
    // The current result of the same selection is a valid new source.
    let current = PersistentImportService::new(store.clone())
        .execute(admin, &legacy.selection.id)
        .await
        .unwrap();
    let refs = store
        .snapshot_e16_budget_sources(&host, &[key(&current.id)])
        .await
        .unwrap();
    assert_eq!(refs[0].schema_version, IMPORT_RESULT_SCHEMA_V2);
    store
        .reserve_e16_budget_call(&host, &reservation(refs))
        .await
        .unwrap();
    // A v2 row claiming another algorithm version is no current source.
    let mut forged = serde_json::to_value(&current).unwrap();
    forged["payload"]["reader_version"] = json!("rsia.e16.reader_set.v1");
    put(store, admin, &current.id, &forged).await;
    assert!(matches!(
        store.snapshot_e16_budget_sources(&host, &[key(&current.id)]).await,
        Err(Error::Conflict(message)) if message == "import_result_invalid"
    ));
}

#[tokio::test]
async fn full_capacity_allows_existing_result_replay() {
    let (directory, store, admin) = open("ag148-capacity").await;
    // A v1 result of another namespace already holds all but two events.
    let other = Context::new("ag148-other", "other-admin", Role::Admin).unwrap();
    put(
        &store,
        &other,
        "e16res-000000000000000000000000",
        &json!({
            "schema_version": LEGACY_IMPORT_RESULT_SCHEMA_V1,
            "id": "e16res-000000000000000000000000",
            "payload": {"aggregate_summary": {"total_events": 9998}}
        }),
    )
    .await;
    let two: &[u8] =
        b"{\"role\":\"user\",\"content\":\"one\"}\n{\"role\":\"user\",\"content\":\"two\"}\n";
    let selection = register(
        &store,
        &admin,
        directory.path(),
        "fills-capacity",
        &[("two.jsonl", SourceFormat::ClaudeFixture, Some(two))],
    )
    .await;
    let service = PersistentImportService::new(store.clone());
    let result = service.execute(&admin, &selection.id).await.unwrap();
    assert_eq!(result.payload.aggregate_summary.total_events, 2);
    let mut session = store.session().await.unwrap();
    assert_eq!(session.capacity_usage_v41(0).await.unwrap().events, 10_000);
    session.commit().await.unwrap();
    // The existing result replays at full capacity, also after a restart.
    let replay = service.execute(&admin, &selection.id).await.unwrap();
    assert_eq!(fingerprint(&replay).unwrap(), fingerprint(&result).unwrap());
    let restarted = PersistentImportService::new(store.clone())
        .execute(&admin, &selection.id)
        .await
        .unwrap();
    assert_eq!(restarted.id, result.id);
    // A new result does not fit and writes nothing.
    let one: &[u8] = b"{\"role\":\"user\",\"content\":\"over\"}\n";
    let over = register(
        &store,
        &admin,
        directory.path(),
        "over-capacity",
        &[("over.jsonl", SourceFormat::ClaudeFixture, Some(one))],
    )
    .await;
    let before = artifacts(&store, &admin).await;
    assert!(matches!(
        service.execute(&admin, &over.id).await,
        Err(Error::Capacity(detail)) if detail.item == evo_core::capacity::CapacityItem::Event
            && (detail.used, detail.limit, detail.delta) == (10_000, 10_000, 1)
    ));
    assert_eq!(artifacts(&store, &admin).await, before);
}
