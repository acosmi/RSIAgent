//! Restricted improver inheritance (plan §9, §9.1; E14 increment 1). Recursion
//! depth 1. The only mechanism class open is the exploration policy; the final
//! grader and every other control-plane value are not writable.
//!
//! Inheritance is shown by real dispatched decisions, never by a flag: the
//! evidence comes from [`MechanismUsageRecordV1`] values that only
//! `PersistentCoordinator::verified_mechanism_usage` can build. This module
//! does not prove that an inherited mechanism is better, and it does not start
//! an Improver job (`meta.start` stays blocked).
//!
//! # MetaTrial fork (plan §9, §5.7; E14.2a, program scope)
//!
//! [`MetaTrialCoordinator::fork`] turns one S0 template world into two
//! exploration worlds, one per improver (the built-in I0 for the old stream and
//! the candidate for the new one), and records the trial that binds them
//! ([`MetaTrialV1`]). Trusted code derives both worlds from the template: the
//! only fields that differ are the world id and the exploration policy, and the
//! request has no way to say anything else per stream. [`verified_meta_trial`]
//! re-reads the stored trial and both worlds and re-checks all of that before it
//! builds a [`MetaTrialView`].
//!
//! Each stream is an ordinary exploration world, so what the existing
//! machinery already keys by world (stage facts, dispatch facts, nodes,
//! development history, model cache keys, budget dispatch group) is disjoint
//! between the streams. Nothing here enqueues a management job.
//!
//! Each stream's spend is read under the trial's billing scope. A stream whose
//! dispatch group has calls under any other scope makes the trial unverifiable
//! (`Conflict`): it spent from a root budget the trial never declared, and
//! reading it under the declared scope would report that spend as zero.
//!
//! Not claimed: that a successor is better or that I1 beats I0 (there is no
//! FormalEvaluation or statistic here); the same evidence step by step (the
//! streams share a source closure and an evidence opportunity declaration, no
//! more); a hard budget per stream (both streams spend one shared root budget,
//! so one stream can exhaust it and starve the other, and an uncertain call of
//! one stream holds the root's concurrency slot); an isolated workspace object;
//! any meta benefit. The Improver approval registry, the next-job binding and
//! the four mechanism digests belong to E14.2b, and `meta.start` to E14.2c.
use crate::evidence::validate_stored_sources;
use crate::exploration::{
    ExplorationWorldV1, MechanismUsageRecordV1, PersistentCoordinator, RootOpportunity, WorldState,
    exploration_world_storage_id, get_record, need_record, put_record, storage_id,
};
use evo_core::contract::assert_candidate_may_write;
use evo_core::improver::ImproverContentV2;
use evo_core::{ArtifactKind, Context, Error, Result, Role, fingerprint, hash, identifier};
use evo_storage::Store;
use evo_storage::budget::BudgetCallState;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const META_DEPTH_CAP: u8 = 1;

/// A restricted Improver candidate: the content it would approve and the digest
/// of the approved parent content it is derived from. `depth` is supplied by
/// the caller; it is not derived from lineage here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaCandidate {
    pub depth: u8,
    pub content: ImproverContentV2,
    pub parent_improver_digest: String,
}

fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl MetaCandidate {
    pub fn validate(&self) -> Result<()> {
        if self.depth > META_DEPTH_CAP {
            return Err(Error::Invalid("meta recursion depth cap is 1".into()));
        }
        self.content.validate()?;
        if !is_lowercase_sha256(&self.parent_improver_digest) {
            return Err(Error::Invalid(
                "parent_improver_digest must be a lowercase sha256 digest".into(),
            ));
        }
        if self.content.content_digest()? == self.parent_improver_digest {
            return Err(Error::Invalid(
                "candidate content equals its parent improver: no change to inherit".into(),
            ));
        }
        Ok(())
    }
}

/// What a job's real dispatched decisions show about one approved mechanism.
/// It records that the approved policy appeared in `decisions` dispatched
/// decisions of one world (with `distinct_action_digests` different actions);
/// it is not evidence that the policy is better.
#[derive(Debug, Clone, Serialize)]
pub struct InheritanceEvidenceV1 {
    pub approved_content_digest: String,
    pub policy_digest: String,
    pub caps_digest: String,
    pub world_id: String,
    pub decisions: usize,
    pub distinct_action_digests: usize,
}

/// Checks that real, already dispatched decisions used the approved mechanism
/// (plan §9.1, V036/V086.d). `usage` must be non-empty, come from a single
/// world, and every record must carry the digest of the approved content's own
/// policy. A job that ran with a different policy (for instance the built-in
/// one) is a `Conflict`; there is no boolean to set and no pointer to swap.
pub fn verify_mechanism_inheritance(
    approved: &ImproverContentV2,
    usage: &[MechanismUsageRecordV1],
) -> Result<InheritanceEvidenceV1> {
    approved.validate()?;
    let approved_content_digest = approved.content_digest()?;
    let policy_digest = approved.exploration_policy().digest()?;
    let Some(first) = usage.first() else {
        return Err(Error::Invalid(
            "no real dispatched decision used the approved mechanism".into(),
        ));
    };
    if usage.iter().any(|record| {
        record.world_id() != first.world_id() || record.caps_digest() != first.caps_digest()
    }) {
        return Err(Error::Invalid(
            "mechanism usage must come from a single exploration world".into(),
        ));
    }
    if usage
        .iter()
        .any(|record| record.policy_digest() != policy_digest)
    {
        return Err(Error::Conflict(
            "job used a different mechanism than the approved improver".into(),
        ));
    }
    let distinct_action_digests: BTreeSet<&str> =
        usage.iter().map(|record| record.action_digest()).collect();
    Ok(InheritanceEvidenceV1 {
        approved_content_digest,
        policy_digest,
        caps_digest: first.caps_digest().to_owned(),
        world_id: first.world_id().to_owned(),
        decisions: usage.len(),
        distinct_action_digests: distinct_action_digests.len(),
    })
}

pub fn cannot_write_protected(field: &str) -> Result<()> {
    assert_candidate_may_write(ArtifactKind::Improver, field)
}

// ---------------------------------------------------------------------------
// MetaTrial fork
// ---------------------------------------------------------------------------

/// Schema of the stored trial record.
pub const META_TRIAL_SCHEMA: &str = "rsia.meta_trial.v1";

/// `record_kind` of a trial inside the exploration artifact envelope
/// (`rsia.exploration_artifact_envelope.v1`). The revocation cleanup classifies
/// an envelope by schema and record kind, so `evo-storage` lists this kind
/// beside the world, node, dispatch and history kinds.
pub(crate) const META_TRIAL_RECORD_KIND: &str = "meta_trial_v1";

/// A trial id is the root of two world ids (`{id}-old`, `{id}-new`) and of the
/// node ids derived from them, all of which must stay identifiers (128 bytes).
pub const META_TRIAL_ID_MAX_BYTES: usize = 64;

/// One of the two independent exploration streams of a trial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetaStream {
    /// `S_old_next = I_old(S0, E0, B)`: the built-in improver I0.
    Old,
    /// `S_new_next = I_new(S0, E0, B)`: the candidate improver.
    New,
}

impl MetaStream {
    pub const BOTH: [Self; 2] = [Self::Old, Self::New];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Old => "old",
            Self::New => "new",
        }
    }
}

/// Id of the exploration world that carries `stream` of a trial. A pure
/// function of the trial id: the same trial always names the same two worlds,
/// and the two worlds never share an id.
pub fn stream_world_id(trial_id: &str, stream: MetaStream) -> String {
    format!("{trial_id}-{}", stream.as_str())
}

