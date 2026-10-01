//! AG-044 (E07/E08/§11, V017): a management request whose dependency's upstream
//! closure holds a revoked source is refused at submit, and nothing is written.
//! Real SQLite store, no model, no provider, zero monetary cost.
//!
//! `begin_revoke` writes the tombstone and the watermark in one transaction, so a
//! source is revoked from the moment it commits (plan §11: a revocation refuses
//! new access at once). The cleanup that follows reaches the nodes below the
//! source one page at a time. Until it does, the pool of a replay, the learner
//! state of a curriculum and the input artifact of an experiment registration are
//! live: the submit-time check judged only the dependencies a request names, so a
//! request over such a node was accepted, its private input (the whole request,
//! for `experiment.register` with the oracle answers in plain text) was written
//! with an edge to it, and its job failed closed afterwards. While two cleanups
//! overlapped, the controller's probe (Q2) saw 141 such requests accepted, each
//! one a late write the cleanup had to visit before it could report `Complete`.
//!
//! The submit check now has two stages. The first is the direct check it always
//! had (a tombstone of the dependency's own kind, a redacted body) and keeps its
//! named messages. The second takes the upstream closure of the dependencies that
//! passed (the nodes they depend on, directly or transitively, along the
//! dependency edges) and refuses the request when a run or an artifact in it was
//! revoked: it carries a tombstone written for its own kind, a tombstone that
//! cannot be read (fail closed), or a body the cleanup already redacted. A
//! tombstone is keyed by the id of its source alone, so one written for the other
//! kind belongs to another object and does not refuse. A closure of more than
//! 10 000 nodes is refused as well, never truncated and then judged live. A
//! refused request writes no private input, job, edge, idempotency row or audit
//! record, and a replay of an accepted request key still returns the stored job
//! before any check runs.
//!
//! Not covered here: the other writers (grant, stage_package, a late
//! `DispatchObserved`) and anything written after the cleanup is `Complete`.
//!
//! The fixtures below are copied from `tests/management_cleanup_v42.rs` (and,
//! through it, from `tests/curriculum_cleanup_v42.rs`,
//! `tests/dispatch_management.rs` and `tests/streaming_evaluator.rs`); test
//! crates cannot import each other, and the originals are untouched.

use evo_core::{Context, Error, Role, fingerprint};
use evo_engine::dispatch::{ManagementDispatcher, ManagementJob, ManagementJobState};
use evo_storage::Store;
use evo_storage::lifecycle::{CleanupState, CleanupStatus, LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};
use std::time::Duration;

use common::*;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

mod common {
    use super::*;
    use evo_core::hash;

    pub const REDACTED: &str = "rsia.redacted.v1";

    /// Every kind the `objects` table accepts: a snapshot of the store reads them
    /// all, so a refused request cannot hide a write under a kind nobody looked at.
    const OBJECT_KINDS: [&str; 15] = [
        "run",
        "feedback",
        "event",
        "candidate",
        "release",
        "pointer",
        "receipt",
        "dataset",
        "evaluation",
        "budget",
        "reservation",
        "job",
        "improvement",
        "artifact",
        "tombstone",
    ];

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

    pub async fn put_raw(store: &Store, ctx: &Context, kind: &str, id: &str, body: &Value) {
        let mut session = store.session().await.unwrap();
        session.put(ctx, kind, id, "admin", body).await.unwrap();
        session.commit().await.unwrap();
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

    /// One cleanup step of `limit` edges; `now` is the step's clock.
    pub async fn step(
        store: &Store,
        admin: &Context,
        status: &CleanupStatus,
        limit: usize,
        now: i64,
    ) -> CleanupStatus {
        LifecycleStore::cleanup_step(admin, store, &status.job_id, limit, now)
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
            status = step(store, admin, &status, page, now).await;
        }
        status
    }

    pub fn finished(status: &CleanupStatus) -> bool {
        matches!(status.state, CleanupState::Complete | CleanupState::Failed)
    }

