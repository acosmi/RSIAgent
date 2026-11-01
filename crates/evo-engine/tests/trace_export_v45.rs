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
use evo_storage::Store;
use evo_storage::lifecycle::{CleanupState, LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};

fn context(actor: &str, role: Role) -> Context {
    Context::new("tenant-a", actor, role).unwrap()
}

#[tokio::test]
async fn trace_export_loads_actual_authorized_run_and_preserves_event_identity() {
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
