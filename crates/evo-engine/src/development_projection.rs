//! Local config-precedence development execution from two explicit frozen Bundles.
//!
//! The target consumes the complete request projection and real owned FD input.
//! Its independent oracle is used only by registration and the grader. This is
//! local software execution, with known zero charge and no production Host claim.

use super::{
    CONTROL_KIND, DEVELOPMENT_EXECUTION_OUTPUT_SCHEMA, DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA,
    DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA_V2, DEVELOPMENT_GRADER_RECEIPT_SCHEMA,
    DEVELOPMENT_RUN_RECEIPT_SCHEMA, DevelopmentCostState, DevelopmentEvidenceScope,
    DevelopmentExecutionOutputV1, DevelopmentExecutionReceiptV1,
    DevelopmentExecutionReceiptVersion, DevelopmentGraderReceiptV1, DevelopmentRunReceiptV1,
    DevelopmentSide, EXECUTION_OUTPUT_KIND, EXECUTION_RECEIPT_KIND, GRADER_RECEIPT_KIND,
    GraderReceiptIssueRequest, RUN_RECEIPT_KIND, VerifiedDevelopmentTaskOutcome,
    budget_call_ref_id, cost_state_from_call, digest, execution_budget_call_id,
    execution_output_id, execution_usage_record_id, get_record, grader_receipt_id,
    legacy_execution_receipt_id, need_record, pair_source_changed, put_record,
    require_live_sources, run_receipt_id, side_bundle_digest, source_legacy, storage_id,
    typed_receipt_closure, valid_time, validate_output_artifact,
};
use crate::compiler::{LoadedAuthenticatedBundleRecord, ReadBundle};
use crate::evidence::load_stored_source;
use crate::optimization::{
    DevRunner, DevelopmentExecutionProvenance, DevelopmentManifest, DevelopmentRunReport,
    DevelopmentRunRequest, DevelopmentTask, OPTIMIZATION_STAGE_FACT_SCHEMA,
    OptimizationJournalStage, PairedTaskResult, StageFact, StageFactKind,
    validate_development_request,
};
use async_trait::async_trait;
use evo_core::contract::RunProjection;
use evo_core::evaluation::config_precedence_generator::{
    ClosedCaseSelector, GeneratedConfigCase, build_descriptive_catalog, generate_case,
    parse_case_selector,
};
use evo_core::evaluation::config_precedence_oracle::{grade_exact, oracle_answer};
use evo_core::evaluation::{
    ControlledTaskAnswerV1, ControlledTaskProfileMaterialsV1, ControlledTaskProfileV1, DataUse,
};
use evo_core::evidence::Purpose;
use evo_core::{Context, Error, Result, Role, fingerprint, hash, identifier};
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallRecord, BudgetCallRef, BudgetCallReservation,
    BudgetCallState, BudgetExecutionProvenance, BudgetStage,
    REGISTERED_EXECUTION_SETTLEMENT_SCHEMA, ReadRegisteredExecutionSettlement,
    RegisteredExecutionProvenance, RegisteredExecutionSettlement, RegisteredExecutionSettlementV2,
    RootBudgetRecord, RootStagePlanBindRequest, RootStagePlanBinding, StagePlanAttribution,
    StagePlanControlSelector, StagePlanRefusal, UsageCharge,
};
use evo_storage::{PersistentContractState, PersistentContractVersion, Session, Store};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

pub const CONFIG_PROJECTION_CONTROL_SCHEMA: &str = "rsia.development_config_control.v1";
pub const CONFIG_PROJECTION_EXECUTION_SCHEMA: &str = "rsia.development_config_execution.v1";
pub(crate) const CONFIG_PROJECTION_EXECUTION_SCHEMA_V2: &str =
    "rsia.development_config_execution.v2";
const CONFIG_PROJECTION_EXECUTION_VERSIONS: &[PersistentContractVersion] = &[
    PersistentContractVersion {
        schema: CONFIG_PROJECTION_EXECUTION_SCHEMA,
        introduced_change: "historical config execution without actual settler proof",
        verifier_retained: false,
        current: false,
    },
    PersistentContractVersion {
        schema: CONFIG_PROJECTION_EXECUTION_SCHEMA_V2,
        introduced_change: "config execution bound to the actual registered settler",
        verifier_retained: true,
        current: true,
    },
];
pub const CONFIG_PROJECTION_REQUEST_SCHEMA: &str = "rsia.development_config_request.v1";
pub const CONFIG_PROJECTION_RUNNER_SCHEMA: &str = "rsia.registered_development_runner.v3";
pub const CONFIG_PROJECTION_TARGET: &str = "config_precedence.v1";
pub const CONFIG_PROJECTION_CONSUMER_VERSION: &str = "config_precedence_projection_consumer.v1";
const CONFIG_GRADER_VERSION: &str = "config_precedence_exact_six_fields.v1";

/// Fixed local consumer contract, independent from the frozen Clamp contracts.
pub fn config_projection_runner_contract() -> serde_json::Value {
    serde_json::json!({
        "schema_version": CONFIG_PROJECTION_RUNNER_SCHEMA,
        "runtime": "local_owned_config_fd", "isolation": "none",
        "provider_dispatch": false, "model_execution": false,
        "targets": [CONFIG_PROJECTION_TARGET],
        "consumer_version": CONFIG_PROJECTION_CONSUMER_VERSION,
        "candidate_consumption": "projection_consumed",
        "instruction_grammar": [
            "config_precedence.v1: highest_layer_first",
            "config_precedence.v1: lowest_layer_first"
        ],
    })
}

pub fn config_projection_runner_digest() -> Result<String> {
    fingerprint(&config_projection_runner_contract())
}

fn target_digest() -> String {
    hash(include_str!("hosts.rs").as_bytes())
}

fn oracle_digest() -> String {
    hash(include_str!("../../evo-core/src/evaluation/config_precedence_oracle.rs").as_bytes())
}

pub fn config_projection_grader_digest() -> Result<String> {
    fingerprint(&(CONFIG_GRADER_VERSION, oracle_digest()))
}

/// A registered Development selector and its answer, frozen before candidate execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigProjectionTaskV1 {
    pub task_id: String,
    pub parent_family: String,
    pub selector: serde_json::Value,
    pub input_digest: String,
    pub expected: ControlledTaskAnswerV1,
}

impl ConfigProjectionTaskV1 {
    pub fn registered(
        parent_family: impl Into<String>,
        selector: serde_json::Value,
    ) -> Result<Self> {
        let case = generate_case(parse_case_selector(&selector)?)?;
        if case.data_use() != DataUse::Development
            || !matches!(case.selector(), ClosedCaseSelector::Task { .. })
        {
            return Err(Error::Forbidden);
        }
        let parent_family = parent_family.into();
        identifier(&parent_family)?;
        Ok(Self {
            task_id: case.task_id().into(),
            parent_family,
            selector,
            input_digest: fingerprint(&case.material())?,
            expected: oracle_answer(&case)?,
        })
    }

    fn manifest_task(&self) -> DevelopmentTask {
        DevelopmentTask {
            id: self.task_id.clone(),
            parent_family: self.parent_family.clone(),
            input_digest: self.input_digest.clone(),
        }
    }
}

/// Admin registration of the complete descriptive catalog and a Development manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigProjectionControlV1 {
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
    pub tasks: Vec<ConfigProjectionTaskV1>,
    pub profile: ControlledTaskProfileV1,
    pub materials: ControlledTaskProfileMaterialsV1,
    pub profile_digest: String,
    pub environment_digest: String,
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

impl ConfigProjectionControlV1 {
    /// Builds the catalog-backed control; registration checks the external boundary.
    #[allow(clippy::too_many_arguments)]
    pub fn registered(
        id: impl Into<String>,
        namespace: impl Into<String>,
        billing_scope: impl Into<String>,
        root_budget_id: impl Into<String>,
        executor_actor: impl Into<String>,
        grader_actor: impl Into<String>,
        proposer_actor: impl Into<String>,
        manifest_id: impl Into<String>,
        tasks: Vec<ConfigProjectionTaskV1>,
        environment_digest: String,
        rules_digest: String,
        tools_digest: String,
        source_ids: Vec<String>,
        created_at_unix_seconds: i64,
    ) -> Result<Self> {
        let (profile, materials) = build_descriptive_catalog()?;
        let profile_digest = profile.digest_against(&materials)?;
        let manifest_id = manifest_id.into();
        let manifest = DevelopmentManifest::build(
            manifest_id.clone(),
            tasks
                .iter()
                .map(ConfigProjectionTaskV1::manifest_task)
                .collect(),
        )?;
        Ok(Self {
            schema_version: CONFIG_PROJECTION_CONTROL_SCHEMA.into(),
            id: id.into(),
            namespace: namespace.into(),
            billing_scope: billing_scope.into(),
            root_budget_id: root_budget_id.into(),
            executor_actor: executor_actor.into(),
            grader_actor: grader_actor.into(),
            proposer_actor: proposer_actor.into(),
            manifest_id,
            manifest_digest: manifest.digest,
            tasks,
            profile,
            materials,
            profile_digest,
            environment_digest,
            grader_digest: config_projection_grader_digest()?,
            oracle_digest: oracle_digest(),
            target_digest: target_digest(),
            runner_digest: config_projection_runner_digest()?,
            rules_digest,
            tools_digest,
            source_ids,
            evidence_scope: DevelopmentEvidenceScope::RegisteredPureFunctionExecution,
            created_at_unix_seconds,
        })
    }

    pub fn manifest(&self) -> Result<DevelopmentManifest> {
        DevelopmentManifest::build(
            self.manifest_id.clone(),
            self.tasks
                .iter()
                .map(ConfigProjectionTaskV1::manifest_task)
                .collect(),
        )
    }
}

// One boundary decode produces the task cases used internally. In particular,
// neither execution side nor each receipt revalidates the 180-task catalog.
struct ConfigControl {
    record: ConfigProjectionControlV1,
    cases: Vec<GeneratedConfigCase>,
    digest: String,
}

