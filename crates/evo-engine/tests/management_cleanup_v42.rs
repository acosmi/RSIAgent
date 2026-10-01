//! AG-027 (E07/E08/E12; plan §6.1, §11.3-§11.5, V017): management jobs and their
//! private inputs reach the source-revocation cleanup closure, and the curriculum
//! replay and redacted-read paths fail closed. Real SQLite store, no model, no
//! provider, zero monetary cost.
//!
//! A management operation leaves two objects in the store: the private input
//! (kind `artifact`, schema `rsia.management_private_input.v1`, the complete
//! request payload) and the job (kind `job`, schema `rsia.management_job.v1`, the
//! record of the action). The private input depends on everything the request
//! reads (for `exploration.start` the runs of the world's source closure, for
//! `curriculum.step` the learner state), and the job depends on its input, so a
//! revoked run reaches both. Before this change the cleanup of such a run ended in
//! `blocked_unknown_scope:job:rsia.management_job.v1:` (after 51 nodes in the
//! controller's curriculum probe).
//!
//! Classification under test:
//! - job: preserved. It is the audit record of an action that happened and holds
//!   ids, digests, enums, counters, booleans and fixed-vocabulary strings only;
//! - private input: redacted. It is the whole request payload, content derived
//!   from sources;
//! - any other schema (or the same schema under another object kind) still fails
//!   the cleanup closed.
//!
//! Read side under test (dispatcher and curriculum coordinator):
//! - a Succeeded job whose private input was redacted is not served as live
//!   (`status` is a `Conflict` naming the redaction); a Failed, Cancelled or
//!   Blocked job carries no source-derived result and is still returned;
//! - a job that is Queued or Running when its input is redacted ends Failed with
//!   `error_code = source_revoked` (never retried, never a panic) and startup
//!   recovery keeps working on such a store;
//! - `schedule_probe_idempotent` no longer hands out a recorded probe job without
//!   re-verifying the learner state, and a redacted curriculum record is named
//!   (`Conflict` with record kind and id) instead of reported as `Internal`.
//!
//! Which management operations a run revocation can reach (walked against the
//! edges the production code writes):
//! - `curriculum.step`: run -> development stage facts -> learner state -> input;
//! - `exploration.start`: run -> input (direct edge) and run -> world -> input;
//! - `replay.run`: run -> replay world -> replay pool -> input;
//! - `experiment.register`: only through a holdout input artifact that is itself
//!   derived from the run (its input ids are operator-supplied references; the
//!   control and the holdout record have no edge to a run);
//! - `evaluation.start` / `evaluation.status`: not reachable. Their inputs depend
//!   on the registered control, the protected holdout and the ticket, none of
//!   which has an edge to a run or an import source.
//!
//! The fixtures below are copied from `tests/curriculum_cleanup_v42.rs`,
//! `tests/dispatch_management.rs` and `tests/streaming_evaluator.rs` (test crates
//! cannot import each other); the originals are untouched.

use evo_core::{Context, Error, Role, fingerprint};
use evo_engine::dispatch::{
    ManagementDispatcher, ManagementJob, ManagementJobState, ManagementResult,
};
use evo_storage::Store;
use evo_storage::lifecycle::{CleanupState, LifecycleStore};
use serde_json::{Value, json};
use std::time::Duration;

use common::*;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

mod common {
    use super::*;
    use evo_core::hash;
    use evo_storage::lifecycle::{CleanupStatus, TypedObjectRef};
    use std::collections::BTreeSet;

    pub const REDACTED: &str = "rsia.redacted.v1";
    pub const INPUT_SCHEMA: &str = "rsia.management_private_input.v1";
    pub const JOB_SCHEMA: &str = "rsia.management_job.v1";

    pub fn d(label: &str) -> String {
        hash(label.as_bytes())
    }

    /// The stored body of one object, straight from the store, so the assertions
    /// do not depend on any consumer's read path.
    pub async fn raw(store: &Store, ctx: &Context, kind: &str, id: &str) -> Value {
        let mut session = store.session().await.unwrap();
        let value = session.need(ctx, kind, id).await.unwrap();
        session.commit().await.unwrap();
        value
    }

    pub async fn try_raw(store: &Store, ctx: &Context, kind: &str, id: &str) -> Option<Value> {
        let mut session = store.session().await.unwrap();
        let value = session.get(ctx, kind, id).await.unwrap();
        session.commit().await.unwrap();
        value
    }

    pub async fn put_raw(store: &Store, ctx: &Context, kind: &str, id: &str, body: &Value) {
        let mut session = store.session().await.unwrap();
        session.put(ctx, kind, id, "admin", body).await.unwrap();
        session.commit().await.unwrap();
    }

    pub async fn put_edge(store: &Store, ctx: &Context, src: (&str, &str), dst: (&str, &str)) {
        let mut session = store.session().await.unwrap();
        session
            .put_edge(ctx, src.0, src.1, dst.0, dst.1)
            .await
            .unwrap();
        session.commit().await.unwrap();
    }

    pub async fn put_job(store: &Store, ctx: &Context, job: &ManagementJob) {
        let mut session = store.session().await.unwrap();
        session
            .put(ctx, "job", &job.id, ctx.actor(), job)
            .await
            .unwrap();
        session.commit().await.unwrap();
    }

    pub async fn job_record(store: &Store, ctx: &Context, job_id: &str) -> ManagementJob {
        let mut session = store.session().await.unwrap();
        let job = session.need(ctx, "job", job_id).await.unwrap();
        session.commit().await.unwrap();
        job
    }

    /// Everything reachable from `source` over "depends on" edges: what the
    /// cleanup frontier expands.
    pub async fn closure(
        store: &Store,
        ctx: &Context,
        source: (&str, &str),
    ) -> BTreeSet<(String, String)> {
        let mut seen = BTreeSet::new();
        let mut pending = vec![(source.0.to_string(), source.1.to_string())];
        let mut session = store.session().await.unwrap();
        while let Some((kind, id)) = pending.pop() {
            for dependent in session.dependents(ctx, &kind, &id).await.unwrap() {
                if seen.insert(dependent.clone()) {
                    pending.push(dependent);
                }
            }
        }
        session.commit().await.unwrap();
        seen
    }

    pub fn reaches(reach: &BTreeSet<(String, String)>, kind: &str, id: &str) -> bool {
        reach.contains(&(kind.to_string(), id.to_string()))
    }

    pub async fn begin(store: &Store, admin: &Context, run: &str) -> CleanupStatus {
        LifecycleStore::begin_revoke(
            admin,
            store,
            TypedObjectRef {
                kind: "run".into(),
                id: run.into(),
            },
            "source revoked",
            500,
        )
        .await
        .unwrap()
    }

