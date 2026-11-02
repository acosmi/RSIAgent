//! Local real config-projection execution; no model, paid call or Host authority.
#[path = "support/trusted_compiler_inputs.rs"]
mod trusted_compiler_inputs;

use evo_core::contract::{
    CompileParts, HostCapabilities, Profile, ResolvedBundleV3, SkillSnapshot,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::{Context, Error, Result, Role, Strategy, fingerprint, hash};
use evo_engine::development::projection::{
    CONFIG_PROJECTION_CONTROL_SCHEMA, ConfigProjectionControlV1, ConfigProjectionDevelopmentRunner,
    ConfigProjectionExecutionIssueRequest, ConfigProjectionExecutionRequestV1,
    ConfigProjectionTaskV1, config_projection_runner_contract,
    issue_config_projection_execution_receipt, load_config_projection_execution_receipt,
    register_config_projection_control,
};
use evo_engine::development::{
    CandidateConsumption, DevelopmentSide, EXECUTION_OUTPUT_KIND, EXECUTION_RECEIPT_KIND,
    ExecutionReceiptIssueRequest, execution_budget_call_id, execution_output_id,
    execution_receipt_id, issue_execution_receipt, load_execution_output, registered_runner_digest,
    storage_id, typed_receipt_closure,
};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::lifecycle::LifecycleCoordinator;
use evo_engine::optimization::{
    DevRunner, DevelopmentRunReport, DevelopmentRunRequest, OPTIMIZATION_STAGE_FACT_SCHEMA,
    OptimizationJournal, OptimizationJournalStage, StageDependency, StageFact, StageFactKind,
    StoreOptimizationJournal, VerifiedDevelopmentProjectionView, verify_development_observation,
    verify_development_projection_observation,
};
use evo_storage::Store;
use evo_storage::budget::{BudgetCallRecord, BudgetCallState, RootBudgetAuthorization};
use evo_storage::lifecycle::CleanupState;
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;

static CONFIG_WORKSPACE_TEST_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const HIGH: &str = "config_precedence.v1: highest_layer_first";
const LOW: &str = "config_precedence.v1: lowest_layer_first";
const NS: &str = "config-development";
const SCOPE: &str = "config-local-budget";

fn d(text: &str) -> String {
    hash(text.as_bytes())
}
fn ctx(actor: &str, role: Role) -> Context {
    Context::new(NS, actor, role).unwrap()
}
fn selector(placement: &str) -> Value {
    json!({"template":"F","layer_count":2,"key_type":"B","placement":placement,"seed":1})
}
fn source() -> StoredTraceAuthority {
    let body = b"real local config source fixture".to_vec();
    StoredTraceAuthority {
        schema_version: "rsia.optimization.source.v1".into(),
        record: StoredRunRecord {
            id: "source-config".into(),
            body: body.clone(),
            parent_family: "config-family".into(),
            task_origin: TaskOrigin::TrustedRun,
            execution_attestation: ExecutionAttestation::TrustedHost,
            purpose: Purpose::Development,
        },
        trace: OptimizationTrace {
            run_id: "source-config".into(),
            parent_family: "config-family".into(),
            source_digest: hash(&body),
            purpose: Purpose::Development,
            outcome: TraceOutcome::Success,
            diagnosis: None,
            excerpt: String::from_utf8(body.clone()).unwrap(),
            seed: 1,
        },
        excerpt_start: 0,
        excerpt_end: body.len(),
    }
}

struct Fixture {
    directory: tempfile::TempDir,
    store: Store,
    admin: Context,
    executor: Context,
    grader: Context,
    control: ConfigProjectionControlV1,
    parent: ResolvedBundleV3,
    candidate: ResolvedBundleV3,
}

impl Fixture {
    async fn new(parent: &str, candidate: &str, with_source: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("config.sqlite3"))
            .await
            .unwrap();
        let admin = ctx("admin", Role::Admin);
        let executor = ctx("executor", Role::Worker);
        let grader = ctx("grader", Role::Evaluator);
        if with_source {
            store_trace_authority(&store, &ctx("source-host", Role::Host), &source())
                .await
                .unwrap();
        }
        let mut s = store.session().await.unwrap();
        s.bump_watermark(&admin, "config-initial").await.unwrap();
        s.commit().await.unwrap();
        store
            .authorize_root_budget(
                &admin,
                &RootBudgetAuthorization {
                    root_budget_id: "config-root".into(),
                    billing_scope: SCOPE.into(),
                    allowed_namespaces: vec![NS.into()],
                    currency: "USD".into(),
                    pricing_version: "local-zero-v1".into(),
                    payment_subject: "software-fixture".into(),
                    authorization_receipt_digest: d("local-admin-budget"),
                    per_call_cap_micros: 10,
                    total_limit_micros: 1000,
                    created_at: 1,
                },
            )
            .await
            .unwrap();
        let control = ConfigProjectionControlV1::registered(
            "config-control",
            NS,
            SCOPE,
            "config-root",
            "executor",
            "grader",
            "optimizer",
            "config-manifest",
            vec![ConfigProjectionTaskV1::registered("config-family", selector("E")).unwrap()],
            d("config-environment"),
            d("rules"),
            d("tools"),
            if with_source {
                vec!["source-config".into()]
            } else {
                vec![]
            },
            1,
        )
        .unwrap();
        register_config_projection_control(&admin, &store, control.clone())
            .await
            .unwrap();
        let parent = Self::bundle_in(
            &store,
            &executor,
            &control.environment_digest,
            parent,
            "parent-profile",
        )
        .await;
        let candidate = Self::bundle_in(
            &store,
            &executor,
            &control.environment_digest,
            candidate,
            "candidate-profile",
        )
        .await;
        Self {
            directory,
            store,
            admin,
            executor,
            grader,
            control,
            parent,
            candidate,
        }
    }

    async fn bundle_in(
        store: &Store,
        executor: &Context,
        environment: &str,
        content: &str,
        profile_id: &str,
    ) -> ResolvedBundleV3 {
        let skill = SkillSnapshot {
            content: content.into(),
            ..SkillSnapshot::empty()
        };
        let profile = Profile {
            id: profile_id.into(),
            evolution_enabled: true,
            parent_digest: d("experimental-parent"),
            baseline_digest: d("experimental-baseline"),
        };
        let baseline = SkillSnapshot::empty();
        let strategy = Strategy::default();
        let caps = HostCapabilities {
            available: Default::default(),
            granted: Default::default(),
        };
        let bundle = trusted_compiler_inputs::bundle_current(
            store,
            executor,
            CompileParts {
                profile: &profile,
                parent: &skill,
                baseline: &baseline,
                parent_strategy: &strategy,
                baseline_strategy: &strategy,
                skill_patch: &Default::default(),
                improver_patch: &Default::default(),
                caps: &caps,
                revoked: &Default::default(),
            },
            environment,
            1,
        )
        .await
        .unwrap();
        trusted_compiler_inputs::persist_bundle_current(store, executor, &bundle, environment, 1)
            .await
            .unwrap();
        bundle
    }

    fn request(&self, key: &str) -> DevelopmentRunRequest {
        DevelopmentRunRequest {
            request_id: format!("request-{key}"),
            namespace: NS.into(),
            purpose: Purpose::Development,
            episode_id: format!("episode-{key}"),
            step: 1,
            attempt: 1,
            manifest: self.control.manifest().unwrap(),
            parent_bundle_digest: self.parent.digest.clone(),
            candidate_bundle_digest: self.candidate.digest.clone(),
            environment_digest: self.control.environment_digest.clone(),
            grader_digest: self.control.grader_digest.clone(),
            rules_digest: self.control.rules_digest.clone(),
            tools_digest: self.control.tools_digest.clone(),
            revoke_watermark: 1,
            idempotency_key: format!("config-key-{key}"),
        }
    }

    fn runner(&self) -> ConfigProjectionDevelopmentRunner {
        ConfigProjectionDevelopmentRunner::with_clock(
            self.store.clone(),
            self.executor.clone(),
            self.grader.clone(),
            self.control.id.clone(),
            600,
            Arc::new(|| 100),
        )
        .unwrap()
    }

    async fn journal(&self, fact: StageFact, request: &DevelopmentRunRequest) -> String {
        let inputs = trusted_compiler_inputs::bundle_use_current(
            &self.store,
            &self.executor,
            &request.candidate_bundle_digest,
            &request.environment_digest,
            request.revoke_watermark,
        )
        .await
        .unwrap();
        let id = fact.artifact_id.clone();
        StoreOptimizationJournal::new(self.store.clone(), self.executor.clone(), "executor")
            .unwrap()
            .commit_source_bound(fact, &inputs)
            .await
            .unwrap();
        id
    }

    fn facts(
        &self,
        request: &DevelopmentRunRequest,
        report: Option<&DevelopmentRunReport>,
    ) -> StageFact {
        let mut dependencies: Vec<StageDependency> = self
            .control
            .source_ids
            .iter()
            .map(|id| StageDependency {
                kind: "run".into(),
                id: id.clone(),
            })
            .collect();
        dependencies.push(StageDependency {
            kind: "revoke_watermark".into(),
            id: "1".into(),
        });
        let (kind, payload) = match report {
            None => {
                for (kind, id) in [
                    ("bundle", &request.parent_bundle_digest),
                    ("bundle", &request.candidate_bundle_digest),
                    ("environment", &request.environment_digest),
                    ("grader", &request.grader_digest),
                ] {
                    dependencies.push(StageDependency {
                        kind: kind.into(),
                        id: id.clone(),
                    });
                }
                (
                    StageFactKind::DevelopmentRequestPrepared,
                    serde_json::to_value(request).unwrap(),
                )
            }
            Some(report) => {
                dependencies.extend(typed_receipt_closure(report).unwrap());
                for result in &report.results {
                    for (kind, id) in [
                        ("execution", &result.parent_execution_id),
                        ("execution", &result.candidate_execution_id),
                        ("grader", &result.grader_receipt_digest),
                    ] {
                        dependencies.push(StageDependency {
                            kind: kind.into(),
                            id: id.clone(),
                        });
                    }
                }
                (
                    StageFactKind::DevelopmentObserved,
                    serde_json::to_value(report).unwrap(),
                )
            }
        };
        StageFact {
            schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
            artifact_id: String::new(),
            namespace: NS.into(),
            episode_id: request.episode_id.clone(),
            step: request.step,
            attempt: request.attempt,
            stage: OptimizationJournalStage::Development,
            kind,
            request_id: request.request_id.clone(),
            input_digest: request.manifest.digest.clone(),
            output_digest: None,
            dependencies,
            payload,
        }
        .seal()
        .unwrap()
    }

    async fn execute(
        &self,
        request: &DevelopmentRunRequest,
    ) -> (DevelopmentRunReport, String, String) {
        let prepared = self.journal(self.facts(request, None), request).await;
        let report = self.runner().run(request.clone()).await.unwrap();
        let observed = self
            .journal(self.facts(request, Some(&report)), request)
            .await;
        (report, prepared, observed)
    }

    async fn verify(
        &self,
        prepared: &str,
        observed: &str,
    ) -> Result<VerifiedDevelopmentProjectionView> {
        verify_development_projection_observation(&self.store, &self.executor, prepared, observed)
            .await
    }

    async fn calls(&self, request: &DevelopmentRunRequest) -> Vec<BudgetCallRecord> {
        let mut s = self.store.session().await.unwrap();
        let calls = s
            .budget_calls_for_group(&self.executor, SCOPE, &request.episode_id)
            .await
            .unwrap();
        s.commit().await.unwrap();
        calls
    }

    async fn raw(&self, kind: &str, id: &str) -> Value {
        let mut s = self.store.session().await.unwrap();
        let value = s.need(&self.admin, kind, id).await.unwrap();
        s.commit().await.unwrap();
        value
    }

    async fn put(&self, kind: &str, id: &str, value: &Value) {
        let mut s = self.store.session().await.unwrap();
        s.put(&self.admin, kind, id, self.admin.actor(), value)
            .await
            .unwrap();
        s.commit().await.unwrap();
    }

    async fn frozen(
        &self,
        request: &DevelopmentRunRequest,
        side: DevelopmentSide,
    ) -> ConfigProjectionExecutionRequestV1 {
        let calls = self.calls(request).await;
        let id =
            execution_budget_call_id(&request.request_id, &self.control.tasks[0].task_id, side)
                .unwrap();
        serde_json::from_str(
            &calls
                .iter()
                .find(|call| call.call_id == id)
                .unwrap()
                .request_artifact
                .as_ref()
                .unwrap()
                .body,
        )
        .unwrap()
    }

    fn replace_budget_request(&self, call_id: &str, body: &Value) {
        let body = serde_json::to_string(body).unwrap();
        let input =
            json!({"call_id":call_id,"scope":SCOPE,"body":body,"digest":hash(body.as_bytes())});
        let mut child = Command::new("python3").args(["-B","-c",r#"
import json,sqlite3,sys
x=json.load(sys.stdin)
c=sqlite3.connect(sys.argv[1])
c.execute('UPDATE root_budget_calls SET request_artifact_body=?,request_artifact_digest=? WHERE billing_scope=? AND call_id=?',(x['body'],x['digest'],x['scope'],x['call_id']))
c.commit()
c.close()
"#]).arg(self.directory.path().join("config.sqlite3")).stdin(Stdio::piped()).spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(serde_json::to_string(&input).unwrap().as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success());
    }
}

fn projection_refusal<T: std::fmt::Debug>(result: Result<T>) {
    assert!(
        matches!(result,Err(Error::Conflict(ref message)) if message=="development_projection_mismatch"),
        "{result:?}"
    );
}

#[tokio::test]
async fn config_projection_control_requires_complete_development_registration_and_separated_roles()
{
    let _workspace = CONFIG_WORKSPACE_TEST_GATE.lock().await;
    let f = Fixture::new(LOW, HIGH, false).await;
    assert_eq!(
        register_config_projection_control(&f.admin, &f.store, f.control.clone())
            .await
            .unwrap(),
        fingerprint(&f.control).unwrap()
    );
    assert_eq!(f.control.schema_version, CONFIG_PROJECTION_CONTROL_SCHEMA);
    let mut incomplete = f.control.clone();
    incomplete.id = "missing-material".into();
    incomplete.materials.task_materials.pop_first();
    assert!(matches!(
        register_config_projection_control(&f.admin, &f.store, incomplete).await,
        Err(Error::Conflict(_))
    ));
    assert!(matches!(
        ConfigProjectionTaskV1::registered(
            "family",
            json!({"template":"F","layer_count":2,"key_type":"I","placement":"E","seed":1})
        ),
        Err(Error::Forbidden)
    ));
    let mut actors = f.control.clone();
    actors.id = "same-actor".into();
    actors.grader_actor = actors.executor_actor.clone();
    assert!(
        matches!(register_config_projection_control(&f.admin,&f.store,actors).await,Err(Error::Invalid(ref message)) if message=="executor, grader, and proposer must be distinct actors")
    );
    let mut contract = f.control.clone();
    contract.id = "unknown-contract".into();
    contract.runner_digest = d("unknown");
    assert!(
        matches!(register_config_projection_control(&f.admin,&f.store,contract).await,Err(Error::Invalid(ref message)) if message=="unregistered config projection contract")
    );
    let executor_admin = ctx("executor", Role::Admin);
    assert!(matches!(
        register_config_projection_control(&executor_admin, &f.store, f.control.clone()).await,
        Err(Error::Forbidden)
    ));
    let (report, p, o) = f.execute(&f.request("roles")).await;
    let view = f.verify(&p, &o).await.unwrap();
    assert_eq!(
        view.observation.candidate_consumption,
        Some(CandidateConsumption::ProjectionConsumed)
    );
    assert_eq!(report.results[0].candidate_score_micros, 1_000_000);
    assert_eq!(
        registered_runner_digest().unwrap(),
        "b9d2428678d492ca3d4fc42352e0492c0d318e7b418b7bc77a98191fe04e3a09"
    );
}

#[tokio::test]
async fn equal_content_reaches_equal_real_projection_inputs_across_execution_identities() {
    let _workspace = CONFIG_WORKSPACE_TEST_GATE.lock().await;
    let f = Fixture::new(HIGH, HIGH, false).await;
    assert_ne!(f.parent.digest, f.candidate.digest);
    let first = f.request("equal-a");
    let second = f.request("equal-b");
    let (_, p, o) = f.execute(&first).await;
    let a = f.verify(&p, &o).await.unwrap();
    let (_, p, o) = f.execute(&second).await;
    let b = f.verify(&p, &o).await.unwrap();
    let a = &a.projection_inputs[0];
    let b = &b.projection_inputs[0];
    assert_eq!(
        a.parent_request_projection_digest,
        a.candidate_request_projection_digest
    );
    assert_eq!(
        a.parent_request_projection_digest,
        b.candidate_request_projection_digest
    );
    assert_eq!(
        a.parent_projection.instructions,
        b.candidate_projection.instructions
    );
    assert_ne!(
        a.parent_projection.bundle_digest,
        a.candidate_projection.bundle_digest
    );
    let parent = f.frozen(&first, DevelopmentSide::Parent).await;
    let candidate = f.frozen(&second, DevelopmentSide::Candidate).await;
    assert_ne!(
        fingerprint(&parent).unwrap(),
        fingerprint(&candidate).unwrap()
    );
    assert_eq!(parent.fd_snapshot, candidate.fd_snapshot);
    assert_eq!(
        parent.projection.instructions,
        candidate.projection.instructions
    );
}

#[tokio::test]
async fn changed_projection_policy_changes_fd_resolver_behavior() {
    let _workspace = CONFIG_WORKSPACE_TEST_GATE.lock().await;
    let f = Fixture::new(LOW, HIGH, false).await;
    let request = f.request("policy");
    let (report, p, o) = f.execute(&request).await;
    let view = f.verify(&p, &o).await.unwrap();
    let inputs = &view.projection_inputs[0];
    assert_ne!(
        inputs.parent_request_projection_digest,
        inputs.candidate_request_projection_digest
    );
    let task = &f.control.tasks[0].task_id;
    let parent = load_execution_output(
        &f.store,
        &f.executor,
        &execution_output_id(&request.request_id, task, DevelopmentSide::Parent).unwrap(),
    )
    .await
    .unwrap();
    let candidate = load_execution_output(
        &f.store,
        &f.executor,
        &execution_output_id(&request.request_id, task, DevelopmentSide::Candidate).unwrap(),
    )
    .await
    .unwrap();
    let parent: Value = serde_json::from_str(&parent.output_utf8).unwrap();
    let candidate: Value = serde_json::from_str(&candidate.output_utf8).unwrap();
    assert_eq!(parent["source_layer"], "defaults");
    assert_eq!(candidate["source_layer"], "override");
    assert_ne!(parent["value"], candidate["value"]);
    assert_eq!(parent.as_object().unwrap().len(), 6);
    assert_eq!(candidate.as_object().unwrap().len(), 6);
    assert_eq!(
        (
            report.results[0].parent_score_micros,
            report.results[0].candidate_score_micros
        ),
        (0, 1_000_000)
    );
    let frozen = f.frozen(&request, DevelopmentSide::Candidate).await;
    assert_eq!(frozen.fd_snapshot["files"].as_array().unwrap().len(), 2);
    assert!(
        frozen.fd_snapshot["files"][0]["utf8"]
            .as_str()
            .unwrap()
            .contains("false")
    );
    assert_eq!(
        config_projection_runner_contract()["candidate_consumption"],
        "projection_consumed"
    );
}

#[tokio::test]
async fn different_loaded_content_can_keep_behavior_and_quality_equal() {
    let _workspace = CONFIG_WORKSPACE_TEST_GATE.lock().await;
    let f = Fixture::new(
        HIGH,
        &format!("{HIGH}\nDescriptive prose; MUST is not an execution policy."),
        false,
    )
    .await;
    let request = f.request("same-output");
    let (report, p, o) = f.execute(&request).await;
    let inputs = f.verify(&p, &o).await.unwrap().projection_inputs.remove(0);
    assert_ne!(
        inputs.parent_request_projection_digest,
        inputs.candidate_request_projection_digest
    );
    let task = &f.control.tasks[0].task_id;
    let parent = load_execution_output(
        &f.store,
        &f.executor,
        &execution_output_id(&request.request_id, task, DevelopmentSide::Parent).unwrap(),
    )
    .await
    .unwrap();
    let candidate = load_execution_output(
        &f.store,
        &f.executor,
        &execution_output_id(&request.request_id, task, DevelopmentSide::Candidate).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(parent.output_utf8, candidate.output_utf8);
    assert_eq!(
        (
            report.results[0].parent_score_micros,
            report.results[0].candidate_score_micros
        ),
        (1_000_000, 1_000_000)
    );
    for text in [
        "config_precedence.v1: random",
        "",
        "config_precedence.v1: highest_layer_first\nconfig_precedence.v1: lowest_layer_first",
    ] {
        let projection = evo_core::contract::RunProjection {
            bundle_digest: d("bundle"),
            instructions: vec![text.into()],
            tools: vec![],
        };
        assert!(
            evo_engine::hosts::run_owned_config_projection(
                evo_core::evaluation::config_precedence_generator::parse_case_selector(&selector(
                    "E"
                ))
                .unwrap(),
                &projection
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn both_explicit_full_bundles_are_required_and_loaded_from_frozen_request() {
    let _workspace = CONFIG_WORKSPACE_TEST_GATE.lock().await;
    let f = Fixture::new(LOW, HIGH, false).await;
    let request = f.request("full-bundles");
    let (_, p, o) = f.execute(&request).await;
    f.verify(&p, &o).await.unwrap();
    let frozen = f.frozen(&request, DevelopmentSide::Candidate).await;
    assert_eq!(frozen.parent.bundle.digest, request.parent_bundle_digest);
    assert_eq!(
        frozen.candidate.bundle.digest,
        request.candidate_bundle_digest
    );
    assert_eq!(frozen.parent.bundle.skill.content, LOW);
    assert_eq!(frozen.candidate.bundle.skill.content, HIGH);
    assert_ne!(
        frozen.candidate.bundle.parent_digest,
        request.parent_bundle_digest
    );
    assert!(!frozen.parent.original_baseline.application_order.is_empty());
    let mut missing = f.request("missing-parent");
    missing.parent_bundle_digest = d("experiment-label-only");
    f.journal(f.facts(&missing, None), &missing).await;
    projection_refusal(f.runner().run(missing).await);
    let parent_id = format!("formal-bundle-{}", f.parent.digest);
    let original = f.raw("artifact", &parent_id).await;
    let mut s = f.store.session().await.unwrap();
    s.delete(&f.admin, "artifact", &parent_id).await.unwrap();
    s.commit().await.unwrap();
    projection_refusal(f.verify(&p, &o).await);
    f.put("artifact", &parent_id, &original).await;
    f.verify(&p, &o).await.unwrap();
}

#[tokio::test]
async fn active_zero_truncated_and_missing_projection_evidence_are_invalid() {
    let _workspace = CONFIG_WORKSPACE_TEST_GATE.lock().await;
    let f = Fixture::new(LOW, HIGH, false).await;
    let request = f.request("bad-projection");
    let (_, p, o) = f.execute(&request).await;
    f.verify(&p, &o).await.unwrap();
    let task = &f.control.tasks[0].task_id;
    let call_id =
        execution_budget_call_id(&request.request_id, task, DevelopmentSide::Candidate).unwrap();
    let original =
        serde_json::to_value(f.frozen(&request, DevelopmentSide::Candidate).await).unwrap();
    let active = Fixture::bundle_in(
        &f.store,
        &f.executor,
        &request.environment_digest,
        &format!("{LOW}\nExisting software production input"),
        "production-profile",
    )
    .await;
    let active_record = f
        .raw("artifact", &format!("formal-bundle-{}", active.digest))
        .await;
    f.put(
        "pointer",
        "profile-pointer-production-profile",
        &json!({
            "id":"profile-pointer-production-profile", "schema_version":"rsia.profile_pointer.v1",
            "profile_id":"production-profile", "active_release_id":"existing-software-release",
            "active_bundle_digest":active.digest, "epoch":1,
        }),
    )
    .await;
    let mut swapped = original.clone();
    swapped["candidate"] = active_record;
    let mut zero = original.clone();
    zero["projection"]["instructions"] = json!([]);
    let mut truncated = original.clone();
    truncated["projection"]["instructions"] = json!(["config_precedence.v1:"]);
    for mutation in [swapped, zero, truncated] {
        f.replace_budget_request(&call_id, &mutation);
        projection_refusal(f.verify(&p, &o).await);
        projection_refusal(verify_development_observation(&f.store, &f.executor, &p, &o).await);
    }
    f.replace_budget_request(&call_id, &original);
    let receipt_id =
        execution_receipt_id(&request.request_id, task, DevelopmentSide::Candidate).unwrap();
    let stored = storage_id(EXECUTION_RECEIPT_KIND, &receipt_id).unwrap();
    let original = f.raw("artifact", &stored).await;
    let mut missing = original.clone();
    missing["payload"]
        .as_object_mut()
        .unwrap()
        .remove("request_projection_digest");
    f.put("artifact", &stored, &missing).await;
    assert!(matches!(f.verify(&p, &o).await, Err(Error::Forbidden)));
    assert!(
        serde_json::from_value::<evo_engine::hosts::VerifiedHostUse>(missing["payload"].clone())
            .is_err()
    );
    f.put("artifact", &stored, &original).await;
    f.verify(&p, &o).await.unwrap();
}

#[tokio::test]
async fn forged_output_or_projection_does_not_verify_even_with_recomputed_outer_digests() {
    let _workspace = CONFIG_WORKSPACE_TEST_GATE.lock().await;
    let f = Fixture::new(LOW, HIGH, false).await;
    let request = f.request("forged");
    let (_, p, o) = f.execute(&request).await;
    f.verify(&p, &o).await.unwrap();
    let task = &f.control.tasks[0].task_id;
    let rid = execution_receipt_id(&request.request_id, task, DevelopmentSide::Candidate).unwrap();
    let oid = execution_output_id(&request.request_id, task, DevelopmentSide::Candidate).unwrap();
    let rstore = storage_id(EXECUTION_RECEIPT_KIND, &rid).unwrap();
    let ostore = storage_id(EXECUTION_OUTPUT_KIND, &oid).unwrap();
    let original_receipt = f.raw("artifact", &rstore).await;
    let original_output = f.raw("artifact", &ostore).await;
    let mut output = original_output.clone();
    let mut answer: Value =
        serde_json::from_str(output["payload"]["output_utf8"].as_str().unwrap()).unwrap();
    answer["value"] = json!("forged-output");
    let body = serde_json::to_string(&answer).unwrap();
    let digest = hash(body.as_bytes());
    output["payload"]["output_utf8"] = json!(body);
    output["payload"]["output_digest"] = json!(digest);
    let mut receipt = original_receipt.clone();
    receipt["payload"]["execution"]["output_digest"] = json!(digest);
    f.put("artifact", &ostore, &output).await;
    f.put("artifact", &rstore, &receipt).await;
    assert!(
        matches!(f.verify(&p,&o).await,Err(Error::Conflict(ref message)) if message=="development_output_mismatch")
    );
    f.put("artifact", &ostore, &original_output).await;
    f.put("artifact", &rstore, &original_receipt).await;
    let mut claimed_projection = original_receipt.clone();
    claimed_projection["payload"]["request_projection_digest"] = json!(d("forged-loaded-content"));
    f.put("artifact", &rstore, &claimed_projection).await;
    projection_refusal(f.verify(&p, &o).await);
    f.put("artifact", &rstore, &original_receipt).await;
    let mut forged = f.frozen(&request, DevelopmentSide::Candidate).await;
    forged.projection.instructions = vec![LOW.into()];
    f.replace_budget_request(
        &execution_budget_call_id(&request.request_id, task, DevelopmentSide::Candidate).unwrap(),
        &serde_json::to_value(forged).unwrap(),
    );
    projection_refusal(f.verify(&p, &o).await);
    assert!(matches!(
        issue_execution_receipt(
            &f.executor,
            &f.store,
            ExecutionReceiptIssueRequest {
                control_id: f.control.id.clone(),
                request: request.clone(),
                task_id: task.clone(),
                side: DevelopmentSide::Candidate,
                output_utf8: serde_json::to_string(&f.control.tasks[0].expected).unwrap(),
                budget_call_id: execution_budget_call_id(
                    &request.request_id,
                    task,
                    DevelopmentSide::Candidate
                )
                .unwrap(),
                issued_at_unix_seconds: 100,
            }
        )
        .await,
        Err(Error::Forbidden)
    ));
}

#[tokio::test]
async fn development_replay_and_recovery_leave_active_and_production_projection_unchanged() {
    let _workspace = CONFIG_WORKSPACE_TEST_GATE.lock().await;
    let f = Fixture::new(LOW, HIGH, false).await;
    // A pre-existing nonempty pointer is software fixture data, never a fresh
    // approval or Host credential. Its associated projection is really executed
    // and frozen first; both original inputs must survive every development phase.
    let pointer_bundle = Fixture::bundle_in(
        &f.store,
        &f.executor,
        &f.control.environment_digest,
        &format!("{HIGH}\nExisting pointer software content"),
        "pointer-fixture-profile",
    )
    .await;
    let mut pointer_input_request = f.request("pointer-input");
    pointer_input_request.parent_bundle_digest = pointer_bundle.digest.clone();
    pointer_input_request.candidate_bundle_digest = pointer_bundle.digest.clone();
    f.execute(&pointer_input_request).await;
    let pointer_input_call = execution_budget_call_id(
        &pointer_input_request.request_id,
        &f.control.tasks[0].task_id,
        DevelopmentSide::Candidate,
    )
    .unwrap();
    let projection = f
        .frozen(&pointer_input_request, DevelopmentSide::Candidate)
        .await;
    let pointer = json!({"id":"profile-pointer-pointer-fixture-profile","schema_version":"rsia.profile_pointer.v1", "profile_id":"pointer-fixture-profile",
        "active_release_id":"existing-pointer-software-release", "active_bundle_digest":pointer_bundle.digest,"epoch":7});
    f.put(
        "pointer",
        "profile-pointer-pointer-fixture-profile",
        &pointer,
    )
    .await;
    let request = f.request("isolation");
    let prepared = f.journal(f.facts(&request, None), &request).await;
    let task = &f.control.tasks[0].task_id;
    for side in [DevelopmentSide::Parent, DevelopmentSide::Candidate] {
        let receipt = issue_config_projection_execution_receipt(
            &f.executor,
            &f.store,
            ConfigProjectionExecutionIssueRequest {
                control_id: f.control.id.clone(),
                request: request.clone(),
                task_id: task.clone(),
                side,
                lease_seconds: 600,
                issued_at_unix_seconds: 100,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            receipt.execution.cost_state,
            evo_engine::development::DevelopmentCostState::Known {
                micros: 0,
                currency: "USD".into(),
                pricing_version: "local-zero-v1".into()
            }
        );
        assert_eq!(
            f.raw("pointer", "profile-pointer-pointer-fixture-profile")
                .await,
            pointer
        );
        assert_eq!(
            serde_json::to_value(
                f.frozen(&pointer_input_request, DevelopmentSide::Candidate)
                    .await
            )
            .unwrap(),
            serde_json::to_value(&projection).unwrap()
        );
    }
    let report = f.runner().run(request.clone()).await.unwrap();
    let observed = f.journal(f.facts(&request, Some(&report)), &request).await;
    f.verify(&prepared, &observed).await.unwrap();
    let before = f.calls(&request).await;
    assert_eq!(before.len(), 2);
    assert!(before.iter().all(|call| call.actual_cost_micros == Some(0)));
    let recovered = f.runner().run(request.clone()).await.unwrap();
    assert_eq!(
        serde_json::to_value(recovered).unwrap(),
        serde_json::to_value(report).unwrap()
    );
    assert_eq!(
        serde_json::to_value(f.calls(&request).await).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    assert_eq!(
        f.raw("pointer", "profile-pointer-pointer-fixture-profile")
            .await,
        pointer
    );
    assert_eq!(
        serde_json::to_value(
            f.frozen(&pointer_input_request, DevelopmentSide::Candidate)
                .await
        )
        .unwrap(),
        serde_json::to_value(projection).unwrap()
    );
    assert_ne!(
        pointer_input_call,
        execution_budget_call_id(&request.request_id, task, DevelopmentSide::Candidate).unwrap()
    );
}

#[tokio::test]
async fn source_revocation_redacts_real_projection_execution_and_cannot_upgrade_legacy_on_recovery()
{
    let _workspace = CONFIG_WORKSPACE_TEST_GATE.lock().await;
    // Legal empty registered closure is real persisted evidence, not a fallback.
    let empty = Fixture::new(LOW, HIGH, false).await;
    let request = empty.request("empty-sources");
    let (_, p, o) = empty.execute(&request).await;
    empty.verify(&p, &o).await.unwrap();
    let call = &empty.calls(&request).await[0];
    let id = format!(
        "budget-ref-{}",
        &fingerprint(&("rsia.budget_call_ref.v1", NS, SCOPE, &call.call_id)).unwrap()[..32]
    );
    let original = empty.raw("artifact", &id).await;
    assert_eq!(original["source_ids"], json!([]));
    let mut forged = original.clone();
    forged["call_id"] = json!("different-call");
    empty.put("artifact", &id, &forged).await;
    assert!(
        matches!(empty.verify(&p,&o).await,Err(Error::Conflict(ref message)) if message=="development source closure mismatch")
    );
    empty.put("artifact", &id, &original).await;
    let mut s = empty.store.session().await.unwrap();
    s.delete(&empty.admin, "artifact", &id).await.unwrap();
    s.commit().await.unwrap();
    assert!(
        matches!(empty.verify(&p,&o).await,Err(Error::Conflict(ref message)) if message=="development source closure mismatch")
    );
    // A replay with an already finalized call cannot manufacture missing proof.
    assert!(
        matches!(empty.runner().run(request).await,Err(Error::Conflict(ref message)) if message=="development source closure mismatch")
    );

    let f = Fixture::new(LOW, HIGH, true).await;
    let request = f.request("revoked");
    let (_, p, o) = f.execute(&request).await;
    f.verify(&p, &o).await.unwrap();
    let before = f.calls(&request).await;
    let mut cleanup = LifecycleCoordinator::revoke_source(
        &f.admin,
        &f.store,
        "source-config",
        "fixture source revoked",
        200,
    )
    .await
    .unwrap();
    for now in 201..500 {
        if matches!(cleanup.state, CleanupState::Complete | CleanupState::Failed) {
            break;
        }
        cleanup =
            LifecycleCoordinator::continue_cleanup(&f.admin, &f.store, &cleanup.job_id, 16, now)
                .await
                .unwrap();
    }
    assert_eq!(cleanup.state, CleanupState::Complete);
    assert!(matches!(
        f.runner().run(request.clone()).await,
        Err(Error::Forbidden)
    ));
    assert!(f.verify(&p, &o).await.is_err());
    let output = storage_id(
        EXECUTION_OUTPUT_KIND,
        &execution_output_id(
            &request.request_id,
            &f.control.tasks[0].task_id,
            DevelopmentSide::Candidate,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        f.raw("artifact", &output).await["schema_version"],
        "rsia.redacted.v1"
    );
    let after = f.calls(&request).await;
    assert_eq!(after.len(), before.len());
    for (a, b) in after.iter().zip(before.iter()) {
        assert_eq!(a.state, b.state);
        assert_eq!(a.actual_cost_micros, b.actual_cost_micros);
        assert_eq!(a.state, BudgetCallState::Finalized);
    }
}

#[tokio::test]
async fn legacy_clamp_contracts_and_receipt_wire_keep_identity_only_semantics() {
    use evo_engine::curriculum_profiles::RegisteredPureFunctionProfileV1;
    use evo_engine::development::{
        DevelopmentControlV1, DevelopmentEvidenceScope, DevelopmentTaskSpecV1,
        RegisteredDevelopmentRunner, RegisteredTargetInputV1, load_execution_receipt,
        register_development_control,
    };
    use evo_engine::streaming_evaluator::{FixedGraderMethod, FixedGraderSpec};
    const V1: &str = r#"{"isolation":"none","model_execution":false,"provider_dispatch":false,"runtime":"in_process_registered_pure_function","schema_version":"rsia.registered_development_runner.v1","targets":["reference_host.clamp_i64.v1"]}"#;
    assert_eq!(
        hash(V1.as_bytes()),
        "e1351fe9605e5ebe374bb83e6f1dbe5489021c0e81b2fa83d967010bdd3b5c21"
    );
    assert_eq!(
        registered_runner_digest().unwrap(),
        "b9d2428678d492ca3d4fc42352e0492c0d318e7b418b7bc77a98191fe04e3a09"
    );
    let f = Fixture::new(LOW, HIGH, true).await;
    let profile = RegisteredPureFunctionProfileV1::clamp_i64();
    let grader = FixedGraderSpec {
        schema_version: FixedGraderSpec::SCHEMA.into(),
        version: "exact-json-v1".into(),
        method: FixedGraderMethod::ExactJsonAnswerV1,
    };
    let mut control = DevelopmentControlV1 {
        schema_version: DevelopmentControlV1::SCHEMA.into(),
        id: "clamp-control".into(),
        namespace: NS.into(),
        billing_scope: SCOPE.into(),
        root_budget_id: "config-root".into(),
        executor_actor: "executor".into(),
        grader_actor: "grader".into(),
        proposer_actor: "optimizer".into(),
        manifest_id: "clamp-manifest".into(),
        manifest_digest: String::new(),
        tasks: vec![
            DevelopmentTaskSpecV1::registered(
                "clamp-task",
                "family",
                RegisteredTargetInputV1::ClampI64 {
                    value: 5,
                    min: 0,
                    max: 3,
                },
            )
            .unwrap(),
        ],
        environment_digest: f.control.environment_digest.clone(),
        grader_digest: fingerprint(&grader).unwrap(),
        fixed_grader: grader,
        oracle_digest: profile.oracle_digest,
        target_digest: profile.target_digest,
        runner_digest: registered_runner_digest().unwrap(),
        rules_digest: d("rules"),
        tools_digest: d("tools"),
        source_ids: vec!["source-config".into()],
        evidence_scope: DevelopmentEvidenceScope::RegisteredPureFunctionExecution,
        created_at_unix_seconds: 1,
    };
    control.manifest_digest = control.manifest().unwrap().digest;
    register_development_control(&f.admin, &f.store, control.clone())
        .await
        .unwrap();
    let mut request = f.request("clamp");
    request.manifest = control.manifest().unwrap();
    request.grader_digest = control.grader_digest.clone();
    let runner = RegisteredDevelopmentRunner::with_clock(
        f.store.clone(),
        f.executor.clone(),
        f.grader.clone(),
        control.id.clone(),
        600,
        Arc::new(|| 100),
    )
    .unwrap();
    let report = runner.run(request.clone()).await.unwrap();
    let p = f.journal(f.facts(&request, None), &request).await;
    let o = f.journal(f.facts(&request, Some(&report)), &request).await;
    let view = f.verify(&p, &o).await.unwrap();
    assert_eq!(
        view.observation.candidate_consumption,
        Some(CandidateConsumption::IdentityOnly)
    );
    assert!(view.projection_inputs.is_empty());
    let receipt = load_execution_receipt(
        &f.store,
        &f.executor,
        &report.results[0].candidate_execution_id,
    )
    .await
    .unwrap();
    let bytes = serde_json::to_vec(&receipt).unwrap();
    let value = serde_json::to_value(&receipt).unwrap();
    assert_eq!(
        value["schema_version"],
        "rsia.development_execution_receipt.v1"
    );
    assert!(value.get("request_projection_digest").is_none());
    let restored: evo_engine::development::DevelopmentExecutionReceiptV1 =
        serde_json::from_slice(&bytes).unwrap();
    assert_eq!(serde_json::to_vec(&restored).unwrap(), bytes);
    assert!(
        load_config_projection_execution_receipt(&f.store, &f.executor, &receipt.receipt_id)
            .await
            .is_err()
    );
    let mut legacy = control;
    legacy.id = "legacy-clamp".into();
    legacy.runner_digest = hash(V1.as_bytes());
    assert!(
        matches!(register_development_control(&f.admin,&f.store,legacy).await,Err(Error::Invalid(ref message)) if message=="legacy development runner contract cannot register a new control")
    );
}
