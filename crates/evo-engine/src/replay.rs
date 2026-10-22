//! Immutable lookup-only replay through the same pure E09 decision consumer.

use evo_core::evaluation::Verdict;
use evo_core::evidence::Purpose;
use evo_core::replay::{
    HistoricalUsage, RationalValue, ReplayActionSpecV1, ReplayBatchRecord, ReplayCoverage,
    ReplayObjective, ReplayReportV2, ReplaySimulationProfile, ReplayTerminal,
    ReplayTransitionOutcome, ReplayTransitionV2, ReplayWorldV2, WorldPartition, replay_report_id,
};
use evo_core::strategy::{
    ActionKindV1, BatchActionV1, BudgetViewV1, ElasticPolicyV1, ExplorationCapsV1, LegalActionV1,
    LegalActionsV1, ObservedStatus, OpportunityWait, PrefixNodeV2, PrefixViewV2, SimulationContext,
    decide_elastic, legal_candidates,
};
use evo_core::{Context, Error, Result, fingerprint, identifier};
use evo_storage::Store;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct LiveWorldAuthority {
    pub revoke_watermark: u64,
    pub revoked_source_ids: BTreeSet<String>,
}

#[derive(Debug, Clone)]
struct ReplayState {
    prefix: PrefixViewV2,
    selected: BTreeSet<u32>,
    // Derived only from Observed Recover transitions revealed by this run.
    // Historical status counters are not dispatch facts for the replay.
    recoveries_by_episode: BTreeMap<String, u8>,
    context_by_node: BTreeMap<u32, String>,
    best_quality: u32,
    batches: Vec<ReplayBatchRecord>,
    probe_curve: Vec<u32>,
    round_curve: Vec<u32>,
    historical_probes: u32,
    simulated_rounds: u32,
    usage: HistoricalUsage,
    coverage: ReplayCoverage,
    approved_root_spent_micros: u64,
}

/// Re-evaluates an old world with the current engine. Its root universe stays
/// Unknown; catalog/exhaustion labels never upgrade it to a proved V3 world.
pub fn run_replay(
    world: &ReplayWorldV2,
    policy: &ElasticPolicyV1,
    profile: &ReplaySimulationProfile,
    caps: &ExplorationCapsV1,
    authority: &LiveWorldAuthority,
) -> Result<evo_core::replay::ReplayReportV3> {
    let validation = validate_inputs(world, profile, caps).and_then(|()| policy.validate());
    run_replay_runtime(
        &ReplayRuntimeWorld::legacy(world),
        policy,
        profile,
        caps,
        authority,
        validation,
    )
}

/// A fully checked sealed loader is the only current-root algorithm input.
/// The report is still plain data; persistent/current read qualification runs
/// independent source and semantic checks in the actual consuming Session.
pub fn run_current_replay(
    loaded: &evo_storage::replay::LoadedReplayWorldV3,
    policy: &ElasticPolicyV1,
    profile: &ReplaySimulationProfile,
    caps: &ExplorationCapsV1,
    authority: &LiveWorldAuthority,
) -> Result<evo_core::replay::ReplayReportV3> {
    let runtime = ReplayRuntimeWorld::current(loaded)?;
    let validation = loaded
        .world()
        .validate_sealed()
        .and_then(|()| profile.validate(caps))
        .and_then(|()| policy.validate());
    run_replay_runtime(&runtime, policy, profile, caps, authority, validation)
}

fn run_replay_runtime(
    world: &ReplayRuntimeWorld<'_>,
    policy: &ElasticPolicyV1,
    profile: &ReplaySimulationProfile,
    caps: &ExplorationCapsV1,
    authority: &LiveWorldAuthority,
    validation: Result<()>,
) -> Result<evo_core::replay::ReplayReportV3> {
    let started = Instant::now();
    let policy_digest = fingerprint(policy)?;
    let profile_digest = fingerprint(profile)?;
    if let Err(error) = validation {
        return terminal_report(
            world,
            &policy_digest,
            &profile_digest,
            profile,
            ReplayTerminal::InvalidWorld,
            error.to_string(),
            started,
        );
    }
    let world_digest = world.sealed_digest.ok_or(Error::Internal)?.to_owned();
    if authority.revoke_watermark != world.manifest.revoke_watermark
        || world
            .manifest
            .source_closure
            .iter()
            .any(|source| authority.revoked_source_ids.contains(&source.source_id))
    {
        return terminal_report(
            world,
            &policy_digest,
            &profile_digest,
            profile,
            ReplayTerminal::SourceRevoked,
            "world source closure is not live".into(),
            started,
        );
    }
    let mut state = ReplayState {
        prefix: PrefixViewV2 {
            schema_version: PrefixViewV2::SCHEMA.into(),
            context_signature: world.manifest.world_context_signature.to_owned(),
            approved_parent_digest: world.manifest.approved_parent_digest.to_owned(),
            initial_baseline_quality_micros: world.manifest.initial_baseline_quality_micros,
            nodes: Vec::new(),
            current_branch_seq: None,
            current_branch_focus_actions: 0,
            decisions_completed: 0,
            waits: Vec::new(),
            nodes_used: 0,
            recovery_dispatches_used: 0,
        },
        selected: BTreeSet::new(),
        recoveries_by_episode: BTreeMap::new(),
        context_by_node: BTreeMap::new(),
        best_quality: world.manifest.initial_baseline_quality_micros,
        batches: Vec::new(),
        probe_curve: Vec::new(),
        round_curve: Vec::new(),
        historical_probes: 0,
        simulated_rounds: 0,
        usage: HistoricalUsage {
            input_tokens: 0,
            output_tokens: 0,
            cost_micros: Some(0),
            latency_millis: Some(0),
        },
        coverage: ReplayCoverage::default(),
        approved_root_spent_micros: 0,
    };

    let mut exclusions = evo_core::replay::ReplayPolicyStopExclusions::default();
    let (terminal, reason) = loop {
        if state.historical_probes > u32::from(profile.probe_budget) {
            return Err(Error::Invalid("real replay probe budget overflow".into()));
        }
        if state.historical_probes == u32::from(profile.probe_budget) {
            break (
                ReplayTerminal::BudgetExhausted,
                "probe budget exhausted".into(),
            );
        }
        state.prefix.validate()?;
        let available = available_actions(world, &state)?;
        let legal = legal_projection(&available);
        let budget = replay_runtime_budget(world, &state, &available, profile)?;
        let eligible = legal_candidates(&state.prefix, &legal, &budget, caps)?;
        let decision = decide_elastic(
            policy,
            &state.prefix,
            &legal,
            &budget,
            caps,
            SimulationContext::Offline {
                w_sim: profile.w_sim,
                fixed_seed: profile.fixed_seed,
            },
        )?;
        match decision {
            BatchActionV1::Stop { reason } => {
                if !eligible.is_empty() {
                    return Err(Error::Conflict(
                        "replay core stopped with legal actions".into(),
                    ));
                }
                if let Some(frontier) = empty_legal_frontier_v3(world, &state, caps)? {
                    break frontier;
                }
                exclusions = policy_stop_exclusions_v3(&state.prefix, &legal, &budget, caps)?;
                break (ReplayTerminal::PolicyStop, reason);
            }
            BatchActionV1::Dispatch {
                action_ids,
                action_seqs,
                ..
            } => {
                let chosen = resolve_batch(&available, &action_ids, &action_seqs, profile.w_sim)?;
                // The legal opportunities of this decision round: what the shared core
                // filter leaves legal of the same inputs, the very set the decision ranked.
                let opportunities: BTreeSet<u32> =
                    legal_candidates(&state.prefix, &legal, &budget, caps)?
                        .into_iter()
                        .map(|action| action.action_seq)
                        .collect();
                let batch_terminal =
                    reveal_batch(world, &mut state, &available, &opportunities, &chosen)?;
                if let Some(terminal) = batch_terminal {
                    break terminal;
                }
            }
        }
    };

    finish_report(
        world,
        world_digest,
        policy_digest,
        profile_digest,
        profile,
        state,
        terminal,
        reason,
        started,
        exclusions,
    )
}

pub async fn run_persisted_replay(
    ctx: &Context,
    store: &Store,
    world_id: &str,
    purpose: Purpose,
    policy: &ElasticPolicyV1,
    profile: &ReplaySimulationProfile,
    caps: &ExplorationCapsV1,
) -> Result<evo_core::replay::ReplayReportV3> {
    let pool = evo_storage::replay::load_live_replay_pool(ctx, store, &profile.pool_digest).await?;
    profile.validate_for_pool(caps, &pool.manifest)?;
    if purpose != pool.manifest.compatibility.purpose {
        return Err(Error::Forbidden);
    }
    let world = pool
        .worlds
        .iter()
        .find(|w| w.world_id() == world_id)
        .ok_or(Error::NotFound)?;
    let report = run_loaded_replay(world, policy, profile, caps)?;
    tokio::task::yield_now().await;
    let final_pool =
        evo_storage::replay::load_live_replay_pool(ctx, store, &profile.pool_digest).await?;
    let final_world = final_pool
        .worlds
        .iter()
        .find(|w| w.world_id() == world_id)
        .ok_or(Error::NotFound)?;
    if final_pool.manifest != pool.manifest || final_world.fingerprint()? != world.fingerprint()? {
        return Err(Error::Conflict(
            "replay world changed during persisted replay".into(),
        ));
    }
    Ok(report)
}

fn run_loaded_replay(
    world: &evo_storage::replay::LoadedReplayWorld,
    policy: &ElasticPolicyV1,
    profile: &ReplaySimulationProfile,
    caps: &ExplorationCapsV1,
) -> Result<evo_core::replay::ReplayReportV3> {
    use evo_storage::replay::LoadedReplayWorld;
    let watermark = match world {
        LoadedReplayWorld::LegacyV2(w) => w.manifest.revoke_watermark,
        LoadedReplayWorld::CurrentV3(w) => w.world().manifest.revoke_watermark,
    };
    let authority = LiveWorldAuthority {
        revoke_watermark: watermark,
        revoked_source_ids: BTreeSet::new(),
    };
    match world {
        LoadedReplayWorld::LegacyV2(w) => run_replay(w, policy, profile, caps, &authority),
        LoadedReplayWorld::CurrentV3(w) => run_current_replay(w, policy, profile, caps, &authority),
    }
}