/// Request id a stream uses for its `step`-th optimization step. A pure,
/// deterministic function of `(trial_id, stream, step)` that differs between
/// the two streams for every trial and step, because the stream is part of both
/// the readable prefix and the hashed input. Two streams must never share a
/// request id: the budget layer keys a call by it and refuses (`Conflict`,
/// fail closed) a call id reused with another effective model request, which
/// would surface as an uncertain dispatch instead of an independent stream.
pub fn stream_request_id(trial_id: &str, stream: MetaStream, step: u32) -> String {
    let digest = hash(
        format!(
            "rsia.meta_trial.request.v1\n{trial_id}\n{}\n{step}",
            stream.as_str()
        )
        .as_bytes(),
    );
    format!("mt-{}-{step}-{}", stream.as_str(), &digest[..32])
}

/// The limits both streams declare, taken from the S0 template. They are the
/// same for both streams by construction; what each stream really spends is
/// recorded apart and may differ (plan §9: a stream is not forced to spend what
/// the other one does, and is not padded with useless calls to match it).
///
/// A world only knows these as an estimate it was registered with (there is no
/// per-stream budget group quota in the ledger, and the root budget is shared),
/// so the trial keeps its own copy: the world's budget counters move with every
/// dispatch and cannot serve as the immutable declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamLimitsV1 {
    pub remaining_root_micros: u64,
    pub root_opportunities: Vec<RootOpportunity>,
    pub successor_cost_upper_micros: u64,
    pub remaining_recovery_dispatches: u8,
}

/// The stored trial: which two improvers run from one S0, in which worlds and
/// under which declared limits. Written once, never updated.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaTrialV1 {
    pub schema_version: String,
    pub trial_id: String,
    /// Digest of the S0 template's registration facts that no stream may
    /// change (see `s0_registration_digest`), including the declared limits.
    pub s0_template_digest: String,
    /// The old improver. This increment fixes it to the built-in I0.
    pub old_content: ImproverContentV2,
    pub old_content_digest: String,
    pub new_content: ImproverContentV2,
    pub new_content_digest: String,
    pub old_world_id: String,
    pub new_world_id: String,
    pub old_policy_digest: String,
    pub new_policy_digest: String,
    /// The root budget both streams are billed against. No world carries it: it
    /// is the trial's own declaration, and what binds it afterwards is the
    /// ledger. A stream whose dispatch group has calls under any other scope
    /// makes the trial unverifiable (see [`verified_meta_trial`]).
    pub billing_scope: String,
    pub declared_stream_limits: StreamLimitsV1,
    /// The trusted runs of the S0 source closure, in the template's order.
    pub source_closure: Vec<String>,
    pub revoke_watermark: u64,
    pub created_at: i64,
}

impl MetaTrialV1 {
    fn stream_world_id(&self, stream: MetaStream) -> &str {
        match stream {
            MetaStream::Old => &self.old_world_id,
            MetaStream::New => &self.new_world_id,
        }
    }

    fn stream_policy_digest(&self, stream: MetaStream) -> &str {
        match stream {
            MetaStream::Old => &self.old_policy_digest,
            MetaStream::New => &self.new_policy_digest,
        }
    }

    fn stream_content_digest(&self, stream: MetaStream) -> &str {
        match stream {
            MetaStream::Old => &self.old_content_digest,
            MetaStream::New => &self.new_content_digest,
        }
    }

    /// Shape and self-consistency of the record: digests recomputed from the
    /// contents, the old content is the built-in I0, the new one is a depth-1
    /// candidate that differs from it, and the two world ids are the ones
    /// derived from the trial id.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != META_TRIAL_SCHEMA {
            return Err(Error::Invalid("unsupported meta trial schema".into()));
        }
        validate_trial_id(&self.trial_id)?;
        identifier(&self.billing_scope)?;
        for (name, value) in [
            ("s0_template_digest", &self.s0_template_digest),
            ("old_content_digest", &self.old_content_digest),
            ("new_content_digest", &self.new_content_digest),
            ("old_policy_digest", &self.old_policy_digest),
            ("new_policy_digest", &self.new_policy_digest),
        ] {
            if !is_lowercase_sha256(value) {
                return Err(Error::Invalid(format!(
                    "meta trial {name} must be a lowercase sha256 digest"
                )));
            }
        }
        self.old_content.validate()?;
        self.new_content.validate()?;
        if self.old_content.content_digest()? != self.old_content_digest
            || self.old_content_digest != ImproverContentV2::builtin_default().content_digest()?
        {
            return Err(Error::Invalid(
                "the old stream of a meta trial carries the built-in I0 content".into(),
            ));
        }
        if self.new_content.content_digest()? != self.new_content_digest {
            return Err(Error::Invalid(
                "meta trial new content digest does not match its content".into(),
            ));
        }
        MetaCandidate {
            depth: META_DEPTH_CAP,
            content: self.new_content.clone(),
            parent_improver_digest: self.old_content_digest.clone(),
        }
        .validate()?;
        if self.old_content.exploration_policy().digest()? != self.old_policy_digest
            || self.new_content.exploration_policy().digest()? != self.new_policy_digest
        {
            return Err(Error::Invalid(
                "meta trial policy digest does not match its content".into(),
            ));
        }
        for stream in MetaStream::BOTH {
            if self.stream_world_id(stream) != stream_world_id(&self.trial_id, stream) {
                return Err(Error::Invalid(
                    "meta trial world id is not derived from the trial id".into(),
                ));
            }
        }
        if self.source_closure.is_empty() {
            return Err(Error::Invalid(
                "meta trial requires a typed source closure".into(),
            ));
        }
        let mut seen = BTreeSet::new();
        for run in &self.source_closure {
            identifier(run)?;
            if !seen.insert(run.as_str()) {
                return Err(Error::Invalid(
                    "meta trial source closure must list each run once".into(),
                ));
            }
        }
        Ok(())
    }

    /// Digest of everything the request decides. `created_at` is a clock
    /// reading of the first writer and is left out, so replaying the same
    /// request later is the same registration, not a different one.
    fn registration_fingerprint(&self) -> Result<String> {
        let mut registration = self.clone();
        registration.created_at = 0;
        fingerprint(&registration)
    }

    /// Checks one stored world against what this trial declared at fork time.
    /// The registration facts are compared in full. The scheduling state and
    /// the budget counters `run_next` spends move on purpose once a dispatch
    /// started, so for such a world the check is only that it never holds more
    /// budget than was declared (a counter lowered by tampering cannot be told
    /// from a spend without replaying the dispatches); a world no dispatch ever
    /// started in must still carry exactly the declared counters and an initial
    /// schedule.
    fn check_stream_world(&self, stream: MetaStream, world: &ExplorationWorldV1) -> Result<()> {
        let conflict =
            |what: &str| Error::Conflict(format!("meta trial {} stream: {what}", stream.as_str()));
        if world.id != self.stream_world_id(stream) {
            return Err(conflict("world id differs from the trial"));
        }
        if world.policy.digest()? != self.stream_policy_digest(stream) {
            return Err(conflict("world policy is not the stream improver's policy"));
        }
        if world.source_watermark != self.revoke_watermark
            || world.dependencies.len() != self.source_closure.len()
            || world
                .dependencies
                .iter()
                .zip(&self.source_closure)
                .any(|(dependency, run)| dependency.kind != "run" || dependency.id != *run)
        {
            return Err(conflict("world source closure differs from the trial"));
        }
        let limits = &self.declared_stream_limits;
        if fingerprint(&world.root_opportunities)? != fingerprint(&limits.root_opportunities)?
            || world.successor_cost_upper_micros != limits.successor_cost_upper_micros
        {
            return Err(conflict(
                "world cost limits differ from the declared limits",
            ));
        }
        if s0_registration_digest(world, limits)? != self.s0_template_digest {
            return Err(conflict(
                "world registration differs from the S0 the trial was forked from",
            ));
        }
        if world.node_ids.is_empty() && world.dispatch_ids.is_empty() {
            // No dispatch ever started in this world: it still is exactly what
            // was registered, so a counter that is lower than the declaration,
            // or any scheduling state, is not a spend.
            if world.remaining_root_micros != limits.remaining_root_micros
                || world.remaining_recovery_dispatches != limits.remaining_recovery_dispatches
                || world.decision_round != 0
                || world.current_branch_seq.is_some()
                || world.current_branch_focus_actions != 0
                || !world.waits.is_empty()
            {
                return Err(conflict(
                    "world no dispatch started in differs from its registration",
                ));
            }
        } else if world.remaining_root_micros > limits.remaining_root_micros
            || world.remaining_recovery_dispatches > limits.remaining_recovery_dispatches
        {
            return Err(conflict("world holds more budget than the trial declared"));
        }
        Ok(())
    }
}