    /// Drives the cleanup job until it stops moving.
    pub async fn drive(
        store: &Store,
        admin: &Context,
        mut status: CleanupStatus,
        page: usize,
    ) -> CleanupStatus {
        for now in 501..2_500 {
            if matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
                break;
            }
            status = LifecycleStore::cleanup_step(admin, store, &status.job_id, page, now)
                .await
                .unwrap();
        }
        status
    }

    pub async fn revoke_and_clean(store: &Store, admin: &Context, run: &str) -> CleanupStatus {
        let status = begin(store, admin, run).await;
        drive(store, admin, status, 8).await
    }

    /// Polls the stored job (not `status`, which refuses after a revocation)
    /// until it is terminal.
    pub async fn wait_job(store: &Store, ctx: &Context, job_id: &str) -> ManagementJob {
        for _ in 0..2_000 {
            let job = job_record(store, ctx, job_id).await;
            if matches!(
                job.state,
                ManagementJobState::Succeeded
                    | ManagementJobState::Failed
                    | ManagementJobState::Cancelled
                    | ManagementJobState::Blocked
            ) {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("management job did not finish");
    }

    /// The exact shape the cleanup leaves in place of a private input: identity,
    /// kind and schema of what was there, its digest, and no metadata (none of
    /// the request's fields is on the cleanup's metadata allow-list).
    pub fn assert_redacted_input(body: &Value, id: &str) {
        let digest = body["original_digest"].as_str().unwrap_or_default();
        assert_eq!(digest.len(), 64, "{body}");
        assert_eq!(
            body,
            &json!({
                "id": id,
                "schema_version": REDACTED,
                "state": "source_revoked",
                "original_kind": "artifact",
                "original_schema": INPUT_SCHEMA,
                "original_digest": digest,
                "metadata": {},
            }),
            "a redacted private input keeps its identity and nothing of the request"
        );
    }

    /// The `Conflict` message, failing on any other outcome (above all on
    /// `Internal`, which is what an expected state must never be reported as).
    pub fn expect_conflict<T: std::fmt::Debug>(result: Result<T, Error>, what: &str) -> String {
        match result {
            Err(Error::Conflict(message)) => message,
            other => panic!("{what}: expected a Conflict, got {other:?}"),
        }
    }

    /// A `Conflict` that names the redaction of `record`.
    pub fn assert_names_redaction(message: &str, record: &[&str]) {
        assert!(message.contains("redacted"), "{message}");
        for part in record {
            assert!(message.contains(part), "{message} lacks {part}");
        }
    }
}

// ---------------------------------------------------------------------------
// Fixture: trusted runs, source selection and world of `exploration.start`
// (copied from tests/dispatch_management.rs)
// ---------------------------------------------------------------------------

mod exploration_fx {
    use super::*;
    use evo_core::evidence::{ExecutionAttestation, Purpose, SourceSelection, TaskOrigin};
    use evo_core::hash;
    use evo_core::optimization::{
        OptimizationTrace, SkillFailureDiagnosis, SkillFailureKind, TraceOutcome,
    };
    use evo_core::skill_edit::EvidenceRef;
    use evo_core::strategy::{ElasticPolicyV1, ExplorationCapsV1, SimulationContext};
    use evo_engine::evidence::{
        StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
    };
    use evo_engine::exploration::{
        ExplorationDependency, ExplorationWorldV1, RootOpportunity, WorldState,
    };

    pub const RUNS: [&str; 2] = ["run-failure", "run-success"];
    pub const UNRELATED_RUN: &str = "run-unrelated";

    pub fn world(id: &str, source_watermark: u64) -> ExplorationWorldV1 {
        let parent_skill = hash(b"parent-skill");
        let parent_bundle = hash(b"parent-bundle");
        let environment = hash(b"environment");
        let model = hash(b"model");
        let tools = hash(b"tools");
        let grader = hash(b"grader");
        let rules = hash(b"rules");
        let context_signature = fingerprint(&(
            &parent_skill,
            &parent_bundle,
            &environment,
            &model,
            &tools,
            &grader,
            &rules,
            source_watermark,
        ))
        .unwrap();
        ExplorationWorldV1 {
            schema_version: ExplorationWorldV1::SCHEMA.into(),
            id: id.into(),
            approved_parent_digest: hash(b"approved-parent"),
            context_signature,
            parent_skill_digest: parent_skill,
            parent_bundle_digest: parent_bundle,
            environment_digest: environment,
            model_digest: model,
            tools_digest: tools,
            grader_digest: grader,
            rules_digest: rules,
            source_watermark,
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
            dependencies: RUNS
                .iter()
                .map(|run| ExplorationDependency {
                    kind: "run".into(),
                    id: (*run).into(),
                })
                .collect(),
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

    pub async fn store_run(store: &Store, id: &str, family: &str, outcome: TraceOutcome) {
        let host = Context::new("n", "host", Role::Host).unwrap();
        let authority = StoredTraceAuthority {
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
        };
        store_trace_authority(store, &host, &authority)
            .await
            .unwrap();
    }

    pub fn selection() -> SourceSelection {
        SourceSelection {
            roots: vec![],
            run_ids: RUNS.iter().map(|run| (*run).into()).collect(),
            purpose: Purpose::Development,
            allow_model_excerpts: true,
        }
    }

    /// Id of the stored source selection artifact; it depends on both runs.
    pub fn selection_id() -> String {
        format!("optgrant-{}", fingerprint(&selection()).unwrap())
    }

    /// Two Host-issued trace authorities, a source selection grant and one
    /// watermark bump so `source_watermark == 1`.
    pub async fn setup(store: &Store) {
        let host = Context::new("n", "host", Role::Host).unwrap();
        store_run(store, "run-failure", "family-a", TraceOutcome::TaskFailure).await;
        store_run(store, "run-success", "family-b", TraceOutcome::Success).await;
        store_source_selection(store, &host, &selection())
            .await
            .unwrap();
        let mut session = store.session().await.unwrap();
        session.bump_watermark(&host, "e09-initial").await.unwrap();
        session.commit().await.unwrap();
    }

    pub fn start_request(request_key: &str, world: &ExplorationWorldV1) -> Value {
        json!({
            "schema_version": "rsia.management.exploration_start.v1",
            "request_key": request_key,
            "world": serde_json::to_value(world).unwrap(),
        })
    }

    pub async fn open(name: &str) -> (tempfile::TempDir, Store, Context) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join(name)).await.unwrap();
        let admin = Context::new("n", "admin", Role::Admin).unwrap();
        (dir, store, admin)
    }
}

// ---------------------------------------------------------------------------
// Fixture: registered runner, development cycles and learner state of
// `curriculum.step` (copied from tests/curriculum_cleanup_v42.rs)
// ---------------------------------------------------------------------------

mod curriculum_fx {
    use super::*;
    use evo_core::curriculum::{CurriculumControlProfileV1, LearnerStateV2};
    use evo_core::evaluation::DataUse;
    use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
    use evo_core::hash;
    use evo_core::optimization::{OptimizationTrace, TraceOutcome};
    use evo_engine::curriculum::{
        CurriculumSourceArtifactV1, CurriculumSourceKindV1, DevelopmentCycleReceiptV1,
        PersistentCurriculumCoordinator,
    };
    use evo_engine::curriculum_profiles::{
        RegisteredPureFunctionProfileV1, registered_profile_source_bodies,
    };
    use evo_engine::development::{
        DevelopmentControlV1, DevelopmentEvidenceScope, DevelopmentTaskSpecV1,
        RegisteredDevelopmentRunner, RegisteredTargetInputV1, register_development_control,
        registered_runner_digest, typed_receipt_closure,
    };
    use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
    use evo_engine::optimization::{
        DevRunner, DevelopmentRunReport, DevelopmentRunRequest, OPTIMIZATION_STAGE_FACT_SCHEMA,
        OptimizationJournal, OptimizationJournalStage, StageDependency, StageFact, StageFactKind,
        StoreOptimizationJournal,
    };
    use evo_engine::streaming_evaluator::{FixedGraderMethod, FixedGraderSpec};
    use evo_storage::budget::RootBudgetAuthorization;
    use std::sync::Arc;

    pub const NAMESPACE: &str = "n";
    pub const SCOPE: &str = "dev-scope";
    pub const CONTROL_ID: &str = "dev-control";
    pub const PROFILE_ID: &str = "pure_function_test_proposal.v1";
    pub const STATE_ID: &str = "learner-state";
    pub const ENVELOPE: &str = "rsia.curriculum_artifact_envelope.v1";

    pub const PROFILE_KIND: &str = "curriculum_profile_v1";
    pub const STATE_KIND: &str = "learner_state_v2";
    pub const JOB_KIND: &str = "coverage_probe_job_v1";
    pub const RECEIPT_KIND: &str = "probe_schedule_receipt_v1";

    pub fn storage_id(record_kind: &str, id: &str) -> String {
        format!("e12-{}", fingerprint(&(record_kind, id)).unwrap())
    }

    fn grader_spec() -> FixedGraderSpec {
        FixedGraderSpec {
            schema_version: FixedGraderSpec::SCHEMA.into(),
            version: "exact-json-v1".into(),
            method: FixedGraderMethod::ExactJsonAnswerV1,
        }
    }

    fn authority(id: &str, family: &str) -> StoredTraceAuthority {
        let body = format!("trusted-run-{id}");
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
        vec![
            DevelopmentTaskSpecV1::registered(
                "task-a",
                "family-a",
                RegisteredTargetInputV1::ClampI64 {
                    value: -2,
                    min: -1,
                    max: 1,
                },
            )
            .unwrap(),
            DevelopmentTaskSpecV1::registered(
                "task-b",
                "family-b",
                RegisteredTargetInputV1::ClampI64 {
                    value: 5,
                    min: 0,
                    max: 3,
                },
            )
            .unwrap(),
        ]
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
            candidate_bundle_digest: d("candidate-bundle"),
            environment_digest: control.environment_digest.clone(),
            grader_digest: control.grader_digest.clone(),
            rules_digest: control.rules_digest.clone(),
            tools_digest: control.tools_digest.clone(),
            revoke_watermark: 1,
            idempotency_key: format!("{request_id}-idem"),
        }
    }

    pub struct Fixture {
        pub _dir: tempfile::TempDir,
        pub store: Store,
        pub admin: Context,
        pub executor: Context,
        pub grader: Context,
        pub control: DevelopmentControlV1,
    }

    impl Fixture {
        async fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(&dir.path().join("management-cleanup.sqlite3"))
                .await
                .unwrap();
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
                .bump_watermark(&admin, "initial-watermark")
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
            register_development_control(&admin, &store, control.clone())
                .await
                .unwrap();
            Self {
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

        /// Commits the request/observation pair the way the E03 step and its
        /// recovery journal would: with run, watermark, and artifact extras.
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
                dependencies: extras.clone(),
                payload: serde_json::to_value(request).unwrap(),
            }
            .seal()
            .unwrap();
            let observed_fact = observed_fact(request, report, extras);
            journal.commit(request_fact.clone()).await.unwrap();
            journal.commit(observed_fact.clone()).await.unwrap();
            (request_fact.artifact_id, observed_fact.artifact_id)
        }
    }

    fn observed_fact(
        request: &DevelopmentRunRequest,
        report: &DevelopmentRunReport,
        extras: Vec<StageDependency>,
    ) -> StageFact {
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
        dependencies.extend(typed_receipt_closure(report).unwrap());
        dependencies.extend(extras);
        StageFact {
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
        .unwrap()
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

    pub fn curriculum_source_ids() -> Vec<String> {
        let registered = RegisteredPureFunctionProfileV1::clamp_i64();
        vec![
            registered.oracle_source_id,
            registered.runner_source_id,
            registered.target_source_id,
            "task-space-source".into(),
        ]
    }

    /// Registers the typed sources, the offline profile and the initial learner
    /// state.
    async fn curriculum(fixture: &Fixture) -> PersistentCurriculumCoordinator {
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
                json!({"buckets":["clamp-boundary"]}),
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
        let profile = CurriculumControlProfileV1::offline_default(
            PROFILE_ID,
            d("task-space"),
            registered.oracle_digest.clone(),
            registered.runner_digest.clone(),
        )
        .unwrap();
        let state = LearnerStateV2 {
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
            coverage_buckets: vec![],
            applied_assets: vec![],
            active_probe_job_id: None,
            last_trigger_window_digest: None,
            cooldown_remaining_cycles: 0,
        };
        coordinator
            .register_profile_and_state(profile, state)
            .await
            .unwrap();
        coordinator
    }

    /// Profile, learner state and two real registered development cycles through
    /// `record_cycle`: the learner state depends on the cycles' stage facts and
    /// those on `run-a` / `run-b`, which is the edge path a run revocation walks
    /// to the state and from there to everything that reads it.
    pub struct Chain {
        pub fixture: Fixture,
        pub coordinator: PersistentCurriculumCoordinator,
        /// (request fact id, observed fact id) of the two development cycles.
        pub facts: Vec<(String, String)>,
    }

    impl Chain {
        pub async fn build() -> Self {
            let fixture = Fixture::new().await;
            let coordinator = curriculum(&fixture).await;
            let mut facts = Vec::new();
            for (n, episode) in ["episode-1", "episode-2"].into_iter().enumerate() {
                let request =
                    build_request(&fixture.control, &format!("dev-request-{}", n + 1), episode);
                let report = fixture.runner().run(request.clone()).await.unwrap();
                let (request_fact_id, observed_fact_id) =
                    fixture.commit_facts(&request, &report).await;
                let state = coordinator
                    .record_cycle(DevelopmentCycleReceiptV1 {
                        schema_version: "rsia.development_cycle_receipt.v1".into(),
                        id: format!("cycle-{}", n + 1),
                        state_id: STATE_ID.into(),
                        development_request_fact_id: request_fact_id.clone(),
                        development_observed_fact_id: observed_fact_id.clone(),
                    })
                    .await
                    .unwrap();
                assert_eq!(state.completed_cycles.len(), n + 1);
                facts.push((request_fact_id, observed_fact_id));
            }
            Self {
                fixture,
                coordinator,
                facts,
            }
        }

        /// A second record of the same cycle the chain already holds (a replay),
        /// or a new one when `id` is unknown.
        pub fn cycle(&self, id: &str) -> DevelopmentCycleReceiptV1 {
            DevelopmentCycleReceiptV1 {
                schema_version: "rsia.development_cycle_receipt.v1".into(),
                id: id.into(),
                state_id: STATE_ID.into(),
                development_request_fact_id: self.facts[1].0.clone(),
                development_observed_fact_id: self.facts[1].1.clone(),
            }
        }
    }

    pub fn step_request(request_key: &str) -> Value {
        json!({
            "schema_version": "rsia.management.curriculum_step.v1",
            "request_key": request_key,
            "profile_id": PROFILE_ID,
            "state_id": STATE_ID,
            "root_budget_limit_micros": 1_000_000u64,
        })
    }

    /// Every curriculum envelope, live or redacted, keyed by its storage id.
    pub async fn curriculum_bodies(
        store: &Store,
        ctx: &Context,
    ) -> std::collections::BTreeMap<String, Value> {
        let mut session = store.session().await.unwrap();
        let all: Vec<Value> = session.list(ctx, "artifact").await.unwrap();
        session.commit().await.unwrap();
        all.into_iter()
            .filter(|value| {
                value["schema_version"] == ENVELOPE
                    || (value["schema_version"] == REDACTED && value["original_schema"] == ENVELOPE)
            })
            .map(|value| (value["id"].as_str().unwrap().to_string(), value))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Fixture: sealed replay pool of `replay.run` (copied from
// tests/dispatch_management.rs)
// ---------------------------------------------------------------------------

mod replay_fx {
    use super::*;
    use evo_core::evidence::Purpose;
    use evo_core::replay::*;
    use evo_core::strategy::{ActionKindV1, ElasticPolicyV1, ExplorationCapsV1, ObservedStatus};
    use evo_storage::replay::{register_replay_pool, seal_replay_world};

    fn action(seq: u32, parent: &str, branch: u32, kind: ActionKindV1) -> ReplayActionSpecV1 {
        ReplayActionSpecV1 {
            record_seq: seq,
            generation_signature: d("generation-v1"),
            parent_context_signature: parent.into(),
            branch_seq: branch,
            target_depth: if matches!(kind, ActionKindV1::Widen { .. }) {
                1
            } else {
                2
            },
            action_kind: kind,
            estimated_cost_upper_micros: Some(10),
            writes_shared_workspace: false,
        }
    }

    fn transition(
        seq: u32,
        id: &str,
        parent: &str,
        next: &str,
        kind: ActionKindV1,
        outcome: ReplayTransitionOutcome,
    ) -> ReplayTransitionV2 {
        ReplayTransitionV2 {
            record_id: id.into(),
            record_seq: seq,
            generation_signature: d("generation-v1"),
            parent_context_signature: parent.into(),
            action_kind: kind,
            next_context_signature: next.into(),
            outcome,
            actual_usage: HistoricalUsage {
                input_tokens: 1,
                output_tokens: 1,
                cost_micros: Some(1),
                latency_millis: Some(1),
            },
            source_ids: vec![format!("source-{seq}")],
            observation_source_id: format!("source-{seq}"),
        }
    }

    fn valid(q: u32) -> ReplayTransitionOutcome {
        ReplayTransitionOutcome::Observed {
            status: ObservedStatus::Valid { quality_micros: q },
        }
    }

    fn refresh_evidence_digests(world: &mut ReplayWorldV2) {
        let digest = replay_baseline_observation_digest(&world.manifest).unwrap();
        let source_id = world.manifest.baseline_observation_source_id.clone();
        world
            .manifest
            .source_closure
            .iter_mut()
            .find(|source| source.source_id == source_id)
            .unwrap()
            .content_digest = digest;
        let observation_digests: Vec<_> = world
            .transitions
            .iter()
            .map(|transition| {
                (
                    transition.observation_source_id.clone(),
                    replay_observation_digest(&world.manifest, transition).unwrap(),
                )
            })
            .collect();
        for (source_id, digest) in observation_digests {
            world
                .manifest
                .source_closure
                .iter_mut()
                .find(|source| source.source_id == source_id)
                .unwrap()
                .content_digest = digest;
        }
    }

    fn sample_world() -> ReplayWorldV2 {
        let actions = vec![
            action(1, &d("baseline"), 1, ActionKindV1::Widen { root_slot: 1 }),
            action(
                2,
                &d("ctx-1"),
                1,
                ActionKindV1::Deepen { parent_node_seq: 1 },
            ),
        ];
        let transitions = vec![
            transition(
                1,
                "opaque-root",
                &d("baseline"),
                &d("ctx-1"),
                ActionKindV1::Widen { root_slot: 1 },
                valid(400_000),
            ),
            transition(
                2,
                "opaque-child",
                &d("ctx-1"),
                &d("ctx-2"),
                ActionKindV1::Deepen { parent_node_seq: 1 },
                valid(800_000),
            ),
        ];
        let mut world = ReplayWorldV2 {
            schema_version: REPLAY_WORLD_SCHEMA.into(),
            manifest: ReplayWorldManifestV2 {
                schema_version: REPLAY_MANIFEST_SCHEMA.into(),
                world_id: "world-1".into(),
                cluster_id: "cluster-1".into(),
                partition: WorldPartition::Select,
                purpose: Purpose::Development,
                generation_signature: d("generation-v1"),
                world_context_signature: d("world-context"),
                baseline_context_signature: d("baseline"),
                approved_parent_digest: d("approved-parent"),
                model_digest: d("model"),
                tools_digest: d("tools"),
                scorer_digest: d("scorer"),
                guidance_digest: d("guidance"),
                repair_template_digest: d("repair"),
                input_order_digest: d("order"),
                initial_baseline_quality_micros: 100_000,
                baseline_observation_source_id: "baseline-source-1".into(),
                source_closure: vec![
                    ReplaySourceRef {
                        source_id: "source-1".into(),
                        content_digest: String::new(),
                    },
                    ReplaySourceRef {
                        source_id: "source-2".into(),
                        content_digest: String::new(),
                    },
                    ReplaySourceRef {
                        source_id: "baseline-source-1".into(),
                        content_digest: d("pending-baseline"),
                    },
                ],
                revoke_watermark: 1,
                prefix_coverage: vec![
                    PrefixCoverageV1 {
                        context_signature: d("baseline"),
                        exhausted: false,
                    },
                    PrefixCoverageV1 {
                        context_signature: d("ctx-1"),
                        exhausted: false,
                    },
                    PrefixCoverageV1 {
                        context_signature: d("ctx-2"),
                        exhausted: true,
                    },
                ],
                action_catalog: actions,
            },
            transitions,
            sealed_digest: None,
        };
        refresh_evidence_digests(&mut world);
        world.seal().unwrap();
        world
    }

    fn sample_profile(pool_digest: String) -> ReplaySimulationProfile {
        ReplaySimulationProfile {
            simulation_version: SIMULATION_VERSION.into(),
            objective: ReplayObjective::ParetoAttainmentV2,
            w_sim: 1,
            probe_budget: 2,
            horizon: 2,
            lambda_work_micros: DEFAULT_LAMBDA_MICROS,
            lambda_round_micros: DEFAULT_LAMBDA_MICROS,
            fixed_seed: 9,
            global_recovery_dispatch_limit: 1,
            pool_digest,
            purpose: Purpose::Development,
            target_runtime_profile: "simulation-only".into(),
        }
    }

    /// Two sealed worlds (`world-1` over `source-1`, `source-2` and the baseline
    /// source, `world-train` over its own runs) and the pool that registers both.
    pub async fn setup_sealed_pool(ctx: &Context, store: &Store) -> ReplayPoolManifestV1 {
        let mut persisted = sample_world();
        persisted.sealed_digest = None;
        persisted.manifest.revoke_watermark = 1;
        persisted.seal().unwrap();

        let mut train = persisted.clone();
        train.sealed_digest = None;
        train.manifest.world_id = "world-train".into();
        train.manifest.cluster_id = "cluster-train".into();
        train.manifest.partition = WorldPartition::Train;
        train.manifest.baseline_observation_source_id = "baseline-source-train".into();
        train.manifest.source_closure[2].source_id = "baseline-source-train".into();
        for (index, transition) in train.transitions.iter_mut().enumerate() {
            let source_id = format!("train-source-{}", index + 1);
            transition.source_ids = vec![source_id.clone()];
            transition.observation_source_id = source_id.clone();
            train.manifest.source_closure[index].source_id = source_id;
        }
        refresh_evidence_digests(&mut train);
        train.seal().unwrap();

        let mut session = store.session().await.unwrap();
        for replay_world in [&persisted, &train] {
            for source in &replay_world.manifest.source_closure {
                let body =
                    if source.source_id == replay_world.manifest.baseline_observation_source_id {
                        replay_baseline_observation_bytes(&replay_world.manifest).unwrap()
                    } else {
                        let transition = replay_world
                            .transitions
                            .iter()
                            .find(|transition| transition.observation_source_id == source.source_id)
                            .unwrap();
                        replay_observation_bytes(&replay_world.manifest, transition).unwrap()
                    };
                let excerpt = String::from_utf8(body.clone()).unwrap();
                session
                    .put(
                        ctx,
                        "run",
                        &source.source_id,
                        "host",
                        &json!({
                            "schema_version":"rsia.optimization.source.v1",
                            "record":{"id":source.source_id,"body":body.clone(),"parent_family":format!("family-{}",source.source_id),"task_origin":"trusted_run","execution_attestation":"trusted_host","purpose":"development"},
                            "trace":{"run_id":source.source_id,"parent_family":format!("family-{}",source.source_id),"source_digest":source.content_digest,"purpose":"development","outcome":"success","diagnosis":null,"excerpt":excerpt,"seed":1},
                            "excerpt_start":0,"excerpt_end":body.len()
                        }),
                    )
                    .await
                    .unwrap();
            }
        }
        session.bump_watermark(ctx, &d("watermark")).await.unwrap();
        session.commit().await.unwrap();

        seal_replay_world(ctx, store, &persisted).await.unwrap();
        seal_replay_world(ctx, store, &train).await.unwrap();
        register_replay_pool(
            ctx,
            store,
            &[
                persisted.manifest.world_id.clone(),
                train.manifest.world_id.clone(),
            ],
        )
        .await
        .unwrap()
    }

    pub fn run_request(request_key: &str, pool: &ReplayPoolManifestV1) -> Value {
        json!({
            "schema_version": "rsia.management.replay_run.v1",
            "request_key": request_key,
            "pool_digest": pool.pool_digest,
            "partition": "select",
            "policy": ElasticPolicyV1::default(),
            "profile": sample_profile(pool.pool_digest.clone()),
            "caps": ExplorationCapsV1::online(),
        })
    }
}

// ---------------------------------------------------------------------------
// Fixture: registered evaluation control, protected holdout and ticket of
// `experiment.register` / `evaluation.start` / `evaluation.status` (copied from
// tests/streaming_evaluator.rs, without the sequential-stopping variant)
// ---------------------------------------------------------------------------

mod e05_fx {
    use super::*;
    use evo_core::evaluation::{
        ControlCondition, ExperimentPlan, FixedOptimizerArm, FixedOptimizerSpec,
        FormalExperimentPlanV41, FormalStatisticalUnit, Money, OptimizationBudgetPlan,
        OptimizerComparisonContract,
    };
    use evo_core::holdout::{
        AnchorCoverageMatrix, CriticalCapabilityCoverage, FrozenCandidatePair, HoldoutManifest,
        SequentialMode,
    };
    use evo_core::sequential::{AlphaAllocation, FormalClaimKind, ResearchFamilyAlphaPlan};
    use evo_engine::dispatch::{
        EvaluationStartRequest, EvaluationStatusRequest, ExperimentRegisterRequest,
        IssueTicketRequestWire,
    };
    use evo_engine::streaming_evaluator::{
        EvaluationEvidenceScope, FixedGraderMethod, FixedGraderSpec, FrozenOracleEntry,
        IndependentEvaluationControl, IssueTicketRequest, ProtectedHoldoutRecord,
        ProtectedInputRef, RegisteredEvaluationControl,
    };
    use evo_storage::budget::RootBudgetAuthorization;

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
            root_budget_scope_id: "billing-1".into(),
            budget_limit: money("0.001"),
        }
    }

    pub struct E05 {
        pub _dir: tempfile::TempDir,
        pub store: Store,
        pub evaluator: Context,
        pub ticket_id: String,
        pub holdout: ProtectedHoldoutRecord,
        pub control: RegisteredEvaluationControl,
    }

    /// The control, the holdout and one issued ticket, registered directly. The
    /// first rotating input of the holdout names `first_input_artifact` (an
    /// operator-supplied reference to an input artifact); the others are opaque
    /// ids of artifacts that do not exist.
    pub async fn fixture(first_input_artifact: &str) -> E05 {
        let n = 5u32;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("e05.sqlite3")).await.unwrap();
        let admin = Context::new("n", "admin", Role::Admin).unwrap();
        let evaluator = Context::new("n", "evaluator", Role::Evaluator).unwrap();
        store
            .authorize_root_budget(
                &admin,
                &RootBudgetAuthorization {
                    root_budget_id: "root-1".into(),
                    billing_scope: "billing-1".into(),
                    allowed_namespaces: vec!["n".into(), "other".into()],
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

        let mut v1 = ExperimentPlan::first_low_risk("formal-e05").unwrap();
        v1.n_planned = n as usize;
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
                    attempt_id: "attempt-1".into(),
                    kind: FormalClaimKind::FixedSampleGain,
                    alpha: "0.04".into(),
                },
                AlphaAllocation {
                    claim_id: "early-reject".into(),
                    attempt_id: "attempt-1".into(),
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
        let mut budget = OptimizationBudgetPlan::unfunded("billing-1", "payer", "USD").unwrap();
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
        assert!(v1.conditions.contains(&ControlCondition::FixedImproverC));
        let formal = FormalExperimentPlanV41::new(
            &v1,
            d("preregistration"),
            &alpha,
            &comparison,
            &budget,
            anchors.digest().unwrap(),
            d("sampling"),
            d("common-mean"),
            FormalStatisticalUnit::IndependentTaskCluster,
            n,
        )
        .unwrap();
        let ordered_clusters: Vec<_> = (0..n).map(|index| format!("cluster-{index}")).collect();
        let grader = FixedGraderSpec {
            schema_version: FixedGraderSpec::SCHEMA.into(),
            version: "exact-json-v1".into(),
            method: FixedGraderMethod::ExactJsonAnswerV1,
        };
        let control = RegisteredEvaluationControl {
            schema_version: RegisteredEvaluationControl::SCHEMA.into(),
            id: "control-1".into(),
            v1_plan_snapshot: v1.clone(),
            formal_plan: formal,
            alpha_plan: alpha.clone(),
            optimizer_comparison: comparison,
            optimization_budget: budget,
            early_stop_plan: None,
            anchor_matrix: anchors.clone(),
            attempt_id: "attempt-1".into(),
            fixed_sample_claim_id: "fixed-gain".into(),
            billing_scope: "billing-1".into(),
            executor_actor: "executor".into(),
            evaluator_actor: "evaluator".into(),
            proposer_actor: "proposer".into(),
            approver_actor: "approver".into(),
            oracle_digest: d("oracle-version"),
            grader_digest: fingerprint(&grader).unwrap(),
            fixed_grader: grader,
            evidence_scope: EvaluationEvidenceScope::ProgramFixture,
            created_at_unix_seconds: 2,
        };
        IndependentEvaluationControl::register_control(&evaluator, &store, control.clone())
            .await
            .unwrap();

        let pair = FrozenCandidatePair::new(v1.digest().unwrap(), d("candidate"), d("baseline"), 3)
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
            fingerprint(&FixedGraderSpec {
                schema_version: FixedGraderSpec::SCHEMA.into(),
                version: "exact-json-v1".into(),
                method: FixedGraderMethod::ExactJsonAnswerV1,
            })
            .unwrap(),
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
                input_artifact_id: if index == 0 {
                    first_input_artifact.to_string()
                } else {
                    format!("input-r{index}")
                },
                input_digest: d(&format!("input-r{index}")),
            })
            .collect();
        let anchor_inputs = vec![ProtectedInputRef {
            target_id: "a0".into(),
            cluster_or_anchor_id: "anchor-1".into(),
            input_artifact_id: "input-a0".into(),
            input_digest: d("input-a0"),
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
            id: "holdout-1".into(),
            registration_id: "control-1".into(),
            pair,
            manifest,
            candidate_bundle_digest: d("candidate-bundle"),
            baseline_bundle_digest: d("baseline-bundle"),
            rotating_inputs,
            anchor_inputs,
            oracle_payload_digest: fingerprint(&oracle_entries).unwrap(),
            oracle_entries,
        };
        IndependentEvaluationControl::register_holdout(&evaluator, &store, holdout.clone())
            .await
            .unwrap();
        let ticket = IndependentEvaluationControl::issue_ticket(
            &evaluator,
            &store,
            IssueTicketRequest {
                ticket_id: "ticket-1".into(),
                registration_id: "control-1".into(),
                holdout_id: "holdout-1".into(),
                issued_at_unix_seconds: 5,
            },
        )
        .await
        .unwrap();
        E05 {
            _dir: dir,
            store,
            evaluator,
            ticket_id: ticket.id,
            holdout,
            control,
        }
    }

    impl E05 {
        pub fn register_request(&self, request_key: &str) -> Value {
            serde_json::to_value(ExperimentRegisterRequest {
                schema_version: "rsia.management.experiment_register.v1".into(),
                request_key: request_key.into(),
                control: self.control.clone(),
                holdout: self.holdout.clone(),
            })
            .unwrap()
        }

        pub fn start_request(&self, request_key: &str) -> Value {
            serde_json::to_value(EvaluationStartRequest {
                schema_version: "rsia.management.evaluation_start.v1".into(),
                request_key: request_key.into(),
                ticket: IssueTicketRequestWire {
                    ticket_id: self.ticket_id.clone(),
                    registration_id: self.control.id.clone(),
                    holdout_id: self.holdout.id.clone(),
                    issued_at_unix_seconds: 5,
                },
            })
            .unwrap()
        }

        pub fn status_request(&self, request_key: &str) -> Value {
            serde_json::to_value(EvaluationStatusRequest {
                schema_version: "rsia.management.evaluation_status.v1".into(),
                request_key: request_key.into(),
                ticket_id: self.ticket_id.clone(),
            })
            .unwrap()
        }

        /// Storage ids of the three e05 artifacts the management inputs depend on.
        pub fn artifact_ids(&self) -> [String; 3] {
            let id = |record_kind: &str, id: &str| {
                format!("e05-{}", fingerprint(&(record_kind, id)).unwrap())
            };
            [
                id("registered_evaluation_control_v41", &self.control.id),
                id("protected_holdout_v41", &self.holdout.id),
                id("evaluation_ticket_v2", &self.ticket_id),
            ]
        }
    }
}

// ---------------------------------------------------------------------------
// 1. curriculum.step: run -> stage facts -> learner state -> private input -> job
// ---------------------------------------------------------------------------

#[tokio::test]
async fn curriculum_step_job_and_private_input_reach_the_cleanup_closure() {
    use curriculum_fx::*;
    let chain = Chain::build().await;
    let fixture = &chain.fixture;
    let (store, admin) = (&fixture.store, &fixture.admin);
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();

    let queued = dispatcher
        .submit(admin, "curriculum.step", step_request("curriculum-step-1"))
        .await
        .unwrap();
    let done = wait_job(store, admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    assert_eq!(done.step, "probe_scheduled");
    let Some(ManagementResult::ProbeScheduled { probe_job_id, .. }) = &done.result else {
        panic!("unexpected result: {:?}", done.result);
    };
    // Live: the read side agrees.
    assert_eq!(
        dispatcher.status(admin, &done.id).await.unwrap().state,
        ManagementJobState::Succeeded
    );

    // The edge path the cleanup walks: the input hangs off the learner state, the
    // job off its input, and the state off the run (through the stage facts).
    let reach = closure(store, admin, ("run", "run-a")).await;
    assert!(reaches(
        &reach,
        "artifact",
        &storage_id(STATE_KIND, STATE_ID)
    ));
    assert!(reaches(&reach, "artifact", &done.private_input_ref));
    assert!(reaches(&reach, "job", &done.id));

    let job_before = raw(store, admin, "job", &done.id).await;
    let input_before = raw(store, admin, "artifact", &done.private_input_ref).await;
    assert_eq!(input_before["schema_version"], INPUT_SCHEMA);
    assert_eq!(input_before["payload"]["profile_id"], PROFILE_ID);
    assert_eq!(input_before["payload"]["state_id"], STATE_ID);
    let records_before = curriculum_bodies(store, admin).await;
    for record_kind in [PROFILE_KIND, JOB_KIND, RECEIPT_KIND] {
        assert!(
            records_before
                .values()
                .any(|value| value["record_kind"] == record_kind),
            "{record_kind} is missing before the revocation"
        );
    }

    let status = revoke_and_clean(store, admin, "run-a").await;
    // Defect: this ended in `Failed: blocked_unknown_scope:job:rsia.management_job.v1:`.
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert_eq!(status.last_error, None);
    assert_eq!(status.pending_nodes, 0);

    // The private input is a redaction tombstone and holds nothing of the request.
    let input_after = raw(store, admin, "artifact", &done.private_input_ref).await;
    assert_redacted_input(&input_after, &done.private_input_ref);
    for needle in [
        PROFILE_ID,
        STATE_ID,
        "curriculum-step-1",
        "root_budget_limit",
    ] {
        assert!(
            !input_after.to_string().contains(needle),
            "{needle} survived the redaction of the private input"
        );
    }
    // The job is the record of the action: preserved byte for byte.
    assert_eq!(
        raw(store, admin, "job", &done.id).await,
        job_before,
        "the management job is preserved"
    );
    // The curriculum records next to it: the control, the probe job and the
    // schedule receipt survive, what was derived from the runs is redacted.
    let records_after = curriculum_bodies(store, admin).await;
    assert_eq!(
        records_after.keys().collect::<Vec<_>>(),
        records_before.keys().collect::<Vec<_>>(),
        "no curriculum record is deleted"
    );
    for (storage, before) in &records_before {
        let record_kind = before["record_kind"].as_str().unwrap();
        if [PROFILE_KIND, JOB_KIND, RECEIPT_KIND].contains(&record_kind) {
            assert_eq!(
                &records_after[storage], before,
                "{record_kind} is preserved byte for byte"
            );
        } else {
            assert_eq!(records_after[storage]["schema_version"], REDACTED);
        }
    }
    // The other run is not part of the closure.
    let other = raw(store, admin, "run", "run-b").await;
    assert_ne!(other["schema_version"], REDACTED);

    // The read side: a Conflict that names the redaction, never Internal.
    let message = expect_conflict(
        dispatcher.status(admin, &done.id).await,
        "status after the cleanup",
    );
    assert_names_redaction(&message, &[&done.private_input_ref]);
    let message = expect_conflict(
        evo_engine::curriculum::verified_probe_job_view(admin, store, probe_job_id).await,
        "probe job view after the cleanup",
    );
    assert_names_redaction(&message, &[STATE_KIND, STATE_ID]);
    // The key can no longer be replayed through the dispatcher.
    expect_conflict(
        dispatcher
            .submit(admin, "curriculum.step", step_request("curriculum-step-1"))
            .await,
        "resubmit of the cleaned job",
    );
    // A restarted service starts on this store and has nothing to recover.
    let restarted = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    assert_eq!(restarted.recover_pending().await.unwrap(), 0);
}

// ---------------------------------------------------------------------------
// 2. exploration.start: run -> private input (direct) and run -> world -> input
// ---------------------------------------------------------------------------

#[tokio::test]
async fn exploration_start_job_and_private_input_reach_the_cleanup_closure() {
    let (_dir, store, admin) = exploration_fx::open("exploration.sqlite3").await;
    exploration_fx::setup(&store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let world = exploration_fx::world("world-1", 1);
    let queued = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_fx::start_request("exploration-start-1", &world),
        )
        .await
        .unwrap();
    let done = wait_job(&store, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    assert_eq!(done.step, "exploration_started");
    assert!(dispatcher.status(&admin, &done.id).await.is_ok());

    let world_storage = evo_engine::exploration::exploration_world_storage_id("world-1").unwrap();
    let reach = closure(&store, &admin, ("run", "run-failure")).await;
    assert!(reaches(&reach, "artifact", &done.private_input_ref));
    assert!(reaches(&reach, "artifact", &world_storage));
    assert!(reaches(&reach, "job", &done.id));

    let job_before = raw(&store, &admin, "job", &done.id).await;
    let input_before = raw(&store, &admin, "artifact", &done.private_input_ref).await;
    assert_eq!(input_before["schema_version"], INPUT_SCHEMA);
    assert_eq!(input_before["payload"]["world"]["id"], "world-1");

    let status = revoke_and_clean(&store, &admin, "run-failure").await;
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert_eq!(status.last_error, None);
    assert_eq!(status.pending_nodes, 0);

    // The private input carried the whole world; nothing of it is left.
    let input_after = raw(&store, &admin, "artifact", &done.private_input_ref).await;
    assert_redacted_input(&input_after, &done.private_input_ref);
    for needle in ["world-1", "approved_parent_digest", "root_opportunities"] {
        assert!(
            !input_after.to_string().contains(needle),
            "{needle} survived the redaction of the private input"
        );
    }
    assert_eq!(
        raw(&store, &admin, "job", &done.id).await,
        job_before,
        "the management job is preserved"
    );
    let world_after = raw(&store, &admin, "artifact", &world_storage).await;
    assert_eq!(world_after["schema_version"], REDACTED, "{world_after}");
    let other = raw(&store, &admin, "run", "run-success").await;
    assert_ne!(other["schema_version"], REDACTED);

    let message = expect_conflict(
        dispatcher.status(&admin, &done.id).await,
        "status after the cleanup",
    );
    assert_names_redaction(&message, &[&done.private_input_ref]);
    // The exploration records name their own redaction (AG-024).
    let message = expect_conflict(
        evo_engine::exploration::verified_world_decision_view(&admin, &store, "world-1").await,
        "world decision view after the cleanup",
    );
    assert_names_redaction(&message, &["exploration_world_v1", "world-1"]);
    expect_conflict(
        dispatcher
            .submit(
                &admin,
                "exploration.start",
                exploration_fx::start_request("exploration-start-1", &world),
            )
            .await,
        "resubmit of the cleaned job",
    );
    let restarted = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    assert_eq!(restarted.recover_pending().await.unwrap(), 0);
}

#[tokio::test]
async fn revoking_a_run_no_management_request_reads_leaves_its_job_and_input_unchanged() {
    let (_dir, store, admin) = exploration_fx::open("isolation.sqlite3").await;
    exploration_fx::setup(&store).await;
    exploration_fx::store_run(
        &store,
        exploration_fx::UNRELATED_RUN,
        "family-x",
        evo_core::optimization::TraceOutcome::Success,
    )
    .await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_fx::start_request(
                "exploration-isolation",
                &exploration_fx::world("world-1", 1),
            ),
        )
        .await
        .unwrap();
    let done = wait_job(&store, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded);
    let world_storage = evo_engine::exploration::exploration_world_storage_id("world-1").unwrap();
    let before = (
        raw(&store, &admin, "job", &done.id).await,
        raw(&store, &admin, "artifact", &done.private_input_ref).await,
        raw(&store, &admin, "artifact", &world_storage).await,
    );
    let reach = closure(&store, &admin, ("run", exploration_fx::UNRELATED_RUN)).await;
    assert!(!reaches(&reach, "artifact", &done.private_input_ref));
    assert!(!reaches(&reach, "job", &done.id));

    let status = revoke_and_clean(&store, &admin, exploration_fx::UNRELATED_RUN).await;
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert_eq!(
        (
            raw(&store, &admin, "job", &done.id).await,
            raw(&store, &admin, "artifact", &done.private_input_ref).await,
            raw(&store, &admin, "artifact", &world_storage).await,
        ),
        before,
        "an unrelated revocation leaves the job, its input and its world as they were"
    );
}

// ---------------------------------------------------------------------------
// 3. The other operations: which ones a run revocation reaches
// ---------------------------------------------------------------------------

#[tokio::test]
async fn replay_run_job_and_private_input_reach_the_cleanup_closure() {
    let (_dir, store, admin) = exploration_fx::open("replay.sqlite3").await;
    let pool = replay_fx::setup_sealed_pool(&admin, &store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(
            &admin,
            "replay.run",
            replay_fx::run_request("replay-run-1", &pool),
        )
        .await
        .unwrap();
    let done = wait_job(&store, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    assert_eq!(done.step, "replay_stored");
    let Some(ManagementResult::ReplayStored { report_id, .. }) = &done.result else {
        panic!("unexpected result: {:?}", done.result);
    };
    assert!(dispatcher.status(&admin, &done.id).await.is_ok());

    // run -> replay world -> pool -> private input -> job (the report hangs off
    // the pool and the world as well).
    let pool_storage = evo_storage::replay::replay_pool_storage_id(&pool.pool_digest).unwrap();
    let reach = closure(&store, &admin, ("run", "source-2")).await;
    assert!(reaches(&reach, "replay_world", "world-1"));
    assert!(reaches(&reach, "artifact", &pool_storage));
    assert!(reaches(&reach, "artifact", report_id));
    assert!(reaches(&reach, "artifact", &done.private_input_ref));
    assert!(reaches(&reach, "job", &done.id));
    assert!(!reaches(&reach, "replay_world", "world-train"));

    let job_before = raw(&store, &admin, "job", &done.id).await;
    let input_before = raw(&store, &admin, "artifact", &done.private_input_ref).await;
    assert_eq!(input_before["schema_version"], INPUT_SCHEMA);
    assert_eq!(
        input_before["payload"]["pool_digest"],
        json!(pool.pool_digest)
    );

    let status = revoke_and_clean(&store, &admin, "source-2").await;
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert_eq!(status.last_error, None);
    assert_eq!(status.pending_nodes, 0);

    assert_redacted_input(
        &raw(&store, &admin, "artifact", &done.private_input_ref).await,
        &done.private_input_ref,
    );
    assert_eq!(
        raw(&store, &admin, "job", &done.id).await,
        job_before,
        "the management job is preserved"
    );
    // The replay store records behind the job are redacted by the closure.
    for id in [&pool_storage, report_id] {
        let body = raw(&store, &admin, "artifact", id).await;
        assert_eq!(body["schema_version"], REDACTED, "{body}");
    }
    // The world and the runs the request never read stay live.
    let mut session = store.session().await.unwrap();
    let (_, train) = session
        .get_world(&admin, "world-train")
        .await
        .unwrap()
        .unwrap();
    session.commit().await.unwrap();
    assert_ne!(train["schema_version"], REDACTED);
    let other = raw(&store, &admin, "run", "train-source-1").await;
    assert_ne!(other["schema_version"], REDACTED);

    // Without the dispatcher's own gate the report read would surface the
    // redacted pool as `Internal`; the job is refused with a named Conflict.
    let message = expect_conflict(
        dispatcher.status(&admin, &done.id).await,
        "status after the cleanup",
    );
    assert_names_redaction(&message, &[&done.private_input_ref]);
}

#[tokio::test]
async fn experiment_register_is_reached_through_a_run_derived_input_and_evaluation_jobs_are_not() {
    // The holdout's first protected input is the stored source selection, an
    // artifact that depends on the runs of its grant: the one way a run
    // revocation reaches an `experiment.register` input. The control, the holdout
    // record and the ticket have no edge to any run, so `evaluation.start` and
    // `evaluation.status` (which read only those) are outside every run closure.
    let e05 = e05_fx::fixture(&exploration_fx::selection_id()).await;
    exploration_fx::setup(&e05.store).await;
    let (store, evaluator) = (&e05.store, &e05.evaluator);
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![evaluator.clone()]).unwrap();

    let mut jobs = Vec::new();
    for (operation, request) in [
        ("experiment.register", e05.register_request("register-1")),
        ("evaluation.start", e05.start_request("start-1")),
        ("evaluation.status", e05.status_request("status-1")),
    ] {
        let queued = dispatcher
            .submit(evaluator, operation, request)
            .await
            .unwrap();
        jobs.push(wait_job(store, evaluator, &queued.id).await);
    }
    let [register, start, status] = <[ManagementJob; 3]>::try_from(jobs).unwrap();
    assert_eq!(
        register.state,
        ManagementJobState::Succeeded,
        "{register:?}"
    );
    assert_eq!(start.state, ManagementJobState::Blocked, "{start:?}");
    assert_eq!(status.state, ManagementJobState::Succeeded, "{status:?}");

    let reach = closure(store, &admin, ("run", "run-failure")).await;
    assert!(reaches(&reach, "artifact", &exploration_fx::selection_id()));
    assert!(reaches(&reach, "artifact", &register.private_input_ref));
    assert!(reaches(&reach, "job", &register.id));
    for job in [&start, &status] {
        assert!(
            !reaches(&reach, "artifact", &job.private_input_ref),
            "{} is outside the run closure",
            job.operation
        );
        assert!(!reaches(&reach, "job", &job.id));
    }
    for id in e05.artifact_ids() {
        assert!(
            !reaches(&reach, "artifact", &id),
            "{id} has no edge to a run"
        );
    }

    // Everything the closure does not reach, byte for byte.
    let mut untouched = Vec::new();
    for job in [&start, &status] {
        untouched.push(("job", job.id.clone()));
        untouched.push(("artifact", job.private_input_ref.clone()));
    }
    for id in e05.artifact_ids() {
        untouched.push(("artifact", id));
    }
    let mut before = Vec::new();
    for (kind, id) in &untouched {
        before.push(raw(store, evaluator, kind, id).await);
    }
    let register_before = raw(store, evaluator, "job", &register.id).await;

    let cleanup = revoke_and_clean(store, &admin, "run-failure").await;
    assert_eq!(cleanup.state, CleanupState::Complete, "{cleanup:?}");
    assert_eq!(cleanup.last_error, None);
    assert_eq!(cleanup.pending_nodes, 0);

    assert_redacted_input(
        &raw(store, evaluator, "artifact", &register.private_input_ref).await,
        &register.private_input_ref,
    );
    assert_eq!(
        raw(store, evaluator, "job", &register.id).await,
        register_before,
        "the management job is preserved"
    );
    let mut after = Vec::new();
    for (kind, id) in &untouched {
        after.push(raw(store, evaluator, kind, id).await);
    }
    assert_eq!(
        after, before,
        "evaluation jobs, inputs and e05 records are unchanged"
    );

    // `experiment.register` is refused with a named Conflict; the evaluation
    // jobs are still served (no source-derived result is involved).
    let message = expect_conflict(
        dispatcher.status(evaluator, &register.id).await,
        "experiment.register status after the cleanup",
    );
    assert_names_redaction(&message, &[&register.private_input_ref]);
    assert_eq!(
        dispatcher.status(evaluator, &start.id).await.unwrap().state,
        ManagementJobState::Blocked
    );
    assert_eq!(
        dispatcher
            .status(evaluator, &status.id)
            .await
            .unwrap()
            .state,
        ManagementJobState::Succeeded
    );
}

// ---------------------------------------------------------------------------
// 4. A job that is Queued or Running when its input is redacted
// ---------------------------------------------------------------------------

/// A live Running job holding the one concurrent management dispatch, so that
/// every other job stays Queued (never claimed, never executed).
fn holding_job() -> ManagementJob {
    ManagementJob {
        id: "management-job-holding-the-dispatch".into(),
        schema_version: JOB_SCHEMA.into(),
        operation: "meta.start".into(),
        request_key: "holding".into(),
        payload_digest: "a".repeat(64),
        owner_actor: "admin".into(),
        owner_role: Role::Admin,
        state: ManagementJobState::Running,
        step: "claimed".into(),
        private_input_ref: "management-input-holding".into(),
        result: None,
        error_code: None,
        cancel_requested: false,
        lease_token: Some("live-lease".into()),
        lease_until: evo_core::now().saturating_add(60),
        generation: 1,
        diagnostics: vec![],
        created_at: 1,
    }
}

#[tokio::test]
async fn a_queued_job_whose_input_was_redacted_fails_with_source_revoked_and_is_not_retried() {
    let (_dir, store, admin) = exploration_fx::open("queued.sqlite3").await;
    exploration_fx::setup(&store).await;
    let holding = holding_job();
    put_job(&store, &admin, &holding).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_fx::start_request(
                "exploration-queued",
                &exploration_fx::world("world-1", 1),
            ),
        )
        .await
        .unwrap();
    assert_eq!(queued.state, ManagementJobState::Queued);
    // The spawned claim is deferred by the capacity bound: the job has not run.
    let mut waiting = job_record(&store, &admin, &queued.id).await;
    for _ in 0..1_000 {
        if waiting.step == ManagementDispatcher::CAPACITY_WAIT_STEP {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
        waiting = job_record(&store, &admin, &queued.id).await;
    }
    assert_eq!(waiting.state, ManagementJobState::Queued);
    assert_eq!(waiting.step, ManagementDispatcher::CAPACITY_WAIT_STEP);
    assert_eq!(waiting.generation, 0);

    // The runs of the request are revoked and cleaned while the job waits.
    let cleanup = revoke_and_clean(&store, &admin, "run-failure").await;
    assert_eq!(cleanup.state, CleanupState::Complete, "{cleanup:?}");
    assert_eq!(cleanup.last_error, None);
    assert_redacted_input(
        &raw(&store, &admin, "artifact", &queued.private_input_ref).await,
        &queued.private_input_ref,
    );
    let still_queued = job_record(&store, &admin, &queued.id).await;
    assert_eq!(
        serde_json::to_value(&still_queued).unwrap(),
        serde_json::to_value(&waiting).unwrap(),
        "the cleanup preserves a Queued job as it is"
    );

    // The dispatch frees up: the deferred claim now reads the redacted input.
    let mut finished = holding.clone();
    finished.state = ManagementJobState::Succeeded;
    finished.step = "finished".into();
    finished.lease_token = None;
    finished.lease_until = 0;
    put_job(&store, &admin, &finished).await;
    let failed = wait_job(&store, &admin, &queued.id).await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.step, "private_input_redacted");
    assert_eq!(failed.error_code.as_deref(), Some("source_revoked"));
    assert!(failed.result.is_none());
    assert_eq!(failed.generation, 1, "claimed once");
    assert!(failed.lease_token.is_none());
    assert_eq!(failed.lease_until, 0);
    // The request never ran: no world was registered.
    assert!(
        try_raw(
            &store,
            &admin,
            "artifact",
            &evo_engine::exploration::exploration_world_storage_id("world-1").unwrap()
        )
        .await
        .is_none()
    );

    // Not retried: the terminal record stays as it is, and a restarted service
    // starts on this store.
    tokio::time::sleep(ManagementDispatcher::CAPACITY_WAIT_RETRY * 2).await;
    let settled = job_record(&store, &admin, &queued.id).await;
    assert_eq!(
        serde_json::to_value(&settled).unwrap(),
        serde_json::to_value(&failed).unwrap()
    );
    let restarted = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    assert_eq!(restarted.recover_pending().await.unwrap(), 0);
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(
        serde_json::to_value(job_record(&store, &admin, &queued.id).await).unwrap(),
        serde_json::to_value(&failed).unwrap()
    );
    // A Failed job carries no result derived from its sources: status returns the
    // record of what happened, `source_revoked` included.
    let observed = restarted.status(&admin, &queued.id).await.unwrap();
    assert_eq!(observed.state, ManagementJobState::Failed);
    assert_eq!(observed.error_code.as_deref(), Some("source_revoked"));
}

#[tokio::test]
async fn a_running_job_with_an_expired_lease_fails_with_source_revoked_when_recovery_reruns_it() {
    use curriculum_fx::*;
    let chain = Chain::build().await;
    let fixture = &chain.fixture;
    let (store, admin) = (&fixture.store, &fixture.admin);
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(admin, "curriculum.step", step_request("curriculum-crash"))
        .await
        .unwrap();
    let done = wait_job(store, admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded);

    // A crash after the consumer committed, before `finish_claim` wrote the
    // terminal: the job is left Running with a dead lease.
    let mut crashed = done.clone();
    crashed.state = ManagementJobState::Running;
    crashed.step = "before_curriculum_step".into();
    crashed.result = None;
    crashed.error_code = None;
    crashed.lease_token = Some("dead-process-lease".into());
    crashed.lease_until = 0;
    put_job(store, admin, &crashed).await;

    let cleanup = revoke_and_clean(store, admin, "run-a").await;
    assert_eq!(cleanup.state, CleanupState::Complete, "{cleanup:?}");
    assert_redacted_input(
        &raw(store, admin, "artifact", &done.private_input_ref).await,
        &done.private_input_ref,
    );
    assert_eq!(
        serde_json::to_value(job_record(store, admin, &done.id).await).unwrap(),
        serde_json::to_value(&crashed).unwrap(),
        "the cleanup preserves a Running job as it is"
    );

    // Startup recovery re-runs it: the input is gone with its source, so the job
    // ends Failed with a reason; nothing panics and recovery itself succeeds.
    let restarted = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    assert_eq!(restarted.recover_pending().await.unwrap(), 1);
    let failed = wait_job(store, admin, &done.id).await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.step, "private_input_redacted");
    assert_eq!(failed.error_code.as_deref(), Some("source_revoked"));
    assert!(failed.result.is_none());
    assert_eq!(failed.generation, done.generation + 1, "claimed once more");

    // Terminal: a second restart has nothing to recover and changes nothing.
    let again = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    assert_eq!(again.recover_pending().await.unwrap(), 0);
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(
        serde_json::to_value(job_record(store, admin, &done.id).await).unwrap(),
        serde_json::to_value(&failed).unwrap()
    );
}

/// A forged Queued job: for the tests that only care what its private input is.
fn queued_job(id: &str, private_input_ref: &str) -> ManagementJob {
    ManagementJob {
        id: id.into(),
        schema_version: JOB_SCHEMA.into(),
        operation: "exploration.start".into(),
        request_key: format!("{id}-key"),
        payload_digest: "b".repeat(64),
        owner_actor: "admin".into(),
        owner_role: Role::Admin,
        state: ManagementJobState::Queued,
        step: "accepted".into(),
        private_input_ref: private_input_ref.into(),
        result: None,
        error_code: None,
        cancel_requested: false,
        lease_token: None,
        lease_until: 0,
        generation: 0,
        diagnostics: vec![],
        created_at: 1,
    }
}

#[tokio::test]
async fn an_input_that_is_not_a_tombstone_and_does_not_decode_still_fails_the_job_as_internal() {
    // Only `rsia.redacted.v1` is named as a revocation. Any other body that is
    // not a private input is corruption: the job fails with the storage-level
    // code and the step of an invalid input, exactly as before.
    let (_dir, store, admin) = exploration_fx::open("corrupt-input.sqlite3").await;
    for (n, body) in [
        json!({"schema_version": INPUT_SCHEMA, "garbage": true}),
        json!({"schema_version": "rsia.redacted.v2", "state": "source_revoked"}),
        json!("not an object"),
    ]
    .into_iter()
    .enumerate()
    {
        let input_id = format!("corrupt-input-{n}");
        put_raw(&store, &admin, "artifact", &input_id, &body).await;
        let job = queued_job(&format!("corrupt-job-{n}"), &input_id);
        put_job(&store, &admin, &job).await;
        let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
        assert_eq!(dispatcher.recover_pending().await.unwrap(), 1);
        let failed = wait_job(&store, &admin, &job.id).await;
        assert_eq!(
            failed.state,
            ManagementJobState::Failed,
            "{body}: {failed:?}"
        );
        assert_eq!(failed.step, "private_input_invalid", "{body}");
        assert_eq!(
            failed.error_code.as_deref(),
            Some("internal_error"),
            "{body}"
        );
    }
    // An absent input keeps its code too.
    let job = queued_job("absent-job", "absent-input");
    put_job(&store, &admin, &job).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    assert_eq!(dispatcher.recover_pending().await.unwrap(), 1);
    let failed = wait_job(&store, &admin, &job.id).await;
    assert_eq!(failed.step, "private_input_invalid");
    assert_eq!(failed.error_code.as_deref(), Some("not_found"));
}

#[tokio::test]
async fn the_redaction_gate_names_one_state_and_passes_an_absent_input() {
    // `status` of a Succeeded job refuses a redacted input and nothing else: a
    // job whose input is absent (or an ordinary record) is served as before.
    let (_dir, store, admin) = exploration_fx::open("gate.sqlite3").await;
    let mut job = queued_job("succeeded-without-input", "absent-input");
    job.operation = "meta.start".into();
    job.state = ManagementJobState::Succeeded;
    job.step = "finished".into();
    put_job(&store, &admin, &job).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    assert_eq!(
        dispatcher.status(&admin, &job.id).await.unwrap().state,
        ManagementJobState::Succeeded
    );

    put_raw(
        &store,
        &admin,
        "artifact",
        "absent-input",
        &json!({"schema_version": "rsia.redacted.v1", "state": "source_revoked"}),
    )
    .await;
    let message = expect_conflict(
        dispatcher.status(&admin, &job.id).await,
        "status with a redacted input",
    );
    assert_names_redaction(&message, &["absent-input"]);

    // A Failed job with the same input is still returned.
    let mut failed = queued_job("failed-with-redacted-input", "absent-input");
    failed.state = ManagementJobState::Failed;
    failed.step = "private_input_redacted".into();
    failed.error_code = Some("source_revoked".into());
    put_job(&store, &admin, &failed).await;
    let observed = dispatcher.status(&admin, &failed.id).await.unwrap();
    assert_eq!(observed.state, ManagementJobState::Failed);
    assert_eq!(observed.error_code.as_deref(), Some("source_revoked"));
}

// ---------------------------------------------------------------------------
// 5. Curriculum replay: a recorded key is not exempt from the revocation gate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn curriculum_replay_fails_closed_after_begin_revoke_and_after_the_cleanup() {
    use curriculum_fx::*;
    use evo_core::curriculum::ProbeTerminal;
    let chain = Chain::build().await;
    let (store, admin, coordinator) = (
        &chain.fixture.store,
        &chain.fixture.admin,
        &chain.coordinator,
    );
    let schedule = |key: &'static str| async move {
        coordinator
            .schedule_probe_idempotent(key, PROFILE_ID, STATE_ID, 1_000_000)
            .await
    };

    let first = schedule("probe-step-1").await.unwrap();
    assert_eq!(first.terminal, Some(ProbeTerminal::BudgetExhausted));
    // A live replay reloads the recorded job (what crash recovery relies on).
    let replayed = schedule("probe-step-1").await.unwrap();
    assert_eq!(replayed.id, first.id);
    assert_eq!(replayed.trigger_digest, first.trigger_digest);
    // A new key runs the unchanged execution path and opens the next window ...
    let second = schedule("probe-step-2").await.unwrap();
    assert_ne!(second.id, first.id);
    assert_eq!(second.terminal, Some(ProbeTerminal::Cooldown));
    // ... and a replay of the first key after the state moved on still returns
    // the first job, because the state is still live.
    let replayed = schedule("probe-step-1").await.unwrap();
    assert_eq!(replayed.id, first.id);
    let records_before = curriculum_bodies(store, admin).await;
    let count = |kind: &str| {
        records_before
            .values()
            .filter(|value| value["record_kind"] == kind)
            .count()
    };
    assert_eq!((count(JOB_KIND), count(RECEIPT_KIND)), (2, 2));

    // Logical block: the watermark bump alone fails every replay (defect: the
    // receipt branch returned the recorded job without reading the state) and
    // the new-key path, as it always did.
    let cleanup = begin(store, admin, "run-a").await;
    assert_eq!(cleanup.state, CleanupState::Pending);
    for key in ["probe-step-1", "probe-step-2", "probe-step-3"] {
        let message = expect_conflict(schedule(key).await, key);
        assert!(message.contains("watermark"), "{key}: {message}");
    }

    // After the cleanup the learner state is a tombstone: the replay is refused
    // by name, not reported as a decode failure.
    let cleanup = drive(store, admin, cleanup, 8).await;
    assert_eq!(cleanup.state, CleanupState::Complete, "{cleanup:?}");
    for key in ["probe-step-1", "probe-step-2", "probe-step-3"] {
        let message = expect_conflict(schedule(key).await, key);
        assert_names_redaction(&message, &[STATE_KIND, STATE_ID]);
    }
    // The refusals wrote nothing; the recorded probe jobs and receipts survived.
    let records_after = curriculum_bodies(store, admin).await;
    assert_eq!(records_after.len(), records_before.len());
    for (storage, before) in &records_before {
        if [JOB_KIND, RECEIPT_KIND].contains(&before["record_kind"].as_str().unwrap()) {
            assert_eq!(&records_after[storage], before);
        }
    }
}

