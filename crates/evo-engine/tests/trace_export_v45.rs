//! Actual service-success fact capture; no external Host or Native-export claim.
use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceClassificationV2, HostSurfaceRegistrationV2,
    ReviewedHostSourceEnvelopeV1, SurfaceCoverage,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::{ArtifactKind, Context, Error, Feedback, Outcome, Prepare, Proposal, Role, hash};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, load_stored_source};
use evo_engine::release_store::ReleaseStore;
use evo_engine::service::{CompleteHostPrepareConfig, HostService};
use evo_engine::trace_export::{
    compiled_exporter_identity, prepare_native_candidate, prepare_native_members,
    scan_native_candidate_members,
};
use evo_storage::Store;
use evo_storage::lifecycle::{CleanupState, LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};

fn context(actor: &str, role: Role) -> Context {
    Context::new("tenant-a", actor, role).unwrap()
}

async fn setup() -> (
    tempfile::TempDir,
    HostService,
    Context,
    CompleteHostPrepareConfig,
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
            mandatory_context_digest: hash(b"mandatory"),
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
    let store = Store::open(&dir.path().join("service.sqlite3"))
        .await
        .unwrap();
    let host = context("gateway-host", Role::Host);
    let admin = context("admin", Role::Admin);
    // Local reviewed software data confers no supported/native-host authority.
    // ToolOnly model identity remains runtime-owned; config capabilities are unchanged.
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
    let record = ReleaseStore::register_host_surface_from_source(
        &admin,
        &store,
        "reference-surface",
        source.clone(),
        registration,
        None,
    )
    .await
    .unwrap();
    assert_eq!(record.source.original_utf8, source.original_utf8);
    assert_eq!(
        record.manifest.source_digest,
        hash(source.original_utf8.as_bytes())
    );
    assert_eq!(
        record.manifest_digest,
        evo_core::fingerprint(&record.manifest).unwrap()
    );
    let service = HostService::new(store, host.clone()).unwrap();
    (dir, service, host, config)
}

