//! Fixed offline-only curriculum profiles. This module does not grant a sandbox.
use evo_core::curriculum::{RegisteredProperty, StructuredTestProposalV1};
use evo_core::{Error, Result, fingerprint, hash, identifier};
use serde::{Deserialize, Serialize};

// The registered pure-function target, compiled from the exact source whose
// digest `target_source_body` registers. `development.rs` holds a private copy
// for its in-process runner and is outside this module's reach, so the
// offline conclusion that `registered_offline_check` recomputes includes the
// same file again (a pure function; no state, no type crosses the modules).
#[allow(clippy::duplicate_mod)]
#[path = "../../../examples/reference-host/src/curriculum_target.rs"]
mod curriculum_target;

pub const PURE_FUNCTION_PROFILE_ID: &str = "pure_function_test_proposal.v1";
pub const CLAMP_TARGET_ID: &str = "reference_host.clamp_i64.v1";
pub const DECLARED_CPU_LIMIT_MS: u32 = 1_000;
pub const DECLARED_OUTPUT_LIMIT_BYTES: usize = 16 * 1024;
pub const CLAMP_TARGET_SOURCE_ID: &str = "reference-host-clamp-i64-source-v1";
pub const CLAMP_ORACLE_SOURCE_ID: &str = "engine-clamp-i64-oracle-v1";
pub const DISABLED_RUNNER_SOURCE_ID: &str = "engine-disabled-curriculum-runner-v1";

fn target_source_body() -> serde_json::Value {
    serde_json::json!({
        "kind":"rust_source",
        "path":"examples/reference-host/src/curriculum_target.rs",
        "utf8":include_str!("../../../examples/reference-host/src/curriculum_target.rs")
    })
}

fn oracle_source_body() -> serde_json::Value {
    serde_json::json!({
        "kind":"fixed_branch_oracle",
        "version":"v1",
        "algorithm":"if value < min { min } else if value > max { max } else { value }"
    })
}