#[tokio::test]
async fn a_recovered_curriculum_step_fails_closed_once_its_source_is_revoked() {
    // A crash between the consumer's commit and `finish_claim` leaves a Running
    // job whose recovery takes the receipt branch. Before this change that branch
    // finished the job Succeeded with the recorded probe job after the revocation.
    use curriculum_fx::*;
    let chain = Chain::build().await;
    let (store, admin) = (&chain.fixture.store, &chain.fixture.admin);
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(admin, "curriculum.step", step_request("curriculum-recover"))
        .await
        .unwrap();
    let done = wait_job(store, admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded);

    let mut crashed = done.clone();
    crashed.state = ManagementJobState::Running;
    crashed.step = "before_curriculum_step".into();
    crashed.result = None;
    crashed.error_code = None;
    crashed.lease_token = Some("dead-process-lease".into());
    crashed.lease_until = 0;
    put_job(store, admin, &crashed).await;
    // The revocation is logically committed; its cleanup has not run yet.
    let cleanup = begin(store, admin, "run-a").await;
    assert_eq!(cleanup.state, CleanupState::Pending);

    let restarted = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    assert_eq!(restarted.recover_pending().await.unwrap(), 1);
    let failed = wait_job(store, admin, &done.id).await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("conflict"));
    assert!(failed.result.is_none());
    assert_eq!(failed.generation, done.generation + 1);
}

