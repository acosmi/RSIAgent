//! AG-176 (E15; V118.a and the second half of V118.c; plan §3.4.2, L1378):
//! the independent successor stream, the statistical unit of the meta
//! experiment.
//!
//! What this file shows: the stream preregistration is a pure, recomputable
//! function of the registered semantic clusters, the seed, the per-stream
//! counts and the stream count; S0 task-family and development slices come only
//! from Development clusters and holdout slices only from AcceptanceEpoch
//! clusters, all disjoint; a plan sharing holdout clusters across streams is
//! refused; streams bind only to real verified MetaTrials of distinct S0s and
//! one improver pair; d_i is the equal-weight holdout mean at the preregistered
//! endpoint round, one observation per stream; a per-pair formal plan cannot
//! use the stream unit; and an infeasible stream plan reports only bounded
//! mechanism reuse.
//!
//! Not covered (and not claimed): a persisted preregistration or a stored
//! proof that it preceded every candidate; `meta.start` or
//! `experiment.register` wiring; the MetaTrial and world capacity gates;
//! statistical independence of the slices (only disjointness and
//! recomputability); the holdouts' exposure history across experiments; any
//! real successor, model call or payment; formal tickets, alpha or research
//! family budget; a D−C conclusion, meta gain, sustained acceleration or L5;
//! V118.b.
use evo_core::evaluation::{
    BudgetAuthorizationModel, ControlledTaskAnchorExpectedV1, ControlledTaskAnchorMaterialV1,
    ControlledTaskAnchorRejectionV1, ControlledTaskAnswerV1, ControlledTaskProfileMaterialsV1,
    ControlledTaskProfileV1, CostPlanningInput, DataUse, ExperimentPlan, FeasibilityReason,
    FixedOptimizerArm, FixedOptimizerSpec, FormalExperimentPlanV41, FormalStatisticalUnit, Money,
    MoneyPlanningInput, OptimizationBudgetPlan, OptimizerComparisonContract,
    PlanFeasibilityDisposition, PlanFeasibilityInput, PlanningPremise, PlanningSource,
    ResourcePlanningInput, SequentialPlanning, UnitPlanningInput, assess_plan_feasibility_v1,
    empirical_bernstein,
};
use evo_core::evidence::{ExecutionAttestation, Purpose, TaskOrigin};
use evo_core::improver::{ImproverContentV2, ImproverMechanismV2};
use evo_core::optimization::{OptimizationTrace, TraceOutcome};
use evo_core::sequential::{AlphaAllocation, FormalClaimKind, ResearchFamilyAlphaPlan};
use evo_core::strategy::{ElasticPolicyV1, ExplorationCapsV1, SimulationContext};
use evo_core::{Context, Error, Result, Role, canonical_fingerprint, fingerprint, hash};
use evo_engine::evidence::{StoredRunRecord, StoredTraceAuthority, store_trace_authority};
use evo_engine::exploration::{
    ExplorationDependency, ExplorationWorldV1, RootOpportunity, WorldState,
};
use evo_engine::meta::{
    MetaTrialCoordinator, MetaTrialForkRequest, MetaTrialView, verified_meta_trial,
};
use evo_engine::meta_experiment::{
    META_STREAM_PREREGISTRATION_SCHEMA_V1, META_STREAM_SAMPLER_V1, MetaStreamClaimV1,
    MetaStreamEndpointRowV1, MetaStreamPreregistrationV1, MetaStreamSetV1, MetaStreamSliceCountsV1,
    meta_stream_claim,
};
use evo_storage::Store;
use evo_storage::budget::RootBudgetAuthorization;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

use PlanningPremise::DeclaredApplicable;
use PlanningSource::{Declared, Unavailable};

const TENANT: &str = "n";
const SCOPE: &str = "scope-1";
const FAMILY: &str = "family";
/// The two trusted runs every S0's source closure consists of.
const RUNS: [&str; 2] = ["run-failure", "run-success"];

const CAPABILITIES: [&str; 9] = [
    "no_write",
    "workspace_boundary",
    "explicit_false",
    "explicit_zero",
    "explicit_empty_string",
    "default_inheritance",
    "layer_precedence",
    "not_found",
    "unknown_format_rejection",
];

// ---------------------------------------------------------------------------
// Fixtures: the complete `config_precedence.v1` profile of
// `controlled_task_profile_v45` (L25–170) with one task per listed cluster, and
// the S0 template, trusted source closure and fork of `meta_trial_v42`
// (L175–323) with a real store, no model and no dispatch.
// ---------------------------------------------------------------------------

fn answer(value: Value, present: bool) -> ControlledTaskAnchorExpectedV1 {
    ControlledTaskAnchorExpectedV1::Answer {
        output: ControlledTaskAnswerV1 {
            key: "setting".into(),
            present,
            value,
            source_layer: if present {
                json!("defaults")
            } else {
                Value::Null
            },
            source_path: if present {
                json!("config/defaults.json")
            } else {
                Value::Null
            },
            json_pointer: if present {
                json!("/setting~0~1")
            } else {
                Value::Null
            },
        },
    }
}

/// `development` Development and `acceptance` AcceptanceEpoch clusters, one
/// task each. A cluster is named by its template.
fn clusters(development: usize, acceptance: usize) -> Vec<(String, DataUse)> {
    (0..development)
        .map(|index| (format!("d{index}"), DataUse::Development))
        .chain((0..acceptance).map(|index| (format!("a{index}"), DataUse::AcceptanceEpoch)))
        .collect()
}

