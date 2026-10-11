//! E16 local material consumer. These fixtures are imported data, never execution proof.
use evo_core::evidence::{
    ExecutionAttestation, ImportArtifactRef, ImportArtifactSchema, ImportEventKey,
    ImportMaterialDispatchPolicy, ImportMaterialPrivacyStatus, ImportMaterialReadRequest,
    ImportedDevelopmentMaterial, MAX_TOTAL_EXCERPT_BYTES, Purpose, TaskOrigin,
};
use evo_core::{Context, Role, fingerprint, hash};
use evo_engine::evidence::{
    read_persisted_imported_development_material, read_persisted_imported_evidence,
};
use evo_engine::import::{
    IMPORT_REGISTRATION_SCHEMA, ImportRegistrationRequest, ImportResultRecord,
    ImportRetentionScope, ImportSourceSpec, PersistentImportService, SourceFormat,
    SourceSelectionRecord,
};
use evo_storage::Store;
use evo_storage::lifecycle::{CleanupState, LifecycleStore, TypedObjectRef};
use serde_json::Value;

struct Fixture {
    directory: tempfile::TempDir,
    store: Store,
    admin: Context,
    service: PersistentImportService,
    selection: SourceSelectionRecord,
    result: ImportResultRecord,
}

fn body(format: SourceFormat, content: &str) -> String {
    let event = serde_json::json!({"role":"user", "content":content});
    match format {
        SourceFormat::RsiaTraceV1 => serde_json::json!({
            "schema_version":"rsia.trace.v1", "events":[event]
        })
        .to_string(),
        SourceFormat::RsihPiFixture => serde_json::json!({
            "schema_version":"rsih.pi.fixture", "prompts":[event]
        })
        .to_string(),
        SourceFormat::ClaudeFixture => event.to_string(),
    }
}

impl Fixture {
    async fn new(
        sources: Vec<(SourceFormat, String, String)>,
        outbound: bool,
        purpose: Purpose,
    ) -> Self {
        Self::with_bodies(
            sources
                .into_iter()
                .map(|(format, id, bytes)| (format, id, Some(bytes)))
                .collect(),
            outbound,
            purpose,
        )
        .await
    }

    async fn with_bodies(
        sources: Vec<(SourceFormat, String, Option<String>)>,
        outbound: bool,
        purpose: Purpose,
    ) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("import.sqlite3"))
            .await
            .unwrap();
        let admin = Context::new("tenant", "admin", Role::Admin).unwrap();
        let mut session = store.session().await.unwrap();
        session
            .bump_watermark(&admin, &hash(b"initial material watermark"))
            .await
            .unwrap();
        session.commit().await.unwrap();
        let mut specs = Vec::new();
        for (index, (reader, source_id, bytes)) in sources.into_iter().enumerate() {
            let path = directory.path().join(format!("source-{index}.json"));
            if let Some(bytes) = &bytes {
                tokio::fs::write(&path, bytes.as_bytes()).await.unwrap();
            }
            specs.push(ImportSourceSpec {
                source_id,
                path: path.to_string_lossy().into_owned(),
                reader,
                expected_digest: hash(
                    bytes
                        .as_deref()
                        .unwrap_or("missing fixture bytes")
                        .as_bytes(),
                ),
            });
        }
        let service = PersistentImportService::new(store.clone());
        let selection = service
            .register(
                &admin,
                ImportRegistrationRequest {
                    schema_version: IMPORT_REGISTRATION_SCHEMA.into(),
                    request_key: "material-import".into(),
                    roots: vec![directory.path().to_string_lossy().into_owned()],
                    purpose,
                    allow_model_excerpts: outbound,
                    outbound_authorized: outbound,
                    retention_scope: if outbound {
                        ImportRetentionScope::LocalWithAuthorizedExcerpts
                    } else {
                        ImportRetentionScope::LocalPrivate
                    },
                    sources: specs,
                },
            )
            .await
            .unwrap();
        let result = service.execute(&admin, &selection.id).await.unwrap();
        Self {
            directory,
            store,
            admin,
            service,
            selection,
            result,
        }
    }

    async fn snapshot(&self) -> String {
        let audit_count = self.store.verify_audit(&self.admin).await.unwrap();
        let mut session = self.store.session().await.unwrap();
        let artifacts: Vec<Value> = session.list(&self.admin, "artifact").await.unwrap();
        let mut edges = Vec::new();
        for object in &artifacts {
            let id = object["id"].as_str().unwrap();
            let mut dependents = session
                .dependents(&self.admin, "artifact", id)
                .await
                .unwrap();
            dependents.sort();
            edges.push((id.to_string(), dependents));
        }
        session.commit().await.unwrap();
        fingerprint(&(artifacts, edges, audit_count)).unwrap()
    }

    fn request(&self, indices: &[(usize, usize)]) -> ImportMaterialReadRequest {
        ImportMaterialReadRequest {
            result: ImportArtifactRef {
                schema: ImportArtifactSchema::ImportResult,
                id: self.result.id.clone(),
                object_digest: fingerprint(&self.result).unwrap(),
            },
            events: indices
                .iter()
                .map(|(source, event)| ImportEventKey {
                    source_id: self.selection.payload.import_source_ids[*source].clone(),
                    event_index: *event,
                })
                .collect(),
        }
    }

    async fn material(
        &self,
        request: &ImportMaterialReadRequest,
    ) -> evo_core::Result<ImportedDevelopmentMaterial> {
        read_persisted_imported_development_material(&self.admin, &self.store, request).await
    }

    async fn put_result(&self, value: &Value) -> ImportMaterialReadRequest {
        let typed: ImportResultRecord = serde_json::from_value(value.clone()).unwrap();
        let mut session = self.store.session().await.unwrap();
        session
            .put(
                &self.admin,
                "artifact",
                &self.result.id,
                self.admin.actor(),
                value,
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
        let mut request = self.request(&[(0, 0)]);
        request.result.object_digest = fingerprint(&typed).unwrap();
        request
    }
}