// ---------------------------------------------------------------------------
// 6. Curriculum reads of redacted records
// ---------------------------------------------------------------------------

/// Every coordinator entry point that reads the learner state, with inputs a live
/// chain would accept: new writes and replays of records already stored.
async fn state_entry_points(
    chain: &curriculum_fx::Chain,
    probe_job_id: &str,
) -> Vec<(&'static str, Result<(), Error>)> {
    use curriculum_fx::*;
    use evo_core::curriculum::{
        ProposalAttemptOutcome, RegisteredProperty, StructuredTestProposalV1,
    };
    use evo_core::evaluation::DataUse;
    use evo_engine::curriculum::ProposalAttemptReceiptV1;
    use evo_engine::curriculum_profiles::{
        CLAMP_TARGET_ID, ClampOutput, RegisteredPureFunctionProfileV1, check_target_outputs,
        runtime_validity_report,
    };
    let coordinator = &chain.coordinator;
    let proposal = StructuredTestProposalV1 {
        schema_version: "rsia.structured_test_proposal.v1".into(),
        id: "proposal-boundary".into(),
        probe_job_id: probe_job_id.into(),
        target_id: CLAMP_TARGET_ID.into(),
        property: RegisteredProperty::BelowMapsToMin,
        value: -2,
        min: -1,
        max: 1,
        parent_family: "family-a".into(),
        data_use: DataUse::Development,
        source_artifact_ids: vec!["task-space-source".into()],
        reason: "exercise registered lower boundary".into(),
    };
    let attempt = ProposalAttemptReceiptV1 {
        schema_version: "rsia.curriculum_proposal_attempt.v1".into(),
        id: "attempt-1".into(),
        job_id: probe_job_id.into(),
        outcome: ProposalAttemptOutcome::Malformed,
        source_artifact_ids: vec!["task-space-source".into()],
    };
    let registered = RegisteredPureFunctionProfileV1::clamp_i64();
    let offline = check_target_outputs(
        &registered,
        &proposal,
        ClampOutput { clamped: -1 },
        ClampOutput { clamped: -1 },
    )
    .unwrap();
    let report = runtime_validity_report(&registered, &proposal, &offline).unwrap();
    vec![
        (
            "record_cycle (new cycle)",
            coordinator
                .record_cycle(chain.cycle("cycle-3"))
                .await
                .map(|_| ()),
        ),
        (
            "record_cycle (replay)",
            coordinator
                .record_cycle(chain.cycle("cycle-2"))
                .await
                .map(|_| ()),
        ),
        (
            "schedule_probe",
            coordinator
                .schedule_probe(PROFILE_ID, STATE_ID, 1_000_000)
                .await
                .map(|_| ()),
        ),
        (
            "schedule_probe_idempotent (new key)",
            coordinator
                .schedule_probe_idempotent("probe-step-9", PROFILE_ID, STATE_ID, 1_000_000)
                .await
                .map(|_| ()),
        ),
        (
            "schedule_probe_idempotent (recorded key)",
            coordinator
                .schedule_probe_idempotent("probe-step-1", PROFILE_ID, STATE_ID, 1_000_000)
                .await
                .map(|_| ()),
        ),
        (
            "record_probe_attempt",
            coordinator.record_probe_attempt(attempt).await.map(|_| ()),
        ),
        (
            "store_proposal",
            coordinator.store_proposal(STATE_ID, proposal).await,
        ),
        (
            "record_validity_report",
            coordinator
                .record_validity_report(STATE_ID, report)
                .await
                .map(|_| ()),
        ),
        (
            "select_next_task",
            coordinator
                .select_next_task(STATE_ID, &[])
                .await
                .map(|_| ()),
        ),
        (
            "record_applied_learning_asset",
            coordinator
                .record_applied_learning_asset(STATE_ID, "run-b", "application", "execution")
                .await
                .map(|_| ()),
        ),
        (
            "verified_probe_job_view",
            evo_engine::curriculum::verified_probe_job_view(
                &chain.fixture.admin,
                &chain.fixture.store,
                probe_job_id,
            )
            .await
            .map(|_| ()),
        ),
    ]
}