fn disabled_runner_source_body() -> serde_json::Value {
    serde_json::json!({
        "kind":"runner_contract",
        "version":"v1",
        "runtime":"disabled",
        "isolation":"sandbox_unavailable",
        "provider_dispatch":false
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileRuntimeStatus {
    DisabledSandboxUnavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredPureFunctionProfileV1 {
    pub schema_version: String,
    pub profile_id: String,
    pub target_id: String,
    pub target_source_id: String,
    pub target_digest: String,
    pub oracle_source_id: String,
    pub oracle_digest: String,
    pub runner_source_id: String,
    pub runner_digest: String,
    pub input_schema_digest: String,
    pub output_schema_digest: String,
    pub min_value: i64,
    pub max_value: i64,
    pub max_concrete_inputs: u8,
    pub max_normalized_json_bytes: usize,
    pub declared_cpu_limit_ms: u32,
    pub declared_output_limit_bytes: usize,
    pub runtime_status: ProfileRuntimeStatus,
}

impl RegisteredPureFunctionProfileV1 {
    pub fn clamp_i64() -> Self {
        Self {
            schema_version: "rsia.registered_pure_function_profile.v1".into(),
            profile_id: PURE_FUNCTION_PROFILE_ID.into(),
            target_id: CLAMP_TARGET_ID.into(),
            target_source_id: CLAMP_TARGET_SOURCE_ID.into(),
            target_digest: fingerprint(&target_source_body()).expect("fixed target source"),
            oracle_source_id: CLAMP_ORACLE_SOURCE_ID.into(),
            oracle_digest: fingerprint(&oracle_source_body()).expect("fixed oracle source"),
            runner_source_id: DISABLED_RUNNER_SOURCE_ID.into(),
            runner_digest: fingerprint(&disabled_runner_source_body())
                .expect("fixed runner contract"),
            input_schema_digest: hash(b"{value:i64,min:i64,max:i64}:domain+-1000000"),
            output_schema_digest: hash(b"{clamped:i64}"),
            min_value: -1_000_000,
            max_value: 1_000_000,
            max_concrete_inputs: 32,
            max_normalized_json_bytes: 16 * 1024,
            declared_cpu_limit_ms: DECLARED_CPU_LIMIT_MS,
            declared_output_limit_bytes: DECLARED_OUTPUT_LIMIT_BYTES,
            runtime_status: ProfileRuntimeStatus::DisabledSandboxUnavailable,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != "rsia.registered_pure_function_profile.v1"
            || self.profile_id != PURE_FUNCTION_PROFILE_ID
            || self.target_id != CLAMP_TARGET_ID
            || self.target_source_id != CLAMP_TARGET_SOURCE_ID
            || self.oracle_source_id != CLAMP_ORACLE_SOURCE_ID
            || self.runner_source_id != DISABLED_RUNNER_SOURCE_ID
            || self.target_digest != fingerprint(&target_source_body())?
            || self.oracle_digest != fingerprint(&oracle_source_body())?
            || self.runner_digest != fingerprint(&disabled_runner_source_body())?
            || self.target_digest == self.oracle_digest
            || self.runner_digest == self.target_digest
            || self.runner_digest == self.oracle_digest
            || self.min_value != -1_000_000
            || self.max_value != 1_000_000
            || self.max_concrete_inputs != 32
            || self.max_normalized_json_bytes != 16 * 1024
            || self.declared_cpu_limit_ms != DECLARED_CPU_LIMIT_MS
            || self.declared_output_limit_bytes != DECLARED_OUTPUT_LIMIT_BYTES
            || self.runtime_status != ProfileRuntimeStatus::DisabledSandboxUnavailable
        {
            return Err(Error::Invalid("unsupported pure-function profile".into()));
        }
        for source_id in [
            &self.target_source_id,
            &self.oracle_source_id,
            &self.runner_source_id,
        ] {
            identifier(source_id)?;
        }
        for value in [
            &self.target_digest,
            &self.oracle_digest,
            &self.runner_digest,
            &self.input_schema_digest,
            &self.output_schema_digest,
        ] {
            if value.len() != 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(Error::Invalid("profile digest is invalid".into()));
            }
        }
        Ok(())
    }
}

pub fn registered_profile_source_bodies() -> [(String, serde_json::Value); 3] {
    [
        (CLAMP_TARGET_SOURCE_ID.into(), target_source_body()),
        (CLAMP_ORACLE_SOURCE_ID.into(), oracle_source_body()),
        (
            DISABLED_RUNNER_SOURCE_ID.into(),
            disabled_runner_source_body(),
        ),
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClampInput {
    pub value: i64,
    pub min: i64,
    pub max: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClampOutput {
    pub clamped: i64,
}

/// Independent reference implementation: explicit branches, not target output.
pub fn clamp_oracle(input: ClampInput) -> Result<ClampOutput> {
    validate_input(input)?;
    let clamped = if input.value < input.min {
        input.min
    } else if input.value > input.max {
        input.max
    } else {
        input.value
    };
    Ok(ClampOutput { clamped })
}

fn validate_input(input: ClampInput) -> Result<()> {
    if input.min < -1_000_000
        || input.max > 1_000_000
        || input.value < -1_000_000
        || input.value > 1_000_000
        || input.min > input.max
    {
        return Err(Error::Invalid(
            "clamp input outside registered domain".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "result", deny_unknown_fields)]
pub enum OfflineProfileCheck {
    Passed {
        target_output_digest: String,
        oracle_output_digest: String,
    },
    Quarantined {
        reason: String,
    },
}

/// Server-side offline conclusion of one proposal (plan §8.3): runs the
/// registered in-process pure-function target twice on the proposal input and
/// checks it against the independent oracle and the registered property with
/// [`check_target_outputs`]. A proposal input outside the registered domain is
/// rejected as invalid. Nothing here is a sandbox, a provider or a model.
pub fn registered_offline_check(
    profile: &RegisteredPureFunctionProfileV1,
    proposal: &StructuredTestProposalV1,
) -> Result<OfflineProfileCheck> {
    profile.validate()?;
    proposal.validate()?;
    if proposal.target_id != curriculum_target::TARGET_ID {
        return Ok(OfflineProfileCheck::Quarantined {
            reason: "unknown_target".into(),
        });
    }
    let execute = || {
        curriculum_target::clamp_i64(curriculum_target::ClampInput {
            value: proposal.value,
            min: proposal.min,
            max: proposal.max,
        })
        .map(|clamped| ClampOutput { clamped })
        .map_err(|_| Error::Invalid("proposal input outside the registered target domain".into()))
    };
    let target = execute()?;
    let repeated = execute()?;
    check_target_outputs(profile, proposal, target, repeated)
}

pub fn check_target_outputs(
    profile: &RegisteredPureFunctionProfileV1,
    proposal: &StructuredTestProposalV1,
    target: ClampOutput,
    repeated_target: ClampOutput,
) -> Result<OfflineProfileCheck> {
    profile.validate()?;
    proposal.validate()?;
    identifier(&proposal.id)?;
    if proposal.target_id != profile.target_id {
        return Ok(OfflineProfileCheck::Quarantined {
            reason: "unknown_target".into(),
        });
    }
    let input = ClampInput {
        value: proposal.value,
        min: proposal.min,
        max: proposal.max,
    };
    let oracle = clamp_oracle(input)?;
    if target != oracle {
        return Ok(OfflineProfileCheck::Quarantined {
            reason: "reference_disagreement".into(),
        });
    }
    if repeated_target != target {
        return Ok(OfflineProfileCheck::Quarantined {
            reason: "nondeterministic_target".into(),
        });
    }
    let property_holds = match proposal.property {
        RegisteredProperty::ReferenceEqual => true,
        RegisteredProperty::DeterministicRepeat => true,
        RegisteredProperty::BelowMapsToMin => {
            input.value < input.min && target.clamped == input.min
        }
        RegisteredProperty::AboveMapsToMax => {
            input.value > input.max && target.clamped == input.max
        }
        RegisteredProperty::InRangeIdentity => {
            input.value >= input.min && input.value <= input.max && target.clamped == input.value
        }
    };
    if !property_holds {
        return Ok(OfflineProfileCheck::Quarantined {
            reason: "registered_property_precondition_or_invariant_failed".into(),
        });
    }
    Ok(OfflineProfileCheck::Passed {
        target_output_digest: fingerprint(&target)?,
        oracle_output_digest: fingerprint(&oracle)?,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", deny_unknown_fields)]
pub enum ValidityReportV1 {
    Quarantined {
        profile_id: String,
        proposal_id: String,
        proposal_digest: String,
        target_digest: String,
        oracle_digest: String,
        runner_digest: String,
        reason: String,
    },
    SandboxUnavailable {
        profile_id: String,
        proposal_id: String,
        proposal_digest: String,
        target_digest: String,
        oracle_digest: String,
        runner_digest: String,
    },
    /// Runner-verified development eligibility (plan §8.2–§8.4, V109). The
    /// report claims that the proposal passed its offline check and that a
    /// verified development observation of the proposal's family exists. A
    /// report only takes effect once `record_validity_report` persisted it,
    /// which re-verifies every claim in one transaction:
    ///
    /// - the proposal has no recorded `quarantined` report, and the registered
    ///   offline check, recomputed with the in-process target and the
    ///   independent oracle, is `passed` (a quarantined proposal never becomes
    ///   eligible);
    /// - `offline_output_digest` is the `evo_core::fingerprint` of exactly that
    ///   `OfflineProfileCheck::Passed` value (absent or different: refused);
    /// - the `DevelopmentRequestPrepared`/`DevelopmentObserved` stage-fact pair
    ///   reloads and verifies (`verified_development_observation_in_session`)
    ///   in the learner state's environment and grader version, one of its
    ///   verified outcomes belongs to the proposal's family, and that
    ///   outcome's registered development control binds the registered
    ///   target, oracle and in-process runner;
    /// - the report gets dependency edges to the proposal, its probe job and
    ///   both stage facts.
    ///
    /// This binds the proposal to already-verified in-process
    /// `RegisteredPureFunctionExecution` facts only. It still grants no
    /// sandbox: the profile runtime status stays `DisabledSandboxUnavailable`
    /// and no provider, model or host execution is implied.
    RunnerVerified {
        profile_id: String,
        proposal_id: String,
        proposal_digest: String,
        target_digest: String,
        oracle_digest: String,
        runner_digest: String,
        development_request_fact_id: String,
        development_observed_fact_id: String,
        /// `evo_core::fingerprint` of the proposal's passed offline
        /// conclusion. A wire without it still decodes and is refused by
        /// `record_validity_report`.
        #[serde(default)]
        offline_output_digest: Option<String>,
    },
}

impl ValidityReportV1 {
    pub fn profile_id(&self) -> &str {
        match self {
            Self::Quarantined { profile_id, .. }
            | Self::SandboxUnavailable { profile_id, .. }
            | Self::RunnerVerified { profile_id, .. } => profile_id,
        }
    }

    pub fn proposal_id(&self) -> &str {
        match self {
            Self::Quarantined { proposal_id, .. }
            | Self::SandboxUnavailable { proposal_id, .. }
            | Self::RunnerVerified { proposal_id, .. } => proposal_id,
        }
    }

    /// Only a runner-verified report can make a proposal development-eligible,
    /// and only as persisted by `record_validity_report` (which re-verifies
    /// it) and while no quarantined report of the proposal exists (selection
    /// re-checks that). Quarantined and sandbox-unavailable reports stay
    /// ineligible, exactly as before.
    pub fn is_development_eligible(&self) -> bool {
        matches!(self, Self::RunnerVerified { .. })
    }

    pub fn binding_digests(&self) -> (&str, &str, &str, &str) {
        match self {
            Self::Quarantined {
                proposal_digest,
                target_digest,
                oracle_digest,
                runner_digest,
                ..
            }
            | Self::SandboxUnavailable {
                proposal_digest,
                target_digest,
                oracle_digest,
                runner_digest,
                ..
            }
            | Self::RunnerVerified {
                proposal_digest,
                target_digest,
                oracle_digest,
                runner_digest,
                ..
            } => (proposal_digest, target_digest, oracle_digest, runner_digest),
        }
    }
}

/// Builds the runner-verified report for a proposal whose offline conclusion
/// passed and whose family a verified development observation covers. A
/// quarantined conclusion is refused here already. Nothing is trusted from
/// this constructor alone: `record_validity_report` re-verifies the offline
/// conclusion, the stage-fact pair, the family coverage and the control
/// binding before it persists the report (see [`ValidityReportV1::RunnerVerified`]).
pub fn runner_verified_report(
    profile: &RegisteredPureFunctionProfileV1,
    proposal: &StructuredTestProposalV1,
    offline: &OfflineProfileCheck,
    development_request_fact_id: &str,
    development_observed_fact_id: &str,
) -> Result<ValidityReportV1> {
    profile.validate()?;
    proposal.validate()?;
    identifier(development_request_fact_id)?;
    identifier(development_observed_fact_id)?;
    let OfflineProfileCheck::Passed { .. } = offline else {
        return Err(Error::Conflict(
            "a runner-verified report requires a passed offline conclusion; a quarantined proposal never becomes eligible".into(),
        ));
    };
    Ok(ValidityReportV1::RunnerVerified {
        profile_id: profile.profile_id.clone(),
        proposal_id: proposal.id.clone(),
        proposal_digest: fingerprint(proposal)?,
        target_digest: profile.target_digest.clone(),
        oracle_digest: profile.oracle_digest.clone(),
        runner_digest: profile.runner_digest.clone(),
        development_request_fact_id: development_request_fact_id.into(),
        development_observed_fact_id: development_observed_fact_id.into(),
        offline_output_digest: Some(fingerprint(offline)?),
    })
}

pub fn runtime_validity_report(
    profile: &RegisteredPureFunctionProfileV1,
    proposal: &StructuredTestProposalV1,
    offline: &OfflineProfileCheck,
) -> Result<ValidityReportV1> {
    profile.validate()?;
    proposal.validate()?;
    let proposal_digest = fingerprint(proposal)?;
    Ok(match offline {
        OfflineProfileCheck::Quarantined { reason } => ValidityReportV1::Quarantined {
            profile_id: profile.profile_id.clone(),
            proposal_id: proposal.id.clone(),
            proposal_digest,
            target_digest: profile.target_digest.clone(),
            oracle_digest: profile.oracle_digest.clone(),
            runner_digest: profile.runner_digest.clone(),
            reason: reason.clone(),
        },
        OfflineProfileCheck::Passed { .. } => ValidityReportV1::SandboxUnavailable {
            profile_id: profile.profile_id.clone(),
            proposal_id: proposal.id.clone(),
            proposal_digest,
            target_digest: profile.target_digest.clone(),
            oracle_digest: profile.oracle_digest.clone(),
            runner_digest: profile.runner_digest.clone(),
        },
    })
}
