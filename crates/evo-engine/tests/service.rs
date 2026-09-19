//! HostService boundary and persistence tests.
use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceManifest, SurfaceCoverage, SurfaceItem,
    SystemSnapshot,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::{
    ArtifactKind, Context, EntityKind, Error, Feedback, Inspect, Outcome, Prepare, Proposal, Role,
    hash,
};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority};
use evo_engine::release_store::{
    AppliedRequestMaterial, ReleaseStore, TrustedHostExecutionEvidence,
};
use evo_engine::service::{HostPrepareConfig, HostService};
use evo_storage::Store;
use serde_json::json;

fn context(actor: &str, role: Role) -> Context {
    Context::new("tenant-a", actor, role).unwrap()
}

fn digest(label: &str) -> String {
    hash(label.as_bytes())
}

fn prepare_config() -> HostPrepareConfig {
    HostPrepareConfig {
        profile_id: "profile-a".into(),
        system_snapshot: SystemSnapshot {
            schema_version: "rsia.system_snapshot.v2".into(),
            profile_id: "profile-a".into(),
            host_id: "reference-host".into(),
            host_version: "1.0.0".into(),
            model_id: "disabled".into(),
            tools: vec!["read_config".into()],
            mandatory_context_digest: digest("mandatory"),
        },
        host_surface_id: "reference-surface".into(),
        host_capabilities: HostCapabilities {
            available: ["fs_read".into()].into(),
            granted: ["fs_read".into()].into(),
        },
        evolution_enabled: true,
        capability_level: CapabilityLevel::ToolOnly,
    }
}

async fn setup() -> (tempfile::TempDir, HostService, Context, HostPrepareConfig) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("service.sqlite3"))
        .await
        .unwrap();
    let host = context("gateway-host", Role::Host);
    let admin = context("admin", Role::Admin);
    let manifest = HostSurfaceManifest {
        schema_version: "rsia.host_surface.v1".into(),
        host: "reference-host".into(),
        host_version: "1.0.0".into(),
        adapter_version: "1.0.0".into(),
        source_digest: digest("reference-surface-source"),
        items: vec![SurfaceItem {
            name: "model".into(),
            coverage: SurfaceCoverage::Supported,
            mapped_field: Some("host.model".into()),
            consumer: Some("reference-host".into()),
            reason: "fixed reference fixture".into(),
        }],
    };
    ReleaseStore::register_host_surface(
        &admin,
        &store,
        "reference-surface",
        manifest,
        vec!["model".into()],
    )
    .await
    .unwrap();
    let service = HostService::new(store, host.clone()).unwrap();
    (dir, service, host, prepare_config())
}

#[tokio::test]
async fn prepare_freezes_zero_skill_baseline_and_is_idempotent() {
    let (_dir, service, _host, config) = setup().await;
    let agent = context("agent-a", Role::Agent);
    let request = Prepare {
        request_key: "prepare-1".into(),
        goal: "locate config precedence".into(),
        capabilities: vec!["fs_read".into()],
    };
    let first = service
        .prepare(&agent, request.clone(), &config)
        .await
        .unwrap();
    let second = service.prepare(&agent, request, &config).await.unwrap();
    assert_eq!(first.run.id, second.run.id);
    assert!(first.skills.is_empty());
    assert!(
        first
            .limitations
            .contains(&"no_active_release_zero_skill_baseline".to_string())
    );
    let snapshot = ReleaseStore::read_run_snapshot(
        &context("gateway-host", Role::Host),
        service.store(),
        &first.run.id,
    )
    .await
    .unwrap();
    assert!(snapshot.projection.instructions.is_empty());
    assert_eq!(snapshot.projection.tools, vec!["read_config"]);
    assert_eq!(
        snapshot.request_material.task_input_digest,
        digest("locate config precedence")
    );
}

#[tokio::test]
async fn same_key_different_body_and_cross_actor_reads_are_rejected() {
    let (_dir, service, _host, config) = setup().await;
    let first = context("agent-a", Role::Agent);
    let second = context("agent-b", Role::Agent);
    let prepared = service
        .prepare(
            &first,
            Prepare {
                request_key: "same-key".into(),
                goal: "first".into(),
                capabilities: vec![],
            },
            &config,
        )
        .await
        .unwrap();
    let changed = service
        .prepare(
            &first,
            Prepare {
                request_key: "same-key".into(),
                goal: "changed".into(),
                capabilities: vec![],
            },
            &config,
        )
        .await;
    assert!(matches!(changed, Err(Error::Conflict(_))));
    assert!(matches!(
        service
            .inspect(
                &second,
                Inspect {
                    kind: EntityKind::Run,
                    id: prepared.run.id
                }
            )
            .await,
        Err(Error::NotFound)
    ));
}

