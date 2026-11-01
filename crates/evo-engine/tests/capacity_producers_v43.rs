//! Producer admission boundaries for the registered instance capacities.
//! Software fixtures retain their recorded provenance and confer no provider or P authority.
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;

use evo_core::capacity::{CapacityConflict, CapacityItem};
use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceClassificationV2, HostSurfaceRegistrationV2,
    ReviewedHostSourceEnvelopeV1, SurfaceCoverage,
};
use evo_core::evaluation::DataUse;
use evo_core::evidence::{ExecutionAttestation, Purpose, SourceSelection, TaskOrigin};
use evo_core::optimization::{
    OptimizationTrace, SkillFailureDiagnosis, SkillFailureKind, TraceOutcome,
};
use evo_core::skill_edit::EvidenceRef;
use evo_core::strategy::{
    ElasticPolicyV1, ExplorationCapsV1, HistoryOutcome, OptimizationHistoryEntry, SimulationContext,
};
use evo_core::{ArtifactKind, Context, Error, Prepare, Proposal, Role, fingerprint, hash};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
};
use evo_engine::exploration::{
    ExplorationDependency, ExplorationWorldV1, PersistentCoordinator, RootOpportunity, WorldState,
};
use evo_engine::release_store::ReleaseStore;
use evo_engine::service::{CompleteHostPrepareConfig, HostService};
use evo_storage::Store;
use serde_json::{Value, json};
use std::path::Path;

fn context(namespace: &str, actor: &str, role: Role) -> Context {
    Context::new(namespace, actor, role).unwrap()
}

fn digest(label: &str) -> String {
    hash(label.as_bytes())
}

fn capacity(error: Error, item: CapacityItem, used: u64, delta: u64) {
    match error {
        Error::Capacity(actual) => assert_eq!(
            actual,
            CapacityConflict {
                item,
                used,
                limit: item.limit(),
                delta
            }
        ),
        other => panic!("expected exact {item} capacity conflict, got {other:?}"),
    }
}

// Observe every SQL table and the whole fixture tree, including DB/WAL bytes.
// SQLite's transient shared-memory lock/read-mark file is not durable product state.
fn image(directory: &Path) -> Value {
    let script = r#"
import hashlib,json,pathlib,sqlite3,sys
root=pathlib.Path(sys.argv[1]); databases=sorted(root.glob('*.sqlite3')); assert len(databases)==1
c=sqlite3.connect('file:'+str(databases[0])+'?mode=ro',uri=True)
rows={}
for (name,) in c.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name"):
    q='"'+name.replace('"','""')+'"'
    rows[name]=sorted([[{'blob':x.hex()} if isinstance(x,bytes) else x for x in row] for row in c.execute('SELECT * FROM '+q)],key=repr)
c.close()
files=[]
for path in sorted(root.rglob('*')):
    rel=str(path.relative_to(root))
    if path.is_symlink(): files.append([rel,'symlink',str(path.readlink())])
    elif path.is_dir(): files.append([rel,'directory'])
    elif not path.name.endswith('-shm'):
        b=path.read_bytes(); files.append([rel,len(b),hashlib.sha256(b).hexdigest()])
print(json.dumps({'tables':rows,'files':files},sort_keys=True))
"#;
    let output = std::process::Command::new("python3")
        .args(["-B", "-c", script])
        .arg(directory)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

async fn database() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("capacity.sqlite3"))
        .await
        .unwrap();
    (dir, store)
}