impl ConfigControl {
    fn decode(record: ConfigProjectionControlV1) -> Result<Self> {
        if record.schema_version != CONFIG_PROJECTION_CONTROL_SCHEMA {
            return Err(Error::Forbidden);
        }
        for value in [
            &record.id,
            &record.namespace,
            &record.billing_scope,
            &record.root_budget_id,
            &record.executor_actor,
            &record.grader_actor,
            &record.proposer_actor,
            &record.manifest_id,
        ] {
            identifier(value)?;
        }
        if BTreeSet::from([
            record.executor_actor.as_str(),
            record.grader_actor.as_str(),
            record.proposer_actor.as_str(),
        ])
        .len()
            != 3
        {
            return Err(Error::Invalid(
                "executor, grader, and proposer must be distinct actors".into(),
            ));
        }
        for value in [
            &record.environment_digest,
            &record.rules_digest,
            &record.tools_digest,
        ] {
            digest(value, "config development digest")?;
        }
        if record.runner_digest != config_projection_runner_digest()?
            || record.target_digest != target_digest()
            || record.oracle_digest != oracle_digest()
            || record.grader_digest != config_projection_grader_digest()?
            || record.evidence_scope != DevelopmentEvidenceScope::RegisteredPureFunctionExecution
        {
            return Err(Error::Invalid(
                "unregistered config projection contract".into(),
            ));
        }
        let profile_digest = record.profile.digest_against(&record.materials)?;
        let (profile, materials) = build_descriptive_catalog()?;
        if record.profile_digest != profile_digest
            || serde_json::to_value(&record.profile).map_err(|_| Error::Internal)?
                != serde_json::to_value(profile).map_err(|_| Error::Internal)?
            || serde_json::to_value(&record.materials).map_err(|_| Error::Internal)?
                != serde_json::to_value(materials).map_err(|_| Error::Internal)?
        {
            return Err(Error::Conflict(
                "config development registration differs from complete catalog".into(),
            ));
        }
        let mut cases = Vec::with_capacity(record.tasks.len());
        for task in &record.tasks {
            identifier(&task.parent_family)?;
            let case = generate_case(parse_case_selector(&task.selector)?)?;
            if case.data_use() != DataUse::Development
                || !matches!(case.selector(), ClosedCaseSelector::Task { .. })
            {
                return Err(Error::Forbidden);
            }
            if task.task_id != case.task_id()
                || task.input_digest != fingerprint(&case.material())?
                || record.materials.task_materials.get(case.task_id()) != Some(&case.material())
                || serde_json::to_value(&task.expected).map_err(|_| Error::Internal)?
                    != serde_json::to_value(oracle_answer(&case)?).map_err(|_| Error::Internal)?
            {
                return Err(Error::Conflict(
                    "config development task differs from registered material".into(),
                ));
            }
            cases.push(case);
        }
        if record.manifest()?.digest != record.manifest_digest {
            return Err(Error::Conflict(
                "control manifest digest differs from its frozen tasks".into(),
            ));
        }
        for source in &record.source_ids {
            identifier(source)?;
        }
        if record.source_ids.windows(2).any(|ids| ids[0] >= ids[1]) {
            return Err(Error::Invalid(
                "control source_ids must be sorted and unique".into(),
            ));
        }
        valid_time(record.created_at_unix_seconds)?;
        let digest = fingerprint(&record)?;
        Ok(Self {
            record,
            cases,
            digest,
        })
    }

    fn task(&self, task_id: &str) -> Result<(&ConfigProjectionTaskV1, &GeneratedConfigCase)> {
        self.record
            .tasks
            .iter()
            .zip(&self.cases)
            .find(|(task, _)| task.task_id == task_id)
            .ok_or_else(|| Error::Conflict("development task is not registered".into()))
    }

    fn bind(&self, request: &DevelopmentRunRequest) -> Result<()> {
        let c = &self.record;
        if request.purpose != Purpose::Development {
            return Err(Error::Forbidden);
        }
        if request.namespace != c.namespace
            || request.manifest.digest != c.manifest_digest
            || request.environment_digest != c.environment_digest
            || request.grader_digest != c.grader_digest
            || request.rules_digest != c.rules_digest
            || request.tools_digest != c.tools_digest
        {
            return Err(Error::Conflict(
                "development request differs from the registered control".into(),
            ));
        }
        Ok(())
    }
}

async fn need_config_control(s: &mut Session, ctx: &Context, id: &str) -> Result<ConfigControl> {
    identifier(id)?;
    let value: serde_json::Value = need_record(s, ctx, CONTROL_KIND, id).await?;
    let record = serde_json::from_value(value).map_err(|_| Error::Forbidden)?;
    let control = ConfigControl::decode(record)?;
    if control.record.id != id || control.record.namespace != ctx.namespace() {
        return Err(Error::Forbidden);
    }
    Ok(control)
}

/// Registers local config development under the existing three-actor/root authority.
pub async fn register_config_projection_control(
    ctx: &Context,
    store: &Store,
    record: ConfigProjectionControlV1,
) -> Result<String> {
    ctx.require(&[Role::Admin])?;
    let control = ConfigControl::decode(record)?;
    let c = &control.record;
    if c.namespace != ctx.namespace()
        || ctx.actor() == c.executor_actor
        || ctx.actor() == c.grader_actor
    {
        return Err(Error::Forbidden);
    }
    let mut s = store.session().await?;
    let root = s
        .root_budget(ctx, &c.billing_scope)
        .await?
        .ok_or(Error::Budget)?;
    if root.root_budget_id != c.root_budget_id {
        return Err(Error::Conflict(
            "control root budget differs from the persisted billing scope".into(),
        ));
    }
    register_config_projection_control_in_session(&mut s, ctx, &control).await?;
    if s.active_root_stage_plan(ctx, &c.billing_scope)
        .await?
        .is_some()
    {
        s.record_root_stage_plan_control(
            ctx,
            &c.billing_scope,
            CONTROL_KIND,
            &c.id,
            &control.digest,
        )
        .await?;
    }
    s.commit().await?;
    Ok(control.digest)
}

/// Binds one Admin-selected root stage plan and registers the config
/// projection control that references it in one Session.
pub async fn register_config_projection_control_with_stage_plan(
    ctx: &Context,
    store: &Store,
    record: ConfigProjectionControlV1,
    bind: &RootStagePlanBindRequest,
) -> Result<(String, RootStagePlanBinding)> {
    ctx.require(&[Role::Admin])?;
    let control = ConfigControl::decode(record)?;
    let c = &control.record;
    if c.namespace != ctx.namespace()
        || ctx.actor() == c.executor_actor
        || ctx.actor() == c.grader_actor
        || c.billing_scope != bind.billing_scope
    {
        return Err(Error::Forbidden);
    }
    let mut s = store.session().await?;
    let root = s
        .root_budget(ctx, &c.billing_scope)
        .await?
        .ok_or(Error::Budget)?;
    if root.root_budget_id != c.root_budget_id {
        return Err(Error::Conflict(
            "control root budget differs from the persisted billing scope".into(),
        ));
    }
    let binding = s.bind_root_stage_plan(ctx, bind).await?;
    register_config_projection_control_in_session(&mut s, ctx, &control).await?;
    s.record_root_stage_plan_control(ctx, &c.billing_scope, CONTROL_KIND, &c.id, &control.digest)
        .await?;
    s.commit().await?;
    Ok((control.digest, binding))
}

async fn register_config_projection_control_in_session(
    s: &mut Session,
    ctx: &Context,
    control: &ConfigControl,
) -> Result<()> {
    let c = &control.record;
    for source in &c.source_ids {
        if s.get::<serde_json::Value>(ctx, "tombstone", source)
            .await?
            .is_some()
        {
            return Err(Error::Forbidden);
        }
        load_stored_source(s, ctx, source).await?;
    }
    if let Some(old) = get_record::<serde_json::Value>(s, ctx, CONTROL_KIND, &c.id).await? {
        if old != serde_json::to_value(c).map_err(|_| Error::Internal)? {
            return Err(Error::Conflict(
                "development control already registered with different content".into(),
            ));
        }
    } else {
        let id = put_record(s, ctx, CONTROL_KIND, &c.id, c).await?;
        for source in &c.source_ids {
            s.put_edge(ctx, "artifact", &id, "run", source).await?;
        }
        s.audit(ctx, "development.control.register", &c.id).await?;
    }
    Ok(())
}

pub async fn load_config_projection_control(
    store: &Store,
    ctx: &Context,
    id: &str,
) -> Result<ConfigProjectionControlV1> {
    let mut s = store.session().await?;
    let control = need_config_control(&mut s, ctx, id).await?;
    s.commit().await?;
    Ok(control.record)
}

/// New execution wire; the original v1 receipt and its wire remain unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigProjectionExecutionReceiptV1 {
    pub schema_version: String,
    pub execution: DevelopmentExecutionReceiptV1,
    pub request_projection_digest: String,
}

/// A read-time classification and lineage, never caller execution authority.
#[derive(Debug, Clone)]
pub struct ConfigProjectionExecutionReceiptRead {
    pub receipt: ConfigProjectionExecutionReceiptV1,
    pub state: PersistentContractState,
    pub legacy_receipt_id: String,
    pub current_receipt_id: String,
}

fn config_projection_execution_receipt_id(
    request_id: &str,
    task_id: &str,
    side: DevelopmentSide,
) -> Result<String> {
    identifier(request_id)?;
    identifier(task_id)?;
    Ok(format!(
        "devexec-{}",
        fingerprint(&(
            CONFIG_PROJECTION_EXECUTION_SCHEMA_V2,
            request_id,
            task_id,
            side
        ))?
    ))
}

/// Immutable budget request plus the existing Prepared fact form the execution ticket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigProjectionExecutionRequestV1 {
    pub schema_version: String,
    pub control_id: String,
    pub control_digest: String,
    pub root_budget_id: String,
    pub executor_actor: String,
    pub request_fact_id: String,
    pub request: DevelopmentRunRequest,
    pub task_id: String,
    pub side: DevelopmentSide,
    pub parent: LoadedAuthenticatedBundleRecord,
    pub candidate: LoadedAuthenticatedBundleRecord,
    pub projection: RunProjection,
    pub fd_snapshot: serde_json::Value,
    pub source_ids: Vec<String>,
}

