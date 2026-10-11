//! AG-095 (E05 independent grader, E06 release gate subset): one fixed
//! RegisteredExternalHost complete batch reaches the independent grader
//! capture, complete-batch authority issuance and the release-side verified
//! report view; a changed stored receipt or a capture bound to another digest
//! fails closed.
//!
//! Only the registered grader runs as an actually observed OS process: the
//! existing fixed `formal_local_host` binary (`grade` / `finalize`) in its
//! private registered workspace, holding its private credential. Host,
//! Administrator and Worker need three further distinct kernel UIDs that one
//! unprivileged test user cannot hold, so this fixture persists their
//! authenticated records (principal-set observation, principal actions,
//! provider captures, meter readings, episode cost) in the exact shapes and
//! content witnesses those principals write. Every product reader and
//! verifier runs unchanged. The ticket is issued before the Worker stages the
//! candidate (the unbound ticket wire, without GA-18 candidate/fact pins).
//! This is a software fixture: no real external host, provider, model, paid
//! execution, benefit or release chain is claimed.
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;

use evo_core::contract::{
    CompileParts, HostCapabilities, ImproverPatch, PatchOp, Profile, ResolvedBundle, SkillPatch,
    SkillSnapshot, compile_bundle,
};
use evo_core::evaluation::{
    CompleteEpisodeCostMeasurements, ExperimentPlan, FixedOptimizerArm, FixedOptimizerSpec,
    FormalExperimentPlanV41, FormalStatisticalUnit, Money, OptimizationBudgetPlan,
    OptimizerComparisonContract, ProfileKind, Verdict,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::holdout::{
    AnchorCoverageMatrix, CriticalCapabilityCoverage, FrozenCandidatePair, HoldoutManifest,
    SequentialMode,
};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::sequential::{AlphaAllocation, FormalClaimKind, ResearchFamilyAlphaPlan};
use evo_core::{Context, Role, Strategy, fingerprint, hash};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::formal_authority::{
    EpisodeCostRecord, FormalAuthorityIssuer, FormalProviderCaptureRecord, FormalRegistry,
    FrozenCompilerProfile, IndependentGradeCapture, METER_SCHEMA, PRINCIPAL_ACTION_SCHEMA,
    PRINCIPALS_SCHEMA, RegisteredExternalProvider, RegisteredFormalHost, RegisteredPrincipal,
    RegisteredPrincipalSet, RegisteredResourceInstrumentation, TypedContentRef, content_digest,
    prepare_formal_task,
};
use evo_engine::release_store::{
    ReleaseCandidateRecord, ReleaseStore, StageBundleRequest, TypedSourceRef,
};
use evo_engine::streaming_evaluator::{
    AnchorEvidenceStatus, BrokerTaskView, CostEvidenceScope, DependencyEvidenceStatus,
    EvaluationEvidenceScope, EvaluationTicketV2, ExecutionReceiptRequest, ExecutionSide,
    FixedGraderMethod, FixedGraderSpec, FrozenOracleEntry, IndependentEvaluationControl,
    IndependentGraderReceiptV2, IssueTicketRequest, ProtectedHoldoutRecord, ProtectedInputRef,
    RegisteredEvaluationControl, StartExecutionRequest, TicketLifecycle, VerifiedReportVariant,
};
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallReservation, BudgetExecutionProvenance, BudgetStage,
    ModelCallSettlementEvidence, RootBudgetAuthorization, UsageCharge,
};
use evo_storage::{Session, Store};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const NS: &str = "n";
const ADMIN: &str = "fixture-admin";
const GRADER: &str = "grader";
const HOST: &str = "host";
const PROPOSER: &str = "proposer";
const BILLING: &str = "billing-1";
const TICKET: &str = "ticket-ag095";
const CONTROL_ID: &str = "control-ag095";
const HOLDOUT_ID: &str = "holdout-ag095";
const ATTEMPT: &str = "attempt-1";
const MANDATORY: &str = "fixture-mandatory-context";
const GRADER_KEY: &str = "RSIA_AG095_GRADER_CREDENTIAL";
const GRADER_SECRET: &str = "ag095-grader-private-fixture-credential-32-bytes";
/// The fixed-sample empirical-Bernstein radius at alpha 0.04 needs 20 clusters
/// before an all-improved batch can clear the frozen minimum effect.
const ROTATING: u32 = 20;
const ENVELOPE_SCHEMA: &str = "rsia.typed_artifact_envelope.v1";
const CONTROL_KIND: &str = "registered_evaluation_control_v41";
const TICKET_KIND: &str = "evaluation_ticket_v2";
const FORMAL_KIND: &str = "formal_evaluation_v2";
const GRADER_RECEIPT_KIND: &str = "independent_grader_receipt_v2";

fn d(label: &str) -> String {
    hash(label.as_bytes())
}

fn ctx(role: Role, actor: &str) -> Context {
    Context::new(NS, actor, role).unwrap()
}

fn storage_id(record_kind: &str, logical_id: &str) -> String {
    format!("e05-{}", fingerprint(&(record_kind, logical_id)).unwrap())
}

fn grade_capture_id(target: &str) -> String {
    format!(
        "formal-grade-{}",
        &fingerprint(&(TICKET, target)).unwrap()[..32]
    )
}

fn task_input(target: &str) -> String {
    format!("ag095 protected task {target}")
}

fn money(amount: &str) -> Money {
    Money {
        currency: "USD".into(),
        pricing_version: "pricing-v1".into(),
        amount: amount.into(),
    }
}

fn optimizer(arm: FixedOptimizerArm, implementation: &str) -> FixedOptimizerSpec {
    FixedOptimizerSpec {
        arm,
        implementation_digest: d(implementation),
        initial_s0_digest: d("s0"),
        base_model_digest: d("model"),
        tools_digest: d("tools"),
        authorized_materials_digest: d("materials"),
        task_partition_digest: d("partition"),
        context_limit_tokens: 4096,
        root_budget_scope_id: BILLING.into(),
        budget_limit: money("0.001"),
    }
}

fn principal(uid: u32, actor: &str, secret: &str) -> RegisteredPrincipal {
    RegisteredPrincipal {
        uid,
        actor: actor.into(),
        principal: hash(secret.as_bytes()),
        credential_digest: hash(secret.as_bytes()),
    }
}

/// The product's typed artifact envelope, serialized in its declared order.
#[derive(Serialize)]
struct Envelope<'a, T> {
    schema_version: &'a str,
    id: &'a str,
    record_kind: &'a str,
    payload: &'a T,
}

/// One registered role's content witness over a stored body, in the
/// operation/entity shape the formal content readers authenticate.
async fn witness<T: Serialize>(
    s: &mut Session,
    role: Role,
    actor: &str,
    kind: &str,
    id: &str,
    body: &T,
) {
    let prefix = match role {
        Role::Admin => "formal.admin.content.v1",
        Role::Host => "formal.host.content.v1",
        Role::Evaluator => "formal.evaluator.content.v1",
        _ => unreachable!("no content witness for {role:?}"),
    };
    let operation = format!("{prefix}.{}", fingerprint(&(NS, kind, id)).unwrap());
    let entity = fingerprint(&(NS, kind, id, content_digest(body).unwrap())).unwrap();
    s.audit(&ctx(role, actor), &operation, &entity)
        .await
        .unwrap();
}

