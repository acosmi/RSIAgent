//! Successor-quality experiment. Three streams × three rounds prove orchestration only.
use evo_core::evaluation::{ControlCondition, Verdict};
use evo_core::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamRound {
    pub stream: u8,
    pub round: u8,
    pub condition: ControlCondition,
    pub successor_digest: String,
}

pub fn orchestration_only(rounds: &[StreamRound]) -> Result<bool> {
    let streams: std::collections::BTreeSet<u8> = rounds.iter().map(|r| r.stream).collect();
    let max_round = rounds.iter().map(|r| r.round).max().unwrap_or(0);
    if streams.len() == 3 && max_round == 3 {
        return Ok(false);
    }
    Err(Error::Invalid("incomplete orchestration schedule".into()))
}

pub fn d_minus_c(d: Verdict, c: Verdict) -> Result<&'static str> {
    match (d, c) {
        (Verdict::Improved, _) => Ok("mechanism_plus_gain_not_claimed_without_n"),
        (_, Verdict::Improved) if d != Verdict::Improved => {
            Ok("no_extra_gain_from_evolving_improver")
        }
        _ => Ok("bounded_mechanism_reuse"),
    }
}

pub fn cannot_claim_l5() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use evo_core::evaluation::ControlCondition;

    #[test]
    fn three_by_three_is_not_statistical_power() {
        let mut rows = Vec::new();
        for s in 1..=3 {
            for r in 1..=3 {
                rows.push(StreamRound {
                    stream: s,
                    round: r,
                    condition: ControlCondition::FixedImproverC,
                    successor_digest: format!("d{s}{r}"),
                });
            }
        }
        assert!(!orchestration_only(&rows).unwrap());
        assert!(cannot_claim_l5());
        assert_eq!(
            d_minus_c(Verdict::Inconclusive, Verdict::Inconclusive).unwrap(),
            "bounded_mechanism_reuse"
        );
    }
}

// Independent successor streams (plan §3.4.2, E15, V118.a and the second half of
// V118.c). Library only: the preregistration is not persisted, and neither
// `meta.start` nor `experiment.register` reaches this code. The slices are
// disjoint and recomputable by construction; that is not statistical
// independence, and nothing stored proves the preregistration preceded any
// candidate.
use crate::meta::MetaTrialView;
use evo_core::evaluation::{
    ClusterObservation, ControlledTaskProfileMaterialsV1, ControlledTaskProfileV1, DataUse,
    FormalStatisticalUnit, PlanFeasibilityDisposition, PlanFeasibilityInput, PlanningSource,
    SCORE_MICROS_MAX, assess_plan_feasibility_v1,
};
use evo_core::{fingerprint, hash, identifier};
use std::collections::{BTreeMap, BTreeSet};

pub const META_STREAM_PREREGISTRATION_SCHEMA_V1: &str = "rsia.meta_stream_preregistration.v1";
pub const META_STREAM_SAMPLER_V1: &str = "rsia.meta_stream_sampler.hash_rank.v1";

/// How many registered semantic clusters every stream gets for each use.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaStreamSliceCountsV1 {
    pub s0_task_family: u32,
    pub development: u32,
    pub holdout: u32,
}

/// One stream's slices: cluster ids of the controlled task profile, never task
/// ids, seeds, candidates or results.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaStreamSlotV1 {
    pub stream_seq: u32,
    pub s0_task_family: Vec<String>,
    pub development: Vec<String>,
    pub holdout: Vec<String>,
}

/// The sampler record of a meta experiment's independent successor streams.
/// Every slice is a function of the registered clusters behind
/// `profile_digest`, `seed`, `per_stream` and the stream count, and is
/// recomputed by [`Self::validate_against`]. "Before any candidate" holds only
/// by construction: the record has no candidate or result field, but it is not
/// stored, so no stored order proves when it was made.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaStreamPreregistrationV1 {
    pub schema_version: String,
    pub id: String,
    pub research_family_id: String,
    pub profile_digest: String,
    pub sampler_version: String,
    pub seed: String,
    pub per_stream: MetaStreamSliceCountsV1,
    pub endpoint_round: u32,
    pub streams: Vec<MetaStreamSlotV1>,
}