/// Read view of the actual complete projection, separated from execution identity.
#[derive(Debug, Clone)]
pub struct VerifiedConfigProjectionTaskInputView {
    pub task_id: String,
    pub parent_request_projection_digest: String,
    pub candidate_request_projection_digest: String,
    pub parent_projection: RunProjection,
    pub candidate_projection: RunProjection,
}

fn projection_mismatch() -> Error {
    Error::Conflict("development_projection_mismatch".into())
}
fn output_mismatch() -> Error {
    Error::Conflict("development_output_mismatch".into())
}

fn projection_content_digest(projection: &RunProjection) -> Result<String> {
    // All content fields actually handed to the consumer, excluding Bundle identity.
    fingerprint(&(&projection.instructions, &projection.tools))
}

// A Prepared fact authorizes an optimization request. Only when it is absent may
// a practice attempt's persisted request closure authorize the same execution;
// without either the ticket is missing exactly as before.
async fn require_request_fact(
    s: &mut Session,
    ctx: &Context,
    c: &ConfigControl,
    request: &DevelopmentRunRequest,
) -> Result<(String, bool)> {
    let expected = StageFact {
        schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
        artifact_id: String::new(),
        namespace: request.namespace.clone(),
        episode_id: request.episode_id.clone(),
        step: request.step,
        attempt: request.attempt,
        stage: OptimizationJournalStage::Development,
        kind: StageFactKind::DevelopmentRequestPrepared,
        request_id: request.request_id.clone(),
        input_digest: request.manifest.digest.clone(),
        output_digest: None,
        dependencies: vec![],
        payload: serde_json::to_value(request).map_err(|_| Error::Internal)?,
    }
    .seal()?;
    let Some(actual) = s
        .get::<StageFact>(ctx, "artifact", &expected.artifact_id)
        .await?
    else {
        return Box::pin(crate::practice::verify_request_closure_in_session(
            s, ctx, &c.record, request,
        ))
        .await?
        .map(|closure| (closure, true))
        .ok_or(Error::NotFound);
    };
    actual.validate()?;
    if actual.payload != expected.payload
        || actual.input_digest != expected.input_digest
        || actual.namespace != ctx.namespace()
        || actual.kind != expected.kind
        || actual.stage != expected.stage
    {
        return Err(Error::Conflict(
            "development prepared request differs from execution ticket".into(),
        ));
    }
    for (kind, id) in [
        ("bundle", request.parent_bundle_digest.as_str()),
        ("bundle", request.candidate_bundle_digest.as_str()),
        ("environment", request.environment_digest.as_str()),
        ("grader", request.grader_digest.as_str()),
    ] {
        if !actual
            .dependencies
            .iter()
            .any(|edge| edge.kind == kind && edge.id == id)
        {
            return Err(Error::Conflict(
                "development prepared request lacks frozen input closure".into(),
            ));
        }
    }
    Ok((actual.artifact_id, false))
}

struct ConfigBundles {
    parent: LoadedAuthenticatedBundleRecord,
    candidate: LoadedAuthenticatedBundleRecord,
    request_fact_id: String,
    practice_closure: bool,
}

async fn load_bundles(
    s: &mut Session,
    ctx: &Context,
    c: &ConfigControl,
    request: &DevelopmentRunRequest,
) -> Result<ConfigBundles> {
    c.bind(request)?;
    require_live_sources(s, ctx, &c.record.source_ids, request.revoke_watermark).await?;
    let (request_fact_id, practice_closure) = require_request_fact(s, ctx, c, request).await?;
    let mut records = Vec::with_capacity(2);
    for digest in [
        &request.parent_bundle_digest,
        &request.candidate_bundle_digest,
    ] {
        let record = crate::compiler::verify_current_bundle_in_session(
            s,
            ctx,
            digest,
            &request.environment_digest,
            request.revoke_watermark,
        )
        .await
        .map_err(|error| match error {
            Error::NotFound | Error::Invalid(_) | Error::Conflict(_) => projection_mismatch(),
            other => other,
        })?;
        if record.bundle.skill.content.is_empty() {
            return Err(projection_mismatch());
        }
        for source in record
            .binding
            .closure
            .nodes
            .iter()
            .filter(|node| node.kind == "run")
        {
            if !c.record.source_ids.contains(&source.id) {
                return Err(projection_mismatch());
            }
        }
        records.push(record);
    }
    let candidate = records.pop().ok_or(Error::Internal)?;
    let parent = records.pop().ok_or(Error::Internal)?;
    Ok(ConfigBundles {
        parent,
        candidate,
        request_fact_id,
        practice_closure,
    })
}

fn freeze_request(
    c: &ConfigControl,
    request: &DevelopmentRunRequest,
    bundles: &ConfigBundles,
    task: &ConfigProjectionTaskV1,
    case: &GeneratedConfigCase,
    side: DevelopmentSide,
) -> Result<ConfigProjectionExecutionRequestV1> {
    let record = match side {
        DevelopmentSide::Parent => &bundles.parent,
        DevelopmentSide::Candidate => &bundles.candidate,
    };
    let bundle = ReadBundle::from_current(record.bundle.clone())?;
    let projection = crate::compiler::projection_for_loaded_snapshot(
        &bundle,
        &record.original_baseline.system_snapshot,
        true,
    )?;
    let files: Vec<_> = case
        .files()
        .iter()
        .map(|file| {
            serde_json::json!({
                "logical_id":file.logical_id(), "source_path":file.source_path(),
                "format":file.format(), "utf8":file.raw_utf8(),
            })
        })
        .collect();
    Ok(ConfigProjectionExecutionRequestV1 {
        schema_version: CONFIG_PROJECTION_REQUEST_SCHEMA.into(),
        control_id: c.record.id.clone(),
        control_digest: c.digest.clone(),
        root_budget_id: c.record.root_budget_id.clone(),
        executor_actor: c.record.executor_actor.clone(),
        request_fact_id: bundles.request_fact_id.clone(),
        request: request.clone(),
        task_id: task.task_id.clone(),
        side,
        parent: bundles.parent.clone(),
        candidate: bundles.candidate.clone(),
        projection,
        fd_snapshot: serde_json::json!({"template_id":case.template().template_id(),
            "layers":case.layer_count().names(), "key_type":case.key_type().registration(),
            "query_segments":case.query_segments(), "files":files}),
        source_ids: c.record.source_ids.clone(),
    })
}

fn execute_frozen(
    case: &GeneratedConfigCase,
    frozen: &ConfigProjectionExecutionRequestV1,
) -> Result<String> {
    let actual = crate::hosts::run_owned_config_projection(case.selector(), &frozen.projection)
        .map_err(|error| Error::Invalid(format!("config projection execution: {error}")))?;
    if actual.snapshot_material != frozen.fd_snapshot {
        return Err(projection_mismatch());
    }
    serde_json::to_string(&actual.answer).map_err(|_| Error::Internal)
}

async fn require_call_sources(
    s: &mut Session,
    ctx: &Context,
    c: &ConfigControl,
    call: &BudgetCallRecord,
    create_empty: bool,
) -> Result<()> {
    let id = budget_call_ref_id(ctx, &c.record.billing_scope, &call.call_id)?;
    let expected = BudgetCallRef {
        id: id.clone(),
        schema_version: "rsia.budget_call_ref.v1".into(),
        namespace: ctx.namespace().into(),
        billing_scope: c.record.billing_scope.clone(),
        call_id: call.call_id.clone(),
        source_ids: c.record.source_ids.clone(),
    };
    let existing = s.get::<serde_json::Value>(ctx, "artifact", &id).await?;
    match existing {
        Some(value) if value == serde_json::to_value(&expected).map_err(|_| Error::Internal)? => {}
        None if create_empty && c.record.source_ids.is_empty() => {
            // Only the same-Session typed reserve calls this branch. It has
            // already checked this new-schema request, its actor and real root.
            s.put(ctx, "artifact", &id, ctx.actor(), &expected).await?;
        }
        _ => {
            return Err(Error::Conflict(
                "development source closure mismatch".into(),
            ));
        }
    }
    Ok(())
}

fn validate_call(
    ctx: &Context,
    c: &ConfigControl,
    call: &BudgetCallRecord,
    frozen: &ConfigProjectionExecutionRequestV1,
    output_digest: &str,
) -> Result<RegisteredExecutionSettlementV2> {
    let request_digest = fingerprint(frozen)?;
    if call.namespace != ctx.namespace()
        || call.billing_scope != c.record.billing_scope
        || call.dispatch_group_id != frozen.request.episode_id
        || call.stage != BudgetStage::DevelopmentExecution
        || call.actual_input_digest != request_digest
        || !call.execution_closed
        || !matches!(
            call.state,
            BudgetCallState::Finalized | BudgetCallState::Uncertain
        )
        || (call.state == BudgetCallState::Finalized && call.actual_cost_micros != Some(0))
        || call.dispatch_id.is_none()
        || call.output_digest.as_deref() != Some(output_digest)
        || call.execution_provenance == Some(BudgetExecutionProvenance::Fixture)
        || call.response_usable != Some(true)
    {
        return Err(Error::Conflict(
            "execution receipt lacks a matching closed development budget call".into(),
        ));
    }
    let fact = match ReadRegisteredExecutionSettlement::from_call(call)? {
        ReadRegisteredExecutionSettlement::Current(fact) => fact,
        ReadRegisteredExecutionSettlement::Legacy(_) => return Err(source_legacy()),
    };
    let settlement = &fact.settlement;
    if settlement.provenance != RegisteredExecutionProvenance::RegisteredPureFunction
        || settlement.target_id != CONFIG_PROJECTION_TARGET
        || settlement.target_digest != c.record.target_digest
        || settlement.runner_digest != c.record.runner_digest
        || settlement.request_digest != request_digest
        || settlement.output_digest != output_digest
    {
        return Err(Error::Conflict(
            "budget row settlement does not bind the registered target and runner".into(),
        ));
    }
    let artifact = call
        .request_artifact
        .as_ref()
        .ok_or_else(projection_mismatch)?;
    if artifact.schema_version != CONFIG_PROJECTION_REQUEST_SCHEMA
        || artifact.digest != hash(artifact.body.as_bytes())
    {
        return Err(projection_mismatch());
    }
    let stored: ConfigProjectionExecutionRequestV1 =
        serde_json::from_str(&artifact.body).map_err(|_| Error::Forbidden)?;
    if fingerprint(&stored)? != request_digest {
        return Err(projection_mismatch());
    }
    if fact.settler_actor != c.record.executor_actor {
        return Err(Error::Conflict(
            "registered execution settler differs from registered executor".into(),
        ));
    }
    Ok(fact)
}