    /// Polls the stored job (not `status`, which refuses after a revocation)
    /// until it is terminal.
    pub async fn wait_job(store: &Store, ctx: &Context, job_id: &str) -> ManagementJob {
        for _ in 0..2_000 {
            let mut session = store.session().await.unwrap();
            let job: ManagementJob = session.need(ctx, "job", job_id).await.unwrap();
            session.commit().await.unwrap();
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

    /// The `Conflict` message, failing on any other outcome (above all on `Ok`,
    /// which is what an accepted request is, and on `Internal`).
    pub fn expect_conflict<T: std::fmt::Debug>(result: Result<T, Error>, what: &str) -> String {
        match result {
            Err(Error::Conflict(message)) => message,
            other => panic!("{what}: expected a Conflict, got {other:?}"),
        }
    }

    /// A `Conflict` that names the revoked upstream node (`kind id`) and says why.
    pub fn assert_names_revoked(message: &str, kind: &str, id: &str) {
        assert!(message.contains(&format!("{kind} {id}")), "{message}");
        assert!(message.contains("revoked"), "{message}");
    }

    /// What a refused submission must leave untouched: the bodies of every object
    /// of the namespace, every edge into the request's dependencies, and the audit
    /// chain.
    #[derive(Debug, PartialEq)]
    pub struct Shape {
        objects: Vec<(&'static str, Vec<Value>)>,
        dependents: Vec<Vec<(String, String)>>,
        audit: usize,
    }

    pub async fn shape(store: &Store, ctx: &Context, nodes: &[(&str, String)]) -> Shape {
        let mut session = store.session().await.unwrap();
        let mut objects = Vec::new();
        for kind in OBJECT_KINDS {
            let bodies: Vec<Value> = session.list(ctx, kind).await.unwrap();
            objects.push((kind, bodies));
        }
        let mut dependents = Vec::new();
        for (kind, id) in nodes {
            let mut found = session.dependents(ctx, kind, id).await.unwrap();
            found.sort();
            dependents.push(found);
        }
        session.commit().await.unwrap();
        let audit = store.verify_audit(ctx).await.unwrap();
        Shape {
            objects,
            dependents,
            audit,
        }
    }

    /// The idempotency row of a request key, `Ok(None)` when there is none.
    pub async fn idempotency_row(
        store: &Store,
        ctx: &Context,
        operation: &str,
        key: &str,
    ) -> Result<Option<ManagementJob>, Error> {
        let mut session = store.session().await.unwrap();
        let row = session
            .cached::<ManagementJob, _>(ctx, operation, key, &json!({}))
            .await;
        session.commit().await.unwrap();
        row
    }

    /// No idempotency row: a half-recorded submission would answer differently.
    pub async fn assert_no_idempotency_row(
        store: &Store,
        ctx: &Context,
        operation: &str,
        key: &str,
    ) {
        let row = idempotency_row(store, ctx, operation, key).await;
        assert!(matches!(row, Ok(None)), "{operation} {key}: {row:?}");
    }
}

// ---------------------------------------------------------------------------
// Fixture: trusted runs and the source selection grant that depends on them (the
// world of `exploration.start` is not needed here); copied from
// tests/dispatch_management.rs
// ---------------------------------------------------------------------------

mod exploration_fx {
    use super::*;
    use evo_core::evidence::{ExecutionAttestation, Purpose, SourceSelection, TaskOrigin};
    use evo_core::hash;
    use evo_core::optimization::{
        OptimizationTrace, SkillFailureDiagnosis, SkillFailureKind, TraceOutcome,
    };
    use evo_core::skill_edit::EvidenceRef;
    use evo_engine::evidence::{
        StoredRunRecord, StoredTraceAuthority, store_source_selection, store_trace_authority,
    };

    pub const RUNS: [&str; 2] = ["run-failure", "run-success"];
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
    }

    impl Chain {
        pub async fn build() -> Self {
            let fixture = Fixture::new().await;
            let coordinator = curriculum(&fixture).await;
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
                        development_request_fact_id: request_fact_id,
                        development_observed_fact_id: observed_fact_id,
                    })
                    .await
                    .unwrap();
                assert_eq!(state.completed_cycles.len(), n + 1);
            }
            Self { fixture }
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

        let mut v1 = ExperimentPlan::first_low_risk("formal-e05", 60).unwrap();
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
// Fixture: `curriculum.step` over a hand-built graph. The request's two
// dependencies (the profile and the learner state) are live artifacts; a case
// hangs nodes below them with the edges, bodies and tombstones it wants to judge.
// ---------------------------------------------------------------------------

mod synthetic {
    use super::*;
    use evo_engine::curriculum::{curriculum_profile_storage_id, curriculum_state_storage_id};
    use evo_storage::lifecycle::{REVOKE_TOMBSTONE_SCHEMA, RevokeTombstone};

    pub const PROFILE_ID: &str = "synthetic-profile";
    pub const STATE_ID: &str = "synthetic-state";

    pub fn profile() -> String {
        curriculum_profile_storage_id(PROFILE_ID).unwrap()
    }

    pub fn state() -> String {
        curriculum_state_storage_id(STATE_ID).unwrap()
    }

    pub fn request(request_key: &str) -> Value {
        json!({
            "schema_version": "rsia.management.curriculum_step.v1",
            "request_key": request_key,
            "profile_id": PROFILE_ID,
            "state_id": STATE_ID,
            "root_budget_limit_micros": 1_000u64,
        })
    }

    pub fn dependencies() -> [(&'static str, String); 2] {
        [("artifact", profile()), ("artifact", state())]
    }

    pub fn live(id: &str) -> Value {
        json!({"id": id, "schema_version": "fixture.live.v1"})
    }

    pub fn redacted(id: &str) -> Value {
        json!({"id": id, "schema_version": REDACTED})
    }

    /// A tombstone as `begin_revoke` writes it, for a source of `source_kind`.
    pub fn tombstone(id: &str, source_kind: &str) -> Value {
        serde_json::to_value(RevokeTombstone {
            id: id.into(),
            schema_version: REVOKE_TOMBSTONE_SCHEMA.into(),
            source_kind: source_kind.into(),
            reason: "test".into(),
            watermark_seq: 1,
            watermark_digest: "a".repeat(64),
            created_at: 1,
        })
        .unwrap()
    }

    /// A tombstone body `begin_revoke` never writes.
    pub fn corrupt(id: &str) -> Value {
        json!({"id": id})
    }

    /// A store whose request dependencies, the profile and the learner state, are
    /// live and have nothing below them yet.
    pub async fn open(name: &str) -> (tempfile::TempDir, Store, Context) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join(name)).await.unwrap();
        let admin = Context::new("n", "admin", Role::Admin).unwrap();
        put_raw(&store, &admin, "artifact", &profile(), &live(&profile())).await;
        put_raw(&store, &admin, "artifact", &state(), &live(&state())).await;
        (dir, store, admin)
    }

    /// Edges `(src kind, src id, dst kind, dst id)`; the id `PROFILE` or `STATE`
    /// stands for the request's profile or learner state.
    pub async fn put_edges(store: &Store, ctx: &Context, edges: &[(&str, &str, &str, &str)]) {
        let resolve = |id: &str| match id {
            "PROFILE" => profile(),
            "STATE" => state(),
            other => other.to_string(),
        };
        let mut session = store.session().await.unwrap();
        for (src_kind, src_id, dst_kind, dst_id) in edges {
            session
                .put_edge(ctx, src_kind, &resolve(src_id), dst_kind, &resolve(dst_id))
                .await
                .unwrap();
        }
        session.commit().await.unwrap();
    }

    /// `length` nodes below the learner state, one behind the other: the live
    /// artifacts `link-1` .. `link-{length - 1}` and a last node `tail` of kind
    /// `tail_kind`. The closure of the request is the profile, the state and this
    /// chain, so it holds `length + 2` nodes.
    pub async fn put_chain(store: &Store, ctx: &Context, length: usize, tail_kind: &str) {
        let mut session = store.session().await.unwrap();
        let mut previous = state();
        for index in 1..length {
            let id = format!("link-{index}");
            session
                .put_edge(ctx, "artifact", &previous, "artifact", &id)
                .await
                .unwrap();
            previous = id;
        }
        session
            .put_edge(ctx, "artifact", &previous, tail_kind, "tail")
            .await
            .unwrap();
        session.commit().await.unwrap();
    }
}

// ---------------------------------------------------------------------------
// 1. begin_revoke only: the cleanup has not reached the live dependency yet
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_replay_run_over_a_pool_whose_source_run_is_revoked_is_refused_at_submit() {
    let (_dir, store, admin) = exploration_fx::open("gate-replay.sqlite3").await;
    let pool = replay_fx::setup_sealed_pool(&admin, &store).await;
    let pool_storage = evo_storage::replay::replay_pool_storage_id(&pool.pool_digest).unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();

    // A request over the live pool is accepted and runs: the gate has no reason
    // to refuse it.
    let live = dispatcher
        .submit(
            &admin,
            "replay.run",
            replay_fx::run_request("replay-live", &pool),
        )
        .await
        .unwrap();
    assert_eq!(
        wait_job(&store, &admin, &live.id).await.state,
        ManagementJobState::Succeeded
    );

    // Only `begin_revoke` ran: the tombstone exists and the watermark moved, the
    // cleanup is Pending and has touched nothing, so the pool is still live and
    // the direct check of the dependency sees no revocation.
    let cleanup = begin(&store, &admin, "source-2").await;
    assert_eq!(cleanup.state, CleanupState::Pending);
    assert_eq!(cleanup.processed_nodes, 0);
    assert_ne!(
        raw(&store, &admin, "artifact", &pool_storage).await["schema_version"],
        REDACTED,
        "the pool is not redacted yet"
    );

    let nodes = [("artifact", pool_storage.clone())];
    let before = shape(&store, &admin, &nodes).await;
    let message = expect_conflict(
        dispatcher
            .submit(
                &admin,
                "replay.run",
                replay_fx::run_request("replay-after-begin", &pool),
            )
            .await,
        "submit over a pool whose source run is revoked",
    );
    assert_names_revoked(&message, "run", "source-2");
    assert_eq!(
        shape(&store, &admin, &nodes).await,
        before,
        "a refused submission writes no private input, job, edge or audit record"
    );
    assert_no_idempotency_row(&store, &admin, "replay.run", "replay-after-begin").await;

    // The refusal does not disturb the cleanup, and once the pool is redacted the
    // direct check names it.
    let cleanup = drive(&store, &admin, cleanup, 8).await;
    assert_eq!(cleanup.state, CleanupState::Complete, "{cleanup:?}");
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
    assert!(message.contains(&pool_storage), "{message}");
    assert!(message.contains("redacted"), "{message}");
}

#[tokio::test]
async fn a_curriculum_step_over_a_learner_state_derived_from_a_revoked_run_is_refused_at_submit() {
    use curriculum_fx::*;
    let chain = Chain::build().await;
    let (store, admin) = (&chain.fixture.store, &chain.fixture.admin);
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();

    let live = dispatcher
        .submit(admin, "curriculum.step", step_request("curriculum-live"))
        .await
        .unwrap();
    assert_eq!(
        wait_job(store, admin, &live.id).await.state,
        ManagementJobState::Succeeded
    );

    // `run-a` is a source of the stage facts the learner state depends on.
    let cleanup = begin(store, admin, "run-a").await;
    assert_eq!(cleanup.state, CleanupState::Pending);
    assert_eq!(cleanup.processed_nodes, 0);
    let state_storage = storage_id(STATE_KIND, STATE_ID);
    assert_eq!(
        raw(store, admin, "artifact", &state_storage).await["schema_version"],
        ENVELOPE,
        "the learner state is not redacted yet"
    );

    let nodes = [
        ("artifact", storage_id(PROFILE_KIND, PROFILE_ID)),
        ("artifact", state_storage.clone()),
    ];
    let before = shape(store, admin, &nodes).await;
    let message = expect_conflict(
        dispatcher
            .submit(
                admin,
                "curriculum.step",
                step_request("curriculum-after-begin"),
            )
            .await,
        "submit over a learner state derived from a revoked run",
    );
    assert_names_revoked(&message, "run", "run-a");
    assert_eq!(
        shape(store, admin, &nodes).await,
        before,
        "a refused submission writes no private input, job, edge or audit record"
    );
    assert_no_idempotency_row(store, admin, "curriculum.step", "curriculum-after-begin").await;

    let cleanup = drive(store, admin, cleanup, 8).await;
    assert_eq!(cleanup.state, CleanupState::Complete, "{cleanup:?}");
}

#[tokio::test]
async fn an_experiment_register_over_an_input_derived_from_a_revoked_run_is_refused_at_submit() {
    // The holdout's first protected input is the stored source selection, an
    // artifact that depends on the runs of its grant: the one edge path from a run
    // to an `experiment.register` input. The oracle answers sit in the request,
    // so an accepted request would store them in plain text next to a revoked run.
    let selection = exploration_fx::selection_id();
    let e05 = e05_fx::fixture(&selection).await;
    exploration_fx::setup(&e05.store).await;
    let (store, evaluator) = (&e05.store, &e05.evaluator);
    let admin = Context::new("n", "admin", Role::Admin).unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![evaluator.clone()]).unwrap();