impl MetaStreamPreregistrationV1 {
    /// Deals the slices without replacement. S0 task-family slices, then
    /// development slices, come from the Development clusters and holdout slices
    /// from the AcceptanceEpoch clusters (plan L1378); each draw ranks its
    /// remaining clusters by `hash(sampler, seed, use, cluster_id)` and hands
    /// them out stream by stream. Too few clusters is refused, never reused.
    #[allow(clippy::too_many_arguments)]
    pub fn preregister(
        id: &str,
        research_family_id: &str,
        profile: &ControlledTaskProfileV1,
        materials: &ControlledTaskProfileMaterialsV1,
        per_stream: MetaStreamSliceCountsV1,
        n_streams: u32,
        endpoint_round: u32,
        seed: &str,
    ) -> Result<Self> {
        let registered = profile.registered_clusters_against(materials)?;
        Self::sample(
            id,
            research_family_id,
            registered,
            per_stream,
            n_streams,
            endpoint_round,
            seed,
        )
    }

    // Both entry points reuse the same sampler after validating the materials.
    fn sample(
        id: &str,
        research_family_id: &str,
        registered: (String, Vec<(String, DataUse)>),
        per_stream: MetaStreamSliceCountsV1,
        n_streams: u32,
        endpoint_round: u32,
        seed: &str,
    ) -> Result<Self> {
        identifier(id)?;
        identifier(research_family_id)?;
        if seed.len() != 64
            || !seed
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(Error::Invalid("meta_stream_seed_invalid".into()));
        }
        if n_streams < 2
            || endpoint_round == 0
            || per_stream.s0_task_family == 0
            || per_stream.development == 0
            || per_stream.holdout == 0
        {
            return Err(Error::Invalid("meta_stream_shape_invalid".into()));
        }
        let (profile_digest, clusters) = registered;
        let mut taken: BTreeSet<String> = BTreeSet::new();
        let mut deal = |purpose: &str, data_use: DataUse, count: u32| -> Result<Vec<Vec<String>>> {
            let mut ranked = clusters
                .iter()
                .filter(|(cluster, cluster_use)| {
                    *cluster_use == data_use && !taken.contains(cluster)
                })
                .map(|(cluster, _)| {
                    let rank = format!("{META_STREAM_SAMPLER_V1}\n{seed}\n{purpose}\n{cluster}");
                    (hash(rank.as_bytes()), cluster.clone())
                })
                .collect::<Vec<_>>();
            ranked.sort_unstable();
            let mut ranked = ranked.into_iter().map(|(_, cluster)| cluster);
            (0..n_streams)
                .map(|_| {
                    let slice = ranked.by_ref().take(count as usize).collect::<Vec<_>>();
                    if slice.len() != count as usize {
                        return Err(Error::Invalid("meta_stream_clusters_insufficient".into()));
                    }
                    taken.extend(slice.iter().cloned());
                    Ok(slice)
                })
                .collect()
        };
        let s0_task_family = deal(
            "s0_task_family",
            DataUse::Development,
            per_stream.s0_task_family,
        )?;
        let development = deal("development", DataUse::Development, per_stream.development)?;
        let holdout = deal("holdout", DataUse::AcceptanceEpoch, per_stream.holdout)?;
        Ok(Self {
            schema_version: META_STREAM_PREREGISTRATION_SCHEMA_V1.into(),
            id: id.into(),
            research_family_id: research_family_id.into(),
            profile_digest,
            sampler_version: META_STREAM_SAMPLER_V1.into(),
            seed: seed.into(),
            per_stream,
            endpoint_round,
            streams: (0..n_streams)
                .zip(s0_task_family.into_iter().zip(development).zip(holdout))
                .map(
                    |(stream_seq, ((s0_task_family, development), holdout))| MetaStreamSlotV1 {
                        stream_seq,
                        s0_task_family,
                        development,
                        holdout,
                    },
                )
                .collect(),
        })
    }

    /// Checks a supplied record against the profile and recomputes it. A holdout
    /// cluster of two streams is `meta_stream_holdout_shared`; any other repeated
    /// cluster, or one outside its use's partition, is
    /// `meta_stream_slices_overlap`; any other difference from the recomputed
    /// record is `meta_stream_preregistration_mismatch`.
    pub fn validate_against(
        &self,
        profile: &ControlledTaskProfileV1,
        materials: &ControlledTaskProfileMaterialsV1,
    ) -> Result<()> {
        if self.schema_version != META_STREAM_PREREGISTRATION_SCHEMA_V1
            || self.sampler_version != META_STREAM_SAMPLER_V1
        {
            return Err(Error::Invalid("meta_stream_schema_unsupported".into()));
        }
        let mismatch = || Error::Conflict("meta_stream_preregistration_mismatch".into());
        let registered = profile.registered_clusters_against(materials)?;
        let uses: BTreeMap<&str, DataUse> = registered
            .1
            .iter()
            .map(|(cluster, data_use)| (cluster.as_str(), *data_use))
            .collect();
        let mut owners = BTreeMap::new();
        for (index, slot) in self.streams.iter().enumerate() {
            for (clusters, data_use, count) in [
                (
                    &slot.s0_task_family,
                    DataUse::Development,
                    self.per_stream.s0_task_family,
                ),
                (
                    &slot.development,
                    DataUse::Development,
                    self.per_stream.development,
                ),
                (
                    &slot.holdout,
                    DataUse::AcceptanceEpoch,
                    self.per_stream.holdout,
                ),
            ] {
                if clusters.len() != count as usize {
                    return Err(mismatch());
                }
                for cluster in clusters {
                    if uses.get(cluster.as_str()) != Some(&data_use) {
                        return Err(Error::Conflict("meta_stream_slices_overlap".into()));
                    }
                    if let Some(owner) = owners.insert(cluster.as_str(), index) {
                        let reason = if data_use == DataUse::AcceptanceEpoch && owner != index {
                            "meta_stream_holdout_shared"
                        } else {
                            "meta_stream_slices_overlap"
                        };
                        return Err(Error::Conflict(reason.into()));
                    }
                }
            }
        }
        let n_streams = u32::try_from(self.streams.len()).map_err(|_| mismatch())?;
        let expected = Self::sample(
            &self.id,
            &self.research_family_id,
            registered,
            self.per_stream,
            n_streams,
            self.endpoint_round,
            &self.seed,
        )?;
        if self.digest()? != expected.digest()? {
            return Err(mismatch());
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String> {
        fingerprint(self)
    }

    /// The only way to build a [`MetaStreamSetV1`], and its only check: this
    /// record is validated once, then stream `i` is bound to `trials[i]`. Only
    /// a view [`crate::meta::verified_meta_trial`] built is accepted. The trials
    /// must be distinct and forked from distinct S0 registrations (another trial
    /// of one S0 adds no n), all with the same old and new improver contents, so
    /// each is one repetition of the same paired unit.
    pub fn bind(
        &self,
        profile: &ControlledTaskProfileV1,
        materials: &ControlledTaskProfileMaterialsV1,
        trials: &[&MetaTrialView],
    ) -> Result<MetaStreamSetV1> {
        self.validate_against(profile, materials)?;
        if trials.len() != self.streams.len() {
            return Err(Error::Invalid("meta_stream_trial_count_mismatch".into()));
        }
        let mut trial_ids = BTreeSet::new();
        let mut s0_digests = BTreeSet::new();
        for trial in trials {
            if !trial_ids.insert(trial.trial_id()) {
                return Err(Error::Conflict("meta_stream_trial_reused".into()));
            }
            if !s0_digests.insert(trial.s0_template_digest()) {
                return Err(Error::Conflict("meta_stream_s0_reused".into()));
            }
            if trial.old_stream().content_digest() != trials[0].old_stream().content_digest()
                || trial.new_stream().content_digest() != trials[0].new_stream().content_digest()
            {
                return Err(Error::Conflict("meta_stream_improver_pair_differs".into()));
            }
        }
        Ok(MetaStreamSetV1 {
            preregistration_digest: self.digest()?,
            research_family_id: self.research_family_id.clone(),
            endpoint_round: self.endpoint_round,
            streams: self
                .streams
                .iter()
                .zip(trials)
                .map(|(slot, trial)| (trial.trial_id().to_owned(), slot.holdout.clone()))
                .collect(),
        })
    }
}

/// Streams bound to verified MetaTrials. Fields are private: only
/// [`MetaStreamPreregistrationV1::bind`] builds one, and nothing re-checks it.
#[derive(Debug, Clone)]
pub struct MetaStreamSetV1 {
    preregistration_digest: String,
    research_family_id: String,
    endpoint_round: u32,
    /// By `stream_seq`: the bound trial id and the stream's holdout clusters.
    streams: Vec<(String, Vec<String>)>,
}

/// One holdout cluster's scores of the two arms' successors at one round:
/// `c_score_micros` for the fixed arm C, `d_score_micros` for the evolving arm D.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaStreamEndpointRowV1 {
    pub stream_seq: u32,
    pub round: u32,
    pub cluster_id: String,
    pub c_score_micros: u32,
    pub d_score_micros: u32,
}

