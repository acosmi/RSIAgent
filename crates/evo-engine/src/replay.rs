//! Immutable lookup-only replay through the same pure E09 decision consumer.

use evo_core::evaluation::Verdict;
use evo_core::evidence::Purpose;
use evo_core::replay::{
    HistoricalUsage, REPLAY_REPORT_SCHEMA, RationalValue, ReplayActionSpecV1, ReplayBatchRecord,
    ReplayCoverage, ReplayObjective, ReplayReportV2, ReplaySimulationProfile, ReplayTerminal,
    ReplayTransitionOutcome, ReplayTransitionV2, ReplayWorldV2, StoredReplayReportV1,
    WorldPartition, replay_report_id,
};
use evo_core::strategy::{
    ActionKindV1, BatchActionV1, BudgetViewV1, ElasticPolicyV1, ExplorationCapsV1, LegalActionV1,
    LegalActionsV1, ObservedStatus, OpportunityWait, PrefixNodeV2, PrefixViewV2, SimulationContext,
    decide_elastic,
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
    context_by_node: BTreeMap<u32, String>,
    best_quality: u32,
    batches: Vec<ReplayBatchRecord>,
    probe_curve: Vec<u32>,
    round_curve: Vec<u32>,
    historical_probes: u32,
    simulated_rounds: u32,
    usage: HistoricalUsage,
    coverage: ReplayCoverage,
}

pub fn run_replay(
    world: &ReplayWorldV2,
    policy: &ElasticPolicyV1,
    profile: &ReplaySimulationProfile,
    caps: &ExplorationCapsV1,
    authority: &LiveWorldAuthority,
) -> Result<ReplayReportV2> {
    let started = Instant::now();
    let policy_digest = fingerprint(policy)?;
    let profile_digest = fingerprint(profile)?;
    if let Err(error) = validate_inputs(world, profile, caps) {
        return Ok(terminal_report(
            world,
            &policy_digest,
            &profile_digest,
            profile,
            ReplayTerminal::InvalidWorld,
            error.to_string(),
            started,
        ));
    }
    let world_digest = world.sealed_digest.clone().ok_or(Error::Internal)?;
    if authority.revoke_watermark != world.manifest.revoke_watermark
        || world
            .manifest
            .source_closure
            .iter()
            .any(|source| authority.revoked_source_ids.contains(&source.source_id))
    {
        return Ok(terminal_report(
            world,
            &policy_digest,
            &profile_digest,
            profile,
            ReplayTerminal::SourceRevoked,
            "world source closure is not live".into(),
            started,
        ));
    }
    let mut state = ReplayState {
        prefix: PrefixViewV2 {
            schema_version: PrefixViewV2::SCHEMA.into(),
            context_signature: world.manifest.world_context_signature.clone(),
            approved_parent_digest: world.manifest.approved_parent_digest.clone(),
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
    };

    let (terminal, reason) = loop {
        if state.selected.len() >= usize::from(profile.probe_budget) {
            break (
                ReplayTerminal::BudgetExhausted,
                "probe budget exhausted".into(),
            );
        }
        let available = available_actions(world, &state)?;
        let legal = legal_projection(&available);
        let budget = BudgetViewV1 {
            remaining_nodes: profile
                .probe_budget
                .saturating_sub(state.selected.len() as u8),
            remaining_recovery_dispatches: profile
                .global_recovery_dispatch_limit
                .saturating_sub(state.prefix.recovery_dispatches_used),
            remaining_root_micros: remaining_known_budget(&available),
        };
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
            BatchActionV1::Stop { reason } if legal.actions.is_empty() => {
                let exhausted = frontier_is_exhausted(world, &state);
                break if exhausted {
                    (ReplayTerminal::WorldExhausted, reason)
                } else {
                    (ReplayTerminal::OutOfSupport, reason)
                };
            }
            BatchActionV1::Stop { reason } => {
                break (ReplayTerminal::PolicyStop, reason);
            }
            BatchActionV1::Dispatch {
                action_ids,
                action_seqs,
                ..
            } => {
                let chosen = resolve_batch(&available, &action_ids, &action_seqs, profile.w_sim)?;
                let batch_terminal = reveal_batch(world, &mut state, &available, &chosen)?;
                if let Some(terminal) = batch_terminal {
                    break terminal;
                }
            }
        }
    };

    Ok(finish_report(
        world,
        world_digest,
        policy_digest,
        profile_digest,
        profile,
        state,
        terminal,
        reason,
        started,
    ))
}