#[tokio::test]
async fn every_curriculum_entry_point_names_a_redacted_learner_state() {
    use curriculum_fx::*;
    let chain = Chain::build().await;
    let (store, admin) = (&chain.fixture.store, &chain.fixture.admin);
    let probe = chain
        .coordinator
        .schedule_probe_idempotent("probe-step-1", PROFILE_ID, STATE_ID, 1_000_000)
        .await
        .unwrap();

    // Logical block: the watermark names itself in every entry point.
    let cleanup = begin(store, admin, "run-a").await;
    for (name, result) in state_entry_points(&chain, &probe.id).await {
        let message = expect_conflict(result, name);
        assert!(message.contains("watermark"), "{name}: {message}");
    }

    // Complete cleanup: the state is a tombstone. Before this change each of
    // these surfaced as `Internal` (a tombstone does not decode as an envelope).
    let cleanup = drive(store, admin, cleanup, 8).await;
    assert_eq!(cleanup.state, CleanupState::Complete, "{cleanup:?}");
    let state_body = raw(store, admin, "artifact", &storage_id(STATE_KIND, STATE_ID)).await;
    assert_eq!(state_body["schema_version"], REDACTED);
    let before = curriculum_bodies(store, admin).await;
    for (name, result) in state_entry_points(&chain, &probe.id).await {
        let message = expect_conflict(result, name);
        assert_names_redaction(&message, &[STATE_KIND, STATE_ID]);
    }
    assert_eq!(
        curriculum_bodies(store, admin).await,
        before,
        "the refused calls wrote nothing"
    );
}