fn prepare_config() -> CompleteHostPrepareConfig {
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
    CompleteHostPrepareConfig {
        profile_id: "profile-a".into(),
        system_snapshot: evo_core::contract::SystemSnapshotV3 {
            schema_version: "rsia.system_snapshot.v3".into(),
            profile_id: "profile-a".into(),
            host_id: "reference-host".into(),
            host_version: "1.0.0".into(),
            model_id: "disabled".into(),
            tools: source.names().unwrap(),
            tool_source: source,
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

async fn host_fixture(
    namespace: &str,
) -> (
    tempfile::TempDir,
    HostService,
    Context,
    CompleteHostPrepareConfig,
) {
    let context = |actor: &str, role: Role| Context::new(namespace, actor, role).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("service.sqlite3"))
        .await
        .unwrap();
    let host = context("gateway-host", Role::Host);
    let admin = context("admin", Role::Admin);
    // Local reviewed software data confers no supported/native-host authority.
    // ToolOnly model identity remains runtime-owned; config capabilities are unchanged.
    let (source, registration) = host_surface_input();
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
    (dir, service, host, prepare_config())
}

#[tokio::test]
async fn skill_propose_is_admitted_before_candidate_cache_edges_and_audit() {
    let (directory, service, _, config) = host_fixture("a").await;
    let agent = context("a", "agent", Role::Agent);
    let prepared = service
        .prepare_complete(
            &agent,
            Prepare {
                request_key: "prepare".into(),
                goal: "software observation".into(),
                capabilities: vec!["fs_read".into()],
            },
            &config,
        )
        .await
        .unwrap();
    let request = Proposal {
        request_key: "skill-0".into(),
        run_id: prepared.run.id,
        kind: ArtifactKind::Skill,
        parent_snapshot: prepared.run.snapshot_id,
        hypothesis: "narrow rule".into(),
        applicability: "fixture".into(),
        counterexample: "outside fixture".into(),
        content: "read the declared source".into(),
        evidence_refs: vec![],
        required_capabilities: vec![],
        dependencies: vec![],
    };
    let first = service.propose(&agent, request.clone()).await.unwrap();
    assert_eq!(first.proposal.kind, ArtifactKind::Skill);
    // Use genuine Candidate records, not schema-labelled shells, in another namespace.
    let admin = context("b", "admin", Role::Admin);
    let mut session = service.store().session().await.unwrap();
    for i in 1..999 {
        let mut row = first.clone();
        row.id = format!("other-skill-{i}");
        row.owner = admin.actor().into();
        row.proposal.request_key = format!("old-{i}");
        row.digest = fingerprint(&row.proposal).unwrap();
        session
            .put(&admin, "candidate", &row.id, admin.actor(), &row)
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    let mut last = request.clone();
    last.request_key = "skill-999".into();
    service.propose(&agent, last.clone()).await.unwrap();
    let before = image(directory.path());
    assert_eq!(
        service
            .propose(&agent, last)
            .await
            .unwrap()
            .proposal
            .request_key,
        "skill-999"
    );
    assert_eq!(image(directory.path()), before);
    let mut denied = request.clone();
    denied.request_key = "skill-1000".into();
    capacity(
        service.propose(&agent, denied).await.unwrap_err(),
        CapacityItem::Skill,
        1000,
        1,
    );
    assert_eq!(image(directory.path()), before);
    let mut improver = request;
    improver.request_key = "improver".into();
    improver.kind = ArtifactKind::Improver;
    improver.content = serde_json::to_string(&evo_core::Strategy::default()).unwrap();
    service.propose(&agent, improver).await.unwrap();
}

#[tokio::test]
async fn imported_events_share_the_instance_limit_before_result_write() {
    use evo_engine::import::{
        IMPORT_REGISTRATION_SCHEMA, ImportRegistrationRequest, ImportRetentionScope,
        ImportSourceSpec, PersistentImportService, SourceFormat,
    };
    let (directory, store) = database().await;
    let a = context("a", "admin", Role::Admin);
    let b = context("b", "admin", Role::Admin);
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&a, "import-capacity-a")
        .await
        .unwrap();
    session
        .bump_watermark(&b, "import-capacity-b")
        .await
        .unwrap();
    session.commit().await.unwrap();
    let path = directory.path().join("last.jsonl");
    let batch_path = directory.path().join("batch.jsonl");
    let line = "{\"role\":\"user\",\"content\":\"software observation\"}\n";
    let batch = line.repeat(9999);
    std::fs::write(&path, line).unwrap();
    std::fs::write(&batch_path, &batch).unwrap();
    let request = |key: &str, path: &Path, body: &[u8]| ImportRegistrationRequest {
        schema_version: IMPORT_REGISTRATION_SCHEMA.into(),
        request_key: key.into(),
        roots: vec![directory.path().to_str().unwrap().into()],
        purpose: Purpose::Development,
        allow_model_excerpts: false,
        outbound_authorized: false,
        retention_scope: ImportRetentionScope::LocalPrivate,
        sources: vec![ImportSourceSpec {
            source_id: format!("source-{key}"),
            path: path.to_str().unwrap().into(),
            reader: SourceFormat::ClaudeFixture,
            expected_digest: hash(body),
        }],
    };
    let service = PersistentImportService::new(store.clone());
    let batch_selection = service
        .register(&b, request("batch", &batch_path, batch.as_bytes()))
        .await
        .unwrap();
    let batch_result = service.execute(&b, &batch_selection.id).await.unwrap();
    assert_eq!(batch_result.payload.aggregate_summary.total_events, 9999);
    let last = service
        .register(&a, request("last", &path, line.as_bytes()))
        .await
        .unwrap();
    let last_result = service.execute(&a, &last.id).await.unwrap();
    assert_eq!(last_result.payload.aggregate_summary.total_events, 1);
    let excess = service
        .register(&a, request("excess", &path, line.as_bytes()))
        .await
        .unwrap();
    let before = image(directory.path());
    capacity(
        service.execute(&a, &excess.id).await.unwrap_err(),
        CapacityItem::Event,
        10000,
        1,
    );
    assert_eq!(image(directory.path()), before);
    assert_eq!(
        service.execute(&a, &last.id).await.unwrap().id,
        last_result.id
    );
    assert_eq!(image(directory.path()), before);
}

fn world() -> ExplorationWorldV1 {
    let parent_skill = hash(b"parent-skill");
    let parent_bundle = hash(b"parent-bundle");
    let environment = hash(b"environment");
    let model = hash(b"model");
    let tools = hash(b"tools");
    let grader = hash(b"grader");
    let rules = hash(b"rules");
    let context_signature = evo_core::fingerprint(&(
        &parent_skill,
        &parent_bundle,
        &environment,
        &model,
        &tools,
        &grader,
        &rules,
        1u64,
    ))
    .unwrap();
    ExplorationWorldV1 {
        schema_version: ExplorationWorldV1::SCHEMA.into(),
        id: "world-1".into(),
        approved_parent_digest: hash(b"approved-parent"),
        context_signature,
        parent_skill_digest: parent_skill,
        parent_bundle_digest: parent_bundle,
        environment_digest: environment,
        model_digest: model,
        tools_digest: tools,
        grader_digest: grader,
        rules_digest: rules,
        source_watermark: 1,
        caps: ExplorationCapsV1::online(),
        policy: ElasticPolicyV1::default(),
        simulation: SimulationContext::Online { fixed_seed: 7 },
        root_opportunities: vec![
            RootOpportunity {
                root_slot: 2,
                branch_seq: 2,
                action_seq: 2,
                estimated_cost_upper_micros: 10,
            },
            RootOpportunity {
                root_slot: 1,
                branch_seq: 1,
                action_seq: 1,
                estimated_cost_upper_micros: 10,
            },
        ],
        dependencies: vec![
            ExplorationDependency {
                kind: "run".into(),
                id: "run-failure".into(),
            },
            ExplorationDependency {
                kind: "run".into(),
                id: "run-success".into(),
            },
        ],
        successor_cost_upper_micros: 10,
        initial_baseline_quality_micros: 500_000,
        remaining_root_micros: 1_000,
        remaining_recovery_dispatches: 2,
        state: WorldState::Collecting,
        node_ids: vec![],
        dispatch_ids: vec![],
        history_ids: vec![],
        current_branch_seq: None,
        current_branch_focus_actions: 0,
        decision_round: 0,
        waits: vec![],
    }
}

fn trace(run: &str, family: &str, outcome: TraceOutcome) -> OptimizationTrace {
    OptimizationTrace {
        run_id: run.into(),
        parent_family: family.into(),
        source_digest: hash(run.as_bytes()),
        purpose: Purpose::Development,
        outcome,
        diagnosis: (outcome == TraceOutcome::TaskFailure).then(|| SkillFailureDiagnosis {
            kind: SkillFailureKind::SkillDefect,
            skill_id: "skill".into(),
            bundle_digest: hash(b"bundle"),
            request_digest: hash(b"request"),
            rule_id: Some("rule".into()),
            support: vec![EvidenceRef {
                id: run.into(),
                digest: hash(run.as_bytes()),
            }],
            counterexamples: vec![],
            reason: "fixture".into(),
        }),
        excerpt: run.into(),
        seed: 1,
    }
}

async fn exploration_sources(store: &Store, namespace: &str) {
    let host = context(namespace, "host", Role::Host);
    for (id, family, outcome) in [
        ("run-failure", "family-a", TraceOutcome::TaskFailure),
        ("run-success", "family-b", TraceOutcome::Success),
    ] {
        store_trace_authority(
            store,
            &host,
            &StoredTraceAuthority {
                schema_version: "rsia.optimization.source.v1".into(),
                record: StoredRunRecord {
                    id: id.into(),
                    body: id.as_bytes().to_vec(),
                    parent_family: family.into(),
                    task_origin: TaskOrigin::TrustedRun,
                    execution_attestation: ExecutionAttestation::TrustedHost,
                    purpose: Purpose::Development,
                },
                trace: trace(id, family, outcome),
                excerpt_start: 0,
                excerpt_end: id.len(),
            },
        )
        .await
        .unwrap();
    }
    store_source_selection(
        store,
        &host,
        &SourceSelection {
            roots: vec![],
            run_ids: vec!["run-failure".into(), "run-success".into()],
            purpose: Purpose::Development,
            allow_model_excerpts: true,
        },
    )
    .await
    .unwrap();
    let mut session = store.session().await.unwrap();
    session
        .bump_watermark(&host, "source-watermark")
        .await
        .unwrap();
    session.commit().await.unwrap();
}

#[tokio::test]
async fn exploration_world_limit_refuses_new_and_keeps_same_world_replay() {
    let (directory, store) = database().await;
    exploration_sources(&store, "a").await;
    exploration_sources(&store, "b").await;
    let a = PersistentCoordinator::new(
        store.clone(),
        context("a", "worker", Role::Worker),
        "worker",
    )
    .unwrap();
    let b = PersistentCoordinator::new(
        store.clone(),
        context("b", "worker", Role::Worker),
        "worker",
    )
    .unwrap();
    for i in 0..99 {
        let mut next = world();
        next.id = format!("world-{i}");
        a.register_world_idempotent(next).await.unwrap();
    }
    let last = world();
    b.register_world_idempotent(last.clone()).await.unwrap();
    let before = image(directory.path());
    b.register_world_idempotent(last.clone()).await.unwrap();
    assert_eq!(image(directory.path()), before);
    let mut new = last;
    new.id = "world-excess".into();
    capacity(
        b.register_world_idempotent(new).await.unwrap_err(),
        CapacityItem::ExplorationWorld,
        100,
        1,
    );
    assert_eq!(image(directory.path()), before);
}

#[tokio::test]
async fn optimization_history_limit_precedes_index_append() {
    let (directory, store) = database().await;
    exploration_sources(&store, "n").await;
    let coordinator = PersistentCoordinator::new(
        store.clone(),
        context("n", "worker", Role::Worker),
        "worker",
    )
    .unwrap();
    coordinator.register_world(world()).await.unwrap();
    for sequence in 1..=1000 {
        coordinator
            .record_history("world-1", history(sequence))
            .await
            .unwrap();
    }
    let before = image(directory.path());
    capacity(
        coordinator
            .record_history("world-1", history(1001))
            .await
            .unwrap_err(),
        CapacityItem::OptimizationHistory,
        1000,
        1,
    );
    assert_eq!(image(directory.path()), before);
}

fn history(sequence: u64) -> OptimizationHistoryEntry {
    OptimizationHistoryEntry {
        entry_id: format!("history-{sequence}"),
        sequence,
        parent_digest: hash(b"approved-parent"),
        environment_digest: hash(b"environment"),
        task_family: "family".into(),
        source_watermark: 1,
        input_digest: hash(b"input"),
        patch_digest: hash(b"patch"),
        evidence_digest: hash(b"evidence"),
        outcome: HistoryOutcome::NoChange,
        deterministic_error: true,
        data_use: DataUse::Development,
        summary: "software history".into(),
    }
}

#[tokio::test]
async fn prepare_slots_are_shared_by_services_of_one_instance() {
    let (directory, store) = database().await;
    let a = HostService::new(store.clone(), context("a", "host", Role::Host)).unwrap();
    let b = HostService::new(store, context("b", "host", Role::Host)).unwrap();
    let mut held = Vec::new();
    for _ in 0..4 {
        held.push(a.begin_inflight_prepare().unwrap());
    }
    held.push(b.begin_inflight_prepare().unwrap());
    assert_eq!((a.inflight_prepare(), b.inflight_prepare()), (5, 5));
    let before = image(directory.path());
    capacity(
        b.begin_inflight_prepare().unwrap_err(),
        CapacityItem::InflightPrepare,
        5,
        1,
    );
    assert_eq!(image(directory.path()), before);
    drop(held);
    assert_eq!((a.inflight_prepare(), b.inflight_prepare()), (0, 0));
    let guards = (0..5)
        .map(|_| b.begin_inflight_prepare().unwrap())
        .collect::<Vec<_>>();
    drop(guards);
    assert_eq!(a.inflight_prepare(), 0);
}

async fn authorize_meta(store: &Store) {
    store
        .authorize_root_budget(
            &context("n", "admin", Role::Admin),
            &evo_storage::budget::RootBudgetAuthorization {
                root_budget_id: "meta-root".into(),
                billing_scope: "meta-scope".into(),
                allowed_namespaces: vec!["n".into()],
                currency: "USD".into(),
                pricing_version: "fixture-price".into(),
                payment_subject: "software-only".into(),
                authorization_receipt_digest: digest("meta-authorization"),
                per_call_cap_micros: 20,
                total_limit_micros: 1000,
                created_at: 1,
            },
        )
        .await
        .unwrap();
}
fn meta_request(id: &str) -> evo_engine::meta::MetaTrialForkRequest {
    let mut candidate = evo_core::improver::ImproverContentV2::builtin_default();
    let evo_core::improver::ImproverMechanismV2::ExplorationPolicy { policy } =
        &mut candidate.mechanism;
    policy.max_focus_actions = 1;
    let mut template = world();
    template.id = "s0-template".into();
    evo_engine::meta::MetaTrialForkRequest {
        trial_id: id.into(),
        s0_template: template,
        new_content: candidate,
        billing_scope: "meta-scope".into(),
    }
}

#[tokio::test]
async fn meta_trial_and_two_worlds_are_admitted_as_one_intent() {
    use evo_engine::meta::MetaTrialCoordinator;
    let (directory, store) = database().await;
    exploration_sources(&store, "n").await;
    authorize_meta(&store).await;
    let admin = context("n", "admin", Role::Admin);
    for i in 0..10 {
        MetaTrialCoordinator::fork(&admin, &store, meta_request(&format!("trial-{i}")))
            .await
            .unwrap();
    }
    let before = image(directory.path());
    capacity(
        MetaTrialCoordinator::fork(&admin, &store, meta_request("trial-excess"))
            .await
            .unwrap_err(),
        CapacityItem::MetaTrial,
        10,
        1,
    );
    assert_eq!(image(directory.path()), before);
    MetaTrialCoordinator::fork(&admin, &store, meta_request("trial-9"))
        .await
        .unwrap();
    assert_eq!(image(directory.path()), before);
    let (directory, store) = database().await;
    exploration_sources(&store, "n").await;
    authorize_meta(&store).await;
    let coordinator =
        PersistentCoordinator::new(store.clone(), admin.clone(), admin.actor()).unwrap();
    for i in 0..99 {
        let mut row = world();
        row.id = format!("standalone-{i}");
        coordinator.register_world(row).await.unwrap();
    }
    let before = image(directory.path());
    capacity(
        MetaTrialCoordinator::fork(&admin, &store, meta_request("whole-intent"))
            .await
            .unwrap_err(),
        CapacityItem::ExplorationWorld,
        99,
        2,
    );
    assert_eq!(image(directory.path()), before);
}

#[tokio::test]
async fn partial_meta_resume_only_charges_missing_identities() {
    use evo_engine::meta::{MetaStream, MetaTrialCoordinator, stream_world_id};
    for missing in 0..=2usize {
        let (directory, store) = database().await;
        exploration_sources(&store, "n").await;
        authorize_meta(&store).await;
        let admin = context("n", "admin", Role::Admin);
        let trial = MetaTrialCoordinator::fork(&admin, &store, meta_request("resume"))
            .await
            .unwrap();
        // Simulate a genuine pre-atomic legacy cut using records produced by the fork above.
        let mut session = store.session().await.unwrap();
        for stream in MetaStream::BOTH.iter().take(missing) {
            let id = format!(
                "e09-{}",
                fingerprint(&("exploration_world_v1", stream_world_id("resume", *stream))).unwrap()
            );
            session.delete(&admin, "artifact", &id).await.unwrap();
        }
        session.commit().await.unwrap();
        let coordinator =
            PersistentCoordinator::new(store.clone(), admin.clone(), admin.actor()).unwrap();
        for i in 0..98 {
            let mut row = world();
            row.id = format!("filler-{i}");
            coordinator.register_world(row).await.unwrap();
        }
        let recovered = MetaTrialCoordinator::fork(&admin, &store, meta_request("resume"))
            .await
            .unwrap();
        assert_eq!(recovered.trial_id, trial.trial_id);
        let mut session = store.session().await.unwrap();
        assert_eq!(
            session
                .capacity_used(CapacityItem::MetaTrial)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            session
                .capacity_used(CapacityItem::ExplorationWorld)
                .await
                .unwrap(),
            100
        );
        session.commit().await.unwrap();
        let before = image(directory.path());
        MetaTrialCoordinator::fork(&admin, &store, meta_request("resume"))
            .await
            .unwrap();
        assert_eq!(image(directory.path()), before);
    }
}

#[tokio::test]
async fn management_record_limit_precedes_private_input_and_keeps_close() {
    use evo_engine::dispatch::{ManagementDispatcher, ManagementJobState};
    let (directory, store) = database().await;
    let admin = context("a", "admin", Role::Admin);
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let payload =
        |key: &str| json!({"schema_version":"rsia.management.meta_start.v1","request_key":key});
    let first = dispatcher
        .submit(&admin, "meta.start", payload("healthy"))
        .await
        .unwrap();
    let wait = async {
        loop {
            let value = dispatcher
                .status(&admin, first.identity().id)
                .await
                .unwrap();
            if value.identity().state == ManagementJobState::Blocked {
                break value;
            }
            tokio::task::yield_now().await;
        }
    };
    let first = tokio::time::timeout(std::time::Duration::from_secs(10), wait)
        .await
        .unwrap();
    let evo_storage::VersionedManagementJob::V2(first) = first else {
        panic!("a fresh management submission must persist v2");
    };
    let other = context("b", "admin", Role::Admin);
    let mut session = store.session().await.unwrap();
    for i in 1..9999 {
        let mut row = first.clone();
        row.id = format!("closed-job-{i}");
        row.request_key = format!("closed-{i}");
        row.owner_actor = other.actor().into();
        row.state = ManagementJobState::Cancelled;
        row.cancel_requested = true;
        session
            .put(&other, "job", &row.id, other.actor(), &row)
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    let last = dispatcher
        .submit(&admin, "meta.start", payload("last"))
        .await
        .unwrap();
    let wait = async {
        loop {
            let value = dispatcher.status(&admin, last.identity().id).await.unwrap();
            if value.identity().state == ManagementJobState::Blocked {
                break value;
            }
            tokio::task::yield_now().await;
        }
    };
    let last = tokio::time::timeout(std::time::Duration::from_secs(10), wait)
        .await
        .unwrap();
    let before = image(directory.path());
    capacity(
        dispatcher
            .submit(&admin, "meta.start", payload("excess"))
            .await
            .unwrap_err(),
        CapacityItem::ManagementJobRecord,
        10000,
        1,
    );
    assert_eq!(image(directory.path()), before);
    assert_eq!(
        dispatcher
            .submit(&admin, "meta.start", payload("last"))
            .await
            .unwrap()
            .identity()
            .id,
        last.identity().id
    );
    assert_eq!(
        dispatcher
            .cancel(&admin, last.identity().id)
            .await
            .unwrap()
            .identity()
            .state,
        last.identity().state
    );
    assert_eq!(image(directory.path()), before);
}

fn host_surface_input() -> (ReviewedHostSourceEnvelopeV1, HostSurfaceRegistrationV2) {
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
    (source, registration)
}

#[tokio::test]
async fn provider_meter_and_host_surface_limits_preserve_idempotency() {
    use evo_engine::formal_authority::{
        FormalRegistry, METER_SCHEMA, RegisteredResourceInstrumentation,
    };
    let (directory, store) = database().await;
    let admin = context("providers", "fixture-admin", Role::Admin);
    let fixture = trusted_compiler_inputs::inputs(
        "providers",
        "provider-capacity",
        evo_core::contract::SkillSnapshot::empty(),
        evo_core::Strategy::default(),
        HostCapabilities {
            available: Default::default(),
            granted: Default::default(),
        },
        &digest("environment"),
    );
    FormalRegistry::register_host(&admin, &store, fixture.host.clone())
        .await
        .unwrap();
    let mut provider = fixture.provider;
    for i in 0..49 {
        provider.id = format!("provider-{i}");
        FormalRegistry::register_provider(&admin, &store, provider.clone())
            .await
            .unwrap();
    }
    let meter = RegisteredResourceInstrumentation {
        id: "meter-last".into(),
        schema_version: METER_SCHEMA.into(),
        namespace: "providers".into(),
        host_registration_id: fixture.host.id,
        endpoint: "https://fixture.invalid/measure".into(),
        credential_environment_key: "UNAVAILABLE_METER_KEY".into(),
        credential_principal: digest("meter-principal"),
        instrumentation_code_digest: digest("meter-code"),
        authorized_by: admin.actor().into(),
    };
    FormalRegistry::register_resource_instrumentation(&admin, &store, meter.clone())
        .await
        .unwrap();
    // Existing authority registrations may refresh their own content witnesses;
    // they do not acquire another service identity at full capacity.
    FormalRegistry::register_provider(&admin, &store, provider.clone())
        .await
        .unwrap();
    FormalRegistry::register_resource_instrumentation(&admin, &store, meter.clone())
        .await
        .unwrap();
    let mut session = store.session().await.unwrap();
    assert_eq!(
        session
            .capacity_used(CapacityItem::ExternalService)
            .await
            .unwrap(),
        50
    );
    session.commit().await.unwrap();
    let before = image(directory.path());
    provider.id = "provider-excess".into();
    capacity(
        FormalRegistry::register_provider(&admin, &store, provider)
            .await
            .unwrap_err(),
        CapacityItem::ExternalService,
        50,
        1,
    );
    let mut excess = meter;
    excess.id = "meter-excess".into();
    capacity(
        FormalRegistry::register_resource_instrumentation(&admin, &store, excess)
            .await
            .unwrap_err(),
        CapacityItem::ExternalService,
        50,
        1,
    );
    assert_eq!(image(directory.path()), before);
    // V3 requires the separate opaque owned writer; this fixture never turns
    // a public V2 packet into an owned capability.
    let (directory, service, _, _) = host_fixture("surfaces").await;
    let admin = context("surfaces", "admin", Role::Admin);
    let (source, registration) = host_surface_input();
    for i in 1..100 {
        ReleaseStore::register_host_surface_from_source(
            &admin,
            service.store(),
            &format!("surface-{i}"),
            source.clone(),
            registration.clone(),
            None,
        )
        .await
        .unwrap();
    }
    let original = ReleaseStore::register_host_surface_from_source(
        &admin,
        service.store(),
        "surface-99",
        source.clone(),
        registration.clone(),
        None,
    )
    .await
    .unwrap();
    let mut session = service.store().session().await.unwrap();
    assert_eq!(
        session
            .capacity_used(CapacityItem::HostSurface)
            .await
            .unwrap(),
        100
    );
    session.commit().await.unwrap();
    let before = image(directory.path());
    assert_eq!(
        ReleaseStore::register_host_surface_from_source(
            &admin,
            service.store(),
            "surface-99",
            source.clone(),
            registration.clone(),
            None
        )
        .await
        .unwrap()
        .id,
        original.id
    );
    capacity(
        ReleaseStore::register_host_surface_from_source(
            &admin,
            service.store(),
            "surface-excess",
            source,
            registration,
            None,
        )
        .await
        .unwrap_err(),
        CapacityItem::HostSurface,
        100,
        1,
    );
    assert_eq!(image(directory.path()), before);
}

mod economics {
    use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
    use evo_core::optimization::{OptimizationTrace, TraceOutcome};
    use evo_core::replay::{
        DEFAULT_LAMBDA_MICROS, HistoricalUsage, PrefixCoverageV1, REPLAY_MANIFEST_SCHEMA,
        REPLAY_WORLD_SCHEMA, ReplayActionSpecV1, ReplayObjective, ReplaySimulationProfile,
        ReplaySourceRef, ReplayTransitionOutcome, ReplayTransitionV2, ReplayWorldManifestV2,
        ReplayWorldV2, SIMULATION_VERSION, StoredReplayReportV2, WorldPartition,
        replay_baseline_observation_bytes, replay_baseline_observation_digest,
        replay_observation_bytes, replay_observation_digest,
    };
    use evo_core::replay_economics::{
        ArmOrder, ComponentBillingBindingV1, ComponentBillingSourceV1, CostComponentKind,
        EXPERIMENT_SCHEMA, EconomicBudgetBindingV1, EconomicBudgetStage, EconomicClaim,
        EconomicSourceRefV1, FullCostPlanV1, MatchedRuntimeContract, PairedTaskRef,
        REPLAY_SELECTION_RULE_V1, ReplayEconomicExperimentV1, ReplaySelectionRef,
        SimulationOnlyDiagnosticRef,
    };
    use evo_core::strategy::{ActionKindV1, ElasticPolicyV1, ExplorationCapsV1, ObservedStatus};
    use evo_core::{Context, Role, fingerprint, hash};
    use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
    use evo_engine::replay::{LiveWorldAuthority, run_replay};
    use evo_engine::replay_experiment::PersistentReplayEconomicCoordinator;
    use evo_storage::Store;
    use std::collections::BTreeSet;

    fn d(value: &str) -> String {
        hash(value.as_bytes())
    }

    fn economic_source_body() -> serde_json::Value {
        serde_json::json!({"schema_version":"fixture.source.v1","id":"source-1"})
    }

    fn budget_binding() -> EconomicBudgetBindingV1 {
        let bindings = [
            (
                CostComponentKind::HistoryCollection,
                "group-history",
                EconomicBudgetStage::HistoryCollection,
            ),
            (
                CostComponentKind::PolicyGeneration,
                "group-policy",
                EconomicBudgetStage::CandidateGeneration,
            ),
            (
                CostComponentKind::ReplayCpu,
                "group-replay-cpu",
                EconomicBudgetStage::StorageCpu,
            ),
            (
                CostComponentKind::ReplayStorage,
                "group-replay-storage",
                EconomicBudgetStage::StorageCpu,
            ),
            (
                CostComponentKind::OnlineDevelopmentFixed,
                "group-fixed",
                EconomicBudgetStage::DevelopmentExecution,
            ),
            (
                CostComponentKind::OnlineDevelopmentReplaySelected,
                "group-replay",
                EconomicBudgetStage::DevelopmentExecution,
            ),
            (
                CostComponentKind::IndependentAcceptance,
                "group-acceptance",
                EconomicBudgetStage::FormalEvaluation,
            ),
            (
                CostComponentKind::CanaryOperations,
                "group-canary",
                EconomicBudgetStage::GrayOperations,
            ),
        ]
        .into_iter()
        .map(|(component, group, stage)| ComponentBillingBindingV1 {
            component,
            source: ComponentBillingSourceV1::BudgetCall {
                dispatch_group_id: group.into(),
                stage,
            },
        });
        EconomicBudgetBindingV1 {
            billing_scope: "economic-scope".into(),
            root_budget_id: "root-1".into(),
            currency: "usd".into(),
            pricing_version: "price-v1".into(),
            payment_subject: "payer-1".into(),
            components: bindings
                .chain(std::iter::once(ComponentBillingBindingV1 {
                    component: CostComponentKind::HumanReview,
                    source: ComponentBillingSourceV1::AdminMeasurement {
                        source_id: "admin-review-receipt".into(),
                    },
                }))
                .collect(),
        }
    }

    fn world(id: &str, cluster: &str, source_id: &str, partition: WorldPartition) -> ReplayWorldV2 {
        let generation = d("generation");
        let baseline = d("baseline-context");
        let next = d(&format!("next-{id}"));
        let action = ReplayActionSpecV1 {
            record_seq: 1,
            generation_signature: generation.clone(),
            parent_context_signature: baseline.clone(),
            branch_seq: 1,
            target_depth: 1,
            action_kind: ActionKindV1::Widen { root_slot: 1 },
            estimated_cost_upper_micros: Some(1),
            writes_shared_workspace: false,
        };
        let transition = ReplayTransitionV2 {
            record_id: format!("transition-{id}"),
            record_seq: 1,
            generation_signature: generation.clone(),
            parent_context_signature: baseline.clone(),
            action_kind: action.action_kind.clone(),
            next_context_signature: next.clone(),
            outcome: ReplayTransitionOutcome::Observed {
                status: ObservedStatus::Valid {
                    quality_micros: 600_000,
                },
            },
            actual_usage: HistoricalUsage {
                input_tokens: 1,
                output_tokens: 1,
                cost_micros: Some(1),
                latency_millis: Some(1),
            },
            source_ids: vec![source_id.into()],
            observation_source_id: source_id.into(),
        };
        let mut world = ReplayWorldV2 {
            schema_version: REPLAY_WORLD_SCHEMA.into(),
            manifest: ReplayWorldManifestV2 {
                schema_version: REPLAY_MANIFEST_SCHEMA.into(),
                world_id: id.into(),
                cluster_id: cluster.into(),
                partition,
                purpose: Purpose::Development,
                generation_signature: generation,
                world_context_signature: d("world-context"),
                baseline_context_signature: baseline,
                approved_parent_digest: d("approved-parent"),
                model_digest: d("model"),
                tools_digest: d("tools"),
                scorer_digest: d("scorer"),
                guidance_digest: d("guidance"),
                repair_template_digest: d("repair"),
                input_order_digest: d("order"),
                initial_baseline_quality_micros: 500_000,
                baseline_observation_source_id: format!("baseline-{source_id}"),
                source_closure: vec![
                    ReplaySourceRef {
                        source_id: source_id.into(),
                        content_digest: String::new(),
                    },
                    ReplaySourceRef {
                        source_id: format!("baseline-{source_id}"),
                        content_digest: String::new(),
                    },
                ],
                revoke_watermark: 1,
                prefix_coverage: vec![
                    PrefixCoverageV1 {
                        context_signature: d("baseline-context"),
                        exhausted: false,
                    },
                    PrefixCoverageV1 {
                        context_signature: next,
                        exhausted: true,
                    },
                ],
                action_catalog: vec![action],
            },
            transitions: vec![transition],
            sealed_digest: None,
        };
        world.manifest.source_closure[1].content_digest =
            replay_baseline_observation_digest(&world.manifest).unwrap();
        world.manifest.source_closure[0].content_digest =
            replay_observation_digest(&world.manifest, &world.transitions[0]).unwrap();
        world.seal().unwrap();
        world
    }

    fn experiment(report: &StoredReplayReportV2) -> ReplayEconomicExperimentV1 {
        ReplayEconomicExperimentV1 {
            schema_version: EXPERIMENT_SCHEMA.into(),
            id: "economic-exp-1".into(),
            profile_id: "profile-1".into(),
            evaluator_actor: "evaluator".into(),
            executor_actor: "executor".into(),
            proposer_actor: "proposer".into(),
            approver_actor: "approver".into(),
            primary_claim: EconomicClaim::NoninferiorSavings,
            min_gain_micros: 20_000,
            noninferiority_margin_micros: 10_000,
            preregistration_digest: d("preregistered"),
            paired_ticket_id: "paired-ticket".into(),
            paired_ticket_digest: d("paired-ticket"),
            dataset_epoch: "epoch-1".into(),
            tasks: vec![PairedTaskRef {
                ordinal: 1,
                task_id: "task-1".into(),
                task_digest: d("task-1"),
                cluster_id: "cluster-1".into(),
                arm_order: ArmOrder::FixedThenReplaySelected,
            }],
            runtime: MatchedRuntimeContract {
                w_online: 1,
                fixed_policy_digest: d("fixed-policy"),
                replay_selected_policy_digest: report.policy_digest.clone(),
                generation_strategy_digest: d("generation"),
                candidate_bundle_digest: d("candidate-bundle"),
                baseline_bundle_digest: d("baseline-bundle"),
                environment_digest: d("environment"),
                model_digest: d("model"),
                tools_digest: d("tools"),
                runner_digest: d("runner"),
                grader_digest: d("scorer"),
                guidance_digest: d("guidance"),
                rules_digest: d("rules"),
                context_signature: d("context"),
                target_runtime_profile: "online-w1".into(),
                per_arm_node_budget: 12,
                per_arm_root_budget_micros: 1_000,
            },
            replay_selection: ReplaySelectionRef {
                report_artifact_id: report.report_id.clone(),
                report_digest: fingerprint(report).unwrap(),
                semantic_digest: report.semantic_reports_digest.clone(),
                world_pool_digest: report.pool_digest.clone(),
                policy_digest: report.policy_digest.clone(),
                profile_digest: report.profile_digest.clone(),
                caps_digest: report.caps_digest.clone(),
                selection_rule_version: REPLAY_SELECTION_RULE_V1.into(),
                selected_with_w_sim: 1,
                target_w_online: 1,
                simulation_only: vec![SimulationOnlyDiagnosticRef {
                    w_sim: 2,
                    report_id: "simulation-w2".into(),
                    report_digest: d("simulation-w2"),
                }],
            },
            cost_plan: FullCostPlanV1 {
                component_kinds: CostComponentKind::ALL.into(),
                currency: "usd".into(),
                pricing_version: "price-v1".into(),
                payment_subject: "payer-1".into(),
            },
            budget_binding: budget_binding(),
            sources: vec![EconomicSourceRefV1 {
                id: "source-1".into(),
                digest: fingerprint(&economic_source_body()).unwrap(),
            }],
            source_watermark: 1,
        }
    }

    async fn setup_in(
        store: &Store,
        namespace: &str,
    ) -> (Context, Context, ReplayEconomicExperimentV1) {
        let store = store.clone();
        let admin = Context::new(namespace, "admin", Role::Admin).unwrap();
        let evaluator = Context::new(namespace, "evaluator", Role::Evaluator).unwrap();
        let train = world(
            "world-train",
            "cluster-train",
            "run-train",
            WorldPartition::Train,
        );
        let select = world(
            "world-select",
            "cluster-select",
            "run-select",
            WorldPartition::Select,
        );
        for (id, cluster, replay_world) in [
            ("run-train", "family-train", &train),
            ("run-select", "family-select", &select),
        ] {
            let body =
                replay_observation_bytes(&replay_world.manifest, &replay_world.transitions[0])
                    .unwrap();
            store_trace_authority(
                &store,
                &Context::new(namespace, "host", Role::Host).unwrap(),
                &StoredTraceAuthority {
                    schema_version: "rsia.optimization.source.v1".into(),
                    record: StoredRunRecord {
                        id: id.into(),
                        body: body.clone(),
                        parent_family: cluster.into(),
                        task_origin: TaskOrigin::TrustedRun,
                        execution_attestation: ExecutionAttestation::TrustedHost,
                        purpose: Purpose::Development,
                    },
                    trace: OptimizationTrace {
                        run_id: id.into(),
                        parent_family: cluster.into(),
                        source_digest: hash(&body),
                        purpose: Purpose::Development,
                        outcome: TraceOutcome::Success,
                        diagnosis: None,
                        excerpt: String::from_utf8(body.clone()).unwrap(),
                        seed: 1,
                    },
                    excerpt_start: 0,
                    excerpt_end: body.len(),
                },
            )
            .await
            .unwrap();
            let baseline_id = replay_world.manifest.baseline_observation_source_id.clone();
            let baseline_body = replay_baseline_observation_bytes(&replay_world.manifest).unwrap();
            store_trace_authority(
                &store,
                &Context::new(namespace, "host", Role::Host).unwrap(),
                &StoredTraceAuthority {
                    schema_version: "rsia.optimization.source.v1".into(),
                    record: StoredRunRecord {
                        id: baseline_id.clone(),
                        body: baseline_body.clone(),
                        parent_family: cluster.into(),
                        task_origin: TaskOrigin::TrustedRun,
                        execution_attestation: ExecutionAttestation::TrustedHost,
                        purpose: Purpose::Development,
                    },
                    trace: OptimizationTrace {
                        run_id: baseline_id,
                        parent_family: cluster.into(),
                        source_digest: hash(&baseline_body),
                        purpose: Purpose::Development,
                        outcome: TraceOutcome::Success,
                        diagnosis: None,
                        excerpt: String::from_utf8(baseline_body.clone()).unwrap(),
                        seed: 1,
                    },
                    excerpt_start: 0,
                    excerpt_end: baseline_body.len(),
                },
            )
            .await
            .unwrap();
        }
        let mut session = store.session().await.unwrap();
        session
            .put(
                &admin,
                "artifact",
                "source-1",
                admin.actor(),
                &economic_source_body(),
            )
            .await
            .unwrap();
        session
            .bump_watermark(&admin, "economic-initial-watermark")
            .await
            .unwrap();
        session.commit().await.unwrap();
        for world in [&train, &select] {
            let mut draft = world.clone();
            draft.sealed_digest = None;
            evo_storage::replay::put_replay_world_draft(&admin, &store, &draft)
                .await
                .unwrap();
            evo_storage::replay::seal_replay_world(&admin, &store, world)
                .await
                .unwrap();
        }
        let pool = evo_storage::replay::register_replay_pool(
            &admin,
            &store,
            &[
                train.manifest.world_id.clone(),
                select.manifest.world_id.clone(),
            ],
        )
        .await
        .unwrap();
        let policy = ElasticPolicyV1::default();
        let caps = ExplorationCapsV1::online();
        let profile = ReplaySimulationProfile {
            simulation_version: SIMULATION_VERSION.into(),
            objective: ReplayObjective::ParetoAttainmentV2,
            w_sim: 1,
            probe_budget: 1,
            horizon: 1,
            lambda_work_micros: DEFAULT_LAMBDA_MICROS,
            lambda_round_micros: DEFAULT_LAMBDA_MICROS,
            fixed_seed: 7,
            global_recovery_dispatch_limit: 1,
            pool_digest: pool.pool_digest.clone(),
            purpose: Purpose::Development,
            target_runtime_profile: "online-w1".into(),
        };
        let replay = run_replay(
            &select,
            &policy,
            &profile,
            &caps,
            &LiveWorldAuthority {
                revoke_watermark: 1,
                revoked_source_ids: BTreeSet::new(),
            },
        )
        .unwrap();
        let stored_report = StoredReplayReportV2::build(
            namespace,
            WorldPartition::Select,
            &pool,
            policy,
            profile,
            caps,
            vec![replay],
        )
        .unwrap();
        evo_storage::replay::put_current_replay_report(&admin, &store, &stored_report)
            .await
            .unwrap();
        let experiment = experiment(&stored_report);
        PersistentReplayEconomicCoordinator::register(&evaluator, &store, experiment.clone())
            .await
            .unwrap();
        (admin, evaluator, experiment)
    }

    #[tokio::test]
    async fn replay_economic_registration_limit_is_global_and_idempotent() {
        use super::{capacity, image};
        use evo_core::capacity::CapacityItem;
        let (directory, store) = super::database().await;
        let (_, a, first) = setup_in(&store, "a").await;
        let (_, b, other) = setup_in(&store, "b").await;
        for i in 1..98 {
            let mut next = first.clone();
            next.id = format!("economic-{i}");
            PersistentReplayEconomicCoordinator::register(&a, &store, next)
                .await
                .unwrap();
        }
        let mut last = other.clone();
        last.id = "economic-last".into();
        let last_digest = PersistentReplayEconomicCoordinator::register(&b, &store, last.clone())
            .await
            .unwrap();
        let before = image(directory.path());
        assert_eq!(
            PersistentReplayEconomicCoordinator::register(&b, &store, last)
                .await
                .unwrap(),
            last_digest
        );
        assert_eq!(image(directory.path()), before);
        let mut excess = first;
        excess.id = "economic-excess".into();
        capacity(
            PersistentReplayEconomicCoordinator::register(&a, &store, excess)
                .await
                .unwrap_err(),
            CapacityItem::ReplayEconomicExperiment,
            100,
            1,
        );
        assert_eq!(image(directory.path()), before);
    }
}

mod groups {
    use super::{capacity, image, trusted_compiler_inputs};
    use async_trait::async_trait;
    use evo_core::capacity::CapacityItem;
    use evo_core::contract::{HostCapabilities, ImproverPatch, Profile, SkillSnapshot};
    use evo_core::evidence::{
        EvidenceSet, ExecutionAttestation, Purpose, SourceSelection, TaskOrigin,
    };
    use evo_core::optimization::{
        ModelRequest, ModelRequestContext, ModelStage, OptimizationTrace, SkillFailureDiagnosis,
        SkillFailureKind, TraceOutcome, TrustedSourceBinding,
    };
    use evo_core::skill_edit::{
        EvidenceClosure, EvidenceRef, SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA,
        SkillEditBatch, TrustedEditContext, skill_snapshot_digest,
    };
    use evo_core::{Result, Role, Strategy, fingerprint, hash};
    use evo_engine::evidence::{
        StoredRunRecord, StoredTraceAuthority, ingest_trusted_run_records, store_source_selection,
        store_trace_authority,
    };
    use evo_engine::groups::{
        GroupOutcomeKind, SkillGroupJobRequest, SkillGroupSpec, run_skill_group_job,
    };
    use evo_engine::model::{
        ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelResponse,
    };
    use evo_engine::optimization::{
        BundleCompileContext, DevRunner, DevelopmentManifest, DevelopmentRunReport,
        DevelopmentRunRequest, DevelopmentTask, OptimizationJournal, OptimizationStepRequest,
    };
    use evo_storage::Store;
    use serde_json::Value;
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use trusted_compiler_inputs::StoreOptimizationJournal;
    const NS: &str = "tenant";
    #[derive(Clone)]
    struct Spec {
        tag: String,
        skill_id: String,
        skill_version: String,
        content: String,
        failure_run: String,
        success_run: String,
        namespace: String,
    }

    impl Spec {
        /// Group `group-<tag>` edits `skill-<tag>` in episode `episode-<tag>`. It
        /// reads its own failing run and `shared-success`, which every group reads.
        fn new(tag: &str, failure_run: &str) -> Self {
            Self {
                tag: tag.into(),
                skill_id: format!("skill-{tag}"),
                skill_version: "v1".into(),
                content: format!("old-{tag}"),
                failure_run: failure_run.into(),
                success_run: "shared-success".into(),
                namespace: NS.into(),
            }
        }
    }

    /// Everything one `OptimizationStepRequest` borrows, owned in one place.
    struct GroupFixture {
        group_id: String,
        episode_id: String,
        parent: SkillSnapshot,
        baseline: SkillSnapshot,
        selection: SourceSelection,
        records: Vec<StoredRunRecord>,
        evidence: EvidenceSet,
        bindings: Vec<TrustedSourceBinding>,
        traces: Vec<OptimizationTrace>,
        edit_context: TrustedEditContext,
        template: SkillEditBatch,
        profile: Profile,
        parent_strategy: Strategy,
        baseline_strategy: Strategy,
        improver_patch: ImproverPatch,
        caps: HostCapabilities,
        revoked: BTreeSet<String>,
        development_request: DevelopmentRunRequest,
        model_context: ModelRequestContext,
    }

    fn trace(run: &str, family: &str, outcome: TraceOutcome, skill_id: &str) -> OptimizationTrace {
        OptimizationTrace {
            run_id: run.into(),
            parent_family: family.into(),
            source_digest: hash(run.as_bytes()),
            purpose: Purpose::Development,
            outcome,
            diagnosis: (outcome == TraceOutcome::TaskFailure).then(|| SkillFailureDiagnosis {
                kind: SkillFailureKind::SkillDefect,
                skill_id: skill_id.into(),
                bundle_digest: hash(b"parent-bundle"),
                request_digest: hash(b"task-request"),
                rule_id: Some("rule-a".into()),
                support: vec![EvidenceRef {
                    id: run.into(),
                    digest: hash(run.as_bytes()),
                }],
                counterexamples: vec![],
                reason: "fixture defect".into(),
            }),
            excerpt: run.into(),
            seed: 1,
        }
    }

    fn build(spec: &Spec) -> GroupFixture {
        let tag = &spec.tag;
        let parent = SkillSnapshot {
            content: spec.content.clone(),
            applicability: "applies".into(),
            counterexample: "counter".into(),
            required_capabilities: vec![],
            dependencies: vec![],
        };
        let failure_family = format!("family-{tag}");
        let runs = [
            (
                spec.failure_run.as_str(),
                failure_family.as_str(),
                TraceOutcome::TaskFailure,
            ),
            (
                spec.success_run.as_str(),
                "family-shared",
                TraceOutcome::Success,
            ),
        ];
        let selection = SourceSelection {
            roots: vec![],
            run_ids: runs.iter().map(|(id, _, _)| (*id).into()).collect(),
            purpose: Purpose::Development,
            allow_model_excerpts: true,
        };
        let records: Vec<StoredRunRecord> = runs
            .iter()
            .map(|(id, family, _)| StoredRunRecord {
                id: (*id).into(),
                body: id.as_bytes().to_vec(),
                parent_family: (*family).into(),
                task_origin: TaskOrigin::TrustedRun,
                execution_attestation: ExecutionAttestation::TrustedHost,
                purpose: Purpose::Development,
            })
            .collect();
        let evidence = ingest_trusted_run_records(&selection, &records).unwrap();
        let bindings = runs
            .iter()
            .map(|(id, family, _)| TrustedSourceBinding {
                source_id: (*id).into(),
                source_digest: hash(id.as_bytes()),
                parent_family: (*family).into(),
            })
            .collect();
        let traces = runs
            .iter()
            .map(|(id, family, outcome)| trace(id, family, *outcome, &spec.skill_id))
            .collect();
        let allowed: Vec<EvidenceRef> = runs
            .iter()
            .map(|(id, _, _)| EvidenceRef {
                id: (*id).into(),
                digest: hash(id.as_bytes()),
            })
            .collect();
        let approved_parent_digest = hash(format!("parent-{}", spec.skill_id).as_bytes());
        let safe_baseline_digest = hash(format!("baseline-{}", spec.skill_id).as_bytes());
        let parent_digest = skill_snapshot_digest(&parent).unwrap();
        let parent_bundle_digest = hash(format!("parent-bundle-{tag}").as_bytes());
        let profile_id = format!("profile-{}", spec.skill_id);
        let edit_context = TrustedEditContext::new(
            spec.namespace.as_str(),
            profile_id.as_str(),
            spec.skill_id.as_str(),
            spec.skill_version.as_str(),
            approved_parent_digest.as_str(),
            safe_baseline_digest.as_str(),
            &parent,
            allowed.clone(),
        )
        .unwrap();
        let template = SkillEditBatch {
            schema_version: SKILL_EDIT_SCHEMA.into(),
            compiler_version: SKILL_EDIT_COMPILER_VERSION.into(),
            namespace: spec.namespace.clone(),
            profile_id: profile_id.clone(),
            skill_id: spec.skill_id.clone(),
            skill_version: spec.skill_version.clone(),
            input_digest: parent_digest.clone(),
            approved_parent_digest: approved_parent_digest.clone(),
            safe_baseline_digest: safe_baseline_digest.clone(),
            evidence: EvidenceClosure {
                support: vec![],
                counterexamples: vec![],
                dependencies: vec![],
            },
            edits: vec![],
        };
        let episode_id = format!("episode-{tag}");
        let development_request = DevelopmentRunRequest {
            request_id: format!("dev-request-{tag}"),
            namespace: spec.namespace.clone(),
            purpose: Purpose::Development,
            episode_id: episode_id.clone(),
            step: 1,
            attempt: 1,
            manifest: DevelopmentManifest::build(
                format!("manifest-{tag}"),
                vec![
                    DevelopmentTask {
                        id: format!("task-{tag}-1"),
                        parent_family: format!("family-{tag}"),
                        input_digest: hash(b"task-1"),
                    },
                    DevelopmentTask {
                        id: format!("task-{tag}-2"),
                        parent_family: "family-shared".into(),
                        input_digest: hash(b"task-2"),
                    },
                ],
            )
            .unwrap(),
            parent_bundle_digest: parent_bundle_digest.clone(),
            candidate_bundle_digest: hash(b"candidate-bundle"),
            environment_digest: hash(b"environment"),
            grader_digest: hash(b"grader"),
            rules_digest: hash(b"rules"),
            tools_digest: hash(b"tools"),
            revoke_watermark: 1,
            idempotency_key: format!("dev-idempotency-{tag}"),
        };
        let model_context = ModelRequestContext {
            request_id: format!("request-{tag}"),
            namespace: spec.namespace.clone(),
            purpose: Purpose::Development,
            stage: ModelStage::ReflectFailure,
            episode_id: episode_id.clone(),
            step: 1,
            attempt: 1,
            parent_skill_digest: parent_digest,
            bundle_digest: parent_bundle_digest,
            source_closure: allowed,
            model_digest: hash(b"fixture-model"),
            tools_digest: hash(b"tools"),
            rules_digest: hash(b"rules"),
            sampling_digest: hash(b"sampling"),
            revoke_watermark: 1,
            max_suggestions: 4,
        };
        GroupFixture {
            group_id: format!("group-{tag}"),
            episode_id,
            baseline: parent.clone(),
            parent,
            selection,
            records,
            evidence,
            bindings,
            traces,
            edit_context,
            template,
            profile: Profile {
                id: profile_id,
                evolution_enabled: true,
                parent_digest: approved_parent_digest,
                baseline_digest: safe_baseline_digest,
            },
            parent_strategy: Strategy::default(),
            baseline_strategy: Strategy::default(),
            improver_patch: ImproverPatch::default(),
            caps: HostCapabilities {
                available: BTreeSet::new(),
                granted: BTreeSet::new(),
            },
            revoked: BTreeSet::new(),
            development_request,
            model_context,
        }
    }

    impl GroupFixture {
        fn spec(&self) -> SkillGroupSpec<'_> {
            SkillGroupSpec {
                group_id: self.group_id.clone(),
                request: self.request(),
            }
        }

        fn request(&self) -> OptimizationStepRequest<'_> {
            trusted_compiler_inputs::request(OptimizationStepRequest {
                evidence: &self.evidence,
                source_selection: &self.selection,
                source_bindings: &self.bindings,
                traces: self.traces.clone(),
                model_context: self.model_context.clone(),
                parent_skill: &self.parent,
                edit_context: &self.edit_context,
                edit_batch_template: self.template.clone(),
                protected_ranges: &[],
                bundle_context: BundleCompileContext {
                    profile: &self.profile,
                    baseline: &self.baseline,
                    parent_strategy: &self.parent_strategy,
                    baseline_strategy: &self.baseline_strategy,
                    improver_patch: &self.improver_patch,
                    caps: &self.caps,
                    revoked: &self.revoked,
                },
                development_request: self.development_request.clone(),
                allow_rank_call: false,
            })
        }
    }

    async fn setup(
        groups: &[GroupFixture],
    ) -> (tempfile::TempDir, Store, StoreOptimizationJournal) {
        let (directory, store) = super::database().await;
        let host = super::context(NS, "host", Role::Host);
        let mut seen = BTreeSet::new();
        for group in groups {
            for (record, trace) in group.records.iter().zip(&group.traces) {
                if seen.insert(record.id.clone()) {
                    store_trace_authority(
                        &store,
                        &host,
                        &StoredTraceAuthority {
                            schema_version: "rsia.optimization.source.v1".into(),
                            record: record.clone(),
                            trace: trace.clone(),
                            excerpt_start: 0,
                            excerpt_end: record.body.len(),
                        },
                    )
                    .await
                    .unwrap();
                }
            }
            store_source_selection(&store, &host, &group.selection)
                .await
                .unwrap();
        }
        let mut session = store.session().await.unwrap();
        session
            .bump_watermark(&host, "groups-capacity")
            .await
            .unwrap();
        session.commit().await.unwrap();
        let journal = StoreOptimizationJournal::new(
            store.clone(),
            super::context(NS, "worker", Role::Worker),
            "worker",
        )
        .unwrap();
        (directory, store, journal)
    }

    #[derive(Default)]
    struct NoChangeModel(AtomicUsize);
    #[async_trait]
    impl ModelPort for NoChangeModel {
        async fn dispatch_compiler_bound(
            &self,
            request: evo_engine::model::ReadBoundModelRequest,
        ) -> Result<ModelResponse> {
            request.validate()?;
            assert_eq!(
                request.schema_version,
                evo_engine::model::CURRENT_BOUND_MODEL_SCHEMA
            );
            self.dispatch(request.request.clone()).await
        }
        async fn dispatch(&self, request: ModelRequest) -> Result<ModelResponse> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ModelResponse::Completed {
                request_id: request.request_id.clone(),
                response_id: format!("response-{}", request.request_id),
                actual_model_digest: request.model_digest,
                input_digest: request.input_digest,
                output: "[]".into(),
                output_digest: hash(b"[]"),
                execution_receipt: ModelExecutionReceipt {
                    call_id: format!("call-{}", request.request_id),
                    dispatch_id: format!("dispatch-{}", request.request_id),
                    root_budget_id: "fixture-budget".into(),
                    provider_request_id: format!("fixture-provider-{}", request.request_id),
                    usage_record_id: format!("fixture-usage-{}", request.request_id),
                    provenance: ModelExecutionProvenance::Fixture,
                },
            })
        }
    }
    struct UnusedRunner;
    #[async_trait]
    impl DevRunner for UnusedRunner {
        async fn run(&self, _request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
            panic!("no-suggestion software calls must not dispatch development")
        }
    }

    #[tokio::test]
    async fn two_group_job_limit_uses_the_core_authority() {
        let groups = [
            build(&Spec::new("a", "failure-a")),
            build(&Spec::new("b", "failure-b")),
            build(&Spec::new("c", "failure-c")),
        ];
        let (directory, _, journal) = setup(&groups).await;
        let model = NoChangeModel::default();
        let healthy = run_skill_group_job(
            Some(&model),
            Some(&UnusedRunner),
            Some(&journal),
            SkillGroupJobRequest {
                job_id: "two-groups".into(),
                groups: groups[..2].iter().map(GroupFixture::spec).collect(),
            },
        )
        .await
        .unwrap();
        assert_eq!(healthy.groups.len(), 2);
        assert!(
            healthy
                .groups
                .iter()
                .all(|outcome| outcome.kind == GroupOutcomeKind::NoChange)
        );
        let calls = model.0.load(Ordering::SeqCst);
        assert!(calls > 0);
        let before = image(directory.path());
        let error = run_skill_group_job(
            Some(&model),
            Some(&UnusedRunner),
            Some(&journal),
            SkillGroupJobRequest {
                job_id: "three-groups".into(),
                groups: groups.iter().map(GroupFixture::spec).collect(),
            },
        )
        .await
        .unwrap_err();
        capacity(error, CapacityItem::SkillGroupPerJob, 0, 3);
        assert_eq!(model.0.load(Ordering::SeqCst), calls);
        assert_eq!(image(directory.path()), before);
    }

    fn exploration_world(group: &GroupFixture) -> evo_engine::exploration::ExplorationWorldV1 {
        let request = group.request();
        let mut world = super::world();
        world.id = group.episode_id.clone();
        world.approved_parent_digest = request.bundle_context.profile.parent_digest.clone();
        world.parent_skill_digest = request.model_context.parent_skill_digest;
        world.parent_bundle_digest = request.model_context.bundle_digest;
        world.environment_digest = request.development_request.environment_digest;
        world.model_digest = request.model_context.model_digest;
        world.tools_digest = request.model_context.tools_digest;
        world.grader_digest = request.development_request.grader_digest;
        world.rules_digest = request.model_context.rules_digest;
        world.dependencies = group
            .selection
            .run_ids
            .iter()
            .map(|id| evo_engine::exploration::ExplorationDependency {
                kind: "run".into(),
                id: id.clone(),
            })
            .collect();
        world.context_signature = fingerprint(&(
            &world.parent_skill_digest,
            &world.parent_bundle_digest,
            &world.environment_digest,
            &world.model_digest,
            &world.tools_digest,
            &world.grader_digest,
            &world.rules_digest,
            world.source_watermark,
        ))
        .unwrap();
        world
    }
    struct HangingModel(tokio::sync::Notify);
    #[async_trait]
    impl ModelPort for HangingModel {
        async fn dispatch_compiler_bound(
            &self,
            request: evo_engine::model::ReadBoundModelRequest,
        ) -> Result<ModelResponse> {
            request.validate()?;
            self.dispatch(request.request.clone()).await
        }
        async fn dispatch(&self, _request: ModelRequest) -> Result<ModelResponse> {
            self.0.notify_one();
            std::future::pending().await
        }
    }

    #[tokio::test]
    async fn claimed_node_intents_cannot_oversubscribe_the_node_limit() {
        use evo_engine::exploration::PersistentCoordinator;
        let groups = [
            build(&Spec::new("healthy", "failure-healthy")),
            build(&Spec::new("held", "failure-held")),
            build(&Spec::new("denied", "failure-denied")),
        ];
        let (directory, store, journal) = setup(&groups).await;
        let worker = super::context(NS, "worker", Role::Worker);
        let coordinator =
            PersistentCoordinator::new(store.clone(), worker.clone(), "worker").unwrap();
        for group in &groups {
            coordinator
                .register_world(exploration_world(group))
                .await
                .unwrap();
        }
        let model = NoChangeModel::default();
        let first = coordinator
            .run_next(
                Some(&model),
                Some(&UnusedRunner),
                Some(&journal),
                groups[0].request(),
            )
            .await
            .unwrap();
        let node_id = first.node_id.unwrap();
        let mut session = store.session().await.unwrap();
        let original: Value = session
            .need(
                &worker,
                "artifact",
                &format!(
                    "e09-{}",
                    fingerprint(&("exploration_node_v1", node_id)).unwrap()
                ),
            )
            .await
            .unwrap();
        let other = super::context("other", "worker", Role::Worker);
        for i in 0..498u64 {
            let mut row = original.clone();
            row["payload"]["world_id"] = serde_json::json!("seed-world");
            row["payload"]["node"]["node_seq"] = serde_json::json!(i);
            let id = format!(
                "e09-{}",
                fingerprint(&("exploration_node_v1", format!("node-seed-world-{i}"))).unwrap()
            );
            row["id"] = serde_json::json!(id);
            session
                .put(&other, "artifact", &id, other.actor(), &row)
                .await
                .unwrap();
        }
        assert_eq!(
            session
                .capacity_used(CapacityItem::ExplorationNode)
                .await
                .unwrap(),
            499
        );
        session.commit().await.unwrap();
        let hanging = HangingModel(tokio::sync::Notify::new());
        let held = coordinator.run_next(
            Some(&hanging),
            Some(&UnusedRunner),
            Some(&journal),
            groups[1].request(),
        );
        tokio::pin!(held);
        tokio::select! { _=hanging.0.notified()=>{}, value=&mut held=>panic!("claim must reach the real model boundary before the second attempt: {value:?}") }
        let before = image(directory.path());
        capacity(
            coordinator
                .run_next(
                    Some(&model),
                    Some(&UnusedRunner),
                    Some(&journal),
                    groups[2].request(),
                )
                .await
                .unwrap_err(),
            CapacityItem::ExplorationNode,
            500,
            1,
        );
        assert_eq!(image(directory.path()), before);
    }
}

