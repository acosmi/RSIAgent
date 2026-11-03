//! AG-175 Native roundtrip controls: a real captured service run, frozen into
//! a real Native candidate, read back through the real reader and in-memory
//! import. Every import stays `UnverifiedImport`; no formal authority, review
//! or delivery fact is created or claimed on the way back in.
use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceClassificationV2, HostSurfaceRegistrationV2,
    ReviewedHostSourceEnvelopeV1, SurfaceCoverage,
};
use evo_core::evidence::{
    ExecutionAttestation, Purpose, SourceSelection, TaskOrigin, TraceCoverage,
};
use evo_core::{ArtifactKind, Context, Feedback, Outcome, Prepare, Proposal, Role};
use evo_engine::import::{
    SourceFormat, detect_format, detect_format_from_bytes, ingest_imported_sources, parse_fixture,
};
use evo_engine::release_store::ReleaseStore;
use evo_engine::service::{CompleteHostPrepareConfig, HostService};
use evo_engine::trace_export::prepare_native_members;
use evo_storage::Store;
use serde_json::{Value, json};

fn context(actor: &str, role: Role) -> Context {
    Context::new("tenant-a", actor, role).unwrap()
}

async fn setup() -> (
    tempfile::TempDir,
    HostService,
    Context,
    Vec<String>,
    Vec<String>,
) {
    let source = evo_core::contract::ToolSourceV1 {
        schema_version: evo_core::contract::TOOL_SOURCE_SCHEMA.into(),
        domain: "mcp_protocol".into(),
        producer_id: "rmcp.ToolRouter.list_all".into(),
        producer_version: "rmcp-3.3.0".into(),
        definitions: serde_json::from_str(include_str!(
            "fixtures/ag168-actual-mcp-descriptors.json"
        ))
        .unwrap(),
    };
    let config = CompleteHostPrepareConfig {
        profile_id: "profile-a".into(),
        system_snapshot: evo_core::contract::SystemSnapshotV3 {
            schema_version: "rsia.system_snapshot.v3".into(),
            profile_id: "profile-a".into(),
            host_id: "reference-host".into(),
            host_version: "1.0.0".into(),
            model_id: "disabled".into(),
            tools: source.names().unwrap(),
            tool_source: source,
            mandatory_context_digest: evo_core::hash(b"mandatory"),
        },
        host_surface_id: "reference-surface".into(),
        host_capabilities: HostCapabilities {
            available: ["fs_read".into()].into(),
            granted: ["fs_read".into()].into(),
        },
        evolution_enabled: true,
        capability_level: CapabilityLevel::ToolOnly,
    };
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("roundtrip.sqlite3"))
        .await
        .unwrap();
    let host = context("gateway-host", Role::Host);
    let admin = context("admin", Role::Admin);
    let source = ReviewedHostSourceEnvelopeV1 {
        schema_version: "rsia.host_source_envelope.v1".into(),
        source_kind: "reviewed_fixture".into(),
        source_schema: "rsia.host_reviewed_declarations.v1".into(),
        extractor_version: "rsia.host_reviewed_declarations.extractor.v1".into(),
        original_utf8: concat!(
            "{\"schema_version\":\"rsia.host_reviewed_declarations.v1\",",
            "\"host\":\"reference-host\",\"host_version\":\"1.0.0\",\"declarations\":[",
            "{\"name\":\"model\",\"value_type\":\"string\",\"enum_values\":[],",
            "\"default\":{\"kind\":\"none\"}}]}"
        )
        .into(),
    };
    let registration = HostSurfaceRegistrationV2 {
        adapter_version: "1.0.0".into(),
        classifications: vec![HostSurfaceClassificationV2 {
            name: "model".into(),
            coverage: SurfaceCoverage::RuntimeOwned,
            mapped_field: None,
            consumer: None,
            reason: "model identity remains host-owned in this ToolOnly fixture".into(),
            renamed_from: None,
            test_ref: None,
            capability: None,
        }],
        retired: vec![],
    };
    ReleaseStore::register_host_surface_from_source(
        &admin,
        &store,
        "reference-surface",
        source.clone(),
        registration,
        None,
    )
    .await
    .unwrap();
    let service = HostService::new(store, host.clone()).unwrap();
    let agent = context("agent-a", Role::Agent);
    let mut source_ids = Vec::new();
    let mut run_ids = Vec::new();
    for key in ["roundtrip-a", "roundtrip-b"] {
        let prepared = service
            .prepare_complete(
                &agent,
                Prepare {
                    request_key: format!("prepare-{key}"),
                    goal: format!("private goal of {key}").into(),
                    capabilities: vec!["fs_read".into()],
                },
                &config,
            )
            .await
            .unwrap();
        service
            .feedback(
                &agent,
                Feedback {
                    request_key: format!("feedback-{key}"),
                    run_id: prepared.run.id.clone(),
                    outcome: Outcome::Failure,
                    details: format!("private feedback of {key}").into(),
                    failure_class: evo_core::FailureClass::Reasoning,
                },
            )
            .await
            .unwrap();
        let captured = service
            .record_service_facts(&host, &prepared.run.id)
            .await
            .unwrap();
        source_ids.push(captured.record.id);
        run_ids.push(prepared.run.id);
    }
    (dir, service, host, source_ids, run_ids)
}

