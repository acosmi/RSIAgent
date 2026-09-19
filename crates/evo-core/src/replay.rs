//! Versioned contracts for immutable lookup-only replay worlds.

use crate::evidence::Purpose;
use crate::strategy::PrefixViewV2;
use crate::strategy::{ActionKindV1, ElasticPolicyV1, ExplorationCapsV1, ObservedStatus};
use crate::{Error, Result, fingerprint, identifier, text};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const REPLAY_WORLD_SCHEMA: &str = "rsia.replay_world.v2";
pub const REPLAY_MANIFEST_SCHEMA: &str = "rsia.replay_manifest.v2";
pub const REPLAY_REPORT_SCHEMA: &str = "rsia.replay_report.v2";
pub const REPLAY_POOL_SCHEMA: &str = "rsia.replay_pool.v1";
pub const STORED_REPLAY_REPORT_SCHEMA: &str = "rsia.stored_replay_report.v1";
pub const REPLAY_POOL_COMPILER_VERSION: &str = "rsia.replay_pool.compiler.v1";
pub const REPLAY_ENGINE_VERSION: &str = "rsia.replay.engine.v2";
pub const REPLAY_BASELINE_OBSERVATION_SCHEMA: &str = "rsia.replay_baseline_observation.v1";
pub const REPLAY_OBSERVATION_SCHEMA: &str = "rsia.replay_observation.v2";
pub const MAX_REPLAY_POOL_WORLDS: usize = 10_000;
pub const SIMULATION_VERSION: &str = "rsia.unit_probe_barrier.v1";
pub const OBJECTIVE_V1: &str = "rsia.attainment_auc.v1";
pub const OBJECTIVE_V2: &str = "rsia.pareto_attainment.v2";
pub const DEFAULT_PROBE_BUDGET: u8 = 12;
pub const DEFAULT_LAMBDA_MICROS: u32 = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldPartition {
    Train,
    Select,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaySourceRef {
    pub source_id: String,
    pub content_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrefixCoverageV1 {
    pub context_signature: String,
    pub exhausted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayWorldManifestV2 {
    pub schema_version: String,
    pub world_id: String,
    pub cluster_id: String,
    pub partition: WorldPartition,
    pub purpose: Purpose,
    pub generation_signature: String,
    pub world_context_signature: String,
    pub baseline_context_signature: String,
    pub approved_parent_digest: String,
    pub model_digest: String,
    pub tools_digest: String,
    pub scorer_digest: String,
    pub guidance_digest: String,
    pub repair_template_digest: String,
    pub input_order_digest: String,
    pub initial_baseline_quality_micros: u32,
    pub baseline_observation_source_id: String,
    pub source_closure: Vec<ReplaySourceRef>,
    pub revoke_watermark: u64,
    pub prefix_coverage: Vec<PrefixCoverageV1>,
    /// Preregistered legal opportunities. This catalog contains no outcomes.
    pub action_catalog: Vec<ReplayActionSpecV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayActionSpecV1 {
    pub record_seq: u32,
    pub generation_signature: String,
    pub parent_context_signature: String,
    pub branch_seq: u32,
    pub target_depth: u8,
    pub action_kind: ActionKindV1,
    pub estimated_cost_upper_micros: Option<u64>,
    pub writes_shared_workspace: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct HistoricalUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_micros: Option<u64>,
    pub latency_millis: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome", deny_unknown_fields)]
pub enum ReplayTransitionOutcome {
    Observed { status: ObservedStatus },
    Censored { reason: String },
    OutOfSupport { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayTransitionV2 {
    /// Opaque audit identity. It never participates in replay choice.
    pub record_id: String,
    /// Stable preregistered ordering and action identity.
    pub record_seq: u32,
    pub generation_signature: String,
    pub parent_context_signature: String,
    pub action_kind: ActionKindV1,
    pub next_context_signature: String,
    pub outcome: ReplayTransitionOutcome,
    pub actual_usage: HistoricalUsage,
    pub source_ids: Vec<String>,
    pub observation_source_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayWorldV2 {
    pub schema_version: String,
    pub manifest: ReplayWorldManifestV2,
    pub transitions: Vec<ReplayTransitionV2>,
    pub sealed_digest: Option<String>,
}

impl ReplayWorldV2 {
    pub fn seal(&mut self) -> Result<String> {
        if self.sealed_digest.is_some() {
            return Err(Error::Conflict("sealed replay world is immutable".into()));
        }
        self.validate_unsealed()?;
        let digest = self.content_digest()?;
        self.sealed_digest = Some(digest.clone());
        Ok(digest)
    }

    pub fn validate_sealed(&self) -> Result<()> {
        self.validate_unsealed()?;
        let sealed = self
            .sealed_digest
            .as_deref()
            .ok_or_else(|| Error::Invalid("replay world is not sealed".into()))?;
        if sealed != self.content_digest()? {
            return Err(Error::Conflict(
                "sealed replay world content changed".into(),
            ));
        }
        Ok(())
    }

    fn validate_unsealed(&self) -> Result<()> {
        if self.schema_version != REPLAY_WORLD_SCHEMA
            || self.manifest.schema_version != REPLAY_MANIFEST_SCHEMA
        {
            return Err(Error::Invalid("unsupported replay world schema".into()));
        }
        let manifest = &self.manifest;
        for value in [&manifest.world_id, &manifest.cluster_id] {
            identifier(value)?;
        }
        for (value, name) in [
            (&manifest.generation_signature, "generation_signature"),
            (&manifest.world_context_signature, "world_context_signature"),
            (
                &manifest.baseline_context_signature,
                "baseline_context_signature",
            ),
            (&manifest.approved_parent_digest, "approved_parent_digest"),
            (&manifest.model_digest, "model_digest"),
            (&manifest.tools_digest, "tools_digest"),
            (&manifest.scorer_digest, "scorer_digest"),
            (&manifest.guidance_digest, "guidance_digest"),
            (&manifest.repair_template_digest, "repair_template_digest"),
            (&manifest.input_order_digest, "input_order_digest"),
        ] {
            validate_digest(value, name)?;
        }
        if manifest.purpose != Purpose::Development
            || manifest.initial_baseline_quality_micros > 1_000_000
            || manifest.source_closure.is_empty()
        {
            return Err(Error::Invalid(
                "replay world requires development sources and bounded baseline".into(),
            ));
        }
        let mut sources = BTreeSet::new();
        for source in &manifest.source_closure {
            identifier(&source.source_id)?;
            validate_digest(&source.content_digest, "source digest")?;
            if !sources.insert(source.source_id.as_str()) {
                return Err(Error::Conflict("duplicate replay source".into()));
            }
        }
        identifier(&manifest.baseline_observation_source_id)?;
        let baseline_source = manifest
            .source_closure
            .iter()
            .find(|source| source.source_id == manifest.baseline_observation_source_id)
            .ok_or_else(|| {
                Error::Invalid("baseline observation source is absent from closure".into())
            })?;
        if baseline_source.content_digest != replay_baseline_observation_digest(manifest)? {
            return Err(Error::Conflict(
                "baseline observation digest differs from manifest inputs".into(),
            ));
        }
        let mut coverage = BTreeSet::new();
        for prefix in &manifest.prefix_coverage {
            validate_digest(&prefix.context_signature, "prefix context signature")?;
            if !coverage.insert(prefix.context_signature.as_str()) {
                return Err(Error::Conflict("duplicate prefix coverage".into()));
            }
        }
        let mut action_sequences = BTreeSet::new();
        for action in &manifest.action_catalog {
            if action.record_seq == 0
                || action.branch_seq == 0
                || action.target_depth == 0
                || action.target_depth > 4
                || action.writes_shared_workspace
                || !action_sequences.insert(action.record_seq)
            {
                return Err(Error::Invalid(
                    "invalid replay action identity or isolation".into(),
                ));
            }
            validate_digest(&action.generation_signature, "action generation signature")?;
            validate_digest(
                &action.parent_context_signature,
                "action parent context signature",
            )?;
            if action.generation_signature != manifest.generation_signature {
                return Err(Error::Conflict(
                    "action generation signature differs from world".into(),
                ));
            }
        }
        let action_by_seq: BTreeMap<_, _> = manifest
            .action_catalog
            .iter()
            .map(|action| (action.record_seq, action))
            .collect();
        let mut sequences = BTreeSet::new();
        let mut keys = BTreeSet::new();
        let mut used_sources = BTreeSet::from([manifest.baseline_observation_source_id.as_str()]);
        let mut observation_source_ids = BTreeSet::new();
        for transition in &self.transitions {
            identifier(&transition.observation_source_id)?;
            if !observation_source_ids.insert(transition.observation_source_id.as_str()) {
                return Err(Error::Conflict(
                    "one observation source cannot attest multiple transitions".into(),
                ));
            }
        }
        for transition in &self.transitions {
            text(&transition.record_id, "record_id", 128)?;
            if transition.record_seq == 0 || !sequences.insert(transition.record_seq) {
                return Err(Error::Invalid(
                    "invalid replay transition identity or isolation".into(),
                ));
            }
            validate_digest(
                &transition.generation_signature,
                "transition generation signature",
            )?;
            validate_digest(
                &transition.parent_context_signature,
                "transition parent context signature",
            )?;
            validate_digest(
                &transition.next_context_signature,
                "transition next context signature",
            )?;
            if transition.generation_signature != manifest.generation_signature {
                return Err(Error::Conflict(
                    "transition generation signature differs from world".into(),
                ));
            }
            let action = action_by_seq
                .get(&transition.record_seq)
                .ok_or_else(|| Error::Invalid("transition has no preregistered action".into()))?;
            if action.generation_signature != transition.generation_signature
                || action.parent_context_signature != transition.parent_context_signature
                || fingerprint(&action.action_kind)? != fingerprint(&transition.action_kind)?
            {
                return Err(Error::Conflict(
                    "transition key differs from preregistered action".into(),
                ));
            }
            let key = (
                transition.generation_signature.clone(),
                transition.parent_context_signature.clone(),
                fingerprint(&transition.action_kind)?,
                transition.record_seq,
            );
            if !keys.insert(key) {
                return Err(Error::Conflict("duplicate replay transition key".into()));
            }
            let mut transition_sources = BTreeSet::new();
            for source_id in &transition.source_ids {
                identifier(source_id)?;
                used_sources.insert(source_id.as_str());
                if !sources.contains(source_id.as_str())
                    || !transition_sources.insert(source_id.as_str())
                {
                    return Err(Error::Invalid(
                        "transition source is missing or duplicated".into(),
                    ));
                }
                if source_id != &transition.observation_source_id
                    && observation_source_ids.contains(source_id.as_str())
                {
                    return Err(Error::Invalid(
                        "transition source closure cannot depend on another observation body"
                            .into(),
                    ));
                }
            }
            if !transition_sources.contains(transition.observation_source_id.as_str()) {
                return Err(Error::Invalid(
                    "observation source is absent from transition closure".into(),
                ));
            }
            let observation_source = manifest
                .source_closure
                .iter()
                .find(|source| source.source_id == transition.observation_source_id)
                .ok_or_else(|| Error::Invalid("observation source is absent".into()))?;
            if observation_source.content_digest != replay_observation_digest(manifest, transition)?
            {
                return Err(Error::Conflict(
                    "stored observation digest differs from transition".into(),
                ));
            }
            match &transition.outcome {
                ReplayTransitionOutcome::Observed { status } => {
                    validate_observed(status)?;
                }
                ReplayTransitionOutcome::Censored { reason }
                | ReplayTransitionOutcome::OutOfSupport { reason } => {
                    text(reason, "terminal reason", 512)?;
                }
            }
        }
        if used_sources != sources {
            return Err(Error::Invalid(
                "replay source closure contains unconsumed sources".into(),
            ));
        }
        Ok(())
    }

    fn content_digest(&self) -> Result<String> {
        #[derive(Serialize)]
        struct Content<'a> {
            schema_version: &'a str,
            manifest: &'a ReplayWorldManifestV2,
            transitions: &'a [ReplayTransitionV2],
        }
        fingerprint(&Content {
            schema_version: &self.schema_version,
            manifest: &self.manifest,
            transitions: &self.transitions,
        })
    }
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct ReplayBaselineObservationMaterial<'a> {
    schema_version: &'static str,
    observation_kind: &'static str,
    cluster_id: &'a str,
    purpose: Purpose,
    generation_signature: &'a str,
    world_context_signature: &'a str,
    baseline_context_signature: &'a str,
    approved_parent_digest: &'a str,
    model_digest: &'a str,
    tools_digest: &'a str,
    scorer_digest: &'a str,
    input_order_digest: &'a str,
    initial_baseline_quality_micros: u32,
}

/// Canonical observed-baseline evidence body. A live TrustedHost development
/// source must store these exact bytes and its digest must be in the world
/// source closure; a caller-provided digest or label is not evidence.
pub fn replay_baseline_observation_bytes(manifest: &ReplayWorldManifestV2) -> Result<Vec<u8>> {
    serde_json::to_vec(&ReplayBaselineObservationMaterial {
        schema_version: REPLAY_BASELINE_OBSERVATION_SCHEMA,
        observation_kind: "observed",
        cluster_id: &manifest.cluster_id,
        purpose: manifest.purpose,
        generation_signature: &manifest.generation_signature,
        world_context_signature: &manifest.world_context_signature,
        baseline_context_signature: &manifest.baseline_context_signature,
        approved_parent_digest: &manifest.approved_parent_digest,
        model_digest: &manifest.model_digest,
        tools_digest: &manifest.tools_digest,
        scorer_digest: &manifest.scorer_digest,
        input_order_digest: &manifest.input_order_digest,
        initial_baseline_quality_micros: manifest.initial_baseline_quality_micros,
    })
    .map_err(|_| Error::Internal)
}

pub fn replay_baseline_observation_digest(manifest: &ReplayWorldManifestV2) -> Result<String> {
    Ok(crate::hash(&replay_baseline_observation_bytes(manifest)?))
}

pub fn replay_observation_bytes(
    manifest: &ReplayWorldManifestV2,
    transition: &ReplayTransitionV2,
) -> Result<Vec<u8>> {
    #[derive(Serialize)]
    struct SourceMaterial<'a> {
        source_id: &'a str,
        /// The attesting observation source omits its own digest to avoid a
        /// recursive hash. Every other actual input source binds its digest.
        content_digest: Option<&'a str>,
    }
    #[derive(Serialize)]
    struct Material<'a> {
        schema_version: &'static str,
        cluster_id: &'a str,
        purpose: Purpose,
        generation_signature: &'a str,
        world_context_signature: &'a str,
        baseline_context_signature: &'a str,
        approved_parent_digest: &'a str,
        model_digest: &'a str,
        tools_digest: &'a str,
        scorer_digest: &'a str,
        guidance_digest: &'a str,
        repair_template_digest: &'a str,
        input_order_digest: &'a str,
        initial_baseline_quality_micros: u32,
        baseline_observation_source_id: &'a str,
        baseline_observation_digest: &'a str,
        record_seq: u32,
        parent_context_signature: &'a str,
        action_kind: &'a ActionKindV1,
        next_context_signature: &'a str,
        outcome: &'a ReplayTransitionOutcome,
        actual_usage: &'a HistoricalUsage,
        observation_source_id: &'a str,
        source_material: Vec<SourceMaterial<'a>>,
    }
    let baseline_source = manifest
        .source_closure
        .iter()
        .find(|source| source.source_id == manifest.baseline_observation_source_id)
        .ok_or_else(|| Error::Invalid("baseline observation source is absent".into()))?;
    let mut source_material = Vec::with_capacity(transition.source_ids.len());
    for source_id in &transition.source_ids {
        let source = manifest
            .source_closure
            .iter()
            .find(|source| source.source_id == *source_id)
            .ok_or_else(|| Error::Invalid("transition source is absent".into()))?;
        source_material.push(SourceMaterial {
            source_id,
            content_digest: (source_id != &transition.observation_source_id)
                .then_some(source.content_digest.as_str()),
        });
    }
    source_material.sort_by(|left, right| left.source_id.cmp(right.source_id));
    serde_json::to_vec(&Material {
        schema_version: REPLAY_OBSERVATION_SCHEMA,
        cluster_id: &manifest.cluster_id,
        purpose: manifest.purpose,
        generation_signature: &manifest.generation_signature,
        world_context_signature: &manifest.world_context_signature,
        baseline_context_signature: &manifest.baseline_context_signature,
        approved_parent_digest: &manifest.approved_parent_digest,
        model_digest: &manifest.model_digest,
        tools_digest: &manifest.tools_digest,
        scorer_digest: &manifest.scorer_digest,
        guidance_digest: &manifest.guidance_digest,
        repair_template_digest: &manifest.repair_template_digest,
        input_order_digest: &manifest.input_order_digest,
        initial_baseline_quality_micros: manifest.initial_baseline_quality_micros,
        baseline_observation_source_id: &manifest.baseline_observation_source_id,
        baseline_observation_digest: &baseline_source.content_digest,
        record_seq: transition.record_seq,
        parent_context_signature: &transition.parent_context_signature,
        action_kind: &transition.action_kind,
        next_context_signature: &transition.next_context_signature,
        outcome: &transition.outcome,
        actual_usage: &transition.actual_usage,
        observation_source_id: &transition.observation_source_id,
        source_material,
    })
    .map_err(|_| Error::Internal)
}

pub fn replay_observation_digest(
    manifest: &ReplayWorldManifestV2,
    transition: &ReplayTransitionV2,
) -> Result<String> {
    Ok(crate::hash(&replay_observation_bytes(
        manifest, transition,
    )?))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayObjective {
    AttainmentAucV1,
    ParetoAttainmentV2,
}

impl ReplayObjective {
    pub fn version(self) -> &'static str {
        match self {
            Self::AttainmentAucV1 => OBJECTIVE_V1,
            Self::ParetoAttainmentV2 => OBJECTIVE_V2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaySimulationProfile {
    pub simulation_version: String,
    pub objective: ReplayObjective,
    pub w_sim: u8,
    pub probe_budget: u8,
    pub horizon: u8,
    pub lambda_work_micros: u32,
    pub lambda_round_micros: u32,
    pub fixed_seed: u64,
    pub global_recovery_dispatch_limit: u8,
    pub pool_digest: String,
    pub purpose: Purpose,
    pub target_runtime_profile: String,
}

impl ReplaySimulationProfile {
    pub fn default_v2(w_sim: u8, pool_digest: String) -> Self {
        Self {
            simulation_version: SIMULATION_VERSION.into(),
            objective: ReplayObjective::ParetoAttainmentV2,
            w_sim,
            probe_budget: DEFAULT_PROBE_BUDGET,
            horizon: DEFAULT_PROBE_BUDGET,
            lambda_work_micros: DEFAULT_LAMBDA_MICROS,
            lambda_round_micros: DEFAULT_LAMBDA_MICROS,
            fixed_seed: 0,
            global_recovery_dispatch_limit: 1,
            pool_digest,
            purpose: Purpose::Development,
            target_runtime_profile: "simulation_only".into(),
        }
    }

    pub fn validate(&self, caps: &ExplorationCapsV1) -> Result<()> {
        caps.validate()?;
        if self.simulation_version != SIMULATION_VERSION
            || !matches!(self.w_sim, 1 | 2 | 4)
            || self.probe_budget == 0
            || self.probe_budget > caps.max_nodes
            || self.horizon != self.probe_budget
            || self.lambda_work_micros == 0
            || self.lambda_round_micros == 0
            || self.purpose != Purpose::Development
            || self.global_recovery_dispatch_limit > self.probe_budget
            || (self.objective == ReplayObjective::AttainmentAucV1 && self.w_sim != 1)
        {
            return Err(Error::Invalid("invalid replay simulation profile".into()));
        }
        validate_digest(&self.pool_digest, "pool_digest")?;
        identifier(&self.target_runtime_profile)
    }

    pub fn validate_for_pool(
        &self,
        caps: &ExplorationCapsV1,
        pool: &ReplayPoolManifestV1,
    ) -> Result<()> {
        self.validate(caps)?;
        pool.validate_identity()?;
        if self.pool_digest != pool.pool_digest {
            return Err(Error::Conflict(
                "simulation profile does not identify the loaded replay pool".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayTerminal {
    PolicyStop,
    BudgetExhausted,
    WorldExhausted,
    OutOfSupport,
    Censored,
    InvalidWorld,
    SourceRevoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayBatchRecord {
    pub decision_round: u32,
    pub action_seqs: Vec<u32>,
    pub record_seqs: Vec<u32>,
    pub revealed_quality_micros: Vec<Option<u32>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RationalValue {
    pub numerator: u32,
    pub denominator: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ReplayCoverage {
    pub attempted_actions: u32,
    pub observed_actions: u32,
    pub censored_actions: u32,
    pub out_of_support_actions: u32,
    pub complete_support: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayReportV2 {
    pub schema_version: String,
    pub world_id: String,
    pub world_digest: String,
    pub cluster_id: String,
    pub partition: WorldPartition,
    pub policy_digest: String,
    pub profile_digest: String,
    pub objective_version: String,
    pub batches: Vec<ReplayBatchRecord>,
    pub probe_best_quality_micros: Vec<u32>,
    pub round_best_quality_micros: Vec<u32>,
    pub final_quality_micros: u32,
    pub probes: u32,
    pub simulated_rounds: u32,
    pub parallel_penalty: Option<RationalValue>,
    pub no_probe: bool,
    pub attainment_auc_micros: Option<u32>,
    pub q_auc_sim_micros: Option<u32>,
    pub score_v2_micros: Option<i64>,
    pub historical_usage: HistoricalUsage,
    pub replay_cpu_nanos: u64,
    pub coverage: ReplayCoverage,
    pub terminal: ReplayTerminal,
    pub terminal_reason: String,
    pub development_only: bool,
    pub revealed_prefix: Option<PrefixViewV2>,
}

pub fn validate_world_partitions(worlds: &[ReplayWorldV2]) -> Result<()> {
    let mut by_cluster = BTreeMap::new();
    let mut world_ids = BTreeSet::new();
    for world in worlds {
        world.validate_sealed()?;
        if !world_ids.insert(world.manifest.world_id.as_str()) {
            return Err(Error::Conflict("duplicate world id in pool".into()));
        }
        if let Some(partition) =
            by_cluster.insert(world.manifest.cluster_id.as_str(), world.manifest.partition)
            && partition != world.manifest.partition
        {
            return Err(Error::Conflict(
                "task cluster crosses train/select partition".into(),
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayPoolCompatibilityV1 {
    pub purpose: Purpose,
    pub generation_signature: String,
    pub approved_parent_digest: String,
    pub model_digest: String,
    pub tools_digest: String,
    pub scorer_digest: String,
    pub guidance_digest: String,
    pub repair_template_digest: String,
    pub revoke_watermark: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayPoolMemberV1 {
    pub world_id: String,
    pub world_digest: String,
    pub cluster_id: String,
    pub partition: WorldPartition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayPoolManifestV1 {
    pub schema_version: String,
    pub compiler_version: String,
    pub pool_digest: String,
    pub compatibility: ReplayPoolCompatibilityV1,
    pub members: Vec<ReplayPoolMemberV1>,
}

impl ReplayPoolManifestV1 {
    pub fn build(worlds: &[ReplayWorldV2]) -> Result<Self> {
        if worlds.is_empty() || worlds.len() > MAX_REPLAY_POOL_WORLDS {
            return Err(Error::Invalid(format!(
                "replay pool must contain 1..={MAX_REPLAY_POOL_WORLDS} worlds"
            )));
        }
        validate_world_partitions(worlds)?;
        let first = &worlds[0].manifest;
        let compatibility = ReplayPoolCompatibilityV1 {
            purpose: first.purpose,
            generation_signature: first.generation_signature.clone(),
            approved_parent_digest: first.approved_parent_digest.clone(),
            model_digest: first.model_digest.clone(),
            tools_digest: first.tools_digest.clone(),
            scorer_digest: first.scorer_digest.clone(),
            guidance_digest: first.guidance_digest.clone(),
            repair_template_digest: first.repair_template_digest.clone(),
            revoke_watermark: first.revoke_watermark,
        };
        validate_pool_compatibility(&compatibility)?;
        let mut members = Vec::with_capacity(worlds.len());
        let mut partitions = BTreeSet::new();
        for world in worlds {
            world.validate_sealed()?;
            if world.manifest.purpose != compatibility.purpose
                || world.manifest.generation_signature != compatibility.generation_signature
                || world.manifest.approved_parent_digest != compatibility.approved_parent_digest
                || world.manifest.model_digest != compatibility.model_digest
                || world.manifest.tools_digest != compatibility.tools_digest
                || world.manifest.scorer_digest != compatibility.scorer_digest
                || world.manifest.guidance_digest != compatibility.guidance_digest
                || world.manifest.repair_template_digest != compatibility.repair_template_digest
                || world.manifest.revoke_watermark != compatibility.revoke_watermark
            {
                return Err(Error::Conflict(
                    "replay world is incompatible with the pool".into(),
                ));
            }
            partitions.insert(world.manifest.partition);
            members.push(ReplayPoolMemberV1 {
                world_id: world.manifest.world_id.clone(),
                world_digest: world.sealed_digest.clone().ok_or(Error::Internal)?,
                cluster_id: world.manifest.cluster_id.clone(),
                partition: world.manifest.partition,
            });
        }
        if !partitions.contains(&WorldPartition::Train)
            || !partitions.contains(&WorldPartition::Select)
        {
            return Err(Error::Invalid(
                "replay pool requires complete Train and Select worlds".into(),
            ));
        }
        members.sort_by(|left, right| left.world_id.cmp(&right.world_id));
        let pool_digest = pool_digest(&compatibility, &members)?;
        let pool = Self {
            schema_version: REPLAY_POOL_SCHEMA.into(),
            compiler_version: REPLAY_POOL_COMPILER_VERSION.into(),
            pool_digest,
            compatibility,
            members,
        };
        pool.validate_identity()?;
        Ok(pool)
    }

    pub fn validate_against(&self, worlds: &[ReplayWorldV2]) -> Result<()> {
        self.validate_identity()?;
        if Self::build(worlds)? != *self {
            return Err(Error::Conflict(
                "replay pool differs from its sealed worlds".into(),
            ));
        }
        Ok(())
    }

    pub fn validate_identity(&self) -> Result<()> {
        if self.schema_version != REPLAY_POOL_SCHEMA
            || self.compiler_version != REPLAY_POOL_COMPILER_VERSION
            || self.members.is_empty()
            || self.members.len() > MAX_REPLAY_POOL_WORLDS
        {
            return Err(Error::Invalid("unsupported replay pool identity".into()));
        }
        validate_pool_compatibility(&self.compatibility)?;
        let mut previous = None;
        let mut partitions = BTreeSet::new();
        for member in &self.members {
            identifier(&member.world_id)?;
            identifier(&member.cluster_id)?;
            validate_digest(&member.world_digest, "world digest")?;
            if previous.is_some_and(|value: &str| value >= member.world_id.as_str()) {
                return Err(Error::Conflict(
                    "replay pool members are duplicated or not canonical".into(),
                ));
            }
            previous = Some(member.world_id.as_str());
            partitions.insert(member.partition);
        }
        if !partitions.contains(&WorldPartition::Train)
            || !partitions.contains(&WorldPartition::Select)
            || self.pool_digest != pool_digest(&self.compatibility, &self.members)?
        {
            return Err(Error::Conflict("replay pool digest mismatch".into()));
        }
        Ok(())
    }
}

fn validate_pool_compatibility(compatibility: &ReplayPoolCompatibilityV1) -> Result<()> {
    if compatibility.purpose != Purpose::Development {
        return Err(Error::Forbidden);
    }
    for (value, name) in [
        (&compatibility.generation_signature, "generation signature"),
        (
            &compatibility.approved_parent_digest,
            "approved parent digest",
        ),
        (&compatibility.model_digest, "model digest"),
        (&compatibility.tools_digest, "tools digest"),
        (&compatibility.scorer_digest, "scorer digest"),
        (&compatibility.guidance_digest, "guidance digest"),
        (
            &compatibility.repair_template_digest,
            "repair template digest",
        ),
    ] {
        validate_digest(value, name)?;
    }
    Ok(())
}

fn pool_digest(
    compatibility: &ReplayPoolCompatibilityV1,
    members: &[ReplayPoolMemberV1],
) -> Result<String> {
    fingerprint(&(
        REPLAY_POOL_SCHEMA,
        REPLAY_POOL_COMPILER_VERSION,
        compatibility,
        members,
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayReportMemberV1 {
    pub world_id: String,
    pub world_digest: String,
    pub report: ReplayReportV2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredReplayReportV1 {
    pub schema_version: String,
    pub replay_engine_version: String,
    pub report_id: String,
    pub namespace: String,
    pub purpose: Purpose,
    pub partition: WorldPartition,
    pub pool_digest: String,
    pub policy: ElasticPolicyV1,
    pub policy_digest: String,
    pub profile: ReplaySimulationProfile,
    pub profile_digest: String,
    pub caps: ExplorationCapsV1,
    pub caps_digest: String,
    pub member_reports: Vec<ReplayReportMemberV1>,
    pub reports_body_digest: String,
    pub semantic_reports_digest: String,
}

impl StoredReplayReportV1 {
    pub fn build(
        namespace: impl Into<String>,
        partition: WorldPartition,
        pool: &ReplayPoolManifestV1,
        policy: ElasticPolicyV1,
        profile: ReplaySimulationProfile,
        caps: ExplorationCapsV1,
        reports: Vec<ReplayReportV2>,
    ) -> Result<Self> {
        let namespace = namespace.into();
        identifier(&namespace)?;
        pool.validate_identity()?;
        policy.validate()?;
        profile.validate_for_pool(&caps, pool)?;
        let policy_digest = fingerprint(&policy)?;
        let profile_digest = fingerprint(&profile)?;
        let caps_digest = fingerprint(&caps)?;
        let expected = pool
            .members
            .iter()
            .filter(|member| member.partition == partition)
            .collect::<Vec<_>>();
        if reports.len() != expected.len() {
            return Err(Error::Conflict(
                "replay report omits or adds partition worlds".into(),
            ));
        }
        let mut by_world = BTreeMap::new();
        for report in reports {
            if by_world.insert(report.world_id.clone(), report).is_some() {
                return Err(Error::Conflict("duplicate world report".into()));
            }
        }
        let mut member_reports = Vec::with_capacity(expected.len());
        for member in expected {
            let report = by_world.remove(&member.world_id).ok_or_else(|| {
                Error::Conflict("replay report does not cover the complete partition".into())
            })?;
            validate_report_member(
                &report,
                member,
                &policy_digest,
                &profile_digest,
                profile.objective.version(),
            )?;
            member_reports.push(ReplayReportMemberV1 {
                world_id: member.world_id.clone(),
                world_digest: member.world_digest.clone(),
                report,
            });
        }
        let reports_body_digest = fingerprint(&member_reports)?;
        let semantic_reports_digest = replay_report_semantic_digest(&member_reports)?;
        let report_id = replay_report_id(
            &namespace,
            partition,
            &pool.pool_digest,
            &policy_digest,
            &profile_digest,
            &caps_digest,
        )?;
        let record = Self {
            schema_version: STORED_REPLAY_REPORT_SCHEMA.into(),
            replay_engine_version: REPLAY_ENGINE_VERSION.into(),
            report_id,
            namespace,
            purpose: Purpose::Development,
            partition,
            pool_digest: pool.pool_digest.clone(),
            policy,
            policy_digest,
            profile,
            profile_digest,
            caps,
            caps_digest,
            member_reports,
            reports_body_digest,
            semantic_reports_digest,
        };
        record.validate_static(pool)?;
        Ok(record)
    }

    pub fn validate_static(&self, pool: &ReplayPoolManifestV1) -> Result<()> {
        if self.schema_version != STORED_REPLAY_REPORT_SCHEMA
            || self.replay_engine_version != REPLAY_ENGINE_VERSION
            || self.purpose != Purpose::Development
        {
            return Err(Error::Invalid("unsupported stored replay report".into()));
        }
        identifier(&self.report_id)?;
        identifier(&self.namespace)?;
        pool.validate_identity()?;
        self.policy.validate()?;
        self.profile.validate_for_pool(&self.caps, pool)?;
        if self.pool_digest != pool.pool_digest
            || self.policy_digest != fingerprint(&self.policy)?
            || self.profile_digest != fingerprint(&self.profile)?
            || self.caps_digest != fingerprint(&self.caps)?
            || self.reports_body_digest != fingerprint(&self.member_reports)?
            || self.semantic_reports_digest != replay_report_semantic_digest(&self.member_reports)?
            || self.report_id
                != replay_report_id(
                    &self.namespace,
                    self.partition,
                    &self.pool_digest,
                    &self.policy_digest,
                    &self.profile_digest,
                    &self.caps_digest,
                )?
        {
            return Err(Error::Conflict(
                "stored replay report digest mismatch".into(),
            ));
        }
        let expected = pool
            .members
            .iter()
            .filter(|member| member.partition == self.partition)
            .collect::<Vec<_>>();
        if expected.len() != self.member_reports.len() {
            return Err(Error::Conflict(
                "stored report partition coverage mismatch".into(),
            ));
        }
        for (member, stored) in expected.into_iter().zip(&self.member_reports) {
            if stored.world_id != member.world_id || stored.world_digest != member.world_digest {
                return Err(Error::Conflict(
                    "stored report world identity mismatch".into(),
                ));
            }
            validate_report_member(
                &stored.report,
                member,
                &self.policy_digest,
                &self.profile_digest,
                self.profile.objective.version(),
            )?;
        }
        Ok(())
    }
}

fn validate_report_member(
    report: &ReplayReportV2,
    member: &ReplayPoolMemberV1,
    policy_digest: &str,
    profile_digest: &str,
    objective_version: &str,
) -> Result<()> {
    if report.schema_version != REPLAY_REPORT_SCHEMA
        || report.world_id != member.world_id
        || report.world_digest != member.world_digest
        || report.cluster_id != member.cluster_id
        || report.partition != member.partition
        || report.policy_digest != policy_digest
        || report.profile_digest != profile_digest
        || report.objective_version != objective_version
        || !report.development_only
    {
        return Err(Error::Conflict(
            "replay report member binding mismatch".into(),
        ));
    }
    Ok(())
}

pub fn replay_report_id(
    namespace: &str,
    partition: WorldPartition,
    pool_digest: &str,
    policy_digest: &str,
    profile_digest: &str,
    caps_digest: &str,
) -> Result<String> {
    identifier(namespace)?;
    for value in [pool_digest, policy_digest, profile_digest, caps_digest] {
        validate_digest(value, "replay report identity digest")?;
    }
    Ok(format!(
        "replay-report-{}",
        fingerprint(&(
            namespace,
            Purpose::Development,
            partition,
            pool_digest,
            policy_digest,
            profile_digest,
            caps_digest,
            REPLAY_ENGINE_VERSION,
        ))?
    ))
}

pub fn replay_report_semantic_digest(reports: &[ReplayReportMemberV1]) -> Result<String> {
    let mut values = Vec::with_capacity(reports.len());
    for report in reports {
        let mut value = serde_json::to_value(report).map_err(|_| Error::Internal)?;
        value["report"]
            .as_object_mut()
            .ok_or(Error::Internal)?
            .remove("replay_cpu_nanos")
            .ok_or(Error::Internal)?;
        values.push(value);
    }
    fingerprint(&values)
}

fn validate_observed(status: &ObservedStatus) -> Result<()> {
    match status {
        ObservedStatus::Valid { quality_micros } if *quality_micros > 1_000_000 => {
            Err(Error::Invalid("quality micros out of range".into()))
        }
        _ => Ok(()),
    }
}

fn validate_digest(value: &str, name: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(Error::Invalid(format!(
            "{name}: expected lowercase sha256 digest"
        )));
    }
    Ok(())
}