mod monitoring {
    use super::trusted_compiler_inputs;
    use async_trait::async_trait;
    use evo_core::Context;
    use evo_core::Error;
    use evo_core::Role;
    use evo_core::Strategy;
    use evo_core::contract::CapabilityLevel;
    use evo_core::contract::HostCapabilities;
    use evo_core::contract::HostSurfaceManifest;
    use evo_core::contract::ImproverPatch;
    use evo_core::contract::Profile;
    use evo_core::contract::SkillSnapshot;
    use evo_core::contract::SurfaceCoverage;
    use evo_core::contract::SurfaceItem;
    use evo_core::contract::SystemSnapshotV3;
    use evo_core::evidence::EvidenceSet;
    use evo_core::evidence::ExecutionAttestation;
    use evo_core::evidence::Purpose;
    use evo_core::evidence::SourceSelection;
    use evo_core::evidence::TaskOrigin;
    use evo_core::fingerprint;
    use evo_core::hash;
    use evo_core::optimization::EditSuggestion;
    use evo_core::optimization::ModelRequest;
    use evo_core::optimization::ModelRequestContext;
    use evo_core::optimization::ModelStage;
    use evo_core::optimization::OptimizationTrace;
    use evo_core::optimization::SkillFailureDiagnosis;
    use evo_core::optimization::SkillFailureKind;
    use evo_core::optimization::TraceOutcome;
    use evo_core::optimization::TrustedSourceBinding;
    use evo_core::skill_edit::EvidenceClosure;
    use evo_core::skill_edit::EvidenceRef;
    use evo_core::skill_edit::SKILL_EDIT_COMPILER_VERSION;
    use evo_core::skill_edit::SKILL_EDIT_SCHEMA;
    use evo_core::skill_edit::SkillEditBatch;
    use evo_core::skill_edit::SkillTextEdit;
    use evo_core::skill_edit::SkillTextField;
    use evo_core::skill_edit::TextEditOperation;
    use evo_core::skill_edit::TrustedEditContext;
    use evo_core::skill_edit::skill_snapshot_digest;
    use evo_engine::broker::BudgetPortBinding;
    use evo_engine::evidence::StoredRunRecord;
    use evo_engine::evidence::StoredTraceAuthority;
    use evo_engine::evidence::ingest_trusted_run_records;
    use evo_engine::evidence::store_source_selection;
    use evo_engine::evidence::store_trace_authority;
    use evo_engine::model::ModelExecutionProvenance;
    use evo_engine::model::ModelExecutionReceipt;
    use evo_engine::model::ModelPort;
    use evo_engine::model::ModelResponse;
    use evo_engine::monitoring::CONSOLIDATION_STAGING_SCHEMA;
    use evo_engine::monitoring::ClaimOutcome;
    use evo_engine::monitoring::CompleteDevelopmentCycleRequest;
    use evo_engine::monitoring::ConsolidationClaim;
    use evo_engine::monitoring::ConsolidationDevRunner;
    use evo_engine::monitoring::ConsolidationModelPort;
    use evo_engine::monitoring::ConsolidationRunOutcome;
    use evo_engine::monitoring::ConsolidationRunRecord;
    use evo_engine::monitoring::CurrentConsolidationProposal as ConsolidationProposal;
    use evo_engine::monitoring::DevelopmentCycleRecord;
    use evo_engine::monitoring::EnvironmentEvidenceScope;
    use evo_engine::monitoring::MonitoringCoordinator;
    use evo_engine::monitoring::RecordEnvironmentRequest;
    use evo_engine::monitoring::RecordedEnvironment;
    use evo_engine::monitoring::StagedCurrentConsolidationCandidate as StagedConsolidationCandidate;
    use evo_engine::optimization::BundleCompileContext;
    use evo_engine::optimization::DevRunner;
    use evo_engine::optimization::DevelopmentExecutionProvenance;
    use evo_engine::optimization::DevelopmentManifest;
    use evo_engine::optimization::DevelopmentRunReport;
    use evo_engine::optimization::DevelopmentRunRequest;
    use evo_engine::optimization::DevelopmentTask;
    use evo_engine::optimization::OptimizationJournal;
    use evo_engine::optimization::OptimizationJournalStage;
    use evo_engine::optimization::OptimizationStepRequest;
    use evo_engine::optimization::PairedTaskResult;
    use evo_engine::optimization::StageDependency;
    use evo_engine::optimization::StageFact;
    use evo_engine::optimization::StageFactKind;
    use evo_engine::release_store::CompletePrepareRunRequest;
    use evo_engine::release_store::ReleaseStore;
    use evo_engine::release_store::TrustedHostExecutionEvidence;
    use evo_storage::Store;
    use evo_storage::budget::BudgetCallFence;
    use evo_storage::budget::BudgetCallReservation;
    use evo_storage::budget::BudgetStage;
    use evo_storage::budget::RootBudgetAuthorization;
    use evo_storage::budget::RootBudgetRecord;
    use evo_storage::budget::UsageCharge;
    use serde_json::Value;
    use serde_json::json;
    use std::collections::BTreeSet;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use trusted_compiler_inputs::StoreOptimizationJournal;