pub async fn run_persisted_replay(
    ctx: &Context,
    store: &Store,
    world_id: &str,
    purpose: Purpose,
    policy: &ElasticPolicyV1,
    profile: &ReplaySimulationProfile,
    caps: &ExplorationCapsV1,
) -> Result<ReplayReportV2> {
    let pool = evo_storage::replay::load_live_replay_pool(ctx, store, &profile.pool_digest).await?;
    profile.validate_for_pool(caps, &pool.manifest)?;
    if purpose != pool.manifest.compatibility.purpose {
        return Err(Error::Forbidden);
    }
    let world = pool
        .worlds
        .iter()
        .find(|world| world.manifest.world_id == world_id)
        .cloned()
        .ok_or(Error::NotFound)?;
    let report = run_replay(
        &world,
        policy,
        profile,
        caps,
        &LiveWorldAuthority {
            revoke_watermark: world.manifest.revoke_watermark,
            revoked_source_ids: BTreeSet::new(),
        },
    )?;
    tokio::task::yield_now().await;
    let final_pool =
        evo_storage::replay::load_live_replay_pool(ctx, store, &profile.pool_digest).await?;
    final_pool.manifest.validate_against(&final_pool.worlds)?;
    let final_world = final_pool
        .worlds
        .iter()
        .find(|candidate| candidate.manifest.world_id == world_id)
        .ok_or(Error::NotFound)?;
    if final_world.sealed_digest != world.sealed_digest
        || fingerprint(&final_world)? != fingerprint(&world)?
    {
        return Err(Error::Conflict(
            "replay world changed during persisted replay".into(),
        ));
    }
    Ok(report)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    pub reports: Vec<ReplayReportV2>,
    pub support_complete: bool,
    pub development_only: bool,
    pub promotion_eligible: bool,
    pub ineligibility_reasons: Vec<String>,
}

pub async fn run_and_persist_pool_replay(
    ctx: &Context,
    store: &Store,
    pool_digest: &str,
    partition: WorldPartition,
    policy: &ElasticPolicyV1,
    profile: &ReplaySimulationProfile,
    caps: &ExplorationCapsV1,
) -> Result<StoredReplayReportV1> {
    ctx.require(&[evo_core::Role::Worker, evo_core::Role::Admin])?;
    let pool = evo_storage::replay::load_live_replay_pool(ctx, store, pool_digest).await?;
    profile.validate_for_pool(caps, &pool.manifest)?;
    let expected_id = replay_report_id(
        ctx.namespace(),
        partition,
        pool_digest,
        &fingerprint(policy)?,
        &fingerprint(profile)?,
        &fingerprint(caps)?,
    )?;
    match evo_storage::replay::load_live_replay_report(ctx, store, &expected_id).await {
        Ok(existing) => {
            verify_stored_report_semantics(&pool, &existing)?;
            return Ok(existing);
        }
        Err(Error::NotFound) => {}
        Err(error) => return Err(error),
    }
    let reports = recompute_partition_reports(&pool, partition, policy, profile, caps)?;
    let stored = StoredReplayReportV1::build(
        ctx.namespace(),
        partition,
        &pool.manifest,
        policy.clone(),
        profile.clone(),
        caps.clone(),
        reports,
    )?;
    evo_storage::replay::put_replay_report(ctx, store, &stored).await?;
    let loaded =
        evo_storage::replay::load_live_replay_report(ctx, store, &stored.report_id).await?;
    verify_stored_report_semantics(&pool, &loaded)?;
    Ok(loaded)
}

pub async fn verified_replay_report_view(
    ctx: &Context,
    store: &Store,
    report_id: &str,
) -> Result<VerifiedReplayReportView> {
    ctx.require(&[evo_core::Role::Evaluator, evo_core::Role::Admin])?;
    let stored = evo_storage::replay::load_live_replay_report(ctx, store, report_id).await?;
    let pool = evo_storage::replay::load_live_replay_pool(ctx, store, &stored.pool_digest).await?;
    let recomputed = verify_stored_report_semantics(&pool, &stored)?;
    let support_complete = recomputed
        .iter()
        .all(|report| report.coverage.complete_support);
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
            .map(|member| {
                (
                    member.world_id.clone(),
                    member.world_digest.clone(),
                    member.report.cluster_id.clone(),
                )
            })
            .collect(),
        reports: stored
            .member_reports
            .iter()
            .map(|member| member.report.clone())
            .collect(),
        support_complete,
        development_only: true,
        promotion_eligible: false,
        ineligibility_reasons: reasons,
    })
}