#[tokio::test]
async fn baseline_public_apis_preserve_private_import_identity_and_raw_fragment() {
    for format in [
        SourceFormat::RsiaTraceV1,
        SourceFormat::RsihPiFixture,
        SourceFormat::ClaudeFixture,
    ] {
        let content = "local 未审查 observation with \\\"quoted\\\" text";
        let fixture = Fixture::new(
            vec![(format, "incident-a.try1".into(), body(format, content))],
            false,
            Purpose::Development,
        )
        .await;
        let before = fixture.snapshot().await;
        let result_bytes = serde_json::to_vec(&fixture.result).unwrap();
        let evaluator = Context::new("tenant", "evaluator", Role::Evaluator).unwrap();
        let summary =
            read_persisted_imported_evidence(&evaluator, &fixture.store, &fixture.result.id)
                .await
                .unwrap();
        let summary_json = serde_json::to_value(&summary).unwrap();
        assert!(summary_json.get("excerpts").is_none());
        assert_eq!(summary.task_origin, TaskOrigin::ImportedHistory);
        assert_eq!(
            summary.execution_attestation,
            ExecutionAttestation::UnverifiedImport
        );
        assert!(!summary.formal_evaluation_eligible);
        assert!(!fixture.selection.payload.outbound_authorized);
        assert!(fixture.result.payload.locators.is_empty());
        let source_id = &fixture.selection.payload.import_source_ids[0];
        let fragment = fixture
            .service
            .read_event_fragment(&fixture.admin, &fixture.result.id, source_id, 0)
            .await
            .unwrap();
        let event: Value = serde_json::from_slice(&fragment).unwrap();
        assert_eq!(event["content"], content);
        assert_ne!(fragment, content.as_bytes());
        assert!(
            fixture
                .service
                .read_event_fragment(&evaluator, &fixture.result.id, source_id, 0)
                .await
                .is_err()
        );
        let live = fixture
            .service
            .load_live_result(&fixture.admin, &fixture.result.id)
            .await
            .unwrap();
        assert_eq!(serde_json::to_vec(&live).unwrap(), result_bytes);
        assert_eq!(
            live.payload.generation_status,
            "blocked_external_generation_conditions_unavailable"
        );
        assert_eq!(fixture.snapshot().await, before);
        println!(
            "baseline {}: summary has no excerpts; fragment is raw structured JSON; local/private; imported/unverified; result and storage unchanged",
            format.as_label()
        );
    }
}

