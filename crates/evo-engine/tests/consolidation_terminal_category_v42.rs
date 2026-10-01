//! E13 (plan v4.2, AG-031): the terminal category of a consolidation run is
//! bounded, so a long rejection reason can no longer strand a claim in `Running`.
//!
//! `run_consolidation` used to refuse to write its terminal record when the reason
//! of the step's outcome was longer than 128 bytes (the serde error for a model
//! answer that is valid JSON with an unknown field is about 150 bytes): it returned
//! `Invalid`, the claim stayed `Running` and a retry failed the same way. The
//! category a terminal record stores is now derived from the reason by
//! `terminal_category`, a pure function with a bound of 128 bytes.
//!
//! * The derivation itself: a reason that fits is kept as it is, an empty one is
//!   `unspecified`, a longer one is cut on a character boundary and ends in `…#`
//!   and 16 hex digits of the sha256 of the whole reason.
//! * A run whose model answer makes a long rejection reason now ends
//!   `CompletedRejected` with a bounded record, is idempotent on retry, never
//!   re-dispatches and leaves the ledger as it was; a claim the old rule left
//!   `Running` finishes on its next call without a dispatch; the `Uncertain` path
//!   is bounded too.
//!
//! The fixtures are copied from `tests/consolidation_budget_stage_v42.rs` and
//! `tests/monitoring_consolidation_v42.rs` because test crates cannot import one
//! another; the provider is a scripted transport behind the real
//! `PersistentModelBroker`, so the model calls reach the real ledger.

use async_trait::async_trait;
use evo_core::contract::{
    CapabilityLevel, HostCapabilities, HostSurfaceManifest, ImproverPatch, Profile, SkillSnapshot,
    SurfaceCoverage, SurfaceItem, SystemSnapshot,
};
use evo_core::evidence::{EvidenceSet, ExecutionAttestation, Purpose, SourceSelection, TaskOrigin};
use evo_core::optimization::{
    EditSuggestion, ModelRequest, ModelRequestContext, ModelStage, OptimizationTrace,
    SkillFailureDiagnosis, SkillFailureKind, TraceOutcome, TrustedSourceBinding,
};
use evo_core::skill_edit::{
    EvidenceClosure, EvidenceRef, SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_SCHEMA, SkillEditBatch,
    SkillTextEdit, SkillTextField, TextEditOperation, TrustedEditContext, skill_snapshot_digest,
};
use evo_core::{Context, Error, Result, Role, Strategy, fingerprint, hash};
use evo_engine::broker::{
    BrokerConfig, BudgetPortBinding, ModelTransport, PersistentModelBroker, TransportCompletion,
};
use evo_engine::evidence::{
    StoredRunRecord, StoredTraceAuthority, ingest_trusted_run_records, store_source_selection,
    store_trace_authority,
};
use evo_engine::model::ModelExecutionProvenance;
use evo_engine::monitoring::{
    ClaimOutcome, CompleteDevelopmentCycleRequest, ConsolidationClaim, ConsolidationClaimState,
    ConsolidationDevRunner, ConsolidationRunOutcome, ConsolidationRunRecord,
    DevelopmentCycleRecord, EnvironmentEvidenceScope, MonitoringCoordinator,
    RecordEnvironmentRequest, RecordedEnvironment, terminal_category,
};
use evo_engine::optimization::{
    BundleCompileContext, DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest,
    DevelopmentRunReport, DevelopmentRunRequest, DevelopmentTask, OPTIMIZATION_STAGE_FACT_SCHEMA,
    OptimizationJournal, OptimizationJournalStage, OptimizationStepRequest, PairedTaskResult,
    StageDependency, StageFact, StageFactKind, StoreOptimizationJournal,
};
use evo_engine::release_store::{PrepareRunRequest, ReleaseStore, TrustedHostExecutionEvidence};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetCallRecord, BudgetCallState, BudgetStage, RootBudgetAuthorization, RootBudgetRecord,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const TENANT: &str = "tenant";
const SCOPE: &str = "billing-scope";
const ROOT: &str = "root-budget";
/// What a fixture model call costs, and what the broker reserves for it.
const COST_MICROS: i64 = 10;
const RESERVED_MICROS: i64 = 20;