    const TENANT: &str = "tenant";
    const SCOPE: &str = "billing-scope";
    const ROOT: &str = "root-budget";

    fn d(value: &str) -> String {
        hash(value.as_bytes())
    }

    fn context(actor: &str, role: Role) -> Context {
        Context::new(TENANT, actor, role).unwrap()
    }

    fn surface() -> HostSurfaceManifest {
        HostSurfaceManifest {
            schema_version: "rsia.host_surface.v1".into(),
            host: "monitor-host".into(),
            host_version: "1.0.0".into(),
            adapter_version: "adapter-v1".into(),
            source_digest: d("surface-source"),
            items: vec![SurfaceItem {
                name: "model".into(),
                coverage: SurfaceCoverage::Supported,
                mapped_field: Some("host.model".into()),
                consumer: Some("runner".into()),
                reason: "fixture projection".into(),
            }],
        }
    }

    fn system(model: &str) -> SystemSnapshotV3 {
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
        SystemSnapshotV3 {
            schema_version: "rsia.system_snapshot.v3".into(),
            profile_id: "profile".into(),
            host_id: "monitor-host".into(),
            host_version: "1.0.0".into(),
            model_id: model.into(),
            tools: source.names().unwrap(),
            tool_source: source,
            mandatory_context_digest: d("mandatory"),
        }
    }