fn verify_stored_report_semantics(
    pool: &evo_storage::replay::LoadedReplayPool,
    stored: &StoredReplayReportV1,
) -> Result<Vec<ReplayReportV2>> {
    stored.validate_static(&pool.manifest)?;
    let recomputed = recompute_partition_reports(
        pool,
        stored.partition,
        &stored.policy,
        &stored.profile,
        &stored.caps,
    )?;
    let rebuilt = StoredReplayReportV1::build(
        &stored.namespace,
        stored.partition,
        &pool.manifest,
        stored.policy.clone(),
        stored.profile.clone(),
        stored.caps.clone(),
        recomputed.clone(),
    )?;
    if rebuilt.report_id != stored.report_id
        || rebuilt.semantic_reports_digest != stored.semantic_reports_digest
    {
        return Err(Error::Conflict(
            "stored replay report differs from semantic recomputation".into(),
        ));
    }
    Ok(recomputed)
}

fn recompute_partition_reports(
    pool: &evo_storage::replay::LoadedReplayPool,
    partition: WorldPartition,
    policy: &ElasticPolicyV1,
    profile: &ReplaySimulationProfile,
    caps: &ExplorationCapsV1,
) -> Result<Vec<ReplayReportV2>> {
    profile.validate_for_pool(caps, &pool.manifest)?;
    pool.worlds
        .iter()
        .filter(|world| world.manifest.partition == partition)
        .map(|world| {
            run_replay(
                world,
                policy,
                profile,
                caps,
                &LiveWorldAuthority {
                    revoke_watermark: world.manifest.revoke_watermark,
                    revoked_source_ids: BTreeSet::new(),
                },
            )
        })
        .collect()
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
    world: &'a ReplayWorldV2,
    state: &ReplayState,
) -> Result<Vec<&'a ReplayActionSpecV1>> {
    let revealed_contexts: BTreeSet<&str> = state
        .context_by_node
        .values()
        .map(String::as_str)
        .chain(std::iter::once(
            world.manifest.baseline_context_signature.as_str(),
        ))
        .collect();
    let revealed_nodes: BTreeSet<u32> = state
        .prefix
        .nodes
        .iter()
        .map(|node| node.node_seq)
        .collect();
    let opened_branches: BTreeSet<u32> = state
        .prefix
        .nodes
        .iter()
        .map(|node| node.branch_seq)
        .collect();
    let mut available = Vec::new();
    for action in &world.manifest.action_catalog {
        if state.selected.contains(&action.record_seq)
            || !revealed_contexts.contains(action.parent_context_signature.as_str())
        {
            continue;
        }
        let legal_parent = match &action.action_kind {
            ActionKindV1::Widen { .. } => {
                action.parent_context_signature == world.manifest.baseline_context_signature
                    && !opened_branches.contains(&action.branch_seq)
            }
            ActionKindV1::Deepen { parent_node_seq } => {
                revealed_nodes.contains(parent_node_seq)
                    && state.context_by_node.get(parent_node_seq)
                        == Some(&action.parent_context_signature)
            }
            ActionKindV1::Recover {
                failed_node_seq, ..
            } => {
                revealed_nodes.contains(failed_node_seq)
                    && state.context_by_node.get(failed_node_seq)
                        == Some(&action.parent_context_signature)
            }
        };
        if legal_parent {
            available.push(action);
        }
    }
    available.sort_by_key(|transition| transition.record_seq);
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
    world: &ReplayWorldV2,
    state: &mut ReplayState,
    previous_available: &[&ReplayActionSpecV1],
    chosen: &[&ReplayActionSpecV1],
) -> Result<Option<(ReplayTerminal, String)>> {
    let mut qualities = Vec::with_capacity(chosen.len());
    let mut censored = None;
    let mut oos = None;
    let mut observed = Vec::new();
    let mut historical_record_seqs = Vec::new();
    let mut historical_in_batch = 0u8;
    for action in chosen {
        state.selected.insert(action.record_seq);
        state.coverage.attempted_actions += 1;
        let Some(transition) = lookup_transition(world, action)? else {
            state.coverage.out_of_support_actions += 1;
            qualities.push(None);
            oos = Some("selected preregistered action has no historical successor".into());
            continue;
        };
        accumulate_usage(&mut state.usage, &transition.actual_usage)?;
        match &transition.outcome {
            ReplayTransitionOutcome::Observed { status } => {
                historical_in_batch = historical_in_batch.saturating_add(1);
                historical_record_seqs.push(transition.record_seq);
                state.coverage.observed_actions += 1;
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
                    state.coverage.censored_actions += 1;
                    censored = Some("historical usage is uncertain".into());
                }
            }
            ReplayTransitionOutcome::Censored { reason } => {
                historical_in_batch = historical_in_batch.saturating_add(1);
                historical_record_seqs.push(transition.record_seq);
                state.coverage.censored_actions += 1;
                qualities.push(None);
                censored = Some(reason.clone());
            }
            ReplayTransitionOutcome::OutOfSupport { reason } => {
                state.coverage.out_of_support_actions += 1;
                qualities.push(None);
                oos = Some(reason.clone());
            }
        }
    }
    let round = state.prefix.decisions_completed + 1;
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
            .checked_add(u32::from(historical_in_batch))
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
    state.prefix.nodes_used = state.prefix.nodes.len() as u8;
    state.prefix.decisions_completed = round;
    update_focus(&mut state.prefix, chosen);
    let next_available = available_actions(world, state)?;
    update_waits(
        &mut state.prefix,
        previous_available,
        chosen,
        &next_available,
    );
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
            state.prefix.recovery_dispatches_used =
                state.prefix.recovery_dispatches_used.saturating_add(1);
            let failed = state
                .prefix
                .nodes
                .iter_mut()
                .find(|node| node.node_seq == *failed_node_seq)
                .ok_or(Error::NotFound)?;
            if let ObservedStatus::RepairableFailure {
                dispatched_repairs, ..
            } = &mut failed.status
            {
                *dispatched_repairs = dispatched_repairs.saturating_add(1);
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
        gains.push(i64::from(*quality_micros) as i32 - i64::from(baseline) as i32);
        if gains.len() > 2 {
            gains.remove(0);
        }
    }
    let repair_failures_dispatched = parent
        .map(|node| node.repair_failures_dispatched)
        .unwrap_or(0)
        + u8::from(is_recover && matches!(status, ObservedStatus::RepairableFailure { .. }));
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

