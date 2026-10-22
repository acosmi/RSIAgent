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
pub const REPLAY_ENGINE_VERSION: &str = "rsia.replay.engine.v3";
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
        let report_id = replay_report_id_for_version(
            ReplayReportIdentityVersion::LegacyV2,
            &namespace,
            partition,
            &pool.pool_digest,
            &policy_digest,
            &profile_digest,
            &caps_digest,
        )?;
        let record = Self {
            schema_version: STORED_REPLAY_REPORT_SCHEMA.into(),
            replay_engine_version: LEGACY_REPLAY_ENGINE_VERSION_V2.into(),
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
            || self.replay_engine_version != LEGACY_REPLAY_ENGINE_VERSION_V2
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
                != replay_report_id_for_version(
                    ReplayReportIdentityVersion::LegacyV2,
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

    pub fn validate_historical_v2_static(&self, pool: &ReplayPoolManifestV1) -> Result<()> {
        if self.schema_version != LEGACY_STORED_REPLAY_REPORT_SCHEMA_V1
            || self.replay_engine_version != LEGACY_REPLAY_ENGINE_VERSION_V2
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
                != replay_report_id_for_version(
                    ReplayReportIdentityVersion::LegacyV2,
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
            validate_legacy_v2_report_member(
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

fn validate_legacy_v2_report_member(
    report: &ReplayReportV2,
    member: &ReplayPoolMemberV1,
    policy_digest: &str,
    profile_digest: &str,
    objective_version: &str,
) -> Result<()> {
    if report.schema_version != LEGACY_REPLAY_REPORT_SCHEMA_V2
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
    replay_report_id_for_version(
        ReplayReportIdentityVersion::NextV3,
        namespace,
        partition,
        pool_digest,
        policy_digest,
        profile_digest,
        caps_digest,
    )
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

// AG147 COMPONENT SOURCE ONLY. Not a v3 producer or semantic verifier.
// These are reviewed static interpretation domains, not caller-supplied labels.
pub const LEGACY_REPLAY_ENGINE_VERSION_V2: &str = "rsia.replay.engine.v2";
pub const LEGACY_STORED_REPLAY_REPORT_SCHEMA_V1: &str = "rsia.stored_replay_report.v1";
pub const LEGACY_REPLAY_REPORT_SCHEMA_V2: &str = "rsia.replay_report.v2";
pub const NEXT_REPLAY_ENGINE_VERSION_V3: &str = "rsia.replay.engine.v3";
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayRecordState {
    CurrentVerified,
    Legacy,
    Invalid,
}
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ReplayHistoricalVersion {
    pub engine_version: &'static str,
    pub stored_schema: &'static str,
    pub member_schema: &'static str,
    pub introduced_change: &'static str,
    pub retains_semantic_verifier: bool,
}
// Closed interpretation table. V2 remains static-only; the engine's current
// row additionally requires real source and semantic verification.
pub const REPLAY_HISTORICAL_VERSIONS: &[ReplayHistoricalVersion] = &[
    ReplayHistoricalVersion {
        engine_version: LEGACY_REPLAY_ENGINE_VERSION_V2,
        stored_schema: LEGACY_STORED_REPLAY_REPORT_SCHEMA_V1,
        member_schema: LEGACY_REPLAY_REPORT_SCHEMA_V2,
        introduced_change: "v2 release had multiple semantic changes; v4.3 §7.4.3 retires every v2 judgment",
        retains_semantic_verifier: false,
    },
    ReplayHistoricalVersion {
        engine_version: NEXT_REPLAY_ENGINE_VERSION_V3,
        stored_schema: STORED_REPLAY_REPORT_SCHEMA_V2,
        member_schema: REPLAY_REPORT_SCHEMA_V3,
        introduced_change: "v4.3 seven-terminal/root-qualified replay",
        retains_semantic_verifier: true,
    },
];
#[derive(Debug, thiserror::Error)]
pub enum ReplayReadIsolation {
    #[error("replay_version_outside_static_table:{engine_version}")]
    VersionOutsideStaticTable { engine_version: String },
    // Unqualified storage input must not reflect an unauthenticated engine
    // string (which itself could contain reversible report/private content).
    #[error("replay_version_outside_static_table:unqualified_record")]
    UnqualifiedVersionOutsideStaticTable,
    #[error("legacy_replay_not_consumable:{report_id}")]
    LegacyNotConsumable { report_id: String },
    #[error("invalid_replay_record:{reason}")]
    InvalidRecord { reason: String },
    #[error("current_replay_reader_not_implemented")]
    CurrentReaderNotImplemented,
    #[error(transparent)]
    Source(#[from] Error),
}
pub type ReplayReadResult<T> = std::result::Result<T, ReplayReadIsolation>;
pub fn replay_historical_version(
    engine_version: &str,
) -> ReplayReadResult<&'static ReplayHistoricalVersion> {
    REPLAY_HISTORICAL_VERSIONS
        .iter()
        .find(|row| row.engine_version == engine_version)
        .ok_or_else(|| ReplayReadIsolation::VersionOutsideStaticTable {
            engine_version: engine_version.into(),
        })
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayReportIdentityVersion {
    LegacyV2,
    NextV3,
}
impl ReplayReportIdentityVersion {
    pub fn engine_version(self) -> &'static str {
        match self {
            Self::LegacyV2 => LEGACY_REPLAY_ENGINE_VERSION_V2,
            Self::NextV3 => NEXT_REPLAY_ENGINE_VERSION_V3,
        }
    }
}
// Non-persistent read result. No Deserialize or public fields/constructor can
// mint CurrentVerified. Only Legacy/Invalid constructors exist in this package.
#[derive(Debug, Clone, Serialize)]
pub struct ReplayStaticReportRead {
    state: ReplayRecordState,
    report: Option<StoredReplayReportV1>,
    invalid_reason: Option<String>,
}
impl ReplayStaticReportRead {
    pub fn state(&self) -> ReplayRecordState {
        self.state
    }
    pub fn source_legacy(&self) -> bool {
        self.state == ReplayRecordState::Legacy
    }
    pub fn report(&self) -> Option<&StoredReplayReportV1> {
        self.report.as_ref()
    }
    pub fn invalid_reason(&self) -> Option<&str> {
        self.invalid_reason.as_deref()
    }
    pub fn invalid(reason: impl Into<String>) -> Self {
        Self {
            state: ReplayRecordState::Invalid,
            report: None,
            invalid_reason: Some(reason.into()),
        }
    }
    pub fn refuse_new_consumption(&self) -> ReplayReadIsolation {
        match self.state {
            ReplayRecordState::Legacy => ReplayReadIsolation::LegacyNotConsumable {
                report_id: self
                    .report
                    .as_ref()
                    .map(|r| r.report_id.clone())
                    .unwrap_or_default(),
            },
            ReplayRecordState::Invalid => ReplayReadIsolation::InvalidRecord {
                reason: self
                    .invalid_reason
                    .clone()
                    .unwrap_or_else(|| "missing static identity".into()),
            },
            ReplayRecordState::CurrentVerified => ReplayReadIsolation::CurrentReaderNotImplemented,
        }
    }
}
pub fn classify_historical_replay_report(
    report: StoredReplayReportV1,
    pool: &ReplayPoolManifestV1,
) -> ReplayReadResult<ReplayStaticReportRead> {
    let row = replay_historical_version(&report.replay_engine_version)?;
    if report.schema_version != row.stored_schema
        || report
            .member_reports
            .iter()
            .any(|m| m.report.schema_version != row.member_schema)
    {
        return Ok(ReplayStaticReportRead::invalid(
            "historical replay schema/engine mixture",
        ));
    }
    // Static identity only: NEVER run the v2 semantic engine, even if v3 would
    // coincidentally agree with the old score or terminal reason.
    match report.validate_historical_v2_static(pool) {
        Ok(()) => Ok(ReplayStaticReportRead {
            state: ReplayRecordState::Legacy,
            report: Some(report),
            invalid_reason: None,
        }),
        Err(error) => Ok(ReplayStaticReportRead::invalid(error.to_string())),
    }
}

/// Shared historical identity part for the future lifecycle adapter. No pool
/// or semantics is invented; full reader coverage still needs the actual pool.
pub fn validate_historical_v2_report_without_pool(report: &StoredReplayReportV1) -> Result<()> {
    if report.schema_version != LEGACY_STORED_REPLAY_REPORT_SCHEMA_V1
        || report.replay_engine_version != LEGACY_REPLAY_ENGINE_VERSION_V2
        || report.purpose != Purpose::Development
    {
        return Err(Error::Invalid(
            "unsupported historical v2 report wire".into(),
        ));
    }
    identifier(&report.report_id)?;
    identifier(&report.namespace)?;
    report.policy.validate()?;
    report.profile.validate(&report.caps)?;
    if report.policy_digest != fingerprint(&report.policy)?
        || report.profile_digest != fingerprint(&report.profile)?
        || report.caps_digest != fingerprint(&report.caps)?
        || report.reports_body_digest != fingerprint(&report.member_reports)?
        || report.semantic_reports_digest != replay_report_semantic_digest(&report.member_reports)?
        || report.report_id
            != replay_report_id_for_version(
                ReplayReportIdentityVersion::LegacyV2,
                &report.namespace,
                report.partition,
                &report.pool_digest,
                &report.policy_digest,
                &report.profile_digest,
                &report.caps_digest,
            )?
    {
        return Err(Error::Invalid(
            "historical v2 report digest or identity mismatch".into(),
        ));
    }
    let mut seen = BTreeSet::new();
    for member in &report.member_reports {
        if !seen.insert(member.world_id.as_str()) {
            return Err(Error::Invalid("duplicate historical world".into()));
        }
        let binding = ReplayPoolMemberV1 {
            world_id: member.world_id.clone(),
            world_digest: member.world_digest.clone(),
            cluster_id: member.report.cluster_id.clone(),
            partition: report.partition,
        };
        validate_legacy_v2_report_member(
            &member.report,
            &binding,
            &report.policy_digest,
            &report.profile_digest,
            report.profile.objective.version(),
        )?;
    }
    Ok(())
}

pub fn replay_report_id_for_version(
    version: ReplayReportIdentityVersion,
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
            version.engine_version(),
        ))?
    ))
}

// AG147 R5: pure registration material. Deserializing it grants no capability.
pub const REPLAY_ROOT_SOURCE_SCHEMA: &str = "rsia.replay.root_source.v1";
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayRootOpportunityV1 {
    pub root_slot: u32,
    pub branch_seq: u32,
    pub action_seq: u32,
    pub estimated_cost_upper_micros: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayRootArtifactRefV1 {
    pub kind: String,
    pub id: String,
    pub schema_version: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayRootOriginalRequestRefV1 {
    pub kind: String,
    pub id: String,
    pub schema_version: String,
    pub body_digest: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "proof_kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReplayRootRegistrationEvidenceV1 {
    ManagementOriginal {
        job_id: String,
        original_request_ref: ReplayRootOriginalRequestRefV1,
        payload_digest: String,
    },
    RegisteredSourceChain {
        source_closure_digest: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayRootSourceV1 {
    pub schema_version: String,
    pub namespace: String,
    pub exploration_world_id: String,
    pub world_artifact_ref: ReplayRootArtifactRefV1,
    pub registration_digest: String,
    pub registration_evidence: ReplayRootRegistrationEvidenceV1,
    pub registration_image_digest: String,
    pub source_watermark: u64,
    pub roots: Vec<ReplayRootOpportunityV1>,
    pub structure_digest: String,
}
impl ReplayRootSourceV1 {
    pub fn expected_structure_digest(&self) -> Result<String> {
        fingerprint(&(
            REPLAY_ROOT_SOURCE_SCHEMA,
            &self.namespace,
            &self.world_artifact_ref,
            &self.registration_digest,
            &self.registration_evidence,
            &self.registration_image_digest,
            self.source_watermark,
            &self.roots,
        ))
    }
    pub fn validate_material(&self, registration_image_utf8: &[u8]) -> Result<()> {
        let invalid = || Error::Invalid("invalid replay root registration material".into());
        if self.schema_version != REPLAY_ROOT_SOURCE_SCHEMA
            || self.world_artifact_ref.kind != "artifact"
            || self.world_artifact_ref.schema_version != "rsia.exploration_world.v1"
            || self.registration_image_digest != crate::hash(registration_image_utf8)
            || self.world_artifact_ref.id
                != format!(
                    "e09-{}",
                    fingerprint(&("exploration_world_v1", &self.exploration_world_id))?
                )
        {
            return Err(invalid());
        }
        identifier(&self.namespace)?;
        identifier(&self.exploration_world_id)?;
        for value in [
            &self.registration_digest,
            &self.registration_image_digest,
            &self.structure_digest,
        ] {
            validate_digest(value, "replay root registration digest")?;
        }
        let (mut slots, mut branches, mut actions) =
            (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        let mut previous = None;
        for root in &self.roots {
            if root.root_slot == 0
                || root.branch_seq == 0
                || root.action_seq == 0
                || root.action_seq >= 1_000_000
                || root.estimated_cost_upper_micros == 0
                || !slots.insert(root.root_slot)
                || !branches.insert(root.branch_seq)
                || !actions.insert(root.action_seq)
                || previous.is_some_and(|slot| root.root_slot <= slot)
            {
                return Err(invalid());
            }
            previous = Some(root.root_slot);
        }
        match &self.registration_evidence {
            ReplayRootRegistrationEvidenceV1::ManagementOriginal {
                job_id,
                original_request_ref,
                payload_digest,
            } => {
                identifier(job_id)?;
                identifier(&original_request_ref.id)?;
                if original_request_ref.kind != "artifact"
                    || original_request_ref.schema_version != "rsia.management_private_input.v1"
                {
                    return Err(invalid());
                }
                validate_digest(
                    &original_request_ref.body_digest,
                    "management original body digest",
                )?;
                validate_digest(payload_digest, "management payload digest")?;
            }
            ReplayRootRegistrationEvidenceV1::RegisteredSourceChain {
                source_closure_digest,
            } => {
                validate_digest(source_closure_digest, "root source closure digest")?;
            }
        }
        if self.expected_structure_digest()? != self.structure_digest {
            return Err(invalid());
        }
        Ok(())
    }
}

// AG147 R5 current world and canonical observation contracts.
// Old V2/V1 declarations, serialization and digest functions remain unchanged.
pub const REPLAY_WORLD_SCHEMA_V3: &str = "rsia.replay_world.v3";
pub const REPLAY_MANIFEST_SCHEMA_V3: &str = "rsia.replay_manifest.v3";
pub const REPLAY_BASELINE_OBSERVATION_SCHEMA_V2: &str = "rsia.replay_baseline_observation.v2";
pub const REPLAY_OBSERVATION_SCHEMA_V3: &str = "rsia.replay_observation.v3";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayWorldManifestV3 {
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
    pub root_source: ReplayRootSourceV1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayWorldV3 {
    pub schema_version: String,
    pub manifest: ReplayWorldManifestV3,
    pub transitions: Vec<ReplayTransitionV2>,
    /// Exact minimized capture bytes, never a raw private management request.
    pub registration_image_utf8: String,
    pub sealed_digest: Option<String>,
}

impl ReplayWorldV3 {
    pub fn validate_draft(&self) -> Result<()> {
        if self.sealed_digest.is_some() {
            return Err(Error::Invalid("draft world already has a seal".into()));
        }
        self.validate_unsealed()
    }
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
        if self.schema_version != REPLAY_WORLD_SCHEMA_V3
            || self.manifest.schema_version != REPLAY_MANIFEST_SCHEMA_V3
        {
            return Err(Error::Invalid("unsupported replay world schema".into()));
        }
        let manifest = &self.manifest;
        manifest
            .root_source
            .validate_material(self.registration_image_utf8.as_bytes())?;
        if manifest.world_id != replay_world_identity_v3(manifest)?
            || manifest.revoke_watermark != manifest.root_source.source_watermark
        {
            return Err(Error::Conflict(
                "replay V3 identity/root watermark differs".into(),
            ));
        }
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
        if baseline_source.content_digest != replay_baseline_observation_digest_v2(manifest)? {
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
        for action in &manifest.action_catalog {
            if let ActionKindV1::Widen { root_slot } = &action.action_kind {
                let root = manifest
                    .root_source
                    .roots
                    .iter()
                    .find(|root| root.root_slot == *root_slot)
                    .ok_or_else(|| {
                        Error::Invalid("replay Widen is outside registered roots".into())
                    })?;
                if action.record_seq != root.action_seq
                    || action.branch_seq != root.branch_seq
                    || action.target_depth != 1
                    || action.parent_context_signature != manifest.baseline_context_signature
                    || action.estimated_cost_upper_micros != Some(root.estimated_cost_upper_micros)
                {
                    return Err(Error::Conflict(
                        "replay Widen differs from registered root".into(),
                    ));
                }
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
            if observation_source.content_digest
                != replay_observation_digest_v3(manifest, transition)?
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
            manifest: &'a ReplayWorldManifestV3,
            transitions: &'a [ReplayTransitionV2],
            registration_image_utf8: &'a str,
        }
        fingerprint(&Content {
            schema_version: &self.schema_version,
            manifest: &self.manifest,
            transitions: &self.transitions,
            registration_image_utf8: &self.registration_image_utf8,
        })
    }
}

/// Pure deterministic identity. The closed flat array is the exact ordered
/// identity tuple; it contains no observation id, outcome, score, clock or seal.
pub fn replay_world_identity_v3(manifest: &ReplayWorldManifestV3) -> Result<String> {
    let material = serde_json::json!([
        "rsia.replay_world.identity.v3",
        &manifest.root_source.namespace,
        &manifest.cluster_id,
        manifest.partition,
        Purpose::Development,
        &manifest.generation_signature,
        &manifest.world_context_signature,
        &manifest.baseline_context_signature,
        &manifest.approved_parent_digest,
        &manifest.model_digest,
        &manifest.tools_digest,
        &manifest.scorer_digest,
        &manifest.guidance_digest,
        &manifest.repair_template_digest,
        &manifest.input_order_digest,
        manifest.initial_baseline_quality_micros,
        manifest.revoke_watermark,
        &manifest.root_source.structure_digest
    ]);
    Ok(format!("replay-world-{}", fingerprint(&material)?))
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct ReplayBaselineObservationMaterialV2<'a> {
    schema_version: &'static str,
    observation_kind: &'static str,
    world_id: &'a str,
    partition: WorldPartition,
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
    root_source: &'a ReplayRootSourceV1,
    registration_image_digest: &'a str,
}

/// Canonical body only. This function never creates a run or a TrustedHost label.
/// The collector must find these exact bytes in an actual live trusted run.
pub fn replay_baseline_observation_bytes_v2(manifest: &ReplayWorldManifestV3) -> Result<Vec<u8>> {
    serde_json::to_vec(&ReplayBaselineObservationMaterialV2 {
        schema_version: REPLAY_BASELINE_OBSERVATION_SCHEMA_V2,
        observation_kind: "observed",
        world_id: &manifest.world_id,
        partition: manifest.partition,
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
        root_source: &manifest.root_source,
        registration_image_digest: &manifest.root_source.registration_image_digest,
    })
    .map_err(|_| Error::Internal)
}
pub fn replay_baseline_observation_digest_v2(manifest: &ReplayWorldManifestV3) -> Result<String> {
    Ok(crate::hash(&replay_baseline_observation_bytes_v2(
        manifest,
    )?))
}

pub fn replay_observation_bytes_v3(
    manifest: &ReplayWorldManifestV3,
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
        world_id: &'a str,
        partition: WorldPartition,
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
        root_structure_digest: &'a str,
        registration_image_digest: &'a str,
    }
    let baseline_source = manifest
        .source_closure
        .iter()
        .find(|source| source.source_id == manifest.baseline_observation_source_id)
        .ok_or_else(|| Error::Invalid("baseline observation source is absent".into()))?;
    let mut source_material = Vec::with_capacity(transition.source_ids.len());
    for source_id in &transition.source_ids {
        // The source is produced AFTER these canonical bytes are prepared.
        // Its own digest is deliberately absent, so no placeholder source or
        // self-hash cycle is necessary. Whole-world validation later requires
        // its real live run and exact actual digest in source_closure.
        if source_id == &transition.observation_source_id {
            source_material.push(SourceMaterial {
                source_id,
                content_digest: None,
            });
            continue;
        }
        let source = manifest
            .source_closure
            .iter()
            .find(|source| source.source_id == *source_id)
            .ok_or_else(|| Error::Invalid("transition source is absent".into()))?;
        source_material.push(SourceMaterial {
            source_id,
            content_digest: Some(source.content_digest.as_str()),
        });
    }
    source_material.sort_by(|left, right| left.source_id.cmp(right.source_id));
    serde_json::to_vec(&Material {
        schema_version: REPLAY_OBSERVATION_SCHEMA_V3,
        world_id: &manifest.world_id,
        partition: manifest.partition,
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
        root_structure_digest: &manifest.root_source.structure_digest,
        registration_image_digest: &manifest.root_source.registration_image_digest,
    })
    .map_err(|_| Error::Internal)
}

pub fn replay_observation_digest_v3(
    manifest: &ReplayWorldManifestV3,
    transition: &ReplayTransitionV2,
) -> Result<String> {
    Ok(crate::hash(&replay_observation_bytes_v3(
        manifest, transition,
    )?))
}

// AG147 R5 core report component, composed after the reviewed historical reader.
// Old V2/V1 DTOs stay byte-identical. This is NOT a source-qualified WorldV3
// producer, a CurrentVerified constructor, or an authorization token.
pub const REPLAY_REPORT_SCHEMA_V3: &str = "rsia.replay_report.v3";
pub const STORED_REPLAY_REPORT_SCHEMA_V2: &str = "rsia.stored_replay_report.v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayRootContractState {
    ProvedClosed,
    Unknown,
}

/// Counts excluded currently revealed catalog opportunities at PolicyStop.
/// Each action is counted once at the first actual shared-core exclusion.
/// New V3 wire requires all six fields; historical V2 has none of these fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ReplayPolicyStopExclusions {
    pub depth_limit: u32,
    pub node_limit: u32,
    pub no_authorized_cost_bound: u32,
    pub insufficient_balance: u32,
    pub recovery_ineligible: u32,
    pub structurally_unavailable: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayReportV3 {
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
    pub policy_stop_exclusions: ReplayPolicyStopExclusions,
    pub root_contract_state: ReplayRootContractState,
    pub development_only: bool,
    pub revealed_prefix: Option<PrefixViewV2>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayReportMemberV2 {
    pub world_id: String,
    pub world_digest: String,
    pub report: ReplayReportV3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayFailureKind {
    Invalid,
    Forbidden,
    Missing,
    Conflict,
    Budget,
    Cancelled,
    ClockRegressed,
    Internal,
}
impl ReplayFailureKind {
    pub fn from_error(error: &Error) -> Self {
        match error {
            Error::Invalid(_) => Self::Invalid,
            Error::Forbidden => Self::Forbidden,
            Error::NotFound => Self::Missing,
            Error::Conflict(_) => Self::Conflict,
            Error::Budget => Self::Budget,
            Error::Cancelled => Self::Cancelled,
            Error::ClockRegressed => Self::ClockRegressed,
            Error::Internal => Self::Internal,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayWorldFailure {
    pub world_id: String,
    pub world_digest: String,
    pub error_kind: ReplayFailureKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredReplayReportV2 {
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
    pub member_reports: Vec<ReplayReportMemberV2>,
    /// Every failed world stays in this partition's attempted denominator.
    pub failed_worlds: Vec<ReplayWorldFailure>,
    pub attempted_worlds: u32,
    pub reports_body_digest: String,
    pub semantic_reports_digest: String,
}

impl StoredReplayReportV2 {
    pub fn build(
        namespace: impl Into<String>,
        partition: WorldPartition,
        pool: &ReplayPoolManifestV1,
        policy: ElasticPolicyV1,
        profile: ReplaySimulationProfile,
        caps: ExplorationCapsV1,
        reports: Vec<ReplayReportV3>,
    ) -> Result<Self> {
        Self::build_with_failures(
            namespace,
            partition,
            pool,
            policy,
            profile,
            caps,
            reports,
            Vec::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_with_failures(
        namespace: impl Into<String>,
        partition: WorldPartition,
        pool: &ReplayPoolManifestV1,
        policy: ElasticPolicyV1,
        profile: ReplaySimulationProfile,
        caps: ExplorationCapsV1,
        reports: Vec<ReplayReportV3>,
        mut failed_worlds: Vec<ReplayWorldFailure>,
    ) -> Result<Self> {
        let namespace = namespace.into();
        identifier(&namespace)?;
        pool.validate_identity()?;
        policy.validate()?;
        profile.validate_for_pool(&caps, pool)?;
        let policy_digest = fingerprint(&policy)?;
        let profile_digest = fingerprint(&profile)?;
        let caps_digest = fingerprint(&caps)?;
        let mut member_reports: Vec<_> = reports
            .into_iter()
            .map(|report| ReplayReportMemberV2 {
                world_id: report.world_id.clone(),
                world_digest: report.world_digest.clone(),
                report,
            })
            .collect();
        member_reports.sort_by(|a, b| a.world_id.cmp(&b.world_id));
        failed_worlds.sort_by(|a, b| a.world_id.cmp(&b.world_id));
        let attempted_worlds = u32::try_from(
            member_reports
                .len()
                .checked_add(failed_worlds.len())
                .ok_or_else(|| Error::Invalid("attempted world count overflow".into()))?,
        )
        .map_err(|_| Error::Invalid("attempted world count overflow".into()))?;
        let reports_body_digest =
            fingerprint(&(&member_reports, &failed_worlds, attempted_worlds))?;
        let semantic_reports_digest = replay_report_v3_record_semantic_digest(
            &member_reports,
            &failed_worlds,
            attempted_worlds,
        )?;
        let record = Self {
            schema_version: STORED_REPLAY_REPORT_SCHEMA_V2.into(),
            replay_engine_version: NEXT_REPLAY_ENGINE_VERSION_V3.into(),
            report_id: replay_report_id_for_version(
                ReplayReportIdentityVersion::NextV3,
                &namespace,
                partition,
                &pool.pool_digest,
                &policy_digest,
                &profile_digest,
                &caps_digest,
            )?,
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
            failed_worlds,
            attempted_worlds,
            reports_body_digest,
            semantic_reports_digest,
        };
        record.validate_static(pool)?;
        Ok(record)
    }

    pub fn validate_static(&self, pool: &ReplayPoolManifestV1) -> Result<()> {
        validate_current_v3_report_without_pool(self)?;
        pool.validate_identity()?;
        self.profile.validate_for_pool(&self.caps, pool)?;
        if self.pool_digest != pool.pool_digest {
            return Err(Error::Conflict("current replay report pool differs".into()));
        }
        let expected: Vec<_> = pool
            .members
            .iter()
            .filter(|m| m.partition == self.partition)
            .collect();
        if expected.len() != usize::try_from(self.attempted_worlds).map_err(|_| Error::Internal)? {
            return Err(Error::Conflict(
                "replay report changed the attempted-world denominator".into(),
            ));
        }
        let mut actual = BTreeMap::new();
        for member in &self.member_reports {
            if actual
                .insert(member.world_id.as_str(), member.world_digest.as_str())
                .is_some()
            {
                return Err(Error::Conflict("duplicate successful world".into()));
            }
            let binding = pool
                .members
                .iter()
                .find(|m| m.world_id == member.world_id)
                .ok_or_else(|| Error::Conflict("report world is outside the pool".into()))?;
            validate_report_v3_member(
                &member.report,
                binding,
                &self.policy_digest,
                &self.profile_digest,
                &self.profile,
                &self.caps,
            )?;
        }
        for failed in &self.failed_worlds {
            if actual
                .insert(failed.world_id.as_str(), failed.world_digest.as_str())
                .is_some()
            {
                return Err(Error::Conflict("failed world duplicates a report".into()));
            }
        }
        for member in expected {
            if actual.get(member.world_id.as_str()).copied() != Some(member.world_digest.as_str()) {
                return Err(Error::Conflict(
                    "replay attempt omits or alters a partition world".into(),
                ));
            }
        }
        Ok(())
    }
}

/// Header/body/identity checks usable by static restore/cleanup. They never
/// grant current consumption: the engine must also recompute over live sources.
pub fn validate_current_v3_report_without_pool(report: &StoredReplayReportV2) -> Result<()> {
    if report.schema_version != STORED_REPLAY_REPORT_SCHEMA_V2
        || report.replay_engine_version != NEXT_REPLAY_ENGINE_VERSION_V3
        || report.purpose != Purpose::Development
    {
        return Err(Error::Invalid(
            "unsupported current replay report wire".into(),
        ));
    }
    identifier(&report.namespace)?;
    identifier(&report.report_id)?;
    validate_digest(&report.pool_digest, "replay pool digest")?;
    report.policy.validate()?;
    report.profile.validate(&report.caps)?;
    if report.profile.pool_digest != report.pool_digest
        || report.policy_digest != fingerprint(&report.policy)?
        || report.profile_digest != fingerprint(&report.profile)?
        || report.caps_digest != fingerprint(&report.caps)?
        || report.reports_body_digest
            != fingerprint(&(
                &report.member_reports,
                &report.failed_worlds,
                report.attempted_worlds,
            ))?
        || report.semantic_reports_digest
            != replay_report_v3_record_semantic_digest(
                &report.member_reports,
                &report.failed_worlds,
                report.attempted_worlds,
            )?
        || report.report_id
            != replay_report_id_for_version(
                ReplayReportIdentityVersion::NextV3,
                &report.namespace,
                report.partition,
                &report.pool_digest,
                &report.policy_digest,
                &report.profile_digest,
                &report.caps_digest,
            )?
    {
        return Err(Error::Invalid(
            "current replay report digest or identity mismatch".into(),
        ));
    }
    let count = report
        .member_reports
        .len()
        .checked_add(report.failed_worlds.len())
        .ok_or_else(|| Error::Invalid("attempted world count overflow".into()))?;
    if count == 0
        || count > MAX_REPLAY_POOL_WORLDS
        || u32::try_from(count).ok() != Some(report.attempted_worlds)
    {
        return Err(Error::Invalid(
            "current replay attempt coverage is invalid".into(),
        ));
    }
    let mut seen = BTreeSet::new();
    let mut previous = None;
    for member in &report.member_reports {
        if !seen.insert(member.world_id.as_str())
            || previous.is_some_and(|id: &str| id >= member.world_id.as_str())
        {
            return Err(Error::Invalid(
                "duplicate or unordered current world reports".into(),
            ));
        }
        previous = Some(member.world_id.as_str());
        let binding = ReplayPoolMemberV1 {
            world_id: member.world_id.clone(),
            world_digest: member.world_digest.clone(),
            cluster_id: member.report.cluster_id.clone(),
            partition: report.partition,
        };
        validate_report_v3_member(
            &member.report,
            &binding,
            &report.policy_digest,
            &report.profile_digest,
            &report.profile,
            &report.caps,
        )?;
    }
    previous = None;
    for failed in &report.failed_worlds {
        identifier(&failed.world_id)?;
        validate_digest(&failed.world_digest, "failed world digest")?;
        if !seen.insert(failed.world_id.as_str())
            || previous.is_some_and(|id: &str| id >= failed.world_id.as_str())
        {
            return Err(Error::Invalid(
                "duplicate or unordered failed world attempts".into(),
            ));
        }
        previous = Some(failed.world_id.as_str());
    }
    Ok(())
}

fn validate_report_v3_member(
    report: &ReplayReportV3,
    member: &ReplayPoolMemberV1,
    policy_digest: &str,
    profile_digest: &str,
    profile: &ReplaySimulationProfile,
    caps: &ExplorationCapsV1,
) -> Result<()> {
    if report.schema_version != REPLAY_REPORT_SCHEMA_V3
        || report.world_id != member.world_id
        || report.world_digest != member.world_digest
        || report.cluster_id != member.cluster_id
        || report.partition != member.partition
        || report.policy_digest != policy_digest
        || report.profile_digest != profile_digest
        || report.objective_version != profile.objective.version()
        || !report.development_only
    {
        return Err(Error::Conflict(
            "replay report member binding mismatch".into(),
        ));
    }
    identifier(&member.world_id)?;
    identifier(&member.cluster_id)?;
    validate_digest(&member.world_digest, "replay member world digest")?;
    let complete = matches!(
        report.terminal,
        ReplayTerminal::PolicyStop | ReplayTerminal::BudgetExhausted
    );
    if report.coverage.complete_support != complete
        || report.probes > u32::from(profile.probe_budget)
        || report.probes
            != report
                .coverage
                .observed_actions
                .checked_add(report.coverage.censored_actions)
                .ok_or_else(|| Error::Invalid("replay coverage count overflow".into()))?
        || report.coverage.attempted_actions < report.probes
        || (complete && report.coverage.attempted_actions != report.probes)
        || report.no_probe != (report.probes == 0)
        || report.parallel_penalty
            != (report.probes > 0).then_some(RationalValue {
                numerator: report.simulated_rounds,
                denominator: report.probes,
            })
        || report.final_quality_micros > 1_000_000
        || report
            .probe_best_quality_micros
            .iter()
            .chain(&report.round_best_quality_micros)
            .any(|q| *q > 1_000_000)
        || (complete
            && (report.attainment_auc_micros.is_none() || report.q_auc_sim_micros.is_none()))
        || (!complete
            && (report.attainment_auc_micros.is_some()
                || report.q_auc_sim_micros.is_some()
                || report.score_v2_micros.is_some()))
        || (complete
            && (profile.objective == ReplayObjective::ParetoAttainmentV2)
                != report.score_v2_micros.is_some())
    {
        return Err(Error::Invalid(
            "replay report terminal/count/score invariant failed".into(),
        ));
    }
    let c = &report.policy_stop_exclusions;
    let _ = [
        c.depth_limit,
        c.node_limit,
        c.no_authorized_cost_bound,
        c.insufficient_balance,
        c.recovery_ineligible,
        c.structurally_unavailable,
    ]
    .into_iter()
    .try_fold(0u32, |a, b| {
        a.checked_add(b)
            .ok_or_else(|| Error::Invalid("policy exclusion count overflow".into()))
    })?;
    if report.terminal != ReplayTerminal::PolicyStop && *c != ReplayPolicyStopExclusions::default()
    {
        return Err(Error::Invalid(
            "non-policy terminal has policy exclusions".into(),
        ));
    }
    if let Some(prefix) = &report.revealed_prefix {
        prefix.validate()?;
        if prefix.nodes_used > caps.max_nodes {
            return Err(Error::Invalid("replay prefix exceeds frozen caps".into()));
        }
    }
    if report.root_contract_state == ReplayRootContractState::Unknown
        && complete
        && report.terminal != ReplayTerminal::BudgetExhausted
        && !report
            .revealed_prefix
            .as_ref()
            .is_some_and(|p| p.nodes_used == caps.max_nodes)
    {
        return Err(Error::Invalid(
            "unknown root universe cannot prove policy completion".into(),
        ));
    }
    Ok(())
}

pub fn replay_report_v3_semantic_digest(reports: &[ReplayReportMemberV2]) -> Result<String> {
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
fn replay_report_v3_record_semantic_digest(
    reports: &[ReplayReportMemberV2],
    failed: &[ReplayWorldFailure],
    attempted: u32,
) -> Result<String> {
    fingerprint(&(
        replay_report_v3_semantic_digest(reports)?,
        failed,
        attempted,
    ))
}

/// Existing registration_image.v1, now typed for offline budget reconstruction.
/// This DTO grants no source or Host authority.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayRegistrationImageV1 {
    pub schema_version: String,
    pub registration_projection_utf8: String,
    pub initial_root_budget_micros: u64,
    pub initial_recovery_dispatches: u8,
    pub source_closure: Vec<(String, String, String)>,
}
impl ReplayRegistrationImageV1 {
    pub fn parse_for_source(text: &str, source: &ReplayRootSourceV1) -> Result<Self> {
        // Typed struct decoding rejects duplicate and unknown outer keys before
        // any Value is created. This is not a second generic JSON parser.
        let image: Self = serde_json::from_str(text)
            .map_err(|_| Error::Invalid("invalid replay registration image".into()))?;
        if image.schema_version != "rsia.replay.registration_image.v1"
            || crate::hash(image.registration_projection_utf8.as_bytes())
                != source.registration_digest
            || crate::hash(text.as_bytes()) != source.registration_image_digest
        {
            return Err(Error::Invalid(
                "invalid replay registration image binding".into(),
            ));
        }
        let mut previous = None;
        for (kind, id, digest) in &image.source_closure {
            identifier(kind)?;
            identifier(id)?;
            validate_digest(digest, "registration source pin")?;
            let key = (kind.as_str(), id.as_str());
            if previous.is_some_and(|old| old >= key) {
                return Err(Error::Invalid(
                    "duplicate or unordered registration source pin".into(),
                ));
            }
            previous = Some(key);
        }
        if let ReplayRootRegistrationEvidenceV1::RegisteredSourceChain {
            source_closure_digest,
        } = &source.registration_evidence
            && *source_closure_digest != fingerprint(&image.source_closure)?
        {
            return Err(Error::Conflict(
                "registration source closure differs".into(),
            ));
        }
        Ok(image)
    }
}

impl ReplayPoolManifestV1 {
    /// Pure projection compiler for already qualified worlds of either schema.
    /// This retains the original compiler.v1/member.v1 hash domain and ordering.
    /// The Store, not this DTO method, establishes live-source authority.
    pub fn build_from_projections(
        inputs: &[(ReplayPoolCompatibilityV1, ReplayPoolMemberV1)],
    ) -> Result<Self> {
        if inputs.is_empty() || inputs.len() > MAX_REPLAY_POOL_WORLDS {
            return Err(Error::Invalid(
                "invalid replay pool projection count".into(),
            ));
        }
        let compatibility = inputs[0].0.clone();
        validate_pool_compatibility(&compatibility)?;
        let mut clusters = BTreeMap::new();
        let mut members = Vec::with_capacity(inputs.len());
        for (candidate, member) in inputs {
            if candidate != &compatibility {
                return Err(Error::Conflict(
                    "replay world is incompatible with pool".into(),
                ));
            }
            if let Some(old) = clusters.insert(member.cluster_id.as_str(), member.partition)
                && old != member.partition
            {
                return Err(Error::Conflict(
                    "task cluster crosses train/select partition".into(),
                ));
            }
            members.push(member.clone());
        }
        members.sort_by(|a, b| a.world_id.cmp(&b.world_id));
        let result = Self {
            schema_version: REPLAY_POOL_SCHEMA.into(),
            compiler_version: REPLAY_POOL_COMPILER_VERSION.into(),
            pool_digest: pool_digest(&compatibility, &members)?,
            compatibility,
            members,
        };
        result.validate_identity()?;
        Ok(result)
    }
}