async fn put_witnessed<T: Serialize>(
    s: &mut Session,
    role: Role,
    actor: &str,
    kind: &str,
    id: &str,
    body: &T,
) {
    s.put(&ctx(role, actor), kind, id, actor, body)
        .await
        .unwrap();
    witness(s, role, actor, kind, id, body).await;
}

struct Fx {
    _db_dir: tempfile::TempDir,
    db: PathBuf,
    _workspace: tempfile::TempDir,
    workspace: PathBuf,
    host: RegisteredFormalHost,
    set: RegisteredPrincipalSet,
    candidate: ReleaseCandidateRecord,
    /// target id -> independent grader receipt id
    graded: BTreeMap<String, String>,
}

/// A registered principal action as the grader authority persists it. Only the
/// kernel uid/peer observation of the Administrator/Worker principals cannot be
/// produced by this single test user; every field a reader compares is real.
#[allow(clippy::too_many_arguments)]
async fn principal_action(
    s: &mut Session,
    host: &RegisteredFormalHost,
    set: &RegisteredPrincipalSet,
    role: Role,
    kind: &str,
    id: &str,
    subject_content_digest: String,
    operation: &str,
) {
    let p = match role {
        Role::Host => &set.host,
        Role::Evaluator => &set.grader,
        Role::Admin => &set.administrator,
        Role::Worker => &set.worker,
        _ => unreachable!(),
    };
    let action_id = format!(
        "principal-action-{}",
        fingerprint(&(kind, id, role)).unwrap()
    );
    let action = json!({
        "id": action_id,
        "schema_version": PRINCIPAL_ACTION_SCHEMA,
        "namespace": NS,
        "host_registration_id": host.id,
        "host_digest": fingerprint(host).unwrap(),
        "principal_set_digest": set.binding_digest().unwrap(),
        "role": role,
        "actor": p.actor,
        "uid": p.uid,
        "principal": p.principal,
        "credential_digest": p.credential_digest,
        "subject_kind": kind,
        "subject_id": id,
        "subject_content_digest": subject_content_digest,
        "operation": operation,
        "authenticated_request_digest": subject_content_digest,
        "peer_uid": p.uid,
        "peer_gid": Value::Null,
        "peer_pid": Value::Null,
        "authority_pid": std::process::id(),
        "authority_uid": set.grader.uid,
        "authority_executable_digest": host.grader_code_digest,
    });
    put_witnessed(s, Role::Evaluator, GRADER, "artifact", &action_id, &action).await;
    let grader = ctx(Role::Evaluator, GRADER);
    s.put_edge(&grader, "artifact", &action_id, "artifact", &set.id)
        .await
        .unwrap();
    s.put_edge(&grader, kind, id, "artifact", &action_id)
        .await
        .unwrap();
}

async fn stored_digest(s: &mut Session, kind: &str, id: &str) -> String {
    let body: Value = s.need(&ctx(Role::Admin, ADMIN), kind, id).await.unwrap();
    content_digest(&body).unwrap()
}

fn source(id: &str) -> StoredTraceAuthority {
    let body = format!("trusted development body for {id}");
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
            outcome: TraceOutcome::Success,
            diagnosis: None,
            excerpt: body.clone(),
            seed: 1,
        },
        excerpt_start: 0,
        excerpt_end: body.len(),
    }
}

fn compile(
    profile: &Profile,
    root_skill: &SkillSnapshot,
    root_strategy: &Strategy,
    frozen: &FrozenCompilerProfile,
    skill_patch: &SkillPatch,
    improver_patch: &ImproverPatch,
) -> ResolvedBundle {
    compile_bundle(CompileParts {
        profile,
        parent: root_skill,
        baseline: &frozen.baseline_skill,
        parent_strategy: root_strategy,
        baseline_strategy: &frozen.baseline_strategy,
        skill_patch,
        improver_patch,
        caps: &frozen.capabilities,
        revoked: &Default::default(),
    })
    .unwrap()
}