/// The frozen members of the real captured sources, read in one transaction.
async fn frozen_members(
    service: &HostService,
    admin: &Context,
    source_ids: &[String],
) -> Vec<evo_storage::LocalExportFile> {
    let mut session = service.store().session().await.unwrap();
    let members = prepare_native_members(admin, &mut session, source_ids)
        .await
        .unwrap();
    session.commit().await.unwrap();
    members.files
}

fn selection(run_ids: &[String]) -> SourceSelection {
    SourceSelection {
        roots: Vec::new(),
        run_ids: run_ids.to_vec(),
        purpose: Purpose::Development,
        allow_model_excerpts: false,
    }
}

/// A real exported artifact comes back through the real reader and the real
/// in-memory import as `UnverifiedImport` history, never as authority.
#[tokio::test]
async fn trace_export_real_artifact_roundtrips_to_unverified_import() {
    let (_dir, service, _host, source_ids, run_ids) = setup().await;
    let admin = context("admin", Role::Admin);
    let files = frozen_members(&service, &admin, &source_ids[..1]).await;
    let trace = files
        .iter()
        .find(|file| file.path == "traces/0000.json")
        .unwrap();
    let trace_text = std::str::from_utf8(&trace.bytes).unwrap();
    let events = parse_fixture(SourceFormat::RsiaTraceV1, trace_text).unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>(),
        ["service.prepare", "service.feedback", "service.propose"]
    );
    assert!(events.iter().all(|event| event.role == "agent"
        && event.attestation == ExecutionAttestation::UnverifiedImport
        && event.task_origin == TaskOrigin::ImportedHistory
        && event.format == SourceFormat::RsiaTraceV1));
    let selection = selection(&run_ids[..1]);
    let imported =
        ingest_imported_sources(&selection, &[("trace0000", trace.bytes.as_slice())], None)
            .unwrap();
    assert_eq!(imported.events.len(), 3);
    assert!(
        imported
            .events
            .iter()
            .all(|event| event.attestation == ExecutionAttestation::UnverifiedImport)
    );
    assert!(
        imported
            .evidence_set
            .members
            .iter()
            .all(
                |member| member.execution_attestation == ExecutionAttestation::UnverifiedImport
                    && member.task_origin == TaskOrigin::ImportedHistory
                    && member.purpose == Purpose::Development
            )
    );
    assert_eq!(imported.evidence_set.members.len(), 1);
    // The manifest is an index, not a trace: it never parses as one.
    let manifest = files
        .iter()
        .find(|file| file.path == "manifest.json")
        .unwrap();
    assert!(
        parse_fixture(
            SourceFormat::RsiaTraceV1,
            std::str::from_utf8(&manifest.bytes).unwrap()
        )
        .is_err()
    );
}