/// Digest of the registration facts of an S0 world that must be identical in
/// both streams: everything except the world id, the exploration policy and
/// the scheduling state. The two budget counters `run_next` spends
/// (`remaining_root_micros`, `remaining_recovery_dispatches`) are taken from
/// the trial's declared `limits`, not from the world, so the digest of a world
/// that already dispatched still equals the digest taken at fork time.
fn s0_registration_digest(world: &ExplorationWorldV1, limits: &StreamLimitsV1) -> Result<String> {
    fingerprint(&(
        "rsia.meta_trial.s0.v1",
        &world.schema_version,
        &world.approved_parent_digest,
        &world.context_signature,
        (
            &world.parent_skill_digest,
            &world.parent_bundle_digest,
            &world.environment_digest,
            &world.model_digest,
            &world.tools_digest,
            &world.grader_digest,
            &world.rules_digest,
        ),
        world.source_watermark,
        &world.caps,
        world.simulation,
        &world.root_opportunities,
        &world.dependencies,
        (
            world.successor_cost_upper_micros,
            world.initial_baseline_quality_micros,
            limits.remaining_root_micros,
            limits.remaining_recovery_dispatches,
        ),
    ))
}

fn validate_trial_id(trial_id: &str) -> Result<()> {
    identifier(trial_id)?;
    if trial_id.len() > META_TRIAL_ID_MAX_BYTES {
        return Err(Error::Invalid(format!(
            "meta trial id exceeds {META_TRIAL_ID_MAX_BYTES} bytes"
        )));
    }
    Ok(())
}

/// A stored record that fails validation is corruption, which the read side
/// names as a `Conflict`, like every other stored fact that stopped agreeing
/// with itself.
fn stored_trial_invalid(error: Error) -> Error {
    match error {
        Error::Invalid(message) => {
            Error::Conflict(format!("stored meta trial is invalid: {message}"))
        }
        other => other,
    }
}

/// The only input of a fork. It names the trial, one S0 template world, the
/// candidate improver content and the root budget. There is no field that could
/// differ between the two streams: the world ids, the policies and everything
/// else are derived by [`MetaTrialCoordinator::fork`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaTrialForkRequest {
    pub trial_id: String,
    /// The S0 world both streams start from. It must be a fresh, collecting
    /// world carrying the built-in I0 policy. It is only a template: its own id
    /// is not used and it is never registered itself.
    pub s0_template: ExplorationWorldV1,
    pub new_content: ImproverContentV2,
    pub billing_scope: String,
}

struct ForkPlan {
    trial: MetaTrialV1,
    old_world: ExplorationWorldV1,
    new_world: ExplorationWorldV1,
}

impl ForkPlan {
    fn world(&self, stream: MetaStream) -> &ExplorationWorldV1 {
        match stream {
            MetaStream::Old => &self.old_world,
            MetaStream::New => &self.new_world,
        }
    }
}

/// Everything except the id and the policy comes from the template unchanged.
fn derive_stream_world(
    template: &ExplorationWorldV1,
    id: String,
    policy: &evo_core::strategy::ElasticPolicyV1,
) -> ExplorationWorldV1 {
    let mut world = template.clone();
    world.id = id;
    world.policy = policy.clone();
    world
}

/// S0 is a fresh start: nothing was dispatched, observed or scheduled yet.
fn require_fresh_s0(template: &ExplorationWorldV1) -> Result<()> {
    if template.state != WorldState::Collecting
        || !template.node_ids.is_empty()
        || !template.dispatch_ids.is_empty()
        || !template.history_ids.is_empty()
        || template.current_branch_seq.is_some()
        || template.current_branch_focus_actions != 0
        || template.decision_round != 0
        || !template.waits.is_empty()
    {
        return Err(Error::Invalid(
            "the S0 template must be a fresh collecting world".into(),
        ));
    }
    Ok(())
}

impl MetaTrialForkRequest {
    /// Validates the request and derives the trial record and the two worlds.
    /// Pure: nothing is read or written.
    fn plan(&self, created_at: i64) -> Result<ForkPlan> {
        validate_trial_id(&self.trial_id)?;
        identifier(&self.billing_scope)?;
        let template = &self.s0_template;
        template.validate()?;
        require_fresh_s0(template)?;
        let old_content = ImproverContentV2::builtin_default();
        if template.policy.digest()? != old_content.exploration_policy().digest()? {
            return Err(Error::Invalid(
                "the S0 template must carry the built-in I0 policy: the stream policies are derived from the improver contents".into(),
            ));
        }
        // A typed value built in code never went through the byte-level gate:
        // pass it through the strict parse once more, so the size bound, the
        // closed mechanism class and the policy bounds hold for it too.
        let new_content = ImproverContentV2::parse(
            &serde_json::to_vec(&self.new_content).map_err(|_| Error::Internal)?,
        )?;
        let old_content_digest = old_content.content_digest()?;
        MetaCandidate {
            depth: META_DEPTH_CAP,
            content: new_content.clone(),
            parent_improver_digest: old_content_digest.clone(),
        }
        .validate()?;
        let limits = StreamLimitsV1 {
            remaining_root_micros: template.remaining_root_micros,
            root_opportunities: template.root_opportunities.clone(),
            successor_cost_upper_micros: template.successor_cost_upper_micros,
            remaining_recovery_dispatches: template.remaining_recovery_dispatches,
        };
        let old_world = derive_stream_world(
            template,
            stream_world_id(&self.trial_id, MetaStream::Old),
            old_content.exploration_policy(),
        );
        let new_world = derive_stream_world(
            template,
            stream_world_id(&self.trial_id, MetaStream::New),
            new_content.exploration_policy(),
        );
        old_world.validate()?;
        new_world.validate()?;
        let trial = MetaTrialV1 {
            schema_version: META_TRIAL_SCHEMA.into(),
            trial_id: self.trial_id.clone(),
            s0_template_digest: s0_registration_digest(template, &limits)?,
            old_policy_digest: old_content.exploration_policy().digest()?,
            new_policy_digest: new_content.exploration_policy().digest()?,
            new_content_digest: new_content.content_digest()?,
            old_content_digest,
            old_content,
            new_content,
            old_world_id: old_world.id.clone(),
            new_world_id: new_world.id.clone(),
            billing_scope: self.billing_scope.clone(),
            declared_stream_limits: limits,
            source_closure: template
                .dependencies
                .iter()
                .map(|dependency| dependency.id.clone())
                .collect(),
            revoke_watermark: template.source_watermark,
            created_at,
        };
        trial.validate()?;
        Ok(ForkPlan {
            trial,
            old_world,
            new_world,
        })
    }
}

/// Forks one S0 into two independent exploration streams. It is a namespace for
/// the trusted fork; it carries no state.
pub struct MetaTrialCoordinator;