    let live = dispatcher
        .submit(
            evaluator,
            "experiment.register",
            e05.register_request("register-live"),
        )
        .await
        .unwrap();
    let live = wait_job(store, evaluator, &live.id).await;
    assert_eq!(live.state, ManagementJobState::Succeeded, "{live:?}");

    let cleanup = begin(store, &admin, "run-failure").await;
    assert_eq!(cleanup.state, CleanupState::Pending);
    assert_eq!(cleanup.processed_nodes, 0);
    assert_ne!(
        raw(store, evaluator, "artifact", &selection).await["schema_version"],
        REDACTED,
        "the input artifact is not redacted yet"
    );

    let mut nodes = vec![("artifact", selection.clone())];
    nodes.extend(e05.artifact_ids().into_iter().map(|id| ("artifact", id)));
    let before = shape(store, evaluator, &nodes).await;
    let message = expect_conflict(
        dispatcher
            .submit(
                evaluator,
                "experiment.register",
                e05.register_request("register-after-begin"),
            )
            .await,
        "submit over an input derived from a revoked run",
    );
    assert_names_revoked(&message, "run", "run-failure");
    assert_eq!(
        shape(store, evaluator, &nodes).await,
        before,
        "a refused submission writes no private input, job, edge or audit record"
    );
    assert_no_idempotency_row(
        store,
        evaluator,
        "experiment.register",
        "register-after-begin",
    )
    .await;