#[tokio::test]
async fn baseline_public_apis_refuse_non_primary_revoked_source() {
    let format = SourceFormat::ClaudeFixture;
    let fixture = Fixture::new(
        vec![
            (
                format,
                "incident-a.try1".into(),
                body(format, "primary observation"),
            ),
            (
                format,
                "incident-b.try1".into(),
                body(format, "counter_example observation"),
            ),
        ],
        false,
        Purpose::Development,
    )
    .await;
    let mut status = LifecycleStore::begin_revoke(
        &fixture.admin,
        &fixture.store,
        TypedObjectRef {
            kind: "artifact".into(),
            id: fixture.selection.payload.import_source_ids[1].clone(),
        },
        "remove non-primary imported history",
        10,
    )
    .await
    .unwrap();
    assert!(
        fixture
            .service
            .load_live_result(&fixture.admin, &fixture.result.id)
            .await
            .is_err()
    );
    for now in 11..80 {
        if status.state == CleanupState::Complete {
            break;
        }
        status =
            LifecycleStore::cleanup_step(&fixture.admin, &fixture.store, &status.job_id, 1, now)
                .await
                .unwrap();
        assert!(
            fixture
                .service
                .read_event_fragment(
                    &fixture.admin,
                    &fixture.result.id,
                    &fixture.selection.payload.import_source_ids[0],
                    0
                )
                .await
                .is_err()
        );
    }
    assert_eq!(status.state, CleanupState::Complete);
    let restarted = PersistentImportService::new(fixture.store.clone());
    assert!(
        restarted
            .load_live_result(&fixture.admin, &fixture.result.id)
            .await
            .is_err()
    );
    println!("legacy public API non-primary revocation: pending/running/complete reads refused");
}

#[tokio::test]
async fn three_pinned_readers_produce_repeatable_local_material_without_store_writes() {
    for format in [
        SourceFormat::RsiaTraceV1,
        SourceFormat::RsihPiFixture,
        SourceFormat::ClaudeFixture,
    ] {
        let content =
            "未审查 é中🦀 \"quoted\" /Users/private/path sk-a中文测试; touch /never-execute";
        let fixture = Fixture::new(
            vec![(format, "private-name.try1".into(), body(format, content))],
            false,
            Purpose::Development,
        )
        .await;
        let request = fixture.request(&[(0, 0)]);
        let before = fixture.snapshot().await;
        let material = fixture.material(&request).await.unwrap();
        material.validate().unwrap();
        assert_eq!(material.excerpts[0].content, content);
        assert_eq!(
            material.excerpts[0].content_digest,
            hash(content.as_bytes())
        );
        assert_eq!(material.task_origin, TaskOrigin::ImportedHistory);
        assert_eq!(
            material.execution_attestation,
            ExecutionAttestation::UnverifiedImport
        );
        assert_eq!(material.purpose, Purpose::Development);
        assert_eq!(
            material.privacy_status,
            ImportMaterialPrivacyStatus::Unreviewed
        );
        assert_eq!(
            material.dispatch_policy,
            ImportMaterialDispatchPolicy::NoModelDispatch
        );
        assert_ne!(
            material.sources[0].artifact.object_digest,
            material.sources[0].raw_digest
        );
        let bytes = serde_json::to_vec(&material).unwrap();
        let roundtrip: ImportedDevelopmentMaterial = serde_json::from_slice(&bytes).unwrap();
        roundtrip.validate().unwrap();
        assert_eq!(serde_json::to_vec(&roundtrip).unwrap(), bytes);
        let repeated = fixture.material(&request).await.unwrap();
        assert_eq!(repeated.digest, material.digest);
        let reopened = Store::open(&fixture.directory.path().join("import.sqlite3"))
            .await
            .unwrap();
        let restarted =
            read_persisted_imported_development_material(&fixture.admin, &reopened, &request)
                .await
                .unwrap();
        assert_eq!(serde_json::to_vec(&restarted).unwrap(), bytes);
        let json: Value = serde_json::from_slice(&bytes).unwrap();
        for field in [
            "roots",
            "authorized_path",
            "logical_source_id",
            "source_refs",
            "formal_evaluation_eligible",
        ] {
            assert!(
                !String::from_utf8(bytes.clone())
                    .unwrap()
                    .contains(&format!("\"{field}\""))
            );
        }
        assert!(!String::from_utf8(bytes).unwrap().contains("private-name"));
        assert_eq!(json["dispatch_policy"], "no_model_dispatch");
        assert_eq!(fixture.snapshot().await, before);
        let mut raw_ref = request.clone();
        raw_ref.result.object_digest = material.sources[0].raw_digest.clone();
        assert!(fixture.material(&raw_ref).await.is_err());
        let summary = fixture.material(&fixture.request(&[])).await.unwrap();
        assert!(summary.excerpts.is_empty());
        assert!(!summary.projection_coverage.partial);
        assert_eq!(summary.projection_coverage.serialized_excerpt_bytes, 2);
    }
}