impl MetaTrialCoordinator {
    /// Records the trial and registers its two worlds, old stream first.
    ///
    /// Not one transaction. The trial record and its dependency edges are
    /// written together; the worlds are then registered through the existing
    /// registration, which opens its own transaction (the store has a single
    /// connection, so it cannot be nested in another session) and whose
    /// persistence stays in `exploration.rs` instead of being copied here. A
    /// crash in between leaves a trial without all of its worlds, and
    /// replaying the same request converges: the stored trial must equal the
    /// request's, and each world is registered only when it is absent and
    /// verified against the trial when it is present. A trial is never
    /// reported live without both worlds ([`verified_meta_trial`]).
    ///
    /// The registration fingerprint excludes the budget counters `run_next`
    /// spends; idempotent registration checks the initial budget separately.
    /// This fork verifies a stored world against the trial's immutable facts
    /// and registers a world only when it is absent, rather than re-registering
    /// a stored world.
    ///
    /// The fork reads the root budget (the billing scope must name one of this
    /// namespace) and reserves nothing. It makes no model call and enqueues no
    /// management job: a research event never queues paid work by itself
    /// (plan §9, V034).
    pub async fn fork(
        ctx: &Context,
        store: &Store,
        request: MetaTrialForkRequest,
    ) -> Result<MetaTrialV1> {
        ctx.require(&[Role::Admin, Role::Worker])?;
        let plan = request.plan(evo_core::now())?;
        store
            .root_budget(ctx, &plan.trial.billing_scope)
            .await?
            .ok_or(Error::Budget)?;
        let coordinator = PersistentCoordinator::new(store.clone(), ctx.clone(), ctx.actor())?;
        // A world id the trial derives may already be taken. It is only
        // acceptable when it already is exactly this stream's world; anything
        // else stops the fork before a trial is recorded over it.
        for stream in MetaStream::BOTH {
            let id = &plan.world(stream).id;
            if world_is_stored(ctx, store, id).await? {
                let stored = coordinator.registered_world(id).await?;
                plan.trial.check_stream_world(stream, &stored)?;
            }
        }
        let trial = persist_trial(ctx, store, &plan.trial).await?;
        for stream in MetaStream::BOTH {
            let world = plan.world(stream);
            if !world_is_stored(ctx, store, &world.id).await? {
                coordinator.register_world_idempotent(world.clone()).await?;
            }
        }
        // What the fork returns is a trial whose record and both worlds read back
        // as the fork it made. The ledger is not read here: a stream billed
        // outside the trial's billing scope is refused by `verified_meta_trial`.
        load_verified(ctx, store, &trial.trial_id).await?;
        Ok(trial)
    }
}

async fn world_is_stored(ctx: &Context, store: &Store, world_id: &str) -> Result<bool> {
    let mut session = store.session().await?;
    let stored = session
        .get::<serde_json::Value>(ctx, "artifact", &exploration_world_storage_id(world_id)?)
        .await?
        .is_some();
    session.commit().await?;
    Ok(stored)
}

/// Writes the trial record at most once and its dependency edges. A stored
/// record must equal the request's registration (`created_at` aside) or the
/// call is a `Conflict`. The edges (`trial -> each source run`, `trial -> each
/// world`) are what carry a revoked run to the record: the cleanup walks
/// dependents. They are written in the transaction of the record, and
/// `put_edge` ignores a repeat.
async fn persist_trial(ctx: &Context, store: &Store, trial: &MetaTrialV1) -> Result<MetaTrialV1> {
    let mut session = store.session().await?;
    // No trial is recorded over a closure that is already revoked or missing.
    validate_stored_sources(
        &mut session,
        ctx,
        &trial.source_closure,
        trial.revoke_watermark,
    )
    .await?;
    let stored =
        match get_record::<MetaTrialV1>(&mut session, ctx, META_TRIAL_RECORD_KIND, &trial.trial_id)
            .await?
        {
            Some(existing) => {
                existing.validate().map_err(stored_trial_invalid)?;
                if existing.registration_fingerprint()? != trial.registration_fingerprint()? {
                    return Err(Error::Conflict(
                        "meta trial already exists with different content".into(),
                    ));
                }
                existing
            }
            None => {
                put_record(
                    &mut session,
                    ctx,
                    META_TRIAL_RECORD_KIND,
                    &trial.trial_id,
                    ctx.actor(),
                    trial,
                )
                .await?;
                trial.clone()
            }
        };
    let trial_storage_id = storage_id(META_TRIAL_RECORD_KIND, &stored.trial_id)?;
    for run in &stored.source_closure {
        session
            .put_edge(ctx, "artifact", &trial_storage_id, "run", run)
            .await?;
    }
    for stream in MetaStream::BOTH {
        session
            .put_edge(
                ctx,
                "artifact",
                &trial_storage_id,
                "artifact",
                &exploration_world_storage_id(stored.stream_world_id(stream))?,
            )
            .await?;
    }
    session.commit().await?;
    Ok(stored)
}

/// Reloads the trial and both worlds and re-checks them: the record agrees with
/// itself, each world is live (its source closure and the revoke watermark) and
/// is exactly the stream's world the trial declared. Read-only.
async fn load_verified(ctx: &Context, store: &Store, trial_id: &str) -> Result<MetaTrialV1> {
    let mut session = store.session().await?;
    let trial: MetaTrialV1 =
        need_record(&mut session, ctx, META_TRIAL_RECORD_KIND, trial_id).await?;
    session.commit().await?;
    trial.validate().map_err(stored_trial_invalid)?;
    if trial.trial_id != trial_id {
        return Err(Error::Conflict(
            "stored meta trial differs from its storage identity".into(),
        ));
    }
    let coordinator = PersistentCoordinator::new(store.clone(), ctx.clone(), ctx.actor())?;
    for stream in MetaStream::BOTH {
        let world = coordinator
            .registered_world(trial.stream_world_id(stream))
            .await?;
        trial.check_stream_world(stream, &world)?;
    }
    Ok(trial)
}

/// What one stream has really spent, read from the budget ledger by dispatch
/// group (the group of a stream is its world id) under the trial's billing
/// scope. Nothing is estimated and the two streams are not compared or
/// equalized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StreamSpendView {
    /// Every call row of the stream's dispatch group, whatever its state.
    pub calls: usize,
    pub finalized_calls: usize,
    /// Calls whose outcome and cost are unknown.
    pub uncertain_calls: usize,
    /// Sum of the actual cost of the calls that have one.
    pub actual_cost_micros: i64,
    /// What reserved, dispatched or uncertain calls still hold: possibly spent,
    /// not settled.
    pub unsettled_reserved_micros: i64,
}

/// Reads the stream's calls under the trial's billing scope, in one ledger
/// snapshot with a check that the stream has no call under any other scope.
///
/// Both streams are compared on the same declared ceiling, one root budget, and
/// each stream's spend is read under that scope. A dispatch group is keyed by
/// its scope, so a stream whose group also (or only) has calls under another
/// root drew money from a budget the trial never declared: reading its group
/// under the declared scope would report that spend as zero, a false account.
/// Such a trial is not verifiable: `Conflict`, fail closed. The message is
/// fixed and names only the stream.
async fn stream_spend(
    ctx: &Context,
    store: &Store,
    trial: &MetaTrialV1,
    stream: MetaStream,
) -> Result<StreamSpendView> {
    let world_id = trial.stream_world_id(stream);
    let mut session = store.session().await?;
    let scopes = session.budget_call_scopes_for_group(ctx, world_id).await?;
    if scopes.iter().any(|scope| *scope != trial.billing_scope) {
        return Err(Error::Conflict(format!(
            "meta trial {} stream: its dispatch group was billed outside the trial's billing scope",
            stream.as_str()
        )));
    }
    let calls = session
        .budget_calls_for_group(ctx, &trial.billing_scope, world_id)
        .await?;
    session.commit().await?;
    let overflow = || Error::Conflict("stream spend overflows the ledger's integer range".into());
    let mut finalized_calls = 0;
    let mut uncertain_calls = 0;
    let mut actual_cost_micros = 0i64;
    let mut unsettled_reserved_micros = 0i64;
    for call in &calls {
        match call.state {
            BudgetCallState::Finalized => finalized_calls += 1,
            BudgetCallState::Uncertain => uncertain_calls += 1,
            _ => {}
        }
        if let Some(actual) = call.actual_cost_micros {
            actual_cost_micros = actual_cost_micros
                .checked_add(actual)
                .ok_or_else(overflow)?;
        }
        if matches!(
            call.state,
            BudgetCallState::Reserved | BudgetCallState::Dispatched | BudgetCallState::Uncertain
        ) {
            unsettled_reserved_micros = unsettled_reserved_micros
                .checked_add(call.reserved_micros)
                .ok_or_else(overflow)?;
        }
    }
    Ok(StreamSpendView {
        calls: calls.len(),
        finalized_calls,
        uncertain_calls,
        actual_cost_micros,
        unsettled_reserved_micros,
    })
}