fn d(value: &str) -> String {
    hash(value.as_bytes())
}

fn context(actor: &str, role: Role) -> Context {
    Context::new(TENANT, actor, role).unwrap()
}

// ---------------------------------------------------------------------------
// The derivation: a pure, bounded function of the raw reason.
// ---------------------------------------------------------------------------

/// The most bytes a terminal category may hold.
const LIMIT: usize = 128;
/// What follows the cut-off prefix of a truncated category, before the digest.
const MARK: &str = "…#";
/// Hex digits of the sha256 of the whole reason that a truncated category keeps.
const DIGEST_HEX: usize = 16;
/// What the cut-off prefix may hold at most: the limit less the mark and the digest.
const PREFIX_BUDGET: usize = LIMIT - MARK.len() - DIGEST_HEX;

/// The first 16 hex digits of the sha256 of the whole of `reason`.
fn digest16(reason: &str) -> String {
    hash(reason.as_bytes())[..DIGEST_HEX].to_string()
}

/// Checks everything a truncated category is and returns its cut-off prefix: at
/// most 128 bytes in all, a prefix of the reason, then `…#` and the digest of the
/// whole reason.
fn cut_prefix<'a>(reason: &str, category: &'a str) -> &'a str {
    assert!(
        category.len() <= LIMIT,
        "{} bytes: {category}",
        category.len()
    );
    let (prefix, digest) = category
        .rsplit_once(MARK)
        .unwrap_or_else(|| panic!("a truncated category ends in {MARK}<digest>: {category}"));
    assert_eq!(digest, digest16(reason), "{category}");
    assert!(
        digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{category}"
    );
    assert!(
        reason.starts_with(prefix),
        "the prefix is cut from the reason: {category}"
    );
    prefix
}

#[test]
fn a_reason_that_fits_is_kept_as_it_is() {
    // 42 three-byte characters and two more bytes: 128 bytes of text.
    let cjk = format!("{}ab", "中".repeat(42));
    assert_eq!(cjk.len(), LIMIT);
    let fitting = [
        "x".to_string(),
        // The fixed categories the run already wrote.
        "source_revoked_after_optimization".to_string(),
        "root budget is stopped or has no dispatchable balance".to_string(),
        "r".repeat(LIMIT - 1),
        "r".repeat(LIMIT),
        cjk,
    ];
    for reason in fitting {
        assert_eq!(terminal_category(&reason), reason);
    }
}

#[test]
fn a_reason_over_the_limit_is_cut_to_128_bytes_and_carries_the_digest_of_the_whole() {
    // The serde error of a model answer that is valid JSON with an unknown field.
    let serde_error = "invalid input: invalid optimizer suggestions: unknown field `bogus`, \
        expected one of `id`, `hypothesis`, `batch_ids`, `support`, `counterexamples`, \
        `dependencies`, `edit` at line 1 column 10"
        .to_string();
    assert!(serde_error.len() > LIMIT);
    for reason in ["r".repeat(LIMIT + 1), serde_error, "r".repeat(10 * 1024)] {
        let category = terminal_category(&reason);
        // ASCII text uses the whole of the prefix budget, so the category is exactly
        // the limit.
        assert_eq!(category.len(), LIMIT, "{category}");
        assert_eq!(cut_prefix(&reason, &category).len(), PREFIX_BUDGET);
    }
}

#[test]
fn a_multibyte_reason_is_cut_on_a_character_boundary_without_panicking() {
    let reasons = [
        // Three-byte characters, where the budget ends exactly on a boundary.
        "中".repeat(400),
        // The 108th byte falls inside a character.
        format!("a{}", "中".repeat(200)),
        // Four-byte characters, on and off the boundary.
        "😀".repeat(100),
        format!("ab{}", "😀".repeat(100)),
        // Two-byte characters.
        format!("a{}", "é".repeat(100)),
        // The mark itself inside the reason.
        MARK.repeat(100),
    ];
    for reason in reasons {
        let category = terminal_category(&reason);
        let prefix = cut_prefix(&reason, &category);
        // At most one character is given up to reach a boundary.
        assert!(
            prefix.len() <= PREFIX_BUDGET && prefix.len() + 4 > PREFIX_BUDGET,
            "{} bytes: {category}",
            prefix.len()
        );
    }
}