fn parse_config_execution_receipt(
    payload: serde_json::Value,
) -> Result<ConfigProjectionExecutionReceiptV1> {
    // Missing projection evidence is a strict-wire refusal, never identity-only credit.
    let receipt: ConfigProjectionExecutionReceiptV1 =
        serde_json::from_value(payload).map_err(|_| Error::Forbidden)?;
    let version = CONFIG_PROJECTION_EXECUTION_VERSIONS
        .iter()
        .find(|version| version.schema == receipt.schema_version)
        .ok_or_else(|| Error::Invalid("unsupported config execution receipt schema".into()))?;
    let base = &receipt.execution;
    let (schema, expected_id) = if version.current {
        (
            DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA_V2,
            config_projection_execution_receipt_id(&base.request_id, &base.task_id, base.side)?,
        )
    } else {
        (
            DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA,
            legacy_execution_receipt_id(&base.request_id, &base.task_id, base.side)?,
        )
    };
    if base.schema_version != schema || base.receipt_id != expected_id {
        return Err(Error::Forbidden);
    }
    digest(
        &receipt.request_projection_digest,
        "request_projection_digest",
    )?;
    Ok(receipt)
}

async fn need_config_execution(
    s: &mut Session,
    ctx: &Context,
    id: &str,
) -> Result<ConfigProjectionExecutionReceiptV1> {
    let raw: serde_json::Value = need_record(s, ctx, EXECUTION_RECEIPT_KIND, id).await?;
    let receipt = parse_config_execution_receipt(raw)?;
    if receipt.execution.receipt_id != id || receipt.execution.namespace != ctx.namespace() {
        return Err(Error::Forbidden);
    }
    Ok(receipt)
}

pub async fn load_config_projection_execution_receipt(
    store: &Store,
    ctx: &Context,
    id: &str,
) -> Result<ConfigProjectionExecutionReceiptV1> {
    let mut s = store.session().await?;
    let receipt = need_config_execution(&mut s, ctx, id).await?;
    s.commit().await?;
    Ok(receipt)
}

/// Reads one exact config wire and its lineage in the caller's transaction.
pub async fn read_config_projection_execution_receipt_in_session(
    session: &mut Session,
    ctx: &Context,
    request: &DevelopmentRunRequest,
    task_id: &str,
    side: DevelopmentSide,
    version: DevelopmentExecutionReceiptVersion,
) -> Result<ConfigProjectionExecutionReceiptRead> {
    validate_development_request(request)?;
    identifier(task_id)?;
    let legacy_receipt_id = legacy_execution_receipt_id(&request.request_id, task_id, side)?;
    let current_receipt_id =
        config_projection_execution_receipt_id(&request.request_id, task_id, side)?;
    // The requested ID fixes the wire: each version derives its own ID domain.
    let id = match version {
        DevelopmentExecutionReceiptVersion::V1 => &legacy_receipt_id,
        DevelopmentExecutionReceiptVersion::V2 => &current_receipt_id,
    };
    let receipt = need_config_execution(session, ctx, id).await?;
    let c = need_config_control(session, ctx, &receipt.execution.control_id).await?;
    let state = match version {
        DevelopmentExecutionReceiptVersion::V1 => {
            c.bind(request)?;
            let (task, _) = c.task(task_id)?;
            let base = &receipt.execution;
            if base.episode_id != request.episode_id
                || base.step != request.step
                || base.attempt != request.attempt
                || base.input_digest != task.input_digest
                || base.bundle_digest != side_bundle_digest(request, side)
                || base.environment_digest != request.environment_digest
                || base.budget_call_id
                    != execution_budget_call_id(&request.request_id, task_id, side)?
                || base.output_artifact_id
                    != execution_output_id(&request.request_id, task_id, side)?
                || base.execution_provenance
                    != DevelopmentExecutionProvenance::RegisteredPureFunction
                || base.revoke_watermark != request.revoke_watermark
                || base.source_ids != c.record.source_ids
                || base.executor_actor != c.record.executor_actor
                || !matches!(base.executor_role, Role::Worker | Role::Host)
            {
                return Err(projection_mismatch());
            }
            PersistentContractState::Legacy
        }
        DevelopmentExecutionReceiptVersion::V2 => {
            let bundles = load_bundles(session, ctx, &c, request).await?;
            let (task, case) = c.task(task_id)?;
            verify_side(session, ctx, &c, request, &bundles, (task, case), side).await?;
            PersistentContractState::CurrentVerified
        }
    };
    Ok(ConfigProjectionExecutionReceiptRead {
        receipt,
        state,
        legacy_receipt_id,
        current_receipt_id,
    })
}

struct ConfigSide {
    receipt: ConfigProjectionExecutionReceiptV1,
    frozen: ConfigProjectionExecutionRequestV1,
    score: u32,
}

async fn verify_side(
    s: &mut Session,
    ctx: &Context,
    c: &ConfigControl,
    request: &DevelopmentRunRequest,
    bundles: &ConfigBundles,
    task_case: (&ConfigProjectionTaskV1, &GeneratedConfigCase),
    side: DevelopmentSide,
) -> Result<ConfigSide> {
    let (task, case) = task_case;
    let id = config_projection_execution_receipt_id(&request.request_id, &task.task_id, side)?;
    let receipt = need_config_execution(s, ctx, &id).await?;
    let base = &receipt.execution;
    let frozen = freeze_request(c, request, bundles, task, case, side)?;
    if receipt.request_projection_digest != projection_content_digest(&frozen.projection)? {
        return Err(projection_mismatch());
    }
    if receipt.schema_version != CONFIG_PROJECTION_EXECUTION_SCHEMA_V2
        || base.schema_version != DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA_V2
        || base.control_id != c.record.id
        || base.episode_id != request.episode_id
        || base.step != request.step
        || base.attempt != request.attempt
        || base.request_id != request.request_id
        || base.task_id != task.task_id
        || base.side != side
        || base.input_digest != task.input_digest
        || base.bundle_digest != side_bundle_digest(request, side)
        || base.environment_digest != request.environment_digest
        || base.request_digest != fingerprint(&frozen)?
        || base.budget_call_id
            != execution_budget_call_id(&request.request_id, &task.task_id, side)?
        || base.output_artifact_id != execution_output_id(&request.request_id, &task.task_id, side)?
        || base.execution_provenance != DevelopmentExecutionProvenance::RegisteredPureFunction
        || base.revoke_watermark != request.revoke_watermark
        || base.source_ids != c.record.source_ids
        || base.executor_actor != c.record.executor_actor
        || !matches!(base.executor_role, Role::Worker | Role::Host)
    {
        return Err(projection_mismatch());
    }
    let call = s
        .budget_call(ctx, &c.record.billing_scope, &base.budget_call_id)
        .await?
        .ok_or(Error::NotFound)?;
    require_call_sources(s, ctx, c, &call, false).await?;
    if call.dispatch_id.as_deref() != Some(base.dispatch_id.as_str())
        || call.usage_record_id.as_deref() != Some(base.usage_record_id.as_str())
        || cost_state_from_call(&call)? != base.cost_state
    {
        return Err(Error::Conflict(
            "execution receipt differs from its persisted budget row".into(),
        ));
    }
    let output: DevelopmentExecutionOutputV1 =
        need_record(s, ctx, EXECUTION_OUTPUT_KIND, &base.output_artifact_id).await?;
    validate_output_artifact(base, &output)?;
    // Re-execute independently of the persisted output, outer hashes and score DTO.
    if execute_frozen(case, &frozen)? != output.output_utf8 {
        return Err(output_mismatch());
    }
    let fact = validate_call(ctx, c, &call, &frozen, &base.output_digest)?;
    if base.executor_actor != fact.settler_actor || base.executor_role != fact.settler_role {
        return Err(Error::Conflict(
            "registered execution settler differs from registered executor".into(),
        ));
    }
    let value = serde_json::from_str(&output.output_utf8).map_err(|_| output_mismatch())?;
    let score = u32::from(grade_exact(&task.expected, &value)) * 1_000_000;
    Ok(ConfigSide {
        receipt,
        frozen,
        score,
    })
}

struct ConfigPair {
    parent: ConfigSide,
    candidate: ConfigSide,
    grader: DevelopmentGraderReceiptV1,
}