#[tokio::test]
async fn feedback_stays_unverified_and_proposal_stays_proposed() {
    let (_dir, service, _host, config) = setup().await;
    let agent = context("agent-a", Role::Agent);
    let prepared = service
        .prepare(
            &agent,
            Prepare {
                request_key: "prepare-1".into(),
                goal: "task".into(),
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
                request_key: "feedback-1".into(),
                run_id: prepared.run.id.clone(),
                outcome: Outcome::Failure,
                details: "self reported failure".into(),
                failure_class: evo_core::FailureClass::Reasoning,
            },
        )
        .await
        .unwrap();
    assert_eq!(feedback.verification, "self_reported_unverified");
    let candidate = service
        .propose(
            &agent,
            Proposal {
                request_key: "proposal-1".into(),
                run_id: prepared.run.id,
                kind: ArtifactKind::Skill,
                parent_snapshot: prepared.run.snapshot_id,
                hypothesis: "a narrow hypothesis".into(),
                applicability: "reference host".into(),
                counterexample: "other hosts".into(),
                content: "read the higher-priority config first".into(),
                evidence_refs: vec![feedback.id],
                required_capabilities: vec!["fs_read".into()],
                dependencies: vec![],
            },
        )
        .await
        .unwrap();
    assert_eq!(candidate.state, evo_core::CandidateState::Proposed);
    assert!(candidate.approved_by.is_none());
    assert!(candidate.evaluation_id.is_none());
}