/// What a trial's data says so far (plan §5.7: a trial without a successor call
/// is only a candidate change). It is derived on every read from verified
/// facts, never stored and never set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetaTrialStatus {
    /// Neither stream has a verified [`MechanismUsageRecordV1`]: no dispatched
    /// decision was taken with either policy, so the trial is only a candidate
    /// change.
    CandidateOnly,
    /// At least one stream has a verified usage record: its frozen policy
    /// appeared in a dispatched decision. That shows the mechanism was used,
    /// not that its successor is better, and it is not a comparison.
    UsageObserved,
}

/// One stream of a verified trial.
#[derive(Debug, Clone, Serialize)]
pub struct MetaStreamView {
    stream: MetaStream,
    world_id: String,
    content_digest: String,
    policy_digest: String,
    verified_usage: usize,
    spend: StreamSpendView,
    dispatch_group_stopped: bool,
}

impl MetaStreamView {
    pub fn stream(&self) -> MetaStream {
        self.stream
    }

    pub fn world_id(&self) -> &str {
        &self.world_id
    }

    pub fn content_digest(&self) -> &str {
        &self.content_digest
    }

    pub fn policy_digest(&self) -> &str {
        &self.policy_digest
    }

    /// Number of verified mechanism usage records of the stream's world: real
    /// dispatches taken with the stream's frozen policy.
    pub fn verified_usage(&self) -> usize {
        self.verified_usage
    }

    pub fn spend(&self) -> &StreamSpendView {
        &self.spend
    }

    /// Whether the stream's budget dispatch group was stopped. Stopping one
    /// stream's group does not stop the other's.
    pub fn dispatch_group_stopped(&self) -> bool {
        self.dispatch_group_stopped
    }
}

/// A trial re-verified from the store (plan §9, V033/V035). Derived and
/// read-only: it cannot be deserialized, its fields are private, and
/// [`verified_meta_trial`] is the only place that builds one.
#[derive(Debug, Clone, Serialize)]
pub struct MetaTrialView {
    trial_id: String,
    status: MetaTrialStatus,
    s0_template_digest: String,
    billing_scope: String,
    declared_stream_limits: StreamLimitsV1,
    source_closure: Vec<String>,
    revoke_watermark: u64,
    old: MetaStreamView,
    new: MetaStreamView,
}

impl MetaTrialView {
    pub fn trial_id(&self) -> &str {
        &self.trial_id
    }

    pub fn status(&self) -> MetaTrialStatus {
        self.status
    }

    pub fn s0_template_digest(&self) -> &str {
        &self.s0_template_digest
    }

    pub fn billing_scope(&self) -> &str {
        &self.billing_scope
    }

    /// The limits both streams declared. They are equal for the two streams;
    /// what each one spent is in [`MetaStreamView::spend`].
    pub fn declared_stream_limits(&self) -> &StreamLimitsV1 {
        &self.declared_stream_limits
    }

    pub fn source_closure(&self) -> &[String] {
        &self.source_closure
    }

    pub fn revoke_watermark(&self) -> u64 {
        self.revoke_watermark
    }

    pub fn old_stream(&self) -> &MetaStreamView {
        &self.old
    }

    pub fn new_stream(&self) -> &MetaStreamView {
        &self.new
    }

    pub fn stream(&self, stream: MetaStream) -> &MetaStreamView {
        match stream {
            MetaStream::Old => &self.old,
            MetaStream::New => &self.new,
        }
    }
}

/// Re-reads a trial from the store and verifies it before describing it
/// (plan §9, V033/V035).
///
/// The stored record must agree with itself; both worlds must be live (the
/// revoke watermark and the source closure: drift is `Conflict`, a tombstoned
/// source `Forbidden`, a redacted record is named as a `Conflict`); each world
/// must carry the policy of its stream's improver; and the registration facts
/// of both worlds, everything except the id, the policy and the scheduling
/// state, must still equal the S0 the trial was forked from. Each stream's spend
/// is read from the ledger by dispatch group under the trial's billing scope and
/// may differ from the other's. A stream whose dispatch group has a call under
/// any other billing scope drew money from a root budget the trial never
/// declared, so the trial is not verifiable: `Conflict`, with a fixed message
/// that names the stream (old or new). Nothing is written and no model is
/// reached.
///
/// A stream's declared limits and its spend are reported side by side. They are
/// not enforced per stream: both streams spend one shared root budget.
pub async fn verified_meta_trial(
    ctx: &Context,
    store: &Store,
    trial_id: &str,
) -> Result<MetaTrialView> {
    ctx.require(&[Role::Admin, Role::Worker])?;
    identifier(trial_id)?;
    let trial = load_verified(ctx, store, trial_id).await?;
    let old = stream_view(ctx, store, &trial, MetaStream::Old).await?;
    let new = stream_view(ctx, store, &trial, MetaStream::New).await?;
    let status = if old.verified_usage == 0 && new.verified_usage == 0 {
        MetaTrialStatus::CandidateOnly
    } else {
        MetaTrialStatus::UsageObserved
    };
    Ok(MetaTrialView {
        trial_id: trial.trial_id,
        status,
        s0_template_digest: trial.s0_template_digest,
        billing_scope: trial.billing_scope,
        declared_stream_limits: trial.declared_stream_limits,
        source_closure: trial.source_closure,
        revoke_watermark: trial.revoke_watermark,
        old,
        new,
    })
}