fn grader_for_pair(
    c: &ConfigControl,
    request: &DevelopmentRunRequest,
    task: &ConfigProjectionTaskV1,
    parent: &ConfigSide,
    candidate: &ConfigSide,
    issued_at: i64,
) -> Result<DevelopmentGraderReceiptV1> {
    let p = &parent.receipt.execution;
    let q = &candidate.receipt.execution;
    Ok(DevelopmentGraderReceiptV1 {
        schema_version: DEVELOPMENT_GRADER_RECEIPT_SCHEMA.into(),
        receipt_id: grader_receipt_id(&request.request_id, &task.task_id)?,
        control_id: c.record.id.clone(),
        namespace: request.namespace.clone(),
        episode_id: request.episode_id.clone(),
        step: request.step,
        attempt: request.attempt,
        request_id: request.request_id.clone(),
        task_id: task.task_id.clone(),
        parent_execution_receipt_id: p.receipt_id.clone(),
        candidate_execution_receipt_id: q.receipt_id.clone(),
        parent_output_digest: p.output_digest.clone(),
        candidate_output_digest: q.output_digest.clone(),
        oracle_digest: c.record.oracle_digest.clone(),
        grader_digest: c.record.grader_digest.clone(),
        grader_version: CONFIG_GRADER_VERSION.into(),
        parent_score_micros: parent.score,
        candidate_score_micros: candidate.score,
        parent_passed: parent.score == 1_000_000,
        candidate_passed: candidate.score == 1_000_000,
        scored_output_pair_digest: fingerprint(&(
            &p.output_digest,
            &q.output_digest,
            &c.record.oracle_digest,
            &c.record.grader_digest,
            parent.score,
            candidate.score,
            parent.score == 1_000_000,
            candidate.score == 1_000_000,
        ))?,
        scoring_budget_call_id: None,
        revoke_watermark: request.revoke_watermark,
        source_ids: c.record.source_ids.clone(),
        grader_actor: c.record.grader_actor.clone(),
        issued_at_unix_seconds: issued_at,
    })
}

async fn verify_pair(
    s: &mut Session,
    ctx: &Context,
    c: &ConfigControl,
    request: &DevelopmentRunRequest,
    bundles: &ConfigBundles,
    task: &ConfigProjectionTaskV1,
    case: &GeneratedConfigCase,
) -> Result<ConfigPair> {
    let parent = verify_side(
        s,
        ctx,
        c,
        request,
        bundles,
        (task, case),
        DevelopmentSide::Parent,
    )
    .await?;
    let candidate = verify_side(
        s,
        ctx,
        c,
        request,
        bundles,
        (task, case),
        DevelopmentSide::Candidate,
    )
    .await?;
    let grader: DevelopmentGraderReceiptV1 = need_record(
        s,
        ctx,
        GRADER_RECEIPT_KIND,
        &grader_receipt_id(&request.request_id, &task.task_id)?,
    )
    .await?;
    let expected = grader_for_pair(
        c,
        request,
        task,
        &parent,
        &candidate,
        grader.issued_at_unix_seconds,
    )?;
    if fingerprint(&grader)? != fingerprint(&expected)? {
        return Err(Error::Conflict(
            "grader receipt scores differ from the server-computed grader result".into(),
        ));
    }
    Ok(ConfigPair {
        parent,
        candidate,
        grader,
    })
}

/// The new issuer accepts no output, score, pass flag or caller trust label.
#[derive(Debug, Clone)]
pub struct ConfigProjectionExecutionIssueRequest {
    pub control_id: String,
    pub request: DevelopmentRunRequest,
    pub task_id: String,
    pub side: DevelopmentSide,
    pub lease_seconds: i64,
    pub issued_at_unix_seconds: i64,
}

async fn reject_legacy_request_in_session(
    s: &mut Session,
    ctx: &Context,
    c: &ConfigControl,
    request: &DevelopmentRunRequest,
    bundles: &ConfigBundles,
) -> Result<()> {
    for (task, case) in c.record.tasks.iter().zip(&c.cases) {
        for side in [DevelopmentSide::Parent, DevelopmentSide::Candidate] {
            let legacy_id = legacy_execution_receipt_id(&request.request_id, &task.task_id, side)?;
            if let Some(raw) =
                get_record::<serde_json::Value>(s, ctx, EXECUTION_RECEIPT_KIND, &legacy_id).await?
            {
                let receipt = parse_config_execution_receipt(raw)?;
                if receipt.execution.receipt_id != legacy_id
                    || receipt.execution.namespace != ctx.namespace()
                {
                    return Err(Error::Forbidden);
                }
                if receipt.execution.control_id != c.record.id {
                    return Err(Error::Conflict(
                        "development request already executed under a different control".into(),
                    ));
                }
                return Err(source_legacy());
            }
            let call_id = execution_budget_call_id(&request.request_id, &task.task_id, side)?;
            if let Some(call) = s
                .budget_call(ctx, &c.record.billing_scope, &call_id)
                .await?
                && call.response_artifact.as_ref().is_some_and(|artifact| {
                    artifact.schema_version == REGISTERED_EXECUTION_SETTLEMENT_SCHEMA
                })
            {
                let frozen = freeze_request(c, request, bundles, task, case, side)?;
                if call.actual_input_digest != fingerprint(&frozen)?
                    || call.dispatch_group_id != request.episode_id
                    || call.stage != BudgetStage::DevelopmentExecution
                {
                    return Err(Error::Conflict(
                        "call_id_reused_with_different_content".into(),
                    ));
                }
                ReadRegisteredExecutionSettlement::from_call(&call)?;
                return Err(source_legacy());
            }
        }
    }
    if let Some(run) = get_record::<DevelopmentRunReceiptV1>(
        s,
        ctx,
        RUN_RECEIPT_KIND,
        &run_receipt_id(&request.request_id)?,
    )
    .await?
    {
        for id in &run.execution_receipt_ids {
            let receipt: serde_json::Value =
                need_record(s, ctx, EXECUTION_RECEIPT_KIND, id).await?;
            if receipt["schema_version"] == CONFIG_PROJECTION_EXECUTION_SCHEMA
                || receipt["schema_version"] == DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA
            {
                return Err(source_legacy());
            }
        }
    }
    Ok(())
}