    // `evaluation.start` and `evaluation.status` read only the control, the
    // holdout and the ticket, which have no edge to a run: still accepted.
    for (operation, request, expected) in [
        (
            "evaluation.start",
            e05.start_request("start-after-begin"),
            ManagementJobState::Blocked,
        ),
        (
            "evaluation.status",
            e05.status_request("status-after-begin"),
            ManagementJobState::Succeeded,
        ),
    ] {
        let queued = dispatcher
            .submit(evaluator, operation, request)
            .await
            .unwrap_or_else(|error| panic!("{operation} was refused: {error:?}"));
        let done = wait_job(store, evaluator, &queued.id).await;
        assert_eq!(done.state, expected, "{operation}: {done:?}");
    }

    let cleanup = drive(store, &admin, cleanup, 8).await;
    assert_eq!(cleanup.state, CleanupState::Complete, "{cleanup:?}");
}

#[tokio::test]
async fn a_recorded_request_key_returns_its_job_before_the_gate_runs() {
    let (_dir, store, admin) = exploration_fx::open("gate-replay-key.sqlite3").await;
    let pool = replay_fx::setup_sealed_pool(&admin, &store).await;
    let pool_storage = evo_storage::replay::replay_pool_storage_id(&pool.pool_digest).unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let request = replay_fx::run_request("replay-recorded", &pool);
    let accepted = dispatcher
        .submit(&admin, "replay.run", request.clone())
        .await
        .unwrap();
    assert_eq!(
        wait_job(&store, &admin, &accepted.id).await.state,
        ManagementJobState::Succeeded
    );

    let cleanup = begin(&store, &admin, "source-2").await;
    assert_eq!(cleanup.state, CleanupState::Pending);

    // The key was accepted before the revocation: a replay returns the stored
    // job, as it did, and writes nothing.
    let nodes = [("artifact", pool_storage)];
    let before = shape(&store, &admin, &nodes).await;
    let replayed = dispatcher
        .submit(&admin, "replay.run", request)
        .await
        .unwrap();
    assert_eq!(replayed.id, accepted.id);
    assert_eq!(shape(&store, &admin, &nodes).await, before);

    // A key that was never accepted is a new request and meets the gate.
    let message = expect_conflict(
        dispatcher
            .submit(
                &admin,
                "replay.run",
                replay_fx::run_request("replay-new-key", &pool),
            )
            .await,
        "a new key over the same revoked source",
    );
    assert_names_revoked(&message, "run", "source-2");
    assert_eq!(shape(&store, &admin, &nodes).await, before);
}

