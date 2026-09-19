//! generate → execute_dev → observe → decide. Intermediate nodes are not published.
use crate::evidence::validate_stored_sources;
use crate::model::ModelPort;
use crate::optimization::{
    DevRunner, OptimizationJournal, OptimizationStepOutcome, OptimizationStepRequest,
    optimization_request_digest, run_optimization_step,
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

const WORLD_RECORD_KIND: &str = "exploration_world_v1";
const NODE_RECORD_KIND: &str = "exploration_node_v1";
const DISPATCH_RECORD_KIND: &str = "exploration_dispatch_v1";
const HISTORY_RECORD_KIND: &str = "optimization_history_v1";
const ENVELOPE_SCHEMA: &str = "rsia.exploration_artifact_envelope.v1";

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoordinatorDecision {
    pub world_id: String,
    pub prefix_digest: String,
    pub legal_actions_digest: String,
    pub action: BatchActionV1,
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
        put_record(
            &mut session,
            &self.context,
            WORLD_RECORD_KIND,
            &world.id,
            &self.owner,
            &world,
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
        session.commit().await
    }

    pub async fn decide_next(&self, world_id: &str) -> Result<CoordinatorDecision> {
        let (world, nodes) = self.load_world_nodes(world_id).await?;
        if world.state != WorldState::Collecting {
            return Ok(CoordinatorDecision {
                world_id: world.id,
                prefix_digest: fingerprint(&nodes)?,
                legal_actions_digest: fingerprint(&Vec::<String>::new())?,
                action: BatchActionV1::Stop {
                    reason: "world_not_collecting".into(),
                },
            });
        }
        let prefix = prefix_projection(&world, &nodes)?;
        let legal = derive_legal_actions(&world, &nodes)?;
        let budget = BudgetViewV1 {
            remaining_nodes: world.caps.max_nodes.saturating_sub(nodes.len() as u8),
            remaining_recovery_dispatches: world.remaining_recovery_dispatches,
            remaining_root_micros: world.remaining_root_micros,
        };
        let action = decide_elastic(
            &world.policy,
            &prefix,
            &legal,
            &budget,
            &world.caps,
            world.simulation,
        )?;
        Ok(CoordinatorDecision {
            world_id: world.id,
            prefix_digest: fingerprint(&prefix)?,
            legal_actions_digest: fingerprint(&legal)?,
            action,
        })
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
            let dispatch: ExplorationDispatchFact = need_record(
                &mut session,
                &self.context,
                DISPATCH_RECORD_KIND,
                dispatch_id,
            )
            .await?;
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
        let mut dispatch = match get_record::<ExplorationDispatchFact>(
            &mut session,
            &self.context,
            DISPATCH_RECORD_KIND,
            &dispatch_id,
        )
        .await?
        {
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
                let fact = ExplorationDispatchFact {
                    schema_version: "rsia.exploration_dispatch.v1".into(),
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
        dispatch = need_record(
            &mut session,
            &self.context,
            DISPATCH_RECORD_KIND,
            &dispatch_id,
        )
        .await?;
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
        let (status, skill_digest, bundle_digest, selection_digest, outcome_reason) = match outcome
        {
            Ok(OptimizationStepOutcome::Candidate {
                edit,
                bundle,
                selection,
            }) => {
                let quality = if task_count == 0 {
                    0
                } else {
                    u32::try_from(selection.candidate_total_micros / task_count as u64)
                        .unwrap_or(1_000_000)
                };
                (
                    ObservedStatus::Valid {
                        quality_micros: quality.min(1_000_000),
                    },
                    Some(skill_snapshot_digest(&edit.output)?),
                    Some(bundle.digest.clone()),
                    Some(fingerprint(&selection)?),
                    "candidate_observed".to_string(),
                )
            }
            Ok(OptimizationStepOutcome::NoChange { reason }) => {
                (ObservedStatus::HardFailure, None, None, None, reason)
            }
            Ok(OptimizationStepOutcome::Rejected { reason }) => {
                (ObservedStatus::HardFailure, None, None, None, reason)
            }
            Ok(OptimizationStepOutcome::Uncertain { reason }) => (
                ObservedStatus::UsageUncertain,
                None,
                None,
                None,
                reason_or_cancelled(reason),
            ),
            Err(Error::Cancelled) => (
                ObservedStatus::UsageUncertain,
                None,
                None,
                None,
                "cancelled_or_uncertain".into(),
            ),
            Err(error) => (
                ObservedStatus::UsageUncertain,
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
        let best_valid_ancestor_micros = match (&status, previous_quality) {
            (ObservedStatus::Valid { quality_micros }, Some(parent)) => {
                Some((*quality_micros).max(parent))
            }
            (_, parent) => parent,
        };
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
        put_record(
            &mut session,
            &self.context,
            NODE_RECORD_KIND,
            &node_id,
            &self.owner,
            &node,
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

fn derive_legal_actions(
    world: &ExplorationWorldV1,
    nodes: &[PersistentSearchNode],
) -> Result<LegalActionsV1> {
    let existing_branches: BTreeSet<_> = nodes.iter().map(|node| node.node.branch_seq).collect();
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

fn storage_id(record_kind: &str, id: &str) -> Result<String> {
    identifier(record_kind)?;
    identifier(id)?;
    Ok(format!("e09-{}", fingerprint(&(record_kind, id))?))
}

async fn get_record<T: DeserializeOwned>(
    session: &mut Session,
    ctx: &Context,
    record_kind: &str,
    id: &str,
) -> Result<Option<T>> {
    let storage_id = storage_id(record_kind, id)?;
    let envelope = session
        .get::<ArtifactEnvelope<T>>(ctx, "artifact", &storage_id)
        .await?;
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

async fn need_record<T: DeserializeOwned>(
    session: &mut Session,
    ctx: &Context,
    record_kind: &str,
    id: &str,
) -> Result<T> {
    get_record(session, ctx, record_kind, id)
        .await?
        .ok_or(Error::NotFound)
}

async fn put_record<T: Serialize>(
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
}