/// Runs the registered grader: the fixed binary in its private workspace with
/// only its private credential in the environment.
fn grader_process(fx: &Fx, command: Value) -> Result<Value, String> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_formal_local_host"))
        .current_dir(&fx.workspace)
        .env_clear()
        .env(GRADER_KEY, GRADER_SECRET)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert_ne!(child.id(), std::process::id());
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&command).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    if output.status.success() {
        return Ok(serde_json::from_slice(&output.stdout).unwrap());
    }
    let refusal: Value = serde_json::from_slice(&output.stderr).unwrap_or_else(|_| {
        panic!(
            "grader process failed without a fixed refusal: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert_eq!(refusal["status"], "refused");
    Err(refusal["reason"].as_str().unwrap().to_string())
}

fn finalize(fx: &Fx, db: &Path) -> Result<Value, String> {
    grader_process(
        fx,
        json!({"operation":"finalize","database":db.to_str().unwrap(),"namespace":NS,
            "actor":GRADER,"ticket_id":TICKET}),
    )
}

/// The frozen member's task view, as `broker_task` derives it from the stored
/// ticket. `prepare_formal_task` refuses any view that differs from that.
fn task_view(ticket: &EvaluationTicketV2, target_id: &str, side: ExecutionSide) -> BrokerTaskView {
    let target = ticket
        .targets
        .iter()
        .find(|target| target.target_id == target_id)
        .unwrap();
    let bundle_digest = match side {
        ExecutionSide::Candidate => ticket.candidate_bundle_digest.clone(),
        ExecutionSide::Baseline => ticket.baseline_bundle_digest.clone(),
    };
    let request_digest = fingerprint(&(
        "rsia.evaluation_execution_request.v1",
        &ticket.id,
        &target.target_id,
        &target.input_digest,
        side,
        &bundle_digest,
        &ticket.environment_digest,
        &ticket.manifest_digest,
    ))
    .unwrap();
    BrokerTaskView {
        ticket_id: ticket.id.clone(),
        target_id: target_id.into(),
        input_artifact_id: target.input_artifact_id.clone(),
        input_digest: target.input_digest.clone(),
        side,
        bundle_digest,
        environment_digest: ticket.environment_digest.clone(),
        request_digest,
    }
}

/// One registered-host execution through the actual ticket, budget-ledger and
/// receipt transitions. Returns the receipt id and the host's provider capture.
#[allow(clippy::too_many_arguments)]
async fn execute(
    store: &Store,
    ticket: &EvaluationTicketV2,
    provider: &RegisteredExternalProvider,
    host_registration: &RegisteredFormalHost,
    candidate: &ReleaseCandidateRecord,
    target: &str,
    side: ExecutionSide,
    output: &str,
    ordinal: u32,
) -> (String, FormalProviderCaptureRecord) {
    let host = ctx(Role::Host, HOST);
    let view = task_view(ticket, target, side);
    let request = prepare_formal_task(
        &host,
        store,
        view.clone(),
        MANDATORY.into(),
        task_input(target),
    )
    .await
    .unwrap();
    let now = 10 + i64::from(ordinal) * 5;
    let call_id = format!(
        "formal-call-{}",
        &hash(view.request_digest.as_bytes())[..32]
    );
    let call = store
        .reserve_budget_call_with_sources(
            &host,
            &BudgetCallReservation {
                billing_scope: BILLING.into(),
                call_id: call_id.clone(),
                dispatch_group_id: TICKET.into(),
                stage: BudgetStage::TaskExecution,
                actual_input_digest: view.request_digest.clone(),
                request_artifact: Some(
                    BudgetArtifact::from_serializable("rsia.formal_task_request.v1", &request)
                        .unwrap(),
                ),
                max_cost_micros: 100,
                lease_token: hash(format!("{NS}:{call_id}:{now}").as_bytes()),
                lease_until: now + 1_000,
                now,
            },
            &candidate
                .sources
                .iter()
                .map(|source| source.id.clone())
                .collect::<Vec<_>>(),
        )
        .await
        .unwrap();
    let fence = BudgetCallFence {
        billing_scope: BILLING.into(),
        call_id: call_id.clone(),
        actual_input_digest: view.request_digest.clone(),
        lease_token: call.lease_token.clone(),
        lease_epoch: call.lease_epoch,
        now,
    };
    let decision = IndependentEvaluationControl::start_execution(
        &host,
        store,
        StartExecutionRequest {
            ticket_id: TICKET.into(),
            target_id: target.into(),
            side,
            fence: fence.clone(),
        },
    )
    .await
    .unwrap();
    assert!(decision.new_dispatch);
    let dispatch_id = decision.call.dispatch_id.clone().unwrap();
    // The registered host's actual application and provider capture shapes.
    let mut messages = vec![json!({"role":"system","content":MANDATORY})];
    for instruction in &request.projection.instructions {
        messages.push(json!({"role":"system","content":instruction}));
    }
    messages.push(json!({"role":"user","content":request.task_input_utf8}));
    let provider_request_id = format!("ag095-provider-{ordinal}");
    let response = json!({"id":provider_request_id,"model":provider.provider_model,
        "choices":[{"message":{"content":output}}],
        "usage":{"prompt_tokens":7,"completion_tokens":3}});
    let response_utf8 = serde_json::to_string(&response).unwrap();
    let usage_record_id = format!(
        "usage-{}",
        &fingerprint(&(provider_request_id.as_str(), &response["usage"])).unwrap()[..32]
    );
    let capture = FormalProviderCaptureRecord {
        id: format!("formal-capture-{}", &hash(dispatch_id.as_bytes())[..32]),
        schema_version: "rsia.actual_formal_provider_capture.v1".into(),
        provider_registration_id: provider.id.clone(),
        provider_registration_digest: fingerprint(provider).unwrap(),
        host_registration_id: host_registration.id.clone(),
        host_registration_digest: fingerprint(host_registration).unwrap(),
        host_principal: host_registration.host_principal.clone(),
        // The simulated host process is this test process, never the grader.
        host_pid: std::process::id(),
        host_uid: host_registration.host_uid,
        host_code_digest: host_registration.code_digest.clone(),
        host_workspace: host_registration.host_workspace.clone(),
        dispatch_id: dispatch_id.clone(),
        request_digest: view.request_digest.clone(),
        application_digest: fingerprint(&messages).unwrap(),
        bundle_digest: view.bundle_digest.clone(),
        input_digest: view.input_digest.clone(),
        output_digest: hash(output.as_bytes()),
        provider_request_id: provider_request_id.clone(),
        usage_record_id: usage_record_id.clone(),
        provider_response_digest: hash(response_utf8.as_bytes()),
        provider_response_utf8: response_utf8,
        actual_model_digest: provider.model_digest.clone(),
        input_tokens: 7,
        output_tokens: 3,
        // ceil((7 + 3) * 1 micro / 1M tokens) under the registered pricing.
        actual_cost_micros: 1,
        currency: provider.currency.clone(),
        pricing_version: provider.pricing_version.clone(),
    };
    let capture_digest = fingerprint(&capture).unwrap();
    let settled = store
        .settle_model_budget_call(
            &host,
            &BudgetCallFence {
                now: now + 1,
                ..fence.clone()
            },
            &UsageCharge {
                amount_micros: capture.actual_cost_micros,
                currency: capture.currency.clone(),
                pricing_version: capture.pricing_version.clone(),
                provider_request_id: provider_request_id.clone(),
                usage_record_id,
                output_digest: capture.output_digest.clone(),
            },
            &ModelCallSettlementEvidence {
                provenance: BudgetExecutionProvenance::ExternalProvider,
                actual_model_digest: Some(capture.actual_model_digest.clone()),
                transport_artifact: BudgetArtifact::from_serializable(
                    "rsia.actual_formal_provider_capture.v1",
                    &capture,
                )
                .unwrap(),
                usable_response: Some(
                    BudgetArtifact::from_serializable(
                        "rsia.formal_provider_response.v1",
                        &json!({"schema_version":"rsia.formal_provider_response.v1",
                            "request_digest":view.request_digest,"output":output,
                            "capture_id":capture.id,"capture_digest":capture_digest}),
                    )
                    .unwrap(),
                ),
                blocked_response: BudgetArtifact::from_serializable(
                    "rsia.blocked_formal_provider_response.v1",
                    &json!({"schema_version":"rsia.blocked_formal_provider_response.v1",
                        "request_digest":view.request_digest,"output_digest":capture.output_digest,
                        "capture_id":capture.id,"capture_digest":capture_digest,
                        "reason":"formal_response_not_usable"}),
                )
                .unwrap(),
                forced_block_reason: None,
            },
        )
        .await
        .unwrap();
    assert!(settled.response_usable);
    store
        .close_budget_call_execution(
            &host,
            BILLING,
            &call_id,
            &dispatch_id,
            "formal_provider_capture_persisted",
            now + 2,
        )
        .await
        .unwrap();
    let receipt = IndependentEvaluationControl::record_execution_receipt(
        &host,
        store,
        ExecutionReceiptRequest {
            receipt_id: format!("formal-execution-{}", &hash(dispatch_id.as_bytes())[..32]),
            ticket_id: TICKET.into(),
            target_id: target.into(),
            side,
            request_digest: view.request_digest,
            output_artifact_id: format!("formal-output-{}", &hash(dispatch_id.as_bytes())[..32]),
            output_utf8: output.into(),
            budget_call_id: call_id,
            latency_micros: 100,
            issued_at_unix_seconds: now + 3,
        },
    )
    .await
    .unwrap();
    (receipt.receipt_id, capture)
}

/// The registered host's persisted provider captures: Host content witness
/// plus the capture -> candidate/provider/host edges.
async fn persist_captures(
    store: &Store,
    candidate: &ReleaseCandidateRecord,
    captures: &[FormalProviderCaptureRecord],
) {
    let host = ctx(Role::Host, HOST);
    let mut s = store.session().await.unwrap();
    for capture in captures {
        put_witnessed(&mut s, Role::Host, HOST, "artifact", &capture.id, capture).await;
        for id in [
            candidate.id.as_str(),
            capture.provider_registration_id.as_str(),
            capture.host_registration_id.as_str(),
        ] {
            s.put_edge(&host, "artifact", &capture.id, "artifact", id)
                .await
                .unwrap();
        }
    }
    s.commit().await.unwrap();
}

/// Registers, executes and grades one complete batch and records its sealed
/// complete-episode cost. The ticket ends ReadyForFinal and the store closed.
async fn prepare() -> Fx {
    let db_dir = tempfile::tempdir().unwrap();
    let db = db_dir.path().join("ag095.sqlite3");
    // The registered grader workspace is private to this test user (0700).
    let workspace_dir = tempfile::tempdir().unwrap();
    let workspace = std::fs::canonicalize(workspace_dir.path()).unwrap();
    std::fs::set_permissions(&workspace, std::fs::Permissions::from_mode(0o700)).unwrap();
    let grader_uid = std::fs::metadata(&workspace).unwrap().uid();
    let mut other_uids = (10_001_u32..).filter(|uid| *uid != grader_uid);
    let (host_uid, admin_uid, worker_uid) = (
        other_uids.next().unwrap(),
        other_uids.next().unwrap(),
        other_uids.next().unwrap(),
    );
    let grader_code = hash(&std::fs::read(env!("CARGO_BIN_EXE_formal_local_host")).unwrap());

    let base = trusted_compiler_inputs::inputs(
        NS,
        "ag095-profile",
        SkillSnapshot::empty(),
        Strategy::default(),
        HostCapabilities {
            available: Default::default(),
            granted: Default::default(),
        },
        &d("environment"),
    );
    let set = RegisteredPrincipalSet {
        id: format!("formal-principals-{}", base.host.id),
        schema_version: PRINCIPALS_SCHEMA.into(),
        namespace: NS.into(),
        host_registration_id: base.host.id.clone(),
        version: 1,
        host: principal(
            host_uid,
            HOST,
            "ag095-host-private-fixture-credential-32-bytes",
        ),
        grader: principal(grader_uid, GRADER, GRADER_SECRET),
        administrator: principal(
            admin_uid,
            ADMIN,
            "ag095-admin-private-fixture-credential-32b",
        ),
        worker: principal(
            worker_uid,
            PROPOSER,
            "ag095-worker-private-fixture-credential-32b",
        ),
    };
    let mut host = base.host.clone();
    host.host_uid = host_uid;
    host.grader_uid = grader_uid;
    host.administrator_uid = admin_uid;
    host.grader_workspace = workspace.to_str().unwrap().into();
    host.grader_code_digest = grader_code;
    host.grader_credential_environment_key = GRADER_KEY.into();
    host.host_principal = set.host.principal.clone();
    host.grader_principal = set.grader.principal.clone();
    host.administrator_principal = set.administrator.principal.clone();
    host.worker_principal = set.worker.principal.clone();
    host.credential_binding_digest = set.binding_digest().unwrap();
    set.validate(&host).unwrap();
    let mut provider = base.provider.clone();
    provider.credential_principal = host.host_principal.clone();
    provider.pricing_version = "pricing-v1".into();
    let meter = RegisteredResourceInstrumentation {
        id: "ag095-meter".into(),
        schema_version: METER_SCHEMA.into(),
        namespace: NS.into(),
        host_registration_id: host.id.clone(),
        endpoint: "https://meter.fixture.invalid/v1/episodes".into(),
        credential_environment_key: "RSIA_AG095_UNUSED_METER_CREDENTIAL".into(),
        credential_principal: d("ag095-meter-credential"),
        instrumentation_code_digest: d("ag095-meter-code"),
        authorized_by: ADMIN.into(),
    };
    let mut frozen = base.frozen.clone();
    frozen.resource_instrumentation_id = Some(meter.id.clone());
    let mut root = base.root.clone();
    root.profile_digest = fingerprint(&frozen).unwrap();
    let profile = Profile {
        id: frozen.profile_id.clone(),
        evolution_enabled: true,
        parent_digest: root.digest().unwrap(),
        baseline_digest: frozen.baseline_digest().unwrap(),
    };
    let candidate_bundle = compile(
        &profile,
        &root.skill,
        &root.strategy,
        &frozen,
        &SkillPatch {
            content: PatchOp::Set {
                value: "ag095 candidate skill".into(),
            },
            ..Default::default()
        },
        &ImproverPatch::default(),
    );
    let baseline_bundle = compile(
        &profile,
        &root.skill,
        &root.strategy,
        &frozen,
        &SkillPatch {
            content: PatchOp::ResetToBaseline,
            applicability: PatchOp::ResetToBaseline,
            counterexample: PatchOp::ResetToBaseline,
            required_capabilities: PatchOp::ResetToBaseline,
            dependencies: PatchOp::ResetToBaseline,
        },
        &ImproverPatch {
            instruction: PatchOp::ResetToBaseline,
            max_candidates: PatchOp::ResetToBaseline,
            max_rounds: PatchOp::ResetToBaseline,
        },
    );

    let store = Store::open(&db).await.unwrap();
    let admin = ctx(Role::Admin, ADMIN);
    FormalRegistry::register_host(&admin, &store, host.clone())
        .await
        .unwrap();
    FormalRegistry::register_provider(&admin, &store, provider.clone())
        .await
        .unwrap();
    FormalRegistry::register_resource_instrumentation(&admin, &store, meter.clone())
        .await
        .unwrap();
    FormalRegistry::freeze_profile(&admin, &store, frozen.clone())
        .await
        .unwrap();
    let parent_identity = FormalRegistry::authorize_empty_root(&admin, &store, root.clone())
        .await
        .unwrap();
    assert_eq!(parent_identity, profile.parent_digest);
    store
        .authorize_root_budget(
            &admin,
            &RootBudgetAuthorization {
                root_budget_id: "root-ag095".into(),
                billing_scope: BILLING.into(),
                allowed_namespaces: vec![NS.into()],
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                payment_subject: "fixture-only".into(),
                authorization_receipt_digest: d("admin-budget-receipt"),
                per_call_cap_micros: 100,
                total_limit_micros: 1_000,
                created_at: 1,
            },
        )
        .await
        .unwrap();

    let candidate_id = d("ag095-candidate");
    // Two trusted development sources for the candidate.
    let host_ctx = ctx(Role::Host, HOST);
    let mut refs = Vec::new();
    for id in ["S1", "S2"] {
        let authority = source(id);
        store_trace_authority(&store, &host_ctx, &authority)
            .await
            .unwrap();
        refs.push(TypedSourceRef {
            kind: "run".into(),
            id: id.into(),
            content_digest: authority.trace.source_digest,
        });
    }
    let mut s = store.session().await.unwrap();
    let watermark = s
        .bump_watermark(&host_ctx, &d("ag095-initial-watermark"))
        .await
        .unwrap();
    s.commit().await.unwrap();

    // Registered control, as the authenticated register_control operation
    // persists it: envelope and grader content witness (its grader principal
    // action is written with the other principal actions below).
    let mut v1 = ExperimentPlan::first_low_risk("formal-e05", 60).unwrap();
    v1.n_planned = ROTATING as usize;
    v1.profile = ProfileKind::QualityGain;
    v1.monetary_budget = money("0.001");
    v1.frozen = true;
    v1.frozen_at = Some(1);
    v1.validate().unwrap();
    let alpha = ResearchFamilyAlphaPlan::new(
        "family-e05",
        "0.05",
        vec![
            AlphaAllocation {
                claim_id: "fixed-gain".into(),
                attempt_id: ATTEMPT.into(),
                kind: FormalClaimKind::FixedSampleGain,
                alpha: "0.04".into(),
            },
            AlphaAllocation {
                claim_id: "early-reject".into(),
                attempt_id: ATTEMPT.into(),
                kind: FormalClaimKind::SequentialReject,
                alpha: "0.01".into(),
            },
        ],
    )
    .unwrap();
    let anchors = AnchorCoverageMatrix::new(
        "quality-gain",
        "anchors-v1",
        vec![CriticalCapabilityCoverage {
            capability_id: "safety".into(),
            anchor_ids: vec!["anchor-1".into()],
        }],
    )
    .unwrap();
    let mut budget = OptimizationBudgetPlan::unfunded(BILLING, "payer", "USD").unwrap();
    budget.root_total = money("0.001");
    budget.admin_authorization_receipt_digest = Some(d("admin-budget-receipt"));
    for stage in &mut budget.stages {
        stage.per_call_limit.pricing_version = "pricing-v1".into();
        stage.experiment_total.pricing_version = "pricing-v1".into();
    }
    budget.validate().unwrap();
    let comparison = OptimizerComparisonContract::new(
        optimizer(FixedOptimizerArm::CSimple, "simple"),
        optimizer(FixedOptimizerArm::CSkillopt, "skillopt"),
        FixedOptimizerArm::CSkillopt,
    )
    .unwrap();
    let formal_plan = FormalExperimentPlanV41::new(
        &v1,
        d("preregistration"),
        &alpha,
        &comparison,
        &budget,
        anchors.digest().unwrap(),
        d("sampling"),
        d("common-mean"),
        FormalStatisticalUnit::IndependentTaskCluster,
        ROTATING,
    )
    .unwrap();
    let grader_spec = FixedGraderSpec {
        schema_version: FixedGraderSpec::SCHEMA.into(),
        version: "exact-json-v1".into(),
        method: FixedGraderMethod::ExactJsonAnswerV1,
    };
    let control = RegisteredEvaluationControl {
        schema_version: RegisteredEvaluationControl::SCHEMA.into(),
        id: CONTROL_ID.into(),
        v1_plan_snapshot: v1.clone(),
        formal_plan,
        alpha_plan: alpha.clone(),
        optimizer_comparison: comparison,
        optimization_budget: budget,
        early_stop_plan: None,
        anchor_matrix: anchors.clone(),
        attempt_id: ATTEMPT.into(),
        fixed_sample_claim_id: "fixed-gain".into(),
        billing_scope: BILLING.into(),
        executor_actor: HOST.into(),
        evaluator_actor: GRADER.into(),
        proposer_actor: PROPOSER.into(),
        approver_actor: ADMIN.into(),
        oracle_digest: d("oracle-version"),
        grader_digest: fingerprint(&grader_spec).unwrap(),
        fixed_grader: grader_spec,
        evidence_scope: EvaluationEvidenceScope::RegisteredExternalHost,
        created_at_unix_seconds: 2,
    };
    control.validate().unwrap();
    let control_storage_id = storage_id(CONTROL_KIND, CONTROL_ID);
    let mut s = store.session().await.unwrap();
    put_witnessed(
        &mut s,
        Role::Evaluator,
        GRADER,
        "artifact",
        &control_storage_id,
        &Envelope {
            schema_version: ENVELOPE_SCHEMA,
            id: &control_storage_id,
            record_kind: CONTROL_KIND,
            payload: &control,
        },
    )
    .await;
    s.commit().await.unwrap();

    let ordered_clusters: Vec<_> = (0..ROTATING).map(|i| format!("cluster-{i}")).collect();
    let pair = FrozenCandidatePair::new(
        v1.digest().unwrap(),
        candidate_id.clone(),
        candidate_bundle.parent_digest.clone(),
        3,
    )
    .unwrap();
    let manifest = HoldoutManifest::issue_after_candidates_frozen(
        &pair,
        "family-e05",
        alpha.digest().unwrap(),
        SequentialMode::Disabled,
        "epoch-1",
        "slice-1",
        ordered_clusters.clone(),
        ordered_clusters
            .iter()
            .map(|id| d(&format!("source-{id}")))
            .collect(),
        "sampler-v1",
        d("oracle-version"),
        anchors.digest().unwrap(),
        control.grader_digest.clone(),
        d("environment"),
        d("private-seed"),
        4,
    )
    .unwrap();
    let rotating_inputs: Vec<_> = ordered_clusters
        .iter()
        .enumerate()
        .map(|(index, cluster)| ProtectedInputRef {
            target_id: format!("r{index}"),
            cluster_or_anchor_id: cluster.clone(),
            input_artifact_id: format!("input-r{index}"),
            input_digest: hash(task_input(&format!("r{index}")).as_bytes()),
        })
        .collect();
    let anchor_inputs = vec![ProtectedInputRef {
        target_id: "a0".into(),
        cluster_or_anchor_id: "anchor-1".into(),
        input_artifact_id: "input-a0".into(),
        input_digest: hash(task_input("a0").as_bytes()),
    }];
    let oracle_entries: Vec<_> = rotating_inputs
        .iter()
        .chain(anchor_inputs.iter())
        .map(|input| FrozenOracleEntry {
            target_id: input.target_id.clone(),
            expected_answer_json: json!("ok").to_string(),
        })
        .collect();
    let holdout = ProtectedHoldoutRecord {
        schema_version: ProtectedHoldoutRecord::SCHEMA.into(),
        id: HOLDOUT_ID.into(),
        registration_id: CONTROL_ID.into(),
        pair,
        manifest,
        candidate_bundle_digest: candidate_bundle.digest.clone(),
        baseline_bundle_digest: baseline_bundle.digest.clone(),
        rotating_inputs,
        anchor_inputs,
        oracle_payload_digest: fingerprint(&oracle_entries).unwrap(),
        oracle_entries,
    };
    let grader = ctx(Role::Evaluator, GRADER);
    IndependentEvaluationControl::register_holdout(&grader, &store, holdout)
        .await
        .unwrap();
    let ticket = IndependentEvaluationControl::issue_ticket(
        &grader,
        &store,
        IssueTicketRequest {
            ticket_id: TICKET.into(),
            registration_id: CONTROL_ID.into(),
            holdout_id: HOLDOUT_ID.into(),
            issued_at_unix_seconds: 5,
        },
    )
    .await
    .unwrap();
    assert!(ticket.candidate_dependency_binding.is_none());

    // Worker stage after the ticket: the unbound ticket wire, without GA-18
    // candidate/fact pins.
    let candidate = ReleaseStore::stage_bundle(
        &ctx(Role::Worker, PROPOSER),
        &store,
        StageBundleRequest {
            candidate_id: candidate_id.clone(),
            bundle: candidate_bundle.clone(),
            environment_digest: d("environment"),
            proposer_actor: PROPOSER.into(),
            sources: refs,
            revoke_watermark: watermark as u64,
        },
    )
    .await
    .unwrap();

    // Bootstrap observation of the four registered principals, plus the
    // Administrator/Worker/grader principal actions over the records they
    // authored.
    let mut s = store.session().await.unwrap();
    let observation = json!({
        "id": set.id,
        "schema_version": "rsia.formal_principal_set_observation.v1",
        "namespace": NS,
        "host_digest": fingerprint(&host).unwrap(),
        "principals": set,
        "grader_pid": std::process::id(),
        "grader_uid": host.grader_uid,
        "grader_executable_digest": host.grader_code_digest,
        "grader_workspace": host.grader_workspace,
    });
    put_witnessed(
        &mut s,
        Role::Admin,
        ADMIN,
        "artifact",
        &set.id,
        &observation,
    )
    .await;
    witness(
        &mut s,
        Role::Evaluator,
        GRADER,
        "artifact",
        &set.id,
        &observation,
    )
    .await;
    let profile_selection = format!("formal-profile-{}", frozen.profile_id);
    for (kind, id, operation) in [
        ("artifact", host.id.as_str(), "bootstrap"),
        ("artifact", provider.id.as_str(), "register_provider"),
        ("artifact", meter.id.as_str(), "register_meter"),
        ("artifact", frozen.id.as_str(), "freeze_profile"),
        ("artifact", profile_selection.as_str(), "freeze_profile"),
        ("artifact", root.id.as_str(), "empty_root"),
        (
            "formal_parent",
            profile.parent_digest.as_str(),
            "empty_root",
        ),
    ] {
        let subject = stored_digest(&mut s, kind, id).await;
        principal_action(
            &mut s,
            &host,
            &set,
            Role::Admin,
            kind,
            id,
            subject,
            operation,
        )
        .await;
    }
    principal_action(
        &mut s,
        &host,
        &set,
        Role::Worker,
        "artifact",
        &candidate.id,
        content_digest(&candidate).unwrap(),
        "stage",
    )
    .await;
    principal_action(
        &mut s,
        &host,
        &set,
        Role::Evaluator,
        "artifact",
        &control_storage_id,
        content_digest(&control).unwrap(),
        "register_control",
    )
    .await;
    s.commit().await.unwrap();

    // Registered host executions of every frozen member, both sides.
    let targets: Vec<String> = (0..ROTATING)
        .map(|i| format!("r{i}"))
        .chain(["a0".to_string()])
        .collect();
    let mut executed = Vec::new();
    let mut captures = Vec::new();
    for (index, target) in targets.iter().enumerate() {
        let baseline_answer = if target == "a0" { "ok" } else { "wrong" };
        let ordinal = u32::try_from(index).unwrap() * 2;
        let (candidate_receipt, candidate_capture) = execute(
            &store,
            &ticket,
            &provider,
            &host,
            &candidate,
            target,
            ExecutionSide::Candidate,
            &json!({"answer":"ok"}).to_string(),
            ordinal,
        )
        .await;
        let (baseline_receipt, baseline_capture) = execute(
            &store,
            &ticket,
            &provider,
            &host,
            &candidate,
            target,
            ExecutionSide::Baseline,
            &json!({ "answer": baseline_answer }).to_string(),
            ordinal + 1,
        )
        .await;
        executed.push((target.clone(), candidate_receipt, baseline_receipt));
        captures.extend([candidate_capture, baseline_capture]);
    }
    store.close().await;

    let mut fx = Fx {
        _db_dir: db_dir,
        db,
        _workspace: workspace_dir,
        workspace,
        host,
        set,
        candidate,
        graded: BTreeMap::new(),
    };
    // Independent grading by the registered grader process, one per member.
    for (index, (target, candidate_receipt, baseline_receipt)) in executed.iter().enumerate() {
        let receipt_id = format!("grade-{target}");
        let receipt = grader_process(
            &fx,
            json!({"operation":"grade","database":fx.db.to_str().unwrap(),"namespace":NS,
                "actor":GRADER,"receipt_id":receipt_id,"ticket_id":TICKET,"target_id":target,
                "candidate_execution_receipt_id":candidate_receipt,
                "baseline_execution_receipt_id":baseline_receipt,
                "issued_at_unix_seconds":500 + i64::try_from(index).unwrap()}),
        )
        .unwrap_or_else(|reason| panic!("registered grader refused {target}: {reason}"));
        assert_eq!(receipt["receipt_id"], receipt_id.as_str());
        fx.graded.insert(target.clone(), receipt_id);
    }

    // Administrator closes the ticket group and seals the root; the registered
    // meter then records the complete episode against that exact seal.
    assert_nothing_issued(&fx.db).await;
    let store = Store::open(&fx.db).await.unwrap();
    persist_captures(&store, &fx.candidate, &captures).await;
    store
        .stop_dispatch_group(&admin, BILLING, TICKET, "ag095_batch_closeout", 1_900)
        .await
        .unwrap();
    let mut s = store.session().await.unwrap();
    let seal = s
        .seal_root_budget(&admin, BILLING, "ag095_episode_seal", 2_000)
        .await
        .unwrap();
    s.commit().await.unwrap();
    assert!(seal.monetary_complete, "{:?}", seal.unresolved);
    let request = json!({"schema_version":"rsia.episode_resource_measurement_request.v1",
        "namespace":NS,"ticket_id":TICKET,"episode_id":ATTEMPT,
        "billing_scope":BILLING,"root_budget_id":seal.root.root_budget_id,
        "root_snapshot_digest":seal.snapshot_digest,"host_registration_id":fx.host.id,
        "call_count":seal.call_count,"dispatch_group_count":seal.dispatch_group_count,
        "stage_totals":seal.stage_totals});
    let request_digest = fingerprint(&request).unwrap();
    let response_utf8 = serde_json::to_string(&json!({
        "schema_version":"rsia.episode_resource_measurements.v1","id":"ag095-meter-reading",
        "request_digest":request_digest,"episode_id":ATTEMPT,
        "root_budget_id":seal.root.root_budget_id,"root_snapshot_digest":seal.snapshot_digest,
        "instrumentation_code_digest":meter.instrumentation_code_digest,
        "complete_episode_root":true,"cpu_nanoseconds":1,"storage_byte_seconds":2,
        "retained_history_bytes":3,"human_nanoseconds":4,"gray_release_nanoseconds":5}))
    .unwrap();
    let mut s = store.session().await.unwrap();
    let mut sources = Vec::new();
    for (dimension, unit, amount) in [
        ("cpu", "nanoseconds", 1),
        ("storage", "byte_seconds", 2),
        ("history", "bytes", 3),
        ("human", "nanoseconds", 4),
        ("gray_release", "nanoseconds", 5),
    ] {
        let id = format!("formal-meter-{TICKET}-{dimension}");
        let record = json!({"id":id,"schema_version":"rsia.trusted_resource_meter.v1",
            "episode_id":ATTEMPT,"ticket_id":TICKET,"billing_scope":BILLING,
            "root_snapshot_digest":seal.snapshot_digest,"host_registration_id":fx.host.id,
            "meter_registration_id":meter.id,"meter_registration_digest":fingerprint(&meter).unwrap(),
            "observed_pid":std::process::id(),"observed_uid":fx.host.host_uid,
            "dimension":dimension,"unit":unit,"amount":amount,
            "response_utf8":response_utf8,"response_digest":hash(response_utf8.as_bytes()),
            "request_digest":request_digest});
        put_witnessed(&mut s, Role::Host, HOST, "artifact", &id, &record).await;
        for to in [meter.id.as_str(), fx.host.id.as_str()] {
            s.put_edge(&host_ctx, "artifact", &id, "artifact", to)
                .await
                .unwrap();
        }
        sources.push(TypedContentRef {
            kind: "artifact".into(),
            id,
            schema_version: "rsia.trusted_resource_meter.v1".into(),
            content_digest: content_digest(&record).unwrap(),
        });
    }
    let cost = EpisodeCostRecord {
        id: format!("formal-cost-{TICKET}"),
        schema_version: "rsia.complete_episode_cost.v1".into(),
        ticket_id: TICKET.into(),
        profile_registration_id: frozen.id.clone(),
        measurements: CompleteEpisodeCostMeasurements {
            episode_id: ATTEMPT.into(),
            cpu_nanoseconds: Some(1),
            storage_byte_seconds: Some(2),
            retained_history_bytes: Some(3),
            human_nanoseconds: Some(4),
            gray_release_nanoseconds: Some(5),
            measurement_sources: sources.iter().map(|r| r.id.clone()).collect(),
        },
        measurement_sources: sources,
        observed_by: HOST.into(),
        observed_pid: std::process::id(),
        observed_uid: fx.host.host_uid,
    };
    put_witnessed(&mut s, Role::Host, HOST, "artifact", &cost.id, &cost).await;
    for source in &cost.measurement_sources {
        s.put_edge(&host_ctx, "artifact", &cost.id, &source.kind, &source.id)
            .await
            .unwrap();
    }
    s.commit().await.unwrap();
    store.close().await;
    fx
}

fn copy_database(fx: &Fx) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(fx.db.parent().unwrap()).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), dir.path().join(entry.file_name())).unwrap();
        }
    }
    let db = dir.path().join(fx.db.file_name().unwrap());
    (dir, db)
}

