//! Explicit copied-store fault fixtures. Old read success is not evidence that
//! direct dependency registration is complete. No production rows are edited.

use evo_core::evidence::Purpose;
use evo_core::{Context, Error, Role, fingerprint, hash};
use evo_engine::dependency_integrity::{
    E16EdgeInspection, InspectionBudget, InspectionStatus, IssueCode, ObjectScope,
    inspect_e16_content_edges,
};
use evo_engine::import::{
    IMPORT_REGISTRATION_SCHEMA, ImportRegistrationRequest, ImportResultRecord, ImportResultState,
    ImportRetentionScope, ImportSourceReadStatus, ImportSourceRecord, ImportSourceSpec,
    PersistentImportService, SourceFormat, SourceSelectionRecord,
};
use evo_storage::Store;
use evo_storage::dependency_read::DirectEdge;
use evo_storage::lifecycle::{CleanupState, LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture {
    _directory: tempfile::TempDir,
    store: Store,
    ctx: Context,
    selection: SourceSelectionRecord,
    sources: Vec<ImportSourceRecord>,
    result: ImportResultRecord,
    database: PathBuf,
}

async fn fixture() -> Fixture {
    fixture_with(SourceFormat::ClaudeFixture, 0).await
}

async fn fixture_with(reader: SourceFormat, missing: usize) -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let database = root.join("writer.sqlite3");
    let store = Store::open(&database).await.unwrap();
    let ctx = Context::new("edge-inspection", "admin", Role::Admin).unwrap();
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&ctx, &hash(b"initial"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    let mut specs = Vec::new();
    for (index, (id, content)) in [
        ("primary", "PRIMARY-PRIVATE observation"),
        (
            "secondary",
            "SECONDARY-PRIVATE tool_result fail counter_example",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let path = root.join(format!("{id}.jsonl"));
        let bytes = match reader {
            SourceFormat::ClaudeFixture => serde_json::to_vec(&json!({"format":"claude.fixture","role":"user","content":content})),
            SourceFormat::RsihPiFixture => serde_json::to_vec(&json!({"format":"rsih.pi.fixture","prompts":[{"role":"user","content":content}]})),
            SourceFormat::RsiaTraceV1 => serde_json::to_vec(&json!({"schema_version":"rsia.trace.v1","events":[{"role":"user","kind":"task","content":content}]})),
        }.unwrap();
        if index < 2 - missing {
            std::fs::write(&path, &bytes).unwrap();
        }
        specs.push(ImportSourceSpec {
            source_id: id.into(),
            path: path.to_string_lossy().into_owned(),
            reader,
            expected_digest: hash(&bytes),
        });
    }
    let service = PersistentImportService::new(store.clone());
    let selection = service
        .register(
            &ctx,
            ImportRegistrationRequest {
                schema_version: IMPORT_REGISTRATION_SCHEMA.into(),
                request_key: "baseline-two-sources".into(),
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
    let result = service.execute(&ctx, &selection.id).await.unwrap();
    assert_eq!(result.payload.aggregate_summary.total_sources, 2 - missing);
    if missing == 0 {
        assert!(result.payload.aggregate_summary.failure_count > 0);
    }
    assert!(result.payload.locators.is_empty());
    let mut session = store.session().await.unwrap();
    let mut sources = Vec::new();
    for id in &selection.payload.import_source_ids {
        sources.push(session.need(&ctx, "artifact", id).await.unwrap());
    }
    session.commit().await.unwrap();
    Fixture {
        _directory: directory,
        store,
        ctx,
        selection,
        sources,
        result,
        database,
    }
}

#[derive(Clone, Copy)]
enum Fault {
    None,
    MissingResultSecondary,
    WrongBlobKind,
    MissingSecondaryObject,
}

async fn copied(f: &Fixture, fault: Fault) -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("copied.sqlite3"))
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&f.ctx, &hash(b"initial"))
        .await
        .unwrap();
    session
        .put(
            &f.ctx,
            "artifact",
            &f.selection.id,
            f.ctx.actor(),
            &f.selection,
        )
        .await
        .unwrap();
    session
        .put(&f.ctx, "artifact", &f.result.id, f.ctx.actor(), &f.result)
        .await
        .unwrap();
    session
        .put_edge(
            &f.ctx,
            "artifact",
            &f.result.id,
            "artifact",
            &f.selection.id,
        )
        .await
        .unwrap();
    for (index, source) in f.sources.iter().enumerate() {
        if !(matches!(fault, Fault::MissingSecondaryObject) && index == 1) {
            session
                .put(&f.ctx, "artifact", &source.id, f.ctx.actor(), source)
                .await
                .unwrap();
        }
        session
            .put_edge(&f.ctx, "artifact", &f.selection.id, "artifact", &source.id)
            .await
            .unwrap();
        if !(matches!(fault, Fault::MissingResultSecondary) && index == 1) {
            session
                .put_edge(&f.ctx, "artifact", &f.result.id, "artifact", &source.id)
                .await
                .unwrap();
        }
        let kind = if matches!(fault, Fault::WrongBlobKind) && index == 1 {
            "artifact"
        } else {
            "blob"
        };
        session
            .put_edge(
                &f.ctx,
                "artifact",
                &source.id,
                kind,
                &source.payload.raw_blob_digest,
            )
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    for source in &f.sources {
        let bytes = f
            .store
            .read_blob(&f.ctx, &source.payload.raw_blob_digest, 64 * 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(
            store.put_blob(&f.ctx, &bytes).await.unwrap(),
            source.payload.raw_blob_digest
        );
    }
    (directory, store)
}

#[tokio::test]
async fn old_api_transitive_path_hides_missing_result_secondary_direct_edge() {
    let f = fixture().await;
    let (_directory, store) = copied(&f, Fault::MissingResultSecondary).await;
    PersistentImportService::new(store.clone())
        .load_live_result(&f.ctx, &f.result.id)
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let closure = session
        .upstream_closure(&f.ctx, &[("artifact", f.result.id.as_str())], 64)
        .await
        .unwrap();
    assert!(
        closure
            .iter()
            .any(|node| node.kind == "artifact" && node.id == f.sources[1].id)
    );
    let dependents = session
        .dependents(&f.ctx, "artifact", &f.sources[1].id)
        .await
        .unwrap();
    assert!(dependents.contains(&("artifact".into(), f.selection.id.clone())));
    assert!(!dependents.contains(&("artifact".into(), f.result.id.clone())));
    session.commit().await.unwrap();
    println!(
        "baseline A: old load succeeds; secondary is transitively reachable; result direct edge absent"
    );
}

#[tokio::test]
async fn old_api_blob_read_hides_same_id_wrong_kind_dependency() {
    let f = fixture().await;
    let (_directory, store) = copied(&f, Fault::WrongBlobKind).await;
    let source = &f.sources[1];
    store
        .read_blob(&f.ctx, &source.payload.raw_blob_digest, 64 * 1024 * 1024)
        .await
        .unwrap();
    PersistentImportService::new(store.clone())
        .load_live_result(&f.ctx, &f.result.id)
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    let blob = session
        .dependents(&f.ctx, "blob", &source.payload.raw_blob_digest)
        .await
        .unwrap();
    let artifact = session
        .dependents(&f.ctx, "artifact", &source.payload.raw_blob_digest)
        .await
        .unwrap();
    assert!(!blob.contains(&("artifact".into(), source.id.clone())));
    assert!(artifact.contains(&("artifact".into(), source.id.clone())));
    session.commit().await.unwrap();
    println!(
        "baseline B: old load/blob read succeed; blob direct edge absent; artifact:same digest present"
    );
}

fn edge(kind: &str, id: &str) -> DirectEdge {
    DirectEdge {
        dst_kind: kind.into(),
        dst_id: id.into(),
    }
}

fn object<'a>(
    report: &'a E16EdgeInspection,
    id: &str,
) -> &'a evo_engine::dependency_integrity::InspectedObject {
    report
        .objects
        .iter()
        .find(|object| object.id == id)
        .unwrap()
}

fn has(report: &E16EdgeInspection, id: &str, code: IssueCode, target: Option<DirectEdge>) -> bool {
    object(report, id)
        .issues
        .iter()
        .any(|issue| issue.code == code && issue.target == target)
}

async fn inspect(ctx: &Context, store: &Store, id: &str) -> E16EdgeInspection {
    inspect_e16_content_edges(ctx, store, id, InspectionBudget::default())
        .await
        .unwrap()
}

async fn put<T: serde::Serialize>(ctx: &Context, store: &Store, id: &str, record: &T) {
    let mut session = store.session().await.unwrap();
    session
        .put(ctx, "artifact", id, ctx.actor(), record)
        .await
        .unwrap();
    session.commit().await.unwrap();
}

fn sqlite(path: &Path, script: &str) -> Value {
    let output = Command::new("python3")
        .arg("-c")
        .arg(script)
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn snapshot(path: &Path) -> Value {
    sqlite(
        path,
        "import json,sqlite3,sys\nc=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True)\nnames=sorted(r[0] for r in c.execute(\"SELECT name FROM sqlite_master WHERE type='table'\"))\ndef rows(n):\n data=[[{'blob':x.hex()} if isinstance(x,bytes) else x for x in r] for r in c.execute('SELECT * FROM '+chr(34)+n.replace(chr(34),chr(34)*2)+chr(34))]\n return sorted(data,key=lambda r:json.dumps(r,sort_keys=True))\nprint(json.dumps({n:rows(n) for n in names}))",
    )
}

#[tokio::test]
async fn three_real_readers_all_three_wires_are_direct_complete_and_read_only() {
    for reader in [
        SourceFormat::RsiaTraceV1,
        SourceFormat::RsihPiFixture,
        SourceFormat::ClaudeFixture,
    ] {
        let f = fixture_with(reader, 0).await;
        assert_eq!(f.result.payload.state, ImportResultState::Complete);
        let before = snapshot(&f.database);
        let report = inspect(&f.ctx, &f.store, &f.result.id).await;
        assert_eq!(report.status, InspectionStatus::Complete, "{report:#?}");
        assert_eq!(report.object_reads, 4);
        assert_eq!(report.edge_rows_read, 7);
        assert_eq!(report.physical_blob_check, "not_checked");
        assert_eq!(report.namespace_watermark.as_ref().unwrap().sequence, 1);
        assert_eq!(
            object(&report, &f.result.id).typed_digest,
            Some(fingerprint(&f.result).unwrap())
        );
        assert_eq!(object(&report, &f.result.id).declared_edges.len(), 3);
        assert!(
            object(&report, &f.result.id)
                .declared_edges
                .contains(&edge("artifact", &f.sources[1].id))
        );
        for source in &f.sources {
            assert_eq!(
                object(&report, &source.id).declared_edges,
                vec![edge("blob", &source.payload.raw_blob_digest)]
            );
            assert!(object(&report, &source.id).direct_edges_exhausted);
            let source_report = inspect(&f.ctx, &f.store, &source.id).await;
            assert_eq!(
                source_report.status,
                InspectionStatus::Complete,
                "{source_report:#?}"
            );
            assert_eq!(source_report.object_reads, 2);
            assert_eq!(
                object(&source_report, &f.selection.id).scope,
                ObjectScope::SelectionBacklinkAuthority
            );
            assert!(!object(&source_report, &f.selection.id).direct_edges_exhausted);
        }
        assert_eq!(
            inspect(&f.ctx, &f.store, &f.selection.id).await.status,
            InspectionStatus::Complete
        );
        assert_eq!(report, inspect(&f.ctx, &f.store, &f.result.id).await);
        let text = serde_json::to_string(&report).unwrap();
        for private in [
            "PRIMARY-PRIVATE",
            "SECONDARY-PRIVATE",
            f.ctx.actor(),
            f.sources[0].payload.authorized_path.as_str(),
        ] {
            assert!(!text.contains(private));
        }
        assert_eq!(snapshot(&f.database), before);
    }
}

#[tokio::test]
async fn copied_missing_secondary_edge_is_found_despite_transitive_reachability() {
    let f = fixture().await;
    let (directory, store) = copied(&f, Fault::MissingResultSecondary).await;
    let path = directory.path().join("copied.sqlite3");
    let before = snapshot(&path);
    let report = inspect(&f.ctx, &store, &f.result.id).await;
    assert_eq!(report.status, InspectionStatus::Anomalous);
    assert!(has(
        &report,
        &f.result.id,
        IssueCode::MissingDeclaredEdge,
        Some(edge("artifact", &f.sources[1].id))
    ));
    assert!(object(&report, &f.result.id).direct_edges_exhausted);
    assert_eq!(
        object(&report, &f.selection.id).status,
        InspectionStatus::Complete
    );
    assert_eq!(snapshot(&path), before);
}

#[tokio::test]
async fn copied_wrong_blob_kind_is_missing_plus_unexpected_without_file_read() {
    let f = fixture().await;
    let (_directory, store) = copied(&f, Fault::WrongBlobKind).await;
    let report = inspect(&f.ctx, &store, &f.result.id).await;
    let source = &f.sources[1];
    assert!(has(
        &report,
        &source.id,
        IssueCode::MissingDeclaredEdge,
        Some(edge("blob", &source.payload.raw_blob_digest))
    ));
    assert!(has(
        &report,
        &source.id,
        IssueCode::UnexpectedExistingEdge,
        Some(edge("artifact", &source.payload.raw_blob_digest))
    ));
    assert_eq!(report.status, InspectionStatus::Anomalous);
    assert_eq!(report.object_reads, 4); // blob is never a JSON authority point.
}

#[tokio::test]
async fn true_complete_partial_failed_results_keep_every_registered_source_edge() {
    for (missing, state) in [
        (0, ImportResultState::Complete),
        (1, ImportResultState::Partial),
        (2, ImportResultState::Failed),
    ] {
        let f = fixture_with(SourceFormat::ClaudeFixture, missing).await;
        assert_eq!(f.result.payload.state, state);
        assert_eq!(f.result.payload.evidence_set.is_none(), missing == 2);
        let report = inspect(&f.ctx, &f.store, &f.result.id).await;
        assert_eq!(report.status, InspectionStatus::Complete, "{report:#?}");
        assert_eq!(object(&report, &f.result.id).declared_edges.len(), 3);
        for source in &f.sources {
            assert!(
                object(&report, &f.result.id)
                    .declared_edges
                    .contains(&edge("artifact", &source.id))
            );
        }
    }
}

#[tokio::test]
async fn real_writer_unreadable_statuses_and_ready_do_not_probe_physical_blobs() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let store = Store::open(&root.join("statuses.sqlite3")).await.unwrap();
    let ctx = Context::new("statuses", "admin", Role::Admin).unwrap();
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&ctx, &hash(b"initial"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    let bytes = br#"{"role":"user","content":"PRIVATE-STATUS"}"#;
    let mut specs = Vec::new();
    let statuses = [
        ImportSourceReadStatus::Ready,
        ImportSourceReadStatus::Missing,
        ImportSourceReadStatus::PermissionDenied,
        ImportSourceReadStatus::SourceChanged,
        ImportSourceReadStatus::ZeroRecords,
        ImportSourceReadStatus::InvalidFile,
        ImportSourceReadStatus::TooLarge,
    ];
    for (index, status) in statuses.iter().enumerate() {
        let path = root.join(format!("source-{index}.jsonl"));
        let expected = match status {
            ImportSourceReadStatus::Missing => hash(bytes),
            ImportSourceReadStatus::ZeroRecords => {
                std::fs::write(&path, b"").unwrap();
                hash(b"")
            }
            ImportSourceReadStatus::InvalidFile => {
                std::fs::create_dir(&path).unwrap();
                hash(bytes)
            }
            ImportSourceReadStatus::TooLarge => {
                let file = std::fs::File::create(&path).unwrap();
                file.set_len(64 * 1024 * 1024 + 1).unwrap();
                hash(bytes)
            }
            _ => {
                std::fs::write(&path, bytes).unwrap();
                if *status == ImportSourceReadStatus::PermissionDenied {
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))
                        .unwrap();
                }
                if *status == ImportSourceReadStatus::SourceChanged {
                    hash(b"different")
                } else {
                    hash(bytes)
                }
            }
        };
        specs.push(ImportSourceSpec {
            source_id: format!("source-{index}"),
            path: path.to_string_lossy().into_owned(),
            reader: SourceFormat::ClaudeFixture,
            expected_digest: expected,
        });
    }
    let selection = PersistentImportService::new(store.clone())
        .register(
            &ctx,
            ImportRegistrationRequest {
                schema_version: IMPORT_REGISTRATION_SCHEMA.into(),
                request_key: "read-statuses".into(),
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
    let mut session = store.session().await.unwrap();
    let mut sources = Vec::new();
    for id in &selection.payload.import_source_ids {
        sources.push(
            session
                .need::<ImportSourceRecord>(&ctx, "artifact", id)
                .await
                .unwrap(),
        );
    }
    session.commit().await.unwrap();
    assert_eq!(
        sources
            .iter()
            .map(|source| source.payload.status)
            .collect::<Vec<_>>(),
        statuses
    );
    // Removing the Ready blob is explicit test fault injection. The inspector
    // still diagnoses declarations only, while old physical reading refuses it.
    let blob = root
        .join("blobs")
        .join(hash(ctx.namespace().as_bytes()))
        .join(&sources[0].payload.raw_blob_digest);
    std::fs::remove_file(blob).unwrap();
    assert!(
        store
            .read_blob(&ctx, &sources[0].payload.raw_blob_digest, 1024)
            .await
            .is_err()
    );
    let before = snapshot(&root.join("statuses.sqlite3"));
    let report = inspect(&ctx, &store, &selection.id).await;
    assert_eq!(report.status, InspectionStatus::Complete, "{report:#?}");
    assert_eq!(report.physical_blob_check, "not_checked");
    assert_eq!(report.object_reads, 8);
    assert_eq!(snapshot(&root.join("statuses.sqlite3")), before);
}

#[tokio::test]
async fn real_prepare_and_interrupted_finalize_are_complete_or_unknown_never_damage() {
    for stop_second in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let database = root.join("prepare.sqlite3");
        let store = Store::open(&database).await.unwrap();
        let ctx = Context::new("prepared", "admin", Role::Admin).unwrap();
        let mut session = store.session().await.unwrap();
        session
            .bump_watermark(&ctx, &hash(b"initial"))
            .await
            .unwrap();
        session.commit().await.unwrap();
        let mut specs = Vec::new();
        for id in ["primary", "secondary"] {
            let path = root.join(format!("{id}.jsonl"));
            let bytes = br#"{"role":"user","content":"PREPARE-PRIVATE"}"#;
            std::fs::write(&path, bytes).unwrap();
            specs.push(ImportSourceSpec {
                source_id: id.into(),
                path: path.to_string_lossy().into_owned(),
                reader: SourceFormat::ClaudeFixture,
                expected_digest: hash(bytes),
            });
        }
        let request = ImportRegistrationRequest {
            schema_version: IMPORT_REGISTRATION_SCHEMA.into(),
            request_key: "interrupted-register".into(),
            roots: vec![root.to_string_lossy().into_owned()],
            purpose: Purpose::Development,
            allow_model_excerpts: false,
            outbound_authorized: false,
            retention_scope: ImportRetentionScope::LocalPrivate,
            sources: specs,
        };
        let selection_id = format!("e16sel-{}", &fingerprint(&request).unwrap()[..24]);
        // A SQLite failure injection after atomic prepare stops the real writer
        // at its source-update boundary. No synthetic prepared records are put.
        let condition = if stop_second {
            " AND json_extract(OLD.body,'$.payload.logical_source_id')='secondary'"
        } else {
            ""
        };
        sqlite(
            &database,
            &format!(
                "import sqlite3,json,sys\nc=sqlite3.connect(sys.argv[1])\nc.execute(\"CREATE TRIGGER stop_finalize BEFORE UPDATE ON objects WHEN json_extract(OLD.body,'$.schema_version')='rsia.e16.import_source.v1'{condition} BEGIN SELECT RAISE(ABORT,'injected-finalize-stop'); END\")\nc.commit()\nprint('null')"
            ),
        );
        let error = PersistentImportService::new(store.clone())
            .register(&ctx, request)
            .await
            .unwrap_err();
        assert!(matches!(error, Error::Internal));
        let mut session = store.session().await.unwrap();
        let selection: SourceSelectionRecord =
            session.need(&ctx, "artifact", &selection_id).await.unwrap();
        let first: ImportSourceRecord = session
            .need(&ctx, "artifact", &selection.payload.import_source_ids[0])
            .await
            .unwrap();
        let second: ImportSourceRecord = session
            .need(&ctx, "artifact", &selection.payload.import_source_ids[1])
            .await
            .unwrap();
        session.commit().await.unwrap();
        assert_eq!(
            first.payload.status,
            if stop_second {
                ImportSourceReadStatus::Ready
            } else {
                ImportSourceReadStatus::Prepared
            }
        );
        assert_eq!(second.payload.status, ImportSourceReadStatus::Prepared);
        let before = snapshot(&database);
        let report = inspect(&ctx, &store, &selection_id).await;
        assert_eq!(
            report.status,
            if stop_second {
                InspectionStatus::Unknown
            } else {
                InspectionStatus::Complete
            },
            "{report:#?}"
        );
        assert!(
            report
                .objects
                .iter()
                .all(|object| object.direct_edges_exhausted)
        );
        assert!(
            !report
                .objects
                .iter()
                .flat_map(|object| &object.issues)
                .any(|issue| issue.class == evo_engine::dependency_integrity::IssueClass::Anomaly)
        );
        if stop_second {
            assert!(has(
                &report,
                &selection_id,
                IssueCode::PendingOrMismatchedAuthority,
                Some(edge("artifact", &first.id))
            ));
        }
        assert_eq!(snapshot(&database), before);
    }
}

#[tokio::test]
async fn actual_cleanup_redaction_is_history_only_and_deleted_blob_is_not_missing_authority() {
    let f = fixture().await;
    let status = LifecycleStore::begin_revoke(
        &f.ctx,
        &f.store,
        TypedObjectRef {
            kind: "artifact".into(),
            id: f.sources[1].id.clone(),
        },
        "revoke private source",
        evo_core::now(),
    )
    .await
    .unwrap();
    let mut finished = status;
    for _ in 0..100 {
        finished =
            LifecycleStore::cleanup_step(&f.ctx, &f.store, &finished.job_id, 1, evo_core::now())
                .await
                .unwrap();
        if matches!(
            finished.state,
            CleanupState::Complete | CleanupState::Failed
        ) {
            break;
        }
    }
    assert_eq!(finished.state, CleanupState::Complete);
    let mut session = f.store.session().await.unwrap();
    let redacted: Value = session
        .need(&f.ctx, "artifact", &f.result.id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(redacted["schema_version"], "rsia.redacted.v1");
    assert!(
        f.store
            .read_blob(&f.ctx, &f.sources[1].payload.raw_blob_digest, 4096)
            .await
            .is_err()
    );
    let before = snapshot(&f.database);
    let report = inspect(&f.ctx, &f.store, &f.result.id).await;
    assert!(
        matches!(
            report.status,
            InspectionStatus::HistoryOnly | InspectionStatus::Unknown
        ),
        "{report:#?}"
    );
    assert!(has(
        &report,
        &f.result.id,
        IssueCode::HistoricalDeclarationsOnly,
        None
    ));
    assert!(
        report
            .objects
            .iter()
            .all(|object| object.issues.iter().all(|issue| !matches!(
                issue.code,
                IssueCode::MissingAuthorityObject | IssueCode::MissingDeclaredEdge
            )))
    );
    assert_eq!(report.physical_blob_check, "not_checked");
    assert_eq!(snapshot(&f.database), before);
}

#[tokio::test]
async fn unknown_schema_and_legal_json_malformed_typed_body_are_distinct_and_no_content_leaks() {
    for (case, code, status) in [
        (0, IssueCode::UnknownSchema, InspectionStatus::Unknown),
        (1, IssueCode::MalformedRecord, InspectionStatus::Anomalous),
    ] {
        let f = fixture().await;
        let mut value = serde_json::to_value(&f.result).unwrap();
        if case == 0 {
            value["schema_version"] = json!("unrecognized-private-version");
        } else {
            value["updated_at"] = json!("PRIVATE-PARSER-CONTENT /private/path");
        }
        put(&f.ctx, &f.store, &f.result.id, &value).await;
        let before = snapshot(&f.database);
        let report = inspect(&f.ctx, &f.store, &f.result.id).await;
        assert_eq!(report.status, status);
        assert!(has(&report, &f.result.id, code, None));
        assert_eq!(report.object_reads, 1);
        assert_eq!(report.edge_rows_read, 0);
        let text = serde_json::to_string(&report).unwrap();
        assert!(
            !text.contains("PRIVATE")
                && !text.contains("/private/path")
                && !text.contains("unrecognized-private-version")
        );
        assert_eq!(snapshot(&f.database), before);
    }
}

#[tokio::test]
async fn absent_authority_is_separate_from_declared_edge_absence() {
    let f = fixture().await;
    let (_directory, store) = copied(&f, Fault::MissingSecondaryObject).await;
    let report = inspect(&f.ctx, &store, &f.result.id).await;
    assert!(has(
        &report,
        &f.result.id,
        IssueCode::MissingAuthorityObject,
        Some(edge("artifact", &f.sources[1].id))
    ));
    assert!(!has(
        &report,
        &f.result.id,
        IssueCode::MissingDeclaredEdge,
        Some(edge("artifact", &f.sources[1].id))
    ));
    assert_eq!(report.status, InspectionStatus::Anomalous);
}

#[tokio::test]
async fn duplicate_conflicting_order_and_input_digest_never_guess_a_missing_edge() {
    for case in 0..4 {
        let f = fixture().await;
        let mut result = f.result.clone();
        match case {
            0 => result.source_refs.push(result.source_refs[1].clone()),
            1 => result.source_refs.swap(1, 2),
            2 => result.input_digest = hash(b"wrong-input"),
            _ => result.source_refs[1].kind = "artifact".into(),
        }
        if case != 2 {
            result.input_digest = fingerprint(&result.source_refs).unwrap();
        }
        put(&f.ctx, &f.store, &result.id, &result).await;
        let report = inspect(&f.ctx, &f.store, &result.id).await;
        assert_eq!(
            report.status,
            InspectionStatus::Anomalous,
            "case {case}: {report:#?}"
        );
        assert!(has(
            &report,
            &result.id,
            if case == 2 {
                IssueCode::InputDigestMismatch
            } else {
                IssueCode::ConflictingReferences
            },
            None
        ));
        assert!(!object(&report, &result.id).declarations_checked);
        assert!(!object(&report, &result.id).direct_edges_exhausted);
        assert!(
            !object(&report, &result.id)
                .issues
                .iter()
                .any(|issue| issue.code == IssueCode::MissingDeclaredEdge)
        );
    }
}

#[tokio::test]
async fn typed_authority_digest_mismatch_is_immutable_for_result_but_pending_for_selection() {
    let f = fixture().await;
    let mut changed = f.sources[1].clone();
    changed.updated_at += 1;
    put(&f.ctx, &f.store, &changed.id, &changed).await;
    let selection = inspect(&f.ctx, &f.store, &f.selection.id).await;
    assert_eq!(selection.status, InspectionStatus::Unknown);
    assert!(has(
        &selection,
        &f.selection.id,
        IssueCode::PendingOrMismatchedAuthority,
        Some(edge("artifact", &changed.id))
    ));
    let result = inspect(&f.ctx, &f.store, &f.result.id).await;
    assert_eq!(result.status, InspectionStatus::Anomalous);
    assert!(has(
        &result,
        &f.result.id,
        IssueCode::ImmutableAuthorityDigestMismatch,
        Some(edge("artifact", &changed.id))
    ));
}

#[tokio::test]
async fn roles_envelope_owner_namespace_and_foreign_authorities_refuse_read_only() {
    let f = fixture().await;
    let before = snapshot(&f.database);
    for role in [Role::Agent, Role::Host, Role::Evaluator, Role::Worker] {
        let context = Context::new(f.ctx.namespace(), f.ctx.actor(), role).unwrap();
        assert!(matches!(
            inspect_e16_content_edges(
                &context,
                &f.store,
                &f.result.id,
                InspectionBudget::default()
            )
            .await,
            Err(Error::Forbidden)
        ));
    }
    let other = Context::new(f.ctx.namespace(), "other-owner", Role::Admin).unwrap();
    assert!(matches!(
        inspect_e16_content_edges(&other, &f.store, &f.result.id, InspectionBudget::default())
            .await,
        Err(Error::Forbidden)
    ));
    assert_eq!(snapshot(&f.database), before);
    for case in 0..2 {
        let f = fixture().await;
        let mut source = f.sources[1].clone();
        if case == 0 {
            source.owner_actor = "foreign-private-actor".into();
        } else {
            source.namespace = "foreign-private-namespace".into();
        }
        put(&f.ctx, &f.store, &source.id, &source).await;
        let before = snapshot(&f.database);
        assert!(matches!(
            inspect_e16_content_edges(&f.ctx, &f.store, &f.result.id, InspectionBudget::default())
                .await,
            Err(Error::Forbidden)
        ));
        assert_eq!(snapshot(&f.database), before);
    }
}

#[tokio::test]
async fn current_watermark_is_snapshot_diagnostic_and_never_repairs_old_records() {
    let f = fixture().await;
    let mut session = f.store.session().await.unwrap();
    session
        .bump_watermark(&f.ctx, &hash(b"revoked"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    let before = snapshot(&f.database);
    let report = inspect(&f.ctx, &f.store, &f.result.id).await;
    assert_eq!(report.status, InspectionStatus::Unknown);
    assert_eq!(report.namespace_watermark.unwrap().sequence, 2);
    assert!(report.objects.iter().all(|object| {
        object
            .issues
            .iter()
            .any(|issue| issue.code == IssueCode::WatermarkChanged)
    }));
    assert_eq!(snapshot(&f.database), before);
}

#[tokio::test]
async fn tiny_object_and_edge_budgets_remain_partial_and_do_not_invent_missing_edges() {
    let f = fixture().await;
    for budget in [
        InspectionBudget {
            max_objects: 0,
            max_edge_rows: 0,
            page_size: 1,
        },
        InspectionBudget {
            max_objects: 1,
            max_edge_rows: 0,
            page_size: 1,
        },
        InspectionBudget {
            max_objects: 2,
            max_edge_rows: 2,
            page_size: 1,
        },
        InspectionBudget {
            max_objects: 202,
            max_edge_rows: 1,
            page_size: 1,
        },
        InspectionBudget {
            max_objects: 202,
            max_edge_rows: 3,
            page_size: 1,
        },
    ] {
        let report = inspect_e16_content_edges(&f.ctx, &f.store, &f.result.id, budget)
            .await
            .unwrap();
        assert_eq!(report.status, InspectionStatus::Partial, "{report:#?}");
        assert!(
            report.object_reads <= budget.max_objects
                && report.edge_rows_read <= budget.max_edge_rows
        );
        for object in &report.objects {
            if !object.direct_edges_exhausted {
                assert!(
                    object
                        .issues
                        .iter()
                        .all(|issue| issue.code != IssueCode::MissingDeclaredEdge)
                );
            }
        }
    }
    for budget in [
        InspectionBudget {
            max_objects: 203,
            ..InspectionBudget::default()
        },
        InspectionBudget {
            max_edge_rows: 10001,
            ..InspectionBudget::default()
        },
        InspectionBudget {
            page_size: 0,
            ..InspectionBudget::default()
        },
        InspectionBudget {
            page_size: 257,
            ..InspectionBudget::default()
        },
        InspectionBudget {
            max_objects: usize::MAX,
            ..InspectionBudget::default()
        },
    ] {
        assert!(matches!(
            inspect_e16_content_edges(&f.ctx, &f.store, &f.result.id, budget).await,
            Err(Error::Invalid(_))
        ));
    }
}

#[tokio::test]
async fn many_extra_edges_use_sql_pages_keep_local_observation_with_partial_and_no_target_expansion()
 {
    let f = fixture().await;
    let mut session = f.store.session().await.unwrap();
    for i in 0..700 {
        session
            .put_edge(
                &f.ctx,
                "artifact",
                &f.result.id,
                "diagnostic",
                &format!("edge-{i:04}"),
            )
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    let before = snapshot(&f.database);
    let complete = inspect_e16_content_edges(
        &f.ctx,
        &f.store,
        &f.result.id,
        InspectionBudget {
            page_size: 17,
            ..InspectionBudget::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(complete.status, InspectionStatus::Unknown);
    assert!(object(&complete, &f.result.id).direct_edges_exhausted);
    assert_eq!(object(&complete, &f.result.id).edge_rows_returned, 703);
    assert_eq!(complete.object_reads, 4);
    let budget = InspectionBudget {
        max_edge_rows: 10,
        page_size: 2,
        ..InspectionBudget::default()
    };
    let partial = inspect_e16_content_edges(&f.ctx, &f.store, &f.result.id, budget)
        .await
        .unwrap();
    assert_eq!(partial.status, InspectionStatus::Partial);
    assert!(partial.edge_rows_read <= 10);
    assert!(!object(&partial, &f.result.id).direct_edges_exhausted);
    assert!(
        object(&partial, &f.result.id)
            .issues
            .iter()
            .any(|issue| issue.code == IssueCode::UnexpectedExistingEdge)
    );
    assert!(
        object(&partial, &f.result.id)
            .issues
            .iter()
            .all(|issue| issue.code != IssueCode::MissingDeclaredEdge)
    );
    assert_eq!(snapshot(&f.database), before);
}

#[tokio::test]
async fn completed_subject_missing_edge_survives_global_partial_for_later_objects() {
    let f = fixture().await;
    let (_directory, store) = copied(&f, Fault::MissingResultSecondary).await;
    let budget = InspectionBudget {
        max_edge_rows: 2,
        page_size: 256,
        ..InspectionBudget::default()
    };
    // Result has two rows; a one-row page plus lookahead cannot exhaust it in
    // budget 2, so the missing declaration remains unproven.
    let partial = inspect_e16_content_edges(&f.ctx, &store, &f.result.id, budget)
        .await
        .unwrap();
    assert!(!object(&partial, &f.result.id).direct_edges_exhausted);
    assert!(!has(
        &partial,
        &f.result.id,
        IssueCode::MissingDeclaredEdge,
        Some(edge("artifact", &f.sources[1].id))
    ));
    let report = inspect_e16_content_edges(
        &f.ctx,
        &store,
        &f.result.id,
        InspectionBudget {
            max_edge_rows: 3,
            page_size: 256,
            ..InspectionBudget::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(report.status, InspectionStatus::Partial);
    assert!(object(&report, &f.result.id).direct_edges_exhausted);
    assert!(has(
        &report,
        &f.result.id,
        IssueCode::MissingDeclaredEdge,
        Some(edge("artifact", &f.sources[1].id))
    ));
}

#[tokio::test]
async fn underlying_storage_error_propagates_without_a_successful_unknown_report() {
    let f = fixture().await;
    // Explicit out-of-writer SQL fault injection; not malformed_record evidence.
    sqlite(
        &f.database,
        "import sqlite3,sys\nc=sqlite3.connect(sys.argv[1])\nc.execute('DROP TABLE dependencies')\nc.commit()\nprint('null')",
    );
    let before = snapshot(&f.database);
    assert!(matches!(
        inspect_e16_content_edges(&f.ctx, &f.store, &f.result.id, InspectionBudget::default())
            .await,
        Err(Error::Internal)
    ));
    assert_eq!(snapshot(&f.database), before);
}

#[tokio::test]
async fn same_id_in_another_namespace_is_not_authority_and_wrong_wire_does_not_expand_scope() {
    let f = fixture().await;
    let (_directory, store) = copied(&f, Fault::None).await;
    let other = Context::new("other-namespace", "admin", Role::Admin).unwrap();
    let mut other_result = f.result.clone();
    other_result.namespace = other.namespace().into();
    put(&other, &store, &other_result.id, &other_result).await;
    let mut session = store.session().await.unwrap();
    session
        .put_edge(&other, "artifact", &f.result.id, "blob", "foreign-edge")
        .await
        .unwrap();
    session.commit().await.unwrap();
    assert_eq!(
        inspect(&f.ctx, &store, &f.result.id).await.status,
        InspectionStatus::Complete
    );
    let mut wrong_wire = f.selection.clone();
    wrong_wire.id = f.sources[1].id.clone();
    put(&f.ctx, &store, &wrong_wire.id, &wrong_wire).await;
    let report = inspect(&f.ctx, &store, &f.result.id).await;
    assert!(has(
        &report,
        &f.result.id,
        IssueCode::AuthorityKindMismatch,
        Some(edge("artifact", &f.sources[1].id))
    ));
    assert_eq!(report.object_reads, 4);
    assert_eq!(report.objects.len(), 4);
}

#[tokio::test]
async fn non_digest_watermark_marker_is_withheld_and_missing_subject_is_explicit() {
    let f = fixture().await;
    let missing = inspect(&f.ctx, &f.store, "absent-subject").await;
    assert!(has(
        &missing,
        "absent-subject",
        IssueCode::SubjectMissing,
        None
    ));
    let mut session = f.store.session().await.unwrap();
    session
        .bump_watermark(&f.ctx, "PRIVATE-ACTOR-MARKER")
        .await
        .unwrap();
    session.commit().await.unwrap();
    let report = inspect(&f.ctx, &f.store, &f.result.id).await;
    assert_eq!(report.status, InspectionStatus::Unknown);
    assert_eq!(report.namespace_watermark.as_ref().unwrap().sequence, 2);
    assert!(
        report
            .namespace_watermark
            .as_ref()
            .unwrap()
            .digest
            .is_none()
    );
    assert!(has(
        &report,
        &f.result.id,
        IssueCode::WatermarkDigestNotCheckable,
        None
    ));
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("PRIVATE-ACTOR-MARKER")
    );
}

#[tokio::test]
async fn unknown_redacted_wire_remains_unknown_and_malformed_retained_fields_do_not_claim_history_complete()
 {
    for case in 0..3 {
        let f = fixture().await;
        let mut retained = json!({
            "id":f.result.id,"schema_version":"rsia.redacted.v1","state":"source_revoked",
            "original_kind":"import_result","original_schema":f.result.schema_version,
            "original_digest":hash(b"original bytes"),
            "metadata":{"namespace":f.ctx.namespace(),"owner_actor":f.ctx.actor(),"request_key":f.result.request_key,
                "input_digest":f.result.input_digest,"created_at":f.result.created_at,"updated_at":f.result.updated_at,
                "source_refs":f.result.source_refs,"revoke_watermark":f.result.revoke_watermark,"payload":{}}
        });
        match case {
            0 => retained["original_schema"] = json!("private-unknown-redacted"),
            1 => retained["metadata"]["source_refs"] = json!("PRIVATE-malformed-refs"),
            _ => retained["metadata"]["input_digest"] = json!(hash(b"changed-history-input")),
        }
        put(&f.ctx, &f.store, &f.result.id, &retained).await;
        let report = inspect(&f.ctx, &f.store, &f.result.id).await;
        assert_ne!(report.status, InspectionStatus::Complete);
        assert!(has(
            &report,
            &f.result.id,
            match case {
                0 => IssueCode::UnknownSchema,
                1 => IssueCode::MalformedRecord,
                _ => IssueCode::InputDigestMismatch,
            },
            None
        ));
        assert!(!object(&report, &f.result.id).declarations_checked);
        assert_eq!(report.edge_rows_read, 0);
        assert!(
            !serde_json::to_string(&report)
                .unwrap()
                .contains("PRIVATE-malformed-refs")
        );
    }
}

#[tokio::test]
async fn real_two_hundred_sources_fit_the_exact_object_ceiling_and_lower_budget_is_partial() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let store = Store::open(&root.join("ceiling.sqlite3")).await.unwrap();
    let ctx = Context::new("ceiling", "admin", Role::Admin).unwrap();
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&ctx, &hash(b"initial"))
        .await
        .unwrap();
    session.commit().await.unwrap();
    let bytes = br#"{"role":"user","content":"PRIVATE-CAPACITY-OBSERVATION"}"#;
    let mut specs = Vec::new();
    for i in 0..200 {
        let path = root.join(format!("source-{i:03}.jsonl"));
        std::fs::write(&path, bytes).unwrap();
        specs.push(ImportSourceSpec {
            source_id: format!("source-{i:03}"),
            path: path.to_string_lossy().into_owned(),
            reader: SourceFormat::ClaudeFixture,
            expected_digest: hash(bytes),
        });
    }
    let service = PersistentImportService::new(store.clone());
    let selection = service
        .register(
            &ctx,
            ImportRegistrationRequest {
                schema_version: IMPORT_REGISTRATION_SCHEMA.into(),
                request_key: "exact-ceiling".into(),
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
    let result = service.execute(&ctx, &selection.id).await.unwrap();
    let before = snapshot(&root.join("ceiling.sqlite3"));
    let report = inspect_e16_content_edges(
        &ctx,
        &store,
        &result.id,
        InspectionBudget {
            page_size: 17,
            ..InspectionBudget::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(report.status, InspectionStatus::Complete, "{report:#?}");
    assert_eq!(report.object_reads, 202);
    assert_eq!(report.objects.len(), 202);
    assert_eq!(object(&report, &result.id).declared_edges.len(), 201);
    assert!(report.edge_rows_read < 10_000);
    let partial = inspect_e16_content_edges(
        &ctx,
        &store,
        &result.id,
        InspectionBudget {
            max_objects: 201,
            ..InspectionBudget::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(partial.status, InspectionStatus::Partial);
    assert_eq!(partial.object_reads, 201);
    assert!(partial.objects.iter().any(|object| {
        object
            .issues
            .iter()
            .any(|issue| issue.code == IssueCode::ObjectBudgetExhausted)
    }));
    assert_eq!(snapshot(&root.join("ceiling.sqlite3")), before);
}

#[tokio::test]
async fn authority_relationship_conflict_and_unbound_point_identity_are_explicit() {
    let f = fixture().await;
    let mut source = f.sources[1].clone();
    source.payload.purpose = Purpose::Inspection;
    put(&f.ctx, &f.store, &source.id, &source).await;
    let report = inspect(&f.ctx, &f.store, &f.result.id).await;
    assert!(has(
        &report,
        &f.result.id,
        IssueCode::ImmutableAuthorityDigestMismatch,
        Some(edge("artifact", &source.id))
    ));
    assert!(has(
        &report,
        &f.selection.id,
        IssueCode::AuthorityRelationMismatch,
        Some(edge("artifact", &source.id))
    ));
    assert_eq!(report.status, InspectionStatus::Anomalous);
    let mut unbound = serde_json::to_value(&f.result).unwrap();
    unbound["id"] = Value::Null; // Existing SQLite CHECK permits a NULL JSON id.
    put(&f.ctx, &f.store, &f.result.id, &unbound).await;
    let report = inspect(&f.ctx, &f.store, &f.result.id).await;
    assert!(has(
        &report,
        &f.result.id,
        IssueCode::IdentityMismatch,
        None
    ));
    assert_eq!(report.object_reads, 1);
    assert!(!object(&report, &f.result.id).declarations_checked);
}
