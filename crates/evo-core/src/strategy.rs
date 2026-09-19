//! Generation vs exploration. Not a third publishable asset.
use crate::evaluation::DataUse;
use crate::{Error, Result, Validate, identifier, text};
use serde::{Deserialize, Serialize};

pub const W_DEFAULT: u8 = 1;
pub const MAX_NODES: u8 = 12;
pub const MAX_DEPTH: u8 = 4;
pub const MAX_REPAIR: u8 = 1;

fn validate_digest(value: &str, name: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::Invalid(format!(
            "{name} must be a lowercase sha256 digest"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationStrategy {
    pub schema_version: String,
    pub instruction: String,
    pub max_candidates: u8,
}

impl Default for GenerationStrategy {
    fn default() -> Self {
        Self {
            schema_version: "rsia.generation.v1".into(),
            instruction: "Propose a narrowly applicable text skill.".into(),
            max_candidates: 2,
        }
    }
}

impl Validate for GenerationStrategy {
    fn validate(&self) -> Result<()> {
        if self.schema_version != "rsia.generation.v1" || !(1..=4).contains(&self.max_candidates) {
            return Err(Error::Invalid("unsupported generation strategy".into()));
        }
        text(&self.instruction, "instruction", 8192)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplorationPolicy {
    pub schema_version: String,
    pub w: u8,
    pub max_nodes: u8,
    pub max_depth: u8,
    pub repair_budget: u8,
}

impl Default for ExplorationPolicy {
    fn default() -> Self {
        Self {
            schema_version: "rsia.exploration.v1".into(),
            w: W_DEFAULT,
            max_nodes: MAX_NODES,
            max_depth: MAX_DEPTH,
            repair_budget: MAX_REPAIR,
        }
    }
}

impl Validate for ExplorationPolicy {
    fn validate(&self) -> Result<()> {
        if self.schema_version != "rsia.exploration.v1" {
            return Err(Error::Invalid("unsupported exploration policy".into()));
        }
        if self.w != 1 {
            return Err(Error::Invalid(
                "W>1 is not enabled; no parallel-gain claim".into(),
            ));
        }
        if self.max_nodes > MAX_NODES
            || self.max_depth > MAX_DEPTH
            || self.repair_budget > MAX_REPAIR
        {
            return Err(Error::Invalid(
                "exploration limits exceed admin caps".into(),
            ));
        }
        if self.max_nodes < 1 || self.max_depth < 1 {
            return Err(Error::Invalid("exploration limits too small".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrefixView {
    pub search_parent: String,
    pub approved_parent: String,
    pub depth: u8,
}

impl PrefixView {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.search_parent)?;
        identifier(&self.approved_parent)?;
        if self.search_parent == self.approved_parent {
            return Err(Error::Invalid(
                "search_parent must be distinct from approved_parent for legacy search nodes"
                    .into(),
            ));
        }
        Ok(())
    }
}

pub const ELASTIC_POLICY_V1: &str = "rsia.elastic_priority.v1";
pub const EXPLORATION_CAPS_V1: &str = "rsia.exploration_caps.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SimulationContext {
    Online { fixed_seed: u64 },
    Offline { w_sim: u8, fixed_seed: u64 },
}

impl SimulationContext {
    pub fn width(self) -> Result<u8> {
        match self {
            Self::Online { .. } => Ok(1),
            Self::Offline { w_sim, .. } if matches!(w_sim, 1 | 2 | 4) => Ok(w_sim),
            Self::Offline { .. } => Err(Error::Invalid("W_sim must be 1, 2, or 4".into())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExplorationCapsV1 {
    pub schema_version: String,
    pub w_online: u8,
    pub max_nodes: u8,
    pub max_depth: u8,
    pub max_repair_dispatches_per_episode: u8,
}

impl ExplorationCapsV1 {
    pub fn online() -> Self {
        Self {
            schema_version: EXPLORATION_CAPS_V1.into(),
            w_online: 1,
            max_nodes: MAX_NODES,
            max_depth: MAX_DEPTH,
            max_repair_dispatches_per_episode: MAX_REPAIR,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != EXPLORATION_CAPS_V1
            || self.w_online != 1
            || !(1..=MAX_NODES).contains(&self.max_nodes)
            || !(1..=MAX_DEPTH).contains(&self.max_depth)
            || self.max_repair_dispatches_per_episode > MAX_REPAIR
        {
            return Err(Error::Invalid(
                "exploration caps exceed 12/4/1 online limits".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElasticPolicyV1 {
    pub schema_version: String,
    pub significant_gain_micros: i32,
    pub stagnation_abs_gain_micros: i32,
    pub stagnation_window: u8,
    pub max_focus_actions: u8,
    pub fairness_wait_rounds: u8,
}

impl Default for ElasticPolicyV1 {
    fn default() -> Self {
        Self {
            schema_version: ELASTIC_POLICY_V1.into(),
            significant_gain_micros: 20_000,
            stagnation_abs_gain_micros: 5_000,
            stagnation_window: 2,
            max_focus_actions: 2,
            fairness_wait_rounds: 4,
        }
    }
}

impl ElasticPolicyV1 {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != ELASTIC_POLICY_V1
            || self.significant_gain_micros != 20_000
            || self.stagnation_abs_gain_micros != 5_000
            || self.stagnation_window != 2
            || self.max_focus_actions != 2
            || self.fairness_wait_rounds != 4
        {
            return Err(Error::Invalid(
                "unsupported elastic-priority policy version".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    Compile,
    Implementation,
    Type,
    OutputShape,
    Environment,
    Resource,
    Safety,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", deny_unknown_fields)]
pub enum ObservedStatus {
    Valid {
        quality_micros: u32,
    },
    RepairableFailure {
        episode_id: String,
        failure_kind: FailureKind,
        repair_template_digest: String,
        environment_reset: bool,
        dispatched_repairs: u8,
    },
    EnvironmentFailure,
    HardFailure,
    SafetyRejected,
    Cancelled,
    UsageUncertain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrefixNodeV2 {
    pub node_seq: u32,
    pub branch_seq: u32,
    pub search_parent_seq: Option<u32>,
    pub approved_parent_digest: String,
    pub depth: u8,
    pub status: ObservedStatus,
    pub best_valid_ancestor_micros: Option<u32>,
    pub recent_valid_gains_micros: Vec<i32>,
    pub repair_failures_dispatched: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpportunityWait {
    pub action_seq: u32,
    pub waited_rounds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrefixViewV2 {
    pub schema_version: String,
    pub context_signature: String,
    pub approved_parent_digest: String,
    pub initial_baseline_quality_micros: u32,
    pub nodes: Vec<PrefixNodeV2>,
    pub current_branch_seq: Option<u32>,
    pub current_branch_focus_actions: u8,
    pub decisions_completed: u32,
    pub waits: Vec<OpportunityWait>,
    pub nodes_used: u8,
    pub recovery_dispatches_used: u8,
}

impl PrefixViewV2 {
    pub const SCHEMA: &'static str = "rsia.prefix_view.v2";

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != Self::SCHEMA {
            return Err(Error::Invalid("unsupported prefix view schema".into()));
        }
        validate_digest(&self.context_signature, "context_signature")?;
        validate_digest(&self.approved_parent_digest, "approved_parent_digest")?;
        if self.initial_baseline_quality_micros > 1_000_000
            || self.nodes_used as usize != self.nodes.len()
            || self.nodes_used > MAX_NODES
        {
            return Err(Error::Invalid(
                "invalid prefix capacity or baseline quality".into(),
            ));
        }
        let mut node_seqs = std::collections::BTreeSet::new();
        let mut wait_seqs = std::collections::BTreeSet::new();
        let mut previous_seq = 0u32;
        for node in &self.nodes {
            if node.node_seq == 0
                || node.branch_seq == 0
                || node.depth > MAX_DEPTH
                || !node_seqs.insert(node.node_seq)
                || node.node_seq <= previous_seq
                || node.recent_valid_gains_micros.len() > 2
                || node
                    .recent_valid_gains_micros
                    .iter()
                    .any(|gain| !(-1_000_000..=1_000_000).contains(gain))
                || node
                    .best_valid_ancestor_micros
                    .is_some_and(|value| value > 1_000_000)
            {
                return Err(Error::Invalid("invalid or duplicate revealed node".into()));
            }
            previous_seq = node.node_seq;
            validate_digest(&node.approved_parent_digest, "node approved_parent_digest")?;
            if node.approved_parent_digest != self.approved_parent_digest {
                return Err(Error::Conflict(
                    "node approved parent differs from prefix".into(),
                ));
            }
            match node.search_parent_seq {
                None if node.depth != 1 => {
                    return Err(Error::Invalid("root node must have depth one".into()));
                }
                Some(parent_seq) => {
                    let parent = self
                        .nodes
                        .iter()
                        .find(|candidate| candidate.node_seq == parent_seq)
                        .ok_or_else(|| Error::Invalid("search parent is not revealed".into()))?;
                    if parent.node_seq >= node.node_seq
                        || parent.branch_seq != node.branch_seq
                        || parent.depth.checked_add(1) != Some(node.depth)
                    {
                        return Err(Error::Invalid(
                            "search parent branch/depth relation is invalid".into(),
                        ));
                    }
                }
                None => {}
            }
            match &node.status {
                ObservedStatus::Valid { quality_micros } if *quality_micros > 1_000_000 => {
                    return Err(Error::Invalid("quality micros out of range".into()));
                }
                ObservedStatus::RepairableFailure {
                    episode_id,
                    repair_template_digest,
                    failure_kind,
                    ..
                } => {
                    identifier(episode_id)?;
                    validate_digest(repair_template_digest, "repair_template_digest")?;
                    if !matches!(
                        failure_kind,
                        FailureKind::Compile
                            | FailureKind::Implementation
                            | FailureKind::Type
                            | FailureKind::OutputShape
                    ) {
                        return Err(Error::Invalid("failure kind is not repairable".into()));
                    }
                }
                _ => {}
            }
            if node.repair_failures_dispatched > MAX_REPAIR {
                return Err(Error::Invalid(
                    "repair failure count exceeds the frozen per-episode cap".into(),
                ));
            }
            let inherited_best = match node.search_parent_seq {
                Some(parent_seq) => self
                    .nodes
                    .iter()
                    .find(|candidate| candidate.node_seq == parent_seq)
                    .and_then(|parent| parent.best_valid_ancestor_micros)
                    .ok_or_else(|| {
                        Error::Invalid("search parent lacks a valid ancestor baseline".into())
                    })?,
                None => self.initial_baseline_quality_micros,
            };
            let expected_best = match &node.status {
                ObservedStatus::Valid { quality_micros } => inherited_best.max(*quality_micros),
                _ => inherited_best,
            };
            if node.best_valid_ancestor_micros != Some(expected_best) {
                return Err(Error::Conflict(format!(
                    "node {} best valid ancestor {:?} does not match revealed parent chain {}",
                    node.node_seq, node.best_valid_ancestor_micros, expected_best
                )));
            }
        }
        for wait in &self.waits {
            if wait.action_seq == 0 || !wait_seqs.insert(wait.action_seq) {
                return Err(Error::Invalid("duplicate opportunity wait".into()));
            }
        }
        if self
            .current_branch_seq
            .is_some_and(|branch| !self.nodes.iter().any(|node| node.branch_seq == branch))
        {
            return Err(Error::Invalid("current branch is not revealed".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action", deny_unknown_fields)]
pub enum ActionKindV1 {
    Widen {
        root_slot: u32,
    },
    Deepen {
        parent_node_seq: u32,
    },
    Recover {
        failed_node_seq: u32,
        episode_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegalActionV1 {
    pub action_id: String,
    pub action_seq: u32,
    pub branch_seq: u32,
    pub target_depth: u8,
    pub kind: ActionKindV1,
    pub estimated_cost_upper_micros: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegalActionsV1 {
    pub schema_version: String,
    pub actions: Vec<LegalActionV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetViewV1 {
    pub remaining_nodes: u8,
    pub remaining_recovery_dispatches: u8,
    pub remaining_root_micros: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "decision", deny_unknown_fields)]
pub enum BatchActionV1 {
    Dispatch {
        action_ids: Vec<String>,
        action_seqs: Vec<u32>,
        reason: String,
        estimated_cost_upper_micros: u64,
    },
    Stop {
        reason: String,
    },
}

#[derive(Debug, Clone)]
struct RankedAction<'a> {
    action: &'a LegalActionV1,
    value: i64,
    waited: u32,
}

pub fn decide_elastic(
    policy: &ElasticPolicyV1,
    prefix: &PrefixViewV2,
    legal: &LegalActionsV1,
    budget: &BudgetViewV1,
    caps: &ExplorationCapsV1,
    simulation: SimulationContext,
) -> Result<BatchActionV1> {
    policy.validate()?;
    prefix.validate()?;
    caps.validate()?;
    if prefix.nodes_used > caps.max_nodes
        || prefix.nodes.iter().any(|node| node.depth > caps.max_depth)
    {
        return Err(Error::Invalid(
            "revealed prefix exceeds the frozen exploration caps".into(),
        ));
    }
    let width = simulation.width()?;
    if matches!(simulation, SimulationContext::Online { .. }) && width != caps.w_online {
        return Err(Error::Invalid(
            "online width differs from frozen caps".into(),
        ));
    }
    if legal.schema_version != "rsia.legal_actions.v1" {
        return Err(Error::Invalid("unsupported legal action schema".into()));
    }
    let waits: std::collections::BTreeMap<_, _> = prefix
        .waits
        .iter()
        .map(|wait| (wait.action_seq, wait.waited_rounds))
        .collect();
    let node_by_seq: std::collections::BTreeMap<_, _> = prefix
        .nodes
        .iter()
        .map(|node| (node.node_seq, node))
        .collect();
    let mut ranked = Vec::new();
    let mut action_ids = std::collections::BTreeSet::new();
    let mut action_seqs = std::collections::BTreeSet::new();
    for action in &legal.actions {
        identifier(&action.action_id)?;
        if !action_ids.insert(action.action_id.as_str()) || !action_seqs.insert(action.action_seq) {
            return Err(Error::Invalid("duplicate legal action identity".into()));
        }
        if action.action_seq == 0
            || action.branch_seq == 0
            || action.target_depth > caps.max_depth
            || budget.remaining_nodes == 0
            || prefix.nodes_used >= caps.max_nodes
        {
            continue;
        }
        let Some(cost) = action.estimated_cost_upper_micros else {
            continue;
        };
        if cost > budget.remaining_root_micros {
            continue;
        }
        if let ActionKindV1::Recover {
            failed_node_seq,
            episode_id,
        } = &action.kind
        {
            if budget.remaining_recovery_dispatches == 0 {
                continue;
            }
            let Some(node) = node_by_seq.get(failed_node_seq) else {
                continue;
            };
            match &node.status {
                ObservedStatus::RepairableFailure {
                    episode_id: observed_episode,
                    environment_reset: true,
                    dispatched_repairs,
                    ..
                } if observed_episode == episode_id
                    && *dispatched_repairs < caps.max_repair_dispatches_per_episode => {}
                _ => continue,
            }
        }
        match &action.kind {
            ActionKindV1::Widen { .. } => {
                if action.target_depth != 1
                    || prefix
                        .nodes
                        .iter()
                        .any(|node| node.branch_seq == action.branch_seq)
                {
                    continue;
                }
            }
            ActionKindV1::Deepen { parent_node_seq } => {
                let Some(parent) = node_by_seq.get(parent_node_seq) else {
                    continue;
                };
                if parent.branch_seq != action.branch_seq
                    || parent.depth.checked_add(1) != Some(action.target_depth)
                    || !matches!(parent.status, ObservedStatus::Valid { .. })
                {
                    continue;
                }
            }
            ActionKindV1::Recover {
                failed_node_seq, ..
            } => {
                let Some(parent) = node_by_seq.get(failed_node_seq) else {
                    continue;
                };
                if parent.branch_seq != action.branch_seq
                    || parent.depth.checked_add(1) != Some(action.target_depth)
                {
                    continue;
                }
            }
        }
        let value = action_value(action, prefix, &node_by_seq)?;
        ranked.push(RankedAction {
            action,
            value,
            waited: *waits.get(&action.action_seq).unwrap_or(&0),
        });
    }
    if ranked.is_empty() {
        return Ok(BatchActionV1::Stop {
            reason: "no_legal_action_with_authorized_cost".into(),
        });
    }
    ranked.sort_by(|left, right| compare_ranked(left, right));
    let selected = select_first(policy, prefix, &ranked, &node_by_seq)?;
    let mut chosen = vec![selected.action];
    let mut total_cost = selected
        .action
        .estimated_cost_upper_micros
        .ok_or_else(|| Error::Invalid("selected action lacks cost bound".into()))?;
    let mut recovery_count =
        usize::from(matches!(selected.action.kind, ActionKindV1::Recover { .. }));
    if width > 1 {
        for candidate in &ranked {
            if chosen.len() >= usize::from(width) {
                break;
            }
            if candidate.action.action_seq == selected.action.action_seq
                || chosen
                    .iter()
                    .any(|chosen| chosen.branch_seq == candidate.action.branch_seq)
            {
                continue;
            }
            let candidate_cost = candidate
                .action
                .estimated_cost_upper_micros
                .ok_or_else(|| Error::Invalid("batch action lacks cost bound".into()))?;
            let candidate_recovery = usize::from(matches!(
                candidate.action.kind,
                ActionKindV1::Recover { .. }
            ));
            if chosen.len() + 1 > usize::from(budget.remaining_nodes)
                || prefix.nodes.len() + chosen.len() + 1 > usize::from(caps.max_nodes)
                || total_cost.saturating_add(candidate_cost) > budget.remaining_root_micros
                || recovery_count + candidate_recovery
                    > usize::from(budget.remaining_recovery_dispatches)
            {
                continue;
            }
            chosen.push(candidate.action);
            total_cost += candidate_cost;
            recovery_count += candidate_recovery;
        }
    }
    Ok(BatchActionV1::Dispatch {
        action_ids: chosen
            .iter()
            .map(|action| action.action_id.clone())
            .collect(),
        action_seqs: chosen.iter().map(|action| action.action_seq).collect(),
        reason: decision_reason(policy, prefix, selected, &ranked, &node_by_seq),
        estimated_cost_upper_micros: total_cost,
    })
}

fn select_first<'a>(
    policy: &ElasticPolicyV1,
    prefix: &PrefixViewV2,
    ranked: &'a [RankedAction<'a>],
    nodes: &std::collections::BTreeMap<u32, &PrefixNodeV2>,
) -> Result<&'a RankedAction<'a>> {
    if prefix.nodes.is_empty() {
        return ranked
            .iter()
            .filter(|item| matches!(item.action.kind, ActionKindV1::Widen { .. }))
            .min_by_key(|item| item.action.action_seq)
            .ok_or_else(|| Error::Invalid("first action must be a preregistered root".into()));
    }
    if let Some(fair) = ranked
        .iter()
        .filter(|item| item.waited >= u32::from(policy.fairness_wait_rounds))
        .max_by(|left, right| {
            left.waited
                .cmp(&right.waited)
                .then_with(|| right.action.action_seq.cmp(&left.action.action_seq))
        })
    {
        return Ok(fair);
    }
    let current = prefix.current_branch_seq;
    let alternatives: Vec<_> = ranked
        .iter()
        .filter(|item| Some(item.action.branch_seq) != current)
        .collect();
    let current_node = current.and_then(|branch| {
        prefix
            .nodes
            .iter()
            .filter(|node| node.branch_seq == branch)
            .max_by_key(|node| node.node_seq)
    });
    let stagnated = current_node.is_some_and(|node| {
        node.recent_valid_gains_micros.len() >= usize::from(policy.stagnation_window)
            && node
                .recent_valid_gains_micros
                .iter()
                .rev()
                .take(usize::from(policy.stagnation_window))
                .all(|gain| gain.unsigned_abs() <= policy.stagnation_abs_gain_micros.unsigned_abs())
    });
    if !alternatives.is_empty()
        && (prefix.current_branch_focus_actions >= policy.max_focus_actions || stagnated)
    {
        if stagnated
            && let Some(root) = alternatives
                .iter()
                .copied()
                .find(|item| matches!(item.action.kind, ActionKindV1::Widen { .. }))
        {
            return Ok(root);
        }
        return Ok(alternatives[0]);
    }
    if prefix.current_branch_focus_actions < policy.max_focus_actions
        && current_node
            .and_then(last_gain)
            .is_some_and(|gain| gain >= policy.significant_gain_micros)
        && let Some(deepen) = ranked.iter().find(|item| {
            Some(item.action.branch_seq) == current
                && matches!(item.action.kind, ActionKindV1::Deepen { .. })
        })
    {
        return Ok(deepen);
    }
    let _ = nodes;
    Ok(&ranked[0])
}

fn action_value(
    action: &LegalActionV1,
    prefix: &PrefixViewV2,
    nodes: &std::collections::BTreeMap<u32, &PrefixNodeV2>,
) -> Result<i64> {
    let (quality, gain, repair_failures) = match &action.kind {
        ActionKindV1::Widen { .. } => (prefix.initial_baseline_quality_micros, 0, 0),
        ActionKindV1::Deepen { parent_node_seq } => {
            let node = nodes.get(parent_node_seq).ok_or(Error::NotFound)?;
            let quality = match node.status {
                ObservedStatus::Valid { quality_micros } => quality_micros,
                _ => node.best_valid_ancestor_micros.unwrap_or(0),
            };
            (
                quality,
                last_gain(node).unwrap_or(0),
                node.repair_failures_dispatched,
            )
        }
        ActionKindV1::Recover {
            failed_node_seq, ..
        } => {
            let node = nodes.get(failed_node_seq).ok_or(Error::NotFound)?;
            (
                node.best_valid_ancestor_micros.unwrap_or(0),
                last_gain(node).unwrap_or(0),
                node.repair_failures_dispatched,
            )
        }
    };
    Ok(
        i64::from(quality) + i64::from(gain.clamp(-100_000, 100_000))
            - i64::from(repair_failures) * 50_000,
    )
}

fn last_gain(node: &PrefixNodeV2) -> Option<i32> {
    node.recent_valid_gains_micros.last().copied()
}

fn compare_ranked(left: &RankedAction<'_>, right: &RankedAction<'_>) -> std::cmp::Ordering {
    right
        .value
        .cmp(&left.value)
        .then_with(|| {
            left.action
                .estimated_cost_upper_micros
                .cmp(&right.action.estimated_cost_upper_micros)
        })
        .then_with(|| left.action.action_seq.cmp(&right.action.action_seq))
}

fn decision_reason(
    policy: &ElasticPolicyV1,
    prefix: &PrefixViewV2,
    selected: &RankedAction<'_>,
    ranked: &[RankedAction<'_>],
    _nodes: &std::collections::BTreeMap<u32, &PrefixNodeV2>,
) -> String {
    if selected.waited >= u32::from(policy.fairness_wait_rounds) {
        "fairness_wait_threshold".into()
    } else if prefix.nodes.is_empty() {
        "first_preregistered_root".into()
    } else if ranked.len() == 1 {
        "only_legal_authorized_action".into()
    } else {
        "elastic_priority".into()
    }
}

pub const HISTORY_MAX_MATCHES: usize = 8;
pub const HISTORY_MAX_UTF8_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryOutcome {
    CompileMismatch,
    NoChange,
    DevelopmentRegression,
    DevelopmentImprovement,
    EnvironmentFailure,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptimizationHistoryEntry {
    pub entry_id: String,
    pub sequence: u64,
    pub parent_digest: String,
    pub environment_digest: String,
    pub task_family: String,
    pub source_watermark: u64,
    pub input_digest: String,
    pub patch_digest: String,
    pub evidence_digest: String,
    pub outcome: HistoryOutcome,
    pub deterministic_error: bool,
    pub data_use: DataUse,
    pub summary: String,
}

#[derive(Debug, Clone)]
pub struct HistoryQuery<'a> {
    pub parent_digest: &'a str,
    pub environment_digest: &'a str,
    pub task_family: &'a str,
    pub source_watermark: u64,
}

pub fn select_optimization_history(
    entries: &[OptimizationHistoryEntry],
    query: HistoryQuery<'_>,
) -> Result<Vec<OptimizationHistoryEntry>> {
    let mut matching: Vec<_> = entries
        .iter()
        .filter(|entry| {
            entry.data_use == DataUse::Development
                && entry.parent_digest == query.parent_digest
                && entry.environment_digest == query.environment_digest
                && entry.task_family == query.task_family
                && entry.source_watermark == query.source_watermark
        })
        .cloned()
        .collect();
    matching.sort_by_key(|entry| std::cmp::Reverse(entry.sequence));
    matching.truncate(HISTORY_MAX_MATCHES);
    matching.reverse();
    let mut bytes = 0usize;
    let mut selected = Vec::new();
    for entry in matching.into_iter().rev() {
        identifier(&entry.entry_id)?;
        for (value, name) in [
            (&entry.parent_digest, "history parent_digest"),
            (&entry.environment_digest, "history environment_digest"),
            (&entry.input_digest, "history input_digest"),
            (&entry.patch_digest, "history patch_digest"),
            (&entry.evidence_digest, "history evidence_digest"),
        ] {
            validate_digest(value, name)?;
        }
        let size = entry.summary.len();
        if size > HISTORY_MAX_UTF8_BYTES || bytes.saturating_add(size) > HISTORY_MAX_UTF8_BYTES {
            continue;
        }
        bytes += size;
        selected.push(entry);
    }
    selected.reverse();
    Ok(selected)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDecision {
    ReuseDeterministicDiagnostic,
    ReconsiderWithNewEvidence,
}

pub fn deterministic_retry_decision(
    history: &[OptimizationHistoryEntry],
    parent_digest: &str,
    input_digest: &str,
    patch_digest: &str,
    environment_digest: &str,
    evidence_digest: &str,
) -> RetryDecision {
    if history.iter().any(|entry| {
        entry.deterministic_error
            && entry.parent_digest == parent_digest
            && entry.input_digest == input_digest
            && entry.patch_digest == patch_digest
            && entry.environment_digest == environment_digest
            && entry.evidence_digest == evidence_digest
    }) {
        RetryDecision::ReuseDeterministicDiagnostic
    } else {
        RetryDecision::ReconsiderWithNewEvidence
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PracticePlan {
    pub schema_version: String,
    pub parent_cluster_id: String,
    pub task_ids: Vec<String>,
    pub attempts_per_task: u8,
    pub authorization_digest: Option<String>,
}

impl PracticePlan {
    pub fn new(
        parent_cluster_id: impl Into<String>,
        task_ids: Vec<String>,
        attempts_per_task: u8,
        authorization_digest: Option<String>,
    ) -> Result<Self> {
        let plan = Self {
            schema_version: "rsia.practice_plan.v1".into(),
            parent_cluster_id: parent_cluster_id.into(),
            task_ids,
            attempts_per_task,
            authorization_digest,
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn validate(&self) -> Result<()> {
        identifier(&self.parent_cluster_id)?;
        if self.schema_version != "rsia.practice_plan.v1"
            || self.task_ids.is_empty()
            || self.task_ids.len() > 2
            || !matches!(self.attempts_per_task, 1 | 3)
            || (self.attempts_per_task == 3 && self.authorization_digest.is_none())
        {
            return Err(Error::Invalid(
                "practice plan violates K/task controls".into(),
            ));
        }
        let mut tasks = std::collections::BTreeSet::new();
        for task in &self.task_ids {
            identifier(task)?;
            if !tasks.insert(task) {
                return Err(Error::Invalid("duplicate practice task".into()));
            }
        }
        if let Some(authorization) = &self.authorization_digest {
            validate_digest(authorization, "practice authorization_digest")?;
        }
        Ok(())
    }

    pub fn independent_cluster_count(&self) -> usize {
        1
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillGroupCandidate {
    pub group_id: String,
    pub candidate_digest: String,
}

pub fn validate_skill_groups(groups: &[SkillGroupCandidate]) -> Result<()> {
    if groups.is_empty() || groups.len() > 2 {
        return Err(Error::Invalid(
            "skill groups must contain one or two groups".into(),
        ));
    }
    let mut ids = std::collections::BTreeMap::new();
    for group in groups {
        identifier(&group.group_id)?;
        validate_digest(&group.candidate_digest, "group candidate_digest")?;
        if ids
            .insert(&group.group_id, &group.candidate_digest)
            .is_some()
        {
            return Err(Error::Conflict("duplicate skill group id".into()));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CombinedSkillCandidate {
    pub schema_version: String,
    pub groups: Vec<SkillGroupCandidate>,
    pub combined_digest: String,
    pub requires_full_development_rerun: bool,
    pub requires_full_formal_evaluation: bool,
}

pub fn build_combined_skill_candidate(
    groups: Vec<SkillGroupCandidate>,
) -> Result<CombinedSkillCandidate> {
    validate_skill_groups(&groups)?;
    Ok(CombinedSkillCandidate {
        schema_version: "rsia.combined_skill_candidate.v1".into(),
        combined_digest: crate::fingerprint(&groups)?,
        groups,
        requires_full_development_rerun: true,
        requires_full_formal_evaluation: true,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsolidationClass {
    Improved,
    Regressed,
    PersistentFail,
    StableSuccess,
}

pub fn classify_consolidation_pair(before_passed: bool, after_passed: bool) -> ConsolidationClass {
    match (before_passed, after_passed) {
        (false, true) => ConsolidationClass::Improved,
        (true, false) => ConsolidationClass::Regressed,
        (false, false) => ConsolidationClass::PersistentFail,
        (true, true) => ConsolidationClass::StableSuccess,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsolidationPair {
    pub task_id: String,
    pub manifest_digest: String,
    pub environment_digest: String,
    pub before_version_digest: String,
    pub after_version_digest: String,
    pub classification: ConsolidationClass,
}

impl ConsolidationPair {
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        task_id: impl Into<String>,
        manifest_digest: impl Into<String>,
        environment_digest: impl Into<String>,
        before_version_digest: impl Into<String>,
        after_version_digest: impl Into<String>,
        before_passed: bool,
        after_passed: bool,
    ) -> Result<Self> {
        let pair = Self {
            task_id: task_id.into(),
            manifest_digest: manifest_digest.into(),
            environment_digest: environment_digest.into(),
            before_version_digest: before_version_digest.into(),
            after_version_digest: after_version_digest.into(),
            classification: classify_consolidation_pair(before_passed, after_passed),
        };
        identifier(&pair.task_id)?;
        for (value, name) in [
            (&pair.manifest_digest, "manifest_digest"),
            (&pair.environment_digest, "environment_digest"),
            (&pair.before_version_digest, "before_version_digest"),
            (&pair.after_version_digest, "after_version_digest"),
        ] {
            validate_digest(value, name)?;
        }
        Ok(pair)
    }
}