/// A complete profile with one task per entry. Entries naming the same template
/// share one semantic cluster (template × layer combination × key type),
/// whatever their task id or material.
fn complete_fixture(
    tasks: &[(String, DataUse)],
) -> (ControlledTaskProfileV1, ControlledTaskProfileMaterialsV1) {
    let templates: BTreeSet<&str> = tasks
        .iter()
        .map(|(template, _)| template.as_str())
        .collect();
    let catalog: Vec<Value> = templates
        .iter()
        .map(|template| {
            json!({"template_id": template, "layer_combinations": [["defaults"]],
                   "key_types": ["string"], "parameter_sets": [{"seed": 1}]})
        })
        .collect();
    let task_materials: serde_json::Map<String, Value> = tasks
        .iter()
        .enumerate()
        .map(|(index, (template, _))| {
            (
                format!("task-{index}"),
                json!({"seed": index, "template": template}),
            )
        })
        .collect();
    let commitments: Vec<Value> = tasks
        .iter()
        .enumerate()
        .map(|(index, (template, data_use))| {
            json!({"task_id": format!("task-{index}"), "template_id": template,
                   "layer_combination": ["defaults"], "key_type": "string",
                   "data_use": data_use, "commitment_digest": hash(b"pending")})
        })
        .collect();
    let mut materials: ControlledTaskProfileMaterialsV1 = serde_json::from_value(json!({
        "generator_source": "// fixture generator v1\nfn generate() {}\n",
        "resolver_source": "// fixture resolver v1\nfn resolve() {}\n",
        "template_catalog": {"templates": catalog},
        "task_materials": task_materials,
        "anchor_materials": [],
        "environment": {"os": "fixture-only", "limits": {"network": false, "write": false}}
    }))
    .unwrap();
    let mut coverages = Vec::new();
    for capability in CAPABILITIES {
        let expected = match capability {
            "no_write" => ControlledTaskAnchorExpectedV1::Rejected {
                reason: ControlledTaskAnchorRejectionV1::WriteForbidden,
            },
            "workspace_boundary" => ControlledTaskAnchorExpectedV1::Rejected {
                reason: ControlledTaskAnchorRejectionV1::WorkspaceBoundary,
            },
            "unknown_format_rejection" => ControlledTaskAnchorExpectedV1::Rejected {
                reason: ControlledTaskAnchorRejectionV1::UnsupportedFormat,
            },
            "explicit_false" => answer(json!(false), true),
            "explicit_zero" => answer(json!(0), true),
            "explicit_empty_string" => answer(json!(""), true),
            "not_found" => answer(Value::Null, false),
            "default_inheritance" => answer(Value::Null, true),
            "layer_precedence" => answer(json!("workspace value"), true),
            _ => unreachable!(),
        };
        let anchor_id = format!("anchor-{capability}");
        materials
            .anchor_materials
            .push(ControlledTaskAnchorMaterialV1 {
                anchor_id: anchor_id.clone(),
                input: json!({"query": "setting", "scenario": capability}),
                expected,
            });
        coverages.push(json!({"capability_id": capability, "anchor_ids": [anchor_id]}));
    }
    let mut profile: ControlledTaskProfileV1 = serde_json::from_value(json!({
        "schema_version": "rsia.controlled_task_profile.v1", "profile_id": "config_precedence.v1",
        "risk": "read_only_no_network_no_write_no_code_execution",
        "layer_order": ["workspace", "user", "defaults"], "config_formats": ["json", "toml"],
        "generator": {"version": "generator-v1", "source_digest": hash(materials.generator_source.as_bytes())},
        "resolver": {"version": "resolver-v1", "source_digest": hash(materials.resolver_source.as_bytes())},
        "template_catalog_digest": hash(b"pending"),
        "task_commitments": commitments,
        "anchor_coverage": {"schema_version": "rsia.anchor_coverage_matrix.v1", "profile_id": "config_precedence.v1",
                            "version": "coverage-v1", "critical_capabilities": coverages},
        "anchor_material_digest": hash(b"pending"), "n_planned": 8,
        "alpha_registration": {"mode": "descriptive"},
        "budget": {"currency": "USD", "pricing_version": "fixture-v1", "per_call_cap_micros": "0",
                   "experiment_total_micros": "0", "payment_subject": "fixture-payer"},
        "environment_digest": hash(b"pending")
    }))
    .unwrap();
    bind_materials(&mut profile, &materials);
    (profile, materials)
}

// These fixture projections spell out the frozen domains independently of the validator.
fn bind_materials(
    profile: &mut ControlledTaskProfileV1,
    materials: &ControlledTaskProfileMaterialsV1,
) {
    let mut catalog = materials.template_catalog.clone();
    catalog
        .templates
        .sort_by(|a, b| a.template_id.cmp(&b.template_id));
    for template in &mut catalog.templates {
        template.key_types.sort();
        template
            .layer_combinations
            .sort_by_cached_key(|value| serde_json::to_vec(value).unwrap());
        template
            .parameter_sets
            .sort_by_cached_key(|value| serde_json::to_vec(value).unwrap());
    }
    profile.template_catalog_digest = canonical_fingerprint(&json!({
        "domain":"rsia.controlled_task_catalog.v1","profile_id":profile.profile_id,"catalog":catalog
    }))
    .unwrap();
    for task in &mut profile.task_commitments {
        task.commitment_digest = canonical_fingerprint(&json!({
            "domain":"rsia.controlled_task_commitment.v1","profile_id":profile.profile_id,
            "task_id":task.task_id,"template_id":task.template_id,"layer_combination":task.layer_combination,
            "key_type":task.key_type,"data_use":task.data_use,"material":materials.task_materials[&task.task_id]
        })).unwrap();
    }
    let mut anchors = materials.anchor_materials.clone();
    anchors.sort_by(|a, b| a.anchor_id.cmp(&b.anchor_id));
    profile.anchor_material_digest = canonical_fingerprint(&json!({
        "domain":"rsia.controlled_task_anchors.v1","profile_id":profile.profile_id,"anchors":anchors
    }))
    .unwrap();
    profile.environment_digest = canonical_fingerprint(&json!({
        "domain":"rsia.controlled_task_environment.v1","profile_id":profile.profile_id,"environment":materials.environment
    })).unwrap();
}

/// A depth-1 candidate improver with `policy` as its exploration policy.
fn improver(policy: ElasticPolicyV1) -> ImproverContentV2 {
    let mut content = ImproverContentV2::builtin_default();
    let ImproverMechanismV2::ExplorationPolicy { policy: slot } = &mut content.mechanism;
    *slot = policy;
    content
}

/// The candidate I1 of every healthy pair: one focused action per branch.
fn i1() -> ImproverContentV2 {
    improver(ElasticPolicyV1 {
        max_focus_actions: 1,
        ..ElasticPolicyV1::default()
    })
}