async fn stored_receipt(store: &Store, receipt_id: &str) -> Value {
    let mut s = store.session().await.unwrap();
    let envelope: Value = s
        .need(
            &ctx(Role::Evaluator, GRADER),
            "artifact",
            &storage_id(GRADER_RECEIPT_KIND, receipt_id),
        )
        .await
        .unwrap();
    s.commit().await.unwrap();
    envelope["payload"].clone()
}

async fn edit_stored_receipt(
    db: &Path,
    receipt_id: &str,
    edit: impl FnOnce(&mut Map<String, Value>),
) {
    let store = Store::open(db).await.unwrap();
    let grader = ctx(Role::Evaluator, GRADER);
    let id = storage_id(GRADER_RECEIPT_KIND, receipt_id);
    let mut s = store.session().await.unwrap();
    let mut envelope: Value = s.need(&grader, "artifact", &id).await.unwrap();
    edit(envelope["payload"].as_object_mut().unwrap());
    s.put(&grader, "artifact", &id, GRADER, &envelope)
        .await
        .unwrap();
    s.commit().await.unwrap();
    store.close().await;
}

type ReceiptEdit = Box<dyn FnOnce(&mut Map<String, Value>)>;

/// A capture that is a correctly witnessed grader record but names `digest`.
async fn rebind_capture(db: &Path, target: &str, digest: String) {
    let store = Store::open(db).await.unwrap();
    let grader = ctx(Role::Evaluator, GRADER);
    let id = grade_capture_id(target);
    let mut s = store.session().await.unwrap();
    let mut capture: IndependentGradeCapture = s.need(&grader, "artifact", &id).await.unwrap();
    assert_ne!(capture.grader_receipt_digest, digest);
    capture.grader_receipt_digest = digest;
    put_witnessed(&mut s, Role::Evaluator, GRADER, "artifact", &id, &capture).await;
    s.commit().await.unwrap();
    store.close().await;
}

