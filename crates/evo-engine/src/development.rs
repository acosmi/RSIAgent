//! E03 trusted development execution/grader receipts and the registered
//! pure-function development runner.
//!
//! Every receipt is a typed envelope bound to a persisted root-budget row, an
//! Admin-registered control, and the live trusted-source closure. A report's
//! self-declared provenance never grants anything: consumers reload each
//! receipt by id, recompute digests and scores from stored outputs, and
//! recheck tombstones and the revoke watermark. In-process registered
//! pure-function execution is not a sandbox and is never called "isolated".

use crate::curriculum_profiles::{
    CLAMP_TARGET_ID, ClampInput, PURE_FUNCTION_PROFILE_ID, RegisteredPureFunctionProfileV1,
    clamp_oracle,
};
use crate::evidence::{load_stored_source, validate_stored_sources};
use crate::optimization::{
    DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest, DevelopmentRunReport,
    DevelopmentRunRequest, DevelopmentTask, PairedTaskResult, StageDependency, StageFact,
    validate_development_request,
};
use crate::streaming_evaluator::{FixedGraderSpec, grade_fixed_output};
use async_trait::async_trait;
use evo_core::evidence::Purpose;
use evo_core::{Context, Error, Result, Role, fingerprint, hash, identifier};
pub use evo_storage::budget::REGISTERED_EXECUTION_REQUEST_SCHEMA;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallRecord, BudgetCallReservation, BudgetCallState,
    BudgetExecutionProvenance, BudgetStage, REGISTERED_EXECUTION_SETTLEMENT_SCHEMA,
    RegisteredExecutionProvenance, RegisteredExecutionSettlement, RootBudgetRecord, UsageCharge,
};
use evo_storage::{Session, Store};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::BTreeSet;
use std::sync::Arc;

// The registered target is the exact source the curriculum profile digests.
#[path = "../../../examples/reference-host/src/curriculum_target.rs"]
mod curriculum_target;

pub const DEVELOPMENT_CONTROL_SCHEMA: &str = "rsia.development_control.v1";
pub const DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA: &str = "rsia.development_execution_receipt.v1";
pub const DEVELOPMENT_GRADER_RECEIPT_SCHEMA: &str = "rsia.development_grader_receipt.v1";
pub const DEVELOPMENT_EXECUTION_OUTPUT_SCHEMA: &str = "rsia.development_execution_output.v1";
pub const DEVELOPMENT_RUN_RECEIPT_SCHEMA: &str = "rsia.development_run_receipt.v1";
pub const REGISTERED_RUNNER_CONTRACT_SCHEMA: &str = "rsia.registered_development_runner.v1";

pub const CONTROL_KIND: &str = "development_control_v1";
pub const EXECUTION_RECEIPT_KIND: &str = "development_execution_receipt_v1";
pub const GRADER_RECEIPT_KIND: &str = "development_grader_receipt_v1";
pub const EXECUTION_OUTPUT_KIND: &str = "development_execution_output_v1";
pub const RUN_RECEIPT_KIND: &str = "development_run_receipt_v1";
const ENVELOPE_SCHEMA: &str = "rsia.typed_artifact_envelope.v1";
const MAX_OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TypedArtifactEnvelope<T> {
    schema_version: String,
    id: String,
    record_kind: String,
    payload: T,
}