fn update_focus(prefix: &mut PrefixViewV2, chosen: &[&ReplayActionSpecV1]) {
    let branch = chosen[0].branch_seq;
    if prefix.current_branch_seq == Some(branch) {
        prefix.current_branch_focus_actions = prefix.current_branch_focus_actions.saturating_add(1);
    } else {
        prefix.current_branch_seq = Some(branch);
        prefix.current_branch_focus_actions = 1;
    }
}

fn update_waits(
    prefix: &mut PrefixViewV2,
    previous: &[&ReplayActionSpecV1],
    chosen: &[&ReplayActionSpecV1],
    next: &[&ReplayActionSpecV1],
) {
    let old: BTreeMap<_, _> = prefix
        .waits
        .iter()
        .map(|wait| (wait.action_seq, wait.waited_rounds))
        .collect();
    let previous: BTreeSet<_> = previous.iter().map(|item| item.record_seq).collect();
    let chosen: BTreeSet<_> = chosen.iter().map(|item| item.record_seq).collect();
    prefix.waits = next
        .iter()
        .map(|item| OpportunityWait {
            action_seq: item.record_seq,
            waited_rounds: if previous.contains(&item.record_seq)
                && !chosen.contains(&item.record_seq)
            {
                old.get(&item.record_seq).copied().unwrap_or(0) + 1
            } else {
                0
            },
        })
        .collect();
}

fn frontier_is_exhausted(world: &ReplayWorldV2, state: &ReplayState) -> bool {
    let coverage: BTreeMap<_, _> = world
        .manifest
        .prefix_coverage
        .iter()
        .map(|item| (item.context_signature.as_str(), item.exhausted))
        .collect();
    let contexts: Vec<&str> = if state.prefix.nodes.is_empty() {
        vec![world.manifest.baseline_context_signature.as_str()]
    } else {
        state
            .prefix
            .nodes
            .iter()
            .filter(|node| !has_selected_child(world, node.node_seq, state))
            .filter_map(|node| {
                state
                    .context_by_node
                    .get(&node.node_seq)
                    .map(String::as_str)
            })
            .collect()
    };
    contexts
        .into_iter()
        .all(|context| coverage.get(context).copied().unwrap_or(false))
}