/// A refused finalization leaves no report, no issued authority and the
/// ticket still ReadyForFinal.
async fn assert_nothing_issued(db: &Path) {
    let store = Store::open(db).await.unwrap();
    let grader = ctx(Role::Evaluator, GRADER);
    let mut s = store.session().await.unwrap();
    assert!(
        s.get::<Value>(&grader, "artifact", &format!("formal-authority-{TICKET}"))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        s.get::<Value>(&grader, "artifact", &storage_id(FORMAL_KIND, TICKET))
            .await
            .unwrap()
            .is_none()
    );
    let envelope: Value = s
        .need(&grader, "artifact", &storage_id(TICKET_KIND, TICKET))
        .await
        .unwrap();
    let ticket: EvaluationTicketV2 = serde_json::from_value(envelope["payload"].clone()).unwrap();
    assert_eq!(ticket.lifecycle, TicketLifecycle::ReadyForFinal);
    s.commit().await.unwrap();
    store.close().await;
}

#[tokio::test]
async fn registered_external_host_complete_batch_issues_authority_and_release_view_recognizes_it() {
    let fx = prepare().await;
    let formal = finalize(&fx, &fx.db)
        .unwrap_or_else(|reason| panic!("complete-batch authority issuance refused: {reason}"));
    assert_eq!(formal["variant"], "complete_batch");
    assert_eq!(formal["evidence_scope"], "registered_external_host");
    assert_eq!(formal["verdict"], "improved");

    let store = Store::open(&fx.db).await.unwrap();
    let grader = ctx(Role::Evaluator, GRADER);
    // Each capture names the one canonical digest of the stored receipt, not
    // its declaration-order encoding (which differs for this receipt type).
    for (target, receipt_id) in &fx.graded {
        let stored = stored_receipt(&store, receipt_id).await;
        let typed: IndependentGraderReceiptV2 = serde_json::from_value(stored.clone()).unwrap();
        let mut s = store.session().await.unwrap();
        let capture: IndependentGradeCapture = s
            .need(&grader, "artifact", &grade_capture_id(target))
            .await
            .unwrap();
        s.commit().await.unwrap();
        assert_eq!(
            capture.grader_receipt_digest,
            content_digest(&stored).unwrap()
        );
        assert_eq!(
            capture.grader_receipt_digest,
            content_digest(&typed).unwrap()
        );
        assert_ne!(capture.grader_receipt_digest, fingerprint(&typed).unwrap());
        assert_eq!(capture.grader_uid, fx.host.grader_uid);
        assert_eq!(capture.grader_workspace, fx.host.grader_workspace);
        assert_eq!(capture.grader_code_digest, fx.host.grader_code_digest);
        assert_ne!(capture.grader_pid, std::process::id());
    }

    let admin = ctx(Role::Admin, ADMIN);
    let authority = FormalAuthorityIssuer::revalidate(&admin, &store, TICKET, &fx.candidate.id)
        .await
        .unwrap();
    assert_eq!(authority.report_id, TICKET);
    assert_eq!(authority.candidate_id, fx.candidate.id);
    assert_eq!(authority.grader_captures.len(), fx.graded.len());
    assert_eq!(authority.provider_captures.len(), 2 * fx.graded.len());
    assert_eq!(
        authority.principal_set_digest,
        fx.set.binding_digest().unwrap()
    );

    // Release side: every condition the E06 approval gate checks on the view.
    for reader in [&admin, &grader] {
        let view = IndependentEvaluationControl::verified_report_view(reader, &store, TICKET)
            .await
            .unwrap();
        assert_eq!(view.variant, VerifiedReportVariant::CompleteBatch);
        assert_eq!(view.verdict, Some(Verdict::Improved));
        assert_eq!(
            view.anchor_status,
            AnchorEvidenceStatus::CompletePassedStoredClosure
        );
        assert!(view.anchor_evidence_digest.is_some());
        assert!(view.resource_evidence_digest.is_some());
        assert_eq!(
            view.evidence_scope,
            EvaluationEvidenceScope::RegisteredExternalHost
        );
        assert_eq!(
            view.cost_evidence_scope,
            Some(CostEvidenceScope::CompleteEpisodeRoot)
        );
        assert_eq!(
            view.dependency_status,
            DependencyEvidenceStatus::CompleteTypedContentClosure
        );
        assert!(view.promotion_eligible, "{:?}", view.ineligibility_reasons);
        assert!(view.ineligibility_reasons.is_empty());
        assert_eq!(view.candidate_digest, fx.candidate.id);
        assert_eq!(view.candidate_bundle_digest, fx.candidate.bundle_digest);
        assert_eq!(view.baseline_parent_digest, fx.candidate.parent_digest);
        assert_eq!(view.environment_digest, fx.candidate.environment_digest);
        assert_eq!(view.proposer_actor, fx.candidate.proposer_actor);
        assert_eq!(
            view.revoke_watermark
                .as_ref()
                .and_then(|(seq, _)| u64::try_from(*seq).ok()),
            Some(fx.candidate.revoke_watermark)
        );
        assert_eq!(view.approver_actor, ADMIN);
        assert_ne!(view.evaluator_actor, ADMIN);
    }
    store.close().await;
}

