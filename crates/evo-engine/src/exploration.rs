//! generate → execute_dev → observe → decide. Intermediate nodes are not published.
//! A dispatched candidate is observed as a valid node only when E03's observation gate
//! verified the development report behind it ([`ExplorationEvidenceV1`]).
use crate::capacity::{
    CapacityField, CapacityLimits, V41CapacityUsage, admit_field, unix_now_secs,
};
use crate::evidence::validate_stored_sources;
use crate::model::ModelPort;
use crate::optimization::{
    DevRunner, DevelopmentExecutionProvenance, DevelopmentRunReport, OptimizationJournal,
    OptimizationStepOutcome, OptimizationStepRequest, StageFact, StageFactKind,
    development_stage_fact_ids, optimization_request_digest, run_optimization_step,
    verified_development_observation_in_session,
};
use evo_core::evaluation::DataUse;
use evo_core::skill_edit::skill_snapshot_digest;
use evo_core::strategy::{
    ActionKindV1, BatchActionV1, BudgetViewV1, ElasticPolicyV1, ExplorationCapsV1,
    ExplorationPolicy, HistoryQuery, LegalActionV1, LegalActionsV1, ObservedStatus,
    OpportunityWait, OptimizationHistoryEntry, PrefixNodeV2, PrefixView, PrefixViewV2,
    RetryDecision, SimulationContext, decide_elastic, deterministic_retry_decision,
    select_optimization_history,
};
use evo_core::{Context, Error, Result, Role, Validate, fingerprint, identifier};
use evo_storage::{Session, Store};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    Generated,
    ExecutedDev,
    Observed,
    Selected,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchNode {
    pub id: String,
    pub prefix: PrefixView,
    pub state: NodeState,
    pub published: bool,
}

pub struct Coordinator {
    pub policy: ExplorationPolicy,
    pub nodes: Vec<SearchNode>,
}

impl Coordinator {
    pub fn new(policy: ExplorationPolicy) -> Result<Self> {
        policy.validate()?;
        Ok(Self {
            policy,
            nodes: Vec::new(),
        })
    }

    pub fn generate(&mut self, id: String, prefix: PrefixView) -> Result<()> {
        prefix.validate()?;
        if self.nodes.len() as u8 >= self.policy.max_nodes {
            return Err(Error::Budget);
        }
        if prefix.depth > self.policy.max_depth {
            return Err(Error::Invalid("depth cap".into()));
        }
        self.nodes.push(SearchNode {
            id,
            prefix,
            state: NodeState::Generated,
            published: false,
        });
        Ok(())
    }

    pub fn execute_dev(&mut self, id: &str) -> Result<()> {
        self.advance(id, NodeState::Generated, NodeState::ExecutedDev)
    }

    pub fn observe(&mut self, id: &str) -> Result<()> {
        self.advance(id, NodeState::ExecutedDev, NodeState::Observed)
    }

    pub fn decide(&mut self, id: &str) -> Result<()> {
        self.advance(id, NodeState::Observed, NodeState::Selected)
    }

    pub fn publish(&self, id: &str) -> Result<()> {
        let node = self
            .nodes
            .iter()
            .find(|n| n.id == id)
            .ok_or(Error::NotFound)?;
        if node.state != NodeState::Selected {
            return Err(Error::Conflict(
                "search intermediate nodes are not published".into(),
            ));
        }
        Err(Error::Conflict(
            "final candidate must be re-resolved against approved_parent before publish".into(),
        ))
    }

    fn advance(&mut self, id: &str, from: NodeState, to: NodeState) -> Result<()> {
        let node = self
            .nodes
            .iter_mut()
            .find(|n| n.id == id)
            .ok_or(Error::NotFound)?;
        if node.state != from {
            return Err(Error::Conflict("illegal node transition".into()));
        }
        node.state = to;
        Ok(())
    }
}

// Revocation closure (plan §11.5, E08): a world depends on its source runs
// (`persist_world_registration`), and every node, dispatch fact and history
// entry depends on its world (`put_world_edge`). The cleanup walks the
// dependents of a revoked run, so it reaches the world and, through it, each of
// those records, which are all on its redact allow-list. A redacted record is
// named by the reads (`read_envelope`), not reported as a storage failure.
const WORLD_RECORD_KIND: &str = "exploration_world_v1";
pub(crate) const NODE_RECORD_KIND: &str = "exploration_node_v1";
const DISPATCH_RECORD_KIND: &str = "exploration_dispatch_v1";
const HISTORY_RECORD_KIND: &str = "optimization_history_v1";
/// Dispatch facts carry the decision's policy/caps digests (E14). The v1 shape
/// has none and is refused on read instead of being reinterpreted.
const DISPATCH_SCHEMA: &str = "rsia.exploration_dispatch.v2";
const DISPATCH_SCHEMA_V1: &str = "rsia.exploration_dispatch.v1";
pub(crate) const ENVELOPE_SCHEMA: &str = "rsia.exploration_artifact_envelope.v1";
/// Schema of the tombstone the revocation cleanup leaves in place of a record
/// whose source was revoked.
const REDACTED_SCHEMA: &str = "rsia.redacted.v1";

