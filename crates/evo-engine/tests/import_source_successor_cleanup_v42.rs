//! Real E16 import sources and Host-issued native authorities. The dependency
//! edges joining them are explicitly registered test closure fixtures: production
//! import currently does not generate native authorities or these edges.

use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{
    OptimizationTrace, SkillFailureDiagnosis, SkillFailureKind, TraceOutcome,
};
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, load_stored_source, read_persisted_imported_evidence,
    store_trace_authority,
};
use evo_engine::import::{
    IMPORT_REGISTRATION_SCHEMA, ImportRegistrationRequest, ImportRetentionScope, ImportSourceSpec,
    PersistentImportService, SourceFormat,
};
use evo_storage::Store;
use evo_storage::lifecycle::{
    BackupManifest, CleanupState, CleanupStatus, LifecycleStore, RevokeTombstone, TypedObjectRef,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

const NS: &str = "native-cleanup";
const RUN: &str = "shared-native-authority";
const NATIVE_SCHEMA: &str = "rsia.optimization.source.v1";

fn context(namespace: &str, role: Role) -> Context {
    let actor = if role == Role::Host { "host" } else { "admin" };
    Context::new(namespace, actor, role).unwrap()
}

fn authority(
    id: &str,
    marker: &str,
    outcome: TraceOutcome,
    empty_excerpt: bool,
) -> StoredTraceAuthority {
    let prefix = "前缀🧠;";
    let excerpt = if empty_excerpt {
        String::new()
    } else {
        format!("EXCERPT-{marker}-汉字")
    };
    let body = format!("{prefix}{excerpt};BODY-{marker}-private");
    StoredTraceAuthority {
        schema_version: NATIVE_SCHEMA.into(),
        record: StoredRunRecord {
            id: id.into(),
            body: body.as_bytes().to_vec(),
            parent_family: format!("family-{id}"),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: id.into(),
            parent_family: format!("family-{id}"),
            source_digest: hash(body.as_bytes()),
            purpose: Purpose::Development,
            outcome,
            // Host validation accepts this typed diagnosis for every outcome;
            // cleanup must not introduce new diagnosis semantics before erasing it.
            diagnosis: Some(SkillFailureDiagnosis {
                kind: SkillFailureKind::Uncertain,
                skill_id: "fixture-skill".into(),
                bundle_digest: hash(b"fixture-bundle"),
                request_digest: hash(b"fixture-request"),
                rule_id: None,
                support: vec![],
                counterexamples: vec![],
                reason: format!("DIAGNOSIS-{marker}-private"),
            }),
            excerpt,
            seed: 17,
        },
        excerpt_start: prefix.len(),
        excerpt_end: prefix.len()
            + if empty_excerpt {
                0
            } else {
                format!("EXCERPT-{marker}-汉字").len()
            },
    }
}

fn sqlite_snapshot(path: &Path, namespace: &str) -> Value {
    // Read-only inspection of exact persisted bytes and cache tombstones, using
    // the existing Python runtime rather than adding a storage dependency.
    let script = r#"import json,sqlite3,sys
con=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True)
con.row_factory=sqlite3.Row
queries={
'objects':'SELECT kind,id,body FROM objects WHERE namespace=? ORDER BY kind,id',
'cache':'SELECT actor,operation,request_key,subject_id,payload_hash,response,redacted FROM idempotency WHERE namespace=? ORDER BY actor,operation,request_key',
'edges':'SELECT src_kind,src_id,dst_kind,dst_id FROM dependencies WHERE namespace=? ORDER BY src_kind,src_id,dst_kind,dst_id',
'audit':'SELECT payload,previous_hash,digest FROM audit WHERE namespace=? ORDER BY seq',
'events':'SELECT job_id,node_kind,node_id,event_kind,details FROM revoke_cleanup_events WHERE namespace=? ORDER BY seq',
'frontier':'SELECT job_id,node_kind,node_id,expanded FROM revoke_cleanup_frontier WHERE namespace=? ORDER BY seq',
}
print(json.dumps({name:[dict(row) for row in con.execute(sql,(sys.argv[2],))] for name,sql in queries.items()}))
"#;
    let output = Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg("-c")
        .arg(script)
        .arg(path)
        .arg(namespace)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    database: PathBuf,
    store: Store,
    sources: Vec<String>,
    blobs: Vec<String>,
    import_result: String,
    authority: StoredTraceAuthority,
}