    /// A trusted Host development source. `source-1` is a diagnosed failure and
    /// `source-2` a success, so the optimizer has a failure and a success batch.
    fn authority(id: &str, body: &str) -> StoredTraceAuthority {
        let failing = id == "source-1";
        StoredTraceAuthority {
            schema_version: "rsia.optimization.source.v1".into(),
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
                outcome: if failing {
                    TraceOutcome::TaskFailure
                } else {
                    TraceOutcome::Success
                },
                diagnosis: failing.then(|| SkillFailureDiagnosis {
                    kind: SkillFailureKind::SkillDefect,
                    skill_id: "skill-a".into(),
                    bundle_digest: d("parent-bundle"),
                    request_digest: d("task-request"),
                    rule_id: Some("rule-a".into()),
                    support: vec![EvidenceRef {
                        id: id.into(),
                        digest: hash(body.as_bytes()),
                    }],
                    counterexamples: vec![],
                    reason: "fixture defect".into(),
                }),
                excerpt: body.into(),
                seed: 1,
            },
            excerpt_start: 0,
            excerpt_end: body.len(),
        }
    }

    /// One paired task of a staged development report.
    #[derive(Clone)]
    struct ResultSpec {
        task_id: &'static str,
        parent_score: u32,
        candidate_score: u32,
        parent_passed: bool,
        candidate_passed: bool,
    }

    /// Both pass, score 500k -> 600k: a contrast made of a score change only.
    fn score_only_contrast() -> Vec<ResultSpec> {
        vec![ResultSpec {
            task_id: "task-a",
            parent_score: 500_000,
            candidate_score: 600_000,
            parent_passed: true,
            candidate_passed: true,
        }]
    }

    /// Identical before and after: no contrast at all.
    fn no_contrast() -> Vec<ResultSpec> {
        vec![ResultSpec {
            task_id: "task-a",
            parent_score: 500_000,
            candidate_score: 500_000,
            parent_passed: true,
            candidate_passed: true,
        }]
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        store: Store,
        admin: Context,
        host: Context,
        worker: Context,
        environment: RecordedEnvironment,
        authorities: Vec<StoredTraceAuthority>,
        manifest: DevelopmentManifest,
        parent_skill: SkillSnapshot,
        manifest_digest: String,
        grader_digest: String,
        strategy_digest: String,
        parent_skill_digest: String,
        parent_bundle_digest: String,
        budget: RootBudgetRecord,
    }

    impl Fixture {
        async fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let database = dir.path().join("consolidation.sqlite3");
            let store = Store::open(&database).await.unwrap();
            let admin = context("admin", Role::Admin);
            let host = context("host", Role::Host);
            let worker = context("worker", Role::Worker);
            trusted_compiler_inputs::register_fixture_host_surface(
                &admin,
                &store,
                "surface",
                surface(),
                vec!["model".into()],
            )
            .await
            .unwrap();
            prepare_host_run(&host, &store, "host-run-1", "model-v1", "task-input").await;
            let authorities = vec![
                authority("source-1", "first development source"),
                authority("source-2", "second development source"),
            ];
            for source in &authorities {
                store_trace_authority(&store, &host, source).await.unwrap();
            }
            let mut session = store.session().await.unwrap();
            assert_eq!(
                session
                    .bump_watermark(&host, &d("initial-watermark"))
                    .await
                    .unwrap(),
                1
            );
            session.commit().await.unwrap();
            let manifest = DevelopmentManifest::build(
                "development-manifest",
                vec![DevelopmentTask {
                    id: "task-a".into(),
                    parent_family: "development-family".into(),
                    input_digest: d("development-task-input"),
                }],
            )
            .unwrap();
            let manifest_digest = manifest.digest.clone();
            let grader_digest = d("development-grader");
            let strategy_digest = fingerprint(&Strategy::default()).unwrap();
            let parent_skill = SkillSnapshot {
                content: "parent rule".into(),
                applicability: "development only".into(),
                counterexample: "counterexample".into(),
                required_capabilities: vec![],
                dependencies: vec![],
            };
            let environment = MonitoringCoordinator::record_environment_from_run(
                &host,
                &store,
                RecordEnvironmentRequest {
                    run_id: "host-run-1".into(),
                    development_manifest_digest: manifest_digest.clone(),
                    grader_digest: grader_digest.clone(),
                    generation_strategy_digest: strategy_digest.clone(),
                    revoke_watermark: 1,
                },
            )
            .await
            .unwrap();
            assert_eq!(
                environment.environment.evidence_scope,
                EnvironmentEvidenceScope::ProgramFixture
            );
            let budget = store
                .authorize_root_budget(
                    &admin,
                    &RootBudgetAuthorization {
                        root_budget_id: ROOT.into(),
                        billing_scope: SCOPE.into(),
                        allowed_namespaces: vec![TENANT.into()],
                        currency: "USD".into(),
                        pricing_version: "pricing-v1".into(),
                        payment_subject: "payer".into(),
                        authorization_receipt_digest: d("authorization"),
                        per_call_cap_micros: 100,
                        total_limit_micros: 1_000,
                        created_at: 1,
                    },
                )
                .await
                .unwrap();
            Self {
                _dir: dir,
                store,
                admin,
                host,
                worker,
                environment,
                authorities,
                manifest,
                parent_skill: parent_skill.clone(),
                manifest_digest,
                grader_digest,
                strategy_digest,
                parent_skill_digest: skill_snapshot_digest(&parent_skill).unwrap(),
                parent_bundle_digest: d("parent-bundle"),
                budget,
            }
        }

        fn cycle_request(&self, fact_id: String) -> CompleteDevelopmentCycleRequest {
            CompleteDevelopmentCycleRequest {
                skill_id: "skill-a".into(),
                profile_id: "profile".into(),
                environment_id: self.environment.environment.id.clone(),
                development_manifest_digest: self.manifest_digest.clone(),
                grader_digest: self.grader_digest.clone(),
                generation_strategy_digest: self.strategy_digest.clone(),
                parent_skill_digest: self.parent_skill_digest.clone(),
                parent_bundle_digest: self.parent_bundle_digest.clone(),
                billing_scope: self.budget.billing_scope.clone(),
                root_budget_id: self.budget.root_budget_id.clone(),
                report_fact_id: fact_id,
                source_ids: vec!["source-2".into(), "source-1".into()],
                revoke_watermark: 1,
            }
        }

        /// Stages a development report for `cycle` and closes it as a cycle.
        async fn close_cycle(
            &self,
            cycle: u32,
            results: &[ResultSpec],
        ) -> Result<DevelopmentCycleRecord, Error> {
            let fact = self.stage_report(cycle, results).await;
            MonitoringCoordinator::close_development_cycle(
                &self.worker,
                &self.store,
                self.cycle_request(fact),
            )
            .await
        }

        /// Closes two cycles with a contrast and claims the first generation.
        async fn claimed(&self) -> ConsolidationClaim {
            for cycle in 1..=2 {
                self.close_cycle(cycle, &score_only_contrast())
                    .await
                    .unwrap();
            }
            self.claim_generation().await
        }

        async fn claim_generation(&self) -> ConsolidationClaim {
            let scope_id = self.scope_id().await;
            let ClaimOutcome::Claimed(claim) =
                MonitoringCoordinator::claim_consolidation(&self.worker, &self.store, &scope_id)
                    .await
                    .unwrap()
            else {
                panic!("the next consolidation generation was not claimed")
            };
            claim
        }

        async fn scope_id(&self) -> String {
            let mut session = self.store.session().await.unwrap();
            let scopes: Vec<Value> = session.list(&self.worker, "artifact").await.unwrap();
            session.commit().await.unwrap();
            let ids: Vec<_> = scopes
                .iter()
                .filter(|value| value["schema_version"] == "rsia.monitoring.consolidation_scope.v1")
                .map(|value| value["id"].as_str().unwrap().to_string())
                .collect();
            assert_eq!(ids.len(), 1, "exactly one consolidation scope");
            ids[0].clone()
        }

        /// Stages a DevelopmentObserved fact whose report pairs `results` for one
        /// cycle, with the fixture receipts the stage fact depends on.
        async fn stage_report(&self, cycle: u32, results: &[ResultSpec]) -> String {
            let runner_execution = format!("runner-execution-{cycle}");
            let mut paired = Vec::new();
            let mut dependencies = Vec::new();
            let mut receipts = vec![("execution", runner_execution.clone())];
            for (index, spec) in results.iter().enumerate() {
                let parent_execution = format!("parent-execution-{cycle}-{index}");
                let candidate_execution = format!("candidate-execution-{cycle}-{index}");
                let grader_receipt = d(&format!("grader-receipt-{cycle}-{index}"));
                receipts.push(("execution", parent_execution.clone()));
                receipts.push(("execution", candidate_execution.clone()));
                receipts.push(("grader", grader_receipt.clone()));
                dependencies.push(StageDependency {
                    kind: "execution".into(),
                    id: parent_execution.clone(),
                });
                dependencies.push(StageDependency {
                    kind: "execution".into(),
                    id: candidate_execution.clone(),
                });
                dependencies.push(StageDependency {
                    kind: "grader".into(),
                    id: grader_receipt.clone(),
                });
                paired.push(PairedTaskResult {
                    task_id: spec.task_id.into(),
                    parent_score_micros: spec.parent_score,
                    candidate_score_micros: spec.candidate_score,
                    parent_passed: spec.parent_passed,
                    candidate_passed: spec.candidate_passed,
                    parent_execution_id: parent_execution,
                    candidate_execution_id: candidate_execution,
                    grader_receipt_digest: grader_receipt,
                });
            }
            let report = DevelopmentRunReport {
                request_id: format!("development-request-{cycle}"),
                manifest_digest: self.manifest_digest.clone(),
                parent_bundle_digest: self.parent_bundle_digest.clone(),
                candidate_bundle_digest: d(&format!("candidate-bundle-{cycle}")),
                environment_digest: self.environment.environment.environment_digest.clone(),
                grader_digest: self.grader_digest.clone(),
                results: paired,
                execution_receipt_id: runner_execution,
                usage_record_ids: vec![],
                provenance: DevelopmentExecutionProvenance::Fixture,
            };
            let mut session = self.store.session().await.unwrap();
            for (logical_kind, id) in &receipts {
                session
                .put(
                    &self.worker,
                    "artifact",
                    id,
                    self.worker.actor(),
                    &json!({"id":id,"schema_version":"rsia.development_receipt.fixture.v1","logical_kind":logical_kind,"cycle":cycle,"fixture":true}),
                )
                .await
                .unwrap();
            }
            session.commit().await.unwrap();
            for source in ["source-1", "source-2"] {
                dependencies.push(StageDependency {
                    kind: "run".into(),
                    id: source.into(),
                });
            }
            dependencies.push(StageDependency {
                kind: "revoke_watermark".into(),
                id: "1".into(),
            });
            let fact = StageFact {
                schema_version: "rsia.optimization.stage_fact.v1".into(),
                artifact_id: "placeholder".into(),
                namespace: TENANT.into(),
                episode_id: format!("development-episode-{cycle}"),
                step: cycle,
                attempt: 1,
                stage: OptimizationJournalStage::Development,
                kind: StageFactKind::DevelopmentObserved,
                request_id: report.request_id.clone(),
                input_digest: d(&format!("development-input-{cycle}")),
                output_digest: None,
                dependencies,
                payload: serde_json::to_value(&report).unwrap(),
            }
            .seal()
            .unwrap();
            let id = fact.artifact_id.clone();
            self.journal().commit(fact).await.unwrap();
            id
        }

        fn journal(&self) -> StoreOptimizationJournal {
            StoreOptimizationJournal::new(self.store.clone(), self.worker.clone(), "worker")
                .unwrap()
        }
    }

    async fn prepare_host_run(
        host: &Context,
        store: &Store,
        run_id: &str,
        model: &str,
        task: &str,
    ) {
        let snapshot = ReleaseStore::prepare_complete_run(
            host,
            store,
            CompletePrepareRunRequest {
                run_id: run_id.into(),
                profile_id: "profile".into(),
                system_snapshot: system(model),
                host_surface_id: "surface".into(),
                host_capabilities: HostCapabilities {
                    available: BTreeSet::new(),
                    granted: BTreeSet::new(),
                },
                task_input_digest: d(task),
                evolution_enabled: true,
                capability_level: CapabilityLevel::ToolOnly,
            },
        )
        .await
        .unwrap();
        ReleaseStore::record_applied_request(
            host,
            store,
            run_id,
            TrustedHostExecutionEvidence {
                actual_request_material: snapshot.request_material.clone(),
                environment_digest: snapshot.environment_digest.clone(),
                host_surface_digest: snapshot.host_surface_digest.clone(),
                host_capabilities_digest: snapshot.host_capabilities_digest.clone(),
                offered: vec![],
                attached: vec![],
                used: vec![],
                execution_receipt_id: None,
                truncated: false,
            },
        )
        .await
        .unwrap();
    }

    /// Model fixture from `tests/optimization.rs::EditingFixtureModelPort`: one
    /// bounded insertion per reflection batch. `tag` makes the inserted text, and
    /// therefore the candidate bundle, differ between instances; the receipt names
    /// the claim's root budget so the root-binding wrapper accepts it.
    struct EditingModelPort {
        tag: &'static str,
        calls: AtomicUsize,
    }

    impl EditingModelPort {
        fn new(tag: &'static str) -> Self {
            Self {
                tag,
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl ModelPort for EditingModelPort {
        // This is an explicit deterministic fixture adapter. Its receipts retain
        // Fixture provenance; the production broker verifies the full use itself.
        async fn dispatch_bound(
            &self,
            request: evo_engine::model::BoundModelRequest,
        ) -> evo_core::Result<evo_engine::model::ModelResponse> {
            request.validate()?;
            self.dispatch(request.request).await
        }

        // Explicit software fixture support; the caller's real Current journal
        // verifies authority. This adapter retains the original scripted response
        // and Fixture provenance, and supplies no provider or OS authority.
        async fn dispatch_compiler_bound(
            &self,
            request: evo_engine::model::ReadBoundModelRequest,
        ) -> evo_core::Result<evo_engine::model::ModelResponse> {
            request.validate()?;
            if request.schema_version != evo_engine::model::CURRENT_BOUND_MODEL_SCHEMA {
                return Err(evo_core::Error::Forbidden);
            }
            self.dispatch(request.request.clone()).await
        }

        async fn dispatch(&self, request: ModelRequest) -> evo_core::Result<ModelResponse> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let source = request.source_closure[0].clone();
            let parent: SkillSnapshot = serde_json::from_str(
                &request
                    .input
                    .iter()
                    .find(|part| part.label == "parent-skill")
                    .ok_or_else(|| Error::Invalid("fixture parent input missing".into()))?
                    .content,
            )
            .map_err(|_| Error::Invalid("fixture parent input invalid".into()))?;
            let strategy: Strategy = serde_json::from_str(
                &request
                    .input
                    .iter()
                    .find(|part| part.label == "generation-strategy")
                    .ok_or_else(|| Error::Invalid("fixture strategy input missing".into()))?
                    .content,
            )
            .map_err(|_| Error::Invalid("fixture strategy input invalid".into()))?;
            // A consolidation step makes every model call under `Consolidate`, so the
            // reflect stage is read from the batch the call carries: its evidence part
            // is labelled with the batch id.
            let stage = match request.stage {
                ModelStage::Consolidate => request
                    .input
                    .iter()
                    .find_map(|part| match part.label.as_str() {
                        "reflection-failure" => Some(ModelStage::ReflectFailure),
                        "reflection-success" => Some(ModelStage::ReflectSuccess),
                        _ => None,
                    })
                    .ok_or_else(|| Error::Invalid("unexpected fixture batch".into()))?,
                other => other,
            };
            let (id, batch_id, field, start) = match stage {
                ModelStage::ReflectFailure => (
                    "fix-rule",
                    "reflection-failure",
                    SkillTextField::Content,
                    parent.content.len(),
                ),
                ModelStage::ReflectSuccess => (
                    "preserve-rule",
                    "reflection-success",
                    SkillTextField::Applicability,
                    parent.applicability.len(),
                ),
                _ => return Err(Error::Invalid("unexpected fixture stage".into())),
            };
            let suggestion = EditSuggestion {
                id: id.into(),
                hypothesis: "bounded fixture hypothesis".into(),
                batch_ids: vec![batch_id.into()],
                support: vec![source.clone()],
                counterexamples: if stage == ModelStage::ReflectSuccess {
                    vec![source.clone()]
                } else {
                    vec![]
                },
                dependencies: vec![source],
                edit: SkillTextEdit {
                    field,
                    start,
                    end: start,
                    expected_text_digest: hash(b""),
                    exact_anchor: None,
                    operation: TextEditOperation::Insert {
                        text: format!(" [{} {}]", strategy.instruction, self.tag),
                    },
                },
            };
            let output = serde_json::to_string(&vec![suggestion]).unwrap();
            Ok(ModelResponse::Completed {
                request_id: request.request_id,
                response_id: format!("fixture-response-{id}"),
                actual_model_digest: request.model_digest,
                input_digest: request.input_digest,
                output_digest: hash(output.as_bytes()),
                output,
                execution_receipt: ModelExecutionReceipt {
                    call_id: format!("fixture-call-{id}"),
                    dispatch_id: format!("fixture-dispatch-{id}"),
                    root_budget_id: ROOT.into(),
                    provider_request_id: format!("fixture-provider-{id}"),
                    usage_record_id: format!("fixture-usage-{id}"),
                    provenance: ModelExecutionProvenance::Fixture,
                },
            })
        }
    }

    #[async_trait]
    impl ConsolidationModelPort for EditingModelPort {
        async fn trusted_budget_binding(
            &self,
            _namespace: &str,
        ) -> evo_core::Result<BudgetPortBinding> {
            Ok(BudgetPortBinding {
                billing_scope: SCOPE.into(),
                root_budget_id: ROOT.into(),
            })
        }
    }

    /// Development runner that spends through the real root ledger under the claim's
    /// dispatch group, as the root-binding wrapper requires, and reports either a
    /// strict improvement or a regression over the frozen manifest.
    struct LedgerDevRunner {
        store: Store,
        worker: Context,
        improves: bool,
        calls: AtomicUsize,
    }

    impl LedgerDevRunner {
        fn new(fixture: &Fixture, improves: bool) -> Self {
            Self {
                store: fixture.store.clone(),
                worker: fixture.worker.clone(),
                improves,
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl DevRunner for LedgerDevRunner {
        async fn run(
            &self,
            request: DevelopmentRunRequest,
        ) -> evo_core::Result<DevelopmentRunReport> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let call_id = format!("dev-call-{}", request.request_id);
            let input = d(&request.request_id);
            let lease = format!("lease-{}", request.request_id);
            let call = self
                .store
                .reserve_budget_call(
                    &self.worker,
                    &BudgetCallReservation {
                        billing_scope: SCOPE.into(),
                        call_id: call_id.clone(),
                        dispatch_group_id: request.episode_id.clone(),
                        stage: BudgetStage::DevelopmentExecution,
                        actual_input_digest: input.clone(),
                        request_artifact: None,
                        max_cost_micros: 10,
                        lease_token: lease.clone(),
                        lease_until: 10_000,
                        now: 100,
                    },
                )
                .await?;
            let fence = BudgetCallFence {
                billing_scope: SCOPE.into(),
                call_id,
                actual_input_digest: input,
                lease_token: lease,
                lease_epoch: call.lease_epoch,
                now: 101,
            };
            let dispatched = self
                .store
                .begin_budget_dispatch(&self.worker, &fence)
                .await?;
            let usage = format!("usage-{}", request.request_id);
            self.store
                .finalize_budget_call(
                    &self.worker,
                    &fence,
                    &UsageCharge {
                        amount_micros: 10,
                        currency: "USD".into(),
                        pricing_version: "pricing-v1".into(),
                        provider_request_id: "fixture-runner".into(),
                        usage_record_id: usage.clone(),
                        output_digest: d("development-output"),
                    },
                )
                .await?;
            // The root admits one open execution at a time: close this one so the
            // next claim's run can dispatch.
            self.store
                .close_budget_call_execution(
                    &self.worker,
                    SCOPE,
                    &fence.call_id,
                    dispatched.call.dispatch_id.as_deref().unwrap(),
                    "fixture_execution_closed",
                    102,
                )
                .await?;
            Ok(DevelopmentRunReport {
                request_id: request.request_id.clone(),
                manifest_digest: request.manifest.digest.clone(),
                parent_bundle_digest: request.parent_bundle_digest.clone(),
                candidate_bundle_digest: request.candidate_bundle_digest.clone(),
                environment_digest: request.environment_digest.clone(),
                grader_digest: request.grader_digest.clone(),
                results: request
                    .manifest
                    .tasks
                    .iter()
                    .map(|task| PairedTaskResult {
                        task_id: task.id.clone(),
                        parent_score_micros: 500_000,
                        candidate_score_micros: if self.improves { 700_000 } else { 400_000 },
                        parent_passed: true,
                        candidate_passed: true,
                        parent_execution_id: format!("parent-{}", task.id),
                        candidate_execution_id: format!("candidate-{}", task.id),
                        grader_receipt_digest: d(&format!("grade-{}", task.id)),
                    })
                    .collect(),
                execution_receipt_id: "development-runner-execution".into(),
                usage_record_ids: vec![usage],
                provenance: DevelopmentExecutionProvenance::Fixture,
            })
        }
    }

    #[async_trait]
    impl ConsolidationDevRunner for LedgerDevRunner {
        async fn trusted_budget_binding(
            &self,
            _namespace: &str,
        ) -> evo_core::Result<BudgetPortBinding> {
            Ok(BudgetPortBinding {
                billing_scope: SCOPE.into(),
                root_budget_id: ROOT.into(),
            })
        }
    }

    /// Everything an `OptimizationStepRequest` borrows, frozen once per fixture.
    struct StepMaterial {
        selection: SourceSelection,
        evidence: EvidenceSet,
        bindings: Vec<TrustedSourceBinding>,
        allowed: Vec<EvidenceRef>,
        edit_context: TrustedEditContext,
        edit_template: SkillEditBatch,
        profile: Profile,
        baseline: SkillSnapshot,
        strategy: Strategy,
        patch: ImproverPatch,
        caps: HostCapabilities,
        revoked: BTreeSet<String>,
    }

    impl StepMaterial {
        async fn new(fixture: &Fixture) -> Self {
            let selection = SourceSelection {
                roots: vec![],
                run_ids: vec!["source-1".into(), "source-2".into()],
                purpose: Purpose::Development,
                allow_model_excerpts: true,
            };
            store_source_selection(&fixture.store, &fixture.host, &selection)
                .await
                .unwrap();
            let records = fixture
                .authorities
                .iter()
                .map(|authority| authority.record.clone())
                .collect::<Vec<_>>();
            let evidence = ingest_trusted_run_records(&selection, &records).unwrap();
            let bindings = fixture
                .authorities
                .iter()
                .map(|authority| TrustedSourceBinding {
                    source_id: authority.record.id.clone(),
                    source_digest: authority.trace.source_digest.clone(),
                    parent_family: authority.record.parent_family.clone(),
                })
                .collect::<Vec<_>>();
            let allowed = fixture
                .authorities
                .iter()
                .map(|authority| EvidenceRef {
                    id: authority.record.id.clone(),
                    digest: authority.trace.source_digest.clone(),
                })
                .collect::<Vec<_>>();
            let approved_parent = d("approved-parent");
            let safe_baseline = d("safe-baseline");
            let edit_context = TrustedEditContext::new(
                TENANT,
                "profile",
                "skill-a",
                "v1",
                approved_parent.clone(),
                safe_baseline.clone(),
                &fixture.parent_skill,
                allowed.clone(),
            )
            .unwrap();
            let edit_template = SkillEditBatch {
                schema_version: SKILL_EDIT_SCHEMA.into(),
                compiler_version: SKILL_EDIT_COMPILER_VERSION.into(),
                namespace: TENANT.into(),
                profile_id: "profile".into(),
                skill_id: "skill-a".into(),
                skill_version: "v1".into(),
                input_digest: skill_snapshot_digest(&fixture.parent_skill).unwrap(),
                approved_parent_digest: approved_parent.clone(),
                safe_baseline_digest: safe_baseline.clone(),
                evidence: EvidenceClosure {
                    support: vec![allowed[0].clone()],
                    counterexamples: vec![allowed[1].clone()],
                    dependencies: allowed.clone(),
                },
                edits: vec![],
            };
            Self {
                selection,
                evidence,
                bindings,
                allowed,
                edit_context,
                edit_template,
                profile: Profile {
                    id: "profile".into(),
                    evolution_enabled: true,
                    parent_digest: approved_parent,
                    baseline_digest: safe_baseline,
                },
                baseline: fixture.parent_skill.clone(),
                strategy: Strategy::default(),
                patch: ImproverPatch::default(),
                caps: HostCapabilities {
                    available: BTreeSet::new(),
                    granted: BTreeSet::new(),
                },
                revoked: BTreeSet::new(),
            }
        }

        /// The claim's optimization request. `with_traces` false leaves the step
        /// without an eligible reflection batch, which ends as `NoChange`.
        fn request<'a>(
            &'a self,
            fixture: &'a Fixture,
            claim_id: &str,
            sequence: u32,
            with_traces: bool,
        ) -> OptimizationStepRequest<'a> {
            trusted_compiler_inputs::request(OptimizationStepRequest {
                evidence: &self.evidence,
                source_selection: &self.selection,
                source_bindings: &self.bindings,
                traces: if with_traces {
                    fixture
                        .authorities
                        .iter()
                        .map(|authority| authority.trace.clone())
                        .collect()
                } else {
                    vec![]
                },
                model_context: ModelRequestContext {
                    request_id: format!("consolidation-request-{sequence}"),
                    namespace: TENANT.into(),
                    purpose: Purpose::Development,
                    stage: ModelStage::Consolidate,
                    episode_id: claim_id.into(),
                    step: sequence,
                    attempt: 1,
                    parent_skill_digest: fixture.parent_skill_digest.clone(),
                    bundle_digest: fixture.parent_bundle_digest.clone(),
                    source_closure: self.allowed.clone(),
                    model_digest: d("fixture-model"),
                    tools_digest: d("tools"),
                    rules_digest: d("rules"),
                    sampling_digest: d("sampling"),
                    revoke_watermark: 1,
                    max_suggestions: 4,
                },
                parent_skill: &fixture.parent_skill,
                edit_context: &self.edit_context,
                edit_batch_template: self.edit_template.clone(),
                protected_ranges: &[],
                bundle_context: BundleCompileContext {
                    profile: &self.profile,
                    baseline: &self.baseline,
                    parent_strategy: &self.strategy,
                    baseline_strategy: &self.strategy,
                    improver_patch: &self.patch,
                    caps: &self.caps,
                    revoked: &self.revoked,
                },
                development_request: DevelopmentRunRequest {
                    request_id: format!("consolidation-development-{sequence}"),
                    namespace: TENANT.into(),
                    purpose: Purpose::Development,
                    episode_id: claim_id.into(),
                    step: sequence,
                    attempt: 1,
                    manifest: fixture.manifest.clone(),
                    parent_bundle_digest: fixture.parent_bundle_digest.clone(),
                    candidate_bundle_digest: d(&format!("candidate-{sequence}")),
                    environment_digest: fixture.environment.environment.environment_digest.clone(),
                    grader_digest: fixture.grader_digest.clone(),
                    rules_digest: d("rules"),
                    tools_digest: d("tools"),
                    revoke_watermark: 1,
                    idempotency_key: format!("consolidation-idempotency-{sequence}"),
                },
                allow_rank_call: false,
            })
        }
    }

    /// A claimed fixture run to its first terminal record.
    struct Run {
        fixture: Fixture,
        material: StepMaterial,
        claim: ConsolidationClaim,
        model: EditingModelPort,
        runner: LedgerDevRunner,
    }

    impl Run {
        async fn claimed(improves: bool) -> Self {
            let fixture = Fixture::new().await;
            let claim = fixture.claimed().await;
            let material = StepMaterial::new(&fixture).await;
            let runner = LedgerDevRunner::new(&fixture, improves);
            Self {
                fixture,
                material,
                claim,
                model: EditingModelPort::new("first"),
                runner,
            }
        }

        /// Runs `claim` through `run_consolidation` with the given ports.
        async fn run_with(
            &self,
            claim: &ConsolidationClaim,
            model: &dyn ConsolidationModelPort,
            journal: &dyn OptimizationJournal,
            sequence: u32,
            with_traces: bool,
        ) -> Result<ConsolidationRunRecord, Error> {
            MonitoringCoordinator::run_consolidation(
                &self.fixture.worker,
                &self.fixture.store,
                &claim.id,
                Some(model),
                Some(&self.runner),
                Some(journal),
                self.material
                    .request(&self.fixture, &claim.id, sequence, with_traces),
            )
            .await
        }

        async fn run(&self, with_traces: bool) -> Result<ConsolidationRunRecord, Error> {
            let journal = self.fixture.journal();
            self.run_with(&self.claim, &self.model, &journal, 1, with_traces)
                .await
        }

        async fn proposal(&self) -> ConsolidationProposal {
            self.proposal_of(&self.claim).await
        }

        async fn proposal_of(&self, claim: &ConsolidationClaim) -> ConsolidationProposal {
            let mut session = self.fixture.store.session().await.unwrap();
            let run: ConsolidationRunRecord = session
                .need(
                    &self.fixture.worker,
                    "artifact",
                    &format!("consolidation-run-{}", claim.id),
                )
                .await
                .unwrap();
            let proposal = session
                .need(
                    &self.fixture.worker,
                    "artifact",
                    run.proposal_id.as_deref().unwrap(),
                )
                .await
                .unwrap();
            session.commit().await.unwrap();
            proposal
        }

        async fn stage(
            &self,
            ctx: &Context,
            proposal: &ConsolidationProposal,
            candidate_id: &str,
        ) -> Result<StagedConsolidationCandidate, Error> {
            MonitoringCoordinator::stage_current_consolidation_candidate(
                ctx,
                &self.fixture.store,
                &proposal.id,
                candidate_id,
            )
            .await
        }
    }

    #[tokio::test]
    async fn consolidation_claim_and_derived_singletons_do_not_expand_at_full_capacity() {
        use super::{capacity, image};
        use evo_core::capacity::CapacityItem;
        let run = Run::claimed(true).await;
        let record = run.run(true).await.unwrap();
        assert_eq!(record.outcome, ConsolidationRunOutcome::Candidate);
        let proposal = run.proposal().await;
        let first = run
            .stage(&run.fixture.worker, &proposal, "first-candidate")
            .await
            .unwrap();
        assert_eq!(first.candidate.id, "first-candidate");
        for generation in 2..=250u32 {
            run.fixture
                .close_cycle(generation * 2 - 1, &no_contrast())
                .await
                .unwrap();
            run.fixture
                .close_cycle(generation * 2, &no_contrast())
                .await
                .unwrap();
            let claim = run.fixture.claim_generation().await;
            MonitoringCoordinator::complete_no_contrast(
                &run.fixture.worker,
                &run.fixture.store,
                &claim.id,
            )
            .await
            .unwrap();
        }
        let mut session = run.fixture.store.session().await.unwrap();
        assert_eq!(
            session
                .capacity_used(CapacityItem::ConsolidationClaim)
                .await
                .unwrap(),
            250
        );
        assert_eq!(
            session
                .capacity_proposals_for_claim(&run.fixture.worker, &run.claim.id)
                .await
                .unwrap(),
            1
        );
        let all: Vec<Value> = session.list(&run.fixture.worker, "artifact").await.unwrap();
        let bindings = all
            .iter()
            .filter(|v| {
                v["schema_version"] == CONSOLIDATION_STAGING_SCHEMA
                    && v["proposal_id"] == proposal.id
            })
            .count();
        assert_eq!(bindings, 1);
        session.commit().await.unwrap();
        let calls = run.model.calls.load(Ordering::SeqCst);
        let before = super::image(run.fixture._dir.path());
        let repeated = run.run(true).await.unwrap();
        assert_eq!(repeated.id, record.id);
        assert_eq!(
            run.stage(&run.fixture.worker, &proposal, "first-candidate")
                .await
                .unwrap()
                .candidate
                .id,
            first.candidate.id
        );
        assert_eq!(run.model.calls.load(Ordering::SeqCst), calls);
        assert_eq!(super::image(run.fixture._dir.path()), before);
        run.fixture.close_cycle(501, &no_contrast()).await.unwrap();
        let last = run.fixture.close_cycle(502, &no_contrast()).await.unwrap();
        let before = image(run.fixture._dir.path());
        capacity(
            MonitoringCoordinator::claim_consolidation(
                &run.fixture.worker,
                &run.fixture.store,
                &last.scope_id,
            )
            .await
            .unwrap_err(),
            CapacityItem::ConsolidationClaim,
            250,
            1,
        );
        assert_eq!(image(run.fixture._dir.path()), before);
    }
}

mod curriculum {
    use super::trusted_compiler_inputs;
    use evo_core::Context;
    use evo_core::Role;
    use evo_core::curriculum::CoverageBucketState;
    use evo_core::curriculum::CurriculumControlProfileV1;
    use evo_core::curriculum::LearnerStateV2;
    use evo_core::curriculum::LearnerStateV3;
    use evo_core::curriculum::ProbeJobV1;
    use evo_core::curriculum::RegisteredProperty;
    use evo_core::curriculum::StructuredTestProposalV1;
    use evo_core::evaluation::DataUse;
    use evo_core::evidence::ExecutionAttestation;
    use evo_core::evidence::Purpose;
    use evo_core::evidence::TaskOrigin;
    use evo_core::fingerprint;
    use evo_core::hash;
    use evo_core::optimization::OptimizationTrace;
    use evo_core::optimization::TraceOutcome;
    use evo_engine::curriculum::CurriculumSourceArtifactV1;
    use evo_engine::curriculum::CurriculumSourceKindV1;
    use evo_engine::curriculum::CurriculumTaskCandidateV1;
    use evo_engine::curriculum::DevelopmentCycleReceiptV1;
    use evo_engine::curriculum::PersistentCurriculumCoordinator;
    use evo_engine::curriculum::curriculum_state_storage_id;
    use evo_engine::curriculum_profiles::CLAMP_TARGET_ID;
    use evo_engine::curriculum_profiles::ClampOutput;
    use evo_engine::curriculum_profiles::OfflineProfileCheck;
    use evo_engine::curriculum_profiles::RegisteredPureFunctionProfileV1;
    use evo_engine::curriculum_profiles::check_target_outputs;
    use evo_engine::curriculum_profiles::registered_profile_source_bodies;
    use evo_engine::curriculum_profiles::runner_verified_report;
    use evo_engine::development::DevelopmentControlV1;
    use evo_engine::development::DevelopmentEvidenceScope;
    use evo_engine::development::DevelopmentTaskSpecV1;
    use evo_engine::development::RegisteredDevelopmentRunner;
    use evo_engine::development::RegisteredTargetInputV1;
    use evo_engine::development::register_development_control;
    use evo_engine::development::registered_runner_digest;
    use evo_engine::evidence::StoredRunRecord;
    use evo_engine::evidence::StoredTraceAuthority;
    use evo_engine::evidence::store_trace_authority;
    use evo_engine::optimization::DevelopmentRunReport;
    use evo_engine::optimization::DevelopmentRunRequest;
    use evo_engine::optimization::OPTIMIZATION_STAGE_FACT_SCHEMA;
    use evo_engine::optimization::OptimizationJournalStage;
    use evo_engine::optimization::StageDependency;
    use evo_engine::optimization::StageFact;
    use evo_engine::optimization::StageFactKind;
    use evo_engine::optimization::StoreOptimizationJournal;
    use evo_engine::optimization::{DevRunner, OptimizationJournal};
    use evo_engine::streaming_evaluator::FixedGraderMethod;
    use evo_engine::streaming_evaluator::FixedGraderSpec;
    use evo_storage::Store;
    use evo_storage::budget::RootBudgetAuthorization;
    use serde_json::Value;
    use serde_json::json;
    use std::sync::Arc;

    const NAMESPACE: &str = "n";
    const SCOPE: &str = "dev-scope";
    const CONTROL_ID: &str = "dev-control";
    const PROFILE_ID: &str = "pure_function_test_proposal.v1";
    const STATE_ID: &str = "learner-state";
    /// The development families of the registered control, one task each.
    const FAMILIES: [&str; 5] = ["family-a", "family-b", "family-c", "family-d", "family-e"];

    fn d(value: &str) -> String {
        hash(value.as_bytes())
    }

    fn storage_id(record_kind: &str, id: &str) -> String {
        format!("e12-{}", fingerprint(&(record_kind, id)).unwrap())
    }

    // ---------------------------------------------------------------------------
    // Fixtures (copied from tests/curriculum_cleanup_v42.rs, tests/release_store.rs)
    // ---------------------------------------------------------------------------

    fn grader_spec() -> FixedGraderSpec {
        FixedGraderSpec {
            schema_version: FixedGraderSpec::SCHEMA.into(),
            version: "exact-json-v1".into(),
            method: FixedGraderMethod::ExactJsonAnswerV1,
        }
    }

    fn run_body(id: &str) -> String {
        format!("trusted-run-{id}")
    }

    fn authority(id: &str, family: &str) -> StoredTraceAuthority {
        let body = run_body(id);
        StoredTraceAuthority {
            schema_version: "rsia.optimization.source.v1".into(),
            record: StoredRunRecord {
                id: id.into(),
                body: body.as_bytes().to_vec(),
                parent_family: family.into(),
                task_origin: TaskOrigin::TrustedRun,
                execution_attestation: ExecutionAttestation::TrustedHost,
                purpose: Purpose::Development,
            },
            trace: OptimizationTrace {
                run_id: id.into(),
                parent_family: family.into(),
                source_digest: hash(body.as_bytes()),
                purpose: Purpose::Development,
                outcome: TraceOutcome::Success,
                diagnosis: None,
                excerpt: body.clone(),
                seed: 1,
            },
            excerpt_start: 0,
            excerpt_end: body.len(),
        }
    }

    fn tasks() -> Vec<DevelopmentTaskSpecV1> {
        FAMILIES
            .iter()
            .map(|family| {
                DevelopmentTaskSpecV1::registered(
                    format!("task-{}", family.trim_start_matches("family-")),
                    *family,
                    RegisteredTargetInputV1::ClampI64 {
                        value: -2,
                        min: -1,
                        max: 1,
                    },
                )
                .unwrap()
            })
            .collect()
    }

    fn control() -> DevelopmentControlV1 {
        let profile = RegisteredPureFunctionProfileV1::clamp_i64();
        let grader = grader_spec();
        let mut control = DevelopmentControlV1 {
            schema_version: DevelopmentControlV1::SCHEMA.into(),
            id: CONTROL_ID.into(),
            namespace: NAMESPACE.into(),
            billing_scope: SCOPE.into(),
            root_budget_id: "dev-root".into(),
            executor_actor: "dev-executor".into(),
            grader_actor: "dev-grader".into(),
            proposer_actor: "optimizer".into(),
            manifest_id: "dev-manifest".into(),
            manifest_digest: String::new(),
            tasks: tasks(),
            environment_digest: d("environment"),
            grader_digest: fingerprint(&grader).unwrap(),
            fixed_grader: grader,
            oracle_digest: profile.oracle_digest,
            target_digest: profile.target_digest,
            runner_digest: registered_runner_digest().unwrap(),
            rules_digest: d("rules"),
            tools_digest: d("tools"),
            source_ids: vec!["run-a".into(), "run-b".into()],
            evidence_scope: DevelopmentEvidenceScope::RegisteredPureFunctionExecution,
            created_at_unix_seconds: 1,
        };
        control.manifest_digest = control.manifest().unwrap().digest;
        control.validate().unwrap();
        control
    }

    fn build_request(
        control: &DevelopmentControlV1,
        request_id: &str,
        episode_id: &str,
        candidate_bundle_digest: &str,
    ) -> DevelopmentRunRequest {
        DevelopmentRunRequest {
            request_id: request_id.into(),
            namespace: NAMESPACE.into(),
            purpose: Purpose::Development,
            episode_id: episode_id.into(),
            step: 1,
            attempt: 1,
            manifest: control.manifest().unwrap(),
            parent_bundle_digest: d("parent-bundle"),
            candidate_bundle_digest: candidate_bundle_digest.into(),
            environment_digest: control.environment_digest.clone(),
            grader_digest: control.grader_digest.clone(),
            rules_digest: control.rules_digest.clone(),
            tools_digest: control.tools_digest.clone(),
            revoke_watermark: 1,
            idempotency_key: format!("{request_id}-idem"),
        }
    }

    struct Fixture {
        candidate_digest: String,
        _dir: tempfile::TempDir,
        store: Store,
        admin: Context,
        executor: Context,
        grader: Context,
        control: DevelopmentControlV1,
    }

    impl Fixture {
        async fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("curriculum-v45.sqlite3");
            let store = Store::open(&path).await.unwrap();
            let admin = Context::new(NAMESPACE, "admin", Role::Admin).unwrap();
            let host = Context::new(NAMESPACE, "host", Role::Host).unwrap();
            let executor = Context::new(NAMESPACE, "dev-executor", Role::Worker).unwrap();
            let grader = Context::new(NAMESPACE, "dev-grader", Role::Evaluator).unwrap();
            for (id, family) in [("run-a", "family-a"), ("run-b", "family-b")] {
                store_trace_authority(&store, &host, &authority(id, family))
                    .await
                    .unwrap();
            }
            let mut session = store.session().await.unwrap();
            session
                .bump_watermark(&admin, &d("initial-watermark"))
                .await
                .unwrap();
            session.commit().await.unwrap();
            store
                .authorize_root_budget(
                    &admin,
                    &RootBudgetAuthorization {
                        root_budget_id: "dev-root".into(),
                        billing_scope: SCOPE.into(),
                        allowed_namespaces: vec![NAMESPACE.into()],
                        currency: "USD".into(),
                        pricing_version: "pricing-v1".into(),
                        payment_subject: "fixture-only".into(),
                        authorization_receipt_digest: d("admin-budget-receipt"),
                        per_call_cap_micros: 10,
                        total_limit_micros: 1_000,
                        created_at: 1,
                    },
                )
                .await
                .unwrap();
            let control = control();
            let candidate = trusted_compiler_inputs::software_candidate_current(
                &store,
                &executor,
                &control.environment_digest,
                "candidate-bundle",
                1,
            )
            .await
            .unwrap();
            register_development_control(&admin, &store, control.clone())
                .await
                .unwrap();
            Self {
                candidate_digest: candidate.digest,
                _dir: dir,
                store,
                admin,
                executor,
                grader,
                control,
            }
        }

        fn runner(&self) -> RegisteredDevelopmentRunner {
            RegisteredDevelopmentRunner::with_clock(
                self.store.clone(),
                self.executor.clone(),
                self.grader.clone(),
                CONTROL_ID,
                600,
                Arc::new(|| 100),
            )
            .unwrap()
        }

        async fn commit_facts(
            &self,
            request: &DevelopmentRunRequest,
            report: &DevelopmentRunReport,
        ) -> (String, String) {
            let journal =
                StoreOptimizationJournal::new(self.store.clone(), self.admin.clone(), "admin")
                    .unwrap();
            let extras = vec![
                StageDependency {
                    kind: "run".into(),
                    id: "run-a".into(),
                },
                StageDependency {
                    kind: "run".into(),
                    id: "run-b".into(),
                },
                StageDependency {
                    kind: "revoke_watermark".into(),
                    id: "1".into(),
                },
                StageDependency {
                    kind: "artifact".into(),
                    id: "optgrant-fixture".into(),
                },
                StageDependency {
                    kind: "artifact".into(),
                    id: "optstage-step-fixture".into(),
                },
            ];
            let request_fact = StageFact {
                schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
                artifact_id: String::new(),
                namespace: NAMESPACE.into(),
                episode_id: request.episode_id.clone(),
                step: request.step,
                attempt: request.attempt,
                stage: OptimizationJournalStage::Development,
                kind: StageFactKind::DevelopmentRequestPrepared,
                request_id: request.request_id.clone(),
                input_digest: request.manifest.digest.clone(),
                output_digest: None,
                dependencies: extras
                    .iter()
                    .cloned()
                    .chain(std::iter::once(StageDependency {
                        kind: "environment".into(),
                        id: request.environment_digest.clone(),
                    }))
                    .collect(),
                payload: serde_json::to_value(request).unwrap(),
            }
            .seal()
            .unwrap();
            let mut dependencies: Vec<StageDependency> = report
                .results
                .iter()
                .flat_map(|result| {
                    [
                        StageDependency {
                            kind: "execution".into(),
                            id: result.parent_execution_id.clone(),
                        },
                        StageDependency {
                            kind: "execution".into(),
                            id: result.candidate_execution_id.clone(),
                        },
                        StageDependency {
                            kind: "grader".into(),
                            id: result.grader_receipt_digest.clone(),
                        },
                    ]
                })
                .collect();
            dependencies.extend(evo_engine::development::typed_receipt_closure(report).unwrap());
            dependencies.extend(extras);
            let observed_fact = StageFact {
                schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
                artifact_id: String::new(),
                namespace: NAMESPACE.into(),
                episode_id: request.episode_id.clone(),
                step: request.step,
                attempt: request.attempt,
                stage: OptimizationJournalStage::Development,
                kind: StageFactKind::DevelopmentObserved,
                request_id: request.request_id.clone(),
                input_digest: request.manifest.digest.clone(),
                output_digest: None,
                dependencies,
                payload: serde_json::to_value(report).unwrap(),
            }
            .seal()
            .unwrap();
            let inputs = trusted_compiler_inputs::bundle_use_current(
                &self.store,
                &self.admin,
                &request.candidate_bundle_digest,
                &request.environment_digest,
                request.revoke_watermark,
            )
            .await
            .unwrap();
            journal
                .commit_source_bound(request_fact.clone(), &inputs)
                .await
                .unwrap();
            journal
                .commit_source_bound(observed_fact.clone(), &inputs)
                .await
                .unwrap();
            (request_fact.artifact_id, observed_fact.artifact_id)
        }
    }

    fn curriculum_source(
        id: &str,
        source_kind: CurriculumSourceKindV1,
        subject_digest: String,
        body: Value,
    ) -> CurriculumSourceArtifactV1 {
        CurriculumSourceArtifactV1 {
            schema_version: CurriculumSourceArtifactV1::SCHEMA.into(),
            id: id.into(),
            source_kind,
            data_use: DataUse::Development,
            subject_digest,
            body_digest: fingerprint(&body).unwrap(),
            body,
            dependency_ids: vec![],
        }
    }

    fn curriculum_source_ids() -> Vec<String> {
        let registered = RegisteredPureFunctionProfileV1::clamp_i64();
        vec![
            registered.oracle_source_id,
            registered.runner_source_id,
            registered.target_source_id,
            "task-space-source".into(),
        ]
    }

    fn registered_v2_state(fixture: &Fixture, buckets: &[&str]) -> LearnerStateV2 {
        let registered = RegisteredPureFunctionProfileV1::clamp_i64();
        LearnerStateV2 {
            schema_version: LearnerStateV2::SCHEMA.into(),
            id: STATE_ID.into(),
            profile_id: PROFILE_ID.into(),
            skill_snapshot_digest: d("skill"),
            improver_snapshot_digest: d("improver"),
            environment_digest: fixture.control.environment_digest.clone(),
            grader_digest: fixture.control.grader_digest.clone(),
            model_tools_digest: d("model-tools"),
            runner_digest: registered.runner_digest,
            rules_digest: d("rules"),
            source_watermark: 1,
            source_artifact_ids: curriculum_source_ids(),
            development_fact_ids: vec![],
            completed_cycles: vec![],
            failure_clusters: vec![],
            coverage_buckets: buckets
                .iter()
                .map(|bucket| CoverageBucketState {
                    bucket_id: (*bucket).into(),
                    task_space_source_id: "task-space-source".into(),
                    observed_independent_clusters: 0,
                    required_independent_clusters: 1,
                })
                .collect(),
            applied_assets: vec![],
            active_probe_job_id: None,
            last_trigger_window_digest: None,
            cooldown_remaining_cycles: 0,
        }
    }

    /// Registers the typed sources, the offline profile (with the given fixed
    /// random seed commitment) and the initial learner state with one coverage
    /// bucket per listed family.
    async fn curriculum(
        fixture: &Fixture,
        seed_commitment: Option<String>,
        buckets: &[&str],
    ) -> PersistentCurriculumCoordinator {
        let coordinator = PersistentCurriculumCoordinator::new(
            fixture.store.clone(),
            fixture.admin.clone(),
            "admin",
        )
        .unwrap();
        coordinator
            .register_source(curriculum_source(
                "task-space-source",
                CurriculumSourceKindV1::TaskSpace,
                d("task-space"),
                json!({"buckets": FAMILIES}),
            ))
            .await
            .unwrap();
        for (id, body) in registered_profile_source_bodies() {
            let kind = if id.contains("target") || id.contains("source") {
                CurriculumSourceKindV1::TargetSpec
            } else if id.contains("oracle") {
                CurriculumSourceKindV1::OracleSpec
            } else {
                CurriculumSourceKindV1::RunnerSpec
            };
            coordinator
                .register_source(curriculum_source(
                    &id,
                    kind,
                    fingerprint(&body).unwrap(),
                    body,
                ))
                .await
                .unwrap();
        }
        let registered = RegisteredPureFunctionProfileV1::clamp_i64();
        let mut profile = CurriculumControlProfileV1::offline_default(
            PROFILE_ID,
            d("task-space"),
            registered.oracle_digest.clone(),
            registered.runner_digest.clone(),
        )
        .unwrap();
        profile.fixed_random_seed_commitment = seed_commitment;
        coordinator
            .register_profile_and_state(profile, registered_v2_state(fixture, buckets))
            .await
            .unwrap();
        coordinator
    }

    /// One real registered development cycle reduced through `record_cycle`;
    /// returns its (request, observed) stage-fact ids.
    async fn real_cycle(
        fixture: &Fixture,
        coordinator: &PersistentCurriculumCoordinator,
        n: u32,
    ) -> (String, String) {
        let request = build_request(
            &fixture.control,
            &format!("dev-request-{n}"),
            &format!("episode-{n}"),
            &fixture.candidate_digest,
        );
        let report = fixture.runner().run(request.clone()).await.unwrap();
        let (request_fact_id, observed_fact_id) = fixture.commit_facts(&request, &report).await;
        coordinator
            .record_cycle(DevelopmentCycleReceiptV1 {
                schema_version: "rsia.development_cycle_receipt.v1".into(),
                id: format!("cycle-{n}"),
                state_id: STATE_ID.into(),
                development_request_fact_id: request_fact_id.clone(),
                development_observed_fact_id: observed_fact_id.clone(),
            })
            .await
            .unwrap();
        (request_fact_id, observed_fact_id)
    }

    async fn probe_job(coordinator: &PersistentCurriculumCoordinator) -> ProbeJobV1 {
        coordinator
            .schedule_probe(PROFILE_ID, STATE_ID, 1_000_000)
            .await
            .unwrap()
    }

    /// A stored registered-boundary proposal (`BelowMapsToMin` on [-1, 1]).
    /// `value = -2` passes the offline check; `value = 0` is quarantined (the
    /// registered precondition fails).
    async fn stored_proposal(
        coordinator: &PersistentCurriculumCoordinator,
        job: &ProbeJobV1,
        id: &str,
        family: &str,
        value: i64,
    ) -> StructuredTestProposalV1 {
        let proposal = StructuredTestProposalV1 {
            schema_version: "rsia.structured_test_proposal.v1".into(),
            id: id.into(),
            probe_job_id: job.id.clone(),
            target_id: CLAMP_TARGET_ID.into(),
            property: RegisteredProperty::BelowMapsToMin,
            value,
            min: -1,
            max: 1,
            parent_family: family.into(),
            data_use: DataUse::Development,
            source_artifact_ids: vec!["task-space-source".into()],
            reason: "exercise registered lower boundary".into(),
        };
        coordinator
            .store_proposal(STATE_ID, proposal.clone())
            .await
            .unwrap();
        proposal
    }

    /// The offline conclusion of a passing proposal, from the observed target
    /// output (clamp(-2, -1, 1) = -1).
    fn passed_offline(proposal: &StructuredTestProposalV1) -> OfflineProfileCheck {
        let offline = check_target_outputs(
            &RegisteredPureFunctionProfileV1::clamp_i64(),
            proposal,
            ClampOutput { clamped: -1 },
            ClampOutput { clamped: -1 },
        )
        .unwrap();
        assert!(matches!(offline, OfflineProfileCheck::Passed { .. }));
        offline
    }

    /// Records a runner-verified report for a passing proposal bound to `facts`.
    async fn verified_report(
        coordinator: &PersistentCurriculumCoordinator,
        proposal: &StructuredTestProposalV1,
        facts: &(String, String),
    ) -> String {
        let report = runner_verified_report(
            &RegisteredPureFunctionProfileV1::clamp_i64(),
            proposal,
            &passed_offline(proposal),
            &facts.0,
            &facts.1,
        )
        .unwrap();
        coordinator
            .record_validity_report(STATE_ID, report)
            .await
            .unwrap()
    }

    /// One eligible candidate per family: a stored passing proposal and its
    /// runner-verified report.
    async fn eligible_candidates(
        coordinator: &PersistentCurriculumCoordinator,
        facts: &(String, String),
        specs: &[(&str, &str, u32, Option<&str>)],
    ) -> Vec<CurriculumTaskCandidateV1> {
        let job = probe_job(coordinator).await;
        let mut candidates = Vec::new();
        for (task_id, family, seq, bucket) in specs {
            let proposal = stored_proposal(
                coordinator,
                &job,
                &format!("proposal-{task_id}"),
                family,
                -2,
            )
            .await;
            let report_id = verified_report(coordinator, &proposal, facts).await;
            candidates.push(CurriculumTaskCandidateV1 {
                id: (*task_id).into(),
                parent_family: (*family).into(),
                coverage_bucket_id: bucket.map(str::to_string),
                stable_choice_seq: *seq,
                proposal_id: proposal.id,
                validity_report_id: report_id,
            });
        }
        candidates
    }

    async fn raw_state(fixture: &Fixture) -> Value {
        let mut session = fixture.store.session().await.unwrap();
        let body: Value = session
            .get(
                &fixture.admin,
                "artifact",
                &curriculum_state_storage_id(STATE_ID).unwrap(),
            )
            .await
            .unwrap()
            .unwrap();
        session.commit().await.unwrap();
        body
    }
    async fn load_state(fixture: &Fixture) -> LearnerStateV3 {
        serde_json::from_value(raw_state(fixture).await["payload"].clone()).unwrap()
    }

    async fn cycle_material(fixture: &Fixture, n: u32) -> DevelopmentCycleReceiptV1 {
        let request = build_request(
            &fixture.control,
            &format!("dev-request-{n}"),
            &format!("episode-{n}"),
            &fixture.candidate_digest,
        );
        let report = fixture.runner().run(request.clone()).await.unwrap();
        let (request_fact, observed_fact) = fixture.commit_facts(&request, &report).await;
        DevelopmentCycleReceiptV1 {
            schema_version: "rsia.development_cycle_receipt.v1".into(),
            id: format!("cycle-{n}"),
            state_id: STATE_ID.into(),
            development_request_fact_id: request_fact,
            development_observed_fact_id: observed_fact,
        }
    }

    #[tokio::test]
    async fn curriculum_cycle_and_new_failure_batch_are_admitted_together() {
        use super::{capacity, image};
        use evo_core::capacity::CapacityItem;
        let fixture = Fixture::new().await;
        let coordinator = curriculum(&fixture, None, &[]).await;
        let first = cycle_material(&fixture, 1).await;
        let state = coordinator.record_cycle(first.clone()).await.unwrap();
        assert_eq!(state.completed_cycles.len(), 1);
        assert!(state.failure_records.is_empty());
        let mut session = fixture.store.session().await.unwrap();
        let original: Value = session
            .need(
                &fixture.admin,
                "artifact",
                &storage_id("development_cycle_receipt_v1", &first.id),
            )
            .await
            .unwrap();
        let other = Context::new("old-cycles", "admin", Role::Admin).unwrap();
        for i in 0..998 {
            let mut row = original.clone();
            let logical = format!("old-cycle-{i}");
            let id = storage_id("development_cycle_receipt_v1", &logical);
            row["id"] = json!(id);
            row["payload"]["id"] = json!(logical);
            row["payload"]["development_observed_fact_id"] = json!(format!("old-observed-{i}"));
            session
                .put(&other, "artifact", &id, other.actor(), &row)
                .await
                .unwrap();
        }
        assert_eq!(
            session
                .capacity_used(CapacityItem::DevelopmentCycle)
                .await
                .unwrap(),
            999
        );
        session.commit().await.unwrap();
        let last = cycle_material(&fixture, 2).await;
        let closed = coordinator.record_cycle(last.clone()).await.unwrap();
        assert_eq!(closed.completed_cycles.len(), 2);
        let excess = cycle_material(&fixture, 3).await;
        let before = image(fixture._dir.path());
        capacity(
            coordinator.record_cycle(excess).await.unwrap_err(),
            CapacityItem::DevelopmentCycle,
            1000,
            1,
        );
        assert_eq!(image(fixture._dir.path()), before);
        assert_eq!(
            coordinator
                .record_cycle(last)
                .await
                .unwrap()
                .completed_cycles
                .len(),
            2
        );
        assert_eq!(image(fixture._dir.path()), before);
        // The registered clamp runner above produces verified passes. It cannot
        // authenticate an invented failing output; failure-batch growth needs a
        // separate genuine failing execution path rather than forged receipts.
    }

    #[tokio::test]
    async fn selection_and_probe_limits_precede_state_and_schedule_writes() {
        use super::{capacity, image};
        use evo_core::capacity::CapacityItem;
        let fixture = Fixture::new().await;
        let coordinator = curriculum(&fixture, None, &[]).await;
        let facts = real_cycle(&fixture, &coordinator, 1).await;
        let offered = eligible_candidates(
            &coordinator,
            &facts,
            &[
                ("a", "family-a", 1, None),
                ("b", "family-b", 2, None),
                ("c", "family-c", 3, None),
            ],
        )
        .await;
        let first = coordinator
            .select_next_task(STATE_ID, &offered[..1])
            .await
            .unwrap();
        let mut session = fixture.store.session().await.unwrap();
        let original: Value = session
            .need(
                &fixture.admin,
                "artifact",
                &storage_id("curriculum_selection_v1", &first.id),
            )
            .await
            .unwrap();
        let other = Context::new("old-selections", "admin", Role::Admin).unwrap();
        for i in 0..998 {
            let mut row = original.clone();
            let logical = format!("old-selection-{i}");
            let id = storage_id("curriculum_selection_v1", &logical);
            row["id"] = json!(id);
            row["payload"]["id"] = json!(logical);
            session
                .put(&other, "artifact", &id, other.actor(), &row)
                .await
                .unwrap();
        }
        assert_eq!(
            session
                .capacity_used(CapacityItem::SelectionRecord)
                .await
                .unwrap(),
            999
        );
        session.commit().await.unwrap();
        let last = coordinator
            .select_next_task(STATE_ID, &offered[1..2])
            .await
            .unwrap();
        assert_eq!(
            coordinator
                .select_next_task(STATE_ID, &offered[1..2])
                .await
                .unwrap()
                .id,
            last.id
        );
        let before = image(fixture._dir.path());
        capacity(
            coordinator
                .select_next_task(STATE_ID, &offered[2..])
                .await
                .unwrap_err(),
            CapacityItem::SelectionRecord,
            1000,
            1,
        );
        assert_eq!(image(fixture._dir.path()), before);
        let actual_state = load_state(&fixture).await;
        assert_eq!(actual_state.completed_cycles.len(), 1);
        // A fresh state isolates probe admission from selection's learner-state digest.
        let fixture = Fixture::new().await;
        let coordinator = curriculum(&fixture, None, &[]).await;
        let first = coordinator
            .schedule_probe_idempotent("healthy", PROFILE_ID, STATE_ID, 1_000_000)
            .await
            .unwrap();
        let mut session = fixture.store.session().await.unwrap();
        let original: Value = session
            .need(
                &fixture.admin,
                "artifact",
                &storage_id("coverage_probe_job_v1", &first.id),
            )
            .await
            .unwrap();
        let other = Context::new("old-probes", "admin", Role::Admin).unwrap();
        for i in 0..98 {
            let mut row = original.clone();
            let logical = format!("old-probe-{i}");
            let id = storage_id("coverage_probe_job_v1", &logical);
            row["id"] = json!(id);
            row["payload"]["id"] = json!(logical);
            session
                .put(&other, "artifact", &id, other.actor(), &row)
                .await
                .unwrap();
        }
        assert_eq!(
            session
                .capacity_used(CapacityItem::CurriculumProbeJob)
                .await
                .unwrap(),
            99
        );
        session.commit().await.unwrap();
        let last = coordinator
            .schedule_probe_idempotent("last", PROFILE_ID, STATE_ID, 1_000_000)
            .await
            .unwrap();
        let before = image(fixture._dir.path());
        capacity(
            coordinator
                .schedule_probe_idempotent("excess", PROFILE_ID, STATE_ID, 1_000_000)
                .await
                .unwrap_err(),
            CapacityItem::CurriculumProbeJob,
            100,
            1,
        );
        assert_eq!(image(fixture._dir.path()), before);
        assert_eq!(
            coordinator
                .schedule_probe_idempotent("last", PROFILE_ID, STATE_ID, 1_000_000)
                .await
                .unwrap()
                .id,
            last.id
        );
        assert_eq!(image(fixture._dir.path()), before);
    }
}