#[derive(Debug, Clone)]
struct CurrentReplayViewSeal;

/// Only this module's actual live/source/semantic verifier can construct it.
/// No Deserialize or public constructor converts a claimed report into Current.
#[derive(Debug, Clone, Serialize)]
pub struct VerifiedReplayReportView {
    pub report_id: String,
    pub report_digest: String,
    pub semantic_digest: String,
    pub pool_digest: String,
    pub partition: WorldPartition,
    pub policy_digest: String,
    pub profile_digest: String,
    pub caps_digest: String,
    pub member_worlds: Vec<(String, String, String)>,
    pub reports: Vec<evo_core::replay::ReplayReportV3>,
    pub attempted_worlds: u32,
    pub failed_worlds: Vec<evo_core::replay::ReplayWorldFailure>,
    pub support_complete: bool,
    pub development_only: bool,
    pub promotion_eligible: bool,
    pub ineligibility_reasons: Vec<String>,
    #[serde(skip)]
    _verified_current: CurrentReplayViewSeal,
}

pub async fn run_and_persist_pool_replay(
    ctx: &Context,
    store: &Store,
    pool_digest: &str,
    partition: WorldPartition,
    policy: &ElasticPolicyV1,
    profile: &ReplaySimulationProfile,
    caps: &ExplorationCapsV1,
) -> Result<evo_core::replay::StoredReplayReportV2> {
    ctx.require(&[evo_core::Role::Worker, evo_core::Role::Admin])?;
    let expected_id = replay_report_id(
        ctx.namespace(),
        partition,
        pool_digest,
        &fingerprint(policy)?,
        &fingerprint(profile)?,
        &fingerprint(caps)?,
    )?;
    let mut session = store.session().await?;
    let pool =
        evo_storage::replay::load_live_replay_pool_in_session(&mut session, ctx, pool_digest)
            .await?;
    profile.validate_for_pool(caps, &pool.manifest)?;
    if let Some(existing) =
        existing_current_report_in_session(&mut session, ctx, &expected_id, &pool).await?
    {
        session.commit().await?;
        return Ok(existing);
    }
    session.commit().await?;
    let (reports, failed) = recompute_partition_reports(&pool, partition, policy, profile, caps)?;
    let stored = evo_core::replay::StoredReplayReportV2::build_with_failures(
        ctx.namespace(),
        partition,
        &pool.manifest,
        policy.clone(),
        profile.clone(),
        caps.clone(),
        reports,
        failed,
    )?;
    // Recheck the actual sources, opaque root proof, version and semantics in
    // the final write transaction; a same-key retry has exactly the same guards.
    let mut write_tx = store.session().await?;
    let final_pool =
        evo_storage::replay::load_live_replay_pool_in_session(&mut write_tx, ctx, pool_digest)
            .await?;
    if final_pool.manifest != pool.manifest {
        return Err(Error::Conflict(
            "replay pool changed before report write".into(),
        ));
    }
    if let Some(existing) =
        existing_current_report_in_session(&mut write_tx, ctx, &expected_id, &final_pool).await?
    {
        write_tx.commit().await?;
        return Ok(existing);
    }
    verify_stored_report_semantics(&final_pool, &stored)?;
    evo_storage::replay::put_current_replay_report_in_session(&mut write_tx, ctx, &stored).await?;
    write_tx.commit().await?;
    Ok(stored)
}

async fn existing_current_report_in_session(
    session: &mut evo_storage::Session,
    ctx: &Context,
    report_id: &str,
    pool: &evo_storage::replay::LoadedReplayPool,
) -> Result<Option<evo_core::replay::StoredReplayReportV2>> {
    use evo_core::replay::ReplayReportIdentityVersion;
    match evo_storage::replay::replay_report_version_in_session(session, ctx, report_id).await {
        Err(evo_core::replay::ReplayReadIsolation::Source(Error::NotFound)) => return Ok(None),
        Err(error) => return Err(replay_read_isolation_for_existing_api(error)),
        Ok(Some(ReplayReportIdentityVersion::NextV3)) => {}
        Ok(Some(ReplayReportIdentityVersion::LegacyV2)) => {
            return Err(Error::Invalid("legacy_replay_not_consumable".into()));
        }
        Ok(None) => {
            return Err(Error::Invalid(
                "invalid replay report at current identity".into(),
            ));
        }
    }
    let record =
        evo_storage::replay::load_current_replay_report_in_session(session, ctx, report_id).await?;
    verify_stored_report_semantics(pool, &record)?;
    Ok(Some(record))
}

pub async fn verified_replay_report_view(
    ctx: &Context,
    store: &Store,
    report_id: &str,
) -> Result<VerifiedReplayReportView> {
    let mut session = store.session().await?;
    let view = verified_replay_report_view_in_session(&mut session, ctx, report_id).await?;
    session.commit().await?;
    Ok(view)
}

pub async fn verified_replay_report_view_in_session(
    session: &mut evo_storage::Session,
    ctx: &Context,
    report_id: &str,
) -> Result<VerifiedReplayReportView> {
    use evo_core::replay::ReplayReportIdentityVersion;
    ctx.require(&[evo_core::Role::Evaluator, evo_core::Role::Admin])?;
    match evo_storage::replay::replay_report_version_in_session(session, ctx, report_id)
        .await
        .map_err(replay_read_isolation_for_existing_api)?
    {
        Some(ReplayReportIdentityVersion::NextV3) => {}
        Some(ReplayReportIdentityVersion::LegacyV2) => {
            return Err(Error::Invalid("legacy_replay_not_consumable".into()));
        }
        None => return Err(Error::Invalid("invalid_replay_record".into())),
    }
    let stored =
        evo_storage::replay::load_current_replay_report_in_session(session, ctx, report_id).await?;
    let pool =
        evo_storage::replay::load_live_replay_pool_in_session(session, ctx, &stored.pool_digest)
            .await?;
    verify_stored_report_semantics(&pool, &stored)?;
    let support_complete = stored.failed_worlds.is_empty()
        && usize::try_from(stored.attempted_worlds).ok() == Some(stored.member_reports.len())
        && stored
            .member_reports
            .iter()
            .all(|m| m.report.coverage.complete_support);
    let mut reasons = vec!["development_only_replay".into()];
    if !support_complete {
        reasons.push("incomplete_world_support".into());
    }
    if stored.profile.w_sim != 1 {
        reasons.push("simulation_only_width".into());
    }
    Ok(VerifiedReplayReportView {
        report_id: stored.report_id.clone(),
        report_digest: fingerprint(&stored)?,
        semantic_digest: stored.semantic_reports_digest.clone(),
        pool_digest: stored.pool_digest.clone(),
        partition: stored.partition,
        policy_digest: stored.policy_digest.clone(),
        profile_digest: stored.profile_digest.clone(),
        caps_digest: stored.caps_digest.clone(),
        member_worlds: stored
            .member_reports
            .iter()
            .map(|m| {
                (
                    m.world_id.clone(),
                    m.world_digest.clone(),
                    m.report.cluster_id.clone(),
                )
            })
            .collect(),
        reports: stored
            .member_reports
            .iter()
            .map(|m| m.report.clone())
            .collect(),
        attempted_worlds: stored.attempted_worlds,
        failed_worlds: stored.failed_worlds.clone(),
        support_complete,
        development_only: true,
        promotion_eligible: false,
        ineligibility_reasons: reasons,
        _verified_current: CurrentReplayViewSeal,
    })
}

fn verify_stored_report_semantics(
    pool: &evo_storage::replay::LoadedReplayPool,
    stored: &evo_core::replay::StoredReplayReportV2,
) -> Result<()> {
    stored.validate_static(&pool.manifest)?;
    let (reports, failed) = recompute_partition_reports(
        pool,
        stored.partition,
        &stored.policy,
        &stored.profile,
        &stored.caps,
    )?;
    let rebuilt = evo_core::replay::StoredReplayReportV2::build_with_failures(
        &stored.namespace,
        stored.partition,
        &pool.manifest,
        stored.policy.clone(),
        stored.profile.clone(),
        stored.caps.clone(),
        reports,
        failed,
    )?;
    if rebuilt.report_id != stored.report_id
        || rebuilt.semantic_reports_digest != stored.semantic_reports_digest
    {
        return Err(Error::Conflict(
            "stored replay report differs from current semantic recomputation".into(),
        ));
    }
    Ok(())
}

fn recompute_partition_reports(
    pool: &evo_storage::replay::LoadedReplayPool,
    partition: WorldPartition,
    policy: &ElasticPolicyV1,
    profile: &ReplaySimulationProfile,
    caps: &ExplorationCapsV1,
) -> Result<(
    Vec<evo_core::replay::ReplayReportV3>,
    Vec<evo_core::replay::ReplayWorldFailure>,
)> {
    profile.validate_for_pool(caps, &pool.manifest)?;
    let mut reports = Vec::new();
    let mut failed = Vec::new();
    for world in pool.worlds.iter().filter(|w| w.partition() == partition) {
        match run_loaded_replay(world, policy, profile, caps) {
            Ok(report) => reports.push(report),
            Err(error) => failed.push(evo_core::replay::ReplayWorldFailure {
                world_id: world.world_id().to_owned(),
                world_digest: world.digest()?.to_owned(),
                error_kind: evo_core::replay::ReplayFailureKind::from_error(&error),
            }),
        }
    }
    // Runtime errors are diagnostics without scored reports, but never vanish
    // from the attempted-world denominator carried by StoredV2.
    Ok((reports, failed))
}

fn validate_inputs(
    world: &ReplayWorldV2,
    profile: &ReplaySimulationProfile,
    caps: &ExplorationCapsV1,
) -> Result<()> {
    world.validate_sealed()?;
    profile.validate(caps)?;
    if profile.objective == ReplayObjective::AttainmentAucV1 && profile.w_sim != 1 {
        return Err(Error::Invalid("v1 replay requires W_sim=1".into()));
    }
    Ok(())
}