async fn fixture(outcome: TraceOutcome, empty_excerpt: bool) -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    // The real restore script rejects symlinks in its path, including /var on macOS.
    let root = directory.path().canonicalize().unwrap();
    let database = root.join("native-cleanup.sqlite3");
    let store = Store::open(&database).await.unwrap();
    let admin = context(NS, Role::Admin);
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&admin, &hash(b"native-cleanup-initial"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    let mut specs = Vec::new();
    for (label, marker) in [
        ("primary", "IMPORTED-PRIMARY-PRIVATE"),
        ("secondary", "IMPORTED-SECONDARY-PRIVATE"),
    ] {
        let path = root.join(format!("{label}.jsonl"));
        let bytes = serde_json::to_vec(&json!({"role":"user","content":marker})).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        specs.push(ImportSourceSpec {
            source_id: format!("source-{label}"),
            path: path.to_string_lossy().into_owned(),
            reader: SourceFormat::ClaudeFixture,
            expected_digest: hash(&bytes),
        });
    }
    let service = PersistentImportService::new(store.clone());
    let selection = service
        .register(
            &admin,
            ImportRegistrationRequest {
                schema_version: IMPORT_REGISTRATION_SCHEMA.into(),
                request_key: "two-imports".into(),
                roots: vec![root.to_string_lossy().into_owned()],
                purpose: Purpose::Development,
                allow_model_excerpts: false,
                outbound_authorized: false,
                retention_scope: ImportRetentionScope::LocalPrivate,
                sources: specs,
            },
        )
        .await
        .unwrap();
    let result = service.execute(&admin, &selection.id).await.unwrap();
    assert_eq!(result.payload.aggregate_summary.total_sources, 2);
    assert_eq!(result.payload.aggregate_summary.total_events, 2);
    let summary = read_persisted_imported_evidence(&admin, &store, &result.id)
        .await
        .unwrap();
    assert_eq!(summary.task_origin, TaskOrigin::ImportedHistory);
    assert_eq!(
        summary.execution_attestation,
        ExecutionAttestation::UnverifiedImport
    );
    assert!(!summary.formal_evaluation_eligible);
    let native = authority(RUN, "shared", outcome, empty_excerpt);
    native.validate().unwrap();
    store_trace_authority(&store, &context(NS, Role::Host), &native)
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let mut blobs = Vec::new();
    for source in &selection.payload.import_source_ids {
        let value: Value = session.need(&admin, "artifact", source).await.unwrap();
        blobs.push(
            value["payload"]["raw_blob_digest"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
        // Explicit test fixture edge; this is not a production import-to-native writer.
        session
            .put_edge(&admin, "run", RUN, "artifact", source)
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    Fixture {
        _directory: directory,
        root,
        database,
        store,
        sources: selection.payload.import_source_ids,
        blobs,
        import_result: result.id,
        authority: native,
    }
}

async fn reopen(fixture: &mut Fixture) {
    fixture.store.close().await;
    fixture.store = Store::open(&fixture.database).await.unwrap();
}

async fn begin_secondary(fixture: &Fixture) -> CleanupStatus {
    LifecycleStore::begin_revoke(
        &context(NS, Role::Admin),
        &fixture.store,
        TypedObjectRef {
            kind: "artifact".into(),
            id: fixture.sources[1].clone(),
        },
        "revoke non-primary imported contributor",
        10,
    )
    .await
    .unwrap()
}

async fn resume_one_node_pages(fixture: &mut Fixture, mut status: CleanupStatus) -> CleanupStatus {
    for now in 11..180 {
        if matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
            break;
        }
        status = LifecycleStore::cleanup_step(
            &context(NS, Role::Admin),
            &fixture.store,
            &status.job_id,
            1,
            now,
        )
        .await
        .unwrap();
        println!(
            "page_limit=1 state={:?} processed={} pending={}",
            status.state, status.processed_nodes, status.pending_nodes
        );
        reopen(fixture).await;
        let reread = LifecycleStore::cleanup_status(
            &context(NS, Role::Admin),
            &fixture.store,
            &status.job_id,
        )
        .await
        .unwrap();
        assert_eq!(reread.state, status.state);
        assert_eq!(reread.processed_nodes, status.processed_nodes);
        assert_eq!(reread.pending_nodes, status.pending_nodes);
    }
    status
}

async fn run_value(store: &Store, namespace: &str, id: &str) -> Option<Value> {
    let mut session = store.session().await.unwrap();
    let value = session
        .get(&context(namespace, Role::Admin), "run", id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    value
}

fn assert_cache_redacted(snapshot: &Value, id: &str) {
    let entry = snapshot["cache"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["subject_id"] == id && row["operation"] == "optimization.source")
        .unwrap();
    assert_eq!(entry["redacted"], 1);
    assert_eq!(entry["response"], "null");
}

async fn assert_redacted(fixture: &Fixture, status: &CleanupStatus, original_body: &str) {
    let value = run_value(&fixture.store, NS, RUN).await.unwrap();
    println!(
        "cleanup_status={} stored_authority={}",
        serde_json::to_string(status).unwrap(),
        value
    );
    assert_eq!(
        status.state,
        CleanupState::Complete,
        "{status:?}; residual={value}"
    );
    assert_eq!(value["schema_version"], "rsia.redacted.v1");
    assert_eq!(value["id"], RUN);
    assert_eq!(value["original_kind"], "run");
    assert_eq!(value["original_schema"], NATIVE_SCHEMA);
    assert_eq!(value["original_digest"], hash(original_body.as_bytes()));
    for field in [
        "record",
        "trace",
        "body",
        "excerpt",
        "diagnosis",
        "excerpt_start",
        "excerpt_end",
    ] {
        assert!(
            value.get(field).is_none(),
            "sensitive field retained: {field}"
        );
    }
    let snapshot = sqlite_snapshot(&fixture.database, NS);
    assert_cache_redacted(&snapshot, RUN);
    assert!(
        snapshot["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["node_id"] == RUN && row["event_kind"] == "content_redacted")
    );
    let mut session = fixture.store.session().await.unwrap();
    let host = context(NS, Role::Host);
    assert!(
        matches!(load_stored_source(&mut session, &host, RUN).await, Err(Error::Invalid(ref message)) if message == "unsupported or unverified legacy run authority")
    );
    assert!(matches!(
        session
            .cached::<StoredTraceAuthority, _>(
                &host,
                "optimization.source",
                RUN,
                &fixture.authority
            )
            .await,
        Err(Error::Conflict(_))
    ));
    session.commit().await.unwrap();
}

async fn outcome_case(outcome: TraceOutcome, empty_excerpt: bool) {
    let mut fixture = fixture(outcome, empty_excerpt).await;
    let before = sqlite_snapshot(&fixture.database, NS);
    let original_body = before["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["kind"] == "run" && row["id"] == RUN)
        .unwrap()["body"]
        .as_str()
        .unwrap()
        .to_owned();
    let status = begin_secondary(&fixture).await;
    let status = resume_one_node_pages(&mut fixture, status).await;
    assert_redacted(&fixture, &status, &original_body).await;
}

#[tokio::test]
async fn success_authority_is_redacted() {
    outcome_case(TraceOutcome::Success, false).await;
}
#[tokio::test]
async fn task_failure_authority_is_redacted() {
    outcome_case(TraceOutcome::TaskFailure, false).await;
}
#[tokio::test]
async fn environment_failure_authority_is_redacted() {
    outcome_case(TraceOutcome::EnvironmentFailure, false).await;
}
#[tokio::test]
async fn invalid_outcome_authority_is_redacted() {
    outcome_case(TraceOutcome::Invalid, false).await;
}
#[tokio::test]
async fn incomplete_authority_with_empty_nonzero_excerpt_is_redacted() {
    outcome_case(TraceOutcome::Incomplete, true).await;
}

#[tokio::test]
async fn non_primary_import_closure_resumes_preserves_identity_and_never_resurrects() {
    let mut fixture = fixture(TraceOutcome::TaskFailure, false).await;
    let unrelated = authority(
        "unrelated-native",
        "unrelated",
        TraceOutcome::Success,
        false,
    );
    let other = authority(RUN, "other-namespace", TraceOutcome::Success, false);
    store_trace_authority(&fixture.store, &context(NS, Role::Host), &unrelated)
        .await
        .unwrap();
    store_trace_authority(&fixture.store, &context("other-tenant", Role::Host), &other)
        .await
        .unwrap();
    let before = sqlite_snapshot(&fixture.database, NS);
    let other_before = sqlite_snapshot(&fixture.database, "other-tenant");
    let original_body = before["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["kind"] == "run" && row["id"] == RUN)
        .unwrap()["body"]
        .as_str()
        .unwrap()
        .to_owned();
    let status = begin_secondary(&fixture).await;
    assert_eq!(status.state, CleanupState::Pending);
    let duplicate = begin_secondary(&fixture).await;
    assert_eq!(duplicate.job_id, status.job_id);
    assert_eq!(duplicate.watermark_seq, status.watermark_seq);
    let status = resume_one_node_pages(&mut fixture, status).await;
    assert_redacted(&fixture, &status, &original_body).await;
    let after = sqlite_snapshot(&fixture.database, NS);
    assert_eq!(after["edges"], before["edges"]);
    let old_audit = before["audit"].as_array().unwrap();
    assert_eq!(
        &after["audit"].as_array().unwrap()[..old_audit.len()],
        old_audit
    );
    assert!(
        fixture
            .store
            .verify_audit(&context(NS, Role::Admin))
            .await
            .unwrap()
            >= old_audit.len()
    );
    assert_eq!(
        sqlite_snapshot(&fixture.database, "other-tenant"),
        other_before
    );
    let mut session = fixture.store.session().await.unwrap();
    let live = load_stored_source(&mut session, &context(NS, Role::Host), &unrelated.record.id)
        .await
        .unwrap();
    assert_eq!(
        fingerprint(&live).unwrap(),
        fingerprint(&unrelated).unwrap()
    );
    let live_other = load_stored_source(&mut session, &context("other-tenant", Role::Host), RUN)
        .await
        .unwrap();
    assert_eq!(
        fingerprint(&live_other).unwrap(),
        fingerprint(&other).unwrap()
    );
    session.commit().await.unwrap();
    assert!(
        PersistentImportService::new(fixture.store.clone())
            .load_live_result(&context(NS, Role::Admin), &fixture.import_result)
            .await
            .is_err()
    );
    assert!(
        fixture
            .store
            .read_blob(&context(NS, Role::Admin), &fixture.blobs[1], 1024)
            .await
            .is_err()
    );
    let serialized_after = serde_json::to_string(&after).unwrap();
    assert!(!serialized_after.contains("IMPORTED-SECONDARY-PRIVATE"));
    assert!(!serialized_after.contains("EXCERPT-shared-汉字"));
    assert!(!serialized_after.contains("DIAGNOSIS-shared-private"));
    for now in 200..203 {
        reopen(&mut fixture).await;
        let repeated = LifecycleStore::cleanup_step(
            &context(NS, Role::Admin),
            &fixture.store,
            &status.job_id,
            1,
            now,
        )
        .await
        .unwrap();
        assert_eq!(repeated.state, CleanupState::Complete);
        for authority in [
            &fixture.authority,
            &authority(RUN, "changed", TraceOutcome::Invalid, true),
        ] {
            assert!(
                store_trace_authority(&fixture.store, &context(NS, Role::Host), authority)
                    .await
                    .is_err()
            );
        }
        assert_eq!(sqlite_snapshot(&fixture.database, NS), after);
    }
}

#[tokio::test]
async fn directly_revoked_native_run_keeps_source_deletion_rule() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let path = root.join("direct.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let native = authority(RUN, "direct", TraceOutcome::Success, false);
    store_trace_authority(&store, &context(NS, Role::Host), &native)
        .await
        .unwrap();
    let mut status = LifecycleStore::begin_revoke(
        &context(NS, Role::Admin),
        &store,
        TypedObjectRef {
            kind: "run".into(),
            id: RUN.into(),
        },
        "direct native source",
        10,
    )
    .await
    .unwrap();
    for now in 11..60 {
        if matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
            break;
        }
        status =
            LifecycleStore::cleanup_step(&context(NS, Role::Admin), &store, &status.job_id, 1, now)
                .await
                .unwrap();
    }
    assert_eq!(status.state, CleanupState::Complete);
    assert!(run_value(&store, NS, RUN).await.is_none());
    assert_cache_redacted(&sqlite_snapshot(&path, NS), RUN);
    assert!(matches!(
        store_trace_authority(&store, &context(NS, Role::Host), &native).await,
        Err(Error::Forbidden)
    ));
}

async fn replace_object(fixture: &Fixture, kind: &str, value: &Value) {
    let admin = context(NS, Role::Admin);
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(&admin, kind, RUN, admin.actor(), value)
        .await
        .unwrap();
    if kind != "run" {
        session.delete(&admin, "run", RUN).await.unwrap();
    }
    for source in &fixture.sources {
        session
            .put_edge(&admin, kind, RUN, "artifact", source)
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
}

fn malformed_cases() -> Vec<(&'static str, &'static str, Value)> {
    let original =
        serde_json::to_value(authority(RUN, "malformed", TraceOutcome::Success, false)).unwrap();
    let mut cases = vec![
        (
            "old-small-unknown",
            "run",
            json!({"id":RUN,"schema_version":NATIVE_SCHEMA,"body":"old source"}),
        ),
        ("wrong-kind", "artifact", original.clone()),
    ];
    let mutations: Vec<(&str, &str, Value)> = vec![
        (
            "near-schema",
            "/schema_version",
            "rsia.optimization.source.v1-near".into(),
        ),
        ("unknown-outer", "/unknown", true.into()),
        ("unknown-record", "/record/unknown", true.into()),
        ("unknown-trace", "/trace/unknown", true.into()),
        ("unknown-diagnosis", "/trace/diagnosis/unknown", true.into()),
        (
            "unknown-origin",
            "/record/task_origin",
            "future_origin".into(),
        ),
        ("unknown-outcome", "/trace/outcome", "future_outcome".into()),
        (
            "unknown-diagnosis-kind",
            "/trace/diagnosis/kind",
            "future_kind".into(),
        ),
        ("record-id", "/record/id", "wrong-id".into()),
        ("trace-id", "/trace/run_id", "wrong-id".into()),
        (
            "record-family",
            "/record/parent_family",
            "wrong-family".into(),
        ),
        (
            "trace-family",
            "/trace/parent_family",
            "wrong-family".into(),
        ),
        ("record-purpose", "/record/purpose", "inspection".into()),
        ("trace-purpose", "/trace/purpose", "inspection".into()),
        (
            "unverified-origin",
            "/record/task_origin",
            "imported_history".into(),
        ),
        (
            "unverified-attestation",
            "/record/execution_attestation",
            "unverified_import".into(),
        ),
        (
            "body-hash",
            "/trace/source_digest",
            hash(b"different body").into(),
        ),
        ("wrong-body-type", "/record/body", "not bytes".into()),
        ("excerpt-out-of-range", "/excerpt_end", 100000.into()),
        ("excerpt-mismatch", "/trace/excerpt", "wrong excerpt".into()),
        ("negative-start", "/excerpt_start", (-1).into()),
    ];
    for (label, pointer, replacement) in mutations {
        let mut value = original.clone();
        if label == "unknown-outer" {
            value["unknown"] = replacement;
        } else if label == "unknown-record" {
            value["record"]["unknown"] = replacement;
        } else if label == "unknown-trace" {
            value["trace"]["unknown"] = replacement;
        } else if label == "unknown-diagnosis" {
            value["trace"]["diagnosis"]["unknown"] = replacement;
        } else {
            *value.pointer_mut(pointer).unwrap() = replacement;
        }
        cases.push((label, "run", value));
    }
    let mut node_mismatch = original.clone();
    node_mismatch["record"]["id"] = "another-valid-id".into();
    node_mismatch["trace"]["run_id"] = "another-valid-id".into();
    cases.push(("bound-node-id", "run", node_mismatch));
    let mut invalid_family = original.clone();
    invalid_family["record"]["parent_family"] = "invalid family".into();
    invalid_family["trace"]["parent_family"] = "invalid family".into();
    cases.push(("invalid-family", "run", invalid_family));
    let mut invalid_id = original.clone();
    invalid_id["record"]["id"] = "invalid id".into();
    invalid_id["trace"]["run_id"] = "invalid id".into();
    cases.push(("invalid-id", "run", invalid_id));
    let mut both_inspection = original;
    both_inspection["record"]["purpose"] = "inspection".into();
    both_inspection["trace"]["purpose"] = "inspection".into();
    cases.push(("both-nondevelopment", "run", both_inspection));
    cases
}

#[tokio::test]
async fn malformed_authorities_remain_blocked_without_touching_unrelated_objects() {
    let mut violations = Vec::new();
    for (label, kind, value) in malformed_cases() {
        let mut fixture = fixture(TraceOutcome::Success, false).await;
        let unrelated = authority(
            "unrelated-native",
            "unrelated",
            TraceOutcome::Success,
            false,
        );
        store_trace_authority(&fixture.store, &context(NS, Role::Host), &unrelated)
            .await
            .unwrap();
        let unrelated_before = run_value(&fixture.store, NS, &unrelated.record.id)
            .await
            .unwrap();
        // Negative fixture corruption is not written by the immutable Host API.
        replace_object(&fixture, kind, &value).await;
        let status = begin_secondary(&fixture).await;
        let status = resume_one_node_pages(&mut fixture, status).await;
        let mut session = fixture.store.session().await.unwrap();
        let stored: Value = session
            .need(&context(NS, Role::Admin), kind, RUN)
            .await
            .unwrap();
        session.commit().await.unwrap();
        println!(
            "malformed_case={label} status={} original={} stored={}",
            serde_json::to_string(&status).unwrap(),
            value,
            stored
        );
        if status.state != CleanupState::Failed
            || !status
                .last_error
                .as_deref()
                .is_some_and(|error| error.contains("blocked_unknown_scope"))
            || stored != value
        {
            violations.push(label);
        }
        assert_eq!(
            run_value(&fixture.store, NS, &unrelated.record.id)
                .await
                .unwrap(),
            unrelated_before
        );
    }
    assert!(
        violations.is_empty(),
        "malformed cleanup cases incorrectly accepted: {violations:?}"
    );
}

#[tokio::test]
async fn failed_job_requires_admin_retry_and_legal_body_before_full_frontier_cleanup() {
    let mut fixture = fixture(TraceOutcome::Success, false).await;
    replace_object(
        &fixture,
        "run",
        &json!({"id":RUN,"schema_version":NATIVE_SCHEMA,"body":"old unknown shape"}),
    )
    .await;
    let status = begin_secondary(&fixture).await;
    let failed = resume_one_node_pages(&mut fixture, status).await;
    assert_eq!(failed.state, CleanupState::Failed);
    let before = sqlite_snapshot(&fixture.database, NS);
    assert!(
        before["frontier"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["node_id"] == RUN && node["expanded"] == 1)
    );
    // Worker does not initiate a retry, and an explicit Admin retry still fails
    // when the authority is malformed. Neither call grants the old shape trust.
    let worker = LifecycleStore::cleanup_step(
        &context(NS, Role::Worker),
        &fixture.store,
        &failed.job_id,
        1,
        200,
    )
    .await
    .unwrap();
    assert_eq!(worker.state, CleanupState::Failed);
    let started = LifecycleStore::cleanup_step(
        &context(NS, Role::Admin),
        &fixture.store,
        &failed.job_id,
        1,
        201,
    )
    .await
    .unwrap();
    let malformed = resume_one_node_pages(&mut fixture, started).await;
    assert_eq!(malformed.state, CleanupState::Failed);
    // Model an old Failed job whose content shape is now recognized. Repair only
    // the test object; recovery uses the public Admin API, with no fixture writes
    // to job state or the persisted frontier.
    let legal = serde_json::to_value(&fixture.authority).unwrap();
    replace_object(&fixture, "run", &legal).await;
    reopen(&mut fixture).await;
    let worker = LifecycleStore::cleanup_step(
        &context(NS, Role::Worker),
        &fixture.store,
        &failed.job_id,
        1,
        300,
    )
    .await
    .unwrap();
    assert_eq!(worker.state, CleanupState::Failed);
    assert_eq!(run_value(&fixture.store, NS, RUN).await.unwrap(), legal);
    let started = LifecycleStore::cleanup_step(
        &context(NS, Role::Admin),
        &fixture.store,
        &failed.job_id,
        1,
        301,
    )
    .await
    .unwrap();
    let complete = resume_one_node_pages(&mut fixture, started).await;
    assert_redacted(&fixture, &complete, &legal.to_string()).await;
    assert!(complete.last_error.is_none());
    assert_eq!(begin_secondary(&fixture).await.job_id, failed.job_id);
}

#[tokio::test]
async fn complete_backup_with_authority_and_edges_restores_pending_and_continues_cleanup() {
    let mut fixture = fixture(TraceOutcome::TaskFailure, false).await;
    let before = sqlite_snapshot(&fixture.database, NS);
    let original_body = before["objects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["kind"] == "run" && row["id"] == RUN)
        .unwrap()["body"]
        .as_str()
        .unwrap()
        .to_owned();
    let backup = fixture.root.join("backup");
    fixture.store.backup(&backup).await.unwrap();
    let backup_snapshot = sqlite_snapshot(&backup.join("rsia.sqlite3"), NS);
    assert_eq!(backup_snapshot["objects"], before["objects"]);
    assert_eq!(backup_snapshot["edges"], before["edges"]);
    let manifest_bytes = std::fs::read(backup.join("backup-manifest.json")).unwrap();
    let manifest: BackupManifest = serde_json::from_slice(&manifest_bytes).unwrap();
    let base = &manifest.watermarks[0];
    let status = begin_secondary(&fixture).await;
    let mut session = fixture.store.session().await.unwrap();
    let tombstone: RevokeTombstone = session
        .need(&context(NS, Role::Admin), "tombstone", &fixture.sources[1])
        .await
        .unwrap();
    session.commit().await.unwrap();
    let delta_path = fixture.root.join("revoke-delta.json");
    std::fs::write(&delta_path, serde_json::to_vec(&json!({
        "schema_version":"rsia.revoke_delta.v1", "base_manifest_sha256":hash(&manifest_bytes),
        "namespaces":[{"namespace":NS,"base_seq":base.seq,"base_digest":base.digest,
            "events":[{"seq":tombstone.watermark_seq,"previous_digest":base.digest,"digest":tombstone.watermark_digest,"source_kind":"artifact","source_id":fixture.sources[1],"tombstone_digest":hash(&serde_json::to_vec(&tombstone).unwrap()),"reason":tombstone.reason,"created_at":tombstone.created_at}],
            "latest_seq":tombstone.watermark_seq,"latest_digest":tombstone.watermark_digest}]
    })).unwrap()).unwrap();
    fixture.store.close().await;
    let destination = fixture.root.join("restored");
    let output = Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/restore_backup.py"
        ))
        .arg("--backup")
        .arg(&backup)
        .arg("--dest")
        .arg(&destination)
        .arg("--revoke-delta")
        .arg(&delta_path)
        .arg("--trusted-revocations-db")
        .arg(&fixture.database)
        .output()
        .unwrap();
    println!(
        "restore_exit={:?} stdout={} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success());
    fixture.database = destination.join("rsia.sqlite3");
    fixture.store = Store::open(&fixture.database).await.unwrap();
    let pending =
        LifecycleStore::cleanup_status(&context(NS, Role::Admin), &fixture.store, &status.job_id)
            .await
            .unwrap();
    assert_eq!(pending.state, CleanupState::Pending);
    let restored = sqlite_snapshot(&fixture.database, NS);
    assert_eq!(restored["edges"], before["edges"]);
    assert_eq!(
        run_value(&fixture.store, NS, RUN).await.unwrap(),
        serde_json::to_value(&fixture.authority).unwrap()
    );
    let status = resume_one_node_pages(&mut fixture, pending).await;
    assert_redacted(&fixture, &status, &original_body).await;
}