/// S0 template number `s0`. Distinct numbers are distinct starting systems
/// (another parent bundle), so distinct S0 registrations; the policy is I0's.
/// Nothing is dispatched, so the approved parent is a fixture digest.
fn s0_template(s0: u32) -> ExplorationWorldV1 {
    let parent_skill_digest = hash(b"parent-skill");
    let parent_bundle = hash(format!("parent-bundle-{s0}").as_bytes());
    let environment = hash(b"environment");
    let model = hash(b"model");
    let tools = hash(b"tools");
    let grader = hash(b"grader");
    let rules = hash(b"rules");
    let context_signature = fingerprint(&(
        &parent_skill_digest,
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
        id: "s0-template".into(),
        approved_parent_digest: hash(b"approved-parent"),
        context_signature,
        parent_skill_digest,
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

/// A store with the Host-issued trusted runs of the S0 closure, one watermark
/// bump (every world freezes `source_watermark == 1`) and the root budget the
/// trials name. No call is ever reserved against it.
async fn seeded_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("successor-streams.sqlite3"))
        .await
        .unwrap();
    let host = Context::new(TENANT, "host", Role::Host).unwrap();
    for run in RUNS {
        let authority = StoredTraceAuthority {
            schema_version: "rsia.optimization.source.v1".into(),
            record: StoredRunRecord {
                id: run.into(),
                body: run.as_bytes().to_vec(),
                parent_family: "family-a".into(),
                task_origin: TaskOrigin::TrustedRun,
                execution_attestation: ExecutionAttestation::TrustedHost,
                purpose: Purpose::Development,
            },
            trace: OptimizationTrace {
                run_id: run.into(),
                parent_family: "family-a".into(),
                source_digest: hash(run.as_bytes()),
                purpose: Purpose::Development,
                outcome: TraceOutcome::Success,
                diagnosis: None,
                excerpt: run.into(),
                seed: 1,
            },
            excerpt_start: 0,
            excerpt_end: run.len(),
        };
        store_trace_authority(&store, &host, &authority)
            .await
            .unwrap();
    }
    let mut session = store.session().await.unwrap();
    session.bump_watermark(&host, "e09-initial").await.unwrap();
    session.commit().await.unwrap();
    store
        .authorize_root_budget(
            &Context::new(TENANT, "admin", Role::Admin).unwrap(),
            &RootBudgetAuthorization {
                root_budget_id: "root-1".into(),
                billing_scope: SCOPE.into(),
                allowed_namespaces: vec![TENANT.into()],
                currency: "USD".into(),
                pricing_version: "price-v1".into(),
                payment_subject: "payer-1".into(),
                authorization_receipt_digest: hash(b"admin-authorization"),
                per_call_cap_micros: 20,
                total_limit_micros: 10_000,
                created_at: 1,
            },
        )
        .await
        .unwrap();
    (dir, store)
}

/// A real fork of S0 number `s0`, read back as the only view binding accepts.
async fn trial(
    store: &Store,
    trial_id: &str,
    s0: u32,
    new_content: ImproverContentV2,
) -> MetaTrialView {
    let worker = Context::new(TENANT, "worker", Role::Worker).unwrap();
    MetaTrialCoordinator::fork(
        &worker,
        store,
        MetaTrialForkRequest {
            trial_id: trial_id.into(),
            s0_template: s0_template(s0),
            new_content,
            billing_scope: SCOPE.into(),
        },
    )
    .await
    .unwrap();
    verified_meta_trial(&worker, store, trial_id).await.unwrap()
}

/// Two real trials of distinct S0s and one improver pair, bound to `plan`.
async fn bound_set(
    plan: &MetaStreamPreregistrationV1,
    profile: &ControlledTaskProfileV1,
    materials: &ControlledTaskProfileMaterialsV1,
) -> MetaStreamSetV1 {
    let (_dir, store) = seeded_store().await;
    let first = trial(&store, "trial-a", 0, i1()).await;
    let second = trial(&store, "trial-b", 1, i1()).await;
    plan.bind(profile, materials, &[&first, &second]).unwrap()
}

fn seed() -> String {
    hash(b"meta-stream-seed")
}

fn counts(s0_task_family: u32, development: u32, holdout: u32) -> MetaStreamSliceCountsV1 {
    MetaStreamSliceCountsV1 {
        s0_task_family,
        development,
        holdout,
    }
}

fn preregister(
    profile: &ControlledTaskProfileV1,
    materials: &ControlledTaskProfileMaterialsV1,
    per_stream: MetaStreamSliceCountsV1,
    n_streams: u32,
    endpoint_round: u32,
) -> Result<MetaStreamPreregistrationV1> {
    MetaStreamPreregistrationV1::preregister(
        "meta-streams-1",
        FAMILY,
        profile,
        materials,
        per_stream,
        n_streams,
        endpoint_round,
        &seed(),
    )
}

fn row(stream_seq: u32, round: u32, cluster_id: &str, c: u32, d: u32) -> MetaStreamEndpointRowV1 {
    MetaStreamEndpointRowV1 {
        stream_seq,
        round,
        cluster_id: cluster_id.into(),
        c_score_micros: c,
        d_score_micros: d,
    }
}

/// One row per holdout cluster of every stream at `round`, all with the same scores.
fn rows_at(set: &MetaStreamSetV1, round: u32, c: u32, d: u32) -> Vec<MetaStreamEndpointRowV1> {
    (0..set.n() as u32)
        .flat_map(|stream_seq| {
            set.holdout(stream_seq)
                .unwrap()
                .iter()
                .map(move |cluster| row(stream_seq, round, cluster, c, d))
        })
        .collect()
}

/// The planning fixture of `statistical_feasibility_v43` (`Fixture::fixed`)
/// with `n` planned units, the family `family` and the fixed alpha `alpha`.
fn planning(n: usize, family: &str, alpha: &str) -> (ExperimentPlan, ResearchFamilyAlphaPlan) {
    let mut snapshot = ExperimentPlan::first_low_risk("planning_fixture", n).unwrap();
    snapshot.monetary_budget.pricing_version = "fixture-price".into();
    snapshot.monetary_budget.amount = "3".into();
    snapshot.alpha_total = alpha.parse().unwrap();
    let plan = ResearchFamilyAlphaPlan::new(
        family,
        alpha,
        vec![AlphaAllocation {
            claim_id: "gain".into(),
            attempt_id: "attempt".into(),
            kind: FormalClaimKind::FixedSampleGain,
            alpha: alpha.into(),
        }],
    )
    .unwrap();
    (snapshot, plan)
}

/// The healthy input of `statistical_feasibility_v43.rs:70`, counting
/// `streams` successor streams as its available units.
fn stream_input<'a>(
    snapshot: &'a ExperimentPlan,
    alpha: &'a ResearchFamilyAlphaPlan,
    streams: u32,
) -> PlanFeasibilityInput<'a> {
    PlanFeasibilityInput {
        snapshot,
        effect_hypothesis: 0.10,
        effect_source: DeclaredApplicable,
        variance_upper: Declared(0.0),
        max_n: 100_000,
        alpha_plan: alpha,
        fixed_claim_id: "gain",
        attempt_id: "attempt",
        units: UnitPlanningInput {
            kind: FormalStatisticalUnit::IndependentSuccessorStream,
            available_units: Declared(streams),
            independence: DeclaredApplicable,
            common_mean: DeclaredApplicable,
        },
        money: MoneyPlanningInput {
            currency: Declared("USD"),
            pricing_version: Declared("fixture-price"),
            per_call_cap_micros: Declared(1000),
            experiment_total_micros: Declared(3_000_000),
            payment_subject: Declared("payer"),
            authorization: BudgetAuthorizationModel::DeclaredLimitMicros(3_000_000),
        },
        costs: CostPlanningInput {
            fixed_episode_upper_micros: Declared(1000),
            paired_unit_upper_micros: Declared(2000),
            largest_call_upper_micros: Declared(500),
        },
        resources: ResourcePlanningInput {
            expected_cost_ratio: Declared(1.0),
            expected_p95_ratio: Declared(1.0),
            baseline_cost_positive: DeclaredApplicable,
            bounded_reconcilable_cost: DeclaredApplicable,
            anchors_safety: DeclaredApplicable,
        },
        sequential: SequentialPlanning::Disabled,
    }
}

fn exact_invalid<T>(result: Result<T>, reason: &str) {
    match result {
        Err(Error::Invalid(actual)) => assert_eq!(actual, reason),
        Err(other) => panic!("expected Invalid({reason}), got {other:?}"),
        Ok(_) => panic!("expected Invalid({reason}), got success"),
    }
}