#[tokio::test]
async fn requests_and_wire_cannot_supply_paths_trust_or_dispatch_authority() {
    let f = SourceFormat::ClaudeFixture;
    let fixture = Fixture::new(
        vec![(f, "incident.try1".into(), body(f, "observation"))],
        false,
        Purpose::Development,
    )
    .await;
    let request = fixture.request(&[(0, 0)]);
    for role in [Role::Evaluator, Role::Agent, Role::Host, Role::Worker] {
        let context = Context::new("tenant", "other", role).unwrap();
        assert!(
            read_persisted_imported_development_material(&context, &fixture.store, &request)
                .await
                .is_err()
        );
    }
    for (namespace, owner) in [("tenant", "other-admin"), ("other-tenant", "admin")] {
        let context = Context::new(namespace, owner, Role::Admin).unwrap();
        assert!(
            read_persisted_imported_development_material(&context, &fixture.store, &request)
                .await
                .is_err()
        );
    }
    let mut invalid = request.clone();
    invalid.events.push(invalid.events[0].clone());
    assert!(fixture.material(&invalid).await.is_err());
    invalid.events = (0..33)
        .map(|index| ImportEventKey {
            source_id: request.events[0].source_id.clone(),
            event_index: index,
        })
        .collect();
    assert!(fixture.material(&invalid).await.is_err());
    invalid = request.clone();
    invalid.events[0].event_index = 99;
    assert!(fixture.material(&invalid).await.is_err());
    invalid.events[0].source_id = "/Users/private/history".into();
    assert!(fixture.material(&invalid).await.is_err());
    invalid = request.clone();
    invalid.result.schema = ImportArtifactSchema::ImportSource;
    assert!(fixture.material(&invalid).await.is_err());
    for digest in ["abcd".into(), "A".repeat(64), hash(b"wrong result")] {
        invalid = request.clone();
        invalid.result.object_digest = digest;
        assert!(fixture.material(&invalid).await.is_err());
    }
    let mut json = serde_json::to_value(&request).unwrap();
    json["result"]["schema"] = "run".into();
    assert!(serde_json::from_value::<ImportMaterialReadRequest>(json).is_err());
    let mut json = serde_json::to_value(&request).unwrap();
    json["content"] = "caller-supplied".into();
    assert!(serde_json::from_value::<ImportMaterialReadRequest>(json).is_err());
    let material = fixture.material(&request).await.unwrap();
    for (field, value) in [
        ("task_origin", "trusted_run"),
        ("execution_attestation", "trusted_host"),
        ("purpose", "generation"),
    ] {
        let mut json = serde_json::to_value(&material).unwrap();
        json[field] = value.into();
        let changed: ImportedDevelopmentMaterial = serde_json::from_value(json).unwrap();
        assert!(changed.validate().is_err());
    }
    for (field, value) in [
        ("privacy_status", "clean"),
        ("dispatch_policy", "model_dispatch"),
    ] {
        let mut json = serde_json::to_value(&material).unwrap();
        json[field] = value.into();
        assert!(serde_json::from_value::<ImportedDevelopmentMaterial>(json).is_err());
    }
    let mut json = serde_json::to_value(&material).unwrap();
    json["excerpts"][0]["unknown"] = true.into();
    assert!(serde_json::from_value::<ImportedDevelopmentMaterial>(json).is_err());
    let inspection = Fixture::new(
        vec![(f, "inspection.try1".into(), body(f, "inspection"))],
        false,
        Purpose::Inspection,
    )
    .await;
    assert!(
        inspection
            .material(&inspection.request(&[(0, 0)]))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn unselected_aggregate_and_missing_sources_remain_in_material_closure() {
    let f = SourceFormat::ClaudeFixture;
    let fixture = Fixture::with_bodies(
        vec![
            (f, "incident-a.try1".into(), Some(body(f, "primary method"))),
            (
                f,
                "incident-b.try1".into(),
                Some(body(f, "counter_example regression error")),
            ),
            (f, "missing.try1".into(), None),
        ],
        false,
        Purpose::Development,
    )
    .await;
    let material = fixture.material(&fixture.request(&[(0, 0)])).await.unwrap();
    assert_eq!(material.sources.len(), 3);
    assert_eq!(
        material
            .sources
            .iter()
            .map(|source| source.artifact.id.clone())
            .collect::<Vec<_>>(),
        fixture.selection.payload.import_source_ids
    );
    assert_eq!(material.excerpts.len(), 1);
    assert_eq!(material.aggregate_summary.coverage.missing, 1);
    assert_eq!(material.aggregate_summary.total_sources, 2);
    assert_eq!(material.independent_cluster_count, 2);
    assert_eq!(
        material.aggregate_summary.counter_examples,
        vec![format!(
            "{}:0",
            fixture.selection.payload.import_source_ids[1]
        )]
    );
    assert!(!material.aggregate_summary.coverage.complete());
    assert!(!material.projection_coverage.partial);
    let repeated = body(f, "copied event");
    let copies = Fixture::new(
        vec![
            (f, "incident.try1".into(), repeated.clone()),
            (f, "incident.try2:fork1".into(), body(f, "retry event")),
            (f, "renamed-copy.try1".into(), repeated),
        ],
        false,
        Purpose::Development,
    )
    .await;
    let material = copies.material(&copies.request(&[(0, 0)])).await.unwrap();
    assert_eq!(material.sources.len(), 3);
    assert_eq!(material.independent_cluster_count, 1);
}

#[tokio::test]
async fn every_persisted_derivation_is_recomputed_even_with_rebound_result_digest() {
    let f = SourceFormat::ClaudeFixture;
    let fixture = Fixture::new(
        vec![
            (f, "a.try1".into(), body(f, "primary method")),
            (f, "b.try1".into(), body(f, "counter_example error")),
        ],
        true,
        Purpose::Development,
    )
    .await;
    let original = serde_json::to_value(&fixture.result).unwrap();
    let mutations = [
        ("/payload/event_records/0/byte_start", serde_json::json!(1)),
        ("/payload/event_records/0/byte_end", serde_json::json!(2)),
        (
            "/payload/event_records/0/raw_fragment_digest",
            serde_json::json!(hash(b"wrong raw")),
        ),
        (
            "/payload/event_records/0/normalized_content_digest",
            serde_json::json!(hash(b"wrong normalized")),
        ),
        (
            "/payload/event_records/0/role",
            serde_json::json!("assistant"),
        ),
        (
            "/payload/event_records/0/kind",
            serde_json::json!("tool_result"),
        ),
        (
            "/payload/aggregate_summary/failure_count",
            serde_json::json!(99),
        ),
        (
            "/payload/aggregate_summary/counter_examples",
            serde_json::json!(["/Users/private/injected"]),
        ),
        (
            "/payload/aggregate_summary/coverage/parsed_ok",
            serde_json::json!(99),
        ),
        (
            "/payload/aggregate_summary/coverage/bytes_read",
            serde_json::json!(0),
        ),
        (
            "/payload/evidence_set/members/0/content_digest",
            serde_json::json!(hash(b"wrong member")),
        ),
        (
            "/payload/evidence_set/members/0/purpose",
            serde_json::json!("generation"),
        ),
        (
            "/payload/evidence_set/independent_clusters",
            serde_json::json!(["injected-cluster"]),
        ),
        (
            "/payload/evidence_set/coverage/parsed_ok",
            serde_json::json!(99),
        ),
        (
            "/payload/evidence_set/digest",
            serde_json::json!(hash(b"wrong evidence")),
        ),
        ("/payload/locators/0/byte_end", serde_json::json!(1)),
        (
            "/payload/locators/0/source_digest",
            serde_json::json!(hash(b"wrong locator")),
        ),
        (
            "/payload/locators/0/excerpt_digest",
            serde_json::json!(hash(b"wrong excerpt")),
        ),
        ("/payload/state", serde_json::json!("partial")),
        ("/input_digest", serde_json::json!(hash(b"wrong refs"))),
    ];
    for (pointer, value) in mutations {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        let request = fixture.put_result(&changed).await;
        assert!(
            fixture.material(&request).await.is_err(),
            "accepted tampered {pointer}"
        );
    }
    fixture.put_result(&original).await;
    fixture.material(&fixture.request(&[(0, 0)])).await.unwrap();
}

#[tokio::test]
async fn source_metadata_binding_rejects_changes_to_any_registered_source() {
    let f = SourceFormat::ClaudeFixture;
    let fixture = Fixture::new(
        vec![
            (f, "a.try1".into(), body(f, "primary")),
            (f, "b.try1".into(), body(f, "counter_example")),
        ],
        false,
        Purpose::Development,
    )
    .await;
    let id = &fixture.selection.payload.import_source_ids[1];
    let mut session = fixture.store.session().await.unwrap();
    let original: Value = session.need(&fixture.admin, "artifact", id).await.unwrap();
    session.commit().await.unwrap();
    for (pointer, value) in [
        ("/owner_actor", serde_json::json!("other")),
        (
            "/payload/selection_id",
            serde_json::json!("e16sel-000000000000000000000000"),
        ),
        (
            "/payload/reader_version",
            serde_json::json!("claude.future"),
        ),
        ("/payload/purpose", serde_json::json!("inspection")),
        (
            "/input_digest",
            serde_json::json!(hash(b"wrong source input")),
        ),
        (
            "/source_refs/0/content_digest",
            serde_json::json!(hash(b"wrong blob ref")),
        ),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        let mut session = fixture.store.session().await.unwrap();
        session
            .put(
                &fixture.admin,
                "artifact",
                id,
                fixture.admin.actor(),
                &changed,
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
        assert!(
            fixture.material(&fixture.request(&[(0, 0)])).await.is_err(),
            "accepted source {pointer}"
        );
    }
    let mut session = fixture.store.session().await.unwrap();
    session
        .put(
            &fixture.admin,
            "artifact",
            id,
            fixture.admin.actor(),
            &original,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    fixture.material(&fixture.request(&[(0, 0)])).await.unwrap();
}

#[tokio::test]
async fn projection_accounts_for_json_escaping_unicode_and_all_requested_keys() {
    let f = SourceFormat::RsiaTraceV1;
    let events: Vec<_> = (0..33)
        .map(|index| serde_json::json!({"role":"user","content":format!("event {index} é中🦀")}))
        .collect();
    let bytes = serde_json::json!({"schema_version":"rsia.trace.v1","events":events}).to_string();
    let fixture = Fixture::new(
        vec![(f, "a.try1".into(), bytes)],
        false,
        Purpose::Development,
    )
    .await;
    let request = fixture.request(&(0..32).map(|index| (0, index)).collect::<Vec<_>>());
    let material = fixture.material(&request).await.unwrap();
    assert_eq!(material.excerpts.len(), 32);
    assert!(!material.projection_coverage.partial);
    assert_eq!(
        material.projection_coverage.serialized_excerpt_bytes,
        serde_json::to_vec(&material.excerpts).unwrap().len()
    );
    let text = "\"\\\n\t é中🦀".repeat(35_000);
    let large = Fixture::new(
        vec![(f, "large.try1".into(), body(f, &text))],
        false,
        Purpose::Development,
    )
    .await;
    let request = large.request(&[(0, 0)]);
    let material = large.material(&request).await.unwrap();
    assert!(material.aggregate_summary.coverage.char_truncated > 0);
    assert!(material.projection_coverage.partial);
    let excerpt = &material.excerpts[0];
    assert!(excerpt.truncated);
    assert!(text.starts_with(&excerpt.content));
    assert_eq!(hash(excerpt.content.as_bytes()), excerpt.content_digest);
    assert_ne!(excerpt.content_digest, excerpt.normalized_content_digest);
    assert_ne!(
        excerpt.raw_fragment_digest,
        excerpt.normalized_content_digest
    );
    let cursor = material.projection_coverage.next.as_ref().unwrap();
    assert_eq!(cursor.event, request.events[0]);
    assert_eq!(cursor.byte_offset, excerpt.content.len());
    assert!(text.is_char_boundary(cursor.byte_offset));
    let serialized = serde_json::to_vec(&material.excerpts).unwrap();
    assert!(serialized.len() <= MAX_TOTAL_EXCERPT_BYTES);
    assert_eq!(
        material.projection_coverage.serialized_excerpt_bytes,
        serialized.len()
    );
    assert!(serialized.len() > excerpt.content.len());
    let mut invalid_trailing = request.clone();
    invalid_trailing.events.push(ImportEventKey {
        source_id: request.events[0].source_id.clone(),
        event_index: 99,
    });
    assert!(large.material(&invalid_trailing).await.is_err());
}

#[tokio::test]
async fn local_material_refuses_actual_blob_tampering_and_non_primary_cleanup() {
    let f = SourceFormat::ClaudeFixture;
    let fixture = Fixture::new(
        vec![
            (f, "a.try1".into(), body(f, "primary")),
            (f, "b.try1".into(), body(f, "secondary")),
        ],
        false,
        Purpose::Development,
    )
    .await;
    let request = fixture.request(&[(0, 0)]);
    let material = fixture.material(&request).await.unwrap();
    let blob = fixture
        .directory
        .path()
        .join("blobs")
        .join(hash(fixture.admin.namespace().as_bytes()))
        .join(&material.sources[1].raw_digest);
    let original = tokio::fs::read(&blob).await.unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&blob, std::fs::Permissions::from_mode(0o600))
            .await
            .unwrap();
    }
    let mut changed = original.clone();
    changed[0] ^= 1;
    tokio::fs::write(&blob, &changed).await.unwrap();
    assert!(fixture.material(&request).await.is_err());
    tokio::fs::write(&blob, b"wrong length").await.unwrap();
    assert!(fixture.material(&request).await.is_err());
    tokio::fs::write(&blob, &original).await.unwrap();
    #[cfg(unix)]
    {
        let linked = fixture.directory.path().join("linked-blob-fixture");
        tokio::fs::hard_link(&blob, &linked).await.unwrap();
        assert!(fixture.material(&request).await.is_err());
        tokio::fs::remove_file(&linked).await.unwrap();
    }
    let mut status = LifecycleStore::begin_revoke(
        &fixture.admin,
        &fixture.store,
        TypedObjectRef {
            kind: "artifact".into(),
            id: fixture.selection.payload.import_source_ids[1].clone(),
        },
        "remove unselected source",
        10,
    )
    .await
    .unwrap();
    assert_eq!(status.state, CleanupState::Pending);
    assert!(fixture.material(&request).await.is_err());
    let mut saw_running = false;
    for now in 11..80 {
        if status.state == CleanupState::Complete {
            break;
        }
        status =
            LifecycleStore::cleanup_step(&fixture.admin, &fixture.store, &status.job_id, 1, now)
                .await
                .unwrap();
        saw_running |= status.state == CleanupState::Running;
        assert!(fixture.material(&request).await.is_err());
    }
    assert!(saw_running);
    assert_eq!(status.state, CleanupState::Complete);
    let reopened = Store::open(&fixture.directory.path().join("import.sqlite3"))
        .await
        .unwrap();
    assert!(
        read_persisted_imported_development_material(&fixture.admin, &reopened, &request)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn unselected_source_upstream_revocation_is_checked_with_a_fresh_watermark() {
    let f = SourceFormat::ClaudeFixture;
    let mut fixture = Fixture::new(
        vec![
            (f, "a.try1".into(), body(f, "primary")),
            (f, "b.try1".into(), body(f, "secondary")),
        ],
        false,
        Purpose::Development,
    )
    .await;
    let revoked = fixture.selection.payload.import_source_ids[0].clone();
    LifecycleStore::begin_revoke(
        &fixture.admin,
        &fixture.store,
        TypedObjectRef {
            kind: "artifact".into(),
            id: revoked.clone(),
        },
        "upstream revoked before new import",
        10,
    )
    .await
    .unwrap();
    let specs = (0..2)
        .map(|index| {
            let content = if index == 0 { "primary" } else { "secondary" };
            ImportSourceSpec {
                source_id: format!("fresh-{index}.try1"),
                path: fixture
                    .directory
                    .path()
                    .join(format!("source-{index}.json"))
                    .to_string_lossy()
                    .into_owned(),
                reader: f,
                expected_digest: hash(body(f, content).as_bytes()),
            }
        })
        .collect();
    fixture.selection = fixture
        .service
        .register(
            &fixture.admin,
            ImportRegistrationRequest {
                schema_version: IMPORT_REGISTRATION_SCHEMA.into(),
                request_key: "fresh-import".into(),
                roots: vec![fixture.directory.path().to_string_lossy().into_owned()],
                purpose: Purpose::Development,
                allow_model_excerpts: false,
                outbound_authorized: false,
                retention_scope: ImportRetentionScope::LocalPrivate,
                sources: specs,
            },
        )
        .await
        .unwrap();
    fixture.result = fixture
        .service
        .execute(&fixture.admin, &fixture.selection.id)
        .await
        .unwrap();
    let request = fixture.request(&[(0, 0)]);
    fixture.material(&request).await.unwrap();
    // Deliberately add a dependency edge to exercise the shared upstream gate.
    // This is a gate fixture, not an assertion of the full import learning chain.
    let mut session = fixture.store.session().await.unwrap();
    session
        .put_edge(
            &fixture.admin,
            "artifact",
            &fixture.selection.payload.import_source_ids[1],
            "artifact",
            &revoked,
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    // Legacy gate only checks these objects' own tombstones and current watermark.
    assert!(
        fixture
            .service
            .load_live_result(&fixture.admin, &fixture.result.id)
            .await
            .is_ok()
    );
    assert!(matches!(
        fixture.material(&request).await,
        Err(evo_core::Error::Forbidden)
    ));
}

#[tokio::test]
async fn fixed_result_at_sixty_four_mib_requires_the_conservative_probe_byte() {
    use evo_core::evidence::MAX_TOTAL_READ_BYTES;
    let prefix = "{\"role\":\"user\",\"content\":\"";
    let suffix = "\"}";
    let raw = format!(
        "{prefix}{}{suffix}",
        "a".repeat(MAX_TOTAL_READ_BYTES - prefix.len() - suffix.len())
    );
    assert_eq!(raw.len(), MAX_TOTAL_READ_BYTES);
    let fixture = Fixture::new(
        vec![(SourceFormat::ClaudeFixture, "boundary.try1".into(), raw)],
        false,
        Purpose::Development,
    )
    .await;
    assert_eq!(fixture.result.payload.aggregate_summary.total_sources, 1);
    assert!(
        fixture
            .result
            .payload
            .aggregate_summary
            .coverage
            .char_truncated
            > 0
    );
    let before = fixture.snapshot().await;
    assert!(
        matches!(fixture.material(&fixture.request(&[])).await,Err(evo_core::Error::Invalid(message)) if message.contains("read budget"))
    );
    assert_eq!(fixture.snapshot().await, before);
}

#[tokio::test]
async fn mismatched_lookup_ids_are_refused_by_the_existing_persisted_object_check() {
    let f = SourceFormat::ClaudeFixture;
    let fixture = Fixture::new(
        vec![(f, "a.try1".into(), body(f, "observation"))],
        false,
        Purpose::Development,
    )
    .await;
    let before = fixture.snapshot().await;
    for id in [
        &fixture.result.id,
        &fixture.selection.id,
        &fixture.selection.payload.import_source_ids[0],
    ] {
        let mut session = fixture.store.session().await.unwrap();
        let mut value: Value = session.need(&fixture.admin, "artifact", id).await.unwrap();
        value["id"] = "mismatched-record-id".into();
        assert!(
            session
                .put(
                    &fixture.admin,
                    "artifact",
                    id,
                    fixture.admin.actor(),
                    &value
                )
                .await
                .is_err()
        );
        // Drop the failed fixture transaction; do not bypass the production CHECK.
    }
    assert_eq!(fixture.snapshot().await, before);
    fixture.material(&fixture.request(&[(0, 0)])).await.unwrap();
}