#[tokio::test]
async fn legacy_run_is_preserved_as_unverified_and_hidden_from_agent() {
    let (_dir, service, host, _config) = setup().await;
    let legacy = json!({"id":"legacy-run","owner":"old","goal":"secret old goal"});
    let mut session = service.store().session().await.unwrap();
    session
        .put(&host, "run", "legacy-run", host.actor(), &legacy)
        .await
        .unwrap();
    session.commit().await.unwrap();

    let view = service
        .inspect(
            &host,
            Inspect {
                kind: EntityKind::Run,
                id: "legacy-run".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(view["authority"], "unverified_legacy");
    assert_eq!(view["usable_as_trusted_source"], false);
    assert!(view.get("goal").is_none());
    assert!(
        service
            .inspect(
                &context("agent-a", Role::Agent),
                Inspect {
                    kind: EntityKind::Run,
                    id: "legacy-run".into(),
                },
            )
            .await
            .is_err()
    );

    let mut session = service.store().session().await.unwrap();
    let unchanged: serde_json::Value = session.need(&host, "run", "legacy-run").await.unwrap();
    session.commit().await.unwrap();
    assert_eq!(unchanged, legacy);
}

#[tokio::test]
async fn zero_skill_and_tool_only_cannot_claim_use() {
    let (_dir, service, host, config) = setup().await;
    let agent = context("agent-a", Role::Agent);
    let prepared = service
        .prepare(
            &agent,
            Prepare {
                request_key: "prepare-1".into(),
                goal: "task".into(),
                capabilities: vec![],
            },
            &config,
        )
        .await
        .unwrap();
    let snapshot = service
        .read_run_snapshot(&host, &prepared.run.id)
        .await
        .unwrap();
    let empty_evidence = TrustedHostExecutionEvidence {
        actual_request_material: AppliedRequestMaterial {
            mandatory_context_digest: snapshot.request_material.mandatory_context_digest.clone(),
            task_input_digest: snapshot.request_material.task_input_digest.clone(),
            instructions: snapshot.request_material.instructions.clone(),
            tools: snapshot.request_material.tools.clone(),
        },
        environment_digest: snapshot.environment_digest.clone(),
        host_surface_digest: snapshot.host_surface_digest.clone(),
        host_capabilities_digest: snapshot.host_capabilities_digest.clone(),
        offered: vec![],
        attached: vec![],
        used: vec![],
        execution_receipt_id: None,
        truncated: false,
    };
    let first = service
        .record_application(&host, &prepared.run.id, empty_evidence.clone())
        .await
        .unwrap();
    let replay = service
        .record_application(&host, &prepared.run.id, empty_evidence)
        .await
        .unwrap();
    assert_eq!(first.id, replay.id);
    let attempt = service
        .record_application(
            &host,
            &prepared.run.id,
            TrustedHostExecutionEvidence {
                actual_request_material: AppliedRequestMaterial {
                    mandatory_context_digest: snapshot
                        .request_material
                        .mandatory_context_digest
                        .clone(),
                    task_input_digest: snapshot.request_material.task_input_digest.clone(),
                    instructions: snapshot.request_material.instructions.clone(),
                    tools: snapshot.request_material.tools.clone(),
                },
                environment_digest: snapshot.environment_digest,
                host_surface_digest: snapshot.host_surface_digest,
                host_capabilities_digest: snapshot.host_capabilities_digest,
                offered: vec!["fake-skill".into()],
                attached: vec!["fake-skill".into()],
                used: vec!["fake-skill".into()],
                execution_receipt_id: None,
                truncated: false,
            },
        )
        .await;
    assert!(matches!(attempt, Err(Error::Invalid(_))));
}

#[tokio::test]
async fn cached_operations_revalidate_live_snapshot_after_deletion() {
    let (_dir, service, host, config) = setup().await;
    let agent = context("agent-a", Role::Agent);
    let prepare = Prepare {
        request_key: "prepare-live".into(),
        goal: "task".into(),
        capabilities: vec![],
    };
    let prepared = service
        .prepare(&agent, prepare.clone(), &config)
        .await
        .unwrap();
    let feedback = Feedback {
        request_key: "feedback-live".into(),
        run_id: prepared.run.id.clone(),
        outcome: Outcome::Failure,
        details: "self report".into(),
        failure_class: evo_core::FailureClass::Reasoning,
    };
    let feedback_record = service.feedback(&agent, feedback.clone()).await.unwrap();
    let proposal = Proposal {
        request_key: "proposal-live".into(),
        run_id: prepared.run.id.clone(),
        kind: ArtifactKind::Skill,
        parent_snapshot: prepared.run.snapshot_id.clone(),
        hypothesis: "hypothesis".into(),
        applicability: "reference host".into(),
        counterexample: "other host".into(),
        content: "bounded content".into(),
        evidence_refs: vec![feedback_record.id],
        required_capabilities: vec![],
        dependencies: vec![],
    };
    service.propose(&agent, proposal.clone()).await.unwrap();
    let snapshot = service
        .read_run_snapshot(&host, &prepared.run.id)
        .await
        .unwrap();
    let evidence = TrustedHostExecutionEvidence {
        actual_request_material: snapshot.request_material.clone(),
        environment_digest: snapshot.environment_digest.clone(),
        host_surface_digest: snapshot.host_surface_digest.clone(),
        host_capabilities_digest: snapshot.host_capabilities_digest.clone(),
        offered: vec![],
        attached: vec![],
        used: vec![],
        execution_receipt_id: None,
        truncated: false,
    };
    service
        .record_application(&host, &prepared.run.id, evidence.clone())
        .await
        .unwrap();

    let admin = context("admin", Role::Admin);
    let mut session = service.store().session().await.unwrap();
    session
        .delete(&admin, "artifact", &prepared.run.snapshot_id)
        .await
        .unwrap();
    session.commit().await.unwrap();

    assert!(matches!(
        service.prepare(&agent, prepare, &config).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        service.feedback(&agent, feedback).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        service.propose(&agent, proposal).await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        service
            .record_application(&host, &prepared.run.id, evidence)
            .await,
        Err(Error::NotFound)
    ));
}

#[tokio::test]
async fn a_different_namespace_cannot_reuse_the_trusted_host() {
    let (_dir, service, _host, config) = setup().await;
    let outsider = Context::new("tenant-b", "agent-a", Role::Agent).unwrap();
    assert!(matches!(
        service
            .prepare(
                &outsider,
                Prepare {
                    request_key: "prepare-1".into(),
                    goal: "task".into(),
                    capabilities: vec![]
                },
                &config
            )
            .await,
        Err(Error::Forbidden)
    ));
}

#[tokio::test]
async fn trace_authority_requires_the_exact_startup_host_and_inspects_redacted() {
    let (_dir, service, host, _config) = setup().await;
    let body = b"trusted failure excerpt".to_vec();
    let authority = StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: "trace-run".into(),
            body: body.clone(),
            parent_family: "family-a".into(),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: "trace-run".into(),
            parent_family: "family-a".into(),
            source_digest: hash(&body),
            purpose: Purpose::Development,
            outcome: TraceOutcome::TaskFailure,
            diagnosis: None,
            excerpt: "trusted failure excerpt".into(),
            seed: 7,
        },
        excerpt_start: 0,
        excerpt_end: body.len(),
    };
    assert!(matches!(
        service
            .record_trace(&context("other-host", Role::Host), &authority)
            .await,
        Err(Error::Forbidden)
    ));
    service.record_trace(&host, &authority).await.unwrap();
    let view = service
        .inspect(
            &host,
            Inspect {
                kind: EntityKind::Run,
                id: "trace-run".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(view["authority"], "trusted_host");
    assert_eq!(view["source_digest"], hash(&body));
    assert!(view.get("excerpt").is_none());
    assert!(view.get("body").is_none());
}

#[test]
fn config_is_strict() {
    assert!(
        serde_json::from_value::<HostPrepareConfig>(json!({
            "profile_id":"p",
            "system_snapshot": {},
            "host_surface_id":"h",
            "host_capabilities": {"available":[],"granted":[]},
            "evolution_enabled":true,
            "capability_level":"tool_only",
            "role":"admin"
        }))
        .is_err()
    );
}