fn available_actions<'a>(
    world: &ReplayRuntimeWorld<'a>,
    state: &ReplayState,
) -> Result<Vec<&'a ReplayActionSpecV1>> {
    let revealed_contexts: BTreeSet<&str> = state
        .context_by_node
        .values()
        .map(String::as_str)
        .chain(std::iter::once(world.manifest.baseline_context_signature))
        .collect();
    // Only known contexts are exposed; structural/cost/recovery exclusions are
    // made by the actual shared core, including the diagnostic classification.
    let mut available: Vec<_> = world
        .manifest
        .action_catalog
        .iter()
        .filter(|action| {
            !state.selected.contains(&action.record_seq)
                && revealed_contexts.contains(action.parent_context_signature.as_str())
        })
        .collect();
    available.sort_by_key(|action| action.record_seq);
    Ok(available)
}

fn legal_projection(actions: &[&ReplayActionSpecV1]) -> LegalActionsV1 {
    LegalActionsV1 {
        schema_version: "rsia.legal_actions.v1".into(),
        actions: actions
            .iter()
            .map(|action| LegalActionV1 {
                action_id: action_id(action.record_seq),
                action_seq: action.record_seq,
                branch_seq: action.branch_seq,
                target_depth: action.target_depth,
                kind: action.action_kind.clone(),
                estimated_cost_upper_micros: action.estimated_cost_upper_micros,
            })
            .collect(),
    }
}

fn resolve_batch<'a>(
    available: &[&'a ReplayActionSpecV1],
    action_ids: &[String],
    action_seqs: &[u32],
    width: u8,
) -> Result<Vec<&'a ReplayActionSpecV1>> {
    if action_ids.len() != action_seqs.len()
        || action_ids.is_empty()
        || action_ids.len() > usize::from(width)
    {
        return Err(Error::Invalid("invalid replay batch width".into()));
    }
    let by_seq: BTreeMap<_, _> = available
        .iter()
        .map(|transition| (transition.record_seq, *transition))
        .collect();
    let mut seen_seq = BTreeSet::new();
    let mut seen_branch = BTreeSet::new();
    let mut chosen = Vec::new();
    for (id, seq) in action_ids.iter().zip(action_seqs) {
        if id != &action_id(*seq) || !seen_seq.insert(*seq) {
            return Err(Error::Conflict("duplicate or forged replay action".into()));
        }
        let transition = by_seq.get(seq).ok_or(Error::NotFound)?;
        if !seen_branch.insert(transition.branch_seq) {
            return Err(Error::Conflict(
                "batch contains two successors from one branch".into(),
            ));
        }
        chosen.push(*transition);
    }
    Ok(chosen)
}

fn reveal_batch(
    world: &ReplayRuntimeWorld<'_>,
    state: &mut ReplayState,
    previous_available: &[&ReplayActionSpecV1],
    round_opportunities: &BTreeSet<u32>,
    chosen: &[&ReplayActionSpecV1],
) -> Result<Option<(ReplayTerminal, String)>> {
    let mut qualities = Vec::with_capacity(chosen.len());
    let mut censored = None;
    let mut oos = None;
    let mut observed = Vec::new();
    let mut historical_record_seqs = Vec::new();
    let mut historical_in_batch = 0u32;
    for action in chosen {
        if !state.selected.insert(action.record_seq) {
            return Err(Error::Conflict("duplicate selected replay action".into()));
        }
        increment_replay_counter(&mut state.coverage.attempted_actions, "attempted actions")?;
        let Some(transition) = lookup_transition(world, action)? else {
            increment_replay_counter(
                &mut state.coverage.out_of_support_actions,
                "out of support actions",
            )?;
            state.usage.cost_micros = None;
            state.usage.latency_millis = None;
            qualities.push(None);
            oos = Some("selected preregistered action has no historical successor".into());
            continue;
        };
        accumulate_usage(&mut state.usage, &transition.actual_usage)?;
        match &transition.outcome {
            ReplayTransitionOutcome::Observed { status } => {
                increment_replay_counter(&mut historical_in_batch, "batch historical probes")?;
                state.approved_root_spent_micros = state
                    .approved_root_spent_micros
                    .checked_add(action.estimated_cost_upper_micros.ok_or_else(|| {
                        Error::Invalid("selected action has no approved bound".into())
                    })?)
                    .ok_or_else(|| Error::Invalid("replay root spend overflow".into()))?;
                historical_record_seqs.push(transition.record_seq);
                if !matches!(status, ObservedStatus::UsageUncertain) {
                    increment_replay_counter(
                        &mut state.coverage.observed_actions,
                        "observed actions",
                    )?;
                }
                let quality = match status {
                    ObservedStatus::Valid { quality_micros } => Some(*quality_micros),
                    _ => None,
                };
                if let Some(quality) = quality {
                    state.best_quality = state.best_quality.max(quality);
                }
                qualities.push(quality);
                observed.push((*action, transition, status.clone()));
                if matches!(status, ObservedStatus::UsageUncertain) {
                    increment_replay_counter(
                        &mut state.coverage.censored_actions,
                        "censored actions",
                    )?;
                    censored = Some("historical usage is uncertain".into());
                }
            }
            ReplayTransitionOutcome::Censored { reason } => {
                increment_replay_counter(&mut historical_in_batch, "batch historical probes")?;
                state.approved_root_spent_micros = state
                    .approved_root_spent_micros
                    .checked_add(action.estimated_cost_upper_micros.ok_or_else(|| {
                        Error::Invalid("selected action has no approved bound".into())
                    })?)
                    .ok_or_else(|| Error::Invalid("replay root spend overflow".into()))?;
                historical_record_seqs.push(transition.record_seq);
                increment_replay_counter(&mut state.coverage.censored_actions, "censored actions")?;
                qualities.push(None);
                censored = Some(reason.clone());
            }
            ReplayTransitionOutcome::OutOfSupport { reason } => {
                increment_replay_counter(
                    &mut state.coverage.out_of_support_actions,
                    "out of support actions",
                )?;
                qualities.push(None);
                oos = Some(reason.clone());
            }
        }
    }
    let round = state
        .prefix
        .decisions_completed
        .checked_add(1)
        .ok_or_else(|| Error::Invalid("replay decision count overflow".into()))?;
    state.batches.push(ReplayBatchRecord {
        decision_round: round,
        action_seqs: chosen.iter().map(|item| item.record_seq).collect(),
        record_seqs: historical_record_seqs,
        revealed_quality_micros: qualities,
    });
    for _ in 0..historical_in_batch {
        state.probe_curve.push(state.best_quality);
    }
    if historical_in_batch > 0 {
        state.historical_probes = state
            .historical_probes
            .checked_add(historical_in_batch)
            .ok_or_else(|| Error::Invalid("historical probe count overflow".into()))?;
        state.simulated_rounds = state
            .simulated_rounds
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("simulated round count overflow".into()))?;
        state.round_curve.push(state.best_quality);
    }

    // The entire selected batch is revealed before any prefix mutation is
    // visible to the next call to decide_elastic.
    for (action, transition, status) in observed {
        let node = node_from_transition(action, status, state)?;
        state
            .context_by_node
            .insert(action.record_seq, transition.next_context_signature.clone());
        state.prefix.nodes.push(node);
    }
    state.prefix.nodes.sort_by_key(|node| node.node_seq);
    // Project after the whole batch, including new failed children and roots
    // revealed after an earlier recovery of their episode. Never rewrite the world.
    for node in &mut state.prefix.nodes {
        if let ObservedStatus::RepairableFailure {
            episode_id,
            dispatched_repairs,
            ..
        } = &mut node.status
        {
            *dispatched_repairs = state
                .recoveries_by_episode
                .get(episode_id)
                .copied()
                .unwrap_or(0);
        }
    }
    state.prefix.nodes_used = u8::try_from(state.prefix.nodes.len())
        .map_err(|_| Error::Invalid("replay node count overflow".into()))?;
    state.prefix.decisions_completed = round;
    update_focus(&mut state.prefix, chosen)?;
    let next_available = available_actions(world, state)?;
    // Terminal diagnostics need the full revealed catalog, including excluded
    // actions. Fairness waits retain the established structural projection:
    // a consumed parent/root is no longer an opportunity after this batch.
    let expanded_parents: BTreeSet<_> = state
        .prefix
        .nodes
        .iter()
        .filter_map(|node| node.search_parent_seq)
        .collect();
    let opened_branches: BTreeSet<_> = state
        .prefix
        .nodes
        .iter()
        .map(|node| node.branch_seq)
        .collect();
    let next_wait_available: Vec<_> = next_available
        .into_iter()
        .filter(|action| match &action.action_kind {
            ActionKindV1::Widen { .. } => {
                action.parent_context_signature == world.manifest.baseline_context_signature
                    && !opened_branches.contains(&action.branch_seq)
            }
            ActionKindV1::Deepen { parent_node_seq } => {
                !expanded_parents.contains(parent_node_seq)
                    && state.context_by_node.get(parent_node_seq)
                        == Some(&action.parent_context_signature)
            }
            ActionKindV1::Recover {
                failed_node_seq, ..
            } => {
                state.context_by_node.get(failed_node_seq) == Some(&action.parent_context_signature)
            }
        })
        .collect();
    update_waits(
        &mut state.prefix,
        previous_available,
        round_opportunities,
        chosen,
        &next_wait_available,
    )?;
    if let Some(reason) = censored {
        return Ok(Some((ReplayTerminal::Censored, reason)));
    }
    if let Some(reason) = oos {
        return Ok(Some((ReplayTerminal::OutOfSupport, reason)));
    }
    Ok(None)
}

