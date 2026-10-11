//! Admin views of stored material. Every read recipe is executed in two separate
//! transactions; neither its internal fingerprints nor hidden payloads are DTOs.

use crate::evidence::{load_stored_source, validate_stored_sources};
use crate::optimization::{
    DevelopmentRunRequest, DevelopmentSelection, DevelopmentSelectionDecision,
    OptimizationJournalStage, StageFact, StageFactKind, StepTerminalClass,
    development_request_fact_id, verified_development_observation_in_session,
};
use crate::release_store::{RELEASE_CANDIDATE_SCHEMA, ReleaseStore, TypedSourceRef};
use crate::streaming_evaluator::{
    AnchorEvidenceStatus, CostEvidenceScope, DependencyEvidenceStatus, EvaluationEvidenceScope,
    EvaluationTicketV2, ExecutionReceiptV2, TargetState, VerifiedReportVariant,
    verified_report_view_in_session,
};
use evo_core::contract::{
    BUNDLE_SCHEMA, HostCapabilities, ImproverPatch, OriginLayer, PatchOp, Profile, ResolvedBundle,
    SkillPatch, SkillSnapshot,
};
use evo_core::evaluation::{ProfileKind, Verdict};
use evo_core::evidence::{EvidenceSet, Purpose, SourceSelection};
use evo_core::optimization::{MergeRecord, ModelRequestContext, OptimizationTrace};
use evo_core::skill_edit::{
    EditApplyReport, EditApplyStatus, EvidenceClosure, MAX_CHANGED_BYTES, MAX_EDITS_PER_BATCH,
    SKILL_EDIT_COMPILER_VERSION, SKILL_EDIT_REPORT_SCHEMA, SKILL_EDIT_SCHEMA, SkillEditBatch,
    SkillTextField, TextEditOperation, skill_snapshot_digest,
};
use evo_core::{Context, Error, Result, Role, Strategy, Validate, fingerprint, hash, identifier};
use evo_storage::{Session, Store};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const REVIEW_SCHEMA: &str = "rsia.review.v1";
pub const PRIVACY_POLICY_SCOPE: &str = "rsia.review.finite_whole_field_privacy.v1";
pub const MAX_REVIEW_FACTS: usize = 32;
pub const MAX_REVIEW_REFS: usize = 128;
pub const MAX_REVIEW_FIELD_BYTES: usize = 16 * 1024;
pub const MAX_REVIEW_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewKind {
    TypedCandidate,
    StageFact,
    FormalReport,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDetail {
    #[default]
    Metadata,
    Exact,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRequest {
    pub kind: ReviewKind,
    pub id: String,
    #[serde(default)]
    pub detail: ReviewDetail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewGap {
    TypedCandidateHistoryUnbound,
    TypedcandidateformalUnbound,
    MissingRejectionReason,
    ParentMaterialUnbound,
    BaselineMaterialUnbound,
    CostUnobserved,
    HostApplicationUnbound,
    ApprovalUnbound,
    RollbackTargetUnbound,
    ReferenceUnavailable,
    PrivacyBlocked,
    UnsupportedText,
    UnsupportedSchema,
    UnsupportedKind,
    SourceChanged,
    ReadChanged,
    SupportLimit,
    MaterialInvalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TextReason {
    Metadata,
    Exact,
    PrivacyBlocked,
    UnsupportedText,
}

#[derive(Debug, Clone, Serialize)]
pub struct TextView {
    pub original_field_sha256: Option<String>,
    pub original_bytes: Option<usize>,
    pub display: Option<String>,
    pub display_sha256: Option<String>,
    pub reason: TextReason,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillView {
    pub content: TextView,
    pub applicability: TextView,
    pub counterexample: TextView,
    pub required_capabilities: Vec<TextView>,
    pub dependencies: Vec<TextView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StrategyView {
    pub schema_version: &'static str,
    pub instruction: TextView,
    pub max_candidates: u8,
    pub max_rounds: u8,
    pub require_counterexample: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum OriginField {
    #[serde(rename = "skill.content")]
    Content,
    #[serde(rename = "skill.applicability")]
    Applicability,
    #[serde(rename = "skill.counterexample")]
    Counterexample,
    #[serde(rename = "skill.required_capabilities")]
    RequiredCapabilities,
    #[serde(rename = "skill.dependencies")]
    Dependencies,
    #[serde(rename = "improver.instruction")]
    Instruction,
    #[serde(rename = "improver.max_candidates")]
    MaxCandidates,
    #[serde(rename = "improver.max_rounds")]
    MaxRounds,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginOperation {
    Inherit,
    ResetToBaseline,
    Set,
}

#[derive(Debug, Clone, Serialize)]
pub struct OriginView {
    pub field: OriginField,
    pub layer: OriginLayer,
    pub operation: OriginOperation,
    pub source_digest: Option<String>,
    pub value_digest: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BundleView {
    pub schema_version: &'static str,
    pub profile_id: TextView,
    pub bundle_digest: String,
    pub declared_parent_digest: String,
    pub declared_baseline_digest: String,
    pub skill_snapshot_digest: Option<String>,
    pub skill: SkillView,
    pub improver: StrategyView,
    pub origins: Vec<OriginView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceView {
    pub kind: &'static str,
    pub id: TextView,
    pub content_digest: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WatermarkView {
    pub seq: i64,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CandidateView {
    pub id: TextView,
    pub namespace: TextView,
    pub proposer_actor: TextView,
    pub environment_digest: String,
    pub bundle: BundleView,
    pub sources: Vec<SourceView>,
    pub revoke_watermark: Option<WatermarkView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FactView {
    pub artifact_id: String,
    pub namespace: TextView,
    pub episode_id: TextView,
    pub step: u32,
    pub attempt: u32,
    pub stage: OptimizationJournalStage,
    pub kind: StageFactKind,
    pub request_id: TextView,
    pub terminal_class_code: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreparedView {
    pub profile_id: TextView,
    pub declared_parent_digest: String,
    pub declared_baseline_digest: String,
    pub parent: SkillView,
    pub baseline: SkillView,
    pub parent_strategy: StrategyView,
    pub baseline_strategy: StrategyView,
}

#[derive(Debug, Clone, Serialize)]
pub struct MergeView {
    pub input_suggestion_ids: Vec<TextView>,
    pub selected_suggestion_ids: Vec<TextView>,
    pub unselected_suggestion_ids: Vec<TextView>,
    pub read_dependencies: Vec<SourceView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EditView {
    pub field: SkillTextField,
    pub start: Option<usize>,
    pub end: Option<usize>,
    pub removed: TextView,
    pub inserted: TextView,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiffView {
    pub status: EditApplyStatus,
    pub changed_bytes: Option<usize>,
    pub edits: Vec<EditView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SelectionView {
    pub request_id: TextView,
    pub manifest_digest: String,
    pub parent_bundle_digest: String,
    pub candidate_bundle_digest: String,
    pub decision: DevelopmentSelectionDecision,
    pub parent_total_micros: u64,
    pub candidate_total_micros: u64,
    pub reason: TextView,
}

#[derive(Debug, Clone, Serialize)]
pub struct StageView {
    pub facts: Vec<FactView>,
    pub sources: Vec<SourceView>,
    pub revoke_watermark: Option<WatermarkView>,
    pub prepared: Option<PreparedView>,
    pub bundle: Option<BundleView>,
    pub merge: Option<MergeView>,
    pub diff: Option<DiffView>,
    pub selection: Option<SelectionView>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FormalIneligibilityReason {
    ProgramFixtureNotProductionAuthority,
    IndependentProcessIsolationNotVerified,
    DependencyRevocationClosureUnverified,
    ReportVariantNotCompleteBatch,
    ResourceEvidenceAbsent,
    CompleteOptimizationCostNotVerified,
    FormalVerdictNotApprovable,
}

#[derive(Debug, Clone, Serialize)]
pub struct FormalView {
    pub report_id: TextView,
    pub report_digest: String,
    pub variant: VerifiedReportVariant,
    pub verdict: Option<Verdict>,
    pub ticket_id: TextView,
    pub ticket_digest: String,
    pub v1_plan_snapshot_digest: String,
    pub formal_plan_digest: String,
    pub manifest_digest: String,
    pub candidate_digest: String,
    pub baseline_parent_digest: String,
    pub candidate_bundle_digest: String,
    pub baseline_bundle_digest: String,
    pub environment_digest: String,
    pub profile: ProfileKind,
    pub proposer_actor: TextView,
    pub evaluator_actor: TextView,
    pub approver_actor: TextView,
    pub anchor_evidence_digest: Option<String>,
    pub resource_evidence_digest: Option<String>,
    pub anchor_status: AnchorEvidenceStatus,
    pub evidence_scope: EvaluationEvidenceScope,
    pub cost_evidence_scope: Option<CostEvidenceScope>,
    pub revoke_watermark: Option<WatermarkView>,
    pub dependency_status: DependencyEvidenceStatus,
    pub promotion_eligible: bool,
    pub ineligibility_reasons: Vec<FormalIneligibilityReason>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "materials", rename_all = "snake_case")]
pub enum ReviewPayload {
    TypedCandidate(Box<CandidateView>),
    StageFact(Box<StageView>),
    FormalReport(Box<FormalView>),
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewResponse {
    pub schema_version: &'static str,
    pub kind: ReviewKind,
    pub detail: ReviewDetail,
    pub privacy_policy_scope: &'static str,
    pub snapshot_semantics: &'static str,
    pub diff_complete: bool,
    pub trusted_tokens: Option<u64>,
    pub total_cost: Option<u64>,
    pub gaps: Vec<ReviewGap>,
    pub payload: Option<ReviewPayload>,
}

impl ReviewResponse {
    fn empty(request: &ReviewRequest, gap: ReviewGap) -> Self {
        Self {
            schema_version: REVIEW_SCHEMA,
            kind: request.kind,
            detail: request.detail,
            privacy_policy_scope: PRIVACY_POLICY_SCOPE,
            snapshot_semantics: "verified_at_second_read",
            diff_complete: false,
            trusted_tokens: None,
            total_cost: None,
            gaps: vec![gap, ReviewGap::CostUnobserved],
            payload: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadFailure {
    Unavailable,
    Internal,
    Gap(ReviewGap),
}
type ReadResult<T> = std::result::Result<T, ReadFailure>;

fn material_error(_: Error) -> ReadFailure {
    ReadFailure::Gap(ReviewGap::MaterialInvalid)
}
fn storage_error(error: Error) -> ReadFailure {
    match error {
        Error::Internal => ReadFailure::Internal,
        _ => ReadFailure::Gap(ReviewGap::ReferenceUnavailable),
    }
}
fn digest(value: &str) -> ReadResult<()> {
    if value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(ReadFailure::Gap(ReviewGap::MaterialInvalid))
    }
}

fn skill_text_limits(skill: &SkillSnapshot) -> ReadResult<()> {
    if [&skill.content, &skill.applicability, &skill.counterexample]
        .into_iter()
        .chain(skill.required_capabilities.iter())
        .chain(skill.dependencies.iter())
        .any(|text| text.len() > MAX_REVIEW_FIELD_BYTES)
    {
        return Err(ReadFailure::Gap(ReviewGap::SupportLimit));
    }
    Ok(())
}

// Canonical-value equality rejects ignored/defaulted unknown nested keys too.
// This is internal validation, never a raw-record-to-JSON response filter.
fn decode<T: DeserializeOwned + Serialize>(value: &Value) -> ReadResult<T> {
    let typed: T = serde_json::from_value(value.clone())
        .map_err(|_| ReadFailure::Gap(ReviewGap::MaterialInvalid))?;
    let encoded = serde_json::to_vec(&typed).map_err(|_| ReadFailure::Internal)?;
    let canonical: Value = serde_json::from_slice(&encoded).map_err(|_| ReadFailure::Internal)?;
    if canonical != *value {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    Ok(typed)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ReadSet {
    objects: BTreeMap<(String, String), String>,
    budget_calls: BTreeMap<String, String>,
    relations: BTreeMap<(String, String), Vec<(String, String)>>,
    closure: Vec<(String, String, bool, Option<String>)>,
    watermark: Option<(i64, String)>,
}

struct ReadSnapshot {
    read_set: ReadSet,
    response: ReviewResponse,
}

async fn capture(
    ctx: &Context,
    session: &mut Session,
    set: &mut ReadSet,
    kind: &str,
    id: &str,
) -> ReadResult<Value> {
    identifier(id).map_err(material_error)?;
    let key = (kind.to_owned(), id.to_owned());
    if !set.objects.contains_key(&key)
        && set.objects.len().saturating_add(set.budget_calls.len()) >= MAX_REVIEW_REFS
    {
        return Err(ReadFailure::Gap(ReviewGap::SupportLimit));
    }
    let value: Value = session
        .get(ctx, kind, id)
        .await
        .map_err(storage_error)?
        .ok_or(ReadFailure::Gap(ReviewGap::ReferenceUnavailable))?;
    set.objects
        .insert(key, fingerprint(&value).map_err(material_error)?);
    Ok(value)
}

async fn check_edge(
    ctx: &Context,
    session: &mut Session,
    set: &mut ReadSet,
    source_kind: &str,
    source_id: &str,
    destination_kind: &str,
    destination_id: &str,
) -> ReadResult<()> {
    let key = (destination_kind.to_owned(), destination_id.to_owned());
    if !set.relations.contains_key(&key) {
        if set.relations.len() >= MAX_REVIEW_REFS {
            return Err(ReadFailure::Gap(ReviewGap::SupportLimit));
        }
        let mut incoming = session
            .dependents(ctx, destination_kind, destination_id)
            .await
            .map_err(storage_error)?;
        if incoming.len() > MAX_REVIEW_REFS {
            return Err(ReadFailure::Gap(ReviewGap::SupportLimit));
        }
        incoming.sort();
        set.relations.insert(key.clone(), incoming);
    }
    if !set.relations[&key]
        .iter()
        .any(|(kind, id)| kind == source_kind && id == source_id)
    {
        return Err(ReadFailure::Gap(ReviewGap::ReferenceUnavailable));
    }
    Ok(())
}

async fn finish_read_set(
    ctx: &Context,
    session: &mut Session,
    set: &mut ReadSet,
) -> ReadResult<()> {
    let roots: Vec<_> = set.objects.keys().cloned().collect();
    let nodes = session
        .upstream_closure(ctx, &roots, MAX_REVIEW_REFS)
        .await
        .map_err(|e| match e {
            Error::Conflict(_) => ReadFailure::Gap(ReviewGap::SupportLimit),
            other => storage_error(other),
        })?;
    let closure_ids: BTreeSet<_> = nodes
        .iter()
        .map(|node| (node.kind.as_str(), node.id.as_str()))
        .collect();
    for node in &nodes {
        if node.tombstone.is_some() || node.redacted {
            return Err(ReadFailure::Gap(ReviewGap::SourceChanged));
        }
        let value = capture(ctx, session, set, &node.kind, &node.id).await?;
        if node.kind == "artifact"
            && value.get("schema_version").and_then(Value::as_str)
                == Some("rsia.budget_call_ref.v1")
        {
            let reference: evo_storage::budget::BudgetCallRef = decode(&value)?;
            let expected = fingerprint(&(
                "rsia.budget_call_ref.v1",
                ctx.namespace(),
                &reference.billing_scope,
                &reference.call_id,
            ))
            .map_err(material_error)?;
            if reference.id != node.id
                || reference.id != format!("budget-ref-{}", &expected[..32])
                || reference.namespace != ctx.namespace()
            {
                return Err(ReadFailure::Gap(ReviewGap::ReferenceUnavailable));
            }
            unique_ids(&reference.source_ids)?;
            if reference
                .source_ids
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            {
                return Err(ReadFailure::Gap(ReviewGap::ReferenceUnavailable));
            }
            // Declared sources must already be in the actual dependency walk.
            // No declaration can add a run after the live closure was read.
            for source in &reference.source_ids {
                check_edge(ctx, session, set, "artifact", &reference.id, "run", source).await?;
                if !closure_ids.contains(&("run", source.as_str())) {
                    return Err(ReadFailure::Gap(ReviewGap::ReferenceUnavailable));
                }
                load_stored_source(session, ctx, source)
                    .await
                    .map_err(storage_error)?;
            }
            let call = session
                .budget_call(ctx, &reference.billing_scope, &reference.call_id)
                .await
                .map_err(storage_error)?
                .ok_or(ReadFailure::Gap(ReviewGap::ReferenceUnavailable))?;
            if call.namespace != reference.namespace
                || call.billing_scope != reference.billing_scope
                || call.call_id != reference.call_id
            {
                return Err(ReadFailure::Gap(ReviewGap::ReferenceUnavailable));
            }
            if !set.budget_calls.contains_key(&reference.id)
                && set.objects.len().saturating_add(set.budget_calls.len()) >= MAX_REVIEW_REFS
            {
                return Err(ReadFailure::Gap(ReviewGap::SupportLimit));
            }
            set.budget_calls
                .insert(reference.id, fingerprint(&call).map_err(material_error)?);
        }
        set.closure.push((
            node.kind.clone(),
            node.id.clone(),
            node.redacted,
            node.tombstone
                .as_ref()
                .map(fingerprint)
                .transpose()
                .map_err(material_error)?,
        ));
    }
    set.watermark = session.watermark(ctx).await.map_err(storage_error)?;
    Ok(())
}

/// Reads stored authority twice. There is deliberately no model, compiler,
/// broker, evaluator-state, approval, release or storage write entry point here.
pub async fn review_stored(
    ctx: &Context,
    store: &Store,
    request: ReviewRequest,
) -> Result<ReviewResponse> {
    ctx.require(&[Role::Admin])?;
    identifier(&request.id).map_err(|_| Error::Invalid("invalid review request".into()))?;
    let mut first_session = store.session().await?;
    let first = read_recipe(ctx, &mut first_session, &request).await;
    first_session.commit().await?;
    let mut second_session = store.session().await?;
    let second = read_recipe(ctx, &mut second_session, &request).await;
    second_session.commit().await?;
    match (first, second) {
        (Err(ReadFailure::Unavailable), _) | (_, Err(ReadFailure::Unavailable)) => {
            Err(Error::NotFound)
        }
        (Err(ReadFailure::Internal), _) | (_, Err(ReadFailure::Internal)) => Err(Error::Internal),
        (Ok(a), Ok(b)) if a.read_set == b.read_set => Ok(b.response),
        (Err(ReadFailure::Gap(a)), Err(ReadFailure::Gap(b))) if a == b => {
            Ok(ReviewResponse::empty(&request, a))
        }
        _ => Ok(ReviewResponse::empty(&request, ReviewGap::ReadChanged)),
    }
}

async fn read_recipe(
    ctx: &Context,
    session: &mut Session,
    request: &ReviewRequest,
) -> ReadResult<ReadSnapshot> {
    let mut set = ReadSet::default();
    let mut projection = Projection::new(request.detail);
    let mut response = ReviewResponse::empty(request, ReviewGap::CostUnobserved);
    response.payload = Some(match request.kind {
        ReviewKind::TypedCandidate => ReviewPayload::TypedCandidate(Box::new(
            read_candidate(ctx, session, &mut set, &mut projection, &request.id).await?,
        )),
        ReviewKind::StageFact => {
            let (view, complete) =
                read_stage(ctx, session, &mut set, &mut projection, &request.id).await?;
            response.diff_complete = complete;
            ReviewPayload::StageFact(Box::new(view))
        }
        ReviewKind::FormalReport => ReviewPayload::FormalReport(Box::new(
            read_formal(ctx, session, &mut set, &mut projection, &request.id).await?,
        )),
    });
    finish_read_set(ctx, session, &mut set).await?;
    if let Some((_, value)) = &set.watermark {
        digest(value)?;
    }
    response.gaps = projection.gaps.into_iter().collect();
    if serde_json::to_vec(&response)
        .map_err(|_| ReadFailure::Internal)?
        .len()
        > MAX_REVIEW_BYTES
    {
        return Err(ReadFailure::Gap(ReviewGap::SupportLimit));
    }
    Ok(ReadSnapshot {
        read_set: set,
        response,
    })
}

struct Projection {
    detail: ReviewDetail,
    gaps: BTreeSet<ReviewGap>,
}
impl Projection {
    fn new(detail: ReviewDetail) -> Self {
        Self {
            detail,
            gaps: [
                ReviewGap::CostUnobserved,
                ReviewGap::HostApplicationUnbound,
                ReviewGap::ApprovalUnbound,
                ReviewGap::RollbackTargetUnbound,
            ]
            .into_iter()
            .collect(),
        }
    }
    fn text(&mut self, text: &str) -> ReadResult<TextView> {
        if text.len() > MAX_REVIEW_FIELD_BYTES {
            return Err(ReadFailure::Gap(ReviewGap::SupportLimit));
        }
        let reason = classify_text(text);
        if reason == TextReason::PrivacyBlocked || reason == TextReason::UnsupportedText {
            self.gaps.insert(if reason == TextReason::PrivacyBlocked {
                ReviewGap::PrivacyBlocked
            } else {
                ReviewGap::UnsupportedText
            });
            return Ok(TextView {
                original_field_sha256: None,
                original_bytes: None,
                display: None,
                display_sha256: None,
                reason,
            });
        }
        let original_digest = hash(text.as_bytes());
        let exact = self.detail == ReviewDetail::Exact;
        Ok(TextView {
            original_field_sha256: Some(original_digest.clone()),
            original_bytes: Some(text.len()),
            display: exact.then(|| text.to_owned()),
            display_sha256: exact.then_some(original_digest),
            reason: if exact {
                TextReason::Exact
            } else {
                TextReason::Metadata
            },
        })
    }
    fn texts(&mut self, values: &[String]) -> ReadResult<Vec<TextView>> {
        values.iter().map(|v| self.text(v)).collect()
    }
    fn skill(&mut self, skill: &SkillSnapshot) -> ReadResult<SkillView> {
        skill_text_limits(skill)?;
        skill.validate().map_err(material_error)?;
        Ok(SkillView {
            content: self.text(&skill.content)?,
            applicability: self.text(&skill.applicability)?,
            counterexample: self.text(&skill.counterexample)?,
            required_capabilities: self.texts(&skill.required_capabilities)?,
            dependencies: self.texts(&skill.dependencies)?,
        })
    }
    fn strategy(&mut self, strategy: &Strategy) -> ReadResult<StrategyView> {
        strategy.validate().map_err(material_error)?;
        Ok(StrategyView {
            schema_version: "evo.strategy.v1",
            instruction: self.text(&strategy.instruction)?,
            max_candidates: strategy.max_candidates,
            max_rounds: strategy.max_rounds,
            require_counterexample: strategy.require_counterexample,
        })
    }
    fn source(&mut self, source: &TypedSourceRef) -> ReadResult<SourceView> {
        digest(&source.content_digest)?;
        Ok(SourceView {
            kind: "run",
            id: self.text(&source.id)?,
            content_digest: source.content_digest.clone(),
        })
    }
}

fn classify_text(text: &str) -> TextReason {
    let lower = text.to_ascii_lowercase();
    const MARKERS: &[&str] = &[
        "begin private key",
        "begin rsa private key",
        "begin openssh private key",
        "begin ec private key",
        "begin dsa private key",
        "begin pgp private key block",
        "sk-",
        "ghp_",
        "gho_",
        "ghs_",
        "xoxb-",
        "xoxp-",
        "aiza",
        "/users/",
        "/home/",
        "c:\\users\\",
        "c:/users/",
        "answer:",
        "ground_truth:",
        "hidden_eval:",
        "oracle_eval:",
        "oracle_solution:",
        "192.168.",
        "10.0.",
        "10.1.",
        "172.16.",
        ".corp",
        ".internal",
        ".local",
        "chat_history",
        "password=",
        "api_secret=",
        "client_secret=",
        "secret_key=",
        "aws_secret_access_key",
        "todo: secret",
        "fixme: token",
        "file:",
    ];
    let compact: String = lower.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let bytes = text.as_bytes();
    let absolute_path = bytes.iter().enumerate().any(|(i, b)| {
        (*b == b'/'
            && (i == 0
                || !bytes[i - 1].is_ascii()
                || bytes[i - 1].is_ascii_whitespace()
                || b"\"'([{=:".contains(&bytes[i - 1])))
            || (*b == b':'
                && i > 0
                && bytes[i - 1].is_ascii_alphabetic()
                && bytes.get(i + 1).is_some_and(|b| matches!(b, b'/' | b'\\')))
    }) || text.contains("\\\\");
    if text.chars().any(char::is_control)
        || absolute_path
        || MARKERS.iter().any(|m| lower.contains(m))
        || compact.contains("\"role\":\"user\"")
        || compact.contains("\"role\":\"assistant\"")
    {
        TextReason::PrivacyBlocked
    } else if text
        .chars()
        .any(|c| matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}'))
    {
        TextReason::UnsupportedText
    } else {
        TextReason::Exact
    }
}

fn safe_skill(skill: &SkillSnapshot) -> bool {
    [&skill.content, &skill.applicability, &skill.counterexample]
        .into_iter()
        .chain(skill.required_capabilities.iter())
        .chain(skill.dependencies.iter())
        .all(|s| classify_text(s) == TextReason::Exact)
}

fn watermark_view(value: Option<(i64, String)>) -> ReadResult<Option<WatermarkView>> {
    value
        .map(|(seq, digest_value)| {
            digest(&digest_value)?;
            Ok(WatermarkView {
                seq,
                digest: digest_value,
            })
        })
        .transpose()
}

async fn read_candidate(
    ctx: &Context,
    session: &mut Session,
    set: &mut ReadSet,
    projection: &mut Projection,
    id: &str,
) -> ReadResult<CandidateView> {
    let raw = capture(ctx, session, set, "artifact", id)
        .await
        .map_err(primary_error)?;
    if raw.get("schema_version").and_then(Value::as_str) != Some(RELEASE_CANDIDATE_SCHEMA) {
        return Err(ReadFailure::Unavailable);
    }
    let decoded: crate::release_store::ReleaseCandidateRecord = decode(&raw)?;
    if decoded.sources.len() > MAX_REVIEW_REFS {
        return Err(ReadFailure::Gap(ReviewGap::SupportLimit));
    }
    skill_text_limits(&decoded.bundle.skill)?;
    let candidate = ReleaseStore::read_review_candidate_in_session(ctx, session, id)
        .await
        .map_err(|error| match error {
            Error::NotFound | Error::Forbidden => ReadFailure::Unavailable,
            Error::Internal => ReadFailure::Internal,
            Error::Conflict(_) => ReadFailure::Gap(ReviewGap::SourceChanged),
            _ => ReadFailure::Gap(ReviewGap::MaterialInvalid),
        })?;
    for source in &candidate.sources {
        capture(ctx, session, set, "run", &source.id).await?;
        check_edge(ctx, session, set, "artifact", id, "run", &source.id).await?;
    }
    projection.gaps.extend([
        ReviewGap::TypedCandidateHistoryUnbound,
        ReviewGap::TypedcandidateformalUnbound,
        ReviewGap::ParentMaterialUnbound,
        ReviewGap::BaselineMaterialUnbound,
    ]);
    Ok(CandidateView {
        id: projection.text(&candidate.id)?,
        namespace: projection.text(ctx.namespace())?,
        proposer_actor: projection.text(&candidate.proposer_actor)?,
        environment_digest: candidate.environment_digest,
        bundle: project_bundle(projection, &candidate.bundle)?,
        sources: candidate
            .sources
            .iter()
            .map(|s| projection.source(s))
            .collect::<ReadResult<_>>()?,
        revoke_watermark: watermark_view(session.watermark(ctx).await.map_err(storage_error)?)?,
    })
}

fn primary_error(error: ReadFailure) -> ReadFailure {
    match error {
        ReadFailure::Gap(ReviewGap::ReferenceUnavailable) => ReadFailure::Unavailable,
        other => other,
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    schema_version: String,
    id: String,
    record_kind: String,
    payload: Value,
}

async fn envelope(
    ctx: &Context,
    session: &mut Session,
    set: &mut ReadSet,
    kind: &str,
    id: &str,
) -> ReadResult<Envelope> {
    let storage_id = format!("e05-{}", fingerprint(&(kind, id)).map_err(material_error)?);
    let value = capture(ctx, session, set, "artifact", &storage_id).await?;
    let result: Envelope = decode(&value)?;
    if result.schema_version != "rsia.typed_artifact_envelope.v1"
        || result.id != storage_id
        || result.record_kind != kind
    {
        return Err(ReadFailure::Gap(ReviewGap::ReferenceUnavailable));
    }
    Ok(result)
}

async fn read_formal(
    ctx: &Context,
    session: &mut Session,
    set: &mut ReadSet,
    projection: &mut Projection,
    id: &str,
) -> ReadResult<FormalView> {
    envelope(ctx, session, set, "formal_evaluation_v2", id)
        .await
        .map_err(primary_error)?;
    let ticket: EvaluationTicketV2 = decode(
        &envelope(ctx, session, set, "evaluation_ticket_v2", id)
            .await?
            .payload,
    )?;
    if ticket
        .targets
        .len()
        .saturating_add(ticket.late_execution_receipt_ids.len())
        .saturating_add(ticket.late_grader_receipt_ids.len())
        > MAX_REVIEW_REFS
    {
        return Err(ReadFailure::Gap(ReviewGap::SupportLimit));
    }
    unique_ids(&ticket.late_execution_receipt_ids)?;
    unique_ids(&ticket.late_grader_receipt_ids)?;
    for (kind, reference) in [
        (
            "registered_evaluation_control_v41",
            ticket.registration_id.as_str(),
        ),
        ("protected_holdout_v41", ticket.holdout_id.as_str()),
        ("evaluation_ticket_issue_receipt_v1", id),
    ] {
        envelope(ctx, session, set, kind, reference).await?;
    }
    for target in &ticket.targets {
        if let TargetState::Completed {
            candidate_receipt_id,
            baseline_receipt_id,
            grader_receipt_id,
            ..
        } = &target.state
        {
            for receipt_id in [candidate_receipt_id, baseline_receipt_id] {
                let receipt: ExecutionReceiptV2 = decode(
                    &envelope(ctx, session, set, "execution_receipt_v2", receipt_id)
                        .await?
                        .payload,
                )?;
                envelope(
                    ctx,
                    session,
                    set,
                    "evaluation_execution_output_v1",
                    &receipt.output_artifact_id,
                )
                .await?;
            }
            envelope(
                ctx,
                session,
                set,
                "independent_grader_receipt_v2",
                grader_receipt_id,
            )
            .await?;
        }
    }
    for receipt_id in &ticket.late_execution_receipt_ids {
        let receipt: ExecutionReceiptV2 = decode(
            &envelope(ctx, session, set, "execution_receipt_v2", receipt_id)
                .await?
                .payload,
        )?;
        envelope(
            ctx,
            session,
            set,
            "evaluation_execution_output_v1",
            &receipt.output_artifact_id,
        )
        .await?;
    }
    for receipt_id in &ticket.late_grader_receipt_ids {
        envelope(
            ctx,
            session,
            set,
            "independent_grader_receipt_v2",
            receipt_id,
        )
        .await?;
    }
    let view = verified_report_view_in_session(ctx, session, id)
        .await
        .map_err(|e| match e {
            Error::Internal => ReadFailure::Internal,
            Error::NotFound | Error::Forbidden => ReadFailure::Gap(ReviewGap::ReferenceUnavailable),
            _ => ReadFailure::Gap(ReviewGap::MaterialInvalid),
        })?;
    if view.variant == VerifiedReportVariant::CompleteBatch {
        envelope(ctx, session, set, "evaluation_resource_evidence_v1", id).await?;
    }
    let reasons = view
        .ineligibility_reasons
        .iter()
        .map(|r| match r.as_str() {
            "program_fixture_not_production_authority" => {
                Ok(FormalIneligibilityReason::ProgramFixtureNotProductionAuthority)
            }
            "independent_process_isolation_not_verified" => {
                Ok(FormalIneligibilityReason::IndependentProcessIsolationNotVerified)
            }
            "dependency_revocation_closure_unverified" => {
                Ok(FormalIneligibilityReason::DependencyRevocationClosureUnverified)
            }
            "report_variant_not_complete_batch" => {
                Ok(FormalIneligibilityReason::ReportVariantNotCompleteBatch)
            }
            "resource_evidence_absent" => Ok(FormalIneligibilityReason::ResourceEvidenceAbsent),
            "complete_optimization_cost_not_verified" => {
                Ok(FormalIneligibilityReason::CompleteOptimizationCostNotVerified)
            }
            "formal_verdict_not_approvable" => {
                Ok(FormalIneligibilityReason::FormalVerdictNotApprovable)
            }
            _ => Err(ReadFailure::Gap(ReviewGap::UnsupportedSchema)),
        })
        .collect::<ReadResult<Vec<_>>>()?;
    Ok(FormalView {
        report_id: projection.text(&view.report_id)?,
        report_digest: view.report_digest,
        variant: view.variant,
        verdict: view.verdict,
        ticket_id: projection.text(&view.ticket_id)?,
        ticket_digest: view.ticket_digest,
        v1_plan_snapshot_digest: view.v1_plan_snapshot_digest,
        formal_plan_digest: view.formal_plan_digest,
        manifest_digest: view.manifest_digest,
        candidate_digest: view.candidate_digest,
        baseline_parent_digest: view.baseline_parent_digest,
        candidate_bundle_digest: view.candidate_bundle_digest,
        baseline_bundle_digest: view.baseline_bundle_digest,
        environment_digest: view.environment_digest,
        profile: view.profile,
        proposer_actor: projection.text(&view.proposer_actor)?,
        evaluator_actor: projection.text(&view.evaluator_actor)?,
        approver_actor: projection.text(&view.approver_actor)?,
        anchor_evidence_digest: view.anchor_evidence_digest,
        resource_evidence_digest: view.resource_evidence_digest,
        anchor_status: view.anchor_status,
        evidence_scope: view.evidence_scope,
        cost_evidence_scope: view.cost_evidence_scope,
        revoke_watermark: watermark_view(view.revoke_watermark)?,
        dependency_status: view.dependency_status,
        promotion_eligible: view.promotion_eligible,
        ineligibility_reasons: reasons,
    })
}

fn unique_ids(values: &[String]) -> ReadResult<()> {
    if values.len() > MAX_REVIEW_REFS {
        return Err(ReadFailure::Gap(ReviewGap::SupportLimit));
    }
    let mut seen = BTreeSet::new();
    for value in values {
        identifier(value).map_err(material_error)?;
        if !seen.insert(value) {
            return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
        }
    }
    Ok(())
}

fn project_bundle(projection: &mut Projection, bundle: &ResolvedBundle) -> ReadResult<BundleView> {
    if bundle.schema_version != BUNDLE_SCHEMA {
        return Err(ReadFailure::Gap(ReviewGap::UnsupportedSchema));
    }
    for value in [
        &bundle.digest,
        &bundle.parent_digest,
        &bundle.baseline_digest,
    ] {
        digest(value)?;
    }
    identifier(&bundle.profile_id).map_err(material_error)?;
    crate::releases::validate_resolved_bundle_identity(bundle).map_err(material_error)?;
    bundle.skill.validate().map_err(material_error)?;
    bundle.improver.validate().map_err(material_error)?;
    if bundle.origins.len() != 8 {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    let mut seen = BTreeSet::new();
    let mut origins = Vec::new();
    for origin in &bundle.origins {
        let (field, actual_digest, safe) = match origin.field.as_str() {
            "skill.content" => (
                OriginField::Content,
                fingerprint(&bundle.skill.content),
                classify_text(&bundle.skill.content) == TextReason::Exact,
            ),
            "skill.applicability" => (
                OriginField::Applicability,
                fingerprint(&bundle.skill.applicability),
                classify_text(&bundle.skill.applicability) == TextReason::Exact,
            ),
            "skill.counterexample" => (
                OriginField::Counterexample,
                fingerprint(&bundle.skill.counterexample),
                classify_text(&bundle.skill.counterexample) == TextReason::Exact,
            ),
            "skill.required_capabilities" => (
                OriginField::RequiredCapabilities,
                fingerprint(&bundle.skill.required_capabilities),
                bundle
                    .skill
                    .required_capabilities
                    .iter()
                    .all(|v| classify_text(v) == TextReason::Exact),
            ),
            "skill.dependencies" => (
                OriginField::Dependencies,
                fingerprint(&bundle.skill.dependencies),
                bundle
                    .skill
                    .dependencies
                    .iter()
                    .all(|v| classify_text(v) == TextReason::Exact),
            ),
            "improver.instruction" => (
                OriginField::Instruction,
                fingerprint(&bundle.improver.instruction),
                classify_text(&bundle.improver.instruction) == TextReason::Exact,
            ),
            "improver.max_candidates" => (
                OriginField::MaxCandidates,
                fingerprint(&bundle.improver.max_candidates),
                true,
            ),
            "improver.max_rounds" => (
                OriginField::MaxRounds,
                fingerprint(&bundle.improver.max_rounds),
                true,
            ),
            _ => return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid)),
        };
        if !seen.insert(field) || actual_digest.map_err(material_error)? != origin.value_digest {
            return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
        }
        let (operation, expected_source, source_digest) = match origin.layer {
            OriginLayer::Parent => (
                OriginOperation::Inherit,
                bundle.parent_digest.as_str(),
                Some(bundle.parent_digest.clone()),
            ),
            OriginLayer::Baseline => (
                OriginOperation::ResetToBaseline,
                bundle.baseline_digest.as_str(),
                Some(bundle.baseline_digest.clone()),
            ),
            OriginLayer::Explicit => (OriginOperation::Set, "set", None),
        };
        let expected_operation = match operation {
            OriginOperation::Inherit => "inherit",
            OriginOperation::ResetToBaseline => "reset_to_baseline",
            OriginOperation::Set => "set",
        };
        if origin.source_digest != expected_source || origin.op != expected_operation {
            return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
        }
        origins.push(OriginView {
            field,
            layer: origin.layer,
            operation,
            source_digest,
            value_digest: safe.then(|| origin.value_digest.clone()),
        });
    }
    Ok(BundleView {
        schema_version: BUNDLE_SCHEMA,
        profile_id: projection.text(&bundle.profile_id)?,
        bundle_digest: bundle.digest.clone(),
        declared_parent_digest: bundle.parent_digest.clone(),
        declared_baseline_digest: bundle.baseline_digest.clone(),
        skill_snapshot_digest: safe_skill(&bundle.skill)
            .then(|| skill_snapshot_digest(&bundle.skill))
            .transpose()
            .map_err(material_error)?,
        skill: projection.skill(&bundle.skill)?,
        improver: projection.strategy(&bundle.improver)?,
        origins,
    })
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PreparedMaterial {
    evidence: EvidenceSet,
    selection: SourceSelection,
    bindings: Vec<(String, String, String)>,
    traces: Vec<OptimizationTrace>,
    context: ModelRequestContext,
    parent: SkillSnapshot,
    trusted_edit_context: String,
    edit_template: SkillEditBatch,
    protected_ranges: String,
    profile: Profile,
    baseline: SkillSnapshot,
    parent_strategy: Strategy,
    baseline_strategy: Strategy,
    improver: ImproverPatch,
    caps: HostCapabilities,
    revoked: BTreeSet<String>,
    development: DevelopmentRunRequest,
    allow_rank: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CompiledMaterial {
    output: SkillSnapshot,
    patch: SkillPatch,
    report: EditApplyReport,
    bundle: ResolvedBundle,
    merge: MergeRecord,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CandidateMaterial {
    status: CandidateStatus,
    output: SkillSnapshot,
    patch: SkillPatch,
    report: EditApplyReport,
    bundle: ResolvedBundle,
    selection: DevelopmentSelection,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum CandidateStatus {
    Candidate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum TerminalStatus {
    Rejected,
    NoChange,
    Uncertain,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TerminalMaterial {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<TerminalStatus>,
    reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    class: Option<StepTerminalClass>,
}

fn terminal_class(fact: &StageFact) -> ReadResult<Option<&'static str>> {
    let terminal = matches!(
        fact.kind,
        StageFactKind::TerminalRejected
            | StageFactKind::TerminalNoChange
            | StageFactKind::TerminalUncertain
    ) || (fact.kind == StageFactKind::StepCompleted
        && fact.payload.get("status").and_then(Value::as_str) != Some("candidate"));
    if !terminal {
        return Ok(None);
    }
    let value: TerminalMaterial = decode(&fact.payload)?;
    let expected = match fact.kind {
        StageFactKind::TerminalRejected => Some(TerminalStatus::Rejected),
        StageFactKind::TerminalNoChange => Some(TerminalStatus::NoChange),
        StageFactKind::TerminalUncertain => Some(TerminalStatus::Uncertain),
        StageFactKind::StepCompleted => value.status,
        _ => None,
    }
    .ok_or(ReadFailure::Gap(ReviewGap::MaterialInvalid))?;
    if fact.kind == StageFactKind::StepCompleted {
        if value.status != Some(expected) {
            return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
        }
    } else if value.status.is_some()
        || fact.input_digest != fingerprint(&fact.payload).map_err(material_error)?
    {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    let class = value.class.unwrap_or(StepTerminalClass::LegacyUnclassified);
    let actual = match class {
        StepTerminalClass::NoEligibleReflectionBatch
        | StepTerminalClass::NoEditSuggestions
        | StepTerminalClass::EditProducedNoChange => Some(TerminalStatus::NoChange),
        StepTerminalClass::KeepIncumbentNotImproved { .. }
        | StepTerminalClass::KeepIncumbentRetentionBroken { .. }
        | StepTerminalClass::GrantUnavailable
        | StepTerminalClass::ModelRejected { .. }
        | StepTerminalClass::SuggestionShapeInvalid
        | StepTerminalClass::SuggestionPoolUnranked
        | StepTerminalClass::EditCompileFailed
        | StepTerminalClass::BundleCompileFailed
        | StepTerminalClass::DevelopmentReportRejected
        | StepTerminalClass::StepError { .. } => Some(TerminalStatus::Rejected),
        StepTerminalClass::ModelUsageUnknown
        | StepTerminalClass::ModelDispatchUnrecorded
        | StepTerminalClass::ModelDispatchConcurrent
        | StepTerminalClass::ModelTransportOutcomeUnknown
        | StepTerminalClass::DevelopmentExecutionUnrecorded
        | StepTerminalClass::DevelopmentExecutionConcurrent
        | StepTerminalClass::DevelopmentExecutionOutcomeUnknown
        | StepTerminalClass::CancelledOrUncertain
        | StepTerminalClass::OptimizationDispatchOutcomeUncertain
        | StepTerminalClass::ExplorationSourceClosureChangedAfterDispatch
        | StepTerminalClass::ExplorationPrefixChangedAfterDispatch
        | StepTerminalClass::ClaimInputsChangedAfterDispatch
        | StepTerminalClass::RecoverTargetMissing
        | StepTerminalClass::DevelopmentEvidenceUnverified
        | StepTerminalClass::CandidateMaterialUnavailable => Some(TerminalStatus::Uncertain),
        StepTerminalClass::LegacyUnclassified => None,
        _ => return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid)),
    };
    if actual.is_some_and(|actual| actual != expected)
        || (value.class.is_some() && value.reason != class.code())
    {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    Ok(Some(class.code()))
}

fn allowed_fact(kind: StageFactKind) -> bool {
    matches!(
        kind,
        StageFactKind::StepPrepared
            | StageFactKind::EditCompiled
            | StageFactKind::TerminalCandidate
            | StageFactKind::StepCompleted
            | StageFactKind::TerminalRejected
            | StageFactKind::TerminalNoChange
            | StageFactKind::TerminalUncertain
    )
}

fn validate_fact(fact: &StageFact, ctx: &Context, root: &StageFact) -> ReadResult<()> {
    fact.validate().map_err(material_error)?;
    if fact.namespace != ctx.namespace()
        || fact.namespace != root.namespace
        || fact.episode_id != root.episode_id
        || fact.step != root.step
        || fact.attempt != root.attempt
        || (allowed_fact(fact.kind) && fact.request_id != root.request_id)
    {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    let stage_matches = match fact.kind {
        StageFactKind::StepPrepared | StageFactKind::StepCompleted => {
            fact.stage == OptimizationJournalStage::Merge
        }
        StageFactKind::EditCompiled => fact.stage == OptimizationJournalStage::EditCompile,
        StageFactKind::TerminalCandidate
        | StageFactKind::TerminalNoChange
        | StageFactKind::TerminalRejected
        | StageFactKind::TerminalUncertain
        | StageFactKind::DevelopmentRequestPrepared
        | StageFactKind::DevelopmentObserved => fact.stage == OptimizationJournalStage::Development,
        _ => !matches!(
            fact.stage,
            OptimizationJournalStage::EditCompile | OptimizationJournalStage::Development
        ),
    };
    if !stage_matches {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    if fact.dependencies.len() > MAX_REVIEW_REFS {
        return Err(ReadFailure::Gap(ReviewGap::SupportLimit));
    }
    let unique: BTreeSet<_> = fact.dependencies.iter().map(|d| (&d.kind, &d.id)).collect();
    if unique.len() != fact.dependencies.len() {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    Ok(())
}

async fn read_stage(
    ctx: &Context,
    session: &mut Session,
    set: &mut ReadSet,
    projection: &mut Projection,
    id: &str,
) -> ReadResult<(StageView, bool)> {
    let raw = capture(ctx, session, set, "artifact", id)
        .await
        .map_err(primary_error)?;
    if raw.get("schema_version").and_then(Value::as_str)
        != Some(crate::optimization::OPTIMIZATION_STAGE_FACT_SCHEMA)
    {
        return Err(ReadFailure::Unavailable);
    }
    let root: StageFact = decode(&raw)?;
    if !allowed_fact(root.kind) {
        return Err(ReadFailure::Unavailable);
    }
    if root.artifact_id != id {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    let mut queue = VecDeque::from([id.to_owned()]);
    let mut queued = BTreeSet::from([id.to_owned()]);
    let mut facts = BTreeMap::new();
    let mut source_ids = BTreeSet::new();
    while let Some(fact_id) = queue.pop_front() {
        if facts.len() >= MAX_REVIEW_FACTS {
            return Err(ReadFailure::Gap(ReviewGap::SupportLimit));
        }
        let value = capture(ctx, session, set, "artifact", &fact_id).await?;
        let fact: StageFact = decode(&value)?;
        validate_fact(&fact, ctx, &root)?;
        if fact.artifact_id != fact_id {
            return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
        }
        let watermarks: Vec<_> = fact
            .dependencies
            .iter()
            .filter(|d| d.kind == "revoke_watermark")
            .collect();
        if watermarks.len() > 1 {
            return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
        }
        for dependency in &fact.dependencies {
            check_edge(
                ctx,
                session,
                set,
                "artifact",
                &fact_id,
                &dependency.kind,
                &dependency.id,
            )
            .await?;
            match dependency.kind.as_str() {
                "run" => {
                    source_ids.insert(dependency.id.clone());
                    capture(ctx, session, set, "run", &dependency.id).await?;
                    load_stored_source(session, ctx, &dependency.id)
                        .await
                        .map_err(|e| match e {
                            Error::Internal => ReadFailure::Internal,
                            _ => ReadFailure::Gap(ReviewGap::SourceChanged),
                        })?;
                }
                "artifact" => {
                    let reference = capture(ctx, session, set, "artifact", &dependency.id).await?;
                    if reference.get("schema_version").and_then(Value::as_str)
                        == Some(crate::optimization::OPTIMIZATION_STAGE_FACT_SCHEMA)
                        && queued.insert(dependency.id.clone())
                    {
                        queue.push_back(dependency.id.clone());
                    }
                }
                "revoke_watermark" => {
                    let expected = dependency
                        .id
                        .parse::<u64>()
                        .map_err(|_| ReadFailure::Gap(ReviewGap::MaterialInvalid))?;
                    if dependency.id != expected.to_string() {
                        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
                    }
                    let ids: Vec<_> = fact
                        .dependencies
                        .iter()
                        .filter(|d| d.kind == "run")
                        .map(|d| d.id.clone())
                        .collect();
                    validate_stored_sources(session, ctx, &ids, expected)
                        .await
                        .map_err(|e| match e {
                            Error::Internal => ReadFailure::Internal,
                            _ => ReadFailure::Gap(ReviewGap::SourceChanged),
                        })?;
                }
                // These logical ids are verified by the existing observation gate,
                // whose real typed artifact dependencies are captured separately.
                "execution" | "grader" if fact.kind == StageFactKind::DevelopmentObserved => {}
                _ => return Err(ReadFailure::Gap(ReviewGap::ReferenceUnavailable)),
            }
        }
        terminal_class(&fact)?;
        facts.insert(fact_id, fact);
    }
    let prepared_facts: Vec<_> = facts
        .values()
        .filter(|f| f.kind == StageFactKind::StepPrepared)
        .collect();
    if prepared_facts.len() > 1 {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    let prepared = match prepared_facts.first() {
        Some(fact) => Some(read_prepared(ctx, session, set, &facts, fact).await?),
        None => None,
    };
    let mut view = StageView {
        facts: Vec::new(),
        sources: Vec::new(),
        revoke_watermark: watermark_view(session.watermark(ctx).await.map_err(storage_error)?)?,
        prepared: None,
        bundle: None,
        merge: None,
        diff: None,
        selection: None,
    };
    for fact in facts.values().filter(|f| allowed_fact(f.kind)) {
        view.facts.push(FactView {
            artifact_id: fact.artifact_id.clone(),
            namespace: projection.text(&fact.namespace)?,
            episode_id: projection.text(&fact.episode_id)?,
            step: fact.step,
            attempt: fact.attempt,
            stage: fact.stage,
            kind: fact.kind,
            request_id: projection.text(&fact.request_id)?,
            terminal_class_code: terminal_class(fact)?,
        });
    }
    for source_id in &source_ids {
        let authority = load_stored_source(session, ctx, source_id)
            .await
            .map_err(storage_error)?;
        view.sources.push(projection.source(&TypedSourceRef {
            kind: "run".into(),
            id: source_id.clone(),
            content_digest: authority.trace.source_digest,
        })?);
    }
    let mut complete = false;
    if let Some(prepared) = prepared {
        view.prepared = Some(PreparedView {
            profile_id: projection.text(&prepared.profile.id)?,
            declared_parent_digest: prepared.profile.parent_digest.clone(),
            declared_baseline_digest: prepared.profile.baseline_digest.clone(),
            parent: projection.skill(&prepared.parent)?,
            baseline: projection.skill(&prepared.baseline)?,
            parent_strategy: projection.strategy(&prepared.parent_strategy)?,
            baseline_strategy: projection.strategy(&prepared.baseline_strategy)?,
        });
        let compile_facts: Vec<_> = facts
            .values()
            .filter(|f| f.kind == StageFactKind::EditCompiled)
            .collect();
        if compile_facts.len() > 1 {
            return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
        }
        if let Some(compile_fact) = compile_facts.first() {
            let compiled: CompiledMaterial = decode(&compile_fact.payload)?;
            validate_compiled(&prepared, compile_fact, &compiled)?;
            view.bundle = Some(project_bundle(projection, &compiled.bundle)?);
            view.merge = Some(project_merge(projection, &prepared, &compiled.merge)?);
            let (diff, visible) = project_diff(projection, &prepared, &compiled)?;
            view.diff = Some(diff);
            complete = visible;
            for candidate in facts.values().filter(|f| {
                f.kind == StageFactKind::TerminalCandidate
                    || (f.kind == StageFactKind::StepCompleted
                        && f.payload.get("status").and_then(Value::as_str) == Some("candidate"))
            }) {
                let candidate_material: CandidateMaterial = decode(&candidate.payload)?;
                if fingerprint(&candidate_material.output).map_err(material_error)?
                    != fingerprint(&compiled.output).map_err(material_error)?
                    || fingerprint(&candidate_material.patch).map_err(material_error)?
                        != fingerprint(&compiled.patch).map_err(material_error)?
                    || candidate_material.report != compiled.report
                    || candidate_material.bundle.digest != compiled.bundle.digest
                    || fingerprint(&candidate_material.bundle).map_err(material_error)?
                        != fingerprint(&compiled.bundle).map_err(material_error)?
                {
                    return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
                }
                let expected_input = if candidate.kind == StageFactKind::StepCompleted {
                    prepared_facts[0].input_digest.clone()
                } else {
                    fingerprint(&prepared.context).map_err(material_error)?
                };
                if candidate.input_digest != expected_input {
                    return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
                }
                let selection = &candidate_material.selection;
                if selection.candidate_bundle_digest != compiled.bundle.digest
                    || selection.parent_bundle_digest != prepared.context.bundle_digest
                    || selection.manifest_digest != prepared.development.manifest.digest
                    || selection.decision != DevelopmentSelectionDecision::AcceptCandidate
                {
                    return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
                }
                let observed: Vec<_> = facts
                    .values()
                    .filter(|f| {
                        f.kind == StageFactKind::DevelopmentObserved
                            && f.request_id == selection.request_id
                    })
                    .collect();
                if observed.len() == 1 {
                    let observed = observed[0];
                    let request_id =
                        development_request_fact_id(observed).map_err(material_error)?;
                    capture(ctx, session, set, "artifact", &request_id).await?;
                    let verified = verified_development_observation_in_session(
                        ctx,
                        session,
                        &request_id,
                        &observed.artifact_id,
                    )
                    .await
                    .map_err(|e| match e {
                        Error::Internal => ReadFailure::Internal,
                        _ => ReadFailure::Gap(ReviewGap::ReferenceUnavailable),
                    })?;
                    if verified.episode_id != root.episode_id
                        || verified.step != root.step
                        || verified.attempt != root.attempt
                        || verified.request_id != selection.request_id
                        || verified.manifest_digest != selection.manifest_digest
                    {
                        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
                    }
                    let expected_ids = crate::optimization::development_stage_fact_ids(
                        &prepared.context,
                        &prepared.development,
                    )
                    .map_err(material_error)?;
                    if expected_ids != (request_id, observed.artifact_id.clone()) {
                        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
                    }
                    let request_fact: StageFact =
                        decode(&capture(ctx, session, set, "artifact", &expected_ids.0).await?)?;
                    let request: DevelopmentRunRequest = decode(&request_fact.payload)?;
                    let report: crate::optimization::DevelopmentRunReport =
                        decode(&observed.payload)?;
                    let actual = crate::optimization::select_development(&request, &report)
                        .map_err(material_error)?;
                    if fingerprint(&actual).map_err(material_error)?
                        != fingerprint(selection).map_err(material_error)?
                    {
                        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
                    }
                    view.selection = Some(SelectionView {
                        request_id: projection.text(&selection.request_id)?,
                        manifest_digest: selection.manifest_digest.clone(),
                        parent_bundle_digest: selection.parent_bundle_digest.clone(),
                        candidate_bundle_digest: selection.candidate_bundle_digest.clone(),
                        decision: selection.decision,
                        parent_total_micros: selection.parent_total_micros,
                        candidate_total_micros: selection.candidate_total_micros,
                        reason: projection.text(&selection.reason)?,
                    });
                } else {
                    projection.gaps.insert(ReviewGap::ReferenceUnavailable);
                    complete = false;
                }
            }
        }
        for fact in facts
            .values()
            .filter(|f| f.kind == StageFactKind::StepCompleted)
        {
            if fact.input_digest != prepared_facts[0].input_digest {
                return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
            }
        }
    } else {
        projection.gaps.extend([
            ReviewGap::ParentMaterialUnbound,
            ReviewGap::BaselineMaterialUnbound,
        ]);
    }
    complete &= projection.detail == ReviewDetail::Exact
        && !projection.gaps.contains(&ReviewGap::PrivacyBlocked)
        && !projection.gaps.contains(&ReviewGap::UnsupportedText);
    Ok((view, complete))
}

async fn read_prepared(
    ctx: &Context,
    session: &mut Session,
    set: &mut ReadSet,
    facts: &BTreeMap<String, StageFact>,
    fact: &StageFact,
) -> ReadResult<PreparedMaterial> {
    let material: PreparedMaterial = decode(&fact.payload)?;
    let context = &material.context;
    if fact.input_digest != fingerprint(&fact.payload).map_err(material_error)?
        || context.namespace != ctx.namespace()
        || context.namespace != fact.namespace
        || context.episode_id != fact.episode_id
        || context.step != fact.step
        || context.attempt != fact.attempt
        || context.request_id != fact.request_id
        || context.purpose != Purpose::Development
    {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    for values in [
        material.selection.run_ids.as_slice(),
        material.selection.roots.as_slice(),
    ] {
        if values.len() > MAX_REVIEW_REFS {
            return Err(ReadFailure::Gap(ReviewGap::SupportLimit));
        }
    }
    unique_ids(&material.selection.run_ids)?;
    if material.bindings.len() > MAX_REVIEW_REFS
        || material.traces.len() > MAX_REVIEW_REFS
        || context.source_closure.len() > MAX_REVIEW_REFS
        || material.evidence.members.len() > MAX_REVIEW_REFS
    {
        return Err(ReadFailure::Gap(ReviewGap::SupportLimit));
    }
    material.selection.validate().map_err(material_error)?;
    if material.selection.purpose != Purpose::Development
        || context.max_suggestions == 0
        || context.max_suggestions > 4
    {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    for value in [
        &material.profile.parent_digest,
        &material.profile.baseline_digest,
        &context.bundle_digest,
        &context.parent_skill_digest,
        &context.model_digest,
        &context.tools_digest,
        &context.rules_digest,
        &context.sampling_digest,
    ] {
        digest(value)?;
    }
    identifier(&material.profile.id).map_err(material_error)?;
    material.parent.validate().map_err(material_error)?;
    material.baseline.validate().map_err(material_error)?;
    material
        .parent_strategy
        .validate()
        .map_err(material_error)?;
    material
        .baseline_strategy
        .validate()
        .map_err(material_error)?;
    let template = &material.edit_template;
    if template.schema_version != SKILL_EDIT_SCHEMA
        || template.compiler_version != SKILL_EDIT_COMPILER_VERSION
        || template.namespace != ctx.namespace()
        || template.profile_id != material.profile.id
        || template.approved_parent_digest != material.profile.parent_digest
        || template.safe_baseline_digest != material.profile.baseline_digest
        || template.input_digest
            != skill_snapshot_digest(&material.parent).map_err(material_error)?
        || context.parent_skill_digest != template.input_digest
    {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    for value in [&template.skill_id, &template.skill_version] {
        identifier(value).map_err(material_error)?;
    }
    let development = &material.development;
    if development.namespace != context.namespace
        || development.episode_id != context.episode_id
        || development.step != context.step
        || development.attempt != context.attempt
        || development.revoke_watermark != context.revoke_watermark
        || development.parent_bundle_digest != context.bundle_digest
    {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    let grant_id = format!(
        "optgrant-{}",
        fingerprint(&material.selection).map_err(material_error)?
    );
    if !fact
        .dependencies
        .iter()
        .any(|d| d.kind == "artifact" && d.id == grant_id)
    {
        return Err(ReadFailure::Gap(ReviewGap::ReferenceUnavailable));
    }
    let grant: SourceSelection = decode(&capture(ctx, session, set, "artifact", &grant_id).await?)?;
    if fingerprint(&grant).map_err(material_error)?
        != fingerprint(&material.selection).map_err(material_error)?
    {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    let selected: BTreeSet<_> = material.selection.run_ids.iter().cloned().collect();
    let mut declared = BTreeMap::new();
    for reference in &context.source_closure {
        digest(&reference.digest)?;
        identifier(&reference.id).map_err(material_error)?;
        if declared
            .insert(reference.id.clone(), reference.digest.clone())
            .is_some()
        {
            return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
        }
    }
    if declared.keys().cloned().collect::<BTreeSet<_>>() != selected
        || material.bindings.len() != selected.len()
    {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    let mut bindings = BTreeMap::new();
    for (id, content, family) in &material.bindings {
        if bindings.insert(id.clone(), (content, family)).is_some() {
            return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
        }
    }
    let mut records = Vec::new();
    validate_stored_sources(
        session,
        ctx,
        &material.selection.run_ids,
        context.revoke_watermark,
    )
    .await
    .map_err(|e| match e {
        Error::Internal => ReadFailure::Internal,
        _ => ReadFailure::Gap(ReviewGap::SourceChanged),
    })?;
    for source_id in &material.selection.run_ids {
        capture(ctx, session, set, "run", source_id).await?;
        let authority = load_stored_source(session, ctx, source_id)
            .await
            .map_err(storage_error)?;
        if declared.get(source_id) != Some(&authority.trace.source_digest)
            || bindings.get(source_id).copied()
                != Some((
                    &authority.trace.source_digest,
                    &authority.record.parent_family,
                ))
        {
            return Err(ReadFailure::Gap(ReviewGap::SourceChanged));
        }
        for trace in material.traces.iter().filter(|t| &t.run_id == source_id) {
            if fingerprint(trace).map_err(material_error)?
                != fingerprint(&authority.trace).map_err(material_error)?
            {
                return Err(ReadFailure::Gap(ReviewGap::SourceChanged));
            }
        }
        records.push(authority.record);
    }
    if material
        .traces
        .iter()
        .any(|t| !selected.contains(&t.run_id))
    {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    let actual = crate::evidence::ingest_trusted_run_records(&material.selection, &records)
        .map_err(material_error)?;
    if fingerprint(&actual.members).map_err(material_error)?
        != fingerprint(&material.evidence.members).map_err(material_error)?
        || actual.independent_clusters != material.evidence.independent_clusters
        || fingerprint(&actual.coverage).map_err(material_error)?
            != fingerprint(&material.evidence.coverage).map_err(material_error)?
    {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    for other in facts.values() {
        let source_set: BTreeSet<_> = other
            .dependencies
            .iter()
            .filter(|d| d.kind == "run")
            .map(|d| d.id.clone())
            .collect();
        if source_set != selected
            || !other.dependencies.iter().any(|d| {
                d.kind == "revoke_watermark" && d.id == context.revoke_watermark.to_string()
            })
            || !other
                .dependencies
                .iter()
                .any(|d| d.kind == "artifact" && d.id == grant_id)
            || (other.artifact_id != fact.artifact_id
                && !other
                    .dependencies
                    .iter()
                    .any(|d| d.kind == "artifact" && d.id == fact.artifact_id))
        {
            return Err(ReadFailure::Gap(ReviewGap::ReferenceUnavailable));
        }
    }
    Ok(material)
}

fn validate_evidence(
    evidence: &EvidenceClosure,
    allowed: &BTreeMap<String, String>,
) -> ReadResult<()> {
    for references in [
        &evidence.support,
        &evidence.counterexamples,
        &evidence.dependencies,
    ] {
        if references.len() > MAX_REVIEW_REFS {
            return Err(ReadFailure::Gap(ReviewGap::SupportLimit));
        }
        let mut ids = BTreeSet::new();
        for reference in references {
            if !ids.insert(&reference.id) || allowed.get(&reference.id) != Some(&reference.digest) {
                return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
            }
        }
    }
    let dependencies: BTreeSet<_> = evidence.dependencies.iter().collect();
    if evidence
        .support
        .iter()
        .chain(&evidence.counterexamples)
        .any(|reference| !dependencies.contains(reference))
    {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    Ok(())
}

fn validate_compiled(
    prepared: &PreparedMaterial,
    fact: &StageFact,
    compiled: &CompiledMaterial,
) -> ReadResult<()> {
    unique_ids(&compiled.merge.input_suggestion_ids)?;
    unique_ids(&compiled.merge.selected_suggestion_ids)?;
    let input: BTreeSet<_> = compiled.merge.input_suggestion_ids.iter().collect();
    if compiled.merge.selected_suggestion_ids.is_empty()
        || compiled.merge.selected_suggestion_ids.len() > MAX_EDITS_PER_BATCH
        || compiled
            .merge
            .selected_suggestion_ids
            .iter()
            .any(|id| !input.contains(id))
        || compiled.merge.edits.len() != compiled.merge.selected_suggestion_ids.len()
        || compiled.merge.read_dependencies.len() > MAX_REVIEW_REFS
    {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    let allowed: BTreeMap<_, _> = prepared
        .context
        .source_closure
        .iter()
        .map(|r| (r.id.clone(), r.digest.clone()))
        .collect();
    let mut read_ids = BTreeSet::new();
    for reference in &compiled.merge.read_dependencies {
        if allowed.get(&reference.id) != Some(&reference.digest) || !read_ids.insert(&reference.id)
        {
            return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
        }
    }
    validate_evidence(&compiled.merge.evidence, &allowed)?;
    let merged_dependencies: BTreeSet<_> = compiled.merge.evidence.dependencies.iter().collect();
    if merged_dependencies != compiled.merge.read_dependencies.iter().collect() {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    let mut batch = prepared.edit_template.clone();
    batch.evidence = compiled.merge.evidence.clone();
    batch.edits = compiled.merge.edits.clone();
    if fact.input_digest != fingerprint(&batch).map_err(material_error)? {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    let report = &compiled.report;
    if report.schema_version != SKILL_EDIT_REPORT_SCHEMA
        || report.compiler_version != SKILL_EDIT_COMPILER_VERSION
        || report.namespace != batch.namespace
        || report.profile_id != batch.profile_id
        || report.skill_id != batch.skill_id
        || report.skill_version != batch.skill_version
        || report.approved_parent_digest != batch.approved_parent_digest
        || report.safe_baseline_digest != batch.safe_baseline_digest
        || report.input_digest != skill_snapshot_digest(&prepared.parent).map_err(material_error)?
        || report.output_digest
            != skill_snapshot_digest(&compiled.output).map_err(material_error)?
        || report.edits.len() != batch.edits.len()
        || report.edits.is_empty()
        || report.edits.len() > MAX_EDITS_PER_BATCH
    {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    let mut canonical_evidence = batch.evidence.clone();
    canonical_evidence.support.sort();
    canonical_evidence.counterexamples.sort();
    canonical_evidence.dependencies.sort();
    if report.evidence != canonical_evidence {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    let mut changed = 0usize;
    for (edit, applied) in batch.edits.iter().zip(&report.edits) {
        let source = skill_field(&prepared.parent, edit.field);
        let removed = source
            .get(edit.start..edit.end)
            .ok_or(ReadFailure::Gap(ReviewGap::MaterialInvalid))?;
        let inserted = match &edit.operation {
            TextEditOperation::Insert { text } if edit.start == edit.end && !text.is_empty() => {
                text.as_str()
            }
            TextEditOperation::Replace { text } if edit.start < edit.end && !text.is_empty() => {
                text.as_str()
            }
            TextEditOperation::Delete if edit.start < edit.end => "",
            _ => return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid)),
        };
        if inserted.contains('\0')
            || hash(removed.as_bytes()) != edit.expected_text_digest
            || applied.field != edit.field
            || applied.start != edit.start
            || applied.end != edit.end
            || applied.removed_text != removed
            || applied.inserted_text != inserted
            || applied.removed_bytes != removed.len()
            || applied.inserted_bytes != inserted.len()
            || applied.removed_digest != hash(removed.as_bytes())
            || applied.inserted_digest != hash(inserted.as_bytes())
        {
            return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
        }
        if let Some(anchor) = &edit.exact_anchor {
            if anchor.text.is_empty() || anchor.text.len() > source.len() {
                return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
            }
            let positions: Vec<_> = source
                .as_bytes()
                .windows(anchor.text.len())
                .enumerate()
                .filter(|(_, window)| *window == anchor.text.as_bytes())
                .map(|(position, _)| position)
                .collect();
            if positions.len() != 1
                || edit.start < positions[0]
                || edit.end > positions[0].saturating_add(anchor.text.len())
            {
                return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
            }
        }
        changed = changed
            .checked_add(removed.len())
            .and_then(|v| v.checked_add(inserted.len()))
            .ok_or(ReadFailure::Gap(ReviewGap::MaterialInvalid))?;
    }
    if changed > MAX_CHANGED_BYTES || changed != report.changed_bytes {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    for (index, left) in batch.edits.iter().enumerate() {
        for right in batch.edits.iter().skip(index + 1) {
            let conflict = match (left.start == left.end, right.start == right.end) {
                (true, true) => left.start == right.start,
                (true, false) => right.start <= left.start && left.start <= right.end,
                (false, true) => left.start <= right.start && right.start <= left.end,
                (false, false) => left.start < right.end && right.start < left.end,
            };
            if left.field == right.field && conflict {
                return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
            }
        }
    }
    // Apply the stored, already-validated spans in descending ORIGINAL coordinates.
    // This proves bytes only; it invokes no compiler or authorization transition.
    let mut reconstructed = prepared.parent.clone();
    for field in [
        SkillTextField::Content,
        SkillTextField::Applicability,
        SkillTextField::Counterexample,
    ] {
        let mut edits: Vec<_> = report.edits.iter().filter(|e| e.field == field).collect();
        edits.sort_by_key(|e| (e.start, e.end));
        for edit in edits.into_iter().rev() {
            skill_field_mut(&mut reconstructed, field)
                .replace_range(edit.start..edit.end, &edit.inserted_text);
        }
    }
    if fingerprint(&reconstructed).map_err(material_error)?
        != fingerprint(&compiled.output).map_err(material_error)?
    {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    let actual_change = report.input_digest != report.output_digest;
    if (report.status == EditApplyStatus::Applied) != actual_change {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    let mut expected_patch = SkillPatch::default();
    for field in [
        SkillTextField::Content,
        SkillTextField::Applicability,
        SkillTextField::Counterexample,
    ] {
        if skill_field(&prepared.parent, field) != skill_field(&compiled.output, field) {
            let operation = PatchOp::Set {
                value: skill_field(&compiled.output, field).to_owned(),
            };
            match field {
                SkillTextField::Content => expected_patch.content = operation,
                SkillTextField::Applicability => expected_patch.applicability = operation,
                SkillTextField::Counterexample => expected_patch.counterexample = operation,
            }
        }
    }
    if fingerprint(&expected_patch).map_err(material_error)?
        != fingerprint(&compiled.patch).map_err(material_error)?
        || compiled.bundle.profile_id != prepared.profile.id
        || compiled.bundle.parent_digest != prepared.profile.parent_digest
        || compiled.bundle.baseline_digest != prepared.profile.baseline_digest
        || fingerprint(&compiled.bundle.skill).map_err(material_error)?
            != fingerprint(&compiled.output).map_err(material_error)?
    {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    let expected_strategy = Strategy {
        schema_version: prepared.parent_strategy.schema_version.clone(),
        instruction: prepared.improver.instruction.apply(
            &prepared.parent_strategy.instruction,
            &prepared.baseline_strategy.instruction,
        ),
        max_candidates: prepared.improver.max_candidates.apply(
            &prepared.parent_strategy.max_candidates,
            &prepared.baseline_strategy.max_candidates,
        ),
        max_rounds: prepared.improver.max_rounds.apply(
            &prepared.parent_strategy.max_rounds,
            &prepared.baseline_strategy.max_rounds,
        ),
        require_counterexample: prepared.parent_strategy.require_counterexample,
    };
    if fingerprint(&expected_strategy).map_err(material_error)?
        != fingerprint(&compiled.bundle.improver).map_err(material_error)?
    {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    for origin in &compiled.bundle.origins {
        let expected = match origin.field.as_str() {
            "skill.content" => compiled.patch.content.name(),
            "skill.applicability" => compiled.patch.applicability.name(),
            "skill.counterexample" => compiled.patch.counterexample.name(),
            "skill.required_capabilities" => compiled.patch.required_capabilities.name(),
            "skill.dependencies" => compiled.patch.dependencies.name(),
            "improver.instruction" => prepared.improver.instruction.name(),
            "improver.max_candidates" => prepared.improver.max_candidates.name(),
            "improver.max_rounds" => prepared.improver.max_rounds.name(),
            _ => return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid)),
        };
        if origin.op != expected {
            return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
        }
    }
    if !prepared.caps.allows(&compiled.output.required_capabilities)
        || compiled
            .output
            .dependencies
            .iter()
            .any(|d| prepared.revoked.contains(d))
    {
        return Err(ReadFailure::Gap(ReviewGap::MaterialInvalid));
    }
    Ok(())
}

fn skill_field(skill: &SkillSnapshot, field: SkillTextField) -> &str {
    match field {
        SkillTextField::Content => &skill.content,
        SkillTextField::Applicability => &skill.applicability,
        SkillTextField::Counterexample => &skill.counterexample,
    }
}
fn skill_field_mut(skill: &mut SkillSnapshot, field: SkillTextField) -> &mut String {
    match field {
        SkillTextField::Content => &mut skill.content,
        SkillTextField::Applicability => &mut skill.applicability,
        SkillTextField::Counterexample => &mut skill.counterexample,
    }
}

fn project_merge(
    projection: &mut Projection,
    prepared: &PreparedMaterial,
    merge: &MergeRecord,
) -> ReadResult<MergeView> {
    let selected: BTreeSet<_> = merge.selected_suggestion_ids.iter().collect();
    let unselected: Vec<_> = merge
        .input_suggestion_ids
        .iter()
        .filter(|id| !selected.contains(id))
        .cloned()
        .collect();
    if !unselected.is_empty() {
        projection.gaps.insert(ReviewGap::MissingRejectionReason);
    }
    let allowed: BTreeSet<_> = prepared.context.source_closure.iter().collect();
    if merge.read_dependencies.iter().any(|r| !allowed.contains(r)) {
        return Err(ReadFailure::Gap(ReviewGap::ReferenceUnavailable));
    }
    Ok(MergeView {
        input_suggestion_ids: projection.texts(&merge.input_suggestion_ids)?,
        selected_suggestion_ids: projection.texts(&merge.selected_suggestion_ids)?,
        unselected_suggestion_ids: projection.texts(&unselected)?,
        read_dependencies: merge
            .read_dependencies
            .iter()
            .map(|r| {
                projection.source(&TypedSourceRef {
                    kind: "run".into(),
                    id: r.id.clone(),
                    content_digest: r.digest.clone(),
                })
            })
            .collect::<ReadResult<_>>()?,
    })
}

fn masked_text(reason: TextReason) -> TextView {
    TextView {
        original_field_sha256: None,
        original_bytes: None,
        display: None,
        display_sha256: None,
        reason,
    }
}

fn project_diff(
    projection: &mut Projection,
    prepared: &PreparedMaterial,
    compiled: &CompiledMaterial,
) -> ReadResult<(DiffView, bool)> {
    let mut edits = Vec::new();
    let mut visible = projection.detail == ReviewDetail::Exact
        && safe_skill(&prepared.parent)
        && safe_skill(&prepared.baseline)
        && safe_skill(&compiled.output)
        && [
            &prepared.parent_strategy.instruction,
            &prepared.baseline_strategy.instruction,
            &compiled.bundle.improver.instruction,
        ]
        .into_iter()
        .all(|s| classify_text(s) == TextReason::Exact);
    for edit in &compiled.report.edits {
        let reasons = [
            skill_field(&prepared.parent, edit.field),
            skill_field(&prepared.baseline, edit.field),
            skill_field(&compiled.output, edit.field),
        ]
        .map(classify_text);
        let blocked = if reasons.contains(&TextReason::PrivacyBlocked) {
            Some(TextReason::PrivacyBlocked)
        } else if reasons.contains(&TextReason::UnsupportedText) {
            Some(TextReason::UnsupportedText)
        } else {
            None
        };
        let (removed, inserted, show_span) = match blocked {
            Some(reason) => {
                visible = false;
                projection
                    .gaps
                    .insert(if reason == TextReason::PrivacyBlocked {
                        ReviewGap::PrivacyBlocked
                    } else {
                        ReviewGap::UnsupportedText
                    });
                (masked_text(reason), masked_text(reason), false)
            }
            None => {
                let removed = projection.text(&edit.removed_text)?;
                let inserted = projection.text(&edit.inserted_text)?;
                let fragment_reason = if [removed.reason, inserted.reason]
                    .contains(&TextReason::PrivacyBlocked)
                {
                    Some(TextReason::PrivacyBlocked)
                } else if [removed.reason, inserted.reason].contains(&TextReason::UnsupportedText) {
                    Some(TextReason::UnsupportedText)
                } else {
                    None
                };
                if let Some(reason) = fragment_reason {
                    visible = false;
                    (masked_text(reason), masked_text(reason), false)
                } else {
                    (removed, inserted, projection.detail == ReviewDetail::Exact)
                }
            }
        };
        edits.push(EditView {
            field: edit.field,
            start: show_span.then_some(edit.start),
            end: show_span.then_some(edit.end),
            removed,
            inserted,
        });
    }
    Ok((
        DiffView {
            status: compiled.report.status,
            changed_bytes: visible.then_some(compiled.report.changed_bytes),
            edits,
        },
        visible,
    ))
}