#[tokio::test]
async fn trace_export_loads_actual_authorized_run_and_preserves_event_identity() {
    let (_dir, service, host, config) = setup().await;
    let admin = context("admin", Role::Admin);
    let agent = context("agent-a", Role::Agent);
    let request = Prepare {
        request_key: "capture-prepare".into(),
        goal: "private-goal-do-not-export".into(),
        capabilities: vec!["fs_read".into()],
    };
    let prepared = service
        .prepare_complete(&agent, request.clone(), &config)
        .await
        .unwrap();
    let other = service
        .prepare_complete(
            &context("agent-b", Role::Agent),
            Prepare {
                request_key: "other-prepare".into(),
                ..request.clone()
            },
            &config,
        )
        .await
        .unwrap();
    let feedback_request = Feedback {
        request_key: "capture-feedback".into(),
        run_id: prepared.run.id.clone(),
        outcome: Outcome::Failure,
        details: "private-feedback-do-not-export".into(),
        failure_class: evo_core::FailureClass::Reasoning,
    };
    let feedback = service
        .feedback(&agent, feedback_request.clone())
        .await
        .unwrap();
    let proposal_request = Proposal {
        request_key: "capture-propose".into(),
        run_id: prepared.run.id.clone(),
        kind: ArtifactKind::Skill,
        parent_snapshot: prepared.run.snapshot_id.clone(),
        hypothesis: "private-hypothesis-do-not-export".into(),
        applicability: "reference host".into(),
        counterexample: "other hosts".into(),
        content: "private-proposal-do-not-export".into(),
        evidence_refs: vec![feedback.id.clone()],
        required_capabilities: vec!["fs_read".into()],
        dependencies: vec![],
    };
    let candidate = service
        .propose(&agent, proposal_request.clone())
        .await
        .unwrap();
    assert_eq!(feedback.verification, "self_reported_unverified");
    assert_eq!(candidate.state, evo_core::CandidateState::Proposed);
    assert!(candidate.evaluation_id.is_none());
    assert!(candidate.approved_by.is_none());

    let mut session = service.store().session().await.unwrap();
    let facts = session
        .service_success_facts(&host, &prepared.run.id)
        .await
        .unwrap();
    let other_facts = session
        .service_success_facts(&host, &other.run.id)
        .await
        .unwrap();
    assert_eq!(facts.len(), 3);
    assert_eq!(other_facts.len(), 1);
    assert!(
        other_facts
            .iter()
            .all(|(actor, fact)| actor == "agent-b" && fact.run_id == other.run.id)
    );
    assert_eq!(
        facts
            .iter()
            .map(|(_, fact)| fact.kind.as_str())
            .collect::<Vec<_>>(),
        ["service.prepare", "service.feedback", "service.propose"]
    );
    assert_eq!(
        facts
            .iter()
            .map(|(_, fact)| fact.subject_id.as_str())
            .collect::<Vec<_>>(),
        [
            prepared.run.id.as_str(),
            feedback.id.as_str(),
            candidate.id.as_str()
        ]
    );
    assert!(facts.iter().all(|(actor, fact)| actor == agent.actor()
        && fact.schema_version == "rsia.service_success_fact.v1"
        && fact.role == Role::Agent
        && fact.run_id == prepared.run.id
        && fact.sequence > 0
        && fact.audit_digest.len() == 64));
    assert!(
        facts
            .windows(2)
            .all(|pair| pair[0].1.sequence < pair[1].1.sequence)
    );
    assert_eq!(
        session
            .service_success_facts(&admin, &prepared.run.id)
            .await
            .unwrap(),
        facts
    );
    assert!(matches!(
        session.service_success_facts(&agent, "bad/id").await,
        Err(Error::Forbidden)
    ));
    session.commit().await.unwrap();

    let audit_before_replay = service.store().verify_audit(&host).await.unwrap();
    assert_eq!(
        service
            .prepare_complete(&agent, request, &config)
            .await
            .unwrap()
            .run
            .id,
        prepared.run.id
    );
    assert_eq!(
        service.feedback(&agent, feedback_request).await.unwrap().id,
        feedback.id
    );
    assert_eq!(
        service.propose(&agent, proposal_request).await.unwrap().id,
        candidate.id
    );
    assert_eq!(
        service.store().verify_audit(&host).await.unwrap(),
        audit_before_replay
    );
    for caller in [
        agent.clone(),
        admin,
        context("different-host", Role::Host),
        Context::new("other-namespace", host.actor(), Role::Host).unwrap(),
    ] {
        assert!(matches!(
            service
                .record_service_facts(&caller, &prepared.run.id)
                .await,
            Err(Error::Forbidden)
        ));
    }
    let captured = service
        .record_service_facts(&host, &prepared.run.id)
        .await
        .unwrap();
    assert_eq!(captured.schema_version, "rsia.optimization.source.v1");
    assert_eq!(captured.record.task_origin, TaskOrigin::TrustedRun);
    assert_eq!(
        captured.record.execution_attestation,
        ExecutionAttestation::TrustedHost
    );
    assert_eq!(captured.record.purpose, Purpose::Development);
    assert_eq!(captured.trace.outcome, TraceOutcome::Incomplete);
    assert!(captured.trace.diagnosis.is_none());
    let body: Value = serde_json::from_slice(&captured.record.body).unwrap();
    assert_eq!(body["schema_version"], "rsia.service_fact_source.v1");
    assert_eq!(body["service_run_id"], prepared.run.id);
    assert_eq!(body["captured_through_sequence"], facts[2].1.sequence);
    assert_eq!(body["license"], "rsia.service_facts.summary.local.v1");
    assert_eq!(
        body["recipient_scope"],
        "origin_instance_local_namespace_only"
    );
    assert_eq!(
        body["facts"],
        serde_json::to_value(facts.iter().map(|(_, fact)| fact).collect::<Vec<_>>()).unwrap()
    );
    for secret in [
        "private-goal-do-not-export",
        "private-feedback-do-not-export",
        "private-hypothesis-do-not-export",
        "private-proposal-do-not-export",
    ] {
        assert!(!captured.trace.excerpt.contains(secret));
    }
    assert_eq!(captured.trace.excerpt.as_bytes(), captured.record.body);
    let audit_after_capture = service.store().verify_audit(&host).await.unwrap();
    assert_eq!(audit_after_capture, audit_before_replay + 1);
    let repeated = service
        .record_service_facts(&host, &prepared.run.id)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&repeated).unwrap(),
        serde_json::to_value(&captured).unwrap()
    );
    assert_eq!(
        service.store().verify_audit(&host).await.unwrap(),
        audit_after_capture
    );
    let mut session = service.store().session().await.unwrap();
    let stored = load_stored_source(&mut session, &host, &captured.record.id)
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(stored).unwrap(),
        serde_json::to_value(&captured).unwrap()
    );
    let source_row = session
        .object_row(&host, "run", &captured.record.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        session
            .need::<evo_core::Run>(&host, "artifact", &prepared.run.id)
            .await
            .unwrap()
            .owner,
        agent.actor()
    );
    assert!(
        session
            .outgoing_edges_bounded(&host, "run", &captured.record.id, 10)
            .await
            .unwrap()
            .iter()
            .any(|(kind, id)| kind == "artifact" && id == &prepared.run.id)
    );
    session.commit().await.unwrap();

    // A later actual service event cannot rewrite the already captured snapshot.
    service
        .feedback(
            &agent,
            Feedback {
                request_key: "later-feedback".into(),
                run_id: prepared.run.id.clone(),
                outcome: Outcome::Success,
                details: "later-private-feedback".into(),
                failure_class: evo_core::FailureClass::Unknown,
            },
        )
        .await
        .unwrap();
    let audit_after_later = service.store().verify_audit(&host).await.unwrap();
    assert!(
        matches!(service.record_service_facts(&host, &prepared.run.id).await,
        Err(Error::Conflict(message)) if message == "immutable source authority differs")
    );
    assert_eq!(
        service.store().verify_audit(&host).await.unwrap(),
        audit_after_later
    );
    let mut session = service.store().session().await.unwrap();
    assert_eq!(
        session
            .object_row(&host, "run", &captured.record.id)
            .await
            .unwrap()
            .unwrap()
            .body,
        source_row.body
    );
    let mut forged = serde_json::to_value(&facts[0].1).unwrap();
    forged["sequence"] = json!(facts[0].1.sequence + 1);
    session
        .audit(&agent, "service.success_fact.v1", &forged.to_string())
        .await
        .unwrap();
    assert!(
        matches!(session.service_success_facts(&host, &prepared.run.id).await,
        Err(Error::Conflict(message)) if message == "audit chain or payload mismatch")
    );
    drop(session); // The deliberately invalid fixture audit is never committed.
    assert_eq!(
        service.store().verify_audit(&host).await.unwrap(),
        audit_after_later
    );

    for (field, value, expected) in [
        ("owner", "other-owner", "service run binding differs"),
        (
            "snapshot_id",
            "other-snapshot",
            "service run snapshot differs",
        ),
    ] {
        let mut session = service.store().session().await.unwrap();
        let mut changed = serde_json::to_value(&prepared.run).unwrap();
        changed[field] = json!(value);
        session
            .put(&host, "artifact", &prepared.run.id, agent.actor(), &changed)
            .await
            .unwrap();
        session.commit().await.unwrap();
        assert!(
            matches!(service.record_service_facts(&host, &prepared.run.id).await,
            Err(Error::Conflict(message)) if message == expected)
        );
        let mut session = service.store().session().await.unwrap();
        session
            .put(
                &host,
                "artifact",
                &prepared.run.id,
                agent.actor(),
                &prepared.run,
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
    }
    let opaque_body = b"opaque old conversation without authenticated events".to_vec();
    let opaque = StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: "opaque-history".into(),
            body: opaque_body.clone(),
            parent_family: "old-family".into(),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: "opaque-history".into(),
            parent_family: "old-family".into(),
            source_digest: hash(&opaque_body),
            purpose: Purpose::Development,
            outcome: TraceOutcome::Incomplete,
            diagnosis: None,
            excerpt: String::from_utf8(opaque_body.clone()).unwrap(),
            seed: 0,
        },
        excerpt_start: 0,
        excerpt_end: opaque_body.len(),
    };
    service.record_trace(&host, &opaque).await.unwrap();
    assert!(matches!(
        service.record_service_facts(&host, &opaque.record.id).await,
        Err(Error::NotFound)
    ));
    let mut session = service.store().session().await.unwrap();
    assert_eq!(
        load_stored_source(&mut session, &host, &opaque.record.id)
            .await
            .unwrap()
            .record
            .body,
        opaque_body
    );
    assert_eq!(
        session
            .object_row(&host, "run", &captured.record.id)
            .await
            .unwrap()
            .unwrap()
            .body,
        source_row.body
    );
    session.commit().await.unwrap();

    // Leave the real upstream source and service artifact intact: revocation must
    // refuse capture immediately, before the pending cleanup reaches either row.
    let sibling = service
        .prepare_complete(
            &agent,
            Prepare {
                request_key: "independent-sibling".into(),
                goal: "independent local service run".into(),
                capabilities: vec!["fs_read".into()],
            },
            &config,
        )
        .await
        .unwrap();
    let mut upstream = opaque.clone();
    upstream.record.id = "revocation-upstream".into();
    upstream.trace.run_id = upstream.record.id.clone();
    service.record_trace(&host, &upstream).await.unwrap();
    let mut session = service.store().session().await.unwrap();
    session
        .put_edge(&host, "artifact", &other.run.id, "run", &upstream.record.id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let cleanup = LifecycleStore::begin_revoke(
        &context("admin", Role::Admin),
        service.store(),
        TypedObjectRef {
            kind: "run".into(),
            id: upstream.record.id.clone(),
        },
        "capture upstream revoked before cleanup",
        10,
    )
    .await
    .unwrap();
    assert_eq!(cleanup.state, CleanupState::Pending);
    let audit_before_refusal = service.store().verify_audit(&host).await.unwrap();
    let mut session = service.store().session().await.unwrap();
    let sources_before_refusal = session.namespace_object_count(&host, "run").await.unwrap();
    let refused_source_id = format!(
        "service-facts-{}",
        &evo_core::fingerprint(&(host.namespace(), other.run.id.as_str())).unwrap()[..32],
    );
    assert!(
        session
            .object_row(&host, "run", &refused_source_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        session
            .cached::<Value, _>(
                &host,
                "optimization.source",
                &refused_source_id,
                &Value::Null
            )
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        session
            .outgoing_edges_bounded(&host, "run", &refused_source_id, 10)
            .await
            .unwrap()
            .is_empty()
    );
    let still_present = session
        .object_row(&host, "artifact", &other.run.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        session
            .need::<StoredTraceAuthority>(&host, "run", &upstream.record.id)
            .await
            .unwrap()
            .record
            .body,
        upstream.record.body
    );
    session.commit().await.unwrap();
    let expected = format!(
        "service fact source's dependency closure holds run {}, whose source was revoked",
        upstream.record.id
    );
    assert!(
        matches!(service.record_service_facts(&host, &other.run.id).await,
        Err(Error::Conflict(message)) if message == expected)
    );
    assert_eq!(
        service.store().verify_audit(&host).await.unwrap(),
        audit_before_refusal
    );
    let mut session = service.store().session().await.unwrap();
    assert_eq!(
        session.namespace_object_count(&host, "run").await.unwrap(),
        sources_before_refusal
    );
    assert!(
        session
            .object_row(&host, "run", &refused_source_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        session
            .cached::<Value, _>(
                &host,
                "optimization.source",
                &refused_source_id,
                &Value::Null
            )
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        session
            .outgoing_edges_bounded(&host, "run", &refused_source_id, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        session
            .object_row(&host, "artifact", &other.run.id)
            .await
            .unwrap()
            .unwrap()
            .body,
        still_present.body
    );
    session.commit().await.unwrap();

    // An unrelated live sibling remains capturable under the new namespace
    // watermark; the refusal is about its typed closure, not a global shutdown.
    let live = service
        .record_service_facts(&host, &sibling.run.id)
        .await
        .unwrap();
    let live_body: Value = serde_json::from_slice(&live.record.body).unwrap();
    assert_eq!(live_body["service_run_id"], sibling.run.id);
    assert_eq!(live_body["facts"].as_array().unwrap().len(), 1);
    assert_eq!(live_body["facts"][0]["kind"], "service.prepare");
    assert_eq!(live.trace.outcome, TraceOutcome::Incomplete);
}

async fn captured_source() -> (
    tempfile::TempDir,
    HostService,
    Context,
    StoredTraceAuthority,
) {
    let (dir, service, host, config) = setup().await;
    let agent = context("agent-a", Role::Agent);
    let prepared = service
        .prepare_complete(
            &agent,
            Prepare {
                request_key: "native-prepare".into(),
                goal: "native-private-goal".into(),
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
                request_key: "native-feedback".into(),
                run_id: prepared.run.id.clone(),
                outcome: Outcome::Failure,
                details: "native-private-feedback".into(),
                failure_class: evo_core::FailureClass::Reasoning,
            },
        )
        .await
        .unwrap();
    let source = service
        .record_service_facts(&host, &prepared.run.id)
        .await
        .unwrap();
    (dir, service, context("admin", Role::Admin), source)
}

#[tokio::test]
async fn trace_export_requires_frozen_native_event_shape_and_explicit_roles() {
    let (_dir, service, admin, source) = captured_source().await;
    let captured: Value = serde_json::from_slice(&source.record.body).unwrap();
    let before = service.store().verify_audit(&admin).await.unwrap();
    let mut session = service.store().session().await.unwrap();
    let runs = session.namespace_object_count(&admin, "run").await.unwrap();
    assert!(matches!(
        prepare_native_candidate(&context("agent-a", Role::Agent), &mut session, "bad/id").await,
        Err(Error::Forbidden)
    ));
    let bytes = prepare_native_candidate(&admin, &mut session, &source.record.id)
        .await
        .unwrap();
    assert_eq!(
        session.namespace_object_count(&admin, "run").await.unwrap(),
        runs
    );
    session.commit().await.unwrap();
    assert_eq!(service.store().verify_audit(&admin).await.unwrap(), before);
    let native: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(native["schema_version"], "rsia.trace.v1");
    assert_eq!(native["run_id"], captured["service_run_id"]);
    assert_eq!(native["events"].as_array().unwrap().len(), 2);
    for (event, fact) in native["events"]
        .as_array()
        .unwrap()
        .iter()
        .zip(captured["facts"].as_array().unwrap())
    {
        assert_eq!(event["sequence"], fact["sequence"]);
        assert_eq!(event["role"], fact["role"]);
        assert_eq!(event["role"], "agent");
        assert_eq!(event["kind"], fact["kind"]);
        assert!(
            event["content"]
                .as_str()
                .unwrap()
                .contains(fact["subject_id"].as_str().unwrap())
        );
    }
    assert_eq!(native["coverage"]["discovery_exhausted"], false);
    assert_eq!(native["coverage"]["files_known"], false);
    assert_eq!(native["coverage"]["events_seen"], 2);
    assert_eq!(native["coverage"]["events_exported"], 2);
    assert_eq!(native["coverage"]["bytes_read"], source.record.body.len());
    // Prospective declarations in private bytes prove the frozen reader shape,
    // never a completed human review, persistence or delivery.
    assert_eq!(native["redaction_report"]["review_status"], "reviewed");
    let decoded = evo_engine::import::parse_fixture(
        evo_engine::import::SourceFormat::RsiaTraceV1,
        std::str::from_utf8(&bytes).unwrap(),
    )
    .unwrap();
    assert_eq!(decoded.len(), 2);
    assert!(decoded.iter().all(|event| event.role == "agent"
        && event.attestation == ExecutionAttestation::UnverifiedImport));
    assert!(
        !std::str::from_utf8(&bytes)
            .unwrap()
            .contains("native-private-goal")
    );
    assert!(
        !std::str::from_utf8(&bytes)
            .unwrap()
            .contains("native-private-feedback")
    );
    service
        .feedback(
            &context("agent-a", Role::Agent),
            Feedback {
                request_key: "after-capture".into(),
                run_id: captured["service_run_id"].as_str().unwrap().into(),
                outcome: Outcome::Success,
                details: "later private content".into(),
                failure_class: evo_core::FailureClass::Unknown,
            },
        )
        .await
        .unwrap();
    let mut session = service.store().session().await.unwrap();
    assert_eq!(
        prepare_native_candidate(&admin, &mut session, &source.record.id)
            .await
            .unwrap(),
        bytes
    );
    session.commit().await.unwrap();
}

#[tokio::test]
async fn trace_export_uses_compiled_exporter_identity() {
    let (_dir, service, admin, source) = captured_source().await;
    let mut session = service.store().session().await.unwrap();
    let compiled = option_env!("RSIA_EXPORTER_COMMIT").unwrap_or("unknown");
    if matches!(compiled, "dirty" | "unknown") {
        assert!(
            matches!(compiled_exporter_identity(), Err(Error::Invalid(message)) if message == "native exporter requires a clean known build commit")
        );
        assert!(
            matches!(prepare_native_candidate(&admin, &mut session, &source.record.id).await, Err(Error::Invalid(message)) if message == "native exporter requires a clean known build commit")
        );
    } else {
        let exporter = compiled_exporter_identity().unwrap();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let actual = std::process::Command::new("git")
            .current_dir(root)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .args(["--no-optional-locks", "rev-parse", "HEAD"])
            .output()
            .unwrap();
        assert!(actual.status.success(), "read actual repository HEAD");
        let actual = String::from_utf8(actual.stdout).unwrap();
        let actual = actual.trim();
        assert_eq!(actual.len(), 40);
        assert!(
            actual
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert_eq!(exporter.commit, actual);
        assert_eq!(compiled, actual);
        assert_eq!(exporter.name, "rsia");
        assert_eq!(exporter.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(exporter.commit, compiled);
        assert_eq!(exporter.commit.len(), 40);
        assert!(
            exporter
                .commit
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        let bytes = prepare_native_candidate(&admin, &mut session, &source.record.id)
            .await
            .unwrap();
        let native: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(native["exporter"], serde_json::to_value(exporter).unwrap());
    }
    session.commit().await.unwrap();
}

#[tokio::test]
async fn trace_export_legacy_opaque_run_never_becomes_native_events() {
    let (_dir, service, admin, source) = captured_source().await;
    let host = context("gateway-host", Role::Host);
    let mut session = service.store().session().await.unwrap();
    let healthy = prepare_native_candidate(&admin, &mut session, &source.record.id)
        .await
        .unwrap();
    session.commit().await.unwrap();
    let mut opaque = source.clone();
    opaque.record.id = "native-old-opaque".into();
    opaque.record.body = b"old opaque conversation without service events".to_vec();
    opaque.trace.run_id = opaque.record.id.clone();
    opaque.trace.source_digest = hash(&opaque.record.body);
    opaque.trace.excerpt = String::from_utf8(opaque.record.body.clone()).unwrap();
    opaque.excerpt_end = opaque.record.body.len();
    service.record_trace(&host, &opaque).await.unwrap();
    let before = service.store().verify_audit(&admin).await.unwrap();
    let mut session = service.store().session().await.unwrap();
    assert!(
        matches!(prepare_native_candidate(&admin, &mut session, &opaque.record.id).await, Err(Error::Invalid(message)) if message == "invalid service fact source")
    );
    assert_eq!(
        load_stored_source(&mut session, &host, &opaque.record.id)
            .await
            .unwrap()
            .record
            .body,
        opaque.record.body
    );
    session.commit().await.unwrap();
    for (field, value, expected) in [
        (
            "schema_version",
            json!("rsia.service_fact_source.future"),
            "forbidden",
        ),
        ("license", json!("unknown"), "forbidden"),
        ("role", json!("host"), "capture"),
        ("sequence", json!(0), "capture"),
    ] {
        let mut changed = source.clone();
        let mut body: Value = serde_json::from_slice(&changed.record.body).unwrap();
        if matches!(field, "role" | "sequence") {
            body["facts"][0][field] = value;
        } else {
            body[field] = value;
        }
        changed.record.body = serde_json::to_vec(&body).unwrap();
        changed.trace.source_digest = hash(&changed.record.body);
        changed.trace.excerpt = String::from_utf8(changed.record.body.clone()).unwrap();
        changed.excerpt_end = changed.record.body.len();
        let mut session = service.store().session().await.unwrap();
        session
            .put(&host, "run", &source.record.id, host.actor(), &changed)
            .await
            .unwrap();
        let refused = prepare_native_candidate(&admin, &mut session, &source.record.id).await;
        match expected {
            "forbidden" => assert!(matches!(refused, Err(Error::Forbidden))),
            _ => assert!(
                matches!(refused, Err(Error::Conflict(message)) if message == "service fact source differs from authenticated capture")
            ),
        }
        drop(session); // Tampering remains an uncommitted negative fixture.
    }
    let mut session = service.store().session().await.unwrap();
    assert_eq!(
        prepare_native_candidate(&admin, &mut session, &source.record.id)
            .await
            .unwrap(),
        healthy
    );
    session.commit().await.unwrap();
    assert_eq!(service.store().verify_audit(&admin).await.unwrap(), before);
}

#[tokio::test]
async fn trace_export_removes_forbidden_content_before_publication() {
    let (_dir, service, host, config) = setup().await;
    let agent = context("agent-a", Role::Agent);
    let admin = context("admin", Role::Admin);
    let private =
        "ANSWER: private-task-answer /Users/private/code sk-FAKE_PRIVATE_TOKEN 192.168.4.2";
    let mut source_ids = Vec::new();
    let mut run_ids = Vec::new();
    for index in 0..2 {
        let prepared = service
            .prepare_complete(
                &agent,
                Prepare {
                    request_key: format!("members-prepare-{index}"),
                    goal: private.into(),
                    capabilities: vec!["fs_read".into()],
                },
                &config,
            )
            .await
            .unwrap();
        let feedback = service
            .feedback(
                &agent,
                Feedback {
                    request_key: format!("members-feedback-{index}"),
                    run_id: prepared.run.id.clone(),
                    outcome: Outcome::Failure,
                    details: private.into(),
                    failure_class: evo_core::FailureClass::Reasoning,
                },
            )
            .await
            .unwrap();
        service
            .propose(
                &agent,
                Proposal {
                    request_key: format!("members-propose-{index}"),
                    run_id: prepared.run.id.clone(),
                    kind: ArtifactKind::Skill,
                    parent_snapshot: prepared.run.snapshot_id,
                    hypothesis: private.into(),
                    applicability: "reference host".into(),
                    counterexample: "other hosts".into(),
                    content: private.into(),
                    evidence_refs: vec![feedback.id],
                    required_capabilities: vec!["fs_read".into()],
                    dependencies: vec![],
                },
            )
            .await
            .unwrap();
        let source = service
            .record_service_facts(&host, &prepared.run.id)
            .await
            .unwrap();
        source_ids.push(source.record.id);
        run_ids.push(prepared.run.id);
    }
    let before = service.store().verify_audit(&admin).await.unwrap();
    let mut session = service.store().session().await.unwrap();
    let run_count = session.namespace_object_count(&admin, "run").await.unwrap();
    let candidate = prepare_native_members(&admin, &mut session, &source_ids)
        .await
        .unwrap();
    assert_eq!(candidate.files.len(), 4);
    assert_eq!(
        candidate.total_bytes,
        candidate
            .files
            .iter()
            .map(|file| file.bytes.len() as u64)
            .sum::<u64>()
    );
    assert_eq!(
        candidate.candidate_digest,
        evo_storage::local_export_tree_digest(&candidate.files)
            .unwrap()
            .0
    );
    assert_eq!(
        prepare_native_members(&admin, &mut session, &source_ids)
            .await
            .unwrap()
            .candidate_digest,
        candidate.candidate_digest
    );
    assert!(
        matches!(prepare_native_members(&admin, &mut session, &[source_ids[0].clone(), source_ids[0].clone()]).await, Err(Error::Invalid(message)) if message == "duplicate native trace source")
    );
    assert!(
        matches!(prepare_native_members(&admin, &mut session, &[]).await, Err(Error::Invalid(message)) if message == "native trace source count is invalid")
    );
    assert!(
        matches!(prepare_native_members(&admin, &mut session, &vec![source_ids[0].clone(); 99]).await, Err(Error::Invalid(message)) if message == "native trace source count is invalid")
    );
    assert_eq!(
        session.namespace_object_count(&admin, "run").await.unwrap(),
        run_count
    );
    session.commit().await.unwrap();
    assert_eq!(service.store().verify_audit(&admin).await.unwrap(), before);
    let manifest: Value = serde_json::from_slice(
        &candidate
            .files
            .iter()
            .find(|file| file.path == "manifest.json")
            .unwrap()
            .bytes,
    )
    .unwrap();
    assert_eq!(manifest["schema_version"], "rsia.trace_export.manifest.v1");
    assert_eq!(manifest["kind"], "trace");
    assert_eq!(
        manifest["license"],
        "RSIA-authored service success facts; origin instance and namespace only"
    );
    assert_eq!(
        manifest["recipient_scope"],
        "origin_instance_local_namespace_only"
    );
    assert_eq!(manifest["sources"].as_array().unwrap().len(), 2);
    for index in 0..2 {
        let file = &candidate.files[index];
        let native: Value = serde_json::from_slice(&file.bytes).unwrap();
        assert_eq!(native["run_id"], run_ids[index]);
        assert_eq!(manifest["sources"][index]["source_id"], source_ids[index]);
        assert_eq!(manifest["sources"][index]["path"], file.path);
        assert_eq!(native["events"].as_array().unwrap().len(), 3);
        assert_eq!(native["events"][0]["kind"], "service.prepare");
        assert_eq!(native["events"][1]["kind"], "service.feedback");
        assert_eq!(native["events"][2]["kind"], "service.propose");
        assert!(
            native["events"]
                .as_array()
                .unwrap()
                .iter()
                .all(|event| event["role"] == "agent")
        );
    }
    for file in &candidate.files {
        let text = std::str::from_utf8(&file.bytes).unwrap();
        for forbidden in [
            "ANSWER:",
            "/Users/",
            "sk-",
            "192.168.",
            "private-task-answer",
            "rsia.service_facts.summary.local.v1",
        ] {
            assert!(
                !text.contains(forbidden),
                "private content omitted before scanning"
            );
        }
        assert_eq!(
            evo_engine::packages::scan_privacy(text).unwrap().status,
            evo_engine::packages::RedactionStatus::Clean
        );
        assert_eq!(
            evo_engine::packages::scan_privacy(&file.path)
                .unwrap()
                .status,
            evo_engine::packages::RedactionStatus::Clean
        );
    }
    let report: Value = serde_json::from_slice(
        &candidate
            .files
            .iter()
            .find(|file| file.path == "redaction-report.json")
            .unwrap()
            .bytes,
    )
    .unwrap();
    assert_eq!(report["status"], "clean");
    assert_eq!(report["findings_by_rule"], json!({}));
    assert!(report.get("findings").is_none());
    let mut changed = candidate.files.clone();
    changed[0].bytes.push(b' ');
    assert_ne!(
        evo_storage::local_export_tree_digest(&changed).unwrap().0,
        candidate.candidate_digest
    );
}

#[test]
fn trace_export_scans_manifest_paths_and_redaction_report_without_raw_snippets() {
    use evo_engine::packages::{MAX_FILE, MAX_FILES, RedactionStatus};
    use evo_storage::LocalExportFile;
    // These members exercise scanner policy only. They grant no Native source,
    // human review, publication, or delivery authority.
    let file = |path: &str, bytes: &[u8]| LocalExportFile {
        path: path.into(),
        bytes: bytes.to_vec(),
    };
    for (path, secret) in [
        ("traces/a.json", "sk-FAKE_TRACE_TOKEN"),
        ("manifest.json", "sk-FAKE_MANIFEST_TOKEN"),
        ("redaction-report.json", "sk-FAKE_REPORT_TOKEN"),
        ("sk-FAKE_PATH_TOKEN.json", "sk-FAKE_PATH_TOKEN"),
    ] {
        let members = if path.starts_with("sk-") {
            vec![file(path, b"{}")]
        } else {
            vec![file(path, secret.as_bytes())]
        };
        let report = scan_native_candidate_members(&members).unwrap();
        assert_eq!(report.status, RedactionStatus::Blocked);
        assert!(report.findings_by_rule["secret_token"] > 0);
        let safe = serde_json::to_string(&report).unwrap();
        assert!(!safe.contains(secret));
        assert!(!safe.contains("snippet"));
        assert!(!safe.contains("detail"));
        assert!(!safe.contains(path));
    }
    for (text, rule) in [
        ("ANSWER: hidden", "hidden_answer"),
        ("BEGIN PRIVATE KEY", "private_key"),
        ("/Users/alice/private", "local_user_path"),
        ("192.168.1.15", "internal_network"),
    ] {
        let report = scan_native_candidate_members(&[file("a.json", text.as_bytes())]).unwrap();
        assert_eq!(report.status, RedactionStatus::Blocked);
        assert!(report.findings_by_rule[rule] > 0);
        assert!(!serde_json::to_string(&report).unwrap().contains(text));
    }
    for path in [
        "../a.json",
        "/tmp/a.json",
        "a//b.json",
        "./a.json",
        "C:/a.json",
        "a.py",
    ] {
        assert!(
            matches!(scan_native_candidate_members(&[file(path, b"{}")]), Err(Error::Invalid(message)) if message == "native candidate member validation failed")
        );
    }
    for members in [
        vec![file("A.json", b"{}"), file("a.json", b"{}")],
        vec![file("a", b"{}"), file("a/b.json", b"{}")],
    ] {
        assert!(
            matches!(scan_native_candidate_members(&members), Err(Error::Invalid(message)) if message == "native candidate member validation failed")
        );
    }
    assert!(
        matches!(scan_native_candidate_members(&[file("empty.json", b"")]), Err(Error::Invalid(message)) if message == "native candidate member validation failed")
    );
    assert!(
        matches!(scan_native_candidate_members(&[file("invalid.json", &[255])]), Err(Error::Invalid(message)) if message == "native candidate member is not UTF-8")
    );
    let hundred: Vec<_> = (0..MAX_FILES)
        .map(|index| file(&format!("{index}.json"), b"{}"))
        .collect();
    assert_eq!(
        scan_native_candidate_members(&hundred).unwrap().status,
        RedactionStatus::Clean
    );
    let mut too_many = hundred;
    too_many.push(file("extra.json", b"{}"));
    assert!(
        matches!(scan_native_candidate_members(&too_many), Err(Error::Invalid(message)) if message == "native candidate member validation failed")
    );
    let max_file = vec![b'a'; MAX_FILE as usize];
    let mut total: Vec<_> = (0..10)
        .map(|index| file(&format!("large-{index}.json"), &max_file))
        .collect();
    assert_eq!(
        scan_native_candidate_members(&total).unwrap().status,
        RedactionStatus::Clean
    );
    total.push(file("one-more.json", b"a"));
    assert!(
        matches!(scan_native_candidate_members(&total), Err(Error::Invalid(message)) if message == "native candidate member validation failed")
    );
    let oversized = vec![b'a'; MAX_FILE as usize + 1];
    assert!(
        matches!(scan_native_candidate_members(&[file("too-large.json", &oversized)]), Err(Error::Invalid(message)) if message == "native candidate member validation failed")
    );
}

#[tokio::test]
async fn trace_export_unknown_license_or_source_grant_is_refused() {
    let (_dir, service, admin, source) = captured_source().await;
    let host = context("gateway-host", Role::Host);
    let captured: Value = serde_json::from_slice(&source.record.body).unwrap();
    let run_id = captured["service_run_id"].as_str().unwrap();
    let audit_before = service.store().verify_audit(&admin).await.unwrap();
    let mut session = service.store().session().await.unwrap();
    let healthy = prepare_native_candidate(&admin, &mut session, &source.record.id)
        .await
        .unwrap();
    let facts_before =
        serde_json::to_value(session.service_success_facts(&admin, run_id).await.unwrap()).unwrap();
    let watermark_before = session.watermark(&admin).await.unwrap();
    let mut rows_before = Vec::new();
    for kind in ["run", "artifact"] {
        let mut after = None;
        let mut image = Vec::new();
        loop {
            let rows = session.object_rows_page(&admin, kind, after).await.unwrap();
            if rows.is_empty() {
                break;
            }
            after = rows.last().map(|row| row.rowid);
            image.extend(
                rows.into_iter()
                    .map(|row| (row.rowid, row.id, row.owner, row.body)),
            );
        }
        rows_before.push(image);
    }
    session.commit().await.unwrap();

    for field in ["license", "recipient_scope"] {
        let mut changed = source.clone();
        let mut body = captured.clone();
        body[field] = json!("unknown");
        changed.record.body = serde_json::to_vec(&body).unwrap();
        changed.trace.source_digest = hash(&changed.record.body);
        changed.trace.excerpt = String::from_utf8(changed.record.body.clone()).unwrap();
        changed.excerpt_end = changed.record.body.len();
        let mut session = service.store().session().await.unwrap();
        assert_eq!(
            prepare_native_candidate(&admin, &mut session, &source.record.id)
                .await
                .unwrap(),
            healthy,
            "healthy source before changing {field}"
        );
        session
            .put(&host, "run", &source.record.id, host.actor(), &changed)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(
                load_stored_source(&mut session, &admin, &source.record.id)
                    .await
                    .unwrap()
            )
            .unwrap(),
            serde_json::to_value(&changed).unwrap(),
            "outer authority remains valid for unknown {field}"
        );
        assert!(
            matches!(
                prepare_native_candidate(&admin, &mut session, &source.record.id).await,
                Err(Error::Forbidden)
            ),
            "unknown {field} must be refused"
        );
        drop(session); // Negative source changes are never committed.
    }

    let other_admin = Context::new("tenant-b", admin.actor(), Role::Admin).unwrap();
    let mut session = service.store().session().await.unwrap();
    assert!(matches!(
        prepare_native_candidate(&other_admin, &mut session, &source.record.id).await,
        Err(Error::NotFound)
    ));
    assert_eq!(
        serde_json::to_value(
            load_stored_source(&mut session, &admin, &source.record.id)
                .await
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(&source).unwrap()
    );
    assert_eq!(
        prepare_native_candidate(&admin, &mut session, &source.record.id)
            .await
            .unwrap(),
        healthy
    );
    assert_eq!(
        serde_json::to_value(session.service_success_facts(&admin, run_id).await.unwrap()).unwrap(),
        facts_before
    );
    assert_eq!(session.watermark(&admin).await.unwrap(), watermark_before);
    for (kind, before) in ["run", "artifact"].into_iter().zip(rows_before) {
        let mut after = None;
        let mut image = Vec::new();
        loop {
            let rows = session.object_rows_page(&admin, kind, after).await.unwrap();
            if rows.is_empty() {
                break;
            }
            after = rows.last().map(|row| row.rowid);
            image.extend(
                rows.into_iter()
                    .map(|row| (row.rowid, row.id, row.owner, row.body)),
            );
        }
        assert_eq!(image, before, "persisted {kind} rows are unchanged");
    }
    session.commit().await.unwrap();
    assert_eq!(
        service.store().verify_audit(&admin).await.unwrap(),
        audit_before
    );
}

/// The all-instance trace export record bound and the same-key rule at the
/// limit, through the real storage `put` gate the prepare job also passes.
/// Boundary seeds are minimal trace-export identities only: they fabricate no
/// review, delivery or Current phase, exactly like the replay-world capacity
/// seeds.
#[tokio::test]
async fn trace_export_global_capacity_and_same_key_at_limit() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("trace-capacity.sqlite3"))
        .await
        .unwrap();
    let admin = context("tenant-a", Role::Admin);
    let other = context("tenant-b", Role::Admin);
    let seed = |id: &str| json!({"schema_version": evo_storage::TRACE_EXPORT_SCHEMA, "id": id});
    // One transaction counts and creates together: the count includes every
    // namespace of the instance, so the last slot is spent by another
    // namespace's record.
    let mut session = store.session().await.unwrap();
    let last = (evo_storage::MVP_MAX_TRACE_EXPORTS - 1) as usize;
    for index in 0..last {
        let id = format!("trace-seed-{index:04}");
        session
            .put(&admin, "artifact", &id, admin.actor(), &seed(&id))
            .await
            .unwrap();
    }
    let cross = format!("trace-seed-{last:04}");
    session
        .put(&other, "artifact", &cross, other.actor(), &seed(&cross))
        .await
        .unwrap();
    for (ctx, id) in [(&admin, "trace-new-a"), (&other, "trace-new-b")] {
        match session
            .put(ctx, "artifact", id, ctx.actor(), &seed(id))
            .await
        {
            Err(Error::Conflict(message)) => assert_eq!(
                message,
                "MVP capacity exceeded: trace exports 1000 >= limit 1000"
            ),
            other => panic!("{id} must hit the all-instance bound, got {other:?}"),
        }
        assert!(
            session
                .object_row(ctx, "artifact", id)
                .await
                .unwrap()
                .is_none(),
            "refused record {id} leaves zero new rows"
        );
    }
    // Same key, same content, full quota: the original object is still the
    // answer and no capacity refusal happens, because this is not a new slot.
    let original = session
        .object_row(&admin, "artifact", "trace-seed-0000")
        .await
        .unwrap()
        .unwrap();
    session
        .put(
            &admin,
            "artifact",
            "trace-seed-0000",
            admin.actor(),
            &seed("trace-seed-0000"),
        )
        .await
        .unwrap();
    assert_eq!(
        session
            .object_row(&admin, "artifact", "trace-seed-0000")
            .await
            .unwrap()
            .unwrap()
            .body,
        original.body
    );
    // History is never deleted to free a slot.
    match session.delete(&admin, "artifact", "trace-seed-0001").await {
        Err(Error::Conflict(message)) => {
            assert_eq!(message, "trace export history must be retained")
        }
        other => panic!("deleting trace history must be retained, got {other:?}"),
    }
    // A real revocation redaction of one record keeps occupying the same
    // all-instance quota and stays read-only: no slot is freed by redacting.
    let redacted_id = "trace-seed-0002".to_string();
    let row = session
        .object_row(&admin, "artifact", &redacted_id)
        .await
        .unwrap()
        .unwrap();
    let tombstone = evo_storage::lifecycle::redacted_object_body(
        "artifact",
        &redacted_id,
        &serde_json::from_str::<Value>(&row.body).unwrap(),
        &row.body,
    );
    session
        .put(&admin, "artifact", &redacted_id, admin.actor(), &tombstone)
        .await
        .unwrap();
    match session
        .put(
            &admin,
            "artifact",
            "trace-late",
            admin.actor(),
            &seed("trace-late"),
        )
        .await
    {
        Err(Error::Conflict(message)) => assert_eq!(
            message,
            "MVP capacity exceeded: trace exports 1000 >= limit 1000"
        ),
        other => panic!("the redacted placeholder keeps its slot, got {other:?}"),
    }
    match session
        .put(
            &admin,
            "artifact",
            &redacted_id,
            admin.actor(),
            &seed(&redacted_id),
        )
        .await
    {
        Err(Error::Conflict(message)) => {
            assert_eq!(message, "redacted trace export history is read only")
        }
        other => panic!("redacted history is read only, got {other:?}"),
    }
    assert_eq!(
        session
            .object_row(&admin, "artifact", &redacted_id)
            .await
            .unwrap()
            .unwrap()
            .body,
        serde_json::to_string(&tombstone).unwrap()
    );
    session.commit().await.unwrap();
    store.close().await;
}