fn node_from_transition(
    transition: &ReplayActionSpecV1,
    status: ObservedStatus,
    state: &mut ReplayState,
) -> Result<PrefixNodeV2> {
    let is_recover = matches!(transition.action_kind, ActionKindV1::Recover { .. });
    let search_parent_seq = match &transition.action_kind {
        ActionKindV1::Widen { .. } => None,
        ActionKindV1::Deepen { parent_node_seq } => Some(*parent_node_seq),
        ActionKindV1::Recover {
            failed_node_seq, ..
        } => {
            state.prefix.recovery_dispatches_used = state
                .prefix
                .recovery_dispatches_used
                .checked_add(1)
                .ok_or_else(|| Error::Invalid("recovery dispatch count overflow".into()))?;
            let failed = state
                .prefix
                .nodes
                .iter()
                .find(|node| node.node_seq == *failed_node_seq)
                .ok_or(Error::NotFound)?;
            if let ObservedStatus::RepairableFailure { episode_id, .. } = &failed.status {
                let used = state
                    .recoveries_by_episode
                    .entry(episode_id.clone())
                    .or_default();
                *used = used
                    .checked_add(1)
                    .ok_or_else(|| Error::Invalid("episode recovery count overflow".into()))?;
            } else {
                return Err(Error::Invalid(
                    "recover action does not target a repairable failure".into(),
                ));
            }
            Some(*failed_node_seq)
        }
    };
    let parent = search_parent_seq
        .and_then(|seq| state.prefix.nodes.iter().find(|node| node.node_seq == seq));
    let fixed_baseline = state.prefix.initial_baseline_quality_micros;
    let best_ancestor = parent
        .map(|node| {
            node_quality(node)
                .into_iter()
                .chain(node.best_valid_ancestor_micros)
                .chain(std::iter::once(fixed_baseline))
                .max()
                .unwrap_or(fixed_baseline)
        })
        .unwrap_or(fixed_baseline);
    let mut gains = parent
        .map(|node| node.recent_valid_gains_micros.clone())
        .unwrap_or_default();
    if let ObservedStatus::Valid { quality_micros } = &status {
        let baseline = parent
            .and_then(node_quality)
            .or_else(|| parent.and_then(|node| node.best_valid_ancestor_micros))
            .unwrap_or(fixed_baseline);
        gains.push(
            i32::try_from(i64::from(*quality_micros) - i64::from(baseline))
                .map_err(|_| Error::Invalid("replay quality gain overflow".into()))?,
        );
        if gains.len() > 2 {
            gains.remove(0);
        }
    }
    let repair_failures_dispatched = parent
        .map(|node| node.repair_failures_dispatched)
        .unwrap_or(0)
        .checked_add(u8::from(
            is_recover && matches!(status, ObservedStatus::RepairableFailure { .. }),
        ))
        .ok_or_else(|| Error::Invalid("repair failure count overflow".into()))?;
    let best_through_node = match &status {
        ObservedStatus::Valid { quality_micros } => best_ancestor.max(*quality_micros),
        _ => best_ancestor,
    };
    Ok(PrefixNodeV2 {
        node_seq: transition.record_seq,
        branch_seq: transition.branch_seq,
        search_parent_seq,
        approved_parent_digest: state.prefix.approved_parent_digest.clone(),
        depth: transition.target_depth,
        status,
        best_valid_ancestor_micros: Some(best_through_node),
        recent_valid_gains_micros: gains,
        repair_failures_dispatched,
    })
}

fn node_quality(node: &PrefixNodeV2) -> Option<u32> {
    match node.status {
        ObservedStatus::Valid { quality_micros } => Some(quality_micros),
        _ => None,
    }
}

fn update_focus(prefix: &mut PrefixViewV2, chosen: &[&ReplayActionSpecV1]) -> Result<()> {
    let branch = chosen
        .first()
        .ok_or_else(|| Error::Invalid("empty replay batch".into()))?
        .branch_seq;
    if prefix.current_branch_seq == Some(branch) {
        prefix.current_branch_focus_actions = prefix
            .current_branch_focus_actions
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("branch focus count overflow".into()))?;
    } else {
        prefix.current_branch_seq = Some(branch);
        prefix.current_branch_focus_actions = 1;
    }
    Ok(())
}