fn digest(value: &str, name: &str) -> Result<()> {
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

fn valid_time(value: i64) -> Result<()> {
    if value < 0 {
        return Err(Error::Invalid("timestamp must be nonnegative".into()));
    }
    Ok(())
}

fn canonical_ids(values: &[String], name: &str) -> Result<()> {
    if values.is_empty() {
        return Err(Error::Invalid(format!("{name} must not be empty")));
    }
    for value in values {
        identifier(value)?;
    }
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(Error::Invalid(format!("{name} must be sorted and unique")));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevelopmentSide {
    Parent,
    Candidate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevelopmentEvidenceScope {
    /// Fixture-only control. Receipts issued under it can never verify a cycle.
    ProgramFixture,
    /// Registered pure-function adapters executed in-process by the fixed
    /// trusted runner. No sandbox, no provider dispatch, no model.
    RegisteredPureFunctionExecution,
}

/// Structured, typed input of a registered pure-function target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "snake_case", deny_unknown_fields)]
pub enum RegisteredTargetInputV1 {
    ClampI64 { value: i64, min: i64, max: i64 },
}

impl RegisteredTargetInputV1 {
    fn target_id(&self) -> &'static str {
        match self {
            Self::ClampI64 { .. } => CLAMP_TARGET_ID,
        }
    }

    /// Independent oracle answer (never the target's own output).
    fn oracle_answer_json(&self) -> Result<String> {
        match *self {
            Self::ClampI64 { value, min, max } => {
                let output = clamp_oracle(ClampInput { value, min, max })?;
                serde_json::to_string(&output).map_err(|_| Error::Internal)
            }
        }
    }

    /// Executes the registered target source and returns the grader-shaped
    /// output `{"answer": ...}`.
    fn execute_target(&self) -> Result<String> {
        match *self {
            Self::ClampI64 { value, min, max } => {
                let answer = match curriculum_target::clamp_i64(curriculum_target::ClampInput {
                    value,
                    min,
                    max,
                }) {
                    Ok(clamped) => serde_json::json!({ "clamped": clamped }),
                    Err(reason) => serde_json::json!({ "error": reason }),
                };
                serde_json::to_string(&serde_json::json!({ "answer": answer }))
                    .map_err(|_| Error::Internal)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentTaskSpecV1 {
    pub task_id: String,
    pub parent_family: String,
    pub profile_id: String,
    pub target_id: String,
    pub input: RegisteredTargetInputV1,
    pub input_digest: String,
    pub expected_answer_json: String,
}

impl DevelopmentTaskSpecV1 {
    /// Builds a task whose expected answer is frozen from the independent
    /// registered oracle before any episode runs.
    pub fn registered(
        task_id: impl Into<String>,
        parent_family: impl Into<String>,
        input: RegisteredTargetInputV1,
    ) -> Result<Self> {
        let spec = Self {
            task_id: task_id.into(),
            parent_family: parent_family.into(),
            profile_id: PURE_FUNCTION_PROFILE_ID.into(),
            target_id: input.target_id().into(),
            input,
            input_digest: fingerprint(&input)?,
            expected_answer_json: input.oracle_answer_json()?,
        };
        spec.validate()?;
        Ok(spec)
    }

    pub fn validate(&self) -> Result<()> {
        identifier(&self.task_id)?;
        identifier(&self.parent_family)?;
        if self.profile_id != PURE_FUNCTION_PROFILE_ID
            || self.target_id != self.input.target_id()
            || self.target_id != curriculum_target::TARGET_ID
        {
            return Err(Error::Invalid(
                "development task targets an unregistered pure function".into(),
            ));
        }
        digest(&self.input_digest, "task input_digest")?;
        if self.input_digest != fingerprint(&self.input)? {
            return Err(Error::Conflict(
                "development task input digest differs from its typed input".into(),
            ));
        }
        if self.expected_answer_json != self.input.oracle_answer_json()? {
            return Err(Error::Conflict(
                "development task expected answer differs from the registered oracle".into(),
            ));
        }
        Ok(())
    }

    fn manifest_task(&self) -> DevelopmentTask {
        DevelopmentTask {
            id: self.task_id.clone(),
            parent_family: self.parent_family.clone(),
            input_digest: self.input_digest.clone(),
        }
    }
}

fn registered_runner_contract_body() -> serde_json::Value {
    serde_json::json!({
        "schema_version": REGISTERED_RUNNER_CONTRACT_SCHEMA,
        "runtime": "in_process_registered_pure_function",
        "isolation": "none",
        "provider_dispatch": false,
        "model_execution": false,
        "targets": [CLAMP_TARGET_ID],
    })
}

/// Digest of the fixed in-process runner contract. It names no sandbox.
pub fn registered_runner_digest() -> Result<String> {
    fingerprint(&registered_runner_contract_body())
}

/// Admin-registered control plane for one development episode family.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentControlV1 {
    pub schema_version: String,
    pub id: String,
    pub namespace: String,
    pub billing_scope: String,
    pub root_budget_id: String,
    pub executor_actor: String,
    pub grader_actor: String,
    pub proposer_actor: String,
    pub manifest_id: String,
    pub manifest_digest: String,
    pub tasks: Vec<DevelopmentTaskSpecV1>,
    pub environment_digest: String,
    pub fixed_grader: FixedGraderSpec,
    pub grader_digest: String,
    pub oracle_digest: String,
    pub target_digest: String,
    pub runner_digest: String,
    pub rules_digest: String,
    pub tools_digest: String,
    pub source_ids: Vec<String>,
    pub evidence_scope: DevelopmentEvidenceScope,
    pub created_at_unix_seconds: i64,
}

impl DevelopmentControlV1 {
    pub const SCHEMA: &'static str = DEVELOPMENT_CONTROL_SCHEMA;

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != Self::SCHEMA {
            return Err(Error::Invalid(
                "unsupported development control schema".into(),
            ));
        }
        for value in [
            &self.id,
            &self.namespace,
            &self.billing_scope,
            &self.root_budget_id,
            &self.executor_actor,
            &self.grader_actor,
            &self.proposer_actor,
            &self.manifest_id,
        ] {
            identifier(value)?;
        }
        for (value, name) in [
            (&self.manifest_digest, "manifest_digest"),
            (&self.environment_digest, "environment_digest"),
            (&self.grader_digest, "grader_digest"),
            (&self.oracle_digest, "oracle_digest"),
            (&self.target_digest, "target_digest"),
            (&self.runner_digest, "runner_digest"),
            (&self.rules_digest, "rules_digest"),
            (&self.tools_digest, "tools_digest"),
        ] {
            digest(value, name)?;
        }
        let actors: BTreeSet<&str> = [
            self.executor_actor.as_str(),
            self.grader_actor.as_str(),
            self.proposer_actor.as_str(),
        ]
        .into_iter()
        .collect();
        if actors.len() != 3 {
            return Err(Error::Invalid(
                "executor, grader, and proposer must be distinct actors".into(),
            ));
        }
        self.fixed_grader.validate()?;
        if self.grader_digest != fingerprint(&self.fixed_grader)? {
            return Err(Error::Invalid(
                "grader_digest does not identify the frozen grader".into(),
            ));
        }
        let profile = RegisteredPureFunctionProfileV1::clamp_i64();
        profile.validate()?;
        if self.oracle_digest != profile.oracle_digest
            || self.target_digest != profile.target_digest
            || self.runner_digest != registered_runner_digest()?
        {
            return Err(Error::Invalid(
                "control does not bind the registered oracle, target, and runner".into(),
            ));
        }
        if self.tasks.is_empty() {
            return Err(Error::Invalid("development control has no tasks".into()));
        }
        for task in &self.tasks {
            task.validate()?;
        }
        if self.manifest()?.digest != self.manifest_digest {
            return Err(Error::Conflict(
                "control manifest digest differs from its frozen tasks".into(),
            ));
        }
        canonical_ids(&self.source_ids, "control source_ids")?;
        valid_time(self.created_at_unix_seconds)?;
        Ok(())
    }

    pub fn manifest(&self) -> Result<DevelopmentManifest> {
        DevelopmentManifest::build(
            self.manifest_id.clone(),
            self.tasks
                .iter()
                .map(DevelopmentTaskSpecV1::manifest_task)
                .collect(),
        )
    }

    pub fn task(&self, task_id: &str) -> Result<&DevelopmentTaskSpecV1> {
        self.tasks
            .iter()
            .find(|task| task.task_id == task_id)
            .ok_or_else(|| Error::Conflict("development task is not registered".into()))
    }

    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        fingerprint(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum DevelopmentCostState {
    Known {
        micros: i64,
        currency: String,
        pricing_version: String,
    },
    Uncertain,
}

/// One executed task side. Issued only by the control's executor actor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentExecutionReceiptV1 {
    pub schema_version: String,
    pub receipt_id: String,
    pub control_id: String,
    pub namespace: String,
    pub episode_id: String,
    pub step: u32,
    pub attempt: u32,
    pub request_id: String,
    pub task_id: String,
    pub side: DevelopmentSide,
    pub input_digest: String,
    pub bundle_digest: String,
    pub environment_digest: String,
    pub request_digest: String,
    pub budget_call_id: String,
    pub dispatch_id: String,
    pub usage_record_id: String,
    pub output_digest: String,
    pub output_artifact_id: String,
    pub cost_state: DevelopmentCostState,
    pub execution_provenance: DevelopmentExecutionProvenance,
    pub revoke_watermark: u64,
    pub source_ids: Vec<String>,
    pub executor_actor: String,
    pub executor_role: Role,
    pub issued_at_unix_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentExecutionOutputV1 {
    pub schema_version: String,
    pub id: String,
    pub control_id: String,
    pub request_id: String,
    pub task_id: String,
    pub side: DevelopmentSide,
    pub output_utf8: String,
    pub output_digest: String,
}

/// Server-scored pair for one task. Issued only by the control's grader actor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentGraderReceiptV1 {
    pub schema_version: String,
    pub receipt_id: String,
    pub control_id: String,
    pub namespace: String,
    pub episode_id: String,
    pub step: u32,
    pub attempt: u32,
    pub request_id: String,
    pub task_id: String,
    pub parent_execution_receipt_id: String,
    pub candidate_execution_receipt_id: String,
    pub parent_output_digest: String,
    pub candidate_output_digest: String,
    pub oracle_digest: String,
    pub grader_digest: String,
    pub grader_version: String,
    pub parent_score_micros: u32,
    pub candidate_score_micros: u32,
    pub parent_passed: bool,
    pub candidate_passed: bool,
    pub scored_output_pair_digest: String,
    pub scoring_budget_call_id: Option<String>,
    pub revoke_watermark: u64,
    pub source_ids: Vec<String>,
    pub grader_actor: String,
    pub issued_at_unix_seconds: i64,
}

/// Run-level closure over the receipts and budget rows of one request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevelopmentRunReceiptV1 {
    pub schema_version: String,
    pub id: String,
    pub control_id: String,
    pub namespace: String,
    pub request_id: String,
    pub episode_id: String,
    pub step: u32,
    pub attempt: u32,
    pub execution_receipt_ids: Vec<String>,
    pub grader_receipt_ids: Vec<String>,
    pub budget_call_ids: Vec<String>,
    pub usage_record_ids: Vec<String>,
    pub executor_actor: String,
    pub issued_at_unix_seconds: i64,
}

/// Persisted request artifact of one budget row (server-recomputed digest).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredExecutionRequestV1 {
    pub schema_version: String,
    pub control_id: String,
    pub request_id: String,
    pub episode_id: String,
    pub task_id: String,
    pub side: DevelopmentSide,
    pub input: RegisteredTargetInputV1,
    pub input_digest: String,
    pub bundle_digest: String,
    pub environment_digest: String,
    pub request_digest: String,
    pub target_id: String,
    pub target_digest: String,
    pub runner_digest: String,
    /// Trusted-source closure of the control; source revocation scans it.
    pub source_ids: Vec<String>,
}

pub fn storage_id(record_kind: &str, id: &str) -> Result<String> {
    identifier(record_kind)?;
    identifier(id)?;
    Ok(format!("e03dev-{}", fingerprint(&(record_kind, id))?))
}

pub fn execution_receipt_id(
    request_id: &str,
    task_id: &str,
    side: DevelopmentSide,
) -> Result<String> {
    Ok(format!(
        "devexec-{}",
        fingerprint(&(request_id, task_id, side))?
    ))
}

pub fn execution_budget_call_id(
    request_id: &str,
    task_id: &str,
    side: DevelopmentSide,
) -> Result<String> {
    Ok(format!(
        "devcall-{}",
        fingerprint(&(request_id, task_id, side))?
    ))
}

fn execution_usage_record_id(
    request_id: &str,
    task_id: &str,
    side: DevelopmentSide,
) -> Result<String> {
    Ok(format!(
        "devusage-{}",
        fingerprint(&(request_id, task_id, side))?
    ))
}

pub fn execution_output_id(
    request_id: &str,
    task_id: &str,
    side: DevelopmentSide,
) -> Result<String> {
    Ok(format!(
        "devout-{}",
        fingerprint(&(request_id, task_id, side))?
    ))
}

pub fn grader_receipt_id(request_id: &str, task_id: &str) -> Result<String> {
    Ok(format!("devgrade-{}", fingerprint(&(request_id, task_id))?))
}

pub fn run_receipt_id(request_id: &str) -> Result<String> {
    Ok(format!("devrun-{}", fingerprint(&request_id)?))
}

fn side_bundle_digest(request: &DevelopmentRunRequest, side: DevelopmentSide) -> &str {
    match side {
        DevelopmentSide::Parent => &request.parent_bundle_digest,
        DevelopmentSide::Candidate => &request.candidate_bundle_digest,
    }
}

/// Server-side identity of one execution. Every stage recomputes it.
pub fn execution_request_digest(
    control: &DevelopmentControlV1,
    request: &DevelopmentRunRequest,
    task: &DevelopmentTaskSpecV1,
    side: DevelopmentSide,
) -> Result<String> {
    fingerprint(&(
        REGISTERED_EXECUTION_REQUEST_SCHEMA,
        &control.id,
        &request.request_id,
        &request.episode_id,
        request.step,
        request.attempt,
        &task.task_id,
        &task.input_digest,
        side,
        side_bundle_digest(request, side),
        &request.environment_digest,
        &request.manifest.digest,
        &request.grader_digest,
        &request.rules_digest,
        &request.tools_digest,
        request.revoke_watermark,
    ))
}

/// Typed closure edges a DevelopmentObserved fact must declare for a
/// non-fixture report so revocation traverses fact -> receipts.
pub fn typed_receipt_closure(report: &DevelopmentRunReport) -> Result<Vec<StageDependency>> {
    let mut dependencies = Vec::new();
    for result in &report.results {
        for receipt_id in [&result.parent_execution_id, &result.candidate_execution_id] {
            dependencies.push(StageDependency {
                kind: "artifact".into(),
                id: storage_id(EXECUTION_RECEIPT_KIND, receipt_id)?,
            });
        }
        dependencies.push(StageDependency {
            kind: "artifact".into(),
            id: storage_id(
                GRADER_RECEIPT_KIND,
                &grader_receipt_id(&report.request_id, &result.task_id)?,
            )?,
        });
    }
    dependencies.push(StageDependency {
        kind: "artifact".into(),
        id: storage_id(RUN_RECEIPT_KIND, &report.execution_receipt_id)?,
    });
    Ok(dependencies)
}

fn validate_control_request_binding(
    control: &DevelopmentControlV1,
    request: &DevelopmentRunRequest,
) -> Result<()> {
    if request.purpose != Purpose::Development {
        return Err(Error::Forbidden);
    }
    if request.namespace != control.namespace
        || request.manifest.digest != control.manifest_digest
        || request.environment_digest != control.environment_digest
        || request.grader_digest != control.grader_digest
        || request.rules_digest != control.rules_digest
        || request.tools_digest != control.tools_digest
    {
        return Err(Error::Conflict(
            "development request differs from the registered control".into(),
        ));
    }
    Ok(())
}

fn registered_task<'a>(
    control: &'a DevelopmentControlV1,
    request: &DevelopmentRunRequest,
    task_id: &str,
) -> Result<&'a DevelopmentTaskSpecV1> {
    let spec = control.task(task_id)?;
    let task = request
        .manifest
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .ok_or_else(|| Error::Conflict("task is outside the frozen manifest".into()))?;
    if task.input_digest != spec.input_digest || task.parent_family != spec.parent_family {
        return Err(Error::Conflict(
            "manifest task differs from the registered task".into(),
        ));
    }
    Ok(spec)
}

/// Tombstones fail closed as Forbidden before the watermark comparison, so a
/// revoked source is reported as a permission failure rather than drift.
async fn require_live_sources(
    session: &mut Session,
    ctx: &Context,
    source_ids: &[String],
    watermark: u64,
) -> Result<()> {
    for id in source_ids {
        if session
            .get::<serde_json::Value>(ctx, "tombstone", id)
            .await?
            .is_some()
        {
            return Err(Error::Forbidden);
        }
    }
    validate_stored_sources(session, ctx, source_ids, watermark).await
}

fn cost_state_from_call(call: &BudgetCallRecord) -> Result<DevelopmentCostState> {
    match call.state {
        BudgetCallState::Finalized => Ok(DevelopmentCostState::Known {
            micros: call
                .actual_cost_micros
                .ok_or_else(|| Error::Conflict("finalized call lacks its actual cost".into()))?,
            currency: call
                .actual_currency
                .clone()
                .ok_or_else(|| Error::Conflict("finalized call lacks its currency".into()))?,
            pricing_version: call.actual_pricing_version.clone().ok_or_else(|| {
                Error::Conflict("finalized call lacks its pricing version".into())
            })?,
        }),
        BudgetCallState::Uncertain => Ok(DevelopmentCostState::Uncertain),
        _ => Err(Error::Conflict(
            "execution budget call is neither finalized nor uncertain".into(),
        )),
    }
}

/// Reloads the row's positive execution evidence. A row without the typed
/// registered settlement, a fixture row, or an open/unsettled row never binds.
fn validate_execution_call(
    ctx: &Context,
    call: &BudgetCallRecord,
    control: &DevelopmentControlV1,
    request: &DevelopmentRunRequest,
    task: &DevelopmentTaskSpecV1,
    request_digest: &str,
    output_digest: &str,
) -> Result<RegisteredExecutionSettlement> {
    if call.namespace != ctx.namespace()
        || call.billing_scope != control.billing_scope
        || call.dispatch_group_id != request.episode_id
        || call.stage != BudgetStage::DevelopmentExecution
        || call.actual_input_digest != request_digest
        || !matches!(
            call.state,
            BudgetCallState::Finalized | BudgetCallState::Uncertain
        )
        || !call.execution_closed
        || call.dispatch_id.is_none()
        || call.output_digest.as_deref() != Some(output_digest)
        || call.execution_provenance == Some(BudgetExecutionProvenance::Fixture)
        || call.response_usable != Some(true)
    {
        return Err(Error::Conflict(
            "execution receipt lacks a matching closed development budget call".into(),
        ));
    }
    let settlement = RegisteredExecutionSettlement::from_call(call)?;
    if settlement.provenance != RegisteredExecutionProvenance::RegisteredPureFunction
        || settlement.target_id != task.target_id
        || settlement.target_digest != control.target_digest
        || settlement.runner_digest != control.runner_digest
    {
        return Err(Error::Conflict(
            "budget row settlement does not bind the registered target and runner".into(),
        ));
    }
    let request_artifact = call
        .request_artifact
        .as_ref()
        .ok_or_else(|| Error::Conflict("execution call lacks its request artifact".into()))?;
    if request_artifact.schema_version != REGISTERED_EXECUTION_REQUEST_SCHEMA {
        return Err(Error::Conflict(
            "execution call request artifact schema differs".into(),
        ));
    }
    let stored_request: RegisteredExecutionRequestV1 = serde_json::from_str(&request_artifact.body)
        .map_err(|_| Error::Conflict("execution call request artifact is invalid".into()))?;
    if stored_request.control_id != control.id
        || stored_request.request_id != request.request_id
        || stored_request.task_id != task.task_id
        || stored_request.input != task.input
        || stored_request.request_digest != request_digest
        || stored_request.source_ids != control.source_ids
    {
        return Err(Error::Conflict(
            "execution call request artifact differs from the receipt".into(),
        ));
    }
    Ok(settlement)
}

fn validate_execution_receipt_binding(
    receipt: &DevelopmentExecutionReceiptV1,
    control: &DevelopmentControlV1,
    request: &DevelopmentRunRequest,
    task: &DevelopmentTaskSpecV1,
    side: DevelopmentSide,
) -> Result<()> {
    if receipt.schema_version != DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA {
        return Err(Error::Invalid(
            "unsupported development execution receipt schema".into(),
        ));
    }
    let expected_id = execution_receipt_id(&request.request_id, &task.task_id, side)?;
    if receipt.receipt_id != expected_id
        || receipt.control_id != control.id
        || receipt.namespace != control.namespace
        || receipt.episode_id != request.episode_id
        || receipt.step != request.step
        || receipt.attempt != request.attempt
        || receipt.request_id != request.request_id
        || receipt.task_id != task.task_id
        || receipt.side != side
        || receipt.input_digest != task.input_digest
        || receipt.bundle_digest != side_bundle_digest(request, side)
        || receipt.environment_digest != request.environment_digest
        || receipt.request_digest != execution_request_digest(control, request, task, side)?
        || receipt.output_artifact_id
            != execution_output_id(&request.request_id, &task.task_id, side)?
        || receipt.execution_provenance != DevelopmentExecutionProvenance::RegisteredPureFunction
        || receipt.revoke_watermark != request.revoke_watermark
        || receipt.source_ids != control.source_ids
        || receipt.executor_actor != control.executor_actor
        || !matches!(receipt.executor_role, Role::Worker | Role::Host)
        || receipt.executor_actor == control.grader_actor
        || receipt.executor_actor == control.proposer_actor
    {
        return Err(Error::Conflict(
            "execution receipt does not bind the registered control and request".into(),
        ));
    }
    digest(&receipt.output_digest, "receipt output_digest")?;
    Ok(())
}

fn validate_output_artifact(
    receipt: &DevelopmentExecutionReceiptV1,
    output: &DevelopmentExecutionOutputV1,
) -> Result<()> {
    if output.schema_version != DEVELOPMENT_EXECUTION_OUTPUT_SCHEMA
        || output.id != receipt.output_artifact_id
        || output.control_id != receipt.control_id
        || output.request_id != receipt.request_id
        || output.task_id != receipt.task_id
        || output.side != receipt.side
        || output.output_digest != receipt.output_digest
        || hash(output.output_utf8.as_bytes()) != receipt.output_digest
    {
        return Err(Error::Conflict(
            "execution output artifact does not match its receipt".into(),
        ));
    }
    Ok(())
}

fn score_stored_output(grader: &FixedGraderSpec, output_utf8: &str, expected: &str) -> Result<u32> {
    match grade_fixed_output(grader, output_utf8, expected) {
        Ok(score) => Ok(score),
        // A malformed output is honest evidence of failure, never a crash.
        Err(Error::Invalid(_)) => Ok(0),
        Err(error) => Err(error),
    }
}

fn scored_pair_digest(
    control: &DevelopmentControlV1,
    parent: &DevelopmentExecutionReceiptV1,
    candidate: &DevelopmentExecutionReceiptV1,
    parent_score_micros: u32,
    candidate_score_micros: u32,
) -> Result<String> {
    fingerprint(&(
        &parent.output_digest,
        &candidate.output_digest,
        &control.oracle_digest,
        &control.grader_digest,
        parent_score_micros,
        candidate_score_micros,
        parent_score_micros == 1_000_000,
        candidate_score_micros == 1_000_000,
    ))
}

fn validate_scoring_call(
    ctx: &Context,
    call: &BudgetCallRecord,
    control: &DevelopmentControlV1,
    request: &DevelopmentRunRequest,
) -> Result<()> {
    if call.namespace != ctx.namespace()
        || call.billing_scope != control.billing_scope
        || call.dispatch_group_id != request.episode_id
        || call.stage != BudgetStage::DevelopmentScoring
        || !matches!(
            call.state,
            BudgetCallState::Finalized | BudgetCallState::Uncertain
        )
        || !call.execution_closed
        || call.execution_provenance == Some(BudgetExecutionProvenance::Fixture)
    {
        return Err(Error::Conflict(
            "scoring budget call does not bind this development episode".into(),
        ));
    }
    Ok(())
}

async fn get_record<T: DeserializeOwned>(
    session: &mut Session,
    ctx: &Context,
    record_kind: &str,
    id: &str,
) -> Result<Option<T>> {
    let storage_id = storage_id(record_kind, id)?;
    let envelope = session
        .get::<TypedArtifactEnvelope<T>>(ctx, "artifact", &storage_id)
        .await;
    match envelope {
        Ok(Some(envelope))
            if envelope.schema_version == ENVELOPE_SCHEMA
                && envelope.id == storage_id
                && envelope.record_kind == record_kind =>
        {
            Ok(Some(envelope.payload))
        }
        Ok(Some(_)) | Err(Error::Internal) => Err(Error::Conflict(
            "development artifact envelope mismatch or redacted".into(),
        )),
        Ok(None) => Ok(None),
        Err(error) => Err(error),
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
    payload: &T,
) -> Result<String> {
    let storage_id = storage_id(record_kind, id)?;
    session
        .put(
            ctx,
            "artifact",
            &storage_id,
            ctx.actor(),
            &TypedArtifactEnvelope {
                schema_version: ENVELOPE_SCHEMA.into(),
                id: storage_id.clone(),
                record_kind: record_kind.into(),
                payload,
            },
        )
        .await?;
    Ok(storage_id)
}

async fn need_control(
    session: &mut Session,
    ctx: &Context,
    control_id: &str,
) -> Result<DevelopmentControlV1> {
    identifier(control_id)?;
    let control: DevelopmentControlV1 = need_record(session, ctx, CONTROL_KIND, control_id).await?;
    control.validate()?;
    if control.id != control_id || control.namespace != ctx.namespace() {
        return Err(Error::Forbidden);
    }
    Ok(control)
}

fn budget_call_ref_id(ctx: &Context, billing_scope: &str, call_id: &str) -> Result<String> {
    let digest = fingerprint(&(
        "rsia.budget_call_ref.v1",
        ctx.namespace(),
        billing_scope,
        call_id,
    ))?;
    Ok(format!("budget-ref-{}", &digest[..32]))
}

/// Registers the control plane before any episode. Admin only; the
/// registering actor cannot also be the executor or the grader.
pub async fn register_development_control(
    ctx: &Context,
    store: &Store,
    control: DevelopmentControlV1,
) -> Result<String> {
    ctx.require(&[Role::Admin])?;
    control.validate()?;
    if control.namespace != ctx.namespace()
        || ctx.actor() == control.executor_actor
        || ctx.actor() == control.grader_actor
    {
        return Err(Error::Forbidden);
    }
    let root = store
        .root_budget(ctx, &control.billing_scope)
        .await?
        .ok_or(Error::Budget)?;
    if root.root_budget_id != control.root_budget_id {
        return Err(Error::Conflict(
            "control root budget differs from the persisted billing scope".into(),
        ));
    }
    let digest = control.digest()?;
    let mut session = store.session().await?;
    for source in &control.source_ids {
        if session
            .get::<serde_json::Value>(ctx, "tombstone", source)
            .await?
            .is_some()
        {
            return Err(Error::Forbidden);
        }
        load_stored_source(&mut session, ctx, source).await?;
    }
    if let Some(existing) =
        get_record::<DevelopmentControlV1>(&mut session, ctx, CONTROL_KIND, &control.id).await?
    {
        if fingerprint(&existing)? != digest {
            return Err(Error::Conflict(
                "development control already registered with different content".into(),
            ));
        }
        session.commit().await?;
        return Ok(digest);
    }
    let control_storage_id =
        put_record(&mut session, ctx, CONTROL_KIND, &control.id, &control).await?;
    for source in &control.source_ids {
        session
            .put_edge(ctx, "artifact", &control_storage_id, "run", source)
            .await?;
    }
    session
        .audit(ctx, "development.control.register", &control.id)
        .await?;
    session.commit().await?;
    Ok(digest)
}

pub async fn load_development_control(
    store: &Store,
    ctx: &Context,
    control_id: &str,
) -> Result<DevelopmentControlV1> {
    let mut session = store.session().await?;
    let control = need_control(&mut session, ctx, control_id).await?;
    session.commit().await?;
    Ok(control)
}

async fn load_typed<T: DeserializeOwned>(
    store: &Store,
    ctx: &Context,
    record_kind: &str,
    id: &str,
) -> Result<T> {
    identifier(id)?;
    let mut session = store.session().await?;
    let value = need_record(&mut session, ctx, record_kind, id).await?;
    session.commit().await?;
    Ok(value)
}

pub async fn load_execution_receipt(
    store: &Store,
    ctx: &Context,
    receipt_id: &str,
) -> Result<DevelopmentExecutionReceiptV1> {
    let receipt: DevelopmentExecutionReceiptV1 =
        load_typed(store, ctx, EXECUTION_RECEIPT_KIND, receipt_id).await?;
    if receipt.schema_version != DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA
        || receipt.receipt_id != receipt_id
        || receipt.namespace != ctx.namespace()
    {
        return Err(Error::Conflict(
            "development execution receipt identity mismatch".into(),
        ));
    }
    Ok(receipt)
}

pub async fn load_grader_receipt(
    store: &Store,
    ctx: &Context,
    receipt_id: &str,
) -> Result<DevelopmentGraderReceiptV1> {
    let receipt: DevelopmentGraderReceiptV1 =
        load_typed(store, ctx, GRADER_RECEIPT_KIND, receipt_id).await?;
    if receipt.schema_version != DEVELOPMENT_GRADER_RECEIPT_SCHEMA
        || receipt.receipt_id != receipt_id
        || receipt.namespace != ctx.namespace()
    {
        return Err(Error::Conflict(
            "development grader receipt identity mismatch".into(),
        ));
    }
    Ok(receipt)
}

pub async fn load_execution_output(
    store: &Store,
    ctx: &Context,
    output_id: &str,
) -> Result<DevelopmentExecutionOutputV1> {
    let output: DevelopmentExecutionOutputV1 =
        load_typed(store, ctx, EXECUTION_OUTPUT_KIND, output_id).await?;
    if output.schema_version != DEVELOPMENT_EXECUTION_OUTPUT_SCHEMA || output.id != output_id {
        return Err(Error::Conflict(
            "development execution output identity mismatch".into(),
        ));
    }
    Ok(output)
}

pub async fn load_run_receipt(
    store: &Store,
    ctx: &Context,
    run_receipt_id: &str,
) -> Result<DevelopmentRunReceiptV1> {
    let receipt: DevelopmentRunReceiptV1 =
        load_typed(store, ctx, RUN_RECEIPT_KIND, run_receipt_id).await?;
    if receipt.schema_version != DEVELOPMENT_RUN_RECEIPT_SCHEMA
        || receipt.id != run_receipt_id
        || receipt.namespace != ctx.namespace()
    {
        return Err(Error::Conflict(
            "development run receipt identity mismatch".into(),
        ));
    }
    Ok(receipt)
}

#[derive(Debug, Clone)]
pub struct ExecutionReceiptIssueRequest {
    pub control_id: String,
    pub request: DevelopmentRunRequest,
    pub task_id: String,
    pub side: DevelopmentSide,
    pub output_utf8: String,
    pub budget_call_id: String,
    pub issued_at_unix_seconds: i64,
}

/// Issues one execution receipt. Only the control's executor actor with the
/// Worker or Host role may issue it; Admin is rejected. The budget row, its
/// typed registered settlement, and the live source closure are reloaded here;
/// the cost state is derived from the row, never supplied by the caller.
pub async fn issue_execution_receipt(
    ctx: &Context,
    store: &Store,
    issue: ExecutionReceiptIssueRequest,
) -> Result<DevelopmentExecutionReceiptV1> {
    ctx.require(&[Role::Worker, Role::Host])?;
    validate_development_request(&issue.request)?;
    identifier(&issue.task_id)?;
    identifier(&issue.budget_call_id)?;
    valid_time(issue.issued_at_unix_seconds)?;
    if issue.output_utf8.is_empty() || issue.output_utf8.len() > MAX_OUTPUT_BYTES {
        return Err(Error::Invalid(
            "execution output must be 1..=262144 UTF-8 bytes".into(),
        ));
    }
    let request = &issue.request;
    let mut session = store.session().await?;
    let control = need_control(&mut session, ctx, &issue.control_id).await?;
    if control.executor_actor != ctx.actor() {
        return Err(Error::Forbidden);
    }
    validate_control_request_binding(&control, request)?;
    let task = registered_task(&control, request, &issue.task_id)?;
    let request_digest = execution_request_digest(&control, request, task, issue.side)?;
    let output_digest = hash(issue.output_utf8.as_bytes());
    let call = session
        .budget_call(ctx, &control.billing_scope, &issue.budget_call_id)
        .await?
        .ok_or(Error::NotFound)?;
    validate_execution_call(
        ctx,
        &call,
        &control,
        request,
        task,
        &request_digest,
        &output_digest,
    )?;
    let cost_state = cost_state_from_call(&call)?;
    require_live_sources(
        &mut session,
        ctx,
        &control.source_ids,
        request.revoke_watermark,
    )
    .await?;
    let output = DevelopmentExecutionOutputV1 {
        schema_version: DEVELOPMENT_EXECUTION_OUTPUT_SCHEMA.into(),
        id: execution_output_id(&request.request_id, &task.task_id, issue.side)?,
        control_id: control.id.clone(),
        request_id: request.request_id.clone(),
        task_id: task.task_id.clone(),
        side: issue.side,
        output_utf8: issue.output_utf8,
        output_digest: output_digest.clone(),
    };
    let receipt = DevelopmentExecutionReceiptV1 {
        schema_version: DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA.into(),
        receipt_id: execution_receipt_id(&request.request_id, &task.task_id, issue.side)?,
        control_id: control.id.clone(),
        namespace: control.namespace.clone(),
        episode_id: request.episode_id.clone(),
        step: request.step,
        attempt: request.attempt,
        request_id: request.request_id.clone(),
        task_id: task.task_id.clone(),
        side: issue.side,
        input_digest: task.input_digest.clone(),
        bundle_digest: side_bundle_digest(request, issue.side).into(),
        environment_digest: request.environment_digest.clone(),
        request_digest,
        budget_call_id: call.call_id.clone(),
        dispatch_id: call.dispatch_id.clone().ok_or(Error::Internal)?,
        usage_record_id: call
            .usage_record_id
            .clone()
            .ok_or_else(|| Error::Conflict("settled call lacks its usage record".into()))?,
        output_digest,
        output_artifact_id: output.id.clone(),
        cost_state,
        execution_provenance: DevelopmentExecutionProvenance::RegisteredPureFunction,
        revoke_watermark: request.revoke_watermark,
        source_ids: control.source_ids.clone(),
        executor_actor: ctx.actor().into(),
        executor_role: ctx.role(),
        issued_at_unix_seconds: issue.issued_at_unix_seconds,
    };
    validate_execution_receipt_binding(&receipt, &control, request, task, issue.side)?;
    if let Some(existing) = get_record::<DevelopmentExecutionReceiptV1>(
        &mut session,
        ctx,
        EXECUTION_RECEIPT_KIND,
        &receipt.receipt_id,
    )
    .await?
    {
        if fingerprint(&existing)? != fingerprint(&receipt)? {
            return Err(Error::Conflict(
                "execution receipt id reused with different evidence".into(),
            ));
        }
        let existing_output: DevelopmentExecutionOutputV1 = need_record(
            &mut session,
            ctx,
            EXECUTION_OUTPUT_KIND,
            &existing.output_artifact_id,
        )
        .await?;
        if fingerprint(&existing_output)? != fingerprint(&output)? {
            return Err(Error::Conflict(
                "stored execution output differs from idempotent receipt".into(),
            ));
        }
        session.commit().await?;
        return Ok(existing);
    }
    if let Some(existing_output) = get_record::<DevelopmentExecutionOutputV1>(
        &mut session,
        ctx,
        EXECUTION_OUTPUT_KIND,
        &output.id,
    )
    .await?
        && fingerprint(&existing_output)? != fingerprint(&output)?
    {
        return Err(Error::Conflict(
            "execution output id reused with different bytes".into(),
        ));
    }
    let output_storage_id = put_record(
        &mut session,
        ctx,
        EXECUTION_OUTPUT_KIND,
        &output.id,
        &output,
    )
    .await?;
    let receipt_storage_id = put_record(
        &mut session,
        ctx,
        EXECUTION_RECEIPT_KIND,
        &receipt.receipt_id,
        &receipt,
    )
    .await?;
    for source in &control.source_ids {
        // The output derives from a source-derived bundle; both the receipt and
        // the output bytes must be reachable from a revoked source.
        for node in [&receipt_storage_id, &output_storage_id] {
            session
                .put_edge(ctx, "artifact", node, "run", source)
                .await?;
        }
    }
    for target in [
        output_storage_id,
        storage_id(CONTROL_KIND, &control.id)?,
        budget_call_ref_id(ctx, &control.billing_scope, &call.call_id)?,
    ] {
        session
            .put_edge(ctx, "artifact", &receipt_storage_id, "artifact", &target)
            .await?;
    }
    session
        .audit(
            ctx,
            "development.execution_receipt.issue",
            &receipt.receipt_id,
        )
        .await?;
    session.commit().await?;
    Ok(receipt)
}

#[derive(Debug, Clone)]
pub struct GraderReceiptIssueRequest {
    pub control_id: String,
    pub request: DevelopmentRunRequest,
    pub task_id: String,
    pub parent_execution_receipt_id: String,
    pub candidate_execution_receipt_id: String,
    pub scoring_budget_call_id: Option<String>,
    pub issued_at_unix_seconds: i64,
}

struct VerifiedPair {
    parent: DevelopmentExecutionReceiptV1,
    candidate: DevelopmentExecutionReceiptV1,
    parent_score_micros: u32,
    candidate_score_micros: u32,
    scored_output_pair_digest: String,
}

/// Reloads both execution receipts, their budget rows and stored outputs, and
/// recomputes the scores with the frozen grader against the registered oracle.
async fn verify_execution_pair(
    ctx: &Context,
    session: &mut Session,
    control: &DevelopmentControlV1,
    request: &DevelopmentRunRequest,
    task: &DevelopmentTaskSpecV1,
    parent_receipt_id: &str,
    candidate_receipt_id: &str,
) -> Result<VerifiedPair> {
    let mut receipts = Vec::with_capacity(2);
    for (receipt_id, side) in [
        (parent_receipt_id, DevelopmentSide::Parent),
        (candidate_receipt_id, DevelopmentSide::Candidate),
    ] {
        identifier(receipt_id)?;
        let receipt: DevelopmentExecutionReceiptV1 =
            need_record(session, ctx, EXECUTION_RECEIPT_KIND, receipt_id).await?;
        if receipt.control_id != control.id {
            return Err(Error::Conflict(
                "execution receipt belongs to a different control".into(),
            ));
        }
        validate_execution_receipt_binding(&receipt, control, request, task, side)?;
        let call = session
            .budget_call(ctx, &control.billing_scope, &receipt.budget_call_id)
            .await?
            .ok_or(Error::NotFound)?;
        validate_execution_call(
            ctx,
            &call,
            control,
            request,
            task,
            &receipt.request_digest,
            &receipt.output_digest,
        )?;
        if call.dispatch_id.as_deref() != Some(receipt.dispatch_id.as_str())
            || call.usage_record_id.as_deref() != Some(receipt.usage_record_id.as_str())
            || cost_state_from_call(&call)? != receipt.cost_state
        {
            return Err(Error::Conflict(
                "execution receipt differs from its persisted budget row".into(),
            ));
        }
        let output: DevelopmentExecutionOutputV1 = need_record(
            session,
            ctx,
            EXECUTION_OUTPUT_KIND,
            &receipt.output_artifact_id,
        )
        .await?;
        validate_output_artifact(&receipt, &output)?;
        let score = score_stored_output(
            &control.fixed_grader,
            &output.output_utf8,
            &task.expected_answer_json,
        )?;
        receipts.push((receipt, score));
    }
    let (candidate, candidate_score_micros) = receipts.pop().ok_or(Error::Internal)?;
    let (parent, parent_score_micros) = receipts.pop().ok_or(Error::Internal)?;
    let scored_output_pair_digest = scored_pair_digest(
        control,
        &parent,
        &candidate,
        parent_score_micros,
        candidate_score_micros,
    )?;
    Ok(VerifiedPair {
        parent,
        candidate,
        parent_score_micros,
        candidate_score_micros,
        scored_output_pair_digest,
    })
}

/// Issues one grader receipt. Only the control's grader actor with the
/// Evaluator role may issue it, and never the executor or the proposer.
/// Scores and pass flags are computed here from stored outputs; the caller
/// supplies none of them.
pub async fn issue_grader_receipt(
    ctx: &Context,
    store: &Store,
    issue: GraderReceiptIssueRequest,
) -> Result<DevelopmentGraderReceiptV1> {
    ctx.require(&[Role::Evaluator])?;
    validate_development_request(&issue.request)?;
    identifier(&issue.task_id)?;
    valid_time(issue.issued_at_unix_seconds)?;
    let request = &issue.request;
    let mut session = store.session().await?;
    let control = need_control(&mut session, ctx, &issue.control_id).await?;
    if control.grader_actor != ctx.actor()
        || ctx.actor() == control.executor_actor
        || ctx.actor() == control.proposer_actor
    {
        return Err(Error::Forbidden);
    }
    validate_control_request_binding(&control, request)?;
    let task = registered_task(&control, request, &issue.task_id)?;
    let pair = verify_execution_pair(
        ctx,
        &mut session,
        &control,
        request,
        task,
        &issue.parent_execution_receipt_id,
        &issue.candidate_execution_receipt_id,
    )
    .await?;
    if let Some(call_id) = &issue.scoring_budget_call_id {
        identifier(call_id)?;
        let call = session
            .budget_call(ctx, &control.billing_scope, call_id)
            .await?
            .ok_or(Error::NotFound)?;
        validate_scoring_call(ctx, &call, &control, request)?;
    }
    require_live_sources(
        &mut session,
        ctx,
        &control.source_ids,
        request.revoke_watermark,
    )
    .await?;
    let receipt = DevelopmentGraderReceiptV1 {
        schema_version: DEVELOPMENT_GRADER_RECEIPT_SCHEMA.into(),
        receipt_id: grader_receipt_id(&request.request_id, &task.task_id)?,
        control_id: control.id.clone(),
        namespace: control.namespace.clone(),
        episode_id: request.episode_id.clone(),
        step: request.step,
        attempt: request.attempt,
        request_id: request.request_id.clone(),
        task_id: task.task_id.clone(),
        parent_execution_receipt_id: pair.parent.receipt_id.clone(),
        candidate_execution_receipt_id: pair.candidate.receipt_id.clone(),
        parent_output_digest: pair.parent.output_digest.clone(),
        candidate_output_digest: pair.candidate.output_digest.clone(),
        oracle_digest: control.oracle_digest.clone(),
        grader_digest: control.grader_digest.clone(),
        grader_version: control.fixed_grader.version.clone(),
        parent_score_micros: pair.parent_score_micros,
        candidate_score_micros: pair.candidate_score_micros,
        parent_passed: pair.parent_score_micros == 1_000_000,
        candidate_passed: pair.candidate_score_micros == 1_000_000,
        scored_output_pair_digest: pair.scored_output_pair_digest,
        scoring_budget_call_id: issue.scoring_budget_call_id,
        revoke_watermark: request.revoke_watermark,
        source_ids: control.source_ids.clone(),
        grader_actor: ctx.actor().into(),
        issued_at_unix_seconds: issue.issued_at_unix_seconds,
    };
    if let Some(existing) = get_record::<DevelopmentGraderReceiptV1>(
        &mut session,
        ctx,
        GRADER_RECEIPT_KIND,
        &receipt.receipt_id,
    )
    .await?
    {
        if fingerprint(&existing)? != fingerprint(&receipt)? {
            return Err(Error::Conflict(
                "grader receipt id reused with different evidence".into(),
            ));
        }
        session.commit().await?;
        return Ok(existing);
    }
    let receipt_storage_id = put_record(
        &mut session,
        ctx,
        GRADER_RECEIPT_KIND,
        &receipt.receipt_id,
        &receipt,
    )
    .await?;
    for source in &control.source_ids {
        session
            .put_edge(ctx, "artifact", &receipt_storage_id, "run", source)
            .await?;
    }
    for target in [
        storage_id(EXECUTION_RECEIPT_KIND, &pair.parent.receipt_id)?,
        storage_id(EXECUTION_RECEIPT_KIND, &pair.candidate.receipt_id)?,
        storage_id(CONTROL_KIND, &control.id)?,
    ] {
        session
            .put_edge(ctx, "artifact", &receipt_storage_id, "artifact", &target)
            .await?;
    }
    session
        .audit(ctx, "development.grader_receipt.issue", &receipt.receipt_id)
        .await?;
    session.commit().await?;
    Ok(receipt)
}

fn validate_grader_receipt_binding(
    receipt: &DevelopmentGraderReceiptV1,
    control: &DevelopmentControlV1,
    request: &DevelopmentRunRequest,
    task: &DevelopmentTaskSpecV1,
    pair: &VerifiedPair,
) -> Result<()> {
    if receipt.schema_version != DEVELOPMENT_GRADER_RECEIPT_SCHEMA {
        return Err(Error::Invalid(
            "unsupported development grader receipt schema".into(),
        ));
    }
    if receipt.receipt_id != grader_receipt_id(&request.request_id, &task.task_id)?
        || receipt.control_id != control.id
        || receipt.namespace != control.namespace
        || receipt.episode_id != request.episode_id
        || receipt.step != request.step
        || receipt.attempt != request.attempt
        || receipt.request_id != request.request_id
        || receipt.task_id != task.task_id
        || receipt.parent_execution_receipt_id != pair.parent.receipt_id
        || receipt.candidate_execution_receipt_id != pair.candidate.receipt_id
        || receipt.parent_output_digest != pair.parent.output_digest
        || receipt.candidate_output_digest != pair.candidate.output_digest
        || receipt.oracle_digest != control.oracle_digest
        || receipt.grader_digest != control.grader_digest
        || receipt.grader_version != control.fixed_grader.version
        || receipt.revoke_watermark != request.revoke_watermark
        || receipt.source_ids != control.source_ids
        || receipt.grader_actor != control.grader_actor
        || receipt.grader_actor == control.executor_actor
        || receipt.grader_actor == control.proposer_actor
    {
        return Err(Error::Conflict(
            "grader receipt does not bind the registered control and executions".into(),
        ));
    }
    if receipt.parent_score_micros != pair.parent_score_micros
        || receipt.candidate_score_micros != pair.candidate_score_micros
        || receipt.parent_passed != (pair.parent_score_micros == 1_000_000)
        || receipt.candidate_passed != (pair.candidate_score_micros == 1_000_000)
        || receipt.scored_output_pair_digest != pair.scored_output_pair_digest
    {
        return Err(Error::Conflict(
            "grader receipt scores differ from the server-computed grader result".into(),
        ));
    }
    Ok(())
}

/// One verified task pair returned to E12/E13 consumers.
#[derive(Debug, Clone)]
pub struct VerifiedDevelopmentTaskOutcome {
    pub task_id: String,
    pub parent_family: String,
    pub parent_score_micros: u32,
    pub candidate_score_micros: u32,
    pub parent_passed: bool,
    pub candidate_passed: bool,
    pub parent_execution_id: String,
    pub candidate_execution_id: String,
    pub grader_receipt_digest: String,
}

/// Verifies a DevelopmentObserved report against its typed receipts inside
/// the caller's transaction. `report.provenance` never grants: the caller has
/// already rejected Fixture, and everything else is reloaded here.
pub(crate) async fn verify_observed_report_in_session(
    ctx: &Context,
    session: &mut Session,
    observed_fact: &StageFact,
    request: &DevelopmentRunRequest,
    report: &DevelopmentRunReport,
) -> Result<Vec<VerifiedDevelopmentTaskOutcome>> {
    if report.provenance == DevelopmentExecutionProvenance::Fixture || report.results.is_empty() {
        return Err(Error::Forbidden);
    }
    let mut control_ids = BTreeSet::new();
    for result in &report.results {
        for receipt_id in [&result.parent_execution_id, &result.candidate_execution_id] {
            let receipt: DevelopmentExecutionReceiptV1 =
                need_record(session, ctx, EXECUTION_RECEIPT_KIND, receipt_id).await?;
            control_ids.insert(receipt.control_id);
        }
    }
    if control_ids.len() != 1 {
        return Err(Error::Conflict(
            "development receipts span different controls".into(),
        ));
    }
    let control_id = control_ids.into_iter().next().ok_or(Error::Internal)?;
    let control = need_control(session, ctx, &control_id).await?;
    if control.evidence_scope != DevelopmentEvidenceScope::RegisteredPureFunctionExecution {
        return Err(Error::Forbidden);
    }
    validate_control_request_binding(&control, request)?;
    let declared: BTreeSet<(&str, &str)> = observed_fact
        .dependencies
        .iter()
        .map(|dependency| (dependency.kind.as_str(), dependency.id.as_str()))
        .collect();
    for dependency in typed_receipt_closure(report)? {
        if !declared.contains(&(dependency.kind.as_str(), dependency.id.as_str())) {
            return Err(Error::Conflict(
                "development observation lacks its typed receipt closure".into(),
            ));
        }
    }
    let mut fact_sources: Vec<String> = observed_fact
        .dependencies
        .iter()
        .filter(|dependency| dependency.kind == "run")
        .map(|dependency| dependency.id.clone())
        .collect();
    fact_sources.sort();
    fact_sources.dedup();
    if !fact_sources.is_empty() && fact_sources != control.source_ids {
        return Err(Error::Conflict(
            "development observation sources differ from the registered control".into(),
        ));
    }
    for watermark in observed_fact
        .dependencies
        .iter()
        .filter(|dependency| dependency.kind == "revoke_watermark")
    {
        if watermark.id.parse::<u64>().ok() != Some(request.revoke_watermark) {
            return Err(Error::Conflict(
                "development observation watermark differs from its request".into(),
            ));
        }
    }
    let mut outcomes = Vec::with_capacity(report.results.len());
    let mut execution_ids = BTreeSet::new();
    let mut grader_ids = BTreeSet::new();
    let mut call_ids = BTreeSet::new();
    let mut usage_ids = BTreeSet::new();
    for result in &report.results {
        let task = registered_task(&control, request, &result.task_id)?;
        let pair = verify_execution_pair(
            ctx,
            session,
            &control,
            request,
            task,
            &result.parent_execution_id,
            &result.candidate_execution_id,
        )
        .await?;
        let grader: DevelopmentGraderReceiptV1 = need_record(
            session,
            ctx,
            GRADER_RECEIPT_KIND,
            &grader_receipt_id(&request.request_id, &task.task_id)?,
        )
        .await?;
        if fingerprint(&grader)? != result.grader_receipt_digest {
            return Err(Error::Conflict(
                "grader receipt digest differs from the observed report".into(),
            ));
        }
        validate_grader_receipt_binding(&grader, &control, request, task, &pair)?;
        if let Some(call_id) = &grader.scoring_budget_call_id {
            let call = session
                .budget_call(ctx, &control.billing_scope, call_id)
                .await?
                .ok_or(Error::NotFound)?;
            validate_scoring_call(ctx, &call, &control, request)?;
            call_ids.insert(call_id.clone());
        }
        if result.parent_score_micros != pair.parent_score_micros
            || result.candidate_score_micros != pair.candidate_score_micros
            || result.parent_passed != grader.parent_passed
            || result.candidate_passed != grader.candidate_passed
        {
            return Err(Error::Conflict(
                "development report scores differ from the server-computed grader result".into(),
            ));
        }
        for receipt in [&pair.parent, &pair.candidate] {
            execution_ids.insert(receipt.receipt_id.clone());
            call_ids.insert(receipt.budget_call_id.clone());
            usage_ids.insert(receipt.usage_record_id.clone());
        }
        grader_ids.insert(grader.receipt_id.clone());
        outcomes.push(VerifiedDevelopmentTaskOutcome {
            task_id: task.task_id.clone(),
            parent_family: task.parent_family.clone(),
            parent_score_micros: pair.parent_score_micros,
            candidate_score_micros: pair.candidate_score_micros,
            parent_passed: grader.parent_passed,
            candidate_passed: grader.candidate_passed,
            parent_execution_id: pair.parent.receipt_id.clone(),
            candidate_execution_id: pair.candidate.receipt_id.clone(),
            grader_receipt_digest: result.grader_receipt_digest.clone(),
        });
    }
    let reported_usage: BTreeSet<String> = report.usage_record_ids.iter().cloned().collect();
    if reported_usage != usage_ids {
        return Err(Error::Conflict(
            "development report usage records differ from its budget rows".into(),
        ));
    }
    let run: DevelopmentRunReceiptV1 =
        need_record(session, ctx, RUN_RECEIPT_KIND, &report.execution_receipt_id).await?;
    if run.schema_version != DEVELOPMENT_RUN_RECEIPT_SCHEMA
        || run.id != run_receipt_id(&request.request_id)?
        || run.control_id != control.id
        || run.namespace != control.namespace
        || run.request_id != request.request_id
        || run.episode_id != request.episode_id
        || run.step != request.step
        || run.attempt != request.attempt
        || run.executor_actor != control.executor_actor
        || run
            .execution_receipt_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            != execution_ids
        || run
            .grader_receipt_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            != grader_ids
        || run.budget_call_ids.iter().cloned().collect::<BTreeSet<_>>() != call_ids
        || run
            .usage_record_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            != usage_ids
    {
        return Err(Error::Conflict(
            "development run receipt does not close over the observed receipts".into(),
        ));
    }
    require_live_sources(session, ctx, &control.source_ids, request.revoke_watermark).await?;
    Ok(outcomes)
}

/// Fixed trusted runner for registered pure-function targets. It is the
/// executor actor for budget rows and execution receipts and uses a separate
/// grader context for scoring. It never dispatches a provider or a model.
pub struct RegisteredDevelopmentRunner {
    store: Store,
    executor: Context,
    grader: Context,
    control_id: String,
    lease_seconds: i64,
    clock: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl RegisteredDevelopmentRunner {
    pub fn new(
        store: Store,
        executor: Context,
        grader: Context,
        control_id: impl Into<String>,
        lease_seconds: i64,
    ) -> Result<Self> {
        Self::with_clock(
            store,
            executor,
            grader,
            control_id,
            lease_seconds,
            Arc::new(evo_core::now),
        )
    }

    pub fn with_clock(
        store: Store,
        executor: Context,
        grader: Context,
        control_id: impl Into<String>,
        lease_seconds: i64,
        clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Result<Self> {
        executor.require(&[Role::Worker, Role::Host])?;
        grader.require(&[Role::Evaluator])?;
        if executor.actor() == grader.actor() || executor.namespace() != grader.namespace() {
            return Err(Error::Forbidden);
        }
        let control_id = control_id.into();
        identifier(&control_id)?;
        if lease_seconds <= 0 {
            return Err(Error::Invalid("lease must be positive".into()));
        }
        Ok(Self {
            store,
            executor,
            grader,
            control_id,
            lease_seconds,
            clock,
        })
    }

    fn now(&self) -> Result<i64> {
        let now = (self.clock)();
        valid_time(now)?;
        Ok(now)
    }

    async fn existing_receipt(
        &self,
        control: &DevelopmentControlV1,
        request: &DevelopmentRunRequest,
        task: &DevelopmentTaskSpecV1,
        side: DevelopmentSide,
    ) -> Result<Option<DevelopmentExecutionReceiptV1>> {
        let mut session = self.store.session().await?;
        let receipt_id = execution_receipt_id(&request.request_id, &task.task_id, side)?;
        let existing = get_record::<DevelopmentExecutionReceiptV1>(
            &mut session,
            &self.executor,
            EXECUTION_RECEIPT_KIND,
            &receipt_id,
        )
        .await?;
        if let Some(receipt) = &existing {
            if receipt.control_id != control.id {
                return Err(Error::Conflict(
                    "development request already executed under a different control".into(),
                ));
            }
            validate_execution_receipt_binding(receipt, control, request, task, side)?;
        }
        session.commit().await?;
        Ok(existing)
    }

    async fn execute_side(
        &self,
        control: &DevelopmentControlV1,
        root: &RootBudgetRecord,
        request: &DevelopmentRunRequest,
        task: &DevelopmentTaskSpecV1,
        side: DevelopmentSide,
    ) -> Result<DevelopmentExecutionReceiptV1> {
        if let Some(existing) = self.existing_receipt(control, request, task, side).await? {
            return Ok(existing);
        }
        let request_digest = execution_request_digest(control, request, task, side)?;
        let call_id = execution_budget_call_id(&request.request_id, &task.task_id, side)?;
        let lease_token = format!(
            "lease-{}",
            &fingerprint(&(&call_id, "registered-execution-lease"))?[..32]
        );
        let stored_request = RegisteredExecutionRequestV1 {
            schema_version: REGISTERED_EXECUTION_REQUEST_SCHEMA.into(),
            control_id: control.id.clone(),
            request_id: request.request_id.clone(),
            episode_id: request.episode_id.clone(),
            task_id: task.task_id.clone(),
            side,
            input: task.input,
            input_digest: task.input_digest.clone(),
            bundle_digest: side_bundle_digest(request, side).into(),
            environment_digest: request.environment_digest.clone(),
            request_digest: request_digest.clone(),
            target_id: task.target_id.clone(),
            target_digest: control.target_digest.clone(),
            runner_digest: control.runner_digest.clone(),
            source_ids: control.source_ids.clone(),
        };
        let now = self.now()?;
        let reservation = BudgetCallReservation {
            billing_scope: control.billing_scope.clone(),
            call_id: call_id.clone(),
            dispatch_group_id: request.episode_id.clone(),
            stage: BudgetStage::DevelopmentExecution,
            actual_input_digest: request_digest.clone(),
            request_artifact: Some(BudgetArtifact::from_serializable(
                REGISTERED_EXECUTION_REQUEST_SCHEMA,
                &stored_request,
            )?),
            // The storage invariant needs a positive cap; the known charge is 0.
            max_cost_micros: 1,
            lease_token,
            lease_until: now.checked_add(self.lease_seconds).ok_or(Error::Internal)?,
            now,
        };
        let call = self
            .store
            .reserve_budget_call_with_sources(&self.executor, &reservation, &control.source_ids)
            .await?;
        let output_utf8 = task.input.execute_target()?;
        let output_digest = hash(output_utf8.as_bytes());
        let call = match call.state {
            BudgetCallState::Reserved | BudgetCallState::Dispatched => {
                let fence = BudgetCallFence {
                    billing_scope: control.billing_scope.clone(),
                    call_id: call_id.clone(),
                    actual_input_digest: request_digest.clone(),
                    lease_token: call.lease_token.clone(),
                    lease_epoch: call.lease_epoch,
                    now: self.now()?,
                };
                let decision = self
                    .store
                    .begin_budget_dispatch(&self.executor, &fence)
                    .await?;
                let dispatch_id = decision.call.dispatch_id.clone().ok_or(Error::Internal)?;
                let settlement = RegisteredExecutionSettlement {
                    schema_version: REGISTERED_EXECUTION_SETTLEMENT_SCHEMA.into(),
                    provenance: RegisteredExecutionProvenance::RegisteredPureFunction,
                    call_id: call_id.clone(),
                    dispatch_id: dispatch_id.clone(),
                    request_digest: request_digest.clone(),
                    output_digest: output_digest.clone(),
                    target_id: task.target_id.clone(),
                    target_digest: control.target_digest.clone(),
                    runner_digest: control.runner_digest.clone(),
                };
                let charge = UsageCharge {
                    amount_micros: 0,
                    currency: root.currency.clone(),
                    pricing_version: root.pricing_version.clone(),
                    provider_request_id: format!("registered-pure-function:{dispatch_id}"),
                    usage_record_id: execution_usage_record_id(
                        &request.request_id,
                        &task.task_id,
                        side,
                    )?,
                    output_digest: output_digest.clone(),
                };
                self.store
                    .settle_registered_execution_call(&self.executor, &fence, &charge, &settlement)
                    .await?
            }
            BudgetCallState::Finalized | BudgetCallState::Uncertain => call,
            BudgetCallState::Released | BudgetCallState::Cancelled => {
                return Err(Error::Cancelled);
            }
        };
        issue_execution_receipt(
            &self.executor,
            &self.store,
            ExecutionReceiptIssueRequest {
                control_id: control.id.clone(),
                request: request.clone(),
                task_id: task.task_id.clone(),
                side,
                output_utf8,
                budget_call_id: call.call_id,
                issued_at_unix_seconds: self.now()?,
            },
        )
        .await
    }

    async fn issue_run_receipt(
        &self,
        control: &DevelopmentControlV1,
        request: &DevelopmentRunRequest,
        receipt: DevelopmentRunReceiptV1,
    ) -> Result<DevelopmentRunReceiptV1> {
        let mut session = self.store.session().await?;
        if let Some(existing) = get_record::<DevelopmentRunReceiptV1>(
            &mut session,
            &self.executor,
            RUN_RECEIPT_KIND,
            &receipt.id,
        )
        .await?
        {
            if fingerprint(&existing)? != fingerprint(&receipt)? {
                return Err(Error::Conflict(
                    "development run receipt reused with different closure".into(),
                ));
            }
            session.commit().await?;
            return Ok(existing);
        }
        require_live_sources(
            &mut session,
            &self.executor,
            &control.source_ids,
            request.revoke_watermark,
        )
        .await?;
        let run_storage_id = put_record(
            &mut session,
            &self.executor,
            RUN_RECEIPT_KIND,
            &receipt.id,
            &receipt,
        )
        .await?;
        for source in &control.source_ids {
            session
                .put_edge(&self.executor, "artifact", &run_storage_id, "run", source)
                .await?;
        }
        for id in &receipt.execution_receipt_ids {
            session
                .put_edge(
                    &self.executor,
                    "artifact",
                    &run_storage_id,
                    "artifact",
                    &storage_id(EXECUTION_RECEIPT_KIND, id)?,
                )
                .await?;
        }
        for id in &receipt.grader_receipt_ids {
            session
                .put_edge(
                    &self.executor,
                    "artifact",
                    &run_storage_id,
                    "artifact",
                    &storage_id(GRADER_RECEIPT_KIND, id)?,
                )
                .await?;
        }
        session
            .audit(&self.executor, "development.run_receipt.issue", &receipt.id)
            .await?;
        session.commit().await?;
        Ok(receipt)
    }
}

#[async_trait]
impl DevRunner for RegisteredDevelopmentRunner {
    async fn run(&self, request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        validate_development_request(&request)?;
        let control =
            load_development_control(&self.store, &self.executor, &self.control_id).await?;
        if control.executor_actor != self.executor.actor()
            || control.grader_actor != self.grader.actor()
            || control.evidence_scope != DevelopmentEvidenceScope::RegisteredPureFunctionExecution
        {
            return Err(Error::Forbidden);
        }
        validate_control_request_binding(&control, &request)?;
        let root = self
            .store
            .root_budget(&self.executor, &control.billing_scope)
            .await?
            .ok_or(Error::Budget)?;
        if root.root_budget_id != control.root_budget_id {
            return Err(Error::Conflict(
                "control root budget differs from the persisted billing scope".into(),
            ));
        }
        {
            let mut session = self.store.session().await?;
            require_live_sources(
                &mut session,
                &self.executor,
                &control.source_ids,
                request.revoke_watermark,
            )
            .await?;
            session.commit().await?;
        }
        let mut results = Vec::with_capacity(request.manifest.tasks.len());
        let mut execution_receipt_ids = Vec::new();
        let mut grader_receipt_ids = Vec::new();
        let mut budget_call_ids = Vec::new();
        let mut usage_record_ids = Vec::new();
        for manifest_task in &request.manifest.tasks {
            let task = registered_task(&control, &request, &manifest_task.id)?;
            let parent = self
                .execute_side(&control, &root, &request, task, DevelopmentSide::Parent)
                .await?;
            let candidate = self
                .execute_side(&control, &root, &request, task, DevelopmentSide::Candidate)
                .await?;
            let grader = issue_grader_receipt(
                &self.grader,
                &self.store,
                GraderReceiptIssueRequest {
                    control_id: control.id.clone(),
                    request: request.clone(),
                    task_id: task.task_id.clone(),
                    parent_execution_receipt_id: parent.receipt_id.clone(),
                    candidate_execution_receipt_id: candidate.receipt_id.clone(),
                    scoring_budget_call_id: None,
                    issued_at_unix_seconds: self.now()?,
                },
            )
            .await?;
            results.push(PairedTaskResult {
                task_id: task.task_id.clone(),
                parent_score_micros: grader.parent_score_micros,
                candidate_score_micros: grader.candidate_score_micros,
                parent_passed: grader.parent_passed,
                candidate_passed: grader.candidate_passed,
                parent_execution_id: parent.receipt_id.clone(),
                candidate_execution_id: candidate.receipt_id.clone(),
                grader_receipt_digest: fingerprint(&grader)?,
            });
            for receipt in [&parent, &candidate] {
                execution_receipt_ids.push(receipt.receipt_id.clone());
                budget_call_ids.push(receipt.budget_call_id.clone());
                usage_record_ids.push(receipt.usage_record_id.clone());
            }
            grader_receipt_ids.push(grader.receipt_id.clone());
        }
        let run = self
            .issue_run_receipt(
                &control,
                &request,
                DevelopmentRunReceiptV1 {
                    schema_version: DEVELOPMENT_RUN_RECEIPT_SCHEMA.into(),
                    id: run_receipt_id(&request.request_id)?,
                    control_id: control.id.clone(),
                    namespace: control.namespace.clone(),
                    request_id: request.request_id.clone(),
                    episode_id: request.episode_id.clone(),
                    step: request.step,
                    attempt: request.attempt,
                    execution_receipt_ids,
                    grader_receipt_ids,
                    budget_call_ids,
                    usage_record_ids: usage_record_ids.clone(),
                    executor_actor: self.executor.actor().into(),
                    issued_at_unix_seconds: self.now()?,
                },
            )
            .await?;
        Ok(DevelopmentRunReport {
            request_id: request.request_id,
            manifest_digest: request.manifest.digest,
            parent_bundle_digest: request.parent_bundle_digest,
            candidate_bundle_digest: request.candidate_bundle_digest,
            environment_digest: request.environment_digest,
            grader_digest: request.grader_digest,
            results,
            execution_receipt_id: run.id,
            usage_record_ids,
            provenance: DevelopmentExecutionProvenance::RegisteredPureFunction,
        })
    }
}