impl MetaStreamSetV1 {
    pub fn preregistration_digest(&self) -> &str {
        &self.preregistration_digest
    }

    pub fn research_family_id(&self) -> &str {
        &self.research_family_id
    }

    /// The meta experiment's n: the number of streams, not tasks, rounds or seeds.
    pub fn n(&self) -> usize {
        self.streams.len()
    }

    pub fn trial_id(&self, stream_seq: u32) -> Option<&str> {
        self.streams
            .get(stream_seq as usize)
            .map(|(trial_id, _)| trial_id.as_str())
    }

    pub fn holdout(&self, stream_seq: u32) -> Option<&[String]> {
        self.streams
            .get(stream_seq as usize)
            .map(|(_, holdout)| holdout.as_slice())
    }

    /// One observation per stream, keyed by its trial id: `d_i` is the
    /// equal-weight mean of D−C over the stream's holdout clusters at the
    /// preregistered endpoint round. Rows of other rounds never enter `d_i`. A
    /// foreign or repeated endpoint row is refused, and a missing one fails the
    /// whole sample instead of returning part of it.
    pub fn endpoint_observations(
        &self,
        rows: &[MetaStreamEndpointRowV1],
    ) -> Result<Vec<ClusterObservation>> {
        let mut differences = BTreeMap::new();
        for row in rows.iter().filter(|row| row.round == self.endpoint_round) {
            if !self
                .holdout(row.stream_seq)
                .is_some_and(|holdout| holdout.contains(&row.cluster_id))
            {
                return Err(Error::Invalid("meta_stream_endpoint_row_foreign".into()));
            }
            if row.c_score_micros > SCORE_MICROS_MAX || row.d_score_micros > SCORE_MICROS_MAX {
                return Err(Error::Invalid(
                    "meta_stream_endpoint_score_out_of_range".into(),
                ));
            }
            let difference = i64::from(row.d_score_micros) - i64::from(row.c_score_micros);
            if differences
                .insert((row.stream_seq, row.cluster_id.as_str()), difference)
                .is_some()
            {
                return Err(Error::Invalid("meta_stream_endpoint_row_duplicate".into()));
            }
        }
        (0_u32..)
            .zip(&self.streams)
            .map(|(stream_seq, (trial_id, holdout))| {
                let sum = holdout
                    .iter()
                    .map(|cluster| {
                        differences
                            .get(&(stream_seq, cluster.as_str()))
                            .copied()
                            .ok_or_else(|| {
                                Error::Invalid("meta_stream_endpoint_rows_incomplete".into())
                            })
                    })
                    .sum::<Result<i64>>()?;
                Ok(ClusterObservation {
                    cluster_id: trial_id.clone(),
                    d: sum as f64 / (holdout.len() as f64 * f64::from(SCORE_MICROS_MAX)),
                    weight: 1.0,
                })
            })
            .collect()
    }
}