#[test]
fn every_length_is_bounded_and_only_a_reason_over_the_limit_is_cut() {
    for len in 0..=1024 {
        let reason = "q".repeat(len);
        let category = terminal_category(&reason);
        match len {
            0 => assert_eq!(category, "unspecified"),
            1..=LIMIT => assert_eq!(category, reason),
            _ => {
                cut_prefix(&reason, &category);
            }
        }
        assert!(!category.is_empty() && category.len() <= LIMIT, "{len}");
    }
    // The same for text whose characters are three bytes wide.
    for chars in 1..=400 {
        let reason = "中".repeat(chars);
        let category = terminal_category(&reason);
        assert!(!category.is_empty() && category.len() <= LIMIT, "{chars}");
        if reason.len() > LIMIT {
            cut_prefix(&reason, &category);
        } else {
            assert_eq!(category, reason);
        }
    }
}

#[test]
fn the_category_is_deterministic_and_tells_long_reasons_apart() {
    let long = "a".repeat(10 * 1024);
    // The same first 10 KiB less one byte: only the digest tells these two apart.
    let late = format!("{}b", &long[..long.len() - 1]);
    // Differing from the very first byte.
    let early = format!("b{}", &long[1..]);
    assert_eq!(terminal_category(&long), terminal_category(&long));
    assert_eq!(terminal_category(&late), terminal_category(&late));
    let categories: BTreeSet<_> = [&long, &late, &early]
        .into_iter()
        .map(|reason| terminal_category(reason))
        .collect();
    assert_eq!(categories.len(), 3, "{categories:?}");
    // A category is its own category, so deriving it again changes nothing.
    for reason in [long, late, early, String::new(), "short".to_string()] {
        let category = terminal_category(&reason);
        assert_eq!(terminal_category(&category), category);
    }
}

#[test]
fn an_empty_reason_is_the_fixed_literal() {
    assert_eq!(terminal_category(""), "unspecified");
}

// ---------------------------------------------------------------------------
// Fixtures copied from tests/consolidation_budget_stage_v42.rs: the broker's
// helpers, then the environment, the cycles, the claim and the step material.
// ---------------------------------------------------------------------------

fn completion(request: &ModelRequest, output: String) -> TransportCompletion {
    TransportCompletion {
        response_id: format!("response-{}", request.request_id),
        provider_request_id: format!("provider-{}", request.request_id),
        usage_record_id: format!("usage-{}", request.request_id),
        actual_model_digest: request.model_digest.clone(),
        output,
        actual_cost_micros: COST_MICROS,
        currency: "USD".into(),
        pricing_version: "pricing-v1".into(),
    }
}

fn broker_config() -> BrokerConfig {
    BrokerConfig {
        billing_scope: SCOPE.into(),
        actor: "trusted-broker".into(),
        currency: "USD".into(),
        pricing_version: "pricing-v1".into(),
        max_cost_micros: RESERVED_MICROS,
        lease_seconds: 60,
    }
}