#[tokio::test]
async fn the_shared_curriculum_read_names_any_redacted_record_kind_and_id() {
    // The naming lives in the one read every entry point shares, so a tombstone
    // in place of a record the cleanup preserves (here forced) is named as well.
    use curriculum_fx::*;
    let chain = Chain::build().await;
    let (store, admin, coordinator) = (
        &chain.fixture.store,
        &chain.fixture.admin,
        &chain.coordinator,
    );
    let probe = coordinator
        .schedule_probe_idempotent("probe-step-1", PROFILE_ID, STATE_ID, 1_000_000)
        .await
        .unwrap();
    let tombstone = |id: &str, record: &str| {
        json!({"id": id, "schema_version": REDACTED, "state": "source_revoked",
               "original_kind": "artifact", "original_schema": ENVELOPE,
               "original_digest": d(record), "metadata": {"record_kind": record}})
    };
    let receipt_id = evo_engine::curriculum::probe_schedule_receipt_id("probe-step-1").unwrap();
    let receipt_storage = storage_id(RECEIPT_KIND, &receipt_id);
    let job_storage = storage_id(JOB_KIND, &probe.id);
    put_raw(
        store,
        admin,
        "artifact",
        &receipt_storage,
        &tombstone(&receipt_storage, RECEIPT_KIND),
    )
    .await;
    let message = expect_conflict(
        coordinator
            .schedule_probe_idempotent("probe-step-1", PROFILE_ID, STATE_ID, 1_000_000)
            .await,
        "replay over a redacted receipt",
    );
    assert_names_redaction(&message, &[RECEIPT_KIND, &receipt_id]);

    put_raw(
        store,
        admin,
        "artifact",
        &job_storage,
        &tombstone(&job_storage, JOB_KIND),
    )
    .await;
    let message = expect_conflict(
        evo_engine::curriculum::verified_probe_job_view(admin, store, &probe.id).await,
        "view over a redacted probe job",
    );
    assert_names_redaction(&message, &[JOB_KIND, &probe.id]);
}