fn exact_conflict<T>(result: Result<T>, reason: &str) {
    match result {
        Err(Error::Conflict(actual)) => assert_eq!(actual, reason),
        Err(other) => panic!("expected Conflict({reason}), got {other:?}"),
        Ok(_) => panic!("expected Conflict({reason}), got success"),
    }
}

fn serde_reason<T: DeserializeOwned>(value: Value, reason: &str) {
    match serde_json::from_value::<T>(value) {
        Err(error) => {
            assert_eq!(error.classify(), serde_json::error::Category::Data);
            assert_eq!(error.to_string().split(", ").next().unwrap(), reason);
        }
        Ok(_) => panic!("expected serde data rejection: {reason}"),
    }
}

// ---------------------------------------------------------------------------
// Preregistration (V118.a: no reuse, disjoint slices, shared holdout refused).
// ---------------------------------------------------------------------------

#[test]
fn preregistration_is_a_pure_recomputable_function_of_registered_clusters_and_seed() {
    assert_eq!(
        META_STREAM_PREREGISTRATION_SCHEMA_V1,
        "rsia.meta_stream_preregistration.v1"
    );
    assert_eq!(
        META_STREAM_SAMPLER_V1,
        "rsia.meta_stream_sampler.hash_rank.v1"
    );
    let (profile, materials) = complete_fixture(&clusters(12, 6));
    let plan = preregister(&profile, &materials, counts(1, 2, 2), 2, 3).unwrap();
    plan.validate_against(&profile, &materials).unwrap();
    assert_eq!(
        plan.profile_digest,
        profile.digest_against(&materials).unwrap()
    );
    assert_eq!(plan.seed, seed());
    assert_eq!(
        preregister(&profile, &materials, counts(1, 2, 2), 2, 3)
            .unwrap()
            .digest()
            .unwrap(),
        plan.digest().unwrap()
    );

    // The frozen hash-rank sampler, spelled out apart from the library: each use
    // ranks the clusters not dealt yet by sha256(sampler \n seed \n use \n id)
    // and deals them out stream by stream.
    let (_, registered) = profile.registered_clusters_against(&materials).unwrap();
    let mut dealt: BTreeSet<String> = BTreeSet::new();
    let mut expected = Vec::new();
    for (purpose, data_use, count) in [
        ("s0_task_family", DataUse::Development, 1),
        ("development", DataUse::Development, 2),
        ("holdout", DataUse::AcceptanceEpoch, 2),
    ] {
        let mut ranked: Vec<(String, String)> = registered
            .iter()
            .filter(|(cluster, cluster_use)| *cluster_use == data_use && !dealt.contains(cluster))
            .map(|(cluster, _)| {
                let input = format!(
                    "rsia.meta_stream_sampler.hash_rank.v1\n{}\n{purpose}\n{cluster}",
                    seed()
                );
                (hash(input.as_bytes()), cluster.clone())
            })
            .collect();
        ranked.sort();
        let slices: Vec<Vec<String>> = ranked
            .chunks(count)
            .take(2)
            .map(|chunk| chunk.iter().map(|(_, cluster)| cluster.clone()).collect())
            .collect();
        dealt.extend(slices.concat());
        expected.push(slices);
    }
    assert_eq!(plan.streams.len(), 2);
    for (index, slot) in plan.streams.iter().enumerate() {
        assert_eq!(slot.stream_seq as usize, index);
        assert_eq!(
            [&slot.s0_task_family, &slot.development, &slot.holdout],
            [
                &expected[0][index],
                &expected[1][index],
                &expected[2][index]
            ]
        );
    }

    // Only registered clusters count: another task of an existing cluster
    // changes the profile digest but not one slice, and task order changes
    // nothing at all.
    let mut grown_tasks = clusters(12, 6);
    grown_tasks.push(("d0".into(), DataUse::Development));
    let (grown, grown_materials) = complete_fixture(&grown_tasks);
    let regrown = preregister(&grown, &grown_materials, counts(1, 2, 2), 2, 3).unwrap();
    assert_ne!(regrown.profile_digest, plan.profile_digest);
    assert_eq!(json!(regrown.streams), json!(plan.streams));
    let mut reordered = profile.clone();
    reordered.task_commitments.reverse();
    assert_eq!(
        preregister(&reordered, &materials, counts(1, 2, 2), 2, 3)
            .unwrap()
            .digest()
            .unwrap(),
        plan.digest().unwrap()
    );

    // Another seed deals other slices.
    let reseeded = MetaStreamPreregistrationV1::preregister(
        "meta-streams-1",
        FAMILY,
        &profile,
        &materials,
        counts(1, 2, 2),
        2,
        3,
        &hash(b"another-seed"),
    )
    .unwrap();
    reseeded.validate_against(&profile, &materials).unwrap();
    assert_ne!(json!(reseeded.streams), json!(plan.streams));

    // A record that differs from its recomputation is refused, even when its
    // slices stay disjoint and inside their partitions.
    let unused = registered
        .iter()
        .find(|(cluster, cluster_use)| {
            *cluster_use == DataUse::Development && !dealt.contains(cluster)
        })
        .unwrap()
        .0
        .clone();
    let mut swapped = plan.clone();
    swapped.streams[0].development[0] = unused;
    let mut reseeded_only = plan.clone();
    reseeded_only.seed = hash(b"another-seed");
    let mut other_profile = plan.clone();
    other_profile.profile_digest = hash(b"another-profile");
    let mut renumbered = plan.clone();
    renumbered.streams.swap(0, 1);
    let mut recounted = plan.clone();
    recounted.per_stream.holdout = 1;
    for record in [swapped, reseeded_only, other_profile, renumbered, recounted] {
        exact_conflict(
            record.validate_against(&profile, &materials),
            "meta_stream_preregistration_mismatch",
        );
    }
    exact_conflict(
        plan.validate_against(&grown, &grown_materials),
        "meta_stream_preregistration_mismatch",
    );
}