#[tokio::test]
async fn changed_stored_receipt_or_capture_digest_refuses_authority_issuance() {
    let fx = prepare().await;
    let rotating = fx.graded["r0"].clone();
    let other = fx.graded["r1"].clone();

    // Controls on copies of the same prepared batch: unchanged content and the
    // same content re-stored in another key order both issue.
    {
        let (_dir, db) = copy_database(&fx);
        let formal = finalize(&fx, &db)
            .unwrap_or_else(|reason| panic!("untampered control refused: {reason}"));
        assert_eq!(formal["variant"], "complete_batch");
    }
    {
        // Re-stored as a JSON value: the persisted key order is now sorted, no
        // longer the writer's declaration order; the content is unchanged.
        let (_dir, db) = copy_database(&fx);
        edit_stored_receipt(&db, &rotating, |_| {}).await;
        let formal = finalize(&fx, &db)
            .unwrap_or_else(|reason| panic!("same receipt content refused: {reason}"));
        assert_eq!(formal["variant"], "complete_batch");
    }

    // Changed receipt fields. Only the capture binds issued_at and the stored
    // explicit null; the typed evidence closure already refuses the others
    // before issuance is reached. Every case leaves nothing issued.
    let cases: Vec<(&str, ReceiptEdit, &str)> = vec![
        (
            "issued_at_unix_seconds",
            Box::new(|p| {
                let at = p["issued_at_unix_seconds"].as_i64().unwrap();
                p.insert("issued_at_unix_seconds".into(), json!(at + 1));
            }),
            "forbidden",
        ),
        (
            "anchor_result",
            Box::new(|p| {
                assert_eq!(p.remove("anchor_result"), Some(Value::Null));
            }),
            "forbidden",
        ),
        (
            "evaluator_actor",
            Box::new(|p| {
                p.insert("evaluator_actor".into(), json!("intruder-grader"));
            }),
            "conflict",
        ),
        (
            "candidate_score_micros",
            Box::new(|p| {
                p.insert("candidate_score_micros".into(), json!(0));
            }),
            "conflict",
        ),
        (
            "scored_output_pair_digest",
            Box::new(|p| {
                p.insert("scored_output_pair_digest".into(), json!(d("forged-pair")));
            }),
            "conflict",
        ),
    ];
    for (field, edit, expected) in cases {
        let (_dir, db) = copy_database(&fx);
        edit_stored_receipt(&db, &rotating, edit).await;
        assert_eq!(
            finalize(&fx, &db).unwrap_err(),
            expected,
            "changed receipt field {field}"
        );
        assert_nothing_issued(&db).await;
    }

    // A correctly witnessed capture that names another digest: the legacy
    // declaration-order encoding of the same receipt, or another receipt.
    let store = Store::open(&fx.db).await.unwrap();
    let same = stored_receipt(&store, &rotating).await;
    let legacy =
        fingerprint(&serde_json::from_value::<IndependentGraderReceiptV2>(same).unwrap()).unwrap();
    let another = content_digest(&stored_receipt(&store, &other).await).unwrap();
    store.close().await;
    for digest in [legacy, another] {
        let (_dir, db) = copy_database(&fx);
        rebind_capture(&db, "r0", digest).await;
        assert_eq!(finalize(&fx, &db).unwrap_err(), "forbidden");
        assert_nothing_issued(&db).await;
    }
}