async fn stream_view(
    ctx: &Context,
    store: &Store,
    trial: &MetaTrialV1,
    stream: MetaStream,
) -> Result<MetaStreamView> {
    let world_id = trial.stream_world_id(stream);
    let coordinator = PersistentCoordinator::new(store.clone(), ctx.clone(), ctx.actor())?;
    let usage = coordinator.verified_mechanism_usage(world_id).await?;
    if usage.iter().any(|record: &MechanismUsageRecordV1| {
        record.world_id() != world_id
            || record.policy_digest() != trial.stream_policy_digest(stream)
    }) {
        return Err(Error::Conflict(format!(
            "meta trial {} stream: mechanism usage does not belong to the stream",
            stream.as_str()
        )));
    }
    let spend = stream_spend(ctx, store, trial, stream).await?;
    let dispatch_group_stopped = store
        .dispatch_group(ctx, &trial.billing_scope, world_id)
        .await?
        .is_some_and(|group| group.stopped);
    Ok(MetaStreamView {
        stream,
        world_id: world_id.to_owned(),
        content_digest: trial.stream_content_digest(stream).to_owned(),
        policy_digest: trial.stream_policy_digest(stream).to_owned(),
        verified_usage: usage.len(),
        spend,
        dispatch_group_stopped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use evo_core::strategy::ElasticPolicyV1;

    fn changed_content() -> ImproverContentV2 {
        let mut content = ImproverContentV2::builtin_default();
        let evo_core::improver::ImproverMechanismV2::ExplorationPolicy { policy } =
            &mut content.mechanism;
        *policy = ElasticPolicyV1 {
            max_focus_actions: 1,
            ..ElasticPolicyV1::default()
        };
        content
    }

    #[test]
    fn depth_and_protected_fields() {
        let parent = ImproverContentV2::builtin_default()
            .content_digest()
            .unwrap();
        let mut c = MetaCandidate {
            depth: 2,
            content: changed_content(),
            parent_improver_digest: parent,
        };
        assert!(c.validate().is_err());
        c.depth = 1;
        c.validate().unwrap();
        assert!(cannot_write_protected("budget").is_err());
        assert!(cannot_write_protected("improver.instruction").is_ok());
    }

    // -----------------------------------------------------------------------
    // MetaTrial: the pure part (no store)
    // -----------------------------------------------------------------------

    use crate::exploration::ExplorationDependency;
    use evo_core::strategy::{ExplorationCapsV1, SimulationContext};

    fn digest(label: &str) -> String {
        hash(label.as_bytes())
    }

    /// A fresh collecting S0 that carries the built-in I0 policy.
    fn template() -> ExplorationWorldV1 {
        let (skill, bundle, environment) = (digest("skill"), digest("bundle"), digest("env"));
        let (model, tools, grader, rules) = (
            digest("model"),
            digest("tools"),
            digest("grader"),
            digest("rules"),
        );
        let context_signature = fingerprint(&(
            &skill,
            &bundle,
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
            approved_parent_digest: digest("approved"),
            context_signature,
            parent_skill_digest: skill,
            parent_bundle_digest: bundle,
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
            dependencies: vec![
                ExplorationDependency {
                    kind: "run".into(),
                    id: "run-a".into(),
                },
                ExplorationDependency {
                    kind: "run".into(),
                    id: "run-b".into(),
                },
            ],
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

    fn request(trial_id: &str) -> MetaTrialForkRequest {
        MetaTrialForkRequest {
            trial_id: trial_id.into(),
            s0_template: template(),
            new_content: changed_content(),
            billing_scope: "scope-1".into(),
        }
    }

    fn plan(trial_id: &str) -> ForkPlan {
        request(trial_id).plan(1_000).unwrap()
    }

    fn as_json(world: &ExplorationWorldV1) -> serde_json::Value {
        serde_json::to_value(world).unwrap()
    }

    #[test]
    fn stream_request_ids_are_pure_differ_between_streams_and_stay_identifiers() {
        for trial in ["t", "trial-1", &"a".repeat(META_TRIAL_ID_MAX_BYTES)] {
            for step in [0, 1, 2, 17, u32::MAX] {
                let old = stream_request_id(trial, MetaStream::Old, step);
                let new = stream_request_id(trial, MetaStream::New, step);
                assert_ne!(old, new, "{trial} step {step}");
                assert_eq!(old, stream_request_id(trial, MetaStream::Old, step));
                assert_eq!(new, stream_request_id(trial, MetaStream::New, step));
                identifier(&old).unwrap();
                identifier(&new).unwrap();
                assert!(old.starts_with("mt-old-") && new.starts_with("mt-new-"));
            }
        }
        // Another step or another trial is another id, in both streams.
        let mut seen = BTreeSet::new();
        for trial in ["t-1", "t-2"] {
            for step in 1..=3 {
                for stream in MetaStream::BOTH {
                    assert!(seen.insert(stream_request_id(trial, stream, step)));
                }
            }
        }
        assert_eq!(seen.len(), 12);
    }

    #[test]
    fn world_ids_are_derived_from_the_trial_id_and_differ_per_stream() {
        assert_eq!(stream_world_id("t-1", MetaStream::Old), "t-1-old");
        assert_eq!(stream_world_id("t-1", MetaStream::New), "t-1-new");
        // A node id of the longest allowed trial is still an identifier.
        let trial = "a".repeat(META_TRIAL_ID_MAX_BYTES);
        for stream in MetaStream::BOTH {
            let world = stream_world_id(&trial, stream);
            identifier(&world).unwrap();
            identifier(&format!("node-{world}-12")).unwrap();
        }
    }

    #[test]
    fn a_plan_derives_two_worlds_that_differ_only_in_id_and_policy() {
        let plan = plan("trial-1");
        assert_eq!(plan.old_world.id, "trial-1-old");
        assert_eq!(plan.new_world.id, "trial-1-new");
        assert_eq!(plan.trial.old_world_id, plan.old_world.id);
        assert_eq!(plan.trial.new_world_id, plan.new_world.id);
        let (mut old, mut new) = (as_json(&plan.old_world), as_json(&plan.new_world));
        let template = as_json(&template());
        for field in ["id", "policy"] {
            assert_ne!(old[field], new[field], "{field}");
            old.as_object_mut().unwrap().remove(field);
            new.as_object_mut().unwrap().remove(field);
        }
        let mut expected = template;
        for field in ["id", "policy"] {
            expected.as_object_mut().unwrap().remove(field);
        }
        assert_eq!(
            old, expected,
            "the old world is the template but for id/policy"
        );
        assert_eq!(
            new, expected,
            "the new world is the template but for id/policy"
        );
        // Same S0: the compatibility signature is equal, the registration is not.
        assert_eq!(
            plan.old_world.context_signature,
            plan.new_world.context_signature
        );
        assert_ne!(
            plan.old_world.registration_fingerprint().unwrap(),
            plan.new_world.registration_fingerprint().unwrap()
        );
        // The policies are the improvers' policies; the template's own is I0's.
        assert_eq!(
            plan.old_world.policy.digest().unwrap(),
            ElasticPolicyV1::default().digest().unwrap()
        );
        assert_eq!(
            plan.new_world.policy.digest().unwrap(),
            changed_content().exploration_policy().digest().unwrap()
        );
        plan.old_world.validate().unwrap();
        plan.new_world.validate().unwrap();
        plan.trial.validate().unwrap();
        // The trial declares the limits of the template, once, for both streams.
        let limits = &plan.trial.declared_stream_limits;
        assert_eq!(limits.remaining_root_micros, 1_000);
        assert_eq!(limits.successor_cost_upper_micros, 10);
        assert_eq!(limits.remaining_recovery_dispatches, 2);
        assert_eq!(
            fingerprint(&limits.root_opportunities).unwrap(),
            fingerprint(&template_root_opportunities()).unwrap()
        );
        assert_eq!(plan.trial.source_closure, vec!["run-a", "run-b"]);
        assert_eq!(plan.trial.revoke_watermark, 1);
        assert_eq!(plan.trial.created_at, 1_000);
        // Both worlds carry the S0 the trial recorded, and each passes its own check.
        for stream in MetaStream::BOTH {
            assert_eq!(
                s0_registration_digest(plan.world(stream), limits).unwrap(),
                plan.trial.s0_template_digest
            );
            plan.trial
                .check_stream_world(stream, plan.world(stream))
                .unwrap();
        }
    }

    fn template_root_opportunities() -> Vec<RootOpportunity> {
        template().root_opportunities
    }

    #[test]
    fn the_s0_digest_ignores_id_policy_and_scheduling_but_nothing_else() {
        let base = template();
        let limits = plan("trial-1").trial.declared_stream_limits;
        let reference = s0_registration_digest(&base, &limits).unwrap();

        // What may differ between the streams, or move while a stream runs.
        let mut world = base.clone();
        world.id = "another-id".into();
        world.policy = changed_content().exploration_policy().clone();
        world.state = WorldState::Stopped;
        world.node_ids = vec!["node-1".into()];
        world.dispatch_ids = vec!["dispatch-1".into()];
        world.history_ids = vec!["history-1".into()];
        world.current_branch_seq = Some(1);
        world.current_branch_focus_actions = 2;
        world.decision_round = 5;
        world.waits = vec![evo_core::strategy::OpportunityWait {
            action_seq: 1,
            waited_rounds: 1,
        }];
        world.remaining_root_micros = 3;
        world.remaining_recovery_dispatches = 0;
        assert_eq!(s0_registration_digest(&world, &limits).unwrap(), reference);

        // Every registration fact moves it.
        type Edit = Box<dyn Fn(&mut ExplorationWorldV1)>;
        let edits: Vec<(&str, Edit)> = vec![
            (
                "approved_parent_digest",
                Box::new(|w| w.approved_parent_digest = digest("other")),
            ),
            (
                "context_signature",
                Box::new(|w| w.context_signature = digest("other")),
            ),
            (
                "parent_skill_digest",
                Box::new(|w| w.parent_skill_digest = digest("other")),
            ),
            (
                "parent_bundle_digest",
                Box::new(|w| w.parent_bundle_digest = digest("other")),
            ),
            (
                "environment_digest",
                Box::new(|w| w.environment_digest = digest("other")),
            ),
            (
                "model_digest",
                Box::new(|w| w.model_digest = digest("other")),
            ),
            (
                "tools_digest",
                Box::new(|w| w.tools_digest = digest("other")),
            ),
            (
                "grader_digest",
                Box::new(|w| w.grader_digest = digest("other")),
            ),
            (
                "rules_digest",
                Box::new(|w| w.rules_digest = digest("other")),
            ),
            ("source_watermark", Box::new(|w| w.source_watermark = 2)),
            ("caps", Box::new(|w| w.caps.max_nodes = 6)),
            (
                "simulation",
                Box::new(|w| w.simulation = SimulationContext::Online { fixed_seed: 8 }),
            ),
            (
                "root opportunity cost",
                Box::new(|w| w.root_opportunities[0].estimated_cost_upper_micros = 11),
            ),
            (
                "root opportunity order",
                Box::new(|w| w.root_opportunities.reverse()),
            ),
            (
                "dependencies",
                Box::new(|w| w.dependencies[1].id = "run-c".into()),
            ),
            (
                "successor_cost_upper_micros",
                Box::new(|w| w.successor_cost_upper_micros = 11),
            ),
            (
                "initial_baseline_quality_micros",
                Box::new(|w| w.initial_baseline_quality_micros = 1),
            ),
        ];
        for (label, edit) in &edits {
            let mut world = base.clone();
            edit(&mut world);
            assert_ne!(
                s0_registration_digest(&world, &limits).unwrap(),
                reference,
                "{label}"
            );
        }
        // The declared counters are part of what the trial recorded.
        let mut other = limits.clone();
        other.remaining_root_micros += 1;
        assert_ne!(s0_registration_digest(&base, &other).unwrap(), reference);
        let mut other = limits;
        other.remaining_recovery_dispatches += 1;
        assert_ne!(s0_registration_digest(&base, &other).unwrap(), reference);
    }

    #[test]
    fn a_plan_refuses_what_is_not_a_fresh_i0_s0_and_a_real_candidate() {
        let invalid = |request: MetaTrialForkRequest, why: &str| {
            assert!(
                matches!(request.plan(1), Err(Error::Invalid(_))),
                "{why}: {:?}",
                request.plan(1).err()
            );
        };

        // The S0 template must carry I0's policy.
        let mut not_i0 = request("trial-1");
        not_i0.s0_template.policy = changed_content().exploration_policy().clone();
        invalid(not_i0, "template policy is not I0");

        // The candidate must be a change, a valid one.
        let mut same = request("trial-1");
        same.new_content = ImproverContentV2::builtin_default();
        invalid(same, "candidate equals I0");
        let mut out_of_bound = request("trial-1");
        let evo_core::improver::ImproverMechanismV2::ExplorationPolicy { policy } =
            &mut out_of_bound.new_content.mechanism;
        policy.fairness_wait_rounds = 12;
        invalid(out_of_bound, "candidate policy out of its bounds");
        let mut wrong_schema = request("trial-1");
        wrong_schema.new_content.schema_version = "rsia.improver_content.v1".into();
        invalid(wrong_schema, "candidate content schema");

        // The S0 must be a fresh collecting world.
        type Edit = Box<dyn Fn(&mut ExplorationWorldV1)>;
        let stale: Vec<(&str, Edit)> = vec![
            ("sealed", Box::new(|w| w.state = WorldState::Sealed)),
            ("stopped", Box::new(|w| w.state = WorldState::Stopped)),
            ("node", Box::new(|w| w.node_ids = vec!["node-1".into()])),
            (
                "dispatch",
                Box::new(|w| w.dispatch_ids = vec!["dispatch-1".into()]),
            ),
            (
                "history",
                Box::new(|w| w.history_ids = vec!["history-1".into()]),
            ),
            ("branch", Box::new(|w| w.current_branch_seq = Some(1))),
            ("focus", Box::new(|w| w.current_branch_focus_actions = 1)),
            ("round", Box::new(|w| w.decision_round = 1)),
            (
                "wait",
                Box::new(|w| {
                    w.waits = vec![evo_core::strategy::OpportunityWait {
                        action_seq: 1,
                        waited_rounds: 1,
                    }]
                }),
            ),
        ];
        for (label, edit) in &stale {
            let mut request = request("trial-1");
            edit(&mut request.s0_template);
            invalid(request, label);
        }

        // A template that is not a valid world.
        let mut broken = request("trial-1");
        broken.s0_template.dependencies.clear();
        assert!(broken.plan(1).is_err());
        let mut signature = request("trial-1");
        signature.s0_template.environment_digest = digest("changed");
        assert!(matches!(signature.plan(1), Err(Error::Conflict(_))));

        // Ids.
        for bad in [
            "",
            " ",
            "has space",
            "slash/trial",
            &"a".repeat(META_TRIAL_ID_MAX_BYTES + 1),
        ] {
            invalid(request(bad), bad);
        }
        let mut scope = request("trial-1");
        scope.billing_scope = "bad scope".into();
        invalid(scope, "billing scope");
        request(&"a".repeat(META_TRIAL_ID_MAX_BYTES))
            .plan(1)
            .unwrap();
    }

    #[test]
    fn the_registration_fingerprint_is_the_request_not_the_clock() {
        let first = request("trial-1").plan(100).unwrap().trial;
        let replay = request("trial-1").plan(900).unwrap().trial;
        assert_ne!(first.created_at, replay.created_at);
        assert_eq!(
            first.registration_fingerprint().unwrap(),
            replay.registration_fingerprint().unwrap()
        );
        let mut other_content = request("trial-1");
        let evo_core::improver::ImproverMechanismV2::ExplorationPolicy { policy } =
            &mut other_content.new_content.mechanism;
        policy.max_focus_actions = 3;
        let other = other_content.plan(100).unwrap().trial;
        assert_ne!(
            first.registration_fingerprint().unwrap(),
            other.registration_fingerprint().unwrap()
        );
        let mut other_scope = request("trial-1");
        other_scope.billing_scope = "scope-2".into();
        assert_ne!(
            first.registration_fingerprint().unwrap(),
            other_scope
                .plan(100)
                .unwrap()
                .trial
                .registration_fingerprint()
                .unwrap()
        );
        let mut other_limits = request("trial-1");
        other_limits.s0_template.remaining_root_micros = 999;
        assert_ne!(
            first.registration_fingerprint().unwrap(),
            other_limits
                .plan(100)
                .unwrap()
                .trial
                .registration_fingerprint()
                .unwrap()
        );
    }

    #[test]
    fn a_trial_record_that_disagrees_with_itself_does_not_validate() {
        let good = plan("trial-1").trial;
        good.validate().unwrap();
        type Edit = Box<dyn Fn(&mut MetaTrialV1)>;
        let edits: Vec<(&str, Edit)> = vec![
            (
                "schema",
                Box::new(|t| t.schema_version = "rsia.meta_trial.v0".into()),
            ),
            (
                "trial id too long",
                Box::new(|t| t.trial_id = "a".repeat(META_TRIAL_ID_MAX_BYTES + 1)),
            ),
            (
                "billing scope",
                Box::new(|t| t.billing_scope = "bad scope".into()),
            ),
            (
                "s0 digest not a digest",
                Box::new(|t| t.s0_template_digest = "S0".into()),
            ),
            (
                "old content digest",
                Box::new(|t| t.old_content_digest = digest("x")),
            ),
            (
                "new content digest",
                Box::new(|t| t.new_content_digest = digest("x")),
            ),
            (
                "old policy digest",
                Box::new(|t| t.old_policy_digest = digest("x")),
            ),
            (
                "new policy digest",
                Box::new(|t| t.new_policy_digest = digest("x")),
            ),
            (
                "old world id",
                Box::new(|t| t.old_world_id = "trial-1-new".into()),
            ),
            (
                "new world id",
                Box::new(|t| t.new_world_id = "other-new".into()),
            ),
            (
                "old content is not I0 (digests kept consistent)",
                Box::new(|t| {
                    t.old_content = changed_content();
                    t.old_content_digest = t.old_content.content_digest().unwrap();
                    t.old_policy_digest = t.old_content.exploration_policy().digest().unwrap();
                }),
            ),
            (
                "new content equals I0",
                Box::new(|t| {
                    t.new_content = ImproverContentV2::builtin_default();
                    t.new_content_digest = t.new_content.content_digest().unwrap();
                    t.new_policy_digest = t.new_content.exploration_policy().digest().unwrap();
                }),
            ),
            (
                "empty source closure",
                Box::new(|t| t.source_closure.clear()),
            ),
            (
                "duplicate source",
                Box::new(|t| t.source_closure = vec!["run-a".into(), "run-a".into()]),
            ),
        ];
        for (label, edit) in &edits {
            let mut trial = good.clone();
            edit(&mut trial);
            assert!(trial.validate().is_err(), "{label}");
            // The read side names a stored record that fails validation a Conflict.
            assert!(
                matches!(
                    trial.validate().map_err(stored_trial_invalid),
                    Err(Error::Conflict(_))
                ),
                "{label}"
            );
        }
    }

    #[test]
    fn a_stored_world_is_checked_against_what_the_trial_declared() {
        let plan = plan("trial-1");
        let trial = &plan.trial;
        // Scheduling state and spent budget may move.
        let mut moved = plan.new_world.clone();
        moved.state = WorldState::Sealed;
        moved.node_ids = vec!["node-trial-1-new-1".into()];
        moved.dispatch_ids = vec!["dispatch-1".into()];
        moved.decision_round = 3;
        moved.remaining_root_micros -= 30;
        moved.remaining_recovery_dispatches = 0;
        trial.check_stream_world(MetaStream::New, &moved).unwrap();

        // A world no dispatch started in is exactly what was registered: a
        // lowered counter or a started schedule is not a spend.
        for (label, edit) in [
            (
                "root budget lowered",
                Box::new(|w: &mut ExplorationWorldV1| w.remaining_root_micros -= 1)
                    as Box<dyn Fn(&mut ExplorationWorldV1)>,
            ),
            (
                "recovery dispatches lowered",
                Box::new(|w| w.remaining_recovery_dispatches -= 1),
            ),
            ("round started", Box::new(|w| w.decision_round = 1)),
            (
                "branch started",
                Box::new(|w| w.current_branch_seq = Some(1)),
            ),
            (
                "focus counted",
                Box::new(|w| w.current_branch_focus_actions = 1),
            ),
            (
                "wait recorded",
                Box::new(|w| {
                    w.waits = vec![evo_core::strategy::OpportunityWait {
                        action_seq: 1,
                        waited_rounds: 1,
                    }]
                }),
            ),
        ] {
            let mut world = plan.new_world.clone();
            edit(&mut world);
            assert!(
                matches!(
                    trial.check_stream_world(MetaStream::New, &world),
                    Err(Error::Conflict(_))
                ),
                "{label}"
            );
        }

        // A world is only its own stream's.
        assert!(matches!(
            trial.check_stream_world(MetaStream::New, &plan.old_world),
            Err(Error::Conflict(_))
        ));
        assert!(matches!(
            trial.check_stream_world(MetaStream::Old, &plan.new_world),
            Err(Error::Conflict(_))
        ));
        // The policy is the stream improver's: the other one's is refused.
        let mut swapped = plan.new_world.clone();
        swapped.policy = ElasticPolicyV1::default();
        assert!(matches!(
            trial.check_stream_world(MetaStream::New, &swapped),
            Err(Error::Conflict(_))
        ));

        type Edit = Box<dyn Fn(&mut ExplorationWorldV1)>;
        let tampered: Vec<(&str, Edit)> = vec![
            (
                "approved parent",
                Box::new(|w| w.approved_parent_digest = digest("other")),
            ),
            ("caps", Box::new(|w| w.caps.max_depth = 3)),
            (
                "simulation",
                Box::new(|w| w.simulation = SimulationContext::Online { fixed_seed: 9 }),
            ),
            (
                "root opportunity cost",
                Box::new(|w| w.root_opportunities[1].estimated_cost_upper_micros = 9),
            ),
            (
                "successor cost",
                Box::new(|w| w.successor_cost_upper_micros = 9),
            ),
            (
                "baseline quality",
                Box::new(|w| w.initial_baseline_quality_micros = 400_000),
            ),
            (
                "source closure",
                Box::new(|w| w.dependencies[0].id = "run-z".into()),
            ),
            ("watermark", Box::new(|w| w.source_watermark = 2)),
            // More budget than was declared is not spending.
            (
                "root budget above the declaration",
                Box::new(|w| w.remaining_root_micros = 1_001),
            ),
            (
                "recovery dispatches above the declaration",
                Box::new(|w| w.remaining_recovery_dispatches = 3),
            ),
        ];
        for (label, edit) in &tampered {
            let mut world = plan.new_world.clone();
            edit(&mut world);
            assert!(
                matches!(
                    trial.check_stream_world(MetaStream::New, &world),
                    Err(Error::Conflict(_))
                ),
                "{label}"
            );
        }
        // And the trial's own declaration is covered: lowering what it declared
        // below what the world still holds, or changing the S0 digest.
        let mut forged = plan.trial.clone();
        forged.declared_stream_limits.remaining_root_micros = 500;
        assert!(matches!(
            forged.check_stream_world(MetaStream::New, &plan.new_world),
            Err(Error::Conflict(_))
        ));
        let mut raised = plan.trial.clone();
        raised.declared_stream_limits.remaining_root_micros = 2_000;
        assert!(
            matches!(
                raised.check_stream_world(MetaStream::New, &plan.new_world),
                Err(Error::Conflict(_))
            ),
            "a raised declaration no longer equals the recorded S0 digest"
        );
        let mut digest_forged = plan.trial.clone();
        digest_forged.s0_template_digest = digest("forged");
        assert!(matches!(
            digest_forged.check_stream_world(MetaStream::New, &plan.new_world),
            Err(Error::Conflict(_))
        ));
        let mut closure_forged = plan.trial.clone();
        closure_forged.source_closure = vec!["run-a".into(), "run-x".into()];
        assert!(matches!(
            closure_forged.check_stream_world(MetaStream::New, &plan.new_world),
            Err(Error::Conflict(_))
        ));
    }
}