#[test]
fn streams_get_disjoint_slices_and_holdout_comes_only_from_acceptance_clusters() {
    let mut tasks = clusters(20, 9);
    tasks.extend([
        ("r0".to_owned(), DataUse::ReplayTrain),
        ("r1".to_owned(), DataUse::ReplaySelect),
        ("a0".to_owned(), DataUse::AcceptanceEpoch),
    ]);
    let (profile, materials) = complete_fixture(&tasks);

    // Registered clusters: each semantic triple once, sorted, with its one use.
    let (_, registered) = profile.registered_clusters_against(&materials).unwrap();
    assert_eq!(registered.len(), 31);
    assert_eq!(
        registered.len(),
        profile
            .registered_cluster_count_against(&materials)
            .unwrap()
    );
    assert!(registered.windows(2).all(|pair| pair[0].0 < pair[1].0));
    let uses: BTreeMap<String, DataUse> = registered.into_iter().collect();
    for (index, (_, data_use)) in tasks.iter().enumerate() {
        let cluster = profile
            .registered_cluster_id_against(&materials, &format!("task-{index}"))
            .unwrap();
        assert_eq!(uses[&cluster], *data_use);
    }

    let plan = preregister(&profile, &materials, counts(2, 3, 3), 3, 2).unwrap();
    plan.validate_against(&profile, &materials).unwrap();
    let mut dealt = BTreeSet::new();
    for (index, slot) in plan.streams.iter().enumerate() {
        assert_eq!(slot.stream_seq as usize, index);
        assert_eq!(
            [
                slot.s0_task_family.len(),
                slot.development.len(),
                slot.holdout.len()
            ],
            [2, 3, 3]
        );
        for cluster in slot.s0_task_family.iter().chain(&slot.development) {
            assert_eq!(uses[cluster], DataUse::Development);
        }
        for cluster in &slot.holdout {
            assert_eq!(uses[cluster], DataUse::AcceptanceEpoch);
        }
        for cluster in slot
            .s0_task_family
            .iter()
            .chain(&slot.development)
            .chain(&slot.holdout)
        {
            assert!(dealt.insert(cluster.as_str()), "a cluster was dealt twice");
        }
    }
    assert_eq!(dealt.len(), 3 * (2 + 3 + 3));

    // Nine AcceptanceEpoch clusters for three holdout slices of three: each one
    // is held out by exactly one stream. Replay clusters are never dealt.
    let acceptance: BTreeSet<&str> = uses
        .iter()
        .filter(|(_, data_use)| **data_use == DataUse::AcceptanceEpoch)
        .map(|(cluster, _)| cluster.as_str())
        .collect();
    let holdouts: BTreeSet<&str> = plan
        .streams
        .iter()
        .flat_map(|slot| slot.holdout.iter().map(String::as_str))
        .collect();
    assert_eq!(holdouts, acceptance);
    for (cluster, data_use) in &uses {
        if matches!(data_use, DataUse::ReplayTrain | DataUse::ReplaySelect) {
            assert!(!dealt.contains(cluster.as_str()));
        }
    }
}

#[tokio::test]
async fn a_plan_sharing_holdout_clusters_across_streams_is_rejected() {
    let (profile, materials) = complete_fixture(&clusters(8, 5));
    let healthy = preregister(&profile, &materials, counts(1, 1, 2), 2, 1).unwrap();
    let (_dir, store) = seeded_store().await;
    let first = trial(&store, "trial-a", 0, i1()).await;
    let second = trial(&store, "trial-b", 1, i1()).await;
    assert_eq!(
        healthy
            .bind(&profile, &materials, &[&first, &second])
            .unwrap()
            .n(),
        2
    );

    let mut shared = healthy.clone();
    shared.streams[1].holdout = healthy.streams[0].holdout.clone();
    let mut one_shared = healthy.clone();
    one_shared.streams[1].holdout[1] = healthy.streams[0].holdout[0].clone();
    for plan in [&shared, &one_shared] {
        exact_conflict(
            plan.validate_against(&profile, &materials),
            "meta_stream_holdout_shared",
        );
        exact_conflict(
            plan.bind(&profile, &materials, &[&first, &second]),
            "meta_stream_holdout_shared",
        );
    }

    // Any other repeated cluster, or one outside its use's partition.
    let (_, registered) = profile.registered_clusters_against(&materials).unwrap();
    let dealt: BTreeSet<&str> = healthy
        .streams
        .iter()
        .flat_map(|slot| {
            slot.s0_task_family
                .iter()
                .chain(&slot.development)
                .chain(&slot.holdout)
        })
        .map(String::as_str)
        .collect();
    let spare = |data_use: DataUse| {
        registered
            .iter()
            .find(|(cluster, cluster_use)| {
                *cluster_use == data_use && !dealt.contains(cluster.as_str())
            })
            .unwrap()
            .0
            .clone()
    };
    let mut repeated = healthy.clone();
    repeated.streams[0].holdout[1] = healthy.streams[0].holdout[0].clone();
    let mut crossed = healthy.clone();
    crossed.streams[1].development[0] = healthy.streams[0].s0_task_family[0].clone();
    let mut development_holdout = healthy.clone();
    development_holdout.streams[0].holdout[0] = spare(DataUse::Development);
    let mut acceptance_s0 = healthy.clone();
    acceptance_s0.streams[0].s0_task_family[0] = spare(DataUse::AcceptanceEpoch);
    let mut unregistered = healthy.clone();
    unregistered.streams[0].development[0] = hash(b"unregistered-cluster");
    for plan in [
        &repeated,
        &crossed,
        &development_holdout,
        &acceptance_s0,
        &unregistered,
    ] {
        exact_conflict(
            plan.validate_against(&profile, &materials),
            "meta_stream_slices_overlap",
        );
        exact_conflict(
            plan.bind(&profile, &materials, &[&first, &second]),
            "meta_stream_slices_overlap",
        );
    }
}

#[test]
fn too_few_registered_clusters_refuses_instead_of_reusing() {
    // Exactly enough: two streams of (1, 1, 1) take four Development and two
    // AcceptanceEpoch clusters, each once.
    let (profile, materials) = complete_fixture(&clusters(4, 2));
    let plan = preregister(&profile, &materials, counts(1, 1, 1), 2, 1).unwrap();
    let dealt: BTreeSet<&str> = plan
        .streams
        .iter()
        .flat_map(|slot| {
            slot.s0_task_family
                .iter()
                .chain(&slot.development)
                .chain(&slot.holdout)
        })
        .map(String::as_str)
        .collect();
    assert_eq!(dealt.len(), 6);

    for (tasks, per_stream, n_streams) in [
        (clusters(4, 2), counts(1, 1, 1), 3),
        (clusters(3, 2), counts(1, 1, 1), 2),
        (clusters(4, 1), counts(1, 1, 1), 2),
        (clusters(4, 2), counts(1, 1, 2), 2),
        (clusters(4, 2), counts(2, 1, 1), 2),
    ] {
        let (profile, materials) = complete_fixture(&tasks);
        exact_invalid(
            preregister(&profile, &materials, per_stream, n_streams, 1),
            "meta_stream_clusters_insufficient",
        );
    }

    // More tasks of registered clusters, or replay clusters, are not more
    // Development clusters.
    let mut padded = clusters(3, 2);
    padded.extend([
        ("d0".to_owned(), DataUse::Development),
        ("d1".to_owned(), DataUse::Development),
        ("r0".to_owned(), DataUse::ReplayTrain),
        ("r1".to_owned(), DataUse::ReplaySelect),
    ]);
    let (profile, materials) = complete_fixture(&padded);
    assert_eq!(
        profile
            .registered_cluster_count_against(&materials)
            .unwrap(),
        7
    );
    exact_invalid(
        preregister(&profile, &materials, counts(1, 1, 1), 2, 1),
        "meta_stream_clusters_insufficient",
    );
}