fn validate_digest(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::Invalid("expected lowercase sha256 digest".into()));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactEnvelope<T> {
    schema_version: String,
    id: String,
    record_kind: String,
    payload: T,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootOpportunity {
    pub root_slot: u32,
    pub branch_seq: u32,
    pub action_seq: u32,
    pub estimated_cost_upper_micros: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplorationDependency {
    pub kind: String,
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldState {
    Collecting,
    Sealed,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplorationWorldV1 {
    pub schema_version: String,
    pub id: String,
    pub approved_parent_digest: String,
    pub context_signature: String,
    pub parent_skill_digest: String,
    pub parent_bundle_digest: String,
    pub environment_digest: String,
    pub model_digest: String,
    pub tools_digest: String,
    pub grader_digest: String,
    pub rules_digest: String,
    pub source_watermark: u64,
    pub caps: ExplorationCapsV1,
    pub policy: ElasticPolicyV1,
    pub simulation: SimulationContext,
    pub root_opportunities: Vec<RootOpportunity>,
    pub dependencies: Vec<ExplorationDependency>,
    pub successor_cost_upper_micros: u64,
    pub initial_baseline_quality_micros: u32,
    pub remaining_root_micros: u64,
    pub remaining_recovery_dispatches: u8,
    pub state: WorldState,
    pub node_ids: Vec<String>,
    pub dispatch_ids: Vec<String>,
    pub history_ids: Vec<String>,
    pub current_branch_seq: Option<u32>,
    pub current_branch_focus_actions: u8,
    pub decision_round: u32,
    pub waits: Vec<OpportunityWait>,
}

impl ExplorationWorldV1 {
    pub const SCHEMA: &'static str = "rsia.exploration_world.v1";

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != Self::SCHEMA {
            return Err(Error::Invalid(
                "unsupported exploration world schema".into(),
            ));
        }
        identifier(&self.id)?;
        for value in [
            &self.approved_parent_digest,
            &self.context_signature,
            &self.parent_skill_digest,
            &self.parent_bundle_digest,
            &self.environment_digest,
            &self.model_digest,
            &self.tools_digest,
            &self.grader_digest,
            &self.rules_digest,
        ] {
            validate_digest(value)?;
        }
        if self.context_signature
            != fingerprint(&(
                &self.parent_skill_digest,
                &self.parent_bundle_digest,
                &self.environment_digest,
                &self.model_digest,
                &self.tools_digest,
                &self.grader_digest,
                &self.rules_digest,
                self.source_watermark,
            ))?
        {
            return Err(Error::Conflict("world context signature mismatch".into()));
        }
        self.caps.validate()?;
        self.policy.validate()?;
        self.simulation.width()?;
        if self.initial_baseline_quality_micros > 1_000_000
            || self.node_ids.len() > usize::from(self.caps.max_nodes)
            || self.successor_cost_upper_micros == 0
        {
            return Err(Error::Invalid("invalid exploration world limits".into()));
        }
        let mut roots = BTreeSet::new();
        for root in &self.root_opportunities {
            if root.root_slot == 0
                || root.branch_seq == 0
                || root.action_seq == 0
                || root.action_seq >= 1_000_000
                || root.estimated_cost_upper_micros == 0
                || !roots.insert(root.action_seq)
            {
                return Err(Error::Invalid("invalid root opportunity".into()));
            }
        }
        let mut dependencies = BTreeSet::new();
        if self.dependencies.is_empty() {
            return Err(Error::Invalid(
                "exploration world requires a typed source closure".into(),
            ));
        }
        for dependency in &self.dependencies {
            identifier(&dependency.kind)?;
            identifier(&dependency.id)?;
            if dependency.kind != "run"
                || !dependencies.insert((dependency.kind.as_str(), dependency.id.as_str()))
            {
                return Err(Error::Invalid(
                    "exploration source closure must contain unique trusted runs".into(),
                ));
            }
        }
        for id in self
            .node_ids
            .iter()
            .chain(self.dispatch_ids.iter())
            .chain(self.history_ids.iter())
        {
            identifier(id)?;
        }
        Ok(())
    }

    /// Digest of the immutable registration request: identity, S0 digests,
    /// source closure, frozen caps/policy/simulation, root opportunities and
    /// the per-step cost and baseline (plan §7.1.1: the facts a world is
    /// registered with never change, its state is derived). Everything
    /// `run_next` and `record_history` move is excluded on purpose, so a crash
    /// re-run of the same registration, or the same registration after the
    /// world dispatched, converges on the persisted world (plan §6.7.4) instead
    /// of conflicting with its own earlier commits: the world state, the
    /// node/dispatch/history ids, branch focus, decision round, waits, and the
    /// two budget counters (`remaining_root_micros`,
    /// `remaining_recovery_dispatches`) that `run_next` spends. The counters are
    /// the registration's initial budget, so
    /// [`PersistentCoordinator::register_world_idempotent`] compares them apart,
    /// against the value the world was registered with.
    pub fn registration_fingerprint(&self) -> Result<String> {
        fingerprint(&(
            &self.schema_version,
            &self.id,
            &self.approved_parent_digest,
            &self.context_signature,
            (
                &self.parent_skill_digest,
                &self.parent_bundle_digest,
                &self.environment_digest,
                &self.model_digest,
                &self.tools_digest,
                &self.grader_digest,
                &self.rules_digest,
            ),
            self.source_watermark,
            &self.caps,
            &self.policy,
            self.simulation,
            &self.root_opportunities,
            &self.dependencies,
            (
                self.successor_cost_upper_micros,
                self.initial_baseline_quality_micros,
            ),
        ))
    }
}

/// What the E03 observation gate made of the development evidence behind a
/// dispatched candidate (plan E09: successors are chosen only after a real
/// per-node evaluation; E03: a self-reported provenance or score grants nothing).
/// A node's `Valid` status is a claim of the optimizer's report until the gate has
/// checked it, so the label travels with the node and its dispatch fact and every
/// consumer that needs a trusted observation reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplorationEvidenceV1 {
    /// The development report passed the observation gate: its receipts, budget
    /// rows and scores were reloaded and the node's quality is the server-recomputed
    /// one. The only label a `Valid` node written by `run_next` can carry.
    Trusted,
    /// The report declared Fixture provenance. That is honest, and it can never be
    /// a trusted observation; the gate was not asked.
    FixtureDeclared,
    /// The report claimed an execution the gate refused: no receipts, scores that
    /// differ from the recomputed ones, or any other mismatch.
    EvidenceRejected,
    /// No gate verdict exists: the outcome was no candidate, the gate could not be
    /// reached, or the record was written before the gate existed (the default a
    /// record without the field reads as).
    #[default]
    NotObserved,
}

impl ExplorationEvidenceV1 {
    /// The label as it is stored, for messages that must not carry a raw error.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trusted => "trusted",
            Self::FixtureDeclared => "fixture_declared",
            Self::EvidenceRejected => "evidence_rejected",
            Self::NotObserved => "not_observed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistentSearchNode {
    pub schema_version: String,
    pub world_id: String,
    pub node: PrefixNodeV2,
    pub candidate_bundle_digest: Option<String>,
    pub candidate_skill_digest: Option<String>,
    pub development_selection_digest: Option<String>,
    pub intermediate_only: bool,
    /// The gate's verdict on the evidence behind this node. A node stored without
    /// the field is `NotObserved`.
    #[serde(default)]
    pub evidence: ExplorationEvidenceV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplorationDispatchState {
    Claimed,
    Observed,
    Uncertain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplorationDispatchFact {
    pub schema_version: String,
    pub id: String,
    pub world_id: String,
    pub action_id: String,
    pub action_seq: u32,
    pub request_digest: String,
    pub idempotency_key: String,
    pub decision: CoordinatorDecision,
    pub selected_action: LegalActionV1,
    pub expected_parent_skill_digest: String,
    pub expected_parent_bundle_digest: String,
    pub context_signature: String,
    pub state: ExplorationDispatchState,
    pub node_id: Option<String>,
    pub outcome_reason: Option<String>,
    /// The gate's verdict on the evidence of the dispatched step, the same label
    /// its node carries. `NotObserved` while the fact is claimed, for an outcome
    /// that is no candidate, and for a fact written before the gate existed (the
    /// schema is unchanged: a v2 fact without the field reads as `NotObserved`).
    #[serde(default)]
    pub evidence: ExplorationEvidenceV1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoordinatorDecision {
    pub world_id: String,
    pub prefix_digest: String,
    pub legal_actions_digest: String,
    /// Digest of the `ElasticPolicyV1` actually passed to `decide_elastic`.
    pub policy_digest: String,
    /// Digest of the `ExplorationCapsV1` actually passed to `decide_elastic`.
    pub caps_digest: String,
    pub action: BatchActionV1,
}

/// Outcome of an idempotent world registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisterWorldOutcome {
    /// The world was persisted by this call.
    Registered,
    /// A world with the same id and the same registration fingerprint was
    /// already persisted; nothing was written.
    AlreadyRegistered,
}

/// Read-only projection of a world's pure decision, used by the management
/// `status` read side to re-verify a stored `exploration.start` result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldDecisionView {
    pub context_signature: String,
    pub state: WorldState,
    pub decision: CoordinatorDecision,
}

/// Whether the real dispatch behind a usage record ended observed or
/// uncertain (an uncertain dispatch already cost something, so it counts).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MechanismUsageState {
    Observed,
    Uncertain,
}

/// Derived, read-only evidence that an already dispatched real decision was
/// taken with a world's frozen policy and caps (plan §9.1, V036). It is a
/// view, not a fact source: it cannot be deserialized, its fields are private,
/// and [`PersistentCoordinator::verified_mechanism_usage`] is the only place
/// that builds one, after re-reading and cross-checking the stored facts. It
/// proves that the policy appeared in a dispatched decision, not that the
/// policy is better.
#[derive(Debug, Clone, Serialize)]
pub struct MechanismUsageRecordV1 {
    world_id: String,
    dispatch_id: String,
    context_signature: String,
    approved_parent_digest: String,
    policy_digest: String,
    caps_digest: String,
    prefix_digest: String,
    legal_actions_digest: String,
    action_digest: String,
    dispatch_state: MechanismUsageState,
    node_id: Option<String>,
    evidence: ExplorationEvidenceV1,
}

impl MechanismUsageRecordV1 {
    pub fn world_id(&self) -> &str {
        &self.world_id
    }

    pub fn dispatch_id(&self) -> &str {
        &self.dispatch_id
    }

    pub fn context_signature(&self) -> &str {
        &self.context_signature
    }

    pub fn approved_parent_digest(&self) -> &str {
        &self.approved_parent_digest
    }

    pub fn policy_digest(&self) -> &str {
        &self.policy_digest
    }

    pub fn caps_digest(&self) -> &str {
        &self.caps_digest
    }

    pub fn prefix_digest(&self) -> &str {
        &self.prefix_digest
    }

    pub fn legal_actions_digest(&self) -> &str {
        &self.legal_actions_digest
    }

    /// `fingerprint` of the decision's actual `BatchActionV1`.
    pub fn action_digest(&self) -> &str {
        &self.action_digest
    }

    pub fn dispatch_state(&self) -> MechanismUsageState {
        self.dispatch_state
    }

    pub fn node_id(&self) -> Option<&str> {
        self.node_id.as_deref()
    }

    /// The gate's verdict on the development evidence of the dispatched step, as
    /// its dispatch fact stores it. A dispatch whose evidence is not `Trusted` still
    /// happened and still cost something; it is not a trusted observation.
    pub fn evidence(&self) -> ExplorationEvidenceV1 {
        self.evidence
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoordinatorStepResult {
    pub decision: CoordinatorDecision,
    pub dispatch_id: Option<String>,
    pub node_id: Option<String>,
    pub outcome: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FinalCandidateRequest {
    pub world_id: String,
    pub selected_node_id: String,
    pub selected_bundle_digest: String,
    pub approved_parent_digest: String,
    pub requires_recompile_against_approved_parent: bool,
    pub requires_independent_formal_evaluation: bool,
}

#[derive(Clone)]
pub struct PersistentCoordinator {
    store: Store,
    context: Context,
    owner: String,
}

impl PersistentCoordinator {
    pub fn new(store: Store, context: Context, owner: impl Into<String>) -> Result<Self> {
        context.require(&[Role::Worker, Role::Admin])?;
        let owner = owner.into();
        identifier(&owner)?;
        Ok(Self {
            store,
            context,
            owner,
        })
    }

    pub async fn register_world(&self, world: ExplorationWorldV1) -> Result<()> {
        ensure_new_world_shape(&world)?;
        let mut session = self.store.session().await?;
        check_world_live(&mut session, &self.context, &world).await?;
        if get_record::<ExplorationWorldV1>(
            &mut session,
            &self.context,
            WORLD_RECORD_KIND,
            &world.id,
        )
        .await?
        .is_some()
        {
            return Err(Error::Conflict("exploration world already exists".into()));
        }
        self.persist_world_registration(&mut session, &world)
            .await?;
        session.commit().await
    }

    /// Registers `world` at most once. A world with the same id whose
    /// registration fingerprint and registered budget equal the request is
    /// reported as `AlreadyRegistered` without writing anything, so a crash
    /// re-run of the same management job, or the same registration after the
    /// world dispatched, converges (plan §6.7.4); a same-id world with a
    /// different registration is a `Conflict`.
    ///
    /// The fingerprint leaves out the two budget counters `run_next` spends, so
    /// they are compared apart and exactly, against the value the world was
    /// registered with: the stored value plus what the world's own dispatch
    /// facts say was spent (`registered_budget`). Neither "not below the
    /// current value" (it would take 2_000 for a world registered with 1_000)
    /// nor the current value itself is a registration. Dispatch facts that do
    /// not account for the world leave no way to tell a spend from a lowered
    /// counter, and are a `Conflict` too. The comparison is
    /// `ensure_registered_as`, the one the management `status` of a started
    /// world makes as well.
    ///
    /// Liveness of the source closure is re-checked on every call: watermark
    /// drift is `Conflict`, a tombstoned dependency is `Forbidden`.
    pub async fn register_world_idempotent(
        &self,
        world: ExplorationWorldV1,
    ) -> Result<RegisterWorldOutcome> {
        ensure_new_world_shape(&world)?;
        let mut session = self.store.session().await?;
        check_world_live(&mut session, &self.context, &world).await?;
        if let Some(existing) = get_record::<ExplorationWorldV1>(
            &mut session,
            &self.context,
            WORLD_RECORD_KIND,
            &world.id,
        )
        .await?
        {
            existing.validate()?;
            ensure_registered_as(&mut session, &self.context, &existing, &world).await?;
            session.commit().await?;
            return Ok(RegisterWorldOutcome::AlreadyRegistered);
        }
        self.persist_world_registration(&mut session, &world)
            .await?;
        session.commit().await?;
        Ok(RegisterWorldOutcome::Registered)
    }

    async fn persist_world_registration(
        &self,
        session: &mut Session,
        world: &ExplorationWorldV1,
    ) -> Result<()> {
        put_record(
            session,
            &self.context,
            WORLD_RECORD_KIND,
            &world.id,
            &self.owner,
            world,
        )
        .await?;
        let world_storage_id = storage_id(WORLD_RECORD_KIND, &world.id)?;
        for dependency in &world.dependencies {
            session
                .put_edge(
                    &self.context,
                    "artifact",
                    &world_storage_id,
                    &dependency.kind,
                    &dependency.id,
                )
                .await?;
        }
        Ok(())
    }

    pub async fn decide_next(&self, world_id: &str) -> Result<CoordinatorDecision> {
        let (world, nodes) = self.load_world_nodes(world_id).await?;
        pure_decision(&world, &nodes)
    }

    /// Same pure decision as [`Self::decide_next`], returned together with the
    /// world's context signature and state so a read side can compare it with
    /// a previously stored result. Nothing is written.
    pub async fn decision_view(&self, world_id: &str) -> Result<WorldDecisionView> {
        let (world, nodes) = self.load_world_nodes(world_id).await?;
        let decision = pure_decision(&world, &nodes)?;
        Ok(WorldDecisionView {
            context_signature: world.context_signature,
            state: world.state,
            decision,
        })
    }

    /// Mechanism usage derived from the world's persisted dispatch facts, in
    /// dispatch order (plan §9.1, V036/V086.d). A record exists only for a
    /// dispatch that really happened: `Observed`, or `Uncertain` (already paid
    /// for). A `Claimed` fact (not yet proven dispatched) yields none, and the
    /// first decision of `decide_next`/`decision_view`/`exploration.start`
    /// yields none either: no dispatch, no use.
    ///
    /// Every fact is cross-checked against the live world: the world must
    /// validate and its source closure must still be live (watermark drift is
    /// `Conflict`, a tombstoned source is `Forbidden`); a fact must belong to
    /// this world and context signature and to a dispatch listed exactly once;
    /// its decision must carry the digests of the world's frozen policy and
    /// caps and a single dispatch action equal to the fact's action; an
    /// observed fact must name a node persisted for this world. Any failure
    /// fails the whole call; nothing partial is returned.
    pub async fn verified_mechanism_usage(
        &self,
        world_id: &str,
    ) -> Result<Vec<MechanismUsageRecordV1>> {
        identifier(world_id)?;
        let mut session = self.store.session().await?;
        let world: ExplorationWorldV1 =
            need_record(&mut session, &self.context, WORLD_RECORD_KIND, world_id).await?;
        world.validate()?;
        if world.id != world_id {
            return Err(Error::Conflict(
                "stored exploration world differs from its storage identity".into(),
            ));
        }
        check_world_live(&mut session, &self.context, &world).await?;
        let policy_digest = world.policy.digest()?;
        let caps_digest = world.caps.digest()?;
        let mut listed = BTreeSet::new();
        let mut records = Vec::new();
        for dispatch_id in &world.dispatch_ids {
            if !listed.insert(dispatch_id.as_str()) {
                return Err(Error::Conflict(
                    "exploration world lists a dispatch more than once".into(),
                ));
            }
            let fact = need_dispatch_fact(&mut session, &self.context, dispatch_id).await?;
            if fact.id != *dispatch_id
                || fact.world_id != world.id
                || fact.context_signature != world.context_signature
                || fact.decision.world_id != world.id
            {
                return Err(Error::Conflict(
                    "dispatch fact does not belong to this exploration world".into(),
                ));
            }
            if fact.decision.policy_digest != policy_digest
                || fact.decision.caps_digest != caps_digest
            {
                return Err(Error::Conflict(
                    "dispatch decision was not taken with the world's frozen policy and caps"
                        .into(),
                ));
            }
            let dispatch_state = match fact.state {
                ExplorationDispatchState::Claimed => continue,
                ExplorationDispatchState::Observed => MechanismUsageState::Observed,
                ExplorationDispatchState::Uncertain => MechanismUsageState::Uncertain,
            };
            validate_digest(&fact.decision.prefix_digest)?;
            validate_digest(&fact.decision.legal_actions_digest)?;
            match &fact.decision.action {
                BatchActionV1::Dispatch {
                    action_ids,
                    action_seqs,
                    ..
                } if action_ids.len() == 1
                    && action_seqs.len() == 1
                    && action_ids[0] == fact.action_id
                    && action_seqs[0] == fact.action_seq
                    && fact.selected_action.action_id == fact.action_id
                    && fact.selected_action.action_seq == fact.action_seq => {}
                _ => {
                    return Err(Error::Conflict(
                        "dispatch fact does not match its decision action".into(),
                    ));
                }
            }
            match (&fact.node_id, dispatch_state) {
                (None, MechanismUsageState::Observed) => {
                    return Err(Error::Conflict(
                        "observed dispatch names no exploration node".into(),
                    ));
                }
                (Some(node_id), _) => {
                    if !world.node_ids.iter().any(|known| known == node_id) {
                        return Err(Error::Conflict(
                            "dispatch node is not listed by the exploration world".into(),
                        ));
                    }
                    let node: PersistentSearchNode =
                        need_record(&mut session, &self.context, NODE_RECORD_KIND, node_id).await?;
                    if node.world_id != world.id {
                        return Err(Error::Conflict(
                            "dispatch node belongs to a different exploration world".into(),
                        ));
                    }
                }
                (None, MechanismUsageState::Uncertain) => {}
            }
            records.push(MechanismUsageRecordV1 {
                world_id: world.id.clone(),
                dispatch_id: fact.id.clone(),
                context_signature: world.context_signature.clone(),
                approved_parent_digest: world.approved_parent_digest.clone(),
                policy_digest: fact.decision.policy_digest.clone(),
                caps_digest: fact.decision.caps_digest.clone(),
                prefix_digest: fact.decision.prefix_digest.clone(),
                legal_actions_digest: fact.decision.legal_actions_digest.clone(),
                action_digest: fingerprint(&fact.decision.action)?,
                dispatch_state,
                node_id: fact.node_id.clone(),
                evidence: fact.evidence,
            });
        }
        session.commit().await?;
        Ok(records)
    }

    pub async fn record_history(
        &self,
        world_id: &str,
        entry: OptimizationHistoryEntry,
    ) -> Result<()> {
        if entry.data_use != DataUse::Development {
            return Err(Error::Forbidden);
        }
        identifier(&entry.entry_id)?;
        let mut session = self.store.session().await?;
        let mut world: ExplorationWorldV1 =
            need_record(&mut session, &self.context, WORLD_RECORD_KIND, world_id).await?;
        world.validate()?;
        check_world_live(&mut session, &self.context, &world).await?;
        if world.state != WorldState::Collecting {
            return Err(Error::Conflict(
                "sealed world cannot accept development history".into(),
            ));
        }
        if get_record::<OptimizationHistoryEntry>(
            &mut session,
            &self.context,
            HISTORY_RECORD_KIND,
            &entry.entry_id,
        )
        .await?
        .is_some()
        {
            return Err(Error::Conflict("history entry already exists".into()));
        }
        world.history_ids.push(entry.entry_id.clone());
        put_record(
            &mut session,
            &self.context,
            HISTORY_RECORD_KIND,
            &entry.entry_id,
            &self.owner,
            &entry,
        )
        .await?;
        // The entry names no source run: it belongs to the world it is listed
        // in (`history_ids`) and is only ever read through that world.
        put_world_edge(
            &mut session,
            &self.context,
            HISTORY_RECORD_KIND,
            &entry.entry_id,
            world_id,
        )
        .await?;
        put_record(
            &mut session,
            &self.context,
            WORLD_RECORD_KIND,
            world_id,
            &self.owner,
            &world,
        )
        .await?;
        session.commit().await
    }

    pub async fn matching_history(
        &self,
        world_id: &str,
        query: HistoryQuery<'_>,
    ) -> Result<Vec<OptimizationHistoryEntry>> {
        let mut session = self.store.session().await?;
        let world: ExplorationWorldV1 =
            need_record(&mut session, &self.context, WORLD_RECORD_KIND, world_id).await?;
        world.validate()?;
        check_world_live(&mut session, &self.context, &world).await?;
        let mut entries = Vec::with_capacity(world.history_ids.len());
        for entry_id in &world.history_ids {
            entries.push(
                need_record(&mut session, &self.context, HISTORY_RECORD_KIND, entry_id).await?,
            );
        }
        session.commit().await?;
        select_optimization_history(&entries, query)
    }

    async fn dispatch_for_request(
        &self,
        world_id: &str,
        request_id: &str,
    ) -> Result<Option<ExplorationDispatchFact>> {
        let mut session = self.store.session().await?;
        let world: ExplorationWorldV1 =
            need_record(&mut session, &self.context, WORLD_RECORD_KIND, world_id).await?;
        world.validate()?;
        check_world_live(&mut session, &self.context, &world).await?;
        let mut found = None;
        for dispatch_id in &world.dispatch_ids {
            let dispatch = need_dispatch_fact(&mut session, &self.context, dispatch_id).await?;
            if dispatch.idempotency_key == request_id {
                if found.is_some() {
                    return Err(Error::Conflict(
                        "request id is bound to multiple exploration dispatches".into(),
                    ));
                }
                found = Some(dispatch);
            }
        }
        session.commit().await?;
        Ok(found)
    }

    pub async fn run_next(
        &self,
        model: Option<&dyn ModelPort>,
        runner: Option<&dyn DevRunner>,
        journal: Option<&dyn OptimizationJournal>,
        request: OptimizationStepRequest<'_>,
    ) -> Result<CoordinatorStepResult> {
        let world_id = request.model_context.episode_id.clone();
        let request_digest = optimization_request_digest(&request)?;
        // The two facts the observation gate reads once the step is done. A pure
        // function of the request, derived before anything is claimed: nothing
        // that can fail may run between the paid step and the node's write.
        let (request_fact_id, observed_fact_id) =
            development_stage_fact_ids(&request.model_context, &request.development_request)?;
        let prior = self
            .dispatch_for_request(&world_id, &request.model_context.request_id)
            .await?;
        let decision = if let Some(existing) = &prior {
            if existing.request_digest != request_digest {
                return Err(Error::Conflict(
                    "request id is already bound to different optimization input".into(),
                ));
            }
            if existing.state != ExplorationDispatchState::Claimed {
                return Ok(CoordinatorStepResult {
                    decision: existing.decision.clone(),
                    dispatch_id: Some(existing.id.clone()),
                    node_id: existing.node_id.clone(),
                    outcome: existing
                        .outcome_reason
                        .clone()
                        .unwrap_or_else(|| "terminal_dispatch".into()),
                });
            }
            existing.decision.clone()
        } else {
            if model.is_none() || runner.is_none() || journal.is_none() {
                return Err(Error::NotFound);
            }
            self.decide_next(&world_id).await?
        };
        if model.is_none() || runner.is_none() || journal.is_none() {
            return Err(Error::NotFound);
        }
        let (action_id, action_seq, cost) = match decision.action.clone() {
            BatchActionV1::Dispatch {
                action_ids,
                action_seqs,
                estimated_cost_upper_micros,
                ..
            } if action_ids.len() == 1 && action_seqs.len() == 1 => (
                action_ids[0].clone(),
                action_seqs[0],
                estimated_cost_upper_micros,
            ),
            BatchActionV1::Stop { reason } => {
                return Ok(CoordinatorStepResult {
                    decision,
                    dispatch_id: None,
                    node_id: None,
                    outcome: reason,
                });
            }
            _ => {
                return Err(Error::Invalid(
                    "real online coordinator requires exactly one W_online action".into(),
                ));
            }
        };
        let dispatch_id = format!(
            "dispatch-{}",
            &fingerprint(&(world_id.as_str(), action_seq))?[..32]
        );
        let mut session = self.store.session().await?;
        let mut world: ExplorationWorldV1 =
            need_record(&mut session, &self.context, WORLD_RECORD_KIND, &world_id).await?;
        world.validate()?;
        check_world_live(&mut session, &self.context, &world).await?;
        let mut history = Vec::with_capacity(world.history_ids.len());
        for entry_id in &world.history_ids {
            history.push(
                need_record(&mut session, &self.context, HISTORY_RECORD_KIND, entry_id).await?,
            );
        }
        if deterministic_retry_decision(
            &history,
            &request.model_context.parent_skill_digest,
            &request_digest,
            &fingerprint(&request.edit_batch_template)?,
            &request.development_request.environment_digest,
            &fingerprint(request.evidence)?,
        ) == RetryDecision::ReuseDeterministicDiagnostic
        {
            session.commit().await?;
            return Ok(CoordinatorStepResult {
                decision,
                dispatch_id: None,
                node_id: None,
                outcome: "deterministic_diagnostic_reused".into(),
            });
        }
        let nodes_before = self.load_nodes_in_session(&mut session, &world).await?;
        let legal_before = derive_legal_actions(&world, &nodes_before)?;
        let selected_action = legal_before
            .actions
            .iter()
            .find(|action| action.action_seq == action_seq && action.action_id == action_id)
            .ok_or_else(|| Error::Conflict("selected action is no longer legal".into()))?
            .clone();
        let (expected_parent_skill_digest, expected_parent_bundle_digest) =
            expected_parent_binding(&world, &nodes_before, &selected_action)?;
        validate_optimization_request_binding(&world, &nodes_before, &selected_action, &request)?;
        let available_action_seqs: Vec<u32> = legal_before
            .actions
            .iter()
            .map(|action| action.action_seq)
            .collect();
        let stored_dispatch = get_dispatch_fact(&mut session, &self.context, &dispatch_id).await?;
        let mut dispatch = match stored_dispatch {
            Some(existing) => {
                if existing.request_digest != request_digest
                    || existing.action_seq != action_seq
                    || fingerprint(&existing.decision)? != fingerprint(&decision)?
                    || fingerprint(&existing.selected_action)? != fingerprint(&selected_action)?
                    || existing.expected_parent_skill_digest != expected_parent_skill_digest
                    || existing.expected_parent_bundle_digest != expected_parent_bundle_digest
                    || existing.context_signature != world.context_signature
                {
                    return Err(Error::Conflict("dispatch idempotency conflict".into()));
                }
                if existing.state != ExplorationDispatchState::Claimed {
                    session.commit().await?;
                    return Ok(CoordinatorStepResult {
                        decision: existing.decision.clone(),
                        dispatch_id: Some(existing.id),
                        node_id: existing.node_id,
                        outcome: existing
                            .outcome_reason
                            .unwrap_or_else(|| "terminal_dispatch".into()),
                    });
                }
                existing
            }
            None => {
                if world.remaining_root_micros < cost {
                    return Err(Error::Budget);
                }
                // E16.5: this dispatch derives one new exploration node. Refuse
                // inside the session, before the dispatch fact and before any
                // model call, when the namespace already holds the MVP maximum.
                let usage: V41CapacityUsage =
                    session.capacity_usage_v41(unix_now_secs()).await?.into();
                admit_field(
                    CapacityField::ExplorationNodes,
                    usage.exploration_nodes,
                    &CapacityLimits::default(),
                )?;
                let fact = ExplorationDispatchFact {
                    schema_version: DISPATCH_SCHEMA.into(),
                    id: dispatch_id.clone(),
                    world_id: world_id.clone(),
                    action_id: action_id.clone(),
                    action_seq,
                    request_digest: request_digest.clone(),
                    idempotency_key: request.model_context.request_id.clone(),
                    decision: decision.clone(),
                    selected_action: selected_action.clone(),
                    expected_parent_skill_digest: expected_parent_skill_digest.into(),
                    expected_parent_bundle_digest: expected_parent_bundle_digest.into(),
                    context_signature: world.context_signature.clone(),
                    state: ExplorationDispatchState::Claimed,
                    node_id: None,
                    outcome_reason: None,
                    evidence: ExplorationEvidenceV1::NotObserved,
                };
                world.dispatch_ids.push(dispatch_id.clone());
                put_record(
                    &mut session,
                    &self.context,
                    DISPATCH_RECORD_KIND,
                    &dispatch_id,
                    &self.owner,
                    &fact,
                )
                .await?;
                put_world_edge(
                    &mut session,
                    &self.context,
                    DISPATCH_RECORD_KIND,
                    &dispatch_id,
                    &world.id,
                )
                .await?;
                put_record(
                    &mut session,
                    &self.context,
                    WORLD_RECORD_KIND,
                    &world.id,
                    &self.owner,
                    &world,
                )
                .await?;
                fact
            }
        };
        if dispatch.state != ExplorationDispatchState::Claimed {
            return Err(Error::Conflict("dispatch is not resumable".into()));
        }
        session.commit().await?;
        let task_count = request.development_request.manifest.tasks.len();
        let outcome = run_optimization_step(model, runner, journal, request).await;
        let mut session = self.store.session().await?;
        world = need_record(&mut session, &self.context, WORLD_RECORD_KIND, &world_id).await?;
        let outcome = match check_world_live(&mut session, &self.context, &world).await {
            Ok(()) => outcome,
            Err(error) => Ok(OptimizationStepOutcome::Uncertain {
                reason: format!("exploration source closure changed after dispatch: {error}"),
            }),
        };
        dispatch = need_dispatch_fact(&mut session, &self.context, &dispatch_id).await?;
        let mut existing_nodes = self.load_nodes_in_session(&mut session, &world).await?;
        let current_prefix = prefix_projection(&world, &existing_nodes)?;
        let current_legal = derive_legal_actions(&world, &existing_nodes)?;
        if fingerprint(&current_prefix)? != decision.prefix_digest
            || fingerprint(&current_legal)? != decision.legal_actions_digest
        {
            return Err(Error::Conflict(
                "exploration prefix changed while optimization was running".into(),
            ));
        }
        let action = dispatch.selected_action.clone();
        let next_seq = world.node_ids.len() as u32 + 1;
        let (status, evidence, skill_digest, bundle_digest, selection_digest, outcome_reason) =
            match outcome {
                Ok(OptimizationStepOutcome::Candidate {
                    edit,
                    bundle,
                    selection,
                }) => {
                    // The step's `Candidate` rests on the runner's own report: its
                    // provenance and scores grant nothing (E03). The observation gate
                    // runs in this session, so its verdict and the node it decides are
                    // one commit. The step is paid for and its dispatch is claimed, so
                    // the verdict is never an error: a refusal is a terminal node, and
                    // a gate that could not answer is the uncertain path.
                    let verdict = judge_candidate_evidence(
                        &mut session,
                        &self.context,
                        &request_fact_id,
                        &observed_fact_id,
                        task_count,
                    )
                    .await;
                    let (status, evidence, reason) = verdict.observed();
                    let (skill_digest, bundle_digest, selection_digest) =
                        if verdict.keeps_candidate() {
                            (
                                Some(skill_snapshot_digest(&edit.output)?),
                                Some(bundle.digest.clone()),
                                Some(fingerprint(&selection)?),
                            )
                        } else {
                            (None, None, None)
                        };
                    (
                        status,
                        evidence,
                        skill_digest,
                        bundle_digest,
                        selection_digest,
                        reason.to_string(),
                    )
                }
                Ok(OptimizationStepOutcome::NoChange { reason }) => (
                    ObservedStatus::HardFailure,
                    ExplorationEvidenceV1::NotObserved,
                    None,
                    None,
                    None,
                    reason,
                ),
                Ok(OptimizationStepOutcome::Rejected { reason }) => (
                    ObservedStatus::HardFailure,
                    ExplorationEvidenceV1::NotObserved,
                    None,
                    None,
                    None,
                    reason,
                ),
                Ok(OptimizationStepOutcome::Uncertain { reason }) => (
                    ObservedStatus::UsageUncertain,
                    ExplorationEvidenceV1::NotObserved,
                    None,
                    None,
                    None,
                    reason_or_cancelled(reason),
                ),
                Err(Error::Cancelled) => (
                    ObservedStatus::UsageUncertain,
                    ExplorationEvidenceV1::NotObserved,
                    None,
                    None,
                    None,
                    "cancelled_or_uncertain".into(),
                ),
                Err(error) => (
                    ObservedStatus::UsageUncertain,
                    ExplorationEvidenceV1::NotObserved,
                    None,
                    None,
                    None,
                    format!("optimization dispatch outcome uncertain: {error}"),
                ),
            };
        let (search_parent_seq, branch_seq, depth) = match &action.kind {
            ActionKindV1::Widen { root_slot: _ } => (None, action.branch_seq, 1),
            ActionKindV1::Deepen { parent_node_seq } => (
                Some(*parent_node_seq),
                action.branch_seq,
                action.target_depth,
            ),
            ActionKindV1::Recover {
                failed_node_seq, ..
            } => {
                world.remaining_recovery_dispatches =
                    world.remaining_recovery_dispatches.saturating_sub(1);
                (
                    Some(*failed_node_seq),
                    action.branch_seq,
                    action.target_depth,
                )
            }
        };
        let parent_node = search_parent_seq.and_then(|parent| {
            existing_nodes
                .iter()
                .find(|node| node.node.node_seq == parent)
        });
        // What a valid node's gain is measured against: the parent's own quality (its
        // best valid ancestor when the parent is not valid, the baseline for a root).
        // The replay computes the gain the same way (`node_from_transition`).
        let previous_quality = parent_node
            .and_then(|parent| match parent.node.status {
                ObservedStatus::Valid { quality_micros } => Some(quality_micros),
                _ => parent.node.best_valid_ancestor_micros,
            })
            .or(Some(world.initial_baseline_quality_micros));
        let mut gains = parent_node
            .map(|parent| parent.node.recent_valid_gains_micros.clone())
            .unwrap_or_default();
        if let (ObservedStatus::Valid { quality_micros }, Some(parent)) =
            (&status, previous_quality)
        {
            gains.push(*quality_micros as i32 - parent as i32);
            if gains.len() > 2 {
                gains.remove(0);
            }
        }
        // The best valid ancestor is not the gain's reference. `PrefixViewV2::validate`
        // requires a node's best to be its parent's best (the baseline for a root),
        // raised to the node's own quality when it is valid. The replay takes the
        // largest of the parent's quality, the parent's best and the baseline, which
        // is the same value on any prefix that check accepts. Taking the parent's
        // *quality* instead leaves a node the check refuses whenever that quality is
        // below the parent's best (a trusted root under the baseline, or a parent
        // that regressed), and `decide_next` and `status` then fail for good.
        let baseline_micros = world.initial_baseline_quality_micros;
        let inherited_best_micros = parent_node.map_or(baseline_micros, |parent| {
            let parent_quality = match parent.node.status {
                ObservedStatus::Valid { quality_micros } => Some(quality_micros),
                _ => None,
            };
            parent_quality
                .into_iter()
                .chain(parent.node.best_valid_ancestor_micros)
                .fold(baseline_micros, u32::max)
        });
        let best_valid_ancestor_micros = Some(match &status {
            ObservedStatus::Valid { quality_micros } => {
                (*quality_micros).max(inherited_best_micros)
            }
            _ => inherited_best_micros,
        });
        let repair_failed = matches!(action.kind, ActionKindV1::Recover { .. })
            && !matches!(status, ObservedStatus::Valid { .. });
        if let ActionKindV1::Recover {
            failed_node_seq, ..
        } = &action.kind
            && let Some(failed) = existing_nodes
                .iter_mut()
                .find(|node| node.node.node_seq == *failed_node_seq)
        {
            if let ObservedStatus::RepairableFailure {
                dispatched_repairs, ..
            } = &mut failed.node.status
            {
                *dispatched_repairs = dispatched_repairs.saturating_add(1);
            }
            if repair_failed {
                failed.node.repair_failures_dispatched =
                    failed.node.repair_failures_dispatched.saturating_add(1);
            }
            let failed_id = world
                .node_ids
                .get((*failed_node_seq as usize).saturating_sub(1))
                .ok_or(Error::NotFound)?;
            put_record(
                &mut session,
                &self.context,
                NODE_RECORD_KIND,
                failed_id,
                &self.owner,
                failed,
            )
            .await?;
        }
        let node_id = format!("node-{}-{next_seq}", world.id);
        let node = PersistentSearchNode {
            schema_version: "rsia.exploration_node.v1".into(),
            world_id: world.id.clone(),
            node: PrefixNodeV2 {
                node_seq: next_seq,
                branch_seq,
                search_parent_seq,
                approved_parent_digest: world.approved_parent_digest.clone(),
                depth,
                status,
                best_valid_ancestor_micros,
                recent_valid_gains_micros: gains,
                repair_failures_dispatched: u8::from(repair_failed),
            },
            candidate_bundle_digest: bundle_digest,
            candidate_skill_digest: skill_digest,
            development_selection_digest: selection_digest,
            intermediate_only: true,
            evidence,
        };
        world.remaining_root_micros = world.remaining_root_micros.saturating_sub(cost);
        world.node_ids.push(node_id.clone());
        let previous_branch = world.current_branch_seq;
        world.current_branch_seq = Some(branch_seq);
        world.current_branch_focus_actions = if previous_branch == Some(branch_seq) {
            world.current_branch_focus_actions.saturating_add(1)
        } else {
            1
        };
        world.decision_round = world.decision_round.saturating_add(1);
        update_waits(&mut world, action_seq, &available_action_seqs);
        dispatch.state = if matches!(node.node.status, ObservedStatus::UsageUncertain) {
            ExplorationDispatchState::Uncertain
        } else {
            ExplorationDispatchState::Observed
        };
        dispatch.node_id = Some(node_id.clone());
        dispatch.outcome_reason = Some(outcome_reason.clone());
        dispatch.evidence = evidence;
        put_record(
            &mut session,
            &self.context,
            NODE_RECORD_KIND,
            &node_id,
            &self.owner,
            &node,
        )
        .await?;
        put_world_edge(
            &mut session,
            &self.context,
            NODE_RECORD_KIND,
            &node_id,
            &world.id,
        )
        .await?;
        put_record(
            &mut session,
            &self.context,
            DISPATCH_RECORD_KIND,
            &dispatch_id,
            &self.owner,
            &dispatch,
        )
        .await?;
        put_record(
            &mut session,
            &self.context,
            WORLD_RECORD_KIND,
            &world.id,
            &self.owner,
            &world,
        )
        .await?;
        session.commit().await?;
        Ok(CoordinatorStepResult {
            decision,
            dispatch_id: Some(dispatch_id),
            node_id: Some(node_id),
            outcome: outcome_reason,
        })
    }

    pub async fn final_candidate_request(
        &self,
        world_id: &str,
        node_id: &str,
    ) -> Result<FinalCandidateRequest> {
        let mut session = self.store.session().await?;
        let world: ExplorationWorldV1 =
            need_record(&mut session, &self.context, WORLD_RECORD_KIND, world_id).await?;
        world.validate()?;
        check_world_live(&mut session, &self.context, &world).await?;
        let node: PersistentSearchNode =
            need_record(&mut session, &self.context, NODE_RECORD_KIND, node_id).await?;
        if node.world_id != world.id
            || !world.node_ids.iter().any(|id| id == node_id)
            || !node.intermediate_only
        {
            return Err(Error::Conflict(
                "selected node does not belong to the requested world".into(),
            ));
        }
        // A final candidate is proposed for re-resolution against the approved
        // parent and independent formal evaluation, so it must rest on a development
        // observation the E03 gate verified: a valid node whose evidence is trusted.
        // A fixture, a refused report, an unobserved outcome and a record written
        // before the gate existed are none of those.
        if !matches!(node.node.status, ObservedStatus::Valid { .. }) {
            return Err(Error::Conflict(
                "selected node is not a valid observed candidate".into(),
            ));
        }
        if node.evidence != ExplorationEvidenceV1::Trusted {
            return Err(Error::Conflict(format!(
                "selected node's development evidence is {}, not trusted",
                node.evidence.as_str()
            )));
        }
        session.commit().await?;
        let bundle = node
            .candidate_bundle_digest
            .ok_or_else(|| Error::Invalid("selected node has no development candidate".into()))?;
        Ok(FinalCandidateRequest {
            world_id: world.id,
            selected_node_id: node_id.into(),
            selected_bundle_digest: bundle,
            approved_parent_digest: world.approved_parent_digest,
            requires_recompile_against_approved_parent: true,
            requires_independent_formal_evaluation: true,
        })
    }

    async fn load_world_nodes(
        &self,
        world_id: &str,
    ) -> Result<(ExplorationWorldV1, Vec<PersistentSearchNode>)> {
        let mut session = self.store.session().await?;
        let world: ExplorationWorldV1 =
            need_record(&mut session, &self.context, WORLD_RECORD_KIND, world_id).await?;
        world.validate()?;
        check_world_live(&mut session, &self.context, &world).await?;
        let nodes = self.load_nodes_in_session(&mut session, &world).await?;
        session.commit().await?;
        Ok((world, nodes))
    }

    async fn load_nodes_in_session(
        &self,
        session: &mut Session,
        world: &ExplorationWorldV1,
    ) -> Result<Vec<PersistentSearchNode>> {
        let mut nodes = Vec::with_capacity(world.node_ids.len());
        for node_id in &world.node_ids {
            nodes.push(need_record(session, &self.context, NODE_RECORD_KIND, node_id).await?);
        }
        Ok(nodes)
    }
}

fn reason_or_cancelled(reason: String) -> String {
    if reason.is_empty() {
        "cancelled_or_uncertain".into()
    } else {
        reason
    }
}

/// Outcome reasons of a dispatched candidate. Fixed literals: a refusal never
/// stores the gate's own error text.
const REASON_CANDIDATE_OBSERVED: &str = "candidate_observed";
const REASON_FIXTURE_EVIDENCE: &str = "fixture_evidence_not_accepted";
const REASON_EVIDENCE_REJECTED: &str = "development_evidence_rejected";
const REASON_EVIDENCE_UNVERIFIED: &str = "development_evidence_unverified";

/// The observation gate's verdict on the evidence behind a `Candidate` outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateVerdict {
    /// The gate verified the report; the quality is the mean of the scores it
    /// recomputed, not the runner's own.
    Trusted { quality_micros: u32 },
    /// The report declared Fixture provenance; the gate was not asked.
    FixtureDeclared,
    /// The gate refused the report.
    Rejected,
    /// The gate could not answer (a storage or infrastructure failure).
    Unverified,
}

impl CandidateVerdict {
    /// The node status, evidence label and outcome reason the verdict records. Only
    /// a verified report is `Valid`; a fixture or refused one is a terminal
    /// `HardFailure` (no new status: replay, strategy and the wire formats keep their
    /// vocabulary) told apart by its label; an unanswered gate is the existing
    /// uncertain path and the dispatch ends `Uncertain`.
    fn observed(self) -> (ObservedStatus, ExplorationEvidenceV1, &'static str) {
        match self {
            Self::Trusted { quality_micros } => (
                ObservedStatus::Valid { quality_micros },
                ExplorationEvidenceV1::Trusted,
                REASON_CANDIDATE_OBSERVED,
            ),
            Self::FixtureDeclared => (
                ObservedStatus::HardFailure,
                ExplorationEvidenceV1::FixtureDeclared,
                REASON_FIXTURE_EVIDENCE,
            ),
            Self::Rejected => (
                ObservedStatus::HardFailure,
                ExplorationEvidenceV1::EvidenceRejected,
                REASON_EVIDENCE_REJECTED,
            ),
            Self::Unverified => (
                ObservedStatus::UsageUncertain,
                ExplorationEvidenceV1::NotObserved,
                REASON_EVIDENCE_UNVERIFIED,
            ),
        }
    }

    /// Whether the node still names the candidate it dispatched (bundle, skill and
    /// selection digests). Every verdict that saw the candidate keeps them, for the
    /// audit trail and the revocation cleanup; an uncertain node names none, like
    /// every other uncertain node.
    fn keeps_candidate(self) -> bool {
        !matches!(self, Self::Unverified)
    }
}

/// How the gate's error maps to a verdict. A refusal of the evidence itself (no
/// report or receipt, a mismatch, a forbidden provenance, a malformed fact) rejects
/// it. A storage or infrastructure failure says nothing about the evidence, so it
/// leaves it unverified rather than rejected. Every variant is named: a new one has
/// to be decided here.
fn verdict_for_gate_error(error: &Error) -> CandidateVerdict {
    match error {
        Error::Forbidden | Error::Conflict(_) | Error::NotFound | Error::Invalid(_) => {
            CandidateVerdict::Rejected
        }
        Error::Budget | Error::Cancelled | Error::Internal => CandidateVerdict::Unverified,
    }
}

/// The mean of the per-task candidate scores over the manifest's tasks, the same
/// rule the node's quality has always used, clamped to the score range.
fn mean_quality_micros(total_micros: u64, task_count: usize) -> u32 {
    if task_count == 0 {
        return 0;
    }
    u32::try_from(total_micros / task_count as u64)
        .unwrap_or(1_000_000)
        .min(1_000_000)
}

/// The provenance the stored `DevelopmentObserved` report declared about itself.
/// `None` when the fact is absent, redacted or no report; the gate then refuses it
/// on its own terms.
async fn declared_provenance(
    session: &mut Session,
    ctx: &Context,
    observed_fact_id: &str,
) -> Result<Option<DevelopmentExecutionProvenance>> {
    let Some(value) = session
        .get::<serde_json::Value>(ctx, "artifact", observed_fact_id)
        .await?
    else {
        return Ok(None);
    };
    let Ok(fact) = serde_json::from_value::<StageFact>(value) else {
        return Ok(None);
    };
    if fact.kind != StageFactKind::DevelopmentObserved {
        return Ok(None);
    }
    Ok(serde_json::from_value::<DevelopmentRunReport>(fact.payload)
        .ok()
        .map(|report| report.provenance))
}

/// Puts the development evidence behind a `Candidate` outcome through E03's
/// observation gate, inside the caller's session. A Fixture report is not sent to
/// the gate: it can never be trusted, and the label says it was a fixture, not that
/// it was forged. Any other declared provenance is a claim, checked by the gate
/// against the persisted receipts, budget rows, recomputed scores and live sources.
/// It never returns an error: the step has been paid for and its dispatch claimed,
/// and an error here would leave that claim open for good (the dispatch id depends
/// only on the world and the action, so a retry meets "dispatch idempotency
/// conflict").
///
/// The facts are read from the store, so a journal that does not persist its facts
/// there leaves nothing to verify: the evidence is refused, not trusted.
async fn judge_candidate_evidence(
    session: &mut Session,
    ctx: &Context,
    request_fact_id: &str,
    observed_fact_id: &str,
    task_count: usize,
) -> CandidateVerdict {
    match declared_provenance(session, ctx, observed_fact_id).await {
        Ok(Some(DevelopmentExecutionProvenance::Fixture)) => {
            return CandidateVerdict::FixtureDeclared;
        }
        Ok(_) => {}
        Err(error) => return verdict_for_gate_error(&error),
    }
    match verified_development_observation_in_session(
        ctx,
        session,
        request_fact_id,
        observed_fact_id,
    )
    .await
    {
        Ok(view) => CandidateVerdict::Trusted {
            quality_micros: mean_quality_micros(
                view.outcomes
                    .iter()
                    .map(|outcome| u64::from(outcome.candidate_score_micros))
                    .sum(),
                task_count,
            ),
        },
        Err(error) => verdict_for_gate_error(&error),
    }
}

fn ensure_new_world_shape(world: &ExplorationWorldV1) -> Result<()> {
    world.validate()?;
    if world.state != WorldState::Collecting
        || !world.node_ids.is_empty()
        || !world.dispatch_ids.is_empty()
        || !world.history_ids.is_empty()
    {
        return Err(Error::Invalid(
            "new world must start empty and collecting".into(),
        ));
    }
    Ok(())
}

/// Pure decision over a read-only prefix (plan §7.1): no store access, no
/// clock, no randomness beyond the world's frozen simulation seed.
fn pure_decision(
    world: &ExplorationWorldV1,
    nodes: &[PersistentSearchNode],
) -> Result<CoordinatorDecision> {
    // The decision records the digests of the very policy and caps it passes
    // to `decide_elastic` (E14): a dispatched decision is then evidence of the
    // mechanism that produced it, not of whichever policy is stored later.
    let policy = &world.policy;
    let caps = &world.caps;
    let policy_digest = policy.digest()?;
    let caps_digest = caps.digest()?;
    if world.state != WorldState::Collecting {
        return Ok(CoordinatorDecision {
            world_id: world.id.clone(),
            prefix_digest: fingerprint(&nodes)?,
            legal_actions_digest: fingerprint(&Vec::<String>::new())?,
            policy_digest,
            caps_digest,
            action: BatchActionV1::Stop {
                reason: "world_not_collecting".into(),
            },
        });
    }
    let prefix = prefix_projection(world, nodes)?;
    let legal = derive_legal_actions(world, nodes)?;
    let budget = BudgetViewV1 {
        remaining_nodes: world.caps.max_nodes.saturating_sub(nodes.len() as u8),
        remaining_recovery_dispatches: world.remaining_recovery_dispatches,
        remaining_root_micros: world.remaining_root_micros,
    };
    let action = decide_elastic(policy, &prefix, &legal, &budget, caps, world.simulation)?;
    Ok(CoordinatorDecision {
        world_id: world.id.clone(),
        prefix_digest: fingerprint(&prefix)?,
        legal_actions_digest: fingerprint(&legal)?,
        policy_digest,
        caps_digest,
        action,
    })
}

fn prefix_projection(
    world: &ExplorationWorldV1,
    nodes: &[PersistentSearchNode],
) -> Result<PrefixViewV2> {
    let projected: Vec<_> = nodes.iter().map(|node| node.node.clone()).collect();
    let recovery_dispatches_used = projected
        .iter()
        .filter_map(|node| match node.status {
            ObservedStatus::RepairableFailure {
                dispatched_repairs, ..
            } => Some(dispatched_repairs),
            _ => None,
        })
        .fold(0u8, u8::saturating_add);
    Ok(PrefixViewV2 {
        schema_version: PrefixViewV2::SCHEMA.into(),
        context_signature: world.context_signature.clone(),
        approved_parent_digest: world.approved_parent_digest.clone(),
        initial_baseline_quality_micros: world.initial_baseline_quality_micros,
        nodes_used: projected.len() as u8,
        nodes: projected,
        current_branch_seq: world.current_branch_seq,
        current_branch_focus_actions: world.current_branch_focus_actions,
        decisions_completed: world.decision_round,
        waits: world.waits.clone(),
        recovery_dispatches_used,
    })
}

/// The actions a world offers over its stored nodes: the roots whose branch has no
/// node yet, and the successors of the nodes that can have one.
///
/// A Deepen is the successor of a trusted observation (E09: a successor is chosen
/// only after a real per-node evaluation): its parent is `Valid` and the E03 gate
/// verified the evidence behind it. A record written before the gate existed, a
/// fixture and a refused report are none of those, exactly as they are no final
/// candidate (`final_candidate_request`).
///
/// A Deepen is also offered once. Its `action_seq`, and so its dispatch id, is fixed
/// by the world and the parent, so a second selection would meet the spent dispatch
/// id and conflict before anything is paid, and the decision being deterministic the
/// world could neither advance nor stop (§7.1.1). It is spent when a node names its
/// parent as the search parent, which `run_next` writes in the commit that consumes
/// the dispatch. It is not spent when the dispatch is merely claimed: a claimed
/// Deepen has to stay legal so that it resumes (`run_next` looks the selected action
/// up again, and checks the legal digest the claim recorded once the step is paid).
fn derive_legal_actions(
    world: &ExplorationWorldV1,
    nodes: &[PersistentSearchNode],
) -> Result<LegalActionsV1> {
    let existing_branches: BTreeSet<_> = nodes.iter().map(|node| node.node.branch_seq).collect();
    // A `Valid` node is only ever the search parent of its Deepen: a Recover starts
    // from a repairable failure.
    let expanded_parents: BTreeSet<_> = nodes
        .iter()
        .filter_map(|node| node.node.search_parent_seq)
        .collect();
    let mut actions = Vec::new();
    for root in &world.root_opportunities {
        if !existing_branches.contains(&root.branch_seq) {
            actions.push(LegalActionV1 {
                action_id: format!("root-{}", root.root_slot),
                action_seq: root.action_seq,
                branch_seq: root.branch_seq,
                target_depth: 1,
                kind: ActionKindV1::Widen {
                    root_slot: root.root_slot,
                },
                estimated_cost_upper_micros: Some(root.estimated_cost_upper_micros),
            });
        }
    }
    let next_seq_base = 1_000_000u32;
    for node in nodes {
        if node.node.depth < world.caps.max_depth
            && matches!(node.node.status, ObservedStatus::Valid { .. })
            && node.evidence == ExplorationEvidenceV1::Trusted
            && !expanded_parents.contains(&node.node.node_seq)
        {
            actions.push(LegalActionV1 {
                action_id: format!("deepen-{}", node.node.node_seq),
                action_seq: next_seq_base + node.node.node_seq * 2,
                branch_seq: node.node.branch_seq,
                target_depth: node.node.depth + 1,
                kind: ActionKindV1::Deepen {
                    parent_node_seq: node.node.node_seq,
                },
                estimated_cost_upper_micros: Some(world.successor_cost_upper_micros),
            });
        }
        if let ObservedStatus::RepairableFailure {
            episode_id,
            environment_reset: true,
            dispatched_repairs,
            ..
        } = &node.node.status
            && *dispatched_repairs < world.caps.max_repair_dispatches_per_episode
        {
            actions.push(LegalActionV1 {
                action_id: format!("recover-{}", node.node.node_seq),
                action_seq: next_seq_base + node.node.node_seq * 2 + 1,
                branch_seq: node.node.branch_seq,
                target_depth: node.node.depth + 1,
                kind: ActionKindV1::Recover {
                    failed_node_seq: node.node.node_seq,
                    episode_id: episode_id.clone(),
                },
                estimated_cost_upper_micros: Some(world.successor_cost_upper_micros),
            });
        }
    }
    Ok(LegalActionsV1 {
        schema_version: "rsia.legal_actions.v1".into(),
        actions,
    })
}

fn expected_parent_binding<'a>(
    world: &'a ExplorationWorldV1,
    nodes: &'a [PersistentSearchNode],
    action: &LegalActionV1,
) -> Result<(&'a str, &'a str)> {
    match &action.kind {
        ActionKindV1::Widen { .. } => Ok((
            world.parent_skill_digest.as_str(),
            world.parent_bundle_digest.as_str(),
        )),
        ActionKindV1::Deepen { parent_node_seq } => {
            let parent = nodes
                .iter()
                .find(|node| node.node.node_seq == *parent_node_seq)
                .ok_or(Error::NotFound)?;
            Ok((
                parent.candidate_skill_digest.as_deref().ok_or_else(|| {
                    Error::Invalid("deepening parent lacks candidate skill".into())
                })?,
                parent.candidate_bundle_digest.as_deref().ok_or_else(|| {
                    Error::Invalid("deepening parent lacks candidate bundle".into())
                })?,
            ))
        }
        ActionKindV1::Recover {
            failed_node_seq, ..
        } => {
            let mut current = nodes
                .iter()
                .find(|node| node.node.node_seq == *failed_node_seq)
                .ok_or(Error::NotFound)?;
            loop {
                if let (Some(skill), Some(bundle)) = (
                    current.candidate_skill_digest.as_deref(),
                    current.candidate_bundle_digest.as_deref(),
                ) {
                    break Ok((skill, bundle));
                }
                let Some(parent_seq) = current.node.search_parent_seq else {
                    break Ok((
                        world.parent_skill_digest.as_str(),
                        world.parent_bundle_digest.as_str(),
                    ));
                };
                current = nodes
                    .iter()
                    .find(|node| node.node.node_seq == parent_seq)
                    .ok_or(Error::NotFound)?;
            }
        }
    }
}

fn validate_optimization_request_binding(
    world: &ExplorationWorldV1,
    nodes: &[PersistentSearchNode],
    action: &LegalActionV1,
    request: &OptimizationStepRequest<'_>,
) -> Result<()> {
    let (expected_skill, expected_bundle) = expected_parent_binding(world, nodes, action)?;
    let actual_parent_skill = skill_snapshot_digest(request.parent_skill)?;
    if request.model_context.episode_id != world.id
        || request.model_context.step != world.decision_round.saturating_add(1)
        || request.model_context.parent_skill_digest != expected_skill
        || actual_parent_skill != expected_skill
        || request.model_context.bundle_digest != expected_bundle
        || request.model_context.namespace != request.development_request.namespace
        || request.development_request.parent_bundle_digest != expected_bundle
        || request.development_request.episode_id != world.id
        || request.development_request.step != world.decision_round.saturating_add(1)
        || request.development_request.attempt != request.model_context.attempt
        || request.development_request.environment_digest != world.environment_digest
        || request.development_request.grader_digest != world.grader_digest
        || request.development_request.rules_digest != world.rules_digest
        || request.development_request.tools_digest != world.tools_digest
        || request.model_context.model_digest != world.model_digest
        || request.model_context.tools_digest != world.tools_digest
        || request.model_context.rules_digest != world.rules_digest
        || request.model_context.revoke_watermark != world.source_watermark
        || request.development_request.revoke_watermark != world.source_watermark
    {
        return Err(Error::Conflict(
            "optimization request does not bind the selected exploration parent/context".into(),
        ));
    }
    let dependency_ids: BTreeSet<_> = world
        .dependencies
        .iter()
        .map(|dependency| dependency.id.as_str())
        .collect();
    if request
        .source_selection
        .run_ids
        .iter()
        .any(|source| !dependency_ids.contains(source.as_str()))
    {
        return Err(Error::Conflict(
            "optimization source selection is outside the world dependency closure".into(),
        ));
    }
    Ok(())
}

fn update_waits(
    world: &mut ExplorationWorldV1,
    selected_action_seq: u32,
    available_action_seqs: &[u32],
) {
    world
        .waits
        .retain(|wait| available_action_seqs.contains(&wait.action_seq));
    for action_seq in available_action_seqs {
        if *action_seq == selected_action_seq {
            continue;
        }
        if let Some(wait) = world
            .waits
            .iter_mut()
            .find(|wait| wait.action_seq == *action_seq)
        {
            wait.waited_rounds = wait.waited_rounds.saturating_add(1);
        } else {
            world.waits.push(OpportunityWait {
                action_seq: *action_seq,
                waited_rounds: 1,
            });
        }
    }
    world.waits.sort_by_key(|wait| wait.action_seq);
}

async fn check_world_live(
    session: &mut Session,
    context: &Context,
    world: &ExplorationWorldV1,
) -> Result<()> {
    let watermark = session
        .watermark(context)
        .await?
        .and_then(|value| u64::try_from(value.0).ok())
        .ok_or_else(|| Error::Conflict("missing exploration source watermark".into()))?;
    if watermark != world.source_watermark {
        return Err(Error::Conflict(
            "exploration world source watermark changed".into(),
        ));
    }
    let source_ids = world
        .dependencies
        .iter()
        .map(|dependency| dependency.id.clone())
        .collect::<Vec<_>>();
    validate_stored_sources(session, context, &source_ids, world.source_watermark).await
}

/// Read-side view for the management `status` of an `exploration.start` job:
/// reloads the world through its live source closure (watermark drift is
/// `Conflict`, a tombstoned dependency is `Forbidden`, a missing source fails
/// closed) and re-runs the pure decision. The stored world is never rewritten.
pub async fn verified_world_decision_view(
    ctx: &Context,
    store: &Store,
    world_id: &str,
) -> Result<WorldDecisionView> {
    ctx.require(&[Role::Admin])?;
    identifier(world_id)?;
    let coordinator = PersistentCoordinator::new(store.clone(), ctx.clone(), ctx.actor())?;
    let view = coordinator.decision_view(world_id).await?;
    if view.decision.world_id != world_id {
        return Err(Error::Conflict(
            "stored exploration world differs from its storage identity".into(),
        ));
    }
    Ok(view)
}

/// What the management `status` of an `exploration.start` job reads of a stored
/// world besides its decision: the world read through its live source closure,
/// the digests its registration froze, and whether it moved past its
/// registration. A derived, read-only view; nothing is written.
#[derive(Debug, Clone)]
pub struct WorldRegistrationView {
    /// The stored world. It validated, it carries the id it is stored under and
    /// its source closure is live.
    pub registered_world: ExplorationWorldV1,
    pub context_signature: String,
    /// Digest of the world's frozen `ElasticPolicyV1`.
    pub policy_digest: String,
    /// Digest of the world's frozen `ExplorationCapsV1`.
    pub caps_digest: String,
    /// Whether a dispatch was claimed, a node written or a decision round taken:
    /// the stored world has moved on and is no longer the registered one (its
    /// budget, nodes, waits and the live decision are what `run_next` made of
    /// them). A world that has not started is exactly what was registered.
    pub started: bool,
}

/// Read-side view for the management `status` of an `exploration.start` job.
/// Unlike [`verified_world_decision_view`] it does not re-run the decision on the
/// stored prefix: it returns what a dispatch never changes (the world's id,
/// context signature and the digests of its frozen policy and caps) and whether
/// the world started, so the caller decides which first decision the stored
/// result is compared with. The world is read through its live source closure
/// (watermark drift is `Conflict`, a tombstoned dependency is `Forbidden`, a
/// missing source fails closed) and the stored world is never rewritten.
pub async fn verified_world_registration_view(
    ctx: &Context,
    store: &Store,
    world_id: &str,
) -> Result<WorldRegistrationView> {
    ctx.require(&[Role::Admin])?;
    identifier(world_id)?;
    let coordinator = PersistentCoordinator::new(store.clone(), ctx.clone(), ctx.actor())?;
    let registered_world = coordinator.registered_world(world_id).await?;
    Ok(WorldRegistrationView {
        context_signature: registered_world.context_signature.clone(),
        policy_digest: registered_world.policy.digest()?,
        caps_digest: registered_world.caps.digest()?,
        started: world_started(&registered_world),
        registered_world,
    })
}

/// Read-side check for the management `status` of an `exploration.start` job
/// whose world started: the world stored under the id of `requested` (the world
/// in the job's private input), read through its live source closure, is still
/// the registration `requested` describes. It is the comparison registration
/// makes (`ensure_registered_as`): the fingerprint of the immutable registration
/// facts and the budget the world was registered with, exactly. A started world
/// has moved on, so its decision cannot be compared, but these two cannot move.
/// A mismatch, and dispatch facts that do not account for the world, are a
/// `Conflict`; nothing is written.
pub(crate) async fn ensure_world_registered_as(
    ctx: &Context,
    store: &Store,
    requested: &ExplorationWorldV1,
) -> Result<()> {
    ctx.require(&[Role::Admin])?;
    let mut session = store.session().await?;
    let stored: ExplorationWorldV1 =
        need_record(&mut session, ctx, WORLD_RECORD_KIND, &requested.id).await?;
    stored.validate()?;
    check_world_live(&mut session, ctx, &stored).await?;
    ensure_registered_as(&mut session, ctx, &stored, requested).await?;
    session.commit().await
}

/// Whether a stored world moved past its registration: a dispatch is claimed or a
/// node is written, or a decision round was taken. Until then the stored world is
/// what was registered.
fn world_started(world: &ExplorationWorldV1) -> bool {
    !(world.node_ids.is_empty() && world.dispatch_ids.is_empty() && world.decision_round == 0)
}

/// The first pure decision of `world` as it was registered: the decision over an
/// empty prefix (plan §7.1), a function of the world's registration alone. After a
/// dispatch it is not the live decision (the prefix, the budget and the waits
/// moved), so a consumer that has to report or re-verify the first decision takes
/// it from the world as it was requested.
pub(crate) fn first_decision(world: &ExplorationWorldV1) -> Result<CoordinatorDecision> {
    world.validate()?;
    pure_decision(world, &[])
}

/// The budget a stored world was registered with, rebuilt from the facts
/// `run_next` left behind: `(remaining_root_micros,
/// remaining_recovery_dispatches)`.
///
/// `run_next` spends the two counters in the one commit that completes a dispatch
/// (its node is written and its fact leaves `Claimed`): the cost the dispatched
/// decision announced, and one recovery dispatch when the selected action is a
/// `Recover`. Nothing else moves them. The registered value is therefore the
/// stored one plus exactly what the world's completed dispatch facts say was
/// spent; a claimed dispatch has spent nothing yet, and for a world nothing was
/// dispatched in it is the stored value itself.
///
/// The facts have to account for the world: each is listed once, belongs to this
/// world, names the one action its decision dispatched and selected, and a
/// completed one names a node the world lists, with as many completed facts as
/// nodes. Anything else leaves no way to tell a spend from a counter someone
/// lowered, so the registered budget is not rebuilt (`Conflict`), never guessed.
///
/// This function states `run_next`'s spending rule from the reading side: a
/// change to when or by how much `run_next` spends has to change it with it
/// (`tests/exploration_start_after_dispatch_v42.rs` pins both).
async fn registered_budget(
    session: &mut Session,
    ctx: &Context,
    world: &ExplorationWorldV1,
) -> Result<(u64, u8)> {
    let unaccounted = || {
        Error::Conflict(
            "the dispatch facts of the exploration world do not account for its budget".into(),
        )
    };
    let mut root_micros = world.remaining_root_micros;
    let mut recovery_dispatches = u32::from(world.remaining_recovery_dispatches);
    let mut listed = BTreeSet::new();
    let mut completed_nodes = BTreeSet::new();
    for dispatch_id in &world.dispatch_ids {
        if !listed.insert(dispatch_id.as_str()) {
            return Err(unaccounted());
        }
        let fact = get_dispatch_fact(session, ctx, dispatch_id)
            .await?
            .ok_or_else(unaccounted)?;
        let BatchActionV1::Dispatch {
            action_ids,
            action_seqs,
            estimated_cost_upper_micros,
            ..
        } = &fact.decision.action
        else {
            return Err(unaccounted());
        };
        if fact.id != *dispatch_id
            || fact.world_id != world.id
            || fact.decision.world_id != world.id
            || fact.context_signature != world.context_signature
            || action_ids.len() != 1
            || action_seqs.len() != 1
            || action_ids[0] != fact.action_id
            || action_seqs[0] != fact.action_seq
            || fact.selected_action.action_id != fact.action_id
            || fact.selected_action.action_seq != fact.action_seq
        {
            return Err(unaccounted());
        }
        // Every state is named, so a state added later has to be decided here:
        // whether it spent the dispatch's budget is the whole question.
        match fact.state {
            // Claimed and not completed: its node is not written, nothing spent.
            ExplorationDispatchState::Claimed => {
                if fact.node_id.is_some() {
                    return Err(unaccounted());
                }
            }
            ExplorationDispatchState::Observed | ExplorationDispatchState::Uncertain => {
                let Some(node_id) = fact.node_id.as_deref() else {
                    return Err(unaccounted());
                };
                if !world.node_ids.iter().any(|listed| listed == node_id)
                    || !completed_nodes.insert(node_id.to_owned())
                {
                    return Err(unaccounted());
                }
                root_micros = root_micros
                    .checked_add(*estimated_cost_upper_micros)
                    .ok_or_else(unaccounted)?;
                if matches!(fact.selected_action.kind, ActionKindV1::Recover { .. }) {
                    recovery_dispatches += 1;
                }
            }
        }
    }
    if completed_nodes.len() != world.node_ids.len() {
        return Err(unaccounted());
    }
    Ok((
        root_micros,
        u8::try_from(recovery_dispatches).map_err(|_| unaccounted())?,
    ))
}

/// The comparison registration and the management `status` of a started world
/// share, read-only: `stored` is still the registration `requested` describes.
/// It has the same id and the same registration fingerprint, and the budget it
/// was registered with, rebuilt from its dispatch facts (`registered_budget`),
/// is the one `requested` declares. Anything else is a `Conflict` with a fixed
/// message, and so are facts that do not account for the world.
async fn ensure_registered_as(
    session: &mut Session,
    ctx: &Context,
    stored: &ExplorationWorldV1,
    requested: &ExplorationWorldV1,
) -> Result<()> {
    if stored.id != requested.id
        || stored.registration_fingerprint()? != requested.registration_fingerprint()?
    {
        return Err(Error::Conflict(
            "exploration world already exists with a different registration".into(),
        ));
    }
    if registered_budget(session, ctx, stored).await?
        != (
            requested.remaining_root_micros,
            requested.remaining_recovery_dispatches,
        )
    {
        return Err(Error::Conflict(
            "exploration world already exists with a different registered budget".into(),
        ));
    }
    Ok(())
}

/// Storage id of a persisted exploration world envelope.
pub fn exploration_world_storage_id(world_id: &str) -> Result<String> {
    storage_id(WORLD_RECORD_KIND, world_id)
}

impl PersistentCoordinator {
    /// Reads one registered world through its live source closure. The stored
    /// world must validate and carry the id it is stored under, and its source
    /// closure must still be live: watermark drift is `Conflict`, a tombstoned
    /// source is `Forbidden`, a missing source fails closed, and a redacted
    /// world is named by the read (`Conflict`). Read-only: nothing is written
    /// and no model is reached. It is the read half the MetaTrial gate
    /// (`meta.rs`) builds on.
    pub(crate) async fn registered_world(&self, id: &str) -> Result<ExplorationWorldV1> {
        identifier(id)?;
        let mut session = self.store.session().await?;
        let world: ExplorationWorldV1 =
            need_record(&mut session, &self.context, WORLD_RECORD_KIND, id).await?;
        world.validate()?;
        if world.id != id {
            return Err(Error::Conflict(
                "stored exploration world differs from its storage identity".into(),
            ));
        }
        check_world_live(&mut session, &self.context, &world).await?;
        session.commit().await?;
        Ok(world)
    }
}

pub(crate) fn storage_id(record_kind: &str, id: &str) -> Result<String> {
    identifier(record_kind)?;
    identifier(id)?;
    Ok(format!("e09-{}", fingerprint(&(record_kind, id))?))
}

/// Reads the stored body of one exploration record as an envelope.
///
/// After a source revocation the cleanup replaces the body with an
/// `rsia.redacted.v1` tombstone, which is not an envelope. That is the expected
/// state of a revoked world, not corruption, so the read names it: a `Conflict`
/// carrying the record kind and id, the verdict E03's development artifacts give
/// the same state (`development.rs`, `get_record`), instead of a decode failure
/// that would surface as `Internal`. The body is inspected before it is decoded,
/// so the expected state does not raise the storage layer's "database operation
/// failed" error log either. Any other body that does not decode is corruption
/// and stays `Internal`.
async fn read_envelope<T: DeserializeOwned>(
    session: &mut Session,
    ctx: &Context,
    record_kind: &str,
    id: &str,
    storage_id: &str,
) -> Result<Option<ArtifactEnvelope<T>>> {
    let Some(body) = session
        .get::<serde_json::Value>(ctx, "artifact", storage_id)
        .await?
    else {
        return Ok(None);
    };
    if body
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
        == Some(REDACTED_SCHEMA)
    {
        return Err(Error::Conflict(format!(
            "exploration record {record_kind} {id} was redacted because its source was revoked"
        )));
    }
    match serde_json::from_value(body) {
        Ok(envelope) => Ok(Some(envelope)),
        Err(error) => {
            tracing::error!(%error, record_kind, id, "stored exploration record does not decode");
            Err(Error::Internal)
        }
    }
}

pub(crate) async fn get_record<T: DeserializeOwned>(
    session: &mut Session,
    ctx: &Context,
    record_kind: &str,
    id: &str,
) -> Result<Option<T>> {
    let storage_id = storage_id(record_kind, id)?;
    let envelope = read_envelope::<T>(session, ctx, record_kind, id, &storage_id).await?;
    match envelope {
        Some(envelope)
            if envelope.schema_version == ENVELOPE_SCHEMA
                && envelope.id == storage_id
                && envelope.record_kind == record_kind =>
        {
            Ok(Some(envelope.payload))
        }
        Some(_) => Err(Error::Conflict(
            "exploration artifact envelope mismatch".into(),
        )),
        None => Ok(None),
    }
}

pub(crate) async fn need_record<T: DeserializeOwned>(
    session: &mut Session,
    ctx: &Context,
    record_kind: &str,
    id: &str,
) -> Result<T> {
    get_record(session, ctx, record_kind, id)
        .await?
        .ok_or(Error::NotFound)
}

/// Reads a dispatch fact. The envelope is checked like every other record, but
/// the payload is inspected as JSON first so a pre-E14 (v1) fact is refused by
/// name instead of failing as an opaque shape error: v1 carries no policy or
/// caps digest and is never reinterpreted as v2.
async fn get_dispatch_fact(
    session: &mut Session,
    ctx: &Context,
    id: &str,
) -> Result<Option<ExplorationDispatchFact>> {
    let storage_id = storage_id(DISPATCH_RECORD_KIND, id)?;
    let envelope =
        read_envelope::<serde_json::Value>(session, ctx, DISPATCH_RECORD_KIND, id, &storage_id)
            .await?;
    match envelope {
        Some(envelope)
            if envelope.schema_version == ENVELOPE_SCHEMA
                && envelope.id == storage_id
                && envelope.record_kind == DISPATCH_RECORD_KIND =>
        {
            decode_dispatch_fact(envelope.payload).map(Some)
        }
        Some(_) => Err(Error::Conflict(
            "exploration artifact envelope mismatch".into(),
        )),
        None => Ok(None),
    }
}

fn decode_dispatch_fact(payload: serde_json::Value) -> Result<ExplorationDispatchFact> {
    match payload
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
    {
        Some(DISPATCH_SCHEMA_V1) => {
            return Err(Error::Conflict(
                "pre-E14 dispatch fact without policy digest".into(),
            ));
        }
        Some(DISPATCH_SCHEMA) => {}
        _ => {
            return Err(Error::Conflict(
                "unsupported exploration dispatch fact schema".into(),
            ));
        }
    }
    serde_json::from_value(payload).map_err(|_| Error::Internal)
}

async fn need_dispatch_fact(
    session: &mut Session,
    ctx: &Context,
    id: &str,
) -> Result<ExplorationDispatchFact> {
    get_dispatch_fact(session, ctx, id)
        .await?
        .ok_or(Error::NotFound)
}

pub(crate) async fn put_record<T: Serialize>(
    session: &mut Session,
    ctx: &Context,
    record_kind: &str,
    id: &str,
    owner: &str,
    payload: &T,
) -> Result<()> {
    let storage_id = storage_id(record_kind, id)?;
    session
        .put(
            ctx,
            "artifact",
            &storage_id,
            owner,
            &ArtifactEnvelope {
                schema_version: ENVELOPE_SCHEMA.into(),
                id: storage_id.clone(),
                record_kind: record_kind.into(),
                payload,
            },
        )
        .await
}

/// Ties one exploration record to the world that owns it with the dependency
/// edge `record -> world` (both are `artifact` objects, addressed by their
/// storage ids). Revocation cleanup follows `dependents`, so this is the edge
/// that carries a revoked run from the world to the record. `put_edge` is an
/// `INSERT OR IGNORE`: repeating the call never adds a second edge.
///
/// Callers write it in the session that first writes the record. A record
/// written before this edge existed has none, and nothing back-fills it.
async fn put_world_edge(
    session: &mut Session,
    ctx: &Context,
    record_kind: &str,
    record_id: &str,
    world_id: &str,
) -> Result<()> {
    session
        .put_edge(
            ctx,
            "artifact",
            &storage_id(record_kind, record_id)?,
            "artifact",
            &storage_id(WORLD_RECORD_KIND, world_id)?,
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use evo_core::strategy::ExplorationPolicy;

    fn prefix() -> PrefixView {
        PrefixView {
            search_parent: "s1".into(),
            approved_parent: "a1".into(),
            depth: 1,
        }
    }

    #[test]
    fn w_greater_than_one_rejected() {
        let p = ExplorationPolicy {
            w: 2,
            ..Default::default()
        };
        assert!(Coordinator::new(p).is_err());
    }

    #[test]
    fn must_eval_before_decide_and_cannot_publish_mid_search() {
        let mut c = Coordinator::new(ExplorationPolicy::default()).unwrap();
        c.generate("n1".into(), prefix()).unwrap();
        assert!(c.decide("n1").is_err());
        c.execute_dev("n1").unwrap();
        c.observe("n1").unwrap();
        c.decide("n1").unwrap();
        assert!(c.publish("n1").is_err());
    }

    #[test]
    fn legacy_search_parent_is_not_approved_parent() {
        let p = PrefixView {
            search_parent: "same".into(),
            approved_parent: "same".into(),
            depth: 1,
        };
        let mut c = Coordinator::new(ExplorationPolicy::default()).unwrap();
        assert!(c.generate("n1".into(), p).is_err());
    }

    // ----- AG-033: the observation gate and the evidence label -----

    fn verdicts() -> [CandidateVerdict; 4] {
        [
            CandidateVerdict::Trusted {
                quality_micros: 750_000,
            },
            CandidateVerdict::FixtureDeclared,
            CandidateVerdict::Rejected,
            CandidateVerdict::Unverified,
        ]
    }

    #[test]
    fn a_gate_error_rejects_the_evidence_or_leaves_it_unverified() {
        // The evidence itself was refused: missing, mismatching, forbidden or
        // malformed.
        for error in [
            Error::Forbidden,
            Error::Conflict("receipt differs".into()),
            Error::NotFound,
            Error::Invalid("not a report".into()),
        ] {
            assert_eq!(
                verdict_for_gate_error(&error),
                CandidateVerdict::Rejected,
                "{error:?}"
            );
        }
        // The gate could not answer: that is no verdict on the evidence.
        for error in [Error::Internal, Error::Budget, Error::Cancelled] {
            assert_eq!(
                verdict_for_gate_error(&error),
                CandidateVerdict::Unverified,
                "{error:?}"
            );
        }
    }

    #[test]
    fn only_a_verified_report_is_a_valid_node_and_every_refusal_is_terminal() {
        let [trusted, fixture, rejected, unverified] = verdicts().map(CandidateVerdict::observed);
        assert!(matches!(
            trusted.0,
            ObservedStatus::Valid {
                quality_micros: 750_000
            }
        ));
        assert_eq!(trusted.1, ExplorationEvidenceV1::Trusted);
        assert_eq!(trusted.2, "candidate_observed");
        // A fixture and a refused report share the status the strategy already knows
        // and are told apart by the label and the fixed reason.
        assert!(matches!(fixture.0, ObservedStatus::HardFailure));
        assert_eq!(fixture.1, ExplorationEvidenceV1::FixtureDeclared);
        assert_eq!(fixture.2, "fixture_evidence_not_accepted");
        assert!(matches!(rejected.0, ObservedStatus::HardFailure));
        assert_eq!(rejected.1, ExplorationEvidenceV1::EvidenceRejected);
        assert_eq!(rejected.2, "development_evidence_rejected");
        // An unanswered gate is the uncertain path: the dispatch ends uncertain, and
        // nothing claims the evidence was seen.
        assert!(matches!(unverified.0, ObservedStatus::UsageUncertain));
        assert_eq!(unverified.1, ExplorationEvidenceV1::NotObserved);
        assert_eq!(unverified.2, "development_evidence_unverified");
        // Only the verdicts that saw the candidate keep naming it.
        assert_eq!(
            verdicts().map(CandidateVerdict::keeps_candidate),
            [true, true, true, false]
        );
    }

    #[test]
    fn the_node_quality_is_the_mean_over_the_manifest_clamped_to_the_score_range() {
        assert_eq!(mean_quality_micros(0, 0), 0);
        assert_eq!(mean_quality_micros(900_000, 1), 900_000);
        assert_eq!(mean_quality_micros(1_000_000, 2), 500_000);
        assert_eq!(mean_quality_micros(1_000_001, 2), 500_000);
        assert_eq!(mean_quality_micros(2_000_000, 2), 1_000_000);
        assert_eq!(mean_quality_micros(5_000_000, 2), 1_000_000);
        assert_eq!(mean_quality_micros(u64::MAX, 1), 1_000_000);
    }

    #[test]
    fn evidence_labels_keep_their_wire_names_and_default_to_not_observed() {
        assert_eq!(
            ExplorationEvidenceV1::default(),
            ExplorationEvidenceV1::NotObserved
        );
        for (label, name) in [
            (ExplorationEvidenceV1::Trusted, "trusted"),
            (ExplorationEvidenceV1::FixtureDeclared, "fixture_declared"),
            (ExplorationEvidenceV1::EvidenceRejected, "evidence_rejected"),
            (ExplorationEvidenceV1::NotObserved, "not_observed"),
        ] {
            assert_eq!(label.as_str(), name);
            assert_eq!(serde_json::to_value(label).unwrap(), name);
            assert_eq!(
                serde_json::from_value::<ExplorationEvidenceV1>(serde_json::json!(name)).unwrap(),
                label
            );
        }
        assert!(
            serde_json::from_value::<ExplorationEvidenceV1>(serde_json::json!("verified")).is_err()
        );
    }

    fn sample_node() -> PersistentSearchNode {
        PersistentSearchNode {
            schema_version: "rsia.exploration_node.v1".into(),
            world_id: "world-1".into(),
            node: PrefixNodeV2 {
                node_seq: 1,
                branch_seq: 1,
                search_parent_seq: None,
                approved_parent_digest: evo_core::hash(b"approved-parent"),
                depth: 1,
                status: ObservedStatus::HardFailure,
                best_valid_ancestor_micros: Some(500_000),
                recent_valid_gains_micros: vec![],
                repair_failures_dispatched: 0,
            },
            candidate_bundle_digest: Some(evo_core::hash(b"bundle")),
            candidate_skill_digest: Some(evo_core::hash(b"skill")),
            development_selection_digest: Some(evo_core::hash(b"selection")),
            intermediate_only: true,
            evidence: ExplorationEvidenceV1::FixtureDeclared,
        }
    }

    fn sample_dispatch_fact() -> ExplorationDispatchFact {
        let action = LegalActionV1 {
            action_id: "root-1".into(),
            action_seq: 1,
            branch_seq: 1,
            target_depth: 1,
            kind: ActionKindV1::Widen { root_slot: 1 },
            estimated_cost_upper_micros: Some(10),
        };
        ExplorationDispatchFact {
            schema_version: DISPATCH_SCHEMA.into(),
            id: "dispatch-1".into(),
            world_id: "world-1".into(),
            action_id: "root-1".into(),
            action_seq: 1,
            request_digest: evo_core::hash(b"request"),
            idempotency_key: "request-1".into(),
            decision: CoordinatorDecision {
                world_id: "world-1".into(),
                prefix_digest: evo_core::hash(b"prefix"),
                legal_actions_digest: evo_core::hash(b"legal"),
                policy_digest: evo_core::hash(b"policy"),
                caps_digest: evo_core::hash(b"caps"),
                action: BatchActionV1::Dispatch {
                    action_ids: vec!["root-1".into()],
                    action_seqs: vec![1],
                    reason: "first_preregistered_root".into(),
                    estimated_cost_upper_micros: 10,
                },
            },
            selected_action: action,
            expected_parent_skill_digest: evo_core::hash(b"parent-skill"),
            expected_parent_bundle_digest: evo_core::hash(b"parent-bundle"),
            context_signature: evo_core::hash(b"context"),
            state: ExplorationDispatchState::Observed,
            node_id: Some("node-world-1-1".into()),
            outcome_reason: Some("fixture_evidence_not_accepted".into()),
            evidence: ExplorationEvidenceV1::FixtureDeclared,
        }
    }

    #[test]
    fn a_node_and_a_dispatch_fact_stored_before_the_label_read_as_not_observed() {
        let mut node = serde_json::to_value(sample_node()).unwrap();
        assert_eq!(node["evidence"], "fixture_declared");
        node.as_object_mut().unwrap().remove("evidence");
        let node: PersistentSearchNode = serde_json::from_value(node).unwrap();
        assert_eq!(node.evidence, ExplorationEvidenceV1::NotObserved);

        let mut fact = serde_json::to_value(sample_dispatch_fact()).unwrap();
        assert_eq!(fact["evidence"], "fixture_declared");
        fact.as_object_mut().unwrap().remove("evidence");
        let fact = decode_dispatch_fact(fact).unwrap();
        assert_eq!(fact.evidence, ExplorationEvidenceV1::NotObserved);
        // The schema is the same v2: the label is an addition, not a new version.
        assert_eq!(fact.schema_version, DISPATCH_SCHEMA);

        // The shape stays strict: a label that is not one of the four, and a field
        // that is not part of the record, are still refused.
        let mut forged = serde_json::to_value(sample_node()).unwrap();
        forged["evidence"] = serde_json::json!("verified");
        assert!(serde_json::from_value::<PersistentSearchNode>(forged).is_err());
        let mut unknown = serde_json::to_value(sample_node()).unwrap();
        unknown["trusted"] = serde_json::json!(true);
        assert!(serde_json::from_value::<PersistentSearchNode>(unknown).is_err());
    }

    fn model_context() -> evo_core::optimization::ModelRequestContext {
        evo_core::optimization::ModelRequestContext {
            request_id: "optimizer-1".into(),
            namespace: "n".into(),
            purpose: evo_core::evidence::Purpose::Development,
            stage: evo_core::optimization::ModelStage::ReflectFailure,
            episode_id: "world-1".into(),
            step: 1,
            attempt: 1,
            parent_skill_digest: evo_core::hash(b"parent-skill"),
            bundle_digest: evo_core::hash(b"parent-bundle"),
            source_closure: vec![],
            model_digest: evo_core::hash(b"model"),
            tools_digest: evo_core::hash(b"tools"),
            rules_digest: evo_core::hash(b"rules"),
            sampling_digest: evo_core::hash(b"sampling"),
            revoke_watermark: 1,
            max_suggestions: 4,
        }
    }

    fn development_request() -> crate::optimization::DevelopmentRunRequest {
        crate::optimization::DevelopmentRunRequest {
            request_id: "dev-1".into(),
            namespace: "n".into(),
            purpose: evo_core::evidence::Purpose::Development,
            episode_id: "world-1".into(),
            step: 1,
            attempt: 1,
            manifest: crate::optimization::DevelopmentManifest::build(
                "manifest",
                vec![crate::optimization::DevelopmentTask {
                    id: "task".into(),
                    parent_family: "family".into(),
                    input_digest: evo_core::hash(b"task"),
                }],
            )
            .unwrap(),
            parent_bundle_digest: evo_core::hash(b"parent-bundle"),
            candidate_bundle_digest: evo_core::hash(b"candidate-bundle"),
            environment_digest: evo_core::hash(b"environment"),
            grader_digest: evo_core::hash(b"grader"),
            rules_digest: evo_core::hash(b"rules"),
            tools_digest: evo_core::hash(b"tools"),
            revoke_watermark: 1,
            idempotency_key: "dev-idempotency-1".into(),
        }
    }

    #[test]
    fn the_stage_fact_pair_follows_the_request_the_caller_built() {
        let context = model_context();
        let request = development_request();
        let (request_fact, observed_fact) = development_stage_fact_ids(&context, &request).unwrap();
        assert!(request_fact.starts_with("optstage-"));
        assert!(observed_fact.starts_with("optstage-"));
        assert_ne!(request_fact, observed_fact, "two facts, one stage");
        assert_eq!(
            development_stage_fact_ids(&context, &request).unwrap(),
            (request_fact.clone(), observed_fact.clone()),
            "a pure function of the request"
        );

        // What the step rewrites (the candidate bundle and the idempotency key) or
        // validates elsewhere is not part of the pair's identity.
        let mut rewritten = request.clone();
        rewritten.candidate_bundle_digest = evo_core::hash(b"another-candidate");
        rewritten.idempotency_key = "another-key".into();
        assert_eq!(
            development_stage_fact_ids(&context, &rewritten).unwrap(),
            (request_fact.clone(), observed_fact.clone())
        );

        // Everything that names the stage does move it.
        type Edit = fn(
            &mut evo_core::optimization::ModelRequestContext,
            &mut crate::optimization::DevelopmentRunRequest,
        );
        let edits: [Edit; 6] = [
            |context, _| context.namespace = "m".into(),
            |context, _| context.episode_id = "world-2".into(),
            |context, _| context.step = 2,
            |context, _| context.attempt = 2,
            |context, _| context.request_id = "optimizer-2".into(),
            |_, request| request.request_id = "dev-2".into(),
        ];
        let mut moved = Vec::new();
        for edit in edits {
            let mut context = model_context();
            let mut request = development_request();
            edit(&mut context, &mut request);
            let ids = development_stage_fact_ids(&context, &request).unwrap();
            assert_ne!(ids.0, request_fact);
            assert_ne!(ids.1, observed_fact);
            moved.push(ids);
        }
        let distinct: BTreeSet<_> = moved.iter().collect();
        assert_eq!(
            distinct.len(),
            moved.len(),
            "each input moves it its own way"
        );
    }
}