fn has_selected_child(world: &ReplayWorldV2, node_seq: u32, state: &ReplayState) -> bool {
    world.transitions.iter().any(|transition| {
        state.selected.contains(&transition.record_seq)
            && match transition.action_kind {
                ActionKindV1::Deepen { parent_node_seq } => parent_node_seq == node_seq,
                ActionKindV1::Recover {
                    failed_node_seq, ..
                } => failed_node_seq == node_seq,
                ActionKindV1::Widen { .. } => false,
            }
    })
}

fn remaining_known_budget(actions: &[&ReplayActionSpecV1]) -> u64 {
    actions
        .iter()
        .filter_map(|item| item.estimated_cost_upper_micros)
        .fold(0u64, u64::saturating_add)
}

fn lookup_transition<'a>(
    world: &'a ReplayWorldV2,
    action: &ReplayActionSpecV1,
) -> Result<Option<&'a ReplayTransitionV2>> {
    let action_kind = fingerprint(&action.action_kind)?;
    let mut found = None;
    for transition in &world.transitions {
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
    world: &ReplayWorldV2,
    world_digest: String,
    policy_digest: String,
    profile_digest: String,
    profile: &ReplaySimulationProfile,
    mut state: ReplayState,
    terminal: ReplayTerminal,
    reason: String,
    started: Instant,
) -> ReplayReportV2 {
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
        let attainment = mean_micros(&state.probe_curve);
        let q_auc = mean_micros(&state.round_curve);
        let score = i64::from(q_auc)
            - i64::from(profile.lambda_work_micros) * i64::from(probes)
                / i64::from(profile.probe_budget)
            - i64::from(profile.lambda_round_micros) * i64::from(rounds)
                / i64::from(profile.horizon);
        (Some(attainment), Some(q_auc), Some(score))
    } else {
        (None, None, None)
    };
    ReplayReportV2 {
        schema_version: REPLAY_REPORT_SCHEMA.into(),
        world_id: world.manifest.world_id.clone(),
        world_digest,
        cluster_id: world.manifest.cluster_id.clone(),
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
        replay_cpu_nanos: started.elapsed().as_nanos().try_into().unwrap_or(u64::MAX),
        coverage: state.coverage,
        terminal,
        terminal_reason: reason,
        development_only: true,
        revealed_prefix: Some(state.prefix),
    }
}

fn terminal_report(
    world: &ReplayWorldV2,
    policy_digest: &str,
    profile_digest: &str,
    profile: &ReplaySimulationProfile,
    terminal: ReplayTerminal,
    reason: String,
    started: Instant,
) -> ReplayReportV2 {
    ReplayReportV2 {
        schema_version: REPLAY_REPORT_SCHEMA.into(),
        world_id: world.manifest.world_id.clone(),
        world_digest: world.sealed_digest.clone().unwrap_or_default(),
        cluster_id: world.manifest.cluster_id.clone(),
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
        replay_cpu_nanos: started.elapsed().as_nanos().try_into().unwrap_or(u64::MAX),
        coverage: ReplayCoverage::default(),
        terminal,
        terminal_reason: reason,
        development_only: true,
        revealed_prefix: None,
    }
}

fn fill_curve(curve: &mut Vec<u32>, length: usize, value: u32) {
    curve.resize(length, value);
}

fn mean_micros(values: &[u32]) -> u32 {
    if values.is_empty() {
        return 0;
    }
    let sum: u64 = values.iter().map(|value| u64::from(*value)).sum();
    (sum / values.len() as u64) as u32
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
    let probes: u32 = batch_sizes.iter().map(|size| u32::from(*size)).sum();
    let rounds: u32 = batch_sizes
        .iter()
        .map(|size| u32::from(size.div_ceil(profile.w_sim)))
        .sum();
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
    let attainment = mean_micros(&probe_curve);
    let q_auc = mean_micros(&round_curve);
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

pub fn compare_complete_v2(
    candidate: &ReplayReportV2,
    incumbent: &ReplayReportV2,
) -> Result<std::cmp::Ordering> {
    for report in [candidate, incumbent] {
        if report.objective_version != evo_core::replay::OBJECTIVE_V2
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