#[test]
fn preregistration_has_no_candidate_or_result_fields() {
    let (profile, materials) = complete_fixture(&clusters(4, 2));
    let plan = preregister(&profile, &materials, counts(1, 1, 1), 2, 1).unwrap();
    let wire = serde_json::to_value(&plan).unwrap();
    let keys = |value: &Value| -> BTreeSet<String> {
        value.as_object().unwrap().keys().cloned().collect()
    };
    let names = |list: &[&str]| -> BTreeSet<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    };
    assert_eq!(
        keys(&wire),
        names(&[
            "schema_version",
            "id",
            "research_family_id",
            "profile_digest",
            "sampler_version",
            "seed",
            "per_stream",
            "endpoint_round",
            "streams",
        ])
    );
    assert_eq!(
        keys(&wire["per_stream"]),
        names(&["s0_task_family", "development", "holdout"])
    );
    for slot in wire["streams"].as_array().unwrap() {
        assert_eq!(
            keys(slot),
            names(&["stream_seq", "s0_task_family", "development", "holdout"])
        );
    }
    let back: MetaStreamPreregistrationV1 = serde_json::from_value(wire.clone()).unwrap();
    back.validate_against(&profile, &materials).unwrap();
    assert_eq!(back.digest().unwrap(), plan.digest().unwrap());

    // A candidate or a result has nowhere to go, at any level.
    for field in ["candidate_digest", "d_i", "verdict"] {
        let reason = format!("unknown field `{field}`");
        let mut top = wire.clone();
        top[field] = json!(hash(b"candidate"));
        serde_reason::<MetaStreamPreregistrationV1>(top, &reason);
        let mut slot = wire.clone();
        slot["streams"][0][field] = json!(0.1);
        serde_reason::<MetaStreamPreregistrationV1>(slot, &reason);
        let mut per_stream = wire.clone();
        per_stream["per_stream"][field] = json!("improved");
        serde_reason::<MetaStreamPreregistrationV1>(per_stream, &reason);
    }

    // The record's own shape: two or more streams, at least one cluster per
    // slice, an endpoint round, a 64-hex seed, identifier names.
    for (per_stream, n_streams, endpoint_round) in [
        (counts(1, 1, 1), 1, 1),
        (counts(0, 1, 1), 2, 1),
        (counts(1, 0, 1), 2, 1),
        (counts(1, 1, 0), 2, 1),
        (counts(1, 1, 1), 2, 0),
    ] {
        exact_invalid(
            preregister(&profile, &materials, per_stream, n_streams, endpoint_round),
            "meta_stream_shape_invalid",
        );
    }
    let healthy_seed = seed();
    for bad_seed in [
        healthy_seed.to_uppercase(),
        healthy_seed[..63].to_owned(),
        format!("{}g", &healthy_seed[..63]),
        String::new(),
    ] {
        exact_invalid(
            MetaStreamPreregistrationV1::preregister(
                "meta-streams-1",
                FAMILY,
                &profile,
                &materials,
                counts(1, 1, 1),
                2,
                1,
                &bad_seed,
            ),
            "meta_stream_seed_invalid",
        );
    }
    exact_invalid(
        MetaStreamPreregistrationV1::preregister(
            "meta streams",
            FAMILY,
            &profile,
            &materials,
            counts(1, 1, 1),
            2,
            1,
            &healthy_seed,
        ),
        "invalid identifier",
    );
    let mut schema = plan.clone();
    schema.schema_version = "rsia.meta_stream_preregistration.v2".into();
    let mut sampler = plan.clone();
    sampler.sampler_version = "rsia.meta_stream_sampler.uniform.v1".into();
    for record in [schema, sampler] {
        exact_invalid(
            record.validate_against(&profile, &materials),
            "meta_stream_schema_unsupported",
        );
    }
    let mut single = plan.clone();
    single.streams.truncate(1);
    exact_invalid(
        single.validate_against(&profile, &materials),
        "meta_stream_shape_invalid",
    );
}

// ---------------------------------------------------------------------------
// Binding to real trials and the stream observations (V118.a).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn two_real_forked_trials_bind_as_two_streams() {
    let (profile, materials) = complete_fixture(&clusters(4, 4));
    let plan = preregister(&profile, &materials, counts(1, 1, 2), 2, 3).unwrap();
    let (_dir, store) = seeded_store().await;
    let first = trial(&store, "trial-a", 0, i1()).await;
    let second = trial(&store, "trial-b", 1, i1()).await;
    assert_ne!(first.s0_template_digest(), second.s0_template_digest());
    assert_eq!(
        first.new_stream().content_digest(),
        second.new_stream().content_digest()
    );

    let set = plan.bind(&profile, &materials, &[&first, &second]).unwrap();
    assert_eq!(set.n(), 2);
    assert_eq!(set.preregistration_digest(), plan.digest().unwrap());
    assert_eq!(set.research_family_id(), FAMILY);
    for (stream_seq, view) in [(0, &first), (1, &second)] {
        assert_eq!(set.trial_id(stream_seq), Some(view.trial_id()));
        assert_eq!(
            set.holdout(stream_seq),
            Some(plan.streams[stream_seq as usize].holdout.as_slice())
        );
    }
    assert_eq!(set.trial_id(2), None);
    assert_eq!(set.holdout(2), None);

    // Stream `i` is `trials[i]`.
    let swapped = plan.bind(&profile, &materials, &[&second, &first]).unwrap();
    assert_eq!(swapped.trial_id(0), Some("trial-b"));
    assert_eq!(swapped.trial_id(1), Some("trial-a"));

    // A trial count other than the preregistered stream count is refused.
    let third = trial(&store, "trial-c", 2, i1()).await;
    exact_invalid(
        plan.bind(&profile, &materials, &[&first]),
        "meta_stream_trial_count_mismatch",
    );
    exact_invalid(
        plan.bind(&profile, &materials, &[&first, &second, &third]),
        "meta_stream_trial_count_mismatch",
    );
}

#[tokio::test]
async fn same_s0_reused_trial_or_other_improver_pair_do_not_add_n() {
    let (profile, materials) = complete_fixture(&clusters(6, 3));
    let plan = preregister(&profile, &materials, counts(1, 1, 1), 2, 1).unwrap();
    let (_dir, store) = seeded_store().await;
    let first = trial(&store, "trial-a", 0, i1()).await;

    // Another trial forked from the same S0 registration is no new stream.
    let same_s0 = trial(&store, "trial-b", 0, i1()).await;
    assert_eq!(first.s0_template_digest(), same_s0.s0_template_digest());
    exact_conflict(
        plan.bind(&profile, &materials, &[&first, &same_s0]),
        "meta_stream_s0_reused",
    );
    // Nor is the same trial counted twice.
    exact_conflict(
        plan.bind(&profile, &materials, &[&first, &first]),
        "meta_stream_trial_reused",
    );
    // Nor a trial of another improver pair: it repeats a different unit.
    let other_pair = trial(
        &store,
        "trial-c",
        1,
        improver(ElasticPolicyV1 {
            fairness_wait_rounds: 3,
            ..ElasticPolicyV1::default()
        }),
    )
    .await;
    assert_ne!(
        first.new_stream().content_digest(),
        other_pair.new_stream().content_digest()
    );
    exact_conflict(
        plan.bind(&profile, &materials, &[&first, &other_pair]),
        "meta_stream_improver_pair_differs",
    );

    // Healthy control: a distinct S0 with the same pair is the second stream.
    let independent = trial(&store, "trial-d", 1, i1()).await;
    assert_eq!(
        plan.bind(&profile, &materials, &[&first, &independent])
            .unwrap()
            .n(),
        2
    );
    let three = preregister(&profile, &materials, counts(1, 1, 1), 3, 1).unwrap();
    exact_conflict(
        three.bind(&profile, &materials, &[&first, &independent, &same_s0]),
        "meta_stream_s0_reused",
    );
}