/// The normalized member content, the original sequence numbers, the explicit
/// roles and the coverage all match the actually captured facts, with none of
/// the private service bytes aboard.
#[tokio::test]
async fn trace_export_normalized_content_sequence_roles_and_coverage_match() {
    let (_dir, service, _host, source_ids, _run_ids) = setup().await;
    let admin = context("admin", Role::Admin);
    let files = frozen_members(&service, &admin, &source_ids).await;
    let manifest: Value = serde_json::from_slice(
        &files
            .iter()
            .find(|file| file.path == "manifest.json")
            .unwrap()
            .bytes,
    )
    .unwrap();
    for (index, source_id) in source_ids.iter().enumerate() {
        let facts: Value = {
            let host = context("gateway-host", Role::Host);
            let mut facts_session = service.store().session().await.unwrap();
            let captured: Value = serde_json::from_slice(
                &evo_engine::evidence::load_stored_source(&mut facts_session, &host, source_id)
                    .await
                    .unwrap()
                    .record
                    .body,
            )
            .unwrap();
            facts_session.commit().await.unwrap();
            captured["facts"].clone()
        };
        let path = format!("traces/{index:04}.json");
        let trace = files
            .iter()
            .find(|file| file.path == path)
            .unwrap()
            .bytes
            .clone();
        let native: Value = serde_json::from_slice(&trace).unwrap();
        assert_eq!(manifest["sources"][index]["source_id"], json!(source_id));
        assert_eq!(manifest["sources"][index]["path"], json!(path));
        let events = native["events"].as_array().unwrap();
        assert_eq!(events.len(), facts.as_array().unwrap().len());
        let mut last_sequence = 0_u64;
        for (event, fact) in events.iter().zip(facts.as_array().unwrap()) {
            let sequence = event["sequence"].as_u64().unwrap();
            assert!(sequence > last_sequence, "sequences strictly increase");
            last_sequence = sequence;
            assert_eq!(json!(sequence), fact["sequence"]);
            assert_eq!(event["role"], fact["role"]);
            assert_eq!(event["role"], json!("agent"));
            assert_eq!(event["kind"], fact["kind"]);
            let content = event["content"].as_str().unwrap();
            assert!(content.contains(fact["subject_id"].as_str().unwrap()));
            assert!(content.starts_with("RSIA committed "));
        }
        let text = std::str::from_utf8(&trace).unwrap();
        assert!(!text.contains("private goal of"));
        assert!(!text.contains("private feedback of"));
        let coverage: TraceCoverage = serde_json::from_value(native["coverage"].clone()).unwrap();
        assert_eq!(coverage.events_seen, events.len() as u64);
        assert_eq!(coverage.events_exported, events.len() as u64);
        assert_eq!(coverage.events_filtered, 0);
        assert_eq!(coverage.parsed_ok, 1);
        let parsed = parse_fixture(SourceFormat::RsiaTraceV1, text).unwrap();
        assert_eq!(
            parsed
                .iter()
                .map(|event| (event.role.as_str(), event.kind.as_str()))
                .collect::<Vec<_>>(),
            events
                .iter()
                .map(|event| (
                    event["role"].as_str().unwrap(),
                    event["kind"].as_str().unwrap()
                ))
                .collect::<Vec<_>>()
        );
    }
}

/// The roundtrip creates no formal authority: nothing that returns claims a
/// trusted execution, a formal task origin or any purpose beyond the
/// selection's, and replaying the same bytes reads identically.
#[tokio::test]
async fn trace_export_roundtrip_does_not_create_formal_authority() {
    let (_dir, service, _host, source_ids, run_ids) = setup().await;
    let admin = context("admin", Role::Admin);
    let files = frozen_members(&service, &admin, &source_ids[..1]).await;
    let trace = files
        .iter()
        .find(|file| file.path == "traces/0000.json")
        .unwrap();
    let selection = selection(&run_ids[..1]);
    let imported =
        ingest_imported_sources(&selection, &[("trace0000", trace.bytes.as_slice())], None)
            .unwrap();
    for event in &imported.events {
        assert_ne!(event.attestation, ExecutionAttestation::TrustedHost);
        assert_eq!(event.attestation, ExecutionAttestation::UnverifiedImport);
        assert_eq!(event.task_origin, TaskOrigin::ImportedHistory);
    }
    for member in &imported.evidence_set.members {
        assert_ne!(
            member.execution_attestation,
            ExecutionAttestation::TrustedHost
        );
        assert_eq!(member.task_origin, TaskOrigin::ImportedHistory);
        assert_eq!(member.purpose, Purpose::Development);
    }
    assert!(!imported.evidence_set.digest.is_empty());
    // The same bytes import to the identical evidence digest: replay adds no
    // authority and changes no fact.
    let replay =
        ingest_imported_sources(&selection, &[("trace0000", trace.bytes.as_slice())], None)
            .unwrap();
    assert_eq!(replay.evidence_set.digest, imported.evidence_set.digest);
    assert_eq!(
        serde_json::to_value(&replay.evidence_set).unwrap(),
        serde_json::to_value(&imported.evidence_set).unwrap()
    );
}