/// Moves the fairness waits on by one completed decision round (plan §7.1.1: waits
/// move per completed round, and only a legal opportunity waits). An action waits one
/// round more when it was a legal opportunity of the round (`round_opportunities`,
/// what the shared core filter left legal for the decision) and the batch did not
/// choose it. An action the filter excluded in the round (no authorized cost bound,
/// past the depth cap, a Recover that is not eligible, a successor the prefix cannot
/// have) is no opportunity: it neither waits nor stays among the waits. An action the
/// batch made available has met no decision round yet and enters at zero.
fn update_waits(
    prefix: &mut PrefixViewV2,
    previous: &[&ReplayActionSpecV1],
    round_opportunities: &BTreeSet<u32>,
    chosen: &[&ReplayActionSpecV1],
    next: &[&ReplayActionSpecV1],
) -> Result<()> {
    let old: BTreeMap<_, _> = prefix
        .waits
        .iter()
        .map(|wait| (wait.action_seq, wait.waited_rounds))
        .collect();
    let previous: BTreeSet<_> = previous.iter().map(|item| item.record_seq).collect();
    let chosen: BTreeSet<_> = chosen.iter().map(|item| item.record_seq).collect();
    prefix.waits = next
        .iter()
        .filter(|item| {
            !previous.contains(&item.record_seq) || round_opportunities.contains(&item.record_seq)
        })
        .map(|item| {
            Ok(OpportunityWait {
                action_seq: item.record_seq,
                waited_rounds: if round_opportunities.contains(&item.record_seq)
                    && !chosen.contains(&item.record_seq)
                {
                    old.get(&item.record_seq)
                        .copied()
                        .unwrap_or(0)
                        .checked_add(1)
                        .ok_or_else(|| Error::Invalid("fairness wait count overflow".into()))?
                } else {
                    0
                },
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(())
}

fn replay_runtime_budget(
    world: &ReplayRuntimeWorld<'_>,
    state: &ReplayState,
    actions: &[&ReplayActionSpecV1],
    profile: &ReplaySimulationProfile,
) -> Result<BudgetViewV1> {
    let used = u8::try_from(state.historical_probes)
        .map_err(|_| Error::Invalid("historical probe count overflow".into()))?;
    let remaining_root_micros = match world.initial_root_budget_micros {
        Some(initial) => initial
            .checked_sub(state.approved_root_spent_micros)
            .ok_or_else(|| Error::Invalid("replay authorized root balance underflow".into()))?,
        // Old worlds never asserted a frozen root-balance source. Preserve the
        // old known catalog bound only as an Unknown-root replay input.
        None => actions
            .iter()
            .filter_map(|action| action.estimated_cost_upper_micros)
            .try_fold(0u64, |sum, cost| {
                sum.checked_add(cost)
                    .ok_or_else(|| Error::Invalid("known replay cost bound overflow".into()))
            })?,
    };
    let recovery_limit = world
        .initial_recovery_dispatches
        .unwrap_or(profile.global_recovery_dispatch_limit)
        .min(profile.global_recovery_dispatch_limit);
    Ok(BudgetViewV1 {
        remaining_nodes: profile
            .probe_budget
            .checked_sub(used)
            .ok_or_else(|| Error::Invalid("replay real probe budget underflow".into()))?,
        remaining_recovery_dispatches: recovery_limit
            .checked_sub(state.prefix.recovery_dispatches_used)
            .ok_or_else(|| Error::Invalid("replay recovery budget underflow".into()))?,
        remaining_root_micros,
    })
}

fn increment_replay_counter(value: &mut u32, label: &str) -> Result<()> {
    *value = value
        .checked_add(1)
        .ok_or_else(|| Error::Invalid(format!("{label} overflow")))?;
    Ok(())
}

fn lookup_transition<'a>(
    world: &ReplayRuntimeWorld<'a>,
    action: &ReplayActionSpecV1,
) -> Result<Option<&'a ReplayTransitionV2>> {
    let action_kind = fingerprint(&action.action_kind)?;
    let mut found = None;
    for transition in world.transitions {
        if transition.record_seq == action.record_seq
            && transition.generation_signature == action.generation_signature
            && transition.parent_context_signature == action.parent_context_signature
            && fingerprint(&transition.action_kind)? == action_kind
        {
            if found.is_some() {
                return Err(Error::Conflict("ambiguous replay transition".into()));
            }
            found = Some(transition);
        }
    }
    Ok(found)
}

fn accumulate_usage(total: &mut HistoricalUsage, item: &HistoricalUsage) -> Result<()> {
    total.input_tokens = total
        .input_tokens
        .checked_add(item.input_tokens)
        .ok_or_else(|| Error::Invalid("historical input token overflow".into()))?;
    total.output_tokens = total
        .output_tokens
        .checked_add(item.output_tokens)
        .ok_or_else(|| Error::Invalid("historical output token overflow".into()))?;
    total.cost_micros = match (total.cost_micros, item.cost_micros) {
        (Some(left), Some(right)) => Some(
            left.checked_add(right)
                .ok_or_else(|| Error::Invalid("historical cost overflow".into()))?,
        ),
        _ => None,
    };
    total.latency_millis = match (total.latency_millis, item.latency_millis) {
        (Some(left), Some(right)) => Some(
            left.checked_add(right)
                .ok_or_else(|| Error::Invalid("historical latency overflow".into()))?,
        ),
        _ => None,
    };
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn finish_report(
    world: &ReplayRuntimeWorld<'_>,
    world_digest: String,
    policy_digest: String,
    profile_digest: String,
    profile: &ReplaySimulationProfile,
    mut state: ReplayState,
    terminal: ReplayTerminal,
    reason: String,
    started: Instant,
    policy_stop_exclusions: evo_core::replay::ReplayPolicyStopExclusions,
) -> Result<evo_core::replay::ReplayReportV3> {
    let complete = matches!(
        terminal,
        ReplayTerminal::PolicyStop | ReplayTerminal::BudgetExhausted
    );
    state.coverage.complete_support = complete;
    let probes = state.historical_probes;
    let rounds = state.simulated_rounds;
    let parallel_penalty = (probes > 0).then_some(RationalValue {
        numerator: rounds,
        denominator: probes,
    });
    let (attainment, q_auc, score) = if complete {
        fill_curve(
            &mut state.probe_curve,
            usize::from(profile.probe_budget),
            state.best_quality,
        );
        fill_curve(
            &mut state.round_curve,
            usize::from(profile.horizon),
            state.best_quality,
        );
        let attainment = mean_micros(&state.probe_curve)?;
        let q_auc = mean_micros(&state.round_curve)?;
        let score = i64::from(q_auc)
            - i64::from(profile.lambda_work_micros) * i64::from(probes)
                / i64::from(profile.probe_budget)
            - i64::from(profile.lambda_round_micros) * i64::from(rounds)
                / i64::from(profile.horizon);
        (Some(attainment), Some(q_auc), Some(score))
    } else {
        (None, None, None)
    };
    Ok(evo_core::replay::ReplayReportV3 {
        schema_version: evo_core::replay::REPLAY_REPORT_SCHEMA_V3.into(),
        world_id: world.manifest.world_id.to_owned(),
        world_digest,
        cluster_id: world.manifest.cluster_id.to_owned(),
        partition: world.manifest.partition,
        policy_digest,
        profile_digest,
        objective_version: profile.objective.version().into(),
        batches: state.batches,
        probe_best_quality_micros: state.probe_curve,
        round_best_quality_micros: state.round_curve,
        final_quality_micros: state.best_quality,
        probes,
        simulated_rounds: rounds,
        parallel_penalty,
        no_probe: probes == 0,
        attainment_auc_micros: attainment,
        q_auc_sim_micros: q_auc,
        score_v2_micros: (profile.objective == ReplayObjective::ParetoAttainmentV2)
            .then_some(score)
            .flatten(),
        historical_usage: state.usage,
        replay_cpu_nanos: started
            .elapsed()
            .as_nanos()
            .try_into()
            .map_err(|_| Error::Invalid("replay CPU counter overflow".into()))?,
        coverage: state.coverage,
        terminal,
        terminal_reason: reason,
        policy_stop_exclusions,
        root_contract_state: world.root_contract_state(),
        development_only: true,
        revealed_prefix: Some(state.prefix),
    })
}

fn terminal_report(
    world: &ReplayRuntimeWorld<'_>,
    policy_digest: &str,
    profile_digest: &str,
    profile: &ReplaySimulationProfile,
    terminal: ReplayTerminal,
    reason: String,
    started: Instant,
) -> Result<evo_core::replay::ReplayReportV3> {
    Ok(evo_core::replay::ReplayReportV3 {
        schema_version: evo_core::replay::REPLAY_REPORT_SCHEMA_V3.into(),
        world_id: world.manifest.world_id.to_owned(),
        world_digest: world.sealed_digest.unwrap_or_default().to_owned(),
        cluster_id: world.manifest.cluster_id.to_owned(),
        partition: world.manifest.partition,
        policy_digest: policy_digest.into(),
        profile_digest: profile_digest.into(),
        objective_version: profile.objective.version().into(),
        batches: Vec::new(),
        probe_best_quality_micros: Vec::new(),
        round_best_quality_micros: Vec::new(),
        final_quality_micros: world.manifest.initial_baseline_quality_micros,
        probes: 0,
        simulated_rounds: 0,
        parallel_penalty: None,
        no_probe: true,
        attainment_auc_micros: None,
        q_auc_sim_micros: None,
        score_v2_micros: None,
        historical_usage: HistoricalUsage::default(),
        replay_cpu_nanos: started
            .elapsed()
            .as_nanos()
            .try_into()
            .map_err(|_| Error::Invalid("replay CPU counter overflow".into()))?,
        coverage: ReplayCoverage::default(),
        terminal,
        terminal_reason: reason,
        policy_stop_exclusions: evo_core::replay::ReplayPolicyStopExclusions::default(),
        root_contract_state: world.root_contract_state(),
        development_only: true,
        revealed_prefix: None,
    })
}

fn fill_curve(curve: &mut Vec<u32>, length: usize, value: u32) {
    curve.resize(length, value);
}

fn mean_micros(values: &[u32]) -> Result<u32> {
    if values.is_empty() {
        return Ok(0);
    }
    let sum = values.iter().try_fold(0u64, |sum, value| {
        sum.checked_add(u64::from(*value))
            .ok_or_else(|| Error::Invalid("replay quality curve sum overflow".into()))
    })?;
    let length = u64::try_from(values.len())
        .map_err(|_| Error::Invalid("replay curve length overflow".into()))?;
    u32::try_from(sum / length).map_err(|_| Error::Invalid("replay curve mean overflow".into()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrajectoryMetrics {
    pub probes: u32,
    pub simulated_rounds: u32,
    pub parallel_penalty: Option<RationalValue>,
    pub attainment_auc_micros: u32,
    pub q_auc_sim_micros: u32,
    pub score_v2_micros: i64,
}

pub fn score_completed_trajectory(
    profile: &ReplaySimulationProfile,
    batch_sizes: &[u8],
    probe_best_quality_micros: &[u32],
    round_best_quality_micros: &[u32],
) -> Result<TrajectoryMetrics> {
    profile.validate(&ExplorationCapsV1::online())?;
    if batch_sizes
        .iter()
        .any(|size| *size == 0 || *size > profile.w_sim)
    {
        return Err(Error::Invalid("invalid completed batch size".into()));
    }
    let probes: u32 = batch_sizes.iter().try_fold(0u32, |sum, size| {
        sum.checked_add(u32::from(*size))
            .ok_or_else(|| Error::Invalid("completed probe count overflow".into()))
    })?;
    let rounds: u32 = batch_sizes.iter().try_fold(0u32, |sum, size| {
        sum.checked_add(u32::from(size.div_ceil(profile.w_sim)))
            .ok_or_else(|| Error::Invalid("completed round count overflow".into()))
    })?;
    if probes as usize != probe_best_quality_micros.len()
        || rounds as usize != round_best_quality_micros.len()
        || probes > u32::from(profile.probe_budget)
        || rounds > u32::from(profile.horizon)
        || probe_best_quality_micros
            .iter()
            .chain(round_best_quality_micros)
            .any(|quality| *quality > 1_000_000)
    {
        return Err(Error::Invalid("completed trajectory shape mismatch".into()));
    }
    if !probe_best_quality_micros
        .windows(2)
        .all(|pair| pair[0] <= pair[1])
        || !round_best_quality_micros
            .windows(2)
            .all(|pair| pair[0] <= pair[1])
    {
        return Err(Error::Invalid(
            "best-quality curves must be monotone".into(),
        ));
    }
    let mut probe_offset = 0usize;
    let mut round_offset = 0usize;
    for size in batch_sizes {
        let round_cost = usize::from(size.div_ceil(profile.w_sim));
        let revealed = round_best_quality_micros
            .get(round_offset + round_cost - 1)
            .ok_or_else(|| Error::Invalid("missing batch barrier quality".into()))?;
        if probe_best_quality_micros[probe_offset..probe_offset + usize::from(*size)]
            .iter()
            .any(|quality| quality != revealed)
        {
            return Err(Error::Invalid(
                "probe curve reveals within-batch ordering".into(),
            ));
        }
        probe_offset += usize::from(*size);
        round_offset += round_cost;
    }
    let final_quality = probe_best_quality_micros
        .last()
        .copied()
        .or_else(|| round_best_quality_micros.last().copied())
        .unwrap_or(0);
    let mut probe_curve = probe_best_quality_micros.to_vec();
    let mut round_curve = round_best_quality_micros.to_vec();
    fill_curve(
        &mut probe_curve,
        usize::from(profile.probe_budget),
        final_quality,
    );
    fill_curve(
        &mut round_curve,
        usize::from(profile.horizon),
        final_quality,
    );
    let attainment = mean_micros(&probe_curve)?;
    let q_auc = mean_micros(&round_curve)?;
    let score = i64::from(q_auc)
        - i64::from(profile.lambda_work_micros) * i64::from(probes)
            / i64::from(profile.probe_budget)
        - i64::from(profile.lambda_round_micros) * i64::from(rounds) / i64::from(profile.horizon);
    Ok(TrajectoryMetrics {
        probes,
        simulated_rounds: rounds,
        parallel_penalty: (probes > 0).then_some(RationalValue {
            numerator: rounds,
            denominator: probes,
        }),
        attainment_auc_micros: attainment,
        q_auc_sim_micros: q_auc,
        score_v2_micros: score,
    })
}

fn action_id(seq: u32) -> String {
    format!("replay-action-{seq}")
}

pub fn replay_v3_is_not_formal(_: &evo_core::replay::ReplayReportV3) -> Result<Verdict> {
    Err(Error::Invalid(
        "ReplayReportV3 is development-only and cannot become FormalEvaluation".into(),
    ))
}

pub fn replay_v2_is_not_formal(_: &ReplayReportV2) -> Result<Verdict> {
    Err(Error::Invalid(
        "ReplayReportV2 is development-only and cannot become FormalEvaluation".into(),
    ))
}

/// Legacy display-only record retained for replay_experiment compatibility.
/// New replay never uses `policy:seed` environment lookup.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayReport {
    pub world_id: String,
    pub policy: String,
    pub seed: String,
    pub action: String,
    pub oos: bool,
    pub censored: bool,
}

pub fn replay_is_not_formal(_: &ReplayReport) -> Result<Verdict> {
    Err(Error::Invalid("ReplayReport is development-only".into()))
}

pub fn compare_complete_v3(
    candidate: &evo_core::replay::ReplayReportV3,
    incumbent: &evo_core::replay::ReplayReportV3,
) -> Result<std::cmp::Ordering> {
    for report in [candidate, incumbent] {
        if report.schema_version != evo_core::replay::REPLAY_REPORT_SCHEMA_V3
            || report.objective_version != evo_core::replay::OBJECTIVE_V2
            || !report.coverage.complete_support
            || report.score_v2_micros.is_none()
        {
            return Err(Error::Invalid(
                "partial or non-v2 reports are not rankable".into(),
            ));
        }
    }
    if candidate.world_digest != incumbent.world_digest
        || candidate.profile_digest != incumbent.profile_digest
    {
        return Err(Error::Conflict("replay reports are not comparable".into()));
    }
    let by_score = candidate.score_v2_micros.cmp(&incumbent.score_v2_micros);
    if by_score != std::cmp::Ordering::Equal {
        return Ok(by_score);
    }
    let by_probes = incumbent.probes.cmp(&candidate.probes);
    if by_probes != std::cmp::Ordering::Equal {
        return Ok(by_probes);
    }
    let by_rounds = incumbent.simulated_rounds.cmp(&candidate.simulated_rounds);
    if by_rounds != std::cmp::Ordering::Equal {
        return Ok(by_rounds);
    }
    match (
        candidate.historical_usage.cost_micros,
        incumbent.historical_usage.cost_micros,
    ) {
        (Some(candidate), Some(incumbent)) => Ok(incumbent.cmp(&candidate)),
        (None, None) => Ok(std::cmp::Ordering::Equal),
        _ => Err(Error::Invalid("unknown cost is not zero".into())),
    }
}

pub fn validate_action_signature(transition: &ReplayTransitionV2) -> Result<()> {
    identifier(&transition.generation_signature)?;
    identifier(&transition.parent_context_signature)?;
    identifier(&transition.next_context_signature)
}

/// Typed, non-persistent three-state audit query. It never reruns the legacy
/// engine, grants new selection, changes old JSON or assumes current v3 exists.
#[derive(Debug, Clone, Serialize)]
pub struct ReplayAuditRecord {
    state: evo_core::replay::ReplayRecordState,
    #[serde(skip_serializing)]
    legacy_report: Option<evo_core::replay::StoredReplayReportV1>,
    #[serde(skip_serializing)]
    current_report: Option<evo_core::replay::StoredReplayReportV2>,
    invalid_reason: Option<String>,
}
impl ReplayAuditRecord {
    pub fn state(&self) -> evo_core::replay::ReplayRecordState {
        self.state
    }
    pub fn report(&self) -> Option<&evo_core::replay::StoredReplayReportV1> {
        self.legacy_report.as_ref()
    }
    pub fn current_report(&self) -> Option<&evo_core::replay::StoredReplayReportV2> {
        self.current_report.as_ref()
    }
    pub fn invalid_reason(&self) -> Option<&str> {
        self.invalid_reason.as_deref()
    }
    pub fn refuse_new_consumption(&self) -> evo_core::replay::ReplayReadIsolation {
        match self.state {
            evo_core::replay::ReplayRecordState::Legacy => {
                evo_core::replay::ReplayReadIsolation::LegacyNotConsumable {
                    report_id: self
                        .legacy_report
                        .as_ref()
                        .map(|r| r.report_id.clone())
                        .unwrap_or_default(),
                }
            }
            evo_core::replay::ReplayRecordState::Invalid => {
                evo_core::replay::ReplayReadIsolation::InvalidRecord {
                    reason: self
                        .invalid_reason
                        .clone()
                        .unwrap_or_else(|| "replay_record_invalid".into()),
                }
            }
            evo_core::replay::ReplayRecordState::CurrentVerified => {
                evo_core::replay::ReplayReadIsolation::InvalidRecord {
                    reason: "current record already qualified; use the verified current view"
                        .into(),
                }
            }
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct ReplayReportAuditView {
    pub record: ReplayAuditRecord,
    pub raw_payload: Option<serde_json::Value>,
    pub invalid_diagnostic: Option<String>,
    pub source_legacy: bool,
    pub lineage: Vec<evo_storage::replay::ReplayLineageEntry>,
}
fn invalid_replay_audit() -> ReplayReportAuditView {
    ReplayReportAuditView {
        record: ReplayAuditRecord {
            state: evo_core::replay::ReplayRecordState::Invalid,
            legacy_report: None,
            current_report: None,
            invalid_reason: Some("replay_record_invalid".into()),
        },
        raw_payload: None,
        invalid_diagnostic: Some("replay_record_invalid".into()),
        source_legacy: false,
        lineage: Vec::new(),
    }
}

pub async fn query_replay_report_record(
    ctx: &Context,
    store: &Store,
    report_id: &str,
) -> evo_core::replay::ReplayReadResult<ReplayReportAuditView> {
    ctx.require(&[evo_core::Role::Evaluator, evo_core::Role::Admin])?;
    let mut session = store.session().await?;
    let view = query_replay_report_record_in_session(&mut session, ctx, report_id).await?;
    session.commit().await?;
    Ok(view)
}

pub async fn query_replay_report_record_in_session(
    session: &mut evo_storage::Session,
    ctx: &Context,
    report_id: &str,
) -> evo_core::replay::ReplayReadResult<ReplayReportAuditView> {
    use evo_core::replay::{ReplayReadIsolation, ReplayRecordState, ReplayReportIdentityVersion};
    ctx.require(&[evo_core::Role::Evaluator, evo_core::Role::Admin])?;
    let version = match evo_storage::replay::replay_report_version_in_session(
        session, ctx, report_id,
    )
    .await
    {
        Ok(Some(version)) => version,
        Ok(None) | Err(ReplayReadIsolation::Source(Error::Invalid(_))) => {
            return Ok(invalid_replay_audit());
        }
        Err(error) => return Err(error),
    };
    let (record, raw_payload, source_legacy, identity) = match version {
        ReplayReportIdentityVersion::LegacyV2 => {
            let old =
                evo_storage::replay::query_replay_report_record_in_session(session, ctx, report_id)
                    .await?;
            if old.record.state() != ReplayRecordState::Legacy {
                return Ok(invalid_replay_audit());
            }
            let report =
                old.record
                    .report()
                    .cloned()
                    .ok_or_else(|| ReplayReadIsolation::InvalidRecord {
                        reason: "replay_record_invalid".into(),
                    })?;
            let identity = ReplayAuditIdentity::legacy(&report);
            (
                ReplayAuditRecord {
                    state: ReplayRecordState::Legacy,
                    legacy_report: Some(report),
                    current_report: None,
                    invalid_reason: None,
                },
                old.raw_payload,
                true,
                identity,
            )
        }
        ReplayReportIdentityVersion::NextV3 => {
            let stored = match load_and_verify_current_in_session(session, ctx, report_id).await {
                Ok(stored) => stored,
                Err(Error::Invalid(_)) => return Ok(invalid_replay_audit()),
                Err(error) => return Err(error.into()),
            };
            let raw = serde_json::to_value(&stored).map_err(|_| Error::Internal)?;
            let identity = ReplayAuditIdentity::current(&stored);
            // Sole current audit-read construction follows complete actual
            // same-Session source/static checks and v3 semantic recomputation.
            (
                ReplayAuditRecord {
                    state: ReplayRecordState::CurrentVerified,
                    legacy_report: None,
                    current_report: Some(stored),
                    invalid_reason: None,
                },
                Some(raw),
                false,
                identity,
            )
        }
    };
    let lineage = replay_lineage_in_session(session, ctx, &identity).await?;
    Ok(ReplayReportAuditView {
        record,
        raw_payload,
        invalid_diagnostic: None,
        source_legacy,
        lineage,
    })
}

struct ReplayAuditIdentity {
    namespace: String,
    partition: WorldPartition,
    pool: String,
    policy: String,
    profile: String,
    caps: String,
}
impl ReplayAuditIdentity {
    fn legacy(r: &evo_core::replay::StoredReplayReportV1) -> Self {
        Self {
            namespace: r.namespace.clone(),
            partition: r.partition,
            pool: r.pool_digest.clone(),
            policy: r.policy_digest.clone(),
            profile: r.profile_digest.clone(),
            caps: r.caps_digest.clone(),
        }
    }
    fn current(r: &evo_core::replay::StoredReplayReportV2) -> Self {
        Self {
            namespace: r.namespace.clone(),
            partition: r.partition,
            pool: r.pool_digest.clone(),
            policy: r.policy_digest.clone(),
            profile: r.profile_digest.clone(),
            caps: r.caps_digest.clone(),
        }
    }
    fn matches(&self, other: &Self) -> bool {
        self.namespace == other.namespace
            && self.partition == other.partition
            && self.pool == other.pool
            && self.policy == other.policy
            && self.profile == other.profile
            && self.caps == other.caps
    }
}
async fn load_and_verify_current_in_session(
    session: &mut evo_storage::Session,
    ctx: &Context,
    report_id: &str,
) -> Result<evo_core::replay::StoredReplayReportV2> {
    let stored =
        evo_storage::replay::load_current_replay_report_in_session(session, ctx, report_id).await?;
    let pool =
        evo_storage::replay::load_live_replay_pool_in_session(session, ctx, &stored.pool_digest)
            .await?;
    verify_stored_report_semantics(&pool, &stored)
        .map_err(|_| Error::Invalid("replay_current_semantics_invalid".into()))?;
    Ok(stored)
}

async fn replay_lineage_in_session(
    session: &mut evo_storage::Session,
    ctx: &Context,
    inputs: &ReplayAuditIdentity,
) -> evo_core::replay::ReplayReadResult<Vec<evo_storage::replay::ReplayLineageEntry>> {
    use evo_core::replay::{
        ReplayReadIsolation, ReplayRecordState, ReplayReportIdentityVersion,
        replay_report_id_for_version,
    };
    let mut result = Vec::new();
    for version in [
        ReplayReportIdentityVersion::LegacyV2,
        ReplayReportIdentityVersion::NextV3,
    ] {
        let id = replay_report_id_for_version(
            version,
            &inputs.namespace,
            inputs.partition,
            &inputs.pool,
            &inputs.policy,
            &inputs.profile,
            &inputs.caps,
        )?;
        let mut item = evo_storage::replay::ReplayLineageEntry {
            engine_version: version.engine_version().into(),
            report_id: id.clone(),
            exists: true,
            record_state: None,
            isolation_reason: None,
        };
        match evo_storage::replay::replay_report_version_in_session(session, ctx, &id).await {
            Err(ReplayReadIsolation::Source(Error::NotFound)) => item.exists = false,
            Err(ReplayReadIsolation::UnqualifiedVersionOutsideStaticTable) => {
                item.isolation_reason =
                    Some("replay_version_outside_static_table:unqualified_record".into());
            }
            Err(ReplayReadIsolation::Source(Error::Invalid(_))) | Ok(None) => {
                item.record_state = Some(ReplayRecordState::Invalid);
            }
            Err(error) => return Err(error),
            Ok(Some(found)) if found != version => {
                item.record_state = Some(ReplayRecordState::Invalid)
            }
            Ok(Some(ReplayReportIdentityVersion::LegacyV2)) => {
                let old =
                    evo_storage::replay::query_replay_report_record_in_session(session, ctx, &id)
                        .await?;
                item.record_state = Some(
                    if old.record.state() == ReplayRecordState::Legacy
                        && old
                            .record
                            .report()
                            .is_some_and(|r| inputs.matches(&ReplayAuditIdentity::legacy(r)))
                    {
                        ReplayRecordState::Legacy
                    } else {
                        ReplayRecordState::Invalid
                    },
                );
            }
            Ok(Some(ReplayReportIdentityVersion::NextV3)) => {
                match load_and_verify_current_in_session(session, ctx, &id).await {
                    Ok(current) if inputs.matches(&ReplayAuditIdentity::current(&current)) => {
                        item.record_state = Some(ReplayRecordState::CurrentVerified)
                    }
                    Ok(_) | Err(Error::Invalid(_)) => {
                        item.record_state = Some(ReplayRecordState::Invalid)
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
        result.push(item);
    }
    Ok(result)
}

// Compatibility boundary only: callers of the old Result<...> API retain its
// type. New consumers must use the typed query/error; legacy/invalid/outside
// reasons stay explicit and are never converted into a semantic Conflict.
pub(crate) fn replay_read_isolation_for_existing_api(
    error: evo_core::replay::ReplayReadIsolation,
) -> Error {
    match error {
        evo_core::replay::ReplayReadIsolation::Source(error) => error,
        other => Error::Invalid(other.to_string()),
    }
}
pub fn compare_complete_v2(
    _candidate: &ReplayReportV2,
    _incumbent: &ReplayReportV2,
) -> Result<std::cmp::Ordering> {
    Err(Error::Invalid("legacy_replay_not_consumable: ReplayReportV2 comparison retired; use qualified current v3 results".into()))
}

// AG147 R5 actual collector entry points. Preparation is plain unpersisted data,
// not a run/Host/paid authority and not a CurrentVerified replay report.
#[derive(Debug, Clone)]
pub struct ReplayWorldV3Seed {
    pub cluster_id: String,
    pub partition: WorldPartition,
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
    pub prefix_coverage: Vec<evo_core::replay::PrefixCoverageV1>,
    pub action_catalog: Vec<ReplayActionSpecV1>,
}

/// Admin actual-registration preparation. The returned DTO is unqualified: a
/// real trusted source producer must record the canonical baseline body first,
/// then collect_replay_world_v3_draft verifies its stored bytes before any put.
/// This function neither creates a run nor changes any source provenance.
pub async fn prepare_replay_world_v3_in_session(
    session: &mut evo_storage::Session,
    ctx: &Context,
    exploration_world_id: &str,
    seed: ReplayWorldV3Seed,
) -> Result<evo_core::replay::ReplayWorldV3> {
    ctx.require(&[evo_core::Role::Admin])?;
    identifier(&seed.baseline_observation_source_id)?;
    let captured = crate::exploration::capture_replay_registration_in_session(
        session,
        ctx,
        exploration_world_id,
    )
    .await?;
    let mut manifest = evo_core::replay::ReplayWorldManifestV3 {
        schema_version: evo_core::replay::REPLAY_MANIFEST_SCHEMA_V3.into(),
        world_id: String::new(),
        cluster_id: seed.cluster_id,
        partition: seed.partition,
        purpose: Purpose::Development,
        generation_signature: seed.generation_signature,
        world_context_signature: seed.world_context_signature,
        baseline_context_signature: seed.baseline_context_signature,
        approved_parent_digest: seed.approved_parent_digest,
        model_digest: seed.model_digest,
        tools_digest: seed.tools_digest,
        scorer_digest: seed.scorer_digest,
        guidance_digest: seed.guidance_digest,
        repair_template_digest: seed.repair_template_digest,
        input_order_digest: seed.input_order_digest,
        initial_baseline_quality_micros: seed.initial_baseline_quality_micros,
        baseline_observation_source_id: seed.baseline_observation_source_id.clone(),
        source_closure: Vec::new(),
        revoke_watermark: captured.source().source_watermark,
        prefix_coverage: seed.prefix_coverage,
        action_catalog: seed.action_catalog,
        root_source: captured.source().clone(),
    };
    // Reject mismatched S0 BEFORE even preparing bytes for an external source.
    captured.validate_replay_v3_s0(&manifest)?;
    manifest.world_id = evo_core::replay::replay_world_identity_v3(&manifest)?;
    let expected_baseline = evo_core::replay::replay_baseline_observation_bytes_v2(&manifest)?;
    manifest
        .source_closure
        .push(evo_core::replay::ReplaySourceRef {
            source_id: seed.baseline_observation_source_id,
            content_digest: evo_core::hash(&expected_baseline),
        });
    let world = evo_core::replay::ReplayWorldV3 {
        schema_version: evo_core::replay::REPLAY_WORLD_SCHEMA_V3.into(),
        manifest,
        transitions: Vec::new(),
        registration_image_utf8: String::from_utf8(captured.registration_image_utf8().to_vec())
            .map_err(|_| Error::Internal)?,
        sealed_digest: None,
    };
    world.validate_draft()?;
    Ok(world)
}

pub async fn prepare_replay_world_v3(
    ctx: &Context,
    store: &Store,
    exploration_world_id: &str,
    seed: ReplayWorldV3Seed,
) -> Result<evo_core::replay::ReplayWorldV3> {
    ctx.require(&[evo_core::Role::Admin])?;
    let mut session = store.session().await?;
    let world =
        prepare_replay_world_v3_in_session(&mut session, ctx, exploration_world_id, seed).await?;
    session.commit().await?;
    Ok(world)
}

/// The actual first collecting write. The proposed DTO is never authority: this
/// writing Session independently reloads registration through the real engine
/// decoder, then storage independently checks root material/S0/live full closure
/// and every exact TrustedRun/TrustedHost baseline/transition body before put.
pub async fn collect_replay_world_v3_draft_in_session(
    session: &mut evo_storage::Session,
    ctx: &Context,
    world: &evo_core::replay::ReplayWorldV3,
) -> Result<()> {
    ctx.require(&[evo_core::Role::Admin])?;
    let existing = match evo_storage::replay::load_replay_world_v3_collecting_in_session(
        session,
        ctx,
        &world.manifest.world_id,
    )
    .await
    {
        Ok(stored) => Some(stored),
        Err(Error::NotFound) => None,
        Err(error) => return Err(error),
    };
    let captured = crate::exploration::capture_replay_registration_in_session(
        session,
        ctx,
        &world.manifest.root_source.exploration_world_id,
    )
    .await?;
    if let Some(stored) = existing {
        // A real same-Session qualified draft retains its valid original proof
        // choice when another valid idempotent registration job was added later.
        // Storage still proves the WHOLE target and all frozen fields independently.
        if stored.manifest.root_source != world.manifest.root_source
            || stored.registration_image_utf8 != world.registration_image_utf8
        {
            return Err(Error::Conflict("replay_root_source_unproven".into()));
        }
    } else if captured.source() != &world.manifest.root_source
        || captured.registration_image_utf8() != world.registration_image_utf8.as_bytes()
    {
        return Err(Error::Conflict("replay_root_source_unproven".into()));
    }
    captured.validate_replay_v3_s0(&world.manifest)?;
    evo_storage::replay::put_replay_world_v3_draft_in_session(session, ctx, world).await
}

pub async fn collect_replay_world_v3_draft(
    ctx: &Context,
    store: &Store,
    world: &evo_core::replay::ReplayWorldV3,
) -> Result<()> {
    ctx.require(&[evo_core::Role::Admin])?;
    let mut session = store.session().await?;
    collect_replay_world_v3_draft_in_session(&mut session, ctx, world).await?;
    session.commit().await
}

/// Reads actual collecting history and actual newly stored observation sources;
/// no task/provider invocation, wall-clock replacement or paid usage synthesis.
pub async fn collect_replay_transition_v3(
    ctx: &Context,
    store: &Store,
    world_id: &str,
    action: &ReplayActionSpecV1,
    transition: &ReplayTransitionV2,
) -> Result<evo_core::replay::ReplayWorldV3> {
    ctx.require(&[evo_core::Role::Admin])?;
    let mut session = store.session().await?;
    let world = evo_storage::replay::collect_replay_transition_v3_in_session(
        &mut session,
        ctx,
        world_id,
        action,
        transition,
    )
    .await?;
    session.commit().await?;
    Ok(world)
}

pub async fn seal_replay_world_v3(
    ctx: &Context,
    store: &Store,
    world_id: &str,
) -> Result<evo_core::replay::ReplayWorldV3> {
    ctx.require(&[evo_core::Role::Admin])?;
    let mut session = store.session().await?;
    let world =
        evo_storage::replay::seal_replay_world_v3_in_session(&mut session, ctx, world_id).await?;
    session.commit().await?;
    Ok(world)
}

/// An internal borrowed algorithm view. It has no serde, schema retagging,
/// persistence or authorization constructor.
struct ReplayRuntimeManifest<'a> {
    world_id: &'a str,
    cluster_id: &'a str,
    partition: WorldPartition,
    world_context_signature: &'a str,
    baseline_context_signature: &'a str,
    approved_parent_digest: &'a str,
    initial_baseline_quality_micros: u32,
    source_closure: &'a [evo_core::replay::ReplaySourceRef],
    revoke_watermark: u64,
    prefix_coverage: &'a [evo_core::replay::PrefixCoverageV1],
    action_catalog: &'a [ReplayActionSpecV1],
}
struct ReplayRuntimeWorld<'a> {
    manifest: ReplayRuntimeManifest<'a>,
    transitions: &'a [ReplayTransitionV2],
    sealed_digest: Option<&'a str>,
    // Only a fully storage-qualified V3 loader supplies Some roots/budget.
    roots: Option<&'a [evo_core::replay::ReplayRootOpportunityV1]>,
    initial_root_budget_micros: Option<u64>,
    initial_recovery_dispatches: Option<u8>,
}
impl<'a> ReplayRuntimeWorld<'a> {
    fn legacy(world: &'a ReplayWorldV2) -> Self {
        let m = &world.manifest;
        Self {
            manifest: ReplayRuntimeManifest {
                world_id: &m.world_id,
                cluster_id: &m.cluster_id,
                partition: m.partition,
                world_context_signature: &m.world_context_signature,
                baseline_context_signature: &m.baseline_context_signature,
                approved_parent_digest: &m.approved_parent_digest,
                initial_baseline_quality_micros: m.initial_baseline_quality_micros,
                source_closure: &m.source_closure,
                revoke_watermark: m.revoke_watermark,
                prefix_coverage: &m.prefix_coverage,
                action_catalog: &m.action_catalog,
            },
            transitions: &world.transitions,
            sealed_digest: world.sealed_digest.as_deref(),
            roots: None,
            initial_root_budget_micros: None,
            initial_recovery_dispatches: None,
        }
    }
    fn current(loaded: &'a evo_storage::replay::LoadedReplayWorldV3) -> Result<Self> {
        let world = loaded.world();
        let m = &world.manifest;
        let image = evo_core::replay::ReplayRegistrationImageV1::parse_for_source(
            &world.registration_image_utf8,
            &m.root_source,
        )?;
        Ok(Self {
            manifest: ReplayRuntimeManifest {
                world_id: &m.world_id,
                cluster_id: &m.cluster_id,
                partition: m.partition,
                world_context_signature: &m.world_context_signature,
                baseline_context_signature: &m.baseline_context_signature,
                approved_parent_digest: &m.approved_parent_digest,
                initial_baseline_quality_micros: m.initial_baseline_quality_micros,
                source_closure: &m.source_closure,
                revoke_watermark: m.revoke_watermark,
                prefix_coverage: &m.prefix_coverage,
                action_catalog: &m.action_catalog,
            },
            transitions: &world.transitions,
            sealed_digest: world.sealed_digest.as_deref(),
            roots: Some(&m.root_source.roots),
            initial_root_budget_micros: Some(image.initial_root_budget_micros),
            initial_recovery_dispatches: Some(image.initial_recovery_dispatches),
        })
    }
    fn root_contract_state(&self) -> evo_core::replay::ReplayRootContractState {
        if self.roots.is_some() {
            evo_core::replay::ReplayRootContractState::ProvedClosed
        } else {
            evo_core::replay::ReplayRootContractState::Unknown
        }
    }
}

/// Structural coverage is decided before cost and global recovery eligibility.
/// Per-node actually used repair quota belongs to the frozen structural limit;
/// environment-reset/global dispatch balance remain the core's later filters.
fn empty_legal_frontier_v3(
    world: &ReplayRuntimeWorld<'_>,
    state: &ReplayState,
    caps: &ExplorationCapsV1,
) -> Result<Option<(ReplayTerminal, String)>> {
    if state.prefix.nodes_used >= caps.max_nodes {
        return Ok(None);
    }
    let Some(roots) = world.roots else {
        // A V2 catalog/exhausted label cannot certify the set of unused roots.
        return Ok(Some((
            ReplayTerminal::OutOfSupport,
            "frontier_not_covered".into(),
        )));
    };
    let coverage: BTreeMap<_, _> = world
        .manifest
        .prefix_coverage
        .iter()
        .map(|item| (item.context_signature.as_str(), item.exhausted))
        .collect();
    let mut missing_contexts = Vec::new();
    for root in roots {
        let opened = world.transitions.iter().any(|transition| {
            state.selected.contains(&transition.record_seq)
                && matches!(
                    transition.outcome,
                    ReplayTransitionOutcome::Observed { .. }
                        | ReplayTransitionOutcome::Censored { .. }
                )
                && matches!(transition.action_kind,
                    ActionKindV1::Widen { root_slot } if root_slot == root.root_slot)
        });
        if opened {
            continue;
        }
        let record = world.manifest.action_catalog.iter().any(|action| {
            !state.selected.contains(&action.record_seq)
                && action.record_seq == root.action_seq && action.branch_seq == root.branch_seq
                && action.target_depth == 1
                && action.parent_context_signature == world.manifest.baseline_context_signature
                && matches!(action.action_kind, ActionKindV1::Widen { root_slot } if root_slot == root.root_slot)
        });
        if !record {
            missing_contexts.push(world.manifest.baseline_context_signature);
        }
    }
    let expanded: BTreeSet<_> = state
        .prefix
        .nodes
        .iter()
        .filter_map(|node| node.search_parent_seq)
        .collect();
    for node in &state.prefix.nodes {
        if node.depth >= caps.max_depth {
            continue;
        }
        let context = state
            .context_by_node
            .get(&node.node_seq)
            .ok_or_else(|| Error::Conflict("revealed node context is absent".into()))?;
        let can_deepen = matches!(node.status, ObservedStatus::Valid { .. })
            && !expanded.contains(&node.node_seq);
        let can_recover = matches!(node.status,
            ObservedStatus::RepairableFailure { dispatched_repairs, .. }
                if dispatched_repairs < caps.max_repair_dispatches_per_episode)
            && node.repair_failures_dispatched < evo_core::strategy::MAX_REPAIR;
        if !can_deepen && !can_recover {
            continue;
        }
        let expected_depth = node
            .depth
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("frontier depth overflow".into()))?;
        let record = world.manifest.action_catalog.iter().any(|action| {
            !state.selected.contains(&action.record_seq)
                && action.parent_context_signature == *context
                && action.branch_seq == node.branch_seq
                && action.target_depth == expected_depth
                && match action.action_kind {
                    ActionKindV1::Deepen { parent_node_seq } => {
                        can_deepen && parent_node_seq == node.node_seq
                    }
                    ActionKindV1::Recover {
                        failed_node_seq, ..
                    } => can_recover && failed_node_seq == node.node_seq,
                    ActionKindV1::Widen { .. } => false,
                }
        });
        if !record {
            missing_contexts.push(context.as_str());
        }
    }
    if missing_contexts
        .iter()
        .any(|context| !coverage.get(context).copied().unwrap_or(false))
    {
        return Ok(Some((
            ReplayTerminal::OutOfSupport,
            "frontier_not_covered".into(),
        )));
    }
    if !missing_contexts.is_empty() {
        return Ok(Some((
            ReplayTerminal::WorldExhausted,
            "logged_frontier_exhausted".into(),
        )));
    }
    Ok(None)
}

#[derive(Clone, Copy)]
enum ReplayCoreExclusion {
    Depth,
    Node,
    NoCost,
    Balance,
    Recovery,
    Structure,
}
fn first_core_exclusion_v3(
    action: &LegalActionV1,
    prefix: &PrefixViewV2,
    budget: &BudgetViewV1,
    caps: &ExplorationCapsV1,
) -> Result<Option<ReplayCoreExclusion>> {
    if action.action_seq == 0 || action.branch_seq == 0 {
        return Err(Error::Invalid("forged replay action identity".into()));
    }
    if action.target_depth > caps.max_depth {
        return Ok(Some(ReplayCoreExclusion::Depth));
    }
    if budget.remaining_nodes == 0 || prefix.nodes_used >= caps.max_nodes {
        return Ok(Some(ReplayCoreExclusion::Node));
    }
    let Some(cost) = action.estimated_cost_upper_micros else {
        return Ok(Some(ReplayCoreExclusion::NoCost));
    };
    if cost > budget.remaining_root_micros {
        return Ok(Some(ReplayCoreExclusion::Balance));
    }
    if let ActionKindV1::Recover {
        failed_node_seq,
        episode_id,
    } = &action.kind
    {
        if budget.remaining_recovery_dispatches == 0 {
            return Ok(Some(ReplayCoreExclusion::Recovery));
        }
        let Some(node) = prefix
            .nodes
            .iter()
            .find(|node| node.node_seq == *failed_node_seq)
        else {
            return Ok(Some(ReplayCoreExclusion::Recovery));
        };
        if node.repair_failures_dispatched >= evo_core::strategy::MAX_REPAIR {
            return Ok(Some(ReplayCoreExclusion::Recovery));
        }
        if !matches!(&node.status, ObservedStatus::RepairableFailure {
            episode_id: observed_episode, environment_reset: true, dispatched_repairs, ..
        } if observed_episode == episode_id && *dispatched_repairs < caps.max_repair_dispatches_per_episode)
        {
            return Ok(Some(ReplayCoreExclusion::Recovery));
        }
    }
    let structural = match &action.kind {
        ActionKindV1::Widen { .. } => {
            action.target_depth != 1
                || prefix
                    .nodes
                    .iter()
                    .any(|node| node.branch_seq == action.branch_seq)
        }
        ActionKindV1::Deepen { parent_node_seq } => !prefix
            .nodes
            .iter()
            .find(|node| node.node_seq == *parent_node_seq)
            .is_some_and(|parent| {
                parent.branch_seq == action.branch_seq
                    && parent.depth.checked_add(1) == Some(action.target_depth)
                    && matches!(parent.status, ObservedStatus::Valid { .. })
                    && !prefix
                        .nodes
                        .iter()
                        .any(|node| node.search_parent_seq == Some(*parent_node_seq))
            }),
        ActionKindV1::Recover {
            failed_node_seq, ..
        } => !prefix
            .nodes
            .iter()
            .find(|node| node.node_seq == *failed_node_seq)
            .is_some_and(|parent| {
                parent.branch_seq == action.branch_seq
                    && parent.depth.checked_add(1) == Some(action.target_depth)
            }),
    };
    Ok(structural.then_some(ReplayCoreExclusion::Structure))
}

fn policy_stop_exclusions_v3(
    prefix: &PrefixViewV2,
    legal: &LegalActionsV1,
    budget: &BudgetViewV1,
    caps: &ExplorationCapsV1,
) -> Result<evo_core::replay::ReplayPolicyStopExclusions> {
    let kept = legal_candidates(prefix, legal, budget, caps)?
        .into_iter()
        .map(|action| action.action_seq)
        .collect::<BTreeSet<_>>();
    let mut counts = evo_core::replay::ReplayPolicyStopExclusions::default();
    for action in &legal.actions {
        let category = first_core_exclusion_v3(action, prefix, budget, caps)?;
        if category.is_some() == kept.contains(&action.action_seq) {
            return Err(Error::Conflict(
                "replay diagnostic/core filter drift".into(),
            ));
        }
        let field = match category {
            None => continue,
            Some(ReplayCoreExclusion::Depth) => &mut counts.depth_limit,
            Some(ReplayCoreExclusion::Node) => &mut counts.node_limit,
            Some(ReplayCoreExclusion::NoCost) => &mut counts.no_authorized_cost_bound,
            Some(ReplayCoreExclusion::Balance) => &mut counts.insufficient_balance,
            Some(ReplayCoreExclusion::Recovery) => &mut counts.recovery_ineligible,
            Some(ReplayCoreExclusion::Structure) => &mut counts.structurally_unavailable,
        };
        *field = field
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("replay diagnostic count overflow".into()))?;
    }
    Ok(counts)
}