// ---------------------------------------------------------------------------
// 2. The verdict on each upstream node: its own kind, its own body
// ---------------------------------------------------------------------------

enum Verdict {
    Accepted,
    /// Refused with a `Conflict` whose message holds every one of these parts.
    Refused(&'static [&'static str]),
}

/// Submits one `curriculum.step` over the prepared store and compares the answer
/// with the verdict; returns what differs (nothing when it matches), so that a
/// table reports every row it fails and not only the first. A refusal must also
/// have written nothing, and its message must not hold any of `forbidden`.
async fn judge(
    store: &Store,
    admin: &Context,
    name: &str,
    key: &str,
    verdict: &Verdict,
    forbidden: &[&str],
) -> Vec<String> {
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let dependencies = synthetic::dependencies();
    let before = shape(store, admin, &dependencies).await;
    let result = dispatcher
        .submit(admin, "curriculum.step", synthetic::request(key))
        .await;
    let mut problems = Vec::new();
    match (verdict, result) {
        (Verdict::Accepted, Ok(job)) => {
            // The synthetic state is no real learner state: the job fails inside,
            // which is not what is under test, but it has to finish before the
            // store goes away.
            wait_job(store, admin, &job.id).await;
        }
        (Verdict::Accepted, Err(error)) => {
            problems.push(format!("{name}: refused, expected accepted: {error:?}"));
        }
        (Verdict::Refused(parts), Err(Error::Conflict(message))) => {
            for part in parts.iter() {
                if !message.contains(part) {
                    problems.push(format!("{name}: `{message}` lacks `{part}`"));
                }
            }
            for part in forbidden {
                if message.contains(part) {
                    problems.push(format!("{name}: `{message}` holds `{part}`"));
                }
            }
            if shape(store, admin, &dependencies).await != before {
                problems.push(format!("{name}: a refused submission wrote to the store"));
            }
            let row = idempotency_row(store, admin, "curriculum.step", key).await;
            if !matches!(row, Ok(None)) {
                problems.push(format!("{name}: a refused submission left {row:?}"));
            }
        }
        (Verdict::Refused(_), Ok(job)) => {
            wait_job(store, admin, &job.id).await;
            problems.push(format!(
                "{name}: accepted as {} (step {}), expected a Conflict",
                job.id, job.step
            ));
        }
        (Verdict::Refused(_), Err(other)) => {
            problems.push(format!("{name}: expected a Conflict, got {other:?}"));
        }
    }
    problems
}