#[tokio::test]
async fn a_curriculum_body_that_is_not_a_tombstone_and_does_not_decode_stays_internal() {
    use curriculum_fx::*;
    let state_storage = storage_id(STATE_KIND, STATE_ID);
    let bodies = [
        // A well-formed envelope around a payload that is not a learner state.
        json!({"schema_version": ENVELOPE, "id": state_storage,
               "record_kind": STATE_KIND, "payload": {"garbage": true}}),
        // Not an object at all.
        json!("not an envelope"),
        // A tombstone of a schema this code does not know.
        json!({"schema_version": "rsia.redacted.v2", "state": "source_revoked"}),
        // Shaped like the tombstone but not stamped as one.
        json!({"id": state_storage, "state": "source_revoked", "metadata": {}}),
    ];
    for body in bodies {
        let chain = Chain::build().await;
        let (store, admin) = (&chain.fixture.store, &chain.fixture.admin);
        put_raw(store, admin, "artifact", &state_storage, &body).await;
        let result = chain.coordinator.record_cycle(chain.cycle("cycle-3")).await;
        assert!(
            matches!(result, Err(Error::Internal)),
            "{body}: expected Internal, got {result:?}"
        );
    }
    // A well-formed envelope of another record kind is a mismatch, as before.
    let chain = Chain::build().await;
    let (store, admin) = (&chain.fixture.store, &chain.fixture.admin);
    let mut body = raw(store, admin, "artifact", &state_storage).await;
    body["record_kind"] = json!(PROFILE_KIND);
    put_raw(store, admin, "artifact", &state_storage, &body).await;
    let message = expect_conflict(
        chain.coordinator.record_cycle(chain.cycle("cycle-3")).await,
        "envelope of another kind",
    );
    assert!(message.contains("envelope mismatch"), "{message}");
}

// ---------------------------------------------------------------------------
// 7. Unknown management schemas still fail the cleanup closed
// ---------------------------------------------------------------------------

/// A record of `kind` with `schema` that depends on the revoked run, next to the
/// real closure; returns the cleanup outcome.
async fn cleanup_with_forged_node(
    kind: &str,
    schema: &str,
) -> evo_storage::lifecycle::CleanupStatus {
    let (_dir, store, admin) = exploration_fx::open("forged.sqlite3").await;
    exploration_fx::setup(&store).await;
    let body = if kind == "job" {
        let mut job = queued_job("forged-node", "forged-input");
        job.schema_version = schema.into();
        serde_json::to_value(&job).unwrap()
    } else {
        json!({"id": "forged-node", "schema_version": schema, "operation": "exploration.start",
               "payload_digest": "c".repeat(64), "payload": {}})
    };
    put_raw(&store, &admin, kind, "forged-node", &body).await;
    put_edge(
        &store,
        &admin,
        (kind, "forged-node"),
        ("run", "run-failure"),
    )
    .await;
    revoke_and_clean(&store, &admin, "run-failure").await
}

#[tokio::test]
async fn an_unknown_management_schema_or_object_kind_fails_the_cleanup_closed() {
    // Known schema under its own kind: the control, cleaned to Complete.
    for (kind, schema) in [("job", JOB_SCHEMA), ("artifact", INPUT_SCHEMA)] {
        let status = cleanup_with_forged_node(kind, schema).await;
        assert_eq!(
            status.state,
            CleanupState::Complete,
            "{kind} {schema}: {status:?}"
        );
        assert_eq!(status.last_error, None);
    }
    // Any other version, or a known schema under the other kind, is unclassified.
    for (kind, schema) in [
        ("job", "rsia.management_job.v2"),
        ("artifact", "rsia.management_private_input.v2"),
        ("artifact", JOB_SCHEMA),
        ("job", INPUT_SCHEMA),
    ] {
        let status = cleanup_with_forged_node(kind, schema).await;
        assert_eq!(
            status.state,
            CleanupState::Failed,
            "{kind} {schema}: {status:?}"
        );
        assert_eq!(
            status.last_error.as_deref(),
            Some(format!("blocked_unknown_scope:{kind}:{schema}:").as_str()),
            "{kind} {schema}"
        );
    }
}

#[tokio::test]
async fn an_unknown_job_schema_on_the_chain_of_a_real_management_job_fails_the_cleanup_closed() {
    // The forged job hangs off the private input of a real, finished
    // exploration.start job: run -> input -> forged job.
    let (_dir, store, admin) = exploration_fx::open("chain.sqlite3").await;
    exploration_fx::setup(&store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_fx::start_request(
                "exploration-chain",
                &exploration_fx::world("world-1", 1),
            ),
        )
        .await
        .unwrap();
    let done = wait_job(&store, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded);
    let mut forged = done.clone();
    forged.id = "management-job-v2".into();
    forged.schema_version = "rsia.management_job.v2".into();
    put_raw(
        &store,
        &admin,
        "job",
        &forged.id,
        &serde_json::to_value(&forged).unwrap(),
    )
    .await;
    put_edge(
        &store,
        &admin,
        ("job", &forged.id),
        ("artifact", &done.private_input_ref),
    )
    .await;
    assert!(reaches(
        &closure(&store, &admin, ("run", "run-failure")).await,
        "job",
        &forged.id
    ));

    let status = revoke_and_clean(&store, &admin, "run-failure").await;
    assert_eq!(status.state, CleanupState::Failed, "{status:?}");
    assert_eq!(
        status.last_error.as_deref(),
        Some("blocked_unknown_scope:job:rsia.management_job.v2:")
    );
    // The job does not claim completion on a further step either.
    let again = LifecycleStore::cleanup_step(&admin, &store, &status.job_id, 8, 3_000)
        .await
        .unwrap();
    assert_eq!(again.state, CleanupState::Failed);
}

// ---------------------------------------------------------------------------
// 8. The management cleanup is paged and resumes after a restart (E08)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_management_cleanup_is_paged_and_resumes_after_a_restart() {
    let (dir, store, admin) = exploration_fx::open("resume.sqlite3").await;
    exploration_fx::setup(&store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_fx::start_request(
                "exploration-resume",
                &exploration_fx::world("world-1", 1),
            ),
        )
        .await
        .unwrap();
    let done = wait_job(&store, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded);
    let job_before = raw(&store, &admin, "job", &done.id).await;

    // One frontier node per step, one edge per page: stop as soon as the private
    // input is redacted while the cleanup is still in flight.
    let mut status = begin(&store, &admin, "run-failure").await;
    for now in 501..2_500 {
        status = LifecycleStore::cleanup_step(&admin, &store, &status.job_id, 1, now)
            .await
            .unwrap();
        let input = raw(&store, &admin, "artifact", &done.private_input_ref).await;
        if input["schema_version"] == REDACTED {
            break;
        }
        assert_eq!(status.state, CleanupState::Running, "{status:?}");
    }
    assert_eq!(status.state, CleanupState::Running, "{status:?}");
    assert!(status.pending_nodes > 0);
    // Half way: the input is gone, the job record is still whole, and the
    // dispatcher already refuses to serve the result.
    assert_eq!(raw(&store, &admin, "job", &done.id).await, job_before);
    expect_conflict(dispatcher.status(&admin, &done.id).await, "status half way");

    // Restart: the job is reloaded from the store and continues.
    let job_id = status.job_id.clone();
    store.close().await;
    let reopened = Store::open(&dir.path().join("resume.sqlite3"))
        .await
        .unwrap();
    let resumed = LifecycleStore::cleanup_status(&admin, &reopened, &job_id)
        .await
        .unwrap();
    assert_eq!(resumed.state, CleanupState::Running);
    let finished = drive(&reopened, &admin, resumed, 1).await;
    assert_eq!(finished.state, CleanupState::Complete, "{finished:?}");
    assert_eq!(finished.last_error, None);

    assert_redacted_input(
        &raw(&reopened, &admin, "artifact", &done.private_input_ref).await,
        &done.private_input_ref,
    );
    assert_eq!(raw(&reopened, &admin, "job", &done.id).await, job_before);
    let restarted = ManagementDispatcher::new(reopened.clone(), vec![admin.clone()]).unwrap();
    assert_eq!(restarted.recover_pending().await.unwrap(), 0);
    let message = expect_conflict(
        restarted.status(&admin, &done.id).await,
        "status after the resumed cleanup",
    );
    assert_names_redaction(&message, &[&done.private_input_ref]);
}