/// An honestly truncated or filtered export never claims complete coverage,
/// and a header whose declaration stops matching its events is refused.
#[tokio::test]
async fn trace_export_truncated_or_redacted_coverage_is_not_claimed_complete() {
    let (_dir, service, _host, source_ids, _run_ids) = setup().await;
    let admin = context("admin", Role::Admin);
    let files = frozen_members(&service, &admin, &source_ids[..1]).await;
    let trace = files
        .iter()
        .find(|file| file.path == "traces/0000.json")
        .unwrap()
        .bytes
        .clone();
    let mut native: Value = serde_json::from_slice(&trace).unwrap();
    // The real artifact's own header declares the honest prefix boundary.
    let header: TraceCoverage = serde_json::from_value(native["coverage"].clone()).unwrap();
    assert!(!header.discovery_exhausted);
    assert!(!header.files_known);
    assert!(!header.complete());
    // An honestly truncated export: one event fewer, declared as filtered.
    native["events"]
        .as_array_mut()
        .unwrap()
        .pop()
        .expect("the real trace holds three events");
    let truncated_seen = 3;
    native["coverage"]["events_exported"] = json!(2);
    native["coverage"]["events_filtered"] = json!(1);
    native["coverage"]["events_seen"] = json!(truncated_seen);
    let truncated_text = serde_json::to_string(&native).unwrap();
    let truncated = parse_fixture(SourceFormat::RsiaTraceV1, &truncated_text).unwrap();
    assert_eq!(truncated.len(), 2);
    let truncated_header: TraceCoverage =
        serde_json::from_value(native["coverage"].clone()).unwrap();
    assert_eq!(truncated_header.events_seen, truncated_seen);
    assert!(!truncated_header.complete());
    // A header that still claims the dropped event is refused by the reader:
    // truncation can never be presented as complete.
    native["coverage"]["events_filtered"] = json!(0);
    native["coverage"]["events_seen"] = json!(3);
    let dishonest_text = serde_json::to_string(&native).unwrap();
    match parse_fixture(SourceFormat::RsiaTraceV1, &dishonest_text) {
        Err(error) => assert!(
            error
                .to_string()
                .contains("inconsistent coverage for rsia.trace"),
            "{error}"
        ),
        Ok(events) => panic!("the incomplete coverage claim must be refused, got {events:?}"),
    }
}

/// The Native format stays a separate closed surface: Pi and Claude histories
/// and unknown versions never cross into it, and it never leaks into them.
#[test]
fn trace_export_native_pi_claude_and_unknown_boundaries_remain_separate() {
    let native = format!(
        concat!(
            r#"{{"schema_version":"rsia.trace.v1","exporter":{{"name":"rsia","version":"1.0.0","commit":"{}"}},"#,
            r#""run_id":"run1","redaction_report":{{"schema_version":"rsia.trace.redaction.v1","scan_status":"passed","#,
            r#""review_status":"reviewed","removed_items":0,"redacted_items":0}},"#,
            r#""coverage":{{"discovery_exhausted":true,"files_known":true,"bytes_read":10,"parsed_ok":1,"#,
            r#""parsed_fail":0,"permission_denied":0,"missing":0,"unsupported_format":0,"zero_records":0,"#,
            r#""events_seen":1,"events_exported":1,"events_filtered":0,"event_truncated":0,"char_truncated":0,"#,
            r#""parameter_truncated_events":0,"parameter_truncated_bytes":0,"excerpt_truncated":0,"outbound_truncated":0}},"#,
            r#""events":[{{"sequence":1,"role":"agent","kind":"chat","content":"solve problem"}}]}}"#
        ),
        "7d22edb5dfe6ec84f7a3167aba02ae683e9941be"
    );
    let pi = r#"{"format":"rsih.pi.fixture","prompts":[{"role":"user","text":"hello pi"}]}"#;
    let claude = r#"{"format":"claude.fixture","role":"user","content":"explain code"}"#;
    assert_eq!(
        detect_format("rsia.trace.v1").unwrap(),
        SourceFormat::RsiaTraceV1
    );
    assert_eq!(
        detect_format_from_bytes(native.as_bytes()).unwrap(),
        SourceFormat::RsiaTraceV1
    );
    assert_eq!(
        detect_format_from_bytes(pi.as_bytes()).unwrap(),
        SourceFormat::RsihPiFixture
    );
    assert_eq!(
        detect_format_from_bytes(claude.as_bytes()).unwrap(),
        SourceFormat::ClaudeFixture
    );
    // Each format reads only its own shape; none accepts the others' bytes.
    assert!(parse_fixture(SourceFormat::RsiaTraceV1, &native).is_ok());
    assert!(parse_fixture(SourceFormat::RsihPiFixture, pi).is_ok());
    assert!(parse_fixture(SourceFormat::ClaudeFixture, claude).is_ok());
    for (format, body) in [
        (SourceFormat::RsihPiFixture, native.as_str()),
        (SourceFormat::ClaudeFixture, native.as_str()),
        (SourceFormat::RsiaTraceV1, pi),
        (SourceFormat::RsiaTraceV1, claude),
    ] {
        assert!(
            parse_fixture(format, body).is_err(),
            "{format:?} must not accept a foreign body"
        );
    }
    // Unknown versions and names stay unsupported; the closed table does not
    // grow a wildcard.
    assert!(detect_format("rsia.trace.v2").is_err());
    assert!(detect_format("codex").is_err());
    assert!(detect_format("some_arbitrary_format").is_err());
    let future = native.replace("rsia.trace.v1", "rsia.trace.v2");
    assert!(parse_fixture(SourceFormat::RsiaTraceV1, &future).is_err());
    assert!(
        detect_format_from_bytes(future.as_bytes()).is_err(),
        "an unknown version detects as no known format"
    );
}