pub async fn issue_config_projection_execution_receipt(
    ctx: &Context,
    store: &Store,
    issue: ConfigProjectionExecutionIssueRequest,
) -> Result<ConfigProjectionExecutionReceiptV1> {
    ctx.require(&[Role::Worker, Role::Host])?;
    validate_development_request(&issue.request)?;
    identifier(&issue.task_id)?;
    valid_time(issue.issued_at_unix_seconds)?;
    if issue.lease_seconds <= 0 {
        return Err(Error::Invalid("lease must be positive".into()));
    }
    let mut s = store.session().await?;
    let c = need_config_control(&mut s, ctx, &issue.control_id).await?;
    if c.record.executor_actor != ctx.actor() {
        return Err(Error::Forbidden);
    }
    let root = s
        .root_budget(ctx, &c.record.billing_scope)
        .await?
        .ok_or(Error::Budget)?;
    if root.root_budget_id != c.record.root_budget_id {
        return Err(Error::Budget);
    }
    let bundles = load_bundles(&mut s, ctx, &c, &issue.request).await?;
    reject_legacy_request_in_session(&mut s, ctx, &c, &issue.request, &bundles).await?;
    s.commit().await?;
    let (task, case) = c.task(&issue.task_id)?;
    execute_side(
        store,
        ctx,
        &c,
        &root,
        &issue.request,
        &bundles,
        task,
        case,
        issue.side,
        issue.lease_seconds,
        issue.issued_at_unix_seconds,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn execute_side(
    store: &Store,
    ctx: &Context,
    c: &ConfigControl,
    root: &RootBudgetRecord,
    request: &DevelopmentRunRequest,
    bundles: &ConfigBundles,
    task: &ConfigProjectionTaskV1,
    case: &GeneratedConfigCase,
    side: DevelopmentSide,
    lease_seconds: i64,
    now: i64,
) -> Result<ConfigProjectionExecutionReceiptV1> {
    let frozen = freeze_request(c, request, bundles, task, case, side)?;
    let receipt_id =
        config_projection_execution_receipt_id(&request.request_id, &task.task_id, side)?;
    let mut s = store.session().await?;
    if get_record::<serde_json::Value>(&mut s, ctx, EXECUTION_RECEIPT_KIND, &receipt_id)
        .await?
        .is_some()
    {
        let current = load_bundles(&mut s, ctx, c, request).await?;
        let verified = verify_side(&mut s, ctx, c, request, &current, (task, case), side).await?;
        s.commit().await?;
        return Ok(verified.receipt);
    }
    let request_digest = fingerprint(&frozen)?;
    let call_id = execution_budget_call_id(&request.request_id, &task.task_id, side)?;
    let reservation = BudgetCallReservation {
        billing_scope: c.record.billing_scope.clone(),
        call_id: call_id.clone(),
        dispatch_group_id: request.episode_id.clone(),
        stage: BudgetStage::DevelopmentExecution,
        actual_input_digest: request_digest.clone(),
        request_artifact: Some(BudgetArtifact::from_serializable(
            CONFIG_PROJECTION_REQUEST_SCHEMA,
            &frozen,
        )?),
        max_cost_micros: 1,
        lease_token: format!(
            "lease-{}",
            &fingerprint(&(&call_id, "config-projection-execution"))?[..32]
        ),
        lease_until: now.checked_add(lease_seconds).ok_or(Error::Internal)?,
        now,
    };
    let current = load_bundles(&mut s, ctx, c, request).await?;
    if fingerprint(&freeze_request(c, request, &current, task, case, side)?)? != request_digest {
        return Err(projection_mismatch());
    }
    let new_call = s
        .budget_call(ctx, &c.record.billing_scope, &call_id)
        .await?
        .is_none();
    let attribution = StagePlanAttribution::Control(StagePlanControlSelector {
        control_kind: CONTROL_KIND.into(),
        namespace: ctx.namespace().into(),
        control_id: c.record.id.clone(),
    });
    let call = match s
        .reserve_budget_call_with_stage_plan_typed(
            ctx,
            &reservation,
            &c.record.source_ids,
            &attribution,
        )
        .await?
    {
        Ok(call) => call,
        Err(refusal) => {
            if let evo_storage::budget::BudgetPreDispatchRefusal::StagePlan { kind, .. } = &refusal
            {
                drop(s);
                let mut events = store.session().await?;
                events
                    .append_stage_plan_refusal(
                        ctx,
                        &StagePlanRefusal {
                            billing_scope: reservation.billing_scope.clone(),
                            stage: reservation.stage,
                            kind: *kind,
                        },
                    )
                    .await?;
                events.commit().await?;
            }
            return Err(refusal.into_reservation_error());
        }
    };
    // Official reserve has checked root, actor, fixed id and this exact artifact.
    require_call_sources(&mut s, ctx, c, &call, new_call).await?;
    s.commit().await?;
    let fence = BudgetCallFence {
        billing_scope: call.billing_scope.clone(),
        call_id: call.call_id.clone(),
        actual_input_digest: call.actual_input_digest.clone(),
        lease_token: call.lease_token.clone(),
        lease_epoch: call.lease_epoch,
        now,
    };
    if matches!(
        call.state,
        BudgetCallState::Released | BudgetCallState::Cancelled
    ) {
        return Err(Error::Cancelled);
    }
    let output_utf8 = match execute_frozen(case, &frozen) {
        Ok(output) => output,
        Err(error) => {
            if call.state == BudgetCallState::Reserved {
                store
                    .release_undispatched_budget_call(
                        ctx,
                        &fence,
                        "development_pre_dispatch_v1.target_execution_failed",
                    )
                    .await?;
            }
            return Err(error);
        }
    };
    let output_digest = hash(output_utf8.as_bytes());
    if matches!(
        call.state,
        BudgetCallState::Reserved | BudgetCallState::Dispatched
    ) {
        let mut s = store.session().await?;
        let gate = async {
            let current = load_bundles(&mut s, ctx, c, request).await?;
            if fingerprint(&freeze_request(c, request, &current, task, case, side)?)?
                != request_digest
            {
                return Err(projection_mismatch());
            }
            require_call_sources(&mut s, ctx, c, &call, false).await?;
            Ok(())
        }
        .await;
        if let Err(error) = gate {
            if call.state == BudgetCallState::Reserved
                && matches!(
                    error,
                    Error::Forbidden | Error::Conflict(_) | Error::NotFound
                )
            {
                s.release_undispatched_budget_call(
                    ctx,
                    &fence,
                    "development_pre_dispatch_v1.compiler_inputs_changed",
                )
                .await?;
                s.commit().await?;
            }
            return Err(error);
        }
        let decision = match s.begin_budget_dispatch_typed(ctx, &fence).await? {
            Ok(decision) => decision,
            Err(refusal) => {
                if call.state == BudgetCallState::Reserved
                    && refusal != evo_storage::budget::BudgetPreDispatchRefusal::SourceRevoked
                {
                    if refusal.is_stage_plan() {
                        // Release this undispatched reservation and record the
                        // single stage refusal in the same Session and commit.
                        s.release_undispatched_budget_call(
                            ctx,
                            &fence,
                            &format!("development_pre_dispatch_v1.{}", refusal.code()),
                        )
                        .await?;
                        let evo_storage::budget::BudgetPreDispatchRefusal::StagePlan {
                            kind, ..
                        } = &refusal
                        else {
                            unreachable!("is_stage_plan checked above");
                        };
                        s.append_stage_plan_refusal(
                            ctx,
                            &StagePlanRefusal {
                                billing_scope: call.billing_scope.clone(),
                                stage: call.stage,
                                kind: *kind,
                            },
                        )
                        .await?;
                    } else {
                        s.release_undispatched_budget_call(
                            ctx,
                            &fence,
                            &format!("development_pre_dispatch_v1.{}", refusal.code()),
                        )
                        .await?;
                    }
                }
                s.commit().await?;
                return Err(refusal.into_dispatch_error());
            }
        };
        s.commit().await?;
        let dispatch_id = decision.call.dispatch_id.clone().ok_or(Error::Internal)?;
        let settlement = RegisteredExecutionSettlement {
            schema_version: REGISTERED_EXECUTION_SETTLEMENT_SCHEMA.into(),
            provenance: RegisteredExecutionProvenance::RegisteredPureFunction,
            call_id: call_id.clone(),
            dispatch_id: dispatch_id.clone(),
            request_digest: request_digest.clone(),
            output_digest: output_digest.clone(),
            target_id: CONFIG_PROJECTION_TARGET.into(),
            target_digest: c.record.target_digest.clone(),
            runner_digest: c.record.runner_digest.clone(),
        };
        let charge = UsageCharge {
            amount_micros: 0,
            currency: root.currency.clone(),
            pricing_version: root.pricing_version.clone(),
            provider_request_id: format!("registered-config-projection:{dispatch_id}"),
            usage_record_id: execution_usage_record_id(&request.request_id, &task.task_id, side)?,
            output_digest: output_digest.clone(),
        };
        store
            .settle_registered_execution_call(ctx, &fence, &charge, &settlement)
            .await?;
    }
    let mut s = store.session().await?;
    require_live_sources(&mut s, ctx, &c.record.source_ids, request.revoke_watermark).await?;
    let call = s
        .budget_call(ctx, &c.record.billing_scope, &call_id)
        .await?
        .ok_or(Error::NotFound)?;
    let fact = validate_call(ctx, c, &call, &frozen, &output_digest)?;
    require_call_sources(&mut s, ctx, c, &call, false).await?;
    let output = DevelopmentExecutionOutputV1 {
        schema_version: DEVELOPMENT_EXECUTION_OUTPUT_SCHEMA.into(),
        id: execution_output_id(&request.request_id, &task.task_id, side)?,
        control_id: c.record.id.clone(),
        request_id: request.request_id.clone(),
        task_id: task.task_id.clone(),
        side,
        output_utf8,
        output_digest: output_digest.clone(),
    };
    let receipt = ConfigProjectionExecutionReceiptV1 {
        schema_version: CONFIG_PROJECTION_EXECUTION_SCHEMA_V2.into(),
        request_projection_digest: projection_content_digest(&frozen.projection)?,
        execution: DevelopmentExecutionReceiptV1 {
            schema_version: DEVELOPMENT_EXECUTION_RECEIPT_SCHEMA_V2.into(),
            receipt_id,
            control_id: c.record.id.clone(),
            namespace: request.namespace.clone(),
            episode_id: request.episode_id.clone(),
            step: request.step,
            attempt: request.attempt,
            request_id: request.request_id.clone(),
            task_id: task.task_id.clone(),
            side,
            input_digest: task.input_digest.clone(),
            bundle_digest: side_bundle_digest(request, side).into(),
            environment_digest: request.environment_digest.clone(),
            request_digest,
            budget_call_id: call.call_id.clone(),
            dispatch_id: call.dispatch_id.clone().ok_or(Error::Internal)?,
            usage_record_id: call.usage_record_id.clone().ok_or(Error::Internal)?,
            output_digest,
            output_artifact_id: output.id.clone(),
            cost_state: cost_state_from_call(&call)?,
            execution_provenance: DevelopmentExecutionProvenance::RegisteredPureFunction,
            revoke_watermark: request.revoke_watermark,
            source_ids: c.record.source_ids.clone(),
            executor_actor: fact.settler_actor,
            executor_role: fact.settler_role,
            issued_at_unix_seconds: now,
        },
    };
    if let Some(existing) =
        get_record::<DevelopmentExecutionOutputV1>(&mut s, ctx, EXECUTION_OUTPUT_KIND, &output.id)
            .await?
        && fingerprint(&existing)? != fingerprint(&output)?
    {
        return Err(Error::Conflict(
            "execution output id reused with different bytes".into(),
        ));
    }
    let output_id = put_record(&mut s, ctx, EXECUTION_OUTPUT_KIND, &output.id, &output).await?;
    let receipt_id = put_record(
        &mut s,
        ctx,
        EXECUTION_RECEIPT_KIND,
        &receipt.execution.receipt_id,
        &receipt,
    )
    .await?;
    for source in &c.record.source_ids {
        for id in [&output_id, &receipt_id] {
            s.put_edge(ctx, "artifact", id, "run", source).await?;
        }
    }
    for target in [
        output_id,
        storage_id(CONTROL_KIND, &c.record.id)?,
        budget_call_ref_id(ctx, &c.record.billing_scope, &call.call_id)?,
        frozen.request_fact_id.clone(),
        bundles.parent.id.clone(),
        bundles.candidate.id.clone(),
    ] {
        s.put_edge(ctx, "artifact", &receipt_id, "artifact", &target)
            .await?;
    }
    s.audit(
        ctx,
        "development.execution_receipt.issue",
        &receipt.execution.receipt_id,
    )
    .await?;
    s.commit().await?;
    Ok(receipt)
}

async fn issue_grader(
    ctx: &Context,
    store: &Store,
    c: &ConfigControl,
    request: &DevelopmentRunRequest,
    task_id: &str,
    issued_at: i64,
) -> Result<DevelopmentGraderReceiptV1> {
    if c.record.grader_actor != ctx.actor() || ctx.role() != Role::Evaluator {
        return Err(Error::Forbidden);
    }
    let mut s = store.session().await?;
    let bundles = load_bundles(&mut s, ctx, c, request).await?;
    let (task, case) = c.task(task_id)?;
    let parent = verify_side(
        &mut s,
        ctx,
        c,
        request,
        &bundles,
        (task, case),
        DevelopmentSide::Parent,
    )
    .await?;
    let candidate = verify_side(
        &mut s,
        ctx,
        c,
        request,
        &bundles,
        (task, case),
        DevelopmentSide::Candidate,
    )
    .await?;
    let id = grader_receipt_id(&request.request_id, task_id)?;
    let existing: Option<DevelopmentGraderReceiptV1> =
        get_record(&mut s, ctx, GRADER_RECEIPT_KIND, &id).await?;
    let receipt = grader_for_pair(
        c,
        request,
        task,
        &parent,
        &candidate,
        existing
            .as_ref()
            .map_or(issued_at, |receipt| receipt.issued_at_unix_seconds),
    )?;
    if let Some(existing) = existing {
        if fingerprint(&existing)? != fingerprint(&receipt)? {
            return Err(Error::Conflict(
                "grader receipt id reused with different evidence".into(),
            ));
        }
    } else {
        let stored = put_record(
            &mut s,
            ctx,
            GRADER_RECEIPT_KIND,
            &receipt.receipt_id,
            &receipt,
        )
        .await?;
        for source in &c.record.source_ids {
            s.put_edge(ctx, "artifact", &stored, "run", source).await?;
        }
        for target in [
            storage_id(EXECUTION_RECEIPT_KIND, &parent.receipt.execution.receipt_id)?,
            storage_id(
                EXECUTION_RECEIPT_KIND,
                &candidate.receipt.execution.receipt_id,
            )?,
            storage_id(CONTROL_KIND, &c.record.id)?,
        ] {
            s.put_edge(ctx, "artifact", &stored, "artifact", &target)
                .await?;
        }
        s.audit(ctx, "development.grader_receipt.issue", &receipt.receipt_id)
            .await?;
    }
    s.commit().await?;
    Ok(receipt)
}

/// Independently re-executes the frozen pair and scores all six answer fields.
pub async fn issue_config_projection_grader_receipt(
    ctx: &Context,
    store: &Store,
    issue: GraderReceiptIssueRequest,
) -> Result<DevelopmentGraderReceiptV1> {
    ctx.require(&[Role::Evaluator])?;
    validate_development_request(&issue.request)?;
    valid_time(issue.issued_at_unix_seconds)?;
    if issue.scoring_budget_call_id.is_some()
        || issue.parent_execution_receipt_id
            != config_projection_execution_receipt_id(
                &issue.request.request_id,
                &issue.task_id,
                DevelopmentSide::Parent,
            )?
        || issue.candidate_execution_receipt_id
            != config_projection_execution_receipt_id(
                &issue.request.request_id,
                &issue.task_id,
                DevelopmentSide::Candidate,
            )?
    {
        return Err(Error::Forbidden);
    }
    let mut s = store.session().await?;
    let c = need_config_control(&mut s, ctx, &issue.control_id).await?;
    s.commit().await?;
    issue_grader(
        ctx,
        store,
        &c,
        &issue.request,
        &issue.task_id,
        issue.issued_at_unix_seconds,
    )
    .await
}

/// Fixed local runner. It never accepts outputs and never reads production Active.
pub struct ConfigProjectionDevelopmentRunner {
    store: Store,
    executor: Context,
    grader: Context,
    control_id: String,
    lease_seconds: i64,
    clock: Arc<dyn Fn() -> Result<i64> + Send + Sync>,
}

impl ConfigProjectionDevelopmentRunner {
    pub fn new(
        store: Store,
        executor: Context,
        grader: Context,
        control_id: impl Into<String>,
        lease_seconds: i64,
    ) -> Result<Self> {
        Self::with_result_clock(
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
        Self::with_result_clock(
            store,
            executor,
            grader,
            control_id,
            lease_seconds,
            Arc::new(move || Ok(clock())),
        )
    }

    fn with_result_clock(
        store: Store,
        executor: Context,
        grader: Context,
        control_id: impl Into<String>,
        lease_seconds: i64,
        clock: Arc<dyn Fn() -> Result<i64> + Send + Sync>,
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
        let now = (self.clock)()?;
        if now < 0 {
            return Err(Error::ClockRegressed);
        }
        Ok(now)
    }

    async fn control(&self) -> Result<(ConfigControl, RootBudgetRecord)> {
        let mut s = self.store.session().await?;
        let c = need_config_control(&mut s, &self.executor, &self.control_id).await?;
        if c.record.executor_actor != self.executor.actor()
            || c.record.grader_actor != self.grader.actor()
        {
            return Err(Error::Forbidden);
        }
        let root = s
            .root_budget(&self.executor, &c.record.billing_scope)
            .await?
            .ok_or(Error::Budget)?;
        if root.root_budget_id != c.record.root_budget_id {
            return Err(Error::Budget);
        }
        s.commit().await?;
        Ok((c, root))
    }
}

#[async_trait]
impl DevRunner for ConfigProjectionDevelopmentRunner {
    fn development_control_selector(&self) -> Option<crate::model::DevelopmentControlSelector> {
        Some(crate::model::DevelopmentControlSelector {
            kind: CONTROL_KIND.into(),
            namespace: self.executor.namespace().into(),
            id: self.control_id.clone(),
        })
    }

    async fn run(&self, request: DevelopmentRunRequest) -> Result<DevelopmentRunReport> {
        validate_development_request(&request)?;
        let (c, root) = self.control().await?;
        let mut s = self.store.session().await?;
        let bundles = load_bundles(&mut s, &self.executor, &c, &request).await?;
        reject_legacy_request_in_session(&mut s, &self.executor, &c, &request, &bundles).await?;
        s.commit().await?;
        let mut results = Vec::with_capacity(c.record.tasks.len());
        let mut executions = BTreeSet::new();
        let mut graders = BTreeSet::new();
        let mut calls = BTreeSet::new();
        let mut usages = BTreeSet::new();
        for (task, case) in c.record.tasks.iter().zip(&c.cases) {
            for side in [DevelopmentSide::Parent, DevelopmentSide::Candidate] {
                // Both issuers are boxed: unoptimized builds otherwise keep their
                // large futures on every polling caller's stack.
                let receipt = Box::pin(execute_side(
                    &self.store,
                    &self.executor,
                    &c,
                    &root,
                    &request,
                    &bundles,
                    task,
                    case,
                    side,
                    self.lease_seconds,
                    self.now()?,
                ))
                .await?;
                executions.insert(receipt.execution.receipt_id);
                calls.insert(receipt.execution.budget_call_id);
                usages.insert(receipt.execution.usage_record_id);
            }
            let grader = Box::pin(issue_grader(
                &self.grader,
                &self.store,
                &c,
                &request,
                &task.task_id,
                self.now()?,
            ))
            .await?;
            graders.insert(grader.receipt_id.clone());
            results.push(PairedTaskResult {
                task_id: task.task_id.clone(),
                parent_score_micros: grader.parent_score_micros,
                candidate_score_micros: grader.candidate_score_micros,
                parent_passed: grader.parent_passed,
                candidate_passed: grader.candidate_passed,
                parent_execution_id: grader.parent_execution_receipt_id.clone(),
                candidate_execution_id: grader.candidate_execution_receipt_id.clone(),
                grader_receipt_digest: fingerprint(&grader)?,
            });
        }
        let mut run = DevelopmentRunReceiptV1 {
            schema_version: DEVELOPMENT_RUN_RECEIPT_SCHEMA.into(),
            id: run_receipt_id(&request.request_id)?,
            control_id: c.record.id.clone(),
            namespace: request.namespace.clone(),
            request_id: request.request_id.clone(),
            episode_id: request.episode_id.clone(),
            step: request.step,
            attempt: request.attempt,
            execution_receipt_ids: executions.into_iter().collect(),
            grader_receipt_ids: graders.into_iter().collect(),
            budget_call_ids: calls.into_iter().collect(),
            usage_record_ids: usages.iter().cloned().collect(),
            executor_actor: self.executor.actor().into(),
            issued_at_unix_seconds: self.now()?,
        };
        let mut s = self.store.session().await?;
        let report = DevelopmentRunReport {
            request_id: request.request_id.clone(),
            manifest_digest: request.manifest.digest.clone(),
            parent_bundle_digest: request.parent_bundle_digest.clone(),
            candidate_bundle_digest: request.candidate_bundle_digest.clone(),
            environment_digest: request.environment_digest.clone(),
            grader_digest: request.grader_digest.clone(),
            results,
            execution_receipt_id: run.id.clone(),
            usage_record_ids: usages.into_iter().collect(),
            provenance: DevelopmentExecutionProvenance::RegisteredPureFunction,
        };
        if let Some(existing) =
            get_record::<DevelopmentRunReceiptV1>(&mut s, &self.executor, RUN_RECEIPT_KIND, &run.id)
                .await?
        {
            require_live_sources(
                &mut s,
                &self.executor,
                &c.record.source_ids,
                request.revoke_watermark,
            )
            .await?;
            run.issued_at_unix_seconds = existing.issued_at_unix_seconds;
            if fingerprint(&existing)? != fingerprint(&run)? {
                return Err(Error::Conflict(
                    "development run receipt reused with different closure".into(),
                ));
            }
        } else {
            load_bundles(&mut s, &self.executor, &c, &request).await?;
            let stored =
                put_record(&mut s, &self.executor, RUN_RECEIPT_KIND, &run.id, &run).await?;
            for source in &c.record.source_ids {
                s.put_edge(&self.executor, "artifact", &stored, "run", source)
                    .await?;
            }
            for id in &run.execution_receipt_ids {
                s.put_edge(
                    &self.executor,
                    "artifact",
                    &stored,
                    "artifact",
                    &storage_id(EXECUTION_RECEIPT_KIND, id)?,
                )
                .await?;
            }
            for id in &run.grader_receipt_ids {
                s.put_edge(
                    &self.executor,
                    "artifact",
                    &stored,
                    "artifact",
                    &storage_id(GRADER_RECEIPT_KIND, id)?,
                )
                .await?;
            }
            s.audit(&self.executor, "development.run_receipt.issue", &run.id)
                .await?;
        }
        s.commit().await?;
        Ok(report)
    }
}

#[async_trait]
impl crate::monitoring::ConsolidationDevRunner for ConfigProjectionDevelopmentRunner {
    async fn trusted_budget_binding(
        &self,
        namespace: &str,
    ) -> Result<crate::broker::BudgetPortBinding> {
        identifier(namespace)?;
        if namespace != self.executor.namespace() {
            return Err(Error::Conflict(
                "development runner serves a different namespace".into(),
            ));
        }
        let (c, _) = self.control().await?;
        Ok(crate::broker::BudgetPortBinding {
            billing_scope: c.record.billing_scope,
            root_budget_id: c.record.root_budget_id,
        })
    }
}

pub(crate) async fn is_config_control(s: &mut Session, ctx: &Context, id: &str) -> Result<bool> {
    let raw: serde_json::Value = need_record(s, ctx, CONTROL_KIND, id).await?;
    Ok(raw["schema_version"] == CONFIG_PROJECTION_CONTROL_SCHEMA)
}

pub(super) fn validate_control_envelope(
    id: &str,
    kind: &str,
    payload: serde_json::Value,
) -> Result<()> {
    let record = serde_json::from_value(payload).map_err(|_| Error::Forbidden)?;
    let c = ConfigControl::decode(record)?;
    if storage_id(kind, &c.record.id)? != id {
        return Err(Error::Forbidden);
    }
    Ok(())
}

pub(super) fn validate_execution_envelope(
    id: &str,
    kind: &str,
    payload: serde_json::Value,
) -> Result<()> {
    let receipt = parse_config_execution_receipt(payload)?;
    if storage_id(kind, &receipt.execution.receipt_id)? != id {
        return Err(Error::Forbidden);
    }
    Ok(())
}

fn bound_pair(
    c: &ConfigControl,
    task: &ConfigProjectionTaskV1,
    pair: &ConfigPair,
) -> Result<evo_core::BoundPairV2> {
    let (baseline, candidate, currency, pricing) = match (
        &pair.parent.receipt.execution.cost_state,
        &pair.candidate.receipt.execution.cost_state,
    ) {
        (
            DevelopmentCostState::Known {
                micros: p,
                currency: pc,
                pricing_version: pp,
            },
            DevelopmentCostState::Known {
                micros: q,
                currency: qc,
                pricing_version: qp,
            },
        ) if pc == qc => (
            u64::try_from(*p).map_err(|_| pair_source_changed())?,
            u64::try_from(*q).map_err(|_| pair_source_changed())?,
            pc.clone(),
            (pp.clone(), qp.clone()),
        ),
        _ => {
            return Err(Error::Conflict(
                "pair metered-cost comparison unavailable".into(),
            ));
        }
    };
    let material = serde_json::to_string(&serde_json::json!({
        "domain":"rsia.config_projection_task_source.v1", "profile":c.record.profile,
        "materials":c.record.materials, "task":task,
        "target_source_utf8":include_str!("hosts.rs"),
        "oracle_source_utf8":include_str!("../../evo-core/src/evaluation/config_precedence_oracle.rs"),
        "runner_contract":config_projection_runner_contract(), "grader_version":CONFIG_GRADER_VERSION,
        "environment_digest":c.record.environment_digest,
        "rules_digest":c.record.rules_digest,"tools_digest":c.record.tools_digest,
    })).map_err(|_| Error::Internal)?;
    evo_core::BoundPairV2::from_development_material(
        &task.task_id,
        &task.parent_family,
        material,
        (pair.parent.score, pair.candidate.score),
        (baseline, candidate),
        currency,
        pricing,
    )
}

pub(super) async fn rebuild_bound_pair_in_session(
    ctx: &Context,
    s: &mut Session,
    control_id: &str,
    request: &DevelopmentRunRequest,
    task_id: &str,
) -> Result<evo_core::BoundPairV2> {
    let c = need_config_control(s, ctx, control_id).await?;
    let bundles = load_bundles(s, ctx, &c, request).await?;
    let (task, case) = c.task(task_id)?;
    let pair = verify_pair(s, ctx, &c, request, &bundles, task, case).await?;
    bound_pair(&c, task, &pair)
}

pub(super) async fn verify_observed_report_in_session(
    ctx: &Context,
    s: &mut Session,
    observed: &StageFact,
    request: &DevelopmentRunRequest,
    report: &DevelopmentRunReport,
    run: &DevelopmentRunReceiptV1,
) -> Result<(
    Vec<VerifiedDevelopmentTaskOutcome>,
    Vec<VerifiedConfigProjectionTaskInputView>,
)> {
    let c = need_config_control(s, ctx, &run.control_id).await?;
    let bundles = load_bundles(s, ctx, &c, request).await?;
    // A Prepared fact pairs with its own observation, exactly as before. A request
    // closure pairs only with the practice transport, which is never sealed or
    // stored, so a closure-authorized run never becomes a stage observation.
    let paired = if bundles.practice_closure {
        observed.artifact_id.is_empty()
    } else {
        bundles.request_fact_id == crate::optimization::development_request_fact_id(observed)?
    };
    if !paired {
        return Err(projection_mismatch());
    }
    for edge in typed_receipt_closure(report)? {
        if !observed
            .dependencies
            .iter()
            .any(|actual| actual.kind == edge.kind && actual.id == edge.id)
        {
            return Err(Error::Conflict(
                "development observation lacks its typed receipt closure".into(),
            ));
        }
    }
    let sources: BTreeSet<_> = observed
        .dependencies
        .iter()
        .filter(|edge| edge.kind == "run")
        .map(|edge| edge.id.as_str())
        .collect();
    if sources != c.record.source_ids.iter().map(String::as_str).collect() {
        return Err(Error::Conflict(
            "development observation sources differ from the registered control".into(),
        ));
    }
    for edge in observed
        .dependencies
        .iter()
        .filter(|edge| edge.kind == "revoke_watermark")
    {
        if edge.id.parse::<u64>().ok() != Some(request.revoke_watermark) {
            return Err(projection_mismatch());
        }
    }
    let mut outcomes = Vec::with_capacity(report.results.len());
    let mut inputs = Vec::with_capacity(report.results.len());
    let mut executions = BTreeSet::new();
    let mut graders = BTreeSet::new();
    let mut calls = BTreeSet::new();
    let mut usages = BTreeSet::new();
    for result in &report.results {
        let (task, case) = c.task(&result.task_id)?;
        let pair = verify_pair(s, ctx, &c, request, &bundles, task, case).await?;
        if fingerprint(&pair.grader)? != result.grader_receipt_digest
            || result.parent_execution_id != pair.parent.receipt.execution.receipt_id
            || result.candidate_execution_id != pair.candidate.receipt.execution.receipt_id
            || result.parent_score_micros != pair.parent.score
            || result.candidate_score_micros != pair.candidate.score
            || result.parent_passed != pair.grader.parent_passed
            || result.candidate_passed != pair.grader.candidate_passed
        {
            return Err(Error::Conflict(
                "development report scores differ from the server-computed grader result".into(),
            ));
        }
        for side in [&pair.parent, &pair.candidate] {
            let base = &side.receipt.execution;
            executions.insert(base.receipt_id.clone());
            calls.insert(base.budget_call_id.clone());
            usages.insert(base.usage_record_id.clone());
        }
        graders.insert(pair.grader.receipt_id.clone());
        // Zero-cost software pairs still carry real integer quality and money
        // facts; they never claim a monetary saving or Formal sample authority.
        if matches!((&pair.parent.receipt.execution.cost_state, &pair.candidate.receipt.execution.cost_state),
            (DevelopmentCostState::Known { currency: p, .. }, DevelopmentCostState::Known { currency: q, .. }) if p == q)
        {
            bound_pair(&c, task, &pair)?;
        }
        outcomes.push(VerifiedDevelopmentTaskOutcome {
            task_id: task.task_id.clone(),
            parent_family: task.parent_family.clone(),
            parent_score_micros: pair.parent.score,
            candidate_score_micros: pair.candidate.score,
            parent_passed: pair.grader.parent_passed,
            candidate_passed: pair.grader.candidate_passed,
            parent_execution_id: result.parent_execution_id.clone(),
            candidate_execution_id: result.candidate_execution_id.clone(),
            grader_receipt_digest: result.grader_receipt_digest.clone(),
        });
        inputs.push(VerifiedConfigProjectionTaskInputView {
            task_id: task.task_id.clone(),
            parent_request_projection_digest: pair.parent.receipt.request_projection_digest,
            candidate_request_projection_digest: pair.candidate.receipt.request_projection_digest,
            parent_projection: pair.parent.frozen.projection,
            candidate_projection: pair.candidate.frozen.projection,
        });
    }
    if run.schema_version != DEVELOPMENT_RUN_RECEIPT_SCHEMA
        || run.id != run_receipt_id(&request.request_id)?
        || run.namespace != request.namespace
        || run.request_id != request.request_id
        || run.episode_id != request.episode_id
        || run.step != request.step
        || run.attempt != request.attempt
        || run.executor_actor != c.record.executor_actor
        || run
            .execution_receipt_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            != executions
        || run
            .grader_receipt_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            != graders
        || run.budget_call_ids.iter().cloned().collect::<BTreeSet<_>>() != calls
        || run
            .usage_record_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            != usages
        || report
            .usage_record_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            != usages
    {
        return Err(Error::Conflict(
            "development run receipt does not close over the observed receipts".into(),
        ));
    }
    Ok((outcomes, inputs))
}

pub(super) async fn verify_current_run_in_session(
    s: &mut Session,
    ctx: &Context,
    run: &DevelopmentRunReceiptV1,
) -> Result<()> {
    // Same canonical coordinates as StageFact. Neither stored provenance nor a
    // new contract label substitutes for re-reading the real observation.
    let observed_id = crate::optimization::canonical_stage_artifact_id(&StageFact {
        schema_version: OPTIMIZATION_STAGE_FACT_SCHEMA.into(),
        artifact_id: String::new(),
        namespace: run.namespace.clone(),
        episode_id: run.episode_id.clone(),
        step: run.step,
        attempt: run.attempt,
        stage: OptimizationJournalStage::Development,
        kind: StageFactKind::DevelopmentObserved,
        request_id: run.request_id.clone(),
        input_digest: String::new(),
        output_digest: None,
        dependencies: vec![],
        payload: serde_json::Value::Null,
    })?;
    let observed: StageFact = s.need(ctx, "artifact", &observed_id).await?;
    observed.validate()?;
    let prepared: StageFact = s
        .need(
            ctx,
            "artifact",
            &crate::optimization::development_request_fact_id(&observed)?,
        )
        .await?;
    prepared.validate()?;
    let request: DevelopmentRunRequest =
        serde_json::from_value(prepared.payload).map_err(|_| Error::Forbidden)?;
    let report: DevelopmentRunReport =
        serde_json::from_value(observed.payload.clone()).map_err(|_| Error::Forbidden)?;
    crate::optimization::select_development(&request, &report)?;
    if report.execution_receipt_id != run.id {
        return Err(projection_mismatch());
    }
    verify_observed_report_in_session(ctx, s, &observed, &request, &report, run).await?;
    Ok(())
}