async fn authorize_root(store: &Store, admin: &Context) -> RootBudgetRecord {
    store
        .authorize_root_budget(
            admin,
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
        .unwrap()
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

fn system(model: &str) -> SystemSnapshot {
    SystemSnapshot {
        schema_version: "rsia.system_snapshot.v2".into(),
        profile_id: "profile".into(),
        host_id: "monitor-host".into(),
        host_version: "1.0.0".into(),
        model_id: model.into(),
        tools: vec!["tool-a".into()],
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

struct Fixture {
    _dir: tempfile::TempDir,
    store: Store,
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
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("consolidation-stage.sqlite3"))
            .await
            .unwrap();
        let admin = context("admin", Role::Admin);
        let host = context("host", Role::Host);
        let worker = context("worker", Role::Worker);
        ReleaseStore::register_host_surface(
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
        authorize_root(&store, &admin).await;
        Self {
            _dir: dir,
            store,
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
            billing_scope: SCOPE.into(),
            root_budget_id: ROOT.into(),
            report_fact_id: fact_id,
            source_ids: vec!["source-2".into(), "source-1".into()],
            revoke_watermark: 1,
        }
    }

    /// Stages a development report for `cycle` and closes it as a cycle.
    async fn close_cycle(&self, cycle: u32, contrast: bool) -> Result<DevelopmentCycleRecord> {
        let fact = self.stage_report(cycle, contrast).await;
        MonitoringCoordinator::close_development_cycle(
            &self.worker,
            &self.store,
            self.cycle_request(fact),
        )
        .await
    }

    /// Closes two cycles, with a before/after contrast or without one, and claims
    /// the first generation.
    async fn claimed(&self, contrast: bool) -> ConsolidationClaim {
        for cycle in 1..=2 {
            self.close_cycle(cycle, contrast).await.unwrap();
        }
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

    /// Stages a DevelopmentObserved fact whose report pairs one task for `cycle`
    /// (both pass; the score goes 500k -> 600k with a contrast, and stays at 500k
    /// without one), with the fixture receipts the stage fact depends on.
    async fn stage_report(&self, cycle: u32, contrast: bool) -> String {
        let runner_execution = format!("runner-execution-{cycle}");
        let parent_execution = format!("parent-execution-{cycle}-0");
        let candidate_execution = format!("candidate-execution-{cycle}-0");
        let grader_receipt = d(&format!("grader-receipt-{cycle}-0"));
        let receipts = [
            ("execution", runner_execution.clone()),
            ("execution", parent_execution.clone()),
            ("execution", candidate_execution.clone()),
            ("grader", grader_receipt.clone()),
        ];
        let mut dependencies = vec![
            StageDependency {
                kind: "execution".into(),
                id: parent_execution.clone(),
            },
            StageDependency {
                kind: "execution".into(),
                id: candidate_execution.clone(),
            },
            StageDependency {
                kind: "grader".into(),
                id: grader_receipt.clone(),
            },
        ];
        let report = DevelopmentRunReport {
            request_id: format!("development-request-{cycle}"),
            manifest_digest: self.manifest_digest.clone(),
            parent_bundle_digest: self.parent_bundle_digest.clone(),
            candidate_bundle_digest: d(&format!("candidate-bundle-{cycle}")),
            environment_digest: self.environment.environment.environment_digest.clone(),
            grader_digest: self.grader_digest.clone(),
            results: vec![PairedTaskResult {
                task_id: "task-a".into(),
                parent_score_micros: 500_000,
                candidate_score_micros: if contrast { 600_000 } else { 500_000 },
                parent_passed: true,
                candidate_passed: true,
                parent_execution_id: parent_execution,
                candidate_execution_id: candidate_execution,
                grader_receipt_digest: grader_receipt,
            }],
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
            schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
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
        StoreOptimizationJournal::new(self.store.clone(), self.worker.clone(), "worker").unwrap()
    }
}

async fn prepare_host_run(host: &Context, store: &Store, run_id: &str, model: &str, task: &str) {
    let snapshot = ReleaseStore::prepare_run(
        host,
        store,
        PrepareRunRequest {
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
            capability_level: CapabilityLevel::Attached,
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

    /// The optimization request of `episode` (a claim id for a consolidation).
    fn request<'a>(&'a self, fixture: &'a Fixture, episode: &str) -> OptimizationStepRequest<'a> {
        OptimizationStepRequest {
            evidence: &self.evidence,
            source_selection: &self.selection,
            source_bindings: &self.bindings,
            traces: fixture
                .authorities
                .iter()
                .map(|authority| authority.trace.clone())
                .collect(),
            model_context: ModelRequestContext {
                request_id: "consolidation-request-1".into(),
                namespace: TENANT.into(),
                purpose: Purpose::Development,
                stage: ModelStage::Consolidate,
                episode_id: episode.into(),
                step: 1,
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
                request_id: "consolidation-development-1".into(),
                namespace: TENANT.into(),
                purpose: Purpose::Development,
                episode_id: episode.into(),
                step: 1,
                attempt: 1,
                manifest: fixture.manifest.clone(),
                parent_bundle_digest: fixture.parent_bundle_digest.clone(),
                candidate_bundle_digest: d("candidate-1"),
                environment_digest: fixture.environment.environment.environment_digest.clone(),
                grader_digest: fixture.grader_digest.clone(),
                rules_digest: d("rules"),
                tools_digest: d("tools"),
                revoke_watermark: 1,
                idempotency_key: "consolidation-idempotency-1".into(),
            },
            allow_rank_call: false,
        }
    }
}

// ---------------------------------------------------------------------------
// The scripted provider and development runner behind the real broker and ledger.
// ---------------------------------------------------------------------------

/// What the provider answers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// One bounded edit suggestion per reflection batch.
    Suggestions,
    /// Valid JSON whose item carries a field a suggestion does not have. The serde
    /// error that names it is about 150 bytes, more than a terminal category held.
    UnknownField,
}

struct ScriptedTransport {
    answer: Answer,
    calls: Arc<AtomicUsize>,
}

impl ScriptedTransport {
    /// One bounded insertion per reflection batch, read from the labelled inputs of
    /// the request (as `tests/monitoring_consolidation_v42.rs` does).
    fn suggestions(request: &ModelRequest) -> Result<String> {
        let input = |label: &str| -> Result<&str> {
            request
                .input
                .iter()
                .find(|part| part.label == label)
                .map(|part| part.content.as_str())
                .ok_or_else(|| Error::Invalid(format!("fixture input {label} missing")))
        };
        let parent: SkillSnapshot = serde_json::from_str(input("parent-skill")?)
            .map_err(|_| Error::Invalid("fixture parent input invalid".into()))?;
        let strategy: Strategy = serde_json::from_str(input("generation-strategy")?)
            .map_err(|_| Error::Invalid("fixture strategy input invalid".into()))?;
        let success = request
            .input
            .iter()
            .any(|part| part.label == "reflection-success");
        let (id, batch_id, field, start) = if success {
            (
                "preserve-rule",
                "reflection-success",
                SkillTextField::Applicability,
                parent.applicability.len(),
            )
        } else {
            (
                "fix-rule",
                "reflection-failure",
                SkillTextField::Content,
                parent.content.len(),
            )
        };
        let source = request.source_closure[0].clone();
        let suggestion = EditSuggestion {
            id: id.into(),
            hypothesis: "bounded fixture hypothesis".into(),
            batch_ids: vec![batch_id.into()],
            support: vec![source.clone()],
            counterexamples: if success {
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
                    text: format!(" [{} terminal-category]", strategy.instruction),
                },
            },
        };
        serde_json::to_string(&vec![suggestion]).map_err(|_| Error::Internal)
    }
}

#[async_trait]
impl ModelTransport for ScriptedTransport {
    fn provenance(&self) -> Option<ModelExecutionProvenance> {
        Some(ModelExecutionProvenance::Fixture)
    }

    async fn execute(
        &self,
        request: &ModelRequest,
        _dispatch_id: &str,
    ) -> Result<TransportCompletion> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let output = match self.answer {
            Answer::Suggestions => Self::suggestions(request)?,
            Answer::UnknownField => r#"[{"bogus":1}]"#.to_string(),
        };
        Ok(completion(request, output))
    }
}

/// A development runner that always fails with a fixed message and counts its calls.
struct FailingDevRunner {
    message: String,
    calls: AtomicUsize,
}

#[async_trait]
impl DevRunner for FailingDevRunner {
    async fn run(&self, _request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(Error::Conflict(self.message.clone()))
    }
}

#[async_trait]
impl ConsolidationDevRunner for FailingDevRunner {
    async fn trusted_budget_binding(&self, _namespace: &str) -> Result<BudgetPortBinding> {
        Ok(BudgetPortBinding {
            billing_scope: SCOPE.into(),
            root_budget_id: ROOT.into(),
        })
    }
}

/// Whether any string in `value` contains `text`.
fn mentions(value: &Value, text: &str) -> bool {
    match value {
        Value::String(string) => string.contains(text),
        Value::Array(items) => items.iter().any(|item| mentions(item, text)),
        Value::Object(fields) => fields.values().any(|item| mentions(item, text)),
        _ => false,
    }
}

/// A fixture, its step material and a real broker over the scripted transport.
struct World {
    fixture: Fixture,
    material: StepMaterial,
    broker: PersistentModelBroker<ScriptedTransport>,
    runner: FailingDevRunner,
    /// Provider calls the transport has executed.
    provider_calls: Arc<AtomicUsize>,
}

impl World {
    /// `runner_message` is what the development runner fails with, if it is reached.
    async fn new(answer: Answer, runner_message: &str) -> Self {
        let fixture = Fixture::new().await;
        let material = StepMaterial::new(&fixture).await;
        let provider_calls = Arc::new(AtomicUsize::new(0));
        let broker = PersistentModelBroker::with_clock(
            fixture.store.clone(),
            ScriptedTransport {
                answer,
                calls: provider_calls.clone(),
            },
            broker_config(),
            Arc::new(|| 10),
        )
        .unwrap();
        Self {
            fixture,
            material,
            broker,
            runner: FailingDevRunner {
                message: runner_message.into(),
                calls: AtomicUsize::new(0),
            },
            provider_calls,
        }
    }

    /// A world with a first-generation claim (with a contrast) to run.
    async fn claimed(answer: Answer, runner_message: &str) -> (Self, ConsolidationClaim) {
        let world = Self::new(answer, runner_message).await;
        let claim = world.fixture.claimed(true).await;
        (world, claim)
    }

    async fn consolidate(&self, claim: &ConsolidationClaim) -> Result<ConsolidationRunRecord> {
        MonitoringCoordinator::run_consolidation(
            &self.fixture.worker,
            &self.fixture.store,
            &claim.id,
            Some(&self.broker),
            Some(&self.runner),
            Some(&self.fixture.journal()),
            self.material.request(&self.fixture, &claim.id),
        )
        .await
    }

    fn provider_calls(&self) -> usize {
        self.provider_calls.load(Ordering::SeqCst)
    }

    fn runner_calls(&self) -> usize {
        self.runner.calls.load(Ordering::SeqCst)
    }

    /// Every ledger row of the claim's dispatch group, oldest first.
    async fn ledger(&self, claim: &ConsolidationClaim) -> Vec<BudgetCallRecord> {
        let mut session = self.fixture.store.session().await.unwrap();
        let mut calls = session
            .budget_calls_for_group(&self.fixture.worker, SCOPE, &claim.id)
            .await
            .unwrap();
        session.commit().await.unwrap();
        calls.sort_by_key(|call| (call.created_at, call.call_id.clone()));
        calls
    }

    async fn root(&self) -> RootBudgetRecord {
        self.fixture
            .store
            .root_budget(&self.fixture.worker, SCOPE)
            .await
            .unwrap()
            .unwrap()
    }

    async fn stored_claim(&self, claim: &ConsolidationClaim) -> ConsolidationClaim {
        let mut session = self.fixture.store.session().await.unwrap();
        let stored = session
            .need(&self.fixture.worker, "artifact", &claim.id)
            .await
            .unwrap();
        session.commit().await.unwrap();
        stored
    }

    /// The schema of every stored artifact that holds `text` in one of its strings.
    async fn artifact_schemas_holding(&self, text: &str) -> BTreeSet<String> {
        let mut session = self.fixture.store.session().await.unwrap();
        let all: Vec<Value> = session
            .list(&self.fixture.worker, "artifact")
            .await
            .unwrap();
        session.commit().await.unwrap();
        all.iter()
            .filter(|artifact| mentions(artifact, text))
            .map(|artifact| {
                artifact["schema_version"]
                    .as_str()
                    .unwrap_or("")
                    .to_string()
            })
            .collect()
    }

    fn run_record_id(claim: &ConsolidationClaim) -> String {
        format!("consolidation-run-{}", claim.id)
    }

    async fn has_run_record(&self, claim: &ConsolidationClaim) -> bool {
        let mut session = self.fixture.store.session().await.unwrap();
        let found = session
            .get::<Value>(
                &self.fixture.worker,
                "artifact",
                &Self::run_record_id(claim),
            )
            .await
            .unwrap()
            .is_some();
        session.commit().await.unwrap();
        found
    }

    /// The whole reason of the step's outcome, read from the optimization journal's
    /// terminal `StepCompleted` fact: the full text lives there, not in the run record.
    async fn step_reason(&self, claim: &ConsolidationClaim) -> String {
        let mut session = self.fixture.store.session().await.unwrap();
        let all: Vec<Value> = session
            .list(&self.fixture.worker, "artifact")
            .await
            .unwrap();
        session.commit().await.unwrap();
        let found: Vec<_> = all
            .iter()
            .filter(|fact| {
                fact["schema_version"] == OPTIMIZATION_STAGE_FACT_SCHEMA
                    && fact["episode_id"] == json!(claim.id)
                    && fact["kind"] == "step_completed"
            })
            .collect();
        assert_eq!(found.len(), 1, "one terminal step fact under the claim");
        found[0]["payload"]["reason"]
            .as_str()
            .expect("a rejected or uncertain step keeps its reason")
            .to_string()
    }

    /// The state the old rule left behind once the step had ended: the claim is
    /// `Running` and no terminal run record was written (the step's outcome and the
    /// billed calls stay where they are).
    async fn strand(&self, claim: &ConsolidationClaim) {
        let worker = &self.fixture.worker;
        let mut session = self.fixture.store.session().await.unwrap();
        let mut stored: ConsolidationClaim =
            session.need(worker, "artifact", &claim.id).await.unwrap();
        stored.state = ConsolidationClaimState::Running;
        session
            .put(worker, "artifact", &claim.id, worker.actor(), &stored)
            .await
            .unwrap();
        session
            .delete(worker, "artifact", &Self::run_record_id(claim))
            .await
            .unwrap();
        session.commit().await.unwrap();
    }
}

// ---------------------------------------------------------------------------
// A long reason no longer strands a claim.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_long_rejection_reason_ends_the_claim_with_a_bounded_category() {
    let (world, claim) = World::claimed(Answer::UnknownField, "never reached").await;
    // This call used to return `Invalid("consolidation terminal category must be
    // 1..=128 bytes")` and leave the claim in `Running`.
    let record = world.consolidate(&claim).await.unwrap();
    assert_eq!(
        record.outcome,
        ConsolidationRunOutcome::Rejected,
        "{record:?}"
    );

    // The reason of the step is the fixed code of its class (AG-048): the serde error
    // that named the field, about 150 bytes and more than a category held, is no longer
    // part of the outcome, so the reason is bounded by construction and the record's
    // category is that code.
    let full = world.step_reason(&claim).await;
    assert_eq!(full, "suggestion_shape_invalid");
    assert!(full.len() <= LIMIT, "{} bytes: {full}", full.len());
    assert_eq!(record.reason, terminal_category(&full));
    assert_eq!(record.reason, full);
    // The claim is terminal, not stranded.
    assert_eq!(
        world.stored_claim(&claim).await.state,
        ConsolidationClaimState::CompletedRejected
    );
    assert!(world.has_run_record(&claim).await);

    // The one model call was dispatched and billed, as a consolidation call.
    let ledger = world.ledger(&claim).await;
    assert_eq!(ledger.len(), 1);
    assert_eq!(ledger[0].stage, BudgetStage::Consolidation);
    assert_eq!(ledger[0].state, BudgetCallState::Finalized);
    assert_eq!(ledger[0].actual_cost_micros, Some(COST_MICROS));
    assert_eq!(record.budget_call_ids, vec![ledger[0].call_id.clone()]);
    let root = world.root().await;
    assert_eq!(root.spent_micros, COST_MICROS);
    assert_eq!(root.reserved_micros, 0);
    assert_eq!(world.runner_calls(), 0);

    // A retry returns the same record: nothing is dispatched again and nothing moves.
    let again = world.consolidate(&claim).await.unwrap();
    assert_eq!(again, record);
    assert_eq!(world.provider_calls(), 1);
    assert_eq!(world.runner_calls(), 0);
    assert_eq!(world.ledger(&claim).await, ledger);
    assert_eq!(world.root().await, root);
    // What is stored of the reason is the code, in the step's terminal fact and in the
    // run record, and what is stored of the parser's error is nothing: no artifact holds
    // the text that named the field.
    assert_eq!(
        world.artifact_schemas_holding(&full).await,
        BTreeSet::from([
            OPTIMIZATION_STAGE_FACT_SCHEMA.to_string(),
            "rsia.monitoring.consolidation_run.v1".to_string()
        ])
    );
    assert!(
        world
            .artifact_schemas_holding("invalid optimizer suggestions")
            .await
            .is_empty()
    );
    assert!(
        world
            .artifact_schemas_holding("unknown field `bogus`")
            .await
            .is_empty()
    );
    assert_eq!(world.step_reason(&claim).await, full);
}

#[tokio::test]
async fn a_claim_the_old_rule_left_running_finishes_on_retry_without_a_dispatch() {
    let (world, claim) = World::claimed(Answer::UnknownField, "never reached").await;
    let first = world.consolidate(&claim).await.unwrap();
    assert_eq!(first.outcome, ConsolidationRunOutcome::Rejected);

    // What the old rule left behind: the step had ended (its outcome is in the
    // journal, its call is billed) but the terminal record was refused.
    world.strand(&claim).await;
    assert_eq!(
        world.stored_claim(&claim).await.state,
        ConsolidationClaimState::Running
    );
    assert!(!world.has_run_record(&claim).await);
    let ledger = world.ledger(&claim).await;
    let root = world.root().await;

    // The next call reads the stored outcome, writes the same terminal record and
    // finishes the claim, with no dispatch.
    let second = world.consolidate(&claim).await.unwrap();
    assert_eq!(second, first);
    assert_eq!(
        world.stored_claim(&claim).await.state,
        ConsolidationClaimState::CompletedRejected
    );
    assert!(world.has_run_record(&claim).await);
    assert_eq!(world.provider_calls(), 1);
    assert_eq!(world.runner_calls(), 0);
    assert_eq!(world.ledger(&claim).await, ledger);
    assert_eq!(world.root().await, root);
}

#[tokio::test]
async fn a_long_uncertain_reason_is_bounded_too() {
    // The development runner fails with a long message after both reflection calls
    // were billed: the step ends `Uncertain`, with a reason over 128 bytes.
    let message = "d".repeat(300);
    let (world, claim) = World::claimed(Answer::Suggestions, &message).await;
    let record = world.consolidate(&claim).await.unwrap();
    assert_eq!(
        record.outcome,
        ConsolidationRunOutcome::Uncertain,
        "{record:?}"
    );

    // The runner's 300-byte message is not part of the outcome (AG-048): the reason is
    // the fixed code of the class, so it is bounded by construction.
    let full = world.step_reason(&claim).await;
    assert_eq!(full, "development_execution_outcome_unknown");
    assert!(full.len() <= LIMIT, "{} bytes", full.len());
    assert_eq!(record.reason, terminal_category(&full));
    assert_eq!(record.reason, full);
    assert!(
        world.artifact_schemas_holding(&message).await.is_empty(),
        "the runner's message is stored nowhere"
    );
    assert_eq!(
        world.stored_claim(&claim).await.state,
        ConsolidationClaimState::CompletedUncertain
    );

    // Two reflection calls, billed as consolidation; the runner was reached once.
    let ledger = world.ledger(&claim).await;
    assert_eq!(ledger.len(), 2);
    assert!(
        ledger
            .iter()
            .all(|call| call.stage == BudgetStage::Consolidation
                && call.state == BudgetCallState::Finalized)
    );
    assert_eq!(world.provider_calls(), 2);
    assert_eq!(world.runner_calls(), 1);
    let root = world.root().await;
    assert_eq!(root.spent_micros, 2 * COST_MICROS);
    assert_eq!(root.reserved_micros, 0);

    // A retry neither calls the provider nor the runner again.
    let again = world.consolidate(&claim).await.unwrap();
    assert_eq!(again, record);
    assert_eq!(world.provider_calls(), 2);
    assert_eq!(world.runner_calls(), 1);
    assert_eq!(world.ledger(&claim).await, ledger);
    assert_eq!(world.root().await, root);
}

#[tokio::test]
async fn the_no_contrast_record_keeps_its_fixed_reason_as_it_is() {
    let fixture = Fixture::new().await;
    let claim = fixture.claimed(false).await;
    assert_eq!(claim.state, ConsolidationClaimState::NoContrast);
    let record =
        MonitoringCoordinator::complete_no_contrast(&fixture.worker, &fixture.store, &claim.id)
            .await
            .unwrap();
    assert_eq!(record.outcome, ConsolidationRunOutcome::NoChange);
    // A fixed reason that fits is stored exactly as written.
    assert_eq!(
        record.reason,
        "two completed development cycles contained no before/after contrast"
    );
    assert_eq!(record.reason, terminal_category(&record.reason));
}