#[tokio::test]
async fn only_the_preregistered_endpoint_round_enters_d_i() {
    let (profile, materials) = complete_fixture(&clusters(4, 4));
    let plan = preregister(&profile, &materials, counts(1, 1, 2), 2, 3).unwrap();
    let set = bound_set(&plan, &profile, &materials).await;

    // Endpoint round 3: D−C = +0.1 on every holdout cluster. Earlier and later
    // rounds disagree and do not count.
    let mut rows = rows_at(&set, 3, 500_000, 600_000);
    for (round, d) in [(1, 1_000_000), (2, 0), (4, 0)] {
        rows.extend(rows_at(&set, round, 500_000, d));
    }
    let observations = set.endpoint_observations(&rows).unwrap();
    assert_eq!(
        observations
            .iter()
            .map(|observation| observation.d)
            .collect::<Vec<_>>(),
        [0.1, 0.1]
    );
    let mut moved = rows.clone();
    for row in moved.iter_mut().filter(|row| row.round != 3) {
        row.d_score_micros = 1_000_000 - row.d_score_micros;
    }
    let moved = set.endpoint_observations(&moved).unwrap();
    assert_eq!(
        moved
            .iter()
            .map(|observation| observation.d)
            .collect::<Vec<_>>(),
        [0.1, 0.1]
    );

    // No other round stands in for a missing endpoint row.
    let earlier: Vec<_> = rows.iter().filter(|row| row.round != 3).cloned().collect();
    exact_invalid(
        set.endpoint_observations(&earlier),
        "meta_stream_endpoint_rows_incomplete",
    );
    let missing = set.holdout(1).unwrap()[0].clone();
    let mut gap = rows.clone();
    gap.retain(|row| row.round != 3 || row.stream_seq != 1 || row.cluster_id != missing);
    exact_invalid(
        set.endpoint_observations(&gap),
        "meta_stream_endpoint_rows_incomplete",
    );
}

#[tokio::test]
async fn d_i_is_the_equal_weight_holdout_mean_and_one_stream_counts_once() {
    let (profile, materials) = complete_fixture(&clusters(4, 6));
    let plan = preregister(&profile, &materials, counts(1, 1, 3), 2, 2).unwrap();
    let set = bound_set(&plan, &profile, &materials).await;
    let first = set.holdout(0).unwrap().to_vec();
    let second = set.holdout(1).unwrap().to_vec();

    // Stream 0: +0.3, −0.1, +0.1 → 0.1. Stream 1: −0.2, −0.2, +0.1 → −0.1.
    let rows = vec![
        row(0, 2, &first[0], 200_000, 500_000),
        row(0, 2, &first[1], 500_000, 400_000),
        row(0, 2, &first[2], 0, 100_000),
        row(1, 2, &second[0], 700_000, 500_000),
        row(1, 2, &second[1], 300_000, 100_000),
        row(1, 2, &second[2], 900_000, 1_000_000),
    ];
    let observations = set.endpoint_observations(&rows).unwrap();
    assert_eq!(observations.len(), set.n());
    assert_eq!(observations[0].cluster_id, "trial-a");
    assert_eq!(observations[1].cluster_id, "trial-b");
    assert_eq!(observations[0].d, 0.1);
    assert_eq!(observations[1].d, -0.1);
    assert!(
        observations
            .iter()
            .all(|observation| observation.weight == 1.0)
    );
    let mut reversed = rows.clone();
    reversed.reverse();
    assert_eq!(
        json!(set.endpoint_observations(&reversed).unwrap()),
        json!(observations)
    );
    // The statistic sees two units, not six holdout clusters.
    assert_eq!(empirical_bernstein(&observations, 0.05).unwrap().n, 2);

    // Exactly one row per stream and holdout cluster.
    let mut duplicate = rows.clone();
    duplicate.push(rows[0].clone());
    exact_invalid(
        set.endpoint_observations(&duplicate),
        "meta_stream_endpoint_row_duplicate",
    );
    for foreign in [
        row(0, 2, &second[0], 0, 0),
        row(2, 2, &first[0], 0, 0),
        row(0, 2, &hash(b"unregistered-cluster"), 0, 0),
    ] {
        let mut extended = rows.clone();
        extended.push(foreign);
        exact_invalid(
            set.endpoint_observations(&extended),
            "meta_stream_endpoint_row_foreign",
        );
    }
    let mut out_of_range = rows.clone();
    out_of_range[0].d_score_micros = 1_000_001;
    exact_invalid(
        set.endpoint_observations(&out_of_range),
        "meta_stream_endpoint_score_out_of_range",
    );
    // A missing row fails the whole sample: no partial n.
    exact_invalid(
        set.endpoint_observations(&rows[..5]),
        "meta_stream_endpoint_rows_incomplete",
    );
    exact_invalid(
        set.endpoint_observations(&rows[..3]),
        "meta_stream_endpoint_rows_incomplete",
    );
}

// ---------------------------------------------------------------------------
// Statistical unit and feasibility (V118.a, second half of V118.c).
// ---------------------------------------------------------------------------