// ---------------------------------------------------------------------------
// 9. A new request over a revoked dependency is refused at submit (R1)
// ---------------------------------------------------------------------------
//
// The cleanup runs once per revocation. A request submitted after it would write
// its private input (for `exploration.start` the submitted world) with edges to a
// revoked source, and nothing would ever clean it. `submit` therefore checks every
// private dependency of a new request before it writes anything: a tombstoned
// source (`begin_revoke` writes one under the source id; the cleanup then deletes
// a run) or a dependency the cleanup already redacted refuses the whole
// submission with a `Conflict` that names the dependency. A dependency that does
// not exist is not refused (the job fails on it inside, as before), and a live
// artifact whose upstream source was revoked but not cleaned yet cannot be judged
// from the dependency alone (the session exposes dependency edges only from the
// dependent side), so that request is accepted and its job fails closed.

/// What a refused submission must leave untouched: the jobs and artifacts of the
/// namespace, every edge into the request's dependencies, and the audit chain.
async fn store_shape(
    store: &Store,
    ctx: &Context,
    dependencies: &[(&str, String)],
) -> (u64, u64, Vec<Vec<(String, String)>>, usize) {
    let mut session = store.session().await.unwrap();
    let jobs = session.namespace_object_count(ctx, "job").await.unwrap();
    let artifacts = session
        .namespace_object_count(ctx, "artifact")
        .await
        .unwrap();
    let mut edges = Vec::new();
    for (kind, id) in dependencies {
        let mut dependents = session.dependents(ctx, kind, id).await.unwrap();
        dependents.sort();
        edges.push(dependents);
    }
    session.commit().await.unwrap();
    let audits = store.verify_audit(ctx).await.unwrap();
    (jobs, artifacts, edges, audits)
}

/// No idempotency row: a half-recorded submission would answer differently.
async fn assert_no_idempotency_row(store: &Store, ctx: &Context, operation: &str, key: &str) {
    let mut session = store.session().await.unwrap();
    let row = session
        .cached::<ManagementJob, _>(ctx, operation, key, &json!({}))
        .await;
    session.commit().await.unwrap();
    assert!(matches!(row, Ok(None)), "{operation} {key}: {row:?}");
}

#[tokio::test]
async fn a_new_exploration_request_over_a_cleaned_run_is_refused_at_submit() {
    let (_dir, store, admin) = exploration_fx::open("refuse-exploration.sqlite3").await;
    exploration_fx::setup(&store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let first = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_fx::start_request(
                "exploration-before",
                &exploration_fx::world("world-1", 1),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        wait_job(&store, &admin, &first.id).await.state,
        ManagementJobState::Succeeded
    );
    let cleanup = revoke_and_clean(&store, &admin, "run-failure").await;
    assert_eq!(cleanup.state, CleanupState::Complete, "{cleanup:?}");

    // The run is gone (deleted by the cleanup) and tombstoned. The store's
    // watermark is 2 now, so a world for it is signed for 2.
    let world = exploration_fx::world("world-2", 2);
    let world_storage = evo_engine::exploration::exploration_world_storage_id("world-2").unwrap();
    let dependencies = [
        ("artifact", world_storage),
        ("run", "run-failure".to_string()),
        ("run", "run-success".to_string()),
    ];
    let before = store_shape(&store, &admin, &dependencies).await;
    let message = expect_conflict(
        dispatcher
            .submit(
                &admin,
                "exploration.start",
                exploration_fx::start_request("exploration-after-cleanup", &world),
            )
            .await,
        "submit over a cleaned run",
    );
    assert!(message.contains("run run-failure"), "{message}");
    assert!(message.contains("revoked"), "{message}");
    assert_eq!(
        store_shape(&store, &admin, &dependencies).await,
        before,
        "a refused submission writes no private input, job, edge or audit record"
    );
    assert_no_idempotency_row(
        &store,
        &admin,
        "exploration.start",
        "exploration-after-cleanup",
    )
    .await;

    // A request whose dependencies are all live is accepted and runs as before.
    let mut live = exploration_fx::world("world-3", 2);
    live.dependencies
        .retain(|dependency| dependency.id == "run-success");
    let queued = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_fx::start_request("exploration-over-live-run", &live),
        )
        .await
        .unwrap();
    let done = wait_job(&store, &admin, &queued.id).await;
    assert_eq!(done.state, ManagementJobState::Succeeded, "{done:?}");
    // Its key is a recorded job: replays keep returning it without a new check.
    let mut session = store.session().await.unwrap();
    session
        .put(
            &admin,
            "tombstone",
            "run-success",
            "admin",
            &json!({"id": "run-success"}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
    let replayed = dispatcher
        .submit(
            &admin,
            "exploration.start",
            exploration_fx::start_request("exploration-over-live-run", &live),
        )
        .await
        .unwrap();
    assert_eq!(
        replayed.id, queued.id,
        "a same-key replay returns the stored job"
    );
}

#[tokio::test]
async fn a_new_curriculum_request_over_a_redacted_learner_state_is_refused_at_submit() {
    use curriculum_fx::*;
    let chain = Chain::build().await;
    let (store, admin) = (&chain.fixture.store, &chain.fixture.admin);
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let first = dispatcher
        .submit(admin, "curriculum.step", step_request("curriculum-before"))
        .await
        .unwrap();
    assert_eq!(
        wait_job(store, admin, &first.id).await.state,
        ManagementJobState::Succeeded
    );
    let cleanup = revoke_and_clean(store, admin, "run-a").await;
    assert_eq!(cleanup.state, CleanupState::Complete, "{cleanup:?}");

    let state_storage = storage_id(STATE_KIND, STATE_ID);
    let dependencies = [
        ("artifact", storage_id(PROFILE_KIND, PROFILE_ID)),
        ("artifact", state_storage.clone()),
    ];
    let before = store_shape(store, admin, &dependencies).await;
    let message = expect_conflict(
        dispatcher
            .submit(
                admin,
                "curriculum.step",
                step_request("curriculum-after-cleanup"),
            )
            .await,
        "submit over a redacted learner state",
    );
    assert_names_redaction(&message, &["artifact", &state_storage]);
    assert_eq!(
        store_shape(store, admin, &dependencies).await,
        before,
        "a refused submission writes no private input, job, edge or audit record"
    );
    assert_no_idempotency_row(store, admin, "curriculum.step", "curriculum-after-cleanup").await;
}

#[tokio::test]
async fn a_new_replay_request_over_a_redacted_pool_is_refused_at_submit() {
    let (_dir, store, admin) = exploration_fx::open("refuse-replay.sqlite3").await;
    let pool = replay_fx::setup_sealed_pool(&admin, &store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let first = dispatcher
        .submit(
            &admin,
            "replay.run",
            replay_fx::run_request("replay-before", &pool),
        )
        .await
        .unwrap();
    assert_eq!(
        wait_job(&store, &admin, &first.id).await.state,
        ManagementJobState::Succeeded
    );
    // A world of the pool lost its source: the cleanup redacts the world, the
    // pool and the report.
    let cleanup = revoke_and_clean(&store, &admin, "source-2").await;
    assert_eq!(cleanup.state, CleanupState::Complete, "{cleanup:?}");

    let pool_storage = evo_storage::replay::replay_pool_storage_id(&pool.pool_digest).unwrap();
    let dependencies = [("artifact", pool_storage.clone())];
    let before = store_shape(&store, &admin, &dependencies).await;
    let message = expect_conflict(
        dispatcher
            .submit(
                &admin,
                "replay.run",
                replay_fx::run_request("replay-after-cleanup", &pool),
            )
            .await,
        "submit over a redacted pool",
    );
    assert_names_redaction(&message, &["artifact", &pool_storage]);
    assert_eq!(
        store_shape(&store, &admin, &dependencies).await,
        before,
        "a refused submission writes no private input, job, edge or audit record"
    );
    assert_no_idempotency_row(&store, &admin, "replay.run", "replay-after-cleanup").await;
}

#[tokio::test]
async fn a_request_submitted_between_begin_revoke_and_the_cleanup_is_refused_when_the_source_is_a_dependency()
 {
    // Only `begin_revoke` ran: the tombstone exists and the watermark moved, the
    // cleanup has not touched anything yet.
    let (_dir, store, admin) = exploration_fx::open("refuse-begin-only.sqlite3").await;
    exploration_fx::setup(&store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let cleanup = begin(&store, &admin, "run-failure").await;
    assert_eq!(cleanup.state, CleanupState::Pending);
    assert!(
        try_raw(&store, &admin, "run", "run-failure")
            .await
            .is_some()
    );

    // exploration.start depends on the runs themselves: the tombstone decides.
    let dependencies = [
        ("run", "run-failure".to_string()),
        ("run", "run-success".to_string()),
    ];
    let before = store_shape(&store, &admin, &dependencies).await;
    let message = expect_conflict(
        dispatcher
            .submit(
                &admin,
                "exploration.start",
                exploration_fx::start_request(
                    "exploration-begin-only",
                    &exploration_fx::world("world-2", 2),
                ),
            )
            .await,
        "submit over a tombstoned run",
    );
    assert!(message.contains("run run-failure"), "{message}");
    assert_eq!(store_shape(&store, &admin, &dependencies).await, before);
    assert_no_idempotency_row(
        &store,
        &admin,
        "exploration.start",
        "exploration-begin-only",
    )
    .await;
}

#[tokio::test]
async fn an_artifact_dependency_whose_source_is_only_marked_revoked_cannot_be_judged_at_submit() {
    // curriculum.step and replay.run depend on artifacts (learner state, pool),
    // which are still live while only `begin_revoke` ran. Their upstream run is
    // tombstoned, but the dependency edges are readable only from the dependent
    // side, so the submit-time check cannot walk up to the run. Such a request is
    // accepted and its job fails closed on the operation's own checks (the
    // behaviour the existing revocation-gate tests pin); the cleanup then reaches
    // its private input like any other.
    use curriculum_fx::*;
    let chain = Chain::build().await;
    let (store, admin) = (&chain.fixture.store, &chain.fixture.admin);
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let cleanup = begin(store, admin, "run-a").await;
    assert_eq!(cleanup.state, CleanupState::Pending);
    let queued = dispatcher
        .submit(
            admin,
            "curriculum.step",
            step_request("curriculum-begin-only"),
        )
        .await
        .unwrap();
    let failed = wait_job(store, admin, &queued.id).await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("conflict"));
    let cleanup = drive(store, admin, cleanup, 8).await;
    assert_eq!(cleanup.state, CleanupState::Complete, "{cleanup:?}");
    assert_redacted_input(
        &raw(store, admin, "artifact", &failed.private_input_ref).await,
        &failed.private_input_ref,
    );

    let (_dir, store, admin) = exploration_fx::open("replay-begin-only.sqlite3").await;
    let pool = replay_fx::setup_sealed_pool(&admin, &store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let cleanup = begin(&store, &admin, "source-2").await;
    assert_eq!(cleanup.state, CleanupState::Pending);
    let queued = dispatcher
        .submit(
            &admin,
            "replay.run",
            replay_fx::run_request("replay-begin-only", &pool),
        )
        .await
        .unwrap();
    let failed = wait_job(&store, &admin, &queued.id).await;
    assert_eq!(failed.state, ManagementJobState::Failed, "{failed:?}");
    assert_eq!(failed.error_code.as_deref(), Some("conflict"));
    let cleanup = drive(&store, &admin, cleanup, 8).await;
    assert_eq!(cleanup.state, CleanupState::Complete, "{cleanup:?}");
    assert_redacted_input(
        &raw(&store, &admin, "artifact", &failed.private_input_ref).await,
        &failed.private_input_ref,
    );
}