/// What a bound stream set may report (plan §3.4.2, E15). Neither variant
/// carries a verdict, an alpha or a ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaStreamClaimV1 {
    /// The feasibility gate found the plan descriptive only: E15 reports
    /// bounded mechanism reuse and nothing else.
    BoundedMechanismReuseOnly,
    /// The gate the final approval also uses found the plan numerically
    /// compatible. It may go on to E01 registration; that is all it means.
    FormalPlanFeasible,
}

/// Runs the one feasibility gate, [`assess_plan_feasibility_v1`], for the
/// bound streams. The input must count successor streams, declare exactly the
/// bound streams as its available units and name the preregistered research
/// family.
pub fn meta_stream_claim(
    set: &MetaStreamSetV1,
    input: &PlanFeasibilityInput<'_>,
) -> Result<MetaStreamClaimV1> {
    if input.units.kind != FormalStatisticalUnit::IndependentSuccessorStream
        || !matches!(
            input.units.available_units,
            PlanningSource::Declared(units) if units as usize == set.n()
        )
        || input.alpha_plan.research_family_id != set.research_family_id
    {
        return Err(Error::Invalid("meta_stream_claim_input_mismatch".into()));
    }
    Ok(match assess_plan_feasibility_v1(input)?.disposition {
        PlanFeasibilityDisposition::NumericallyCompatible => MetaStreamClaimV1::FormalPlanFeasible,
        PlanFeasibilityDisposition::DescriptiveOnly => MetaStreamClaimV1::BoundedMechanismReuseOnly,
    })
}