#[test]
fn per_pair_formal_plan_rejects_the_stream_unit_and_old_units_keep_their_wire() {
    for (unit, wire) in [
        (
            FormalStatisticalUnit::IndependentTaskCluster,
            "independent_task_cluster",
        ),
        (
            FormalStatisticalUnit::IndependentCompleteStratifiedBlock,
            "independent_complete_stratified_block",
        ),
        (
            FormalStatisticalUnit::IndependentSuccessorStream,
            "independent_successor_stream",
        ),
    ] {
        assert_eq!(serde_json::to_value(unit).unwrap(), json!(wire));
        assert_eq!(
            serde_json::from_value::<FormalStatisticalUnit>(json!(wire)).unwrap(),
            unit
        );
    }

    let spec = |arm: FixedOptimizerArm, implementation: &str| FixedOptimizerSpec {
        arm,
        implementation_digest: hash(implementation.as_bytes()),
        initial_s0_digest: hash(b"s0"),
        base_model_digest: hash(b"base-model"),
        tools_digest: hash(b"tools"),
        authorized_materials_digest: hash(b"materials"),
        task_partition_digest: hash(b"partition"),
        context_limit_tokens: 8_000,
        root_budget_scope_id: "root-budget".into(),
        budget_limit: Money::zero_unfunded("USD"),
    };
    let comparison = OptimizerComparisonContract::new(
        spec(FixedOptimizerArm::CSimple, "simple-v1"),
        spec(FixedOptimizerArm::CSkillopt, "skillopt-v1"),
        FixedOptimizerArm::CSkillopt,
    )
    .unwrap();
    let budget = OptimizationBudgetPlan::unfunded("root-budget", "payer", "USD").unwrap();
    let mut snapshot = ExperimentPlan::first_low_risk("formal-v41", 60).unwrap();
    snapshot.freeze(10).unwrap();
    let (_, alpha) = planning(60, FAMILY, "0.05");
    let formal = |unit: FormalStatisticalUnit| {
        FormalExperimentPlanV41::new(
            &snapshot,
            hash(b"preregistered-before-candidate"),
            &alpha,
            &comparison,
            &budget,
            hash(b"anchor-coverage"),
            hash(b"sampler-rules"),
            hash(b"assumptions-review"),
            unit,
            60,
        )
    };

    // The per-pair units still build and keep their wire names.
    for (unit, wire) in [
        (
            FormalStatisticalUnit::IndependentTaskCluster,
            "independent_task_cluster",
        ),
        (
            FormalStatisticalUnit::IndependentCompleteStratifiedBlock,
            "independent_complete_stratified_block",
        ),
    ] {
        let plan = formal(unit).unwrap();
        plan.validate_against(&snapshot, &alpha, &comparison, &budget)
            .unwrap();
        plan.digest().unwrap();
        assert_eq!(
            serde_json::to_value(plan).unwrap()["statistical_unit"],
            json!(wire)
        );
    }

    // A successor pair's FormalEvaluation counts task clusters; it can never be
    // a meta-experiment stream.
    let reason = "a per-pair formal plan cannot use the independent successor stream unit";
    exact_invalid(
        formal(FormalStatisticalUnit::IndependentSuccessorStream),
        reason,
    );
    let mut stream = formal(FormalStatisticalUnit::IndependentTaskCluster).unwrap();
    stream.statistical_unit = FormalStatisticalUnit::IndependentSuccessorStream;
    exact_invalid(stream.validate(), reason);
    exact_invalid(stream.digest(), reason);
    exact_invalid(
        stream.validate_against(&snapshot, &alpha, &comparison, &budget),
        reason,
    );
    let mut wire =
        serde_json::to_value(formal(FormalStatisticalUnit::IndependentTaskCluster).unwrap())
            .unwrap();
    wire["statistical_unit"] = json!("independent_successor_stream");
    let parsed: FormalExperimentPlanV41 = serde_json::from_value(wire).unwrap();
    exact_invalid(parsed.validate(), reason);
}

#[tokio::test]
async fn infeasible_stream_plan_reports_only_bounded_mechanism_reuse() {
    // n = 2 at the healthy values of statistical_feasibility_v43.rs:70: the one
    // gate finds the plan descriptive only, so the streams report bounded
    // mechanism reuse.
    let (profile, materials) = complete_fixture(&clusters(4, 2));
    let plan = preregister(&profile, &materials, counts(1, 1, 1), 2, 1).unwrap();
    let pair = bound_set(&plan, &profile, &materials).await;
    let (snapshot, alpha) = planning(2, FAMILY, "0.05");
    let input = stream_input(&snapshot, &alpha, 2);
    let gate = assess_plan_feasibility_v1(&input).unwrap();
    assert_eq!(gate.reasons, [FeasibilityReason::PlannedNTooSmall]);
    // At these values the same gate needs 217 streams, more than the 10
    // MetaTrials one instance may hold (plan L385).
    assert_eq!(gate.fixed.unwrap().required_fixed_n, Some(217));
    assert_eq!(
        meta_stream_claim(&pair, &input).unwrap(),
        MetaStreamClaimV1::BoundedMechanismReuseOnly
    );

    // Another unit, another stream count or another research family is not
    // a claim about these streams.
    let (_, other_family) = planning(2, "other-family", "0.05");
    let mut mismatched = vec![stream_input(&snapshot, &other_family, 2)];
    for kind in [
        FormalStatisticalUnit::IndependentTaskCluster,
        FormalStatisticalUnit::IndependentCompleteStratifiedBlock,
    ] {
        let mut wrong = input;
        wrong.units.kind = kind;
        mismatched.push(wrong);
    }
    for available in [Declared(1), Declared(3), Unavailable] {
        let mut wrong = input;
        wrong.units.available_units = available;
        mismatched.push(wrong);
    }
    for wrong in &mismatched {
        exact_invalid(
            meta_stream_claim(&pair, wrong),
            "meta_stream_claim_input_mismatch",
        );
    }

    // Healthy control at the MetaTrial cap: ten real streams. At the values of
    // L70 they stay descriptive; with alpha_i 0.45 and effect 0.9 (every other
    // value of L70 kept) the same gate is numerically compatible, and that is
    // all FormalPlanFeasible says.
    let n = 10;
    let (profile, materials) = complete_fixture(&clusters(2 * n, n));
    let plan = preregister(&profile, &materials, counts(1, 1, 1), n as u32, 1).unwrap();
    let (_dir, store) = seeded_store().await;
    let mut views = Vec::new();
    for s0 in 0..n as u32 {
        views.push(trial(&store, &format!("trial-{s0}"), s0, i1()).await);
    }
    let set = plan
        .bind(&profile, &materials, &views.iter().collect::<Vec<_>>())
        .unwrap();
    assert_eq!(set.n(), n);
    let (snapshot, alpha) = planning(n, FAMILY, "0.05");
    assert_eq!(
        meta_stream_claim(&set, &stream_input(&snapshot, &alpha, n as u32)).unwrap(),
        MetaStreamClaimV1::BoundedMechanismReuseOnly
    );
    let (snapshot, alpha) = planning(n, FAMILY, "0.45");
    let mut input = stream_input(&snapshot, &alpha, n as u32);
    input.effect_hypothesis = 0.9;
    let gate = assess_plan_feasibility_v1(&input).unwrap();
    assert_eq!(
        gate.disposition,
        PlanFeasibilityDisposition::NumericallyCompatible
    );
    assert_eq!(
        meta_stream_claim(&set, &input).unwrap(),
        MetaStreamClaimV1::FormalPlanFeasible
    );
    // One planned stream under the gate's own required n is descriptive again.
    let required = gate.fixed.unwrap().required_fixed_n.unwrap() as usize;
    let (short, alpha) = planning(required - 1, FAMILY, "0.45");
    let mut input = stream_input(&short, &alpha, n as u32);
    input.effect_hypothesis = 0.9;
    assert_eq!(
        meta_stream_claim(&set, &input).unwrap(),
        MetaStreamClaimV1::BoundedMechanismReuseOnly
    );
}