struct Case {
    name: &'static str,
    /// Objects stored besides the live profile and learner state: (kind, body).
    objects: Vec<(&'static str, Value)>,
    /// Edges (src kind, src id, dst kind, dst id); `PROFILE` / `STATE` stand for
    /// the request's two dependencies.
    edges: Vec<(&'static str, &'static str, &'static str, &'static str)>,
    /// Tombstones, each keyed by the id of its source alone.
    tombstones: Vec<Value>,
    verdict: Verdict,
}

#[tokio::test]
async fn every_upstream_run_and_artifact_is_judged_by_its_own_kind_and_body() {
    use synthetic::{corrupt, live, redacted, tombstone};
    let cases = vec![
        // The tombstone of the node's own kind refuses it, whether the node is
        // still stored or the cleanup already deleted it (a revoked run).
        Case {
            name: "a revoked run below the state",
            objects: vec![("run", live("run-r1"))],
            edges: vec![("artifact", "STATE", "run", "run-r1")],
            tombstones: vec![tombstone("run-r1", "run")],
            verdict: Verdict::Refused(&["run run-r1", "revoked"]),
        },
        Case {
            name: "a revoked run the cleanup already deleted",
            objects: vec![],
            edges: vec![("artifact", "STATE", "run", "run-r2")],
            tombstones: vec![tombstone("run-r2", "run")],
            verdict: Verdict::Refused(&["run run-r2", "revoked"]),
        },
        Case {
            name: "a revoked import source artifact below the state",
            objects: vec![("artifact", live("art-a1"))],
            edges: vec![("artifact", "STATE", "artifact", "art-a1")],
            tombstones: vec![tombstone("art-a1", "artifact")],
            verdict: Verdict::Refused(&["artifact art-a1", "revoked"]),
        },
        Case {
            name: "a revoked run reached from the profile, not the state",
            objects: vec![("run", live("run-r6"))],
            edges: vec![("artifact", "PROFILE", "run", "run-r6")],
            tombstones: vec![tombstone("run-r6", "run")],
            verdict: Verdict::Refused(&["run run-r6", "revoked"]),
        },
        // The walk goes through nodes of every kind, not only runs and artifacts.
        Case {
            name: "a revoked run behind a live artifact, a release and a candidate",
            objects: vec![
                ("artifact", live("art-m1")),
                ("release", live("rel-1")),
                ("candidate", live("cand-1")),
                ("run", live("run-r3")),
            ],
            edges: vec![
                ("artifact", "STATE", "artifact", "art-m1"),
                ("artifact", "art-m1", "release", "rel-1"),
                ("release", "rel-1", "candidate", "cand-1"),
                ("candidate", "cand-1", "run", "run-r3"),
            ],
            tombstones: vec![tombstone("run-r3", "run")],
            verdict: Verdict::Refused(&["run run-r3", "revoked"]),
        },
        Case {
            name: "a cycle that leads to a revoked run",
            objects: vec![("run", live("run-r4"))],
            edges: vec![
                ("artifact", "STATE", "artifact", "art-c1"),
                ("artifact", "art-c1", "artifact", "art-c2"),
                ("artifact", "art-c2", "artifact", "art-c1"),
                ("artifact", "art-c2", "run", "run-r4"),
            ],
            tombstones: vec![tombstone("run-r4", "run")],
            verdict: Verdict::Refused(&["run run-r4", "revoked"]),
        },
        Case {
            name: "a diamond that leads to a revoked run",
            objects: vec![("run", live("run-r5"))],
            edges: vec![
                ("artifact", "STATE", "artifact", "art-d1"),
                ("artifact", "STATE", "artifact", "art-d2"),
                ("artifact", "art-d1", "run", "run-r5"),
                ("artifact", "art-d2", "run", "run-r5"),
            ],
            tombstones: vec![tombstone("run-r5", "run")],
            verdict: Verdict::Refused(&["run run-r5", "revoked"]),
        },
        // A tombstone is keyed by the id alone and records the kind of the source it
        // was written for: one written for the other kind is another object's.
        Case {
            name: "a run whose id a revoked artifact shares",
            objects: vec![("run", live("shared-1"))],
            edges: vec![("artifact", "STATE", "run", "shared-1")],
            tombstones: vec![tombstone("shared-1", "artifact")],
            verdict: Verdict::Accepted,
        },
        Case {
            name: "an artifact whose id a revoked run shares",
            objects: vec![("artifact", live("shared-2"))],
            edges: vec![("artifact", "STATE", "artifact", "shared-2")],
            tombstones: vec![tombstone("shared-2", "run")],
            verdict: Verdict::Accepted,
        },
        Case {
            name: "a run and an artifact of one id, the run revoked",
            objects: vec![("run", live("shared-3")), ("artifact", live("shared-3"))],
            edges: vec![
                ("artifact", "STATE", "artifact", "shared-3"),
                ("artifact", "STATE", "run", "shared-3"),
            ],
            tombstones: vec![tombstone("shared-3", "run")],
            verdict: Verdict::Refused(&["run shared-3", "revoked"]),
        },
        // A tombstone that is not one `begin_revoke` writes cannot be matched to
        // anything: the node is treated as revoked.
        Case {
            name: "a run whose tombstone does not decode",
            objects: vec![("run", live("run-x1"))],
            edges: vec![("artifact", "STATE", "run", "run-x1")],
            tombstones: vec![corrupt("run-x1")],
            verdict: Verdict::Refused(&["run run-x1", "cannot be read"]),
        },
        Case {
            name: "an artifact whose tombstone does not decode",
            objects: vec![("artifact", live("art-x2"))],
            edges: vec![("artifact", "STATE", "artifact", "art-x2")],
            tombstones: vec![corrupt("art-x2")],
            verdict: Verdict::Refused(&["artifact art-x2", "cannot be read"]),
        },
        Case {
            name: "an artifact whose tombstone names an unknown source kind",
            objects: vec![("artifact", live("art-x3"))],
            edges: vec![("artifact", "STATE", "artifact", "art-x3")],
            tombstones: vec![tombstone("art-x3", "release")],
            verdict: Verdict::Refused(&["artifact art-x3", "cannot be read"]),
        },
        // Only runs and artifacts are judged by a tombstone, and only artifacts by
        // their body.
        Case {
            name: "a release whose id carries a tombstone that does not decode",
            objects: vec![("release", live("rel-2"))],
            edges: vec![("artifact", "STATE", "release", "rel-2")],
            tombstones: vec![corrupt("rel-2")],
            verdict: Verdict::Accepted,
        },
        Case {
            name: "an artifact the cleanup redacted, without a tombstone of its own",
            objects: vec![("artifact", redacted("art-w1"))],
            edges: vec![("artifact", "STATE", "artifact", "art-w1")],
            tombstones: vec![],
            verdict: Verdict::Refused(&["artifact art-w1", "redacted"]),
        },
        Case {
            name: "a run whose stored body says redacted",
            objects: vec![("run", redacted("run-q1"))],
            edges: vec![("artifact", "STATE", "run", "run-q1")],
            tombstones: vec![],
            verdict: Verdict::Accepted,
        },
        Case {
            name: "live runs and artifacts only",
            objects: vec![
                ("artifact", live("art-l1")),
                ("run", live("run-l2")),
                ("run", live("run-l3")),
            ],
            edges: vec![
                ("artifact", "STATE", "artifact", "art-l1"),
                ("artifact", "art-l1", "run", "run-l2"),
                ("artifact", "PROFILE", "run", "run-l3"),
            ],
            tombstones: vec![],
            verdict: Verdict::Accepted,
        },
    ];

    let mut problems = Vec::new();
    for case in cases {
        let (_dir, store, admin) = synthetic::open("cases.sqlite3").await;
        for (kind, body) in &case.objects {
            put_raw(&store, &admin, kind, body["id"].as_str().unwrap(), body).await;
        }
        synthetic::put_edges(&store, &admin, &case.edges).await;
        for body in &case.tombstones {
            put_raw(
                &store,
                &admin,
                "tombstone",
                body["id"].as_str().unwrap(),
                body,
            )
            .await;
        }
        problems.extend(judge(&store, &admin, case.name, "case-key", &case.verdict, &[]).await);
    }
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[tokio::test]
async fn a_revoked_source_of_another_namespace_does_not_refuse() {
    let (_dir, store, admin) = synthetic::open("namespaces.sqlite3").await;
    let other = Context::new("other", "admin", Role::Admin).unwrap();
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();

    // The same ids in the namespace `other`: the learner state depends on a run
    // there, and that run is revoked there.
    put_raw(&store, &other, "run", "run-o", &synthetic::live("run-o")).await;
    put_raw(
        &store,
        &other,
        "tombstone",
        "run-o",
        &synthetic::tombstone("run-o", "run"),
    )
    .await;
    synthetic::put_edges(&store, &other, &[("artifact", "STATE", "run", "run-o")]).await;
    let accepted = dispatcher
        .submit(&admin, "curriculum.step", synthetic::request("ns-edges"))
        .await
        .unwrap_or_else(|error| panic!("edges of another namespace were followed: {error:?}"));
    wait_job(&store, &admin, &accepted.id).await;

    // The edge exists in `n` too, but the tombstone is only in `other`.
    put_raw(&store, &admin, "run", "run-o", &synthetic::live("run-o")).await;
    synthetic::put_edges(&store, &admin, &[("artifact", "STATE", "run", "run-o")]).await;
    let accepted = dispatcher
        .submit(
            &admin,
            "curriculum.step",
            synthetic::request("ns-tombstone"),
        )
        .await
        .unwrap_or_else(|error| panic!("a tombstone of another namespace was read: {error:?}"));
    wait_job(&store, &admin, &accepted.id).await;

    // With the tombstone in `n` as well, the request is refused.
    put_raw(
        &store,
        &admin,
        "tombstone",
        "run-o",
        &synthetic::tombstone("run-o", "run"),
    )
    .await;
    let message = expect_conflict(
        dispatcher
            .submit(&admin, "curriculum.step", synthetic::request("ns-revoked"))
            .await,
        "the same graph with the tombstone in the request's own namespace",
    );
    assert_names_revoked(&message, "run", "run-o");
}

// ---------------------------------------------------------------------------
// 3. A closure over the bound is refused, never truncated and then judged live
// ---------------------------------------------------------------------------

/// The bound of the closure the submit-time check walks (plan §11.1: a derivation
/// beyond the scale is refused, never silently cut short).
const CLOSURE_BOUND: usize = 10_000;

#[tokio::test]
async fn a_closure_over_the_bound_is_refused_not_truncated() {
    // (nodes in the closure, kind and revocation of the last node, verdict). The
    // closure is the profile, the learner state and a chain below the state; its
    // last node is the one a truncated walk would never reach.
    let cases: [(usize, &str, bool, Verdict); 5] = [
        (CLOSURE_BOUND, "run", false, Verdict::Accepted),
        (
            CLOSURE_BOUND,
            "run",
            true,
            Verdict::Refused(&["run tail", "revoked"]),
        ),
        (
            CLOSURE_BOUND + 1,
            "run",
            false,
            Verdict::Refused(&["closure", "10000"]),
        ),
        (
            CLOSURE_BOUND + 1,
            "run",
            true,
            Verdict::Refused(&["closure", "10000"]),
        ),
        (
            CLOSURE_BOUND + 50,
            "artifact",
            true,
            Verdict::Refused(&["closure", "10000"]),
        ),
    ];
    let mut problems = Vec::new();
    for (size, tail_kind, revoked, verdict) in cases {
        let name = format!("{size} nodes, {tail_kind} tail, revoked {revoked}");
        let (_dir, store, admin) = synthetic::open("bound.sqlite3").await;
        synthetic::put_chain(&store, &admin, size - 2, tail_kind).await;
        if revoked {
            put_raw(
                &store,
                &admin,
                "tombstone",
                "tail",
                &synthetic::tombstone("tail", tail_kind),
            )
            .await;
        }
        // A walk cut at the bound must not name a node it never judged.
        let forbidden: &[&str] = if size > CLOSURE_BOUND { &["tail"] } else { &[] };
        problems.extend(judge(&store, &admin, &name, "bound-key", &verdict, forbidden).await);
    }
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

// ---------------------------------------------------------------------------
// 4. The controller's Q2 probe: two overlapping cleanups, continuous submissions
// ---------------------------------------------------------------------------

struct Overlap {
    /// Rounds until both cleanups finished.
    rounds: i64,
    first: CleanupStatus,
    second: CleanupStatus,
    accepted: Vec<String>,
    refused: Vec<String>,
}

/// Two cleanup jobs over overlapping closures (`source-2`, then `source-1` begun
/// after round 3; both are sources of the pool's first world), stepped
/// alternately one edge at a time. With `late`, a new `replay.run` over the pool is
/// submitted before every step.
async fn overlapping_cleanups(name: &str, late: bool) -> Overlap {
    let (_dir, store, admin) = exploration_fx::open(name).await;
    let pool = replay_fx::setup_sealed_pool(&admin, &store).await;
    let dispatcher = ManagementDispatcher::new(store.clone(), vec![admin.clone()]).unwrap();
    let queued = dispatcher
        .submit(
            &admin,
            "replay.run",
            replay_fx::run_request("ctrl-q2-chain", &pool),
        )
        .await
        .unwrap();
    assert_eq!(
        wait_job(&store, &admin, &queued.id).await.state,
        ManagementJobState::Succeeded
    );
    let mut first = begin(&store, &admin, "source-2").await;
    let mut second: Option<CleanupStatus> = None;
    let (mut accepted, mut refused) = (Vec::new(), Vec::new());
    let mut round = 0i64;
    loop {
        if late {
            // The request keys are those of the controller's probe, so that the ids
            // of the private inputs (and with them their position relative to the
            // cleanups' cursors) are the ones it observed 141 acceptances with.
            let request = replay_fx::run_request(&format!("ctrl-q2-{round}"), &pool);
            match dispatcher.submit(&admin, "replay.run", request).await {
                Ok(job) => {
                    wait_job(&store, &admin, &job.id).await;
                    accepted.push(job.id);
                }
                Err(Error::Conflict(message)) => refused.push(message),
                Err(other) => panic!("round {round}: {other:?}"),
            }
        }
        if round == 3 {
            second = Some(begin(&store, &admin, "source-1").await);
        }
        if finished(&first) && second.as_ref().is_some_and(finished) {
            break;
        }
        round += 1;
        assert!(round < 900, "the cleanups did not finish");
        if !finished(&first) && (round % 2 == 1 || second.as_ref().is_none_or(finished)) {
            first = step(&store, &admin, &first, 1, 500 + round).await;
        } else if let Some(status) = second.as_mut().filter(|status| !finished(status)) {
            *status = step(&store, &admin, status, 1, 500 + round).await;
        }
    }
    Overlap {
        rounds: round,
        first,
        second: second.unwrap(),
        accepted,
        refused,
    }
}

#[tokio::test]
async fn continuous_submissions_over_two_overlapping_cleanups_accept_none_and_cost_no_extra_steps()
{
    // Without a single late submission: the steps and the processed nodes of the
    // two cleanups.
    let baseline = overlapping_cleanups("overlap-baseline.sqlite3", false).await;
    for status in [&baseline.first, &baseline.second] {
        assert_eq!(status.state, CleanupState::Complete, "{status:?}");
        assert_eq!(status.last_error, None);
        assert_eq!(status.pending_nodes, 0);
    }

    // A new request before every one of those steps. On the code before this
    // change the cleanups needed 724 steps instead of the baseline's and 141 of the
    // requests were accepted (the rest were refused once the cleanup had redacted
    // the pool); each accepted one was a late write the cleanup had to reach
    // before it could report Complete.
    let probed = overlapping_cleanups("overlap-probed.sqlite3", true).await;
    println!(
        "OVERLAP baseline: {} rounds, {}/{} nodes | probed: {} rounds, {}/{} nodes, {} accepted, {} refused",
        baseline.rounds,
        baseline.first.processed_nodes,
        baseline.second.processed_nodes,
        probed.rounds,
        probed.first.processed_nodes,
        probed.second.processed_nodes,
        probed.accepted.len(),
        probed.refused.len(),
    );
    assert!(
        probed.accepted.is_empty(),
        "{} submissions were accepted over a revoked source",
        probed.accepted.len()
    );
    assert_eq!(
        probed.refused.len() as i64,
        probed.rounds + 1,
        "one refusal per round"
    );
    for message in &probed.refused {
        assert!(message.contains("revoked"), "{message}");
    }
    for status in [&probed.first, &probed.second] {
        assert_eq!(status.state, CleanupState::Complete, "{status:?}");
        assert_eq!(status.last_error, None);
        assert_eq!(status.pending_nodes, 0);
    }
    assert_eq!(
        probed.rounds, baseline.rounds,
        "the cleanups took more steps"
    );
    assert_eq!(probed.first.processed_nodes, baseline.first.processed_nodes);
    assert_eq!(
        probed.second.processed_nodes,
        baseline.second.processed_nodes
    );
}
