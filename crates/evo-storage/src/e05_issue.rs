//! Closed native E05 writers. Caller Values are intents, never phase authority.
use super::*;
use crate::budget::BudgetCallFence;
use crate::{NativePool, Session, SessionCommitFailure};
use evo_core::holdout::MonetaryReservationState;
use evo_core::sequential::{
    CompletePairedUnit, EarlyStopCertificateParts, EarlyStopDecision, SequentialRejectOnly,
};
use evo_core::{Context, Role};
use sqlx::{Connection, TransactionManager, sqlite::SqliteTransactionManager};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU8, Ordering},
};
use tokio::sync::oneshot;

#[cfg(test)]
mod native_early_cancellation_r44_tests {
    use super::*;
    #[tokio::test]
    async fn already_cancelled_grade_and_reconcile_never_create_stop_capsule_or_audit() {
        for intent in [
            NativeIntent::Grade(Value::Null),
            NativeIntent::Reconcile(Value::Null),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("cancelled-r44.sqlite3");
            let store = crate::Store::open(&path).await.unwrap();
            let context = Context::new("r44-cancel", "evaluator", Role::Evaluator).unwrap();
            let mut session = store.session().await.unwrap();
            let before = load_sqlx_database(session.tx.as_mut().unwrap())
                .await
                .unwrap();
            let operation =
                begin_native(session, context, intent, Arc::new(AtomicU8::new(CANCELLED)));
            let outcome = operation.finish_outcome().await;
            assert!(
                outcome.native.rollback_confirmed
                    && outcome.native.rollback_command_acknowledged
                    && !outcome.native.terminal_unconfirmed
            );
            assert!(!outcome.native.commit_attempted && !outcome.native.commit_confirmed);
            let failure = outcome.result.unwrap_err();
            assert!(matches!(failure.error, Error::Cancelled));
            assert!(!failure.commit_confirmed && !failure.commit_outcome_unknown);
            let mut session = store.session().await.unwrap();
            let after = load_sqlx_database(session.tx.as_mut().unwrap())
                .await
                .unwrap();
            same_database(&before, &after).unwrap();
            session.commit().await.unwrap();
            store.close().await;
            let reopened = crate::Store::open(&path).await.unwrap();
            reopened.session().await.unwrap().commit().await.unwrap();
            reopened.close().await;
        }
    }
}

const INTENT_SCHEMA: &str = "rsia.e05_issue_intent.v2";
const ACTIVE: u8 = 0;
const CANCELLED: u8 = 1;
const COMMIT_ADMITTED: u8 = 2;
const COMMIT_ATTEMPTED: u8 = 1;
const COMMIT_ACK: u8 = 2;
const ROLLBACK_COMMAND_ACK: u8 = 4;
const NATIVE_CLOSE_ACK: u8 = 8;
const TERMINAL_ACK: u8 = 16;
const ROLLBACK_CONFIRMED: u8 = 32;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IssueIntent {
    schema_version: String,
    ticket_id: String,
    registration_id: String,
    holdout_id: String,
}
impl IssueIntent {
    fn read(value: Value) -> Result<Self> {
        let intent: Self = serde_json::from_value(value).map_err(|_| {
            Error::Invalid("E05 issue requires an explicit v2 intent without caller time".into())
        })?;
        if intent.schema_version != INTENT_SCHEMA {
            return Err(Error::Invalid("unsupported E05 issue intent schema".into()));
        }
        for id in [
            &intent.ticket_id,
            &intent.registration_id,
            &intent.holdout_id,
        ] {
            identifier(id)?;
        }
        Ok(intent)
    }
}

// Reserved only for the closed native owner. Legacy audit operations retain
// their original names and readers; matching a string never creates a permit.
pub(crate) fn reserved_native_audit_operation(operation: &str) -> bool {
    matches!(
        operation,
        "formal.native.asof.issue.v1"
            | "formal.native.asof.execution.v1"
            | "formal.native.asof.grade.v1"
            | "formal.native.asof.complete.v1"
            | "formal.native.asof.reconcile.v1"
            | "formal.native.asof.invalidate.v1"
            | COMPLETE_BINDING_OPERATION_R24
            | native_formal_consumers_r49::NATIVE_RECORD_OPERATION_R49
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativePlanEffectR1 {
    NoNewFormal,
    NonFormalWrites,
    LegacyFormalWrites,
    NewComplete,
    NewInvalid,
    NewEarlyR44,
}
impl NativePlanEffectR1 {
    fn needs_versioned_insert(self) -> bool {
        matches!(
            self,
            Self::NewComplete | Self::NewInvalid | Self::NewEarlyR44
        )
    }
}
enum CompleteRequestWorkR1 {
    Complete {
        report: Box<MirrorBernsteinReportWire>,
        resource: MirrorResourceEvidenceRecord,
    },
    Invalid {
        formal: Box<MirrorFormalEvaluationV2>,
    },
    EarlyR44 {
        formal: Box<MirrorFormalEvaluationV2>,
    },
}
impl CompleteRequestWorkR1 {
    fn effect(&self) -> NativePlanEffectR1 {
        match self {
            Self::Complete { .. } => NativePlanEffectR1::NewComplete,
            Self::Invalid { .. } => NativePlanEffectR1::NewInvalid,
            Self::EarlyR44 { .. } => NativePlanEffectR1::NewEarlyR44,
        }
    }
}
#[derive(Clone, Copy)]
enum GradeFormalDestinationR1 {
    Legacy,
    VersionedCompleteRequestInvalid,
}
// Produced only after exact native plan/binding checks, before the commit CAS.
// Neither type is Clone/Serde/public. Serialized rows cannot construct them.
struct ReadyFormalBindingR1 {
    reference: evo_core::formal_version::ReportRef,
    asof_sequence: i64,
    binding_sequence: i64,
    binding_digest: String,
}
struct NativeReadyToCommitR1 {
    binding: Option<ReadyFormalBindingR1>,
}
struct NativeCommittedR1 {
    formal: Option<CommittedCompleteR24>,
}
fn carrier_effect_r1(
    operation: &evo_core::formal_version::Operation,
    formal: &MirrorFormalEvaluationV2,
) -> Result<NativePlanEffectR1> {
    use evo_core::formal_version::Operation;
    match (operation, formal) {
        (Operation::Complete, MirrorFormalEvaluationV2::CompleteBatch { .. }) => {
            Ok(NativePlanEffectR1::NewComplete)
        }
        (Operation::Invalidate, MirrorFormalEvaluationV2::Invalid { .. }) => {
            Ok(NativePlanEffectR1::NewInvalid)
        }
        (
            Operation::EarlyStop | Operation::Reconcile,
            MirrorFormalEvaluationV2::EarlyStopped { .. },
        ) => Ok(NativePlanEffectR1::NewEarlyR44),
        _ => Err(Error::ClockRegressed),
    }
}

enum NativeAsOfOperation {
    Issue,
    Execution,
    Grade,
    Complete,
    Reconcile,
    Invalidate,
}
impl NativeAsOfOperation {
    fn for_intent(intent: &NativeIntent) -> Result<Self> {
        Ok(match intent {
            NativeIntent::ConsumerR49(_) => return Err(Error::Forbidden),
            NativeIntent::Issue(_) => Self::Issue,
            NativeIntent::Execution(_) => Self::Execution,
            NativeIntent::Grade(_) => Self::Grade,
            NativeIntent::Complete(_) => Self::Complete,
            NativeIntent::Reconcile(_) => Self::Reconcile,
            NativeIntent::Invalidate(_) => Self::Invalidate,
        })
    }
    fn audit_operation(&self) -> &'static str {
        match self {
            Self::Issue => "formal.native.asof.issue.v1",
            Self::Execution => "formal.native.asof.execution.v1",
            Self::Grade => "formal.native.asof.grade.v1",
            Self::Complete => "formal.native.asof.complete.v1",
            Self::Reconcile => "formal.native.asof.reconcile.v1",
            Self::Invalidate => "formal.native.asof.invalidate.v1",
        }
    }
}

// Row hashes bind exact storage classes, column names, body bytes, order-free
// table contents and multiplicity. These are provenance, never a report ID.
fn native_database_digest(database: &RawDatabase, include_audit: bool) -> Result<String> {
    let mut tables = BTreeMap::new();
    for (name, rows) in &database.tables {
        if !include_audit && name == "audit" {
            continue;
        }
        let mut hashes: Vec<_> = rows.iter().map(|row| hash(&row_bytes(row))).collect();
        hashes.sort();
        tables.insert(name.as_str(), hashes);
    }
    typed_fingerprint(&("rsia.native.raw_database_digest.v1", tables))
}

fn native_audit_rows(database: &RawDatabase) -> Result<BTreeMap<i64, String>> {
    let mut output = BTreeMap::new();
    for row in database.tables.get("audit").ok_or(Error::ClockRegressed)? {
        let seq = native_integer(row, "seq")?;
        require(seq > 0 && output.insert(seq, hash(&row_bytes(row))).is_none())?;
    }
    Ok(output)
}

fn native_audit_tip(database: &RawDatabase, namespace: &str) -> Result<(i64, String)> {
    let mut tip = (0, String::new());
    for row in database.tables.get("audit").ok_or(Error::ClockRegressed)? {
        let seq = native_integer(row, "seq")?;
        let actual_namespace = native_text(row, "namespace")?;
        if actual_namespace == namespace && seq > tip.0 {
            tip = (seq, native_text(row, "digest")?);
        }
    }
    Ok(tip)
}

#[derive(serde::Serialize)]
struct NativeAsOfSummary {
    schema_version: &'static str,
    namespace: String,
    actor: String,
    role: Role,
    operation: &'static str,
    ticket_id: String,
    actual_ticket_row_digest: String,
    capture_seconds: i64,
    capture_millis: i64,
    capture_database_digest: String,
    prepared_prefix_digest: String,
    closed_ack_prefix_digest: String,
    after_legacy_audit_prefix_digest: String,
    acknowledged_writes: usize,
    previous_audit_sequence: i64,
    previous_audit_hash: String,
    audit_timestamp: i64,
}

// No constructor takes a DTO, a time argument, an "asof" boolean, a callback or
// raw Session from outside this private native module. No Clone/Serde/Deref.
struct NativeAsOfPermit {
    connection: Arc<crate::NativeOwner>,
    clock: crate::write_clock::NativeEventBinding,
    gate: Arc<AtomicU8>,
    facts: Arc<AtomicU8>,
    namespace: String,
    actor: String,
    role: Role,
    operation: NativeAsOfOperation,
    actual_effect_r1: Option<NativePlanEffectR1>,
    capture_seconds: i64,
    capture_millis: i64,
    capture_digest: String,
    prepared_digest: Option<String>,
    prefix_digest: String,
    acknowledged_writes: usize,
    closed_prefix_digest: Option<String>,
    closed_non_audit_digest: Option<String>,
    closed_audit_rows: Option<BTreeMap<i64, String>>,
    witness: Option<NativeAsOfAuditAck>,
    complete_ack_r24: Option<CompleteBindingAckR24>,
}

struct NativeAsOfAuditAck {
    summary: NativeAsOfSummary,
    entity: String,
    seq: i64,
    previous: String,
    digest: String,
    payload: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyFormalWireR24 {
    schema_version: String,
    id: String,
    record_kind: String,
    payload: MirrorFormalEvaluationV2,
}
fn r24_fault(f: evo_core::formal_version::Fault) -> Error {
    use evo_core::formal_version::Fault;
    match f {
        Fault::UnknownVersion => Error::Invalid("formal_unknown_version_quarantine".into()),
        Fault::SourceInsufficient => Error::Conflict("formal_source_insufficient".into()),
        Fault::Capacity => Error::Invalid("formal_capacity".into()),
        Fault::InvalidIdentity => Error::Conflict("formal_invalid_identity".into()),
        Fault::InvalidContent => Error::Conflict("formal_invalid_content".into()),
    }
}
fn r24_formal_ticket(formal: &MirrorFormalEvaluationV2) -> &str {
    match formal {
        MirrorFormalEvaluationV2::CompleteBatch { ticket_id, .. }
        | MirrorFormalEvaluationV2::EarlyStopped { ticket_id, .. }
        | MirrorFormalEvaluationV2::Invalid { ticket_id, .. } => ticket_id,
    }
}

fn validate_legacy_formal_shape_r1(
    namespace: &str,
    expected_ticket: &str,
    formal: &MirrorFormalEvaluationV2,
) -> Result<()> {
    if r24_formal_ticket(formal) != expected_ticket {
        return Err(Error::Conflict("formal_invalid_identity".into()));
    }
    match formal {
        MirrorFormalEvaluationV2::EarlyStopped {
            ticket_id,
            certificate,
            ..
        } => {
            let wire = serde_json::to_value(certificate).map_err(crate::internal)?;
            if wire.get("schema_version").and_then(Value::as_str)
                != Some("rsia.early_stop_certificate.v1")
            {
                return Err(Error::Invalid(
                    "formal_unknown_certificate_version_quarantine".into(),
                ));
            }
            certificate
                .validate_shape()
                .map_err(|_| Error::Conflict("formal_invalid_legacy_certificate_shape".into()))?;
            if certificate.ticket_id() != ticket_id || ticket_id != expected_ticket {
                return Err(Error::Conflict(
                    "formal_invalid_legacy_certificate_identity".into(),
                ));
            }
        }
        MirrorFormalEvaluationV2::CompleteBatch {
            ticket_id,
            ticket_digest,
            report,
            stored_dependency_binding,
            ..
        } => {
            if report.stats_version != "rsia.empirical_bernstein.v2" {
                return Err(Error::Invalid(
                    "formal_unknown_statistics_version_quarantine".into(),
                ));
            }
            if let Some(binding) = stored_dependency_binding {
                binding
                    .validate()
                    .map_err(|_| Error::Conflict("formal_invalid_legacy_binding_shape".into()))?;
                if binding.namespace != namespace
                    || binding.ticket_id != *ticket_id
                    || binding.ticket_digest != *ticket_digest
                {
                    return Err(Error::Conflict(
                        "formal_invalid_legacy_binding_identity".into(),
                    ));
                }
            }
        }
        MirrorFormalEvaluationV2::Invalid { .. } => {}
    }
    Ok(())
}

// Private fixed query. Known reference contradictions precede even opening the query.
pub(crate) async fn read_legacy_formal_material_r24(
    session: &mut Session,
    ctx: &Context,
    ticket_id: &str,
    expected: Option<&evo_core::formal_version::AnyReportRef>,
) -> Result<Option<evo_core::formal_version::LegacyFormalMaterial>> {
    use evo_core::formal_version as fv;
    use sqlx::Row as _;
    {
        let _ = session.transaction()?;
    }
    ctx.require(&[Role::Evaluator, Role::Admin, Role::Host])?;
    identifier(ticket_id)?;
    let physical = fv::old_lookup_address(ticket_id).map_err(r24_fault)?;
    let expected = match expected {
        Some(fv::AnyReportRef::VersionedV3(_)) => {
            return Err(Error::Conflict("formal_invalid_reference_protocol".into()));
        }
        Some(fv::AnyReportRef::ImplicitLegacyV2(reference)) => {
            reference.validate_wire().map_err(r24_fault)?;
            if reference.namespace != ctx.namespace()
                || reference.ticket_id != ticket_id
                || reference.physical_id != physical
            {
                return Err(Error::Conflict("formal_invalid_reference_identity".into()));
            }
            Some(reference)
        }
        None => None,
    };
    let row = sqlx::query("SELECT namespace,kind,id,owner,body,revision FROM objects WHERE namespace=? AND kind='artifact' AND id=?")
        .bind(ctx.namespace()).bind(&physical)
        .fetch_optional(&mut **session.transaction()?).await.map_err(crate::internal)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let namespace: String = row.try_get("namespace").map_err(crate::internal)?;
    let kind: String = row.try_get("kind").map_err(crate::internal)?;
    let id: String = row.try_get("id").map_err(crate::internal)?;
    let owner: String = row.try_get("owner").map_err(crate::internal)?;
    let raw_body: String = row.try_get("body").map_err(crate::internal)?;
    let revision: i64 = row.try_get("revision").map_err(crate::internal)?;
    if namespace != ctx.namespace()
        || kind != "artifact"
        || id != physical
        || revision <= 0
        || raw_body.len() > 4 * 1024 * 1024
    {
        return Err(Error::Conflict("formal_invalid_row_identity".into()));
    }
    identifier(&owner)?;
    // Reject duplicate/unknown fields; use the actual original typed mirror for its fingerprint.
    parse_owned_json(&raw_body).map_err(|_| Error::Conflict("formal_invalid_json".into()))?;
    let original: LegacyFormalWireR24 = serde_json::from_str(&raw_body)
        .map_err(|_| Error::Conflict("formal_invalid_content".into()))?;
    fv::version(&original.schema_version, &original.record_kind, None, None).map_err(r24_fault)?;
    if original.id != physical || r24_formal_ticket(&original.payload) != ticket_id {
        return Err(Error::Conflict("formal_invalid_identity".into()));
    }
    validate_legacy_formal_shape_r1(ctx.namespace(), ticket_id, &original.payload)?;
    let actual = fv::LegacyRef {
        namespace,
        owner,
        physical_id: physical,
        ticket_id: ticket_id.into(),
        payload_digest: typed_fingerprint(&original.payload)?,
    };
    actual.validate_wire().map_err(r24_fault)?;
    if expected.is_some_and(|reference| reference != &actual) {
        return Err(Error::Conflict(
            "formal_invalid_reference_content_or_owner".into(),
        ));
    }
    Ok(Some(fv::LegacyFormalMaterial {
        report_ref: actual,
        raw_body,
    }))
}

type FrozenCallR24 = e05_budget_call_snapshot_r19_r1::FrozenCall;
type FrozenCarrierR24 = evo_core::formal_version::Carrier<
    MirrorEvaluationTicketV2,
    FrozenCallR24,
    MirrorFormalEvaluationV2,
>;
type FrozenCarrierR36 = evo_core::formal_version::CarrierV4<
    MirrorEvaluationTicketV2,
    FrozenCallR24,
    MirrorFormalEvaluationV2,
>;
fn frozen_call_error_r24(error: e05_budget_call_snapshot_r19_r1::ReadFailure) -> Error {
    use e05_budget_call_snapshot_r19_r1::ReadFailure;
    match error {
        ReadFailure::InvalidSnapshot(code) => {
            Error::Conflict(format!("formal_invalid_frozen_call:{code}"))
        }
        ReadFailure::SourceInsufficient(code) => {
            Error::Conflict(format!("formal_source_insufficient:{code}"))
        }
    }
}
fn complete_raw_object_r24<'a>(
    raw: &'a RawDatabase,
    namespace: &str,
    kind: &str,
    id: &str,
) -> Result<&'a OwnedRow> {
    let mut found = raw
        .tables
        .get("objects")
        .ok_or(Error::ClockRegressed)?
        .iter()
        .filter(|row| {
            native_text(row, "namespace").ok().as_deref() == Some(namespace)
                && native_text(row, "kind").ok().as_deref() == Some(kind)
                && native_text(row, "id").ok().as_deref() == Some(id)
        });
    let row = found.next().ok_or(Error::ClockRegressed)?;
    require(found.next().is_none())?;
    Ok(row)
}

fn validate_complete_carrier_r24(
    carrier: &FrozenCarrierR24,
    namespace: &str,
    id: &str,
) -> Result<()> {
    carrier.validate_wire(namespace, id).map_err(r24_fault)?;
    let ticket = &carrier.input.ticket_snapshot;
    ticket.validate()?;
    require(
        ticket.id == carrier.input.ticket_id
            && typed_fingerprint(ticket)? == carrier.input.ticket_digest,
    )?;
    validate_legacy_formal_shape_r1(namespace, &ticket.id, &carrier.payload)?;
    match carrier_effect_r1(&carrier.input.operation, &carrier.payload)? {
        NativePlanEffectR1::NewComplete => {
            require(ticket.lifecycle == MirrorTicketLifecycle::CompleteBatch)?;
            let MirrorFormalEvaluationV2::CompleteBatch {
                evidence_scope,
                ticket_id,
                ticket_digest,
                manifest_digest,
                candidate_bundle_digest,
                baseline_bundle_digest,
                environment_digest,
                verdict,
                report,
                stored_dependency_binding,
                ..
            } = &carrier.payload
            else {
                return Err(Error::ClockRegressed);
            };
            require(
                ticket_id == &ticket.id
                    && ticket_digest == &typed_fingerprint(ticket)?
                    && evidence_scope == &ticket.evidence_scope
                    && manifest_digest == &ticket.manifest_digest
                    && candidate_bundle_digest == &ticket.candidate_bundle_digest
                    && baseline_bundle_digest == &ticket.baseline_bundle_digest
                    && environment_digest == &ticket.environment_digest
                    && *verdict == report.verdict
                    && report.stats_version == "rsia.empirical_bernstein.v2"
                    && report.n
                        == usize::try_from(ticket.n_planned).map_err(|_| Error::ClockRegressed)?,
            )?;
            match (
                &ticket.candidate_dependency_binding,
                stored_dependency_binding,
            ) {
                (Some(_), Some(binding)) => binding.validate()?,
                (None, None) => {}
                _ => return Err(Error::ClockRegressed),
            }
        }
        NativePlanEffectR1::NewInvalid => {
            let MirrorFormalEvaluationV2::Invalid {
                evidence_scope,
                ticket_id,
                ticket_digest,
                reasons,
            } = &carrier.payload
            else {
                return Err(Error::ClockRegressed);
            };
            require(
                ticket.lifecycle == MirrorTicketLifecycle::Invalid
                    && ticket_id == &ticket.id
                    && ticket_digest == &typed_fingerprint(ticket)?
                    && evidence_scope == &ticket.evidence_scope
                    && reasons == &ticket.invalid_reasons
                    && !ticket.unattributed_budget_call_ids.is_empty()
                    && reasons
                        .iter()
                        .any(|r| r == "unattributed_dispatch_group_call"),
            )?;
        }
        _ => return Err(Error::ClockRegressed),
    }
    for call in &carrier.input.calls {
        call.snapshot
            .validate_frozen_target(&call.namespace, &call.billing_scope, &call.call_id)
            .map_err(frozen_call_error_r24)?;
    }
    Ok(())
}

#[derive(Serialize)]
struct FrozenCompleteIntentR24<'a> {
    schema_version: &'static str,
    ticket_id: &'a str,
}
fn freeze_complete_carrier_r24(
    raw: &RawDatabase,
    snapshot: &OccurrenceSnapshot,
    ctx: &Context,
    ticket: &MirrorEvaluationTicketV2,
    formal: MirrorFormalEvaluationV2,
    occurred_at_millis: i64,
) -> Result<FrozenCarrierR24> {
    use evo_core::formal_version as fv;
    ctx.require(&[Role::Evaluator])?;
    require(ticket.evaluator_actor == ctx.actor() && occurred_at_millis >= 0)?;
    let control = evaluation_control(snapshot, ctx.namespace(), &ticket.registration_id)?;
    let holdout = evaluation_holdout(snapshot, ctx.namespace(), &ticket.holdout_id, &control)?;
    producer_witnesses(snapshot, ctx.namespace(), &control, &holdout)?;
    e05_complete_ticket_identity(snapshot, ctx.namespace(), ticket, &control, &holdout)?;
    e05_complete_group(
        snapshot,
        ctx.namespace(),
        &control.billing_scope,
        &ticket.id,
    )?;
    let effect = carrier_effect_r1(
        &match &formal {
            MirrorFormalEvaluationV2::CompleteBatch { .. } => fv::Operation::Complete,
            MirrorFormalEvaluationV2::Invalid { .. } => fv::Operation::Invalidate,
            _ => return Err(Error::ClockRegressed),
        },
        &formal,
    )?;
    let ticket_physical = evaluation_storage_id(TICKET_KIND, &ticket.id)?;
    let mut roots = vec![("artifact".into(), ticket_physical)];
    if effect == NativePlanEffectR1::NewComplete {
        roots.push((
            "artifact".into(),
            evaluation_storage_id(RESOURCE_KIND, &ticket.id)?,
        ));
    }
    let nodes = derive_nodes(snapshot, ctx.namespace(), roots)?;
    let mut facts = Vec::with_capacity(nodes.len());
    for node in nodes {
        // The original derive_nodes codec gives each ORIGINAL typed record fingerprint.
        // Never substitute serde_json::Value ordering for node.content_digest.
        let object = snapshot.object(ctx.namespace(), &node.kind, &node.id)?;
        let row = complete_raw_object_r24(raw, ctx.namespace(), &node.kind, &node.id)?;
        let outgoing = e05_outgoing(snapshot, ctx.namespace(), &node.kind, &node.id)?;
        require(
            outgoing.len() == node.outgoing_edge_count
                && typed_fingerprint(&outgoing)? == node.outgoing_edge_digest,
        )?;
        facts.push(fv::Fact {
            namespace: ctx.namespace().into(),
            kind: node.kind,
            id: node.id,
            owner: object.owner.clone(),
            schema_version: node.schema_version,
            record_kind: node.record_kind,
            logical_id: node.logical_id,
            raw_row_digest: hash(&row_bytes(row)),
            raw_body_digest: hash(native_text(row, "body")?.as_bytes()),
            typed_record_digest: node.content_digest,
            outgoing,
            outgoing_digest: node.outgoing_edge_digest,
        });
    }
    facts.sort_by(|a, b| (&a.namespace, &a.kind, &a.id).cmp(&(&b.namespace, &b.kind, &b.id)));
    let actual_calls: Vec<_> = snapshot
        .calls
        .values()
        .filter(|call| {
            call.billing_scope == control.billing_scope && call.dispatch_group_id == ticket.id
        })
        .collect();
    let mut calls = Vec::with_capacity(actual_calls.len());
    for actual in actual_calls {
        // Full group includes other namespaces. R19 rejects a contradicted namespace;
        // no prior namespace filter can hide a foreign or unattributed call.
        let frozen = FrozenCallR24::freeze_from_qualified_snapshot(
            snapshot,
            ctx.namespace(),
            &control.billing_scope,
            &actual.call_id,
        )
        .map_err(frozen_call_error_r24)?;
        let original = frozen
            .reconstruct_from_qualified_snapshot(
                snapshot,
                ctx.namespace(),
                &control.billing_scope,
                &actual.call_id,
            )
            .map_err(frozen_call_error_r24)?;
        require(
            &original == actual && typed_fingerprint(&original)? == typed_fingerprint(actual)?,
        )?;
        calls.push(fv::Call {
            namespace: actual.namespace.clone(),
            billing_scope: actual.billing_scope.clone(),
            call_id: actual.call_id.clone(),
            snapshot_digest: typed_fingerprint(&frozen)?,
            snapshot: frozen,
        });
    }
    calls.sort_by(|a, b| {
        (&a.namespace, &a.billing_scope, &a.call_id).cmp(&(
            &b.namespace,
            &b.billing_scope,
            &b.call_id,
        ))
    });
    let input = fv::FrozenInput {
        schema_version: fv::INPUT_SCHEMA.into(),
        namespace: ctx.namespace().into(),
        ticket_id: ticket.id.clone(),
        ticket_digest: typed_fingerprint(ticket)?,
        ticket_snapshot: ticket.clone(),
        operation: match effect {
            NativePlanEffectR1::NewComplete => fv::Operation::Complete,
            NativePlanEffectR1::NewInvalid => fv::Operation::Invalidate,
            _ => return Err(Error::ClockRegressed),
        },
        // This digest remains the REAL Complete request; operation is actual effect.
        intent_digest: typed_fingerprint(&FrozenCompleteIntentR24 {
            schema_version: COMPLETE_INTENT_SCHEMA,
            ticket_id: &ticket.id,
        })?,
        occurred_at_unix_ms: occurred_at_millis,
        facts,
        calls,
        watermark: snapshot
            .watermarks
            .get(ctx.namespace())
            .map(|(seq, d)| {
                i64::try_from(*seq)
                    .map(|seq| (seq, d.clone()))
                    .map_err(|_| Error::ClockRegressed)
            })
            .transpose()?,
    };
    let carrier = fv::Carrier::from_frozen_input(input, formal).map_err(r24_fault)?;
    validate_complete_carrier_r24(&carrier, ctx.namespace(), &carrier.id)?;
    Ok(carrier)
}

fn validate_complete_known_r36(
    carrier: &FrozenCarrierR36,
    namespace: &str,
    id: &str,
) -> Result<()> {
    carrier
        .validate_known_wire(namespace, id)
        .map_err(r24_fault)?;
    let ticket = &carrier.input.ticket_snapshot;
    ticket.validate()?;
    require(
        ticket.id == carrier.input.ticket_id
            && typed_fingerprint(ticket)? == carrier.input.ticket_digest,
    )?;
    validate_legacy_formal_shape_r1(namespace, &ticket.id, &carrier.payload)?;
    match &carrier.payload {
        MirrorFormalEvaluationV2::CompleteBatch { .. } => {
            r36_check(
                matches!(
                    &carrier.input.operation,
                    evo_core::formal_version::Operation::Complete
                ),
                "complete_operation_payload",
            )?;
            require(ticket.lifecycle == MirrorTicketLifecycle::CompleteBatch)?;
            let MirrorFormalEvaluationV2::CompleteBatch {
                evidence_scope,
                ticket_id,
                ticket_digest,
                manifest_digest,
                candidate_bundle_digest,
                baseline_bundle_digest,
                environment_digest,
                verdict,
                report,
                stored_dependency_binding,
                ..
            } = &carrier.payload
            else {
                return Err(Error::ClockRegressed);
            };
            require(
                ticket_id == &ticket.id
                    && ticket_digest == &typed_fingerprint(ticket)?
                    && evidence_scope == &ticket.evidence_scope
                    && manifest_digest == &ticket.manifest_digest
                    && candidate_bundle_digest == &ticket.candidate_bundle_digest
                    && baseline_bundle_digest == &ticket.baseline_bundle_digest
                    && environment_digest == &ticket.environment_digest
                    && *verdict == report.verdict
                    && report.stats_version == "rsia.empirical_bernstein.v2"
                    && report.n
                        == usize::try_from(ticket.n_planned).map_err(|_| Error::ClockRegressed)?,
            )?;
            match (
                &ticket.candidate_dependency_binding,
                stored_dependency_binding,
            ) {
                (Some(_), Some(binding)) => binding.validate()?,
                (None, None) => {}
                _ => return Err(Error::ClockRegressed),
            }
        }
        MirrorFormalEvaluationV2::Invalid { .. } => {
            r36_check(
                matches!(
                    &carrier.input.operation,
                    evo_core::formal_version::Operation::Invalidate
                ),
                "invalid_operation_payload",
            )?;
            let MirrorFormalEvaluationV2::Invalid {
                evidence_scope,
                ticket_id,
                ticket_digest,
                reasons,
            } = &carrier.payload
            else {
                return Err(Error::ClockRegressed);
            };
            require(
                ticket.lifecycle == MirrorTicketLifecycle::Invalid
                    && ticket_id == &ticket.id
                    && ticket_digest == &typed_fingerprint(ticket)?
                    && evidence_scope == &ticket.evidence_scope
                    && reasons == &ticket.invalid_reasons
                    && !ticket.unattributed_budget_call_ids.is_empty()
                    && reasons
                        .iter()
                        .any(|r| r == "unattributed_dispatch_group_call"),
            )?;
        }
        MirrorFormalEvaluationV2::EarlyStopped {
            evidence_scope,
            ticket_id,
            ticket_digest,
            certificate,
        } => {
            r36_check(
                matches!(
                    &carrier.input.operation,
                    evo_core::formal_version::Operation::EarlyStop
                        | evo_core::formal_version::Operation::Reconcile
                ),
                "early_operation_payload",
            )?;
            certificate.validate_shape()?;
            let certificate_wire = serde_json::to_value(certificate).map_err(crate::internal)?;
            let pending = ticket
                .pending_certificate
                .as_ref()
                .ok_or_else(|| r24_fault(evo_core::formal_version::Fault::InvalidContent))?;
            r36_check(
                ticket.lifecycle == MirrorTicketLifecycle::EarlyStopped
                    && evidence_scope == &ticket.evidence_scope
                    && ticket_id == &ticket.id
                    && ticket_digest == &typed_fingerprint(ticket)?
                    && typed_fingerprint(pending)? == typed_fingerprint(certificate)?
                    && ticket.stop_member_terminal_digest.as_deref()
                        == Some(certificate.member_terminal_digest())
                    && certificate_wire
                        .get("stopped_at_unix_ms")
                        .and_then(Value::as_i64)
                        .is_some_and(|time| time <= carrier.input.occurred_at_unix_ms),
                "known_early_certificate_identity",
            )?;
        }
    }
    if let Some(source) = &carrier.input.native_first_stop {
        source.snapshot.validate_known().map_err(r24_fault)?;
        r36_check(
            source.typed_digest == typed_fingerprint(&source.snapshot)?
                && source.snapshot.namespace == namespace
                && source.snapshot.ticket_id == ticket.id,
            "known_native_first_stop_identity",
        )?;
        reconstruct_first_stop_r44(ticket, &source.snapshot)?;
    }
    for call in &carrier.input.calls {
        call.snapshot
            .validate_frozen_target(&call.namespace, &call.billing_scope, &call.call_id)
            .map_err(frozen_call_error_r24)?;
    }
    // Every actual typed call's 29 values and 3 descriptors were checked above.
    // This extra binding uses the ORIGINAL frozen group, never a later paid row.
    for call in &carrier.input.calls {
        call.snapshot
            .validate_ticket_binding_r36(namespace, &ticket.id)
            .map_err(frozen_call_error_r24)?;
    }
    if let Some(exposure) = &carrier.input.exposure {
        let exposure = &exposure.snapshot;
        r36_check(
            exposure.research_family_id == ticket.research_family_id
                && exposure.alpha_plan_digest == ticket.alpha_plan_digest
                && exposure.entry.ticket_id == ticket.id
                && exposure.entry.manifest_digest == ticket.manifest_digest,
            "exposure_ticket_binding",
        )?;
        let rotating: Vec<_> = ticket
            .targets
            .iter()
            .filter_map(|target| match &target.kind {
                MirrorEvaluationTargetKind::Rotating { cluster_id } => Some(cluster_id.clone()),
                MirrorEvaluationTargetKind::Anchor { .. } => None,
            })
            .collect();
        r36_check(
            exposure.entry.task_cluster_ids == rotating,
            "exposure_rotating_targets",
        )?;
    }
    if let Some(group) = &carrier.input.dispatch_group {
        r36_check(
            group.snapshot.owner_namespace == namespace
                && group.snapshot.dispatch_group_id == ticket.id
                && group.snapshot.stopped
                && group
                    .snapshot
                    .stop_reason
                    .as_ref()
                    .is_some_and(|reason| !reason.is_empty())
                && group.snapshot.stop_committed_at.is_some(),
            "closed_dispatch_group",
        )?;
    }
    if let MirrorFormalEvaluationV2::CompleteBatch {
        cost_evidence_scope,
        ..
    } = &carrier.payload
    {
        r36_check(
            *cost_evidence_scope == MirrorCostEvidenceScope::TicketExecutionOnly,
            "persistent_cost_scope",
        )?;
    }
    Ok(())
}

// Engine4/input2 data remains unprivileged. Only the existing closed native
// operation below can capture it; no serialized flag, current row or mutable
// paid overlay is promoted into an original source.
fn r36_check(ok: bool, code: &'static str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::Conflict(format!("formal_invalid_input2:{code}")))
    }
}
fn r36_sources(gaps: &BTreeSet<&'static str>) -> Result<()> {
    if gaps.is_empty() {
        Ok(())
    } else {
        Err(Error::Conflict(format!(
            "formal_source_insufficient:input2:{}",
            gaps.iter().copied().collect::<Vec<_>>().join(",")
        )))
    }
}
fn root_projection_r36(
    actual: &crate::budget::RootBudgetRecord,
) -> evo_core::formal_version::RootBudgetSnapshotV2 {
    evo_core::formal_version::RootBudgetSnapshotV2 {
        root_budget_id: actual.root_budget_id.clone(),
        billing_scope: actual.billing_scope.clone(),
        authorizing_namespace: actual.authorizing_namespace.clone(),
        allowed_namespaces: actual.allowed_namespaces.clone(),
        currency: actual.currency.clone(),
        pricing_version: actual.pricing_version.clone(),
        payment_subject: actual.payment_subject.clone(),
        authorization_receipt_digest: actual.authorization_receipt_digest.clone(),
        per_call_cap_micros: actual.per_call_cap_micros,
        total_limit_micros: actual.total_limit_micros,
        spent_micros: actual.spent_micros,
        reserved_micros: actual.reserved_micros,
        stopped: actual.stopped,
        stop_reason: actual.stop_reason.clone(),
        stop_committed_at: actual.stop_committed_at,
        created_at: actual.created_at,
    }
}
fn group_projection_r36(
    actual: &DispatchGroupRecord,
) -> evo_core::formal_version::DispatchGroupSnapshotV2 {
    evo_core::formal_version::DispatchGroupSnapshotV2 {
        billing_scope: actual.billing_scope.clone(),
        dispatch_group_id: actual.dispatch_group_id.clone(),
        owner_namespace: actual.owner_namespace.clone(),
        stopped: actual.stopped,
        stop_reason: actual.stop_reason.clone(),
        stop_committed_at: actual.stop_committed_at,
        created_at: actual.created_at,
    }
}
fn exposure_projection_r36(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
) -> Result<Option<evo_core::formal_version::ExposureProjectionV2>> {
    let key = (
        namespace.to_owned(),
        "artifact".to_owned(),
        evaluation_storage_id(LEDGER_KIND, &ticket.research_family_id)?,
    );
    let Some(object) = snapshot.objects.get(&key) else {
        return Ok(None);
    };
    let ledger: ExposureLedger = decode(envelope(
        object,
        "rsia.typed_artifact_envelope.v1",
        LEDGER_KIND,
    )?)?;
    ledger.validate_restore()?;
    r36_check(
        object.owner == ticket.evaluator_actor
            && ledger.research_family_id == ticket.research_family_id
            && ledger.alpha_plan_digest == ticket.alpha_plan_digest,
        "actual_exposure_header_identity",
    )?;
    let mut entries = ledger
        .entries
        .iter()
        .filter(|entry| entry.ticket_id == ticket.id);
    let Some(entry) = entries.next() else {
        return Ok(None);
    };
    r36_check(entries.next().is_none(), "duplicate_actual_exposure_entry")?;
    Ok(Some(evo_core::formal_version::ExposureProjectionV2 {
        schema_version: ledger.schema_version,
        research_family_id: ledger.research_family_id,
        alpha_plan_digest: ledger.alpha_plan_digest,
        query_limit: ledger.query_limit,
        entry: entry.clone(),
    }))
}

// Strictly SAME-NATIVE-PREFIX comparison, not an old-record read algorithm.
// The actual root/entry here cannot be replaced with a later live projection.
// All known carrier/call/projection/entity contradictions are inspected before
// any accumulated missing source is returned. Frozen None is never filled.
// Pure known identities precede all issue-receipt/witness/source lookups.
fn known_ticket_control_r36(
    ticket: &MirrorEvaluationTicketV2,
    control: &MirrorRegisteredEvaluationControl,
) -> Result<()> {
    ticket.validate()?;
    control.validate()?;
    let mut bound = control.v1_plan_snapshot.clone();
    bound.bind_candidate(ticket.candidate_digest.clone())?;
    r36_check(
        ticket.registration_id == control.id
            && ticket.registration_digest == typed_fingerprint(control)?
            && ticket.v1_plan_snapshot_digest == control.v1_plan_snapshot.digest()?
            && ticket.bound_v1_plan_digest == bound.digest()?
            && ticket.formal_plan_digest == control.formal_plan.digest()?
            && ticket.alpha_plan_digest == control.alpha_plan.digest()?
            && ticket.research_family_id == control.alpha_plan.research_family_id
            && ticket.attempt_id == control.attempt_id
            && ticket.profile == control.formal_plan.profile
            && ticket.n_planned == control.formal_plan.n_planned
            && ticket.proposer_actor == control.proposer_actor
            && ticket.evaluator_actor == control.evaluator_actor
            && ticket.executor_actor == control.executor_actor
            && ticket.approver_actor == control.approver_actor
            && ticket.evidence_scope == control.evidence_scope,
        "known_ticket_control_fields",
    )
}
fn known_ticket_holdout_r36(
    ticket: &MirrorEvaluationTicketV2,
    holdout: &MirrorProtectedHoldoutRecord,
) -> Result<()> {
    ticket.validate()?;
    holdout.pair.validate()?;
    holdout.manifest.validate()?;
    r36_check(
        ticket.holdout_id == holdout.id
            && ticket.registration_id == holdout.registration_id
            && ticket.manifest_digest == holdout.manifest.digest()?
            && ticket.candidate_digest == holdout.pair.candidate_digest
            && ticket.baseline_digest == holdout.pair.baseline_digest
            && ticket.candidate_bundle_digest == holdout.candidate_bundle_digest
            && ticket.baseline_bundle_digest == holdout.baseline_bundle_digest
            && ticket.environment_digest == holdout.manifest.environment_digest,
        "known_ticket_holdout_fields",
    )?;
    let inputs = holdout
        .rotating_inputs
        .iter()
        .map(|input| (input, false))
        .chain(holdout.anchor_inputs.iter().map(|input| (input, true)))
        .collect::<Vec<_>>();
    r36_check(
        ticket.targets.len() == inputs.len()
            && ticket
                .targets
                .iter()
                .zip(inputs)
                .all(|(target, (input, anchor))| {
                    target.target_id == input.target_id
                        && target.input_artifact_id == input.input_artifact_id
                        && target.input_digest == input.input_digest
                        && match &target.kind {
                            MirrorEvaluationTargetKind::Rotating { cluster_id } => {
                                !anchor && cluster_id == &input.cluster_or_anchor_id
                            }
                            MirrorEvaluationTargetKind::Anchor { anchor_id } => {
                                anchor && anchor_id == &input.cluster_or_anchor_id
                            }
                        }
                }),
        "known_ticket_holdout_targets",
    )
}
#[derive(Default)]
struct NativeSourceInspectionR36 {
    actual: BTreeSet<&'static str>,
    frozen: BTreeSet<&'static str>,
}
impl NativeSourceInspectionR36 {
    fn finish(mut self) -> Result<()> {
        self.actual.extend(self.frozen);
        r36_sources(&self.actual)
    }
}
fn inspect_issue_and_witness_sources_r36(
    snapshot: &OccurrenceSnapshot,
    ns: &str,
    ticket: &MirrorEvaluationTicketV2,
    control: Option<&MirrorRegisteredEvaluationControl>,
    holdout: Option<&MirrorProtectedHoldoutRecord>,
    gaps: &mut BTreeSet<&'static str>,
) -> Result<()> {
    for (kind, logical) in [
        (
            "registered_evaluation_control_v41",
            ticket.registration_id.as_str(),
        ),
        (HOLDOUT_KIND, ticket.holdout_id.as_str()),
    ] {
        let key = (
            ns.to_owned(),
            "artifact".to_owned(),
            evaluation_storage_id(kind, logical)?,
        );
        if let Some(object) = snapshot.objects.get(&key) {
            let subject = typed_fingerprint(&(&object.namespace, &object.kind, &object.id))?;
            let operation = format!("formal.evaluator.content.v1.{subject}");
            if snapshot
                .witnesses
                .contains_key(&(ns.to_owned(), "evaluator".into(), operation))
            {
                // Presence does not imply validity: retain the exact original check.
                snapshot.witness(object, "evaluator")?;
            } else {
                gaps.insert("original_evaluator_witness_missing");
            }
        }
    }
    let mut selected = None;
    for (kind, schema) in [
        (
            "evaluation_ticket_issue_receipt_v1",
            "rsia.evaluation_ticket_issue_receipt.v1",
        ),
        (
            "evaluation_ticket_issue_receipt_v2",
            "rsia.evaluation_ticket_issue_receipt.v2",
        ),
    ] {
        let key = (
            ns.to_owned(),
            "artifact".to_owned(),
            evaluation_storage_id(kind, &ticket.id)?,
        );
        let Some(object) = snapshot.objects.get(&key) else {
            continue;
        };
        r36_check(selected.is_none(), "multiple_issue_protocols")?;
        e05_issue_receipt_binding(kind, ticket.candidate_dependency_binding.as_ref())?;
        let payload = envelope(object, "rsia.typed_artifact_envelope.v1", kind)?;
        let issue = if kind == "evaluation_ticket_issue_receipt_v1" {
            let raw: MirrorTicketIssueReceipt = decode(payload)?;
            r36_check(raw.schema_version == schema, "issue_schema")?;
            let actual_digest = typed_fingerprint(&raw)?;
            E05IssueReceiptIdentity {
                ticket_id: raw.ticket_id,
                registration_digest: raw.registration_digest,
                manifest_digest: raw.manifest_digest,
                evaluator_actor: raw.evaluator_actor,
                issued_at_unix_seconds: raw.issued_at_unix_seconds,
                issued_at_unix_ms: None,
                actual_digest,
            }
        } else {
            let raw: MirrorTicketIssueReceiptV2 = decode(payload)?;
            r36_check(
                raw.schema_version == schema
                    && raw.issued_at_unix_seconds >= 0
                    && raw.issued_at_unix_ms >= 0
                    && raw.issued_at_unix_seconds == raw.issued_at_unix_ms / 1000,
                "issue_v2_known_time",
            )?;
            if let Some(control) = control {
                r36_check(
                    raw.issued_at_unix_seconds >= control.created_at_unix_seconds,
                    "issue_control_time",
                )?;
            }
            if let Some(holdout) = holdout {
                r36_check(
                    raw.issued_at_unix_ms >= holdout.manifest.issued_at_unix_ms,
                    "issue_holdout_time",
                )?;
            }
            let actual_digest = typed_fingerprint(&raw)?;
            E05IssueReceiptIdentity {
                ticket_id: raw.ticket_id,
                registration_digest: raw.registration_digest,
                manifest_digest: raw.manifest_digest,
                evaluator_actor: raw.evaluator_actor,
                issued_at_unix_seconds: raw.issued_at_unix_seconds,
                issued_at_unix_ms: Some(raw.issued_at_unix_ms),
                actual_digest,
            }
        };
        r36_check(
            object.namespace == ns
                && object.owner == ticket.evaluator_actor
                && issue.ticket_id == ticket.id
                && issue.registration_digest == ticket.registration_digest
                && issue.manifest_digest == ticket.manifest_digest
                && issue.evaluator_actor == ticket.evaluator_actor
                && issue.issued_at_unix_seconds == ticket.issued_at_unix_seconds
                && issue.actual_digest == ticket.issue_receipt_digest,
            "issue_known_identity",
        )?;
        selected = Some(kind);
    }
    if selected.is_none() {
        gaps.insert("original_issue_receipt_missing");
    }
    Ok(())
}
fn inspect_complete_native_capture_r36(
    carrier: &FrozenCarrierR36,
    snapshot: &OccurrenceSnapshot,
) -> Result<NativeSourceInspectionR36> {
    let ns = carrier.input.namespace.as_str();
    validate_complete_known_r36(carrier, ns, &carrier.id)?;
    let ticket = &carrier.input.ticket_snapshot;
    let mut gaps = BTreeSet::new();
    inspect_native_mutable_projection_shape_r39(carrier, &mut gaps)?;

    let ticket_key = (
        ns.to_owned(),
        "artifact".to_owned(),
        evaluation_storage_id(TICKET_KIND, &ticket.id)?,
    );
    if let Some(object) = snapshot.objects.get(&ticket_key) {
        let actual: MirrorEvaluationTicketV2 = decode(envelope(
            object,
            "rsia.typed_artifact_envelope.v1",
            TICKET_KIND,
        )?)?;
        actual.validate()?;
        r36_check(
            actual.id == ticket.id
                && typed_fingerprint(&actual)? == carrier.input.ticket_digest
                && [
                    ticket.evaluator_actor.as_str(),
                    ticket.executor_actor.as_str(),
                ]
                .contains(&object.owner.as_str()),
            "actual_full_ticket_original",
        )?;
        e05_dependency_live(snapshot, object)?;
    } else {
        gaps.insert("original_ticket_missing");
    }

    // A present actual row that disagrees with a claimed saved projection is
    // known bad even if the control is absent. Looking up the exact saved tuple
    // only compares available evidence; it does not grant that tuple authority.
    if let Some(saved) = &carrier.input.root_budget {
        if let Some(actual) = snapshot.roots.get(&saved.snapshot.billing_scope) {
            let projection = root_projection_r36(actual);
            projection
                .validate_wire(carrier.input.occurred_at_unix_ms)
                .map_err(r24_fault)?;
            r36_check(
                saved.snapshot == projection
                    && saved.typed_digest == typed_fingerprint(&projection)?,
                "available_root_original_mismatch",
            )?;
        } else {
            gaps.insert("saved_root_original_missing");
        }
    }
    if let Some(saved) = &carrier.input.dispatch_group {
        if let Some(actual) = snapshot.groups.get(&(
            saved.snapshot.billing_scope.clone(),
            saved.snapshot.dispatch_group_id.clone(),
        )) {
            let projection = group_projection_r36(actual);
            projection
                .validate_wire(carrier.input.occurred_at_unix_ms)
                .map_err(r24_fault)?;
            r36_check(
                saved.snapshot == projection
                    && saved.typed_digest == typed_fingerprint(&projection)?,
                "available_group_original_mismatch",
            )?;
        } else {
            gaps.insert("saved_group_original_missing");
        }
    }

    let control_key = (
        ns.to_owned(),
        "artifact".to_owned(),
        evaluation_storage_id("registered_evaluation_control_v41", &ticket.registration_id)?,
    );
    let control = if snapshot.objects.contains_key(&control_key) {
        let actual = evaluation_control(snapshot, ns, &ticket.registration_id)?;
        r36_check(
            actual.id == ticket.registration_id
                && typed_fingerprint(&actual)? == ticket.registration_digest
                && actual.evaluator_actor == ticket.evaluator_actor,
            "actual_control_ticket_identity",
        )?;
        known_ticket_control_r36(ticket, &actual)?;
        Some(actual)
    } else {
        gaps.insert("registered_control_missing");
        None
    };
    let holdout_key = (
        ns.to_owned(),
        "artifact".to_owned(),
        evaluation_storage_id(HOLDOUT_KIND, &ticket.holdout_id)?,
    );
    let holdout = if let Some(object) = snapshot.objects.get(&holdout_key) {
        let actual: MirrorProtectedHoldoutRecord = decode(envelope(
            object,
            "rsia.typed_artifact_envelope.v1",
            HOLDOUT_KIND,
        )?)?;
        actual.pair.validate()?;
        actual.manifest.validate()?;
        r36_check(
            actual.schema_version == MirrorProtectedHoldoutRecord::SCHEMA
                && actual.id == ticket.holdout_id
                && actual.registration_id == ticket.registration_id
                && object.owner == ticket.evaluator_actor
                && actual.manifest.digest()? == ticket.manifest_digest
                && actual.pair.candidate_digest == ticket.candidate_digest
                && actual.pair.baseline_digest == ticket.baseline_digest
                && actual.candidate_bundle_digest == ticket.candidate_bundle_digest
                && actual.baseline_bundle_digest == ticket.baseline_bundle_digest
                && actual.manifest.candidate_pair_digest == actual.pair.digest()?
                && actual.oracle_payload_digest == typed_fingerprint(&actual.oracle_entries)?,
            "actual_holdout_ticket_identity",
        )?;
        // These checks need no missing control or oracle to be synthesized.
        let mut target_ids = BTreeSet::new();
        for input in actual
            .rotating_inputs
            .iter()
            .chain(actual.anchor_inputs.iter())
        {
            input.validate()?;
            r36_check(
                target_ids.insert(input.target_id.as_str()),
                "actual_holdout_duplicate_target",
            )?;
        }
        let mut oracle_ids = BTreeSet::new();
        for oracle in &actual.oracle_entries {
            identifier(&oracle.target_id)?;
            parse_owned_json(&oracle.expected_answer_json)?;
            r36_check(
                oracle_ids.insert(oracle.target_id.as_str()),
                "actual_holdout_duplicate_oracle",
            )?;
        }
        r36_check(target_ids == oracle_ids, "actual_holdout_oracle_targets")?;
        known_ticket_holdout_r36(ticket, &actual)?;
        Some(actual)
    } else {
        gaps.insert("protected_holdout_missing");
        None
    };

    // Local saved descriptors have ALL been validated, including private 29
    // values and the 3 exact optional slot refs. Actual reconstructions also
    // accumulate missing bodies across calls instead of hiding a later error.
    let mut frozen_ids = BTreeSet::new();
    for saved in &carrier.input.calls {
        r36_check(
            frozen_ids.insert((
                saved.namespace.clone(),
                saved.billing_scope.clone(),
                saved.call_id.clone(),
            )),
            "duplicate_frozen_call",
        )?;
        match saved.snapshot.reconstruct_from_qualified_snapshot(
            snapshot,
            &saved.namespace,
            &saved.billing_scope,
            &saved.call_id,
        ) {
            Ok(_) => {}
            Err(e05_budget_call_snapshot_r19_r1::ReadFailure::InvalidSnapshot(code)) => {
                return Err(frozen_call_error_r24(
                    e05_budget_call_snapshot_r19_r1::ReadFailure::InvalidSnapshot(code),
                ));
            }
            Err(e05_budget_call_snapshot_r19_r1::ReadFailure::SourceInsufficient(code)) => {
                gaps.insert(code);
            }
        }
    }

    if let Some(control) = &control {
        r36_check(
            control.billing_scope == control.optimization_budget.root_budget_scope_id,
            "control_root_scope",
        )?;
        // Scope is from the ACTUAL registered control, never an invented
        // control.root_budget_id field or a caller's paid-overlay selection.
        let receipt = control
            .optimization_budget
            .admin_authorization_receipt_digest
            .as_deref();
        let (negative, units) =
            evo_core::evaluation::parse_decimal(&control.optimization_budget.root_total.amount)?;
        r36_check(
            !negative && units != 0 && units % 100 == 0,
            "control_root_amount",
        )?;
        let amount = i64::try_from(units / 100).map_err(|_| Error::ClockRegressed)?;
        let actual_root = snapshot.roots.get(&control.billing_scope);
        if let Some(actual) = actual_root {
            let projection = root_projection_r36(actual);
            projection
                .validate_wire(carrier.input.occurred_at_unix_ms)
                .map_err(r24_fault)?;
            r36_check(
                actual.billing_scope == control.billing_scope
                    && actual.authorizing_namespace == ns
                    && actual.allowed_namespaces.iter().any(|member| member == ns)
                    && actual.currency == control.optimization_budget.root_total.currency
                    && actual.pricing_version
                        == control.optimization_budget.root_total.pricing_version
                    && actual.total_limit_micros == amount
                    && receipt == Some(actual.authorization_receipt_digest.as_str()),
                "actual_root_authorization",
            )?;
            if let Some(saved) = &carrier.input.root_budget {
                r36_check(
                    saved.snapshot == projection
                        && saved.typed_digest == typed_fingerprint(&projection)?,
                    "frozen_root_all16_original_fields",
                )?;
            }
        } else {
            gaps.insert("original_root_missing");
        }
        if let Some(saved) = &carrier.input.root_budget {
            r36_check(
                saved.snapshot.billing_scope == control.billing_scope
                    && saved.snapshot.authorizing_namespace == ns
                    && saved
                        .snapshot
                        .allowed_namespaces
                        .iter()
                        .any(|member| member == ns)
                    && saved.snapshot.total_limit_micros == amount
                    && saved.snapshot.currency == control.optimization_budget.root_total.currency
                    && saved.snapshot.pricing_version
                        == control.optimization_budget.root_total.pricing_version
                    && control
                        .optimization_budget
                        .admin_authorization_receipt_digest
                        .as_deref()
                        == Some(saved.snapshot.authorization_receipt_digest.as_str()),
                "saved_root_control_binding",
            )?;
        }

        let actual_group = snapshot
            .groups
            .get(&(control.billing_scope.clone(), ticket.id.clone()));
        if let Some(actual) = actual_group {
            let projection = group_projection_r36(actual);
            projection
                .validate_wire(carrier.input.occurred_at_unix_ms)
                .map_err(r24_fault)?;
            r36_check(
                actual.owner_namespace == ns
                    && actual.stopped
                    && actual
                        .stop_reason
                        .as_ref()
                        .is_some_and(|reason| !reason.is_empty())
                    && actual.stop_committed_at.is_some(),
                "actual_complete_group",
            )?;
            if let Some(saved) = &carrier.input.dispatch_group {
                r36_check(
                    saved.snapshot == projection
                        && saved.typed_digest == typed_fingerprint(&projection)?,
                    "frozen_group_all7_original_fields",
                )?;
            }
        } else {
            gaps.insert("original_dispatch_group_missing");
        }
        if let Some(saved) = &carrier.input.dispatch_group {
            r36_check(
                saved.snapshot.billing_scope == control.billing_scope,
                "saved_group_control_scope",
            )?;
        }
        if let Some(saved) = &carrier.input.exposure {
            r36_check(
                saved.snapshot.query_limit == control.v1_plan_snapshot.query_limit
                    && saved.snapshot.research_family_id == control.alpha_plan.research_family_id
                    && saved.snapshot.alpha_plan_digest == control.alpha_plan.digest()?,
                "saved_exposure_control_binding",
            )?;
        }
        for saved in &carrier.input.calls {
            r36_check(
                saved.billing_scope == control.billing_scope,
                "saved_call_actual_control_scope",
            )?;
        }
        for actual in snapshot.calls.values().filter(|call| {
            call.billing_scope == control.billing_scope && call.dispatch_group_id == ticket.id
        }) {
            r36_check(
                frozen_ids.contains(&(
                    actual.namespace.clone(),
                    actual.billing_scope.clone(),
                    actual.call_id.clone(),
                )),
                "complete_group_extra_call",
            )?;
        }
    } else {
        // Do not use a saved DTO to invent the unavailable registered scope.
        gaps.insert("original_control_scope_unavailable");
    }

    let actual_exposure = exposure_projection_r36(snapshot, ns, ticket)?;
    if let Some(actual) = actual_exposure {
        actual
            .validate_wire(&ticket.id, carrier.input.occurred_at_unix_ms)
            .map_err(r24_fault)?;
        if let Some(saved) = &carrier.input.exposure {
            r36_check(
                saved.typed_digest == typed_fingerprint(&actual)?
                    && typed_fingerprint(&saved.snapshot)? == typed_fingerprint(&actual)?,
                "frozen_ticket_exposure_all4_plus8_original_fields",
            )?;
        }
    } else {
        gaps.insert("original_ticket_exposure_missing");
    }

    // An available holdout is checked even when another original is missing.
    if let (Some(holdout), Some(saved)) = (&holdout, &carrier.input.exposure) {
        r36_check(
            saved.snapshot.entry.manifest_digest == holdout.manifest.digest()?
                && saved.snapshot.entry.candidate_pair_digest
                    == holdout.manifest.candidate_pair_digest
                && saved.snapshot.entry.epoch_id == holdout.manifest.epoch_id
                && saved.snapshot.entry.slice_id == holdout.manifest.slice_id
                && saved.snapshot.entry.task_cluster_ids
                    == holdout.manifest.ordered_task_cluster_ids,
            "saved_exposure_original_holdout_binding",
        )?;
    }
    if let (Some(control), Some(holdout)) = (&control, &holdout) {
        holdout.validate_against(control)?;
    }
    inspect_issue_and_witness_sources_r36(
        snapshot,
        ns,
        ticket,
        control.as_ref(),
        holdout.as_ref(),
        &mut gaps,
    )?;
    let mut frozen = BTreeSet::new();
    for (missing, name) in [
        (
            carrier.input.exposure.is_none(),
            "frozen_ticket_exposure_missing",
        ),
        (carrier.input.root_budget.is_none(), "frozen_root_missing"),
        (
            carrier.input.dispatch_group.is_none(),
            "frozen_dispatch_group_missing",
        ),
    ] {
        if missing {
            frozen.insert(name);
        }
    }
    // No missing error is emitted here. The whole original known kernel must
    // still inspect actual facts/edges/resources/outputs before finish().
    Ok(NativeSourceInspectionR36 {
        actual: gaps,
        frozen,
    })
}
fn validate_complete_native_capture_r36(
    raw: &RawDatabase,
    carrier: &FrozenCarrierR36,
    snapshot: &OccurrenceSnapshot,
) -> Result<()> {
    let mut inspected = inspect_complete_native_capture_r36(carrier, snapshot)?;
    frozen_complete_kernel_r29::inspect_available_originals_r36(
        raw,
        snapshot,
        carrier,
        &mut inspected.actual,
    )?;
    for actual in native_mutable_projections_r39(
        raw,
        snapshot,
        &carrier.input.namespace,
        &carrier.input.ticket_snapshot,
        &carrier.input.facts,
    )? {
        if let Some(saved) = carrier
            .input
            .mutable_fact_projections
            .iter()
            .find(|saved| saved.kind == actual.kind && saved.id == actual.id)
        {
            r36_check(
                typed_fingerprint(saved)? == typed_fingerprint(&actual)?,
                "actual_native_projection_ack_source",
            )?;
        } else {
            inspected
                .actual
                .insert("frozen_native_mutable_projection_missing");
        }
    }
    validate_native_exposure_capture_ack_r2(raw, snapshot, carrier, &mut inspected.actual)?;
    inspected.finish()
}

fn native_mutable_projections_r39(
    raw: &RawDatabase,
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
    facts: &[evo_core::formal_version::Fact],
) -> Result<Vec<evo_core::formal_version::NativeMutableProjectionV2>> {
    use evo_core::formal_version as fv;
    let ticket_id = evaluation_storage_id(TICKET_KIND, &ticket.id)?;
    let mut output = Vec::new();
    for fact in facts {
        if !snapshot.objects.contains_key(&(
            namespace.to_owned(),
            fact.kind.clone(),
            fact.id.clone(),
        )) {
            continue;
        }
        let projection = if fact.kind == "artifact" && fact.id == ticket_id {
            let actual: MirrorEvaluationTicketV2 =
                evaluation_record(snapshot, namespace, TICKET_KIND, &ticket.id)?;
            r36_check(
                typed_fingerprint(&actual)? == typed_fingerprint(ticket)?,
                "native_mutable_ticket_original",
            )?;
            Some((
                fv::NativeMutableProjectionKindV2::TicketSnapshot,
                typed_fingerprint(&actual)?,
            ))
        } else {
            None
        };
        if let Some((projection, projection_digest)) = projection {
            let row = complete_raw_object_r24(raw, namespace, &fact.kind, &fact.id)?;
            let object = snapshot.object(namespace, &fact.kind, &fact.id)?;
            let (_, _, _, actual_typed) = e05_dependency_identity(snapshot, object)?;
            r36_check(
                fact.namespace == namespace
                    && fact.owner == object.owner
                    && fact.raw_row_digest == hash(&row_bytes(row))
                    && fact.raw_body_digest == hash(native_text(row, "body")?.as_bytes())
                    && fact.typed_record_digest == actual_typed,
                "native_mutable_original_row_and_body",
            )?;
            output.push(fv::NativeMutableProjectionV2 {
                kind: fact.kind.clone(),
                id: fact.id.clone(),
                projection,
                projection_digest,
                original_row_digest: fact.raw_row_digest.clone(),
                original_body_digest: fact.raw_body_digest.clone(),
                original_typed_digest: fact.typed_record_digest.clone(),
            });
        }
    }
    output.sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
    Ok(output)
}

// Known descriptors/actual projection mismatches are never hidden by a
// different absent original. A missing map stays a gap until all known facts.
fn inspect_native_mutable_projection_shape_r39(
    carrier: &FrozenCarrierR36,
    gaps: &mut BTreeSet<&'static str>,
) -> Result<()> {
    use evo_core::formal_version as fv;
    let ticket = &carrier.input.ticket_snapshot;
    let expected_ticket = evaluation_storage_id(TICKET_KIND, &ticket.id)?;
    for saved in &carrier.input.mutable_fact_projections {
        r36_check(
            saved.projection == fv::NativeMutableProjectionKindV2::TicketSnapshot
                && saved.kind == "artifact"
                && saved.id == expected_ticket,
            "mutable_projection_exact_real_graph_ticket",
        )?;
    }
    for fact in &carrier.input.facts {
        if fact.kind == "artifact"
            && fact.id == expected_ticket
            && !carrier
                .input
                .mutable_fact_projections
                .iter()
                .any(|saved| saved.kind == fact.kind && saved.id == fact.id)
        {
            gaps.insert("frozen_native_mutable_projection_missing");
        }
    }
    Ok(())
}

fn native_exposure_capture_r2(
    raw: &RawDatabase,
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
) -> Result<Option<evo_core::formal_version::NativeExposureCaptureV2>> {
    use evo_core::formal_version as fv;
    let physical_id = evaluation_storage_id(LEDGER_KIND, &ticket.research_family_id)?;
    let key = (
        namespace.to_owned(),
        "artifact".to_owned(),
        physical_id.clone(),
    );
    let Some(object) = snapshot.objects.get(&key) else {
        return Ok(None);
    };
    let ledger: ExposureLedger =
        evaluation_record(snapshot, namespace, LEDGER_KIND, &ticket.research_family_id)?;
    ledger.validate_restore()?;
    let Some(projection) = exposure_projection_r36(snapshot, namespace, ticket)? else {
        return Ok(None);
    };
    let row = complete_raw_object_r24(raw, namespace, "artifact", &physical_id)?;
    r36_check(
        object.namespace == namespace && object.kind == "artifact" && object.id == physical_id,
        "native_exposure_actual_identity",
    )?;
    Ok(Some(fv::NativeExposureCaptureV2 {
        schema_version: "rsia.e05.native_exposure_capture.v1".into(),
        namespace: namespace.into(),
        physical_id,
        original_owner: object.owner.clone(),
        ledger_schema_version: ledger.schema_version.clone(),
        raw_row_digest: hash(&row_bytes(row)),
        raw_body_digest: hash(native_text(row, "body")?.as_bytes()),
        typed_ledger_digest: typed_fingerprint(&ledger)?,
        projection_digest: typed_fingerprint(&projection)?,
    }))
}
fn validate_native_exposure_capture_ack_r2(
    raw: &RawDatabase,
    snapshot: &OccurrenceSnapshot,
    carrier: &FrozenCarrierR36,
    gaps: &mut BTreeSet<&'static str>,
) -> Result<()> {
    let expected = evaluation_storage_id(
        LEDGER_KIND,
        &carrier.input.ticket_snapshot.research_family_id,
    )?;
    if let Some(saved) = &carrier.input.native_exposure_capture {
        r36_check(
            saved.namespace == carrier.input.namespace && saved.physical_id == expected,
            "native_exposure_capture_exact_source_key",
        )?;
    }
    let captured = native_exposure_capture_r2(
        raw,
        snapshot,
        &carrier.input.namespace,
        &carrier.input.ticket_snapshot,
    )?;
    match (&carrier.input.native_exposure_capture, captured) {
        (Some(saved), Some(actual)) => r36_check(
            typed_fingerprint(saved)? == typed_fingerprint(&actual)?,
            "native_exposure_actual_INSERT_ACK_source",
        )?,
        _ => {
            gaps.insert("native_exposure_actual_capture_missing");
        }
    }
    Ok(())
}

fn freeze_native_early_carrier_r44(
    raw: &RawDatabase,
    snapshot: &OccurrenceSnapshot,
    ctx: &Context,
    ticket: &MirrorEvaluationTicketV2,
    formal: MirrorFormalEvaluationV2,
    occurred_at_millis: i64,
    operation: evo_core::formal_version::Operation,
) -> Result<FrozenCarrierR36> {
    use evo_core::formal_version as fv;
    ctx.require(&[Role::Evaluator])?;
    require(ticket.evaluator_actor == ctx.actor() && occurred_at_millis >= 0)?;
    let control = evaluation_control(snapshot, ctx.namespace(), &ticket.registration_id)?;
    let holdout = evaluation_holdout(snapshot, ctx.namespace(), &ticket.holdout_id, &control)?;
    producer_witnesses(snapshot, ctx.namespace(), &control, &holdout)?;
    e05_complete_ticket_identity(snapshot, ctx.namespace(), ticket, &control, &holdout)?;
    e05_complete_group(
        snapshot,
        ctx.namespace(),
        &control.billing_scope,
        &ticket.id,
    )?;
    require(
        matches!(
            operation,
            fv::Operation::EarlyStop | fv::Operation::Reconcile
        ) && ticket.lifecycle == MirrorTicketLifecycle::EarlyStopped
            && matches!(formal, MirrorFormalEvaluationV2::EarlyStopped { .. }),
    )?;
    let first_stop = first_stop_source_from_snapshot_r44(snapshot, ctx.namespace(), ticket)?
        .ok_or_else(|| r24_fault(fv::Fault::SourceInsufficient))?;
    reconstruct_first_stop_r44(ticket, &first_stop)?;
    let ticket_physical = evaluation_storage_id(TICKET_KIND, &ticket.id)?;
    let roots = vec![("artifact".into(), ticket_physical)];
    let nodes = derive_nodes(snapshot, ctx.namespace(), roots)?;
    let mut facts = Vec::with_capacity(nodes.len());
    for node in nodes {
        // The original derive_nodes codec gives each ORIGINAL typed record fingerprint.
        // Never substitute serde_json::Value ordering for node.content_digest.
        let object = snapshot.object(ctx.namespace(), &node.kind, &node.id)?;
        let row = complete_raw_object_r24(raw, ctx.namespace(), &node.kind, &node.id)?;
        let outgoing = e05_outgoing(snapshot, ctx.namespace(), &node.kind, &node.id)?;
        require(
            outgoing.len() == node.outgoing_edge_count
                && typed_fingerprint(&outgoing)? == node.outgoing_edge_digest,
        )?;
        facts.push(fv::Fact {
            namespace: ctx.namespace().into(),
            kind: node.kind,
            id: node.id,
            owner: object.owner.clone(),
            schema_version: node.schema_version,
            record_kind: node.record_kind,
            logical_id: node.logical_id,
            raw_row_digest: hash(&row_bytes(row)),
            raw_body_digest: hash(native_text(row, "body")?.as_bytes()),
            typed_record_digest: node.content_digest,
            outgoing,
            outgoing_digest: node.outgoing_edge_digest,
        });
    }
    facts.sort_by(|a, b| (&a.namespace, &a.kind, &a.id).cmp(&(&b.namespace, &b.kind, &b.id)));
    let actual_calls: Vec<_> = snapshot
        .calls
        .values()
        .filter(|call| {
            call.billing_scope == control.billing_scope && call.dispatch_group_id == ticket.id
        })
        .collect();
    let mut calls = Vec::with_capacity(actual_calls.len());
    for actual in actual_calls {
        // Full group includes other namespaces. R19 rejects a contradicted namespace;
        // no prior namespace filter can hide a foreign or unattributed call.
        let frozen = FrozenCallR24::freeze_from_qualified_snapshot(
            snapshot,
            ctx.namespace(),
            &control.billing_scope,
            &actual.call_id,
        )
        .map_err(frozen_call_error_r24)?;
        let original = frozen
            .reconstruct_from_qualified_snapshot(
                snapshot,
                ctx.namespace(),
                &control.billing_scope,
                &actual.call_id,
            )
            .map_err(frozen_call_error_r24)?;
        require(
            &original == actual && typed_fingerprint(&original)? == typed_fingerprint(actual)?,
        )?;
        calls.push(fv::Call {
            namespace: actual.namespace.clone(),
            billing_scope: actual.billing_scope.clone(),
            call_id: actual.call_id.clone(),
            snapshot_digest: typed_fingerprint(&frozen)?,
            snapshot: frozen,
        });
    }
    calls.sort_by(|a, b| {
        (&a.namespace, &a.billing_scope, &a.call_id).cmp(&(
            &b.namespace,
            &b.billing_scope,
            &b.call_id,
        ))
    });
    let input = fv::FrozenInputV2 {
        schema_version: fv::INPUT_SCHEMA_V2.into(),
        resource_scope: fv::FrozenResourceScopeV2::TicketExecutionOnly,
        mutable_fact_projections: native_mutable_projections_r39(
            raw,
            snapshot,
            ctx.namespace(),
            ticket,
            &facts,
        )?,
        native_exposure_capture: native_exposure_capture_r2(
            raw,
            snapshot,
            ctx.namespace(),
            ticket,
        )?,
        native_first_stop: Some(fv::OriginalValueV2::from_snapshot(first_stop).map_err(r24_fault)?),
        exposure: exposure_projection_r36(snapshot, ctx.namespace(), ticket)?
            .map(fv::OriginalValueV2::from_snapshot)
            .transpose()
            .map_err(r24_fault)?,
        root_budget: snapshot
            .roots
            .get(&control.billing_scope)
            .map(root_projection_r36)
            .map(fv::OriginalValueV2::from_snapshot)
            .transpose()
            .map_err(r24_fault)?,
        dispatch_group: snapshot
            .groups
            .get(&(control.billing_scope.clone(), ticket.id.clone()))
            .map(group_projection_r36)
            .map(fv::OriginalValueV2::from_snapshot)
            .transpose()
            .map_err(r24_fault)?,

        namespace: ctx.namespace().into(),
        ticket_id: ticket.id.clone(),
        ticket_digest: typed_fingerprint(ticket)?,
        ticket_snapshot: ticket.clone(),
        intent_digest: typed_fingerprint(&(
            "rsia.e05.native_early_intent.v1",
            &operation,
            &ticket.id,
            ticket
                .pending_certificate
                .as_ref()
                .ok_or(Error::ClockRegressed)?,
        ))?,
        operation,
        occurred_at_unix_ms: occurred_at_millis,
        facts,
        calls,
        watermark: snapshot
            .watermarks
            .get(ctx.namespace())
            .map(|(seq, d)| {
                i64::try_from(*seq)
                    .map(|seq| (seq, d.clone()))
                    .map_err(|_| Error::ClockRegressed)
            })
            .transpose()?,
    };
    let carrier = fv::CarrierV4::from_frozen_input(input, formal).map_err(r24_fault)?;
    validate_complete_native_capture_r36(raw, &carrier, snapshot)?;
    Ok(carrier)
}
fn freeze_complete_carrier_r36(
    raw: &RawDatabase,
    snapshot: &OccurrenceSnapshot,
    ctx: &Context,
    ticket: &MirrorEvaluationTicketV2,
    formal: MirrorFormalEvaluationV2,
    occurred_at_millis: i64,
) -> Result<FrozenCarrierR36> {
    use evo_core::formal_version as fv;
    // Preserve the actual engine3/input1 codec+producer for real old constants.
    // This invocation uses the SAME actual snapshot, never an old saved report
    // or a later current row to backfill an original. No old record is written.
    let old = freeze_complete_carrier_r24(raw, snapshot, ctx, ticket, formal, occurred_at_millis)?;
    let control = evaluation_control(snapshot, ctx.namespace(), &ticket.registration_id)?;
    let exposure = exposure_projection_r36(snapshot, ctx.namespace(), ticket)?
        .map(fv::OriginalValueV2::from_snapshot)
        .transpose()
        .map_err(r24_fault)?;
    let root_budget = snapshot
        .roots
        .get(&control.billing_scope)
        .map(root_projection_r36)
        .map(fv::OriginalValueV2::from_snapshot)
        .transpose()
        .map_err(r24_fault)?;
    let dispatch_group = snapshot
        .groups
        .get(&(control.billing_scope.clone(), ticket.id.clone()))
        .map(group_projection_r36)
        .map(fv::OriginalValueV2::from_snapshot)
        .transpose()
        .map_err(r24_fault)?;
    let mutable_fact_projections =
        native_mutable_projections_r39(raw, snapshot, ctx.namespace(), ticket, &old.input.facts)?;
    let native_exposure_capture =
        native_exposure_capture_r2(raw, snapshot, ctx.namespace(), ticket)?;
    let input = fv::FrozenInputV2 {
        schema_version: fv::INPUT_SCHEMA_V2.into(),
        namespace: old.input.namespace,
        ticket_id: old.input.ticket_id,
        ticket_digest: old.input.ticket_digest,
        ticket_snapshot: old.input.ticket_snapshot,
        operation: old.input.operation,
        intent_digest: old.input.intent_digest,
        occurred_at_unix_ms: old.input.occurred_at_unix_ms,
        facts: old.input.facts,
        calls: old.input.calls,
        watermark: old.input.watermark,
        resource_scope: fv::FrozenResourceScopeV2::TicketExecutionOnly,
        mutable_fact_projections,
        native_exposure_capture,
        native_first_stop: first_stop_source_from_snapshot_r44(snapshot, ctx.namespace(), ticket)?
            .map(fv::OriginalValueV2::from_snapshot)
            .transpose()
            .map_err(r24_fault)?,
        exposure,
        root_budget,
        dispatch_group,
    };
    // The full core factory checks missing data, so do not invoke it before
    // storage's concrete all-known validation and entity/call/source checks.
    input.validate_known_wire().map_err(r24_fault)?;
    let input_digest = typed_fingerprint(&input)?;
    let carrier = fv::CarrierV4 {
        schema_version: fv::SCHEMA.into(),
        record_kind: fv::KIND.into(),
        id: fv::address(
            ctx.namespace(),
            fv::ENGINE_V4,
            fv::INPUT_SCHEMA_V2,
            &input_digest,
        )
        .map_err(r24_fault)?,
        engine_version: fv::ENGINE_V4.into(),
        input_schema_version: fv::INPUT_SCHEMA_V2.into(),
        input_digest,
        payload_digest: typed_fingerprint(&old.payload)?,
        input,
        payload: old.payload,
    };
    validate_complete_native_capture_r36(raw, &carrier, snapshot)?;
    Ok(carrier)
}

const FIRST_STOP_ASOF_ENTITY_R44: &str = "rsia.native_first_stop_asof_entity.v1";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeFirstStopAsOfEntityR44 {
    schema_version: String,
    summary: StoredAsOfSummaryR24,
    native_first_stop_digest: String,
}
fn first_stop_sha_r44(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
fn validate_first_stop_summary_r44(summary: &StoredAsOfSummaryR24) -> Result<()> {
    r44_stop_require(
        summary.schema_version == "rsia.native_asof.audit_summary.v1"
            && summary.role == Role::Evaluator
            && summary.operation == "formal.native.asof.grade.v1"
            && !summary.namespace.is_empty()
            && !summary.actor.is_empty()
            && !summary.ticket_id.is_empty()
            && summary.capture_seconds >= 0
            && summary.capture_millis >= 0
            && summary.capture_millis / 1000 == summary.capture_seconds
            && summary.audit_timestamp >= summary.capture_seconds
            && summary.previous_audit_sequence >= 0
            && summary.acknowledged_writes > 0,
    )?;
    for value in [
        &summary.actual_ticket_row_digest,
        &summary.capture_database_digest,
        &summary.prepared_prefix_digest,
        &summary.closed_ack_prefix_digest,
        &summary.after_legacy_audit_prefix_digest,
    ] {
        r44_stop_require(first_stop_sha_r44(value))?;
    }
    r44_stop_require(if summary.previous_audit_sequence == 0 {
        summary.previous_audit_hash.is_empty()
    } else {
        first_stop_sha_r44(&summary.previous_audit_hash)
    })?;
    Ok(())
}
// This codec carries full original summary bytes, not a reverse lookup of a
// summary hash. It is data only; native construction/ACK and the real audit
// chain are still required and no DTO can construct a permit or Current value.
fn decode_first_stop_asof_entity_r44(entity: &str) -> Result<Option<NativeFirstStopAsOfEntityR44>> {
    if first_stop_sha_r44(entity) {
        return Ok(None);
    }
    let value = parse_owned_json(entity)
        .map_err(|_| r24_fault(evo_core::formal_version::Fault::InvalidContent))?;
    let wrapper: NativeFirstStopAsOfEntityR44 = serde_json::from_value(value)
        .map_err(|_| r24_fault(evo_core::formal_version::Fault::InvalidContent))?;
    r44_stop_require(
        wrapper.schema_version == FIRST_STOP_ASOF_ENTITY_R44
            && first_stop_sha_r44(&wrapper.native_first_stop_digest),
    )?;
    validate_first_stop_summary_r44(&wrapper.summary)?;
    r44_stop_require(serde_json::to_string(&wrapper).map_err(crate::internal)? == entity)?;
    Ok(Some(wrapper))
}
fn native_asof_entity_r44<T: Serialize>(summary: &T, first_stop: Option<&str>) -> Result<String> {
    let Some(first_stop) = first_stop else {
        return typed_fingerprint(summary);
    };
    r44_stop_require(first_stop_sha_r44(first_stop))?;
    let saved: StoredAsOfSummaryR24 =
        serde_json::from_value(serde_json::to_value(summary).map_err(crate::internal)?)
            .map_err(crate::internal)?;
    validate_first_stop_summary_r44(&saved)?;
    serde_json::to_string(&NativeFirstStopAsOfEntityR44 {
        schema_version: FIRST_STOP_ASOF_ENTITY_R44.into(),
        summary: saved,
        native_first_stop_digest: first_stop.into(),
    })
    .map_err(crate::internal)
}

fn validate_first_stop_asof_row_r44(
    raw: &RawDatabase,
    row: &OwnedRow,
    payload: &AuditPayloadR24,
    summary: &StoredAsOfSummaryR24,
) -> Result<()> {
    validate_first_stop_summary_r44(summary)?;
    let sequence = native_integer(row, "seq")?;
    let payload_text = native_text(row, "payload")?;
    let previous = native_text(row, "previous_hash")?;
    r44_stop_require(
        native_text(row, "namespace")? == summary.namespace
            && payload.actor == summary.actor
            && payload.role == summary.role
            && payload.operation == summary.operation
            && payload.timestamp == summary.audit_timestamp
            && previous == summary.previous_audit_hash
            && sequence > summary.previous_audit_sequence
            && native_text(row, "digest")?
                == hash(format!("{previous}\n{payload_text}").as_bytes()),
    )?;
    let mut prior_sequence = 0_i64;
    let mut prior_hash = String::new();
    for prior in raw.tables.get("audit").ok_or(Error::ClockRegressed)? {
        if native_text(prior, "namespace")? != summary.namespace {
            continue;
        }
        let candidate = native_integer(prior, "seq")?;
        if candidate < sequence && candidate > prior_sequence {
            prior_sequence = candidate;
            prior_hash = native_text(prior, "digest")?;
        }
    }
    r44_stop_require(
        prior_sequence == summary.previous_audit_sequence
            && prior_hash == summary.previous_audit_hash,
    )?;
    Ok(())
}

fn check_native_asof_entity_r44<T: Serialize>(entity: &str, summary: &T) -> Result<Option<String>> {
    match decode_first_stop_asof_entity_r44(entity)? {
        None => {
            require(entity == typed_fingerprint(summary)?)?;
            Ok(None)
        }
        Some(wrapper) => {
            r44_stop_require(typed_fingerprint(&wrapper.summary)? == typed_fingerprint(summary)?)?;
            Ok(Some(wrapper.native_first_stop_digest))
        }
    }
}
fn operation_effect_matches_r44(
    effect: NativePlanEffectR1,
    operation: &NativeAsOfOperation,
) -> bool {
    matches!(
        (effect, operation),
        (
            NativePlanEffectR1::NewEarlyR44,
            NativeAsOfOperation::Grade | NativeAsOfOperation::Reconcile
        ) | (
            NativePlanEffectR1::NewComplete | NativePlanEffectR1::NewInvalid,
            NativeAsOfOperation::Complete
        )
    )
}

const COMPLETE_BINDING_OPERATION_R24: &str = "formal.native.report_binding.v1";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredAsOfSummaryR24 {
    schema_version: String,
    namespace: String,
    actor: String,
    role: Role,
    operation: String,
    ticket_id: String,
    actual_ticket_row_digest: String,
    capture_seconds: i64,
    capture_millis: i64,
    capture_database_digest: String,
    prepared_prefix_digest: String,
    closed_ack_prefix_digest: String,
    after_legacy_audit_prefix_digest: String,
    acknowledged_writes: usize,
    previous_audit_sequence: i64,
    previous_audit_hash: String,
    audit_timestamp: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuditPayloadR24 {
    actor: String,
    role: Role,
    operation: String,
    entity: String,
    timestamp: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompleteBindingR24 {
    schema_version: String,
    summary: StoredAsOfSummaryR24,
    asof_sequence: i64,
    asof_previous: String,
    asof_digest: String,
    asof_payload: String,
    report_ref: evo_core::formal_version::ReportRef,
    insert_index: usize,
    insert_row_digest: String,
}
// These are static actual-row metadata, not a current/native-read permit.
#[derive(Clone)]
pub(super) struct CompleteOriginR24 {
    reference: evo_core::formal_version::ReportRef,
}
// Constructible only from the exact plan's already executed INSERT ACK.
pub(super) struct CompletePendingR24 {
    namespace: String,
    reference: evo_core::formal_version::ReportRef,
    outgoing: Vec<(String, String)>,
}
struct CompleteFrozenStateR24 {
    carrier: FrozenCarrierR36,
    reference: evo_core::formal_version::ReportRef,
    insert_index: usize,
    clock: crate::write_clock::NativeEventBinding,
}
fn decode_complete_raw_r24(
    raw: &RawDatabase,
    reference: &evo_core::formal_version::ReportRef,
) -> Result<(FrozenCarrierR24, String)> {
    reference.validate_wire().map_err(r24_fault)?;
    let row = complete_raw_object_r24(
        raw,
        &reference.namespace,
        "artifact",
        &reference.physical_id,
    )?;
    let body = native_text(row, "body")?;
    require(body.len() <= 4 * 1024 * 1024 && native_text(row, "owner")? == reference.owner)?;
    parse_owned_json(&body)?;
    let carrier: FrozenCarrierR24 = serde_json::from_str(&body).map_err(crate::internal)?;
    validate_complete_carrier_r24(&carrier, &reference.namespace, &reference.physical_id)?;
    // Full TYPED carrier reference includes payload_digest/ticket_digest/input_digest.
    // A well-formed but forged digest in a DTO cannot survive this equality.
    require(
        carrier.report_ref(&reference.owner).map_err(r24_fault)? == *reference
            && hash(body.as_bytes()) == reference.carrier_digest
            && carrier.payload_digest == reference.payload_digest
            && carrier.input.ticket_digest == reference.ticket_digest,
    )?;
    Ok((carrier, hash(&row_bytes(row))))
}
// Validate known typed data/reference first, without returning missing yet.
// Callers must finish either static-wire or actual native source validation.
fn known_complete_reference_r36(
    carrier: &FrozenCarrierR36,
    owner: &str,
) -> Result<evo_core::formal_version::ReportRef> {
    use evo_core::formal_version as fv;
    validate_complete_known_r36(carrier, &carrier.input.namespace, &carrier.id)?;
    identifier(owner)?;
    let reference = fv::ReportRef {
        schema_version: fv::REF_SCHEMA.into(),
        namespace: carrier.input.namespace.clone(),
        owner: owner.into(),
        physical_id: carrier.id.clone(),
        engine_version: carrier.engine_version.clone(),
        input_schema_version: carrier.input_schema_version.clone(),
        input_digest: carrier.input_digest.clone(),
        payload_digest: carrier.payload_digest.clone(),
        carrier_digest: typed_fingerprint(carrier)?,
        ticket_id: carrier.input.ticket_id.clone(),
        ticket_digest: carrier.input.ticket_digest.clone(),
    };
    reference.validate_wire().map_err(r24_fault)?;
    Ok(reference)
}
fn decode_complete_known_raw_r36(
    raw: &RawDatabase,
    reference: &evo_core::formal_version::ReportRef,
) -> Result<(FrozenCarrierR36, String)> {
    reference.validate_wire().map_err(r24_fault)?;
    let row = complete_raw_object_r24(
        raw,
        &reference.namespace,
        "artifact",
        &reference.physical_id,
    )?;
    let body = native_text(row, "body")?;
    r36_check(
        body.len() <= 4 * 1024 * 1024 && native_text(row, "owner")? == reference.owner,
        "original_carrier_row",
    )?;
    parse_owned_json(&body)?;
    let carrier: FrozenCarrierR36 = serde_json::from_str(&body).map_err(crate::internal)?;
    validate_complete_known_r36(&carrier, &reference.namespace, &reference.physical_id)?;
    // Do not call report_ref() before concrete all-known checks: its factory
    // includes a missing-source branch. These are the same exact typed fields,
    // all rederived here and compared; this private DTO grants no permission.
    let actual = known_complete_reference_r36(&carrier, &reference.owner)?;
    r36_check(
        actual == *reference
            && hash(body.as_bytes()) == reference.carrier_digest
            && carrier.input.ticket_snapshot.evaluator_actor == reference.owner,
        "known_typed_carrier_reference",
    )?;
    Ok((carrier, hash(&row_bytes(row))))
}
fn decode_complete_native_raw_r36(
    raw: &RawDatabase,
    reference: &evo_core::formal_version::ReportRef,
) -> Result<(FrozenCarrierR36, String)> {
    let result = decode_complete_known_raw_r36(raw, reference)?;
    let (actual, _) = snapshot(raw)?;
    validate_complete_native_capture_r36(raw, &result.0, &actual)?;
    Ok(result)
}
// Small static metadata, not a large carrier enum or a Current read owner.
struct CompleteOriginMaterialR36 {
    occurred_at_millis: i64,
    ticket_raw_row_digest: String,
    carrier_raw_row_digest: String,
}
fn complete_origin_material_r36(
    raw: &RawDatabase,
    reference: &evo_core::formal_version::ReportRef,
) -> Result<CompleteOriginMaterialR36> {
    use evo_core::formal_version as fv;
    reference.validate_wire().map_err(r24_fault)?;
    let ticket_physical = evaluation_storage_id(TICKET_KIND, &reference.ticket_id)?;
    match (
        reference.engine_version.as_str(),
        reference.input_schema_version.as_str(),
    ) {
        (fv::ENGINE, fv::INPUT_SCHEMA) => {
            let (carrier, row_digest) = decode_complete_raw_r24(raw, reference)?;
            let saved = carrier
                .input
                .facts
                .iter()
                .find(|fact| {
                    fact.namespace == reference.namespace
                        && fact.kind == "artifact"
                        && fact.id == ticket_physical
                })
                .ok_or(Error::ClockRegressed)?;
            Ok(CompleteOriginMaterialR36 {
                occurred_at_millis: carrier.input.occurred_at_unix_ms,
                ticket_raw_row_digest: saved.raw_row_digest.clone(),
                carrier_raw_row_digest: row_digest,
            })
        }
        (fv::ENGINE_V4, fv::INPUT_SCHEMA_V2) => {
            let (carrier, row_digest) = decode_complete_known_raw_r36(raw, reference)?;
            let saved = carrier
                .input
                .facts
                .iter()
                .find(|fact| {
                    fact.namespace == reference.namespace
                        && fact.kind == "artifact"
                        && fact.id == ticket_physical
                })
                .ok_or(Error::ClockRegressed)?;
            Ok(CompleteOriginMaterialR36 {
                occurred_at_millis: carrier.input.occurred_at_unix_ms,
                ticket_raw_row_digest: saved.raw_row_digest.clone(),
                carrier_raw_row_digest: row_digest,
            })
        }
        _ => Err(r24_fault(fv::Fault::UnknownVersion)),
    }
}
fn actual_audit_row_r24<'a>(
    raw: &'a RawDatabase,
    namespace: &str,
    seq: i64,
) -> Result<&'a OwnedRow> {
    let mut rows = raw
        .tables
        .get("audit")
        .ok_or(Error::ClockRegressed)?
        .iter()
        .filter(|row| {
            native_integer(row, "seq").ok() == Some(seq)
                && native_text(row, "namespace").ok().as_deref() == Some(namespace)
        });
    let row = rows.next().ok_or(Error::ClockRegressed)?;
    require(rows.next().is_none())?;
    Ok(row)
}
// collect_audit has already validated the COMPLETE raw audit chain. This adds
// exact native-origin bindings, including every report rather than only last op.
pub(super) fn load_complete_origins_r24(
    raw: &RawDatabase,
    snapshot: &mut OccurrenceSnapshot,
) -> Result<()> {
    for row in raw.tables.get("audit").ok_or(Error::ClockRegressed)? {
        let payload_raw = native_text(row, "payload")?;
        parse_owned_json(&payload_raw)?;
        let payload: AuditPayloadR24 =
            serde_json::from_str(&payload_raw).map_err(crate::internal)?;
        if payload.operation != COMPLETE_BINDING_OPERATION_R24 {
            continue;
        }
        require(payload.entity.len() <= 4 * 1024 * 1024)?;
        parse_owned_json(&payload.entity)?;
        let binding: CompleteBindingR24 =
            serde_json::from_str(&payload.entity).map_err(crate::internal)?;
        let summary = &binding.summary;
        let ns = native_text(row, "namespace")?;
        require(
            binding.schema_version == "rsia.native.complete_binding.v1"
                && summary.schema_version == "rsia.native_asof.audit_summary.v1"
                && summary.namespace == ns
                && payload.actor == summary.actor
                && payload.role == Role::Evaluator
                && summary.role == Role::Evaluator
                && matches!(
                    summary.operation.as_str(),
                    "formal.native.asof.complete.v1"
                        | "formal.native.asof.grade.v1"
                        | "formal.native.asof.reconcile.v1"
                )
                && payload.timestamp >= summary.audit_timestamp
                && summary.capture_seconds >= 0
                && summary.capture_millis >= 0
                && summary.capture_millis / 1000 == summary.capture_seconds
                && summary.audit_timestamp >= summary.capture_seconds
                && binding.asof_sequence > summary.previous_audit_sequence
                && native_integer(row, "seq")? > binding.asof_sequence
                && native_text(row, "previous_hash")? == binding.asof_digest
                && binding.asof_previous == summary.previous_audit_hash
                && binding.insert_index < summary.acknowledged_writes,
        )?;
        let prior_tip = raw
            .tables
            .get("audit")
            .ok_or(Error::ClockRegressed)?
            .iter()
            .filter(|candidate| {
                native_text(candidate, "namespace").ok().as_deref() == Some(&ns)
                    && native_integer(candidate, "seq").is_ok_and(|seq| seq < binding.asof_sequence)
            })
            .map(|candidate| native_integer(candidate, "seq"))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .max()
            .unwrap_or(0);
        require(prior_tip == summary.previous_audit_sequence)?;
        let actual_asof = actual_audit_row_r24(raw, &ns, binding.asof_sequence)?;
        require(
            native_text(actual_asof, "payload")? == binding.asof_payload
                && native_text(actual_asof, "previous_hash")? == binding.asof_previous
                && native_text(actual_asof, "digest")? == binding.asof_digest
                && hash(format!("{}\n{}", binding.asof_previous, binding.asof_payload).as_bytes())
                    == binding.asof_digest,
        )?;
        parse_owned_json(&binding.asof_payload)?;
        let asof: AuditPayloadR24 =
            serde_json::from_str(&binding.asof_payload).map_err(crate::internal)?;
        let first_stop_entity = check_native_asof_entity_r44(&asof.entity, summary)?;
        if first_stop_entity.is_some() {
            validate_first_stop_asof_row_r44(raw, actual_asof, &asof, summary)?;
        }
        require(
            asof.actor == summary.actor
                && asof.role == summary.role
                && asof.operation == summary.operation
                && asof.timestamp == summary.audit_timestamp,
        )?;
        for value in [
            &summary.actual_ticket_row_digest,
            &summary.capture_database_digest,
            &summary.prepared_prefix_digest,
            &summary.closed_ack_prefix_digest,
            &summary.after_legacy_audit_prefix_digest,
            &binding.asof_digest,
            &binding.insert_row_digest,
        ] {
            digest(value)?;
        }
        if summary.previous_audit_sequence == 0 {
            require(summary.previous_audit_hash.is_empty())?;
        } else {
            digest(&summary.previous_audit_hash)?;
        }
        let reference = &binding.report_ref;
        require(
            reference.namespace == ns
                && reference.owner == summary.actor
                && reference.ticket_id == summary.ticket_id,
        )?;
        let (actual_carrier, _) = decode_complete_known_raw_r36(raw, &binding.report_ref)?;
        let expected_operation = match &actual_carrier.input.operation {
            evo_core::formal_version::Operation::EarlyStop => "formal.native.asof.grade.v1",
            evo_core::formal_version::Operation::Reconcile => "formal.native.asof.reconcile.v1",
            _ => "formal.native.asof.complete.v1",
        };
        require(summary.operation == expected_operation)?;
        if let Some(first_stop_entity) = first_stop_entity {
            r44_stop_require(matches!(
                &actual_carrier.input.operation,
                evo_core::formal_version::Operation::EarlyStop
            ))?;
            if let Some(saved) = actual_carrier.input.native_first_stop.as_ref() {
                r44_stop_require(
                    saved.typed_digest == first_stop_entity
                        && typed_fingerprint(&saved.snapshot)? == first_stop_entity,
                )?;
            }
            // A missing capsule remains an explicit final source gap. The
            // metadata origin is not itself a Current/content capability.
        }
        let material = complete_origin_material_r36(raw, reference)?;
        require(
            material.carrier_raw_row_digest == binding.insert_row_digest
                && material.occurred_at_millis >= summary.capture_millis
                && summary.audit_timestamp >= material.occurred_at_millis / 1000
                && material.ticket_raw_row_digest == summary.actual_ticket_row_digest,
        )?;
        require(
            snapshot
                .complete_origins_r24
                .insert(
                    (ns, reference.physical_id.clone()),
                    CompleteOriginR24 {
                        reference: reference.clone(),
                    },
                )
                .is_none(),
        )?;
    }
    Ok(())
}
pub(super) fn qualify_frozen_complete_r24(
    object: &OccurrenceObject,
    snapshot: &OccurrenceSnapshot,
    floor: &mut OccurrenceFloor,
) -> Result<()> {
    require(object.kind == "artifact")?;
    use evo_core::formal_version as fv;
    let reference = match (
        object.body.get("engine_version").and_then(Value::as_str),
        object
            .body
            .get("input_schema_version")
            .and_then(Value::as_str),
    ) {
        (Some(fv::ENGINE), Some(fv::INPUT_SCHEMA)) => {
            let carrier: FrozenCarrierR24 = decode(&object.body)?;
            validate_complete_carrier_r24(&carrier, &object.namespace, &object.id)?;
            require(object.owner == carrier.input.ticket_snapshot.evaluator_actor)?;
            floor.observe_millis(carrier.input.occurred_at_unix_ms)?;
            floor.observe_seconds(carrier.input.ticket_snapshot.issued_at_unix_seconds)?;
            for call in &carrier.input.calls {
                call.snapshot
                    .observe_saved_times(floor)
                    .map_err(frozen_call_error_r24)?;
            }
            carrier.report_ref(&object.owner).map_err(r24_fault)?
        }
        (Some(fv::ENGINE_V4), Some(fv::INPUT_SCHEMA_V2)) => {
            let carrier: FrozenCarrierR36 = decode(&object.body)?;
            validate_complete_known_r36(&carrier, &object.namespace, &object.id)?;
            // Static preserved material only. All known concrete descriptors
            // and present entity identities preceded this missing-source check.
            r36_check(
                object.owner == carrier.input.ticket_snapshot.evaluator_actor,
                "carrier_object_owner",
            )?;
            carrier
                .validate_wire(&object.namespace, &object.id)
                .map_err(r24_fault)?;
            floor.observe_millis(carrier.input.occurred_at_unix_ms)?;
            floor.observe_seconds(carrier.input.ticket_snapshot.issued_at_unix_seconds)?;
            for call in &carrier.input.calls {
                call.snapshot
                    .observe_saved_times(floor)
                    .map_err(frozen_call_error_r24)?;
            }
            carrier.report_ref(&object.owner).map_err(r24_fault)?
        }
        _ => return Err(r24_fault(fv::Fault::UnknownVersion)),
    };
    if let Some(origin) = snapshot
        .complete_origins_r24
        .get(&(object.namespace.clone(), object.id.clone()))
    {
        require(origin.reference == reference)?;
        // Origin is static proof of this preserved wire, NOT Current or new-use permission.
        return Ok(());
    }
    let pending = snapshot
        .complete_pending_r24
        .as_ref()
        .ok_or(Error::ClockRegressed)?;
    require(
        pending.namespace == object.namespace
            && pending.reference == reference
            && e05_outgoing(snapshot, &object.namespace, "artifact", &object.id)?
                == pending.outgoing,
    )?;
    // Whole raw database equality and exact INSERT row ACK were checked by
    // E05CompletePlan immediately before this private pending evidence was made.
    Ok(())
}

struct CompleteInsertAckR24 {
    reference: evo_core::formal_version::ReportRef,
    insert_index: usize,
    insert_row_digest: String,
}
struct CompleteBindingAckR24 {
    reference: evo_core::formal_version::ReportRef,
    asof_sequence: i64,
    sequence: i64,
    previous: String,
    payload: String,
    digest: String,
}
struct CommittedCompleteR24 {
    reference: evo_core::formal_version::ReportRef,
    asof_sequence: i64,
    binding_sequence: i64,
    binding_digest: String,
}
impl E05CompletePlan {
    fn insert_ack_r24(&self) -> Result<CompleteInsertAckR24> {
        let frozen = self.frozen_r24.as_ref().ok_or(Error::ClockRegressed)?;
        require(self.materialized && self.prefix.next > frozen.insert_index)?;
        let IssueWrite::Object {
            before: None,
            after,
        } = self
            .prefix
            .writes
            .get(frozen.insert_index)
            .ok_or(Error::ClockRegressed)?
        else {
            return Err(Error::ClockRegressed);
        };
        require(
            native_text(after, "namespace")? == self.prefix.namespace
                && native_text(after, "kind")? == "artifact"
                && native_text(after, "id")? == frozen.reference.physical_id
                && native_integer(after, "revision")? == 1,
        )?;
        let (actual, row_digest) =
            decode_complete_native_raw_r36(&self.prefix.expected, &frozen.reference)?;
        require(
            typed_fingerprint(&actual)? == typed_fingerprint(&frozen.carrier)?
                && carrier_effect_r1(&actual.input.operation, &actual.payload)?
                    == self.work_r1.effect()
                && row_digest == hash(&row_bytes(after)),
        )?;
        Ok(CompleteInsertAckR24 {
            reference: frozen.reference.clone(),
            insert_index: frozen.insert_index,
            insert_row_digest: row_digest,
        })
    }
    fn pending_r24(&self) -> Result<Option<CompletePendingR24>> {
        let Some(frozen) = &self.frozen_r24 else {
            return Ok(None);
        };
        if self.prefix.next <= frozen.insert_index {
            return Ok(None);
        }
        let member = self.insert_ack_r24()?;
        let (snapshot, _) = snapshot(&self.prefix.expected)?;
        Ok(Some(CompletePendingR24 {
            namespace: self.prefix.namespace.clone(),
            reference: member.reference,
            outgoing: e05_outgoing(
                &snapshot,
                &self.prefix.namespace,
                "artifact",
                &frozen.reference.physical_id,
            )?,
        }))
    }
    fn qualify_pending_raw_r24(
        &self,
        raw: &RawDatabase,
        inputs: &PreparePreimages,
    ) -> Result<OccurrenceFloor> {
        // Original append qualifier still checks entire raw equality, actor,
        // control/holdout/source witnesses, candidate binding, ALL objects/caches.
        self.prefix
            .qualify_with_pending_r24(raw, inputs, self.pending_r24()?)
    }
    async fn admit_pending_r24(&self, session: &mut Session) -> Result<()> {
        session.clock.check()?;
        if let Some(frozen) = &self.frozen_r24 {
            session.clock.require_event_binding(&frozen.clock)?;
        }
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        require(
            SqliteTransactionManager::get_transaction_depth(&**tx) == 1 && tx.is_in_transaction(),
        )?;
        let raw = load_sqlx_database(tx)
            .await
            .map_err(|_| session.clock.fail())?;
        let floor = self
            .qualify_pending_raw_r24(&raw, &session.prepare_inputs)
            .map_err(|_| session.clock.fail())?;
        let stamp = session.clock.seconds(&floor)?;
        session.clock.take_seconds(stamp)?;
        Ok(())
    }
    async fn admit_all_pending_r24(&self, session: &mut Session) -> Result<()> {
        require(self.prefix.next == self.prefix.writes.len())?;
        self.admit_pending_r24(session).await
    }
}
impl OwnedOperationPlan {
    fn actual_effect_r1(&self) -> Result<NativePlanEffectR1> {
        if let Self::Complete(plan) = self {
            return Ok(plan.work_r1.effect());
        }
        for write in self.writes() {
            if let IssueWrite::Object { after, .. } = write
                && native_text(after, "kind")? == "artifact"
            {
                let body = parse_owned_json(&native_text(after, "body")?)?;
                if body.get("record_kind").and_then(Value::as_str) == Some(FORMAL_KIND) {
                    return Ok(NativePlanEffectR1::LegacyFormalWrites);
                }
            }
        }
        Ok(
            if self.writes().is_empty() && self.native_legacy_audit().is_none() {
                NativePlanEffectR1::NoNewFormal
            } else {
                NativePlanEffectR1::NonFormalWrites
            },
        )
    }
    // A no-write plan can still follow real budget preparation. Only exact
    // equality with the actual captured database identifies the legal no-op.
    fn bound_effect_r1(&self, capture: &str, prepared: &str) -> Result<NativePlanEffectR1> {
        match self.actual_effect_r1()? {
            NativePlanEffectR1::NoNewFormal if capture != prepared => {
                Ok(NativePlanEffectR1::NonFormalWrites)
            }
            other => Ok(other),
        }
    }
    fn qualify_native_prefix_r24(
        &self,
        raw: &RawDatabase,
        inputs: &PreparePreimages,
    ) -> Result<OccurrenceFloor> {
        same_database(raw, self.native_expected())?;
        match self {
            Self::Complete(plan) => plan.qualify_pending_raw_r24(raw, inputs),
            _ => collect_raw(raw, inputs, SchemaRead::Stable),
        }
    }
    fn complete_insert_ack_r24(&self) -> Result<Option<CompleteInsertAckR24>> {
        match self {
            Self::Complete(plan) => {
                require(plan.prefix.next == plan.prefix.writes.len())?;
                Ok(Some(plan.insert_ack_r24()?))
            }
            _ => Ok(None),
        }
    }
    async fn admit_final_stable_r24(&self, session: &mut Session) -> Result<()> {
        require(self.native_next() == self.writes().len())?;
        session.clock.check()?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        require(
            tx.is_in_transaction() && SqliteTransactionManager::get_transaction_depth(&**tx) == 1,
        )?;
        let raw = load_sqlx_database(tx)
            .await
            .map_err(|_| session.clock.fail())?;
        same_database(&raw, self.native_expected())?;
        // Deliberately NO pending phase, callback or DTO on the final check.
        let floor = collect_raw(&raw, &session.prepare_inputs, SchemaRead::Stable)
            .map_err(|_| session.clock.fail())?;
        let stamp = session.clock.seconds(&floor)?;
        session.clock.take_seconds(stamp)?;
        Ok(())
    }
    fn into_output_after_native_ack_r1(self, ack: NativeCommittedR1) -> Result<Value> {
        // All plan/effect/content checks ran before commit admission. This
        // unforgeable value is constructed only after real COMMIT+terminal ACK.
        // A committed business refusal keeps its original error and facts.
        if let Some(formal) = ack.formal {
            tracing::debug!(report=%formal.reference.physical_id,asof=formal.asof_sequence,
                binding=formal.binding_sequence,digest=%formal.binding_digest,
                "new formal member committed; no Current authority");
        }
        self.into_output()
    }
}

impl NativeAsOfPermit {
    async fn append_complete_binding_r24(
        &mut self,
        session: &mut Session,
        ctx: &Context,
        plan: &mut OwnedOperationPlan,
    ) -> Result<()> {
        self.same_session(session, true)?;
        require(self.complete_ack_r24.is_none())?;
        let effect = self.actual_effect_r1.ok_or(Error::ClockRegressed)?;
        if !effect.needs_versioned_insert() {
            require(plan.complete_insert_ack_r24()?.is_none())?;
            return Ok(());
        }
        require(effect == plan.actual_effect_r1()?)?;
        let insert = plan
            .complete_insert_ack_r24()?
            .ok_or(Error::ClockRegressed)?;
        require(
            operation_effect_matches_r44(effect, &self.operation)
                && ctx.namespace() == self.namespace
                && ctx.actor() == self.actor
                && ctx.role() == self.role
                && self.role == Role::Evaluator
                && self.acknowledged_writes == plan.native_next()
                && insert.insert_index < self.acknowledged_writes
                && self.closed_prefix_digest.is_some(),
        )?;
        if let OwnedOperationPlan::Complete(complete) = plan {
            let frozen = complete.frozen_r24.as_ref().ok_or(Error::ClockRegressed)?;
            session.clock.require_event_binding(&frozen.clock)?;
        }
        let witness = self.witness.as_ref().ok_or(Error::ClockRegressed)?;
        let summary: StoredAsOfSummaryR24 =
            serde_json::from_slice(&serde_json::to_vec(&witness.summary).map_err(crate::internal)?)
                .map_err(crate::internal)?;
        require(
            check_native_asof_entity_r44(&witness.entity, &summary)?
                == plan.native_first_stop_digest_r44()?,
        )?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        let raw = load_sqlx_database(tx).await.map_err(crate::internal)?;
        same_database(&raw, plan.native_expected())?;
        let floor = plan.qualify_native_prefix_r24(&raw, &session.prepare_inputs)?;
        let (actual, row_digest) = decode_complete_native_raw_r36(&raw, &insert.reference)?;
        let (tip, previous) = native_audit_tip(&raw, &self.namespace)?;
        require(
            carrier_effect_r1(&actual.input.operation, &actual.payload)? == effect
                && tip == witness.seq
                && previous == witness.digest
                && row_digest == insert.insert_row_digest
                && actual.report_ref(ctx.actor()).map_err(r24_fault)? == insert.reference
                && insert.reference.namespace == self.namespace
                && insert.reference.owner == self.actor
                && insert.reference.ticket_id == summary.ticket_id
                && actual.input.ticket_digest == insert.reference.ticket_digest
                && actual.payload_digest == insert.reference.payload_digest
                && actual.input.occurred_at_unix_ms >= self.capture_millis
                && summary.audit_timestamp >= actual.input.occurred_at_unix_ms / 1000
                && summary.capture_millis == self.capture_millis,
        )?;
        let saved_ticket_id = evaluation_storage_id(TICKET_KIND, &insert.reference.ticket_id)?;
        let saved = actual
            .input
            .facts
            .iter()
            .find(|fact| {
                fact.namespace == self.namespace
                    && fact.kind == "artifact"
                    && fact.id == saved_ticket_id
            })
            .ok_or(Error::ClockRegressed)?;
        require(saved.raw_row_digest == summary.actual_ticket_row_digest)?;
        // Ref comes ONLY from this exact plan's newly INSERTed/ACKed member.
        // There is no input Ref parameter and no route for rebinding an old row.
        let entity = serde_json::to_string(&CompleteBindingR24 {
            schema_version: "rsia.native.complete_binding.v1".into(),
            summary,
            asof_sequence: witness.seq,
            asof_previous: witness.previous.clone(),
            asof_digest: witness.digest.clone(),
            asof_payload: witness.payload.clone(),
            report_ref: insert.reference.clone(),
            insert_index: insert.insert_index,
            insert_row_digest: insert.insert_row_digest,
        })
        .map_err(crate::internal)?;
        require(entity.len() <= 4 * 1024 * 1024)?;
        let stamp = session.clock.seconds(&floor)?;
        let timestamp = session.clock.take_seconds(stamp)?;
        let payload = serde_json::json!({"actor":ctx.actor(),"role":ctx.role(),
            "operation":COMPLETE_BINDING_OPERATION_R24,"entity":entity,"timestamp":timestamp})
        .to_string();
        let digest = hash(format!("{previous}\n{payload}").as_bytes());
        let seq:i64 = sqlx::query_scalar(
            "INSERT INTO audit(namespace,payload,previous_hash,digest) VALUES(?,?,?,?) RETURNING seq"
        ).bind(&self.namespace).bind(&payload).bind(&previous).bind(&digest)
         .fetch_one(&mut **tx).await.map_err(crate::internal)?;
        require(
            seq > raw
                .tables
                .get("audit")
                .ok_or(Error::ClockRegressed)?
                .iter()
                .map(|row| native_integer(row, "seq"))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .max()
                .unwrap_or(0),
        )?;
        plan.native_expected_mut()
            .tables
            .get_mut("audit")
            .ok_or(Error::ClockRegressed)?
            .push(OwnedRow {
                cells: BTreeMap::from([
                    ("seq".into(), OwnedCell::Integer(seq)),
                    ("namespace".into(), OwnedCell::Text(self.namespace.clone())),
                    ("payload".into(), OwnedCell::Text(payload.clone())),
                    ("previous_hash".into(), OwnedCell::Text(previous.clone())),
                    ("digest".into(), OwnedCell::Text(digest.clone())),
                ]),
            });
        let after = load_sqlx_database(tx).await.map_err(crate::internal)?;
        same_database(&after, plan.native_expected())?;
        // This is the FIRST proof-complete check: no pending exception at all.
        collect_raw(&after, &session.prepare_inputs, SchemaRead::Stable)?;
        self.complete_ack_r24 = Some(CompleteBindingAckR24 {
            reference: insert.reference,
            asof_sequence: witness.seq,
            sequence: seq,
            previous,
            payload,
            digest,
        });
        Ok(())
    }
}

impl E05CompletePlan {
    async fn audit_pending_r24(&mut self, session: &mut Session, ctx: &Context) -> Result<()> {
        let Some((operation, entity)) = &self.prefix.audit else {
            return Ok(());
        };
        self.admit_all_pending_r24(session).await?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        let previous = self
            .prefix
            .expected
            .tables
            .get("audit")
            .ok_or(Error::ClockRegressed)?
            .iter()
            .filter(|row| native_text(row, "namespace").ok().as_deref() == Some(ctx.namespace()))
            .max_by_key(|row| native_integer(row, "seq").unwrap_or(i64::MIN))
            .map(|row| native_text(row, "digest"))
            .transpose()?
            .unwrap_or_default();
        let raw = load_sqlx_database(tx).await.map_err(crate::internal)?;
        let floor = self.qualify_pending_raw_r24(&raw, &session.prepare_inputs)?;
        let stamp = session.clock.seconds(&floor)?;
        let at = session.clock.take_seconds(stamp)?;
        let payload=serde_json::json!({"actor":ctx.actor(),"role":ctx.role(),"operation":operation,"entity":entity,"timestamp":at}).to_string();
        let digest = hash(format!("{previous}\n{payload}").as_bytes());
        let seq:i64=sqlx::query_scalar("INSERT INTO audit(namespace,payload,previous_hash,digest) VALUES(?,?,?,?) RETURNING seq")
            .bind(ctx.namespace()).bind(&payload).bind(&previous).bind(&digest)
            .fetch_one(&mut **tx).await.map_err(crate::internal)?;
        require(
            seq > 0
                && self
                    .prefix
                    .expected
                    .tables
                    .get("audit")
                    .ok_or(Error::ClockRegressed)?
                    .iter()
                    .all(|row| native_integer(row, "seq").is_ok_and(|old| old < seq)),
        )?;
        self.prefix
            .expected
            .tables
            .get_mut("audit")
            .ok_or(Error::ClockRegressed)?
            .push(OwnedRow {
                cells: BTreeMap::from([
                    ("seq".into(), OwnedCell::Integer(seq)),
                    ("namespace".into(), OwnedCell::Text(ctx.namespace().into())),
                    ("payload".into(), OwnedCell::Text(payload)),
                    ("previous_hash".into(), OwnedCell::Text(previous)),
                    ("digest".into(), OwnedCell::Text(digest)),
                ]),
            });
        Ok(())
    }
}

impl NativeAsOfPermit {
    async fn capture(
        session: &mut Session,
        ctx: &Context,
        operation: NativeAsOfOperation,
        intent: &NativeIntent,
        gate: Arc<AtomicU8>,
        facts: Arc<AtomicU8>,
    ) -> Result<(RawDatabase, crate::write_clock::QualifiedEventTime, Self)> {
        session.clock.check()?;
        require(gate.load(Ordering::Acquire) == ACTIVE && facts.load(Ordering::Acquire) == 0)?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        require(
            !session.write_started
                && tx.open
                && SqliteTransactionManager::get_transaction_depth(&**tx) == 1
                && tx.is_in_transaction(),
        )?;
        let connection = Arc::clone(&tx.connection.owner);
        let database = load_sqlx_database(tx)
            .await
            .map_err(|_| session.clock.fail())?;
        require(tx.is_in_transaction())?;
        native_formal_consumers_r49::reject_known_complete_attack_r49(&database, ctx, intent)?;
        let floor = collect_raw(&database, &session.prepare_inputs, SchemaRead::Stable)
            .map_err(|_| session.clock.fail())?;
        let stamp = session.clock.event(&floor)?;
        let (event, clock) = session.clock.take_bound_event(stamp)?;
        let capture_digest = native_database_digest(&database, true)?;
        let permit = Self {
            connection,
            clock,
            gate,
            facts,
            namespace: ctx.namespace().into(),
            actor: ctx.actor().into(),
            role: ctx.role(),
            operation,
            actual_effect_r1: None,
            capture_seconds: event.seconds(),
            capture_millis: event.millis(),
            capture_digest: capture_digest.clone(),
            prepared_digest: None,
            prefix_digest: capture_digest,
            acknowledged_writes: 0,
            closed_prefix_digest: None,
            closed_non_audit_digest: None,
            closed_audit_rows: None,
            witness: None,
            complete_ack_r24: None,
        };
        Ok((database, event, permit))
    }

    fn same_session(&self, session: &Session, in_transaction: bool) -> Result<()> {
        session.clock.require_event_binding(&self.clock)?;
        let tx = session.tx.as_ref().ok_or(Error::ClockRegressed)?;
        require(
            Arc::ptr_eq(&self.connection, &tx.connection.owner)
                && tx.open == in_transaction
                && tx.is_in_transaction() == in_transaction
                && SqliteTransactionManager::get_transaction_depth(&**tx)
                    == usize::from(in_transaction),
        )
    }

    async fn bind_prepared(
        &mut self,
        session: &mut Session,
        plan: &OwnedOperationPlan,
    ) -> Result<()> {
        self.same_session(session, true)?;
        require(
            self.prepared_digest.is_none()
                && self.actual_effect_r1.is_none()
                && plan.native_next() == 0,
        )?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        let raw = load_sqlx_database(tx).await.map_err(crate::internal)?;
        same_database(&raw, plan.native_expected())?;
        let floor = plan.qualify_native_prefix_r24(&raw, &session.prepare_inputs)?;
        let stamp = session.clock.seconds(&floor)?;
        session.clock.take_seconds(stamp)?;
        self.prefix_digest = native_database_digest(&raw, true)?;
        let effect = plan.bound_effect_r1(&self.capture_digest, &self.prefix_digest)?;
        require(
            !effect.needs_versioned_insert()
                || operation_effect_matches_r44(effect, &self.operation),
        )?;
        self.actual_effect_r1 = Some(effect);
        self.prepared_digest = Some(self.prefix_digest.clone());
        Ok(())
    }

    fn before_write(
        &self,
        session: &Session,
        plan: &OwnedOperationPlan,
        step: usize,
    ) -> Result<()> {
        self.same_session(session, true)?;
        require(
            self.closed_prefix_digest.is_none()
                && self.prepared_digest.is_some()
                && step == self.acknowledged_writes
                && plan.native_next() == step
                && native_database_digest(plan.native_expected(), true)? == self.prefix_digest,
        )
    }

    // Called only immediately after IssueWrite::execute returned its actual SQL
    // rows_affected==1 ACK and the matching plan acknowledged the same step.
    fn acknowledge(
        &mut self,
        session: &Session,
        plan: &OwnedOperationPlan,
        step: usize,
    ) -> Result<()> {
        self.same_session(session, true)?;
        let next = step.checked_add(1).ok_or(Error::ClockRegressed)?;
        require(
            step == self.acknowledged_writes
                && plan.native_next() == next
                && self.closed_prefix_digest.is_none(),
        )?;
        self.prefix_digest = native_database_digest(plan.native_expected(), true)?;
        self.acknowledged_writes = next;
        Ok(())
    }

    fn close_prefix(&mut self, session: &Session, plan: &OwnedOperationPlan) -> Result<()> {
        self.same_session(session, true)?;
        require(
            plan.native_next() == plan.writes().len()
                && plan.native_next() == self.acknowledged_writes
                && native_database_digest(plan.native_expected(), true)? == self.prefix_digest
                && self.closed_prefix_digest.is_none(),
        )?;
        self.closed_prefix_digest = Some(self.prefix_digest.clone());
        self.closed_non_audit_digest = Some(native_database_digest(plan.native_expected(), false)?);
        self.closed_audit_rows = Some(native_audit_rows(plan.native_expected())?);
        Ok(())
    }

    async fn append_audit(
        &mut self,
        session: &mut Session,
        ctx: &Context,
        plan: &mut OwnedOperationPlan,
    ) -> Result<()> {
        self.same_session(session, true)?;
        require(
            ctx.namespace() == self.namespace
                && ctx.actor() == self.actor
                && ctx.role() == self.role
                && self.witness.is_none(),
        )?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        let raw = load_sqlx_database(tx).await.map_err(crate::internal)?;
        same_database(&raw, plan.native_expected())?;
        let floor = plan.qualify_native_prefix_r24(&raw, &session.prepare_inputs)?;
        require(
            self.closed_non_audit_digest.as_ref() == Some(&native_database_digest(&raw, false)?),
        )?;
        let old = self
            .closed_audit_rows
            .as_ref()
            .ok_or(Error::ClockRegressed)?;
        let now = native_audit_rows(&raw)?;
        require(old.iter().all(|(seq, digest)| now.get(seq) == Some(digest)))?;
        let additions: Vec<i64> = now
            .keys()
            .filter(|seq| !old.contains_key(*seq))
            .copied()
            .collect();
        match plan.native_legacy_audit() {
            Some((operation, entity)) => {
                require(additions.len() == 1)?;
                let row = raw
                    .tables
                    .get("audit")
                    .ok_or(Error::ClockRegressed)?
                    .iter()
                    .find(|row| {
                        native_integer(row, "seq").is_ok_and(|seq| Some(&seq) == additions.first())
                    })
                    .ok_or(Error::ClockRegressed)?;
                let payload = parse_owned_json(&native_text(row, "payload")?)?;
                require(
                    native_text(row, "namespace")? == self.namespace
                        && string(&payload, "actor")? == self.actor
                        && decode::<Role>(&payload["role"])? == self.role
                        && string(&payload, "operation")? == operation
                        && string(&payload, "entity")? == entity,
                )?;
            }
            None => require(additions.is_empty())?,
        }
        let after_legacy_digest = native_database_digest(&raw, true)?;
        if after_legacy_digest == self.capture_digest {
            // Exact idempotent replay performs no additional write.
            return Ok(());
        }
        let ticket = plan.native_ticket();
        let (snapshot, _) = snapshot(&raw)?;
        let actual = evaluation_ticket(&snapshot, &self.namespace, &ticket.id)?;
        require(typed_fingerprint(&actual)? == typed_fingerprint(ticket)?)?;
        let physical = evaluation_storage_id(TICKET_KIND, &actual.id)?;
        let ticket_rows: Vec<_> = raw
            .tables
            .get("objects")
            .ok_or(Error::ClockRegressed)?
            .iter()
            .filter(|row| {
                native_text(row, "namespace").ok().as_ref() == Some(&self.namespace)
                    && native_text(row, "kind").ok().as_deref() == Some("artifact")
                    && native_text(row, "id").ok().as_ref() == Some(&physical)
            })
            .collect();
        require(ticket_rows.len() == 1)?;
        let (previous_sequence, previous) = native_audit_tip(&raw, &self.namespace)?;
        let stamp = session.clock.seconds(&floor)?;
        let timestamp = session.clock.take_seconds(stamp)?;
        let operation = self.operation.audit_operation();
        require(reserved_native_audit_operation(operation))?;
        let summary = NativeAsOfSummary {
            schema_version: "rsia.native_asof.audit_summary.v1",
            namespace: self.namespace.clone(),
            actor: self.actor.clone(),
            role: ctx.role(),
            operation,
            ticket_id: actual.id,
            actual_ticket_row_digest: hash(&row_bytes(ticket_rows[0])),
            capture_seconds: self.capture_seconds,
            capture_millis: self.capture_millis,
            capture_database_digest: self.capture_digest.clone(),
            prepared_prefix_digest: self.prepared_digest.clone().ok_or(Error::ClockRegressed)?,
            closed_ack_prefix_digest: self
                .closed_prefix_digest
                .clone()
                .ok_or(Error::ClockRegressed)?,
            after_legacy_audit_prefix_digest: after_legacy_digest,
            acknowledged_writes: self.acknowledged_writes,
            previous_audit_sequence: previous_sequence,
            previous_audit_hash: previous.clone(),
            audit_timestamp: timestamp,
        };
        let first_stop = plan.native_first_stop_digest_r44()?;
        if let Some(expected) = first_stop.as_ref() {
            r44_stop_require(
                matches!(&self.operation, NativeAsOfOperation::Grade)
                    && self.role == Role::Evaluator,
            )?;
            let body = parse_owned_json(&native_text(ticket_rows[0], "body")?)?;
            let actual_source = first_stop_from_body_r44(&body, &self.namespace, &physical)?
                .ok_or(Error::ClockRegressed)?;
            r44_stop_require(
                typed_fingerprint(&actual_source)? == *expected
                    && actual_source.first_ticket_digest == typed_fingerprint(ticket)?,
            )?;
        }
        let entity = native_asof_entity_r44(&summary, first_stop.as_deref())?;
        let payload = serde_json::json!({
            "actor":ctx.actor(),"role":ctx.role(),"operation":operation,
            "entity":entity,"timestamp":timestamp
        })
        .to_string();
        let digest = hash(format!("{previous}\n{payload}").as_bytes());
        let seq: i64 = sqlx::query_scalar(
            "INSERT INTO audit(namespace,payload,previous_hash,digest) VALUES(?,?,?,?) RETURNING seq",
        )
        .bind(&self.namespace).bind(&payload).bind(&previous).bind(&digest)
        .fetch_one(&mut **tx).await.map_err(crate::internal)?;
        require(seq > now.keys().last().copied().unwrap_or(0))?;
        let acknowledged = OwnedRow {
            cells: BTreeMap::from([
                ("seq".into(), OwnedCell::Integer(seq)),
                ("namespace".into(), OwnedCell::Text(self.namespace.clone())),
                ("payload".into(), OwnedCell::Text(payload.clone())),
                ("previous_hash".into(), OwnedCell::Text(previous.clone())),
                ("digest".into(), OwnedCell::Text(digest.clone())),
            ]),
        };
        plan.native_expected_mut()
            .tables
            .get_mut("audit")
            .ok_or(Error::ClockRegressed)?
            .push(acknowledged);
        let after = load_sqlx_database(tx).await.map_err(crate::internal)?;
        same_database(&after, plan.native_expected())?;
        // Full chain, exact seq/prev/digest/body and all ordinary owned guards
        // are checked again after the actual INSERT ACK, before any COMMIT.
        plan.qualify_native_prefix_r24(&after, &session.prepare_inputs)?;
        self.witness = Some(NativeAsOfAuditAck {
            summary,
            entity,
            seq,
            previous,
            digest,
            payload,
        });
        Ok(())
    }

    fn prepare_commit_r1(
        &self,
        session: &Session,
        plan: &OwnedOperationPlan,
    ) -> Result<NativeReadyToCommitR1> {
        self.same_session(session, true)?;
        let effect = self.actual_effect_r1.ok_or(Error::ClockRegressed)?;
        let prepared = self
            .prepared_digest
            .as_deref()
            .ok_or(Error::ClockRegressed)?;
        require(
            effect == plan.bound_effect_r1(&self.capture_digest, prepared)?
                && self.acknowledged_writes == plan.native_next()
                && plan.native_next() == plan.writes().len()
                && self.closed_prefix_digest.is_some(),
        )?;
        if effect == NativePlanEffectR1::NoNewFormal {
            require(
                plan.writes().is_empty()
                    && self.acknowledged_writes == 0
                    && plan.native_legacy_audit().is_none()
                    && prepared == self.capture_digest
                    && self.closed_prefix_digest.as_deref() == Some(self.capture_digest.as_str())
                    && native_database_digest(plan.native_expected(), true)? == self.capture_digest
                    && self.witness.is_none()
                    && self.complete_ack_r24.is_none(),
            )?;
            return Ok(NativeReadyToCommitR1 { binding: None });
        }
        let asof = self.witness.as_ref().ok_or(Error::ClockRegressed)?;
        let first_stop = plan.native_first_stop_digest_r44()?;
        require(
            asof.seq > asof.summary.previous_audit_sequence
                && asof.previous == asof.summary.previous_audit_hash
                && asof.entity == native_asof_entity_r44(&asof.summary, first_stop.as_deref())?
                && asof.digest == hash(format!("{}\n{}", asof.previous, asof.payload).as_bytes())
                && asof.summary.operation == self.operation.audit_operation()
                && asof.summary.namespace == self.namespace
                && asof.summary.actor == self.actor
                && asof.summary.role == self.role
                && asof.summary.ticket_id == plan.native_ticket().id
                && asof.summary.capture_database_digest == self.capture_digest
                && asof.summary.prepared_prefix_digest == prepared
                && Some(&asof.summary.closed_ack_prefix_digest)
                    == self.closed_prefix_digest.as_ref()
                && asof.summary.acknowledged_writes == self.acknowledged_writes,
        )?;
        if !effect.needs_versioned_insert() {
            require(self.complete_ack_r24.is_none() && plan.complete_insert_ack_r24()?.is_none())?;
            return Ok(NativeReadyToCommitR1 { binding: None });
        }
        require(operation_effect_matches_r44(effect, &self.operation))?;
        let OwnedOperationPlan::Complete(complete) = plan else {
            return Err(Error::ClockRegressed);
        };
        let frozen = complete.frozen_r24.as_ref().ok_or(Error::ClockRegressed)?;
        session.clock.require_event_binding(&frozen.clock)?;
        require(
            complete.materialized
                && complete.work_r1.effect() == effect
                && carrier_effect_r1(&frozen.carrier.input.operation, &frozen.carrier.payload)?
                    == effect,
        )?;
        let insert = complete.insert_ack_r24()?;
        let binding = self
            .complete_ack_r24
            .as_ref()
            .ok_or(Error::ClockRegressed)?;
        require(
            binding.reference == insert.reference
                && binding.asof_sequence == asof.seq
                && binding.sequence > asof.seq
                && binding.previous == asof.digest
                && binding.digest
                    == hash(format!("{}\n{}", binding.previous, binding.payload).as_bytes()),
        )?;
        let payload: AuditPayloadR24 =
            serde_json::from_str(&binding.payload).map_err(crate::internal)?;
        require(
            payload.actor == self.actor
                && payload.role == self.role
                && payload.operation == COMPLETE_BINDING_OPERATION_R24,
        )?;
        let saved: CompleteBindingR24 =
            serde_json::from_str(&payload.entity).map_err(crate::internal)?;
        require(
            saved.schema_version == "rsia.native.complete_binding.v1"
                && saved.report_ref == binding.reference
                && saved.asof_sequence == asof.seq
                && saved.asof_digest == asof.digest
                && check_native_asof_entity_r44(&asof.entity, &saved.summary)? == first_stop,
        )?;
        digest(&binding.digest)?;
        Ok(NativeReadyToCommitR1 {
            binding: Some(ReadyFormalBindingR1 {
                reference: insert.reference,
                asof_sequence: asof.seq,
                binding_sequence: binding.sequence,
                binding_digest: binding.digest.clone(),
            }),
        })
    }

    fn finish(self, ready: NativeReadyToCommitR1) -> Result<NativeCommittedR1> {
        // There is NO plan classification, JSON decode, fingerprint, reference,
        // source lookup, clock stamp or report creation after commit admission.
        // run() already checked native transaction depth=0 and ping() before
        // setting TERMINAL_ACK. Only its real admission/COMMIT/terminal ACKs
        // can promote the private prechecked binding.
        if self.gate.load(Ordering::Acquire) != COMMIT_ADMITTED
            || self.facts.load(Ordering::Acquire) & (COMMIT_ACK | TERMINAL_ACK)
                != (COMMIT_ACK | TERMINAL_ACK)
        {
            return Err(Error::Internal);
        }
        Ok(NativeCommittedR1 {
            formal: ready.binding.map(|binding| CommittedCompleteR24 {
                reference: binding.reference,
                asof_sequence: binding.asof_sequence,
                binding_sequence: binding.binding_sequence,
                binding_digest: binding.binding_digest,
            }),
        })
    }
}

enum NativeIntent {
    Issue(Value),
    Execution(Value),
    Grade(Value),
    Complete(Value),
    Reconcile(Value),
    Invalidate(Value),
    ConsumerR49(Box<native_formal_consumers_r49::ConsumerIntentR49>),
}

/// No Clone, Serde, Deref, raw SQL, callback or Session escape is exposed.
#[must_use = "finish the operation or request cancellation; Drop is not a native ACK"]
#[derive(Debug)]
pub struct E05IssueOperation {
    completion: Option<oneshot::Receiver<E05IssueOutcome>>,
    gate: Arc<AtomicU8>,
    facts: Arc<AtomicU8>,
}
impl E05IssueOperation {
    /// True means cancellation owns the pre-commit gate. False means the worker
    /// already won the irreversible commit-admission CAS; finish must drain it.
    pub fn cancel(&self) -> bool {
        request_cancellation(&self.gate)
    }
    pub async fn finish(self) -> Result<Value> {
        self.finish_detailed()
            .await
            .map_err(SessionCommitFailure::into_error)
    }
    pub async fn finish_detailed(self) -> std::result::Result<Value, SessionCommitFailure> {
        self.finish_outcome().await.result
    }
    pub async fn finish_outcome(mut self) -> E05IssueOutcome {
        let Some(completion) = self.completion.take() else {
            return lost_completion(
                self.facts.load(Ordering::Acquire),
                self.gate.load(Ordering::Acquire),
            );
        };
        completion.await.unwrap_or_else(|_| {
            lost_completion(
                self.facts.load(Ordering::Acquire),
                self.gate.load(Ordering::Acquire),
            )
        })
    }
}
impl Drop for E05IssueOperation {
    fn drop(&mut self) {
        let _ = self.cancel();
    }
}

#[derive(Debug)]
pub struct E05NativeFacts {
    pub commit_admitted: bool,
    pub commit_attempted: bool,
    /// SQLx's public manager gives no separate send ACK on error.
    /// None means attempted/may-have-been-sent, not a claim of actual sending.
    pub commit_command_sent: Option<bool>,
    pub commit_confirmed: bool,
    pub rollback_command_acknowledged: bool,
    /// Only an open transaction with no attempted COMMIT can receive this fact.
    pub rollback_confirmed: bool,
    pub native_close_confirmed: bool,
    pub terminal_unconfirmed: bool,
}
#[derive(Debug)]
pub struct E05IssueOutcome {
    pub result: std::result::Result<Value, SessionCommitFailure>,
    pub native: E05NativeFacts,
}
fn native_facts(facts: u8, gate: u8) -> E05NativeFacts {
    let attempted = facts & COMMIT_ATTEMPTED != 0;
    let confirmed = facts & COMMIT_ACK != 0;
    E05NativeFacts {
        commit_admitted: gate == COMMIT_ADMITTED,
        commit_attempted: attempted,
        commit_command_sent: if confirmed {
            Some(true)
        } else if attempted {
            None
        } else {
            Some(false)
        },
        commit_confirmed: confirmed,
        rollback_command_acknowledged: facts & ROLLBACK_COMMAND_ACK != 0,
        rollback_confirmed: facts & ROLLBACK_CONFIRMED != 0 && !attempted,
        native_close_confirmed: facts & NATIVE_CLOSE_ACK != 0,
        terminal_unconfirmed: facts & TERMINAL_ACK == 0,
    }
}
fn channel_failure(facts: u8) -> SessionCommitFailure {
    SessionCommitFailure {
        error: Error::Internal,
        rollback_error: None,
        commit_outcome_unknown: facts & COMMIT_ATTEMPTED != 0 && facts & COMMIT_ACK == 0,
        commit_confirmed: facts & COMMIT_ACK != 0,
    }
}
fn lost_completion(facts: u8, gate: u8) -> E05IssueOutcome {
    E05IssueOutcome {
        result: Err(channel_failure(facts)),
        native: native_facts(facts, gate),
    }
}

struct NativeIssueOwner {
    session: Option<Session>,
    gate: Arc<AtomicU8>,
    facts: Arc<AtomicU8>,
    terminal_confirmed: bool,
}
impl Drop for NativeIssueOwner {
    fn drop(&mut self) {
        if !self.terminal_confirmed
            && let Some(session) = self.session.take()
        {
            // All transaction/clock/activity/runtime resources remain retained,
            // not just an OS handle whose writer barrier could have disappeared.
            std::mem::forget(session);
        }
    }
}
impl NativeIssueOwner {
    fn failure(&mut self, error: Error) -> SessionCommitFailure {
        let facts = self.facts.load(Ordering::Acquire);
        let mut failure = match self.session.as_mut() {
            Some(session) => session.clock.failure(error),
            None => SessionCommitFailure {
                error,
                rollback_error: None,
                commit_outcome_unknown: false,
                commit_confirmed: false,
            },
        };
        failure.commit_outcome_unknown = facts & COMMIT_ATTEMPTED != 0 && facts & COMMIT_ACK == 0;
        failure.commit_confirmed = facts & COMMIT_ACK != 0;
        failure
    }

    async fn drain_failure(&mut self) {
        // A committed plan may intentionally return its original business error.
        // Do not manufacture a rollback-command ACK after its real terminal ACK.
        if self.terminal_confirmed {
            return;
        }
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(tx) = session.tx.as_mut() else {
            // A legacy helper may already have consumed its transaction on
            // refusal. Its Drop is not this owner's ACK: require native close.
            if session._runtime.pool.close().await.is_ok() {
                self.facts
                    .fetch_or(NATIVE_CLOSE_ACK | TERMINAL_ACK, Ordering::AcqRel);
                self.terminal_confirmed = true;
            }
            return;
        };
        let pool = NativePool {
            owner: Arc::clone(&tx.connection.owner),
        };
        let depth_before = SqliteTransactionManager::get_transaction_depth(&**tx);
        match SqliteTransactionManager::rollback(&mut **tx).await {
            Ok(()) if SqliteTransactionManager::get_transaction_depth(&**tx) == 0 => {
                match tx.ping().await {
                    Ok(()) => {
                        tx.open = false;
                        let mut acknowledged = ROLLBACK_COMMAND_ACK | TERMINAL_ACK;
                        if depth_before == 1
                            && self.facts.load(Ordering::Acquire) & COMMIT_ATTEMPTED == 0
                        {
                            acknowledged |= ROLLBACK_CONFIRMED;
                        }
                        self.facts.fetch_or(acknowledged, Ordering::AcqRel);
                        self.terminal_confirmed = true;
                        return;
                    }
                    Err(error) => session.clock.record_rollback_error(error),
                }
            }
            Ok(()) => {}
            Err(error) => session.clock.record_rollback_error(error),
        }
        drop(session.tx.take());
        match pool.close().await {
            Ok(()) => {
                self.facts
                    .fetch_or(NATIVE_CLOSE_ACK | TERMINAL_ACK, Ordering::AcqRel);
                self.terminal_confirmed = true;
            }
            Err(error) => session.clock.record_rollback_error(error),
        }
    }

    async fn run(&mut self, ctx: Context, intent: NativeIntent) -> Result<Value> {
        check_cancel(&self.gate)?;
        let intent = match intent {
            NativeIntent::ConsumerR49(intent) => return self.run_consumer_r49(ctx, *intent).await,
            other => other,
        };
        let session = self.session.as_mut().ok_or(Error::Internal)?;
        let operation = NativeAsOfOperation::for_intent(&intent)?;
        let (database, event, mut asof) = NativeAsOfPermit::capture(
            session,
            &ctx,
            operation,
            &intent,
            Arc::clone(&self.gate),
            Arc::clone(&self.facts),
        )
        .await?;
        let at = event.seconds();
        let mut plan = match intent {
            NativeIntent::ConsumerR49(_) => return Err(Error::Forbidden),
            NativeIntent::Issue(value) => {
                ctx.require(&[Role::Evaluator])?;
                OwnedOperationPlan::Issue(Box::new(E05IssuePlan::derive(
                    &database,
                    &ctx,
                    IssueIntent::read(value)?,
                    &event,
                )?))
            }
            NativeIntent::Execution(value) => {
                ctx.require(&[Role::Worker, Role::Host])?;
                OwnedOperationPlan::Append(Box::new(E05AppendPlan::execution(
                    &database,
                    &ctx,
                    ExecutionIntent::read(value)?,
                    at,
                )?))
            }
            NativeIntent::Grade(value) => {
                ctx.require(&[Role::Evaluator])?;
                let mut draft =
                    E05GradeDraft::derive(&database, &ctx, GradeIntent::read(value)?, at)?;
                draft
                    .prepare_registered_grade(session, &ctx, &asof, Arc::clone(&self.gate))
                    .await?;
                draft.prepare_budget(session, &ctx, &self.gate).await?;
                check_cancel(&self.gate)?;
                let database =
                    load_sqlx_database(&mut **session.tx.as_mut().ok_or(Error::ClockRegressed)?)
                        .await?;
                collect_raw(&database, &session.prepare_inputs, SchemaRead::Stable)
                    .map_err(|_| session.clock.fail())?;
                OwnedOperationPlan::Append(Box::new(E05AppendPlan::grade(&database, &ctx, draft)?))
            }
            NativeIntent::Complete(value) => {
                ctx.require(&[Role::Evaluator])?;
                E05CompleteDraft::derive(
                    &database,
                    &ctx,
                    TicketIntent::read(value, COMPLETE_INTENT_SCHEMA)?,
                    at,
                )?
                .prepare(session, &ctx, &self.gate)
                .await?
            }
            NativeIntent::Reconcile(value) => {
                ctx.require(&[Role::Evaluator])?;
                prepare_reconcile(
                    &database,
                    session,
                    &ctx,
                    TicketIntent::read(value, RECONCILE_INTENT_SCHEMA)?,
                    &self.gate,
                    at,
                )
                .await?
            }
            NativeIntent::Invalidate(value) => {
                ctx.require(&[Role::Evaluator])?;
                prepare_invalidate(
                    &database,
                    session,
                    &ctx,
                    InvalidateIntent::read(value)?,
                    &self.gate,
                    at,
                )
                .await?
            }
        };
        plan = match plan {
            OwnedOperationPlan::Append(prefix)
                if prefix.before_ticket.lifecycle != MirrorTicketLifecycle::EarlyStopped
                    && prefix.after_ticket.lifecycle == MirrorTicketLifecycle::EarlyStopped
                    && prefix.first_stop_r44.is_some() =>
            {
                OwnedOperationPlan::Complete(Box::new(E05CompletePlan::from_native_early_r44(
                    *prefix,
                )?))
            }
            other => other,
        };
        asof.bind_prepared(session, &plan).await?;
        let mut step = 0;
        loop {
            check_cancel(&self.gate)?;
            plan.prepare_next(session, &ctx, &asof).await?;
            if step == plan.writes().len() {
                break;
            }
            check_cancel(&self.gate)?;
            plan.admit(session).await?;
            check_cancel(&self.gate)?;
            asof.before_write(session, &plan, step)?;
            plan.writes()[step].execute(session).await?;
            plan.acknowledge(step)?;
            asof.acknowledge(session, &plan, step)?;
            session.write_started = true;
            step = step.checked_add(1).ok_or(Error::ClockRegressed)?;
        }
        plan.admit_stable(session).await?;
        asof.close_prefix(session, &plan)?;
        check_cancel(&self.gate)?;
        plan.audit(session, &ctx).await?;
        asof.append_audit(session, &ctx, &mut plan).await?;
        asof.append_complete_binding_r24(session, &ctx, &mut plan)
            .await?;
        plan.admit_final_stable_r24(session).await?;
        let ready_r1 = asof.prepare_commit_r1(session, &plan)?;
        frozen_complete_kernel_r29::verify_before_commit(&asof, session, &ctx, &plan, &ready_r1)
            .await?;
        if let OwnedOperationPlan::Complete(complete) = &plan
            && complete.work_r1.effect() == NativePlanEffectR1::NewComplete
        {
            let reference = &complete
                .frozen_r24
                .as_ref()
                .ok_or(Error::ClockRegressed)?
                .reference;
            native_formal_consumers_r49::issue_inside_complete_owner_r49(
                session,
                &ctx,
                reference,
                Arc::clone(&self.gate),
            )
            .await?;
        }
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        require(SqliteTransactionManager::get_transaction_depth(&**tx) == 1)?;
        // This single CAS is the cancellation/irreversible-admission boundary.
        // A cancellation that wins here prevents calling COMMIT at all.
        admit_commit(&self.gate)?;
        self.facts.fetch_or(COMMIT_ATTEMPTED, Ordering::AcqRel);
        SqliteTransactionManager::commit(&mut **tx)
            .await
            .map_err(crate::internal)?;
        self.facts.fetch_or(COMMIT_ACK, Ordering::AcqRel);
        tx.open = false;
        require(SqliteTransactionManager::get_transaction_depth(&**tx) == 0)?;
        tx.ping().await.map_err(crate::internal)?;
        self.terminal_confirmed = true;
        self.facts.fetch_or(TERMINAL_ACK, Ordering::AcqRel);
        let committed_r1 = asof.finish(ready_r1)?;
        plan.into_output_after_native_ack_r1(committed_r1)
    }
}
fn request_cancellation(gate: &AtomicU8) -> bool {
    match gate.compare_exchange(ACTIVE, CANCELLED, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) | Err(CANCELLED) => true,
        Err(_) => false,
    }
}
fn admit_commit(gate: &AtomicU8) -> Result<()> {
    gate.compare_exchange(ACTIVE, COMMIT_ADMITTED, Ordering::AcqRel, Ordering::Acquire)
        .map(|_| ())
        .map_err(|_| Error::Cancelled)
}
fn check_cancel(gate: &AtomicU8) -> Result<()> {
    if gate.load(Ordering::Acquire) == CANCELLED {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

pub(crate) fn begin(session: Session, ctx: Context, intent: Value) -> Result<E05IssueOperation> {
    Ok(begin_native(
        session,
        ctx,
        NativeIntent::Issue(intent),
        Arc::new(AtomicU8::new(ACTIVE)),
    ))
}
pub(crate) fn begin_execution(
    session: Session,
    ctx: Context,
    intent: Value,
) -> Result<E05IssueOperation> {
    Ok(begin_native(
        session,
        ctx,
        NativeIntent::Execution(intent),
        Arc::new(AtomicU8::new(ACTIVE)),
    ))
}
pub(crate) fn begin_grade(
    session: Session,
    ctx: Context,
    intent: Value,
) -> Result<E05IssueOperation> {
    Ok(begin_native(
        session,
        ctx,
        NativeIntent::Grade(intent),
        Arc::new(AtomicU8::new(ACTIVE)),
    ))
}
pub(crate) fn begin_complete(
    session: Session,
    ctx: Context,
    intent: Value,
) -> Result<E05IssueOperation> {
    Ok(begin_native(
        session,
        ctx,
        NativeIntent::Complete(intent),
        Arc::new(AtomicU8::new(ACTIVE)),
    ))
}
pub(crate) fn begin_reconcile(
    session: Session,
    ctx: Context,
    intent: Value,
) -> Result<E05IssueOperation> {
    Ok(begin_native(
        session,
        ctx,
        NativeIntent::Reconcile(intent),
        Arc::new(AtomicU8::new(ACTIVE)),
    ))
}
pub(crate) fn begin_invalidate(
    session: Session,
    ctx: Context,
    intent: Value,
) -> Result<E05IssueOperation> {
    Ok(begin_native(
        session,
        ctx,
        NativeIntent::Invalidate(intent),
        Arc::new(AtomicU8::new(ACTIVE)),
    ))
}
#[cfg(test)]
fn begin_with_cancellation(
    session: Session,
    ctx: Context,
    intent: Value,
    gate: Arc<AtomicU8>,
) -> E05IssueOperation {
    begin_native(session, ctx, NativeIntent::Issue(intent), gate)
}
fn begin_native(
    session: Session,
    ctx: Context,
    intent: NativeIntent,
    gate: Arc<AtomicU8>,
) -> E05IssueOperation {
    let facts = Arc::new(AtomicU8::new(0));
    let (sent, received) = oneshot::channel();
    let handoff = Arc::new(Mutex::new(Some((
        NativeIssueOwner {
            session: Some(session),
            gate: Arc::clone(&gate),
            facts: Arc::clone(&facts),
            terminal_confirmed: false,
        },
        sent,
    ))));
    let thread_handoff = Arc::clone(&handoff);
    let started = std::thread::Builder::new().name("rsia-e05-native-writer".into()).spawn(move || {
        let Some((mut owner, sent)) = thread_handoff.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() else { return };
        let result = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => {
                let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.block_on(owner.run(ctx, intent))));
                match ran {
                    Ok(Ok(value)) => Ok(value),
                    Ok(Err(error)) => {
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.block_on(owner.drain_failure())));
                        Err(owner.failure(error))
                    }
                    Err(_) => {
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.block_on(owner.drain_failure())));
                        Err(owner.failure(Error::Internal))
                    }
                }
            }
            Err(error) => {
                tracing::error!(error=%error, "E05 native runtime unavailable; retaining full operation");
                Err(owner.failure(Error::Internal))
            }
        };
        let facts = owner.facts.load(Ordering::Acquire);
        let gate = owner.gate.load(Ordering::Acquire);
        if sent.send(E05IssueOutcome { result, native: native_facts(facts, gate) }).is_err() {
            tracing::warn!(commit_admitted=gate == COMMIT_ADMITTED,
                commit_attempted=facts & COMMIT_ATTEMPTED != 0,
                commit_confirmed=facts & COMMIT_ACK != 0,
                rollback_command_acknowledged=facts & ROLLBACK_COMMAND_ACK != 0,
                rollback_confirmed=facts & ROLLBACK_CONFIRMED != 0,
                native_close_confirmed=facts & NATIVE_CLOSE_ACK != 0,
                terminal_unconfirmed=facts & TERMINAL_ACK == 0,
                "E05 caller disappeared; native facts require reconciliation");
        }
    });
    if let Err(error) = started
        && let Some((mut owner, sent)) = handoff
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    {
        let failure = owner.failure(Error::Internal);
        let _ = sent.send(E05IssueOutcome {
            result: Err(failure),
            native: native_facts(
                owner.facts.load(Ordering::Acquire),
                owner.gate.load(Ordering::Acquire),
            ),
        });
        tracing::error!(error=%error, "E05 native owner unavailable; retaining full operation");
    }
    E05IssueOperation {
        completion: Some(received),
        gate,
        facts,
    }
}

enum OwnedOperationPlan {
    Issue(Box<E05IssuePlan>),
    Append(Box<E05AppendPlan>),
    Complete(Box<E05CompletePlan>),
}
impl OwnedOperationPlan {
    fn native_expected(&self) -> &RawDatabase {
        match self {
            Self::Issue(p) => &p.expected,
            Self::Append(p) => &p.expected,
            Self::Complete(p) => &p.prefix.expected,
        }
    }
    fn native_expected_mut(&mut self) -> &mut RawDatabase {
        match self {
            Self::Issue(p) => &mut p.expected,
            Self::Append(p) => &mut p.expected,
            Self::Complete(p) => &mut p.prefix.expected,
        }
    }
    fn native_next(&self) -> usize {
        match self {
            Self::Issue(p) => p.next,
            Self::Append(p) => p.next,
            Self::Complete(p) => p.prefix.next,
        }
    }
    fn native_ticket(&self) -> &MirrorEvaluationTicketV2 {
        match self {
            Self::Issue(p) => &p.ticket,
            Self::Append(p) => &p.after_ticket,
            Self::Complete(p) => &p.prefix.after_ticket,
        }
    }
    fn native_first_stop_digest_r44(&self) -> Result<Option<String>> {
        let prefix = match self {
            Self::Issue(_) => return Ok(None),
            Self::Append(prefix) => prefix.as_ref(),
            Self::Complete(complete) => &complete.prefix,
        };
        if !prefix.first_stop_new_r44 {
            return Ok(None);
        }
        let source = prefix
            .first_stop_r44
            .as_ref()
            .ok_or(Error::ClockRegressed)?;
        reconstruct_first_stop_r44(&prefix.after_ticket, source)?;
        r44_stop_require(
            source.first_ticket_digest == typed_fingerprint(&prefix.after_ticket)?
                && source.namespace == prefix.namespace
                && source.evaluator_actor == prefix.actor
                && prefix.audit.is_none(),
        )?;
        Ok(Some(typed_fingerprint(source)?))
    }

    fn native_legacy_audit(&self) -> Option<(&'static str, &str)> {
        match self {
            Self::Issue(p) => Some(("issue_evaluation_ticket", &p.ticket.id)),
            Self::Append(p) => p.audit.as_ref().map(|(op, id)| (*op, id.as_str())),
            Self::Complete(p) => p.prefix.audit.as_ref().map(|(op, id)| (*op, id.as_str())),
        }
    }
    async fn prepare_next(
        &mut self,
        session: &mut Session,
        ctx: &Context,
        asof: &NativeAsOfPermit,
    ) -> Result<()> {
        match self {
            Self::Complete(p) => p.materialize_after_prefix_ack(session, ctx, asof).await,
            _ => Ok(()),
        }
    }
    fn writes(&self) -> &[IssueWrite] {
        match self {
            Self::Issue(p) => &p.writes,
            Self::Append(p) => &p.writes,
            Self::Complete(p) => &p.prefix.writes,
        }
    }
    fn acknowledge(&mut self, step: usize) -> Result<()> {
        match self {
            Self::Issue(p) => p.acknowledge(step),
            Self::Append(p) => p.acknowledge(step),
            Self::Complete(p) => p.prefix.acknowledge(step),
        }
    }
    async fn admit(&self, session: &mut Session) -> Result<()> {
        match self {
            Self::Issue(p) => p.admit(session).await,
            Self::Append(p) => p.admit(session).await,
            Self::Complete(p) => p.admit_pending_r24(session).await,
        }
    }
    async fn admit_stable(&self, session: &mut Session) -> Result<()> {
        match self {
            Self::Issue(p) => p.admit_stable(session).await,
            Self::Append(p) => p.admit_stable(session).await,
            Self::Complete(p) => p.admit_all_pending_r24(session).await,
        }
    }
    async fn audit(&mut self, session: &mut Session, ctx: &Context) -> Result<()> {
        match self {
            Self::Issue(p) => p.audit(session, ctx).await,
            Self::Append(p) => p.audit(session, ctx).await,
            Self::Complete(p) => p.audit_pending_r24(session, ctx).await,
        }
    }
    fn into_output(self) -> Result<Value> {
        match self {
            Self::Issue(p) => serde_json::to_value(p.ticket).map_err(crate::internal),
            Self::Append(p) => p.output,
            Self::Complete(p) if p.materialized => p.prefix.output,
            Self::Complete(_) => Err(Error::Internal),
        }
    }
}

// RawDatabase and native plans carry exact OwnedCell variants. The synthetic
// __clock_type_* columns belong only to rows() projections; global collectors
// keep their own stricter projection contract.
fn native_text(row: &OwnedRow, column: &str) -> Result<String> {
    clock(row.try_get::<String, _>(column))
}

fn native_integer(row: &OwnedRow, column: &str) -> Result<i64> {
    clock(row.try_get::<i64, _>(column))
}

enum IssueWrite {
    Object {
        before: Option<OwnedRow>,
        after: OwnedRow,
    },
    Edge(OwnedRow),
}
impl IssueWrite {
    async fn execute(&self, session: &mut Session) -> Result<()> {
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        let affected = match self {
            Self::Object { before, after } => {
                let namespace = native_text(after, "namespace")?;
                let kind = native_text(after, "kind")?;
                let id = native_text(after, "id")?;
                let owner = native_text(after, "owner")?;
                let body = native_text(after, "body")?;
                if let Some(before) = before {
                    sqlx::query("UPDATE objects SET owner=?,body=?,revision=revision+1 WHERE namespace=? AND kind=? AND id=? AND owner=? AND body=? AND revision=?")
                        .bind(owner).bind(body).bind(namespace).bind(kind).bind(id)
                        .bind(native_text(before, "owner")?).bind(native_text(before, "body")?)
                        .bind(native_integer(before, "revision")?)
                        .execute(&mut **tx).await.map_err(crate::internal)?.rows_affected()
                } else {
                    sqlx::query("INSERT INTO objects(namespace,kind,id,owner,body) VALUES(?,?,?,?,?)")
                        .bind(namespace).bind(kind).bind(id).bind(owner).bind(body)
                        .execute(&mut **tx).await.map_err(crate::internal)?.rows_affected()
                }
            }
            Self::Edge(row) => {
                sqlx::query("INSERT INTO dependencies(namespace,src_kind,src_id,dst_kind,dst_id) VALUES(?,?,?,?,?)")
                    .bind(native_text(row, "namespace")?).bind(native_text(row, "src_kind")?)
                    .bind(native_text(row, "src_id")?).bind(native_text(row, "dst_kind")?)
                    .bind(native_text(row, "dst_id")?)
                    .execute(&mut **tx).await.map_err(crate::internal)?.rows_affected()
            }
        };
        require(affected == 1)
    }

    fn acknowledge(&self, expected: &mut RawDatabase) -> Result<()> {
        match self {
            Self::Object { before, after } => {
                let table = expected
                    .tables
                    .get_mut("objects")
                    .ok_or(Error::ClockRegressed)?;
                if let Some(before) = before {
                    let old = row_bytes(before);
                    let positions: Vec<_> = table
                        .iter()
                        .enumerate()
                        .filter_map(|(i, row)| (row_bytes(row) == old).then_some(i))
                        .collect();
                    require(positions.len() == 1)?;
                    table[positions[0]] = after.clone();
                } else {
                    table.push(after.clone());
                }
            }
            Self::Edge(row) => expected
                .tables
                .get_mut("dependencies")
                .ok_or(Error::ClockRegressed)?
                .push(row.clone()),
        }
        Ok(())
    }
}

struct E05IssuePlan {
    namespace: String,
    actor: String,
    expected: RawDatabase,
    control_digest: String,
    holdout_digest: String,
    ticket: MirrorEvaluationTicketV2,
    writes: Vec<IssueWrite>,
    next: usize,
    receipt_step: usize,
    ticket_step: usize,
}
impl E05IssuePlan {
    fn derive(
        database: &RawDatabase,
        ctx: &Context,
        intent: IssueIntent,
        event: &crate::write_clock::QualifiedEventTime,
    ) -> Result<Self> {
        let issued_at = event.seconds();
        let issued_ms = event.millis();
        require(issued_at == issued_ms / 1000)?;
        let (snapshot, _) = snapshot(database)?;
        if record_optional::<Value>(
            &snapshot,
            ctx.namespace(),
            "registered_evaluation_control_v41",
            &intent.registration_id,
        )?
        .is_none()
            || record_optional::<Value>(
                &snapshot,
                ctx.namespace(),
                "protected_holdout_v41",
                &intent.holdout_id,
            )?
            .is_none()
        {
            return Err(Error::NotFound);
        }
        let control = evaluation_control(&snapshot, ctx.namespace(), &intent.registration_id)?;
        let holdout = evaluation_holdout(&snapshot, ctx.namespace(), &intent.holdout_id, &control)?;
        producer_witnesses(&snapshot, ctx.namespace(), &control, &holdout)?;
        require(control.evaluator_actor == ctx.actor())?;
        require(
            issued_at >= control.created_at_unix_seconds
                && issued_ms >= holdout.manifest.issued_at_unix_ms,
        )?;
        validate_root(&snapshot, ctx.namespace(), &control)?;
        e05_require_exact_edges(
            &snapshot,
            ctx.namespace(),
            &evaluation_storage_id("protected_holdout_v41", &holdout.id)?,
            vec![e05_artifact_edge(
                "registered_evaluation_control_v41",
                &control.id,
            )?],
        )?;
        let binding = derive_candidate(&snapshot, ctx.namespace(), &control, &holdout)?;
        let upstream = derive_nodes(
            &snapshot,
            ctx.namespace(),
            vec![
                ("artifact".into(), holdout.pair.candidate_digest.clone()),
                e05_artifact_edge("protected_holdout_v41", &holdout.id)?,
            ],
        )?;
        let allocations: Vec<_> = control
            .alpha_plan
            .allocations
            .iter()
            .filter(|allocation| allocation.attempt_id == control.attempt_id)
            .collect();
        require(
            !allocations.is_empty()
                && upstream
                    .len()
                    .saturating_add(2)
                    .saturating_add(allocations.len())
                    <= 10000,
        )?;
        let ledger_id =
            evaluation_storage_id("exposure_ledger_v1", &control.alpha_plan.research_family_id)?;
        let ledger_key = (
            ctx.namespace().to_owned(),
            "artifact".to_owned(),
            ledger_id.clone(),
        );
        let mut ledger = match snapshot.objects.get(&ledger_key) {
            Some(object) => decode::<ExposureLedger>(envelope(
                object,
                "rsia.typed_artifact_envelope.v1",
                "exposure_ledger_v1",
            )?)?,
            None => ExposureLedger::new(
                control.alpha_plan.research_family_id.clone(),
                control.alpha_plan.digest()?,
                control.v1_plan_snapshot.query_limit,
            )?,
        };
        ledger.reserve(intent.ticket_id.clone(), &holdout.manifest)?;
        let receipt = MirrorTicketIssueReceiptV2 {
            schema_version: "rsia.evaluation_ticket_issue_receipt.v2".into(),
            ticket_id: intent.ticket_id.clone(),
            registration_digest: typed_fingerprint(&control)?,
            manifest_digest: holdout.manifest.digest()?,
            evaluator_actor: ctx.actor().into(),
            issued_at_unix_seconds: issued_at,
            issued_at_unix_ms: issued_ms,
        };
        let mut bound_v1 = control.v1_plan_snapshot.clone();
        bound_v1.bind_candidate(holdout.pair.candidate_digest.clone())?;
        let targets = holdout
            .rotating_inputs
            .iter()
            .map(|input| MirrorEvaluationTarget {
                target_id: input.target_id.clone(),
                kind: MirrorEvaluationTargetKind::Rotating {
                    cluster_id: input.cluster_or_anchor_id.clone(),
                },
                input_artifact_id: input.input_artifact_id.clone(),
                input_digest: input.input_digest.clone(),
                state: MirrorTargetState::Planned,
            })
            .chain(
                holdout
                    .anchor_inputs
                    .iter()
                    .map(|input| MirrorEvaluationTarget {
                        target_id: input.target_id.clone(),
                        kind: MirrorEvaluationTargetKind::Anchor {
                            anchor_id: input.cluster_or_anchor_id.clone(),
                        },
                        input_artifact_id: input.input_artifact_id.clone(),
                        input_digest: input.input_digest.clone(),
                        state: MirrorTargetState::Planned,
                    }),
            )
            .collect();
        let ticket = MirrorEvaluationTicketV2 {
            schema_version: "rsia.evaluation_ticket.v2".into(),
            id: intent.ticket_id.clone(),
            registration_id: control.id.clone(),
            registration_digest: receipt.registration_digest.clone(),
            holdout_id: holdout.id.clone(),
            manifest_digest: receipt.manifest_digest.clone(),
            v1_plan_snapshot_digest: control.formal_plan.v1_plan_snapshot_digest.clone(),
            bound_v1_plan_digest: bound_v1.digest()?,
            formal_plan_digest: control.formal_plan.digest()?,
            profile: control.formal_plan.profile,
            n_planned: control.formal_plan.n_planned,
            research_family_id: control.alpha_plan.research_family_id.clone(),
            alpha_plan_digest: control.alpha_plan.digest()?,
            candidate_digest: holdout.pair.candidate_digest.clone(),
            baseline_digest: holdout.pair.baseline_digest.clone(),
            candidate_bundle_digest: holdout.candidate_bundle_digest.clone(),
            baseline_bundle_digest: holdout.baseline_bundle_digest.clone(),
            environment_digest: holdout.manifest.environment_digest.clone(),
            proposer_actor: control.proposer_actor.clone(),
            evaluator_actor: control.evaluator_actor.clone(),
            executor_actor: control.executor_actor.clone(),
            approver_actor: control.approver_actor.clone(),
            issue_receipt_digest: typed_fingerprint(&receipt)?,
            evidence_scope: control.evidence_scope,
            attempt_id: control.attempt_id.clone(),
            targets,
            lifecycle: MirrorTicketLifecycle::Reserved,
            next_sequence: 1,
            pending_certificate: None,
            stop_member_terminal_digest: None,
            late_execution_receipt_ids: Vec::new(),
            late_grader_receipt_ids: Vec::new(),
            unattributed_budget_call_ids: Vec::new(),
            invalid_reasons: Vec::new(),
            issued_at_unix_seconds: issued_at,
            candidate_dependency_binding: Some(binding),
        };
        ticket.validate()?;
        e05_candidate_binding(
            &snapshot,
            ctx.namespace(),
            &ticket,
            &control,
            &holdout,
            ticket
                .candidate_dependency_binding
                .as_ref()
                .ok_or(Error::ClockRegressed)?,
        )?;
        let mut writes = Vec::new();
        let mut edges = vec![
            ("artifact".to_owned(), ticket.candidate_digest.clone()),
            e05_artifact_edge("protected_holdout_v41", &ticket.holdout_id)?,
            e05_artifact_edge("evaluation_ticket_issue_receipt_v2", &ticket.id)?,
        ];
        for allocation in allocations {
            let id = format!(
                "{}:{}",
                control.alpha_plan.research_family_id, allocation.claim_id
            );
            let reservation = MirrorAlphaClaimReservation {
                schema_version: "rsia.alpha_claim_reservation.v1".into(),
                research_family_id: control.alpha_plan.research_family_id.clone(),
                claim_id: allocation.claim_id.clone(),
                attempt_id: allocation.attempt_id.clone(),
                ticket_id: ticket.id.clone(),
            };
            writes.push(object_write(
                database,
                ctx,
                "alpha_claim_reservation_v1",
                &id,
                &reservation,
                false,
            )?);
            edges.push(e05_artifact_edge("alpha_claim_reservation_v1", &id)?);
        }
        writes.push(object_write(
            database,
            ctx,
            "exposure_ledger_v1",
            &control.alpha_plan.research_family_id,
            &ledger,
            true,
        )?);
        let receipt_step = writes.len();
        writes.push(object_write(
            database,
            ctx,
            "evaluation_ticket_issue_receipt_v2",
            &ticket.id,
            &receipt,
            false,
        )?);
        let ticket_step = writes.len();
        writes.push(object_write(
            database,
            ctx,
            "evaluation_ticket_v2",
            &ticket.id,
            &ticket,
            false,
        )?);
        let physical_ticket = evaluation_storage_id("evaluation_ticket_v2", &ticket.id)?;
        // A dangling old edge is not this operation's acknowledged prefix.
        require(
            e05_outgoing(&snapshot, ctx.namespace(), "artifact", &physical_ticket)?.is_empty(),
        )?;
        edges.sort();
        edges.dedup();
        for (kind, id) in edges {
            writes.push(IssueWrite::Edge(OwnedRow {
                cells: BTreeMap::from([
                    ("namespace".into(), OwnedCell::Text(ctx.namespace().into())),
                    ("src_kind".into(), OwnedCell::Text("artifact".into())),
                    ("src_id".into(), OwnedCell::Text(physical_ticket.clone())),
                    ("dst_kind".into(), OwnedCell::Text(kind)),
                    ("dst_id".into(), OwnedCell::Text(id)),
                ]),
            }));
        }
        Ok(Self {
            namespace: ctx.namespace().into(),
            actor: ctx.actor().into(),
            expected: clone_database(database),
            control_digest: typed_fingerprint(&control)?,
            holdout_digest: typed_fingerprint(&holdout)?,
            ticket,
            writes,
            next: 0,
            receipt_step,
            ticket_step,
        })
    }

    fn acknowledge(&mut self, step: usize) -> Result<()> {
        require(self.next == step)?;
        self.writes
            .get(step)
            .ok_or(Error::ClockRegressed)?
            .acknowledge(&mut self.expected)?;
        self.next = self.next.checked_add(1).ok_or(Error::ClockRegressed)?;
        Ok(())
    }

    fn qualify(
        &self,
        database: &RawDatabase,
        inputs: &PreparePreimages,
    ) -> Result<OccurrenceFloor> {
        same_database(database, &self.expected)?;
        let (snapshot, mut floor) = snapshot(database)?;
        let control = evaluation_control(&snapshot, &self.namespace, &self.ticket.registration_id)?;
        let holdout = evaluation_holdout(
            &snapshot,
            &self.namespace,
            &self.ticket.holdout_id,
            &control,
        )?;
        require(
            typed_fingerprint(&control)? == self.control_digest
                && typed_fingerprint(&holdout)? == self.holdout_digest
                && control.evaluator_actor == self.actor,
        )?;
        producer_witnesses(&snapshot, &self.namespace, &control, &holdout)?;
        validate_root(&snapshot, &self.namespace, &control)?;
        let binding = derive_candidate(&snapshot, &self.namespace, &control, &holdout)?;
        require(Some(&binding) == self.ticket.candidate_dependency_binding.as_ref())?;
        // These are construction inputs, NOT substituted objects in snapshot.
        // Only a member actually inserted by an acknowledged exact step receives
        // its narrow pending interpretation; every other record remains Stable.
        let receipt_id =
            evaluation_storage_id("evaluation_ticket_issue_receipt_v2", &self.ticket.id)?;
        let ticket_id = evaluation_storage_id("evaluation_ticket_v2", &self.ticket.id)?;
        for object in snapshot.objects.values() {
            let own = object.namespace == self.namespace && object.kind == "artifact";
            let pending = self.next < self.writes.len()
                && own
                && ((object.id == receipt_id && self.next > self.receipt_step)
                    || (object.id == ticket_id && self.next > self.ticket_step));
            if pending {
                // Exact original bytes, owner, revision and full SQL prefix were
                // compared above; future members never appear in this snapshot.
                floor.observe_seconds(self.ticket.issued_at_unix_seconds)?;
                if object.id == receipt_id {
                    let actual: MirrorTicketIssueReceiptV2 = decode(envelope(
                        object,
                        "rsia.typed_artifact_envelope.v1",
                        "evaluation_ticket_issue_receipt_v2",
                    )?)?;
                    require(actual.issued_at_unix_seconds == actual.issued_at_unix_ms / 1000)?;
                    floor.observe_millis(actual.issued_at_unix_ms)?;
                }
            } else {
                qualify_object(object, &snapshot, inputs, &mut floor)?;
            }
        }
        collect_caches(database, &snapshot, inputs, &mut floor)?;
        Ok(floor)
    }

    async fn admit(&self, session: &mut Session) -> Result<()> {
        session.clock.check()?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        require(SqliteTransactionManager::get_transaction_depth(&**tx) == 1)?;
        require(tx.is_in_transaction())?;
        let database = load_sqlx_database(tx)
            .await
            .map_err(|_| session.clock.fail())?;
        require(tx.is_in_transaction())?;
        let floor = match self.qualify(&database, &session.prepare_inputs) {
            Ok(floor) => floor,
            Err(_) => return Err(session.clock.fail()),
        };
        let stamp = session.clock.seconds(&floor)?;
        session.clock.take_seconds(stamp)?;
        Ok(())
    }

    async fn admit_stable(&self, session: &mut Session) -> Result<()> {
        require(self.next == self.writes.len())?;
        session.clock.check()?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        require(
            SqliteTransactionManager::get_transaction_depth(&**tx) == 1 && tx.is_in_transaction(),
        )?;
        let database = load_sqlx_database(tx)
            .await
            .map_err(|_| session.clock.fail())?;
        require(tx.is_in_transaction())?;
        same_database(&database, &self.expected).map_err(|_| session.clock.fail())?;
        let floor = collect_raw(&database, &session.prepare_inputs, SchemaRead::Stable)
            .map_err(|_| session.clock.fail())?;
        let stamp = session.clock.seconds(&floor)?;
        session.clock.take_seconds(stamp)?;
        Ok(())
    }

    async fn audit(&mut self, session: &mut Session, ctx: &Context) -> Result<()> {
        self.admit_stable(session).await?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        let previous = self
            .expected
            .tables
            .get("audit")
            .ok_or(Error::ClockRegressed)?
            .iter()
            .filter(|row| native_text(row, "namespace").ok().as_deref() == Some(ctx.namespace()))
            .max_by_key(|row| native_integer(row, "seq").unwrap_or(i64::MIN))
            .map(|row| native_text(row, "digest"))
            .transpose()?
            .unwrap_or_default();
        // This fresh sample is the audit event; it does not rewrite issue time.
        let database = load_sqlx_database(tx).await?;
        let floor = collect_raw(&database, &session.prepare_inputs, SchemaRead::Stable)?;
        let stamp = session.clock.seconds(&floor)?;
        let timestamp = session.clock.take_seconds(stamp)?;
        let payload = serde_json::json!({
            "actor":ctx.actor(), "role":ctx.role(), "operation":"issue_evaluation_ticket",
            "entity":self.ticket.id, "timestamp":timestamp
        })
        .to_string();
        let current = hash(format!("{previous}\n{payload}").as_bytes());
        let seq: i64 = sqlx::query_scalar("INSERT INTO audit(namespace,payload,previous_hash,digest) VALUES(?,?,?,?) RETURNING seq")
            .bind(ctx.namespace()).bind(&payload).bind(&previous).bind(&current)
            .fetch_one(&mut **tx).await.map_err(crate::internal)?;
        require(
            seq > 0
                && self
                    .expected
                    .tables
                    .get("audit")
                    .ok_or(Error::ClockRegressed)?
                    .iter()
                    .all(|row| native_integer(row, "seq").is_ok_and(|old| old < seq)),
        )?;
        self.expected
            .tables
            .get_mut("audit")
            .ok_or(Error::ClockRegressed)?
            .push(OwnedRow {
                cells: BTreeMap::from([
                    ("seq".into(), OwnedCell::Integer(seq)),
                    ("namespace".into(), OwnedCell::Text(ctx.namespace().into())),
                    ("payload".into(), OwnedCell::Text(payload)),
                    ("previous_hash".into(), OwnedCell::Text(previous)),
                    ("digest".into(), OwnedCell::Text(current)),
                ]),
            });
        Ok(())
    }
}

fn snapshot(database: &RawDatabase) -> Result<(OccurrenceSnapshot, OccurrenceFloor)> {
    let mut floor = OccurrenceFloor::default();
    let prefix = schema_prefix(database, SchemaRead::Stable, &mut floor)?;
    require(prefix > 0)?;
    let mut snapshot = OccurrenceSnapshot::default();
    collect_audit(database, &mut floor, &mut snapshot)?;
    collect_objects(database, &mut snapshot)?;
    if prefix >= 2 {
        load_dependencies(database, &mut snapshot)?;
    }
    if prefix >= 3 {
        collect_replay_worlds(database, &mut snapshot)?;
    }
    if prefix >= 4 {
        collect_budget(database, &mut floor, &mut snapshot)?;
    }
    if prefix >= 5 {
        collect_cleanup(database, &mut floor, &snapshot)?;
    }
    load_caches(database, &mut snapshot)?;
    load_complete_origins_r24(database, &mut snapshot)?;
    snapshot.precompute_actual_body_digests_r26();
    Ok((snapshot, floor))
}

fn validate_root(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    control: &MirrorRegisteredEvaluationControl,
) -> Result<()> {
    require(control.billing_scope == control.optimization_budget.root_budget_scope_id)?;
    let root = snapshot
        .roots
        .get(&control.billing_scope)
        .ok_or(Error::Budget)?;
    let receipt = control
        .optimization_budget
        .admin_authorization_receipt_digest
        .as_deref()
        .ok_or(Error::Budget)?;
    let (negative, units) =
        evo_core::evaluation::parse_decimal(&control.optimization_budget.root_total.amount)?;
    require(!negative && units != 0 && units % 100 == 0)?;
    let amount = i64::try_from(units / 100).map_err(|_| Error::Budget)?;
    if root.authorizing_namespace != namespace {
        return Err(Error::Forbidden);
    }
    require(
        !root.stopped
            && root.currency == control.optimization_budget.root_total.currency
            && root.pricing_version == control.optimization_budget.root_total.pricing_version
            && root.total_limit_micros == amount
            && root.authorization_receipt_digest == receipt,
    )
}

fn derive_nodes(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    roots: Vec<(String, String)>,
) -> Result<Vec<MirrorFormalDependencyNode>> {
    let mut pending: BTreeSet<_> = roots.into_iter().collect();
    let mut seen = BTreeSet::new();
    let mut nodes = Vec::new();
    let mut compiler_roots = Vec::new();
    while let Some((kind, id)) = pending.pop_first() {
        if !seen.insert((kind.clone(), id.clone())) {
            continue;
        }
        require(seen.len() <= 10000)?;
        let outgoing = e05_outgoing(snapshot, namespace, &kind, &id)?;
        for edge in &outgoing {
            if !seen.contains(edge) {
                pending.insert(edge.clone());
            }
        }
        require(seen.len() + pending.len() <= 10000)?;
        let object = snapshot.object(namespace, &kind, &id)?;
        let (schema_version, record_kind, logical_id, content_digest) =
            e05_dependency_identity(snapshot, object)?;
        if compiler_node_schema(&kind, &schema_version) {
            compiler_roots.push((kind.clone(), id.clone()));
        }
        nodes.push(MirrorFormalDependencyNode {
            kind,
            id,
            schema_version,
            record_kind,
            logical_id,
            content_digest,
            outgoing_edge_digest: typed_fingerprint(&outgoing)?,
            outgoing_edge_count: outgoing.len(),
        });
    }
    nodes.sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
    for root in compiler_roots {
        e05_compiler_closure(snapshot, namespace, root)?;
    }
    Ok(nodes)
}

fn derive_candidate(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    control: &MirrorRegisteredEvaluationControl,
    holdout: &MirrorProtectedHoldoutRecord,
) -> Result<MirrorFormalCandidateDependencyBinding> {
    let object = snapshot.object(namespace, "artifact", &holdout.pair.candidate_digest)?;
    time_free_record(object, snapshot)?;
    e05_dependency_live(snapshot, object)?;
    let candidate: MirrorReleaseCandidateRecord = decode(&object.body)?;
    let watermark = snapshot
        .watermarks
        .get(namespace)
        .ok_or(Error::ClockRegressed)?;
    require(
        candidate.id == holdout.pair.candidate_digest
            && candidate.bundle_digest == holdout.candidate_bundle_digest
            && candidate.parent_digest == holdout.pair.baseline_digest
            && candidate.environment_digest == holdout.manifest.environment_digest
            && candidate.proposer_actor == control.proposer_actor
            && candidate.revoke_watermark == watermark.0,
    )?;
    let outgoing = e05_outgoing(snapshot, namespace, "artifact", &candidate.id)?;
    for source in &candidate.sources {
        require(outgoing.contains(&(source.kind.clone(), source.id.clone())))?;
        let object = snapshot.object(namespace, &source.kind, &source.id)?;
        e05_dependency_live(snapshot, object)?;
    }
    let binding = MirrorFormalCandidateDependencyBinding {
        schema_version: "rsia.formal_candidate_dependency_binding.v1".into(),
        namespace: namespace.into(),
        candidate_id: candidate.id.clone(),
        candidate_record_digest: typed_fingerprint(&candidate)?,
        bundle_digest: candidate.bundle_digest,
        parent_digest: candidate.parent_digest,
        profile_id: candidate.profile_id,
        environment_digest: candidate.environment_digest,
        proposer_actor: candidate.proposer_actor,
        sources: candidate.sources,
        revoke_watermark_seq: i64::try_from(watermark.0).map_err(|_| Error::ClockRegressed)?,
        revoke_watermark_digest: watermark.1.clone(),
        nodes: derive_nodes(snapshot, namespace, vec![("artifact".into(), candidate.id)])?,
    };
    binding.validate()?;
    Ok(binding)
}

fn r44_stop_require(condition: bool) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(r24_fault(evo_core::formal_version::Fault::InvalidContent))
    }
}

fn first_stop_from_body_r44(
    value: &Value,
    namespace: &str,
    physical: &str,
) -> Result<Option<evo_core::formal_version::NativeFirstStopV2>> {
    use evo_core::formal_version as fv;
    if value.get("schema_version").and_then(Value::as_str) != Some(fv::NATIVE_TICKET_ENVELOPE) {
        return Ok(None);
    }
    fv::native_ticket_payload(value, namespace, physical).map_err(r24_fault)?;
    value
        .get("native_first_stop")
        .filter(|v| !v.is_null())
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(crate::internal)
}
fn reconstruct_first_stop_r44(
    final_ticket: &MirrorEvaluationTicketV2,
    source: &evo_core::formal_version::NativeFirstStopV2,
) -> Result<MirrorEvaluationTicketV2> {
    source.validate_known().map_err(r24_fault)?;
    r44_stop_require(
        source.ticket_id == final_ticket.id
            && source.evaluator_actor == final_ticket.evaluator_actor
            && source.member_flags.len() == final_ticket.targets.len(),
    )?;
    let mut original = final_ticket.clone();
    for (target, code) in original.targets.iter_mut().zip(source.member_flags.bytes()) {
        let flags = (code as char).to_digit(16).ok_or(Error::ClockRegressed)? as u8;
        let tag = flags & 3;
        let state = target.state.clone();
        target.state = match (tag, state) {
            (0, state @ MirrorTargetState::Completed { .. }) if flags == 0 => state,
            (1, state @ MirrorTargetState::NotDispatched { .. }) if flags == 1 => state,
            (
                2,
                MirrorTargetState::CancelledAfterDispatch {
                    mut candidate,
                    mut baseline,
                },
            )
            | (
                3,
                MirrorTargetState::CancelledAfterDispatch {
                    mut candidate,
                    mut baseline,
                },
            )
            | (
                3,
                MirrorTargetState::RunningUsageUnknown {
                    mut candidate,
                    mut baseline,
                },
            ) => {
                for (slot, present) in [
                    (&mut candidate, flags & 4 != 0),
                    (&mut baseline, flags & 8 != 0),
                ] {
                    match slot {
                        Some(slot) if present => r44_stop_require(slot.receipt_id.is_some())?,
                        Some(slot) => slot.receipt_id = None,
                        None => r44_stop_require(!present)?,
                    }
                }
                if tag == 3 {
                    MirrorTargetState::RunningUsageUnknown {
                        candidate,
                        baseline,
                    }
                } else {
                    MirrorTargetState::CancelledAfterDispatch {
                        candidate,
                        baseline,
                    }
                }
            }
            _ => return Err(r24_fault(evo_core::formal_version::Fault::InvalidContent)),
        };
    }
    let lengths = source.initial_list_lengths;
    r44_stop_require(
        original.late_execution_receipt_ids.len() >= lengths[0]
            && original.late_grader_receipt_ids.len() >= lengths[1]
            && original.unattributed_budget_call_ids.len() >= lengths[2]
            && original.invalid_reasons.len() >= lengths[3],
    )?;
    original.late_execution_receipt_ids.truncate(lengths[0]);
    original.late_grader_receipt_ids.truncate(lengths[1]);
    original.unattributed_budget_call_ids.truncate(lengths[2]);
    original.invalid_reasons.truncate(lengths[3]);
    original.next_sequence = source.first_next_sequence;
    original.lifecycle = if source.initially_draining {
        MirrorTicketLifecycle::Draining
    } else {
        MirrorTicketLifecycle::EarlyStopped
    };
    original.pending_certificate = Some(source.certificate.clone());
    original.stop_member_terminal_digest = Some(source.member_terminal_digest.clone());
    r44_stop_require(
        typed_fingerprint(&original.targets)? == source.member_terminal_digest
            && typed_fingerprint(&original)? == source.first_ticket_digest,
    )?;
    original.validate()?;
    Ok(original)
}
fn freeze_first_stop_r44(
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
) -> Result<evo_core::formal_version::NativeFirstStopV2> {
    use evo_core::formal_version as fv;
    r44_stop_require(matches!(
        ticket.lifecycle,
        MirrorTicketLifecycle::Draining | MirrorTicketLifecycle::EarlyStopped
    ))?;
    let mut flags = String::with_capacity(ticket.targets.len());
    for target in &ticket.targets {
        let flag = match &target.state {
            MirrorTargetState::Completed { .. } => 0,
            MirrorTargetState::NotDispatched { .. } => 1,
            MirrorTargetState::CancelledAfterDispatch {
                candidate,
                baseline,
            }
            | MirrorTargetState::RunningUsageUnknown {
                candidate,
                baseline,
            } => {
                let tag = if matches!(target.state, MirrorTargetState::RunningUsageUnknown { .. }) {
                    3
                } else {
                    2
                };
                tag | if candidate.as_ref().is_some_and(|s| s.receipt_id.is_some()) {
                    4
                } else {
                    0
                } | if baseline.as_ref().is_some_and(|s| s.receipt_id.is_some()) {
                    8
                } else {
                    0
                }
            }
            _ => return Err(Error::ClockRegressed),
        };
        flags.push(char::from_digit(flag, 16).ok_or(Error::ClockRegressed)?);
    }
    let source = fv::NativeFirstStopV2 {
        schema_version: fv::FIRST_STOP_SCHEMA.into(),
        namespace: namespace.into(),
        ticket_id: ticket.id.clone(),
        evaluator_actor: ticket.evaluator_actor.clone(),
        first_ticket_digest: typed_fingerprint(ticket)?,
        member_terminal_digest: typed_fingerprint(&ticket.targets)?,
        member_flags: flags,
        first_next_sequence: ticket.next_sequence,
        initially_draining: ticket.lifecycle == MirrorTicketLifecycle::Draining,
        initial_list_lengths: [
            ticket.late_execution_receipt_ids.len(),
            ticket.late_grader_receipt_ids.len(),
            ticket.unattributed_budget_call_ids.len(),
            ticket.invalid_reasons.len(),
        ],
        certificate: ticket
            .pending_certificate
            .clone()
            .ok_or(Error::ClockRegressed)?,
    };
    source.validate_known().map_err(r24_fault)?;
    reconstruct_first_stop_r44(ticket, &source)?;
    Ok(source)
}
pub(super) fn validate_native_ticket_stop_r44(
    object: &OccurrenceObject,
    ticket: &MirrorEvaluationTicketV2,
) -> Result<()> {
    if let Some(source) = first_stop_from_body_r44(&object.body, &object.namespace, &object.id)? {
        reconstruct_first_stop_r44(ticket, &source)?;
        let expected = if ticket.lifecycle == MirrorTicketLifecycle::EarlyStopped
            && source.initially_draining
        {
            source.certificate.clone().with_usage_reconciled()?
        } else {
            source.certificate.clone()
        };
        r44_stop_require(ticket.pending_certificate.as_ref().is_some_and(|actual| {
            typed_fingerprint(actual).ok() == typed_fingerprint(&expected).ok()
        }))?;
    } else if object.body["schema_version"] == evo_core::formal_version::NATIVE_TICKET_ENVELOPE {
        r44_stop_require(!matches!(
            ticket.lifecycle,
            MirrorTicketLifecycle::Draining | MirrorTicketLifecycle::EarlyStopped
        ))?;
    }
    Ok(())
}
fn first_stop_source_from_snapshot_r44(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
) -> Result<Option<evo_core::formal_version::NativeFirstStopV2>> {
    let object = snapshot.object(
        namespace,
        "artifact",
        &evaluation_storage_id(TICKET_KIND, &ticket.id)?,
    )?;
    first_stop_from_body_r44(&object.body, namespace, &object.id)
}
fn verify_actual_first_stop_r44(
    raw: &RawDatabase,
    namespace: &str,
    ticket: &MirrorEvaluationTicketV2,
    saved: Option<
        &evo_core::formal_version::OriginalValueV2<evo_core::formal_version::NativeFirstStopV2>,
    >,
    gaps: &mut BTreeSet<&'static str>,
) -> Result<()> {
    if let Some(saved) = saved {
        saved.snapshot.validate_known().map_err(r24_fault)?;
        r44_stop_require(
            saved.typed_digest == typed_fingerprint(&saved.snapshot)?
                && saved.snapshot.namespace == namespace,
        )?;
        reconstruct_first_stop_r44(ticket, &saved.snapshot)?;
    } else {
        gaps.insert("native_first_stop_original_missing");
    }
    let physical = evaluation_storage_id(TICKET_KIND, &ticket.id)?;
    let mut actual_source = None;
    let mut actual_rows = 0_usize;
    for row in raw.tables.get("objects").ok_or(Error::ClockRegressed)? {
        if native_text(row, "namespace")? != namespace
            || native_text(row, "kind")? != "artifact"
            || native_text(row, "id")? != physical
        {
            continue;
        }
        actual_rows = actual_rows.checked_add(1).ok_or(Error::ClockRegressed)?;
        let body = parse_owned_json(&native_text(row, "body")?)?;
        if let Some(actual) = first_stop_from_body_r44(&body, namespace, &physical)? {
            reconstruct_first_stop_r44(ticket, &actual)?;
            if let Some(saved) = saved {
                r44_stop_require(typed_fingerprint(&actual)? == saved.typed_digest)?;
            }
            actual_source = Some(actual);
        } else {
            gaps.insert("actual_ticket_first_stop_capsule_missing");
        }
    }
    r44_stop_require(actual_rows <= 1)?;
    if actual_rows == 0 {
        gaps.insert("actual_ticket_first_stop_source_missing");
    }
    let mut matches = 0_usize;
    for row in raw.tables.get("audit").ok_or(Error::ClockRegressed)? {
        if native_text(row, "namespace")? != namespace {
            continue;
        }
        let text = native_text(row, "payload")?;
        parse_owned_json(&text)?;
        let payload: AuditPayloadR24 = serde_json::from_str(&text).map_err(crate::internal)?;
        if payload.operation != "formal.native.asof.grade.v1" {
            continue;
        }
        let Some(wrapper) = decode_first_stop_asof_entity_r44(&payload.entity)? else {
            // An old summary digest has no retained first-stop preimage.
            continue;
        };
        // These known checks do not depend on a missing frozen capsule or row.
        validate_first_stop_asof_row_r44(raw, row, &payload, &wrapper.summary)?;
        if wrapper.summary.ticket_id != ticket.id {
            continue;
        }
        r44_stop_require(
            wrapper.summary.namespace == namespace
                && wrapper.summary.actor == ticket.evaluator_actor,
        )?;
        if let Some(saved) = saved {
            r44_stop_require(wrapper.native_first_stop_digest == saved.typed_digest)?;
        }
        if let Some(source) = actual_source
            .as_ref()
            .or_else(|| saved.map(|v| &v.snapshot))
        {
            r44_stop_require(wrapper.native_first_stop_digest==typed_fingerprint(source)?
                && wrapper.summary.audit_timestamp>=serde_json::to_value(&source.certificate)
                    .map_err(crate::internal)?["stopped_at_unix_ms"]
                    .as_i64().ok_or(Error::ClockRegressed)?/1000)?;
        }
        matches = matches.checked_add(1).ok_or(Error::ClockRegressed)?;
    }
    r44_stop_require(matches <= 1)?;
    if matches == 0 {
        gaps.insert("actual_native_first_stop_grade_asof_missing");
    }
    Ok(())
}

fn object_write<T: Serialize>(
    database: &RawDatabase,
    ctx: &Context,
    kind: &str,
    id: &str,
    payload: &T,
    replace: bool,
) -> Result<IssueWrite> {
    let physical = evaluation_storage_id(kind, id)?;
    let mut body = serde_json::to_string(&E05FingerprintEnvelope {
        schema_version: "rsia.typed_artifact_envelope.v1",
        id: &physical,
        record_kind: kind,
        payload,
    })
    .map_err(crate::internal)?;
    require(body.len() <= 4 * 1024 * 1024)?;
    let table = database
        .tables
        .get("objects")
        .ok_or(Error::ClockRegressed)?;
    let matching: Vec<_> = table
        .iter()
        .filter(|row| {
            native_text(row, "namespace").ok().as_deref() == Some(ctx.namespace())
                && native_text(row, "kind").ok().as_deref() == Some("artifact")
                && native_text(row, "id").ok().as_deref() == Some(physical.as_str())
        })
        .collect();
    require(matching.len() <= 1 && (replace || matching.is_empty()))?;
    let before = matching.first().map(|row| (**row).clone());
    if kind == TICKET_KIND {
        let saved = before
            .as_ref()
            .map(|row| native_text(row, "body"))
            .transpose()?
            .map(|text| parse_owned_json(&text))
            .transpose()?;
        let new_native = before.is_none()
            || saved.as_ref().is_some_and(|value| {
                value["schema_version"] == evo_core::formal_version::NATIVE_TICKET_ENVELOPE
            });
        if new_native {
            let source = saved
                .as_ref()
                .and_then(|value| value.get("native_first_stop"))
                .filter(|value| !value.is_null())
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(crate::internal)?;
            let envelope = evo_core::formal_version::NativeTicketEnvelopeV2 {
                schema_version: evo_core::formal_version::NATIVE_TICKET_ENVELOPE.into(),
                id: physical.clone(),
                record_kind: TICKET_KIND.into(),
                payload,
                native_first_stop: source,
            };
            let value = serde_json::to_value(&envelope).map_err(crate::internal)?;
            evo_core::formal_version::native_ticket_payload(&value, ctx.namespace(), &physical)
                .map_err(r24_fault)?;
            body = serde_json::to_string(&envelope).map_err(crate::internal)?;
        }
    }
    let revision = match &before {
        Some(row) => native_integer(row, "revision")?
            .checked_add(1)
            .ok_or(Error::ClockRegressed)?,
        None => 1,
    };
    Ok(IssueWrite::Object {
        before,
        after: OwnedRow {
            cells: BTreeMap::from([
                ("namespace".into(), OwnedCell::Text(ctx.namespace().into())),
                ("kind".into(), OwnedCell::Text("artifact".into())),
                ("id".into(), OwnedCell::Text(physical)),
                ("owner".into(), OwnedCell::Text(ctx.actor().into())),
                ("body".into(), OwnedCell::Text(body)),
                ("revision".into(), OwnedCell::Integer(revision)),
            ]),
        },
    })
}

fn clone_database(database: &RawDatabase) -> RawDatabase {
    RawDatabase {
        tables: database.tables.clone(),
    }
}

fn row_bytes(row: &OwnedRow) -> Vec<u8> {
    fn part(output: &mut Vec<u8>, value: &[u8]) {
        output.extend_from_slice(&(value.len() as u128).to_le_bytes());
        output.extend_from_slice(value);
    }
    let mut output = Vec::new();
    for (key, value) in &row.cells {
        part(&mut output, key.as_bytes());
        match value {
            OwnedCell::Null => output.push(0),
            OwnedCell::Integer(value) => {
                output.push(1);
                output.extend_from_slice(&value.to_le_bytes());
            }
            OwnedCell::Real(value) => {
                output.push(2);
                output.extend_from_slice(&value.to_bits().to_le_bytes());
            }
            OwnedCell::Text(value) => {
                output.push(3);
                part(&mut output, value.as_bytes());
            }
            OwnedCell::Blob(value) => {
                output.push(4);
                part(&mut output, value);
            }
        }
    }
    output
}
fn database_bytes(database: &RawDatabase) -> BTreeMap<&str, Vec<Vec<u8>>> {
    database
        .tables
        .iter()
        .map(|(table, rows)| {
            let mut rows: Vec<_> = rows.iter().map(row_bytes).collect();
            rows.sort();
            (table.as_str(), rows)
        })
        .collect()
}
fn same_database(actual: &RawDatabase, expected: &RawDatabase) -> Result<()> {
    require(database_bytes(actual) == database_bytes(expected))
}

const EXECUTION_INTENT_SCHEMA: &str = "rsia.e05_execution_intent.v2";
const GRADE_INTENT_SCHEMA: &str = "rsia.e05_grade_intent.v2";
const TICKET_KIND: &str = "evaluation_ticket_v2";
const OUTPUT_KIND: &str = "evaluation_execution_output_v1";
const EXECUTION_RECEIPT_KIND: &str = "execution_receipt_v2";
const GRADER_RECEIPT_KIND: &str = "independent_grader_receipt_v2";
const LEDGER_KIND: &str = "exposure_ledger_v1";
const HOLDOUT_KIND: &str = "protected_holdout_v41";
const FORMAL_KIND: &str = "formal_evaluation_v2";
// Add to e05_issue.rs; old object_write/as and old IDs stay unchanged.
fn versioned_formal_object_write<T: Serialize, C: Serialize, P: Serialize>(
    database: &RawDatabase,
    ctx: &Context,
    carrier: &evo_core::formal_version::CarrierV4<T, C, P>,
) -> Result<IssueWrite> {
    carrier
        .validate_wire(ctx.namespace(), &carrier.id)
        .map_err(|_| Error::ClockRegressed)?;
    let body = serde_json::to_string(carrier).map_err(crate::internal)?;
    require(body.len() <= 4 * 1024 * 1024)?;
    let rows = database
        .tables
        .get("objects")
        .ok_or(Error::ClockRegressed)?;
    let found: Vec<_> = rows
        .iter()
        .filter(|row| {
            native_text(row, "namespace").ok().as_deref() == Some(ctx.namespace())
                && native_text(row, "kind").ok().as_deref() == Some("artifact")
                && native_text(row, "id").ok().as_deref() == Some(carrier.id.as_str())
        })
        .collect();
    // Exact replay is checked before this append. Never update an existing report.
    if !found.is_empty() {
        return Err(Error::Conflict(
            "versioned_formal_id_exists_exact_replay_or_successor_required".into(),
        ));
    }
    Ok(IssueWrite::Object {
        before: None,
        after: OwnedRow {
            cells: BTreeMap::from([
                ("namespace".into(), OwnedCell::Text(ctx.namespace().into())),
                ("kind".into(), OwnedCell::Text("artifact".into())),
                ("id".into(), OwnedCell::Text(carrier.id.clone())),
                ("owner".into(), OwnedCell::Text(ctx.actor().into())),
                ("body".into(), OwnedCell::Text(body)),
                ("revision".into(), OwnedCell::Integer(1)),
            ]),
        },
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutionIntent {
    schema_version: String,
    receipt_id: String,
    ticket_id: String,
    target_id: String,
    side: MirrorExecutionSide,
    request_digest: String,
    budget_call_id: String,
    output_artifact_id: String,
    output_utf8: String,
    latency_micros: u64,
}
impl ExecutionIntent {
    fn read(value: Value) -> Result<Self> {
        let intent: Self = serde_json::from_value(value)
            .map_err(|_| Error::Invalid("invalid E05 execution v2 intent".into()))?;
        if intent.schema_version != EXECUTION_INTENT_SCHEMA {
            return Err(Error::Invalid(
                "unsupported E05 execution intent schema".into(),
            ));
        }
        for id in [
            &intent.receipt_id,
            &intent.ticket_id,
            &intent.target_id,
            &intent.budget_call_id,
            &intent.output_artifact_id,
        ] {
            identifier(id)?;
        }
        digest(&intent.request_digest)?;
        if intent.output_utf8.is_empty() || intent.output_utf8.len() > 256 * 1024 {
            return Err(Error::Invalid(
                "execution output must be 1..=262144 UTF-8 bytes".into(),
            ));
        }
        Ok(intent)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GradeIntent {
    schema_version: String,
    receipt_id: String,
    ticket_id: String,
    target_id: String,
    candidate_execution_receipt_id: String,
    baseline_execution_receipt_id: String,
}
impl GradeIntent {
    fn read(value: Value) -> Result<Self> {
        let intent: Self = serde_json::from_value(value)
            .map_err(|_| Error::Invalid("invalid E05 grade v2 intent".into()))?;
        if intent.schema_version != GRADE_INTENT_SCHEMA {
            return Err(Error::Invalid("unsupported E05 grade intent schema".into()));
        }
        for id in [
            &intent.receipt_id,
            &intent.ticket_id,
            &intent.target_id,
            &intent.candidate_execution_receipt_id,
            &intent.baseline_execution_receipt_id,
        ] {
            identifier(id)?;
        }
        Ok(intent)
    }
}
impl MirrorEvaluationTicketV2 {
    fn target(&self, id: &str) -> Result<&MirrorEvaluationTarget> {
        self.targets
            .iter()
            .find(|target| target.target_id == id)
            .ok_or(Error::NotFound)
    }
    fn target_mut(&mut self, id: &str) -> Result<&mut MirrorEvaluationTarget> {
        self.targets
            .iter_mut()
            .find(|target| target.target_id == id)
            .ok_or(Error::NotFound)
    }
}

// Closed variants, not a caller-defined pending-member set or callback.
enum AppendScope {
    Execution {
        receipt_physical: String,
    },
    Grade {
        receipt_physical: Option<String>,
    },
    Complete {
        resource_physical: String,
        formal_physical: Option<String>,
    },
    CompleteInvalidR1 {
        formal_physical: Option<String>,
    },
    EarlyR44 {
        formal_physical: Option<String>,
    },
    Invalidate,
}
impl AppendScope {
    fn new_member(&self) -> Option<&str> {
        match self {
            Self::Execution { receipt_physical } => Some(receipt_physical),
            Self::Grade { receipt_physical } => receipt_physical.as_deref(),
            Self::Complete {
                formal_physical, ..
            } => formal_physical.as_deref(),
            Self::CompleteInvalidR1 { formal_physical } => formal_physical.as_deref(),
            Self::EarlyR44 { formal_physical } => formal_physical.as_deref(),
            Self::Invalidate => None,
        }
    }
    fn resource_member(&self) -> Option<&str> {
        match self {
            Self::Complete {
                resource_physical, ..
            } => Some(resource_physical),
            _ => None,
        }
    }
}
/// Constructible only after the private native plan matched the complete actual
/// raw database against its SQL-ACK prefix. It cannot be cloned or deserialized.
pub(super) struct E05PhaseEvidence {
    namespace: String,
    ticket_physical: String,
    ticket_fingerprint: String,
    ticket_edges: Vec<(String, String)>,
    new_member: Option<(String, Vec<(String, String)>)>,
    resource_member: Option<(String, Vec<(String, String)>)>,
}
impl E05PhaseEvidence {
    pub(super) fn require_current_prefix(
        &self,
        snapshot: &OccurrenceSnapshot,
        namespace: &str,
        source: &str,
    ) -> Option<Result<()>> {
        if namespace != self.namespace {
            return None;
        }
        let expected = if source == self.ticket_physical {
            &self.ticket_edges
        } else if let Some((_, edges)) = self.new_member.as_ref().filter(|(id, _)| source == id) {
            edges
        } else if let Some((_, edges)) =
            self.resource_member.as_ref().filter(|(id, _)| source == id)
        {
            edges
        } else {
            return None;
        };
        Some((|| {
            // Actual current member, never a future/synthetic ticket.
            snapshot.object(namespace, "artifact", source)?;
            let object = snapshot.object(namespace, "artifact", &self.ticket_physical)?;
            let current: MirrorEvaluationTicketV2 = decode(envelope(
                object,
                "rsia.typed_artifact_envelope.v1",
                TICKET_KIND,
            )?)?;
            current.validate()?;
            require(typed_fingerprint(&current)? == self.ticket_fingerprint)?;
            require(e05_outgoing(snapshot, namespace, "artifact", source)? == *expected)
        })())
    }
}

struct E05AppendPlan {
    namespace: String,
    actor: String,
    expected: RawDatabase,
    before_ticket: MirrorEvaluationTicketV2,
    after_ticket: MirrorEvaluationTicketV2,
    control_digest: String,
    holdout_digest: String,
    scope: AppendScope,
    writes: Vec<IssueWrite>,
    next: usize,
    prior_early_formal: Option<String>,
    first_stop_r44: Option<evo_core::formal_version::NativeFirstStopV2>,
    first_stop_new_r44: bool,
    audit: Option<(&'static str, String)>,
    output: Result<Value>,
}
impl E05AppendPlan {
    fn base(
        database: &RawDatabase,
        ctx: &Context,
        before: MirrorEvaluationTicketV2,
        after: MirrorEvaluationTicketV2,
        scope: AppendScope,
        output: Result<Value>,
    ) -> Result<Self> {
        let (snapshot, _) = snapshot(database)?;
        let control = evaluation_control(&snapshot, ctx.namespace(), &before.registration_id)?;
        let holdout = evaluation_holdout(&snapshot, ctx.namespace(), &before.holdout_id, &control)?;
        producer_witnesses(&snapshot, ctx.namespace(), &control, &holdout)?;
        let prior_early_formal = if before.lifecycle == MirrorTicketLifecycle::EarlyStopped
            && first_stop_source_from_snapshot_r44(&snapshot, ctx.namespace(), &before)?.is_none()
        {
            let object = snapshot.object(
                ctx.namespace(),
                "artifact",
                &evaluation_storage_id(FORMAL_KIND, &before.id)?,
            )?;
            let formal: MirrorFormalEvaluationV2 = decode(envelope(
                object,
                "rsia.typed_artifact_envelope.v1",
                FORMAL_KIND,
            )?)?;
            match formal {
                MirrorFormalEvaluationV2::EarlyStopped {
                    ticket_digest,
                    certificate,
                    ..
                } => {
                    require(
                        ticket_digest == typed_fingerprint(&before)?
                            && before.pending_certificate.as_ref().is_some_and(|old| {
                                typed_fingerprint(old).ok() == typed_fingerprint(&certificate).ok()
                            }),
                    )?;
                    Some(object.value_body_digest_r26()?)
                }
                _ => return Err(Error::ClockRegressed),
            }
        } else {
            None
        };
        let source = first_stop_source_from_snapshot_r44(&snapshot, ctx.namespace(), &before)?;
        let is_first = source.is_none()
            && !matches!(
                before.lifecycle,
                MirrorTicketLifecycle::Draining | MirrorTicketLifecycle::EarlyStopped
            )
            && matches!(
                after.lifecycle,
                MirrorTicketLifecycle::Draining | MirrorTicketLifecycle::EarlyStopped
            );
        let native_ticket = snapshot
            .object(
                ctx.namespace(),
                "artifact",
                &evaluation_storage_id(TICKET_KIND, &before.id)?,
            )?
            .body["schema_version"]
            == evo_core::formal_version::NATIVE_TICKET_ENVELOPE;
        let first_stop_r44 = if is_first && native_ticket {
            Some(freeze_first_stop_r44(ctx.namespace(), &after)?)
        } else {
            source
        };
        Ok(Self {
            namespace: ctx.namespace().into(),
            actor: ctx.actor().into(),
            expected: clone_database(database),
            before_ticket: before,
            after_ticket: after,
            control_digest: typed_fingerprint(&control)?,
            holdout_digest: typed_fingerprint(&holdout)?,
            scope,
            writes: Vec::new(),
            next: 0,
            prior_early_formal,
            first_stop_new_r44: is_first && native_ticket,
            first_stop_r44,
            audit: None,
            output,
        })
    }

    fn execution(
        database: &RawDatabase,
        ctx: &Context,
        intent: ExecutionIntent,
        at: i64,
    ) -> Result<Self> {
        let (snapshot, _) = snapshot(database)?;
        if record_optional::<Value>(&snapshot, ctx.namespace(), TICKET_KIND, &intent.ticket_id)?
            .is_none()
        {
            return Err(Error::NotFound);
        }
        let before = evaluation_ticket(&snapshot, ctx.namespace(), &intent.ticket_id)?;
        let control = evaluation_control(&snapshot, ctx.namespace(), &before.registration_id)?;
        let holdout = evaluation_holdout(&snapshot, ctx.namespace(), &before.holdout_id, &control)?;
        producer_witnesses(&snapshot, ctx.namespace(), &control, &holdout)?;
        if before.executor_actor != ctx.actor() {
            return Err(Error::Forbidden);
        }
        let target = before.target(&intent.target_id)?;
        let bundle = match intent.side {
            MirrorExecutionSide::Candidate => &before.candidate_bundle_digest,
            MirrorExecutionSide::Baseline => &before.baseline_bundle_digest,
        };
        if e05_request_digest(
            &before,
            target,
            intent.side,
            bundle,
            &before.environment_digest,
        )? != intent.request_digest
        {
            return Err(Error::Conflict(
                "execution request differs from the frozen ticket member".into(),
            ));
        }
        let call = snapshot
            .calls
            .get(&(control.billing_scope.clone(), intent.budget_call_id.clone()))
            .filter(|call| call.namespace == ctx.namespace())
            .ok_or(Error::NotFound)?;
        if call.dispatch_group_id != before.id
            || call.stage != BudgetStage::TaskExecution
            || call.actual_input_digest != intent.request_digest
            || !matches!(
                call.state,
                BudgetCallState::Dispatched
                    | BudgetCallState::Uncertain
                    | BudgetCallState::Finalized
            )
        {
            return Err(Error::Conflict(
                "execution receipt lacks the matching dispatched budget call".into(),
            ));
        }
        let output = MirrorExecutionOutputArtifact {
            schema_version: "rsia.execution_output.v1".into(),
            id: intent.output_artifact_id,
            ticket_id: before.id.clone(),
            target_id: intent.target_id.clone(),
            side: intent.side,
            output_digest: hash(intent.output_utf8.as_bytes()),
            output_utf8: intent.output_utf8,
        };
        let mut receipt = MirrorExecutionReceiptV2 {
            schema_version: "rsia.execution_receipt.v2".into(),
            receipt_id: intent.receipt_id.clone(),
            ticket_id: before.id.clone(),
            target_id: intent.target_id.clone(),
            side: intent.side,
            request_digest: intent.request_digest,
            bundle_digest: bundle.clone(),
            environment_digest: before.environment_digest.clone(),
            output_digest: output.output_digest.clone(),
            output_artifact_id: output.id.clone(),
            budget_call_id: intent.budget_call_id,
            executor_actor: ctx.actor().into(),
            latency_micros: intent.latency_micros,
            issued_at_unix_seconds: at,
        };
        let receipt_physical = evaluation_storage_id(EXECUTION_RECEIPT_KIND, &receipt.receipt_id)?;
        if let Some(existing) = record_optional::<MirrorExecutionReceiptV2>(
            &snapshot,
            ctx.namespace(),
            EXECUTION_RECEIPT_KIND,
            &receipt.receipt_id,
        )? {
            // No caller timestamp is reconstructed on a v2 replay. Compare all
            // request material to the one actually persisted original event.
            receipt.issued_at_unix_seconds = existing.issued_at_unix_seconds;
            if typed_fingerprint(&receipt)? != typed_fingerprint(&existing)? {
                return Err(Error::Conflict(
                    "execution receipt id reused with different evidence".into(),
                ));
            }
            let old: MirrorExecutionOutputArtifact = evaluation_record(
                &snapshot,
                ctx.namespace(),
                OUTPUT_KIND,
                &existing.output_artifact_id,
            )?;
            if typed_fingerprint(&old)? != typed_fingerprint(&output)? {
                return Err(Error::Conflict(
                    "stored execution output differs from idempotent receipt".into(),
                ));
            }
            return Self::base(
                database,
                ctx,
                before.clone(),
                before,
                AppendScope::Execution { receipt_physical },
                serde_json::to_value(existing).map_err(crate::internal),
            );
        }
        if before.lifecycle == MirrorTicketLifecycle::EarlyStopped {
            // A new late member changes ticket_digest. Until AG135 supplies an
            // actual versioned successor, no same-ID Formal rewrite is legal.
            require_new_formal(&snapshot, ctx.namespace(), &before.id)?;
        }
        owned_validate_started_slot(
            before.target(&intent.target_id)?,
            intent.side,
            &receipt.budget_call_id,
            &receipt.request_digest,
        )?;
        let mut after = before.clone();
        if owned_is_stopping(after.lifecycle) {
            if after.lifecycle != MirrorTicketLifecycle::Invalid {
                after.late_execution_receipt_ids.push(intent.receipt_id);
            }
        } else {
            owned_set_running_receipt(
                after.target_mut(&intent.target_id)?,
                intent.side,
                &receipt.receipt_id,
            )?;
            after.lifecycle = MirrorTicketLifecycle::Running;
        }
        after.validate()?;
        let accepted =
            before.candidate_dependency_binding.is_some() && !owned_is_stopping(before.lifecycle);
        let response = serde_json::to_value(&receipt).map_err(crate::internal);
        let mut plan = Self::base(
            database,
            ctx,
            before,
            after,
            AppendScope::Execution {
                receipt_physical: receipt_physical.clone(),
            },
            response,
        )?;
        if let Some(old) = record_optional::<MirrorExecutionOutputArtifact>(
            &snapshot,
            ctx.namespace(),
            OUTPUT_KIND,
            &output.id,
        )? {
            if typed_fingerprint(&old)? != typed_fingerprint(&output)? {
                return Err(Error::Conflict(
                    "execution output id reused with different bytes or binding".into(),
                ));
            }
        } else {
            plan.writes.push(object_write(
                database,
                ctx,
                OUTPUT_KIND,
                &output.id,
                &output,
                false,
            )?);
        }
        plan.writes.push(object_write(
            database,
            ctx,
            EXECUTION_RECEIPT_KIND,
            &receipt.receipt_id,
            &receipt,
            false,
        )?);
        if accepted {
            for dst in [
                e05_artifact_edge(TICKET_KIND, &plan.before_ticket.id)?,
                e05_artifact_edge(OUTPUT_KIND, &output.id)?,
            ] {
                plan.push_edge(&snapshot, &receipt_physical, dst)?;
            }
            plan.push_edge(
                &snapshot,
                &evaluation_storage_id(TICKET_KIND, &plan.before_ticket.id)?,
                e05_artifact_edge(EXECUTION_RECEIPT_KIND, &receipt.receipt_id)?,
            )?;
        }
        if plan.before_ticket.lifecycle == MirrorTicketLifecycle::Invalid {
            plan.audit = Some((
                "late_execution_receipt_after_formal",
                receipt.receipt_id.clone(),
            ));
        } else {
            plan.push_ticket(database, ctx)?;
            plan.push_early_formal(database, ctx)?;
        }
        Ok(plan)
    }

    fn grade(database: &RawDatabase, ctx: &Context, draft: E05GradeDraft) -> Result<Self> {
        Self::grade_to_destination_r1(database, ctx, draft, GradeFormalDestinationR1::Legacy)
    }
    fn grade_to_destination_r1(
        database: &RawDatabase,
        ctx: &Context,
        draft: E05GradeDraft,
        destination: GradeFormalDestinationR1,
    ) -> Result<Self> {
        if matches!(
            destination,
            GradeFormalDestinationR1::VersionedCompleteRequestInvalid
        ) {
            require(
                draft.invalidated
                    && !draft.replay
                    && !draft.audit_only
                    && !draft.audit_error
                    && draft.receipt.is_none()
                    && draft.before.lifecycle == MirrorTicketLifecycle::ReadyForFinal
                    && draft.after.lifecycle == MirrorTicketLifecycle::Invalid,
            )?;
        }
        let (snapshot, _) = snapshot(database)?;
        draft.check_grade_inputs(&snapshot, ctx)?;
        if let Some(ack) = &draft.prepared_grade {
            ack.verify(database, ctx, &draft)?;
        }
        let receipt_prepared = draft.prepared_grade.is_some();
        let receipt_physical = draft
            .receipt
            .as_ref()
            .map(|r| evaluation_storage_id(GRADER_RECEIPT_KIND, &r.receipt_id))
            .transpose()?;
        let mut plan = Self::base(
            database,
            ctx,
            draft.before,
            draft.after,
            AppendScope::Grade {
                receipt_physical: receipt_physical.clone(),
            },
            draft.output,
        )?;
        if draft.replay {
            return Ok(plan);
        }
        if let Some(receipt) = draft.receipt {
            if !receipt_prepared {
                plan.writes.push(object_write(
                    database,
                    ctx,
                    GRADER_RECEIPT_KIND,
                    &receipt.receipt_id,
                    &receipt,
                    false,
                )?);
            }
            if draft.accepted {
                for dst in [
                    e05_artifact_edge(
                        EXECUTION_RECEIPT_KIND,
                        &receipt.candidate_execution_receipt_id,
                    )?,
                    e05_artifact_edge(
                        EXECUTION_RECEIPT_KIND,
                        &receipt.baseline_execution_receipt_id,
                    )?,
                ] {
                    plan.push_edge(
                        &snapshot,
                        receipt_physical.as_deref().ok_or(Error::Internal)?,
                        dst,
                    )?;
                }
                plan.push_edge(
                    &snapshot,
                    &evaluation_storage_id(TICKET_KIND, &plan.before_ticket.id)?,
                    e05_artifact_edge(GRADER_RECEIPT_KIND, &receipt.receipt_id)?,
                )?;
            }
            if draft.audit_only {
                plan.audit = Some(("late_grader_receipt_after_final_report", receipt.receipt_id));
                return Ok(plan);
            }
        }
        if draft.audit_error {
            plan.audit = Some(("late_grader_error_after_final_report", draft.receipt_id));
            return Ok(plan);
        }
        plan.push_ticket(database, ctx)?;
        if draft.invalidated {
            let formal = MirrorFormalEvaluationV2::Invalid {
                evidence_scope: plan.after_ticket.evidence_scope,
                ticket_id: plan.after_ticket.id.clone(),
                ticket_digest: typed_fingerprint(&plan.after_ticket)?,
                reasons: plan.after_ticket.invalid_reasons.clone(),
            };
            if matches!(destination, GradeFormalDestinationR1::Legacy) {
                plan.writes.push(object_write_as(
                    database,
                    ctx,
                    FORMAL_KIND,
                    &plan.after_ticket.id,
                    ctx.actor(),
                    &formal,
                    false,
                )?);
            }
        } else {
            plan.push_early_formal(database, ctx)?;
        }
        Ok(plan)
    }

    fn push_edge(
        &mut self,
        snapshot: &OccurrenceSnapshot,
        source: &str,
        dst: (String, String),
    ) -> Result<()> {
        if e05_outgoing(snapshot, &self.namespace, "artifact", source)?.contains(&dst) {
            return Ok(());
        }
        self.writes.push(IssueWrite::Edge(OwnedRow {
            cells: BTreeMap::from([
                ("namespace".into(), OwnedCell::Text(self.namespace.clone())),
                ("src_kind".into(), OwnedCell::Text("artifact".into())),
                ("src_id".into(), OwnedCell::Text(source.into())),
                ("dst_kind".into(), OwnedCell::Text(dst.0)),
                ("dst_id".into(), OwnedCell::Text(dst.1)),
            ]),
        }));
        Ok(())
    }
    fn push_ticket(&mut self, database: &RawDatabase, ctx: &Context) -> Result<()> {
        let mut write = object_write_as(
            database,
            ctx,
            TICKET_KIND,
            &self.after_ticket.id,
            &self.after_ticket.evaluator_actor,
            &self.after_ticket,
            true,
        )?;
        if let Some(source) = &self.first_stop_r44 {
            reconstruct_first_stop_r44(&self.after_ticket, source)?;
            let IssueWrite::Object { after, .. } = &mut write else {
                return Err(Error::ClockRegressed);
            };
            let value = evo_core::formal_version::NativeTicketEnvelopeV2 {
                schema_version: evo_core::formal_version::NATIVE_TICKET_ENVELOPE.into(),
                id: evaluation_storage_id(TICKET_KIND, &self.after_ticket.id)?,
                record_kind: TICKET_KIND.into(),
                payload: &self.after_ticket,
                native_first_stop: Some(source.clone()),
            };
            let json = serde_json::to_value(&value).map_err(crate::internal)?;
            evo_core::formal_version::native_ticket_payload(&json, ctx.namespace(), &value.id)
                .map_err(r24_fault)?;
            after.cells.insert(
                "body".into(),
                OwnedCell::Text(serde_json::to_string(&value).map_err(crate::internal)?),
            );
            if self.first_stop_new_r44 {
                require(
                    ctx.role() == Role::Evaluator
                        && ctx.actor() == self.after_ticket.evaluator_actor
                        && self.audit.is_none(),
                )?;
            }
        }
        self.writes.push(write);
        Ok(())
    }
    fn push_early_formal(&mut self, database: &RawDatabase, ctx: &Context) -> Result<()> {
        // Native v2 tickets materialize a genuine versioned member only AFTER
        // this full prefix actually ACKs; never write a legacy alias.
        if self.first_stop_r44.is_some() {
            return Ok(());
        }
        if self.after_ticket.lifecycle == MirrorTicketLifecycle::EarlyStopped {
            let (current, _) = snapshot(database)?;
            require_new_formal(&current, ctx.namespace(), &self.after_ticket.id)?;
            let certificate = self
                .after_ticket
                .pending_certificate
                .clone()
                .ok_or(Error::ClockRegressed)?;
            // The immutable certificate is re-used, never re-scored for late evidence.
            if self.before_ticket.lifecycle == MirrorTicketLifecycle::EarlyStopped {
                require(
                    typed_fingerprint(&self.before_ticket.pending_certificate)?
                        == typed_fingerprint(&self.after_ticket.pending_certificate)?
                        && self.before_ticket.stop_member_terminal_digest
                            == self.after_ticket.stop_member_terminal_digest,
                )?;
            }
            let formal = MirrorFormalEvaluationV2::EarlyStopped {
                evidence_scope: self.after_ticket.evidence_scope,
                ticket_id: self.after_ticket.id.clone(),
                ticket_digest: typed_fingerprint(&self.after_ticket)?,
                certificate,
            };
            self.writes.push(object_write_as(
                database,
                ctx,
                FORMAL_KIND,
                &self.after_ticket.id,
                &self.after_ticket.evaluator_actor,
                &formal,
                false,
            )?);
        }
        Ok(())
    }
    fn acknowledge(&mut self, step: usize) -> Result<()> {
        require(self.next == step)?;
        self.writes
            .get(step)
            .ok_or(Error::ClockRegressed)?
            .acknowledge(&mut self.expected)?;
        self.next = self.next.checked_add(1).ok_or(Error::ClockRegressed)?;
        Ok(())
    }
    fn phase(&self) -> Result<E05PhaseEvidence> {
        let (expected, _) = snapshot(&self.expected)?;
        let physical = evaluation_storage_id(TICKET_KIND, &self.before_ticket.id)?;
        let object = expected.object(&self.namespace, "artifact", &physical)?;
        let current: MirrorEvaluationTicketV2 = decode(envelope(
            object,
            "rsia.typed_artifact_envelope.v1",
            TICKET_KIND,
        )?)?;
        require(
            typed_fingerprint(&current)? == typed_fingerprint(&self.before_ticket)?
                || typed_fingerprint(&current)? == typed_fingerprint(&self.after_ticket)?,
        )?;
        let new_member = self
            .scope
            .new_member()
            .map(|id| {
                Ok::<_, Error>((
                    id.to_owned(),
                    e05_outgoing(&expected, &self.namespace, "artifact", id)?,
                ))
            })
            .transpose()?;
        let resource_member = self
            .scope
            .resource_member()
            .map(|id| {
                Ok::<_, Error>((
                    id.to_owned(),
                    e05_outgoing(&expected, &self.namespace, "artifact", id)?,
                ))
            })
            .transpose()?;
        Ok(E05PhaseEvidence {
            namespace: self.namespace.clone(),
            ticket_physical: physical.clone(),
            ticket_fingerprint: typed_fingerprint(&current)?,
            ticket_edges: e05_outgoing(&expected, &self.namespace, "artifact", &physical)?,
            new_member,
            resource_member,
        })
    }
    fn qualify(
        &self,
        database: &RawDatabase,
        inputs: &PreparePreimages,
    ) -> Result<OccurrenceFloor> {
        self.qualify_with_pending_r24(database, inputs, None)
    }
    fn qualify_with_pending_r24(
        &self,
        database: &RawDatabase,
        inputs: &PreparePreimages,
        pending: Option<CompletePendingR24>,
    ) -> Result<OccurrenceFloor> {
        same_database(database, &self.expected)?;
        let (mut current, mut floor) = snapshot(database)?;
        match &self.scope {
            AppendScope::Execution { .. } => {
                require(self.actor == self.before_ticket.executor_actor)?
            }
            AppendScope::Grade { .. }
            | AppendScope::Complete { .. }
            | AppendScope::EarlyR44 { .. }
            | AppendScope::CompleteInvalidR1 { .. }
            | AppendScope::Invalidate => require(self.actor == self.before_ticket.evaluator_actor)?,
        }
        let control = evaluation_control(
            &current,
            &self.namespace,
            &self.before_ticket.registration_id,
        )?;
        let holdout = evaluation_holdout(
            &current,
            &self.namespace,
            &self.before_ticket.holdout_id,
            &control,
        )?;
        producer_witnesses(&current, &self.namespace, &control, &holdout)?;
        require(
            typed_fingerprint(&control)? == self.control_digest
                && typed_fingerprint(&holdout)? == self.holdout_digest,
        )?;
        if let Some(binding) = &self.before_ticket.candidate_dependency_binding {
            e05_candidate_binding(
                &current,
                &self.namespace,
                &self.before_ticket,
                &control,
                &holdout,
                binding,
            )?;
        }
        current.e05_phase = Some(self.phase()?);
        current.complete_pending_r24 = pending;
        let mut ticket_scan = TicketRelationScan {
            snapshot: &current,
            checked_relations: BTreeSet::new(),
            successful_tickets: BTreeMap::new(),
        };
        for object in current.objects.values() {
            let own_prior = self.next < self.writes.len()
                && object.namespace == self.namespace
                && object.kind == "artifact"
                && object.id == evaluation_storage_id(FORMAL_KIND, &self.before_ticket.id)?
                && self
                    .prior_early_formal
                    .as_ref()
                    .is_some_and(|fp| object.value_body_digest_r26().ok().as_ref() == Some(fp));
            if own_prior {
                // Actual old report is a pending member of this exact late
                // append, not a current-verified report for the changed ticket.
                let formal: MirrorFormalEvaluationV2 = decode(envelope(
                    object,
                    "rsia.typed_artifact_envelope.v1",
                    FORMAL_KIND,
                )?)?;
                if let MirrorFormalEvaluationV2::EarlyStopped { certificate, .. } = formal {
                    observe_millis_field(
                        &mut floor,
                        &serde_json::to_value(certificate).map_err(crate::internal)?,
                        "stopped_at_unix_ms",
                    )?;
                } else {
                    return Err(Error::ClockRegressed);
                }
            } else {
                qualify_object_with_ticket_scan(
                    object,
                    &current,
                    inputs,
                    &mut floor,
                    Some(&mut ticket_scan),
                )?;
            }
        }
        collect_caches(database, &current, inputs, &mut floor)?;
        Ok(floor)
    }
    async fn admit(&self, session: &mut Session) -> Result<()> {
        session.clock.check()?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        require(
            SqliteTransactionManager::get_transaction_depth(&**tx) == 1 && tx.is_in_transaction(),
        )?;
        let raw = load_sqlx_database(tx)
            .await
            .map_err(|_| session.clock.fail())?;
        let floor = self
            .qualify(&raw, &session.prepare_inputs)
            .map_err(|_| session.clock.fail())?;
        let stamp = session.clock.seconds(&floor)?;
        session.clock.take_seconds(stamp)?;
        Ok(())
    }
    async fn admit_stable(&self, session: &mut Session) -> Result<()> {
        require(self.next == self.writes.len())?;
        session.clock.check()?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        require(
            SqliteTransactionManager::get_transaction_depth(&**tx) == 1 && tx.is_in_transaction(),
        )?;
        let raw = load_sqlx_database(tx)
            .await
            .map_err(|_| session.clock.fail())?;
        same_database(&raw, &self.expected).map_err(|_| session.clock.fail())?;
        let floor = collect_raw(&raw, &session.prepare_inputs, SchemaRead::Stable)
            .map_err(|_| session.clock.fail())?;
        let stamp = session.clock.seconds(&floor)?;
        session.clock.take_seconds(stamp)?;
        Ok(())
    }
    async fn audit(&mut self, session: &mut Session, ctx: &Context) -> Result<()> {
        let Some((operation, entity)) = &self.audit else {
            return Ok(());
        };
        self.admit_stable(session).await?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        let previous = self
            .expected
            .tables
            .get("audit")
            .ok_or(Error::ClockRegressed)?
            .iter()
            .filter(|row| native_text(row, "namespace").ok().as_deref() == Some(ctx.namespace()))
            .max_by_key(|row| native_integer(row, "seq").unwrap_or(i64::MIN))
            .map(|row| native_text(row, "digest"))
            .transpose()?
            .unwrap_or_default();
        let floor = collect_floor(tx, &session.prepare_inputs, SchemaRead::Stable).await?;
        let stamp = session.clock.seconds(&floor)?;
        let at = session.clock.take_seconds(stamp)?;
        let payload=serde_json::json!({"actor":ctx.actor(),"role":ctx.role(),"operation":operation,"entity":entity,"timestamp":at}).to_string();
        let digest = hash(format!("{previous}\n{payload}").as_bytes());
        let seq:i64=sqlx::query_scalar("INSERT INTO audit(namespace,payload,previous_hash,digest) VALUES(?,?,?,?) RETURNING seq")
            .bind(ctx.namespace()).bind(&payload).bind(&previous).bind(&digest)
            .fetch_one(&mut **tx).await.map_err(crate::internal)?;
        require(
            seq > 0
                && self
                    .expected
                    .tables
                    .get("audit")
                    .ok_or(Error::ClockRegressed)?
                    .iter()
                    .all(|row| native_integer(row, "seq").is_ok_and(|old| old < seq)),
        )?;
        self.expected
            .tables
            .get_mut("audit")
            .ok_or(Error::ClockRegressed)?
            .push(OwnedRow {
                cells: BTreeMap::from([
                    ("seq".into(), OwnedCell::Integer(seq)),
                    ("namespace".into(), OwnedCell::Text(ctx.namespace().into())),
                    ("payload".into(), OwnedCell::Text(payload)),
                    ("previous_hash".into(), OwnedCell::Text(previous)),
                    ("digest".into(), OwnedCell::Text(digest)),
                ]),
            });
        Ok(())
    }
}

fn producer_witnesses(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    control: &MirrorRegisteredEvaluationControl,
    holdout: &MirrorProtectedHoldoutRecord,
) -> Result<()> {
    for (kind, id) in [
        ("registered_evaluation_control_v41", control.id.as_str()),
        ("protected_holdout_v41", holdout.id.as_str()),
    ] {
        snapshot.witness(
            snapshot.object(namespace, "artifact", &evaluation_storage_id(kind, id)?)?,
            "evaluator",
        )?;
    }
    Ok(())
}
fn record_optional<T: DeserializeOwned>(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    kind: &str,
    id: &str,
) -> Result<Option<T>> {
    snapshot
        .objects
        .get(&(
            namespace.into(),
            "artifact".into(),
            evaluation_storage_id(kind, id)?,
        ))
        .map(|object| decode(envelope(object, "rsia.typed_artifact_envelope.v1", kind)?))
        .transpose()
}
fn object_write_as<T: Serialize>(
    database: &RawDatabase,
    ctx: &Context,
    kind: &str,
    id: &str,
    owner: &str,
    body: &T,
    replace: bool,
) -> Result<IssueWrite> {
    let mut write = object_write(database, ctx, kind, id, body, replace)?;
    if let IssueWrite::Object { after, .. } = &mut write {
        identifier(owner)?;
        after
            .cells
            .insert("owner".into(), OwnedCell::Text(owner.into()));
    }
    Ok(write)
}

struct E05GradeDraft {
    before: MirrorEvaluationTicketV2,
    after: MirrorEvaluationTicketV2,
    control: MirrorRegisteredEvaluationControl,
    control_digest: String,
    holdout_digest: String,
    receipt: Option<MirrorIndependentGraderReceiptV2>,
    receipt_id: String,
    input_fingerprints: Vec<(String, String)>,
    at: i64,
    accepted: bool,
    replay: bool,
    audit_only: bool,
    audit_error: bool,
    invalidated: bool,
    invalid_reason: Option<&'static str>,
    feedback: bool,
    stop: Option<EarlyStopDecision>,
    prepared_grade: Option<native_formal_consumers_r49::GradeCaptureAckR49>,
    output: Result<Value>,
}
impl E05GradeDraft {
    fn derive(database: &RawDatabase, ctx: &Context, intent: GradeIntent, at: i64) -> Result<Self> {
        let (snapshot, _) = snapshot(database)?;
        if record_optional::<Value>(&snapshot, ctx.namespace(), TICKET_KIND, &intent.ticket_id)?
            .is_none()
        {
            return Err(Error::NotFound);
        }
        let before = evaluation_ticket(&snapshot, ctx.namespace(), &intent.ticket_id)?;
        let control = evaluation_control(&snapshot, ctx.namespace(), &before.registration_id)?;
        let holdout = evaluation_holdout(&snapshot, ctx.namespace(), &before.holdout_id, &control)?;
        producer_witnesses(&snapshot, ctx.namespace(), &control, &holdout)?;
        if before.evaluator_actor != ctx.actor()
            || ctx.actor() == before.proposer_actor
            || ctx.actor() == before.approver_actor
        {
            return Err(Error::Forbidden);
        }
        let accepted =
            before.candidate_dependency_binding.is_some() && !owned_is_stopping(before.lifecycle);
        let preserve = before.candidate_dependency_binding.is_some()
            && matches!(
                before.lifecycle,
                MirrorTicketLifecycle::Draining | MirrorTicketLifecycle::EarlyStopped
            )
            && before.pending_certificate.is_some();
        let audit_only = owned_late_grading_after_final_report(&before, &intent.target_id)?;
        let candidate: MirrorExecutionReceiptV2 = evaluation_record(
            &snapshot,
            ctx.namespace(),
            EXECUTION_RECEIPT_KIND,
            &intent.candidate_execution_receipt_id,
        )?;
        let baseline: MirrorExecutionReceiptV2 = evaluation_record(
            &snapshot,
            ctx.namespace(),
            EXECUTION_RECEIPT_KIND,
            &intent.baseline_execution_receipt_id,
        )?;
        owned_validate_execution_pair(&before, &intent.target_id, &candidate, &baseline)?;
        let candidate_output: MirrorExecutionOutputArtifact = evaluation_record(
            &snapshot,
            ctx.namespace(),
            OUTPUT_KIND,
            &candidate.output_artifact_id,
        )?;
        let baseline_output: MirrorExecutionOutputArtifact = evaluation_record(
            &snapshot,
            ctx.namespace(),
            OUTPUT_KIND,
            &baseline.output_artifact_id,
        )?;
        for (receipt, output) in [
            (&candidate, &candidate_output),
            (&baseline, &baseline_output),
        ] {
            owned_validate_output_artifact(receipt, output)?;
        }
        let mut input_fingerprints = Vec::new();
        for (kind, id) in [
            (EXECUTION_RECEIPT_KIND, candidate.receipt_id.as_str()),
            (EXECUTION_RECEIPT_KIND, baseline.receipt_id.as_str()),
            (OUTPUT_KIND, candidate_output.id.as_str()),
            (OUTPUT_KIND, baseline_output.id.as_str()),
        ] {
            let physical = evaluation_storage_id(kind, id)?;
            input_fingerprints.push((
                physical.clone(),
                canonical_fingerprint(
                    &snapshot
                        .object(ctx.namespace(), "artifact", &physical)?
                        .body,
                )?,
            ));
        }
        let oracle = holdout
            .oracle_entries
            .iter()
            .find(|entry| entry.target_id == intent.target_id)
            .ok_or_else(|| Error::Invalid("frozen oracle entry missing".into()))?;
        let target_kind = before.target(&intent.target_id)?.kind.clone();
        let grade = (|| {
            let candidate_score = owned_grade_fixed_output(
                &control.fixed_grader,
                &candidate_output.output_utf8,
                &oracle.expected_answer_json,
            )
            .map_err(|error| (error, "candidate_output_schema_invalid"))?;
            let baseline_score = owned_grade_fixed_output(
                &control.fixed_grader,
                &baseline_output.output_utf8,
                &oracle.expected_answer_json,
            )
            .map_err(|error| (error, "baseline_output_schema_invalid"))?;
            let anchor = match target_kind {
                MirrorEvaluationTargetKind::Rotating { .. } => None,
                MirrorEvaluationTargetKind::Anchor { .. } if baseline_score != 1_000_000 => {
                    return Err((
                        Error::Invalid(
                            "baseline failed a frozen critical anchor; comparison is invalid"
                                .into(),
                        ),
                        "baseline_anchor_invalid",
                    ));
                }
                MirrorEvaluationTargetKind::Anchor { .. } if candidate_score == 1_000_000 => {
                    Some(AnchorResult::Passed)
                }
                MirrorEvaluationTargetKind::Anchor { .. } => Some(AnchorResult::CriticalRegression),
            };
            Ok((candidate_score, baseline_score, anchor))
        })();
        let mut draft = Self {
            before: before.clone(),
            after: before.clone(),
            control: control.clone(),
            control_digest: typed_fingerprint(&control)?,
            holdout_digest: typed_fingerprint(&holdout)?,
            receipt: None,
            receipt_id: intent.receipt_id.clone(),
            input_fingerprints,
            at,
            accepted,
            replay: false,
            audit_only,
            audit_error: false,
            invalidated: false,
            invalid_reason: None,
            feedback: false,
            stop: None,
            prepared_grade: None,
            output: Err(Error::Internal),
        };
        let (candidate_score, baseline_score, anchor) = match grade {
            Ok(scores) => scores,
            Err((error, reason)) => {
                if audit_only {
                    draft.audit_error = true;
                } else if preserve {
                    return Err(error);
                } else {
                    draft.invalidated = true;
                    draft.invalid_reason = Some(reason);
                    draft.after.lifecycle = MirrorTicketLifecycle::Invalid;
                    if !draft.after.invalid_reasons.iter().any(|old| old == reason) {
                        draft.after.invalid_reasons.push(reason.into());
                    }
                }
                draft.output = Err(error);
                return Ok(draft);
            }
        };
        let mut receipt = MirrorIndependentGraderReceiptV2 {
            schema_version: "rsia.independent_grader_receipt.v2".into(),
            receipt_id: intent.receipt_id.clone(),
            ticket_id: before.id.clone(),
            target_id: intent.target_id.clone(),
            candidate_execution_receipt_id: candidate.receipt_id.clone(),
            baseline_execution_receipt_id: baseline.receipt_id.clone(),
            candidate_output_digest: candidate.output_digest.clone(),
            baseline_output_digest: baseline.output_digest.clone(),
            oracle_digest: control.oracle_digest.clone(),
            grader_digest: control.grader_digest.clone(),
            scored_output_pair_digest: typed_fingerprint(&(
                &candidate.output_digest,
                &baseline.output_digest,
                &control.oracle_digest,
                &control.grader_digest,
                candidate_score,
                baseline_score,
                anchor,
            ))?,
            candidate_score_micros: candidate_score,
            baseline_score_micros: baseline_score,
            anchor_result: anchor,
            evaluator_actor: ctx.actor().into(),
            issued_at_unix_seconds: at,
        };
        if let Some(existing) = record_optional::<MirrorIndependentGraderReceiptV2>(
            &snapshot,
            ctx.namespace(),
            GRADER_RECEIPT_KIND,
            &receipt.receipt_id,
        )? {
            receipt.issued_at_unix_seconds = existing.issued_at_unix_seconds;
            if typed_fingerprint(&receipt)? != typed_fingerprint(&existing)? {
                return Err(Error::Conflict(
                    "grader receipt id reused with different evidence".into(),
                ));
            }
            draft.receipt = Some(existing.clone());
            draft.output = serde_json::to_value(existing).map_err(crate::internal);
            draft.replay = true;
            return Ok(draft);
        }
        if before.lifecycle == MirrorTicketLifecycle::EarlyStopped && !audit_only {
            require_new_formal(&snapshot, ctx.namespace(), &before.id)?;
        }
        draft.feedback = true;
        draft.output = serde_json::to_value(&receipt).map_err(crate::internal);
        draft.receipt = Some(receipt);
        if audit_only {
            return Ok(draft);
        }
        if owned_is_stopping(before.lifecycle) {
            draft.after.late_grader_receipt_ids.push(intent.receipt_id);
        } else {
            owned_validate_target_receipt_pair(
                before.target(&intent.target_id)?,
                &candidate.receipt_id,
                &baseline.receipt_id,
            )?;
            if matches!(
                before.target(&intent.target_id)?.state,
                MirrorTargetState::Completed { .. }
            ) {
                return Err(Error::Conflict(
                    "completed target score is immutable".into(),
                ));
            }
            if control.evidence_scope == MirrorEvaluationEvidenceScope::RegisteredExternalHost {
                // Only the real same-owner receipt/capture ACK can complete this target.
                draft.after.validate()?;
                return Ok(draft);
            }
            draft.complete_target_from_receipt()?;
        }
        draft.after.validate()?;
        Ok(draft)
    }

    fn check_grade_inputs(&self, snapshot: &OccurrenceSnapshot, ctx: &Context) -> Result<()> {
        let current = evaluation_ticket(snapshot, ctx.namespace(), &self.before.id)?;
        require(typed_fingerprint(&current)? == typed_fingerprint(&self.before)?)?;
        let control = evaluation_control(snapshot, ctx.namespace(), &current.registration_id)?;
        let holdout = evaluation_holdout(snapshot, ctx.namespace(), &current.holdout_id, &control)?;
        producer_witnesses(snapshot, ctx.namespace(), &control, &holdout)?;
        require(
            typed_fingerprint(&control)? == self.control_digest
                && typed_fingerprint(&holdout)? == self.holdout_digest,
        )?;
        for (id, fp) in &self.input_fingerprints {
            let object = snapshot.object(ctx.namespace(), "artifact", id)?;
            require(object.value_body_digest_r26()? == *fp)?;
        }
        Ok(())
    }
    fn complete_target_from_receipt(&mut self) -> Result<()> {
        let receipt = self.receipt.as_ref().ok_or(Error::Internal)?.clone();
        let target_id = receipt.target_id.clone();
        let candidate_score = receipt.candidate_score_micros;
        let baseline_score = receipt.baseline_score_micros;
        let anchor = receipt.anchor_result;
        let sequence = self.after.next_sequence;
        self.after.next_sequence = sequence
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("ticket sequence overflow".into()))?;
        self.after.target_mut(&target_id)?.state = MirrorTargetState::Completed {
            candidate_score_micros: candidate_score,
            baseline_score_micros: baseline_score,
            anchor_result: anchor,
            candidate_receipt_id: receipt.candidate_execution_receipt_id.clone(),
            baseline_receipt_id: receipt.baseline_execution_receipt_id.clone(),
            grader_receipt_id: receipt.receipt_id.clone(),
            completion_sequence: sequence,
        };
        if anchor == Some(AnchorResult::CriticalRegression) {
            self.stop = Some(EarlyStopDecision::CriticalRegression);
        } else if let Some(plan) = &self.control.early_stop_plan {
            let (decision, _) = owned_rebuild_stream(&self.after, plan, &self.control.alpha_plan)?;
            if decision.is_early_stop() {
                self.stop = Some(decision);
            } else if decision == EarlyStopDecision::FinalFixedSample
                && owned_anchors_passed(&self.after)
            {
                self.after.lifecycle = MirrorTicketLifecycle::ReadyForFinal;
            }
        } else if owned_rotating_complete(&self.after) && owned_anchors_passed(&self.after) {
            self.after.lifecycle = MirrorTicketLifecycle::ReadyForFinal;
        }
        self.after.validate()
    }
    async fn prepare_registered_grade(
        &mut self,
        session: &mut Session,
        ctx: &Context,
        asof: &NativeAsOfPermit,
        gate: Arc<AtomicU8>,
    ) -> Result<()> {
        if self.control.evidence_scope != MirrorEvaluationEvidenceScope::RegisteredExternalHost {
            return Ok(());
        }
        let Some(receipt) = self.receipt.as_ref() else {
            return Ok(());
        };
        let completed_replay = self.replay
            && matches!(
                &self.before.target(&receipt.target_id)?.state,
                MirrorTargetState::Completed { grader_receipt_id, .. }
                    if grader_receipt_id == &receipt.receipt_id
            );
        if !completed_replay
            && (self.replay || self.audit_only || owned_is_stopping(self.before.lifecycle))
        {
            return Ok(());
        }
        let ack = native_formal_consumers_r49::prepare_grade_inside_owner_r49(
            session,
            ctx,
            self,
            asof,
            Arc::clone(&gate),
        )
        .await?;
        check_cancel(&gate)?;
        ack.confirm_owner(session, asof)?;
        self.prepared_grade = Some(ack);
        if !self.replay {
            self.complete_target_from_receipt()?;
        }
        Ok(())
    }
    async fn prepare_budget(
        &mut self,
        session: &mut Session,
        ctx: &Context,
        gate: &AtomicU8,
    ) -> Result<()> {
        check_cancel(gate)?;
        if self.invalidated || self.stop.is_some() {
            let raw = load_sqlx_database(&mut **session.tx.as_mut().ok_or(Error::ClockRegressed)?)
                .await?;
            let (current, _) = snapshot(&raw)?;
            require_new_formal(&current, ctx.namespace(), &self.before.id)?;
        }
        if self.feedback {
            owned_mark_feedback_used_if_needed(session, ctx, &self.before, self.at).await?;
        }
        check_cancel(gate)?;
        if self.invalidated {
            let reason = self.invalid_reason.ok_or(Error::Internal)?;
            let uncertain = owned_close_execution_slots(
                session,
                ctx,
                &self.control,
                &mut self.after,
                reason,
                self.at,
            )
            .await?;
            owned_settle_exposure_money(session, ctx, &self.after, uncertain).await?;
        } else if let Some(decision) = self.stop {
            owned_stop_ticket(
                session,
                ctx,
                &self.control,
                &mut self.after,
                decision,
                self.receipt.as_ref().ok_or(Error::Internal)?,
                self.at,
            )
            .await?;
        }
        check_cancel(gate)?;
        self.after.validate()
    }
}

async fn need_record<T: DeserializeOwned>(
    session: &mut Session,
    ctx: &Context,
    kind: &str,
    id: &str,
) -> Result<T> {
    let physical = evaluation_storage_id(kind, id)?;
    let value = session
        .get::<Value>(ctx, "artifact", &physical)
        .await?
        .ok_or(Error::NotFound)?;
    if kind == TICKET_KIND
        && value["schema_version"] == evo_core::formal_version::NATIVE_TICKET_ENVELOPE
    {
        let payload =
            evo_core::formal_version::native_ticket_payload(&value, ctx.namespace(), &physical)
                .map_err(r24_fault)?;
        return decode(payload);
    }
    exact(
        &value,
        &["schema_version", "id", "record_kind", "payload"],
        &[],
    )?;
    require(
        value["schema_version"] == "rsia.typed_artifact_envelope.v1"
            && value["id"] == physical
            && value["record_kind"] == kind,
    )?;
    decode(&value["payload"])
}
async fn put_record<T: Serialize>(
    session: &mut Session,
    ctx: &Context,
    kind: &str,
    id: &str,
    owner: &str,
    payload: &T,
) -> Result<()> {
    // Only private closed preparation helpers call this, with the exact Exposure
    // ledger kind. It grants no pending phase and retains normal Session guards.
    require(kind == LEDGER_KIND)?;
    let physical = evaluation_storage_id(kind, id)?;
    let value = serde_json::to_value(E05FingerprintEnvelope {
        schema_version: "rsia.typed_artifact_envelope.v1",
        id: &physical,
        record_kind: kind,
        payload,
    })
    .map_err(crate::internal)?;
    session.put(ctx, "artifact", &physical, owner, &value).await
}
async fn load_slice_id(session: &mut Session, ctx: &Context, id: &str) -> Result<String> {
    let holdout: MirrorProtectedHoldoutRecord = need_record(session, ctx, HOLDOUT_KIND, id).await?;
    Ok(holdout.manifest.slice_id)
}
fn owned_grade_fixed_output(
    grader: &MirrorFixedGraderSpec,
    output_utf8: &str,
    expected_answer_json: &str,
) -> Result<u32> {
    grader.validate()?;
    let output = owned_parse_strict_json(output_utf8)?;
    let Value::Object(mut object) = output.0 else {
        return Err(Error::Invalid(
            "execution output violates fixed grader schema".into(),
        ));
    };
    if object.len() != 1 || !object.contains_key("answer") {
        return Err(Error::Invalid(
            "execution output violates fixed grader schema".into(),
        ));
    }
    let answer = object.remove("answer").ok_or(Error::Internal)?;
    let expected = owned_parse_strict_json(expected_answer_json)?.0;
    match grader.method {
        MirrorFixedGraderMethod::ExactJsonAnswerV1 => {
            Ok(if answer == expected { 1_000_000 } else { 0 })
        }
    }
}
fn owned_parse_strict_json(value: &str) -> Result<StrictValue> {
    let mut deserializer = serde_json::Deserializer::from_str(value);
    let parsed = StrictValue::deserialize(&mut deserializer)
        .map_err(|_| Error::Invalid("JSON contains invalid or duplicate fields".into()))?;
    deserializer
        .end()
        .map_err(|_| Error::Invalid("JSON contains trailing content".into()))?;
    Ok(parsed)
}
fn owned_validate_started_slot(
    target: &MirrorEvaluationTarget,
    side: MirrorExecutionSide,
    call_id: &str,
    request_digest: &str,
) -> Result<()> {
    let (candidate, baseline) = match &target.state {
        MirrorTargetState::Running {
            candidate,
            baseline,
        }
        | MirrorTargetState::RunningUsageUnknown {
            candidate,
            baseline,
        }
        | MirrorTargetState::CancelledAfterDispatch {
            candidate,
            baseline,
        } => (candidate, baseline),
        _ => {
            return Err(Error::Conflict(
                "execution target has no started slot".into(),
            ));
        }
    };
    let slot = match side {
        MirrorExecutionSide::Candidate => candidate,
        MirrorExecutionSide::Baseline => baseline,
    }
    .as_ref()
    .ok_or_else(|| Error::Conflict("execution side has no started slot".into()))?;
    if slot.budget_call_id != call_id
        || slot.request_digest != request_digest
        || !matches!(slot.phase, MirrorExecutionSlotPhase::Dispatched { .. })
    {
        return Err(Error::Conflict(
            "execution receipt differs from the persisted dispatch slot".into(),
        ));
    }
    Ok(())
}

fn owned_set_running_receipt(
    target: &mut MirrorEvaluationTarget,
    side: MirrorExecutionSide,
    receipt_id: &str,
) -> Result<()> {
    let (mut candidate, mut baseline) = match &target.state {
        MirrorTargetState::Running {
            candidate,
            baseline,
        } => (candidate.clone(), baseline.clone()),
        _ => {
            return Err(Error::Conflict(
                "target cannot accept a new execution receipt".into(),
            ));
        }
    };
    let slot = match side {
        MirrorExecutionSide::Candidate => &mut candidate,
        MirrorExecutionSide::Baseline => &mut baseline,
    }
    .as_mut()
    .ok_or_else(|| Error::Conflict("execution side is not bound".into()))?;
    if slot.receipt_id.is_some()
        || !matches!(slot.phase, MirrorExecutionSlotPhase::Dispatched { .. })
    {
        return Err(Error::Conflict(
            "execution side cannot accept a receipt".into(),
        ));
    }
    slot.receipt_id = Some(receipt_id.into());
    target.state = MirrorTargetState::Running {
        candidate,
        baseline,
    };
    Ok(())
}

fn owned_validate_execution_pair(
    ticket: &MirrorEvaluationTicketV2,
    target_id: &str,
    candidate: &MirrorExecutionReceiptV2,
    baseline: &MirrorExecutionReceiptV2,
) -> Result<()> {
    if candidate.ticket_id != ticket.id
        || baseline.ticket_id != ticket.id
        || candidate.target_id != target_id
        || baseline.target_id != target_id
        || candidate.side != MirrorExecutionSide::Candidate
        || baseline.side != MirrorExecutionSide::Baseline
        || candidate.bundle_digest != ticket.candidate_bundle_digest
        || baseline.bundle_digest != ticket.baseline_bundle_digest
        || candidate.environment_digest != ticket.environment_digest
        || baseline.environment_digest != ticket.environment_digest
    {
        return Err(Error::Conflict(
            "execution receipts do not form the frozen candidate/baseline pair".into(),
        ));
    }
    Ok(())
}

fn owned_is_stopping(state: MirrorTicketLifecycle) -> bool {
    matches!(
        state,
        MirrorTicketLifecycle::StopRequested
            | MirrorTicketLifecycle::Draining
            | MirrorTicketLifecycle::EarlyStopped
            | MirrorTicketLifecycle::CompleteBatch
            | MirrorTicketLifecycle::Invalid
    )
}

fn owned_late_grading_after_final_report(
    ticket: &MirrorEvaluationTicketV2,
    target_id: &str,
) -> Result<bool> {
    Ok(match ticket.lifecycle {
        MirrorTicketLifecycle::CompleteBatch | MirrorTicketLifecycle::Invalid => true,
        MirrorTicketLifecycle::EarlyStopped => matches!(
            ticket.target(target_id)?.state,
            MirrorTargetState::Completed { .. }
        ),
        _ => false,
    })
}

fn owned_rotating_complete(ticket: &MirrorEvaluationTicketV2) -> bool {
    ticket.targets.iter().all(|target| {
        !matches!(target.kind, MirrorEvaluationTargetKind::Rotating { .. })
            || matches!(target.state, MirrorTargetState::Completed { .. })
    })
}

fn owned_anchors_passed(ticket: &MirrorEvaluationTicketV2) -> bool {
    ticket
        .targets
        .iter()
        .all(|target| match (&target.kind, &target.state) {
            (
                MirrorEvaluationTargetKind::Anchor { .. },
                MirrorTargetState::Completed { anchor_result, .. },
            ) => *anchor_result == Some(AnchorResult::Passed),
            (MirrorEvaluationTargetKind::Anchor { .. }, _) => false,
            _ => true,
        })
}

fn owned_rebuild_stream(
    ticket: &MirrorEvaluationTicketV2,
    plan: &EarlyStopPlan,
    alpha: &ResearchFamilyAlphaPlan,
) -> Result<(EarlyStopDecision, SequentialRejectOnly)> {
    let mut stream = SequentialRejectOnly::open(plan.clone(), alpha)?;
    let mut completed: Vec<_> = ticket
        .targets
        .iter()
        .filter_map(|target| match (&target.kind, &target.state) {
            (
                MirrorEvaluationTargetKind::Rotating { cluster_id },
                MirrorTargetState::Completed {
                    candidate_score_micros,
                    baseline_score_micros,
                    completion_sequence,
                    ..
                },
            ) => Some((
                *completion_sequence,
                CompletePairedUnit {
                    cluster_id: cluster_id.clone(),
                    baseline_score_micros: *baseline_score_micros,
                    candidate_score_micros: *candidate_score_micros,
                },
            )),
            _ => None,
        })
        .collect();
    completed.sort_by_key(|(sequence, _)| *sequence);
    for (_, unit) in completed {
        let decision = stream.record_complete_unit(unit)?;
        if decision.is_terminal() {
            return Ok((decision, stream));
        }
    }
    Ok((stream.decision()?, stream))
}

async fn owned_mark_exposure_dispatched_if_needed(
    session: &mut Session,
    ctx: &Context,
    ticket: &MirrorEvaluationTicketV2,
    at_unix_seconds: i64,
) -> Result<()> {
    let at_unix_ms = at_unix_seconds
        .checked_mul(1_000)
        .ok_or_else(|| Error::Invalid("dispatch timestamp conversion overflow".into()))?;
    let mut ledger: ExposureLedger =
        need_record(session, ctx, LEDGER_KIND, &ticket.research_family_id).await?;
    let state = ledger
        .entries
        .iter()
        .find(|entry| entry.ticket_id == ticket.id)
        .map(|entry| entry.state.clone())
        .ok_or(Error::NotFound)?;
    match state {
        ExposureState::Reserved => {
            ledger.mark_dispatched(&ticket.id, at_unix_ms)?;
            put_record(
                session,
                ctx,
                LEDGER_KIND,
                &ticket.research_family_id,
                ticket.evaluator_actor.as_str(),
                &ledger,
            )
            .await?;
        }
        ExposureState::Dispatched { .. } | ExposureState::FeedbackUsed { .. } => {}
    }
    Ok(())
}

async fn owned_mark_feedback_used_if_needed(
    session: &mut Session,
    ctx: &Context,
    ticket: &MirrorEvaluationTicketV2,
    at_unix_seconds: i64,
) -> Result<()> {
    let at_unix_ms = at_unix_seconds
        .checked_mul(1_000)
        .ok_or_else(|| Error::Invalid("feedback timestamp conversion overflow".into()))?;
    let mut ledger: ExposureLedger =
        need_record(session, ctx, LEDGER_KIND, &ticket.research_family_id).await?;
    let entry = ledger
        .entries
        .iter()
        .find(|entry| entry.ticket_id == ticket.id)
        .ok_or(Error::NotFound)?;
    match entry.state.clone() {
        ExposureState::Dispatched { .. } => {
            ledger.mark_feedback_used(&ticket.id, at_unix_ms)?;
            put_record(
                session,
                ctx,
                LEDGER_KIND,
                &ticket.research_family_id,
                ticket.evaluator_actor.as_str(),
                &ledger,
            )
            .await?;
        }
        ExposureState::FeedbackUsed { .. } => {}
        ExposureState::Reserved => {
            return Err(Error::Conflict(
                "grader feedback cannot precede real dispatch".into(),
            ));
        }
    }
    Ok(())
}

async fn owned_settle_exposure_money(
    session: &mut Session,
    ctx: &Context,
    ticket: &MirrorEvaluationTicketV2,
    usage_uncertain: bool,
) -> Result<()> {
    let mut ledger: ExposureLedger =
        need_record(session, ctx, LEDGER_KIND, &ticket.research_family_id).await?;
    let entry = ledger
        .entries
        .iter()
        .find(|entry| entry.ticket_id == ticket.id)
        .ok_or(Error::NotFound)?;
    let mut changed = false;
    let exposure_state = entry.state.clone();
    let monetary_state = entry.monetary_state;
    match (&exposure_state, monetary_state) {
        (ExposureState::Reserved, MonetaryReservationState::HeldUndispatched)
            if !usage_uncertain =>
        {
            ledger.release_undispatched_money(&ticket.id)?;
            changed = true;
        }
        (ExposureState::Reserved, MonetaryReservationState::HeldUndispatched) => {
            return Err(Error::Conflict(
                "cannot release exposure money while dispatched usage is unknown".into(),
            ));
        }
        (
            ExposureState::Dispatched { .. } | ExposureState::FeedbackUsed { .. },
            MonetaryReservationState::DispatchedCostPending,
        ) if !usage_uncertain => {
            ledger.finalize_dispatched_cost(&ticket.id)?;
            changed = true;
        }
        (_, MonetaryReservationState::DispatchedCostPending) if usage_uncertain => {}
        (
            _,
            MonetaryReservationState::Finalized | MonetaryReservationState::ReleasedUndispatched,
        ) => {}
        _ => {
            return Err(Error::Conflict(
                "exposure monetary state is inconsistent at terminal settlement".into(),
            ));
        }
    }
    if changed {
        put_record(
            session,
            ctx,
            LEDGER_KIND,
            &ticket.research_family_id,
            ticket.evaluator_actor.as_str(),
            &ledger,
        )
        .await?;
    }
    Ok(())
}

async fn owned_close_execution_slots(
    session: &mut Session,
    ctx: &Context,
    control: &MirrorRegisteredEvaluationControl,
    ticket: &mut MirrorEvaluationTicketV2,
    reason: &str,
    now: i64,
) -> Result<bool> {
    session
        .stop_dispatch_group(ctx, &control.billing_scope, &ticket.id, reason, now)
        .await?;
    let calls = session
        .budget_calls_for_group(ctx, &control.billing_scope, &ticket.id)
        .await?;
    let earliest_dispatch = calls
        .iter()
        .filter(|call| {
            matches!(
                call.state,
                BudgetCallState::Dispatched
                    | BudgetCallState::Uncertain
                    | BudgetCallState::Finalized
            )
        })
        .map(|call| {
            call.dispatched_at.ok_or_else(|| {
                Error::Conflict("dispatched budget call lacks dispatch timestamp".into())
            })
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .min();
    if let Some(dispatched_at) = earliest_dispatch {
        owned_mark_exposure_dispatched_if_needed(session, ctx, ticket, dispatched_at).await?;
    }
    let mut by_id: BTreeMap<String, BudgetCallRecord> = calls
        .into_iter()
        .map(|call| (call.call_id.clone(), call))
        .collect();
    let mut attributed = BTreeSet::new();
    for target in &ticket.targets {
        match &target.state {
            MirrorTargetState::Running {
                candidate,
                baseline,
            }
            | MirrorTargetState::RunningUsageUnknown {
                candidate,
                baseline,
            }
            | MirrorTargetState::CancelledAfterDispatch {
                candidate,
                baseline,
            } => {
                for slot in [candidate, baseline].into_iter().flatten() {
                    attributed.insert(slot.budget_call_id.clone());
                }
            }
            MirrorTargetState::Completed {
                candidate_receipt_id,
                baseline_receipt_id,
                ..
            } => {
                for receipt_id in [candidate_receipt_id, baseline_receipt_id] {
                    let receipt: MirrorExecutionReceiptV2 =
                        need_record(session, ctx, EXECUTION_RECEIPT_KIND, receipt_id).await?;
                    attributed.insert(receipt.budget_call_id);
                }
            }
            _ => {}
        }
    }
    let mut usage_uncertain = false;
    for call in by_id.values_mut() {
        if attributed.contains(&call.call_id) {
            continue;
        }
        ticket
            .unattributed_budget_call_ids
            .push(call.call_id.clone());
        if !ticket
            .invalid_reasons
            .iter()
            .any(|value| value == "unattributed_dispatch_group_call")
        {
            ticket
                .invalid_reasons
                .push("unattributed_dispatch_group_call".into());
        }
        match call.state {
            BudgetCallState::Reserved => {
                let fence = owned_call_fence(call, now);
                *call = session
                    .release_undispatched_budget_call(ctx, &fence, "unattributed_ticket_stop")
                    .await?;
            }
            BudgetCallState::Dispatched | BudgetCallState::Uncertain => usage_uncertain = true,
            BudgetCallState::Finalized if !call.execution_closed => usage_uncertain = true,
            BudgetCallState::Finalized => {
                // Cost is known, but this unregistered dispatch permanently
                // invalidates the evaluation even after accounting closes.
            }
            BudgetCallState::Released | BudgetCallState::Cancelled => {}
        }
    }
    for target in &mut ticket.targets {
        let current = target.state.clone();
        match current {
            MirrorTargetState::Planned => {
                target.state = MirrorTargetState::NotDispatched {
                    released_call_ids: Vec::new(),
                };
            }
            MirrorTargetState::Running {
                candidate,
                baseline,
            } => {
                let mut released = Vec::new();
                let mut dispatched = false;
                let mut target_uncertain = false;
                for slot in [&candidate, &baseline].into_iter().flatten() {
                    let call = by_id
                        .get_mut(&slot.budget_call_id)
                        .ok_or_else(|| Error::Conflict("bound budget call is missing".into()))?;
                    match call.state {
                        BudgetCallState::Reserved => {
                            let fence = owned_call_fence(call, now);
                            *call = session
                                .release_undispatched_budget_call(
                                    ctx,
                                    &fence,
                                    "ticket_stopped_before_dispatch",
                                )
                                .await?;
                            released.push(call.call_id.clone());
                        }
                        BudgetCallState::Released | BudgetCallState::Cancelled => {
                            released.push(call.call_id.clone());
                        }
                        BudgetCallState::Dispatched | BudgetCallState::Uncertain => {
                            dispatched = true;
                            target_uncertain = true;
                        }
                        BudgetCallState::Finalized => {
                            dispatched = true;
                            target_uncertain |= !call.execution_closed;
                        }
                    }
                }
                usage_uncertain |= target_uncertain;
                target.state = if target_uncertain {
                    MirrorTargetState::RunningUsageUnknown {
                        candidate,
                        baseline,
                    }
                } else if dispatched {
                    MirrorTargetState::CancelledAfterDispatch {
                        candidate,
                        baseline,
                    }
                } else {
                    MirrorTargetState::NotDispatched {
                        released_call_ids: released,
                    }
                };
            }
            _ => {}
        }
    }
    ticket.unattributed_budget_call_ids.sort();
    ticket.unattributed_budget_call_ids.dedup();
    Ok(usage_uncertain)
}

fn owned_call_fence(call: &BudgetCallRecord, now: i64) -> BudgetCallFence {
    BudgetCallFence {
        billing_scope: call.billing_scope.clone(),
        call_id: call.call_id.clone(),
        actual_input_digest: call.actual_input_digest.clone(),
        lease_token: call.lease_token.clone(),
        lease_epoch: call.lease_epoch,
        now,
    }
}

async fn owned_stop_ticket(
    session: &mut Session,
    ctx: &Context,
    control: &MirrorRegisteredEvaluationControl,
    ticket: &mut MirrorEvaluationTicketV2,
    decision: EarlyStopDecision,
    issuer: &MirrorIndependentGraderReceiptV2,
    now: i64,
) -> Result<()> {
    ticket.lifecycle = MirrorTicketLifecycle::StopRequested;
    let usage_uncertain = owned_close_execution_slots(
        session,
        ctx,
        control,
        ticket,
        owned_stop_reason(decision),
        now,
    )
    .await?;
    owned_settle_exposure_money(session, ctx, ticket, usage_uncertain).await?;
    let (k, lcb, ucb, prefix_digest) = if decision == EarlyStopDecision::CriticalRegression {
        (0, None, None, fingerprint(&Vec::<String>::new())?)
    } else {
        let plan = control
            .early_stop_plan
            .as_ref()
            .ok_or_else(|| Error::Invalid("statistical stop lacks early-stop plan".into()))?;
        let (_, stream) = owned_rebuild_stream(ticket, plan, &control.alpha_plan)?;
        let bound = stream
            .boundary()?
            .ok_or_else(|| Error::Invalid("statistical stop lacks a completed prefix".into()))?;
        let wire = bound.to_wire()?;
        (
            bound.k,
            Some(wire.lcb),
            Some(wire.ucb),
            stream.completed_prefix_digest()?,
        )
    };
    let member_terminal_digest = fingerprint(&ticket.targets)?;
    ticket.stop_member_terminal_digest = Some(member_terminal_digest.clone());
    let certificate = EarlyStopCertificate::from_verified_parts(EarlyStopCertificateParts {
        ticket_id: ticket.id.clone(),
        v1_plan_snapshot_digest: ticket.v1_plan_snapshot_digest.clone(),
        early_stop_plan_digest: control
            .early_stop_plan
            .as_ref()
            .map(EarlyStopPlan::digest)
            .transpose()?
            .unwrap_or(control.formal_plan.digest()?),
        alpha_plan_digest: ticket.alpha_plan_digest.clone(),
        manifest_digest: ticket.manifest_digest.clone(),
        slice_id: load_slice_id(session, ctx, &ticket.holdout_id).await?,
        candidate_digest: ticket.candidate_digest.clone(),
        baseline_digest: ticket.baseline_digest.clone(),
        grader_digest: control.grader_digest.clone(),
        issuer_receipt_digest: fingerprint(issuer)?,
        k,
        lcb,
        ucb,
        completed_prefix_digest: prefix_digest,
        member_terminal_digest,
        stop_reason: decision,
        stopped_at_unix_ms: now
            .checked_mul(1_000)
            .ok_or_else(|| Error::Invalid("stop timestamp conversion overflow".into()))?,
        stop_sequence: ticket.next_sequence,
        usage_uncertain,
    })?;
    ticket.pending_certificate = Some(certificate);
    ticket.lifecycle = if usage_uncertain {
        MirrorTicketLifecycle::Draining
    } else {
        MirrorTicketLifecycle::EarlyStopped
    };
    Ok(())
}

fn owned_validate_target_receipt_pair(
    target: &MirrorEvaluationTarget,
    candidate_receipt_id: &str,
    baseline_receipt_id: &str,
) -> Result<()> {
    match &target.state {
        MirrorTargetState::Running {
            candidate: Some(candidate),
            baseline: Some(baseline),
        } if candidate.receipt_id.as_deref() == Some(candidate_receipt_id)
            && baseline.receipt_id.as_deref() == Some(baseline_receipt_id) =>
        {
            Ok(())
        }
        MirrorTargetState::Completed { .. } => Err(Error::Conflict(
            "completed target score is immutable".into(),
        )),
        _ => Err(Error::Invalid(
            "grader requires both execution receipts recorded on the target".into(),
        )),
    }
}

fn owned_stop_reason(decision: EarlyStopDecision) -> &'static str {
    match decision {
        EarlyStopDecision::CriticalRegression => "critical_regression",
        EarlyStopDecision::FutilityQualityGain => "futility_quality_gain",
        EarlyStopDecision::FutilityNoninferiority => "futility_noninferiority",
        EarlyStopDecision::RegressedSequential => "sequential_regression",
        _ => "invalid_early_stop",
    }
}
fn owned_validate_output_artifact(
    receipt: &MirrorExecutionReceiptV2,
    output: &MirrorExecutionOutputArtifact,
) -> Result<()> {
    if output.schema_version != "rsia.execution_output.v1"
        || output.id != receipt.output_artifact_id
        || output.ticket_id != receipt.ticket_id
        || output.target_id != receipt.target_id
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

const COMPLETE_INTENT_SCHEMA: &str = "rsia.e05_complete_intent.v2";
const RECONCILE_INTENT_SCHEMA: &str = "rsia.e05_reconcile_intent.v2";
const RESOURCE_KIND: &str = "evaluation_resource_evidence_v1";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TicketIntent {
    schema_version: String,
    ticket_id: String,
}
impl TicketIntent {
    fn read(value: Value, schema: &str) -> Result<Self> {
        let intent: Self = serde_json::from_value(value)
            .map_err(|_| Error::Invalid("invalid E05 terminal v2 intent".into()))?;
        if intent.schema_version != schema {
            return Err(Error::Invalid(
                "unsupported E05 terminal intent schema".into(),
            ));
        }
        identifier(&intent.ticket_id)?;
        Ok(intent)
    }
}

struct E05CompleteDraft {
    before: MirrorEvaluationTicketV2,
    control: MirrorRegisteredEvaluationControl,
    control_digest: String,
    holdout_digest: String,
    expected_calls: Vec<BudgetCallRecord>,
    extra_calls: bool,
    at: i64,
}
impl E05CompleteDraft {
    fn derive(
        database: &RawDatabase,
        ctx: &Context,
        intent: TicketIntent,
        at: i64,
    ) -> Result<Self> {
        let (snapshot, _) = snapshot(database)?;
        if record_optional::<Value>(&snapshot, ctx.namespace(), TICKET_KIND, &intent.ticket_id)?
            .is_none()
        {
            return Err(Error::NotFound);
        }
        let before = evaluation_ticket(&snapshot, ctx.namespace(), &intent.ticket_id)?;
        if before.evaluator_actor != ctx.actor()
            || before.lifecycle != MirrorTicketLifecycle::ReadyForFinal
        {
            return Err(Error::Conflict(
                "ticket is not ready for fixed-sample finalization".into(),
            ));
        }
        require_new_formal(&snapshot, ctx.namespace(), &before.id)?;
        let control = evaluation_control(&snapshot, ctx.namespace(), &before.registration_id)?;
        let holdout = evaluation_holdout(&snapshot, ctx.namespace(), &before.holdout_id, &control)?;
        producer_witnesses(&snapshot, ctx.namespace(), &control, &holdout)?;
        e05_complete_ticket_identity(&snapshot, ctx.namespace(), &before, &control, &holdout)?;
        native_formal_consumers_r49::preflight_host_complete_r49(database, ctx, &before, &control)?;
        let mut expected_calls = Vec::new();
        let mut used = BTreeSet::new();
        for target in &before.targets {
            let MirrorTargetState::Completed {
                candidate_receipt_id,
                baseline_receipt_id,
                ..
            } = &target.state
            else {
                return Err(Error::Invalid(
                    "complete ticket contains a non-complete target".into(),
                ));
            };
            // This independently rereads original output/oracle and regrades
            // both executions; the ticket's scores alone never supply a report.
            e05_completed_target_evidence(
                &snapshot,
                ctx.namespace(),
                &before,
                &control,
                &holdout,
                target,
            )?;
            for id in [candidate_receipt_id, baseline_receipt_id] {
                let receipt: MirrorExecutionReceiptV2 =
                    evaluation_record(&snapshot, ctx.namespace(), EXECUTION_RECEIPT_KIND, id)?;
                let call = qualified_call(
                    &snapshot,
                    ctx.namespace(),
                    &control.billing_scope,
                    &receipt.budget_call_id,
                )?;
                if call.dispatch_group_id != before.id
                    || call.state != BudgetCallState::Finalized
                    || call.actual_cost_micros.is_none()
                    || !call.execution_closed
                {
                    return Err(Error::Conflict(
                        "formal completion requires finalized execution usage".into(),
                    ));
                }
                if !used.insert(call.call_id.clone()) {
                    return Err(e05_complete_changed());
                }
                expected_calls.push(call.clone());
            }
        }
        // The complete billing/group slice includes other namespaces. Such a
        // call cannot disappear by filtering to the requested namespace first.
        let all: Vec<_> = snapshot
            .calls
            .values()
            .filter(|call| {
                call.billing_scope == control.billing_scope && call.dispatch_group_id == before.id
            })
            .collect();
        let actual_ids: BTreeSet<_> = all.iter().map(|call| call.call_id.as_str()).collect();
        require(actual_ids.len() == all.len())?;
        let expected_ids: BTreeSet<_> = expected_calls
            .iter()
            .map(|call| call.call_id.as_str())
            .collect();
        let extra_calls = expected_ids != actual_ids;
        Ok(Self {
            before,
            control_digest: typed_fingerprint(&control)?,
            holdout_digest: typed_fingerprint(&holdout)?,
            control,
            expected_calls,
            extra_calls,
            at,
        })
    }
    async fn prepare(
        self,
        session: &mut Session,
        ctx: &Context,
        gate: &AtomicU8,
    ) -> Result<OwnedOperationPlan> {
        check_cancel(gate)?;
        if self.extra_calls {
            let error = Error::Invalid("ticket dispatch group contains unattributed calls".into());
            let mut invalid = invalid_draft(
                self.before,
                self.control,
                self.control_digest,
                self.holdout_digest,
                "unattributed_dispatch_group_call",
                self.at,
                Err(error),
            );
            invalid.prepare_budget(session, ctx, gate).await?;
            let raw = load_sqlx_database(&mut **session.tx.as_mut().ok_or(Error::ClockRegressed)?)
                .await?;
            collect_raw(&raw, &session.prepare_inputs, SchemaRead::Stable)
                .map_err(|_| session.clock.fail())?;
            return Ok(OwnedOperationPlan::Complete(Box::new(
                E05CompletePlan::from_complete_invalid_r1(&raw, ctx, invalid)?,
            )));
        }
        session
            .stop_dispatch_group(
                ctx,
                &self.control.billing_scope,
                &self.before.id,
                "complete_batch",
                self.at,
            )
            .await?;
        check_cancel(gate)?;
        owned_settle_exposure_money(session, ctx, &self.before, false).await?;
        check_cancel(gate)?;
        let raw =
            load_sqlx_database(&mut **session.tx.as_mut().ok_or(Error::ClockRegressed)?).await?;
        collect_raw(&raw, &session.prepare_inputs, SchemaRead::Stable)
            .map_err(|_| session.clock.fail())?;
        Ok(OwnedOperationPlan::Complete(Box::new(
            E05CompletePlan::derive(&raw, ctx, self)?,
        )))
    }
}

impl E05CompletePlan {
    fn from_native_early_r44(prefix: E05AppendPlan) -> Result<Self> {
        require(
            prefix.after_ticket.lifecycle == MirrorTicketLifecycle::EarlyStopped
                && prefix.before_ticket.lifecycle != MirrorTicketLifecycle::EarlyStopped
                && prefix.first_stop_r44.is_some(),
        )?;
        let formal = MirrorFormalEvaluationV2::EarlyStopped {
            evidence_scope: prefix.after_ticket.evidence_scope,
            ticket_id: prefix.after_ticket.id.clone(),
            ticket_digest: typed_fingerprint(&prefix.after_ticket)?,
            certificate: prefix
                .after_ticket
                .pending_certificate
                .clone()
                .ok_or(Error::ClockRegressed)?,
        };
        Ok(Self {
            prefix,
            work_r1: CompleteRequestWorkR1::EarlyR44 {
                formal: Box::new(formal),
            },
            materialized: false,
            frozen_r24: None,
        })
    }
    async fn materialize_native_early_r44(
        &mut self,
        session: &mut Session,
        ctx: &Context,
        asof: &NativeAsOfPermit,
    ) -> Result<()> {
        require(
            !self.materialized
                && self.prefix.next == self.prefix.writes.len()
                && self.work_r1.effect() == NativePlanEffectR1::NewEarlyR44,
        )?;
        asof.same_session(session, true)?;
        require(
            matches!(
                &asof.operation,
                NativeAsOfOperation::Grade | NativeAsOfOperation::Reconcile
            ) && asof.actual_effect_r1 == Some(NativePlanEffectR1::NewEarlyR44)
                && asof.namespace == ctx.namespace()
                && asof.actor == ctx.actor()
                && asof.role == ctx.role()
                && asof.acknowledged_writes == self.prefix.next
                && asof.closed_prefix_digest.is_none()
                && asof.prefix_digest == native_database_digest(&self.prefix.expected, true)?,
        )?;
        self.prefix.admit(session).await?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        let raw = load_sqlx_database(tx)
            .await
            .map_err(|_| session.clock.fail())?;
        same_database(&raw, &self.prefix.expected).map_err(|_| session.clock.fail())?;
        let (snapshot, _) = snapshot(&raw)?;
        let actual = evaluation_ticket(
            &snapshot,
            &self.prefix.namespace,
            &self.prefix.after_ticket.id,
        )?;
        require(typed_fingerprint(&actual)? == typed_fingerprint(&self.prefix.after_ticket)?)?;
        let CompleteRequestWorkR1::EarlyR44 { formal } = &self.work_r1 else {
            return Err(Error::ClockRegressed);
        };
        require(
            actual.lifecycle == MirrorTicketLifecycle::EarlyStopped
                && r24_formal_ticket(formal) == actual.id,
        )?;
        let frozen_floor = self.prefix.qualify(&raw, &session.prepare_inputs)?;
        let stamp = session.clock.event(&frozen_floor)?;
        let (event, clock) = session.clock.take_bound_event(stamp)?;
        let carrier = freeze_native_early_carrier_r44(
            &raw,
            &snapshot,
            ctx,
            &actual,
            formal.as_ref().clone(),
            event.millis(),
            match &asof.operation {
                NativeAsOfOperation::Grade => evo_core::formal_version::Operation::EarlyStop,
                NativeAsOfOperation::Reconcile => evo_core::formal_version::Operation::Reconcile,
                _ => return Err(Error::ClockRegressed),
            },
        )?;
        require(
            carrier_effect_r1(&carrier.input.operation, &carrier.payload)?
                == NativePlanEffectR1::NewEarlyR44,
        )?;
        let reference = carrier.report_ref(ctx.actor()).map_err(r24_fault)?;
        let insert_index = self.prefix.writes.len();
        self.prefix
            .writes
            .push(versioned_formal_object_write(&raw, ctx, &carrier)?);
        self.prefix.scope = AppendScope::EarlyR44 {
            formal_physical: Some(carrier.id.clone()),
        };
        if actual.candidate_dependency_binding.is_some() {
            self.prefix.push_edge(
                &snapshot,
                &carrier.id,
                e05_artifact_edge(TICKET_KIND, &actual.id)?,
            )?;
        }
        self.frozen_r24 = Some(CompleteFrozenStateR24 {
            carrier,
            reference,
            insert_index,
            clock,
        });
        self.materialized = true;
        // Preserve the real grade-receipt/reconcile-ticket output. A new Ref
        // is independently discovered by the fixed reader; never reinterpret it as a ticket ID.
        Ok(())
    }
}

struct E05CompletePlan {
    prefix: E05AppendPlan,
    work_r1: CompleteRequestWorkR1,
    materialized: bool,
    frozen_r24: Option<CompleteFrozenStateR24>,
}
impl E05CompletePlan {
    fn from_complete_invalid_r1(
        database: &RawDatabase,
        ctx: &Context,
        draft: E05GradeDraft,
    ) -> Result<Self> {
        require(
            draft.invalidated
                && !draft.replay
                && !draft.accepted
                && !draft.feedback
                && draft.receipt.is_none()
                && !draft.audit_only
                && !draft.audit_error
                && draft.stop.is_none()
                && draft.before.lifecycle == MirrorTicketLifecycle::ReadyForFinal
                && draft.after.lifecycle == MirrorTicketLifecycle::Invalid
                && draft.invalid_reason == Some("unattributed_dispatch_group_call")
                && !draft.after.unattributed_budget_call_ids.is_empty()
                && matches!(&draft.output,Err(Error::Invalid(reason))
                if reason == "ticket dispatch group contains unattributed calls"),
        )?;
        let formal = MirrorFormalEvaluationV2::Invalid {
            evidence_scope: draft.after.evidence_scope,
            ticket_id: draft.after.id.clone(),
            ticket_digest: typed_fingerprint(&draft.after)?,
            reasons: draft.after.invalid_reasons.clone(),
        };
        let mut prefix = E05AppendPlan::grade_to_destination_r1(
            database,
            ctx,
            draft,
            GradeFormalDestinationR1::VersionedCompleteRequestInvalid,
        )?;
        prefix.scope = AppendScope::CompleteInvalidR1 {
            formal_physical: None,
        };
        require(!prefix.writes.is_empty())?;
        Ok(Self {
            prefix,
            work_r1: CompleteRequestWorkR1::Invalid {
                formal: Box::new(formal),
            },
            materialized: false,
            frozen_r24: None,
        })
    }

    async fn materialize_invalid_after_prefix_ack_r1(
        &mut self,
        session: &mut Session,
        ctx: &Context,
        asof: &NativeAsOfPermit,
    ) -> Result<()> {
        require(
            !self.materialized
                && self.prefix.next == self.prefix.writes.len()
                && self.work_r1.effect() == NativePlanEffectR1::NewInvalid,
        )?;
        asof.same_session(session, true)?;
        require(
            matches!(&asof.operation, NativeAsOfOperation::Complete)
                && asof.actual_effect_r1 == Some(NativePlanEffectR1::NewInvalid)
                && asof.namespace == ctx.namespace()
                && asof.actor == ctx.actor()
                && asof.role == ctx.role()
                && asof.acknowledged_writes == self.prefix.next
                && asof.closed_prefix_digest.is_none()
                && asof.prefix_digest == native_database_digest(&self.prefix.expected, true)?,
        )?;
        self.prefix.admit(session).await?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        let raw = load_sqlx_database(tx)
            .await
            .map_err(|_| session.clock.fail())?;
        same_database(&raw, &self.prefix.expected).map_err(|_| session.clock.fail())?;
        let (snapshot, _) = snapshot(&raw)?;
        let actual = evaluation_ticket(
            &snapshot,
            &self.prefix.namespace,
            &self.prefix.after_ticket.id,
        )?;
        require(typed_fingerprint(&actual)? == typed_fingerprint(&self.prefix.after_ticket)?)?;
        let CompleteRequestWorkR1::Invalid { formal } = &self.work_r1 else {
            return Err(Error::ClockRegressed);
        };
        require(
            actual.lifecycle == MirrorTicketLifecycle::Invalid
                && r24_formal_ticket(formal) == actual.id,
        )?;
        let frozen_floor = self.prefix.qualify(&raw, &session.prepare_inputs)?;
        let stamp = session.clock.event(&frozen_floor)?;
        let (event, clock) = session.clock.take_bound_event(stamp)?;
        let carrier = freeze_complete_carrier_r36(
            &raw,
            &snapshot,
            ctx,
            &actual,
            formal.as_ref().clone(),
            event.millis(),
        )?;
        require(
            carrier_effect_r1(&carrier.input.operation, &carrier.payload)?
                == NativePlanEffectR1::NewInvalid,
        )?;
        let reference = carrier.report_ref(ctx.actor()).map_err(r24_fault)?;
        let insert_index = self.prefix.writes.len();
        self.prefix
            .writes
            .push(versioned_formal_object_write(&raw, ctx, &carrier)?);
        self.prefix.scope = AppendScope::CompleteInvalidR1 {
            formal_physical: Some(carrier.id.clone()),
        };
        if actual.candidate_dependency_binding.is_some() {
            self.prefix.push_edge(
                &snapshot,
                &carrier.id,
                e05_artifact_edge(TICKET_KIND, &actual.id)?,
            )?;
        }
        self.frozen_r24 = Some(CompleteFrozenStateR24 {
            carrier,
            reference,
            insert_index,
            clock,
        });
        self.materialized = true;
        // Keep prefix.output EXACTLY the original unattributed refusal. The
        // owner exposes it only after this new member/audit/actual COMMIT ACK.
        Ok(())
    }

    fn derive(database: &RawDatabase, ctx: &Context, draft: E05CompleteDraft) -> Result<Self> {
        let (snapshot, _) = snapshot(database)?;
        let current = evaluation_ticket(&snapshot, ctx.namespace(), &draft.before.id)?;
        require(typed_fingerprint(&current)? == typed_fingerprint(&draft.before)?)?;
        let control = evaluation_control(&snapshot, ctx.namespace(), &current.registration_id)?;
        let holdout =
            evaluation_holdout(&snapshot, ctx.namespace(), &current.holdout_id, &control)?;
        producer_witnesses(&snapshot, ctx.namespace(), &control, &holdout)?;
        require(
            typed_fingerprint(&control)? == draft.control_digest
                && typed_fingerprint(&holdout)? == draft.holdout_digest,
        )?;
        e05_complete_ticket_identity(&snapshot, ctx.namespace(), &current, &control, &holdout)?;
        e05_complete_group(
            &snapshot,
            ctx.namespace(),
            &control.billing_scope,
            &current.id,
        )?;
        let actual: Vec<_> = snapshot
            .calls
            .values()
            .filter(|call| {
                call.billing_scope == control.billing_scope && call.dispatch_group_id == current.id
            })
            .cloned()
            .collect();
        let expected_by_id: BTreeMap<_, _> = draft
            .expected_calls
            .iter()
            .map(|call| (call.call_id.as_str(), call))
            .collect();
        require(
            actual.len() == expected_by_id.len()
                && actual.iter().all(|call| {
                    expected_by_id
                        .get(call.call_id.as_str())
                        .is_some_and(|expected| *expected == call)
                }),
        )?;
        let mut after = current.clone();
        after.lifecycle = MirrorTicketLifecycle::CompleteBatch;
        after.validate()?;
        // This is only a private construction input. No future ticket is added
        // to snapshot, no report/source authority is granted by this computation.
        let report = e05_recompute_complete_report(
            &snapshot,
            ctx.namespace(),
            &after,
            &control,
            &holdout,
            &actual,
        )?;
        let resource = MirrorResourceEvidenceRecord {
            schema_version: "rsia.evaluation_resource_evidence.v1".into(),
            ticket_id: after.id.clone(),
            billing_scope: control.billing_scope.clone(),
            cost_scope: MirrorCostEvidenceScope::TicketExecutionOnly,
            calls: draft.expected_calls,
        };
        let resource_physical = evaluation_storage_id(RESOURCE_KIND, &after.id)?;
        let scope = AppendScope::Complete {
            resource_physical: resource_physical.clone(),
            formal_physical: None,
        };
        let mut prefix =
            E05AppendPlan::base(database, ctx, current, after, scope, Err(Error::Internal))?;
        prefix.writes.push(object_write(
            database,
            ctx,
            RESOURCE_KIND,
            &prefix.after_ticket.id,
            &resource,
            true,
        )?);
        if prefix.after_ticket.candidate_dependency_binding.is_some() {
            prefix.push_edge(
                &snapshot,
                &resource_physical,
                e05_artifact_edge(TICKET_KIND, &prefix.after_ticket.id)?,
            )?;
        }
        prefix.push_ticket(database, ctx)?;
        prefix.audit = Some(("complete_formal_evaluation", prefix.after_ticket.id.clone()));
        Ok(Self {
            prefix,
            work_r1: CompleteRequestWorkR1::Complete {
                report: Box::new(report),
                resource,
            },
            materialized: false,
            frozen_r24: None,
        })
    }

    async fn materialize_after_prefix_ack(
        &mut self,
        session: &mut Session,
        ctx: &Context,
        asof: &NativeAsOfPermit,
    ) -> Result<()> {
        if self.materialized || self.prefix.next != self.prefix.writes.len() {
            return Ok(());
        }
        if matches!(&self.work_r1, CompleteRequestWorkR1::EarlyR44 { .. }) {
            return self.materialize_native_early_r44(session, ctx, asof).await;
        }
        if matches!(&self.work_r1, CompleteRequestWorkR1::Invalid { .. }) {
            return self
                .materialize_invalid_after_prefix_ack_r1(session, ctx, asof)
                .await;
        }
        let CompleteRequestWorkR1::Complete {
            report: expected_report,
            resource: expected_resource,
        } = &self.work_r1
        else {
            return Err(Error::ClockRegressed);
        };
        asof.same_session(session, true)?;
        require(
            matches!(&asof.operation, NativeAsOfOperation::Complete)
                && asof.actual_effect_r1 == Some(NativePlanEffectR1::NewComplete)
                && asof.namespace == ctx.namespace()
                && asof.actor == ctx.actor()
                && asof.role == ctx.role()
                && asof.acknowledged_writes == self.prefix.next
                && asof.closed_prefix_digest.is_none()
                && asof.prefix_digest == native_database_digest(&self.prefix.expected, true)?,
        )?;
        // This gate is reached only after resource/edge/final-ticket actual SQL
        // ACKs. The whole actual raw database must equal this precise prefix.
        self.prefix.admit(session).await?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        let raw = load_sqlx_database(tx)
            .await
            .map_err(|_| session.clock.fail())?;
        same_database(&raw, &self.prefix.expected).map_err(|_| session.clock.fail())?;
        let (snapshot, _) = snapshot(&raw)?;
        let actual = evaluation_ticket(
            &snapshot,
            &self.prefix.namespace,
            &self.prefix.after_ticket.id,
        )?;
        require(typed_fingerprint(&actual)? == typed_fingerprint(&self.prefix.after_ticket)?)?;
        let resource: MirrorResourceEvidenceRecord =
            evaluation_record(&snapshot, &self.prefix.namespace, RESOURCE_KIND, &actual.id)?;
        require(typed_fingerprint(&resource)? == typed_fingerprint(expected_resource)?)?;
        let control =
            evaluation_control(&snapshot, &self.prefix.namespace, &actual.registration_id)?;
        let holdout = evaluation_holdout(
            &snapshot,
            &self.prefix.namespace,
            &actual.holdout_id,
            &control,
        )?;
        producer_witnesses(&snapshot, &self.prefix.namespace, &control, &holdout)?;
        let calls: Vec<_> = snapshot
            .calls
            .values()
            .filter(|call| {
                call.billing_scope == control.billing_scope && call.dispatch_group_id == actual.id
            })
            .cloned()
            .collect();
        let report = e05_recompute_complete_report(
            &snapshot,
            &self.prefix.namespace,
            &actual,
            &control,
            &holdout,
            &calls,
        )?;
        require(&report == expected_report.as_ref())?;
        let ticket_physical = evaluation_storage_id(TICKET_KIND, &actual.id)?;
        let resource_physical = evaluation_storage_id(RESOURCE_KIND, &actual.id)?;
        let binding = if let Some(candidate) = &actual.candidate_dependency_binding {
            let binding = MirrorFormalStoredDependencyBinding {
                schema_version: "rsia.formal_stored_dependency_binding.v1".into(),
                namespace: self.prefix.namespace.clone(),
                candidate_binding_digest: typed_fingerprint(candidate)?,
                ticket_id: actual.id.clone(),
                ticket_physical_id: ticket_physical.clone(),
                ticket_digest: typed_fingerprint(&actual)?,
                resource_physical_id: resource_physical.clone(),
                nodes: derive_nodes(
                    &snapshot,
                    &self.prefix.namespace,
                    vec![
                        ("artifact".into(), ticket_physical.clone()),
                        ("artifact".into(), resource_physical.clone()),
                    ],
                )?,
            };
            e05_stored_binding(&snapshot, &self.prefix.namespace, &actual, &binding)?;
            Some(binding)
        } else {
            None
        };
        let anchors: Vec<_> = actual
            .targets
            .iter()
            .filter(|target| matches!(target.kind, MirrorEvaluationTargetKind::Anchor { .. }))
            .collect();
        let formal = MirrorFormalEvaluationV2::CompleteBatch {
            evidence_scope: control.evidence_scope,
            cost_evidence_scope: MirrorCostEvidenceScope::TicketExecutionOnly,
            ticket_id: actual.id.clone(),
            ticket_digest: typed_fingerprint(&actual)?,
            manifest_digest: actual.manifest_digest.clone(),
            candidate_bundle_digest: actual.candidate_bundle_digest.clone(),
            baseline_bundle_digest: actual.baseline_bundle_digest.clone(),
            environment_digest: actual.environment_digest.clone(),
            verdict: report.verdict,
            report,
            anchor_evidence_digest: typed_fingerprint(&anchors)?,
            resource_evidence_digest: typed_fingerprint(&resource)?,
            stored_dependency_binding: binding,
        };
        // Actual prefix is unchanged between its complete qualification and this
        // real bound event. No serialized timestamp or initial-capture alias.
        let frozen_floor = self.prefix.qualify(&raw, &session.prepare_inputs)?;
        let stamp = session.clock.event(&frozen_floor)?;
        let (frozen_event, frozen_clock) = session.clock.take_bound_event(stamp)?;
        let carrier = freeze_complete_carrier_r36(
            &raw,
            &snapshot,
            ctx,
            &actual,
            formal.clone(),
            frozen_event.millis(),
        )?;
        let reference = carrier.report_ref(ctx.actor()).map_err(r24_fault)?;
        let insert_index = self.prefix.writes.len();
        let write = versioned_formal_object_write(&raw, ctx, &carrier)?;
        self.prefix.scope = AppendScope::Complete {
            resource_physical: resource_physical.clone(),
            formal_physical: Some(carrier.id.clone()),
        };
        self.prefix.writes.push(write);
        if actual.candidate_dependency_binding.is_some() {
            for edge in [
                ("artifact".into(), ticket_physical),
                ("artifact".into(), resource_physical),
            ] {
                self.prefix.push_edge(&snapshot, &carrier.id, edge)?;
            }
        }
        self.prefix.output =
            serde_json::to_value(evo_core::formal_version::NativeCompleteContent {
                report_ref: reference.clone(),
                formal,
            })
            .map_err(crate::internal);
        self.frozen_r24 = Some(CompleteFrozenStateR24 {
            carrier,
            reference,
            insert_index,
            clock: frozen_clock,
        });
        self.materialized = true;
        Ok(())
    }
}

fn invalid_draft(
    before: MirrorEvaluationTicketV2,
    control: MirrorRegisteredEvaluationControl,
    control_digest: String,
    holdout_digest: String,
    reason: &'static str,
    at: i64,
    output: Result<Value>,
) -> E05GradeDraft {
    let mut after = before.clone();
    after.lifecycle = MirrorTicketLifecycle::Invalid;
    if !after.invalid_reasons.iter().any(|old| old == reason) {
        after.invalid_reasons.push(reason.into());
    }
    E05GradeDraft {
        before,
        after,
        control,
        control_digest,
        holdout_digest,
        receipt: None,
        receipt_id: String::new(),
        input_fingerprints: Vec::new(),
        at,
        accepted: false,
        replay: false,
        audit_only: false,
        audit_error: false,
        invalidated: true,
        invalid_reason: Some(reason),
        feedback: false,
        stop: None,
        prepared_grade: None,
        output,
    }
}

async fn prepare_reconcile(
    database: &RawDatabase,
    session: &mut Session,
    ctx: &Context,
    intent: TicketIntent,
    gate: &AtomicU8,
    at: i64,
) -> Result<OwnedOperationPlan> {
    let (snapshot, _) = snapshot(database)?;
    if record_optional::<Value>(&snapshot, ctx.namespace(), TICKET_KIND, &intent.ticket_id)?
        .is_none()
    {
        return Err(Error::NotFound);
    }
    let before = evaluation_ticket(&snapshot, ctx.namespace(), &intent.ticket_id)?;
    if before.evaluator_actor != ctx.actor() {
        return Err(Error::Forbidden);
    }
    let control = evaluation_control(&snapshot, ctx.namespace(), &before.registration_id)?;
    let holdout = evaluation_holdout(&snapshot, ctx.namespace(), &before.holdout_id, &control)?;
    producer_witnesses(&snapshot, ctx.namespace(), &control, &holdout)?;
    let control_digest = typed_fingerprint(&control)?;
    let holdout_digest = typed_fingerprint(&holdout)?;
    if before.lifecycle != MirrorTicketLifecycle::Draining {
        return unchanged_ticket_plan(database, ctx, before);
    }
    for target in &before.targets {
        if let MirrorTargetState::RunningUsageUnknown {
            candidate,
            baseline,
        } = &target.state
        {
            for slot in [candidate, baseline].into_iter().flatten() {
                let call = qualified_call(
                    &snapshot,
                    ctx.namespace(),
                    &control.billing_scope,
                    &slot.budget_call_id,
                )?;
                if matches!(
                    call.state,
                    BudgetCallState::Dispatched | BudgetCallState::Uncertain
                ) || (call.state == BudgetCallState::Finalized && !call.execution_closed)
                {
                    return unchanged_ticket_plan(database, ctx, before.clone());
                }
            }
        }
    }
    for id in &before.unattributed_budget_call_ids {
        let call = qualified_call(&snapshot, ctx.namespace(), &control.billing_scope, id)?;
        if matches!(
            call.state,
            BudgetCallState::Dispatched | BudgetCallState::Uncertain
        ) || (call.state == BudgetCallState::Finalized && !call.execution_closed)
        {
            return unchanged_ticket_plan(database, ctx, before);
        }
    }
    require_new_formal(&snapshot, ctx.namespace(), &before.id)?;
    if before
        .invalid_reasons
        .iter()
        .any(|reason| reason == "unattributed_dispatch_group_call")
    {
        let output = serde_json::to_value({
            let mut after = before.clone();
            after.lifecycle = MirrorTicketLifecycle::Invalid;
            after
        })
        .map_err(crate::internal);
        let mut draft = invalid_draft(
            before,
            control,
            control_digest,
            holdout_digest,
            "unattributed_dispatch_group_call",
            at,
            output,
        );
        draft.prepare_budget(session, ctx, gate).await?;
        // close_execution_slots may append exact paid/unattributed facts; return
        // the actual after image, never the earlier speculative output copy.
        draft.output = serde_json::to_value(&draft.after).map_err(crate::internal);
        let raw =
            load_sqlx_database(&mut **session.tx.as_mut().ok_or(Error::ClockRegressed)?).await?;
        collect_raw(&raw, &session.prepare_inputs, SchemaRead::Stable)
            .map_err(|_| session.clock.fail())?;
        return Ok(OwnedOperationPlan::Append(Box::new(E05AppendPlan::grade(
            &raw, ctx, draft,
        )?)));
    }
    let mut after = before.clone();
    for target in &mut after.targets {
        if let MirrorTargetState::RunningUsageUnknown {
            candidate,
            baseline,
        } = &target.state
        {
            target.state = MirrorTargetState::CancelledAfterDispatch {
                candidate: candidate.clone(),
                baseline: baseline.clone(),
            };
        }
    }
    check_cancel(gate)?;
    owned_settle_exposure_money(session, ctx, &after, false).await?;
    check_cancel(gate)?;
    after.pending_certificate = Some(
        after
            .pending_certificate
            .take()
            .ok_or_else(|| Error::Invalid("draining stop lacks certificate".into()))?
            .with_usage_reconciled()?,
    );
    after.lifecycle = MirrorTicketLifecycle::EarlyStopped;
    after.validate()?;
    let raw = load_sqlx_database(&mut **session.tx.as_mut().ok_or(Error::ClockRegressed)?).await?;
    collect_raw(&raw, &session.prepare_inputs, SchemaRead::Stable)
        .map_err(|_| session.clock.fail())?;
    let output = serde_json::to_value(&after).map_err(crate::internal);
    let mut plan = E05AppendPlan::base(
        &raw,
        ctx,
        before,
        after,
        AppendScope::Grade {
            receipt_physical: None,
        },
        output,
    )?;
    plan.push_ticket(&raw, ctx)?;
    plan.push_early_formal(&raw, ctx)?;
    Ok(OwnedOperationPlan::Append(Box::new(plan)))
}

fn unchanged_ticket_plan(
    database: &RawDatabase,
    ctx: &Context,
    before: MirrorEvaluationTicketV2,
) -> Result<OwnedOperationPlan> {
    let output = serde_json::to_value(&before).map_err(crate::internal);
    Ok(OwnedOperationPlan::Append(Box::new(E05AppendPlan::base(
        database,
        ctx,
        before.clone(),
        before,
        AppendScope::Grade {
            receipt_physical: None,
        },
        output,
    )?)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InvalidateIntent {
    schema_version: String,
    ticket_id: String,
    reason: String,
}
impl InvalidateIntent {
    fn read(value: Value) -> Result<Self> {
        let intent: Self = serde_json::from_value(value)
            .map_err(|_| Error::Invalid("invalid E05 invalidation v2 intent".into()))?;
        if intent.schema_version != "rsia.e05_invalidate_intent.v2" {
            return Err(Error::Invalid(
                "unsupported E05 invalidation intent schema".into(),
            ));
        }
        identifier(&intent.ticket_id)?;
        identifier(&intent.reason)?;
        Ok(intent)
    }
}
async fn prepare_invalidate(
    database: &RawDatabase,
    session: &mut Session,
    ctx: &Context,
    intent: InvalidateIntent,
    gate: &AtomicU8,
    at: i64,
) -> Result<OwnedOperationPlan> {
    let (snapshot, _) = snapshot(database)?;
    if record_optional::<Value>(&snapshot, ctx.namespace(), TICKET_KIND, &intent.ticket_id)?
        .is_none()
    {
        return Err(Error::NotFound);
    }
    let before = evaluation_ticket(&snapshot, ctx.namespace(), &intent.ticket_id)?;
    if before.evaluator_actor != ctx.actor() {
        return Err(Error::Forbidden);
    }
    require_new_formal(&snapshot, ctx.namespace(), &before.id)?;
    let control = evaluation_control(&snapshot, ctx.namespace(), &before.registration_id)?;
    let holdout = evaluation_holdout(&snapshot, ctx.namespace(), &before.holdout_id, &control)?;
    producer_witnesses(&snapshot, ctx.namespace(), &control, &holdout)?;
    let mut after = before.clone();
    after.lifecycle = MirrorTicketLifecycle::Invalid;
    if !after.invalid_reasons.contains(&intent.reason) {
        after.invalid_reasons.push(intent.reason.clone());
    }
    check_cancel(gate)?;
    let uncertain =
        owned_close_execution_slots(session, ctx, &control, &mut after, &intent.reason, at).await?;
    check_cancel(gate)?;
    owned_settle_exposure_money(session, ctx, &after, uncertain).await?;
    check_cancel(gate)?;
    after.validate()?;
    let raw = load_sqlx_database(&mut **session.tx.as_mut().ok_or(Error::ClockRegressed)?).await?;
    collect_raw(&raw, &session.prepare_inputs, SchemaRead::Stable)
        .map_err(|_| session.clock.fail())?;
    let formal = MirrorFormalEvaluationV2::Invalid {
        evidence_scope: control.evidence_scope,
        ticket_id: after.id.clone(),
        ticket_digest: typed_fingerprint(&after)?,
        reasons: after.invalid_reasons.clone(),
    };
    let output = serde_json::to_value(&formal).map_err(crate::internal);
    let mut plan = E05AppendPlan::base(&raw, ctx, before, after, AppendScope::Invalidate, output)?;
    plan.push_ticket(&raw, ctx)?;
    plan.writes.push(object_write_as(
        &raw,
        ctx,
        FORMAL_KIND,
        &plan.after_ticket.id,
        ctx.actor(),
        &formal,
        false,
    )?);
    Ok(OwnedOperationPlan::Append(Box::new(plan)))
}

fn require_new_formal(
    snapshot: &OccurrenceSnapshot,
    namespace: &str,
    ticket_id: &str,
) -> Result<()> {
    let physical = evaluation_storage_id(FORMAL_KIND, ticket_id)?;
    if snapshot
        .objects
        .contains_key(&(namespace.into(), "artifact".into(), physical))
    {
        return Err(Error::Conflict(
            "formal_record_immutable_requires_versioned_successor".into(),
        ));
    }
    Ok(())
}

// AG132_R19_R1_BUDGET_CALL_SNAPSHOT_SOURCE_BEGIN
// Private candidate code only. This module performs lossless data conversion,
// not current/as-of authority. A native Stable-qualified, same-connection snapshot
// and an unforgeable operation permit must be connected separately by its owner.
// Deserializing FrozenCall is never proof of its origin. No provider, SQL, file,
// object-kind, history table, body copy, callback, or raw Session outlet is added.
mod e05_budget_call_snapshot_r19_r1 {
    use super::{E05MathCallR39, OccurrenceSnapshot, parse_owned_json, qualified_call};
    use crate::budget::{
        BudgetArtifact, BudgetCallRecord, BudgetCallState, BudgetExecutionProvenance, BudgetStage,
    };
    use serde::{Deserialize, Serialize};

    const SCHEMA: &str = "rsia.e05_budget_call_snapshot.v1";
    const MAX_ARTIFACT_BYTES: usize = 4 * 1024 * 1024;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum ReadFailure {
        InvalidSnapshot(&'static str),
        SourceInsufficient(&'static str),
    }
    type SnapshotResult<T> = std::result::Result<T, ReadFailure>;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum Slot {
        Request,
        Transport,
        Response,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ArtifactRef {
        namespace: String,
        billing_scope: String,
        call_id: String,
        slot: Slot,
        schema_version: String,
        digest: String,
        body_bytes: u64,
    }

    // No Clone/Copy/Serde/Deref: an externally supplied DTO cannot create this
    // borrow. This lookup itself does not establish Stable/native/as-of authority.
    // The future native caller must retain its real snapshot/permit and recheck
    // the required phase, source witnesses, Clock floor and terminal ACK.
    struct QualifiedCallSource<'a> {
        call: &'a BudgetCallRecord,
    }

    impl<'a> QualifiedCallSource<'a> {
        fn from_qualified_snapshot(
            snapshot: &'a OccurrenceSnapshot,
            namespace: &str,
            billing_scope: &str,
            call_id: &str,
        ) -> SnapshotResult<Self> {
            // Distinguish absence from an extant row with contradicted identity.
            // Do not erase the original qualified_call check after this lookup.
            let stored = snapshot
                .calls
                .get(&(billing_scope.to_owned(), call_id.to_owned()))
                .ok_or(ReadFailure::SourceInsufficient("qualified_call_missing"))?;
            if stored.namespace != namespace
                || stored.billing_scope != billing_scope
                || stored.call_id != call_id
            {
                return Err(ReadFailure::InvalidSnapshot("qualified_call_identity"));
            }
            let call = qualified_call(snapshot, namespace, billing_scope, call_id)
                .map_err(|_| ReadFailure::InvalidSnapshot("qualified_call_check_failed"))?;
            if call.namespace != namespace
                || call.billing_scope != billing_scope
                || call.call_id != call_id
            {
                return Err(ReadFailure::InvalidSnapshot("qualified_call_identity"));
            }
            // Inspect every known actual slot before freeze can return unavailable.
            // A valid redaction in one slot cannot conceal another corrupt slot.
            for artifact in [
                call.request_artifact.as_ref(),
                call.transport_artifact.as_ref(),
                call.response_artifact.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                check_actual_artifact(artifact)?;
                if artifact.schema_version == "rsia.redacted.v1" {
                    checked_redaction(artifact)?;
                }
            }
            Ok(Self { call })
        }

        fn artifact(&self, slot: Slot) -> Option<&'a BudgetArtifact> {
            match slot {
                Slot::Request => self.call.request_artifact.as_ref(),
                Slot::Transport => self.call.transport_artifact.as_ref(),
                Slot::Response => self.call.response_artifact.as_ref(),
            }
        }

        fn freeze_artifact(&self, slot: Slot) -> SnapshotResult<Option<ArtifactRef>> {
            let Some(artifact) = self.artifact(slot) else {
                return Ok(None);
            };
            check_actual_artifact(artifact)?;
            if artifact.schema_version == "rsia.redacted.v1" {
                // Only a fully validated existing five-field redaction is an
                // unavailable original. A label or malformed body is invalid.
                checked_redaction(artifact)?;
                return Err(ReadFailure::SourceInsufficient(
                    "artifact_original_redacted",
                ));
            }
            let body_bytes = u64::try_from(artifact.body.len())
                .map_err(|_| ReadFailure::SourceInsufficient("artifact_byte_length"))?;
            Ok(Some(ArtifactRef {
                namespace: self.call.namespace.clone(),
                billing_scope: self.call.billing_scope.clone(),
                call_id: self.call.call_id.clone(),
                slot,
                schema_version: artifact.schema_version.clone(),
                digest: artifact.digest.clone(),
                body_bytes,
            }))
        }
    }

    // The exact original 32 BudgetCallRecord fields retain their names and order;
    // all 29 non-artifact values are stored, and ONLY the three body slots change
    // type. Original fields are not derived from current paid/usage overlays.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(super) struct FrozenCall {
        schema_version: String,
        original_call_digest: String,
        billing_scope: String,
        call_id: String,
        dispatch_group_id: String,
        namespace: String,
        stage: BudgetStage,
        actual_input_digest: String,
        request_artifact: Option<ArtifactRef>,
        reserved_micros: i64,
        state: BudgetCallState,
        lease_token: String,
        lease_epoch: i64,
        lease_until: i64,
        dispatch_id: Option<String>,
        provider_request_id: Option<String>,
        usage_record_id: Option<String>,
        output_digest: Option<String>,
        actual_cost_micros: Option<i64>,
        actual_currency: Option<String>,
        actual_pricing_version: Option<String>,
        execution_provenance: Option<BudgetExecutionProvenance>,
        actual_model_digest: Option<String>,
        transport_artifact: Option<ArtifactRef>,
        response_artifact: Option<ArtifactRef>,
        response_usable: Option<bool>,
        response_block_reason: Option<String>,
        execution_closed: bool,
        execution_closed_at: Option<i64>,
        execution_close_reason: Option<String>,
        terminal_reason: Option<String>,
        created_at: i64,
        dispatched_at: Option<i64>,
        finalized_at: Option<i64>,
    }

    fn check_digest(value: &str) -> SnapshotResult<()> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ReadFailure::InvalidSnapshot("digest_format"));
        }
        Ok(())
    }

    fn check_actual_artifact(artifact: &BudgetArtifact) -> SnapshotResult<()> {
        if artifact.body.len() > MAX_ARTIFACT_BYTES {
            return Err(ReadFailure::InvalidSnapshot("actual_artifact_capacity"));
        }
        // Existing generic BudgetArtifact validation, on original raw UTF8:
        // identifier, SHA256, JSON and 4MiB. Never downgrade a bad actual body.
        artifact
            .validate()
            .map_err(|_| ReadFailure::InvalidSnapshot("actual_artifact_invalid"))
    }

    // Exact existing budget redaction representation, mirroring the private
    // typed_budget::BudgetRedaction type rather than accepting a schema label.
    // Only redactions use the existing strict parser; ordinary historical
    // BudgetArtifact JSON keeps the original generic validation rule above.
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct BudgetRedaction {
        schema_version: String,
        state: String,
        reason: String,
        original_schema: String,
        original_digest: String,
    }

    fn checked_redaction(artifact: &BudgetArtifact) -> SnapshotResult<BudgetRedaction> {
        // Callers must validate actual raw hash/JSON/capacity before this step.
        let value = parse_owned_json(&artifact.body)
            .map_err(|_| ReadFailure::InvalidSnapshot("redaction_json"))?;
        let redaction: BudgetRedaction = serde_json::from_value(value)
            .map_err(|_| ReadFailure::InvalidSnapshot("redaction_fields"))?;
        if artifact.schema_version != "rsia.redacted.v1"
            || redaction.schema_version != artifact.schema_version
            || redaction.state != "source_revoked"
            || evo_core::text(&redaction.reason, "reason", 512).is_err()
            || evo_core::identifier(&redaction.original_schema).is_err()
        {
            return Err(ReadFailure::InvalidSnapshot("redaction_shape"));
        }
        check_digest(&redaction.original_digest)?;
        Ok(redaction)
    }

    enum ArtifactInspection<'a> {
        FrozenNone,
        Original(&'a BudgetArtifact),
        Unavailable(&'static str),
    }

    struct ResolvedArtifacts<'a> {
        request: Option<&'a BudgetArtifact>,
        transport: Option<&'a BudgetArtifact>,
        response: Option<&'a BudgetArtifact>,
    }

    impl FrozenCall {
        pub(super) fn math_projection_r39(&self) -> SnapshotResult<E05MathCallR39> {
            self.validate_frozen_target(&self.namespace, &self.billing_scope, &self.call_id)?;
            Ok(E05MathCallR39 {
                billing_scope: self.billing_scope.clone(),
                call_id: self.call_id.clone(),
                dispatch_group_id: self.dispatch_group_id.clone(),
                namespace: self.namespace.clone(),
                stage: self.stage,
                actual_input_digest: self.actual_input_digest.clone(),
                state: self.state,
                output_digest: self.output_digest.clone(),
                actual_cost_micros: self.actual_cost_micros,
                actual_currency: self.actual_currency.clone(),
                actual_pricing_version: self.actual_pricing_version.clone(),
                execution_closed: self.execution_closed,
                dispatch_id: self.dispatch_id.clone(),
                dispatched_at: self.dispatched_at,
                finalized_at: self.finalized_at,
                execution_closed_at: self.execution_closed_at,
            })
        }

        pub(super) fn freeze_from_qualified_snapshot(
            snapshot: &OccurrenceSnapshot,
            namespace: &str,
            billing_scope: &str,
            call_id: &str,
        ) -> SnapshotResult<Self> {
            let source = QualifiedCallSource::from_qualified_snapshot(
                snapshot,
                namespace,
                billing_scope,
                call_id,
            )?;
            Self::freeze(&source)
        }

        pub(super) fn reconstruct_from_qualified_snapshot(
            &self,
            snapshot: &OccurrenceSnapshot,
            namespace: &str,
            billing_scope: &str,
            call_id: &str,
        ) -> SnapshotResult<BudgetCallRecord> {
            // All known DTO/ref contradictions are checked BEFORE a lookup can
            // return missing. The caller's target must also match this snapshot.
            self.validate_saved_descriptors()?;
            if self.namespace != namespace
                || self.billing_scope != billing_scope
                || self.call_id != call_id
            {
                return Err(ReadFailure::InvalidSnapshot("requested_call_identity"));
            }
            let source = QualifiedCallSource::from_qualified_snapshot(
                snapshot,
                namespace,
                billing_scope,
                call_id,
            )?;
            self.reconstruct(&source)
        }

        fn freeze(source: &QualifiedCallSource<'_>) -> SnapshotResult<Self> {
            let call = source.call;
            let original_call_digest = evo_core::fingerprint(call)
                .map_err(|_| ReadFailure::InvalidSnapshot("call_serialization"))?;
            check_digest(&original_call_digest)?;
            let frozen = Self {
                schema_version: SCHEMA.into(),
                original_call_digest,
                billing_scope: call.billing_scope.clone(),
                call_id: call.call_id.clone(),
                dispatch_group_id: call.dispatch_group_id.clone(),
                namespace: call.namespace.clone(),
                stage: call.stage,
                actual_input_digest: call.actual_input_digest.clone(),
                request_artifact: source.freeze_artifact(Slot::Request)?,
                reserved_micros: call.reserved_micros,
                state: call.state,
                lease_token: call.lease_token.clone(),
                lease_epoch: call.lease_epoch,
                lease_until: call.lease_until,
                dispatch_id: call.dispatch_id.clone(),
                provider_request_id: call.provider_request_id.clone(),
                usage_record_id: call.usage_record_id.clone(),
                output_digest: call.output_digest.clone(),
                actual_cost_micros: call.actual_cost_micros,
                actual_currency: call.actual_currency.clone(),
                actual_pricing_version: call.actual_pricing_version.clone(),
                execution_provenance: call.execution_provenance,
                actual_model_digest: call.actual_model_digest.clone(),
                transport_artifact: source.freeze_artifact(Slot::Transport)?,
                response_artifact: source.freeze_artifact(Slot::Response)?,
                response_usable: call.response_usable,
                response_block_reason: call.response_block_reason.clone(),
                execution_closed: call.execution_closed,
                execution_closed_at: call.execution_closed_at,
                execution_close_reason: call.execution_close_reason.clone(),
                terminal_reason: call.terminal_reason.clone(),
                created_at: call.created_at,
                dispatched_at: call.dispatched_at,
                finalized_at: call.finalized_at,
            };
            // This verifies the concrete inverse against the SAME borrowed source
            // before returning data. It still does not mint a native/as-of permit.
            let reconstructed = frozen.reconstruct(source)?;
            if reconstructed != *call {
                return Err(ReadFailure::InvalidSnapshot("freeze_inverse"));
            }
            Ok(frozen)
        }

        fn require_same_call(&self, source: &QualifiedCallSource<'_>) -> SnapshotResult<()> {
            let actual = source.call;
            // These identities never change in the existing reserve, dispatch,
            // settle, reconcile, close, refence or redaction APIs. Mutable state,
            // lease, financial and closure fields deliberately are NOT compared.
            if self.namespace != actual.namespace
                || self.billing_scope != actual.billing_scope
                || self.call_id != actual.call_id
                || self.dispatch_group_id != actual.dispatch_group_id
                || self.stage != actual.stage
                || self.actual_input_digest != actual.actual_input_digest
                || self.reserved_micros != actual.reserved_micros
                || self.created_at != actual.created_at
            {
                return Err(ReadFailure::InvalidSnapshot("actual_call_identity"));
            }
            Ok(())
        }

        fn validate_reference(
            &self,
            slot: Slot,
            reference: Option<&ArtifactRef>,
        ) -> SnapshotResult<()> {
            let Some(reference) = reference else {
                return Ok(());
            };
            if reference.namespace != self.namespace
                || reference.billing_scope != self.billing_scope
                || reference.call_id != self.call_id
                || reference.slot != slot
            {
                return Err(ReadFailure::InvalidSnapshot("artifact_slot_identity"));
            }
            if evo_core::identifier(&reference.schema_version).is_err()
                || reference.schema_version == "rsia.redacted.v1"
            {
                return Err(ReadFailure::InvalidSnapshot("artifact_reference_schema"));
            }
            check_digest(&reference.digest)?;
            // No valid JSON body is empty. This contradiction is known even if
            // its actual original has since disappeared or been redacted.
            if reference.body_bytes == 0 || reference.body_bytes > MAX_ARTIFACT_BYTES as u64 {
                return Err(ReadFailure::InvalidSnapshot("artifact_reference_capacity"));
            }
            Ok(())
        }

        pub(super) fn validate_original_copy_r36(
            &self,
            original: &BudgetCallRecord,
        ) -> SnapshotResult<()> {
            self.validate_saved_descriptors()?;
            if original.namespace != self.namespace
                || original.billing_scope != self.billing_scope
                || original.call_id != self.call_id
            {
                return Err(ReadFailure::InvalidSnapshot(
                    "resource_call_original_identity",
                ));
            }
            for artifact in [
                &original.request_artifact,
                &original.transport_artifact,
                &original.response_artifact,
            ]
            .into_iter()
            .flatten()
            {
                check_actual_artifact(artifact)?;
            }
            let digest = evo_core::fingerprint(original)
                .map_err(|_| ReadFailure::InvalidSnapshot("resource_call_serialization"))?;
            if digest != self.original_call_digest {
                return Err(ReadFailure::InvalidSnapshot(
                    "resource_call_original_digest",
                ));
            }
            Ok(())
        }

        pub(super) fn validate_ticket_binding_r36(
            &self,
            namespace: &str,
            ticket_id: &str,
        ) -> SnapshotResult<()> {
            self.validate_saved_descriptors()?;
            if self.namespace != namespace || self.dispatch_group_id != ticket_id {
                return Err(ReadFailure::InvalidSnapshot(
                    "snapshot_ticket_group_binding",
                ));
            }
            Ok(())
        }

        fn validate_saved_descriptors(&self) -> SnapshotResult<()> {
            self.validate_frozen_29()?;
            if self.schema_version != SCHEMA {
                return Err(ReadFailure::InvalidSnapshot("snapshot_schema"));
            }
            check_digest(&self.original_call_digest)?;
            for identity in [&self.namespace, &self.billing_scope, &self.call_id] {
                if evo_core::identifier(identity).is_err() {
                    return Err(ReadFailure::InvalidSnapshot(
                        "snapshot_call_identity_format",
                    ));
                }
            }
            // No SourceInsufficient path occurs until ALL saved refs are checked.
            self.validate_reference(Slot::Request, self.request_artifact.as_ref())?;
            self.validate_reference(Slot::Transport, self.transport_artifact.as_ref())?;
            self.validate_reference(Slot::Response, self.response_artifact.as_ref())?;
            Ok(())
        }

        fn inspect_artifact<'a>(
            &self,
            source: &QualifiedCallSource<'a>,
            slot: Slot,
            reference: Option<&ArtifactRef>,
        ) -> SnapshotResult<ArtifactInspection<'a>> {
            let Some(reference) = reference else {
                // A frozen None is never filled from a later Some.
                return Ok(ArtifactInspection::FrozenNone);
            };
            // Descriptors are all prevalidated, so neither missing actual data
            // nor a redaction can hide a known tuple/slot/schema/length error.
            self.validate_reference(slot, Some(reference))?;
            let Some(artifact) = source.artifact(slot) else {
                return Ok(ArtifactInspection::Unavailable("artifact_original_missing"));
            };
            check_actual_artifact(artifact)?;
            if artifact.schema_version == "rsia.redacted.v1" {
                let redaction = checked_redaction(artifact)?;
                // Match the same saved original, not the current redaction's
                // digest, and never treat original_digest as recoverable content.
                if redaction.original_schema != reference.schema_version
                    || redaction.original_digest != reference.digest
                {
                    return Err(ReadFailure::InvalidSnapshot("redaction_original_binding"));
                }
                return Ok(ArtifactInspection::Unavailable(
                    "artifact_original_redacted",
                ));
            }
            let body_bytes = u64::try_from(artifact.body.len())
                .map_err(|_| ReadFailure::InvalidSnapshot("actual_artifact_byte_length"))?;
            if artifact.schema_version != reference.schema_version
                || artifact.digest != reference.digest
                || body_bytes != reference.body_bytes
            {
                return Err(ReadFailure::InvalidSnapshot(
                    "artifact_original_reference_mismatch",
                ));
            }
            Ok(ArtifactInspection::Original(artifact))
        }

        fn preflight_artifacts<'a>(
            &self,
            source: &QualifiedCallSource<'a>,
        ) -> SnapshotResult<ResolvedArtifacts<'a>> {
            // An unavailable slot is DATA here, not an early error: inspect all
            // later actual slots before classifying anything as source missing.
            // Thus a missing request cannot hide a forged transport/response.
            let inspected = [
                self.inspect_artifact(source, Slot::Request, self.request_artifact.as_ref())?,
                self.inspect_artifact(source, Slot::Transport, self.transport_artifact.as_ref())?,
                self.inspect_artifact(source, Slot::Response, self.response_artifact.as_ref())?,
            ];
            let mut bodies = [None, None, None];
            let mut unavailable = None;
            for (index, inspection) in inspected.into_iter().enumerate() {
                match inspection {
                    ArtifactInspection::FrozenNone => {}
                    ArtifactInspection::Original(artifact) => bodies[index] = Some(artifact),
                    ArtifactInspection::Unavailable(reason) => {
                        if unavailable.is_none() {
                            unavailable = Some(reason);
                        }
                    }
                }
            }
            if let Some(reason) = unavailable {
                return Err(ReadFailure::SourceInsufficient(reason));
            }
            Ok(ResolvedArtifacts {
                request: bodies[0],
                transport: bodies[1],
                response: bodies[2],
            })
        }

        fn reconstruct(
            &self,
            source: &QualifiedCallSource<'_>,
        ) -> SnapshotResult<BudgetCallRecord> {
            self.validate_saved_descriptors()?;
            self.require_same_call(source)?;
            let artifacts = self.preflight_artifacts(source)?;
            let original = BudgetCallRecord {
                billing_scope: self.billing_scope.clone(),
                call_id: self.call_id.clone(),
                dispatch_group_id: self.dispatch_group_id.clone(),
                namespace: self.namespace.clone(),
                stage: self.stage,
                actual_input_digest: self.actual_input_digest.clone(),
                request_artifact: artifacts.request.cloned(),
                reserved_micros: self.reserved_micros,
                state: self.state,
                lease_token: self.lease_token.clone(),
                lease_epoch: self.lease_epoch,
                lease_until: self.lease_until,
                dispatch_id: self.dispatch_id.clone(),
                provider_request_id: self.provider_request_id.clone(),
                usage_record_id: self.usage_record_id.clone(),
                output_digest: self.output_digest.clone(),
                actual_cost_micros: self.actual_cost_micros,
                actual_currency: self.actual_currency.clone(),
                actual_pricing_version: self.actual_pricing_version.clone(),
                execution_provenance: self.execution_provenance,
                actual_model_digest: self.actual_model_digest.clone(),
                transport_artifact: artifacts.transport.cloned(),
                response_artifact: artifacts.response.cloned(),
                response_usable: self.response_usable,
                response_block_reason: self.response_block_reason.clone(),
                execution_closed: self.execution_closed,
                execution_closed_at: self.execution_closed_at,
                execution_close_reason: self.execution_close_reason.clone(),
                terminal_reason: self.terminal_reason.clone(),
                created_at: self.created_at,
                dispatched_at: self.dispatched_at,
                finalized_at: self.finalized_at,
            };
            let digest = evo_core::fingerprint(&original)
                .map_err(|_| ReadFailure::InvalidSnapshot("call_serialization"))?;
            if digest != self.original_call_digest {
                return Err(ReadFailure::InvalidSnapshot(
                    "original_call_digest_mismatch",
                ));
            }
            Ok(original)
        }
    }
    impl FrozenCall {
        // Static validation mirrors collect_budget; no later actual row supplies
        // any frozen business value, and no lease-freshness or cost cap is added.
        fn validate_frozen_29(&self) -> SnapshotResult<()> {
            let invalid = |reason| ReadFailure::InvalidSnapshot(reason);
            for value in [
                &self.namespace,
                &self.billing_scope,
                &self.call_id,
                &self.dispatch_group_id,
                &self.lease_token,
            ] {
                evo_core::identifier(value).map_err(|_| invalid("frozen_identity_format"))?;
            }
            evo_core::identifier(self.stage.as_str()).map_err(|_| invalid("frozen_stage"))?;
            check_digest(&self.actual_input_digest)?;
            for value in [&self.dispatch_id, &self.usage_record_id]
                .into_iter()
                .flatten()
            {
                evo_core::identifier(value).map_err(|_| invalid("frozen_optional_identity"))?;
            }
            for value in [&self.output_digest, &self.actual_model_digest]
                .into_iter()
                .flatten()
            {
                check_digest(value)?;
            }
            if self.reserved_micros <= 0
                || self.lease_epoch <= 0
                || self.lease_until < 0
                || self.created_at < 0
                || self.actual_cost_micros.is_some_and(|v| v < 0)
                || [
                    self.execution_closed_at,
                    self.dispatched_at,
                    self.finalized_at,
                ]
                .into_iter()
                .flatten()
                .any(|v| v < 0)
            {
                return Err(invalid("frozen_numeric_shape"));
            }
            // Other optional strings, bools and closed enums have already been
            // decoded into their actual original field types. Do not invent a
            // paid/lease/current-row predicate for those frozen values.
            if !self.execution_closed {
                if self.execution_closed_at.is_some() || self.execution_close_reason.is_some() {
                    return Err(invalid("frozen_open_execution"));
                }
            } else if self.dispatch_id.is_some()
                && (self.execution_closed_at.is_none() || self.execution_close_reason.is_none())
            {
                return Err(invalid("frozen_closed_execution"));
            }
            match self.state {
                BudgetCallState::Reserved => {
                    if self.dispatch_id.is_some()
                        || self.dispatched_at.is_some()
                        || !self.execution_closed
                    {
                        return Err(invalid("frozen_reserved_phase"));
                    }
                }
                BudgetCallState::Dispatched | BudgetCallState::Uncertain => {
                    if self.dispatch_id.is_none() || self.dispatched_at.is_none() {
                        return Err(invalid("frozen_dispatched_phase"));
                    }
                }
                BudgetCallState::Finalized => {
                    if self.dispatch_id.is_none()
                        || self.dispatched_at.is_none()
                        || self.finalized_at.is_none()
                    {
                        return Err(invalid("frozen_finalized_phase"));
                    }
                }
                BudgetCallState::Released | BudgetCallState::Cancelled => {
                    if self.dispatch_id.is_some() || self.dispatched_at.is_some() {
                        return Err(invalid("frozen_undispatched_terminal_phase"));
                    }
                }
            }
            Ok(())
        }
        pub(super) fn validate_frozen_target(
            &self,
            namespace: &str,
            billing_scope: &str,
            call_id: &str,
        ) -> SnapshotResult<()> {
            self.validate_saved_descriptors()?;
            if self.namespace != namespace
                || self.billing_scope != billing_scope
                || self.call_id != call_id
            {
                return Err(ReadFailure::InvalidSnapshot("frozen_target_identity"));
            }
            Ok(())
        }
        pub(super) fn observe_saved_times(
            &self,
            floor: &mut super::OccurrenceFloor,
        ) -> SnapshotResult<()> {
            self.validate_saved_descriptors()?;
            for value in [
                Some(self.created_at),
                self.dispatched_at,
                self.finalized_at,
                self.execution_closed_at,
            ]
            .into_iter()
            .flatten()
            {
                floor
                    .observe_seconds(value)
                    .map_err(|_| ReadFailure::InvalidSnapshot("frozen_time"))?;
            }
            Ok(())
        }
    }

    #[cfg(test)]
    mod input2_ticket_binding_controls_r36 {
        use super::*;
        fn call() -> BudgetCallRecord {
            BudgetCallRecord {
                billing_scope: "scope".into(),
                call_id: "call".into(),
                dispatch_group_id: "group".into(),
                namespace: "n".into(),
                stage: BudgetStage::FormalEvaluation,
                actual_input_digest: evo_core::hash(b"input"),
                request_artifact: None,
                reserved_micros: 10,
                state: BudgetCallState::Reserved,
                lease_token: "lease".into(),
                lease_epoch: 1,
                lease_until: 0,
                dispatch_id: None,
                provider_request_id: None,
                usage_record_id: None,
                output_digest: None,
                actual_cost_micros: None,
                actual_currency: None,
                actual_pricing_version: None,
                execution_provenance: None,
                actual_model_digest: None,
                transport_artifact: None,
                response_artifact: None,
                response_usable: None,
                response_block_reason: None,
                execution_closed: true,
                execution_closed_at: None,
                execution_close_reason: None,
                terminal_reason: None,
                created_at: 1,
                dispatched_at: None,
                finalized_at: None,
            }
        }

        #[test]
        fn original_copy_known_mismatch_precedes_actual_call_missing_r36_r2() {
            let original = call();
            let mut snapshot = OccurrenceSnapshot::default();
            snapshot
                .calls
                .insert(("scope".into(), "call".into()), original.clone());
            let saved = FrozenCall::freeze_from_qualified_snapshot(&snapshot, "n", "scope", "call")
                .unwrap();
            saved.validate_original_copy_r36(&original).unwrap();
            snapshot.calls.clear();
            let mut changed = original.clone();
            changed.reserved_micros += 1;
            assert!(matches!(
                saved.validate_original_copy_r36(&changed),
                Err(ReadFailure::InvalidSnapshot(
                    "resource_call_original_digest"
                ))
            ));
            changed = original.clone();
            changed.namespace = "other".into();
            assert!(matches!(
                saved.validate_original_copy_r36(&changed),
                Err(ReadFailure::InvalidSnapshot(
                    "resource_call_original_identity"
                ))
            ));
            assert!(matches!(
                saved.reconstruct_from_qualified_snapshot(&snapshot, "n", "scope", "call",),
                Err(ReadFailure::SourceInsufficient(_))
            ));
            saved.validate_original_copy_r36(&original).unwrap();
        }

        #[test]
        fn saved_original_ticket_group_is_known_without_current_call_or_bodies_r36() {
            let actual = call();
            let mut snapshot = OccurrenceSnapshot::default();
            snapshot
                .calls
                .insert(("scope".into(), "call".into()), actual);
            let saved = FrozenCall::freeze_from_qualified_snapshot(&snapshot, "n", "scope", "call")
                .unwrap();
            saved.validate_ticket_binding_r36("n", "group").unwrap();
            for (field, bad) in [
                ("dispatch_group_id", "other-ticket"),
                ("namespace", "other-namespace"),
            ] {
                let mut value = serde_json::to_value(&saved).unwrap();
                value[field] = serde_json::json!(bad);
                let forged: FrozenCall = serde_json::from_value(value).unwrap();
                assert!(matches!(
                    forged.validate_ticket_binding_r36("n", "group"),
                    Err(ReadFailure::InvalidSnapshot(
                        "snapshot_ticket_group_binding"
                    ))
                ));
            }
            // Source removal supplies no authority and cannot mask the saved
            // namespace/group contradiction. The original object is unchanged.
            snapshot.calls.clear();
            saved.validate_ticket_binding_r36("n", "group").unwrap();
            assert!(matches!(
                saved.reconstruct_from_qualified_snapshot(&snapshot, "n", "scope", "call",),
                Err(ReadFailure::SourceInsufficient(_))
            ));
            assert!(
                saved.request_artifact.is_none()
                    && saved.transport_artifact.is_none()
                    && saved.response_artifact.is_none()
            );
        }
    }

    #[cfg(test)]
    mod r24_static_controls {
        use super::*;
        fn call() -> BudgetCallRecord {
            BudgetCallRecord {
                billing_scope: "scope".into(),
                call_id: "call".into(),
                dispatch_group_id: "group".into(),
                namespace: "n".into(),
                stage: BudgetStage::FormalEvaluation,
                actual_input_digest: evo_core::hash(b"input"),
                request_artifact: None,
                reserved_micros: 10,
                state: BudgetCallState::Reserved,
                lease_token: "lease".into(),
                lease_epoch: 1,
                lease_until: 0,
                dispatch_id: None,
                provider_request_id: None,
                usage_record_id: None,
                output_digest: None,
                actual_cost_micros: None,
                actual_currency: None,
                actual_pricing_version: None,
                execution_provenance: None,
                actual_model_digest: None,
                transport_artifact: None,
                response_artifact: None,
                response_usable: None,
                response_block_reason: None,
                execution_closed: true,
                execution_closed_at: None,
                execution_close_reason: None,
                terminal_reason: None,
                created_at: 1,
                dispatched_at: None,
                finalized_at: None,
            }
        }
        #[test]
        fn frozen29_known_invalid_precedes_actual_missing() {
            let actual = call();
            let frozen = FrozenCall::freeze(&QualifiedCallSource { call: &actual }).unwrap();
            let missing = OccurrenceSnapshot::default();
            assert!(matches!(
                frozen.reconstruct_from_qualified_snapshot(&missing, "n", "scope", "call"),
                Err(ReadFailure::SourceInsufficient(_))
            ));
            for (field, value) in [
                ("reserved_micros", serde_json::json!(-1)),
                ("lease_epoch", serde_json::json!(0)),
                ("created_at", serde_json::json!(-1)),
                ("dispatch_group_id", serde_json::json!("bad group")),
                ("actual_cost_micros", serde_json::json!(-1)),
                ("output_digest", serde_json::json!("bad")),
                ("state", serde_json::json!("dispatched")),
            ] {
                let mut wire = serde_json::to_value(&frozen).unwrap();
                wire[field] = value;
                let broken: FrozenCall = serde_json::from_value(wire).unwrap();
                assert!(
                    matches!(
                        broken.reconstruct_from_qualified_snapshot(&missing, "n", "scope", "call"),
                        Err(ReadFailure::InvalidSnapshot(_))
                    ),
                    "known field {field} was hidden by missing source"
                );
            }
        }
        #[test]
        fn frozen29_keeps_uncertain_charge_refence_and_nonfinal_occurrence() {
            let mut original = call();
            original.state = BudgetCallState::Uncertain;
            original.dispatch_id = Some("dispatch".into());
            original.dispatched_at = Some(2);
            original.finalized_at = Some(3);
            original.execution_closed = false;
            original.actual_cost_micros = Some(50);
            original.actual_currency = Some("USD".into());
            original.actual_pricing_version = Some("price".into());
            original.provider_request_id = Some("provider request".into());
            original.usage_record_id = Some("usage".into());
            original.output_digest = Some(evo_core::hash(b"output"));
            let frozen = FrozenCall::freeze(&QualifiedCallSource { call: &original }).unwrap();
            let mut late = original.clone();
            late.lease_token = "new-lease".into();
            late.lease_epoch = 2;
            late.lease_until = 10_000;
            late.actual_cost_micros = Some(60);
            late.finalized_at = Some(20);
            late.execution_closed = true;
            late.execution_closed_at = Some(21);
            late.execution_close_reason = Some("late close".into());
            let mut snapshot = OccurrenceSnapshot::default();
            snapshot.calls.insert(("scope".into(), "call".into()), late);
            let reconstructed = frozen
                .reconstruct_from_qualified_snapshot(&snapshot, "n", "scope", "call")
                .unwrap();
            assert_eq!(reconstructed, original);
            assert_eq!(
                evo_core::fingerprint(&reconstructed).unwrap(),
                frozen.original_call_digest
            );
            assert!(reconstructed.request_artifact.is_none());
            assert!(reconstructed.transport_artifact.is_none());
            assert!(reconstructed.response_artifact.is_none());
        }
    }

    #[cfg(test)]
    mod r39_native_frozen_slot_controls {
        use super::*;
        use crate::owned_occurrence::typed_fingerprint;
        use evo_core::hash;
        fn call() -> BudgetCallRecord {
            BudgetCallRecord {
                billing_scope: "scope".into(),
                call_id: "call".into(),
                dispatch_group_id: "group".into(),
                namespace: "n".into(),
                stage: BudgetStage::FormalEvaluation,
                actual_input_digest: evo_core::hash(b"input"),
                request_artifact: None,
                reserved_micros: 10,
                state: BudgetCallState::Reserved,
                lease_token: "lease".into(),
                lease_epoch: 1,
                lease_until: 0,
                dispatch_id: None,
                provider_request_id: None,
                usage_record_id: None,
                output_digest: None,
                actual_cost_micros: None,
                actual_currency: None,
                actual_pricing_version: None,
                execution_provenance: None,
                actual_model_digest: None,
                transport_artifact: None,
                response_artifact: None,
                response_usable: None,
                response_block_reason: None,
                execution_closed: true,
                execution_closed_at: None,
                execution_close_reason: None,
                terminal_reason: None,
                created_at: 1,
                dispatched_at: None,
                finalized_at: None,
            }
        }

        #[test]
        fn r39_frozen_none_never_fills_from_later_slot_and_keeps_original32() {
            let original = call();
            let frozen = FrozenCall::freeze(&QualifiedCallSource { call: &original }).unwrap();
            let body = "{}".to_owned();
            let mut latest = original.clone();
            latest.request_artifact = Some(BudgetArtifact {
                schema_version: "rsia.fixture.request.v1".into(),
                digest: hash(body.as_bytes()),
                body,
            });
            let mut snapshot = OccurrenceSnapshot::default();
            snapshot
                .calls
                .insert(("scope".into(), "call".into()), latest);
            let reconstructed = frozen
                .reconstruct_from_qualified_snapshot(&snapshot, "n", "scope", "call")
                .unwrap();
            assert!(reconstructed.request_artifact.is_none());
            assert_eq!(reconstructed, original);
            assert_eq!(
                typed_fingerprint(&reconstructed).unwrap(),
                frozen.original_call_digest
            );
        }

        #[test]
        fn r39_missing_and_real_redacted_slot_are_source_gaps_but_wrong_binding_is_invalid() {
            let body = "{\"answer\":1}".to_owned();
            let artifact = BudgetArtifact {
                schema_version: "rsia.fixture.request.v1".into(),
                digest: hash(body.as_bytes()),
                body,
            };
            let mut original = call();
            original.request_artifact = Some(artifact.clone());
            let frozen = FrozenCall::freeze(&QualifiedCallSource { call: &original }).unwrap();
            let mut latest = original.clone();
            latest.request_artifact = None;
            let mut snapshot = OccurrenceSnapshot::default();
            snapshot
                .calls
                .insert(("scope".into(), "call".into()), latest.clone());
            assert!(matches!(
                frozen.reconstruct_from_qualified_snapshot(&snapshot, "n", "scope", "call"),
                Err(ReadFailure::SourceInsufficient(_))
            ));
            let redacted = serde_json::json!({
                "schema_version":"rsia.redacted.v1","state":"source_revoked",
                "reason":"revoked source","original_schema":artifact.schema_version,
                "original_digest":artifact.digest
            });
            let body = serde_json::to_string(&redacted).unwrap();
            latest.request_artifact = Some(BudgetArtifact {
                schema_version: "rsia.redacted.v1".into(),
                digest: hash(body.as_bytes()),
                body,
            });
            snapshot
                .calls
                .insert(("scope".into(), "call".into()), latest.clone());
            assert!(matches!(
                frozen.reconstruct_from_qualified_snapshot(&snapshot, "n", "scope", "call"),
                Err(ReadFailure::SourceInsufficient(_))
            ));
            let mut wrong = redacted;
            wrong["original_digest"] = serde_json::json!(hash(b"other-original"));
            let body = serde_json::to_string(&wrong).unwrap();
            latest.request_artifact = Some(BudgetArtifact {
                schema_version: "rsia.redacted.v1".into(),
                digest: hash(body.as_bytes()),
                body,
            });
            snapshot
                .calls
                .insert(("scope".into(), "call".into()), latest);
            assert!(matches!(
                frozen.reconstruct_from_qualified_snapshot(&snapshot, "n", "scope", "call"),
                Err(ReadFailure::InvalidSnapshot(_))
            ));
        }

        #[test]
        fn r39_math_projection_does_not_claim_missing_original_artifact_or_erase_bad_later_slot() {
            let body = "{}".to_owned();
            let artifact = BudgetArtifact {
                schema_version: "rsia.fixture.request.v1".into(),
                digest: hash(body.as_bytes()),
                body,
            };
            let mut original = call();
            original.request_artifact = Some(artifact.clone());
            original.response_artifact = Some(artifact);
            let frozen = FrozenCall::freeze(&QualifiedCallSource { call: &original }).unwrap();
            let math = frozen.math_projection_r39().unwrap();
            assert_eq!(math.call_id, original.call_id);
            assert_eq!(math.actual_cost_micros, original.actual_cost_micros);
            let mut latest = original.clone();
            latest.request_artifact = None;
            latest.response_artifact.as_mut().unwrap().digest = hash(b"bad-actual-digest");
            let mut snapshot = OccurrenceSnapshot::default();
            snapshot
                .calls
                .insert(("scope".into(), "call".into()), latest);
            assert!(matches!(
                frozen.reconstruct_from_qualified_snapshot(&snapshot, "n", "scope", "call"),
                Err(ReadFailure::InvalidSnapshot(_))
            ));
        }
    }
}
// AG132_R19_R1_BUDGET_CALL_SNAPSHOT_SOURCE_END

// Private, original-native-operation recomputation only. This is NOT a historical
// read owner or a Current capability. Old V3/input-v1 carriers do not retain the
// original ExposureEntry or full episode-root inputs; this module never substitutes
// a later ledger, mutable ticket or paid overlay for those absent historical facts.
mod frozen_complete_kernel_r29 {
    use super::*;

    // No Clone, Serde, public fields, callback, arbitrary pending ID or DTO ctor.
    // Only verify_before_commit below can construct this, after actual native ACKs,
    // a same-connection load and the unchanged complete Stable admission.
    struct OriginalCapture {
        raw: RawDatabase,
        snapshot: OccurrenceSnapshot,
        carrier: FrozenCarrierR36,
        reference: evo_core::formal_version::ReportRef,
    }

    struct AvailableCompleteInputsR36<'a> {
        raw: &'a RawDatabase,
        snapshot: &'a OccurrenceSnapshot,
        carrier: &'a FrozenCarrierR36,
        reference: evo_core::formal_version::ReportRef,
    }

    fn invalid(code: &'static str) -> Error {
        Error::Conflict(format!("formal_invalid_recomputed_complete:r29:{code}"))
    }
    fn missing(codes: &BTreeSet<&'static str>) -> Error {
        Error::Conflict(format!(
            "formal_source_insufficient:r29:{}",
            codes.iter().copied().collect::<Vec<_>>().join(",")
        ))
    }
    fn check(condition: bool, code: &'static str) -> Result<()> {
        if condition {
            Ok(())
        } else {
            Err(invalid(code))
        }
    }
    #[cfg(test)]
    fn require_sources(codes: &BTreeSet<&'static str>) -> Result<()> {
        if codes.is_empty() {
            Ok(())
        } else {
            Err(missing(codes))
        }
    }

    // A failed full Stable qualification still poisons this exact operation.
    // Keep its typed formal-data classification; a missing source or known bad
    // identity is not a newly sampled wall-clock regression. No string routing.
    fn qualified_floor_or_poison_r3(
        clock: &mut crate::write_clock::OperationClock,
        qualified: Result<OccurrenceFloor>,
    ) -> Result<OccurrenceFloor> {
        match qualified {
            Ok(floor) => Ok(floor),
            Err(error) => {
                let clock_error = clock.fail();
                match error {
                    error @ (Error::Invalid(_) | Error::Conflict(_)) => Err(error),
                    _ => Err(clock_error),
                }
            }
        }
    }

    // This entry is called by the existing native owner before its single COMMIT
    // admission CAS. It returns unit, never a serializable authority or read result.
    async fn verify_early_before_commit_r44(
        permit: &NativeAsOfPermit,
        session: &mut Session,
        ctx: &Context,
        plan: &OwnedOperationPlan,
        ready: &NativeReadyToCommitR1,
    ) -> Result<()> {
        if plan.actual_effect_r1()? != NativePlanEffectR1::NewEarlyR44 {
            return Ok(());
        }
        session.clock.check()?;
        check_cancel(&permit.gate)?;
        permit.same_session(session, true)?;
        ctx.require(&[Role::Evaluator])?;
        check(
            permit.namespace == ctx.namespace()
                && permit.actor == ctx.actor()
                && permit.role == ctx.role()
                && matches!(
                    &permit.operation,
                    NativeAsOfOperation::Grade | NativeAsOfOperation::Reconcile
                )
                && permit.actual_effect_r1 == Some(NativePlanEffectR1::NewEarlyR44)
                && permit.facts.load(Ordering::Acquire) == 0,
            "native_operation_identity",
        )?;
        let inserted = plan
            .complete_insert_ack_r24()?
            .ok_or_else(|| invalid("insert_ack_missing"))?;
        let reference = inserted.reference;
        // Pure known Ref contradictions precede any lookup or missing-source result.
        reference.validate_wire().map_err(r24_fault)?;
        let prepared = ready
            .binding
            .as_ref()
            .ok_or_else(|| invalid("prepared_binding_missing"))?;
        let binding = permit
            .complete_ack_r24
            .as_ref()
            .ok_or_else(|| invalid("binding_ack_missing"))?;
        let witness = permit
            .witness
            .as_ref()
            .ok_or_else(|| invalid("asof_ack_missing"))?;
        check(
            reference.namespace == ctx.namespace()
                && reference.owner == ctx.actor()
                && prepared.reference == reference
                && binding.reference == reference
                && prepared.asof_sequence == witness.seq
                && prepared.binding_sequence == binding.sequence
                && prepared.binding_digest == binding.digest
                && binding.asof_sequence == witness.seq
                && permit.acknowledged_writes == plan.native_next()
                && plan.native_next() == plan.writes().len(),
            "actual_ack_identity",
        )?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        require(
            tx.open
                && tx.is_in_transaction()
                && SqliteTransactionManager::get_transaction_depth(&**tx) == 1,
        )?;
        let raw = load_sqlx_database(tx)
            .await
            .map_err(|_| session.clock.fail())?;
        same_database(&raw, plan.native_expected()).map_err(|_| session.clock.fail())?;
        let qualified_floor = collect_raw(&raw, &session.prepare_inputs, SchemaRead::Stable);
        let qualified_floor = qualified_floor_or_poison_r3(&mut session.clock, qualified_floor);
        if matches!(&qualified_floor, Err(Error::ClockRegressed)) {
            return Err(Error::ClockRegressed);
        }
        // Typed data failures stay poisoned and unsatisfied. Inspect the actual
        // known data below before choosing a missing-source result; this performs
        // no SQL, time sampling or writes and cannot turn this floor into Ok.
        check(
            permit.closed_non_audit_digest.as_deref()
                == Some(native_database_digest(&raw, false)?.as_str()),
            "original_closed_prefix",
        )?;
        let actual_binding = actual_audit_row_r24(&raw, ctx.namespace(), binding.sequence)?;
        let actual_asof = actual_audit_row_r24(&raw, ctx.namespace(), witness.seq)?;
        check(
            native_text(actual_binding, "payload")? == binding.payload
                && native_text(actual_binding, "previous_hash")? == binding.previous
                && native_text(actual_binding, "digest")? == binding.digest
                && native_text(actual_asof, "payload")? == witness.payload
                && native_text(actual_asof, "previous_hash")? == witness.previous
                && native_text(actual_asof, "digest")? == witness.digest,
            "original_audit_ack",
        )?;
        let (carrier, actual_row_digest) = decode_complete_known_raw_r36(&raw, &reference)?;
        check(
            actual_row_digest == inserted.insert_row_digest
                && carrier.input.occurred_at_unix_ms >= permit.capture_millis,
            "actual_insert_row",
        )?;
        let (snapshot, _) = snapshot(&raw)?;
        let mut inspected = inspect_complete_native_capture_r36(&carrier, &snapshot)?;
        inspect_available_originals_r36(&raw, &snapshot, &carrier, &mut inspected.actual)?;
        for actual in native_mutable_projections_r39(
            &raw,
            &snapshot,
            &carrier.input.namespace,
            &carrier.input.ticket_snapshot,
            &carrier.input.facts,
        )? {
            if let Some(saved) = carrier
                .input
                .mutable_fact_projections
                .iter()
                .find(|saved| saved.kind == actual.kind && saved.id == actual.id)
            {
                r36_check(
                    typed_fingerprint(saved)? == typed_fingerprint(&actual)?,
                    "actual_native_projection_ack_source",
                )?;
            } else {
                inspected
                    .actual
                    .insert("frozen_native_mutable_projection_missing");
            }
        }

        validate_native_exposure_capture_ack_r2(&raw, &snapshot, &carrier, &mut inspected.actual)?;
        // Inspect every available-known Early certificate and math source before
        // choosing a typed missing floor/projection. This unit-only inspection
        // cannot clear the poisoned floor or construct a reader capability.
        authenticated_frozen_reader_r39::inspect_known_early_before_source_gate_r44(
            &raw, &snapshot, ctx, &reference,
        )?;
        // The original full Stable floor must have succeeded before a capture,
        // stamp or COMMIT admission. A deferred typed error is never cleared.
        let floor = qualified_floor?;
        inspected.finish()?;
        let origin = snapshot
            .complete_origins_r24
            .get(&(reference.namespace.clone(), reference.physical_id.clone()))
            .ok_or_else(|| invalid("actual_origin_binding"))?;
        check(origin.reference == reference, "actual_origin_reference")?;
        let capture = OriginalCapture {
            raw,
            snapshot,
            carrier,
            reference,
        };
        authenticated_frozen_reader_r39::authenticate(
            &capture.raw,
            &capture.snapshot,
            ctx,
            &capture.reference,
        )?;
        // This is a unit-returning pre-COMMIT verification, not an after-close
        // Current value exported to the caller or an authorization object.
        // The expensive pure work changes no DB state. Recheck cancellation,
        // retained same-native identity and a NEW actual wall-clock sample before
        // returning to the existing irreversible-admission CAS.
        check_cancel(&permit.gate)?;
        permit.same_session(session, true)?;
        let stamp = session.clock.seconds(&floor)?;
        session.clock.take_seconds(stamp)?;
        Ok(())
    }
    pub(super) async fn verify_before_commit(
        permit: &NativeAsOfPermit,
        session: &mut Session,
        ctx: &Context,
        plan: &OwnedOperationPlan,
        ready: &NativeReadyToCommitR1,
    ) -> Result<()> {
        if plan.actual_effect_r1()? == NativePlanEffectR1::NewEarlyR44 {
            return verify_early_before_commit_r44(permit, session, ctx, plan, ready).await;
        }
        if plan.actual_effect_r1()? != NativePlanEffectR1::NewComplete {
            return Ok(());
        }
        session.clock.check()?;
        check_cancel(&permit.gate)?;
        permit.same_session(session, true)?;
        ctx.require(&[Role::Evaluator])?;
        check(
            permit.namespace == ctx.namespace()
                && permit.actor == ctx.actor()
                && permit.role == ctx.role()
                && matches!(&permit.operation, NativeAsOfOperation::Complete)
                && permit.actual_effect_r1 == Some(NativePlanEffectR1::NewComplete)
                && permit.facts.load(Ordering::Acquire) == 0,
            "native_operation_identity",
        )?;
        let inserted = plan
            .complete_insert_ack_r24()?
            .ok_or_else(|| invalid("insert_ack_missing"))?;
        let reference = inserted.reference;
        // Pure known Ref contradictions precede any lookup or missing-source result.
        reference.validate_wire().map_err(r24_fault)?;
        let prepared = ready
            .binding
            .as_ref()
            .ok_or_else(|| invalid("prepared_binding_missing"))?;
        let binding = permit
            .complete_ack_r24
            .as_ref()
            .ok_or_else(|| invalid("binding_ack_missing"))?;
        let witness = permit
            .witness
            .as_ref()
            .ok_or_else(|| invalid("asof_ack_missing"))?;
        check(
            reference.namespace == ctx.namespace()
                && reference.owner == ctx.actor()
                && prepared.reference == reference
                && binding.reference == reference
                && prepared.asof_sequence == witness.seq
                && prepared.binding_sequence == binding.sequence
                && prepared.binding_digest == binding.digest
                && binding.asof_sequence == witness.seq
                && permit.acknowledged_writes == plan.native_next()
                && plan.native_next() == plan.writes().len(),
            "actual_ack_identity",
        )?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        require(
            tx.open
                && tx.is_in_transaction()
                && SqliteTransactionManager::get_transaction_depth(&**tx) == 1,
        )?;
        let raw = load_sqlx_database(tx)
            .await
            .map_err(|_| session.clock.fail())?;
        same_database(&raw, plan.native_expected()).map_err(|_| session.clock.fail())?;
        let qualified_floor = collect_raw(&raw, &session.prepare_inputs, SchemaRead::Stable);
        let qualified_floor = qualified_floor_or_poison_r3(&mut session.clock, qualified_floor);
        if matches!(&qualified_floor, Err(Error::ClockRegressed)) {
            return Err(Error::ClockRegressed);
        }
        // Typed data failures stay poisoned and unsatisfied. Inspect the actual
        // known data below before choosing a missing-source result; this performs
        // no SQL, time sampling or writes and cannot turn this floor into Ok.
        check(
            permit.closed_non_audit_digest.as_deref()
                == Some(native_database_digest(&raw, false)?.as_str()),
            "original_closed_prefix",
        )?;
        let actual_binding = actual_audit_row_r24(&raw, ctx.namespace(), binding.sequence)?;
        let actual_asof = actual_audit_row_r24(&raw, ctx.namespace(), witness.seq)?;
        check(
            native_text(actual_binding, "payload")? == binding.payload
                && native_text(actual_binding, "previous_hash")? == binding.previous
                && native_text(actual_binding, "digest")? == binding.digest
                && native_text(actual_asof, "payload")? == witness.payload
                && native_text(actual_asof, "previous_hash")? == witness.previous
                && native_text(actual_asof, "digest")? == witness.digest,
            "original_audit_ack",
        )?;
        let (carrier, actual_row_digest) = decode_complete_known_raw_r36(&raw, &reference)?;
        check(
            actual_row_digest == inserted.insert_row_digest
                && carrier.input.occurred_at_unix_ms >= permit.capture_millis,
            "actual_insert_row",
        )?;
        let (snapshot, _) = snapshot(&raw)?;
        let mut inspected = inspect_complete_native_capture_r36(&carrier, &snapshot)?;
        inspect_available_originals_r36(&raw, &snapshot, &carrier, &mut inspected.actual)?;
        for actual in native_mutable_projections_r39(
            &raw,
            &snapshot,
            &carrier.input.namespace,
            &carrier.input.ticket_snapshot,
            &carrier.input.facts,
        )? {
            if let Some(saved) = carrier
                .input
                .mutable_fact_projections
                .iter()
                .find(|saved| saved.kind == actual.kind && saved.id == actual.id)
            {
                r36_check(
                    typed_fingerprint(saved)? == typed_fingerprint(&actual)?,
                    "actual_native_projection_ack_source",
                )?;
            } else {
                inspected
                    .actual
                    .insert("frozen_native_mutable_projection_missing");
            }
        }

        validate_native_exposure_capture_ack_r2(&raw, &snapshot, &carrier, &mut inspected.actual)?;
        // The original full Stable floor must have succeeded before a capture,
        // stamp or COMMIT admission. A deferred typed error is never cleared.
        let floor = qualified_floor?;
        inspected.finish()?;
        let origin = snapshot
            .complete_origins_r24
            .get(&(reference.namespace.clone(), reference.physical_id.clone()))
            .ok_or_else(|| invalid("actual_origin_binding"))?;
        check(origin.reference == reference, "actual_origin_reference")?;
        let capture = OriginalCapture {
            raw,
            snapshot,
            carrier,
            reference,
        };
        verify_complete(&capture)?;
        // The expensive pure work changes no DB state. Recheck cancellation,
        // retained same-native identity and a NEW actual wall-clock sample before
        // returning to the existing irreversible-admission CAS.
        check_cancel(&permit.gate)?;
        permit.same_session(session, true)?;
        let stamp = session.clock.seconds(&floor)?;
        session.clock.take_seconds(stamp)?;
        Ok(())
    }

    pub(super) fn frozen_graph(
        carrier: &FrozenCarrierR36,
    ) -> Result<Vec<MirrorFormalDependencyNode>> {
        let ticket = &carrier.input.ticket_snapshot;
        let mut roots = vec![(
            "artifact".to_owned(),
            evaluation_storage_id(TICKET_KIND, &ticket.id)?,
        )];
        if matches!(
            carrier.input.operation,
            evo_core::formal_version::Operation::Complete
        ) {
            roots.push((
                "artifact".to_owned(),
                evaluation_storage_id(RESOURCE_KIND, &ticket.id)?,
            ));
        }
        let by_key: BTreeMap<_, _> = carrier
            .input
            .facts
            .iter()
            .map(|fact| ((fact.kind.clone(), fact.id.clone()), fact))
            .collect();
        check(
            by_key.len() == carrier.input.facts.len(),
            "duplicate_frozen_fact",
        )?;
        let mut pending: BTreeSet<_> = roots.into_iter().collect();
        let mut seen = BTreeSet::new();
        while let Some(key) = pending.pop_first() {
            if !seen.insert(key.clone()) {
                continue;
            }
            check(seen.len() <= 10_000, "frozen_graph_capacity")?;
            let node = by_key
                .get(&key)
                .ok_or_else(|| invalid("incomplete_frozen_fact_graph"))?;
            for edge in &node.outgoing {
                if !seen.contains(edge) {
                    pending.insert(edge.clone());
                }
            }
            check(
                seen.len() + pending.len() <= 10_000,
                "frozen_graph_capacity",
            )?;
        }
        check(seen.len() == by_key.len(), "unreachable_frozen_fact")?;
        Ok(carrier
            .input
            .facts
            .iter()
            .map(|fact| MirrorFormalDependencyNode {
                kind: fact.kind.clone(),
                id: fact.id.clone(),
                schema_version: fact.schema_version.clone(),
                record_kind: fact.record_kind.clone(),
                logical_id: fact.logical_id.clone(),
                content_digest: fact.typed_record_digest.clone(),
                outgoing_edge_digest: fact.outgoing_digest.clone(),
                outgoing_edge_count: fact.outgoing.len(),
            })
            .collect())
    }

    // Pure typed-record data projection only. The domains/field order below
    // match e05_dependency_identity, including its deliberate Value domains.
    // No witness, qualification, Current token, time floor or successful result
    // is created here; original full source/authority guards still run below.
    pub(super) fn known_dependency_identity_r3(
        snapshot: &OccurrenceSnapshot,
        object: &OccurrenceObject,
    ) -> Result<(String, Option<String>, String, String)> {
        check(
            serde_json::to_vec(&object.body)
                .map_err(crate::internal)?
                .len()
                <= 4 * 1024 * 1024,
            "known_node_capacity",
        )?;
        if object.kind == "run" {
            validate_native_authority(object)?;
            let record: MirrorStoredTraceAuthority = decode(&object.body)?;
            return Ok((
                record.schema_version.clone(),
                None,
                object.id.clone(),
                typed_fingerprint(&record)?,
            ));
        }
        check(
            matches!(
                object.kind.as_str(),
                "artifact" | "formal_parent" | "release" | "pointer" | "receipt"
            ),
            "known_node_kind",
        )?;
        if object.kind == "artifact"
            && object.id.starts_with("optgrant-")
            && object.body.get("schema_version").is_none()
        {
            let grant: evo_core::evidence::SourceSelection = decode(&object.body)?;
            grant.validate()?;
            check(
                object.id == format!("optgrant-{}", typed_fingerprint(&grant)?),
                "known_source_selection_id",
            )?;
            return Ok((
                "rsia.legacy_source_selection.v1".into(),
                None,
                object.id.clone(),
                typed_fingerprint(&object.body)?,
            ));
        }
        let schema = string(&object.body, "schema_version")?;
        if compiler_node_schema(&object.kind, schema) {
            if schema == "rsia.exploration_artifact_envelope.v1" {
                qualify_exploration(object)?;
            } else {
                // Reuse every original schema-specific local validator, including
                // body id/namespace, typed decoding, hashes and nested contracts.
                // The exact compiler registry excludes release_candidate, the
                // sole time_free_record branch that looks up snapshot sources.
                // This is data inspection; full compiler authority/closure below
                // remains mandatory and is never replaced by this projection.
                time_free_record(object, snapshot)?;
            }
            return Ok((
                schema.into(),
                None,
                object.id.clone(),
                typed_fingerprint(&object.body)?,
            ));
        }
        check(object.kind == "artifact", "known_envelope_kind")?;
        if schema == "rsia.release_candidate.v1" {
            let candidate: MirrorReleaseCandidateRecord = decode(&object.body)?;
            validate_bundle(&candidate.bundle)?;
            e05_builtin_parent_strategy(&candidate.bundle.improver)?;
            check(
                candidate.profile_id == candidate.bundle.profile_id
                    && candidate.bundle_digest == candidate.bundle.digest
                    && candidate.parent_digest == candidate.bundle.parent_digest
                    && !candidate.sources.is_empty()
                    && candidate.sources.windows(2).all(|pair| pair[0] < pair[1]),
                "known_candidate_self_content",
            )?;
            digest(&candidate.environment_digest)?;
            identifier(&candidate.proposer_actor)?;
            for source in &candidate.sources {
                check(source.kind == "run", "known_candidate_source_kind")?;
                digest(&source.content_digest)?;
            }
            return Ok((
                schema.into(),
                None,
                object.id.clone(),
                typed_fingerprint(&candidate)?,
            ));
        }
        check(
            schema == "rsia.typed_artifact_envelope.v1"
                || (schema == evo_core::formal_version::NATIVE_TICKET_ENVELOPE
                    && string(&object.body, "record_kind")? == "evaluation_ticket_v2"),
            "known_envelope_schema",
        )?;
        let kind = string(&object.body, "record_kind")?;
        let payload = envelope(object, schema, kind)?;
        let expected =
            dependency_payload_schema(kind).ok_or_else(|| invalid("known_record_kind"))?;
        let actual = string(payload, "schema_version")?;
        check(
            actual == expected
                || (kind == "registered_evaluation_control_v41"
                    && actual == "rsia.registered_evaluation_control.v1"),
            "known_payload_schema",
        )?;
        e05_dependency_payload_identity(kind, payload)?;
        let logical = match kind {
            "registered_evaluation_control_v41"
            | "protected_holdout_v41"
            | "evaluation_ticket_v2"
            | "evaluation_execution_output_v1" => string(payload, "id")?.to_owned(),
            "execution_receipt_v2" | "independent_grader_receipt_v2" => {
                string(payload, "receipt_id")?.to_owned()
            }
            "alpha_claim_reservation_v1" => format!(
                "{}:{}",
                string(payload, "research_family_id")?,
                string(payload, "claim_id")?
            ),
            _ => string(payload, "ticket_id")?.to_owned(),
        };
        check(
            object.id == evaluation_storage_id(kind, &logical)?,
            "known_logical_id",
        )?;
        macro_rules! fp {
            ($ty:ty) => {
                e05_typed_envelope_fingerprint::<$ty>(object, kind, payload)?
            };
        }
        let content = match kind {
            "registered_evaluation_control_v41" => {
                let control: MirrorRegisteredEvaluationControl = decode(payload)?;
                control.validate()?;
                fp!(MirrorRegisteredEvaluationControl)
            }
            "protected_holdout_v41" => fp!(MirrorProtectedHoldoutRecord),
            "evaluation_ticket_v2" => {
                let ticket: MirrorEvaluationTicketV2 = decode(payload)?;
                ticket.validate()?;
                fp!(MirrorEvaluationTicketV2)
            }
            "evaluation_ticket_issue_receipt_v1" => fp!(MirrorTicketIssueReceipt),
            "evaluation_ticket_issue_receipt_v2" => fp!(MirrorTicketIssueReceiptV2),
            "alpha_claim_reservation_v1" => fp!(MirrorAlphaClaimReservation),
            "execution_receipt_v2" => fp!(MirrorExecutionReceiptV2),
            "independent_grader_receipt_v2" => fp!(MirrorIndependentGraderReceiptV2),
            "evaluation_execution_output_v1" => {
                let output: MirrorExecutionOutputArtifact = decode(payload)?;
                check(
                    !output.output_utf8.is_empty()
                        && output.output_utf8.len() <= 256 * 1024
                        && hash(output.output_utf8.as_bytes()) == output.output_digest,
                    "known_output_content",
                )?;
                fp!(MirrorExecutionOutputArtifact)
            }
            "evaluation_resource_evidence_v1" => {
                let resource: MirrorResourceEvidenceRecord = decode(payload)?;
                check(
                    resource.cost_scope == MirrorCostEvidenceScope::TicketExecutionOnly
                        && !resource.calls.is_empty(),
                    "known_resource_scope",
                )?;
                let mut ids = BTreeSet::new();
                for call in &resource.calls {
                    check(
                        ids.insert(&call.call_id)
                            && call.dispatch_group_id == resource.ticket_id
                            && call.billing_scope == resource.billing_scope,
                        "known_resource_call_tuple",
                    )?;
                }
                fp!(MirrorResourceEvidenceRecord)
            }
            _ => return Err(invalid("known_typed_record_kind")),
        };
        Ok((schema.into(), Some(kind.into()), logical, content))
    }

    pub(super) fn known_dependency_node_r3(
        snapshot: &OccurrenceSnapshot,
        object: &OccurrenceObject,
    ) -> Result<MirrorFormalDependencyNode> {
        e05_dependency_live(snapshot, object)
            .map_err(|_| invalid("known_node_revoked_or_redacted"))?;
        let (schema_version, record_kind, logical_id, content_digest) =
            known_dependency_identity_r3(snapshot, object)
                .map_err(|_| invalid("known_node_typed_record"))?;
        let outgoing = e05_outgoing(snapshot, &object.namespace, &object.kind, &object.id)?;
        Ok(MirrorFormalDependencyNode {
            kind: object.kind.clone(),
            id: object.id.clone(),
            schema_version,
            record_kind,
            logical_id,
            content_digest,
            outgoing_edge_digest: typed_fingerprint(&outgoing)?,
            outgoing_edge_count: outgoing.len(),
        })
    }

    // Check EVERY available node and relation before missing is returned. A
    // missing node only leaves unknown reachability through that node; it does
    // not suppress comparisons for any other existing node. Roots and all known
    // edges must already belong to the declared closed graph.
    fn inspect_known_dependency_graph_r3(
        snapshot: &OccurrenceSnapshot,
        namespace: &str,
        roots: Vec<(String, String)>,
        expected: &[MirrorFormalDependencyNode],
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<()> {
        let by_key: BTreeMap<_, _> = expected
            .iter()
            .map(|node| ((node.kind.clone(), node.id.clone()), node))
            .collect();
        check(
            !by_key.is_empty() && by_key.len() == expected.len() && by_key.len() <= 10_000,
            "known_graph_keys",
        )?;
        for root in &roots {
            check(by_key.contains_key(root), "known_graph_root_missing")?;
        }
        let mut actual_outgoing = BTreeMap::new();
        for node in expected {
            let key = (namespace.to_owned(), node.kind.clone(), node.id.clone());
            if let Some(object) = snapshot.objects.get(&key) {
                check(
                    object.namespace == namespace
                        && object.kind == node.kind
                        && object.id == node.id,
                    "known_node_storage_identity",
                )?;
                let actual = known_dependency_node_r3(snapshot, object)?;
                check(actual == *node, "known_typed_fact_digest_or_edges")?;
                let outgoing = e05_outgoing(snapshot, namespace, &node.kind, &node.id)?;
                for edge in &outgoing {
                    check(by_key.contains_key(edge), "known_graph_extra_edge")?;
                }
                actual_outgoing.insert((node.kind.clone(), node.id.clone()), outgoing);
            } else {
                gaps.insert("original_typed_dependency_node_missing");
            }
        }
        let mut pending: BTreeSet<_> = roots.into_iter().collect();
        let mut seen = BTreeSet::new();
        let mut unknown_reachability = false;
        while let Some(key) = pending.pop_first() {
            if !seen.insert(key.clone()) {
                continue;
            }
            check(seen.len() <= 10_000, "known_graph_capacity")?;
            if let Some(outgoing) = actual_outgoing.get(&key) {
                for edge in outgoing {
                    if !seen.contains(edge) {
                        pending.insert(edge.clone());
                    }
                }
            } else {
                unknown_reachability = true;
            }
            check(seen.len() + pending.len() <= 10_000, "known_graph_capacity")?;
        }
        if !unknown_reachability {
            check(seen.len() == by_key.len(), "known_graph_unreachable_node")?;
        }
        Ok(())
    }

    pub(super) fn inspect_available_candidate_r3(
        snapshot: &OccurrenceSnapshot,
        namespace: &str,
        ticket: &MirrorEvaluationTicketV2,
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<()> {
        let Some(binding) = &ticket.candidate_dependency_binding else {
            return Ok(());
        };
        binding
            .validate()
            .map_err(|_| invalid("known_candidate_binding_shape"))?;
        check(
            binding.namespace == namespace
                && binding.candidate_id == ticket.candidate_digest
                && binding.bundle_digest == ticket.candidate_bundle_digest
                && binding.parent_digest == ticket.baseline_digest
                && binding.environment_digest == ticket.environment_digest
                && binding.proposer_actor == ticket.proposer_actor,
            "known_candidate_ticket_binding",
        )?;
        // A missing candidate must not hide known control/holdout contradictions.
        let control: Option<MirrorRegisteredEvaluationControl> = available_record_r36(
            snapshot,
            namespace,
            "registered_evaluation_control_v41",
            &ticket.registration_id,
            gaps,
            "original_registered_control_missing",
        )?;
        if let Some(control) = &control {
            control.validate()?;
            known_ticket_control_r36(ticket, control)?;
            check(
                binding.proposer_actor == control.proposer_actor,
                "known_candidate_control_binding",
            )?;
        }
        let holdout: Option<MirrorProtectedHoldoutRecord> = available_record_r36(
            snapshot,
            namespace,
            HOLDOUT_KIND,
            &ticket.holdout_id,
            gaps,
            "original_holdout_or_oracle_missing",
        )?;
        if let Some(holdout) = &holdout {
            known_ticket_holdout_r36(ticket, holdout)?;
            check(
                binding.candidate_id == holdout.pair.candidate_digest
                    && binding.bundle_digest == holdout.candidate_bundle_digest
                    && binding.parent_digest == holdout.pair.baseline_digest
                    && binding.environment_digest == holdout.manifest.environment_digest,
                "known_candidate_holdout_binding",
            )?;
        }
        if let Some(watermark) = snapshot.watermarks.get(namespace) {
            check(
                binding.revoke_watermark_seq
                    == i64::try_from(watermark.0)
                        .map_err(|_| invalid("known_watermark_capacity"))?
                    && binding.revoke_watermark_digest == watermark.1,
                "known_candidate_watermark",
            )?;
        } else {
            gaps.insert("original_candidate_watermark_missing");
        }
        if let Some(object) = snapshot.objects.get(&(
            namespace.to_owned(),
            "artifact".to_owned(),
            binding.candidate_id.clone(),
        )) {
            let candidate: MirrorReleaseCandidateRecord = decode(&object.body)?;
            check(
                candidate.id == binding.candidate_id
                    && binding.candidate_record_digest == typed_fingerprint(&candidate)?
                    && binding.bundle_digest == candidate.bundle_digest
                    && binding.parent_digest == candidate.parent_digest
                    && binding.profile_id == candidate.profile_id
                    && binding.revoke_watermark_seq
                        == i64::try_from(candidate.revoke_watermark)
                            .map_err(|_| invalid("known_candidate_watermark_capacity"))?
                    && binding.environment_digest == candidate.environment_digest
                    && binding.proposer_actor == candidate.proposer_actor
                    && binding.sources == candidate.sources,
                "known_actual_candidate_binding",
            )?;
            let outgoing = e05_outgoing(snapshot, namespace, "artifact", &candidate.id)?;
            for source in &candidate.sources {
                check(
                    outgoing.contains(&(source.kind.clone(), source.id.clone())),
                    "known_actual_candidate_source_edge",
                )?;
                if let Some(native) = snapshot.objects.get(&(
                    namespace.to_owned(),
                    source.kind.clone(),
                    source.id.clone(),
                )) {
                    validate_native_authority(native)
                        .map_err(|_| invalid("known_candidate_native_source"))?;
                    let native: MirrorStoredTraceAuthority = decode(&native.body)?;
                    check(
                        native.trace.source_digest == source.content_digest,
                        "known_candidate_source_digest",
                    )?;
                } else {
                    gaps.insert("original_candidate_native_source_missing");
                }
            }
        } else {
            gaps.insert("original_candidate_record_missing");
        }
        inspect_known_dependency_graph_r3(
            snapshot,
            namespace,
            vec![("artifact".into(), binding.candidate_id.clone())],
            &binding.nodes,
            gaps,
        )
    }

    fn verify_actual_facts(
        source: &AvailableCompleteInputsR36<'_>,
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<()> {
        let mut raw_rows = BTreeMap::new();
        for row in source
            .raw
            .tables
            .get("objects")
            .ok_or(Error::ClockRegressed)?
        {
            let key = (
                native_text(row, "namespace")?,
                native_text(row, "kind")?,
                native_text(row, "id")?,
            );
            check(
                raw_rows.insert(key, row).is_none(),
                "duplicate_actual_raw_object",
            )?;
        }
        for fact in &source.carrier.input.facts {
            let key = (fact.namespace.clone(), fact.kind.clone(), fact.id.clone());
            match (source.snapshot.objects.get(&key), raw_rows.get(&key)) {
                (Some(object), Some(row)) => {
                    // These are the ACTUAL same-operation originals, not hashes used
                    // to synthesize missing content and not a later mutable projection.
                    check(
                        object.namespace == fact.namespace
                            && object.kind == fact.kind
                            && object.id == fact.id
                            && object.owner == fact.owner
                            && hash(&row_bytes(row)) == fact.raw_row_digest
                            && hash(native_text(row, "body")?.as_bytes()) == fact.raw_body_digest
                            && e05_outgoing(
                                source.snapshot,
                                &fact.namespace,
                                &fact.kind,
                                &fact.id,
                            )? == fact.outgoing,
                        "actual_fact_identity_content_or_edges",
                    )?;
                    e05_dependency_live(source.snapshot, object)
                        .map_err(|_| invalid("revoked_or_redacted_actual_fact"))?;
                }
                (None, None) => {
                    gaps.insert("original_fact_missing");
                }
                _ => return Err(invalid("actual_raw_snapshot_disagreement")),
            }
        }
        Ok(())
    }

    fn verify_original_typed_graph_r36(
        source: &AvailableCompleteInputsR36<'_>,
        expected_nodes: &[MirrorFormalDependencyNode],
    ) -> Result<()> {
        let ticket = &source.carrier.input.ticket_snapshot;
        let mut roots = vec![(
            "artifact".into(),
            evaluation_storage_id(TICKET_KIND, &ticket.id)?,
        )];
        if matches!(
            source.carrier.input.operation,
            evo_core::formal_version::Operation::Complete
        ) {
            roots.push((
                "artifact".into(),
                evaluation_storage_id(RESOURCE_KIND, &ticket.id)?,
            ));
        }
        let nodes = derive_nodes(source.snapshot, &source.reference.namespace, roots)
            .map_err(|_| invalid("original_typed_dependency_closure"))?;
        check(
            nodes == expected_nodes,
            "original_typed_fact_digest_or_graph",
        )
    }

    pub(super) fn reconstruct_calls(
        snapshot: &OccurrenceSnapshot,
        saved_calls: &[evo_core::formal_version::Call<FrozenCallR24>],
    ) -> Result<(Vec<BudgetCallRecord>, BTreeSet<&'static str>)> {
        use e05_budget_call_snapshot_r19_r1::ReadFailure;
        // First check ALL frozen DTO identities/shapes; a missing first call cannot
        // conceal an already-known bad descriptor belonging to a later call.
        for saved in saved_calls {
            saved
                .snapshot
                .validate_frozen_target(&saved.namespace, &saved.billing_scope, &saved.call_id)
                .map_err(frozen_call_error_r24)?;
        }
        let mut calls = Vec::new();
        let mut gaps = BTreeSet::new();
        for saved in saved_calls {
            match saved.snapshot.reconstruct_from_qualified_snapshot(
                snapshot,
                &saved.namespace,
                &saved.billing_scope,
                &saved.call_id,
            ) {
                Ok(original) => calls.push(original),
                Err(ReadFailure::InvalidSnapshot(code)) => {
                    return Err(frozen_call_error_r24(ReadFailure::InvalidSnapshot(code)));
                }
                Err(ReadFailure::SourceInsufficient(code)) => {
                    gaps.insert(code);
                }
            }
        }
        Ok((calls, gaps))
    }

    // Classification only, never a source/permission constructor. Production
    // passes a scope only from the actual control decoded and validated below.
    pub(super) fn reject_known_group_extras_r1(
        snapshot: &OccurrenceSnapshot,
        namespace: &str,
        ticket_id: &str,
        actual_control_scope: Option<&str>,
        frozen_ids: &BTreeSet<(String, String, String)>,
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<()> {
        let Some(scope) = actual_control_scope else {
            gaps.insert("original_registered_control_missing");
            return Ok(());
        };
        for call in snapshot
            .calls
            .values()
            .filter(|call| call.billing_scope == scope && call.dispatch_group_id == ticket_id)
        {
            check(
                call.namespace == namespace
                    && frozen_ids.contains(&(
                        call.namespace.clone(),
                        call.billing_scope.clone(),
                        call.call_id.clone(),
                    )),
                "complete_group_extra_call",
            )?;
        }
        // Deliberately NOT set equality: a purely missing original remains a gap.
        Ok(())
    }

    pub(super) fn verify_report(
        actual: &MirrorBernsteinReportWire,
        expected: &MirrorBernsteinReportWire,
    ) -> Result<()> {
        check(actual == expected, "complete_statistics_or_reasons")
    }

    fn available_record_r36<T: DeserializeOwned>(
        snapshot: &OccurrenceSnapshot,
        ns: &str,
        kind: &str,
        id: &str,
        gaps: &mut BTreeSet<&'static str>,
        missing: &'static str,
    ) -> Result<Option<T>> {
        let key = (
            ns.to_owned(),
            "artifact".to_owned(),
            evaluation_storage_id(kind, id)?,
        );
        let Some(object) = snapshot.objects.get(&key) else {
            gaps.insert(missing);
            return Ok(None);
        };
        let payload = envelope(object, "rsia.typed_artifact_envelope.v1", kind)?;
        e05_dependency_payload_identity(kind, payload)?;
        e05_dependency_live(snapshot, object)?;
        Ok(Some(decode(payload)?))
    }
    // Return is only data availability, never a caller-supplied permit/bool.
    // Inspect every available target even if another target/source is absent.
    pub(super) fn inspect_target_rows_r36(
        snapshot: &OccurrenceSnapshot,
        ns: &str,
        ticket: &MirrorEvaluationTicketV2,
        control: Option<&MirrorRegisteredEvaluationControl>,
        holdout: Option<&MirrorProtectedHoldoutRecord>,
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<bool> {
        let mut available = true;
        for target in &ticket.targets {
            let MirrorTargetState::Completed {
                candidate_score_micros,
                baseline_score_micros,
                anchor_result,
                candidate_receipt_id,
                baseline_receipt_id,
                grader_receipt_id,
                ..
            } = &target.state
            else {
                return Err(invalid("incomplete_frozen_target"));
            };
            let mut receipts = Vec::new();
            for (id, side, score) in [
                (
                    candidate_receipt_id,
                    MirrorExecutionSide::Candidate,
                    *candidate_score_micros,
                ),
                (
                    baseline_receipt_id,
                    MirrorExecutionSide::Baseline,
                    *baseline_score_micros,
                ),
            ] {
                let receipt: Option<MirrorExecutionReceiptV2> = available_record_r36(
                    snapshot,
                    ns,
                    EXECUTION_RECEIPT_KIND,
                    id,
                    gaps,
                    "original_execution_receipt_missing",
                )?;
                let Some(receipt) = receipt else {
                    available = false;
                    continue;
                };
                let bundle = match side {
                    MirrorExecutionSide::Candidate => &ticket.candidate_bundle_digest,
                    MirrorExecutionSide::Baseline => &ticket.baseline_bundle_digest,
                };
                check(
                    receipt.schema_version == "rsia.execution_receipt.v2"
                        && receipt.receipt_id == *id
                        && receipt.ticket_id == ticket.id
                        && receipt.target_id == target.target_id
                        && receipt.side == side
                        && receipt.bundle_digest == *bundle
                        && receipt.environment_digest == ticket.environment_digest
                        && receipt.executor_actor == ticket.executor_actor
                        && receipt.request_digest
                            == e05_request_digest(
                                ticket,
                                target,
                                side,
                                bundle,
                                &ticket.environment_digest,
                            )?,
                    "available_execution_identity",
                )?;
                let output: Option<MirrorExecutionOutputArtifact> = available_record_r36(
                    snapshot,
                    ns,
                    OUTPUT_KIND,
                    &receipt.output_artifact_id,
                    gaps,
                    "original_execution_output_missing",
                )?;
                if let Some(output) = output {
                    e05_output_artifact(&receipt, &output)?;
                    if let (Some(control), Some(holdout)) = (control, holdout) {
                        let oracle = holdout
                            .oracle_entries
                            .iter()
                            .find(|entry| entry.target_id == target.target_id);
                        if let Some(oracle) = oracle {
                            let actual = owned_grade_fixed_output(
                                &control.fixed_grader,
                                &output.output_utf8,
                                &oracle.expected_answer_json,
                            )?;
                            check(actual == score, "available_original_output_score")?;
                        } else {
                            gaps.insert("original_target_oracle_missing");
                            available = false;
                        }
                    }
                } else {
                    available = false;
                }
                receipts.push(receipt);
            }
            let grader: Option<MirrorIndependentGraderReceiptV2> = available_record_r36(
                snapshot,
                ns,
                GRADER_RECEIPT_KIND,
                grader_receipt_id,
                gaps,
                "original_grader_receipt_missing",
            )?;
            if let Some(grader) = grader {
                check(
                    grader.schema_version == "rsia.independent_grader_receipt.v2"
                        && grader.receipt_id == *grader_receipt_id
                        && grader.ticket_id == ticket.id
                        && grader.target_id == target.target_id
                        && grader.evaluator_actor == ticket.evaluator_actor
                        && grader.candidate_execution_receipt_id == *candidate_receipt_id
                        && grader.baseline_execution_receipt_id == *baseline_receipt_id
                        && grader.candidate_score_micros == *candidate_score_micros
                        && grader.baseline_score_micros == *baseline_score_micros
                        && grader.anchor_result == *anchor_result,
                    "available_grader_ticket_identity",
                )?;
                if let Some(control) = control {
                    check(
                        grader.oracle_digest == control.oracle_digest
                            && grader.grader_digest == control.grader_digest,
                        "available_grader_control_identity",
                    )?;
                }
                for receipt in &receipts {
                    let recorded = match receipt.side {
                        MirrorExecutionSide::Candidate => &grader.candidate_output_digest,
                        MirrorExecutionSide::Baseline => &grader.baseline_output_digest,
                    };
                    check(
                        *recorded == receipt.output_digest,
                        "available_grader_output_identity",
                    )?;
                }
                if receipts.len() == 2 {
                    e05_execution_pair(ticket, &target.target_id, &receipts[0], &receipts[1])?;
                    if let Some(control) = control {
                        let pair = typed_fingerprint(&(
                            &receipts[0].output_digest,
                            &receipts[1].output_digest,
                            &control.oracle_digest,
                            &control.grader_digest,
                            *candidate_score_micros,
                            *baseline_score_micros,
                            *anchor_result,
                        ))?;
                        check(
                            grader.scored_output_pair_digest == pair,
                            "available_scored_output_pair_digest",
                        )?;
                    }
                }
            } else {
                available = false;
            }
        }
        Ok(available)
    }
    pub(super) fn inspect_available_originals_r36(
        raw: &RawDatabase,
        snapshot: &OccurrenceSnapshot,
        carrier: &FrozenCarrierR36,
        actual_gaps: &mut BTreeSet<&'static str>,
    ) -> Result<()> {
        validate_complete_known_r36(carrier, &carrier.input.namespace, &carrier.id)?;
        let reference =
            known_complete_reference_r36(carrier, &carrier.input.ticket_snapshot.evaluator_actor)?;
        let source = AvailableCompleteInputsR36 {
            raw,
            snapshot,
            carrier,
            reference,
        };
        let ticket = &carrier.input.ticket_snapshot;
        let expected_nodes = frozen_graph(carrier)?;
        let mut roots = vec![(
            "artifact".into(),
            evaluation_storage_id(TICKET_KIND, &ticket.id)?,
        )];
        if matches!(
            &carrier.input.operation,
            evo_core::formal_version::Operation::Complete
        ) {
            roots.push((
                "artifact".into(),
                evaluation_storage_id(RESOURCE_KIND, &ticket.id)?,
            ));
        }
        inspect_known_dependency_graph_r3(
            snapshot,
            &carrier.input.namespace,
            roots,
            &expected_nodes,
            actual_gaps,
        )?;
        verify_actual_facts(&source, actual_gaps)?;
        inspect_available_candidate_r3(snapshot, &carrier.input.namespace, ticket, actual_gaps)?;
        if carrier_effect_r1(&carrier.input.operation, &carrier.payload)?
            != NativePlanEffectR1::NewComplete
        {
            if actual_gaps.is_empty() {
                verify_original_typed_graph_r36(&source, &expected_nodes)?;
            }
            return Ok(());
        }
        let MirrorFormalEvaluationV2::CompleteBatch {
            cost_evidence_scope,
            report,
            stored_dependency_binding,
            anchor_evidence_digest,
            resource_evidence_digest,
            ..
        } = &carrier.payload
        else {
            return Err(invalid("not_complete_effect"));
        };
        check(
            matches!(
                &carrier.input.operation,
                evo_core::formal_version::Operation::Complete
            ),
            "complete_operation_or_reference",
        )?;
        let (calls, call_gaps) = reconstruct_calls(snapshot, &carrier.input.calls)?;
        let calls_available = call_gaps.is_empty();
        actual_gaps.extend(call_gaps);
        let frozen_ids: BTreeSet<_> = carrier
            .input
            .calls
            .iter()
            .map(|call| {
                (
                    call.namespace.clone(),
                    call.billing_scope.clone(),
                    call.call_id.clone(),
                )
            })
            .collect();
        let ns = carrier.input.namespace.as_str();
        let control_key = (
            ns.to_owned(),
            "artifact".to_owned(),
            evaluation_storage_id("registered_evaluation_control_v41", &ticket.registration_id)?,
        );
        let control = if snapshot.objects.contains_key(&control_key) {
            let actual = evaluation_control(snapshot, ns, &ticket.registration_id)?;
            known_ticket_control_r36(ticket, &actual)?;
            Some(actual)
        } else {
            actual_gaps.insert("original_registered_control_missing");
            None
        };
        reject_known_group_extras_r1(
            snapshot,
            ns,
            &ticket.id,
            control.as_ref().map(|c| c.billing_scope.as_str()),
            &frozen_ids,
            actual_gaps,
        )?;
        let holdout: Option<MirrorProtectedHoldoutRecord> = available_record_r36(
            snapshot,
            ns,
            HOLDOUT_KIND,
            &ticket.holdout_id,
            actual_gaps,
            "original_holdout_or_oracle_missing",
        )?;
        if let Some(holdout) = &holdout {
            known_ticket_holdout_r36(ticket, holdout)?;
            if let Some(control) = &control {
                holdout.validate_against(control)?;
            }
        }
        let resource_key = evaluation_storage_id(RESOURCE_KIND, &ticket.id)?;
        let resource: Option<MirrorResourceEvidenceRecord> = available_record_r36(
            snapshot,
            ns,
            RESOURCE_KIND,
            &ticket.id,
            actual_gaps,
            "original_resource_evidence_missing",
        )?;
        if let Some(resource) = &resource {
            check(
                resource.schema_version == "rsia.evaluation_resource_evidence.v1"
                    && resource.ticket_id == ticket.id
                    && resource.cost_scope == MirrorCostEvidenceScope::TicketExecutionOnly
                    && *cost_evidence_scope == resource.cost_scope
                    && resource.calls.len() == carrier.input.calls.len()
                    && typed_fingerprint(resource)? == *resource_evidence_digest,
                "available_resource_identity_or_digest",
            )?;
            if let Some(control) = &control {
                check(
                    resource.billing_scope == control.billing_scope,
                    "resource_actual_control_scope",
                )?;
            }
            let saved_by_id: BTreeMap<_, _> = carrier
                .input
                .calls
                .iter()
                .map(|c| {
                    (
                        (
                            c.namespace.as_str(),
                            c.billing_scope.as_str(),
                            c.call_id.as_str(),
                        ),
                        c,
                    )
                })
                .collect();
            let mut copied = BTreeSet::new();
            for original in &resource.calls {
                let key = (
                    original.namespace.as_str(),
                    original.billing_scope.as_str(),
                    original.call_id.as_str(),
                );
                check(copied.insert(key), "duplicate_resource_call")?;
                let saved = saved_by_id
                    .get(&key)
                    .ok_or_else(|| invalid("extra_resource_call"))?;
                saved
                    .snapshot
                    .validate_original_copy_r36(original)
                    .map_err(frozen_call_error_r24)?;
            }
        }
        let anchors: Vec<_> = ticket
            .targets
            .iter()
            .filter(|target| matches!(&target.kind, MirrorEvaluationTargetKind::Anchor { .. }))
            .collect();
        check(
            typed_fingerprint(&anchors)? == *anchor_evidence_digest,
            "anchor_evidence_digest",
        )?;
        // The stored snapshot's own complete namespace/ticket/digest/graph
        // identities are knowable even when a real source is missing.
        let expected_binding = match (
            &ticket.candidate_dependency_binding,
            stored_dependency_binding,
        ) {
            (Some(candidate), Some(stored)) => {
                let expected = MirrorFormalStoredDependencyBinding {
                    schema_version: "rsia.formal_stored_dependency_binding.v1".into(),
                    namespace: ns.into(),
                    candidate_binding_digest: typed_fingerprint(candidate)?,
                    ticket_id: ticket.id.clone(),
                    ticket_physical_id: evaluation_storage_id(TICKET_KIND, &ticket.id)?,
                    ticket_digest: typed_fingerprint(ticket)?,
                    resource_physical_id: resource_key,
                    nodes: expected_nodes.clone(),
                };
                check(stored == &expected, "stored_dependency_binding")?;
                Some(expected)
            }
            (None, None) => None,
            _ => return Err(invalid("mixed_candidate_stored_binding")),
        };
        let targets_available = inspect_target_rows_r36(
            snapshot,
            ns,
            ticket,
            control.as_ref(),
            holdout.as_ref(),
            actual_gaps,
        )?;
        let exposure = exposure_projection_r36(snapshot, ns, ticket)?;
        if exposure.is_none() {
            actual_gaps.insert("original_exposure_entry_missing");
        }
        // Only real available originals feed mathematics. A missing root,
        // permission witness, issue receipt or frozen projection does not prevent
        // checking already-known numerical contradictions. No gap is filled.
        let expected_report = if calls_available && targets_available && exposure.is_some() {
            if let (Some(control), Some(holdout)) = (&control, &holdout) {
                let actual =
                    e05_recompute_complete_report(snapshot, ns, ticket, control, holdout, &calls)
                        .map_err(|_| invalid("original_complete_math_or_exposure"))?;
                verify_report(report, &actual)?;
                Some(actual)
            } else {
                None
            }
        } else {
            None
        };
        if let (Some(expected_report), Some(resource)) = (expected_report, &resource) {
            // All semantic fields, diagnostic reasons and actual statistics are
            // compared using the unchanged original payload constructor.
            let expected = MirrorFormalEvaluationV2::CompleteBatch {
                evidence_scope: ticket.evidence_scope,
                cost_evidence_scope: MirrorCostEvidenceScope::TicketExecutionOnly,
                ticket_id: ticket.id.clone(),
                ticket_digest: typed_fingerprint(ticket)?,
                manifest_digest: ticket.manifest_digest.clone(),
                candidate_bundle_digest: ticket.candidate_bundle_digest.clone(),
                baseline_bundle_digest: ticket.baseline_bundle_digest.clone(),
                environment_digest: ticket.environment_digest.clone(),
                verdict: expected_report.verdict,
                report: expected_report,
                anchor_evidence_digest: typed_fingerprint(&anchors)?,
                resource_evidence_digest: typed_fingerprint(resource)?,
                stored_dependency_binding: expected_binding.clone(),
            };
            check(
                typed_fingerprint(&expected)? == typed_fingerprint(&carrier.payload)?,
                "complete_full_payload",
            )?;
        }
        // This is an inspection returning explicit gaps, not verification success.
        // Every original permission/typed-closure guard below remains mandatory
        // when originals are present. finish() refuses any gap afterwards.
        if actual_gaps.is_empty() {
            let control = control.ok_or_else(|| invalid("available_control_invariant"))?;
            let holdout = holdout.ok_or_else(|| invalid("available_holdout_invariant"))?;
            producer_witnesses(snapshot, ns, &control, &holdout)?;
            e05_complete_ticket_identity(snapshot, ns, ticket, &control, &holdout)?;
            e05_complete_group(snapshot, ns, &control.billing_scope, &ticket.id)?;
            let actual_ids: BTreeSet<_> = snapshot
                .calls
                .values()
                .filter(|call| {
                    call.billing_scope == control.billing_scope
                        && call.dispatch_group_id == ticket.id
                })
                .map(|c| {
                    (
                        c.namespace.clone(),
                        c.billing_scope.clone(),
                        c.call_id.clone(),
                    )
                })
                .collect();
            check(
                actual_ids == frozen_ids && frozen_ids.len() == calls.len(),
                "complete_group_call_set",
            )?;
            let resource = resource.ok_or_else(|| invalid("available_resource_invariant"))?;
            let saved_by_id: BTreeMap<_, _> =
                calls.iter().map(|c| (c.call_id.as_str(), c)).collect();
            let mut copied = BTreeSet::new();
            check(
                resource.calls.len() == calls.len()
                    && resource.calls.iter().all(|c| {
                        copied.insert(c.call_id.as_str())
                            && saved_by_id
                                .get(c.call_id.as_str())
                                .is_some_and(|saved| *saved == c)
                    }),
                "original_resource_or_frozen_calls",
            )?;
            // Explicitly regrade the REAL original output against the REAL stored oracle.
            // No saved score, report summary or caller authority substitutes for bytes.
            for target in &ticket.targets {
                let grader =
                    e05_completed_target_evidence(snapshot, ns, ticket, &control, &holdout, target)
                        .map_err(|_| invalid("actual_output_receipt_grader_closure"))?;
                let oracle = holdout
                    .oracle_entries
                    .iter()
                    .find(|entry| entry.target_id == target.target_id)
                    .ok_or_else(|| {
                        let gaps = BTreeSet::from(["original_target_oracle_missing"]);
                        missing(&gaps)
                    })?;
                let MirrorTargetState::Completed {
                    candidate_receipt_id,
                    baseline_receipt_id,
                    ..
                } = &target.state
                else {
                    return Err(invalid("incomplete_frozen_target"));
                };
                for (receipt_id, expected_score) in [
                    (candidate_receipt_id, grader.candidate_score_micros),
                    (baseline_receipt_id, grader.baseline_score_micros),
                ] {
                    let receipt: MirrorExecutionReceiptV2 =
                        evaluation_record(snapshot, ns, EXECUTION_RECEIPT_KIND, receipt_id)
                            .map_err(|_| invalid("original_execution_receipt"))?;
                    let output_key =
                        evaluation_storage_id(OUTPUT_KIND, &receipt.output_artifact_id)?;
                    if !source.snapshot.objects.contains_key(&(
                        ns.into(),
                        "artifact".into(),
                        output_key,
                    )) {
                        return Err(missing(&BTreeSet::from([
                            "original_execution_output_missing",
                        ])));
                    }
                    let output: MirrorExecutionOutputArtifact =
                        evaluation_record(snapshot, ns, OUTPUT_KIND, &receipt.output_artifact_id)
                            .map_err(|_| invalid("original_output_decode"))?;
                    e05_output_artifact(&receipt, &output)
                        .map_err(|_| invalid("original_output_binding"))?;
                    let actual_score = owned_grade_fixed_output(
                        &control.fixed_grader,
                        &output.output_utf8,
                        &oracle.expected_answer_json,
                    )
                    .map_err(|_| invalid("original_output_regrade"))?;
                    check(actual_score == expected_score, "regraded_score")?;
                }
            }
            // Original qualified closure, candidate and stored-binding guards
            // were not replaced with fingerprints or DTO permissions.
            verify_original_typed_graph_r36(&source, &expected_nodes)?;
            if let (Some(candidate), Some(expected)) =
                (&ticket.candidate_dependency_binding, &expected_binding)
            {
                e05_candidate_binding(snapshot, ns, ticket, &control, &holdout, candidate)
                    .map_err(|_| invalid("original_candidate_binding"))?;
                e05_stored_binding(snapshot, ns, ticket, expected)
                    .map_err(|_| invalid("original_stored_dependency_closure"))?;
            }
        }
        Ok(())
    }
    fn verify_complete(source: &OriginalCapture) -> Result<()> {
        let reference = known_complete_reference_r36(&source.carrier, &source.reference.owner)?;
        check(
            reference == source.reference,
            "complete_operation_or_reference",
        )?;
        validate_complete_native_capture_r36(&source.raw, &source.carrier, &source.snapshot)
    }

    #[cfg(test)]
    mod r36_original_compiler_static_tests {
        use super::*;

        // ProgramFixture data only. No evaluator witness, source replacement,
        // Current permit, claimed as-of snapshot or live Host is constructed.
        fn original_compiler_fixture() -> OccurrenceSnapshot {
            let identity = hash(b"r36-static-program-fixture");
            let record = MirrorEnvironmentIdentityRecord {
                id: format!("monitor-env-{}", &identity[..32]),
                schema_version: "rsia.monitoring.environment.v1".into(),
                identity_digest: identity,
                namespace: "r36-ns".into(),
                evidence_scope: MirrorEnvironmentEvidenceScope::ProgramFixture,
                profile_id: "r36-profile".into(),
                release_id: None,
                bundle_digest: None,
                environment_digest: hash(b"r36-environment"),
                model_identity: "r36-fixture-model".into(),
                ordered_tools: vec!["r36-fixture-tool".into()],
                host_id: "r36-fixture-host".into(),
                host_version: "r36-fixture-version".into(),
                host_surface_digest: hash(b"r36-surface"),
                host_capabilities_digest: hash(b"r36-capabilities"),
                mandatory_context_digest: hash(b"r36-context"),
                development_manifest_digest: hash(b"r36-manifest"),
                grader_digest: hash(b"r36-grader"),
                generation_strategy_digest: hash(b"r36-strategy"),
                revoke_watermark: 0,
            };
            let object = OccurrenceObject {
                namespace: record.namespace.clone(),
                kind: "artifact".into(),
                id: record.id.clone(),
                owner: "r36-fixture-evaluator".into(),
                body: serde_json::to_value(record).unwrap(),
                body_value_digest_r26: None,
            };
            let mut snapshot = OccurrenceSnapshot::default();
            snapshot.objects.insert(
                (
                    object.namespace.clone(),
                    object.kind.clone(),
                    object.id.clone(),
                ),
                object,
            );
            snapshot
        }

        // An attack may recompute all Value hashes after corrupting its body.
        // Matching this unqualified claim is never sufficient for valid content.
        fn self_consistent_value_claim(object: &OccurrenceObject) -> MirrorFormalDependencyNode {
            MirrorFormalDependencyNode {
                kind: object.kind.clone(),
                id: object.id.clone(),
                schema_version: "rsia.monitoring.environment.v1".into(),
                record_kind: None,
                logical_id: object.id.clone(),
                content_digest: typed_fingerprint(&object.body).unwrap(),
                outgoing_edge_digest: typed_fingerprint(&Vec::<(String, String)>::new()).unwrap(),
                outgoing_edge_count: 0,
            }
        }

        #[test]
        fn complete_original_static_validators_keep_healthy_data_without_granting_authority() {
            let snapshot = original_compiler_fixture();
            let object = snapshot.objects.values().next().unwrap();
            assert!(time_free_record(object, &snapshot).is_ok());
            let node = self_consistent_value_claim(object);
            let mut gaps = BTreeSet::from(["control_evaluator_witness_missing"]);
            inspect_known_dependency_graph_r3(
                &snapshot,
                "r36-ns",
                vec![(node.kind.clone(), node.id.clone())],
                &[node],
                &mut gaps,
            )
            .unwrap();
            assert_eq!(gaps, BTreeSet::from(["control_evaluator_witness_missing"]));
            assert!(matches!(require_sources(&gaps), Err(Error::Conflict(code))
                if code.starts_with("formal_source_insufficient:r29:")));
        }

        #[test]
        fn missing_witness_cannot_hide_compiler_static_identity_type_or_content_corruption() {
            for fault in 0..4 {
                let mut snapshot = original_compiler_fixture();
                let object = snapshot.objects.values_mut().next().unwrap();
                let mut body = object.body.clone();
                match fault {
                    0 => body["id"] = Value::String("different-self-id".into()),
                    1 => body["namespace"] = Value::String("different-namespace".into()),
                    2 => {
                        let _ = body.as_object_mut().unwrap().remove("host_id");
                    }
                    3 => body["environment_digest"] = Value::String("not-a-sha256".into()),
                    _ => unreachable!(),
                }
                object.replace_body_r26(body);
                let node = self_consistent_value_claim(object);
                let key = ("r36-ns".to_owned(), node.kind.clone(), node.id.clone());
                // The unchanged original validator is the independent control:
                // these are known bad bytes even with a fully rehashed claim.
                assert!(time_free_record(snapshot.objects.get(&key).unwrap(), &snapshot).is_err());
                let mut gaps = BTreeSet::from(["control_evaluator_witness_missing"]);
                assert!(
                    matches!(inspect_known_dependency_graph_r3(&snapshot, "r36-ns",
                    vec![(node.kind.clone(), node.id.clone())], &[node], &mut gaps),
                    Err(Error::Conflict(code))
                    if code == "formal_invalid_recomputed_complete:r29:known_node_typed_record")
                );
            }
        }
    }

    #[cfg(test)]
    mod r36_known_before_missing_tests {
        use super::*;

        // Original typed decoder/graph producer is the control, not the new
        // projection being tested. This is pure source data, not Current.
        fn original_output_graph() -> (OccurrenceSnapshot, Vec<MirrorFormalDependencyNode>) {
            let output = MirrorExecutionOutputArtifact {
                schema_version: "rsia.execution_output.v1".into(),
                id: "r36-output".into(),
                ticket_id: "r36-ticket".into(),
                target_id: "r36-target".into(),
                side: MirrorExecutionSide::Candidate,
                output_utf8: "{\"answer\":1}".into(),
                output_digest: hash(b"{\"answer\":1}"),
            };
            let id = evaluation_storage_id(OUTPUT_KIND, &output.id).unwrap();
            let body = serde_json::to_value(E05FingerprintEnvelope {
                schema_version: "rsia.typed_artifact_envelope.v1",
                id: &id,
                record_kind: OUTPUT_KIND,
                payload: &output,
            })
            .unwrap();
            let object = OccurrenceObject {
                namespace: "r36-ns".into(),
                kind: "artifact".into(),
                id: id.clone(),
                owner: "r36-executor".into(),
                body,
                body_value_digest_r26: None,
            };
            let mut snapshot = OccurrenceSnapshot::default();
            snapshot
                .objects
                .insert(("r36-ns".into(), "artifact".into(), id.clone()), object);
            let expected =
                derive_nodes(&snapshot, "r36-ns", vec![("artifact".into(), id)]).unwrap();
            (snapshot, expected)
        }

        #[test]
        fn qualified_floor_preserves_data_kind_and_still_poisons_the_operation() {
            for original in [
                r24_fault(evo_core::formal_version::Fault::SourceInsufficient),
                r24_fault(evo_core::formal_version::Fault::InvalidContent),
                Error::Invalid("formal_unknown_version_quarantine".into()),
            ] {
                let mut clock = crate::write_clock::OperationClock::default();
                assert!(clock.check().is_ok());
                let text = original.to_string();
                let actual = qualified_floor_or_poison_r3(&mut clock, Err(original)).unwrap_err();
                assert_eq!(actual.to_string(), text);
                assert!(matches!(clock.check(), Err(Error::ClockRegressed)));
            }
            for original in [Error::ClockRegressed, Error::Internal] {
                let mut clock = crate::write_clock::OperationClock::default();
                assert!(matches!(
                    qualified_floor_or_poison_r3(&mut clock, Err(original)),
                    Err(Error::ClockRegressed)
                ));
                assert!(matches!(clock.check(), Err(Error::ClockRegressed)));
            }
            let mut clock = crate::write_clock::OperationClock::default();
            let mut floor = OccurrenceFloor::default();
            floor.observe_seconds(17).unwrap();
            assert_eq!(
                qualified_floor_or_poison_r3(&mut clock, Ok(floor.clone())).unwrap(),
                floor
            );
            assert!(clock.check().is_ok());
        }

        #[test]
        fn unrelated_witness_gap_cannot_hide_known_typed_digest_corruption() {
            let (snapshot, expected) = original_output_graph();
            let root = vec![(expected[0].kind.clone(), expected[0].id.clone())];
            let mut gaps = BTreeSet::from(["control_evaluator_witness_missing"]);
            inspect_known_dependency_graph_r3(
                &snapshot,
                "r36-ns",
                root.clone(),
                &expected,
                &mut gaps,
            )
            .unwrap();
            assert_eq!(gaps, BTreeSet::from(["control_evaluator_witness_missing"]));
            let mut changed = expected.clone();
            changed[0].content_digest = hash(b"different-valid-sha");
            assert!(
                matches!(inspect_known_dependency_graph_r3(&snapshot, "r36-ns",
                root, &changed, &mut gaps), Err(Error::Conflict(code))
                if code == "formal_invalid_recomputed_complete:r29:known_typed_fact_digest_or_edges")
            );
        }

        #[test]
        fn missing_node_does_not_hide_another_known_node_but_pure_missing_stays_missing() {
            let (snapshot, expected) = original_output_graph();
            let mut with_missing = expected.clone();
            let mut absent = expected[0].clone();
            absent.id = evaluation_storage_id(OUTPUT_KIND, "r36-missing-output").unwrap();
            absent.logical_id = "r36-missing-output".into();
            with_missing.push(absent.clone());
            let roots = with_missing
                .iter()
                .map(|n| (n.kind.clone(), n.id.clone()))
                .collect::<Vec<_>>();
            let mut gaps = BTreeSet::from(["control_evaluator_witness_missing"]);
            inspect_known_dependency_graph_r3(
                &snapshot,
                "r36-ns",
                roots.clone(),
                &with_missing,
                &mut gaps,
            )
            .unwrap();
            assert!(gaps.contains("original_typed_dependency_node_missing"));
            assert!(matches!(require_sources(&gaps), Err(Error::Conflict(code))
                if code.starts_with("formal_source_insufficient:r29:")));
            with_missing[0].content_digest = hash(b"tampered-known-record");
            assert!(
                matches!(inspect_known_dependency_graph_r3(&snapshot, "r36-ns",
                roots, &with_missing, &mut gaps), Err(Error::Conflict(code))
                if code == "formal_invalid_recomputed_complete:r29:known_typed_fact_digest_or_edges")
            );
        }

        #[test]
        fn actual_edge_or_output_corruption_is_not_downgraded_by_missing_witness() {
            let (mut snapshot, expected) = original_output_graph();
            let id = expected[0].id.clone();
            let roots = vec![("artifact".into(), id.clone())];
            let mut gaps = BTreeSet::from(["control_evaluator_witness_missing"]);
            snapshot.edges.insert((
                "r36-ns".into(),
                "artifact".into(),
                id.clone(),
                "artifact".into(),
                "other-object".into(),
            ));
            assert!(matches!(
                inspect_known_dependency_graph_r3(
                    &snapshot,
                    "r36-ns",
                    roots.clone(),
                    &expected,
                    &mut gaps
                ),
                Err(Error::Conflict(_))
            ));
            snapshot.edges.clear();
            let object = snapshot
                .objects
                .get_mut(&("r36-ns".into(), "artifact".into(), id))
                .unwrap();
            let mut body = object.body.clone();
            body["payload"]["output_digest"] = Value::String(hash(b"wrong-output"));
            object.replace_body_r26(body);
            assert!(
                matches!(inspect_known_dependency_graph_r3(&snapshot, "r36-ns",
                roots, &expected, &mut gaps), Err(Error::Conflict(code))
                if code == "formal_invalid_recomputed_complete:r29:known_node_typed_record")
            );
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use evo_core::evaluation::{ClusterObservation, ExperimentPlan, ProfileKind};
        use evo_core::sequential::{AlphaAllocation, FormalClaimKind, ResearchFamilyAlphaPlan};

        // Pure mathematics/output tests, not a constructed NativeAsOfPermit.
        // The existing nine real engine producer fixtures exercise the only
        // production entry above when Root separately runs them on the new head.
        fn report_from_real_outputs() -> MirrorBernsteinReportWire {
            let grader = MirrorFixedGraderSpec {
                schema_version: "rsia.fixed_grader.v1".into(),
                version: "fixed-v1".into(),
                method: MirrorFixedGraderMethod::ExactJsonAnswerV1,
            };
            let mut plan = ExperimentPlan::first_low_risk("r29-plan", 60).unwrap();
            plan.frozen = true;
            plan.frozen_at = Some(1); // pure algorithm input, no live Clock assertion.
            plan.validate().unwrap();
            let candidate = hash(b"r29-candidate");
            let mut bound = plan.clone();
            bound.bind_candidate(candidate.clone()).unwrap();
            let alpha = ResearchFamilyAlphaPlan::new(
                "r29-family",
                "0.05",
                vec![AlphaAllocation {
                    claim_id: "r29-fixed".into(),
                    attempt_id: "r29-attempt".into(),
                    kind: FormalClaimKind::FixedSampleGain,
                    alpha: "0.04".into(),
                }],
            )
            .unwrap();
            let mut rows = Vec::new();
            for i in 0..60 {
                let candidate_score =
                    owned_grade_fixed_output(&grader, r#"{"answer":1}"#, "1").unwrap();
                let baseline_score =
                    owned_grade_fixed_output(&grader, r#"{"answer":0}"#, "1").unwrap();
                rows.push(ClusterObservation {
                    cluster_id: format!("r29-cluster-{i}"),
                    d: (i64::from(candidate_score) - i64::from(baseline_score)) as f64
                        / 1_000_000.0,
                    weight: 1.0,
                });
            }
            let report = evo_core::formal_complete_statistics::derive_complete_report(
                "rsia.registered_evaluation_control.v2",
                ProfileKind::QualityGain,
                &plan,
                (&candidate, &bound.digest().unwrap()),
                (&alpha, "r29-fixed", "r29-attempt"),
                &rows,
                (1.0, 1.0),
            )
            .unwrap();
            e05_complete_wire(&report).unwrap()
        }

        #[test]
        fn output_regrade_shared_math_and_all_report_fields_are_compared() {
            let original = report_from_real_outputs();
            assert!(verify_report(&original, &original).is_ok());
            let mut changed = original.clone();
            changed.mean = "0".into();
            assert!(matches!(verify_report(&changed, &original),
                Err(Error::Conflict(code)) if code == "formal_invalid_recomputed_complete:r29:complete_statistics_or_reasons"));
            let mut changed = original.clone();
            changed.reasons.push("unproved_override".into());
            assert!(verify_report(&changed, &original).is_err());
            let mut changed = original.clone();
            changed.alpha_i = "0.05".into();
            assert!(verify_report(&changed, &original).is_err());
        }

        fn call(call_id: &str) -> BudgetCallRecord {
            BudgetCallRecord {
                billing_scope: "r29-scope".into(),
                call_id: call_id.into(),
                dispatch_group_id: "r29-ticket".into(),
                namespace: "r29-ns".into(),
                stage: BudgetStage::TaskExecution,
                actual_input_digest: hash(b"r29-input"),
                request_artifact: None,
                reserved_micros: 10,
                state: BudgetCallState::Reserved,
                lease_token: "r29-lease".into(),
                lease_epoch: 1,
                lease_until: 100,
                dispatch_id: None,
                provider_request_id: None,
                usage_record_id: None,
                output_digest: None,
                actual_cost_micros: None,
                actual_currency: None,
                actual_pricing_version: None,
                execution_provenance: None,
                actual_model_digest: None,
                transport_artifact: None,
                response_artifact: None,
                response_usable: None,
                response_block_reason: None,
                execution_closed: true,
                execution_closed_at: None,
                execution_close_reason: None,
                terminal_reason: None,
                created_at: 1,
                dispatched_at: None,
                finalized_at: None,
            }
        }

        #[test]
        fn frozen_call_missing_cannot_hide_later_known_actual_identity_failure() {
            let mut original = OccurrenceSnapshot::default();
            for id in ["a", "b"] {
                original
                    .calls
                    .insert(("r29-scope".into(), id.into()), call(id));
            }
            let mut saved = Vec::new();
            for id in ["a", "b"] {
                let frozen = FrozenCallR24::freeze_from_qualified_snapshot(
                    &original,
                    "r29-ns",
                    "r29-scope",
                    id,
                )
                .unwrap();
                saved.push(evo_core::formal_version::Call {
                    namespace: "r29-ns".into(),
                    billing_scope: "r29-scope".into(),
                    call_id: id.into(),
                    snapshot_digest: typed_fingerprint(&frozen).unwrap(),
                    snapshot: frozen,
                });
            }
            let (calls, gaps) = reconstruct_calls(&original, &saved).unwrap();
            assert_eq!(calls, vec![call("a"), call("b")]);
            assert!(gaps.is_empty());
            original.calls.remove(&("r29-scope".into(), "a".into()));
            let (calls, gaps) = reconstruct_calls(&original, &saved).unwrap();
            assert_eq!(calls, vec![call("b")]);
            assert!(!gaps.is_empty());
            original
                .calls
                .get_mut(&("r29-scope".into(), "b".into()))
                .unwrap()
                .namespace = "wrong-ns".into();
            assert!(matches!(reconstruct_calls(&original, &saved),
                Err(Error::Conflict(code)) if code.starts_with("formal_invalid_frozen_call:")));
        }

        fn original_pair_r1() -> (
            OccurrenceSnapshot,
            Vec<evo_core::formal_version::Call<FrozenCallR24>>,
        ) {
            let mut original = OccurrenceSnapshot::default();
            for id in ["a", "b"] {
                original
                    .calls
                    .insert(("r29-scope".into(), id.into()), call(id));
            }
            let saved = ["a", "b"]
                .into_iter()
                .map(|id| {
                    let frozen = FrozenCallR24::freeze_from_qualified_snapshot(
                        &original,
                        "r29-ns",
                        "r29-scope",
                        id,
                    )
                    .unwrap();
                    evo_core::formal_version::Call {
                        namespace: "r29-ns".into(),
                        billing_scope: "r29-scope".into(),
                        call_id: id.into(),
                        snapshot_digest: typed_fingerprint(&frozen).unwrap(),
                        snapshot: frozen,
                    }
                })
                .collect();
            (original, saved)
        }

        #[test]
        fn missing_original_cannot_hide_later_foreign_namespace_group_extra_r1() {
            let (mut snapshot, saved) = original_pair_r1();
            snapshot.calls.remove(&("r29-scope".into(), "a".into()));
            let mut extra = call("z-extra");
            extra.namespace = "foreign-ns".into();
            snapshot
                .calls
                .insert(("r29-scope".into(), "z-extra".into()), extra);
            let (known, mut gaps) = reconstruct_calls(&snapshot, &saved).unwrap();
            assert_eq!(known, vec![call("b")]);
            assert!(!gaps.is_empty());
            let ids = saved
                .iter()
                .map(|c| {
                    (
                        c.namespace.clone(),
                        c.billing_scope.clone(),
                        c.call_id.clone(),
                    )
                })
                .collect();
            assert!(matches!(reject_known_group_extras_r1(
                &snapshot, "r29-ns", "r29-ticket", Some("r29-scope"), &ids, &mut gaps,
            ), Err(Error::Conflict(code))
                if code == "formal_invalid_recomputed_complete:r29:complete_group_extra_call"));
        }

        #[test]
        fn pure_missing_and_missing_control_stay_source_insufficient_r1() {
            let (mut snapshot, saved) = original_pair_r1();
            snapshot.calls.remove(&("r29-scope".into(), "a".into()));
            let mut unrelated = call("other-group");
            unrelated.dispatch_group_id = "different-ticket".into();
            unrelated.namespace = "foreign-ns".into();
            snapshot
                .calls
                .insert(("r29-scope".into(), "other-group".into()), unrelated);
            let (known, mut gaps) = reconstruct_calls(&snapshot, &saved).unwrap();
            assert_eq!(known, vec![call("b")]);
            let ids = saved
                .iter()
                .map(|c| {
                    (
                        c.namespace.clone(),
                        c.billing_scope.clone(),
                        c.call_id.clone(),
                    )
                })
                .collect();
            reject_known_group_extras_r1(
                &snapshot,
                "r29-ns",
                "r29-ticket",
                Some("r29-scope"),
                &ids,
                &mut gaps,
            )
            .unwrap();
            assert!(matches!(require_sources(&gaps),
                Err(Error::Conflict(code)) if code.starts_with("formal_source_insufficient:r29:")));
            let mut no_control = BTreeSet::new();
            reject_known_group_extras_r1(
                &snapshot,
                "r29-ns",
                "r29-ticket",
                None,
                &ids,
                &mut no_control,
            )
            .unwrap();
            assert!(no_control.contains("original_registered_control_missing"));
            assert!(matches!(require_sources(&no_control),
                Err(Error::Conflict(code)) if code.starts_with("formal_source_insufficient:r29:")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_early_material_checks_original_shape_and_inner_ticket() {
        let d = hash(b"legacy-early-shape");
        let certificate = EarlyStopCertificate::from_verified_parts(EarlyStopCertificateParts {
            ticket_id: "legacy-ticket".into(),
            v1_plan_snapshot_digest: d.clone(),
            early_stop_plan_digest: d.clone(),
            alpha_plan_digest: d.clone(),
            manifest_digest: d.clone(),
            slice_id: "legacy-slice".into(),
            candidate_digest: d.clone(),
            baseline_digest: d.clone(),
            grader_digest: d.clone(),
            issuer_receipt_digest: d.clone(),
            k: 0,
            lcb: None,
            ucb: None,
            completed_prefix_digest: d.clone(),
            member_terminal_digest: d.clone(),
            stop_reason: EarlyStopDecision::CriticalRegression,
            stopped_at_unix_ms: 1,
            stop_sequence: 1,
            usage_uncertain: false,
        })
        .unwrap();
        let formal = MirrorFormalEvaluationV2::EarlyStopped {
            evidence_scope: MirrorEvaluationEvidenceScope::ProgramFixture,
            ticket_id: "legacy-ticket".into(),
            ticket_digest: d,
            certificate,
        };
        validate_legacy_formal_shape_r1("legacy-ns", "legacy-ticket", &formal).unwrap();
        let original = serde_json::to_value(&formal).unwrap();
        for (field, bad) in [
            ("stopped_at_unix_ms", serde_json::json!(-1)),
            ("stop_sequence", serde_json::json!(0)),
            ("k", serde_json::json!(1)),
            ("lcb", serde_json::json!("0.9")),
        ] {
            let mut changed = original.clone();
            changed["certificate"][field] = bad;
            let decoded: MirrorFormalEvaluationV2 = serde_json::from_value(changed).unwrap();
            assert!(
                matches!(validate_legacy_formal_shape_r1("legacy-ns","legacy-ticket",&decoded),
                Err(Error::Conflict(ref reason)) if reason=="formal_invalid_legacy_certificate_shape")
            );
        }
        let mut changed = original.clone();
        changed["certificate"]["ticket_id"] = serde_json::json!("other-ticket");
        let decoded: MirrorFormalEvaluationV2 = serde_json::from_value(changed).unwrap();
        if let MirrorFormalEvaluationV2::EarlyStopped { certificate, .. } = &decoded {
            certificate.validate_shape().unwrap(); // own shape alone is insufficient
        }
        assert!(
            matches!(validate_legacy_formal_shape_r1("legacy-ns","legacy-ticket",&decoded),
            Err(Error::Conflict(ref reason)) if reason=="formal_invalid_legacy_certificate_identity")
        );
        assert_eq!(serde_json::to_value(&formal).unwrap(), original);
    }

    #[test]
    fn legacy_complete_material_binds_original_dependency_to_outer_identity() {
        fn binding(ticket: &str) -> MirrorFormalStoredDependencyBinding {
            let mut nodes: Vec<_> = [TICKET_KIND, RESOURCE_KIND]
                .into_iter()
                .map(|kind| MirrorFormalDependencyNode {
                    kind: "artifact".into(),
                    id: evaluation_storage_id(kind, ticket).unwrap(),
                    schema_version: "rsia.typed_artifact_envelope.v1".into(),
                    record_kind: Some(kind.into()),
                    logical_id: ticket.into(),
                    content_digest: hash(kind.as_bytes()),
                    outgoing_edge_digest: typed_fingerprint(&Vec::<(String, String)>::new())
                        .unwrap(),
                    outgoing_edge_count: 0,
                })
                .collect();
            nodes.sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
            let b = MirrorFormalStoredDependencyBinding {
                schema_version: "rsia.formal_stored_dependency_binding.v1".into(),
                namespace: "legacy-ns".into(),
                candidate_binding_digest: hash(b"candidate"),
                ticket_id: ticket.into(),
                ticket_physical_id: evaluation_storage_id(TICKET_KIND, ticket).unwrap(),
                ticket_digest: hash(b"saved-ticket"),
                resource_physical_id: evaluation_storage_id(RESOURCE_KIND, ticket).unwrap(),
                nodes,
            };
            b.validate().unwrap();
            b
        }
        let good = binding("legacy-ticket");
        let formal = MirrorFormalEvaluationV2::CompleteBatch {
            evidence_scope: MirrorEvaluationEvidenceScope::ProgramFixture,
            cost_evidence_scope: MirrorCostEvidenceScope::TicketExecutionOnly,
            ticket_id: "legacy-ticket".into(),
            ticket_digest: good.ticket_digest.clone(),
            manifest_digest: hash(b"manifest"),
            candidate_bundle_digest: hash(b"candidate"),
            baseline_bundle_digest: hash(b"baseline"),
            environment_digest: hash(b"environment"),
            verdict: Verdict::Improved,
            report: MirrorBernsteinReportWire {
                stats_version: "rsia.empirical_bernstein.v2".into(),
                verdict: Verdict::Improved,
                n: 60,
                mean: "1".into(),
                variance: "0".into(),
                radius: "0".into(),
                lcb: "1".into(),
                alpha_i: "0.04".into(),
                reasons: vec![],
            },
            anchor_evidence_digest: hash(b"anchor"),
            resource_evidence_digest: hash(b"resource"),
            stored_dependency_binding: Some(good.clone()),
        };
        validate_legacy_formal_shape_r1("legacy-ns", "legacy-ticket", &formal).unwrap();
        let original = serde_json::to_value(&formal).unwrap();
        let mut bad_namespace = good.clone();
        bad_namespace.namespace = "other-ns".into();
        let mut bad_digest = good.clone();
        bad_digest.ticket_digest = hash(b"other-ticket-content");
        for changed in [bad_namespace, bad_digest, binding("other-ticket")] {
            changed.validate().unwrap(); // each is independently well-shaped
            let mut bad = formal.clone();
            if let MirrorFormalEvaluationV2::CompleteBatch {
                stored_dependency_binding,
                ..
            } = &mut bad
            {
                *stored_dependency_binding = Some(changed);
            }
            assert!(
                matches!(validate_legacy_formal_shape_r1("legacy-ns","legacy-ticket",&bad),
                Err(Error::Conflict(ref reason)) if reason=="formal_invalid_legacy_binding_identity")
            );
        }
        let mut bad = formal.clone();
        if let MirrorFormalEvaluationV2::CompleteBatch {
            stored_dependency_binding: Some(b),
            ..
        } = &mut bad
        {
            b.resource_physical_id = "wrong-physical".into();
        }
        assert!(
            matches!(validate_legacy_formal_shape_r1("legacy-ns","legacy-ticket",&bad),
            Err(Error::Conflict(ref reason)) if reason=="formal_invalid_legacy_binding_shape")
        );
        assert_eq!(serde_json::to_value(&formal).unwrap(), original);
    }

    #[tokio::test]
    async fn native_cells_keep_real_sqlite_storage_classes_without_shadow_columns() {
        use sqlx::Connection;

        let mut conn = sqlx::SqliteConnection::connect("sqlite::memory:")
            .await
            .unwrap();
        let rows = sqlx_native_rows(
            sqlx::query("SELECT 'text' AS t, 1 AS i, 1.0 AS r, NULL AS n, X'01' AS b")
                .fetch_all(&mut conn)
                .await
                .unwrap(),
        )
        .unwrap();
        let row = &rows[0];
        assert!(!row.cells.keys().any(|k| k.starts_with("__clock_type_")));
        assert_eq!(native_text(row, "t").unwrap(), "text");
        assert_eq!(native_integer(row, "i").unwrap(), 1);

        for name in ["i", "r", "n", "b", "missing"] {
            assert!(matches!(native_text(row, name), Err(Error::ClockRegressed)));
        }
        for name in ["t", "r", "n", "b", "missing"] {
            assert!(matches!(
                native_integer(row, name),
                Err(Error::ClockRegressed)
            ));
        }
        conn.close().await.unwrap();
    }

    #[tokio::test]
    async fn native_object_and_edge_writes_require_real_ack_and_exact_revision() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("raw-writer.sqlite3");
        let store = crate::Store::open(&path).await.unwrap();
        let ctx = Context::new("raw-owner", "evaluator", Role::Evaluator).unwrap();
        let mut session = store.session().await.unwrap();

        let baseline = load_sqlx_database(session.tx.as_mut().unwrap())
            .await
            .unwrap();
        let mut expected = clone_database(&baseline);

        // Low-level writer/CAS fixture only; no synthetic payload is qualified
        // as a production E05 source. Every change is rolled back below.
        let insert = object_write(
            &expected,
            &ctx,
            "raw_writer_fixture",
            "entry",
            &serde_json::json!({"value": 1}),
            false,
        )
        .unwrap();
        match &insert {
            IssueWrite::Object { before, after } => {
                assert!(before.is_none());
                assert!(!after.cells.keys().any(|k| k.starts_with("__clock_type_")));
                assert_eq!(native_integer(after, "revision").unwrap(), 1);
            }
            _ => panic!("expected object write"),
        }

        insert.execute(&mut session).await.unwrap();
        let actual = load_sqlx_database(session.tx.as_mut().unwrap())
            .await
            .unwrap();
        assert!(same_database(&actual, &expected).is_err());
        insert.acknowledge(&mut expected).unwrap();
        assert!(same_database(&actual, &expected).is_ok());

        assert!(
            object_write(
                &expected,
                &ctx,
                "raw_writer_fixture",
                "entry",
                &serde_json::json!({"value": 2}),
                false,
            )
            .is_err()
        );

        let replace = object_write(
            &expected,
            &ctx,
            "raw_writer_fixture",
            "entry",
            &serde_json::json!({"value": 2}),
            true,
        )
        .unwrap();
        match &replace {
            IssueWrite::Object { before, after } => {
                assert_eq!(
                    native_integer(before.as_ref().unwrap(), "revision").unwrap(),
                    1
                );
                assert_eq!(native_integer(after, "revision").unwrap(), 2);
            }
            _ => panic!("expected object replacement"),
        }
        replace.execute(&mut session).await.unwrap();
        let actual = load_sqlx_database(session.tx.as_mut().unwrap())
            .await
            .unwrap();
        assert!(same_database(&actual, &expected).is_err());
        replace.acknowledge(&mut expected).unwrap();
        assert!(same_database(&actual, &expected).is_ok());

        // The stale revision predicate must really affect zero SQLite rows.
        assert!(matches!(
            replace.execute(&mut session).await,
            Err(Error::ClockRegressed)
        ));
        let actual = load_sqlx_database(session.tx.as_mut().unwrap())
            .await
            .unwrap();
        assert!(same_database(&actual, &expected).is_ok());

        let edge_rows = sqlx_native_rows(
            sqlx::query(
                "SELECT 'raw-owner' AS namespace, 'artifact' AS src_kind, \
                 'source' AS src_id, 'artifact' AS dst_kind, 'target' AS dst_id",
            )
            .fetch_all(&mut **session.tx.as_mut().unwrap())
            .await
            .unwrap(),
        )
        .unwrap();
        let edge = IssueWrite::Edge(edge_rows.into_iter().next().unwrap());
        edge.execute(&mut session).await.unwrap();
        let actual = load_sqlx_database(session.tx.as_mut().unwrap())
            .await
            .unwrap();
        assert!(same_database(&actual, &expected).is_err());
        edge.acknowledge(&mut expected).unwrap();
        assert!(same_database(&actual, &expected).is_ok());

        session.tx.take().unwrap().rollback().await.unwrap();
        drop(session);
        let mut check = store.session().await.unwrap();
        let actual = load_sqlx_database(check.tx.as_mut().unwrap())
            .await
            .unwrap();
        assert!(same_database(&actual, &baseline).is_ok());
        check.commit().await.unwrap();
        store.close().await;
    }

    #[test]
    fn terminal_intents_never_accept_caller_time_phase_or_arbitrary_kind() {
        for schema in [COMPLETE_INTENT_SCHEMA, RECONCILE_INTENT_SCHEMA] {
            let valid = serde_json::json!({"schema_version":schema,"ticket_id":"ticket"});
            assert!(TicketIntent::read(valid.clone(), schema).is_ok());
            for extra in [
                "now",
                "issued_at_unix_seconds",
                "issued_at_unix_ms",
                "pending",
                "verified",
                "record_kind",
            ] {
                let mut bad = valid.clone();
                bad[extra] = serde_json::json!(1);
                assert!(TicketIntent::read(bad, schema).is_err());
            }
            let mut wrong = valid;
            wrong["schema_version"] = serde_json::json!(INTENT_SCHEMA);
            assert!(TicketIntent::read(wrong, schema).is_err());
        }
        assert!(InvalidateIntent::read(serde_json::json!({"schema_version":"rsia.e05_invalidate_intent.v2","ticket_id":"ticket","reason":"operator_invalid"})).is_ok());
        for reason in ["", "bad reason", "bad/reason"] {
            assert!(InvalidateIntent::read(serde_json::json!({"schema_version":"rsia.e05_invalidate_intent.v2","ticket_id":"ticket","reason":reason})).is_err());
        }
    }

    #[test]
    fn first_formal_is_allowed_but_exact_existing_formal_is_immutable_without_namespace_ban() {
        let mut source = OccurrenceSnapshot::default();
        let physical = evaluation_storage_id(FORMAL_KIND, "ticket").unwrap();
        let formal = MirrorFormalEvaluationV2::Invalid {
            evidence_scope: MirrorEvaluationEvidenceScope::ProgramFixture,
            ticket_id: "ticket".into(),
            ticket_digest: hash(b"actual-ticket"),
            reasons: vec!["operator_invalid".into()],
        };
        let body = serde_json::json!({"schema_version":"rsia.typed_artifact_envelope.v1","id":physical,"record_kind":FORMAL_KIND,"payload":formal});
        assert!(require_new_formal(&source, "owner", "ticket").is_ok());
        source.objects.insert(
            ("other".into(), "artifact".into(), physical.clone()),
            OccurrenceObject {
                namespace: "other".into(),
                kind: "artifact".into(),
                id: physical.clone(),
                owner: "grader".into(),
                body: body.clone(),
                body_value_digest_r26: None,
            },
        );
        assert!(require_new_formal(&source, "owner", "ticket").is_ok());
        source.objects.insert(
            ("owner".into(), "artifact".into(), physical.clone()),
            OccurrenceObject {
                namespace: "owner".into(),
                kind: "artifact".into(),
                id: physical.clone(),
                owner: "grader".into(),
                body: body.clone(),
                body_value_digest_r26: None,
            },
        );
        let before = canonical_fingerprint(
            &source
                .objects
                .get(&("owner".into(), "artifact".into(), physical.clone()))
                .unwrap()
                .body,
        )
        .unwrap();
        assert!(
            matches!(require_new_formal(&source,"owner","ticket"),Err(Error::Conflict(ref reason)) if reason=="formal_record_immutable_requires_versioned_successor")
        );
        assert_eq!(
            source
                .objects
                .get(&("owner".into(), "artifact".into(), physical))
                .unwrap()
                .body,
            body
        );
        assert_eq!(before, canonical_fingerprint(&body).unwrap());
        assert!(require_new_formal(&source, "owner", "another-ticket").is_ok());
    }

    #[test]
    fn receipt2_time_checks_preserve_same_second_and_reject_millisecond_tampering() {
        assert!(e05_issue_receipt_v2_time(10, 10_502, 10, 10_501).is_ok());
        for (seconds, millis, created, manifest) in [
            (11, 10_502, 10, 10_501),
            (10, 11_502, 10, 10_501),
            (10, 10_500, 10, 10_501),
            (-1, 10_502, 10, 10_501),
            (10, -1, 10, 10_501),
            (10, 10_502, 11, 10_501),
            (10, 10_502, 10, -1),
        ] {
            assert!(matches!(
                e05_issue_receipt_v2_time(seconds, millis, created, manifest),
                Err(Error::ClockRegressed)
            ));
        }
    }

    #[test]
    fn receipt2_binding_cannot_fall_back_to_unbound_v1_legacy() {
        assert!(e05_issue_receipt_binding("evaluation_ticket_issue_receipt_v1", None).is_ok());
        assert!(matches!(
            e05_issue_receipt_binding("evaluation_ticket_issue_receipt_v2", None),
            Err(Error::ClockRegressed)
        ));
    }
    #[test]
    fn cancel_and_commit_have_one_linear_admission_winner() {
        use std::sync::Barrier;
        for _ in 0..32 {
            let gate = Arc::new(AtomicU8::new(ACTIVE));
            let barrier = Arc::new(Barrier::new(3));
            let cancel_gate = Arc::clone(&gate);
            let cancel_barrier = Arc::clone(&barrier);
            let canceller = std::thread::spawn(move || {
                cancel_barrier.wait();
                request_cancellation(&cancel_gate)
            });
            let commit_gate = Arc::clone(&gate);
            let commit_barrier = Arc::clone(&barrier);
            let committer = std::thread::spawn(move || {
                commit_barrier.wait();
                admit_commit(&commit_gate).is_ok()
            });
            barrier.wait();
            let cancelled = canceller.join().unwrap();
            let admitted = committer.join().unwrap();
            assert_ne!(cancelled, admitted);
            assert_eq!(
                gate.load(Ordering::Acquire),
                if cancelled {
                    CANCELLED
                } else {
                    COMMIT_ADMITTED
                }
            );
            if cancelled {
                assert!(admit_commit(&gate).is_err());
            } else {
                assert!(!request_cancellation(&gate));
            }
        }
    }
    #[test]
    fn attempted_commit_and_depth_zero_rollback_command_ack_remain_unknown() {
        let bits = COMMIT_ATTEMPTED | ROLLBACK_COMMAND_ACK | NATIVE_CLOSE_ACK | TERMINAL_ACK;
        let facts = native_facts(bits, COMMIT_ADMITTED);
        assert!(facts.commit_admitted && facts.commit_attempted);
        assert_eq!(facts.commit_command_sent, None);
        assert!(facts.rollback_command_acknowledged);
        assert!(!facts.rollback_confirmed && !facts.commit_confirmed);
        assert!(channel_failure(bits).commit_outcome_unknown);
        let known = native_facts(bits | COMMIT_ACK, COMMIT_ADMITTED);
        assert_eq!(known.commit_command_sent, Some(true));
        assert!(known.commit_confirmed && !known.rollback_confirmed);
        let retained = lost_completion(0, ACTIVE);
        assert!(retained.native.terminal_unconfirmed);
        assert!(
            !retained.native.rollback_confirmed && !retained.native.rollback_command_acknowledged
        );
    }
    #[test]
    fn execution_and_grader_intents_reject_time_or_phase_authority() {
        let execution = serde_json::json!({
            "schema_version":EXECUTION_INTENT_SCHEMA,"receipt_id":"r","ticket_id":"t","target_id":"x",
            "side":"candidate","request_digest":"0".repeat(64),"budget_call_id":"c","output_artifact_id":"o",
            "output_utf8":"{\"answer\":1}","latency_micros":1
        });
        assert!(ExecutionIntent::read(execution.clone()).is_ok());
        let grade = serde_json::json!({
            "schema_version":GRADE_INTENT_SCHEMA,"receipt_id":"g","ticket_id":"t","target_id":"x",
            "candidate_execution_receipt_id":"r1","baseline_execution_receipt_id":"r2"
        });
        assert!(GradeIntent::read(grade.clone()).is_ok());
        for field in [
            "issued_at_unix_seconds",
            "phase",
            "verified",
            "pending_ticket",
        ] {
            let mut e = execution.clone();
            e[field] = 5.into();
            assert!(ExecutionIntent::read(e).is_err());
            let mut g = grade.clone();
            g[field] = 5.into();
            assert!(GradeIntent::read(g).is_err());
        }
    }
    #[test]
    fn actual_output_grading_keeps_original_scores_and_error_priority() {
        let grader = MirrorFixedGraderSpec {
            schema_version: "rsia.fixed_grader.v1".into(),
            version: "fixed-v1".into(),
            method: MirrorFixedGraderMethod::ExactJsonAnswerV1,
        };
        assert_eq!(
            owned_grade_fixed_output(&grader, r#"{"answer":1}"#, "1").unwrap(),
            1_000_000
        );
        assert_eq!(
            owned_grade_fixed_output(&grader, r#"{"answer":2}"#, "1").unwrap(),
            0
        );
        for output in [r#"{"answer":1,"answer":1}"#, "not-json"] {
            assert!(
                matches!(owned_grade_fixed_output(&grader,output,"1"),Err(Error::Invalid(ref message)) if message=="JSON contains invalid or duplicate fields")
            );
        }
        assert!(
            matches!(owned_grade_fixed_output(&grader,r#"{"answer":1}null"#,"1"),Err(Error::Invalid(ref message)) if message=="JSON contains trailing content")
        );
        assert!(
            matches!(owned_grade_fixed_output(&grader,r#"{"extra":1}"#,"1"),Err(Error::Invalid(ref message)) if message=="execution output violates fixed grader schema")
        );
    }

    #[test]
    fn v2_intent_rejects_caller_time_unknown_schema_and_extra_fields() {
        let intent = serde_json::json!({
            "schema_version": INTENT_SCHEMA, "ticket_id":"t1",
            "registration_id":"c1", "holdout_id":"h1"
        });
        assert!(IssueIntent::read(intent.clone()).is_ok());
        let mut timed = intent.clone();
        timed["issued_at_unix_seconds"] = 5.into();
        assert!(IssueIntent::read(timed).is_err());
        let mut old = intent.clone();
        old["schema_version"] = "rsia.e05_issue_intent.v1".into();
        assert!(IssueIntent::read(old).is_err());
        let mut trusted = intent;
        trusted["verified"] = true.into();
        assert!(IssueIntent::read(trusted).is_err());
    }

    #[test]
    fn frozen_rows_preserve_native_types_revision_body_and_other_namespaces() {
        let row = OwnedRow {
            cells: BTreeMap::from([
                ("namespace".into(), OwnedCell::Text("other".into())),
                ("body".into(), OwnedCell::Text("{\"x\":1}".into())),
                ("revision".into(), OwnedCell::Integer(1)),
                ("number".into(), OwnedCell::Real(-0.0)),
            ]),
        };
        let original = RawDatabase {
            tables: BTreeMap::from([("objects".into(), vec![row])]),
        };
        let mut changed = clone_database(&original);
        assert!(same_database(&original, &changed).is_ok());
        changed.tables.get_mut("objects").unwrap()[0]
            .cells
            .insert("number".into(), OwnedCell::Real(0.0));
        assert!(same_database(&original, &changed).is_err());
        changed = clone_database(&original);
        changed.tables.get_mut("objects").unwrap()[0]
            .cells
            .insert("revision".into(), OwnedCell::Integer(2));
        assert!(same_database(&original, &changed).is_err());
        changed = clone_database(&original);
        changed.tables.get_mut("objects").unwrap()[0]
            .cells
            .insert("body".into(), OwnedCell::Text("{\"x\":2}".into()));
        assert!(same_database(&original, &changed).is_err());
    }

    #[test]
    fn uncertain_commit_is_not_erased_by_a_later_rollback_ack() {
        let unknown = channel_failure(COMMIT_ATTEMPTED | ROLLBACK_COMMAND_ACK | NATIVE_CLOSE_ACK);
        assert!(unknown.commit_outcome_unknown);
        assert!(!unknown.commit_confirmed);
        let confirmed = channel_failure(COMMIT_ATTEMPTED | COMMIT_ACK | NATIVE_CLOSE_ACK);
        assert!(!confirmed.commit_outcome_unknown);
        assert!(confirmed.commit_confirmed);
    }

    #[test]
    fn copied_phase_plan_members_and_edges_are_not_an_issue_intent() {
        let mut value = serde_json::json!({
            "schema_version": INTENT_SCHEMA, "ticket_id":"t1",
            "registration_id":"c1", "holdout_id":"h1"
        });
        for (field, replay) in [
            ("phase", serde_json::json!(3)),
            ("ticket", serde_json::json!({"id":"t1"})),
            ("edges", serde_json::json!([["artifact", "somewhere"]])),
            ("source_checked", serde_json::json!(true)),
        ] {
            value[field] = replay;
            assert!(
                IssueIntent::read(value.clone()).is_err(),
                "{field} became authority"
            );
            value.as_object_mut().unwrap().remove(field);
        }
        assert!(IssueIntent::read(value).is_ok());
    }

    #[test]
    fn exact_acknowledged_prefix_does_not_accept_a_missing_or_different_edge() {
        let expected = RawDatabase {
            tables: BTreeMap::from([("dependencies".into(), Vec::new())]),
        };
        let edge = OwnedRow {
            cells: BTreeMap::from([
                ("namespace".into(), OwnedCell::Text("own".into())),
                ("src_kind".into(), OwnedCell::Text("artifact".into())),
                ("src_id".into(), OwnedCell::Text("ticket".into())),
                ("dst_kind".into(), OwnedCell::Text("artifact".into())),
                ("dst_id".into(), OwnedCell::Text("required".into())),
            ]),
        };
        let mut next = clone_database(&expected);
        IssueWrite::Edge(edge.clone())
            .acknowledge(&mut next)
            .unwrap();
        assert!(
            same_database(&expected, &next).is_err(),
            "missing edge accepted"
        );
        let mut wrong = clone_database(&next);
        wrong.tables.get_mut("dependencies").unwrap()[0]
            .cells
            .insert("dst_id".into(), OwnedCell::Text("different".into()));
        assert!(same_database(&wrong, &next).is_err(), "wrong edge accepted");
        assert!(same_database(&next, &clone_database(&next)).is_ok());
    }

    #[tokio::test]
    async fn issue_intent_never_exempts_another_namespaces_bad_owned_row() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("store.sqlite3");
        let store = crate::Store::open(&path).await.unwrap();
        let ctx = Context::new("owner-test", "evaluator", Role::Evaluator).unwrap();
        let mut session = store.session().await.unwrap();
        let baseline = load_sqlx_database(session.tx.as_mut().unwrap())
            .await
            .unwrap();
        // Deliberate storage damage in this test's own uncommitted temporary DB;
        // not a healthy writer, producer, caller authority or new legacy scope.
        sqlx::query("INSERT INTO objects(namespace,kind,id,owner,body) VALUES(?,?,?,?,?)")
            .bind("foreign")
            .bind("artifact")
            .bind("bad-record")
            .bind("foreign-owner")
            .bind(r#"{"id":"bad-record","schema_version":"rsia.release_candidate.v1"}"#)
            .execute(&mut **session.tx.as_mut().unwrap())
            .await
            .unwrap();
        let value = serde_json::json!({
            "schema_version": INTENT_SCHEMA, "ticket_id":"t1",
            "registration_id":"c1", "holdout_id":"h1"
        });
        let failure = begin(session, ctx, value)
            .unwrap()
            .finish_detailed()
            .await
            .unwrap_err();
        assert!(matches!(failure.error, Error::ClockRegressed));
        assert!(!failure.commit_outcome_unknown && !failure.commit_confirmed);
        assert!(failure.rollback_error.is_none());
        let mut after = store.session().await.unwrap();
        let actual = load_sqlx_database(after.tx.as_mut().unwrap())
            .await
            .unwrap();
        assert!(
            same_database(&actual, &baseline).is_ok(),
            "refusal did not roll back the catalog"
        );
        after.commit().await.unwrap();
        store.close().await;
    }

    #[tokio::test]
    async fn cancelled_native_owner_drains_then_allows_close_reopen() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("store.sqlite3");
        let store = crate::Store::open(&path).await.unwrap();
        let ctx = Context::new("owner-test", "evaluator", Role::Evaluator).unwrap();
        let session = store.session().await.unwrap();
        let operation = begin_with_cancellation(
            session,
            ctx,
            serde_json::Value::Null,
            Arc::new(AtomicU8::new(CANCELLED)),
        );
        let outcome = operation.finish_outcome().await;
        assert!(
            outcome.native.rollback_confirmed
                && outcome.native.rollback_command_acknowledged
                && !outcome.native.terminal_unconfirmed
        );
        assert!(!outcome.native.commit_attempted && !outcome.native.commit_confirmed);
        let failure = outcome.result.unwrap_err();
        assert!(matches!(failure.error, Error::Cancelled));
        assert!(!failure.commit_outcome_unknown && !failure.commit_confirmed);
        assert!(failure.rollback_error.is_none());
        store.session().await.unwrap().commit().await.unwrap();
        store.close().await;
        let reopened = crate::Store::open(&path).await.unwrap();
        reopened.session().await.unwrap().commit().await.unwrap();
        reopened.close().await;
    }
}

#[cfg(test)]
mod original_projection_controls_r36 {
    use super::*;
    #[test]
    fn full_original_root16_group7_preserve_closed_overbudget_and_first_stop_r36() {
        let original = crate::budget::RootBudgetRecord {
            root_budget_id: "root-physical".into(),
            billing_scope: "billing".into(),
            authorizing_namespace: "n".into(),
            allowed_namespaces: vec!["n".into()],
            currency: "USD".into(),
            pricing_version: "price".into(),
            payment_subject: "payer".into(),
            authorization_receipt_digest: hash(b"authorization"),
            per_call_cap_micros: 10,
            total_limit_micros: 10,
            spent_micros: 100,
            reserved_micros: 7,
            stopped: true,
            stop_reason: Some("operator_stop".into()),
            stop_committed_at: Some(2),
            created_at: 1,
        };
        let projected = root_projection_r36(&original);
        projected.validate_wire(5_000).unwrap();
        assert_eq!(
            typed_fingerprint(&projected).unwrap(),
            typed_fingerprint(&original).unwrap()
        );
        let group = DispatchGroupRecord {
            billing_scope: "billing".into(),
            dispatch_group_id: "ticket".into(),
            owner_namespace: "n".into(),
            stopped: true,
            stop_reason: Some("operator_stop".into()),
            stop_committed_at: Some(2),
            created_at: 1,
        };
        let saved = group_projection_r36(&group);
        saved.validate_wire(5_000).unwrap();
        assert_eq!(
            typed_fingerprint(&saved).unwrap(),
            typed_fingerprint(&group).unwrap()
        );
        let mut later_finance = original.clone();
        later_finance.spent_micros += 3;
        assert_ne!(
            typed_fingerprint(&projected).unwrap(),
            typed_fingerprint(&root_projection_r36(&later_finance)).unwrap()
        );
        assert_eq!(projected.stop_reason.as_deref(), Some("operator_stop"));
        assert_eq!(saved.stop_reason, group.stop_reason);
        assert_eq!(projected.spent_micros, 100);
        // Different later finance cannot rewrite this saved original. The
        // projection/equality check itself is no Current or as-of permit.
    }
}

// Fixed original-RO source seam. This transport is deliberately NOT a Current
// result. The frozen-input authenticator/recompute factory is a separate,
// unfinished step and consumers must not substitute this type for it.
pub use authenticated_frozen_reader_r39::{NativeCurrentCompleteR39, NativeCurrentFormalR39};
pub(super) use native_formal_consumers_r49::NativeRecordOriginR49 as NativeConsumerOriginR49;
pub use native_formal_consumers_r49::{
    NativeCompilerClosureDataR49, NativeCompilerInputsDataR49, NativeHostEvidenceR49,
    NativeOperatorCredentialR49, NativeReleaseCredentialR49, validate_native_compiler_value_r49,
};
pub(crate) use native_formal_consumers_r49::{
    inspect_native_compiler_bundle_r49, inspect_native_compiler_inputs_r49,
    inspect_native_typed_closure_r49,
};
pub(super) use native_formal_consumers_r49::{
    load_native_origins_r49 as load_native_consumer_origins_r49, qualify_native_admin_r49,
    static_release_action_r49 as native_release_action_static_r49,
};
fn diagnostic_original_input_bound_r49(
    raw: &RawDatabase,
    ctx: &Context,
    id: &str,
    carrier: &FrozenCarrierR36,
) -> Result<bool> {
    for row in raw.tables.get("audit").ok_or(Error::ClockRegressed)? {
        let payload_raw = native_text(row, "payload")?;
        parse_owned_json(&payload_raw)?;
        let payload: AuditPayloadR24 =
            serde_json::from_str(&payload_raw).map_err(crate::internal)?;
        if payload.operation != COMPLETE_BINDING_OPERATION_R24 {
            continue;
        }
        require(payload.entity.len() <= 4 * 1024 * 1024)?;
        parse_owned_json(&payload.entity)?;
        let binding: CompleteBindingR24 =
            serde_json::from_str(&payload.entity).map_err(crate::internal)?;
        let summary = &binding.summary;
        if binding.report_ref.namespace != ctx.namespace() || binding.report_ref.physical_id != id {
            continue;
        }
        let ns = native_text(row, "namespace")?;
        require(
            binding.schema_version == "rsia.native.complete_binding.v1"
                && summary.schema_version == "rsia.native_asof.audit_summary.v1"
                && summary.namespace == ns
                && payload.actor == summary.actor
                && payload.role == Role::Evaluator
                && summary.role == Role::Evaluator
                && matches!(
                    summary.operation.as_str(),
                    "formal.native.asof.complete.v1"
                        | "formal.native.asof.grade.v1"
                        | "formal.native.asof.reconcile.v1"
                )
                && payload.timestamp >= summary.audit_timestamp
                && summary.capture_seconds >= 0
                && summary.capture_millis >= 0
                && summary.capture_millis / 1000 == summary.capture_seconds
                && summary.audit_timestamp >= summary.capture_seconds
                && binding.asof_sequence > summary.previous_audit_sequence
                && native_integer(row, "seq")? > binding.asof_sequence
                && native_text(row, "previous_hash")? == binding.asof_digest
                && binding.asof_previous == summary.previous_audit_hash
                && binding.insert_index < summary.acknowledged_writes,
        )?;
        let prior_tip = raw
            .tables
            .get("audit")
            .ok_or(Error::ClockRegressed)?
            .iter()
            .filter(|candidate| {
                native_text(candidate, "namespace").ok().as_deref() == Some(&ns)
                    && native_integer(candidate, "seq").is_ok_and(|seq| seq < binding.asof_sequence)
            })
            .map(|candidate| native_integer(candidate, "seq"))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .max()
            .unwrap_or(0);
        require(prior_tip == summary.previous_audit_sequence)?;
        let actual_asof = actual_audit_row_r24(raw, &ns, binding.asof_sequence)?;
        require(
            native_text(actual_asof, "payload")? == binding.asof_payload
                && native_text(actual_asof, "previous_hash")? == binding.asof_previous
                && native_text(actual_asof, "digest")? == binding.asof_digest
                && hash(format!("{}\n{}", binding.asof_previous, binding.asof_payload).as_bytes())
                    == binding.asof_digest,
        )?;
        parse_owned_json(&binding.asof_payload)?;
        let asof: AuditPayloadR24 =
            serde_json::from_str(&binding.asof_payload).map_err(crate::internal)?;
        let first_stop_entity = check_native_asof_entity_r44(&asof.entity, summary)?;
        if first_stop_entity.is_some() {
            validate_first_stop_asof_row_r44(raw, actual_asof, &asof, summary)?;
        }
        require(
            asof.actor == summary.actor
                && asof.role == summary.role
                && asof.operation == summary.operation
                && asof.timestamp == summary.audit_timestamp,
        )?;
        for value in [
            &summary.actual_ticket_row_digest,
            &summary.capture_database_digest,
            &summary.prepared_prefix_digest,
            &summary.closed_ack_prefix_digest,
            &summary.after_legacy_audit_prefix_digest,
            &binding.asof_digest,
            &binding.insert_row_digest,
        ] {
            digest(value)?;
        }
        if summary.previous_audit_sequence == 0 {
            require(summary.previous_audit_hash.is_empty())?;
        } else {
            digest(&summary.previous_audit_hash)?;
        }
        let reference = &binding.report_ref;
        require(
            reference.namespace == ns
                && reference.owner == summary.actor
                && reference.ticket_id == summary.ticket_id,
        )?;

        reference.validate_wire().map_err(r24_fault)?;
        require(
            reference.engine_version == evo_core::formal_version::ENGINE_V4
                && reference.input_schema_version == evo_core::formal_version::INPUT_SCHEMA_V2
                && reference.owner == carrier.input.ticket_snapshot.evaluator_actor
                && reference.input_digest == carrier.input_digest
                && reference.input_digest == typed_fingerprint(&carrier.input)?
                && reference.ticket_id == carrier.input.ticket_id
                && reference.ticket_digest == carrier.input.ticket_digest
                && carrier.input.ticket_digest
                    == typed_fingerprint(&carrier.input.ticket_snapshot)?,
        )?;
        // Current row/payload digests are deliberately NOT accepted here.
        // This branch can only diagnose corruption. The original full decoder
        // and real INSERT/body binding are still mandatory for every success.
        return Ok(true);
    }
    Ok(false)
}

pub(crate) use native_formal_consumers_r49::{
    begin_approval_r49, begin_authority_r49, begin_grade_capture_r49, begin_release_transition_r49,
};

pub(crate) fn reject_known_host_complete_connection_r49(
    connection: &rusqlite::Connection,
    ctx: &Context,
    ticket_id: &str,
) -> Result<()> {
    ctx.require(&[Role::Evaluator])?;
    identifier(ticket_id)?;
    require(!connection.is_autocommit())?;
    let raw = load_rusqlite_database(connection)?;
    require(!connection.is_autocommit())?;
    reject_known_host_complete_raw_r50(&raw, ctx, ticket_id)
}

pub(crate) fn reject_known_host_complete_raw_r50(
    raw: &RawDatabase,
    ctx: &Context,
    ticket_id: &str,
) -> Result<()> {
    ctx.require(&[Role::Evaluator])?;
    identifier(ticket_id)?;
    native_formal_consumers_r49::reject_known_complete_attack_r49(
        raw,
        ctx,
        &NativeIntent::Complete(serde_json::json!({
            "schema_version":COMPLETE_INTENT_SCHEMA,"ticket_id":ticket_id})),
    )
}

pub use fixed_formal_read_r38::{
    ClosedFormalReadR39, ClosedFormalSourceR38, FormalReadDataR49, FormalReadSelectorR38,
    FormalSourceClassR38, NativeReadRecordR39, NativeTypedReadSetR39,
};
pub(crate) use fixed_formal_read_r38::{
    capture_formal_snapshot_r39, capture_formal_source_r38, finish_formal_snapshot_r39,
    finish_formal_source_r38, prepare_formal_snapshot_from_raw_r50,
    prepare_formal_source_from_raw_r50,
};

pub(crate) use fixed_formal_read_r38::inspect_formal_data_r49;
mod native_formal_consumers_r49 {
    use super::*;
    use evo_core::contract::{
        CompileParts, HostCapabilities, ImproverPatch, Profile, ResolvedBundle, SkillPatch,
        SkillSnapshot, SystemSnapshot, compile_bundle, replay_resolved_bundle,
    };
    use evo_core::evaluation::CompleteEpisodeCostMeasurements;
    use evo_core::evidence::Purpose;
    use evo_core::formal_version as fv;
    use evo_core::{Strategy, Validate, fingerprint};
    use sqlx::ConnectOptions;
    const PROFILE_SCHEMA: &str = "rsia.frozen_compiler_profile.v1";
    const ROOT_SCHEMA: &str = "rsia.empty_root_authorization.v1";
    const BINDING_SCHEMA: &str = "rsia.formal_compiler_input_binding.v2";
    const FORMAL_SCHEMA: &str = "rsia.issued_formal_authority.v2";
    const HOST_SCHEMA: &str = "rsia.registered_formal_host.v1";
    const PROVIDER_SCHEMA: &str = "rsia.registered_external_provider.v1";
    const PROVIDER_CURRENT_SCHEMA: &str = "rsia.registered_external_provider.v2";
    const METER_SCHEMA: &str = "rsia.registered_resource_instrumentation.v1";
    const PRINCIPALS_SCHEMA: &str = "rsia.registered_formal_principals.v1";
    const PRINCIPAL_ACTION_SCHEMA: &str = "rsia.observed_principal_action.v1";
    const CONTROL_PRINCIPAL_ACTION_SCHEMA: &str = "rsia.observed_control_principal_action.v2";

    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct TypedContentRef {
        pub kind: String,
        pub id: String,
        pub schema_version: String,
        pub content_digest: String,
    }

    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct TypedContentEdge {
        pub from_kind: String,
        pub from_id: String,
        pub to_kind: String,
        pub to_id: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct TypedContentClosure {
        pub nodes: Vec<TypedContentRef>,
        pub edges: Vec<TypedContentEdge>,
        pub digest: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RegisteredFormalHost {
        pub id: String,
        pub schema_version: String,
        pub namespace: String,
        pub host_id: String,
        pub host_version: String,
        pub code_digest: String,
        pub grader_code_digest: String,
        pub host_uid: u32,
        pub grader_uid: u32,
        pub administrator_uid: u32,
        pub host_workspace: String,
        pub grader_workspace: String,
        pub grader_credential_environment_key: String,
        /// Authenticated credential/principal identities, not Context actor aliases.
        pub host_principal: String,
        pub grader_principal: String,
        pub administrator_principal: String,
        pub worker_principal: String,
        pub credential_binding_digest: String,
        pub isolated_workspace_digest: String,
        pub grader_workspace_digest: String,
        pub snapshot: SystemSnapshot,
        pub capabilities: HostCapabilities,
        pub host_surface_digest: String,
        pub adapter_digest: String,
        pub authorized_by: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RegisteredPrincipal {
        pub uid: u32,
        pub actor: String,
        pub principal: String,
        /// SHA256 commitment to a distinct private bearer credential. The bearer
        /// itself is accepted only over a kernel-authenticated local connection.
        pub credential_digest: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RegisteredPrincipalSet {
        pub id: String,
        pub schema_version: String,
        pub namespace: String,
        pub host_registration_id: String,
        pub version: u64,
        pub host: RegisteredPrincipal,
        pub grader: RegisteredPrincipal,
        pub administrator: RegisteredPrincipal,
        pub worker: RegisteredPrincipal,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct PrincipalSetObservation {
        id: String,
        schema_version: String,
        namespace: String,
        host_digest: String,
        principals: RegisteredPrincipalSet,
        grader_pid: u32,
        grader_uid: u32,
        grader_executable_digest: String,
        grader_workspace: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct PrincipalAction {
        id: String,
        schema_version: String,
        namespace: String,
        host_registration_id: String,
        host_digest: String,
        principal_set_digest: String,
        role: Role,
        actor: String,
        uid: u32,
        principal: String,
        credential_digest: String,
        subject_kind: String,
        subject_id: String,
        subject_content_digest: String,
        operation: String,
        authenticated_request_digest: String,
        peer_uid: u32,
        peer_gid: Option<u32>,
        peer_pid: Option<u32>,
        authority_pid: u32,
        authority_uid: u32,
        authority_executable_digest: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RegisteredExternalProvider {
        pub id: String,
        pub schema_version: String,
        pub namespace: String,
        pub host_registration_id: String,
        pub endpoint: String,
        pub provider_model: String,
        pub model_digest: String,
        pub transport_code_digest: String,
        pub credential_environment_key: String,
        pub credential_principal: String,
        pub currency: String,
        pub pricing_version: String,
        pub input_micros_per_million_tokens: u64,
        pub output_micros_per_million_tokens: u64,
        pub max_output_tokens: u32,
        pub authorized_by: String,
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    enum ExternalServiceCategory {
        #[serde(rename = "model")]
        Model,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RegisteredExternalProviderV2 {
        pub id: String,
        pub schema_version: String,
        pub namespace: String,
        pub host_registration_id: String,
        pub endpoint: String,
        pub provider_model: String,
        pub model_digest: String,
        pub transport_code_digest: String,
        pub credential_environment_key: String,
        pub credential_principal: String,
        pub currency: String,
        pub pricing_version: String,
        pub input_micros_per_million_tokens: u64,
        pub output_micros_per_million_tokens: u64,
        pub max_output_tokens: u32,
        pub authorized_by: String,
        pub service_category: ExternalServiceCategory,
        pub protocol_id: evo_core::model_protocol::ModelProtocolId,
        pub sampling_profile: evo_core::model_protocol::SamplingProfileId,
        pub sampling_digest: String,
        pub request_timeout_seconds: u32,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(untagged)]
    enum ProviderRegistrationWire {
        Legacy(RegisteredExternalProvider),
        Current(RegisteredExternalProviderV2),
    }

    #[derive(Debug, Clone)]
    struct FrozenProviderRegistration {
        record: Box<ProviderRegistrationWire>,
        common: Box<RegisteredExternalProvider>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FrozenCompilerProfile {
        pub id: String,
        pub schema_version: String,
        pub namespace: String,
        pub profile_id: String,
        pub version: u64,
        pub host_registration_id: String,
        pub provider_registration_id: String,
        pub resource_instrumentation_id: Option<String>,
        pub system_snapshot: SystemSnapshot,
        pub capabilities: HostCapabilities,
        pub environment_digest: String,
        pub baseline_skill: SkillSnapshot,
        pub baseline_strategy: Strategy,
        pub dependencies: Vec<TypedContentRef>,
        pub purpose: Purpose,
        pub application_order: Vec<String>,
        pub authorized_by: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct EmptyRootAuthorization {
        pub id: String,
        pub schema_version: String,
        pub namespace: String,
        pub profile_registration_id: String,
        pub profile_digest: String,
        pub host_registration_id: String,
        pub skill: SkillSnapshot,
        pub strategy: Strategy,
        pub authorized_by: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ProfileSelection {
        id: String,
        schema_version: String,
        profile_id: String,
        registration_id: String,
        registration_digest: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    enum ParentAuthority {
        EmptyRoot { id: String },
        ApprovedRelease { release_id: String },
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ParentSelection {
        id: String,
        schema_version: String,
        authority: ParentAuthority,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct CompilerInputRequest {
        pub profile: Profile,
        /// Search parent is distinct from the approved control-plane P.
        pub search_parent: SkillSnapshot,
        pub baseline: SkillSnapshot,
        /// Generation strategy of the search parent, not authority for approved P.
        pub search_parent_strategy: Strategy,
        pub expected_parent_strategy_digest: Option<String>,
        pub baseline_strategy: Strategy,
        pub capabilities: HostCapabilities,
        pub environment_digest: String,
        pub purpose: Purpose,
        pub revoke_watermark: u64,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FormalCompilerInputBinding {
        pub schema_version: String,
        pub namespace: String,
        pub profile_registration_id: String,
        pub profile_digest: String,
        pub parent_digest: String,
        pub baseline_digest: String,
        pub parent_skill_digest: String,
        pub parent_strategy_digest: String,
        pub search_parent_skill_digest: String,
        pub search_parent_strategy_digest: String,
        pub baseline_skill_digest: String,
        pub baseline_strategy_digest: String,
        pub environment_digest: String,
        pub capabilities_digest: String,
        pub purpose: Purpose,
        pub application_order: Vec<String>,
        pub watermark: Option<(i64, String)>,
        pub live_profile_pointer_digest: Option<String>,
        pub closure: TypedContentClosure,
        pub binding_digest: String,
    }

    #[derive(Debug, Clone)]
    struct VerifiedCompilerInputs {
        profile: FrozenCompilerProfile,
        parent_skill: SkillSnapshot,
        parent_strategy: Strategy,
        search_parent_strategy: Strategy,
        binding: FormalCompilerInputBinding,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FormalProviderCaptureRecord {
        pub id: String,
        pub schema_version: String,
        pub provider_registration_id: String,
        pub provider_registration_digest: String,
        pub host_registration_id: String,
        pub host_registration_digest: String,
        pub host_principal: String,
        pub host_pid: u32,
        pub host_uid: u32,
        pub host_code_digest: String,
        pub host_workspace: String,
        pub dispatch_id: String,
        pub request_digest: String,
        pub application_digest: String,
        pub bundle_digest: String,
        pub input_digest: String,
        pub output_digest: String,
        pub provider_request_id: String,
        pub usage_record_id: String,
        pub provider_response_digest: String,
        pub provider_response_utf8: String,
        pub actual_model_digest: String,
        pub input_tokens: u64,
        pub output_tokens: u64,
        pub actual_cost_micros: i64,
        pub currency: String,
        pub pricing_version: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct EpisodeCostRecord {
        pub id: String,
        pub schema_version: String,
        pub ticket_id: String,
        pub profile_registration_id: String,
        pub measurements: CompleteEpisodeCostMeasurements,
        pub measurement_sources: Vec<TypedContentRef>,
        pub observed_by: String,
        pub observed_pid: u32,
        pub observed_uid: u32,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct IndependentGradeCapture {
        pub id: String,
        pub schema_version: String,
        pub ticket_id: String,
        pub target_id: String,
        pub grader_receipt_digest: String,
        pub host_registration_id: String,
        pub grader_principal: String,
        pub grader_pid: u32,
        pub grader_uid: u32,
        pub grader_code_digest: String,
        pub grader_workspace: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct IssuedFormalAuthority {
        pub id: String,
        pub schema_version: String,
        pub report_id: String,
        pub raw_report_digest: String,
        pub ticket_digest: String,
        pub candidate_id: String,
        pub candidate_bundle_digest: String,
        pub environment_digest: String,
        pub compiler_binding: FormalCompilerInputBinding,
        pub billing_scope: String,
        pub root_seal_digest: String,
        pub episode_cost_digest: String,
        pub host_registration_id: String,
        pub principal_set_digest: String,
        pub report_principal_action_id: String,
        pub provider_captures: Vec<TypedContentRef>,
        pub grader_captures: Vec<TypedContentRef>,
        pub closure_roots: Vec<(String, String)>,
        pub closure: TypedContentClosure,
        pub watermark: Option<(i64, String)>,
        pub issued_by: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RegisteredResourceInstrumentation {
        pub id: String,
        pub schema_version: String,
        pub namespace: String,
        pub host_registration_id: String,
        pub endpoint: String,
        pub credential_environment_key: String,
        pub credential_principal: String,
        pub instrumentation_code_digest: String,
        pub authorized_by: String,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ResourceInstrumentationResponse {
        schema_version: String,
        id: String,
        request_digest: String,
        episode_id: String,
        root_budget_id: String,
        root_snapshot_digest: String,
        instrumentation_code_digest: String,
        complete_episode_root: bool,
        cpu_nanoseconds: Option<u64>,
        storage_byte_seconds: Option<u64>,
        retained_history_bytes: Option<u64>,
        human_nanoseconds: Option<u64>,
        gray_release_nanoseconds: Option<u64>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ResourceMeasurementRecord {
        id: String,
        schema_version: String,
        episode_id: String,
        ticket_id: String,
        billing_scope: String,
        root_snapshot_digest: String,
        host_registration_id: String,
        meter_registration_id: String,
        meter_registration_digest: String,
        observed_pid: u32,
        observed_uid: u32,
        dimension: String,
        unit: String,
        amount: u64,
        response_utf8: String,
        response_digest: String,
        request_digest: String,
    }

    impl RegisteredFormalHost {
        fn validate(&self, ctx: &Context) -> Result<()> {
            if self.schema_version != HOST_SCHEMA
                || self.namespace != ctx.namespace()
                || self.snapshot.host_id != self.host_id
                || self.snapshot.host_version != self.host_version
            {
                return Err(Error::Conflict("host registration scope mismatch".into()));
            }
            for v in [
                &self.id,
                &self.host_id,
                &self.host_version,
                &self.host_principal,
                &self.grader_principal,
                &self.administrator_principal,
                &self.worker_principal,
            ] {
                identifier(v)?;
            }
            let principals: BTreeSet<_> = [
                &self.host_principal,
                &self.grader_principal,
                &self.administrator_principal,
                &self.worker_principal,
            ]
            .into_iter()
            .collect();
            if principals.len() != 4
                || self.isolated_workspace_digest == self.grader_workspace_digest
            {
                return Err(Error::Forbidden);
            }
            if self.host_uid == self.grader_uid
                || self.host_uid == self.administrator_uid
                || self.grader_uid == self.administrator_uid
                || self.host_workspace == self.grader_workspace
            {
                return Err(Error::Forbidden);
            }
            digest(&self.grader_code_digest)?;
            for v in [
                &self.code_digest,
                &self.credential_binding_digest,
                &self.isolated_workspace_digest,
                &self.grader_workspace_digest,
                &self.host_surface_digest,
                &self.adapter_digest,
            ] {
                digest(v)?;
            }
            Ok(())
        }
    }

    impl RegisteredPrincipalSet {
        pub fn binding_digest(&self) -> Result<String> {
            fingerprint(self)
        }

        pub fn validate(&self, host: &RegisteredFormalHost) -> Result<()> {
            if self.schema_version != PRINCIPALS_SCHEMA
                || self.version == 0
                || self.namespace != host.namespace
                || self.host_registration_id != host.id
                || self.id != format!("formal-principals-{}", host.id)
                || self.host.uid != host.host_uid
                || self.grader.uid != host.grader_uid
                || self.administrator.uid != host.administrator_uid
                || self.host.principal != host.host_principal
                || self.grader.principal != host.grader_principal
                || self.administrator.principal != host.administrator_principal
                || self.worker.principal != host.worker_principal
                || self.binding_digest()? != host.credential_binding_digest
            {
                return Err(Error::Forbidden);
            }
            let all = [&self.host, &self.grader, &self.administrator, &self.worker];
            for p in all {
                identifier(&p.actor)?;
                identifier(&p.principal)?;
                digest(&p.credential_digest)?;
            }
            if all.iter().map(|p| p.uid).collect::<BTreeSet<_>>().len() != 4
                || all.iter().map(|p| &p.actor).collect::<BTreeSet<_>>().len() != 4
                || all
                    .iter()
                    .map(|p| &p.principal)
                    .collect::<BTreeSet<_>>()
                    .len()
                    != 4
                || all
                    .iter()
                    .map(|p| &p.credential_digest)
                    .collect::<BTreeSet<_>>()
                    .len()
                    != 4
            {
                return Err(Error::Forbidden);
            }
            Ok(())
        }
        fn principal(&self, role: Role) -> Result<&RegisteredPrincipal> {
            match role {
                Role::Host => Ok(&self.host),
                Role::Evaluator => Ok(&self.grader),
                Role::Admin => Ok(&self.administrator),
                Role::Worker => Ok(&self.worker),
                _ => Err(Error::Forbidden),
            }
        }
    }

    impl RegisteredExternalProvider {
        fn validate(&self, ctx: &Context) -> Result<()> {
            if self.schema_version != PROVIDER_SCHEMA
                || self.namespace != ctx.namespace()
                || !self.endpoint.starts_with("https://")
                || self.endpoint.contains('@')
                || self.max_output_tokens == 0
                || self.max_output_tokens > 32768
            {
                return Err(Error::Forbidden);
            }
            for v in [
                &self.id,
                &self.host_registration_id,
                &self.provider_model,
                &self.credential_environment_key,
                &self.credential_principal,
                &self.currency,
                &self.pricing_version,
            ] {
                identifier(v)?;
            }
            digest(&self.model_digest)?;
            digest(&self.transport_code_digest)?;
            Ok(())
        }
    }

    impl RegisteredExternalProviderV2 {
        fn validate_public_fields(&self) -> Result<()> {
            if self.schema_version != PROVIDER_CURRENT_SCHEMA
                || self.sampling_digest
                    != evo_core::model_protocol::sampling_digest_v1(
                        self.protocol_id,
                        self.sampling_profile,
                    )?
            {
                return Err(Error::Forbidden);
            }
            match self.service_category {
                ExternalServiceCategory::Model => {}
            }
            if !(1..=120).contains(&self.request_timeout_seconds) {
                return Err(Error::Invalid(
                    "external request timeout must be 1..120 seconds".into(),
                ));
            }
            let endpoint = parse_provider_url(
                &self.endpoint,
                sqlx::sqlite::SqliteConnectOptions::to_url_lossy,
            )
            .map_err(|_| Error::Forbidden)?;
            // Current's first endpoint profile is query-free. Legacy records keep
            // their original reader; credentials cannot enter new public material.
            if endpoint.scheme() != "https"
                || endpoint.host_str().is_none()
                || !endpoint.username().is_empty()
                || endpoint.password().is_some()
                || endpoint.query().is_some()
                || endpoint.fragment().is_some()
                || self.endpoint.chars().any(char::is_whitespace)
            {
                return Err(Error::Forbidden);
            }
            Ok(())
        }
        /// Pure data conversion, never a source attestation or registration.

        fn common_view(&self) -> RegisteredExternalProvider {
            RegisteredExternalProvider {
                id: self.id.clone(),
                schema_version: PROVIDER_SCHEMA.into(),
                namespace: self.namespace.clone(),
                host_registration_id: self.host_registration_id.clone(),
                endpoint: self.endpoint.clone(),
                provider_model: self.provider_model.clone(),
                model_digest: self.model_digest.clone(),
                transport_code_digest: self.transport_code_digest.clone(),
                credential_environment_key: self.credential_environment_key.clone(),
                credential_principal: self.credential_principal.clone(),
                currency: self.currency.clone(),
                pricing_version: self.pricing_version.clone(),
                input_micros_per_million_tokens: self.input_micros_per_million_tokens,
                output_micros_per_million_tokens: self.output_micros_per_million_tokens,
                max_output_tokens: self.max_output_tokens,
                authorized_by: self.authorized_by.clone(),
            }
        }

        // Nonsecret public registration inputs for the separate environment
        // material producer. This does not attest model or adapter source bytes.
    }

    impl FrozenProviderRegistration {
        fn from_record(record: ProviderRegistrationWire) -> Self {
            let common = match &record {
                ProviderRegistrationWire::Legacy(p) => p.clone(),
                ProviderRegistrationWire::Current(p) => p.common_view(),
            };
            Self {
                record: Box::new(record),
                common: Box::new(common),
            }
        }

        fn validate(&self, ctx: &Context) -> Result<()> {
            self.common.validate(ctx)?;
            if let ProviderRegistrationWire::Current(p) = self.record.as_ref() {
                p.validate_public_fields()?;
            }
            Ok(())
        }

        /* R49 removed unused copied helper require_legacy_profile */
    }

    impl FrozenCompilerProfile {
        pub fn baseline_digest(&self) -> Result<String> {
            fingerprint(self)
        }
        fn validate(&self, ctx: &Context) -> Result<()> {
            if self.schema_version != PROFILE_SCHEMA
                || self.namespace != ctx.namespace()
                || self.system_snapshot.profile_id != self.profile_id
                || self.version == 0
                || self.application_order != ["mandatory_context", "skill", "task"]
            {
                return Err(Error::Conflict(
                    "frozen profile identity/order mismatch".into(),
                ));
            }
            identifier(&self.id)?;
            identifier(&self.profile_id)?;
            digest(&self.environment_digest)?;
            self.baseline_skill.validate()?;
            self.baseline_strategy.validate()?;
            require_builtin_default_strategy(&self.baseline_strategy)?;
            if !self
                .capabilities
                .allows(&self.baseline_skill.required_capabilities)
            {
                return Err(Error::Forbidden);
            }
            Ok(())
        }
    }

    impl EmptyRootAuthorization {
        pub fn digest(&self) -> Result<String> {
            fingerprint(self)
        }
        fn validate(&self, ctx: &Context, profile: &FrozenCompilerProfile) -> Result<()> {
            if self.schema_version != ROOT_SCHEMA
                || self.namespace != ctx.namespace()
                || self.profile_registration_id != profile.id
                || self.profile_digest != fingerprint(profile)?
                || self.host_registration_id != profile.host_registration_id
                || fingerprint(&self.skill)? != fingerprint(&SkillSnapshot::empty())?
                || fingerprint(&self.strategy)? != fingerprint(&empty_root_strategy())?
            {
                return Err(Error::Forbidden);
            }
            Ok(())
        }
    }

    impl VerifiedCompilerInputs {
        pub fn compile(
            &self,
            skill_patch: &SkillPatch,
            improver_patch: &ImproverPatch,
        ) -> Result<ResolvedBundle> {
            let bundle = compile_bundle(CompileParts {
                profile: &Profile {
                    id: self.profile.profile_id.clone(),
                    evolution_enabled: true,
                    parent_digest: self.binding.parent_digest.clone(),
                    baseline_digest: self.binding.baseline_digest.clone(),
                },
                parent: &self.parent_skill,
                baseline: &self.profile.baseline_skill,
                parent_strategy: &self.parent_strategy,
                baseline_strategy: &self.profile.baseline_strategy,
                skill_patch,
                improver_patch,
                caps: &self.profile.capabilities,
                revoked: &BTreeSet::new(),
            })?;
            require_resolved_strategy(&bundle.improver, &self.parent_strategy)?;
            Ok(bundle)
        }

        pub fn replay(&self, bundle: &ResolvedBundle) -> Result<()> {
            require_resolved_strategy(&bundle.improver, &self.parent_strategy)?;
            if bundle.profile_id != self.profile.profile_id
                || bundle.parent_digest != self.binding.parent_digest
                || bundle.baseline_digest != self.binding.baseline_digest
            {
                return Err(Error::Conflict("bundle P/B identity mismatch".into()));
            }
            replay_resolved_bundle(
                bundle,
                &self.parent_skill,
                &self.profile.baseline_skill,
                &self.parent_strategy,
                &self.profile.baseline_strategy,
                &self.profile.capabilities,
                &BTreeSet::new(),
            )
        }
    }

    impl RegisteredResourceInstrumentation {
        fn validate(&self, ctx: &Context) -> Result<()> {
            if self.schema_version != METER_SCHEMA
                || self.namespace != ctx.namespace()
                || self.authorized_by.is_empty()
                || !self.endpoint.starts_with("https://")
                || self.endpoint.contains('@')
            {
                return Err(Error::Forbidden);
            }
            for id in [
                &self.id,
                &self.host_registration_id,
                &self.credential_environment_key,
            ] {
                identifier(id)?;
            }
            digest(&self.credential_principal)?;
            digest(&self.instrumentation_code_digest)
        }
    }

    impl std::ops::Deref for FrozenProviderRegistration {
        type Target = RegisteredExternalProvider;
        fn deref(&self) -> &Self::Target {
            &self.common
        }
    }

    impl Serialize for FrozenProviderRegistration {
        fn serialize<S: serde::Serializer>(
            &self,
            serializer: S,
        ) -> std::result::Result<S::Ok, S::Error> {
            self.record.serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for FrozenProviderRegistration {
        fn deserialize<D: serde::Deserializer<'de>>(
            deserializer: D,
        ) -> std::result::Result<Self, D::Error> {
            Ok(Self::from_record(ProviderRegistrationWire::deserialize(
                deserializer,
            )?))
        }
    }

    fn digest(value: &str) -> Result<()> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Invalid("expected lowercase SHA256".into()));
        }
        Ok(())
    }

    fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T> {
        if !v.is_object() {
            return Err(Error::Conflict(
                "stored authority must be a JSON object; quarantine".into(),
            ));
        }
        serde_json::from_value(v).map_err(|_| {
            Error::Conflict("stored authority schema is unreadable; quarantine".into())
        })
    }

    pub fn content_digest<T: Serialize>(body: &T) -> Result<String> {
        evo_core::canonical_fingerprint(body)
    }

    fn require_content_witness(
        s: &mut SourceR49<'_>,
        ctx: &Context,
        kind: &str,
        id: &str,
        body: &Value,
        role: Role,
    ) -> Result<()> {
        let operation = match role {
            Role::Admin => "formal.admin.content.v1",
            Role::Host => "formal.host.content.v1",
            Role::Evaluator => "formal.evaluator.content.v1",
            _ => return Err(Error::Forbidden),
        };
        let operation = format!("{operation}.{}", fingerprint(&(ctx.namespace(), kind, id))?);
        let entity = fingerprint(&(ctx.namespace(), kind, id, content_digest(body)?))?;
        if !s.has_role_audit_witness(ctx, role, &operation, &entity)? {
            return Err(Error::Conflict(
                "missing authenticated content authorization; quarantine".into(),
            ));
        }
        Ok(())
    }

    fn attested_read<T: serde::de::DeserializeOwned>(
        s: &mut SourceR49<'_>,
        ctx: &Context,
        kind: &str,
        id: &str,
        role: Role,
    ) -> Result<T> {
        let body: Value = s.need(ctx, kind, id)?;
        require_content_witness(s, ctx, kind, id, &body, role)?;
        if role == Role::Admin {
            require_registered_body_authority(s, ctx, kind, id, &body)?;
        }
        if kind == "release"
            && body["schema_version"] == evo_core::formal_version::PERSISTENT_RELEASE_V2
        {
            let wrapper: evo_core::formal_version::PersistentReleaseV2<MirrorPersistentRelease> =
                decode(body)?;
            wrapper.validate_known().map_err(r24_fault)?;
            return serde_json::from_value(
                serde_json::to_value(wrapper.release).map_err(crate::internal)?,
            )
            .map_err(crate::internal);
        }
        decode(body)
    }

    /* R49 removed unused copied helper compiler_dependency_schema */

    fn supported_node_schema(kind: &str, id: &str, body: &Value) -> Result<String> {
        if kind == "artifact" && id.starts_with("optgrant-") && body.get("schema_version").is_none()
        {
            let grant: evo_core::evidence::SourceSelection = decode(body.clone())?;
            grant.validate()?;
            if id != format!("optgrant-{}", fingerprint(&grant)?) {
                return Err(Error::Conflict("source grant identity alias".into()));
            }
            return Ok("rsia.legacy_source_selection.v1".into());
        }
        let schema = body
            .get("schema_version")
            .and_then(Value::as_str)
            .ok_or(Error::Forbidden)?;
        identifier(kind)?;
        identifier(id)?;
        if schema == evo_core::formal_version::SCHEMA {
            let reference = report_ref_from_value_r49(body)?;
            if reference.physical_id != id {
                return Err(Error::Conflict("formal carrier identity differs".into()));
            }
            return Ok(schema.into());
        }
        // This pure function validates the original stored type. Actual namespace,
        // liveness and authority are checked by SourceR49::need before it is called.
        let schema = body
            .get("schema_version")
            .and_then(Value::as_str)
            .ok_or(Error::Forbidden)?;
        if known_time_free_schema(kind, schema)
            || schema == "rsia.typed_artifact_envelope.v1"
            || schema == evo_core::formal_version::NATIVE_TICKET_ENVELOPE
            || schema == "rsia.optimization.source.v1"
        {
            return Ok(schema.into());
        }
        Err(Error::Conflict("unsupported typed dependency".into()))
    }

    fn read_typed_closure(
        s: &mut SourceR49<'_>,
        ctx: &Context,
        roots: &[(String, String)],
    ) -> Result<TypedContentClosure> {
        if roots.len() > 10_000 {
            return Err(Error::Conflict(
                "typed content root count exceeds bound".into(),
            ));
        }
        s.ensure_live(ctx, roots)?;
        let mut pending = roots.to_vec();
        let mut nodes = BTreeMap::new();
        let mut edges = BTreeSet::new();
        let mut total_bytes = 0usize;
        // Validated E05 envelopes at either end of a GA-18 attachment edge.
        let mut tickets = BTreeSet::new();
        let mut attached_facts = BTreeSet::new();
        while let Some((kind, id)) = pending.pop() {
            identifier(&kind)?;
            identifier(&id)?;
            if nodes.contains_key(&(kind.clone(), id.clone())) {
                continue;
            }
            if nodes.len() >= 10_000 {
                return Err(Error::Conflict(
                    "typed content closure exceeds bound".into(),
                ));
            }
            let body: Value = s.need(ctx, &kind, &id)?;
            let body_bytes = serde_json::to_vec(&body)
                .map_err(|_| Error::Internal)?
                .len();
            total_bytes = total_bytes
                .checked_add(body_bytes)
                .ok_or(Error::Forbidden)?;
            if body_bytes > 4 * 1024 * 1024 || total_bytes > 64 * 1024 * 1024 {
                return Err(Error::Conflict(
                    "typed dependency exceeds byte bound".into(),
                ));
            }
            let schema = supported_node_schema(&kind, &id, &body)?;
            if schema == "rsia.typed_artifact_envelope.v1"
                || schema == evo_core::formal_version::NATIVE_TICKET_ENVELOPE
            {
                match body["record_kind"].as_str() {
                    Some("evaluation_ticket_v2") => {
                        tickets.insert((kind.clone(), id.clone()));
                    }
                    Some("execution_receipt_v2" | "independent_grader_receipt_v2") => {
                        attached_facts.insert((kind.clone(), id.clone()));
                    }
                    _ => {}
                }
            }
            let authority_role = match schema.as_str() {
                PROFILE_SCHEMA
                | ROOT_SCHEMA
                | HOST_SCHEMA
                | PROVIDER_SCHEMA
                | PROVIDER_CURRENT_SCHEMA
                | METER_SCHEMA
                | "rsia.formal_profile_selection.v1"
                | "rsia.formal_parent_selection.v1"
                | "rsia.persistent_release.v1"
                | "rsia.persistent_release.v2"
                | "rsia.profile_pointer.v1"
                | "rsia.host_surface_record.v1" => Some(Role::Admin),
                FORMAL_SCHEMA
                | "rsia.issued_formal_authority.v3"
                | "rsia.independent_grade_capture.v1"
                | "rsia.independent_grade_capture.v2" => Some(Role::Evaluator),
                "rsia.actual_formal_provider_capture.v1"
                | "rsia.complete_episode_cost.v1"
                | "rsia.trusted_resource_meter.v1"
                | "rsia.run_application.v1"
                | "rsia.host_application.v1"
                | "rsia.host_execution_receipt.v1" => Some(Role::Host),
                _ => None,
            };
            if let Some(role) = authority_role {
                require_content_witness(s, ctx, &kind, &id, &body, role)?;
            }
            if schema == "rsia.issued_formal_authority.v3" {
                let wrapper: fv::FormalAuthorityV3<IssuedFormalAuthority> = decode(body.clone())?;
                wrapper.validate_known().map_err(r24_fault)?;
                s.origin_for_bound_r49(ctx, &kind, &id, Some(&wrapper.report_ref))?;
            } else if schema == "rsia.independent_grade_capture.v2" {
                // Its own native origin predates any report. The actual closure
                // and full Parent/current ticket checks bind its ticket separately.
                s.origin_for_bound_r49(ctx, &kind, &id, None)?;
            }
            if authority_role == Some(Role::Admin) {
                require_registered_body_authority(s, ctx, &kind, &id, &body)?;
            }
            if matches!(
                schema.as_str(),
                PRINCIPAL_ACTION_SCHEMA
                    | CONTROL_PRINCIPAL_ACTION_SCHEMA
                    | "rsia.release_action.v2"
            ) {
                require_content_witness(s, ctx, &kind, &id, &body, Role::Evaluator)?;
            }
            if schema == "rsia.formal_principal_set_observation.v1" {
                require_content_witness(s, ctx, &kind, &id, &body, Role::Admin)?;
                require_content_witness(s, ctx, &kind, &id, &body, Role::Evaluator)?;
            }
            if schema == "rsia.typed_artifact_envelope.v1"
                && matches!(
                    body["record_kind"].as_str(),
                    Some("registered_evaluation_control_v41" | "protected_holdout_v41")
                )
            {
                require_content_witness(s, ctx, &kind, &id, &body, Role::Evaluator)?;
            }

            let outgoing = s.dependency_edges(ctx, &kind, &id)?;
            for (to_kind, to_id) in outgoing {
                edges.insert(TypedContentEdge {
                    from_kind: kind.clone(),
                    from_id: id.clone(),
                    to_kind: to_kind.clone(),
                    to_id: to_id.clone(),
                });
                if edges.len() > 100_000 {
                    return Err(Error::Conflict("typed closure exceeds edge bound".into()));
                }
                pending.push((to_kind, to_id));
            }
            nodes.insert(
                (kind.clone(), id.clone()),
                TypedContentRef {
                    kind,
                    id,
                    schema_version: schema,
                    content_digest: fingerprint(&body)?,
                },
            );
        }
        // Kahn's algorithm detects a real directed cycle without recursion. A
        // ticket -> receipt attachment edge is not counted, so only a cycle it
        // closes is accepted; output order and digest below never depend on this.
        let mut degrees: BTreeMap<_, usize> = nodes.keys().cloned().map(|key| (key, 0)).collect();
        let mut children: BTreeMap<(String, String), Vec<(String, String)>> = BTreeMap::new();
        for edge in &edges {
            let parent = (edge.from_kind.clone(), edge.from_id.clone());
            let child = (edge.to_kind.clone(), edge.to_id.clone());
            if tickets.contains(&parent) && attached_facts.contains(&child) {
                continue;
            }
            *degrees.get_mut(&child).ok_or(Error::NotFound)? += 1;
            children.entry(parent).or_default().push(child);
        }
        let mut ready: Vec<_> = degrees
            .iter()
            .filter(|(_, n)| **n == 0)
            .map(|(key, _)| key.clone())
            .collect();
        let mut visited = 0;
        while let Some(key) = ready.pop() {
            visited += 1;
            if let Some(outgoing) = children.remove(&key) {
                for child in outgoing {
                    let n = degrees.get_mut(&child).ok_or(Error::NotFound)?;
                    *n -= 1;
                    if *n == 0 {
                        ready.push(child);
                    }
                }
            }
        }
        if visited != nodes.len() {
            return Err(Error::Conflict("typed content dependency cycle".into()));
        }
        let nodes: Vec<_> = nodes.into_values().collect();
        let edges: Vec<_> = edges.into_iter().collect();
        let digest = fingerprint(&(&nodes, &edges))?;
        Ok(TypedContentClosure {
            nodes,
            edges,
            digest,
        })
    }

    /* R49 removed unused copied helper verify_typed_closure */

    fn registered_subject_host(
        s: &mut SourceR49<'_>,
        ctx: &Context,
        kind: &str,
        body: &Value,
    ) -> Result<Option<RegisteredFormalHost>> {
        let schema = body
            .get("schema_version")
            .and_then(Value::as_str)
            .unwrap_or("");
        let host_id = match schema {
            HOST_SCHEMA => body.get("id").and_then(Value::as_str).map(str::to_owned),
            PROFILE_SCHEMA
            | ROOT_SCHEMA
            | PROVIDER_SCHEMA
            | PROVIDER_CURRENT_SCHEMA
            | METER_SCHEMA => body
                .get("host_registration_id")
                .and_then(Value::as_str)
                .map(str::to_owned),
            "rsia.formal_profile_selection.v1" => {
                let id = body["registration_id"].as_str().ok_or(Error::Forbidden)?;
                let profile: Value = s.need(ctx, "artifact", id)?;
                require_content_witness(s, ctx, "artifact", id, &profile, Role::Admin)?;
                Some(
                    profile["host_registration_id"]
                        .as_str()
                        .ok_or(Error::Forbidden)?
                        .into(),
                )
            }
            "rsia.formal_parent_selection.v1" => {
                let selection: ParentSelection = decode(body.clone())?;
                Some(match selection.authority {
                    ParentAuthority::EmptyRoot { id } => {
                        let root: Value = s.need(ctx, "artifact", &id)?;
                        require_content_witness(s, ctx, "artifact", &id, &root, Role::Admin)?;
                        root["host_registration_id"]
                            .as_str()
                            .ok_or(Error::Forbidden)?
                            .to_string()
                    }
                    ParentAuthority::ApprovedRelease { release_id } => {
                        let release: Value = s.need(ctx, "release", &release_id)?;
                        require_content_witness(
                            s,
                            ctx,
                            "release",
                            &release_id,
                            &release,
                            Role::Admin,
                        )?;
                        host_id_for_profile(
                            s,
                            ctx,
                            release.get("release").unwrap_or(&release)["profile_id"]
                                .as_str()
                                .ok_or(Error::Forbidden)?,
                        )?
                    }
                })
            }
            "rsia.persistent_release.v2" => {
                let release: fv::PersistentReleaseV2<MirrorPersistentRelease> =
                    decode(body.clone())?;
                release.validate_known().map_err(r24_fault)?;
                Some(host_id_for_profile(s, ctx, &release.release.profile_id)?)
            }
            "rsia.profile_pointer.v1" | "rsia.persistent_release.v1" => Some(host_id_for_profile(
                s,
                ctx,
                body["profile_id"].as_str().ok_or(Error::Forbidden)?,
            )?),
            _ => None,
        };
        if let Some(id) = host_id {
            let host_body: Value = s.need(ctx, "artifact", &id)?;
            require_content_witness(s, ctx, "artifact", &id, &host_body, Role::Admin)?;
            let host: RegisteredFormalHost = decode(host_body)?;
            host.validate(ctx)?;
            Ok(Some(host))
        } else {
            let _ = kind;
            Ok(None)
        }
    }

    fn host_id_for_profile(
        s: &mut SourceR49<'_>,
        ctx: &Context,
        profile_id: &str,
    ) -> Result<String> {
        let selection_id = format!("formal-profile-{profile_id}");
        let selection: Value = s.need(ctx, "artifact", &selection_id)?;
        require_content_witness(s, ctx, "artifact", &selection_id, &selection, Role::Admin)?;
        let profile_id = selection["registration_id"]
            .as_str()
            .ok_or(Error::Forbidden)?;
        let profile: Value = s.need(ctx, "artifact", profile_id)?;
        require_content_witness(s, ctx, "artifact", profile_id, &profile, Role::Admin)?;
        Ok(profile["host_registration_id"]
            .as_str()
            .ok_or(Error::Forbidden)?
            .into())
    }

    fn require_registered_body_authority(
        s: &mut SourceR49<'_>,
        ctx: &Context,
        kind: &str,
        id: &str,
        body: &Value,
    ) -> Result<()> {
        if let Some(host) = registered_subject_host(s, ctx, kind, body)?
            && s.get::<Value>(ctx, "artifact", &format!("formal-principals-{}", host.id))?
                .is_some()
        {
            require_principal_action(s, ctx, &host, kind, id, body, Role::Admin)?;
        }
        Ok(())
    }

    fn principal_set(
        s: &mut SourceR49<'_>,
        ctx: &Context,
        host: &RegisteredFormalHost,
    ) -> Result<RegisteredPrincipalSet> {
        let id = format!("formal-principals-{}", host.id);
        let body: Value = s.need(ctx, "artifact", &id)?;
        require_content_witness(s, ctx, "artifact", &id, &body, Role::Admin)?;
        require_content_witness(s, ctx, "artifact", &id, &body, Role::Evaluator)?;
        let observation: PrincipalSetObservation = decode(body)?;
        observation.principals.validate(host)?;
        if observation.id != id
            || observation.schema_version != "rsia.formal_principal_set_observation.v1"
            || observation.namespace != ctx.namespace()
            || observation.host_digest != fingerprint(host)?
            || observation.grader_uid != host.grader_uid
            || observation.grader_executable_digest != host.grader_code_digest
            || observation.grader_workspace != host.grader_workspace
            || observation.grader_pid == 0
        {
            return Err(Error::Forbidden);
        }
        Ok(observation.principals)
    }

    fn principal_action_id(kind: &str, id: &str, role: Role) -> Result<String> {
        Ok(format!(
            "principal-action-{}",
            fingerprint(&(kind, id, role))?
        ))
    }

    fn control_principal_action_id(kind: &str, id: &str) -> Result<String> {
        Ok(format!(
            "principal-action-{}",
            fingerprint(&(kind, id, Role::Evaluator, CONTROL_PRINCIPAL_ACTION_SCHEMA))?
        ))
    }

    fn control_request_digest(control: &MirrorRegisteredEvaluationControl) -> Result<String> {
        #[derive(Serialize)]
        #[serde(tag = "operation", rename_all = "snake_case")]
        enum Request<'a> {
            RegisterControl {
                control: &'a MirrorRegisteredEvaluationControl,
            },
        }
        fingerprint(&Request::RegisterControl { control })
    }

    fn require_principal_action<T: Serialize>(
        s: &mut SourceR49<'_>,
        ctx: &Context,
        host: &RegisteredFormalHost,
        kind: &str,
        id: &str,
        body: &T,
        role: Role,
    ) -> Result<RegisteredPrincipal> {
        // Closure validation is nested in ticket, report, and release readers. Keep
        // this authorization state machine off their default-size thread stacks.
        {
            let set = principal_set(s, ctx, host)?;
            let body_value = serde_json::to_value(body).map_err(|_| Error::Internal)?;
            let current_subject = s.get::<Value>(ctx, kind, id)?;
            let subject = if let Some(current) = current_subject
                && current["schema_version"] == evo_core::formal_version::PERSISTENT_RELEASE_V2
            {
                let wrapper: evo_core::formal_version::PersistentReleaseV2<
                    MirrorPersistentRelease,
                > = decode(current.clone())?;
                wrapper.validate_known().map_err(r24_fault)?;
                // The fixed callers inspect either this actual V2 wrapper or
                // its exact typed inner release. Both remain bound below to
                // the original wrapper's action digest and native Ref/origin.
                let supplied_digest = content_digest(&body_value)?;
                if supplied_digest != content_digest(&current)?
                    && supplied_digest != content_digest(&wrapper.release)?
                {
                    return Err(Error::Conflict("release wrapper/payload differs".into()));
                }
                current
            } else {
                body_value.clone()
            };
            let control = if role == Role::Evaluator
                && matches!(
                    body_value["schema_version"].as_str(),
                    Some(
                        MirrorRegisteredEvaluationControl::SCHEMA
                            | MirrorRegisteredEvaluationControl::SCHEMA_V1
                    )
                ) {
                Some(decode::<MirrorRegisteredEvaluationControl>(body_value)?)
            } else {
                None
            };
            let native_subject = matches!(
                subject["schema_version"].as_str(),
                Some(fv::PERSISTENT_RELEASE_V2 | "rsia.profile_pointer.v1")
            ) || (kind == "formal_parent"
                && s.latest_origin_r49(ctx, kind, id)?.is_some());
            let native_proof = if native_subject {
                let proof = s
                    .latest_origin_r49(ctx, kind, id)?
                    .ok_or(Error::Forbidden)?;
                let expected = if kind == "release" {
                    let release: fv::PersistentReleaseV2<MirrorPersistentRelease> =
                        decode(subject.clone())?;
                    Some(release.report_ref)
                } else if kind == "pointer" {
                    let pointer: MirrorPersistentProfilePointer = decode(subject.clone())?;
                    let release_id = pointer
                        .active_release_id
                        .as_deref()
                        .ok_or(Error::Forbidden)?;
                    let release: fv::PersistentReleaseV2<MirrorPersistentRelease> =
                        decode(s.need::<Value>(ctx, "release", release_id)?)?;
                    release.validate_known().map_err(r24_fault)?;
                    Some(release.report_ref)
                } else {
                    proof.report_ref.clone()
                };
                Some(s.origin_for_bound_r49(ctx, kind, id, expected.as_ref())?)
            } else {
                None
            };
            let action_id = if let Some(proof) = &native_proof {
                proof.action_id.clone().ok_or(Error::Forbidden)?
            } else if control.is_some() {
                control_principal_action_id(kind, id)?
            } else {
                principal_action_id(kind, id, role)?
            };
            if control.is_some()
                && s.get::<Value>(ctx, "artifact", &action_id)?.is_none()
                && s.get::<Value>(ctx, "artifact", &principal_action_id(kind, id, role)?)?
                    .is_some()
            {
                return Err(Error::Conflict(
                    "legacy control authorization cannot authorize new formal consumption".into(),
                ));
            }
            let action_body: Value = s.need(ctx, "artifact", &action_id)?;
            require_content_witness(
                s,
                ctx,
                "artifact",
                &action_id,
                &action_body,
                Role::Evaluator,
            )?;
            let action: PrincipalAction = if native_subject {
                let record: ReleaseActionR49 = decode(action_body)?;
                validate_release_action_r49(&record)?;
                let proof = native_proof.as_ref().ok_or(Error::Forbidden)?;
                if proof.action_id.as_deref() != Some(record.id.as_str())
                    || proof.request_digest.as_deref()
                        != Some(record.action.authenticated_request_digest.as_str())
                    || proof.peer_uid != Some(record.action.peer_uid)
                    || proof.peer_gid != record.action.peer_gid
                    || proof.peer_pid != record.action.peer_pid
                {
                    return Err(Error::Forbidden);
                }
                record.action
            } else {
                decode(action_body)?
            };
            let p = set.principal(role)?;
            if action.id != action_id
                || action.schema_version
                    != if control.is_some() {
                        CONTROL_PRINCIPAL_ACTION_SCHEMA
                    } else {
                        PRINCIPAL_ACTION_SCHEMA
                    }
                || action.namespace != ctx.namespace()
                || action.role != role
                || action.actor != p.actor
                || action.uid != p.uid
                || action.peer_uid != p.uid
                || action.principal != p.principal
                || action.credential_digest != p.credential_digest
                || action.host_registration_id != host.id
                || action.host_digest != fingerprint(host)?
                || action.principal_set_digest != set.binding_digest()?
                || action.subject_kind != kind
                || action.subject_id != id
                || action.subject_content_digest != content_digest(&subject)?
                || action.authority_uid != set.grader.uid
                || action.authority_executable_digest != host.grader_code_digest
                || action.authority_pid == 0
            {
                return Err(Error::Conflict(
                    "actual proposer/approver credential binding is missing or stale".into(),
                ));
            }
            digest(&action.authenticated_request_digest)?;
            if let Some(control) = &control
                && (action.operation != "register_control"
                    || action.authenticated_request_digest != control_request_digest(control)?
                    || action.peer_gid.is_none())
            {
                return Err(Error::Conflict(
                    "control authorization differs from the authenticated request".into(),
                ));
            }
            let valid_operation = match role {
                Role::Worker => action.operation == "stage",
                Role::Admin => matches!(
                    action.operation.as_str(),
                    "bootstrap"
                        | "register_provider"
                        | "register_meter"
                        | "freeze_profile"
                        | "empty_root"
                        | "approve"
                        | "approve_current_v2"
                        | "canary"
                        | "activate"
                        | "rollback"
                        | "canary_current_v2"
                        | "activate_current_v2"
                        | "rollback_current_v2"
                ),
                Role::Evaluator => matches!(
                    action.operation.as_str(),
                    "register_control" | "issue_report"
                ),
                _ => false,
            };
            if !valid_operation {
                return Err(Error::Forbidden);
            }
            if action.operation == "approve_current_v2" {
                let release: fv::PersistentReleaseV2<MirrorPersistentRelease> = if kind == "release"
                {
                    decode(subject.clone())?
                } else if kind == "formal_parent" {
                    let parent: ParentSelection = decode(subject.clone())?;
                    let ParentAuthority::ApprovedRelease { release_id } = parent.authority else {
                        return Err(Error::Forbidden);
                    };
                    decode(s.need::<Value>(ctx, "release", &release_id)?)?
                } else {
                    return Err(Error::Forbidden);
                };
                release.validate_known().map_err(r24_fault)?;
                let expected = fingerprint(&ApprovalCommandR49 {
                    operation: "approve_current_v2".into(),
                    candidate_id: release.release.candidate_id.clone(),
                    report_ref: release.report_ref.clone(),
                })?;
                let proof = native_proof.as_ref().ok_or(Error::Forbidden)?;
                if action.authenticated_request_digest != expected
                    || proof.request_digest.as_deref() != Some(expected.as_str())
                    || proof.peer_uid != Some(action.peer_uid)
                    || proof.peer_gid != action.peer_gid
                    || proof.peer_pid != action.peer_pid
                {
                    return Err(Error::Forbidden);
                }
            }
            Ok(p.clone())
        }
    }

    /* R49 removed unused copied helper verify_candidate_principals_from_binding */

    fn verify_candidate_principals(
        s: &mut SourceR49<'_>,
        ctx: &Context,
        host_id: &str,
        candidate: &MirrorReleaseCandidateRecord,
        control: &MirrorRegisteredEvaluationControl,
    ) -> Result<RegisteredPrincipalSet> {
        let host: RegisteredFormalHost = attested_read(s, ctx, "artifact", host_id, Role::Admin)?;
        let set = principal_set(s, ctx, &host)?;
        let worker = require_principal_action(
            s,
            ctx,
            &host,
            "artifact",
            &candidate.id,
            candidate,
            Role::Worker,
        )?;
        let grader = require_principal_action(
            s,
            ctx,
            &host,
            "artifact",
            &evaluation_storage_id("registered_evaluation_control_v41", &control.id)?,
            control,
            Role::Evaluator,
        )?;
        if candidate.proposer_actor != worker.actor
            || control.proposer_actor != worker.actor
            || control.evaluator_actor != grader.actor
            || control.approver_actor != set.administrator.actor
            || control.executor_actor != set.host.actor
            || control.evidence_scope != MirrorEvaluationEvidenceScope::RegisteredExternalHost
        {
            return Err(Error::Forbidden);
        }
        Ok(set)
    }

    fn verify_release_principal(
        s: &mut SourceR49<'_>,
        ctx: &Context,
        release: &MirrorPersistentRelease,
        candidate: &MirrorReleaseCandidateRecord,
    ) -> Result<()> {
        let selection: ProfileSelection = attested_read(
            s,
            ctx,
            "artifact",
            &format!("formal-profile-{}", candidate.profile_id),
            Role::Admin,
        )?;
        let profile: FrozenCompilerProfile =
            attested_read(s, ctx, "artifact", &selection.registration_id, Role::Admin)?;
        let host: RegisteredFormalHost = attested_read(
            s,
            ctx,
            "artifact",
            &profile.host_registration_id,
            Role::Admin,
        )?;
        let admin =
            require_principal_action(s, ctx, &host, "release", &release.id, release, Role::Admin)?;
        let worker = require_principal_action(
            s,
            ctx,
            &host,
            "artifact",
            &candidate.id,
            candidate,
            Role::Worker,
        )?;
        if admin.actor != release.approved_by
            || worker.actor != candidate.proposer_actor
            || worker.actor != release.proposer_actor
            || admin.uid == worker.uid
            || admin.principal == worker.principal
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }

    pub(crate) fn require_builtin_default_strategy(strategy: &Strategy) -> Result<()> {
        strategy.validate()?;
        if fingerprint(strategy)? != fingerprint(&Strategy::default())? {
            return Err(Error::Forbidden);
        }
        Ok(())
    }

    fn require_resolved_strategy(strategy: &Strategy, parent: &Strategy) -> Result<()> {
        strategy.validate()?;
        if require_builtin_default_strategy(strategy).is_ok() {
            return Ok(());
        }
        if fingerprint(parent)? == fingerprint(&empty_root_strategy())?
            && fingerprint(strategy)? == fingerprint(parent)?
        {
            return Ok(());
        }
        Err(Error::Forbidden)
    }

    pub(crate) fn require_builtin_parent_strategy(strategy: &Strategy) -> Result<()> {
        require_resolved_strategy(strategy, &empty_root_strategy())
    }

    pub fn empty_root_strategy() -> Strategy {
        Strategy {
            schema_version: "evo.strategy.v1".into(),
            instruction:
                "Preserve the unchanged host baseline; no evolution instruction is applied.".into(),
            max_candidates: 1,
            max_rounds: 1,
            require_counterexample: true,
        }
    }

    fn parent_selection(s: &mut SourceR49<'_>, ctx: &Context, id: &str) -> Result<ParentAuthority> {
        let selection: ParentSelection = attested_read(s, ctx, "formal_parent", id, Role::Admin)?;
        if selection.id != id || selection.schema_version != "rsia.formal_parent_selection.v1" {
            return Err(Error::Forbidden);
        }
        Ok(selection.authority)
    }

    fn verify_refs(s: &mut SourceR49<'_>, ctx: &Context, refs: &[TypedContentRef]) -> Result<()> {
        let mut keys = BTreeSet::new();
        for r in refs {
            if !keys.insert((&r.kind, &r.id)) {
                return Err(Error::Conflict("duplicate typed input".into()));
            }
            let body: Value = s.need(ctx, &r.kind, &r.id)?;
            if supported_node_schema(&r.kind, &r.id, &body)? != r.schema_version
                || fingerprint(&body)? != r.content_digest
            {
                return Err(Error::Conflict(
                    "typed input content/schema mismatch".into(),
                ));
            }
        }
        read_typed_closure(
            s,
            ctx,
            &refs
                .iter()
                .map(|r| (r.kind.clone(), r.id.clone()))
                .collect::<Vec<_>>(),
        )?;
        Ok(())
    }

    fn resolve_compiler_inputs(
        s: &mut SourceR49<'_>,
        ctx: &Context,
        request: &CompilerInputRequest,
    ) -> Result<VerifiedCompilerInputs> {
        ctx.require(&[Role::Worker, Role::Host, Role::Admin, Role::Evaluator])?;
        request.search_parent.validate()?;
        request.baseline.validate()?;
        request.search_parent_strategy.validate()?;
        request.baseline_strategy.validate()?;
        let selection: ProfileSelection = attested_read(
            s,
            ctx,
            "artifact",
            &format!("formal-profile-{}", request.profile.id),
            Role::Admin,
        )?;
        let profile: FrozenCompilerProfile =
            attested_read(s, ctx, "artifact", &selection.registration_id, Role::Admin)?;
        profile.validate(ctx)?;
        let watermark = s.watermark(ctx)?;
        if watermark.is_none() && request.revoke_watermark != 0 {
            return Err(Error::Conflict("missing source revoke watermark".into()));
        }
        let sequence = watermark
            .as_ref()
            .map(|w| u64::try_from(w.0).map_err(|_| Error::Conflict("unknown watermark".into())))
            .transpose()?
            .unwrap_or(0);
        if selection.schema_version != "rsia.formal_profile_selection.v1"
            || selection.profile_id != request.profile.id
            || selection.registration_digest != fingerprint(&profile)?
            || profile.environment_digest != request.environment_digest
            || profile.purpose != request.purpose
            || request.revoke_watermark != sequence
            || fingerprint(&profile.baseline_skill)? != fingerprint(&request.baseline)?
            || fingerprint(&profile.baseline_strategy)? != fingerprint(&request.baseline_strategy)?
            || fingerprint(&profile.capabilities)? != fingerprint(&request.capabilities)?
            || profile.baseline_digest()? != request.profile.baseline_digest
        {
            return Err(Error::Conflict(
                "full P/B profile/environment/version/capability binding mismatch".into(),
            ));
        }
        let pointer_id = format!("profile-pointer-{}", request.profile.id);
        let live_profile_pointer_digest = match s.get::<Value>(ctx, "pointer", &pointer_id)? {
            Some(body) => {
                require_content_witness(s, ctx, "pointer", &pointer_id, &body, Role::Admin)?;
                supported_node_schema("pointer", &pointer_id, &body)?;
                require_registered_body_authority(s, ctx, "pointer", &pointer_id, &body)?;
                Some(content_digest(&body)?)
            }
            None => None,
        };
        let authority = parent_selection(s, ctx, &request.profile.parent_digest)?;
        let (parent_skill, parent_strategy, parent_root) = match authority {
            ParentAuthority::EmptyRoot { id } => {
                let root: EmptyRootAuthorization =
                    attested_read(s, ctx, "artifact", &id, Role::Admin)?;
                root.validate(ctx, &profile)?;
                if root.digest()? != request.profile.parent_digest {
                    return Err(Error::Conflict("empty root digest mismatch".into()));
                }
                (root.skill, root.strategy, ("artifact".into(), id))
            }
            ParentAuthority::ApprovedRelease { release_id } => {
                let release: MirrorPersistentRelease =
                    attested_read(s, ctx, "release", &release_id, Role::Admin)?;
                if !matches!(
                    release.state,
                    MirrorPersistentReleaseState::Approved
                        | MirrorPersistentReleaseState::Canary
                        | MirrorPersistentReleaseState::Active
                        | MirrorPersistentReleaseState::Superseded
                ) {
                    return Err(Error::Forbidden);
                }
                let candidate: MirrorReleaseCandidateRecord =
                    s.need(ctx, "artifact", &release.candidate_id)?;
                verify_release_principal(s, ctx, &release, &candidate)?;
                verify_parent_release_r49(s, ctx, &release, &candidate)?;
                if release.profile_id != profile.profile_id
                    || release.environment_digest != profile.environment_digest
                    || release.bundle_digest != candidate.bundle.digest
                    || release.report_digest.is_empty()
                    || candidate.bundle.digest != request.profile.parent_digest
                {
                    return Err(Error::Conflict("approved P lineage mismatch".into()));
                }
                (
                    candidate.bundle.skill,
                    candidate.bundle.improver,
                    ("release".into(), release.id),
                )
            }
        };
        require_builtin_parent_strategy(&parent_strategy)?;
        require_resolved_strategy(&request.search_parent_strategy, &parent_strategy)?;
        let parent_strategy_digest = fingerprint(&parent_strategy)?;
        if request
            .expected_parent_strategy_digest
            .as_ref()
            .is_some_and(|expected| expected != &parent_strategy_digest)
        {
            return Err(Error::Conflict("P strategy body substitution".into()));
        }
        let mut roots = vec![
            parent_root,
            ("artifact".into(), profile.id.clone()),
            ("artifact".into(), selection.id),
            (
                "formal_parent".into(),
                request.profile.parent_digest.clone(),
            ),
        ];
        // Both original P and B dependencies are resolved before any Reset/Set.
        for dep in parent_skill
            .dependencies
            .iter()
            .chain(&profile.baseline_skill.dependencies)
            .chain(&request.search_parent.dependencies)
        {
            let run = s.get::<Value>(ctx, "run", dep)?.is_some();
            let artifact = s.get::<Value>(ctx, "artifact", dep)?.is_some();
            if run == artifact {
                return Err(Error::Conflict(
                    "missing or ambiguous untyped Skill dependency".into(),
                ));
            }
            roots.push((if run { "run" } else { "artifact" }.into(), dep.clone()));
        }
        let closure = read_typed_closure(s, ctx, &roots)?;
        let mut binding = FormalCompilerInputBinding {
            schema_version: BINDING_SCHEMA.into(),
            namespace: ctx.namespace().into(),
            profile_registration_id: profile.id.clone(),
            profile_digest: fingerprint(&profile)?,
            parent_digest: request.profile.parent_digest.clone(),
            baseline_digest: request.profile.baseline_digest.clone(),
            parent_skill_digest: fingerprint(&parent_skill)?,
            parent_strategy_digest,
            search_parent_skill_digest: fingerprint(&request.search_parent)?,
            search_parent_strategy_digest: fingerprint(&request.search_parent_strategy)?,
            baseline_skill_digest: fingerprint(&profile.baseline_skill)?,
            baseline_strategy_digest: fingerprint(&profile.baseline_strategy)?,
            environment_digest: profile.environment_digest.clone(),
            capabilities_digest: fingerprint(&profile.capabilities)?,
            purpose: request.purpose,
            application_order: profile.application_order.clone(),
            watermark,
            live_profile_pointer_digest,
            closure,
            binding_digest: String::new(),
        };
        binding.binding_digest = fingerprint(&binding)?;
        Ok(VerifiedCompilerInputs {
            profile,
            parent_skill,
            parent_strategy,
            search_parent_strategy: request.search_parent_strategy.clone(),
            binding,
        })
    }

    fn resolve_bundle_in_session(
        s: &mut SourceR49<'_>,
        ctx: &Context,
        bundle: &ResolvedBundle,
        environment_digest: &str,
        watermark: u64,
        purpose: Purpose,
    ) -> Result<VerifiedCompilerInputs> {
        let selection: ProfileSelection = attested_read(
            s,
            ctx,
            "artifact",
            &format!("formal-profile-{}", bundle.profile_id),
            Role::Admin,
        )?;
        let profile: FrozenCompilerProfile =
            attested_read(s, ctx, "artifact", &selection.registration_id, Role::Admin)?;
        let authority = parent_selection(s, ctx, &bundle.parent_digest)?;
        let parent_strategy = match authority {
            ParentAuthority::EmptyRoot { id } => {
                attested_read::<EmptyRootAuthorization>(s, ctx, "artifact", &id, Role::Admin)?
                    .strategy
            }
            ParentAuthority::ApprovedRelease { release_id } => {
                let release: MirrorPersistentRelease =
                    attested_read(s, ctx, "release", &release_id, Role::Admin)?;
                s.need::<MirrorReleaseCandidateRecord>(ctx, "artifact", &release.candidate_id)?
                    .bundle
                    .improver
            }
        };
        let verified = resolve_compiler_inputs(
            s,
            ctx,
            &CompilerInputRequest {
                profile: Profile {
                    id: bundle.profile_id.clone(),
                    evolution_enabled: true,
                    parent_digest: bundle.parent_digest.clone(),
                    baseline_digest: bundle.baseline_digest.clone(),
                },
                search_parent: bundle.skill.clone(),
                baseline: profile.baseline_skill,
                search_parent_strategy: bundle.improver.clone(),
                expected_parent_strategy_digest: Some(fingerprint(&parent_strategy)?),
                baseline_strategy: profile.baseline_strategy,
                capabilities: profile.capabilities,
                environment_digest: environment_digest.into(),
                purpose,
                revoke_watermark: watermark,
            },
        )?;
        verified.replay(bundle)?;
        // Set dependencies are new reads and are checked after replay as well.
        let output_roots = bundle
            .skill
            .dependencies
            .iter()
            .map(|id| {
                let found = verified.binding.closure.nodes.iter().find(|r| &r.id == id);
                found
                    .map(|r| (r.kind.clone(), r.id.clone()))
                    .ok_or_else(|| Error::Conflict("undeclared output dependency".into()))
            })
            .collect::<Result<Vec<_>>>()?;
        read_typed_closure(s, ctx, &output_roots)?;
        Ok(verified)
    }

    fn validate_provider_response(c: &FormalProviderCaptureRecord) -> Result<Value> {
        if c.schema_version != "rsia.actual_formal_provider_capture.v1"
            || c.provider_response_utf8.len() > 2 * 1024 * 1024
            || hash(c.provider_response_utf8.as_bytes()) != c.provider_response_digest
        {
            return Err(Error::Forbidden);
        }
        let v: Value =
            serde_json::from_str(&c.provider_response_utf8).map_err(|_| Error::Forbidden)?;
        let output = v["choices"][0]["message"]["content"]
            .as_str()
            .ok_or(Error::Forbidden)?;
        if v["id"].as_str() != Some(c.provider_request_id.as_str())
            || v["usage"]["prompt_tokens"].as_u64() != Some(c.input_tokens)
            || v["usage"]["completion_tokens"].as_u64() != Some(c.output_tokens)
            || hash(output.as_bytes()) != c.output_digest
            || c.usage_record_id
                != format!(
                    "usage-{}",
                    &fingerprint(&(c.provider_request_id.as_str(), &v["usage"]))?[..32]
                )
        {
            return Err(Error::Conflict(
                "provider original response does not bind capture".into(),
            ));
        }
        Ok(v)
    }

    fn verify_provider_registration(
        s: &mut SourceR49<'_>,
        ctx: &Context,
        c: &FormalProviderCaptureRecord,
        host: &RegisteredFormalHost,
    ) -> Result<()> {
        let p: FrozenProviderRegistration =
            attested_read(s, ctx, "artifact", &c.provider_registration_id, Role::Admin)?;
        p.validate(ctx)?;
        if matches!(p.record.as_ref(), ProviderRegistrationWire::Current(_)) {
            return Err(Error::Conflict(
                "current_provider_source_unavailable".into(),
            ));
        }
        let v = validate_provider_response(c)?;
        let micros = u128::from(c.input_tokens)
            .checked_mul(u128::from(p.input_micros_per_million_tokens))
            .and_then(|n| {
                n.checked_add(
                    u128::from(c.output_tokens) * u128::from(p.output_micros_per_million_tokens),
                )
            })
            .and_then(|n| n.checked_add(999_999))
            .ok_or(Error::Budget)?
            / 1_000_000;
        if fingerprint(&p)? != c.provider_registration_digest
            || fingerprint(host)? != c.host_registration_digest
            || p.host_registration_id != host.id
            || p.model_digest != c.actual_model_digest
            || v["model"].as_str() != Some(p.provider_model.as_str())
            || c.currency != p.currency
            || c.pricing_version != p.pricing_version
            || c.output_tokens > u64::from(p.max_output_tokens)
            || i64::try_from(micros).map_err(|_| Error::Budget)? != c.actual_cost_micros
        {
            return Err(Error::Conflict(
                "provider registration/model/pricing changed".into(),
            ));
        }
        Ok(())
    }

    fn strict_raw_object<T: serde::de::DeserializeOwned>(raw: &[u8], limit: usize) -> Result<T> {
        if raw.len() > limit
            || raw
                .iter()
                .find(|b| !matches!(**b, b' ' | b'\t' | b'\r' | b'\n'))
                != Some(&b'{')
        {
            return Err(Error::Forbidden);
        }
        // Decode original bytes directly: Value would collapse duplicate keys.
        serde_json::from_slice(raw).map_err(|_| Error::Forbidden)
    }

    fn validate_meter_response(
        r: &ResourceMeasurementRecord,
    ) -> Result<ResourceInstrumentationResponse> {
        if r.schema_version != "rsia.trusted_resource_meter.v1"
            || r.response_utf8.len() > 512 * 1024
            || hash(r.response_utf8.as_bytes()) != r.response_digest
        {
            return Err(Error::Forbidden);
        }
        let response: ResourceInstrumentationResponse =
            strict_raw_object(r.response_utf8.as_bytes(), 512 * 1024)?;
        if response.schema_version != "rsia.episode_resource_measurements.v1"
            || !response.complete_episode_root
            || response.episode_id != r.episode_id
            || response.root_snapshot_digest != r.root_snapshot_digest
            || response.request_digest != r.request_digest
        {
            return Err(Error::Forbidden);
        }
        Ok(response)
    }

    fn validate_episode_meter_sources(
        s: &mut SourceR49<'_>,
        ctx: &Context,
        profile: &FrozenCompilerProfile,
        host: &RegisteredFormalHost,
        ticket: &MirrorEvaluationTicketV2,
        control: &MirrorRegisteredEvaluationControl,
        seal: &crate::budget::SealedRootBudgetEvidence,
        measurements: &CompleteEpisodeCostMeasurements,
        sources: &[TypedContentRef],
    ) -> Result<()> {
        measurements.validate_complete()?;
        if measurements.episode_id != control.attempt_id
            || sources.len() != 5
            || !seal.monetary_complete
            || seal.root.billing_scope != control.billing_scope
        {
            return Err(Error::Forbidden);
        }
        let meter: RegisteredResourceInstrumentation = attested_read(
            s,
            ctx,
            "artifact",
            profile
                .resource_instrumentation_id
                .as_deref()
                .ok_or(Error::Forbidden)?,
            Role::Admin,
        )?;
        meter.validate(ctx)?;
        if meter.host_registration_id != host.id {
            return Err(Error::Forbidden);
        }
        let request = serde_json::json!({"schema_version":"rsia.episode_resource_measurement_request.v1",
        "namespace":ctx.namespace(),"ticket_id":ticket.id,"episode_id":control.attempt_id,
        "billing_scope":control.billing_scope,"root_budget_id":seal.root.root_budget_id,
        "root_snapshot_digest":seal.snapshot_digest,"host_registration_id":host.id,
        "call_count":seal.call_count,"dispatch_group_count":seal.dispatch_group_count,"stage_totals":seal.stage_totals});
        let request_digest = fingerprint(&request)?;
        let mut actual = BTreeMap::new();
        for source in sources {
            if source.kind != "artifact"
                || source.schema_version != "rsia.trusted_resource_meter.v1"
            {
                return Err(Error::Forbidden);
            }
            let r: ResourceMeasurementRecord =
                attested_read(s, ctx, "artifact", &source.id, Role::Host)?;
            let response = validate_meter_response(&r)?;
            if r.ticket_id != ticket.id
                || r.episode_id != control.attempt_id
                || r.billing_scope != control.billing_scope
                || r.root_snapshot_digest != seal.snapshot_digest
                || r.host_registration_id != host.id
                || r.observed_pid == 0
                || r.observed_uid != host.host_uid
                || r.meter_registration_id != meter.id
                || r.meter_registration_digest != fingerprint(&meter)?
                || response.instrumentation_code_digest != meter.instrumentation_code_digest
                || response.root_budget_id != seal.root.root_budget_id
                || r.request_digest != request_digest
                || actual
                    .insert(r.dimension.clone(), (r.unit.clone(), r.amount))
                    .is_some()
            {
                return Err(Error::Forbidden);
            }
            let amount = match r.dimension.as_str() {
                "cpu" => response.cpu_nanoseconds,
                "storage" => response.storage_byte_seconds,
                "history" => response.retained_history_bytes,
                "human" => response.human_nanoseconds,
                "gray_release" => response.gray_release_nanoseconds,
                _ => return Err(Error::Forbidden),
            };
            if amount != Some(r.amount) {
                return Err(Error::Forbidden);
            }
        }
        for (dimension, unit, amount) in [
            ("cpu", "nanoseconds", measurements.cpu_nanoseconds),
            ("storage", "byte_seconds", measurements.storage_byte_seconds),
            ("history", "bytes", measurements.retained_history_bytes),
            ("human", "nanoseconds", measurements.human_nanoseconds),
            (
                "gray_release",
                "nanoseconds",
                measurements.gray_release_nanoseconds,
            ),
        ] {
            if actual.get(dimension) != amount.map(|n| (unit.to_string(), n)).as_ref() {
                return Err(Error::Forbidden);
            }
        }
        Ok(())
    }

    // Only fixed entry functions below create this private adapter. Its reads
    // always refer to one actual native transaction; no caller SQL/callback or
    // pending-object selector is accepted.
    struct SourceR49<'a> {
        native: Option<&'a mut crate::Session>,
        raw: RawDatabase,
        actual: OccurrenceSnapshot,
        context: Context,
        reference: Option<evo_core::formal_version::ReportRef>,
        gate: Option<Arc<AtomicU8>>,
        pending: BTreeSet<(String, String)>,
        expected: RawDatabase,
        written: BTreeSet<(String, String)>,
        subjects: BTreeSet<(String, String)>,
    }
    impl<'a> SourceR49<'a> {
        fn captured(
            raw: &RawDatabase,
            ctx: &Context,
            reference: Option<&evo_core::formal_version::ReportRef>,
        ) -> Result<Self> {
            if let Some(reference) = reference {
                reference.validate_wire().map_err(r24_fault)?;
                if reference.namespace != ctx.namespace() {
                    return Err(Error::Forbidden);
                }
            }
            let (actual, _) = snapshot(raw)?;
            Ok(Self {
                native: None,
                raw: clone_database(raw),
                actual,
                context: ctx.clone(),
                reference: reference.cloned(),
                gate: None,
                pending: BTreeSet::new(),
                expected: clone_database(raw),
                written: BTreeSet::new(),
                subjects: BTreeSet::new(),
            })
        }
        async fn owned(
            session: &'a mut crate::Session,
            ctx: &Context,
            reference: Option<&evo_core::formal_version::ReportRef>,
            gate: Arc<AtomicU8>,
        ) -> Result<Self> {
            session.clock.check()?;
            check_cancel(&gate)?;
            let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
            require(
                SqliteTransactionManager::get_transaction_depth(&**tx) == 1
                    && tx.is_in_transaction(),
            )?;
            let raw = load_sqlx_database(tx).await?;
            let floor = collect_raw(&raw, &session.prepare_inputs, SchemaRead::Stable)?;
            let stamp = session.clock.seconds(&floor)?;
            session.clock.take_seconds(stamp)?;
            let mut source = Self::captured(&raw, ctx, reference)?;
            source.native = Some(session);
            source.gate = Some(gate);
            Ok(source)
        }
        fn same_scope(&self, ctx: &Context) -> Result<()> {
            if ctx.namespace() != self.context.namespace() {
                return Err(Error::Forbidden);
            }
            Ok(())
        }
        fn get<T: serde::de::DeserializeOwned>(
            &mut self,
            ctx: &Context,
            kind: &str,
            id: &str,
        ) -> Result<Option<T>> {
            self.same_scope(ctx)?;
            identifier(kind)?;
            identifier(id)?;
            let key = (ctx.namespace().to_owned(), kind.to_owned(), id.to_owned());
            let Some(object) = self.actual.objects.get(&key) else {
                return Ok(None);
            };
            e05_dependency_live(&self.actual, object)?;
            if object.body.get("schema_version").and_then(Value::as_str) == Some("rsia.redacted.v1")
            {
                qualify_redacted(object)?;
                return Err(r24_fault(
                    evo_core::formal_version::Fault::SourceInsufficient,
                ));
            }
            if self.pending.contains(&(kind.to_owned(), id.to_owned())) {
                time_free_record(object, &self.actual)?;
            } else if object.body.get("schema_version").and_then(Value::as_str)
                == Some(evo_core::formal_version::SCHEMA)
            {
                known_current_reference_r49(object)?;
            } else {
                e05_dependency_identity(&self.actual, object)?;
            }
            let raw = self
                .raw
                .tables
                .get("objects")
                .ok_or(Error::ClockRegressed)?
                .iter()
                .find(|row| {
                    native_text(row, "namespace").ok().as_deref() == Some(ctx.namespace())
                        && native_text(row, "kind").ok().as_deref() == Some(kind)
                        && native_text(row, "id").ok().as_deref() == Some(id)
                })
                .ok_or(Error::ClockRegressed)?;
            let text = native_text(raw, "body")?;
            // Native bytes, not a caller Value, select the strict codec.
            let value = parse_owned_json(&text)?;
            if value != object.body {
                return Err(Error::ClockRegressed);
            }
            serde_json::from_str(&text)
                .map(Some)
                .map_err(|_| Error::Conflict("formal native consumer stored type differs".into()))
        }
        fn need<T: serde::de::DeserializeOwned>(
            &mut self,
            ctx: &Context,
            kind: &str,
            id: &str,
        ) -> Result<T> {
            self.get(ctx, kind, id)?
                .ok_or_else(|| r24_fault(evo_core::formal_version::Fault::SourceInsufficient))
        }
        fn dependency_edges(
            &mut self,
            ctx: &Context,
            kind: &str,
            id: &str,
        ) -> Result<Vec<(String, String)>> {
            self.same_scope(ctx)?;
            let edges = e05_outgoing(&self.actual, ctx.namespace(), kind, id)?;
            if edges.len() > 10000 {
                return Err(Error::Conflict("dependency edge limit exceeded".into()));
            }
            Ok(edges)
        }
        fn watermark(&mut self, ctx: &Context) -> Result<Option<(i64, String)>> {
            self.same_scope(ctx)?;
            self.actual
                .watermarks
                .get(ctx.namespace())
                .map(|(seq, digest)| {
                    Ok((
                        i64::try_from(*seq).map_err(|_| Error::ClockRegressed)?,
                        digest.clone(),
                    ))
                })
                .transpose()
        }
        fn has_role_audit_witness(
            &mut self,
            ctx: &Context,
            role: Role,
            operation: &str,
            entity: &str,
        ) -> Result<bool> {
            self.same_scope(ctx)?;
            let role = match role {
                Role::Admin => "admin",
                Role::Evaluator => "evaluator",
                Role::Host => "host",
                _ => return Err(Error::Forbidden),
            };
            digest(entity)?;
            Ok(self
                .actual
                .witnesses
                .get(&(ctx.namespace().into(), role.into(), operation.into()))
                .is_some_and(|(stored, _)| stored == entity))
        }
        fn ensure_live(&mut self, ctx: &Context, roots: &[(String, String)]) -> Result<()> {
            self.same_scope(ctx)?;
            for (kind, id) in roots {
                let object = self
                    .actual
                    .objects
                    .get(&(ctx.namespace().into(), kind.clone(), id.clone()))
                    .ok_or_else(|| {
                        r24_fault(evo_core::formal_version::Fault::SourceInsufficient)
                    })?;
                e05_dependency_live(&self.actual, object)?;
            }
            Ok(())
        }
        fn budget_call(
            &mut self,
            ctx: &Context,
            billing: &str,
            id: &str,
        ) -> Result<Option<BudgetCallRecord>> {
            self.same_scope(ctx)?;
            if !self.actual.calls.contains_key(&(billing.into(), id.into())) {
                return Ok(None);
            }
            Ok(Some(
                qualified_call(&self.actual, ctx.namespace(), billing, id)?.clone(),
            ))
        }
        fn sealed_root_budget_evidence(
            &mut self,
            ctx: &Context,
            billing: &str,
        ) -> Result<Option<crate::budget::SealedRootBudgetEvidence>> {
            self.same_scope(ctx)?;
            let evidence = strict_latest_root_seal_r42(&self.raw, &self.actual, ctx, billing)?;
            evidence.evidence().map(|value| value.cloned())
        }
        fn qualify_prefix(&self, inputs: &PreparePreimages) -> Result<OccurrenceFloor> {
            same_database(&self.raw, &self.expected)?;
            let (actual, mut floor) = snapshot(&self.raw)?;
            let mut ticket_scan = TicketRelationScan {
                snapshot: &actual,
                checked_relations: BTreeSet::new(),
                successful_tickets: BTreeMap::new(),
            };
            for object in actual.objects.values() {
                if object.namespace == self.context.namespace()
                    && self
                        .pending
                        .contains(&(object.kind.clone(), object.id.clone()))
                {
                    // Only exact SQL-ACKed objects inserted by the fixed private
                    // business code receive the short witness/edge construction
                    // phase. Their own original static codec still runs.
                    time_free_record(object, &actual)?;
                } else {
                    qualify_object_with_ticket_scan(
                        object,
                        &actual,
                        inputs,
                        &mut floor,
                        Some(&mut ticket_scan),
                    )?;
                }
            }
            collect_caches(&self.raw, &actual, inputs, &mut floor)?;
            Ok(floor)
        }
        async fn admit(&mut self) -> Result<()> {
            let floor =
                self.qualify_prefix(&self.native.as_ref().ok_or(Error::Forbidden)?.prepare_inputs)?;
            let session = self.native.as_mut().ok_or(Error::Forbidden)?;
            session.clock.check()?;
            check_cancel(self.gate.as_ref().ok_or(Error::Forbidden)?)?;
            let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
            require(
                tx.is_in_transaction()
                    && SqliteTransactionManager::get_transaction_depth(&**tx) == 1,
            )?;
            let actual = load_sqlx_database(tx).await?;
            same_database(&actual, &self.expected)?;
            let stamp = session.clock.seconds(&floor)?;
            session.clock.take_seconds(stamp)?;
            Ok(())
        }
        async fn refresh_after_ack(&mut self) -> Result<()> {
            let session = self.native.as_mut().ok_or(Error::Forbidden)?;
            let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
            let raw = load_sqlx_database(tx).await?;
            same_database(&raw, &self.expected)?;
            let (actual, _) = snapshot(&raw)?;
            self.raw = raw;
            self.actual = actual;
            session.write_started = true;
            Ok(())
        }
        async fn put<T: Serialize>(
            &mut self,
            ctx: &Context,
            kind: &str,
            id: &str,
            owner: &str,
            body: &T,
        ) -> Result<()> {
            self.same_scope(ctx)?;
            if owner != ctx.actor() {
                return Err(Error::Forbidden);
            }
            let value = serde_json::to_value(body).map_err(crate::internal)?;
            check_consumer_write_r49(&self.reference, ctx, kind, id, &value)?;
            let body = serde_json::to_string(body).map_err(crate::internal)?;
            if body.len() > 4 * 1024 * 1024 {
                return Err(Error::Forbidden);
            }
            if let Some(old) = self.get::<Value>(ctx, kind, id)? {
                if content_digest(&old)? == content_digest(&value)? {
                    self.subjects.insert((kind.into(), id.into()));
                    return Ok(());
                }
                return Err(Error::Conflict(
                    "immutable authority identity reused".into(),
                ));
            }
            let write = IssueWrite::Object {
                before: None,
                after: OwnedRow {
                    cells: BTreeMap::from([
                        ("namespace".into(), OwnedCell::Text(ctx.namespace().into())),
                        ("kind".into(), OwnedCell::Text(kind.into())),
                        ("id".into(), OwnedCell::Text(id.into())),
                        ("owner".into(), OwnedCell::Text(owner.into())),
                        ("body".into(), OwnedCell::Text(body)),
                        ("revision".into(), OwnedCell::Integer(1)),
                    ]),
                },
            };
            self.admit().await?;
            write
                .execute(self.native.as_mut().ok_or(Error::Forbidden)?)
                .await?;
            write.acknowledge(&mut self.expected)?;
            self.pending.insert((kind.into(), id.into()));
            self.written.insert((kind.into(), id.into()));
            self.subjects.insert((kind.into(), id.into()));
            self.refresh_after_ack().await
        }

        async fn put_grade_receipt(&mut self, ctx: &Context, draft: &E05GradeDraft) -> Result<()> {
            self.same_scope(ctx)?;
            ctx.require(&[Role::Evaluator])?;
            require(
                draft.control.evidence_scope
                    == MirrorEvaluationEvidenceScope::RegisteredExternalHost
                    && !draft.replay
                    && !draft.audit_only
                    && !owned_is_stopping(draft.before.lifecycle),
            )?;
            draft.check_grade_inputs(&self.actual, ctx)?;
            let receipt = draft.receipt.as_ref().ok_or(Error::Internal)?;
            require(
                receipt.evaluator_actor == ctx.actor() && receipt.ticket_id == draft.before.id,
            )?;
            owned_validate_target_receipt_pair(
                draft.before.target(&receipt.target_id)?,
                &receipt.candidate_execution_receipt_id,
                &receipt.baseline_execution_receipt_id,
            )?;
            let physical = evaluation_storage_id(GRADER_RECEIPT_KIND, &receipt.receipt_id)?;
            let write = object_write(
                &self.raw,
                ctx,
                GRADER_RECEIPT_KIND,
                &receipt.receipt_id,
                receipt,
                false,
            )?;
            self.admit().await?;
            write
                .execute(self.native.as_mut().ok_or(Error::Forbidden)?)
                .await?;
            write.acknowledge(&mut self.expected)?;
            // This typed receipt has its complete ordinary qualifier, never a pending exemption.
            self.written.insert(("artifact".into(), physical.clone()));
            self.subjects.insert(("artifact".into(), physical.clone()));
            self.refresh_after_ack().await?;
            if draft.accepted {
                for id in [
                    &receipt.candidate_execution_receipt_id,
                    &receipt.baseline_execution_receipt_id,
                ] {
                    let dst = evaluation_storage_id(EXECUTION_RECEIPT_KIND, id)?;
                    self.put_edge(ctx, "artifact", &physical, "artifact", &dst)
                        .await?;
                }
            }
            Ok(())
        }

        async fn replace_release_row_r49<T: Serialize>(
            &mut self,
            ctx: &Context,
            kind: &str,
            id: &str,
            before_value: &Value,
            body: &T,
        ) -> Result<()> {
            self.same_scope(ctx)?;
            if ctx.role() != Role::Admin || !matches!(kind, "release" | "pointer") {
                return Err(Error::Forbidden);
            }
            let value = serde_json::to_value(body).map_err(crate::internal)?;
            check_consumer_write_r49(&self.reference, ctx, kind, id, &value)?;
            let object = self.actual.object(ctx.namespace(), kind, id)?;
            if content_digest(&object.body)? != content_digest(before_value)? {
                return Err(Error::Conflict("cas_lost".into()));
            }
            let before = self
                .raw
                .tables
                .get("objects")
                .ok_or(Error::ClockRegressed)?
                .iter()
                .find(|row| {
                    matches!(native_text(row,"namespace"),Ok(value) if value==ctx.namespace())
                        && matches!(native_text(row,"kind"),Ok(value) if value==kind)
                        && matches!(native_text(row,"id"),Ok(value) if value==id)
                })
                .ok_or(Error::NotFound)?
                .clone();
            let owner = native_text(&before, "owner")?;
            let revision = native_integer(&before, "revision")?
                .checked_add(1)
                .ok_or(Error::Internal)?;
            let body = serde_json::to_string(&value).map_err(crate::internal)?;
            if body.len() > 4 * 1024 * 1024 {
                return Err(Error::Forbidden);
            }
            let after = OwnedRow {
                cells: BTreeMap::from([
                    ("namespace".into(), OwnedCell::Text(ctx.namespace().into())),
                    ("kind".into(), OwnedCell::Text(kind.into())),
                    ("id".into(), OwnedCell::Text(id.into())),
                    ("owner".into(), OwnedCell::Text(owner)),
                    ("body".into(), OwnedCell::Text(body)),
                    ("revision".into(), OwnedCell::Integer(revision)),
                ]),
            };
            let write = IssueWrite::Object {
                before: Some(before),
                after,
            };
            self.admit().await?;
            write
                .execute(self.native.as_mut().ok_or(Error::Forbidden)?)
                .await?;
            write.acknowledge(&mut self.expected)?;
            self.pending.insert((kind.into(), id.into()));
            self.written.insert((kind.into(), id.into()));
            self.subjects.insert((kind.into(), id.into()));
            self.refresh_after_ack().await
        }

        async fn put_edge(
            &mut self,
            ctx: &Context,
            sk: &str,
            si: &str,
            dk: &str,
            di: &str,
        ) -> Result<()> {
            self.same_scope(ctx)?;
            if e05_outgoing(&self.actual, ctx.namespace(), sk, si)?
                .contains(&(dk.into(), di.into()))
            {
                return Ok(());
            }
            if !self.subjects.contains(&(sk.into(), si.into()))
                && !(sk == "artifact"
                    && self
                        .reference
                        .as_ref()
                        .is_some_and(|reference| si == reference.physical_id)
                    && di == principal_action_id("artifact", si, Role::Evaluator)?)
            {
                return Err(Error::Forbidden);
            }
            if e05_outgoing(&self.actual, ctx.namespace(), sk, si)?
                .contains(&(dk.into(), di.into()))
            {
                return Ok(());
            }
            let write = IssueWrite::Edge(OwnedRow {
                cells: BTreeMap::from([
                    ("namespace".into(), OwnedCell::Text(ctx.namespace().into())),
                    ("src_kind".into(), OwnedCell::Text(sk.into())),
                    ("src_id".into(), OwnedCell::Text(si.into())),
                    ("dst_kind".into(), OwnedCell::Text(dk.into())),
                    ("dst_id".into(), OwnedCell::Text(di.into())),
                ]),
            });
            self.admit().await?;
            write
                .execute(self.native.as_mut().ok_or(Error::Forbidden)?)
                .await?;
            write.acknowledge(&mut self.expected)?;
            self.refresh_after_ack().await
        }
        async fn audit(&mut self, ctx: &Context, operation: &str, entity: &str) -> Result<()> {
            self.same_scope(ctx)?;
            // This adapter is private to the three closed consumers. Public
            // Session::audit never receives their reserved origin operation.
            self.admit().await?;
            let (previous_seq, previous) = native_audit_tip(&self.raw, ctx.namespace())?;
            let floor =
                self.qualify_prefix(&self.native.as_ref().ok_or(Error::Forbidden)?.prepare_inputs)?;
            let session = self.native.as_mut().ok_or(Error::Forbidden)?;
            let stamp = session.clock.seconds(&floor)?;
            let timestamp = session.clock.take_seconds(stamp)?;
            let payload = serde_json::json!({"actor":ctx.actor(),"role":ctx.role(),
                "operation":operation,"entity":entity,"timestamp":timestamp})
            .to_string();
            let digest = hash(format!("{previous}\n{payload}").as_bytes());
            let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
            let seq:i64=sqlx::query_scalar(
                "INSERT INTO audit(namespace,payload,previous_hash,digest) VALUES(?,?,?,?) RETURNING seq")
                .bind(ctx.namespace()).bind(&payload).bind(&previous).bind(&digest)
                .fetch_one(&mut **tx).await.map_err(crate::internal)?;
            require(seq > previous_seq)?;
            self.expected
                .tables
                .get_mut("audit")
                .ok_or(Error::ClockRegressed)?
                .push(OwnedRow {
                    cells: BTreeMap::from([
                        ("seq".into(), OwnedCell::Integer(seq)),
                        ("namespace".into(), OwnedCell::Text(ctx.namespace().into())),
                        ("payload".into(), OwnedCell::Text(payload)),
                        ("previous_hash".into(), OwnedCell::Text(previous)),
                        ("digest".into(), OwnedCell::Text(digest)),
                    ]),
                });
            self.refresh_after_ack().await
        }
        async fn stable(&mut self) -> Result<()> {
            let session = self.native.as_mut().ok_or(Error::Forbidden)?;
            let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
            let raw = load_sqlx_database(tx).await?;
            same_database(&raw, &self.expected)?;
            let floor = collect_raw(&raw, &session.prepare_inputs, SchemaRead::Stable)?;
            let stamp = session.clock.seconds(&floor)?;
            session.clock.take_seconds(stamp)?;
            self.pending.clear();
            Ok(())
        }
    }

    pub(super) const NATIVE_RECORD_OPERATION_R49: &str = "formal.native.consumer.record.v1";
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(crate) struct NativeRecordOriginR49 {
        schema_version: String,
        namespace: String,
        kind: String,
        id: String,
        body_content_digest: String,
        report_ref: Option<evo_core::formal_version::ReportRef>,
        grader_actor: String,
        grader_pid: u32,
        grader_uid: u32,
        grader_code_digest: String,
        grader_workspace: String,
        peer_uid: Option<u32>,
        peer_gid: Option<u32>,
        peer_pid: Option<u32>,
        request_digest: Option<String>,
        action_id: Option<String>,
        previous_content_digest: Option<String>,
    }
    #[derive(Debug, Clone)]
    struct ObservedFieldsR49 {
        pid: u32,
    }
    fn observe_registered_process_local_r49(
        uid: u32,
        code: &str,
        workspace: &str,
    ) -> Result<evo_core::formal_version::NativeObservedProcessR49> {
        evo_core::formal_version::observe_registered_process_r49(uid, code, workspace)
    }
    fn process_principal(
        ctx: &Context,
        host: &RegisteredFormalHost,
        set: &RegisteredPrincipalSet,
        role: Role,
    ) -> Result<evo_core::formal_version::NativeObservedProcessR49> {
        let principal = set.principal(role)?;
        if role != Role::Evaluator || ctx.role() != role || ctx.actor() != principal.actor {
            return Err(Error::Forbidden);
        }
        let bearer =
            std::env::var(&host.grader_credential_environment_key).map_err(|_| Error::Forbidden)?;
        if bearer.len() < 32
            || bearer.len() > 4096
            || hash(bearer.as_bytes()) != principal.credential_digest
        {
            return Err(Error::Forbidden);
        }
        observe_registered_process_local_r49(
            principal.uid,
            &host.grader_code_digest,
            &host.grader_workspace,
        )
    }
    async fn immutable<T: Serialize>(
        source: &mut SourceR49<'_>,
        ctx: &Context,
        kind: &str,
        id: &str,
        body: &T,
    ) -> Result<()> {
        if let Some(old) = source.get::<Value>(ctx, kind, id)? {
            if content_digest(&old)? != content_digest(body)? {
                return Err(Error::Conflict(
                    "immutable authority identity reused".into(),
                ));
            }
            return Ok(());
        }
        source.put(ctx, kind, id, ctx.actor(), body).await
    }
    async fn attest_content<T: Serialize>(
        source: &mut SourceR49<'_>,
        ctx: &Context,
        kind: &str,
        id: &str,
        body: &T,
    ) -> Result<()> {
        let prefix = match ctx.role() {
            Role::Admin => "formal.admin.content.v1",
            Role::Evaluator => "formal.evaluator.content.v1",
            Role::Host => "formal.host.content.v1",
            _ => return Err(Error::Forbidden),
        };
        let operation = format!("{prefix}.{}", fingerprint(&(ctx.namespace(), kind, id))?);
        let entity = fingerprint(&(ctx.namespace(), kind, id, content_digest(body)?))?;
        source.audit(ctx, &operation, &entity).await
    }
    async fn record_grader_action<T: Serialize>(
        source: &mut SourceR49<'_>,
        ctx: &Context,
        host: &RegisteredFormalHost,
        kind: &str,
        id: &str,
        body: &T,
        operation: &str,
    ) -> Result<()> {
        if operation != "issue_report" {
            return Err(Error::Forbidden);
        }
        let set = principal_set(source, ctx, host)?;
        let observed = process_principal(ctx, host, &set, Role::Evaluator)?;
        let principal = set.principal(Role::Evaluator)?;
        if s_has_native_action_r49(source, ctx, host, &set, kind, id, body)? {
            return Ok(());
        }
        let action = PrincipalAction {
            id: principal_action_id(kind, id, Role::Evaluator)?,
            schema_version: PRINCIPAL_ACTION_SCHEMA.into(),
            namespace: ctx.namespace().into(),
            host_registration_id: host.id.clone(),
            host_digest: fingerprint(host)?,
            principal_set_digest: set.binding_digest()?,
            role: Role::Evaluator,
            actor: principal.actor.clone(),
            uid: principal.uid,
            principal: principal.principal.clone(),
            credential_digest: principal.credential_digest.clone(),
            subject_kind: kind.into(),
            subject_id: id.into(),
            subject_content_digest: content_digest(body)?,
            operation: operation.into(),
            authenticated_request_digest: fingerprint(&(operation, kind, id, body))?,
            peer_uid: observed.uid(),
            peer_gid: None,
            peer_pid: Some(observed.pid()),
            authority_pid: observed.pid(),
            authority_uid: observed.uid(),
            authority_executable_digest: observed.executable_digest().to_owned(),
        };
        immutable(source, ctx, "artifact", &action.id, &action).await?;
        attest_content(source, ctx, "artifact", &action.id, &action).await?;
        source
            .put_edge(ctx, "artifact", &action.id, "artifact", &set.id)
            .await?;
        source
            .put_edge(ctx, kind, id, "artifact", &action.id)
            .await?;
        source
            .origin(ctx, "artifact", &action.id, &observed, None)
            .await
    }
    impl SourceR49<'_> {
        async fn origin(
            &mut self,
            ctx: &Context,
            kind: &str,
            id: &str,
            observed: &evo_core::formal_version::NativeObservedProcessR49,
            peer: Option<(&tokio::net::unix::UCred, &str)>,
        ) -> Result<()> {
            let actual: Value = self.need(ctx, kind, id)?;
            let previous = self.latest_origin_r49(ctx, kind, id)?;
            let actual_digest = content_digest(&actual)?;
            let action_id = self
                .actual
                .objects
                .values()
                .filter(|object| {
                    object.namespace == ctx.namespace()
                        && object.kind == "artifact"
                        && object.body["schema_version"] == "rsia.release_action.v2"
                })
                .map(|object| decode::<ReleaseActionR49>(object.body.clone()))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .filter(|record| {
                    record.action.subject_kind == kind
                        && record.action.subject_id == id
                        && record.action.subject_content_digest == actual_digest
                        && peer.as_ref().is_some_and(|(_, request)| {
                            record.action.authenticated_request_digest == *request
                        })
                })
                .map(|record| record.id)
                .collect::<Vec<_>>();
            if action_id.len() > 1 {
                return Err(Error::Forbidden);
            }
            let proof = NativeRecordOriginR49 {
                schema_version: "rsia.native.consumer_record_origin.v1".into(),
                namespace: ctx.namespace().into(),
                kind: kind.into(),
                id: id.into(),
                body_content_digest: content_digest(&actual)?,
                report_ref: self.reference.clone(),
                grader_actor: ctx.actor().into(),
                grader_pid: observed.pid(),
                grader_uid: observed.uid(),
                grader_code_digest: observed.executable_digest().to_owned(),
                grader_workspace: observed.workspace().to_owned(),
                peer_uid: peer.as_ref().map(|(peer, _)| peer.uid()),
                peer_gid: peer.as_ref().map(|(peer, _)| peer.gid()),
                peer_pid: peer
                    .as_ref()
                    .and_then(|(peer, _)| peer.pid())
                    .map(u32::try_from)
                    .transpose()
                    .map_err(|_| Error::Forbidden)?,
                request_digest: peer.map(|(_, request)| request.into()),
                action_id: action_id.into_iter().next(),
                previous_content_digest: previous.map(|proof| proof.body_content_digest),
            };
            let entity = serde_json::to_string(&proof).map_err(crate::internal)?;
            self.audit(ctx, NATIVE_RECORD_OPERATION_R49, &entity).await
        }
        fn origin_for(&self, ctx: &Context, kind: &str, id: &str) -> Result<NativeRecordOriginR49> {
            self.origin_for_bound_r49(ctx, kind, id, self.reference.as_ref())
        }

        fn latest_origin_r49(
            &self,
            ctx: &Context,
            kind: &str,
            id: &str,
        ) -> Result<Option<NativeRecordOriginR49>> {
            self.same_scope(ctx)?;
            let mut found: Option<NativeRecordOriginR49> = None;
            let mut ordered = self
                .raw
                .tables
                .get("audit")
                .ok_or(Error::ClockRegressed)?
                .iter()
                .map(|row| Ok((native_integer(row, "seq")?, row)))
                .collect::<Result<Vec<_>>>()?;
            ordered.sort_by_key(|(seq, _)| *seq);
            for (_, row) in ordered {
                if native_text(row, "namespace")? != ctx.namespace() {
                    continue;
                }
                let payload: AuditPayloadR24 =
                    serde_json::from_str(&native_text(row, "payload")?).map_err(crate::internal)?;
                if payload.operation != NATIVE_RECORD_OPERATION_R49 {
                    continue;
                }
                let proof: NativeRecordOriginR49 =
                    serde_json::from_str(&payload.entity).map_err(|_| Error::Forbidden)?;
                if proof.kind != kind || proof.id != id {
                    continue;
                }
                if proof.schema_version != "rsia.native.consumer_record_origin.v1"
                    || proof.namespace != ctx.namespace()
                    || payload.role != Role::Evaluator
                    || payload.actor != proof.grader_actor
                    || proof.grader_pid == 0
                    || proof.previous_content_digest
                        != found.as_ref().map(|old| old.body_content_digest.clone())
                {
                    return Err(Error::Forbidden);
                }
                if let Some(action_id) = &proof.action_id {
                    let action_object =
                        self.actual.object(ctx.namespace(), "artifact", action_id)?;
                    let action: ReleaseActionR49 = decode(action_object.body.clone())?;
                    validate_release_action_r49(&action)?;
                    if action.action.subject_kind != kind
                        || action.action.subject_id != id
                        || action.action.subject_content_digest != proof.body_content_digest
                        || action.previous_action_id
                            != found.as_ref().and_then(|old| old.action_id.clone())
                    {
                        return Err(Error::Forbidden);
                    }
                }
                found = Some(proof);
            }
            Ok(found)
        }

        fn origin_for_bound_r49(
            &self,
            ctx: &Context,
            kind: &str,
            id: &str,
            expected: Option<&fv::ReportRef>,
        ) -> Result<NativeRecordOriginR49> {
            self.same_scope(ctx)?;
            let object = self.actual.object(ctx.namespace(), kind, id)?;
            let proof = self
                .latest_origin_r49(ctx, kind, id)?
                .ok_or_else(|| r24_fault(fv::Fault::SourceInsufficient))?;
            if proof.body_content_digest != content_digest(&object.body)?
                || (proof.report_ref.as_ref() != expected
                    && !(proof.report_ref.is_none()
                        && object.body["schema_version"] == "rsia.independent_grade_capture.v2"
                        && expected.is_some_and(|reference| {
                            object.body["ticket_id"] == reference.ticket_id
                        })))
            {
                return Err(Error::Forbidden);
            }
            Ok(proof)
        }
    }
    fn observed_native_report_action_r49(
        s: &mut SourceR49<'_>,
        ctx: &Context,
        host: &RegisteredFormalHost,
        set: &RegisteredPrincipalSet,
        reference: &evo_core::formal_version::ReportRef,
    ) -> Result<ObservedFieldsR49> {
        let action_id = principal_action_id("artifact", &reference.physical_id, Role::Evaluator)?;
        let body: Value = s.need(ctx, "artifact", &reference.physical_id)?;
        require_principal_action(
            s,
            ctx,
            host,
            "artifact",
            &reference.physical_id,
            &body,
            Role::Evaluator,
        )?;
        let proof = s.origin_for(ctx, "artifact", &action_id)?;
        if proof.grader_actor != set.grader.actor
            || proof.grader_pid == 0
            || proof.grader_uid != host.grader_uid
            || proof.grader_uid != set.grader.uid
            || proof.grader_code_digest != host.grader_code_digest
            || proof.grader_workspace != host.grader_workspace
            || proof.report_ref.as_ref() != Some(reference)
        {
            return Err(Error::Forbidden);
        }
        Ok(ObservedFieldsR49 {
            pid: proof.grader_pid,
        })
    }
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct NativeBrokerTaskR49 {
        ticket_id: String,
        target_id: String,
        input_artifact_id: String,
        input_digest: String,
        side: MirrorExecutionSide,
        bundle_digest: String,
        environment_digest: String,
        request_digest: String,
    }
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct MirrorFormalTaskRequest {
        task: NativeBrokerTaskR49,
        candidate_id: String,
        profile_registration_id: String,
        compiler_binding_digest: String,
        model_digest: String,
        mandatory_context: String,
        projection: evo_core::contract::RunProjection,
        task_input_utf8: String,
    }
    impl MirrorFormalTaskRequest {
        fn validate(&self) -> Result<()> {
            identifier(&self.candidate_id)?;
            identifier(&self.profile_registration_id)?;
            digest(&self.compiler_binding_digest)?;
            digest(&self.model_digest)?;
            if self.task_input_utf8.is_empty()
                || self.task_input_utf8.len() > 128 * 1024
                || hash(self.task_input_utf8.as_bytes()) != self.task.input_digest
                || self.projection.bundle_digest != self.task.bundle_digest
            {
                return Err(Error::Conflict("formal input/application mismatch".into()));
            }
            Ok(())
        }
    }
    fn report_ref_from_value_r49(value: &Value) -> Result<evo_core::formal_version::ReportRef> {
        let carrier: FrozenCarrierR36 =
            serde_json::from_value(value.clone()).map_err(crate::internal)?;
        // Data only; this helper does NOT grant Current, Host or write authority.
        known_complete_reference_r36(&carrier, &carrier.input.ticket_snapshot.evaluator_actor)
    }
    fn check_consumer_write_r49(
        reference: &Option<evo_core::formal_version::ReportRef>,
        ctx: &Context,
        kind: &str,
        id: &str,
        body: &Value,
    ) -> Result<()> {
        identifier(id)?;
        if body["id"] != id {
            return Err(Error::Forbidden);
        }
        match (kind, body["schema_version"].as_str()) {
            ("artifact", Some("rsia.independent_grade_capture.v2")) => {
                let record: IndependentGradeCapture = decode(body.clone())?;
                if ctx.role() != Role::Evaluator
                    || record.ticket_id.is_empty()
                    || record.id
                        != format!(
                            "formal-grade-v2-{}",
                            &fingerprint(&(record.ticket_id.as_str(), record.target_id.as_str()))?
                                [..32]
                        )
                {
                    return Err(Error::Forbidden);
                }
            }
            ("artifact", Some(PRINCIPAL_ACTION_SCHEMA)) => {
                let record: PrincipalAction = decode(body.clone())?;
                if record.namespace != ctx.namespace()
                    || record.id
                        != principal_action_id(
                            &record.subject_kind,
                            &record.subject_id,
                            record.role,
                        )?
                    || ctx.role() != Role::Evaluator
                {
                    return Err(Error::Forbidden);
                }
            }
            ("artifact", Some(evo_core::formal_version::FORMAL_AUTHORITY_V3)) => {
                let value: evo_core::formal_version::FormalAuthorityV3<IssuedFormalAuthority> =
                    serde_json::from_value(body.clone()).map_err(crate::internal)?;
                value.validate_known().map_err(r24_fault)?;
                if reference.as_ref() != Some(&value.report_ref) || ctx.role() != Role::Evaluator {
                    return Err(Error::Forbidden);
                }
            }
            ("release", Some(evo_core::formal_version::PERSISTENT_RELEASE_V2)) => {
                let value: evo_core::formal_version::PersistentReleaseV2<MirrorPersistentRelease> =
                    serde_json::from_value(body.clone()).map_err(crate::internal)?;
                value.validate_known().map_err(r24_fault)?;
                if reference.as_ref() != Some(&value.report_ref) || ctx.role() != Role::Admin {
                    return Err(Error::Forbidden);
                }
            }
            ("artifact", Some("rsia.release_action.v2")) => {
                let record: ReleaseActionR49 = decode(body.clone())?;
                validate_release_action_r49(&record)?;
                if ctx.role() != Role::Evaluator || record.action.namespace != ctx.namespace() {
                    return Err(Error::Forbidden);
                }
            }
            ("pointer", Some("rsia.profile_pointer.v1")) => {
                let pointer: MirrorPersistentProfilePointer = decode(body.clone())?;
                if ctx.role() != Role::Admin
                    || pointer.id != id
                    || pointer.id != format!("profile-pointer-{}", pointer.profile_id)
                    || pointer.active_release_id.is_some() != pointer.active_bundle_digest.is_some()
                {
                    return Err(Error::Forbidden);
                }
                if let Some(bundle) = &pointer.active_bundle_digest {
                    digest(bundle)?;
                }
            }
            ("formal_parent", Some("rsia.formal_parent_selection.v1")) => {
                let selection: ParentSelection = decode(body.clone())?;
                if ctx.role() != Role::Admin || selection.id != id {
                    return Err(Error::Forbidden);
                }
            }
            _ => return Err(Error::Forbidden),
        }
        Ok(())
    }

    fn s_has_native_action_r49<T: Serialize>(
        source: &mut SourceR49<'_>,
        ctx: &Context,
        host: &RegisteredFormalHost,
        set: &RegisteredPrincipalSet,
        kind: &str,
        id: &str,
        body: &T,
    ) -> Result<bool> {
        let action_id = principal_action_id(kind, id, Role::Evaluator)?;
        if source.get::<Value>(ctx, "artifact", &action_id)?.is_none() {
            return Ok(false);
        }
        require_principal_action(source, ctx, host, kind, id, body, Role::Evaluator)?;
        let proof = source.origin_for(ctx, "artifact", &action_id)?;
        if proof.grader_actor != set.grader.actor
            || proof.grader_uid != host.grader_uid
            || proof.grader_pid == 0
            || proof.grader_code_digest != host.grader_code_digest
            || proof.grader_workspace != host.grader_workspace
        {
            return Err(Error::Forbidden);
        }
        Ok(true)
    }

    fn need_record<T: serde::de::DeserializeOwned>(
        source: &mut SourceR49<'_>,
        ctx: &Context,
        kind: &str,
        id: &str,
    ) -> Result<T> {
        let physical = evaluation_storage_id(kind, id)?;
        let value: Value = source.need(ctx, "artifact", &physical)?;
        let payload = if kind == TICKET_KIND
            && value["schema_version"] == evo_core::formal_version::NATIVE_TICKET_ENVELOPE
        {
            evo_core::formal_version::native_ticket_payload(&value, ctx.namespace(), &physical)
                .map_err(r24_fault)?
        } else {
            if value["schema_version"] != "rsia.typed_artifact_envelope.v1"
                || value["id"] != physical
                || value["record_kind"] != kind
            {
                return Err(Error::Conflict(
                    "stored typed envelope identity differs".into(),
                ));
            }
            value.get("payload").ok_or(Error::Forbidden)?
        };
        serde_json::from_value(payload.clone())
            .map_err(|_| Error::Conflict("stored typed envelope payload differs".into()))
    }

    async fn capture_independent_grade(
        s: &mut SourceR49<'_>,
        ctx: &Context,
        ticket: &MirrorEvaluationTicketV2,
        target_id: &str,
        receipt_id: &str,
        receipt_digest: &str,
        new_grade: Option<&E05GradeDraft>,
    ) -> Result<()> {
        ctx.require(&[Role::Evaluator])?;
        let candidate: MirrorReleaseCandidateRecord =
            s.need(ctx, "artifact", &ticket.candidate_digest)?;
        let inputs = resolve_bundle_in_session(
            s,
            ctx,
            &candidate.bundle,
            &candidate.environment_digest,
            candidate.revoke_watermark,
            Purpose::Development,
        )?;
        let host: RegisteredFormalHost = attested_read(
            s,
            ctx,
            "artifact",
            &inputs.profile.host_registration_id,
            Role::Admin,
        )?;
        host.validate(ctx)?;
        let control: MirrorRegisteredEvaluationControl = need_record(
            s,
            ctx,
            "registered_evaluation_control_v41",
            &ticket.registration_id,
        )?;
        let set = verify_candidate_principals(s, ctx, &host.id, &candidate, &control)?;
        process_principal(ctx, &host, &set, Role::Evaluator)?;
        let credential =
            std::env::var(&host.grader_credential_environment_key).map_err(|_| Error::Forbidden)?;
        if hash(credential.as_bytes()) != host.grader_principal
            || host.grader_principal == host.host_principal
        {
            return Err(Error::Forbidden);
        }
        let observed = observe_registered_process_local_r49(
            host.grader_uid,
            &host.grader_code_digest,
            &host.grader_workspace,
        )?;
        let capture_id = format!(
            "formal-grade-v2-{}",
            &fingerprint(&(ticket.id.as_str(), target_id))?[..32]
        );
        if let Some(existing) = s.get::<IndependentGradeCapture>(ctx, "artifact", &capture_id)? {
            if new_grade.is_some() {
                return Err(Error::Conflict(
                    "native grade capture precedes its receipt".into(),
                ));
            }
            let original: Value = s.need(ctx, "artifact", &capture_id)?;
            require_content_witness(s, ctx, "artifact", &capture_id, &original, Role::Evaluator)?;
            let proof = s.origin_for(ctx, "artifact", &capture_id)?;
            if existing.schema_version != "rsia.independent_grade_capture.v2"
                || existing.ticket_id != ticket.id
                || existing.target_id != target_id
                || existing.grader_receipt_digest != receipt_digest
                || existing.host_registration_id != host.id
                || existing.grader_principal != host.grader_principal
                || existing.grader_uid != host.grader_uid
                || existing.grader_pid == 0
                || existing.grader_code_digest != host.grader_code_digest
                || existing.grader_workspace != host.grader_workspace
                || proof.grader_pid != existing.grader_pid
                || proof.grader_uid != existing.grader_uid
            {
                return Err(Error::Conflict(
                    "native grader observation identity reused".into(),
                ));
            }
            return Ok(());
        }
        // Authentication, actual process observation, and replay checks precede any SQL write.
        let draft = new_grade.ok_or_else(|| r24_fault(fv::Fault::SourceInsufficient))?;
        s.put_grade_receipt(ctx, draft).await?;
        let record = IndependentGradeCapture {
            id: format!(
                "formal-grade-v2-{}",
                &fingerprint(&(ticket.id.as_str(), target_id))?[..32]
            ),
            schema_version: "rsia.independent_grade_capture.v2".into(),
            ticket_id: ticket.id.clone(),
            target_id: target_id.into(),
            grader_receipt_digest: receipt_digest.into(),
            host_registration_id: host.id.clone(),
            grader_principal: host.grader_principal,
            grader_pid: observed.pid(),
            grader_uid: observed.uid(),
            grader_code_digest: observed.executable_digest().to_owned(),
            grader_workspace: observed.workspace().to_owned(),
        };
        immutable(s, ctx, "artifact", &record.id, &record).await?;
        attest_content(s, ctx, "artifact", &record.id, &record).await?;
        s.put_edge(ctx, "artifact", &record.id, "artifact", &host.id)
            .await?;
        s.put_edge(
            ctx,
            "artifact",
            &record.id,
            "artifact",
            &evaluation_storage_id("independent_grader_receipt_v2", receipt_id)?,
        )
        .await?;
        s.origin(ctx, "artifact", &record.id, &observed, None)
            .await?;
        Ok(())
    }

    // Created only after actual same-transaction SQL ACKs and complete source qualification.
    // No caller DTO, Clone, Deserialize, or public constructor can manufacture this proof.
    pub(super) struct GradeCaptureAckR49 {
        owner: Arc<crate::NativeOwner>,
        binding: String,
        receipt_physical: String,
        receipt_body: String,
        capture_id: String,
        capture_body: String,
        origin: String,
        roots: Vec<(String, String)>,
        closure: TypedContentClosure,
    }
    fn grade_binding_r49(ctx: &Context, draft: &E05GradeDraft) -> Result<String> {
        content_digest(&(
            ctx.namespace(),
            ctx.actor(),
            &draft.before,
            &draft.control_digest,
            &draft.holdout_digest,
            &draft.input_fingerprints,
            &draft.receipt,
        ))
    }
    impl GradeCaptureAckR49 {
        pub(super) fn confirm_owner(
            &self,
            session: &Session,
            asof: &NativeAsOfPermit,
        ) -> Result<()> {
            asof.same_session(session, true)?;
            require(
                Arc::ptr_eq(&self.owner, &asof.connection)
                    && matches!(asof.operation, NativeAsOfOperation::Grade)
                    && asof.prepared_digest.is_none(),
            )
        }
        pub(super) fn verify(
            &self,
            raw: &RawDatabase,
            ctx: &Context,
            draft: &E05GradeDraft,
        ) -> Result<()> {
            draft.check_grade_inputs(&snapshot(raw)?.0, ctx)?;
            require(self.binding == grade_binding_r49(ctx, draft)?)?;
            let mut s = SourceR49::captured(raw, ctx, None)?;
            let receipt: Value = s.need(ctx, "artifact", &self.receipt_physical)?;
            let expected = E05FingerprintEnvelope {
                schema_version: "rsia.typed_artifact_envelope.v1",
                id: &self.receipt_physical,
                record_kind: GRADER_RECEIPT_KIND,
                payload: draft.receipt.as_ref().ok_or(Error::Internal)?,
            };
            require(
                content_digest(&receipt)? == self.receipt_body
                    && self.receipt_body == content_digest(&expected)?,
            )?;
            let capture: Value = s.need(ctx, "artifact", &self.capture_id)?;
            let origin = s.origin_for_bound_r49(ctx, "artifact", &self.capture_id, None)?;
            require(
                content_digest(&capture)? == self.capture_body
                    && content_digest(&origin)? == self.origin,
            )?;
            let closure = read_typed_closure(&mut s, ctx, &self.roots)?;
            require(closure.digest == self.closure.digest)
        }
    }
    pub(super) async fn prepare_grade_inside_owner_r49(
        session: &mut Session,
        ctx: &Context,
        draft: &E05GradeDraft,
        asof: &NativeAsOfPermit,
        gate: Arc<AtomicU8>,
    ) -> Result<GradeCaptureAckR49> {
        asof.same_session(session, true)?;
        require(
            matches!(asof.operation, NativeAsOfOperation::Grade) && asof.prepared_digest.is_none(),
        )?;
        let mut source = SourceR49::owned(session, ctx, None, Arc::clone(&gate)).await?;
        require(native_database_digest(&source.raw, true)? == asof.capture_digest)?;
        draft.check_grade_inputs(&source.actual, ctx)?;
        let receipt = draft.receipt.as_ref().ok_or(Error::Internal)?;
        let receipt_physical = evaluation_storage_id(GRADER_RECEIPT_KIND, &receipt.receipt_id)?;
        let capture_id = format!(
            "formal-grade-v2-{}",
            &fingerprint(&(draft.before.id.as_str(), receipt.target_id.as_str()))?[..32]
        );
        capture_independent_grade(
            &mut source,
            ctx,
            &draft.before,
            &receipt.target_id,
            &receipt.receipt_id,
            &content_digest(receipt)?,
            if draft.replay { None } else { Some(draft) },
        )
        .await?;
        // Neither edge set may be weakened by an otherwise valid closure.
        let mut expected_receipt_edges = if draft.before.candidate_dependency_binding.is_some() {
            vec![
                e05_artifact_edge(
                    EXECUTION_RECEIPT_KIND,
                    &receipt.candidate_execution_receipt_id,
                )?,
                e05_artifact_edge(
                    EXECUTION_RECEIPT_KIND,
                    &receipt.baseline_execution_receipt_id,
                )?,
            ]
        } else {
            Vec::new()
        };
        expected_receipt_edges.sort();
        require(
            source.dependency_edges(ctx, "artifact", &receipt_physical)? == expected_receipt_edges,
        )?;
        let capture: IndependentGradeCapture = source.need(ctx, "artifact", &capture_id)?;
        let mut expected_capture_edges = vec![
            ("artifact".into(), capture.host_registration_id.clone()),
            ("artifact".into(), receipt_physical.clone()),
        ];
        expected_capture_edges.sort();
        require(source.dependency_edges(ctx, "artifact", &capture_id)? == expected_capture_edges)?;
        source.stable().await?;
        let mut roots = vec![
            (
                "artifact".into(),
                evaluation_storage_id(TICKET_KIND, &draft.before.id)?,
            ),
            ("artifact".into(), receipt_physical.clone()),
            ("artifact".into(), capture_id.clone()),
        ];
        roots.extend(
            draft
                .input_fingerprints
                .iter()
                .map(|(id, _)| ("artifact".to_owned(), id.clone())),
        );
        let closure = read_typed_closure(&mut source, ctx, &roots)?;
        let receipt_body: Value = source.need(ctx, "artifact", &receipt_physical)?;
        let capture_body: Value = source.need(ctx, "artifact", &capture_id)?;
        let origin = source.origin_for_bound_r49(ctx, "artifact", &capture_id, None)?;
        let ack = GradeCaptureAckR49 {
            owner: Arc::clone(&asof.connection),
            binding: grade_binding_r49(ctx, draft)?,
            receipt_physical,
            receipt_body: content_digest(&receipt_body)?,
            capture_id,
            capture_body: content_digest(&capture_body)?,
            origin: content_digest(&origin)?,
            roots,
            closure,
        };
        ack.verify(&source.raw, ctx, draft)?;
        check_cancel(&gate)?;
        Ok(ack)
    }

    fn derive_complete_authority_r49(
        s: &mut SourceR49,
        ctx: &Context,
        ticket: &MirrorEvaluationTicketV2,
        control: &MirrorRegisteredEvaluationControl,
        formal: &MirrorFormalEvaluationV2,
    ) -> Result<IssuedFormalAuthority> {
        ctx.require(&[Role::Evaluator])?;
        if control.evidence_scope != MirrorEvaluationEvidenceScope::RegisteredExternalHost {
            return Err(Error::Forbidden);
        }
        let candidate: MirrorReleaseCandidateRecord =
            s.need(ctx, "artifact", &ticket.candidate_digest)?;
        let inputs = resolve_bundle_in_session(
            s,
            ctx,
            &candidate.bundle,
            &candidate.environment_digest,
            candidate.revoke_watermark,
            Purpose::Development,
        )?;
        let host: RegisteredFormalHost = attested_read(
            s,
            ctx,
            "artifact",
            &inputs.profile.host_registration_id,
            Role::Admin,
        )?;
        host.validate(ctx)?;
        let set = verify_candidate_principals(s, ctx, &host.id, &candidate, control)?;
        let observation = observed_native_report_action_r49(
            s,
            ctx,
            &host,
            &set,
            &s.reference.clone().ok_or(Error::Forbidden)?,
        )?;
        let reference = s.reference.clone().ok_or(Error::Forbidden)?;
        let report_storage_id = reference.physical_id.clone();
        let _: Value = s.need(ctx, "artifact", &report_storage_id)?;
        let report_principal_action_id =
            principal_action_id("artifact", &report_storage_id, Role::Evaluator)?;
        let observed = observation;
        let seal = s
            .sealed_root_budget_evidence(ctx, &control.billing_scope)?
            .ok_or(Error::NotFound)?;
        if !seal.monetary_complete || !seal.unresolved.is_empty() {
            return Err(Error::Conflict("complete root cost unresolved".into()));
        }
        let cost: EpisodeCostRecord = attested_read(
            s,
            ctx,
            "artifact",
            &format!("formal-cost-{}", ticket.id),
            Role::Host,
        )?;
        cost.measurements.validate_complete()?;
        verify_refs(s, ctx, &cost.measurement_sources)?;
        if cost.ticket_id != ticket.id
            || cost.profile_registration_id != inputs.profile.id
            || cost.observed_uid != host.host_uid
        {
            return Err(Error::Conflict("episode meter scope mismatch".into()));
        }
        let mut provider_captures = Vec::new();
        let mut grader_captures = Vec::new();
        if cost.measurements.episode_id != control.attempt_id
            || seal.root.billing_scope != control.billing_scope
        {
            return Err(Error::Conflict("episode/root identity differs".into()));
        }
        validate_episode_meter_sources(
            s,
            ctx,
            &inputs.profile,
            &host,
            ticket,
            control,
            &seal,
            &cost.measurements,
            &cost.measurement_sources,
        )?;
        let mut roots = vec![
            ("artifact".into(), candidate.id.clone()),
            ("artifact".into(), cost.id.clone()),
            ("artifact".into(), report_storage_id.clone()),
            (
                "artifact".into(),
                evaluation_storage_id("evaluation_ticket_v2", &ticket.id)?,
            ),
            (
                "artifact".into(),
                evaluation_storage_id(
                    "registered_evaluation_control_v41",
                    &ticket.registration_id,
                )?,
            ),
            (
                "artifact".into(),
                evaluation_storage_id("protected_holdout_v41", &ticket.holdout_id)?,
            ),
            ("artifact".into(), set.id.clone()),
            ("artifact".into(), report_principal_action_id.clone()),
        ];
        for target in &ticket.targets {
            let MirrorTargetState::Completed {
                candidate_receipt_id,
                baseline_receipt_id,
                grader_receipt_id,
                ..
            } = &target.state
            else {
                return Err(Error::Conflict("formal target incomplete".into()));
            };
            for receipt_id in [candidate_receipt_id, baseline_receipt_id] {
                let receipt: MirrorExecutionReceiptV2 =
                    need_record(s, ctx, "execution_receipt_v2", receipt_id)?;
                let call = s
                    .budget_call(ctx, &control.billing_scope, &receipt.budget_call_id)?
                    .ok_or(Error::NotFound)?;
                if call.execution_provenance
                    != Some(crate::budget::BudgetExecutionProvenance::ExternalProvider)
                    || !call.execution_closed
                    || call.state != crate::budget::BudgetCallState::Finalized
                    || call.dispatch_group_id != ticket.id
                    || call.response_usable != Some(true)
                {
                    return Err(Error::Forbidden);
                }
                let id = format!(
                    "formal-capture-{}",
                    &hash(call.dispatch_id.as_ref().ok_or(Error::NotFound)?.as_bytes())[..32]
                );
                let capture: FormalProviderCaptureRecord =
                    attested_read(s, ctx, "artifact", &id, Role::Host)?;
                verify_provider_registration(s, ctx, &capture, &host)?;
                let request_artifact = call.request_artifact.as_ref().ok_or(Error::NotFound)?;
                if request_artifact.schema_version != "rsia.formal_task_request.v1" {
                    return Err(Error::Forbidden);
                }
                let actual_request: MirrorFormalTaskRequest =
                    serde_json::from_str(&request_artifact.body).map_err(|_| Error::Forbidden)?;
                actual_request.validate()?;
                let applied_bundle = if receipt.side == MirrorExecutionSide::Candidate {
                    candidate.bundle.clone()
                } else {
                    inputs.compile(
                        &SkillPatch {
                            content: evo_core::contract::PatchOp::ResetToBaseline,
                            applicability: evo_core::contract::PatchOp::ResetToBaseline,
                            counterexample: evo_core::contract::PatchOp::ResetToBaseline,
                            required_capabilities: evo_core::contract::PatchOp::ResetToBaseline,
                            dependencies: evo_core::contract::PatchOp::ResetToBaseline,
                        },
                        &ImproverPatch {
                            instruction: evo_core::contract::PatchOp::ResetToBaseline,
                            max_candidates: evo_core::contract::PatchOp::ResetToBaseline,
                            max_rounds: evo_core::contract::PatchOp::ResetToBaseline,
                        },
                    )?
                };
                let projection = evo_core::contract::project_run(
                    &applied_bundle,
                    &inputs.profile.system_snapshot,
                    true,
                )?;
                let mut application = vec![
                    serde_json::json!({"role":"system","content":actual_request.mandatory_context}),
                ];
                for instruction in &projection.instructions {
                    application.push(serde_json::json!({"role":"system","content":instruction}));
                }
                application.push(
                    serde_json::json!({"role":"user","content":actual_request.task_input_utf8}),
                );
                if actual_request.candidate_id != candidate.id
                    || actual_request.profile_registration_id != inputs.profile.id
                    || actual_request.compiler_binding_digest != inputs.binding.binding_digest
                    || actual_request.task.ticket_id != ticket.id
                    || actual_request.task.target_id != target.target_id
                    || actual_request.task.side != receipt.side
                    || actual_request.task.request_digest != receipt.request_digest
                    || actual_request.task.input_digest != target.input_digest
                    || applied_bundle.digest != receipt.bundle_digest
                    || hash(actual_request.mandatory_context.as_bytes())
                        != inputs.profile.system_snapshot.mandatory_context_digest
                    || fingerprint(&actual_request.projection)? != fingerprint(&projection)?
                    || fingerprint(&application)? != capture.application_digest
                {
                    return Err(Error::Conflict(
                        "actual request application differs from frozen P/B".into(),
                    ));
                }
                let transport = call.transport_artifact.as_ref().ok_or(Error::NotFound)?;
                if decode::<FormalProviderCaptureRecord>(
                    serde_json::from_str(&transport.body).map_err(|_| Error::Internal)?,
                )?
                .id != capture.id
                    || fingerprint(&capture)?
                        != fingerprint(&decode::<FormalProviderCaptureRecord>(
                            serde_json::from_str(&transport.body).map_err(|_| Error::Internal)?,
                        )?)?
                    || capture.host_registration_id != host.id
                    || capture.host_uid != host.host_uid
                    || capture.host_pid == observed.pid
                    || capture.host_principal != host.host_principal
                    || capture.host_code_digest != host.code_digest
                    || capture.host_workspace != host.host_workspace
                    || capture.request_digest != receipt.request_digest
                    || capture.bundle_digest != receipt.bundle_digest
                    || capture.output_digest != receipt.output_digest
                    || capture.input_digest != target.input_digest
                    || call.actual_cost_micros != Some(capture.actual_cost_micros)
                    || call.actual_currency.as_deref() != Some(capture.currency.as_str())
                    || call.actual_pricing_version.as_deref()
                        != Some(capture.pricing_version.as_str())
                    || seal.root.currency != capture.currency
                    || seal.root.pricing_version != capture.pricing_version
                    || call.output_digest.as_deref() != Some(capture.output_digest.as_str())
                    || call.provider_request_id.as_deref()
                        != Some(capture.provider_request_id.as_str())
                    || call.usage_record_id.as_deref() != Some(capture.usage_record_id.as_str())
                {
                    return Err(Error::Conflict(
                        "actual provider/host/output/usage mismatch".into(),
                    ));
                }
                let r = TypedContentRef {
                    kind: "artifact".into(),
                    id: id.clone(),
                    schema_version: capture.schema_version.clone(),
                    content_digest: content_digest(&capture)?,
                };
                provider_captures.push(r);
                roots.push(("artifact".into(), id));
                for kind_id in [
                    ("execution_receipt_v2", receipt_id.as_str()),
                    (
                        "evaluation_execution_output_v1",
                        receipt.output_artifact_id.as_str(),
                    ),
                ] {
                    roots.push((
                        "artifact".into(),
                        evaluation_storage_id(kind_id.0, kind_id.1)?,
                    ));
                }
            }
            let grade_id = format!(
                "formal-grade-v2-{}",
                &fingerprint(&(ticket.id.as_str(), target.target_id.as_str()))?[..32]
            );
            let grade: IndependentGradeCapture =
                attested_read(s, ctx, "artifact", &grade_id, Role::Evaluator)?;
            // The capture names the canonical digest of the stored receipt itself:
            // any changed field, or a capture of another digest, fails closed.
            let original: Value =
                need_record(s, ctx, "independent_grader_receipt_v2", grader_receipt_id)?;
            if grade.grader_receipt_digest != evo_core::canonical_fingerprint(&original)?
                || grade.grader_uid != host.grader_uid
                || grade.grader_principal != host.grader_principal
                || grade.grader_code_digest != host.grader_code_digest
                || grade.grader_workspace != host.grader_workspace
                || grade.ticket_id != ticket.id
                || grade.target_id != target.target_id
            {
                return Err(Error::Forbidden);
            }
            let native_grade = s.origin_for(ctx, "artifact", &grade_id)?;
            if native_grade.grader_pid != grade.grader_pid
                || native_grade.grader_uid != grade.grader_uid
                || native_grade.grader_code_digest != grade.grader_code_digest
                || native_grade.grader_workspace != grade.grader_workspace
            {
                return Err(Error::Conflict("native grade origin differs".into()));
            }
            grader_captures.push(TypedContentRef {
                kind: "artifact".into(),
                id: grade_id.clone(),
                schema_version: grade.schema_version.clone(),
                content_digest: content_digest(&grade)?,
            });
            roots.push(("artifact".into(), grade_id));
            roots.push((
                "artifact".into(),
                evaluation_storage_id("independent_grader_receipt_v2", grader_receipt_id)?,
            ));
        }
        let closure = read_typed_closure(s, ctx, &roots)?;
        let record = IssuedFormalAuthority {
            id: evo_core::formal_version::formal_authority_id_v3(&reference, &fingerprint(&seal)?)
                .map_err(r24_fault)?,
            schema_version: FORMAL_SCHEMA.into(),
            report_id: ticket.id.clone(),
            raw_report_digest: fingerprint(formal)?,
            ticket_digest: fingerprint(ticket)?,
            candidate_id: candidate.id,
            candidate_bundle_digest: candidate.bundle_digest,
            environment_digest: candidate.environment_digest,
            compiler_binding: inputs.binding,
            billing_scope: control.billing_scope.clone(),
            root_seal_digest: fingerprint(&seal)?,
            episode_cost_digest: fingerprint(&cost)?,
            host_registration_id: host.id,
            principal_set_digest: set.binding_digest()?,
            report_principal_action_id,
            provider_captures,
            grader_captures,
            closure_roots: roots,
            closure,
            watermark: s.watermark(ctx)?,
            issued_by: ctx.actor().into(),
        };
        Ok(record)
    }

    async fn issue_complete_authority_in_session(
        s: &mut SourceR49<'_>,
        ctx: &Context,
        ticket: &MirrorEvaluationTicketV2,
        control: &MirrorRegisteredEvaluationControl,
        formal: &MirrorFormalEvaluationV2,
    ) -> Result<evo_core::formal_version::FormalAuthorityV3<IssuedFormalAuthority>> {
        ctx.require(&[Role::Evaluator])?;
        let candidate: MirrorReleaseCandidateRecord =
            s.need(ctx, "artifact", &ticket.candidate_digest)?;
        let inputs = resolve_bundle_in_session(
            s,
            ctx,
            &candidate.bundle,
            &candidate.environment_digest,
            candidate.revoke_watermark,
            Purpose::Development,
        )?;
        let host: RegisteredFormalHost = attested_read(
            s,
            ctx,
            "artifact",
            &inputs.profile.host_registration_id,
            Role::Admin,
        )?;
        let set = verify_candidate_principals(s, ctx, &host.id, &candidate, control)?;
        let reference = s.reference.clone().ok_or(Error::Forbidden)?;
        let body: Value = s.need(ctx, "artifact", &reference.physical_id)?;
        record_grader_action(
            s,
            ctx,
            &host,
            "artifact",
            &reference.physical_id,
            &body,
            "issue_report",
        )
        .await?;
        let authority = derive_complete_authority_r49(s, ctx, ticket, control, formal)?;
        let envelope = evo_core::formal_version::FormalAuthorityV3 {
            schema_version: evo_core::formal_version::FORMAL_AUTHORITY_V3.into(),
            id: authority.id.clone(),
            report_ref: reference.clone(),
            ticket_id: ticket.id.clone(),
            input_digest: authority.root_seal_digest.clone(),
            authority_digest: fingerprint(&authority)?,
            authority,
        };
        envelope.validate_known().map_err(r24_fault)?;
        immutable(s, ctx, "artifact", &envelope.id, &envelope).await?;
        attest_content(s, ctx, "artifact", &envelope.id, &envelope).await?;
        for (kind, id) in &envelope.authority.closure_roots {
            s.put_edge(ctx, "artifact", &envelope.id, kind, id).await?;
        }
        // Both authority and action are real ACKed rows. Only the closed native
        // writer can append their reserved causal proof.
        let observed = process_principal(ctx, &host, &set, Role::Evaluator)?;
        s.origin(ctx, "artifact", &envelope.id, &observed, None)
            .await?;
        Ok(envelope)
    }

    fn known_object_r49<'s>(
        actual: &'s OccurrenceSnapshot,
        ctx: &Context,
        kind: &str,
        id: &str,
        schema: Option<&str>,
        role: Option<&str>,
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<Option<&'s OccurrenceObject>> {
        let Some(object) = actual
            .objects
            .get(&(ctx.namespace().into(), kind.into(), id.into()))
        else {
            gaps.insert("host_original_object_missing");
            return Ok(None);
        };
        e05_dependency_live(actual, object)?;
        if object.body["schema_version"] == "rsia.redacted.v1" {
            qualify_redacted(object)?;
            gaps.insert("host_original_object_redacted");
            return Ok(None);
        }
        if object
            .body
            .get("id")
            .is_some_and(|value| value.as_str() != Some(id))
            || object
                .body
                .get("namespace")
                .is_some_and(|value| value.as_str() != Some(ctx.namespace()))
            || schema.is_some_and(|schema| object.body["schema_version"] != schema)
        {
            return Err(Error::Conflict(
                "formal Host known object identity/schema differs".into(),
            ));
        }
        if let Some(role) = role {
            let prefix = match role {
                "admin" => "formal.admin.content.v1",
                "host" => "formal.host.content.v1",
                "evaluator" => "formal.evaluator.content.v1",
                _ => return Err(Error::Forbidden),
            };
            let operation = format!("{prefix}.{}", fingerprint(&(ctx.namespace(), kind, id))?);
            let expected =
                fingerprint(&(ctx.namespace(), kind, id, content_digest(&object.body)?))?;
            match actual
                .witnesses
                .get(&(ctx.namespace().into(), role.into(), operation))
            {
                Some((entity, _)) if entity != &expected => {
                    return Err(Error::Conflict(
                        "formal Host known content witness differs".into(),
                    ));
                }
                None => {
                    gaps.insert("host_content_witness_missing");
                }
                _ => {}
            }
        }
        Ok(Some(object))
    }

    fn revalidate_release_r49(
        source: &mut SourceR49<'_>,
        ctx: &Context,
        wrapper: &fv::PersistentReleaseV2<MirrorPersistentRelease>,
    ) -> Result<(RegisteredFormalHost, RegisteredPrincipalSet)> {
        wrapper.validate_known().map_err(r24_fault)?;
        source.reference = Some(wrapper.report_ref.clone());
        source.origin_for(ctx, "release", &wrapper.id)?;
        let current = current_data_r49(source)?;
        let authority = verify_issued_authority_r49(source, ctx, &current)?;
        let ticket = &current.carrier.input.ticket_snapshot;
        let candidate: MirrorReleaseCandidateRecord =
            source.need(ctx, "artifact", &wrapper.release.candidate_id)?;
        let control = evaluation_control(&source.actual, ctx.namespace(), &ticket.registration_id)?;
        let host: RegisteredFormalHost = attested_read(
            source,
            ctx,
            "artifact",
            &authority.authority.host_registration_id,
            Role::Admin,
        )?;
        let set = verify_candidate_principals(source, ctx, &host.id, &candidate, &control)?;
        require_principal_action(
            source,
            ctx,
            &host,
            "artifact",
            &candidate.id,
            &candidate,
            Role::Worker,
        )?;
        require_principal_action(
            source,
            ctx,
            &host,
            "release",
            &wrapper.id,
            &wrapper.release,
            Role::Admin,
        )?;
        let release = &wrapper.release;
        if candidate.id != ticket.candidate_digest
            || candidate.bundle_digest != ticket.candidate_bundle_digest
            || candidate.parent_digest != ticket.baseline_digest
            || candidate.environment_digest != ticket.environment_digest
            || candidate.revoke_watermark
                != source
                    .actual
                    .watermarks
                    .get(ctx.namespace())
                    .ok_or(Error::Forbidden)?
                    .0
            || candidate.bundle_digest != release.bundle_digest
            || candidate.environment_digest != release.environment_digest
            || candidate.parent_digest != release.parent_digest
            || candidate.proposer_actor != release.proposer_actor
            || release.profile_id != candidate.profile_id
            || release.report_id != ticket.id
            || release.evaluator_actor != ticket.evaluator_actor
            || release.approved_by != ticket.approver_actor
            || release.approved_by == release.proposer_actor
            || release.approved_by == release.evaluator_actor
            || release.report_digest
                != fingerprint(&(&current.carrier.payload, &authority.authority))?
        {
            return Err(Error::Conflict(
                "release evidence changed after approval".into(),
            ));
        }
        let MirrorFormalEvaluationV2::CompleteBatch { verdict, .. } = &current.carrier.payload
        else {
            return Err(Error::Forbidden);
        };
        if !matches!(
            verdict,
            evo_core::evaluation::Verdict::Improved | evo_core::evaluation::Verdict::Noninferior
        ) {
            return Err(Error::Forbidden);
        }
        Ok((host, set))
    }
    async fn record_release_state_r49(
        source: &mut SourceR49<'_>,
        ctx: &Context,
        host: &RegisteredFormalHost,
        set: &RegisteredPrincipalSet,
        credential: &NativeReleaseCredentialR49,
        kind: &str,
        id: &str,
        before: &Value,
        after: &Value,
    ) -> Result<()> {
        let administrator = set.principal(Role::Admin)?;
        if ctx.actor() != administrator.actor
            || credential.peer.uid() != administrator.uid
            || hash(&credential.credential) != administrator.credential_digest
            || credential.request_digest != fingerprint(&credential.command)?
        {
            return Err(Error::Forbidden);
        }
        let grader = Context::new(ctx.namespace(), &set.grader.actor, Role::Evaluator)?;
        let observed = process_principal(&grader, host, set, Role::Evaluator)?;
        let previous = source
            .latest_origin_r49(ctx, kind, id)?
            .and_then(|proof| proof.action_id);
        let operation = match &credential.command {
            ReleaseCommandR49::CanaryCurrentV2 { .. } => "canary_current_v2",
            ReleaseCommandR49::ActivateCurrentV2 { .. } => "activate_current_v2",
            ReleaseCommandR49::RollbackCurrentV2 { .. } => "rollback_current_v2",
        };
        let command = serde_json::to_value(&credential.command).map_err(crate::internal)?;
        let mut action = PrincipalAction {
            id: String::new(),
            schema_version: PRINCIPAL_ACTION_SCHEMA.into(),
            namespace: ctx.namespace().into(),
            host_registration_id: host.id.clone(),
            host_digest: fingerprint(host)?,
            principal_set_digest: set.binding_digest()?,
            role: Role::Admin,
            actor: administrator.actor.clone(),
            uid: administrator.uid,
            principal: administrator.principal.clone(),
            credential_digest: administrator.credential_digest.clone(),
            subject_kind: kind.into(),
            subject_id: id.into(),
            subject_content_digest: content_digest(after)?,
            operation: operation.into(),
            authenticated_request_digest: credential.request_digest.clone(),
            peer_uid: credential.peer.uid(),
            peer_gid: Some(credential.peer.gid()),
            peer_pid: credential
                .peer
                .pid()
                .map(u32::try_from)
                .transpose()
                .map_err(|_| Error::Forbidden)?,
            authority_pid: observed.pid(),
            authority_uid: observed.uid(),
            authority_executable_digest: observed.executable_digest().to_owned(),
        };
        action.id = release_action_id_r49(previous.as_deref(), &command, &action)?;
        let record = ReleaseActionR49 {
            schema_version: "rsia.release_action.v2".into(),
            id: action.id.clone(),
            previous_action_id: previous,
            command,
            action,
        };
        validate_release_action_r49(&record)?;
        if source.get::<Value>(ctx, kind, id)?.is_some() {
            source
                .replace_release_row_r49(ctx, kind, id, before, after)
                .await?;
        } else {
            source.put(ctx, kind, id, ctx.actor(), after).await?;
        }
        attest_content(source, ctx, kind, id, after).await?;
        immutable(source, &grader, "artifact", &record.id, &record).await?;
        attest_content(source, &grader, "artifact", &record.id, &record).await?;
        source
            .put_edge(&grader, "artifact", &record.id, "artifact", &set.id)
            .await?;
        source
            .put_edge(&grader, kind, id, "artifact", &record.id)
            .await?;
        source
            .origin(
                &grader,
                kind,
                id,
                &observed,
                Some((&credential.peer, &credential.request_digest)),
            )
            .await
    }
    fn load_pointer_r49(
        source: &mut SourceR49<'_>,
        ctx: &Context,
        profile: &str,
    ) -> Result<MirrorPersistentProfilePointer> {
        let id = format!("profile-pointer-{profile}");
        let Some(body) = source.get::<Value>(ctx, "pointer", &id)? else {
            return Ok(MirrorPersistentProfilePointer {
                id,
                schema_version: "rsia.profile_pointer.v1".into(),
                profile_id: profile.into(),
                active_release_id: None,
                active_bundle_digest: None,
                epoch: 0,
            });
        };
        let pointer: MirrorPersistentProfilePointer = decode(body.clone())?;
        if pointer.id != id
            || pointer.profile_id != profile
            || pointer.schema_version != "rsia.profile_pointer.v1"
            || pointer.active_release_id.is_some() != pointer.active_bundle_digest.is_some()
        {
            return Err(Error::Conflict("profile pointer identity mismatch".into()));
        }
        require_content_witness(source, ctx, "pointer", &id, &body, Role::Admin)?;
        require_registered_body_authority(source, ctx, "pointer", &id, &body)?;
        Ok(pointer)
    }
    async fn cas_pointer_r49(
        source: &mut SourceR49<'_>,
        ctx: &Context,
        host: &RegisteredFormalHost,
        set: &RegisteredPrincipalSet,
        credential: &NativeReleaseCredentialR49,
        profile: &str,
        epoch: u64,
        expected: Option<&str>,
        next: &MirrorPersistentRelease,
    ) -> Result<()> {
        let mut pointer = load_pointer_r49(source, ctx, profile)?;
        if pointer.epoch != epoch || pointer.active_release_id.as_deref() != expected {
            return Err(Error::Conflict("cas_lost".into()));
        }
        let before = serde_json::to_value(&pointer).map_err(crate::internal)?;
        pointer.epoch = pointer
            .epoch
            .checked_add(1)
            .ok_or_else(|| Error::Conflict("pointer_epoch_overflow".into()))?;
        pointer.active_release_id = Some(next.id.clone());
        pointer.active_bundle_digest = Some(next.bundle_digest.clone());
        let after = serde_json::to_value(&pointer).map_err(crate::internal)?;
        record_release_state_r49(
            source,
            ctx,
            host,
            set,
            credential,
            "pointer",
            &pointer.id,
            &before,
            &after,
        )
        .await
    }
    async fn write_release_r49(
        source: &mut SourceR49<'_>,
        ctx: &Context,
        credential: &NativeReleaseCredentialR49,
        host: &RegisteredFormalHost,
        set: &RegisteredPrincipalSet,
        before: &fv::PersistentReleaseV2<MirrorPersistentRelease>,
        state: MirrorPersistentReleaseState,
    ) -> Result<fv::PersistentReleaseV2<MirrorPersistentRelease>> {
        source.reference = Some(before.report_ref.clone());
        let before_body = serde_json::to_value(before).map_err(crate::internal)?;
        let mut after = before.clone();
        after.release.state = state;
        after.release_digest = fingerprint(&after.release)?;
        after.validate_known().map_err(r24_fault)?;
        let after_body = serde_json::to_value(&after).map_err(crate::internal)?;
        record_release_state_r49(
            source,
            ctx,
            host,
            set,
            credential,
            "release",
            &after.id,
            &before_body,
            &after_body,
        )
        .await?;
        Ok(after)
    }

    fn prior_release_source_r49(
        source: &mut SourceR49<'_>,
        ctx: &Context,
        wrapper: &fv::PersistentReleaseV2<MirrorPersistentRelease>,
    ) -> Result<(RegisteredFormalHost, RegisteredPrincipalSet)> {
        wrapper.validate_known().map_err(r24_fault)?;
        source.reference = Some(wrapper.report_ref.clone());
        let body: Value = source.need(ctx, "release", &wrapper.id)?;
        source.origin_for(ctx, "release", &wrapper.id)?;
        require_content_witness(source, ctx, "release", &wrapper.id, &body, Role::Admin)?;
        let host =
            registered_subject_host(source, ctx, "release", &body)?.ok_or(Error::Forbidden)?;
        let set = principal_set(source, ctx, &host)?;
        require_principal_action(
            source,
            ctx,
            &host,
            "release",
            &wrapper.id,
            &wrapper.release,
            Role::Admin,
        )?;
        Ok((host, set))
    }

    async fn transition_release_r49(
        source: &mut SourceR49<'_>,
        ctx: &Context,
        credential: &NativeReleaseCredentialR49,
    ) -> Result<Value> {
        ctx.require(&[Role::Admin])?;
        let target_id = match &credential.command {
            ReleaseCommandR49::CanaryCurrentV2 { release_id }
            | ReleaseCommandR49::ActivateCurrentV2 { release_id, .. } => release_id.as_str(),
            ReleaseCommandR49::RollbackCurrentV2 {
                target_release_id, ..
            } => target_release_id.as_str(),
        };
        let target: fv::PersistentReleaseV2<MirrorPersistentRelease> =
            source.need(ctx, "release", target_id)?;
        let (host, set) = revalidate_release_r49(source, ctx, &target)?;
        // No caller DTO grants authority: this actual peer and credential are
        // rechecked against the actual principal set before the first SQL write.
        let admin = set.principal(Role::Admin)?;
        if credential.peer.uid() != admin.uid
            || ctx.actor() != admin.actor
            || hash(&credential.credential) != admin.credential_digest
            || credential.request_digest != fingerprint(&credential.command)?
        {
            return Err(Error::Forbidden);
        }
        let result = match &credential.command {
            ReleaseCommandR49::CanaryCurrentV2 { .. } => {
                if target.release.state != MirrorPersistentReleaseState::Approved
                    || target.release.approved_by != ctx.actor()
                {
                    return Err(Error::Forbidden);
                }
                write_release_r49(
                    source,
                    ctx,
                    credential,
                    &host,
                    &set,
                    &target,
                    MirrorPersistentReleaseState::Canary,
                )
                .await?
            }
            ReleaseCommandR49::ActivateCurrentV2 {
                expected_pointer_epoch,
                ..
            } => {
                if target.release.state != MirrorPersistentReleaseState::Canary
                    || target.release.approved_by != ctx.actor()
                {
                    return Err(Error::Forbidden);
                }
                let previous = if let Some(id) = &target.release.expected_parent_release_id {
                    let previous: fv::PersistentReleaseV2<MirrorPersistentRelease> =
                        source.need(ctx, "release", id)?;
                    if previous.release.state != MirrorPersistentReleaseState::Active {
                        return Err(Error::Conflict(
                            "expected parent release is not active".into(),
                        ));
                    }
                    prior_release_source_r49(source, ctx, &previous)?;
                    Some(previous)
                } else {
                    None
                };
                source.reference = Some(target.report_ref.clone());
                cas_pointer_r49(
                    source,
                    ctx,
                    &host,
                    &set,
                    credential,
                    &target.release.profile_id,
                    *expected_pointer_epoch,
                    target.release.expected_parent_release_id.as_deref(),
                    &target.release,
                )
                .await?;
                if let Some(previous) = previous {
                    let (previous_host, previous_set) =
                        prior_release_source_r49(source, ctx, &previous)?;
                    write_release_r49(
                        source,
                        ctx,
                        credential,
                        &previous_host,
                        &previous_set,
                        &previous,
                        MirrorPersistentReleaseState::Superseded,
                    )
                    .await?;
                }
                write_release_r49(
                    source,
                    ctx,
                    credential,
                    &host,
                    &set,
                    &target,
                    MirrorPersistentReleaseState::Active,
                )
                .await?
            }
            ReleaseCommandR49::RollbackCurrentV2 {
                profile_id,
                expected_pointer_epoch,
                environment_digest,
                ..
            } => {
                let pointer = load_pointer_r49(source, ctx, profile_id)?;
                let current_id = pointer
                    .active_release_id
                    .ok_or_else(|| Error::Conflict("profile has no active release".into()))?;
                if current_id == target.id {
                    return Err(Error::Conflict("rollback target is already active".into()));
                }
                if target.release.profile_id != *profile_id
                    || target.release.environment_digest != *environment_digest
                    || target.release.state != MirrorPersistentReleaseState::Superseded
                {
                    return Err(Error::Conflict(
                        "rollback target is not a compatible prior approved release".into(),
                    ));
                }
                let current: fv::PersistentReleaseV2<MirrorPersistentRelease> =
                    source.need(ctx, "release", &current_id)?;
                let (current_host, current_set) = prior_release_source_r49(source, ctx, &current)?;
                source.reference = Some(target.report_ref.clone());
                cas_pointer_r49(
                    source,
                    ctx,
                    &host,
                    &set,
                    credential,
                    profile_id,
                    *expected_pointer_epoch,
                    Some(&current_id),
                    &target.release,
                )
                .await?;
                write_release_r49(
                    source,
                    ctx,
                    credential,
                    &current_host,
                    &current_set,
                    &current,
                    MirrorPersistentReleaseState::RolledBack,
                )
                .await?;
                write_release_r49(
                    source,
                    ctx,
                    credential,
                    &host,
                    &set,
                    &target,
                    MirrorPersistentReleaseState::Active,
                )
                .await?
            }
        };
        serde_json::to_value(result).map_err(crate::internal)
    }

    pub(super) fn preflight_host_complete_r49(
        raw: &RawDatabase,
        ctx: &Context,
        ticket: &MirrorEvaluationTicketV2,
        control: &MirrorRegisteredEvaluationControl,
    ) -> Result<()> {
        if control.evidence_scope != MirrorEvaluationEvidenceScope::RegisteredExternalHost {
            return Ok(());
        }
        ctx.require(&[Role::Evaluator])?;
        let mut captured = SourceR49::captured(raw, ctx, None)?;
        let s = &mut captured;
        let candidate: MirrorReleaseCandidateRecord =
            s.need(ctx, "artifact", &ticket.candidate_digest)?;
        let inputs = resolve_bundle_in_session(
            s,
            ctx,
            &candidate.bundle,
            &candidate.environment_digest,
            candidate.revoke_watermark,
            Purpose::Development,
        )?;
        let host: RegisteredFormalHost = attested_read(
            s,
            ctx,
            "artifact",
            &inputs.profile.host_registration_id,
            Role::Admin,
        )?;
        host.validate(ctx)?;
        let set = verify_candidate_principals(s, ctx, &host.id, &candidate, control)?;
        let native = process_principal(ctx, &host, &set, Role::Evaluator)?;
        let observed = ObservedFieldsR49 { pid: native.pid() };
        let seal = s
            .sealed_root_budget_evidence(ctx, &control.billing_scope)?
            .ok_or(Error::NotFound)?;
        if !seal.monetary_complete || !seal.unresolved.is_empty() {
            return Err(Error::Conflict("complete root cost unresolved".into()));
        }
        let cost: EpisodeCostRecord = attested_read(
            s,
            ctx,
            "artifact",
            &format!("formal-cost-{}", ticket.id),
            Role::Host,
        )?;
        cost.measurements.validate_complete()?;
        verify_refs(s, ctx, &cost.measurement_sources)?;
        if cost.ticket_id != ticket.id
            || cost.profile_registration_id != inputs.profile.id
            || cost.observed_uid != host.host_uid
        {
            return Err(Error::Conflict("episode meter scope mismatch".into()));
        }

        if cost.measurements.episode_id != control.attempt_id
            || seal.root.billing_scope != control.billing_scope
        {
            return Err(Error::Conflict("episode/root identity differs".into()));
        }
        validate_episode_meter_sources(
            s,
            ctx,
            &inputs.profile,
            &host,
            ticket,
            control,
            &seal,
            &cost.measurements,
            &cost.measurement_sources,
        )?;
        let mut roots = vec![
            ("artifact".into(), candidate.id.clone()),
            ("artifact".into(), cost.id.clone()),
            (
                "artifact".into(),
                evaluation_storage_id("evaluation_ticket_v2", &ticket.id)?,
            ),
            (
                "artifact".into(),
                evaluation_storage_id(
                    "registered_evaluation_control_v41",
                    &ticket.registration_id,
                )?,
            ),
            (
                "artifact".into(),
                evaluation_storage_id("protected_holdout_v41", &ticket.holdout_id)?,
            ),
            ("artifact".into(), set.id.clone()),
        ];
        for target in &ticket.targets {
            let MirrorTargetState::Completed {
                candidate_receipt_id,
                baseline_receipt_id,
                grader_receipt_id,
                ..
            } = &target.state
            else {
                return Err(Error::Conflict("formal target incomplete".into()));
            };
            for receipt_id in [candidate_receipt_id, baseline_receipt_id] {
                let receipt: MirrorExecutionReceiptV2 =
                    need_record(s, ctx, "execution_receipt_v2", receipt_id)?;
                let call = s
                    .budget_call(ctx, &control.billing_scope, &receipt.budget_call_id)?
                    .ok_or(Error::NotFound)?;
                if call.execution_provenance
                    != Some(crate::budget::BudgetExecutionProvenance::ExternalProvider)
                    || !call.execution_closed
                    || call.state != crate::budget::BudgetCallState::Finalized
                    || call.dispatch_group_id != ticket.id
                    || call.response_usable != Some(true)
                {
                    return Err(Error::Forbidden);
                }
                let id = format!(
                    "formal-capture-{}",
                    &hash(call.dispatch_id.as_ref().ok_or(Error::NotFound)?.as_bytes())[..32]
                );
                let capture: FormalProviderCaptureRecord =
                    attested_read(s, ctx, "artifact", &id, Role::Host)?;
                verify_provider_registration(s, ctx, &capture, &host)?;
                let request_artifact = call.request_artifact.as_ref().ok_or(Error::NotFound)?;
                if request_artifact.schema_version != "rsia.formal_task_request.v1" {
                    return Err(Error::Forbidden);
                }
                let actual_request: MirrorFormalTaskRequest =
                    serde_json::from_str(&request_artifact.body).map_err(|_| Error::Forbidden)?;
                actual_request.validate()?;
                let applied_bundle = if receipt.side == MirrorExecutionSide::Candidate {
                    candidate.bundle.clone()
                } else {
                    inputs.compile(
                        &SkillPatch {
                            content: evo_core::contract::PatchOp::ResetToBaseline,
                            applicability: evo_core::contract::PatchOp::ResetToBaseline,
                            counterexample: evo_core::contract::PatchOp::ResetToBaseline,
                            required_capabilities: evo_core::contract::PatchOp::ResetToBaseline,
                            dependencies: evo_core::contract::PatchOp::ResetToBaseline,
                        },
                        &ImproverPatch {
                            instruction: evo_core::contract::PatchOp::ResetToBaseline,
                            max_candidates: evo_core::contract::PatchOp::ResetToBaseline,
                            max_rounds: evo_core::contract::PatchOp::ResetToBaseline,
                        },
                    )?
                };
                let projection = evo_core::contract::project_run(
                    &applied_bundle,
                    &inputs.profile.system_snapshot,
                    true,
                )?;
                let mut application = vec![
                    serde_json::json!({"role":"system","content":actual_request.mandatory_context}),
                ];
                for instruction in &projection.instructions {
                    application.push(serde_json::json!({"role":"system","content":instruction}));
                }
                application.push(
                    serde_json::json!({"role":"user","content":actual_request.task_input_utf8}),
                );
                if actual_request.candidate_id != candidate.id
                    || actual_request.profile_registration_id != inputs.profile.id
                    || actual_request.compiler_binding_digest != inputs.binding.binding_digest
                    || actual_request.task.ticket_id != ticket.id
                    || actual_request.task.target_id != target.target_id
                    || actual_request.task.side != receipt.side
                    || actual_request.task.request_digest != receipt.request_digest
                    || actual_request.task.input_digest != target.input_digest
                    || applied_bundle.digest != receipt.bundle_digest
                    || hash(actual_request.mandatory_context.as_bytes())
                        != inputs.profile.system_snapshot.mandatory_context_digest
                    || fingerprint(&actual_request.projection)? != fingerprint(&projection)?
                    || fingerprint(&application)? != capture.application_digest
                {
                    return Err(Error::Conflict(
                        "actual request application differs from frozen P/B".into(),
                    ));
                }
                let transport = call.transport_artifact.as_ref().ok_or(Error::NotFound)?;
                if decode::<FormalProviderCaptureRecord>(
                    serde_json::from_str(&transport.body).map_err(|_| Error::Internal)?,
                )?
                .id != capture.id
                    || fingerprint(&capture)?
                        != fingerprint(&decode::<FormalProviderCaptureRecord>(
                            serde_json::from_str(&transport.body).map_err(|_| Error::Internal)?,
                        )?)?
                    || capture.host_registration_id != host.id
                    || capture.host_uid != host.host_uid
                    || capture.host_pid == observed.pid
                    || capture.host_principal != host.host_principal
                    || capture.host_code_digest != host.code_digest
                    || capture.host_workspace != host.host_workspace
                    || capture.request_digest != receipt.request_digest
                    || capture.bundle_digest != receipt.bundle_digest
                    || capture.output_digest != receipt.output_digest
                    || capture.input_digest != target.input_digest
                    || call.actual_cost_micros != Some(capture.actual_cost_micros)
                    || call.actual_currency.as_deref() != Some(capture.currency.as_str())
                    || call.actual_pricing_version.as_deref()
                        != Some(capture.pricing_version.as_str())
                    || seal.root.currency != capture.currency
                    || seal.root.pricing_version != capture.pricing_version
                    || call.output_digest.as_deref() != Some(capture.output_digest.as_str())
                    || call.provider_request_id.as_deref()
                        != Some(capture.provider_request_id.as_str())
                    || call.usage_record_id.as_deref() != Some(capture.usage_record_id.as_str())
                {
                    return Err(Error::Conflict(
                        "actual provider/host/output/usage mismatch".into(),
                    ));
                }
                roots.push(("artifact".into(), id));
                for kind_id in [
                    ("execution_receipt_v2", receipt_id.as_str()),
                    (
                        "evaluation_execution_output_v1",
                        receipt.output_artifact_id.as_str(),
                    ),
                ] {
                    roots.push((
                        "artifact".into(),
                        evaluation_storage_id(kind_id.0, kind_id.1)?,
                    ));
                }
            }
            let grade_id = format!(
                "formal-grade-v2-{}",
                &fingerprint(&(ticket.id.as_str(), target.target_id.as_str()))?[..32]
            );
            let grade: IndependentGradeCapture =
                attested_read(s, ctx, "artifact", &grade_id, Role::Evaluator)?;
            // The capture names the canonical digest of the stored receipt itself:
            // any changed field, or a capture of another digest, fails closed.
            let original: Value =
                need_record(s, ctx, "independent_grader_receipt_v2", grader_receipt_id)?;
            if grade.grader_receipt_digest != evo_core::canonical_fingerprint(&original)?
                || grade.grader_uid != host.grader_uid
                || grade.grader_principal != host.grader_principal
                || grade.grader_code_digest != host.grader_code_digest
                || grade.grader_workspace != host.grader_workspace
                || grade.ticket_id != ticket.id
                || grade.target_id != target.target_id
            {
                return Err(Error::Forbidden);
            }
            let native_grade = s.origin_for(ctx, "artifact", &grade_id)?;
            if native_grade.grader_pid != grade.grader_pid
                || native_grade.grader_uid != grade.grader_uid
                || native_grade.grader_code_digest != grade.grader_code_digest
                || native_grade.grader_workspace != grade.grader_workspace
            {
                return Err(Error::Conflict("native grade origin differs".into()));
            }
            roots.push(("artifact".into(), grade_id));
            roots.push((
                "artifact".into(),
                evaluation_storage_id("independent_grader_receipt_v2", grader_receipt_id)?,
            ));
        }

        // The future report and its future action are deliberately NOT source
        // rows. Every existing root, raw output/oracle and full actual resource
        // capture is checked here; actual report/action INSERT ACKs are checked
        // again in the same owning transaction below.
        let _closure = read_typed_closure(s, ctx, &roots)?;
        Ok(())
    }
    pub(super) async fn issue_inside_complete_owner_r49(
        session: &mut Session,
        ctx: &Context,
        reference: &fv::ReportRef,
        gate: Arc<AtomicU8>,
    ) -> Result<()> {
        let mut source = SourceR49::owned(session, ctx, Some(reference), gate).await?;
        let current = current_data_r49(&source)?;
        let ticket = &current.carrier.input.ticket_snapshot;
        if ticket.evidence_scope == MirrorEvaluationEvidenceScope::ProgramFixture {
            return Ok(());
        }
        let control = evaluation_control(&source.actual, ctx.namespace(), &ticket.registration_id)?;
        let gaps = inspect_known_host_sources_r49(
            &source.raw,
            &source.actual,
            ctx,
            reference,
            &current.carrier,
        )?;
        if !gaps.is_empty() {
            return Err(r24_fault(fv::Fault::SourceInsufficient));
        }
        issue_complete_authority_in_session(
            &mut source,
            ctx,
            ticket,
            &control,
            &current.carrier.payload,
        )
        .await?;
        source.stable().await
    }
    pub(super) fn reject_known_complete_attack_r49(
        raw: &RawDatabase,
        ctx: &Context,
        intent: &NativeIntent,
    ) -> Result<()> {
        let NativeIntent::Complete(value) = intent else {
            return Ok(());
        };
        let intent = TicketIntent::read(value.clone(), COMPLETE_INTENT_SCHEMA)?;
        let (actual, _) = snapshot(raw)?;
        let ticket = evaluation_ticket(&actual, ctx.namespace(), &intent.ticket_id)?;
        if ticket.evaluator_actor != ctx.actor() || ctx.role() != Role::Evaluator {
            return Err(Error::Forbidden);
        }
        let control = evaluation_control(&actual, ctx.namespace(), &ticket.registration_id)?;
        if control.evidence_scope != MirrorEvaluationEvidenceScope::RegisteredExternalHost {
            return Ok(());
        }
        let mut authentication = SourceR49::captured(raw, ctx, None)?;
        let candidate: MirrorReleaseCandidateRecord =
            authentication.need(ctx, "artifact", &ticket.candidate_digest)?;
        let host_id = host_id_for_profile(&mut authentication, ctx, &candidate.profile_id)?;
        let host: RegisteredFormalHost =
            attested_read(&mut authentication, ctx, "artifact", &host_id, Role::Admin)?;
        let principals = principal_set(&mut authentication, ctx, &host)?;
        let _actual_grader = process_principal(ctx, &host, &principals, Role::Evaluator)?;
        let holdout = evaluation_holdout(&actual, ctx.namespace(), &ticket.holdout_id, &control)?;
        // This path can only reject. It cannot supply a clock/floor/permit or
        // authorize a write; the unchanged full admission still follows it.
        for target in &ticket.targets {
            if matches!(target.state, MirrorTargetState::Completed { .. }) {
                let grader = e05_completed_target_evidence(
                    &actual,
                    ctx.namespace(),
                    &ticket,
                    &control,
                    &holdout,
                    target,
                )?;
                let receipt_id =
                    evaluation_storage_id("independent_grader_receipt_v2", &grader.receipt_id)?;
                let receipt = actual.object(ctx.namespace(), "artifact", &receipt_id)?;
                let capture_id = format!(
                    "formal-grade-v2-{}",
                    &fingerprint(&(ticket.id.as_str(), target.target_id.as_str()))?[..32]
                );
                let capture_object = actual.object(ctx.namespace(), "artifact", &capture_id)?;
                let capture: IndependentGradeCapture = decode(capture_object.body.clone())?;
                actual.witness(capture_object, "evaluator")?;
                let native = actual
                    .native_consumer_origins_r49
                    .get(&(
                        ctx.namespace().into(),
                        "artifact".into(),
                        capture_id.clone(),
                    ))
                    .ok_or(Error::Forbidden)?;
                if capture.schema_version != "rsia.independent_grade_capture.v2"
                    || capture.ticket_id != ticket.id
                    || capture.target_id != target.target_id
                    || capture.grader_receipt_digest != content_digest(&receipt.body["payload"])?
                    || native.body_content_digest != content_digest(&capture_object.body)?
                    || native.grader_actor != ticket.evaluator_actor
                    || native.grader_pid != capture.grader_pid
                    || native.grader_uid != capture.grader_uid
                    || native.grader_code_digest != capture.grader_code_digest
                    || native.grader_workspace != capture.grader_workspace
                {
                    return Err(Error::Forbidden);
                }
            }
        }
        preflight_host_complete_r49(raw, ctx, &ticket, &control)
    }

    pub(crate) fn load_native_origins_r49(
        raw: &RawDatabase,
        snapshot: &mut OccurrenceSnapshot,
    ) -> Result<()> {
        let mut rows = raw
            .tables
            .get("audit")
            .ok_or(Error::ClockRegressed)?
            .iter()
            .map(|row| Ok((native_integer(row, "seq")?, row)))
            .collect::<Result<Vec<_>>>()?;
        rows.sort_by_key(|(seq, _)| *seq);
        for (_, row) in rows {
            let payload: AuditPayloadR24 =
                serde_json::from_str(&native_text(row, "payload")?).map_err(crate::internal)?;
            if payload.operation != NATIVE_RECORD_OPERATION_R49 {
                continue;
            }
            parse_owned_json(&payload.entity)?;
            let proof: NativeRecordOriginR49 =
                serde_json::from_str(&payload.entity).map_err(crate::internal)?;
            let key = (
                proof.namespace.clone(),
                proof.kind.clone(),
                proof.id.clone(),
            );
            let previous = snapshot.native_consumer_origins_r49.get(&key);
            if native_text(row, "namespace")? != proof.namespace
                || proof.schema_version != "rsia.native.consumer_record_origin.v1"
                || payload.role != Role::Evaluator
                || payload.actor != proof.grader_actor
                || proof.grader_pid == 0
                || proof.previous_content_digest
                    != previous.map(|value| value.body_content_digest.clone())
            {
                return Err(Error::Forbidden);
            }
            for id in [
                &proof.namespace,
                &proof.kind,
                &proof.id,
                &proof.grader_actor,
            ] {
                identifier(id)?;
            }
            for value in [&proof.body_content_digest, &proof.grader_code_digest] {
                digest(value)?;
            }
            if let Some(reference) = &proof.report_ref {
                reference.validate_wire().map_err(r24_fault)?;
                if reference.namespace != proof.namespace {
                    return Err(Error::Forbidden);
                }
            }
            if let Some(request) = &proof.request_digest {
                digest(request)?;
            }
            snapshot.native_consumer_origins_r49.insert(key, proof);
        }
        Ok(())
    }
    pub(crate) fn static_release_action_r49(object: &OccurrenceObject) -> Result<()> {
        let record: ReleaseActionR49 = decode(object.body.clone())?;
        validate_release_action_r49(&record)?;
        if object.kind != "artifact"
            || record.id != object.id
            || record.action.namespace != object.namespace
        {
            return Err(Error::Forbidden);
        }
        for id in [
            &record.action.subject_kind,
            &record.action.subject_id,
            &record.action.actor,
            &record.action.host_registration_id,
        ] {
            identifier(id)?;
        }
        for value in [
            &record.action.subject_content_digest,
            &record.action.host_digest,
            &record.action.principal_set_digest,
            &record.action.credential_digest,
            &record.action.authority_executable_digest,
        ] {
            digest(value)?;
        }
        Ok(())
    }
    pub(crate) fn qualify_native_admin_r49(
        snapshot: &OccurrenceSnapshot,
        object: &OccurrenceObject,
    ) -> Result<bool> {
        let key = (
            object.namespace.clone(),
            object.kind.clone(),
            object.id.clone(),
        );
        let Some(proof) = snapshot.native_consumer_origins_r49.get(&key) else {
            if object.body["schema_version"] == fv::PERSISTENT_RELEASE_V2 {
                return Err(Error::ClockRegressed);
            }
            return Ok(false);
        };
        let host = e05_subject_host(snapshot, object)?.ok_or(Error::Forbidden)?;
        let observed = snapshot.object(
            &object.namespace,
            "artifact",
            &format!("formal-principals-{}", host.id),
        )?;
        snapshot.witness(observed, "admin")?;
        snapshot.witness(observed, "evaluator")?;
        let observed: MirrorPrincipalSetObservation = super::super::decode(&observed.body)?;
        let set = &observed.principals;
        set.validate(&host)?;
        if observed.namespace != object.namespace
            || observed.host_digest != typed_fingerprint(&host)?
            || observed.grader_uid != host.grader_uid
            || observed.grader_executable_digest != host.grader_code_digest
            || observed.grader_workspace != host.grader_workspace
            || observed.grader_pid == 0
        {
            return Err(Error::Forbidden);
        }
        if object.kind == "pointer" {
            let pointer: MirrorPersistentProfilePointer = super::super::decode(&object.body)?;
            let release_id = pointer
                .active_release_id
                .as_deref()
                .ok_or(Error::Forbidden)?;
            let release = snapshot.object(&object.namespace, "release", release_id)?;
            let release: fv::PersistentReleaseV2<MirrorPersistentRelease> =
                decode(release.body.clone())?;
            release.validate_known().map_err(r24_fault)?;
            if proof.report_ref.as_ref() != Some(&release.report_ref) {
                return Err(Error::Forbidden);
            }
        } else if object.body["schema_version"] == fv::PERSISTENT_RELEASE_V2 {
            let release: fv::PersistentReleaseV2<MirrorPersistentRelease> =
                decode(object.body.clone())?;
            if proof.report_ref.as_ref() != Some(&release.report_ref) {
                return Err(Error::Forbidden);
            }
        }
        let action_id = proof.action_id.as_deref().ok_or(Error::Forbidden)?;
        let record_object = snapshot.object(&object.namespace, "artifact", action_id)?;
        static_release_action_r49(record_object)?;
        snapshot.witness(record_object, "evaluator")?;
        let record: ReleaseActionR49 = decode(record_object.body.clone())?;
        let action = &record.action;
        let administrator = set.principal(Role::Admin)?;
        if proof.body_content_digest != content_digest(&object.body)?
            || proof.grader_actor != set.grader.actor
            || proof.grader_uid != host.grader_uid
            || proof.grader_code_digest != host.grader_code_digest
            || proof.grader_workspace != host.grader_workspace
            || record_object.owner != set.grader.actor
            || record.id != action_id
            || action.namespace != object.namespace
            || action.role != Role::Admin
            || action.actor != administrator.actor
            || action.uid != administrator.uid
            || action.peer_uid != administrator.uid
            || action.principal != administrator.principal
            || action.credential_digest != administrator.credential_digest
            || action.host_registration_id != host.id
            || action.host_digest != typed_fingerprint(&host)?
            || action.principal_set_digest != set.binding_digest()?
            || action.subject_kind != object.kind
            || action.subject_id != object.id
            || action.subject_content_digest != proof.body_content_digest
            || action.authority_pid != proof.grader_pid
            || action.authority_uid != proof.grader_uid
            || action.authority_executable_digest != proof.grader_code_digest
            || proof.request_digest.as_deref() != Some(action.authenticated_request_digest.as_str())
            || proof.peer_uid != Some(action.peer_uid)
            || proof.peer_gid != action.peer_gid
            || proof.peer_pid != action.peer_pid
        {
            return Err(Error::Forbidden);
        }
        if let Some(previous) = &record.previous_action_id {
            let previous = snapshot.object(&object.namespace, "artifact", previous)?;
            static_release_action_r49(previous)?;
            snapshot.witness(previous, "evaluator")?;
            let previous: ReleaseActionR49 = decode(previous.body.clone())?;
            if previous.action.subject_kind != object.kind
                || previous.action.subject_id != object.id
                || proof.previous_content_digest.as_deref()
                    != Some(previous.action.subject_content_digest.as_str())
            {
                return Err(Error::Forbidden);
            }
        }
        Ok(true)
    }

    #[cfg(test)]
    mod r49_release_command_codec_controls {
        use super::*;
        #[test]
        fn release_commands_are_explicit_complete_and_reject_legacy_or_extra_fields() {
            for raw in [
                serde_json::json!({"operation":"canary_current_v2","release_id":"release-a"}),
                serde_json::json!({"operation":"activate_current_v2","release_id":"release-a","expected_pointer_epoch":3}),
                serde_json::json!({"operation":"rollback_current_v2","profile_id":"profile-a","target_release_id":"release-b",
                "expected_pointer_epoch":7,"environment_digest":"1".repeat(64)}),
            ] {
                let command: ReleaseCommandR49 = serde_json::from_value(raw.clone()).unwrap();
                assert_eq!(serde_json::to_value(command).unwrap(), raw);
                let mut wrong = raw;
                wrong["verified"] = Value::Bool(true);
                assert!(serde_json::from_value::<ReleaseCommandR49>(wrong).is_err());
            }
            assert!(
                serde_json::from_value::<ReleaseCommandR49>(
                    serde_json::json!({"operation":"canary","release_id":"release-a"})
                )
                .is_err()
            );
            assert!(
                serde_json::from_value::<ReleaseCommandR49>(
                    serde_json::json!({"operation":"activate_current_v2","release_id":"release-a"})
                )
                .is_err()
            );
        }
    }

    fn inspect_existing_native_origins_known_r49(
        raw: &RawDatabase,
        actual: &OccurrenceSnapshot,
        ctx: &Context,
    ) -> Result<()> {
        // collect_audit has already checked the actual complete hash/seq chain.
        // These are known contradictions only. Missing/redacted subjects are
        // still source gaps checked by the ordinary consumer/source factory.
        let mut source = SourceR49::captured(raw, ctx, None)?;
        for row in raw.tables.get("audit").ok_or(Error::ClockRegressed)? {
            if native_text(row, "namespace")? != ctx.namespace() {
                continue;
            }
            let payload_text = native_text(row, "payload")?;
            parse_owned_json(&payload_text)?;
            let payload: AuditPayloadR24 =
                serde_json::from_str(&payload_text).map_err(crate::internal)?;
            if payload.operation != NATIVE_RECORD_OPERATION_R49 {
                continue;
            }
            parse_owned_json(&payload.entity)?;
            let proof: NativeRecordOriginR49 =
                serde_json::from_str(&payload.entity).map_err(|_| Error::Forbidden)?;
            if proof.schema_version != "rsia.native.consumer_record_origin.v1"
                || proof.namespace != ctx.namespace()
                || payload.role != Role::Evaluator
                || payload.actor != proof.grader_actor
                || proof.grader_pid == 0
            {
                return Err(Error::Forbidden);
            }
            for id in [&proof.kind, &proof.id, &proof.grader_actor] {
                identifier(id)?;
            }
            for digest_value in [&proof.body_content_digest, &proof.grader_code_digest] {
                digest(digest_value)?;
            }
            if let Some(reference) = &proof.report_ref {
                reference.validate_wire().map_err(r24_fault)?;
                if reference.namespace != proof.namespace {
                    return Err(Error::Forbidden);
                }
            }
            if let Some(request) = &proof.request_digest {
                digest(request)?;
            }
            let Some(subject) =
                actual
                    .objects
                    .get(&(ctx.namespace().into(), proof.kind.clone(), proof.id.clone()))
            else {
                continue;
            };
            if subject.body["schema_version"] == "rsia.redacted.v1" {
                qualify_redacted(subject)?;
                continue;
            }
            let latest = source
                .latest_origin_r49(ctx, &proof.kind, &proof.id)?
                .ok_or(Error::Forbidden)?;
            if content_digest(&subject.body)? != latest.body_content_digest {
                return Err(Error::Forbidden);
            }
            if let Some(action_id) = &proof.action_id {
                let record: ReleaseActionR49 = source.need(ctx, "artifact", action_id)?;
                validate_release_action_r49(&record)?;
                if record.action.subject_kind != proof.kind
                    || record.action.subject_id != proof.id
                    || record.action.subject_content_digest != proof.body_content_digest
                    || record.action.peer_uid != proof.peer_uid.ok_or(Error::Forbidden)?
                    || record.action.peer_gid != proof.peer_gid
                    || record.action.peer_pid != proof.peer_pid
                    || Some(record.action.authenticated_request_digest.as_str())
                        != proof.request_digest.as_deref()
                {
                    return Err(Error::Forbidden);
                }
            }
            if proof.report_ref.is_none()
                && subject.body["schema_version"] != "rsia.independent_grade_capture.v2"
            {
                return Err(Error::Forbidden);
            }
            let host_id = subject
                .body
                .get("host_registration_id")
                .and_then(Value::as_str)
                .or_else(|| {
                    subject
                        .body
                        .get("authority")
                        .and_then(|authority| authority.get("host_registration_id"))
                        .and_then(Value::as_str)
                });
            if let Some(host_id) = host_id
                && let Some(host) =
                    actual
                        .objects
                        .get(&(ctx.namespace().into(), "artifact".into(), host_id.into()))
                && host.body["schema_version"] != "rsia.redacted.v1"
            {
                let host: RegisteredFormalHost = decode(host.body.clone())?;
                host.validate(ctx)?;
                if proof.grader_uid != host.grader_uid
                    || proof.grader_code_digest != host.grader_code_digest
                    || proof.grader_workspace != host.grader_workspace
                {
                    return Err(Error::Forbidden);
                }
            }
        }
        Ok(())
    }

    fn inspect_known_host_sources_r49(
        raw: &RawDatabase,
        actual: &OccurrenceSnapshot,
        ctx: &Context,
        reference: &evo_core::formal_version::ReportRef,
        carrier: &FrozenCarrierR36,
    ) -> Result<BTreeSet<&'static str>> {
        inspect_existing_native_origins_known_r49(raw, actual, ctx)?;
        let mut gaps = BTreeSet::new();
        let ticket = &carrier.input.ticket_snapshot;
        let action_id = principal_action_id("artifact", &reference.physical_id, Role::Evaluator)?;
        let mut expected = vec![
            (
                "artifact".to_owned(),
                evaluation_storage_id(TICKET_KIND, &ticket.id)?,
            ),
            (
                "artifact".to_owned(),
                evaluation_storage_id(RESOURCE_KIND, &ticket.id)?,
            ),
        ];
        if actual.objects.contains_key(&(
            ctx.namespace().into(),
            "artifact".into(),
            action_id.clone(),
        )) {
            expected.push(("artifact".to_owned(), action_id));
        }
        expected.sort();
        if e05_outgoing(actual, ctx.namespace(), "artifact", &reference.physical_id)? != expected {
            return Err(Error::Conflict(
                "formal fact dependency edges differ".into(),
            ));
        }

        let candidate = known_object_r49(
            actual,
            ctx,
            "artifact",
            &ticket.candidate_digest,
            Some("rsia.release_candidate.v1"),
            None,
            &mut gaps,
        )?
        .map(|object| decode::<MirrorReleaseCandidateRecord>(object.body.clone()))
        .transpose()?;
        if let Some(candidate) = candidate.as_ref() {
            validate_bundle(&candidate.bundle)?;
            if candidate.id != ticket.candidate_digest
                || candidate.bundle_digest != ticket.candidate_bundle_digest
                || candidate.environment_digest != ticket.environment_digest
                || candidate.proposer_actor != ticket.proposer_actor
                || candidate.parent_digest != ticket.baseline_digest
            {
                return Err(Error::Conflict(
                    "known Host candidate/ticket differs".into(),
                ));
            }
        }
        let control_id =
            evaluation_storage_id("registered_evaluation_control_v41", &ticket.registration_id)?;
        let control = known_object_r49(
            actual,
            ctx,
            "artifact",
            &control_id,
            None,
            Some("evaluator"),
            &mut gaps,
        )?
        .map(|object| {
            envelope(
                object,
                "rsia.typed_artifact_envelope.v1",
                "registered_evaluation_control_v41",
            )
            .and_then(|value| decode::<MirrorRegisteredEvaluationControl>(value.clone()))
        })
        .transpose()?;
        if let Some(control) = control.as_ref() {
            control.validate()?;
            if ticket.registration_digest != typed_fingerprint(control)?
                || ticket.evaluator_actor != control.evaluator_actor
                || ticket.formal_plan_digest != control.formal_plan.digest()?
                || ticket.n_planned != control.formal_plan.n_planned
            {
                return Err(Error::Conflict("known Host control/ticket differs".into()));
            }
        }
        let profile_id = candidate
            .as_ref()
            .map(|candidate| candidate.profile_id.as_str());
        let selection = profile_id
            .map(|id| format!("formal-profile-{id}"))
            .map(|id| {
                known_object_r49(
                    actual,
                    ctx,
                    "artifact",
                    &id,
                    Some("rsia.formal_profile_selection.v1"),
                    Some("admin"),
                    &mut gaps,
                )
                .and_then(|value| {
                    value
                        .map(|object| decode::<ProfileSelection>(object.body.clone()))
                        .transpose()
                })
            })
            .transpose()?
            .flatten();
        let profile = selection
            .as_ref()
            .map(|selection| {
                known_object_r49(
                    actual,
                    ctx,
                    "artifact",
                    &selection.registration_id,
                    Some(PROFILE_SCHEMA),
                    Some("admin"),
                    &mut gaps,
                )
                .and_then(|value| {
                    value
                        .map(|object| decode::<FrozenCompilerProfile>(object.body.clone()))
                        .transpose()
                })
            })
            .transpose()?
            .flatten();
        if let Some(profile) = profile.as_ref() {
            profile.validate(ctx)?;
            let profile_digest = fingerprint(profile)?;
            if profile.environment_digest != ticket.environment_digest
                || selection
                    .as_ref()
                    .is_some_and(|selection| selection.registration_digest != profile_digest)
            {
                return Err(Error::Conflict("known Host profile/ticket differs".into()));
            }
        }
        let host = profile
            .as_ref()
            .map(|profile| {
                known_object_r49(
                    actual,
                    ctx,
                    "artifact",
                    &profile.host_registration_id,
                    Some(HOST_SCHEMA),
                    Some("admin"),
                    &mut gaps,
                )
                .and_then(|value| {
                    value
                        .map(|object| decode::<RegisteredFormalHost>(object.body.clone()))
                        .transpose()
                })
            })
            .transpose()?
            .flatten();
        if let Some(host) = host.as_ref() {
            host.validate(ctx)?;
        }
        let set_observation = host
            .as_ref()
            .map(|host| {
                known_object_r49(
                    actual,
                    ctx,
                    "artifact",
                    &format!("formal-principals-{}", host.id),
                    Some("rsia.formal_principal_set_observation.v1"),
                    Some("evaluator"),
                    &mut gaps,
                )
                .and_then(|value| {
                    value
                        .map(|object| decode::<PrincipalSetObservation>(object.body.clone()))
                        .transpose()
                })
            })
            .transpose()?
            .flatten();
        if let (Some(host), Some(observation)) = (&host, &set_observation) {
            observation.principals.validate(host)?;
            if observation.namespace != ctx.namespace()
                || observation.host_digest != fingerprint(host)?
                || observation.grader_uid != host.grader_uid
                || observation.grader_pid == 0
                || observation.grader_executable_digest != host.grader_code_digest
                || observation.grader_workspace != host.grader_workspace
            {
                return Err(Error::Conflict(
                    "known Host principal observation differs".into(),
                ));
            }
        }
        let cost = known_object_r49(
            actual,
            ctx,
            "artifact",
            &format!("formal-cost-{}", ticket.id),
            Some("rsia.complete_episode_cost.v1"),
            Some("host"),
            &mut gaps,
        )?
        .map(|object| decode::<EpisodeCostRecord>(object.body.clone()))
        .transpose()?;
        if let Some(cost) = &cost {
            cost.measurements.validate_complete()?;
            if cost.ticket_id != ticket.id
                || profile
                    .as_ref()
                    .is_some_and(|profile| profile.id != cost.profile_registration_id)
                || host
                    .as_ref()
                    .is_some_and(|host| host.host_uid != cost.observed_uid)
                || control
                    .as_ref()
                    .is_some_and(|control| control.attempt_id != cost.measurements.episode_id)
            {
                return Err(Error::Conflict("known Host cost identity differs".into()));
            }
            for measurement in &cost.measurement_sources {
                let actual_meter = known_object_r49(
                    actual,
                    ctx,
                    &measurement.kind,
                    &measurement.id,
                    Some("rsia.trusted_resource_meter.v1"),
                    Some("host"),
                    &mut gaps,
                )?;
                if let Some(object) = actual_meter {
                    let meter: ResourceMeasurementRecord = decode(object.body.clone())?;
                    let response = validate_meter_response(&meter)?;
                    if measurement.content_digest != content_digest(&object.body)?
                        || meter.ticket_id != ticket.id
                        || control.as_ref().is_some_and(|control| {
                            meter.episode_id != control.attempt_id
                                || meter.billing_scope != control.billing_scope
                        })
                        || host.as_ref().is_some_and(|host| {
                            meter.host_registration_id != host.id
                                || meter.observed_uid != host.host_uid
                        })
                        || response.episode_id != meter.episode_id
                    {
                        return Err(Error::Conflict("known Host meter identity differs".into()));
                    }
                }
            }
        }
        for target in &ticket.targets {
            let MirrorTargetState::Completed {
                candidate_receipt_id,
                baseline_receipt_id,
                grader_receipt_id,
                ..
            } = &target.state
            else {
                return Err(Error::Conflict("known Host target incomplete".into()));
            };
            for id in [candidate_receipt_id, baseline_receipt_id] {
                let physical = evaluation_storage_id("execution_receipt_v2", id)?;
                let receipt =
                    known_object_r49(actual, ctx, "artifact", &physical, None, None, &mut gaps)?
                        .map(|object| {
                            envelope(
                                object,
                                "rsia.typed_artifact_envelope.v1",
                                "execution_receipt_v2",
                            )
                            .and_then(|value| decode::<MirrorExecutionReceiptV2>(value.clone()))
                        })
                        .transpose()?;
                let Some(receipt) = receipt else { continue };
                if receipt.ticket_id != ticket.id || receipt.target_id != target.target_id {
                    return Err(Error::Conflict("known Host receipt/ticket differs".into()));
                }
                let Some(control) = control.as_ref() else {
                    continue;
                };
                let Some(call) = actual.calls.get(&(
                    control.billing_scope.clone(),
                    receipt.budget_call_id.clone(),
                )) else {
                    gaps.insert("host_actual_budget_call_missing");
                    continue;
                };
                // Every available original call is qualified independently of
                // unrelated missing meter/authority/capture evidence.
                let call = qualified_call(
                    actual,
                    ctx.namespace(),
                    &control.billing_scope,
                    &call.call_id,
                )?;
                if call.dispatch_group_id != ticket.id
                    || !call.execution_closed
                    || call.execution_provenance
                        != Some(crate::budget::BudgetExecutionProvenance::ExternalProvider)
                    || call.state != crate::budget::BudgetCallState::Finalized
                    || call.response_usable != Some(true)
                {
                    return Err(Error::Conflict("known Host actual call differs".into()));
                }
                let capture_id = call
                    .dispatch_id
                    .as_ref()
                    .map(|dispatch| format!("formal-capture-{}", &hash(dispatch.as_bytes())[..32]));
                let Some(capture_id) = capture_id else {
                    gaps.insert("host_dispatch_id_missing");
                    continue;
                };
                let capture = known_object_r49(
                    actual,
                    ctx,
                    "artifact",
                    &capture_id,
                    Some("rsia.actual_formal_provider_capture.v1"),
                    Some("host"),
                    &mut gaps,
                )?
                .map(|object| decode::<FormalProviderCaptureRecord>(object.body.clone()))
                .transpose()?;
                if let Some(capture) = capture {
                    validate_provider_response(&capture)?;
                    if capture.request_digest != receipt.request_digest
                        || capture.bundle_digest != receipt.bundle_digest
                        || capture.output_digest != receipt.output_digest
                        || capture.input_digest != target.input_digest
                        || call.actual_cost_micros != Some(capture.actual_cost_micros)
                        || call.actual_currency.as_deref() != Some(capture.currency.as_str())
                        || call.actual_pricing_version.as_deref()
                            != Some(capture.pricing_version.as_str())
                        || call.output_digest.as_deref() != Some(capture.output_digest.as_str())
                        || call.provider_request_id.as_deref()
                            != Some(capture.provider_request_id.as_str())
                        || call.usage_record_id.as_deref() != Some(capture.usage_record_id.as_str())
                        || host.as_ref().is_some_and(|host| {
                            capture.host_registration_id != host.id
                                || capture.host_uid != host.host_uid
                                || capture.host_principal != host.host_principal
                                || capture.host_code_digest != host.code_digest
                                || capture.host_workspace != host.host_workspace
                        })
                    {
                        return Err(Error::Conflict(
                            "known Host capture/call/receipt differs".into(),
                        ));
                    }
                    if let Some(object) = known_object_r49(
                        actual,
                        ctx,
                        "artifact",
                        &capture.provider_registration_id,
                        None,
                        Some("admin"),
                        &mut gaps,
                    )? {
                        let provider: FrozenProviderRegistration = decode(object.body.clone())?;
                        provider.validate(ctx)?;
                        let parsed = validate_provider_response(&capture)?;
                        let micros = u128::from(capture.input_tokens)
                            .checked_mul(u128::from(provider.input_micros_per_million_tokens))
                            .and_then(|n| {
                                n.checked_add(
                                    u128::from(capture.output_tokens)
                                        * u128::from(provider.output_micros_per_million_tokens),
                                )
                            })
                            .and_then(|n| n.checked_add(999999))
                            .ok_or(Error::Budget)?
                            / 1000000;
                        if fingerprint(&provider)? != capture.provider_registration_digest
                            || provider.model_digest != capture.actual_model_digest
                            || parsed["model"].as_str() != Some(provider.provider_model.as_str())
                            || capture.currency != provider.currency
                            || capture.pricing_version != provider.pricing_version
                            || capture.output_tokens > u64::from(provider.max_output_tokens)
                            || i64::try_from(micros).map_err(|_| Error::Budget)?
                                != capture.actual_cost_micros
                        {
                            return Err(Error::Conflict(
                                "known provider pricing/model differs".into(),
                            ));
                        }
                    }
                }
            }
            let capture_id = format!(
                "formal-grade-v2-{}",
                &fingerprint(&(ticket.id.as_str(), target.target_id.as_str()))?[..32]
            );
            if let Some(object) = known_object_r49(
                actual,
                ctx,
                "artifact",
                &capture_id,
                Some("rsia.independent_grade_capture.v2"),
                Some("evaluator"),
                &mut gaps,
            )? {
                let capture: IndependentGradeCapture = decode(object.body.clone())?;
                let physical =
                    evaluation_storage_id("independent_grader_receipt_v2", grader_receipt_id)?;
                if let Some(receipt) =
                    known_object_r49(actual, ctx, "artifact", &physical, None, None, &mut gaps)?
                {
                    let value = envelope(
                        receipt,
                        "rsia.typed_artifact_envelope.v1",
                        "independent_grader_receipt_v2",
                    )?;
                    if capture.grader_receipt_digest != content_digest(value)? {
                        return Err(Error::Forbidden);
                    }
                }
                if capture.ticket_id != ticket.id
                    || capture.target_id != target.target_id
                    || capture.grader_pid == 0
                    || host.as_ref().is_some_and(|host| {
                        capture.grader_uid != host.grader_uid
                            || capture.grader_principal != host.grader_principal
                            || capture.grader_code_digest != host.grader_code_digest
                            || capture.grader_workspace != host.grader_workspace
                    })
                {
                    return Err(Error::Conflict(
                        "known grader capture identity differs".into(),
                    ));
                }
            }
        }
        if let Some(control) = control {
            let seal = strict_latest_root_seal_r42(raw, actual, ctx, &control.billing_scope)?;
            if let Ok(Some(seal)) = seal.evidence() {
                let id = evo_core::formal_version::formal_authority_id_v3(
                    reference,
                    &fingerprint(seal)?,
                )
                .map_err(r24_fault)?;
                if actual.objects.contains_key(&(
                    ctx.namespace().into(),
                    "artifact".into(),
                    id.clone(),
                )) && let Some(object) = known_object_r49(
                    actual,
                    ctx,
                    "artifact",
                    &id,
                    Some(evo_core::formal_version::FORMAL_AUTHORITY_V3),
                    Some("evaluator"),
                    &mut gaps,
                )? {
                    let authority: evo_core::formal_version::FormalAuthorityV3<
                        IssuedFormalAuthority,
                    > = decode(object.body.clone())?;
                    authority.validate_known().map_err(r24_fault)?;
                    if authority.report_ref != *reference
                        || object.owner != reference.owner
                        || authority.authority.candidate_id != ticket.candidate_digest
                        || authority.authority.ticket_digest != reference.ticket_digest
                        || authority.authority.issued_by != ticket.evaluator_actor
                    {
                        return Err(Error::Conflict("known authority Ref/owner differs".into()));
                    }
                }
            }
        }
        Ok(gaps)
    }

    fn known_current_reference_r49(
        object: &OccurrenceObject,
    ) -> Result<evo_core::formal_version::ReportRef> {
        let carrier: FrozenCarrierR36 =
            serde_json::from_value(object.body.clone()).map_err(crate::internal)?;
        if carrier.id != object.id
            || carrier.input.namespace != object.namespace
            || carrier.input.ticket_snapshot.evaluator_actor != object.owner
        {
            return Err(Error::Conflict(
                "formal carrier owner/identity differs".into(),
            ));
        }
        known_complete_reference_r36(&carrier, &object.owner)
    }
    struct CurrentDataR49 {
        reference: evo_core::formal_version::ReportRef,
        carrier: FrozenCarrierR36,
    }
    fn current_data_r49(source: &SourceR49<'_>) -> Result<CurrentDataR49> {
        let reference = source.reference.as_ref().ok_or(Error::Forbidden)?;
        let (carrier, _) = decode_complete_known_raw_r36(&source.raw, reference)?;
        // Concrete known fields/original row/Ref are checked before any source
        // absence. Host auxiliary known facts are inspected separately below.
        let _actual = authenticated_frozen_reader_r39::authenticate(
            &source.raw,
            &source.actual,
            &source.context,
            reference,
        )?;
        Ok(CurrentDataR49 {
            reference: reference.clone(),
            carrier,
        })
    }
    pub struct NativeHostEvidenceR49 {
        reference: evo_core::formal_version::ReportRef,
        authority: Value,
        authority_digest: String,
        report_digest: String,
        closure: Value,
    }
    impl NativeHostEvidenceR49 {
        pub fn reference(&self) -> &evo_core::formal_version::ReportRef {
            &self.reference
        }
        pub fn authority(&self) -> &Value {
            &self.authority
        }
        pub fn authority_digest(&self) -> &str {
            &self.authority_digest
        }
        pub fn report_digest(&self) -> &str {
            &self.report_digest
        }
        pub fn typed_closure(&self) -> &Value {
            &self.closure
        }
    }
    pub(super) enum HostInspectionR49 {
        NotApplicable,
        Missing,
        Refused(Error),
        Ready(NativeHostEvidenceR49),
    }
    impl HostInspectionR49 {
        pub(super) fn evidence(&self) -> Result<Option<&NativeHostEvidenceR49>> {
            match self {
                Self::NotApplicable => Ok(None),
                Self::Missing => Err(r24_fault(
                    evo_core::formal_version::Fault::SourceInsufficient,
                )),
                Self::Refused(error) => Err(copy_host_error_r49(error)),
                Self::Ready(evidence) => Ok(Some(evidence)),
            }
        }
    }
    fn verify_issued_authority_r49(
        source: &mut SourceR49<'_>,
        ctx: &Context,
        current: &CurrentDataR49,
    ) -> Result<evo_core::formal_version::FormalAuthorityV3<IssuedFormalAuthority>> {
        let ticket = &current.carrier.input.ticket_snapshot;
        let control = evaluation_control(&source.actual, ctx.namespace(), &ticket.registration_id)?;
        let seal = source
            .sealed_root_budget_evidence(ctx, &control.billing_scope)?
            .ok_or_else(|| r24_fault(evo_core::formal_version::Fault::SourceInsufficient))?;
        let id = evo_core::formal_version::formal_authority_id_v3(
            &current.reference,
            &fingerprint(&seal)?,
        )
        .map_err(r24_fault)?;
        let stored: Value = source.get(ctx, "artifact", &id)?.ok_or_else(|| {
            Error::Conflict("issued formal authority missing for external-host report".into())
        })?;
        let envelope: evo_core::formal_version::FormalAuthorityV3<IssuedFormalAuthority> =
            decode(stored.clone())?;
        envelope.validate_known().map_err(r24_fault)?;
        if envelope.report_ref != current.reference
            || envelope.id != id
            || envelope.input_digest != fingerprint(&seal)?
        {
            return Err(Error::Conflict("formal authority Ref/input differs".into()));
        }
        require_content_witness(source, ctx, "artifact", &id, &stored, Role::Evaluator)?;
        let proof = source.origin_for(ctx, "artifact", &id)?;
        if proof.report_ref.as_ref() != Some(&current.reference)
            || proof.grader_actor != ticket.evaluator_actor
        {
            return Err(Error::Forbidden);
        }
        let evaluator = Context::new(ctx.namespace(), &ticket.evaluator_actor, Role::Evaluator)?;
        let actual = derive_complete_authority_r49(
            source,
            &evaluator,
            ticket,
            &control,
            &current.carrier.payload,
        )?;
        if fingerprint(&actual)? != envelope.authority_digest
            || fingerprint(&actual)? != fingerprint(&envelope.authority)?
        {
            return Err(Error::Conflict(
                "actual Host authority source changed".into(),
            ));
        }
        Ok(envelope)
    }
    pub(super) fn inspect_host_r49(
        raw: &RawDatabase,
        actual: &OccurrenceSnapshot,
        ctx: &Context,
        reference: &evo_core::formal_version::ReportRef,
    ) -> Result<HostInspectionR49> {
        let (carrier, _) = decode_complete_known_raw_r36(raw, reference)?;
        if carrier.input.ticket_snapshot.evidence_scope
            != MirrorEvaluationEvidenceScope::RegisteredExternalHost
            || !matches!(
                &carrier.payload,
                MirrorFormalEvaluationV2::CompleteBatch { .. }
            )
        {
            return Ok(HostInspectionR49::NotApplicable);
        }
        let gaps = inspect_known_host_sources_r49(raw, actual, ctx, reference, &carrier)?;
        // A gap is never returned until all currently available known facts
        // have been independently checked by the preceding scan.
        if !gaps.is_empty() {
            return Ok(HostInspectionR49::Missing);
        }
        let mut source = SourceR49::captured(raw, ctx, Some(reference))?;
        let current = current_data_r49(&source)?;
        let authority = match verify_issued_authority_r49(&mut source, ctx, &current) {
            Ok(authority) => authority,
            Err(error) => return Ok(HostInspectionR49::Refused(error)),
        };
        let report_digest = fingerprint(&(&current.carrier.payload, &authority.authority))?;
        Ok(HostInspectionR49::Ready(NativeHostEvidenceR49 {
            reference: reference.clone(),
            authority_digest: fingerprint(&authority)?,
            closure: serde_json::to_value(&authority.authority.closure).map_err(crate::internal)?,
            authority: serde_json::to_value(authority).map_err(crate::internal)?,
            report_digest,
        }))
    }

    /// Exact wire checks only. This function grants no authority and does not
    /// replace the actual capture/origin checks in the fixed reader below.
    pub fn validate_native_compiler_value_r49(
        kind: &str,
        id: &str,
        body: &Value,
    ) -> Result<Option<String>> {
        let schema = body
            .get("schema_version")
            .and_then(Value::as_str)
            .unwrap_or("");
        let recognized = matches!(
            (kind, schema),
            ("release", fv::PERSISTENT_RELEASE_V2)
                | (
                    "artifact",
                    fv::FORMAL_AUTHORITY_V3
                        | "rsia.independent_grade_capture.v2"
                        | "rsia.release_action.v2"
                )
        );
        if !recognized {
            return Ok(None);
        }
        identifier(id)?;
        if body["id"] != id
            || serde_json::to_vec(body).map_err(crate::internal)?.len() > 4 * 1024 * 1024
        {
            return Err(Error::Conflict(
                "native compiler value identity/capacity differs".into(),
            ));
        }
        match schema {
            fv::PERSISTENT_RELEASE_V2 => {
                let value: fv::PersistentReleaseV2<MirrorPersistentRelease> = decode(body.clone())?;
                value.validate_known().map_err(r24_fault)?;
            }
            fv::FORMAL_AUTHORITY_V3 => {
                let value: fv::FormalAuthorityV3<IssuedFormalAuthority> = decode(body.clone())?;
                value.validate_known().map_err(r24_fault)?;
            }
            "rsia.independent_grade_capture.v2" => {
                let value: IndependentGradeCapture = decode(body.clone())?;
                for value in [
                    &value.ticket_id,
                    &value.target_id,
                    &value.host_registration_id,
                ] {
                    identifier(value)?;
                }
                for value in [&value.grader_receipt_digest, &value.grader_code_digest] {
                    digest(value)?;
                }
                if value.grader_pid == 0
                    || value.id
                        != format!(
                            "formal-grade-v2-{}",
                            &fingerprint(&(value.ticket_id.as_str(), value.target_id.as_str()))?
                                [..32]
                        )
                {
                    return Err(Error::Forbidden);
                }
            }
            "rsia.release_action.v2" => {
                let value: ReleaseActionR49 = decode(body.clone())?;
                validate_release_action_r49(&value)?;
                for value in [
                    &value.action.namespace,
                    &value.action.subject_kind,
                    &value.action.subject_id,
                    &value.action.actor,
                    &value.action.host_registration_id,
                ] {
                    identifier(value)?;
                }
                for value in [
                    &value.action.subject_content_digest,
                    &value.action.host_digest,
                    &value.action.principal_set_digest,
                    &value.action.credential_digest,
                    &value.action.authority_executable_digest,
                ] {
                    digest(value)?;
                }
            }
            _ => return Err(Error::Forbidden),
        }
        Ok(Some(schema.into()))
    }
    /// Private-constructor data of one real held SQLx transaction. No caller
    /// wire can construct this value and it carries no mutation capability.
    pub struct NativeCompilerClosureDataR49 {
        closure: Value,
    }
    impl NativeCompilerClosureDataR49 {
        pub fn closure(&self) -> &Value {
            &self.closure
        }
    }
    fn compiler_has_native_chain_r49(
        source: &mut SourceR49<'_>,
        ctx: &Context,
        roots: &[(String, String)],
    ) -> Result<bool> {
        let mut pending = roots.to_vec();
        let mut seen = BTreeSet::new();
        let mut bytes = 0usize;
        let mut edge_count = 0usize;
        while let Some((kind, id)) = pending.pop() {
            identifier(&kind)?;
            identifier(&id)?;
            if !seen.insert((kind.clone(), id.clone())) {
                continue;
            }
            if seen.len() > 10_000 {
                return Err(Error::Conflict(
                    "typed content closure exceeds bound".into(),
                ));
            }
            let object = source.actual.object(ctx.namespace(), &kind, &id)?;
            let size = serde_json::to_vec(&object.body)
                .map_err(crate::internal)?
                .len();
            bytes = bytes.checked_add(size).ok_or(Error::Forbidden)?;
            if size > 4 * 1024 * 1024 || bytes > 64 * 1024 * 1024 {
                return Err(Error::Conflict(
                    "typed dependency exceeds byte bound".into(),
                ));
            }
            if matches!(
                object.body["schema_version"].as_str(),
                Some(
                    fv::PERSISTENT_RELEASE_V2
                        | fv::FORMAL_AUTHORITY_V3
                        | "rsia.independent_grade_capture.v2"
                        | "rsia.release_action.v2"
                )
            ) || source.actual.native_consumer_origins_r49.contains_key(&(
                ctx.namespace().into(),
                kind.clone(),
                id.clone(),
            )) {
                return Ok(true);
            }
            for (target_kind, target_id) in source.dependency_edges(ctx, &kind, &id)? {
                edge_count = edge_count.checked_add(1).ok_or(Error::Forbidden)?;
                if edge_count > 100_000 {
                    return Err(Error::Conflict("typed closure exceeds edge bound".into()));
                }
                pending.push((target_kind, target_id));
            }
        }
        Ok(false)
    }
    fn qualify_native_compiler_node_r49(
        source: &mut SourceR49<'_>,
        ctx: &Context,
        kind: &str,
        id: &str,
    ) -> Result<()> {
        let body: Value = source.need(ctx, kind, id)?;
        let schema = body["schema_version"].as_str().unwrap_or("");
        match (kind, schema) {
            ("release", fv::PERSISTENT_RELEASE_V2) => {
                let release: fv::PersistentReleaseV2<MirrorPersistentRelease> = decode(body)?;
                revalidate_release_r49(source, ctx, &release)?;
            }
            ("artifact", fv::FORMAL_AUTHORITY_V3) => {
                let authority: fv::FormalAuthorityV3<IssuedFormalAuthority> = decode(body)?;
                authority.validate_known().map_err(r24_fault)?;
                source.origin_for_bound_r49(ctx, kind, id, Some(&authority.report_ref))?;
                source.reference = Some(authority.report_ref.clone());
                let current = current_data_r49(source)?;
                let actual = verify_issued_authority_r49(source, ctx, &current)?;
                if fingerprint(&actual)? != fingerprint(&authority)? {
                    return Err(Error::Forbidden);
                }
            }
            ("artifact", "rsia.independent_grade_capture.v2") => {
                let capture: IndependentGradeCapture = decode(body.clone())?;
                let proof = source.origin_for_bound_r49(ctx, kind, id, None)?;
                require_content_witness(source, ctx, kind, id, &body, Role::Evaluator)?;
                let host: RegisteredFormalHost = attested_read(
                    source,
                    ctx,
                    "artifact",
                    &capture.host_registration_id,
                    Role::Admin,
                )?;
                let set = principal_set(source, ctx, &host)?;
                let object = source.actual.object(ctx.namespace(), kind, id)?;
                if object.owner != set.grader.actor
                    || proof.grader_actor != set.grader.actor
                    || capture.grader_principal != host.grader_principal
                    || capture.grader_uid != host.grader_uid
                    || capture.grader_pid != proof.grader_pid
                    || capture.grader_uid != proof.grader_uid
                    || capture.grader_code_digest != host.grader_code_digest
                    || capture.grader_code_digest != proof.grader_code_digest
                    || capture.grader_workspace != host.grader_workspace
                    || capture.grader_workspace != proof.grader_workspace
                {
                    return Err(Error::Forbidden);
                }
                let mut edges = source.dependency_edges(ctx, kind, id)?;
                edges.sort();
                edges.dedup();
                let host_edge = ("artifact".to_owned(), host.id.clone());
                if edges.len() != 2 || !edges.contains(&host_edge) {
                    return Err(Error::Forbidden);
                }
                let receipt_edge = edges
                    .iter()
                    .find(|edge| *edge != &host_edge)
                    .ok_or(Error::Forbidden)?;
                if receipt_edge.0 != "artifact" {
                    return Err(Error::Forbidden);
                }
                let receipt: Value = source.need(ctx, &receipt_edge.0, &receipt_edge.1)?;
                if receipt["schema_version"] != "rsia.typed_artifact_envelope.v1"
                    || receipt["record_kind"] != "independent_grader_receipt_v2"
                    || receipt["payload"]["ticket_id"] != capture.ticket_id
                    || receipt["payload"]["target_id"] != capture.target_id
                    || content_digest(&receipt["payload"])? != capture.grader_receipt_digest
                {
                    return Err(Error::Forbidden);
                }
            }
            ("artifact", "rsia.release_action.v2") => {
                let record: ReleaseActionR49 = decode(body.clone())?;
                validate_release_action_r49(&record)?;
                require_content_witness(source, ctx, kind, id, &body, Role::Evaluator)?;
                // Validate the real complete per-subject historical origin chain,
                // including previous action IDs. An old action remains readable
                // after a healthy later transition changes the mutable subject.
                source
                    .latest_origin_r49(ctx, &record.action.subject_kind, &record.action.subject_id)?
                    .ok_or_else(|| r24_fault(fv::Fault::SourceInsufficient))?;
                let mut matched = None;
                for row in source
                    .raw
                    .tables
                    .get("audit")
                    .ok_or(Error::ClockRegressed)?
                {
                    if native_text(row, "namespace")? != ctx.namespace() {
                        continue;
                    }
                    let audit: AuditPayloadR24 =
                        serde_json::from_str(&native_text(row, "payload")?)
                            .map_err(crate::internal)?;
                    if audit.operation != NATIVE_RECORD_OPERATION_R49 {
                        continue;
                    }
                    let proof: NativeRecordOriginR49 =
                        serde_json::from_str(&audit.entity).map_err(crate::internal)?;
                    if proof.action_id.as_deref() != Some(id) {
                        continue;
                    }
                    if matched.is_some() {
                        return Err(Error::Forbidden);
                    }
                    matched = Some(proof);
                }
                let proof = matched.ok_or_else(|| r24_fault(fv::Fault::SourceInsufficient))?;
                let host: RegisteredFormalHost = attested_read(
                    source,
                    ctx,
                    "artifact",
                    &record.action.host_registration_id,
                    Role::Admin,
                )?;
                let set = principal_set(source, ctx, &host)?;
                let admin = set.principal(Role::Admin)?;
                let object = source.actual.object(ctx.namespace(), kind, id)?;
                let action = &record.action;
                if object.owner != set.grader.actor
                    || action.namespace != ctx.namespace()
                    || action.role != Role::Admin
                    || action.actor != admin.actor
                    || action.uid != admin.uid
                    || action.peer_uid != admin.uid
                    || action.principal != admin.principal
                    || action.credential_digest != admin.credential_digest
                    || action.host_digest != fingerprint(&host)?
                    || action.principal_set_digest != set.binding_digest()?
                    || action.authority_pid != proof.grader_pid
                    || action.authority_uid != proof.grader_uid
                    || action.authority_executable_digest != proof.grader_code_digest
                    || proof.grader_actor != set.grader.actor
                    || proof.grader_uid != host.grader_uid
                    || proof.grader_code_digest != host.grader_code_digest
                    || proof.grader_workspace != host.grader_workspace
                    || proof.namespace != ctx.namespace()
                    || proof.kind != action.subject_kind
                    || proof.id != action.subject_id
                    || proof.body_content_digest != action.subject_content_digest
                    || proof.request_digest.as_deref()
                        != Some(action.authenticated_request_digest.as_str())
                    || proof.peer_uid != Some(action.peer_uid)
                    || proof.peer_gid != action.peer_gid
                    || proof.peer_pid != action.peer_pid
                {
                    return Err(Error::Forbidden);
                }
                let reference = proof.report_ref.as_ref().ok_or(Error::Forbidden)?;
                reference.validate_wire().map_err(r24_fault)?;
                if reference.namespace != ctx.namespace() {
                    return Err(Error::Forbidden);
                }
                if action.operation == "approve_current_v2" {
                    let command: ApprovalCommandR49 = decode(record.command.clone())?;
                    if command.report_ref != *reference {
                        return Err(Error::Forbidden);
                    }
                }
            }
            ("formal_parent", _) | ("pointer", _) => {
                if source.actual.native_consumer_origins_r49.contains_key(&(
                    ctx.namespace().into(),
                    kind.into(),
                    id.into(),
                )) {
                    let object = source.actual.object(ctx.namespace(), kind, id)?;
                    if !qualify_native_admin_r49(&source.actual, object)? {
                        return Err(Error::Forbidden);
                    }
                    require_registered_body_authority(source, ctx, kind, id, &body)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    pub(crate) async fn inspect_native_typed_closure_r49(
        session: &mut crate::Session,
        ctx: &Context,
        roots: Vec<(String, String)>,
    ) -> Result<Option<NativeCompilerClosureDataR49>> {
        if roots.len() > 10_000 {
            return Err(Error::Conflict(
                "typed content root count exceeds bound".into(),
            ));
        }
        session.clock.check()?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        require(
            tx.open
                && tx.is_in_transaction()
                && SqliteTransactionManager::get_transaction_depth(&**tx) == 1,
        )?;
        let raw = load_sqlx_database(&mut **tx).await?;
        collect_raw(&raw, &session.prepare_inputs, SchemaRead::Stable)?;
        session.clock.check()?;
        let mut source = SourceR49::captured(&raw, ctx, None)?;
        if !compiler_has_native_chain_r49(&mut source, ctx, &roots)? {
            return Ok(None);
        }
        let closure = read_typed_closure(&mut source, ctx, &roots)?;
        for node in &closure.nodes {
            qualify_native_compiler_node_r49(&mut source, ctx, &node.kind, &node.id)?;
        }
        Ok(Some(NativeCompilerClosureDataR49 {
            closure: serde_json::to_value(closure).map_err(crate::internal)?,
        }))
    }
    /// Same-transaction compiler data. This is not a transferable write or
    /// Current capability and has no caller constructor or wire decoder.
    pub struct NativeCompilerInputsDataR49 {
        profile: Value,
        parent_skill: Value,
        parent_strategy: Value,
        search_parent_strategy: Value,
        binding: Value,
    }
    impl NativeCompilerInputsDataR49 {
        pub fn profile(&self) -> &Value {
            &self.profile
        }
        pub fn parent_skill(&self) -> &Value {
            &self.parent_skill
        }
        pub fn parent_strategy(&self) -> &Value {
            &self.parent_strategy
        }
        pub fn search_parent_strategy(&self) -> &Value {
            &self.search_parent_strategy
        }
        pub fn binding(&self) -> &Value {
            &self.binding
        }
        fn from_actual(value: VerifiedCompilerInputs) -> Result<Self> {
            Ok(Self {
                profile: serde_json::to_value(value.profile).map_err(crate::internal)?,
                parent_skill: serde_json::to_value(value.parent_skill).map_err(crate::internal)?,
                parent_strategy: serde_json::to_value(value.parent_strategy)
                    .map_err(crate::internal)?,
                search_parent_strategy: serde_json::to_value(value.search_parent_strategy)
                    .map_err(crate::internal)?,
                binding: serde_json::to_value(value.binding).map_err(crate::internal)?,
            })
        }
    }
    async fn captured_compiler_source_r49(
        session: &mut crate::Session,
        ctx: &Context,
    ) -> Result<SourceR49<'static>> {
        ctx.require(&[Role::Worker, Role::Host, Role::Admin, Role::Evaluator])?;
        session.clock.check()?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        require(
            tx.open
                && tx.is_in_transaction()
                && SqliteTransactionManager::get_transaction_depth(&**tx) == 1,
        )?;
        let raw = load_sqlx_database(&mut **tx).await?;
        collect_raw(&raw, &session.prepare_inputs, SchemaRead::Stable)?;
        session.clock.check()?;
        SourceR49::captured(&raw, ctx, None)
    }
    fn native_parent_reference_r49(
        source: &mut SourceR49<'_>,
        ctx: &Context,
        parent_id: &str,
    ) -> Result<Option<fv::ReportRef>> {
        let Some(body) = source.get::<Value>(ctx, "formal_parent", parent_id)? else {
            return Ok(None);
        };
        let selection: ParentSelection = decode(body)?;
        if selection.schema_version != "rsia.formal_parent_selection.v1"
            || selection.id != parent_id
        {
            return Err(Error::Forbidden);
        }
        let ParentAuthority::ApprovedRelease { release_id } = selection.authority else {
            return Ok(None);
        };
        let release: Value = source.need(ctx, "release", &release_id)?;
        if release["schema_version"] == "rsia.persistent_release.v1" {
            return Err(Error::Conflict(
                "legacy release cannot authorize new compiler inputs".into(),
            ));
        }
        if release["schema_version"] != fv::PERSISTENT_RELEASE_V2 {
            return Err(Error::Conflict(
                "unknown approved parent release version; quarantine".into(),
            ));
        }
        let release: fv::PersistentReleaseV2<MirrorPersistentRelease> = decode(release)?;
        release.validate_known().map_err(r24_fault)?;
        Ok(Some(release.report_ref))
    }
    pub(crate) async fn inspect_native_compiler_inputs_r49(
        session: &mut crate::Session,
        ctx: &Context,
        request: Value,
    ) -> Result<Option<NativeCompilerInputsDataR49>> {
        ctx.require(&[Role::Worker, Role::Host, Role::Admin, Role::Evaluator])?;
        let request: CompilerInputRequest = decode(request)?;
        let mut source = captured_compiler_source_r49(session, ctx).await?;
        let Some(reference) =
            native_parent_reference_r49(&mut source, ctx, &request.profile.parent_digest)?
        else {
            return Ok(None);
        };
        source.reference = Some(reference);
        NativeCompilerInputsDataR49::from_actual(resolve_compiler_inputs(
            &mut source,
            ctx,
            &request,
        )?)
        .map(Some)
    }
    pub(crate) async fn inspect_native_compiler_bundle_r49(
        session: &mut crate::Session,
        ctx: &Context,
        bundle: Value,
        environment: &str,
        watermark: u64,
        purpose: Purpose,
    ) -> Result<Option<NativeCompilerInputsDataR49>> {
        ctx.require(&[Role::Worker, Role::Host, Role::Admin, Role::Evaluator])?;
        let bundle: ResolvedBundle = decode(bundle)?;
        let mut source = captured_compiler_source_r49(session, ctx).await?;
        let Some(reference) = native_parent_reference_r49(&mut source, ctx, &bundle.parent_digest)?
        else {
            return Ok(None);
        };
        source.reference = Some(reference);
        NativeCompilerInputsDataR49::from_actual(resolve_bundle_in_session(
            &mut source,
            ctx,
            &bundle,
            environment,
            watermark,
            purpose,
        )?)
        .map(Some)
    }
    fn verify_parent_release_r49(
        source: &mut SourceR49<'_>,
        ctx: &Context,
        release: &MirrorPersistentRelease,
        candidate: &MirrorReleaseCandidateRecord,
    ) -> Result<()> {
        let outer: Value = source.need(ctx, "release", &release.id)?;
        let wrapper: evo_core::formal_version::PersistentReleaseV2<MirrorPersistentRelease> =
            decode(outer.clone())?;
        wrapper.validate_known().map_err(r24_fault)?;
        if wrapper.release.candidate_id != candidate.id
            || wrapper.release.bundle_digest != candidate.bundle_digest
            || wrapper.release.approved_by != release.approved_by
        {
            return Err(Error::Conflict("approved parent identity differs".into()));
        }
        source.origin_for_bound_r49(ctx, "release", &release.id, Some(&wrapper.report_ref))?;
        let mut parent = SourceR49::captured(&source.raw, ctx, Some(&wrapper.report_ref))?;
        let current = current_data_r49(&parent)?;
        let authority = verify_issued_authority_r49(&mut parent, ctx, &current)?;
        if authority.id != wrapper.authority_id {
            return Err(Error::Forbidden);
        }
        verify_release_principal(&mut parent, ctx, release, candidate)
    }

    fn copy_host_error_r49(error: &Error) -> Error {
        match error {
            Error::Conflict(code) => Error::Conflict(code.clone()),
            Error::Invalid(code) => Error::Invalid(code.clone()),
            Error::Forbidden => Error::Forbidden,
            Error::NotFound => Error::NotFound,
            Error::ClockRegressed => Error::ClockRegressed,
            Error::Budget => Error::Budget,
            Error::Cancelled => Error::Cancelled,
            _ => Error::Internal,
        }
    }

    /// Captured kernel peer and original request only. This is not an operator
    /// permission: the actual private native owner authenticates the credential
    /// against its current Host/principal/role/namespace/command snapshot.
    pub struct NativeOperatorCredentialR49 {
        peer: tokio::net::unix::UCred,
        credential: Vec<u8>,
        command: ApprovalCommandR49,
        request_digest: String,
    }
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ApprovalCommandR49 {
        operation: String,
        candidate_id: String,
        report_ref: evo_core::formal_version::ReportRef,
    }
    impl NativeOperatorCredentialR49 {
        pub fn from_socket(
            stream: &tokio::net::UnixStream,
            credential: &str,
            command: Value,
        ) -> Result<Self> {
            let command: ApprovalCommandR49 =
                serde_json::from_value(command).map_err(|_| Error::Forbidden)?;
            if command.operation != "approve_current_v2"
                || credential.len() < 32
                || credential.len() > 4096
            {
                return Err(Error::Forbidden);
            }
            command.report_ref.validate_wire().map_err(r24_fault)?;
            identifier(&command.candidate_id)?;
            let peer = stream.peer_cred().map_err(|_| Error::Forbidden)?;
            peer.pid()
                .map(u32::try_from)
                .transpose()
                .map_err(|_| Error::Forbidden)?;
            let request_digest = fingerprint(&command)?;
            Ok(Self {
                peer,
                credential: credential.as_bytes().to_vec(),
                command,
                request_digest,
            })
        }
    }
    impl Drop for NativeOperatorCredentialR49 {
        fn drop(&mut self) {
            self.credential.fill(0);
        }
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
    enum ReleaseCommandR49 {
        CanaryCurrentV2 {
            release_id: String,
        },
        ActivateCurrentV2 {
            release_id: String,
            expected_pointer_epoch: u64,
        },
        RollbackCurrentV2 {
            profile_id: String,
            target_release_id: String,
            expected_pointer_epoch: u64,
            environment_digest: String,
        },
    }
    pub struct NativeReleaseCredentialR49 {
        peer: tokio::net::unix::UCred,
        credential: Vec<u8>,
        command: ReleaseCommandR49,
        request_digest: String,
    }
    impl NativeReleaseCredentialR49 {
        pub fn from_socket(
            stream: &tokio::net::UnixStream,
            credential: &str,
            command: Value,
        ) -> Result<Self> {
            let command: ReleaseCommandR49 = decode(command)?;
            if !(32..=4096).contains(&credential.len()) {
                return Err(Error::Forbidden);
            }
            match &command {
                ReleaseCommandR49::CanaryCurrentV2 { release_id }
                | ReleaseCommandR49::ActivateCurrentV2 { release_id, .. } => {
                    identifier(release_id)?
                }
                ReleaseCommandR49::RollbackCurrentV2 {
                    profile_id,
                    target_release_id,
                    environment_digest,
                    ..
                } => {
                    identifier(profile_id)?;
                    identifier(target_release_id)?;
                    digest(environment_digest)?;
                }
            }
            let peer = stream.peer_cred().map_err(|_| Error::Forbidden)?;
            peer.pid()
                .map(u32::try_from)
                .transpose()
                .map_err(|_| Error::Forbidden)?;
            let request_digest = fingerprint(&command)?;
            Ok(Self {
                peer,
                credential: credential.as_bytes().to_vec(),
                command,
                request_digest,
            })
        }
    }
    impl Drop for NativeReleaseCredentialR49 {
        fn drop(&mut self) {
            self.credential.fill(0);
        }
    }
    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ReleaseActionR49 {
        schema_version: String,
        id: String,
        previous_action_id: Option<String>,
        command: Value,
        action: PrincipalAction,
    }
    fn release_action_id_r49(
        previous: Option<&str>,
        command: &Value,
        action: &PrincipalAction,
    ) -> Result<String> {
        Ok(format!(
            "release-action-v2-{}",
            &fingerprint(&(
                previous,
                command,
                &action.namespace,
                &action.subject_kind,
                &action.subject_id,
                &action.subject_content_digest,
                &action.authenticated_request_digest
            ))?[..40]
        ))
    }
    fn validate_release_action_r49(record: &ReleaseActionR49) -> Result<()> {
        if record.schema_version != "rsia.release_action.v2"
            || record.action.schema_version != PRINCIPAL_ACTION_SCHEMA
            || record.id != record.action.id
            || record.id
                != release_action_id_r49(
                    record.previous_action_id.as_deref(),
                    &record.command,
                    &record.action,
                )?
            || record.action.authenticated_request_digest != fingerprint(&record.command)?
        {
            return Err(Error::Forbidden);
        }
        if record.action.operation == "approve_current_v2" {
            let command: ApprovalCommandR49 = decode(record.command.clone())?;
            if command.operation != record.action.operation {
                return Err(Error::Forbidden);
            }
            command.report_ref.validate_wire().map_err(r24_fault)?;
            identifier(&command.candidate_id)?;
        } else {
            let command: ReleaseCommandR49 = decode(record.command.clone())?;
            let operation = match command {
                ReleaseCommandR49::CanaryCurrentV2 { .. } => "canary_current_v2",
                ReleaseCommandR49::ActivateCurrentV2 { .. } => "activate_current_v2",
                ReleaseCommandR49::RollbackCurrentV2 { .. } => "rollback_current_v2",
            };
            if record.action.operation != operation {
                return Err(Error::Forbidden);
            }
        }
        Ok(())
    }
    pub(crate) fn begin_release_transition_r49(
        session: Session,
        ctx: Context,
        credential: NativeReleaseCredentialR49,
    ) -> Result<E05IssueOperation> {
        ctx.require(&[Role::Admin])?;
        begin_consumer_r49(session, ctx, ConsumerIntentR49::Release(credential))
    }

    pub(super) enum ConsumerIntentR49 {
        GradeCapture {
            ticket_id: String,
            target_id: String,
            receipt_id: String,
        },
        IssueAuthority {
            reference: evo_core::formal_version::ReportRef,
        },
        Approve(NativeOperatorCredentialR49),
        Release(NativeReleaseCredentialR49),
    }
    pub(crate) fn begin_grade_capture_r49(
        session: crate::Session,
        ctx: Context,
        ticket_id: String,
        target_id: String,
        receipt_id: String,
    ) -> Result<E05IssueOperation> {
        ctx.require(&[Role::Evaluator])?;
        for id in [&ticket_id, &target_id, &receipt_id] {
            identifier(id)?;
        }
        begin_consumer_r49(
            session,
            ctx,
            ConsumerIntentR49::GradeCapture {
                ticket_id,
                target_id,
                receipt_id,
            },
        )
    }
    pub(crate) fn begin_authority_r49(
        session: crate::Session,
        ctx: Context,
        reference: evo_core::formal_version::ReportRef,
    ) -> Result<E05IssueOperation> {
        ctx.require(&[Role::Evaluator])?;
        reference.validate_wire().map_err(r24_fault)?;
        if reference.namespace != ctx.namespace() || reference.owner != ctx.actor() {
            return Err(Error::Forbidden);
        }
        begin_consumer_r49(
            session,
            ctx,
            ConsumerIntentR49::IssueAuthority { reference },
        )
    }
    pub(crate) fn begin_approval_r49(
        session: crate::Session,
        ctx: Context,
        credential: NativeOperatorCredentialR49,
    ) -> Result<E05IssueOperation> {
        ctx.require(&[Role::Admin])?;
        if credential.command.report_ref.namespace != ctx.namespace() {
            return Err(Error::Forbidden);
        }
        begin_consumer_r49(session, ctx, ConsumerIntentR49::Approve(credential))
    }
    fn begin_consumer_r49(
        session: crate::Session,
        ctx: Context,
        intent: ConsumerIntentR49,
    ) -> Result<E05IssueOperation> {
        session.clock.check()?;
        let tx = session.tx.as_ref().ok_or(Error::ClockRegressed)?;
        require(
            tx.open
                && tx.is_in_transaction()
                && SqliteTransactionManager::get_transaction_depth(&**tx) == 1,
        )?;
        Ok(begin_native(
            session,
            ctx,
            NativeIntent::ConsumerR49(Box::new(intent)),
            Arc::new(AtomicU8::new(ACTIVE)),
        ))
    }
    pub(super) async fn execute_r49(
        session: &mut crate::Session,
        ctx: &Context,
        intent: ConsumerIntentR49,
        gate: Arc<AtomicU8>,
    ) -> Result<Value> {
        let reference = match &intent {
            ConsumerIntentR49::GradeCapture { .. } | ConsumerIntentR49::Release(_) => None,
            ConsumerIntentR49::IssueAuthority { reference } => Some(reference.clone()),
            ConsumerIntentR49::Approve(credential) => Some(credential.command.report_ref.clone()),
        };
        let mut source = SourceR49::owned(session, ctx, reference.as_ref(), gate).await?;
        let result = match intent {
            ConsumerIntentR49::GradeCapture {
                ticket_id,
                target_id,
                receipt_id,
            } => {
                let ticket: MirrorEvaluationTicketV2 =
                    evaluation_record(&source.actual, ctx.namespace(), TICKET_KIND, &ticket_id)?;
                ticket.validate()?;
                if ticket.evaluator_actor != ctx.actor() {
                    return Err(Error::Forbidden);
                }
                if ticket.evidence_scope == MirrorEvaluationEvidenceScope::ProgramFixture {
                    Value::Null
                } else {
                    let receipt: MirrorIndependentGraderReceiptV2 = evaluation_record(
                        &source.actual,
                        ctx.namespace(),
                        "independent_grader_receipt_v2",
                        &receipt_id,
                    )?;
                    if receipt.ticket_id != ticket.id
                        || receipt.target_id != target_id
                        || receipt.evaluator_actor != ctx.actor()
                    {
                        return Err(Error::Conflict(
                            "native grader capture receipt identity differs".into(),
                        ));
                    }
                    let value = serde_json::to_value(&receipt).map_err(crate::internal)?;
                    let digest = content_digest(&value)?;
                    capture_independent_grade(
                        &mut source,
                        ctx,
                        &ticket,
                        &target_id,
                        &receipt_id,
                        &digest,
                        None,
                    )
                    .await?;
                    Value::Null
                }
            }
            ConsumerIntentR49::IssueAuthority { reference: _ } => {
                let current = current_data_r49(&source)?;
                let ticket = &current.carrier.input.ticket_snapshot;
                if !matches!(
                    &current.carrier.payload,
                    MirrorFormalEvaluationV2::CompleteBatch { .. }
                ) {
                    return Err(Error::Forbidden);
                }
                if ticket.evidence_scope == MirrorEvaluationEvidenceScope::ProgramFixture {
                    Value::Null
                } else {
                    let gaps = inspect_known_host_sources_r49(
                        &source.raw,
                        &source.actual,
                        ctx,
                        &current.reference,
                        &current.carrier,
                    )?;
                    if !gaps.is_empty() {
                        return Err(r24_fault(
                            evo_core::formal_version::Fault::SourceInsufficient,
                        ));
                    }
                    let control = evaluation_control(
                        &source.actual,
                        ctx.namespace(),
                        &ticket.registration_id,
                    )?;
                    let value = issue_complete_authority_in_session(
                        &mut source,
                        ctx,
                        ticket,
                        &control,
                        &current.carrier.payload,
                    )
                    .await?;
                    serde_json::to_value(value).map_err(crate::internal)?
                }
            }
            ConsumerIntentR49::Release(credential) => {
                transition_release_r49(&mut source, ctx, &credential).await?
            }
            ConsumerIntentR49::Approve(credential) => {
                approve_current_r49(&mut source, ctx, &credential).await?
            }
        };
        source.stable().await?;
        Ok(result)
    }
    async fn record_approval_action_r49(
        source: &mut SourceR49<'_>,
        ctx: &Context,
        host: &RegisteredFormalHost,
        set: &RegisteredPrincipalSet,
        credential: &NativeOperatorCredentialR49,
        kind: &str,
        id: &str,
        body: &Value,
    ) -> Result<()> {
        let administrator = set.principal(Role::Admin)?;
        if ctx.role() != Role::Admin
            || ctx.actor() != administrator.actor
            || credential.peer.uid() != administrator.uid
            || hash(&credential.credential) != administrator.credential_digest
            || credential.request_digest != fingerprint(&credential.command)?
            || credential.command.report_ref.namespace != ctx.namespace()
        {
            return Err(Error::Forbidden);
        }
        let grader = Context::new(ctx.namespace(), &set.grader.actor, Role::Evaluator)?;
        let observed = process_principal(&grader, host, set, Role::Evaluator)?;
        let mut action = PrincipalAction {
            id: principal_action_id(kind, id, Role::Admin)?,
            schema_version: PRINCIPAL_ACTION_SCHEMA.into(),
            namespace: ctx.namespace().into(),
            host_registration_id: host.id.clone(),
            host_digest: fingerprint(host)?,
            principal_set_digest: set.binding_digest()?,
            role: Role::Admin,
            actor: administrator.actor.clone(),
            uid: administrator.uid,
            principal: administrator.principal.clone(),
            credential_digest: administrator.credential_digest.clone(),
            subject_kind: kind.into(),
            subject_id: id.into(),
            subject_content_digest: content_digest(body)?,
            operation: "approve_current_v2".into(),
            authenticated_request_digest: credential.request_digest.clone(),
            peer_uid: credential.peer.uid(),
            peer_gid: Some(credential.peer.gid()),
            peer_pid: credential
                .peer
                .pid()
                .map(u32::try_from)
                .transpose()
                .map_err(|_| Error::Forbidden)?,
            authority_pid: observed.pid(),
            authority_uid: observed.uid(),
            authority_executable_digest: observed.executable_digest().to_owned(),
        };
        let command = serde_json::to_value(&credential.command).map_err(crate::internal)?;
        let previous = source
            .latest_origin_r49(ctx, kind, id)?
            .and_then(|proof| proof.action_id);
        action.id = release_action_id_r49(previous.as_deref(), &command, &action)?;
        let record = ReleaseActionR49 {
            schema_version: "rsia.release_action.v2".into(),
            id: action.id.clone(),
            previous_action_id: previous,
            command,
            action,
        };
        validate_release_action_r49(&record)?;
        immutable(source, &grader, "artifact", &record.id, &record).await?;
        attest_content(source, &grader, "artifact", &record.id, &record).await?;
        source
            .put_edge(&grader, "artifact", &record.id, "artifact", &set.id)
            .await?;
        source
            .put_edge(&grader, kind, id, "artifact", &record.id)
            .await?;
        source
            .origin(
                &grader,
                kind,
                id,
                &observed,
                Some((&credential.peer, &credential.request_digest)),
            )
            .await
    }
    async fn approve_current_r49(
        source: &mut SourceR49<'_>,
        ctx: &Context,
        credential: &NativeOperatorCredentialR49,
    ) -> Result<Value> {
        ctx.require(&[Role::Admin])?;
        let current = current_data_r49(source)?;
        if current.reference != credential.command.report_ref {
            return Err(Error::Forbidden);
        }
        let ticket = &current.carrier.input.ticket_snapshot;
        let authority = verify_issued_authority_r49(source, ctx, &current)?;
        let candidate: MirrorReleaseCandidateRecord =
            source.need(ctx, "artifact", &credential.command.candidate_id)?;
        let control = evaluation_control(&source.actual, ctx.namespace(), &ticket.registration_id)?;
        let host: RegisteredFormalHost = attested_read(
            source,
            ctx,
            "artifact",
            &authority.authority.host_registration_id,
            Role::Admin,
        )?;
        let set = verify_candidate_principals(source, ctx, &host.id, &candidate, &control)?;
        let worker = require_principal_action(
            source,
            ctx,
            &host,
            "artifact",
            &candidate.id,
            &candidate,
            Role::Worker,
        )?;
        let administrator = set.principal(Role::Admin)?;
        if ctx.actor() != administrator.actor
            || credential.peer.uid() != administrator.uid
            || hash(&credential.credential) != administrator.credential_digest
            || ctx.actor() == worker.actor
            || ctx.actor() == ticket.evaluator_actor
            || ticket.approver_actor != ctx.actor()
            || candidate.proposer_actor != worker.actor
            || candidate.id != ticket.candidate_digest
            || candidate.bundle_digest != ticket.candidate_bundle_digest
            || candidate.parent_digest != ticket.baseline_digest
            || candidate.environment_digest != ticket.environment_digest
            || candidate.revoke_watermark
                != source
                    .actual
                    .watermarks
                    .get(ctx.namespace())
                    .ok_or(Error::Forbidden)?
                    .0
        {
            return Err(Error::Forbidden);
        }
        let MirrorFormalEvaluationV2::CompleteBatch { verdict, .. } = &current.carrier.payload
        else {
            return Err(Error::Forbidden);
        };
        if !matches!(
            verdict,
            evo_core::evaluation::Verdict::Improved | evo_core::evaluation::Verdict::Noninferior
        ) {
            return Err(Error::Forbidden);
        }
        let pointer_id = format!("profile-pointer-{}", candidate.profile_id);
        let pointer: Option<MirrorPersistentProfilePointer> =
            source.get(ctx, "pointer", &pointer_id)?;
        if let Some(pointer) = &pointer {
            let body: Value = source.need(ctx, "pointer", &pointer_id)?;
            require_content_witness(source, ctx, "pointer", &pointer_id, &body, Role::Admin)?;
            require_registered_body_authority(source, ctx, "pointer", &pointer_id, &body)?;
            if pointer.schema_version != "rsia.profile_pointer.v1"
                || pointer.profile_id != candidate.profile_id
                || pointer.id != pointer_id
            {
                return Err(Error::Conflict("approval pointer identity differs".into()));
            }
        }
        let approval_input_digest = fingerprint(&(
            current.reference.clone(),
            &candidate,
            &authority.id,
            &pointer,
        ))?;
        let id = evo_core::formal_version::persistent_release_id_v2(
            &current.reference,
            &authority.id,
            &approval_input_digest,
        )
        .map_err(r24_fault)?;
        let report_digest = fingerprint(&(&current.carrier.payload, &authority.authority))?;
        let release = MirrorPersistentRelease {
            id: id.clone(),
            schema_version: "rsia.persistent_release.v1".into(),
            candidate_id: candidate.id.clone(),
            bundle_digest: candidate.bundle_digest.clone(),
            environment_digest: candidate.environment_digest.clone(),
            profile_id: candidate.profile_id.clone(),
            parent_digest: candidate.parent_digest.clone(),
            baseline_bundle_digest: ticket.baseline_bundle_digest.clone(),
            report_id: ticket.id.clone(),
            report_digest,
            proposer_actor: candidate.proposer_actor.clone(),
            evaluator_actor: ticket.evaluator_actor.clone(),
            approved_by: ctx.actor().into(),
            expected_parent_release_id: pointer
                .as_ref()
                .and_then(|pointer| pointer.active_release_id.clone()),
            expected_pointer_epoch: pointer.as_ref().map(|pointer| pointer.epoch).unwrap_or(0),
            state: MirrorPersistentReleaseState::Approved,
        };
        let wrapper = evo_core::formal_version::PersistentReleaseV2 {
            schema_version: evo_core::formal_version::PERSISTENT_RELEASE_V2.into(),
            id: id.clone(),
            report_ref: current.reference.clone(),
            ticket_id: ticket.id.clone(),
            authority_id: authority.id.clone(),
            approval_input_digest,
            release_digest: fingerprint(&release)?,
            release,
        };
        wrapper.validate_known().map_err(r24_fault)?;
        let value = serde_json::to_value(&wrapper).map_err(crate::internal)?;
        if let Some(existing) = source.get::<Value>(ctx, "release", &id)? {
            let existing: evo_core::formal_version::PersistentReleaseV2<MirrorPersistentRelease> =
                decode(existing)?;
            existing.validate_known().map_err(r24_fault)?;
            if fingerprint(&existing)? != fingerprint(&wrapper)? {
                return Err(Error::Conflict("release approval identity reused".into()));
            }
            source.origin_for(ctx, "release", &id)?;
            return serde_json::to_value(existing).map_err(crate::internal);
        }
        immutable(source, ctx, "release", &id, &wrapper).await?;
        attest_content(source, ctx, "release", &id, &wrapper).await?;
        record_approval_action_r49(source, ctx, &host, &set, credential, "release", &id, &value)
            .await?;
        for (kind, dependency) in [
            ("artifact", candidate.id.clone()),
            ("artifact", current.reference.physical_id.clone()),
            ("artifact", authority.id.clone()),
        ] {
            source
                .put_edge(ctx, "release", &id, kind, &dependency)
                .await?;
        }
        let parent = ParentSelection {
            id: candidate.bundle_digest.clone(),
            schema_version: "rsia.formal_parent_selection.v1".into(),
            authority: ParentAuthority::ApprovedRelease {
                release_id: id.clone(),
            },
        };
        immutable(source, ctx, "formal_parent", &parent.id, &parent).await?;
        attest_content(source, ctx, "formal_parent", &parent.id, &parent).await?;
        let parent_body = serde_json::to_value(&parent).map_err(crate::internal)?;
        record_approval_action_r49(
            source,
            ctx,
            &host,
            &set,
            credential,
            "formal_parent",
            &parent.id,
            &parent_body,
        )
        .await?;
        source
            .put_edge(ctx, "formal_parent", &parent.id, "release", &id)
            .await?;
        Ok(value)
    }
}

impl NativeIssueOwner {
    async fn run_consumer_r49(
        &mut self,
        ctx: Context,
        intent: native_formal_consumers_r49::ConsumerIntentR49,
    ) -> Result<Value> {
        check_cancel(&self.gate)?;
        let session = self.session.as_mut().ok_or(Error::Internal)?;
        let value =
            native_formal_consumers_r49::execute_r49(session, &ctx, intent, Arc::clone(&self.gate))
                .await?;
        check_cancel(&self.gate)?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        require(
            tx.is_in_transaction() && SqliteTransactionManager::get_transaction_depth(&**tx) == 1,
        )?;
        admit_commit(&self.gate)?;
        self.facts.fetch_or(COMMIT_ATTEMPTED, Ordering::AcqRel);
        SqliteTransactionManager::commit(&mut **tx)
            .await
            .map_err(crate::internal)?;
        self.facts.fetch_or(COMMIT_ACK, Ordering::AcqRel);
        tx.open = false;
        require(SqliteTransactionManager::get_transaction_depth(&**tx) == 0)?;
        tx.ping().await.map_err(crate::internal)?;
        self.terminal_confirmed = true;
        self.facts.fetch_or(TERMINAL_ACK, Ordering::AcqRel);
        Ok(value)
    }
}

mod fixed_formal_read_r38 {
    use super::*;
    use evo_core::formal_version as fv;

    #[derive(Debug, Clone)]
    pub enum FormalReadSelectorR38 {
        Ticket(String),
        Reference(fv::AnyReportRef),
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum FormalSourceClassR38 {
        StaticLegacy,
        // Not a V104 read state and never an authority/eligibility claim.
        // A healthy input-v2 source reaches here without being mislabeled legacy
        // or refused merely because the read factory is not implemented yet.
        FrozenVerificationPending,
        /// Native content qualified in this actual capture and released only
        /// after close ACK. No write/Host/approval permission is granted.
        NativeCompleteAfterClose,
        /// Verified non-Complete content, sealed after actual close ACK.
        NativeFormalAfterClose,
    }

    struct SelectedSource {
        reference: fv::AnyReportRef,
        raw_body: String,
        class: FormalSourceClassR38,
    }

    // Private actual raw capture retained for the later fixed read authenticator.
    // It is not deserializable, not an API for replacing captured facts, and is
    // not an OriginalCapture/native writer permit.
    pub(crate) struct PreparedFormalSourceR38 {
        selected: SelectedSource,
        _raw: RawDatabase,
        _origin: Option<CompleteOriginR24>,
        authenticated: Option<authenticated_frozen_reader_r39::AuthenticatedFrozenInputs>,
    }

    /// Original native read completed its rollback, close ACK and source recheck.
    /// This is source transport only; it has no Current constructor or flag.
    pub struct ClosedFormalSourceR38 {
        captured: PreparedFormalSourceR38,
        current_complete: Option<NativeCurrentCompleteR39>,
        current_formal: Option<NativeCurrentFormalR39>,
    }
    impl ClosedFormalSourceR38 {
        pub fn reference(&self) -> &fv::AnyReportRef {
            &self.captured.selected.reference
        }
        pub fn source_class(&self) -> FormalSourceClassR38 {
            self.captured.selected.class
        }
        pub fn raw_body(&self) -> &str {
            &self.captured.selected.raw_body
        }
        pub fn current_complete(&self) -> Option<&NativeCurrentCompleteR39> {
            self.current_complete.as_ref()
        }
        pub fn current_formal(&self) -> Option<&NativeCurrentFormalR39> {
            self.current_formal.as_ref()
        }
    }

    fn invalid(code: &'static str) -> Error {
        Error::Conflict(format!("formal_invalid_read_source:r38:{code}"))
    }
    fn check(ok: bool, code: &'static str) -> Result<()> {
        if ok { Ok(()) } else { Err(invalid(code)) }
    }

    fn check_original_carrier_bytes(body: &str, reference: &fv::ReportRef) -> Result<()> {
        reference.validate_wire().map_err(r24_fault)?;
        check(
            hash(body.as_bytes()) == reference.carrier_digest,
            "original_carrier_raw_body_digest",
        )
    }

    impl FormalReadSelectorR38 {
        pub(crate) fn validate_request(&self, ctx: &Context) -> Result<()> {
            ctx.require(&[Role::Evaluator, Role::Admin, Role::Host])?;
            match self {
                Self::Ticket(id) => identifier(id),
                Self::Reference(fv::AnyReportRef::ImplicitLegacyV2(reference)) => {
                    reference.validate_wire().map_err(r24_fault)?;
                    check(
                        reference.namespace == ctx.namespace(),
                        "reference_namespace",
                    )
                }
                Self::Reference(fv::AnyReportRef::VersionedV3(reference)) => {
                    reference.validate_wire().map_err(r24_fault)?;
                    check(
                        reference.namespace == ctx.namespace(),
                        "reference_namespace",
                    )
                }
            }
        }
    }

    fn physical_id(reference: &fv::AnyReportRef) -> &str {
        match reference {
            fv::AnyReportRef::ImplicitLegacyV2(reference) => &reference.physical_id,
            fv::AnyReportRef::VersionedV3(reference) => &reference.physical_id,
        }
    }
    fn ticket_id(reference: &fv::AnyReportRef) -> &str {
        match reference {
            fv::AnyReportRef::ImplicitLegacyV2(reference) => &reference.ticket_id,
            fv::AnyReportRef::VersionedV3(reference) => &reference.ticket_id,
        }
    }

    // Called only after exact native type/identity extraction. It validates all
    // recognized report rows before selection, so missing origin material cannot
    // hide a known bad row belonging to the requested namespace.
    fn parse_source(namespace: &str, row: &OwnedRow) -> Result<Option<SelectedSource>> {
        let row_ns = native_text(row, "namespace")?;
        let kind = native_text(row, "kind")?;
        if row_ns != namespace || kind != "artifact" {
            return Ok(None);
        }
        let body = native_text(row, "body")?;
        let value = parse_owned_json(&body).map_err(|_| invalid("owned_json"))?;
        let schema = value.get("schema_version").and_then(Value::as_str);
        let record_kind = value.get("record_kind").and_then(Value::as_str);
        if schema != Some(fv::SCHEMA)
            && record_kind != Some(fv::KIND)
            && record_kind != Some(fv::OLD_KIND)
        {
            return Ok(None);
        }
        if schema != Some(fv::SCHEMA)
            && !(schema == Some(fv::OLD_SCHEMA) && record_kind == Some(fv::OLD_KIND))
        {
            return Err(r24_fault(fv::Fault::UnknownVersion));
        }
        let id = native_text(row, "id")?;
        let owner = native_text(row, "owner")?;
        identifier(&owner)?;
        check(
            body.len() <= fv::MAX_BYTES && native_integer(row, "revision")? > 0,
            "row_capacity_or_revision",
        )?;
        if schema == Some(fv::OLD_SCHEMA) {
            let original: LegacyFormalWireR24 =
                serde_json::from_str(&body).map_err(|_| invalid("legacy_typed_wire"))?;
            fv::version(&original.schema_version, &original.record_kind, None, None)
                .map_err(r24_fault)?;
            let ticket = r24_formal_ticket(&original.payload);
            check(
                original.id == id && id == fv::old_lookup_address(ticket).map_err(r24_fault)?,
                "legacy_row_or_ticket_identity",
            )?;
            validate_legacy_formal_shape_r1(namespace, ticket, &original.payload)?;
            let reference = fv::LegacyRef {
                namespace: namespace.into(),
                owner,
                physical_id: id,
                ticket_id: ticket.into(),
                payload_digest: typed_fingerprint(&original.payload)?,
            };
            reference.validate_wire().map_err(r24_fault)?;
            return Ok(Some(SelectedSource {
                reference: fv::AnyReportRef::ImplicitLegacyV2(reference),
                raw_body: body,
                class: FormalSourceClassR38::StaticLegacy,
            }));
        }
        check(record_kind == Some(fv::KIND), "versioned_record_kind")?;
        let engine = value.get("engine_version").and_then(Value::as_str);
        let input = value.get("input_schema_version").and_then(Value::as_str);
        let version = fv::version(fv::SCHEMA, fv::KIND, engine, input).map_err(r24_fault)?;
        match (engine, input) {
            (Some(fv::ENGINE), Some(fv::INPUT_SCHEMA)) => {
                check(
                    version.policy == fv::ReadPolicy::HistoricalNoEvaluator,
                    "historical_policy",
                )?;
                let carrier: FrozenCarrierR24 =
                    serde_json::from_str(&body).map_err(|_| invalid("legacy_carrier_wire"))?;
                validate_complete_carrier_r24(&carrier, namespace, &id)?;
                check(
                    owner == carrier.input.ticket_snapshot.evaluator_actor,
                    "legacy_carrier_owner",
                )?;
                let reference = carrier.report_ref(&owner).map_err(r24_fault)?;
                check_original_carrier_bytes(&body, &reference)?;
                Ok(Some(SelectedSource {
                    reference: fv::AnyReportRef::VersionedV3(reference),
                    raw_body: body,
                    class: FormalSourceClassR38::StaticLegacy,
                }))
            }
            (Some(fv::ENGINE_V4), Some(fv::INPUT_SCHEMA_V2)) => {
                check(
                    version.policy == fv::ReadPolicy::NativeVerificationRequired,
                    "native_policy",
                )?;
                let carrier: FrozenCarrierR36 =
                    serde_json::from_str(&body).map_err(|_| invalid("current_carrier_wire"))?;
                validate_complete_known_r36(&carrier, namespace, &id)?;
                check(
                    owner == carrier.input.ticket_snapshot.evaluator_actor,
                    "current_carrier_owner",
                )?;
                let reference = known_complete_reference_r36(&carrier, &owner)?;
                check_original_carrier_bytes(&body, &reference)?;
                // Full descriptor/known checks above do not invent missing
                // originals or assert that this source is Current.
                Ok(Some(SelectedSource {
                    reference: fv::AnyReportRef::VersionedV3(reference),
                    raw_body: body,
                    class: FormalSourceClassR38::FrozenVerificationPending,
                }))
            }
            _ => Err(r24_fault(fv::Fault::UnknownVersion)),
        }
    }

    fn unique_reference(references: &[fv::AnyReportRef]) -> Result<Option<usize>> {
        let mut physical = BTreeSet::new();
        for reference in references {
            match reference {
                fv::AnyReportRef::ImplicitLegacyV2(r) => r.validate_wire().map_err(r24_fault)?,
                fv::AnyReportRef::VersionedV3(r) => r.validate_wire().map_err(r24_fault)?,
            }
            check(
                physical.insert(physical_id(reference)),
                "duplicate_physical_report",
            )?;
        }
        if references.len() > 1 {
            return Err(Error::Conflict("formal_ambiguous_report_reference".into()));
        }
        Ok((!references.is_empty()).then_some(0))
    }

    fn select_source(
        raw: &RawDatabase,
        ctx: &Context,
        selector: &FormalReadSelectorR38,
    ) -> Result<Option<SelectedSource>> {
        selector.validate_request(ctx)?;
        let mut candidates = Vec::new();
        let exact_physical = match selector {
            FormalReadSelectorR38::Ticket(ticket) => {
                fv::old_lookup_address(ticket).map_err(r24_fault)?
            }
            FormalReadSelectorR38::Reference(reference) => physical_id(reference).to_owned(),
        };
        for row in raw
            .tables
            .get("objects")
            .ok_or_else(|| invalid("objects_missing"))?
        {
            // An artifact at the requested physical address with another
            // schema is known bad, not missing. Other object kinds may legally
            // share its ID and do not alias a formal artifact.
            let in_namespace = native_text(row, "namespace")? == ctx.namespace();
            let exact_row = in_namespace
                && native_text(row, "kind")? == "artifact"
                && native_text(row, "id")? == exact_physical;
            let Some(selected) = parse_source(ctx.namespace(), row)? else {
                if exact_row {
                    return Err(invalid("reference_schema_or_kind"));
                }
                continue;
            };
            let matches = match selector {
                FormalReadSelectorR38::Ticket(ticket) => ticket_id(&selected.reference) == ticket,
                FormalReadSelectorR38::Reference(reference) => {
                    physical_id(&selected.reference) == physical_id(reference)
                }
            };
            if matches {
                candidates.push(selected);
            }
        }
        let references: Vec<_> = candidates
            .iter()
            .map(|item| item.reference.clone())
            .collect();
        let Some(index) = unique_reference(&references)? else {
            return Ok(None);
        };
        let selected = candidates.swap_remove(index);
        if let FormalReadSelectorR38::Reference(expected) = selector {
            check(
                &selected.reference == expected,
                "reference_content_owner_or_protocol",
            )?;
        }
        Ok(Some(selected))
    }

    // Fixed query entry called inside the one original Deferred RO transaction.
    // Exactly one RawDatabase load; all further work uses those actual rows.
    pub(crate) fn capture_formal_source_r38(
        connection: &rusqlite::Connection,
        ctx: &Context,
        selector: &FormalReadSelectorR38,
    ) -> Result<Option<PreparedFormalSourceR38>> {
        Ok(capture_formal_snapshot_r39(connection, ctx, selector)?.formal)
    }

    pub struct NativeReadRecordR39 {
        namespace: String,
        kind: String,
        id: String,
        owner: String,
        revision: i64,
        raw_body: String,
        raw_body_digest: String,
        body: Value,
    }
    impl NativeReadRecordR39 {
        pub fn namespace(&self) -> &str {
            &self.namespace
        }
        pub fn kind(&self) -> &str {
            &self.kind
        }
        pub fn id(&self) -> &str {
            &self.id
        }
        pub fn owner(&self) -> &str {
            &self.owner
        }
        pub fn revision(&self) -> i64 {
            self.revision
        }
        pub fn raw_body(&self) -> &str {
            &self.raw_body
        }
        pub fn raw_body_digest(&self) -> &str {
            &self.raw_body_digest
        }
        pub fn body(&self) -> &Value {
            &self.body
        }
    }
    pub struct NativeTypedReadSetR39 {
        // No public iterator/constructor/Serde/raw DB. Exact selected closure,
        // request namespace and explicit current-policy schemas only.
        records: BTreeMap<(String, String), NativeReadRecordR39>,
    }
    impl NativeTypedReadSetR39 {
        pub fn get(&self, kind: &str, id: &str) -> Option<&NativeReadRecordR39> {
            self.records.get(&(kind.to_owned(), id.to_owned()))
        }
        pub fn material_digest_r49(&self) -> Result<String> {
            let tuples = self
                .records
                .iter()
                .map(|(key, record)| {
                    (
                        key,
                        &record.namespace,
                        &record.owner,
                        record.revision,
                        &record.raw_body_digest,
                    )
                })
                .collect::<Vec<_>>();
            typed_fingerprint(&tuples)
        }
    }
    pub struct ClosedFormalReadR39 {
        namespace: String,
        logical_ticket_id: String,
        live_ticket: Option<Value>,
        formal: Option<ClosedFormalSourceR38>,
        watermark: Option<(u64, String)>,
        records: NativeTypedReadSetR39,
        lineage: Option<fv::LineageAddresses>,
        latest_root_seal: ScopedLatestRootSealR42,
        host: super::native_formal_consumers_r49::HostInspectionR49,
    }
    impl ClosedFormalReadR39 {
        pub fn namespace(&self) -> &str {
            &self.namespace
        }
        pub fn logical_ticket_id(&self) -> &str {
            &self.logical_ticket_id
        }
        pub fn live_ticket(&self) -> Option<&Value> {
            self.live_ticket.as_ref()
        }
        pub fn formal(&self) -> Option<&ClosedFormalSourceR38> {
            self.formal.as_ref()
        }
        pub fn watermark(&self) -> Option<&(u64, String)> {
            self.watermark.as_ref()
        }
        pub fn records(&self) -> &NativeTypedReadSetR39 {
            &self.records
        }
        pub fn host_evidence(&self) -> Result<Option<&super::NativeHostEvidenceR49>> {
            self.host.evidence()
        }

        /// This selected root's latest seal, checked against every current
        /// stage/group/call/non-seal event in the same native RO snapshot.
        /// A stale seal remains Conflict; this does not grant complete episode,
        /// Host, approval, publication or mutation authority.
        pub fn latest_root_seal(&self) -> Result<Option<&crate::budget::SealedRootBudgetEvidence>> {
            self.latest_root_seal.evidence()
        }
        /// Static lookup candidates only, never a proven ancestor relation.
        pub fn lineage_lookup_candidates(&self) -> Option<&fv::LineageAddresses> {
            self.lineage.as_ref()
        }
    }
    pub(crate) struct PreparedFormalReadR39 {
        namespace: String,
        logical_ticket_id: String,
        live_ticket: Option<Value>,
        formal: Option<PreparedFormalSourceR38>,
        watermark: Option<(u64, String)>,
        records: NativeTypedReadSetR39,
        lineage: Option<fv::LineageAddresses>,
        latest_root_seal: ScopedLatestRootSealR42,
        host: super::native_formal_consumers_r49::HostInspectionR49,
    }

    fn policy_record_visible_r39(object: &OccurrenceObject, ticket_id: &str) -> Result<bool> {
        let schema = object.body.get("schema_version").and_then(Value::as_str);
        if object.kind == "artifact" && object.id == evaluation_storage_id(TICKET_KIND, ticket_id)?
        {
            return Ok(true);
        }
        Ok(schema.is_some_and(|schema| {
            compiler_node_schema(&object.kind, schema)
                || schema == "rsia.release_candidate.v1"
                || schema == "rsia.registered_formal_principals.v1"
        }))
        // Typed holdout/oracle, execution output and private request/source
        // bodies are deliberately excluded even if present in the graph.
    }

    pub(crate) fn capture_formal_snapshot_r39(
        connection: &rusqlite::Connection,
        ctx: &Context,
        selector: &FormalReadSelectorR38,
    ) -> Result<PreparedFormalReadR39> {
        selector.validate_request(ctx)?;
        require(!connection.is_autocommit())?;
        let raw = load_rusqlite_database(connection)?;
        require(!connection.is_autocommit())?;
        prepare_actual_formal_snapshot_r49(raw, ctx, selector)
    }

    pub(crate) fn prepare_formal_snapshot_from_raw_r50(
        raw: RawDatabase,
        ctx: &Context,
        selector: &FormalReadSelectorR38,
    ) -> Result<PreparedFormalReadR39> {
        selector.validate_request(ctx)?;
        prepare_actual_formal_snapshot_r49(raw, ctx, selector)
    }

    pub(crate) fn prepare_formal_source_from_raw_r50(
        raw: RawDatabase,
        ctx: &Context,
        selector: &FormalReadSelectorR38,
    ) -> Result<Option<PreparedFormalSourceR38>> {
        Ok(prepare_formal_snapshot_from_raw_r50(raw, ctx, selector)?.formal)
    }

    fn diagnose_selected_statistics_r49(
        raw: &RawDatabase,
        ctx: &Context,
        selector: &FormalReadSelectorR38,
    ) -> Result<()> {
        for row in raw.tables.get("objects").ok_or(Error::ClockRegressed)? {
            if native_text(row, "namespace")? != ctx.namespace()
                || native_text(row, "kind")? != "artifact"
            {
                continue;
            }
            let id = native_text(row, "id")?;
            let text = native_text(row, "body")?;
            if text.len() > 4 * 1024 * 1024 {
                continue;
            }
            let value = parse_owned_json(&text)?;
            if value["schema_version"] != fv::SCHEMA
                || value["record_kind"] != fv::KIND
                || value["engine_version"] != fv::ENGINE_V4
                || value["input_schema_version"] != fv::INPUT_SCHEMA_V2
            {
                continue;
            }
            let selected = match selector {
                FormalReadSelectorR38::Ticket(ticket) => {
                    value["input"]["ticket_id"] == ticket.as_str()
                }
                FormalReadSelectorR38::Reference(fv::AnyReportRef::VersionedV3(reference)) => {
                    id == reference.physical_id
                }
                _ => false,
            };
            if !selected {
                continue;
            }
            let carrier: FrozenCarrierR36 = serde_json::from_str(&text).map_err(crate::internal)?;
            if typed_fingerprint(&carrier.payload)? == carrier.payload_digest {
                continue;
            }
            require(native_text(row, "owner")? == carrier.input.ticket_snapshot.evaluator_actor)?;
            if let FormalReadSelectorR38::Reference(fv::AnyReportRef::VersionedV3(reference)) =
                selector
            {
                reference.validate_wire().map_err(r24_fault)?;
                require(
                    reference.namespace == ctx.namespace()
                        && reference.owner == carrier.input.ticket_snapshot.evaluator_actor
                        && reference.ticket_id == carrier.input.ticket_id
                        && reference.input_digest == carrier.input_digest,
                )?;
            }
            authenticated_frozen_reader_r39::diagnose_statistics_before_checksum_r49(
                raw, ctx, &id, &carrier,
            )?;
        }
        Ok(())
    }

    fn prepare_actual_formal_snapshot_r49(
        raw: RawDatabase,
        ctx: &Context,
        selector: &FormalReadSelectorR38,
    ) -> Result<PreparedFormalReadR39> {
        selector.validate_request(ctx)?;
        // Preserve R38R2's known-row check before any source gap, and its
        // mandatory actual audit/origin validation even when formal is absent.
        diagnose_selected_statistics_r49(&raw, ctx, selector)?;
        let selected = select_source(&raw, ctx, selector)?;
        let (actual, _floor) = snapshot(&raw)?;
        let logical_ticket_id = match selector {
            FormalReadSelectorR38::Ticket(id) => id.clone(),
            FormalReadSelectorR38::Reference(reference) => ticket_id(reference).to_owned(),
        };
        let live_key = (
            ctx.namespace().to_owned(),
            "artifact".to_owned(),
            evaluation_storage_id(TICKET_KIND, &logical_ticket_id)?,
        );
        let live_ticket = if let Some(live_object) = actual.objects.get(&live_key) {
            if live_object
                .body
                .get("schema_version")
                .and_then(Value::as_str)
                == Some("rsia.redacted.v1")
            {
                qualify_redacted(live_object)?;
                if selected.is_none() {
                    return Err(Error::Forbidden);
                }
                None
            } else {
                let value: MirrorEvaluationTicketV2 =
                    evaluation_record(&actual, ctx.namespace(), TICKET_KIND, &logical_ticket_id)?;
                value.validate()?;
                if let FormalReadSelectorR38::Reference(reference) = selector {
                    let expected_owner = match reference {
                        fv::AnyReportRef::ImplicitLegacyV2(reference) => &reference.owner,
                        fv::AnyReportRef::VersionedV3(reference) => &reference.owner,
                    };
                    check(
                        value.evaluator_actor == *expected_owner,
                        "typed_reference_known_ticket_owner",
                    )?;
                }
                if selected.is_none() {
                    let object = actual.object(ctx.namespace(), "artifact", &live_key.2)?;
                    let mut floor = OccurrenceFloor::default();
                    qualify_evaluation(object, &actual, &mut floor)?;
                    let control =
                        evaluation_control(&actual, ctx.namespace(), &value.registration_id)?;
                    let holdout =
                        evaluation_holdout(&actual, ctx.namespace(), &value.holdout_id, &control)?;
                    producer_witnesses(&actual, ctx.namespace(), &control, &holdout)?;
                    e05_complete_ticket_identity(
                        &actual,
                        ctx.namespace(),
                        &value,
                        &control,
                        &holdout,
                    )?;
                }
                Some(value)
            }
        } else {
            None
        };
        let latest_root_seal = if let Some(ticket) = live_ticket.as_ref() {
            let physical = evaluation_storage_id(
                "registered_evaluation_control_v41",
                &ticket.registration_id,
            )?;
            let control_key = (ctx.namespace().to_owned(), "artifact".to_owned(), physical);
            match actual.objects.get(&control_key) {
                Some(object)
                    if object.body.get("schema_version").and_then(Value::as_str)
                        != Some("rsia.redacted.v1") =>
                {
                    let control =
                        evaluation_control(&actual, ctx.namespace(), &ticket.registration_id)?;
                    check(
                        ticket.registration_digest == typed_fingerprint(&control)?
                            && ticket.evaluator_actor == control.evaluator_actor,
                        "selected_ticket_control_identity",
                    )?;
                    strict_latest_root_seal_r42(&raw, &actual, ctx, &control.billing_scope)?
                }
                _ => ScopedLatestRootSealR42::Absent,
            }
        } else {
            ScopedLatestRootSealR42::Absent
        };
        let mut allowed = BTreeSet::new();
        allowed.insert(("artifact".to_owned(), live_key.2.clone()));
        if let Some(ticket) = &live_ticket
            && let Some(binding) = &ticket.candidate_dependency_binding
        {
            for node in &binding.nodes {
                allowed.insert((node.kind.clone(), node.id.clone()));
            }
        }
        if let Some(selected) = &selected {
            if let fv::AnyReportRef::VersionedV3(reference) = &selected.reference {
                match (
                    reference.engine_version.as_str(),
                    reference.input_schema_version.as_str(),
                ) {
                    (fv::ENGINE_V4, fv::INPUT_SCHEMA_V2) => {
                        let carrier: FrozenCarrierR36 =
                            serde_json::from_str(&selected.raw_body).map_err(crate::internal)?;
                        for fact in carrier.input.facts {
                            allowed.insert((fact.kind, fact.id));
                        }
                    }
                    (fv::ENGINE, fv::INPUT_SCHEMA) => {
                        let carrier: FrozenCarrierR24 =
                            serde_json::from_str(&selected.raw_body).map_err(crate::internal)?;
                        for fact in carrier.input.facts {
                            allowed.insert((fact.kind, fact.id));
                        }
                    }
                    _ => return Err(r24_fault(fv::Fault::UnknownVersion)),
                }
            }
        }
        let mut records = BTreeMap::new();
        for row in raw
            .tables
            .get("objects")
            .ok_or_else(|| invalid("objects_missing"))?
        {
            let namespace = native_text(row, "namespace")?;
            let kind = native_text(row, "kind")?;
            let id = native_text(row, "id")?;
            if namespace != ctx.namespace() || !allowed.contains(&(kind.clone(), id.clone())) {
                continue;
            }
            let object = actual.object(&namespace, &kind, &id)?;
            if !policy_record_visible_r39(object, &logical_ticket_id)? {
                continue;
            }
            let raw_body = native_text(row, "body")?;
            let record = NativeReadRecordR39 {
                namespace,
                kind: kind.clone(),
                id: id.clone(),
                owner: native_text(row, "owner")?,
                revision: native_integer(row, "revision")?,
                raw_body_digest: hash(raw_body.as_bytes()),
                raw_body,
                body: object.body.clone(),
            };
            check(
                records.insert((kind, id), record).is_none(),
                "duplicate_visible_read_record",
            )?;
        }
        let lineage = match selected.as_ref().map(|source| &source.reference) {
            Some(fv::AnyReportRef::VersionedV3(reference)) => Some(
                fv::lineage_addresses(ctx.namespace(), &logical_ticket_id, &reference.input_digest)
                    .map_err(r24_fault)?,
            ),
            _ => None,
        };
        let watermark = actual.watermarks.get(ctx.namespace()).cloned();
        let live_ticket = live_ticket
            .map(serde_json::to_value)
            .transpose()
            .map_err(crate::internal)?;
        let host = match selected.as_ref().map(|source| &source.reference) {
            Some(fv::AnyReportRef::VersionedV3(reference))
                if reference.engine_version == fv::ENGINE_V4
                    && reference.input_schema_version == fv::INPUT_SCHEMA_V2 =>
            {
                super::native_formal_consumers_r49::inspect_host_r49(&raw, &actual, ctx, reference)?
            }
            _ => super::native_formal_consumers_r49::HostInspectionR49::NotApplicable,
        };
        let formal = prepare_from_actual_raw_r39(raw, actual, ctx, selected)?;
        Ok(PreparedFormalReadR39 {
            namespace: ctx.namespace().into(),
            logical_ticket_id,
            live_ticket,
            formal,
            watermark,
            records: NativeTypedReadSetR39 { records },
            lineage,
            latest_root_seal,
            host,
        })
    }

    fn prepare_from_actual_raw_r39(
        raw: RawDatabase,
        actual: OccurrenceSnapshot,
        ctx: &Context,
        selected: Option<SelectedSource>,
    ) -> Result<Option<PreparedFormalSourceR38>> {
        let Some(selected) = selected else {
            return Ok(None);
        };
        let origin = match &selected.reference {
            fv::AnyReportRef::ImplicitLegacyV2(_) => None,
            fv::AnyReportRef::VersionedV3(reference) => actual
                .complete_origins_r24
                .get(&(reference.namespace.clone(), reference.physical_id.clone()))
                .cloned(),
        };
        let authenticated = match &selected.reference {
            fv::AnyReportRef::VersionedV3(reference)
                if reference.engine_version == fv::ENGINE_V4
                    && reference.input_schema_version == fv::INPUT_SCHEMA_V2 =>
            {
                Some(authenticated_frozen_reader_r39::authenticate(
                    &raw, &actual, ctx, reference,
                )?)
            }
            _ => None,
        };
        Ok(Some(PreparedFormalSourceR38 {
            selected,
            _raw: raw,
            _origin: origin,
            authenticated,
        }))
    }

    pub struct FormalReadDataR49 {
        reference: fv::AnyReportRef,
        frozen_ticket: Value,
        payload: Value,
        watermark: Option<(u64, String)>,
        host_authority: Option<Value>,
        host_report_digest: Option<String>,
        source_legacy: bool,
    }
    impl FormalReadDataR49 {
        pub fn reference(&self) -> &fv::AnyReportRef {
            &self.reference
        }
        pub fn frozen_ticket(&self) -> &Value {
            &self.frozen_ticket
        }
        pub fn payload(&self) -> &Value {
            &self.payload
        }
        pub fn watermark(&self) -> Option<&(u64, String)> {
            self.watermark.as_ref()
        }
        pub fn host_authority(&self) -> Option<&Value> {
            self.host_authority.as_ref()
        }
        pub fn host_report_digest(&self) -> Option<&str> {
            self.host_report_digest.as_deref()
        }
        pub fn source_legacy(&self) -> bool {
            self.source_legacy
        }
    }
    impl ClosedFormalReadR39 {
        /// A data projection of this already closed actual capture. Promotion
        /// authority still requires a new fixed native approval transaction.
        pub fn data_view_r49(&self) -> Result<FormalReadDataR49> {
            let formal = self.formal.as_ref().ok_or(Error::NotFound)?;
            let (frozen_ticket, payload, legacy) = if let Some(current) = formal.current_formal() {
                (current.frozen_ticket()?, current.payload().clone(), false)
            } else {
                let raw: Value =
                    serde_json::from_str(formal.raw_body()).map_err(crate::internal)?;
                (
                    self.live_ticket
                        .clone()
                        .ok_or_else(|| r24_fault(fv::Fault::SourceInsufficient))?,
                    raw.get("payload")
                        .cloned()
                        .ok_or_else(|| Error::Conflict("legacy formal lacks payload".into()))?,
                    true,
                )
            };
            let host = self.host.evidence()?;
            Ok(FormalReadDataR49 {
                reference: formal.reference().clone(),
                frozen_ticket,
                payload,
                watermark: self.watermark.clone(),
                host_authority: host.map(|evidence| evidence.authority().clone()),
                host_report_digest: host.map(|evidence| evidence.report_digest().to_owned()),
                source_legacy: legacy,
            })
        }
    }
    pub(crate) async fn inspect_formal_data_r49(
        session: &mut crate::Session,
        ctx: &Context,
        selector: FormalReadSelectorR38,
    ) -> Result<FormalReadDataR49> {
        session.clock.check()?;
        let tx = session.tx.as_mut().ok_or(Error::ClockRegressed)?;
        require(
            tx.open
                && tx.is_in_transaction()
                && SqliteTransactionManager::get_transaction_depth(&**tx) == 1,
        )?;
        let raw = load_sqlx_database(&mut **tx).await?;
        let prepared = prepare_actual_formal_snapshot_r49(raw, ctx, &selector)?;
        let source = prepared.formal.as_ref().ok_or(Error::NotFound)?;
        let (frozen_ticket, payload, legacy) = if let Some(inputs) = source.authenticated.as_ref() {
            let (ticket, payload) = authenticated_frozen_reader_r39::data_projection_r49(inputs)?;
            (ticket, payload, false)
        } else {
            let raw: Value =
                serde_json::from_str(&source.selected.raw_body).map_err(crate::internal)?;
            (
                prepared
                    .live_ticket
                    .clone()
                    .ok_or_else(|| r24_fault(fv::Fault::SourceInsufficient))?,
                raw.get("payload")
                    .cloned()
                    .ok_or_else(|| Error::Conflict("legacy formal lacks payload".into()))?,
                true,
            )
        };
        let host = prepared.host.evidence()?;
        Ok(FormalReadDataR49 {
            reference: source.selected.reference.clone(),
            frozen_ticket,
            payload,
            watermark: prepared.watermark.clone(),
            host_authority: host.map(|evidence| evidence.authority().clone()),
            host_report_digest: host.map(|evidence| evidence.report_digest().to_owned()),
            source_legacy: legacy,
        })
    }

    pub(crate) fn finish_formal_snapshot_r39(
        prepared: PreparedFormalReadR39,
        completed: crate::lifecycle::OriginalReadCompletedR38,
    ) -> ClosedFormalReadR39 {
        let formal = prepared
            .formal
            .map(|source| finish_formal_source_r38(source, completed));
        ClosedFormalReadR39 {
            namespace: prepared.namespace,
            logical_ticket_id: prepared.logical_ticket_id,
            live_ticket: prepared.live_ticket,
            formal,
            watermark: prepared.watermark,
            records: prepared.records,
            lineage: prepared.lineage,
            latest_root_seal: prepared.latest_root_seal,
            host: prepared.host,
        }
    }

    pub(crate) fn finish_formal_source_r38(
        mut captured: PreparedFormalSourceR38,
        completed: crate::lifecycle::OriginalReadCompletedR38,
    ) -> ClosedFormalSourceR38 {
        let current_formal = captured
            .authenticated
            .take()
            .map(|source| authenticated_frozen_reader_r39::after_close(source, &completed));
        let current_complete = current_formal
            .as_ref()
            .and_then(NativeCurrentFormalR39::complete_projection);
        if current_complete.is_some() {
            captured.selected.class = FormalSourceClassR38::NativeCompleteAfterClose;
        } else if current_formal.is_some() {
            captured.selected.class = FormalSourceClassR38::NativeFormalAfterClose;
        }
        ClosedFormalSourceR38 {
            captured,
            current_complete,
            current_formal,
        }
    }
    #[cfg(test)]
    mod source_controls {
        use super::*;

        fn context() -> Context {
            Context::new("r38", "evaluator", Role::Evaluator).unwrap()
        }

        fn reference(input_byte: char) -> fv::ReportRef {
            let input_digest = input_byte.to_string().repeat(64);
            fv::ReportRef {
                schema_version: fv::REF_SCHEMA.into(),
                namespace: "r38".into(),
                owner: "evaluator".into(),
                physical_id: fv::address("r38", fv::ENGINE_V4, fv::INPUT_SCHEMA_V2, &input_digest)
                    .unwrap(),
                engine_version: fv::ENGINE_V4.into(),
                input_schema_version: fv::INPUT_SCHEMA_V2.into(),
                input_digest,
                payload_digest: "b".repeat(64),
                carrier_digest: "c".repeat(64),
                ticket_id: "ticket".into(),
                ticket_digest: "d".repeat(64),
            }
        }

        // A static decoder control, not a claimed real historical constant or
        // a replacement for Root's before-upgrade native capture.
        fn legacy_raw(ticket: &str, stored_id: &str) -> RawDatabase {
            let body = serde_json::to_string(&serde_json::json!({
                "schema_version": fv::OLD_SCHEMA,
                "id": stored_id,
                "record_kind": fv::OLD_KIND,
                "payload": {
                    "variant": "invalid",
                    "evidence_scope": "program_fixture",
                    "ticket_id": ticket,
                    "ticket_digest": "a".repeat(64),
                    "reasons": ["controlled_fixture_invalid"]
                }
            }))
            .unwrap();
            let cells = [
                ("namespace".into(), OwnedCell::Text("r38".into())),
                ("kind".into(), OwnedCell::Text("artifact".into())),
                ("id".into(), OwnedCell::Text(stored_id.into())),
                ("owner".into(), OwnedCell::Text("evaluator".into())),
                ("revision".into(), OwnedCell::Integer(1)),
                ("body".into(), OwnedCell::Text(body)),
            ]
            .into_iter()
            .collect();
            let mut raw = RawDatabase::default();
            raw.tables
                .insert("objects".into(), vec![OwnedRow { cells }]);
            raw
        }

        #[test]
        fn actual_original_three_one_remains_static_legacy_and_raw_mutation_is_known_invalid() {
            // Genuine native63604 original body bytes; never a newly built DTO.
            let body =
                include_str!("../tests/fixtures/e05-engine3-input1-invalid-original-r36.json");
            assert_eq!(body.len(), 456166);
            assert_eq!(
                hash(body.as_bytes()),
                "b2073a576809b7590bc0aec5f495f62c2178834b18d79334c6051e8833345be9"
            );
            let carrier: FrozenCarrierR24 = serde_json::from_str(body).unwrap();
            let ctx = Context::new(
                &carrier.input.namespace,
                &carrier.input.ticket_snapshot.evaluator_actor,
                Role::Evaluator,
            )
            .unwrap();
            let reference = carrier.report_ref(ctx.actor()).unwrap();
            assert_eq!(reference.engine_version, fv::ENGINE);
            assert_eq!(reference.input_schema_version, fv::INPUT_SCHEMA);
            let row = |text: &str| OwnedRow {
                cells: [
                    (
                        "namespace".into(),
                        OwnedCell::Text(reference.namespace.clone()),
                    ),
                    ("kind".into(), OwnedCell::Text("artifact".into())),
                    ("id".into(), OwnedCell::Text(reference.physical_id.clone())),
                    ("owner".into(), OwnedCell::Text(reference.owner.clone())),
                    ("revision".into(), OwnedCell::Integer(1)),
                    ("body".into(), OwnedCell::Text(text.into())),
                ]
                .into_iter()
                .collect(),
            };
            let mut raw = RawDatabase::default();
            raw.tables.insert("objects".into(), vec![row(body)]);
            let selector =
                FormalReadSelectorR38::Reference(fv::AnyReportRef::VersionedV3(reference.clone()));
            let selected = select_source(&raw, &ctx, &selector).unwrap().unwrap();
            assert_eq!(selected.class, FormalSourceClassR38::StaticLegacy);
            assert_eq!(selected.raw_body.as_bytes(), body.as_bytes());
            assert_eq!(
                selected.reference,
                fv::AnyReportRef::VersionedV3(reference.clone())
            );
            // Missing audit/origin is intentional. The typed carrier is
            // unchanged by whitespace, but its original-byte binding is bad.
            let changed = format!("{body} ");
            let typed: FrozenCarrierR24 = serde_json::from_str(&changed).unwrap();
            assert_eq!(typed.report_ref(ctx.actor()).unwrap(), reference);
            raw.tables.insert("objects".into(), vec![row(&changed)]);
            assert!(matches!(select_source(&raw, &ctx, &selector),
                Err(Error::Conflict(ref reason))
                    if reason == "formal_invalid_read_source:r38:original_carrier_raw_body_digest"));
        }

        #[test]
        fn typed_reference_is_not_a_ticket_alias_or_current_claim() {
            let ctx = context();
            let valid = reference('a');
            FormalReadSelectorR38::Reference(fv::AnyReportRef::VersionedV3(valid.clone()))
                .validate_request(&ctx)
                .unwrap();
            let mut bad = valid.clone();
            bad.physical_id = bad.ticket_id.clone();
            assert!(
                FormalReadSelectorR38::Reference(fv::AnyReportRef::VersionedV3(bad),)
                    .validate_request(&ctx)
                    .is_err()
            );
            let mut foreign = valid;
            foreign.namespace = "other".into();
            assert!(
                FormalReadSelectorR38::Reference(fv::AnyReportRef::VersionedV3(foreign),)
                    .validate_request(&ctx)
                    .is_err()
            );

            let actual_id = fv::old_lookup_address("ticket").unwrap();
            let raw = legacy_raw("ticket", &actual_id);
            let source = select_source(&raw, &ctx, &FormalReadSelectorR38::Ticket("ticket".into()))
                .unwrap()
                .unwrap();
            assert_eq!(source.class, FormalSourceClassR38::StaticLegacy);
            assert!(
                select_source(&raw, &ctx, &FormalReadSelectorR38::Ticket(actual_id))
                    .unwrap()
                    .is_none()
            );
        }

        #[test]
        fn old3_input1_has_no_retained_full_evaluator_and_unknown_is_quarantined() {
            assert_eq!(
                fv::version(
                    fv::SCHEMA,
                    fv::KIND,
                    Some(fv::ENGINE),
                    Some(fv::INPUT_SCHEMA)
                )
                .unwrap()
                .policy,
                fv::ReadPolicy::HistoricalNoEvaluator
            );
            assert_eq!(
                fv::version(
                    fv::SCHEMA,
                    fv::KIND,
                    Some(fv::ENGINE_V4),
                    Some(fv::INPUT_SCHEMA_V2)
                )
                .unwrap()
                .policy,
                fv::ReadPolicy::NativeVerificationRequired
            );
            assert!(matches!(
                fv::version(
                    fv::SCHEMA,
                    fv::KIND,
                    Some("rsia.e05.formal_engine.v999"),
                    Some(fv::INPUT_SCHEMA_V2)
                ),
                Err(fv::Fault::UnknownVersion)
            ));
        }

        #[test]
        fn ticket_selector_requires_unique_report_and_duplicate_identity_is_invalid() {
            let first = fv::AnyReportRef::VersionedV3(reference('a'));
            let second = fv::AnyReportRef::VersionedV3(reference('b'));
            assert!(matches!(unique_reference(&[first.clone(), second]),
                Err(Error::Conflict(message)) if message == "formal_ambiguous_report_reference"));
            assert!(matches!(unique_reference(&[first.clone(), first]),
                Err(Error::Conflict(message)) if message.ends_with("duplicate_physical_report")));
        }

        #[test]
        fn known_bad_identity_precedes_unrelated_missing_audit_origin() {
            let raw = legacy_raw("ticket", "wrong-physical");
            assert!(!raw.tables.contains_key("audit"));
            assert!(matches!(select_source(&raw, &context(),
                &FormalReadSelectorR38::Ticket("ticket".into())),
                Err(Error::Conflict(message)) if message.ends_with("legacy_row_or_ticket_identity")));
        }

        #[test]
        fn exact_artifact_wrong_schema_is_invalid_but_other_kind_does_not_alias() {
            let actual_id = fv::old_lookup_address("ticket").unwrap();
            let mut raw = legacy_raw("ticket", &actual_id);
            raw.tables.get_mut("objects").unwrap()[0]
                .cells
                .insert("body".into(), OwnedCell::Text("{}".into()));
            assert!(matches!(select_source(&raw, &context(),
                &FormalReadSelectorR38::Ticket("ticket".into())),
                Err(Error::Conflict(message)) if message.ends_with("reference_schema_or_kind")));
            raw.tables.get_mut("objects").unwrap()[0]
                .cells
                .insert("kind".into(), OwnedCell::Text("blob".into()));
            assert!(
                select_source(
                    &raw,
                    &context(),
                    &FormalReadSelectorR38::Ticket("ticket".into())
                )
                .unwrap()
                .is_none()
            );
        }

        #[test]
        fn source_selection_does_not_backfill_from_later_financial_or_shared_rows() {
            let actual_id = fv::old_lookup_address("ticket").unwrap();
            let mut raw = legacy_raw("ticket", &actual_id);
            let before = select_source(
                &raw,
                &context(),
                &FormalReadSelectorR38::Ticket("ticket".into()),
            )
            .unwrap()
            .unwrap();
            // Pure selection control only, not SQL/as-of qualification. Missing
            // originals are never synthesized from either of these later rows.
            raw.tables.insert(
                "root_budget_calls".into(),
                vec![OwnedRow {
                    cells: [("later_paid_overlay".into(), OwnedCell::Integer(99))]
                        .into_iter()
                        .collect(),
                }],
            );
            raw.tables.insert(
                "unrelated_shared_exposure".into(),
                vec![OwnedRow {
                    cells: [("later_other_ticket".into(), OwnedCell::Text("other".into()))]
                        .into_iter()
                        .collect(),
                }],
            );
            let after = select_source(
                &raw,
                &context(),
                &FormalReadSelectorR38::Ticket("ticket".into()),
            )
            .unwrap()
            .unwrap();
            assert_eq!(before.reference, after.reference);
            assert_eq!(before.raw_body, after.raw_body);
            assert_eq!(before.class, after.class);
            // Actual input-v2 late-finance Current recomputation still requires
            // the unfinished private frozen-input authentication factory.
        }
    }
}

mod authenticated_frozen_reader_r39 {
    use super::*;
    use evo_core::formal_version as fv;

    // This is native source ownership, never a wire DTO or an "as_of=true".
    pub(super) struct AuthenticatedFrozenInputs {
        reference: fv::ReportRef,
        payload: Value,
        frozen_ticket: MirrorEvaluationTicketV2,
        frozen_root: fv::RootBudgetSnapshotV2,
        frozen_group: fv::DispatchGroupSnapshotV2,
        frozen_exposure: fv::ExposureProjectionV2,
        original_calls: Vec<BudgetCallRecord>,
        paid_overlay: Vec<BudgetCallRecord>,
        verified_variant: VerifiedFormalVariantR2,
    }
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum VerifiedFormalVariantR2 {
        Complete,
        EarlyStopped,
        Invalid,
    }

    /// Qualified content from this read capture, returned only after the
    /// original native transaction and close ACK. It grants no write permit,
    /// provider authority, FullHost capability or CompleteEpisodeRoot evidence.
    pub struct NativeCurrentCompleteR39 {
        source: Arc<AuthenticatedFrozenInputs>,
    }
    /// Actual variant-neutral content validation, never an approval permission.
    /// The Complete capability is available separately only for CompleteBatch.
    pub struct NativeCurrentFormalR39 {
        source: Arc<AuthenticatedFrozenInputs>,
    }
    impl NativeCurrentFormalR39 {
        pub fn reference(&self) -> &fv::ReportRef {
            &self.source.reference
        }
        pub fn payload(&self) -> &Value {
            &self.source.payload
        }
        pub fn frozen_ticket(&self) -> Result<Value> {
            serde_json::to_value(&self.source.frozen_ticket).map_err(crate::internal)
        }
        pub(super) fn complete_projection(&self) -> Option<NativeCurrentCompleteR39> {
            (self.source.verified_variant == VerifiedFormalVariantR2::Complete).then(|| {
                NativeCurrentCompleteR39 {
                    source: Arc::clone(&self.source),
                }
            })
        }
    }
    impl NativeCurrentCompleteR39 {
        pub fn reference(&self) -> &fv::ReportRef {
            &self.source.reference
        }
        pub fn payload(&self) -> &Value {
            &self.source.payload
        }
        pub fn original_calls(&self) -> &[BudgetCallRecord] {
            &self.source.original_calls
        }
        pub fn paid_overlay(&self) -> &[BudgetCallRecord] {
            &self.source.paid_overlay
        }
        pub fn frozen_ticket(&self) -> Result<Value> {
            serde_json::to_value(&self.source.frozen_ticket).map_err(crate::internal)
        }
        pub fn frozen_root(&self) -> &fv::RootBudgetSnapshotV2 {
            &self.source.frozen_root
        }
        pub fn frozen_group(&self) -> &fv::DispatchGroupSnapshotV2 {
            &self.source.frozen_group
        }
        pub fn frozen_exposure(&self) -> &fv::ExposureProjectionV2 {
            &self.source.frozen_exposure
        }
    }

    fn invalid(code: &'static str) -> Error {
        Error::Conflict(format!("formal_invalid_frozen_read:r39:{code}"))
    }
    fn check(ok: bool, code: &'static str) -> Result<()> {
        if ok { Ok(()) } else { Err(invalid(code)) }
    }
    fn missing(gaps: &BTreeSet<&'static str>) -> Error {
        Error::Conflict(format!(
            "formal_source_insufficient:r39:{}",
            gaps.iter().copied().collect::<Vec<_>>().join(",")
        ))
    }

    // Does not own a fabricated Snapshot. Every ordinary record comes from
    // the one ACTUAL captured immutable body, already matched against its fact.
    // Only the independently authenticated mutable projection has a different
    // mathematical data source from the ordinary actual-snapshot reducer.
    struct FrozenRecords<'a> {
        actual: &'a OccurrenceSnapshot,
        facts: &'a [fv::Fact],
        exposure: &'a fv::ExposureProjectionV2,
    }
    impl E05CompleteRecordsR39 for FrozenRecords<'_> {
        fn record<T: DeserializeOwned>(&self, namespace: &str, kind: &str, id: &str) -> Result<T> {
            let physical = evaluation_storage_id(kind, id)?;
            check(
                self.facts.iter().any(|fact| {
                    fact.namespace == namespace && fact.kind == "artifact" && fact.id == physical
                }),
                "math_record_not_in_authenticated_fact_graph",
            )?;
            evaluation_record(self.actual, namespace, kind, id)
        }
        fn exposure(
            &self,
            namespace: &str,
            ticket: &MirrorEvaluationTicketV2,
        ) -> Result<fv::ExposureProjectionV2> {
            check(
                namespace
                    == self
                        .facts
                        .first()
                        .ok_or_else(|| invalid("empty_facts"))?
                        .namespace
                    && self.exposure.research_family_id == ticket.research_family_id
                    && self.exposure.alpha_plan_digest == ticket.alpha_plan_digest
                    && self.exposure.entry.ticket_id == ticket.id,
                "authenticated_exposure_math_identity",
            )?;
            Ok(self.exposure.clone())
        }
    }

    fn current_ticket_overlay(
        actual: &MirrorEvaluationTicketV2,
        frozen: &MirrorEvaluationTicketV2,
    ) -> Result<()> {
        actual.validate()?;
        frozen.validate()?;
        for (old, new) in [
            (
                &frozen.late_execution_receipt_ids,
                &actual.late_execution_receipt_ids,
            ),
            (
                &frozen.late_grader_receipt_ids,
                &actual.late_grader_receipt_ids,
            ),
        ] {
            check(new.starts_with(old), "late_ticket_receipts_not_append_only")?;
        }
        let mut old = serde_json::to_value(frozen).map_err(crate::internal)?;
        let mut new = serde_json::to_value(actual).map_err(crate::internal)?;
        for field in ["late_execution_receipt_ids", "late_grader_receipt_ids"] {
            old.as_object_mut()
                .ok_or_else(|| invalid("frozen_ticket_shape"))?
                .remove(field);
            new.as_object_mut()
                .ok_or_else(|| invalid("actual_ticket_shape"))?
                .remove(field);
        }
        check(
            old == new,
            "mutable_ticket_changed_immutable_decision_inputs",
        )
    }

    fn current_root_overlay(
        actual: &fv::RootBudgetSnapshotV2,
        frozen: &fv::RootBudgetSnapshotV2,
    ) -> Result<()> {
        if frozen.stopped {
            check(
                actual.stopped
                    && actual.stop_reason == frozen.stop_reason
                    && actual.stop_committed_at == frozen.stop_committed_at,
                "root_first_stop_changed",
            )?;
        }
        let mut old = serde_json::to_value(frozen).map_err(crate::internal)?;
        let mut new = serde_json::to_value(actual).map_err(crate::internal)?;
        // Paid totals and a later first close/stop are overlay facts. They never
        // fill or replace the original values used by the completed report.
        for field in [
            "spent_micros",
            "reserved_micros",
            "stopped",
            "stop_reason",
            "stop_committed_at",
        ] {
            old.as_object_mut()
                .ok_or_else(|| invalid("frozen_root_shape"))?
                .remove(field);
            new.as_object_mut()
                .ok_or_else(|| invalid("actual_root_shape"))?
                .remove(field);
        }
        check(old == new, "root_authorization_identity_changed")
    }

    fn current_exposure_overlay(
        actual: &fv::ExposureProjectionV2,
        frozen: &fv::ExposureProjectionV2,
    ) -> Result<()> {
        let mut old = serde_json::to_value(frozen).map_err(crate::internal)?;
        let mut new = serde_json::to_value(actual).map_err(crate::internal)?;
        // The original entry/first feedback stays immutable. Only monetary
        // reconciliation is a later overlay; other tickets are not in either
        // projection and cannot perturb this report.
        old.get_mut("entry")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| invalid("frozen_exposure_shape"))?
            .remove("monetary_state");
        new.get_mut("entry")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| invalid("actual_exposure_shape"))?
            .remove("monetary_state");
        check(old == new, "exposure_original_entry_changed")
    }

    fn inspect_native_exposure_capture_r2(
        actual: &OccurrenceSnapshot,
        carrier: &FrozenCarrierR36,
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<()> {
        let ticket = &carrier.input.ticket_snapshot;
        let ns = carrier.input.namespace.as_str();
        let physical = evaluation_storage_id(LEDGER_KIND, &ticket.research_family_id)?;
        let saved = carrier.input.native_exposure_capture.as_ref();
        if let Some(saved) = saved {
            check(
                saved.schema_version == "rsia.e05.native_exposure_capture.v1"
                    && saved.namespace == ns
                    && saved.physical_id == physical
                    && saved.ledger_schema_version == evo_core::holdout::EXPOSURE_LEDGER_SCHEMA,
                "native_exposure_known_source_descriptor",
            )?;
            identifier(&saved.original_owner)?;
            for value in [
                &saved.raw_row_digest,
                &saved.raw_body_digest,
                &saved.typed_ledger_digest,
                &saved.projection_digest,
            ] {
                digest(value)?;
            }
            if let Some(original) = &carrier.input.exposure {
                check(
                    saved.projection_digest == typed_fingerprint(&original.snapshot)?,
                    "native_exposure_frozen_projection_binding",
                )?;
            }
        } else {
            gaps.insert("native_exposure_original_capture_missing");
        }
        let key = (ns.to_owned(), "artifact".to_owned(), physical.clone());
        let Some(object) = actual.objects.get(&key) else {
            gaps.insert("actual_shared_exposure_source_missing");
            return Ok(());
        };
        if object.body.get("schema_version").and_then(Value::as_str) == Some("rsia.redacted.v1") {
            qualify_redacted(object)?;
            if let Some(saved) = saved {
                check(
                    object.body.get("original_digest").and_then(Value::as_str)
                        == Some(saved.raw_body_digest.as_str())
                        && object.body.get("original_schema").and_then(Value::as_str)
                            == Some("rsia.typed_artifact_envelope.v1"),
                    "native_exposure_redaction_source_binding",
                )?;
            }
            gaps.insert("actual_shared_exposure_original_redacted");
            return Ok(());
        }
        let ledger: ExposureLedger = decode(envelope(
            object,
            "rsia.typed_artifact_envelope.v1",
            LEDGER_KIND,
        )?)?;
        ledger.validate_restore()?;
        check(
            object.namespace == ns
                && object.kind == "artifact"
                && object.id == physical
                && ledger.research_family_id == ticket.research_family_id
                && ledger.alpha_plan_digest == ticket.alpha_plan_digest,
            "current_shared_exposure_identity",
        )?;
        if let Some(revoked) =
            actual
                .objects
                .get(&(ns.to_owned(), "tombstone".to_owned(), physical))
            && revoked.body.get("source_kind").and_then(Value::as_str) == Some("artifact")
        {
            let mut floor = OccurrenceFloor::default();
            qualify_tombstone(revoked, actual, &mut floor)?;
            gaps.insert("actual_shared_exposure_source_revoked");
        }
        if let Some(entry) = ledger
            .entries
            .iter()
            .find(|entry| entry.ticket_id == ticket.id)
        {
            let projection = fv::ExposureProjectionV2 {
                schema_version: ledger.schema_version.clone(),
                research_family_id: ledger.research_family_id.clone(),
                alpha_plan_digest: ledger.alpha_plan_digest.clone(),
                query_limit: ledger.query_limit,
                entry: entry.clone(),
            };
            if let Some(original) = &carrier.input.exposure {
                current_exposure_overlay(&projection, &original.snapshot)?;
            }
        } else {
            gaps.insert("current_original_exposure_entry_missing");
        }
        // Later shared ownership/body/revision and other tickets are not an
        // old full-ledger preimage. Original source owner/raw/typed digests and
        // this projection were captured by the real writer and exact asof/
        // INSERT-ACK binding; they are never reconstructed from this later row.
        Ok(())
    }

    fn inspect_mutable_and_immutable_facts(
        raw: &RawDatabase,
        actual: &OccurrenceSnapshot,
        carrier: &FrozenCarrierR36,
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<()> {
        inspect_native_mutable_projection_shape_r39(carrier, gaps)?;
        let ns = carrier.input.namespace.as_str();
        let ticket = &carrier.input.ticket_snapshot;
        let mut raw_by_key = BTreeMap::new();
        for row in raw
            .tables
            .get("objects")
            .ok_or_else(|| invalid("objects_missing"))?
        {
            let key = (
                native_text(row, "namespace")?,
                native_text(row, "kind")?,
                native_text(row, "id")?,
            );
            check(
                raw_by_key.insert(key, row).is_none(),
                "duplicate_native_object",
            )?;
        }
        for fact in &carrier.input.facts {
            let key = (fact.namespace.clone(), fact.kind.clone(), fact.id.clone());
            match (actual.objects.get(&key), raw_by_key.get(&key)) {
                (Some(object), Some(row)) => {
                    check(
                        object.namespace == ns
                            && object.kind == fact.kind
                            && object.id == fact.id
                            && object.owner == fact.owner
                            && native_integer(row, "revision")? > 0,
                        "actual_source_identity_owner_or_revision",
                    )?;
                    if object.body.get("schema_version").and_then(Value::as_str)
                        == Some("rsia.redacted.v1")
                    {
                        qualify_redacted(object)?;
                        check(
                            object.body.get("original_digest").and_then(Value::as_str)
                                == Some(fact.raw_body_digest.as_str())
                                && object.body.get("original_schema").and_then(Value::as_str)
                                    == Some(fact.schema_version.as_str()),
                            "redacted_original_identity_or_digest",
                        )?;
                        gaps.insert("retained_original_body_redacted");
                        continue;
                    }
                    let revoked = actual
                        .objects
                        .get(&(fact.namespace.clone(), "tombstone".into(), fact.id.clone()))
                        .filter(|tombstone| {
                            tombstone.body.get("source_kind").and_then(Value::as_str)
                                == Some(fact.kind.as_str())
                        });
                    if let Some(tombstone) = revoked {
                        let mut floor = OccurrenceFloor::default();
                        qualify_tombstone(tombstone, actual, &mut floor)?;
                        gaps.insert("original_source_revoked");
                    }
                    let projected = carrier
                        .input
                        .mutable_fact_projections
                        .iter()
                        .find(|saved| saved.kind == fact.kind && saved.id == fact.id);
                    let projection_kind = if fact.kind == "artifact"
                        && fact.id == evaluation_storage_id(TICKET_KIND, &ticket.id)?
                    {
                        Some(fv::NativeMutableProjectionKindV2::TicketSnapshot)
                    } else if fact.kind == "artifact"
                        && fact.id
                            == evaluation_storage_id(LEDGER_KIND, &ticket.research_family_id)?
                    {
                        Some(fv::NativeMutableProjectionKindV2::TicketExposure)
                    } else {
                        None
                    };
                    if let Some(projection_kind) = projection_kind {
                        // This is a native-authenticated VALUE projection, not
                        // a claim that the whole old shared body was retained.
                        if let Some(projected) = projected {
                            check(
                                projected.projection == projection_kind
                                    && projected.original_row_digest == fact.raw_row_digest
                                    && projected.original_body_digest == fact.raw_body_digest
                                    && projected.original_typed_digest == fact.typed_record_digest,
                                "projection_original_descriptor",
                            )?;
                        } else {
                            gaps.insert("frozen_native_mutable_projection_missing");
                        }
                        match projection_kind {
                            fv::NativeMutableProjectionKindV2::TicketSnapshot => {
                                let latest: MirrorEvaluationTicketV2 = decode(envelope(
                                    object,
                                    "rsia.typed_artifact_envelope.v1",
                                    TICKET_KIND,
                                )?)?;
                                current_ticket_overlay(&latest, ticket)?;
                                if let Some(projected) = projected {
                                    check(
                                        projected.projection_digest == typed_fingerprint(ticket)?,
                                        "frozen_ticket_projection_digest",
                                    )?;
                                }
                            }
                            fv::NativeMutableProjectionKindV2::TicketExposure => {
                                if let Some(saved) = &carrier.input.exposure {
                                    if let Some(projected) = projected {
                                        check(
                                            projected.projection_digest
                                                == typed_fingerprint(&saved.snapshot)?,
                                            "frozen_exposure_projection_digest",
                                        )?;
                                    }
                                    if let Some(latest) =
                                        exposure_projection_r36(actual, ns, ticket)?
                                    {
                                        current_exposure_overlay(&latest, &saved.snapshot)?;
                                    } else {
                                        gaps.insert("current_original_exposure_entry_missing");
                                    }
                                } else {
                                    gaps.insert("frozen_exposure_original_missing");
                                }
                            }
                        }
                        let latest_edges = e05_outgoing(actual, ns, &fact.kind, &fact.id)?;
                        // Legitimate late edges/other ticket appends are allowed
                        // only on these explicit mutable sources. Original
                        // references still have to exist in the actual graph.
                        for edge in &fact.outgoing {
                            if !latest_edges.contains(edge) {
                                gaps.insert("original_mutable_edge_missing");
                            }
                        }
                    } else {
                        let (schema_version, record_kind, logical_id, content_digest) =
                            frozen_complete_kernel_r29::known_dependency_identity_r3(
                                actual, object,
                            )?;
                        let outgoing = e05_outgoing(actual, ns, &fact.kind, &fact.id)?;
                        let actual_node = MirrorFormalDependencyNode {
                            kind: fact.kind.clone(),
                            id: fact.id.clone(),
                            schema_version,
                            record_kind,
                            logical_id,
                            content_digest,
                            outgoing_edge_digest: typed_fingerprint(&outgoing)?,
                            outgoing_edge_count: outgoing.len(),
                        };
                        check(
                            hash(native_text(row, "body")?.as_bytes()) == fact.raw_body_digest
                                && actual_node.schema_version == fact.schema_version
                                && actual_node.record_kind == fact.record_kind
                                && actual_node.logical_id == fact.logical_id
                                && actual_node.content_digest == fact.typed_record_digest
                                && actual_node.outgoing_edge_digest == fact.outgoing_digest
                                && actual_node.outgoing_edge_count == fact.outgoing.len()
                                && e05_outgoing(actual, ns, &fact.kind, &fact.id)? == fact.outgoing,
                            "retained_immutable_body_typed_or_edges",
                        )?;
                        // The current row may have an idempotent same-body
                        // revision. We authenticate the original row fingerprint
                        // through its native binding, not by relabeling this row
                        // as an old byte-for-byte row or fabricating metadata.
                    }
                }
                (None, None) => {
                    gaps.insert("retained_original_fact_missing");
                }
                _ => return Err(invalid("actual_raw_snapshot_disagreement")),
            }
        }
        Ok(())
    }

    fn record_available<T: DeserializeOwned>(
        actual: &OccurrenceSnapshot,
        ns: &str,
        kind: &str,
        id: &str,
        gaps: &mut BTreeSet<&'static str>,
        gap: &'static str,
    ) -> Result<Option<T>> {
        let key = (
            ns.to_owned(),
            "artifact".to_owned(),
            evaluation_storage_id(kind, id)?,
        );
        if let Some(object) = actual.objects.get(&key) {
            if object.body.get("schema_version").and_then(Value::as_str) == Some("rsia.redacted.v1")
            {
                qualify_redacted(object)?;
                gaps.insert(gap);
                Ok(None)
            } else {
                evaluation_record(actual, ns, kind, id).map(Some)
            }
        } else {
            gaps.insert(gap);
            Ok(None)
        }
    }

    fn verify_complete_payload_r2(
        actual: &OccurrenceSnapshot,
        carrier: &FrozenCarrierR36,
        ctx: &Context,
        control: Option<&MirrorRegisteredEvaluationControl>,
        holdout: Option<&MirrorProtectedHoldoutRecord>,
        expected_nodes: &[MirrorFormalDependencyNode],
        math_calls: &[E05MathCallR39],
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<()> {
        use frozen_complete_kernel_r29 as kernel;
        let ticket = &carrier.input.ticket_snapshot;
        let targets_available =
            inspect_target_rows_r39(actual, ctx.namespace(), ticket, control, holdout, gaps)?;
        let resource: Option<MirrorResourceEvidenceRecord> = record_available(
            actual,
            ctx.namespace(),
            RESOURCE_KIND,
            &ticket.id,
            gaps,
            "retained_resource_evidence_missing",
        )?;
        let MirrorFormalEvaluationV2::CompleteBatch {
            cost_evidence_scope,
            report,
            stored_dependency_binding,
            resource_evidence_digest,
            anchor_evidence_digest,
            ..
        } = &carrier.payload
        else {
            return Err(invalid("Current_complete_reader_requires_complete_variant"));
        };
        if let Some(resource) = &resource {
            check(
                resource.schema_version == "rsia.evaluation_resource_evidence.v1"
                    && resource.ticket_id == ticket.id
                    && resource.cost_scope == MirrorCostEvidenceScope::TicketExecutionOnly
                    && *cost_evidence_scope == resource.cost_scope
                    && typed_fingerprint(resource)? == *resource_evidence_digest
                    && resource.calls.len() == carrier.input.calls.len(),
                "retained_resource_identity_or_digest",
            )?;
            if let Some(control) = &control {
                check(
                    resource.billing_scope == control.billing_scope,
                    "resource_control_scope",
                )?;
            }
            let by_id: BTreeMap<_, _> = carrier
                .input
                .calls
                .iter()
                .map(|saved| {
                    (
                        (
                            saved.namespace.as_str(),
                            saved.billing_scope.as_str(),
                            saved.call_id.as_str(),
                        ),
                        saved,
                    )
                })
                .collect();
            let mut seen = BTreeSet::new();
            for call in &resource.calls {
                let key = (
                    call.namespace.as_str(),
                    call.billing_scope.as_str(),
                    call.call_id.as_str(),
                );
                check(seen.insert(key), "resource_duplicate_call")?;
                let saved = by_id
                    .get(&key)
                    .ok_or_else(|| invalid("resource_extra_call"))?;
                saved
                    .snapshot
                    .validate_original_copy_r36(call)
                    .map_err(frozen_call_error_r24)?;
            }
        }
        let expected_binding = match (
            &ticket.candidate_dependency_binding,
            stored_dependency_binding,
        ) {
            (Some(candidate), Some(stored)) => {
                let expected = MirrorFormalStoredDependencyBinding {
                    schema_version: "rsia.formal_stored_dependency_binding.v1".into(),
                    namespace: ctx.namespace().into(),
                    candidate_binding_digest: typed_fingerprint(candidate)?,
                    ticket_id: ticket.id.clone(),
                    ticket_physical_id: evaluation_storage_id(TICKET_KIND, &ticket.id)?,
                    ticket_digest: typed_fingerprint(ticket)?,
                    resource_physical_id: evaluation_storage_id(RESOURCE_KIND, &ticket.id)?,
                    nodes: expected_nodes.to_vec(),
                };
                check(stored == &expected, "stored_binding_original_frozen_graph")?;
                Some(expected)
            }
            (None, None) => None,
            _ => return Err(invalid("mixed_candidate_and_stored_binding")),
        };
        let anchors: Vec<_> = ticket
            .targets
            .iter()
            .filter(|target| matches!(&target.kind, MirrorEvaluationTargetKind::Anchor { .. }))
            .collect();
        check(
            typed_fingerprint(&anchors)? == *anchor_evidence_digest,
            "original_anchor_digest",
        )?;
        if targets_available
            && let (Some(control), Some(holdout), Some(exposure), Some(resource)) =
                (control, holdout, &carrier.input.exposure, &resource)
        {
            let records = FrozenRecords {
                actual: actual,
                facts: &carrier.input.facts,
                exposure: &exposure.snapshot,
            };
            // This calls the SAME real-output oracle/grader, first-stop replay,
            // full fixed-sample statistics and resource ratios as the producer.
            let expected_report = e05_recompute_complete_report_r39(
                &records,
                ctx.namespace(),
                ticket,
                control,
                holdout,
                math_calls,
            )?;
            kernel::verify_report(report, &expected_report)?;
            let expected = MirrorFormalEvaluationV2::CompleteBatch {
                evidence_scope: ticket.evidence_scope,
                cost_evidence_scope: MirrorCostEvidenceScope::TicketExecutionOnly,
                ticket_id: ticket.id.clone(),
                ticket_digest: typed_fingerprint(ticket)?,
                manifest_digest: ticket.manifest_digest.clone(),
                candidate_bundle_digest: ticket.candidate_bundle_digest.clone(),
                baseline_bundle_digest: ticket.baseline_bundle_digest.clone(),
                environment_digest: ticket.environment_digest.clone(),
                verdict: expected_report.verdict,
                report: expected_report,
                anchor_evidence_digest: typed_fingerprint(&anchors)?,
                resource_evidence_digest: typed_fingerprint(resource)?,
                stored_dependency_binding: expected_binding,
            };
            check(
                typed_fingerprint(&expected)? == typed_fingerprint(&carrier.payload)?,
                "all_recomputed_Formal_payload_fields",
            )?;
        }
        Ok(())
    }

    fn verify_invalid_complete_request_r2(
        actual: &OccurrenceSnapshot,
        carrier: &FrozenCarrierR36,
        ctx: &Context,
        control: Option<&MirrorRegisteredEvaluationControl>,
        holdout: Option<&MirrorProtectedHoldoutRecord>,
        math_calls: &[E05MathCallR39],
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<()> {
        let ticket = &carrier.input.ticket_snapshot;
        let MirrorFormalEvaluationV2::Invalid {
            evidence_scope,
            ticket_id,
            ticket_digest,
            reasons,
        } = &carrier.payload
        else {
            return Err(invalid("invalid_variant"));
        };
        check(
            matches!(&carrier.input.operation, fv::Operation::Invalidate)
                && ticket.lifecycle == MirrorTicketLifecycle::Invalid
                && ticket_id == &ticket.id
                && ticket_digest == &typed_fingerprint(ticket)?
                && evidence_scope == &ticket.evidence_scope,
            "invalid_known_identity",
        )?;
        // The existing engine4 producer materializes precisely this first
        // failure of a real Complete request. Grade/invalidate legacy records
        // remain old bytes and are never guessed into this decision family.
        check(
            reasons == &vec!["unattributed_dispatch_group_call".to_owned()]
                && ticket.invalid_reasons == *reasons,
            "invalid_original_first_failure_reason",
        )?;
        let mut records_ready =
            inspect_target_rows_r39(actual, ctx.namespace(), ticket, control, holdout, gaps)?;
        let by_id: BTreeMap<_, _> = math_calls
            .iter()
            .map(|call| (call.call_id.as_str(), call))
            .collect();
        check(
            by_id.len() == math_calls.len(),
            "invalid_duplicate_original_group_call",
        )?;
        let mut expected = BTreeSet::new();
        let mut receipts_complete = true;
        for target in &ticket.targets {
            let MirrorTargetState::Completed {
                candidate_receipt_id,
                baseline_receipt_id,
                ..
            } = &target.state
            else {
                return Err(invalid("invalid_complete_request_not_ready"));
            };
            for receipt_id in [candidate_receipt_id, baseline_receipt_id] {
                let receipt: Option<MirrorExecutionReceiptV2> = record_available(
                    actual,
                    ctx.namespace(),
                    EXECUTION_RECEIPT_KIND,
                    receipt_id,
                    gaps,
                    "invalid_original_execution_receipt_missing",
                )?;
                let Some(receipt) = receipt else {
                    receipts_complete = false;
                    continue;
                };
                check(
                    expected.insert(receipt.budget_call_id.clone()),
                    "invalid_reused_complete_request_call",
                )?;
                let call = by_id
                    .get(receipt.budget_call_id.as_str())
                    .ok_or_else(|| invalid("invalid_expected_call_absent_from_frozen_group"))?;
                check(
                    call.namespace == ctx.namespace()
                        && call.dispatch_group_id == ticket.id
                        && call.state == BudgetCallState::Finalized
                        && call.actual_cost_micros.is_some()
                        && call.execution_closed
                        && call.actual_input_digest == receipt.request_digest
                        && call.output_digest.as_deref() == Some(receipt.output_digest.as_str()),
                    "invalid_original_complete_preconditions",
                )?;
                if let Some(control) = control {
                    check(
                        call.billing_scope == control.billing_scope,
                        "invalid_original_control_scope",
                    )?;
                }
            }
        }
        if let (Some(control), Some(holdout), Some(exposure)) =
            (control, holdout, &carrier.input.exposure)
        {
            let source = FrozenRecords {
                actual,
                facts: &carrier.input.facts,
                exposure: &exposure.snapshot,
            };
            if records_ready {
                // Exactly the producer's before-first-failure order: original
                // outputs/oracle are graded before the whole-group difference.
                for target in &ticket.targets {
                    e05_completed_target_evidence_r39(
                        &source,
                        ctx.namespace(),
                        ticket,
                        control,
                        holdout,
                        target,
                    )?;
                }
            }
        } else {
            records_ready = false;
        }
        if receipts_complete {
            let actual_ids: BTreeSet<_> =
                math_calls.iter().map(|call| call.call_id.clone()).collect();
            check(
                expected.is_subset(&actual_ids),
                "invalid_missing_expected_original_call",
            )?;
            let extras: BTreeSet<_> = actual_ids.difference(&expected).cloned().collect();
            let saved: BTreeSet<_> = ticket
                .unattributed_budget_call_ids
                .iter()
                .cloned()
                .collect();
            check(
                !extras.is_empty()
                    && extras == saved
                    && saved.len() == ticket.unattributed_budget_call_ids.len(),
                "invalid_unattributed_actual_whole_group_difference",
            )?;
        }
        if let Some(group) = &carrier.input.dispatch_group {
            check(
                group.snapshot.stop_reason.as_deref() == Some("unattributed_dispatch_group_call"),
                "invalid_original_first_group_stop",
            )?;
        }
        if !records_ready {
            gaps.insert("invalid_original_output_or_oracle_sources_missing");
        }
        // The entire actual typed payload, including the diagnostic reason
        // order, is independently constructed from this proven first failure.
        let expected = MirrorFormalEvaluationV2::Invalid {
            evidence_scope: ticket.evidence_scope,
            ticket_id: ticket.id.clone(),
            ticket_digest: typed_fingerprint(ticket)?,
            reasons: vec!["unattributed_dispatch_group_call".to_owned()],
        };
        check(
            typed_fingerprint(&expected)? == typed_fingerprint(&carrier.payload)?,
            "invalid_full_rederived_payload",
        )
    }

    fn verify_early_payload_r2(
        raw: &RawDatabase,
        actual: &OccurrenceSnapshot,
        carrier: &FrozenCarrierR36,
        ctx: &Context,
        control: Option<&MirrorRegisteredEvaluationControl>,
        holdout: Option<&MirrorProtectedHoldoutRecord>,
        math_calls: &[E05MathCallR39],
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<()> {
        let ticket = &carrier.input.ticket_snapshot;
        let MirrorFormalEvaluationV2::EarlyStopped {
            evidence_scope,
            ticket_id,
            ticket_digest,
            certificate,
        } = &carrier.payload
        else {
            return Err(invalid("early_variant"));
        };
        verify_actual_first_stop_r44(
            raw,
            ctx.namespace(),
            ticket,
            carrier.input.native_first_stop.as_ref(),
            gaps,
        )?;
        certificate.validate_shape()?;
        check(
            ticket_id == &ticket.id
                && evidence_scope == &ticket.evidence_scope
                && ticket_digest == &typed_fingerprint(ticket)?
                && ticket.lifecycle == MirrorTicketLifecycle::EarlyStopped,
            "early_frozen_identity",
        )?;
        let sources_ready = inspect_target_rows_variant_r2(
            actual,
            ctx.namespace(),
            ticket,
            control,
            holdout,
            gaps,
            true,
        )?;
        if sources_ready
            && let (Some(control), Some(holdout), Some(exposure)) =
                (control, holdout, &carrier.input.exposure)
        {
            let source = FrozenRecords {
                actual,
                facts: &carrier.input.facts,
                exposure: &exposure.snapshot,
            };
            // This is the original full bound-certificate reducer, including
            // actual-output regrading, all terminal/cancelled calls, first
            // feedback, exact ordered first stop, frozen member-terminal digest,
            // issuer receipt, exact confidence bounds and all certificate fields.
            e05_native_early_certificate_r44(
                &source,
                ctx.namespace(),
                ticket,
                control,
                holdout,
                certificate,
                math_calls,
            )?;
        } else {
            gaps.insert("early_original_stop_replay_source_missing");
        }
        Ok(())
    }

    // The Missing variant contains no authenticated value, no native seal and no
    // authority. It exists only to finish every known pure check before choosing
    // a missing-source error; the normal reader still rejects it exactly.
    enum NativeFrozenInspectionR44 {
        Missing(BTreeSet<&'static str>),
        Ready(Box<AuthenticatedFrozenInputs>),
    }

    fn inspect_authenticated_r44(
        raw: &RawDatabase,
        actual: &OccurrenceSnapshot,
        ctx: &Context,
        reference: &fv::ReportRef,
    ) -> Result<NativeFrozenInspectionR44> {
        use frozen_complete_kernel_r29 as kernel;
        reference.validate_wire().map_err(r24_fault)?;
        check(
            reference.namespace == ctx.namespace(),
            "requested_namespace",
        )?;
        check(
            reference.engine_version == fv::ENGINE_V4
                && reference.input_schema_version == fv::INPUT_SCHEMA_V2,
            "historical_versions_cannot_become_Current",
        )?;
        // This decoder is all-known only. No missing source can conceal an
        // owner/typed-body/reference mismatch before the rest is inspected.
        let (carrier, _) = decode_complete_known_raw_r36(raw, reference)?;
        let ticket = &carrier.input.ticket_snapshot;
        check(
            reference.owner == ticket.evaluator_actor,
            "actual_formal_owner",
        )?;
        let mut gaps = BTreeSet::new();
        if let Some(tombstone) = actual.objects.get(&(
            reference.namespace.clone(),
            "tombstone".into(),
            reference.physical_id.clone(),
        )) && tombstone.body.get("source_kind").and_then(Value::as_str) == Some("artifact")
        {
            let mut floor = OccurrenceFloor::default();
            qualify_tombstone(tombstone, actual, &mut floor)?;
            gaps.insert("formal_report_itself_revoked");
        }
        // Presence is not authority: load_complete_origins_r24, invoked by the
        // actual snapshot builder, validates original INSERT row + role/actor +
        // closed prefix + exact asof and binding audit entities/seq/prev chain.
        if let Some(origin) = actual
            .complete_origins_r24
            .get(&(reference.namespace.clone(), reference.physical_id.clone()))
        {
            check(
                origin.reference == *reference,
                "actual_native_asof_binding_reference",
            )?;
        } else {
            gaps.insert("actual_native_asof_and_INSERT_binding_missing");
        }

        validate_complete_known_r36(&carrier, ctx.namespace(), &reference.physical_id)?;
        inspect_mutable_and_immutable_facts(raw, actual, &carrier, &mut gaps)?;
        inspect_native_exposure_capture_r2(actual, &carrier, &mut gaps)?;
        let expected_nodes = kernel::frozen_graph(&carrier)?;
        for (kind, id) in [
            (
                "registered_evaluation_control_v41",
                ticket.registration_id.as_str(),
            ),
            (HOLDOUT_KIND, ticket.holdout_id.as_str()),
        ] {
            let physical = evaluation_storage_id(kind, id)?;
            check(
                carrier.input.facts.iter().any(|fact| {
                    fact.namespace == ctx.namespace()
                        && fact.kind == "artifact"
                        && fact.id == physical
                }),
                "required_original_record_not_in_frozen_fact_graph",
            )?;
        }
        if matches!(
            &carrier.payload,
            MirrorFormalEvaluationV2::CompleteBatch { .. }
        ) {
            let physical = evaluation_storage_id(RESOURCE_KIND, &ticket.id)?;
            check(
                carrier.input.facts.iter().any(|fact| {
                    fact.namespace == ctx.namespace()
                        && fact.kind == "artifact"
                        && fact.id == physical
                }),
                "complete_required_original_resource_fact",
            )?;
        }
        inspect_actual_compiler_authorities(actual, &carrier, &mut gaps)?;
        inspect_candidate_r39(actual, &carrier, &mut gaps)?;
        let (calls, call_gaps) = kernel::reconstruct_calls(actual, &carrier.input.calls)?;
        let math_calls: Vec<_> = carrier
            .input
            .calls
            .iter()
            .map(|saved| {
                saved
                    .snapshot
                    .math_projection_r39()
                    .map_err(frozen_call_error_r24)
            })
            .collect::<Result<Vec<_>>>()?;
        gaps.extend(call_gaps);
        let control: Option<MirrorRegisteredEvaluationControl> = record_available(
            actual,
            ctx.namespace(),
            "registered_evaluation_control_v41",
            &ticket.registration_id,
            &mut gaps,
            "original_registered_control_missing",
        )?;
        if let Some(control) = &control {
            control.validate()?;
            known_ticket_control_r36(ticket, control)?;
        }
        let holdout: Option<MirrorProtectedHoldoutRecord> = record_available(
            actual,
            ctx.namespace(),
            HOLDOUT_KIND,
            &ticket.holdout_id,
            &mut gaps,
            "original_holdout_missing",
        )?;
        if let Some(holdout) = &holdout {
            known_ticket_holdout_r36(ticket, holdout)?;
            if let Some(control) = &control {
                holdout.validate_against(control)?;
            }
        }
        let original_ids: BTreeSet<_> = carrier
            .input
            .calls
            .iter()
            .map(|saved| {
                (
                    saved.namespace.clone(),
                    saved.billing_scope.clone(),
                    saved.call_id.clone(),
                )
            })
            .collect();
        kernel::reject_known_group_extras_r1(
            actual,
            ctx.namespace(),
            &ticket.id,
            control.as_ref().map(|c| c.billing_scope.as_str()),
            &original_ids,
            &mut gaps,
        )?;
        if let Some(saved) = &carrier.input.root_budget {
            if let Some(latest) = actual.roots.get(&saved.snapshot.billing_scope) {
                current_root_overlay(&root_projection_r36(latest), &saved.snapshot)?;
            } else {
                gaps.insert("current_root_original_missing");
            }
        } else {
            gaps.insert("frozen_root_original_missing");
        }
        if let Some(saved) = &carrier.input.dispatch_group {
            if let Some(latest) = actual.groups.get(&(
                saved.snapshot.billing_scope.clone(),
                saved.snapshot.dispatch_group_id.clone(),
            )) {
                check(
                    group_projection_r36(latest) == saved.snapshot,
                    "group_identity_or_first_stop_changed",
                )?;
            } else {
                gaps.insert("current_group_original_missing");
            }
        } else {
            gaps.insert("frozen_group_original_missing");
        }
        if carrier.input.exposure.is_none() {
            gaps.insert("frozen_exposure_original_missing");
        }

        // Issue receipt lookup is deliberately after independent known
        // ticket/control/holdout validation above. Missing does not silence it.
        let issue_kind = ticket_issue_receipt_kind_r39(
            actual,
            ctx.namespace(),
            ticket,
            control.as_ref(),
            holdout.as_ref(),
            &mut gaps,
        )?;
        let mut producer_witnesses_ready = true;
        for (kind, id) in [
            (
                "registered_evaluation_control_v41",
                ticket.registration_id.as_str(),
            ),
            (HOLDOUT_KIND, ticket.holdout_id.as_str()),
        ] {
            if let Some(object) = actual.objects.get(&(
                ctx.namespace().to_owned(),
                "artifact".to_owned(),
                evaluation_storage_id(kind, id)?,
            )) {
                if object.body.get("schema_version").and_then(Value::as_str)
                    == Some("rsia.redacted.v1")
                {
                    producer_witnesses_ready = false;
                } else if !actual_evaluator_witness_r39(actual, object, &mut gaps)? {
                    producer_witnesses_ready = false;
                }
            } else {
                producer_witnesses_ready = false;
            }
        }
        if issue_kind.is_some()
            && producer_witnesses_ready
            && let (Some(control), Some(holdout)) = (&control, &holdout)
        {
            producer_witnesses(actual, ctx.namespace(), control, holdout)?;
            e05_complete_ticket_identity(actual, ctx.namespace(), ticket, control, holdout)?;
        }
        let verified_variant = match &carrier.payload {
            MirrorFormalEvaluationV2::CompleteBatch { .. } => {
                verify_complete_payload_r2(
                    actual,
                    &carrier,
                    ctx,
                    control.as_ref(),
                    holdout.as_ref(),
                    &expected_nodes,
                    &math_calls,
                    &mut gaps,
                )?;
                VerifiedFormalVariantR2::Complete
            }
            MirrorFormalEvaluationV2::Invalid { .. } => {
                verify_invalid_complete_request_r2(
                    actual,
                    &carrier,
                    ctx,
                    control.as_ref(),
                    holdout.as_ref(),
                    &math_calls,
                    &mut gaps,
                )?;
                VerifiedFormalVariantR2::Invalid
            }
            MirrorFormalEvaluationV2::EarlyStopped { .. } => {
                verify_early_payload_r2(
                    raw,
                    actual,
                    &carrier,
                    ctx,
                    control.as_ref(),
                    holdout.as_ref(),
                    &math_calls,
                    &mut gaps,
                )?;
                VerifiedFormalVariantR2::EarlyStopped
            }
        };
        // The final native source gate comes AFTER every available-known check
        // and all possible recomputation. Nothing gets a success capability on
        // a missing audit, projection, original body, slot, root or permission.
        if !gaps.is_empty() {
            return Ok(NativeFrozenInspectionR44::Missing(gaps));
        }
        let frozen_root = carrier
            .input
            .root_budget
            .as_ref()
            .ok_or_else(|| invalid("closed_root_invariant"))?
            .snapshot
            .clone();
        let frozen_group = carrier
            .input
            .dispatch_group
            .as_ref()
            .ok_or_else(|| invalid("closed_group_invariant"))?
            .snapshot
            .clone();
        let frozen_exposure = carrier
            .input
            .exposure
            .as_ref()
            .ok_or_else(|| invalid("closed_exposure_invariant"))?
            .snapshot
            .clone();
        let paid_overlay = carrier
            .input
            .calls
            .iter()
            .map(|saved| {
                actual
                    .calls
                    .get(&(saved.billing_scope.clone(), saved.call_id.clone()))
                    .cloned()
                    .ok_or_else(|| invalid("closed_call_overlay_invariant"))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(NativeFrozenInspectionR44::Ready(Box::new(
            AuthenticatedFrozenInputs {
                reference: reference.clone(),
                payload: serde_json::to_value(&carrier.payload).map_err(crate::internal)?,
                frozen_ticket: ticket.clone(),
                frozen_root,
                frozen_group,
                frozen_exposure,
                original_calls: calls,
                paid_overlay,
                verified_variant,
            },
        )))
    }

    pub(super) fn authenticate(
        raw: &RawDatabase,
        actual: &OccurrenceSnapshot,
        ctx: &Context,
        reference: &fv::ReportRef,
    ) -> Result<AuthenticatedFrozenInputs> {
        match inspect_authenticated_r44(raw, actual, ctx, reference)? {
            NativeFrozenInspectionR44::Ready(authenticated) => Ok(*authenticated),
            NativeFrozenInspectionR44::Missing(gaps) => Err(missing(&gaps)),
        }
    }

    pub(super) fn inspect_known_early_before_source_gate_r44(
        raw: &RawDatabase,
        actual: &OccurrenceSnapshot,
        ctx: &Context,
        reference: &fv::ReportRef,
    ) -> Result<()> {
        // Unit means only "no known contradiction"; it is never an as-of,
        // binding, qualified-floor, close or Current proof. The native caller
        // must still enforce the original floor, all source gaps, real ACKs and
        // full authenticate before COMMIT admission.
        match inspect_authenticated_r44(raw, actual, ctx, reference)? {
            NativeFrozenInspectionR44::Ready(_) | NativeFrozenInspectionR44::Missing(_) => Ok(()),
        }
    }

    fn actual_evaluator_witness_r39(
        actual: &OccurrenceSnapshot,
        object: &OccurrenceObject,
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<bool> {
        role_witness_available_r2(actual, object, "evaluator", gaps)
    }

    fn ticket_issue_receipt_kind_r39(
        actual: &OccurrenceSnapshot,
        namespace: &str,
        ticket: &MirrorEvaluationTicketV2,
        control: Option<&MirrorRegisteredEvaluationControl>,
        holdout: Option<&MirrorProtectedHoldoutRecord>,
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<Option<()>> {
        let mut count = 0;
        let mut typed_ready = false;
        for (kind, schema) in [
            (
                "evaluation_ticket_issue_receipt_v1",
                "rsia.evaluation_ticket_issue_receipt.v1",
            ),
            (
                "evaluation_ticket_issue_receipt_v2",
                "rsia.evaluation_ticket_issue_receipt.v2",
            ),
        ] {
            let key = (
                namespace.to_owned(),
                "artifact".to_owned(),
                evaluation_storage_id(kind, &ticket.id)?,
            );
            let Some(object) = actual.objects.get(&key) else {
                continue;
            };
            count += 1;
            check(count <= 1, "ambiguous_original_issue_receipt_protocol")?;
            if object.body.get("schema_version").and_then(Value::as_str) == Some("rsia.redacted.v1")
            {
                qualify_redacted(object)?;
                check(
                    object.body.get("original_kind").and_then(Value::as_str) == Some("artifact")
                        && object.body.get("original_schema").and_then(Value::as_str)
                            == Some("rsia.typed_artifact_envelope.v1"),
                    "redacted_issue_original_shape",
                )?;
                gaps.insert("retained_issue_receipt_redacted");
                continue;
            }
            let payload = envelope(object, "rsia.typed_artifact_envelope.v1", kind)?;
            e05_dependency_payload_identity(kind, payload)?;
            let identity = if kind == "evaluation_ticket_issue_receipt_v1" {
                let value: MirrorTicketIssueReceipt = decode(payload)?;
                check(value.schema_version == schema, "known_issue_schema_v1")?;
                E05IssueReceiptIdentity {
                    actual_digest: typed_fingerprint(&value)?,
                    ticket_id: value.ticket_id,
                    registration_digest: value.registration_digest,
                    manifest_digest: value.manifest_digest,
                    evaluator_actor: value.evaluator_actor,
                    issued_at_unix_seconds: value.issued_at_unix_seconds,
                    issued_at_unix_ms: None,
                }
            } else {
                check(
                    ticket.candidate_dependency_binding.is_some(),
                    "known_v2_issue_requires_binding",
                )?;
                let value: MirrorTicketIssueReceiptV2 = decode(payload)?;
                check(
                    value.schema_version == schema
                        && value.issued_at_unix_seconds >= 0
                        && value.issued_at_unix_ms >= 0
                        && value.issued_at_unix_ms / 1000 == value.issued_at_unix_seconds,
                    "known_issue_schema_and_native_time_v2",
                )?;
                E05IssueReceiptIdentity {
                    actual_digest: typed_fingerprint(&value)?,
                    ticket_id: value.ticket_id,
                    registration_digest: value.registration_digest,
                    manifest_digest: value.manifest_digest,
                    evaluator_actor: value.evaluator_actor,
                    issued_at_unix_seconds: value.issued_at_unix_seconds,
                    issued_at_unix_ms: Some(value.issued_at_unix_ms),
                }
            };
            check(
                object.namespace == namespace
                    && object.owner == ticket.evaluator_actor
                    && identity.ticket_id == ticket.id
                    && identity.registration_digest == ticket.registration_digest
                    && identity.manifest_digest == ticket.manifest_digest
                    && identity.evaluator_actor == ticket.evaluator_actor
                    && identity.issued_at_unix_seconds == ticket.issued_at_unix_seconds
                    && identity.actual_digest == ticket.issue_receipt_digest,
                "known_issue_ticket_owner_identity_or_digest",
            )?;
            if let Some(control) = control {
                if identity.issued_at_unix_ms.is_some() {
                    check(
                        identity.issued_at_unix_seconds >= control.created_at_unix_seconds,
                        "available_issue_control_occurrence",
                    )?;
                }
            } else {
                gaps.insert("issue_registered_control_missing");
            }
            if let Some(holdout) = holdout {
                if let Some(millis) = identity.issued_at_unix_ms {
                    check(
                        millis >= holdout.manifest.issued_at_unix_ms,
                        "available_issue_manifest_occurrence",
                    )?;
                }
            } else {
                gaps.insert("issue_registered_holdout_missing");
            }
            typed_ready = true;
            // Preserve the original full protocol selector only after all of
            // its actual prerequisites are present. A pure missing auxiliary
            // source cannot be turned into ClockRegressed by its need() path.
            if control.is_some() && holdout.is_some() {
                e05_issue_receipt(actual, namespace, ticket)?;
            }
        }
        if count == 0 {
            gaps.insert("retained_issue_receipt_missing");
        }
        Ok(typed_ready.then_some(()))
    }

    fn verify_complete_payload_rejection_r49(
        actual: &OccurrenceSnapshot,
        carrier: &FrozenCarrierR36,
        ctx: &Context,
        control: Option<&MirrorRegisteredEvaluationControl>,
        holdout: Option<&MirrorProtectedHoldoutRecord>,
        expected_nodes: &[MirrorFormalDependencyNode],
        math_calls: &[E05MathCallR39],
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<()> {
        let ticket = &carrier.input.ticket_snapshot;
        let targets_available =
            inspect_target_rows_r39(actual, ctx.namespace(), ticket, control, holdout, gaps)?;
        let resource: Option<MirrorResourceEvidenceRecord> = record_available(
            actual,
            ctx.namespace(),
            RESOURCE_KIND,
            &ticket.id,
            gaps,
            "retained_resource_evidence_missing",
        )?;
        let MirrorFormalEvaluationV2::CompleteBatch {
            cost_evidence_scope,
            report,
            stored_dependency_binding,
            resource_evidence_digest,
            anchor_evidence_digest,
            ..
        } = &carrier.payload
        else {
            return Err(invalid("Current_complete_reader_requires_complete_variant"));
        };
        if let Some(resource) = &resource {
            check(
                resource.schema_version == "rsia.evaluation_resource_evidence.v1"
                    && resource.ticket_id == ticket.id
                    && resource.cost_scope == MirrorCostEvidenceScope::TicketExecutionOnly
                    && *cost_evidence_scope == resource.cost_scope
                    && typed_fingerprint(resource)? == *resource_evidence_digest
                    && resource.calls.len() == carrier.input.calls.len(),
                "retained_resource_identity_or_digest",
            )?;
            if let Some(control) = &control {
                check(
                    resource.billing_scope == control.billing_scope,
                    "resource_control_scope",
                )?;
            }
            let by_id: BTreeMap<_, _> = carrier
                .input
                .calls
                .iter()
                .map(|saved| {
                    (
                        (
                            saved.namespace.as_str(),
                            saved.billing_scope.as_str(),
                            saved.call_id.as_str(),
                        ),
                        saved,
                    )
                })
                .collect();
            let mut seen = BTreeSet::new();
            for call in &resource.calls {
                let key = (
                    call.namespace.as_str(),
                    call.billing_scope.as_str(),
                    call.call_id.as_str(),
                );
                check(seen.insert(key), "resource_duplicate_call")?;
                let saved = by_id
                    .get(&key)
                    .ok_or_else(|| invalid("resource_extra_call"))?;
                saved
                    .snapshot
                    .validate_original_copy_r36(call)
                    .map_err(frozen_call_error_r24)?;
            }
        }
        let expected_binding = match (
            &ticket.candidate_dependency_binding,
            stored_dependency_binding,
        ) {
            (Some(candidate), Some(stored)) => {
                let expected = MirrorFormalStoredDependencyBinding {
                    schema_version: "rsia.formal_stored_dependency_binding.v1".into(),
                    namespace: ctx.namespace().into(),
                    candidate_binding_digest: typed_fingerprint(candidate)?,
                    ticket_id: ticket.id.clone(),
                    ticket_physical_id: evaluation_storage_id(TICKET_KIND, &ticket.id)?,
                    ticket_digest: typed_fingerprint(ticket)?,
                    resource_physical_id: evaluation_storage_id(RESOURCE_KIND, &ticket.id)?,
                    nodes: expected_nodes.to_vec(),
                };
                check(stored == &expected, "stored_binding_original_frozen_graph")?;
                Some(expected)
            }
            (None, None) => None,
            _ => return Err(invalid("mixed_candidate_and_stored_binding")),
        };
        let anchors: Vec<_> = ticket
            .targets
            .iter()
            .filter(|target| matches!(&target.kind, MirrorEvaluationTargetKind::Anchor { .. }))
            .collect();
        check(
            typed_fingerprint(&anchors)? == *anchor_evidence_digest,
            "original_anchor_digest",
        )?;
        if targets_available
            && let (Some(control), Some(holdout), Some(exposure), Some(resource)) =
                (control, holdout, &carrier.input.exposure, &resource)
        {
            let records = FrozenRecords {
                actual: actual,
                facts: &carrier.input.facts,
                exposure: &exposure.snapshot,
            };
            // This calls the SAME real-output oracle/grader, first-stop replay,
            // full fixed-sample statistics and resource ratios as the producer.
            let expected_report = e05_recompute_complete_report_r39(
                &records,
                ctx.namespace(),
                ticket,
                control,
                holdout,
                math_calls,
            )?;
            if report != &expected_report {
                return Err(Error::Conflict(
                    "formal_complete_batch_statistics_changed".into(),
                ));
            }
            let expected = MirrorFormalEvaluationV2::CompleteBatch {
                evidence_scope: ticket.evidence_scope,
                cost_evidence_scope: MirrorCostEvidenceScope::TicketExecutionOnly,
                ticket_id: ticket.id.clone(),
                ticket_digest: typed_fingerprint(ticket)?,
                manifest_digest: ticket.manifest_digest.clone(),
                candidate_bundle_digest: ticket.candidate_bundle_digest.clone(),
                baseline_bundle_digest: ticket.baseline_bundle_digest.clone(),
                environment_digest: ticket.environment_digest.clone(),
                verdict: expected_report.verdict,
                report: expected_report,
                anchor_evidence_digest: typed_fingerprint(&anchors)?,
                resource_evidence_digest: typed_fingerprint(resource)?,
                stored_dependency_binding: expected_binding,
            };
            check(
                typed_fingerprint(&expected)? == typed_fingerprint(&carrier.payload)?,
                "all_recomputed_Formal_payload_fields",
            )?;
        }
        Ok(())
    }
    pub(super) fn diagnose_statistics_before_checksum_r49(
        raw: &RawDatabase,
        ctx: &Context,
        id: &str,
        carrier: &FrozenCarrierR36,
    ) -> Result<()> {
        if !diagnostic_original_input_bound_r49(raw, ctx, id, carrier)? {
            return Ok(());
        }
        carrier.input.validate_known_wire().map_err(r24_fault)?;
        require(matches!(&carrier.input.operation, fv::Operation::Complete))?;
        digest(&carrier.payload_digest)?;
        let ticket = &carrier.input.ticket_snapshot;
        ticket.validate()?;
        require(
            carrier.schema_version == fv::SCHEMA
                && carrier.record_kind == fv::KIND
                && carrier.engine_version == fv::ENGINE_V4
                && carrier.input_schema_version == fv::INPUT_SCHEMA_V2
                && carrier.input.namespace == ctx.namespace()
                && carrier.id == id
                && fv::address(
                    ctx.namespace(),
                    fv::ENGINE_V4,
                    fv::INPUT_SCHEMA_V2,
                    &carrier.input_digest,
                )
                .map_err(r24_fault)?
                    == id,
        )?;
        let MirrorFormalEvaluationV2::CompleteBatch {
            ticket_id,
            ticket_digest,
            manifest_digest,
            evidence_scope,
            candidate_bundle_digest,
            baseline_bundle_digest,
            environment_digest,
            ..
        } = &carrier.payload
        else {
            return Ok(());
        };
        require(
            ticket_id == &ticket.id
                && ticket_digest == &carrier.input.ticket_digest
                && manifest_digest == &ticket.manifest_digest
                && evidence_scope == &ticket.evidence_scope
                && candidate_bundle_digest == &ticket.candidate_bundle_digest
                && baseline_bundle_digest == &ticket.baseline_bundle_digest
                && environment_digest == &ticket.environment_digest,
        )?;

        // An honest partial actual capture for a rejection-only diagnostic.
        // No object/row/body/ledger is rewritten and no success permit/result
        // can be created from it. Full load_complete_origins remains below in
        // the ordinary unmodified success path.
        let mut floor = OccurrenceFloor::default();
        let prefix = schema_prefix(raw, SchemaRead::Stable, &mut floor)?;
        require(prefix > 0)?;
        let mut actual = OccurrenceSnapshot::default();
        collect_audit(raw, &mut floor, &mut actual)?;
        collect_objects(raw, &mut actual)?;
        if prefix >= 2 {
            load_dependencies(raw, &mut actual)?;
        }
        if prefix >= 3 {
            collect_replay_worlds(raw, &mut actual)?;
        }
        if prefix >= 4 {
            collect_budget(raw, &mut floor, &mut actual)?;
        }
        if prefix >= 5 {
            collect_cleanup(raw, &mut floor, &actual)?;
        }
        load_caches(raw, &mut actual)?;
        actual.precompute_actual_body_digests_r26();
        let mut gaps = BTreeSet::new();
        inspect_mutable_and_immutable_facts(raw, &actual, carrier, &mut gaps)?;
        inspect_native_exposure_capture_r2(&actual, carrier, &mut gaps)?;
        let nodes = frozen_complete_kernel_r29::frozen_graph(carrier)?;
        inspect_actual_compiler_authorities(&actual, carrier, &mut gaps)?;
        inspect_candidate_r39(&actual, carrier, &mut gaps)?;
        let (_calls, call_gaps) =
            frozen_complete_kernel_r29::reconstruct_calls(&actual, &carrier.input.calls)?;
        gaps.extend(call_gaps);
        let control: Option<MirrorRegisteredEvaluationControl> = record_available(
            &actual,
            ctx.namespace(),
            "registered_evaluation_control_v41",
            &ticket.registration_id,
            &mut gaps,
            "diagnostic_original_control_missing",
        )?;
        if let Some(control) = &control {
            control.validate()?;
            known_ticket_control_r36(ticket, control)?;
        }
        let holdout: Option<MirrorProtectedHoldoutRecord> = record_available(
            &actual,
            ctx.namespace(),
            HOLDOUT_KIND,
            &ticket.holdout_id,
            &mut gaps,
            "diagnostic_original_holdout_missing",
        )?;
        if let Some(holdout) = &holdout {
            known_ticket_holdout_r36(ticket, holdout)?;
            if let Some(control) = &control {
                holdout.validate_against(control)?;
            }
        }
        let math_calls = carrier
            .input
            .calls
            .iter()
            .map(|saved| {
                saved
                    .snapshot
                    .math_projection_r39()
                    .map_err(frozen_call_error_r24)
            })
            .collect::<Result<Vec<_>>>()?;
        // This is the original complete report recomputation using real output,
        // retained oracle, authenticated frozen ticket/exposure and call29+3.
        // A bad statistic yields the original precise STATISTICS_CHANGED.
        verify_complete_payload_rejection_r49(
            &actual,
            carrier,
            ctx,
            control.as_ref(),
            holdout.as_ref(),
            &nodes,
            &math_calls,
            &mut gaps,
        )?;
        Ok(())
    }

    pub(super) fn data_projection_r49(
        source: &AuthenticatedFrozenInputs,
    ) -> Result<(Value, Value)> {
        Ok((
            serde_json::to_value(&source.frozen_ticket).map_err(crate::internal)?,
            source.payload.clone(),
        ))
    }
    pub(super) fn after_close(
        source: AuthenticatedFrozenInputs,
        _completed: &crate::lifecycle::OriginalReadCompletedR38,
    ) -> NativeCurrentFormalR39 {
        NativeCurrentFormalR39 {
            source: Arc::new(source),
        }
    }

    fn frozen_subgraph_r39(
        carrier: &FrozenCarrierR36,
        root: (String, String),
    ) -> Result<Vec<MirrorFormalDependencyNode>> {
        let facts: BTreeMap<_, _> = carrier
            .input
            .facts
            .iter()
            .map(|fact| ((fact.kind.clone(), fact.id.clone()), fact))
            .collect();
        let mut pending = BTreeSet::from([root]);
        let mut seen = BTreeSet::new();
        let mut nodes = Vec::new();
        while let Some(key) = pending.pop_first() {
            if !seen.insert(key.clone()) {
                continue;
            }
            check(seen.len() <= 10_000, "candidate_graph_capacity")?;
            let fact = facts
                .get(&key)
                .ok_or_else(|| invalid("candidate_graph_fact_missing"))?;
            for edge in &fact.outgoing {
                if !seen.contains(edge) {
                    pending.insert(edge.clone());
                }
            }
            nodes.push(MirrorFormalDependencyNode {
                kind: fact.kind.clone(),
                id: fact.id.clone(),
                schema_version: fact.schema_version.clone(),
                record_kind: fact.record_kind.clone(),
                logical_id: fact.logical_id.clone(),
                content_digest: fact.typed_record_digest.clone(),
                outgoing_edge_digest: fact.outgoing_digest.clone(),
                outgoing_edge_count: fact.outgoing.len(),
            });
        }
        nodes.sort_by(|a, b| (&a.kind, &a.id).cmp(&(&b.kind, &b.id)));
        Ok(nodes)
    }

    fn inspect_candidate_r39(
        actual: &OccurrenceSnapshot,
        carrier: &FrozenCarrierR36,
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<()> {
        let ticket = &carrier.input.ticket_snapshot;
        let namespace = carrier.input.namespace.as_str();
        let Some(binding) = &ticket.candidate_dependency_binding else {
            return Ok(());
        };
        binding.validate()?;
        check(
            binding.namespace == namespace
                && binding.candidate_id == ticket.candidate_digest
                && binding.bundle_digest == ticket.candidate_bundle_digest
                && binding.parent_digest == ticket.baseline_digest
                && binding.environment_digest == ticket.environment_digest
                && binding.proposer_actor == ticket.proposer_actor,
            "known_candidate_ticket_binding",
        )?;
        check(
            binding.nodes
                == frozen_subgraph_r39(carrier, ("artifact".into(), binding.candidate_id.clone()))?,
            "candidate_binding_frozen_nodes",
        )?;
        if let Some(watermark) = actual.watermarks.get(namespace) {
            check(
                binding.revoke_watermark_seq
                    == i64::try_from(watermark.0).map_err(|_| invalid("watermark_capacity"))?
                    && binding.revoke_watermark_digest == watermark.1,
                "current_candidate_watermark",
            )?;
        } else {
            gaps.insert("current_candidate_watermark_missing");
        }
        let key = (
            namespace.to_owned(),
            "artifact".to_owned(),
            binding.candidate_id.clone(),
        );
        if let Some(object) = actual.objects.get(&key) {
            if object.body.get("schema_version").and_then(Value::as_str) == Some("rsia.redacted.v1")
            {
                qualify_redacted(object)?;
                gaps.insert("retained_candidate_redacted");
            } else {
                let candidate: MirrorReleaseCandidateRecord = decode(&object.body)?;
                check(
                    candidate.id == binding.candidate_id
                        && binding.candidate_record_digest == typed_fingerprint(&candidate)?
                        && binding.bundle_digest == candidate.bundle_digest
                        && binding.parent_digest == candidate.parent_digest
                        && binding.profile_id == candidate.profile_id
                        && binding.revoke_watermark_seq
                            == i64::try_from(candidate.revoke_watermark)
                                .map_err(|_| invalid("candidate_watermark_capacity"))?
                        && binding.environment_digest == candidate.environment_digest
                        && binding.proposer_actor == candidate.proposer_actor
                        && binding.sources == candidate.sources,
                    "actual_candidate_frozen_binding",
                )?;
                let outgoing = e05_outgoing(actual, namespace, "artifact", &candidate.id)?;
                for source in &candidate.sources {
                    check(
                        outgoing.contains(&(source.kind.clone(), source.id.clone())),
                        "actual_candidate_source_edge",
                    )?;
                    if let Some(original) = actual.objects.get(&(
                        namespace.to_owned(),
                        source.kind.clone(),
                        source.id.clone(),
                    )) {
                        if original.body.get("schema_version").and_then(Value::as_str)
                            == Some("rsia.redacted.v1")
                        {
                            qualify_redacted(original)?;
                            gaps.insert("retained_candidate_source_redacted");
                        } else {
                            validate_native_authority(original)?;
                            let original: MirrorStoredTraceAuthority = decode(&original.body)?;
                            check(
                                original.trace.source_digest == source.content_digest,
                                "actual_candidate_source_content",
                            )?;
                        }
                    } else {
                        gaps.insert("retained_candidate_source_missing");
                    }
                }
            }
        } else {
            gaps.insert("retained_candidate_missing");
        }
        Ok(())
    }

    fn actual_subtree_available(
        actual: &OccurrenceSnapshot,
        namespace: &str,
        root: (String, String),
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<bool> {
        let mut pending = BTreeSet::from([root]);
        let mut seen = BTreeSet::new();
        let mut available = true;
        while let Some((kind, id)) = pending.pop_first() {
            if !seen.insert((kind.clone(), id.clone())) {
                continue;
            }
            check(seen.len() <= 10_000, "actual_authority_subtree_capacity")?;
            if let Some(object) = actual
                .objects
                .get(&(namespace.into(), kind.clone(), id.clone()))
            {
                if object.body.get("schema_version").and_then(Value::as_str)
                    == Some("rsia.redacted.v1")
                {
                    qualify_redacted(object)?;
                    gaps.insert("current_authority_subtree_redacted");
                    available = false;
                } else {
                    let mut auxiliary_gaps = BTreeSet::new();
                    inspect_compiler_auxiliary_r2(actual, object, &mut auxiliary_gaps)?;
                    if !auxiliary_gaps.is_empty() {
                        available = false;
                    }
                    gaps.extend(auxiliary_gaps);
                    for edge in e05_outgoing(actual, namespace, &kind, &id)? {
                        if !seen.contains(&edge) {
                            pending.insert(edge);
                        }
                    }
                }
            } else {
                gaps.insert("current_authority_subtree_missing");
                available = false;
            }
        }
        Ok(available)
    }

    fn role_witness_available_r2(
        actual: &OccurrenceSnapshot,
        object: &OccurrenceObject,
        role: &str,
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<bool> {
        let prefix = match role {
            "admin" => "formal.admin.content.v1",
            "evaluator" => "formal.evaluator.content.v1",
            "host" => "formal.host.content.v1",
            _ => return Err(invalid("known_content_role")),
        };
        let subject = typed_fingerprint(&(&object.namespace, &object.kind, &object.id))?;
        let key = (
            object.namespace.clone(),
            role.to_owned(),
            format!("{prefix}.{subject}"),
        );
        let content = object.value_body_digest_r26()?;
        let entity = typed_fingerprint(&(&object.namespace, &object.kind, &object.id, content))?;
        if let Some((observed, _actor)) = actual.witnesses.get(&key) {
            check(
                observed == &entity,
                "actual_auxiliary_content_witness_mismatch",
            )?;
            actual.witness(object, role)?;
            Ok(true)
        } else {
            gaps.insert("actual_auxiliary_content_witness_missing");
            Ok(false)
        }
    }

    fn auxiliary_object_r2<'a>(
        actual: &'a OccurrenceSnapshot,
        namespace: &str,
        kind: &str,
        id: &str,
        schema: Option<&str>,
        role: Option<&str>,
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<Option<&'a OccurrenceObject>> {
        let Some(object) =
            actual
                .objects
                .get(&(namespace.to_owned(), kind.to_owned(), id.to_owned()))
        else {
            gaps.insert("actual_auxiliary_authority_object_missing");
            return Ok(None);
        };
        if object.body.get("schema_version").and_then(Value::as_str) == Some("rsia.redacted.v1") {
            qualify_redacted(object)?;
            if let Some(schema) = schema {
                check(
                    object.body.get("original_schema").and_then(Value::as_str) == Some(schema),
                    "auxiliary_redacted_original_schema",
                )?;
            }
            gaps.insert("actual_auxiliary_authority_object_redacted");
            return Ok(None);
        }
        if let Some(schema) = schema {
            check(
                object.body.get("schema_version").and_then(Value::as_str) == Some(schema),
                "auxiliary_authority_schema",
            )?;
        }
        frozen_complete_kernel_r29::known_dependency_identity_r3(actual, object)?;
        if let Some(role) = role {
            role_witness_available_r2(actual, object, role, gaps)?;
        }
        Ok(Some(object))
    }

    fn profile_host_id_available_r2(
        actual: &OccurrenceSnapshot,
        namespace: &str,
        profile: &str,
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<Option<String>> {
        let Some(selection) = auxiliary_object_r2(
            actual,
            namespace,
            "artifact",
            &format!("formal-profile-{profile}"),
            Some("rsia.formal_profile_selection.v1"),
            Some("admin"),
            gaps,
        )?
        else {
            return Ok(None);
        };
        let id = string(&selection.body, "registration_id")?;
        let Some(record) = auxiliary_object_r2(
            actual,
            namespace,
            "artifact",
            id,
            Some("rsia.frozen_compiler_profile.v1"),
            Some("admin"),
            gaps,
        )?
        else {
            return Ok(None);
        };
        Ok(Some(string(&record.body, "host_registration_id")?.into()))
    }

    fn subject_host_id_available_r2(
        actual: &OccurrenceSnapshot,
        object: &OccurrenceObject,
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<Option<String>> {
        let ns = object.namespace.as_str();
        let body = &object.body;
        Ok(match string(body, "schema_version")? {
            "rsia.registered_formal_host.v1" => Some(string(body, "id")?.into()),
            "rsia.frozen_compiler_profile.v1"
            | "rsia.empty_root_authorization.v1"
            | "rsia.registered_external_provider.v1"
            | "rsia.registered_external_provider.v2"
            | "rsia.registered_resource_instrumentation.v1" => {
                Some(string(body, "host_registration_id")?.into())
            }
            "rsia.formal_profile_selection.v1" => {
                let Some(profile) = auxiliary_object_r2(
                    actual,
                    ns,
                    "artifact",
                    string(body, "registration_id")?,
                    Some("rsia.frozen_compiler_profile.v1"),
                    Some("admin"),
                    gaps,
                )?
                else {
                    return Ok(None);
                };
                Some(string(&profile.body, "host_registration_id")?.into())
            }
            "rsia.formal_parent_selection.v1" => {
                let selection: MirrorParentSelection = decode(body)?;
                match selection.authority {
                    MirrorParentAuthority::EmptyRoot { id } => {
                        let Some(root) = auxiliary_object_r2(
                            actual,
                            ns,
                            "artifact",
                            &id,
                            Some("rsia.empty_root_authorization.v1"),
                            Some("admin"),
                            gaps,
                        )?
                        else {
                            return Ok(None);
                        };
                        Some(string(&root.body, "host_registration_id")?.into())
                    }
                    MirrorParentAuthority::ApprovedRelease { release_id } => {
                        let Some(release) = auxiliary_object_r2(
                            actual,
                            ns,
                            "release",
                            &release_id,
                            Some("rsia.persistent_release.v1"),
                            Some("admin"),
                            gaps,
                        )?
                        else {
                            return Ok(None);
                        };
                        profile_host_id_available_r2(
                            actual,
                            ns,
                            string(&release.body, "profile_id")?,
                            gaps,
                        )?
                    }
                }
            }
            "rsia.profile_pointer.v1" | "rsia.persistent_release.v1" => {
                profile_host_id_available_r2(actual, ns, string(body, "profile_id")?, gaps)?
            }
            _ => None,
        })
    }

    fn registered_admin_available_r2(
        actual: &OccurrenceSnapshot,
        object: &OccurrenceObject,
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<()> {
        let Some(host_id) = subject_host_id_available_r2(actual, object, gaps)? else {
            return Ok(());
        };
        let host_object = auxiliary_object_r2(
            actual,
            &object.namespace,
            "artifact",
            &host_id,
            Some("rsia.registered_formal_host.v1"),
            Some("admin"),
            gaps,
        )?;
        let host: Option<MirrorRegisteredFormalHost> =
            host_object.map(|object| decode(&object.body)).transpose()?;
        if let Some(host) = &host {
            host.validate(&object.namespace)?;
        }
        let set_id = format!("formal-principals-{host_id}");
        let set_key = (
            object.namespace.clone(),
            "artifact".to_owned(),
            set_id.clone(),
        );
        // Keep the original presence-based compatibility rule. An absent
        // principal observation is not invented or newly required.
        if !actual.objects.contains_key(&set_key) {
            return Ok(());
        }
        let observed_object = auxiliary_object_r2(
            actual,
            &object.namespace,
            "artifact",
            &set_id,
            Some("rsia.formal_principal_set_observation.v1"),
            None,
            gaps,
        )?;
        if let Some(observed) = observed_object {
            role_witness_available_r2(actual, observed, "admin", gaps)?;
            role_witness_available_r2(actual, observed, "evaluator", gaps)?;
        }
        let observed: Option<MirrorPrincipalSetObservation> = observed_object
            .map(|object| decode(&object.body))
            .transpose()?;
        if let Some(observed) = &observed {
            let set = &observed.principals;
            check(
                set.schema_version == "rsia.registered_formal_principals.v1" && set.version != 0,
                "available_registered_principal_set_version",
            )?;
            let principals = [&set.host, &set.grader, &set.administrator, &set.worker];
            for principal in principals {
                identifier(&principal.actor)?;
                identifier(&principal.principal)?;
                digest(&principal.credential_digest)?;
            }
            check(
                principals
                    .iter()
                    .map(|p| p.uid)
                    .collect::<BTreeSet<_>>()
                    .len()
                    == 4
                    && principals
                        .iter()
                        .map(|p| &p.actor)
                        .collect::<BTreeSet<_>>()
                        .len()
                        == 4
                    && principals
                        .iter()
                        .map(|p| &p.principal)
                        .collect::<BTreeSet<_>>()
                        .len()
                        == 4
                    && principals
                        .iter()
                        .map(|p| &p.credential_digest)
                        .collect::<BTreeSet<_>>()
                        .len()
                        == 4,
                "available_principal_set_distinct_roles",
            )?;
            check(
                observed.id == set_id
                    && observed.schema_version == "rsia.formal_principal_set_observation.v1"
                    && observed.namespace == object.namespace
                    && set.id == set_id
                    && set.namespace == object.namespace
                    && set.host_registration_id == host_id
                    && observed.grader_pid != 0,
                "available_principal_observation_identity",
            )?;
            digest(&observed.host_digest)?;
            digest(&observed.grader_executable_digest)?;
            if let Some(host) = &host {
                set.validate(host)?;
                check(
                    observed.host_digest == typed_fingerprint(host)?
                        && observed.grader_uid == host.grader_uid
                        && observed.grader_executable_digest == host.grader_code_digest
                        && observed.grader_workspace == host.grader_workspace,
                    "available_principal_observation_host",
                )?;
            }
        }
        let action_id = format!(
            "principal-action-{}",
            typed_fingerprint(&(&object.kind, &object.id, Role::Admin))?
        );
        let action_object = auxiliary_object_r2(
            actual,
            &object.namespace,
            "artifact",
            &action_id,
            Some("rsia.observed_principal_action.v1"),
            Some("evaluator"),
            gaps,
        )?;
        let Some(action_object) = action_object else {
            return Ok(());
        };
        let action: MirrorPrincipalAction = decode(&action_object.body)?;
        check(
            action.id == action_id
                && action.schema_version == "rsia.observed_principal_action.v1"
                && action.namespace == object.namespace
                && action.role == Role::Admin
                && action.host_registration_id == host_id
                && action.subject_kind == object.kind
                && action.subject_id == object.id
                && action.subject_content_digest == object.value_body_digest_r26()?
                && action.authority_pid != 0,
            "available_registered_admin_subject",
        )?;
        identifier(&action.actor)?;
        identifier(&action.principal)?;
        digest(&action.credential_digest)?;
        digest(&action.authority_executable_digest)?;
        digest(&action.subject_content_digest)?;
        digest(&action.host_digest)?;
        digest(&action.principal_set_digest)?;
        digest(&action.authenticated_request_digest)?;
        check(
            matches!(
                action.operation.as_str(),
                "bootstrap"
                    | "register_provider"
                    | "register_meter"
                    | "freeze_profile"
                    | "empty_root"
                    | "approve"
                    | "canary"
                    | "activate"
                    | "rollback"
            ),
            "available_registered_admin_operation",
        )?;
        if let Some(observed) = &observed {
            let set = &observed.principals;
            let principal = set.principal(Role::Admin)?;
            check(
                action.actor == principal.actor
                    && action.uid == principal.uid
                    && action.peer_uid == principal.uid
                    && action.principal == principal.principal
                    && action.credential_digest == principal.credential_digest
                    && action.principal_set_digest == set.binding_digest()?
                    && action.authority_uid == set.grader.uid,
                "available_registered_admin_principal",
            )?;
        }
        if let Some(host) = &host {
            check(
                action.host_digest == typed_fingerprint(host)?
                    && action.authority_executable_digest == host.grader_code_digest,
                "available_registered_admin_host",
            )?;
        }
        Ok(())
    }

    fn inspect_compiler_auxiliary_r2(
        actual: &OccurrenceSnapshot,
        object: &OccurrenceObject,
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<()> {
        let Some(schema) = object.body.get("schema_version").and_then(Value::as_str) else {
            return Ok(());
        };
        if !compiler_node_schema(&object.kind, schema) {
            if object.kind == "artifact"
                && object
                    .body
                    .get("record_kind")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| {
                        matches!(
                            kind,
                            "registered_evaluation_control_v41" | "protected_holdout_v41"
                        )
                    })
            {
                role_witness_available_r2(actual, object, "evaluator", gaps)?;
            }
            return Ok(());
        }
        let role = match schema {
            "rsia.frozen_compiler_profile.v1"
            | "rsia.empty_root_authorization.v1"
            | "rsia.registered_formal_host.v1"
            | "rsia.registered_external_provider.v1"
            | "rsia.registered_external_provider.v2"
            | "rsia.registered_resource_instrumentation.v1"
            | "rsia.formal_profile_selection.v1"
            | "rsia.formal_parent_selection.v1"
            | "rsia.persistent_release.v1"
            | "rsia.profile_pointer.v1"
            | "rsia.host_surface_record.v1" => Some("admin"),
            "rsia.issued_formal_authority.v2" | "rsia.independent_grade_capture.v1" => {
                Some("evaluator")
            }
            "rsia.actual_formal_provider_capture.v1"
            | "rsia.complete_episode_cost.v1"
            | "rsia.trusted_resource_meter.v1"
            | "rsia.run_application.v1"
            | "rsia.host_application.v1"
            | "rsia.host_execution_receipt.v1" => Some("host"),
            _ => None,
        };
        if let Some(role) = role {
            role_witness_available_r2(actual, object, role, gaps)?;
        }
        if role == Some("admin") {
            registered_admin_available_r2(actual, object, gaps)?;
        }
        if matches!(
            schema,
            "rsia.observed_principal_action.v1" | "rsia.observed_control_principal_action.v2"
        ) {
            role_witness_available_r2(actual, object, "evaluator", gaps)?;
        }
        if schema == "rsia.formal_principal_set_observation.v1" {
            role_witness_available_r2(actual, object, "admin", gaps)?;
            role_witness_available_r2(actual, object, "evaluator", gaps)?;
        }
        Ok(())
    }

    fn inspect_known_actual_graph_r2(
        snapshot: &OccurrenceSnapshot,
        namespace: &str,
        root: (String, String),
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<()> {
        let mut pending = BTreeSet::from([root]);
        let mut inspected_keys = BTreeSet::new();
        let mut nodes = BTreeMap::new();
        let mut edges = BTreeSet::new();
        let mut bytes = 0usize;
        let mut tickets = BTreeSet::new();
        let mut attached = BTreeSet::new();
        while let Some((kind, id)) = pending.pop_first() {
            if !inspected_keys.insert((kind.clone(), id.clone())) {
                continue;
            }
            check(
                inspected_keys.len() <= 10_000,
                "known_actual_graph_capacity",
            )?;
            require(nodes.len() < 10000)?;
            let Some(object) =
                snapshot
                    .objects
                    .get(&(namespace.to_owned(), kind.clone(), id.clone()))
            else {
                gaps.insert("actual_authority_graph_object_missing");
                continue;
            };
            if object.body.get("schema_version").and_then(Value::as_str) == Some("rsia.redacted.v1")
            {
                qualify_redacted(object)?;
                gaps.insert("actual_authority_graph_object_redacted");
                continue;
            }
            let (schema, record_kind, _, _) =
                frozen_complete_kernel_r29::known_dependency_identity_r3(snapshot, object)?;
            bytes = bytes
                .checked_add(
                    serde_json::to_vec(&object.body)
                        .map_err(|_| Error::ClockRegressed)?
                        .len(),
                )
                .ok_or(Error::ClockRegressed)?;
            require(bytes <= 64 * 1024 * 1024)?;
            if schema == "rsia.typed_artifact_envelope.v1" {
                if record_kind.as_deref() == Some("evaluation_ticket_v2") {
                    tickets.insert((kind.clone(), id.clone()));
                }
                if matches!(
                    record_kind.as_deref(),
                    Some("execution_receipt_v2" | "independent_grader_receipt_v2")
                ) {
                    attached.insert((kind.clone(), id.clone()));
                }
                if matches!(
                    record_kind.as_deref(),
                    Some("registered_evaluation_control_v41" | "protected_holdout_v41")
                ) {
                    role_witness_available_r2(snapshot, object, "evaluator", gaps)?;
                }
            }
            for (k, i) in e05_outgoing(snapshot, namespace, &kind, &id)? {
                edges.insert((kind.clone(), id.clone(), k.clone(), i.clone()));
                require(edges.len() <= 100000)?;
                pending.insert((k, i));
            }
            nodes.insert((kind, id), schema);
        }
        let mut degrees: BTreeMap<_, usize> = nodes.keys().cloned().map(|key| (key, 0)).collect();
        let mut children = BTreeMap::<(String, String), Vec<(String, String)>>::new();
        for (k, i, to_k, to_i) in edges {
            let parent = (k, i);
            let child = (to_k, to_i);
            // Missing endpoints stay gaps; all cycles among known actual nodes
            // remain checked even when an unrelated auxiliary source is absent.
            if !nodes.contains_key(&parent) || !nodes.contains_key(&child) {
                continue;
            }
            if tickets.contains(&parent) && attached.contains(&child) {
                continue;
            }
            *degrees.get_mut(&child).ok_or(Error::ClockRegressed)? += 1;
            children.entry(parent).or_default().push(child);
        }
        let mut ready: Vec<_> = degrees
            .iter()
            .filter(|(_, n)| **n == 0)
            .map(|(key, _)| key.clone())
            .collect();
        let mut seen = 0usize;
        while let Some(key) = ready.pop() {
            seen += 1;
            if let Some(edges) = children.remove(&key) {
                for key in edges {
                    let n = degrees.get_mut(&key).ok_or(Error::ClockRegressed)?;
                    *n -= 1;
                    if *n == 0 {
                        ready.push(key);
                    }
                }
            }
        }
        check(seen == nodes.len(), "known_actual_compiler_graph_cycle")
    }
    fn inspect_actual_compiler_authorities(
        actual: &OccurrenceSnapshot,
        carrier: &FrozenCarrierR36,
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<()> {
        for fact in &carrier.input.facts {
            if !compiler_node_schema(&fact.kind, &fact.schema_version) {
                continue;
            }
            let key = (fact.namespace.clone(), fact.kind.clone(), fact.id.clone());
            let Some(object) = actual.objects.get(&key) else {
                continue;
            };
            if object.body.get("schema_version").and_then(Value::as_str) == Some("rsia.redacted.v1")
            {
                continue;
            }
            // Evaluate each available complete authority subtree independently.
            // An unrelated missing root/slot/origin never gates this check.
            inspect_known_actual_graph_r2(
                actual,
                &fact.namespace,
                (fact.kind.clone(), fact.id.clone()),
                gaps,
            )?;
            if actual_subtree_available(
                actual,
                &fact.namespace,
                (fact.kind.clone(), fact.id.clone()),
                gaps,
            )? {
                e05_compiler_closure(
                    actual,
                    &fact.namespace,
                    (fact.kind.clone(), fact.id.clone()),
                )?;
            }
        }
        Ok(())
    }

    fn inspect_target_rows_r39(
        snapshot: &OccurrenceSnapshot,
        ns: &str,
        ticket: &MirrorEvaluationTicketV2,
        control: Option<&MirrorRegisteredEvaluationControl>,
        holdout: Option<&MirrorProtectedHoldoutRecord>,
        gaps: &mut BTreeSet<&'static str>,
    ) -> Result<bool> {
        inspect_target_rows_variant_r2(snapshot, ns, ticket, control, holdout, gaps, false)
    }

    fn inspect_target_rows_variant_r2(
        snapshot: &OccurrenceSnapshot,
        ns: &str,
        ticket: &MirrorEvaluationTicketV2,
        control: Option<&MirrorRegisteredEvaluationControl>,
        holdout: Option<&MirrorProtectedHoldoutRecord>,
        gaps: &mut BTreeSet<&'static str>,
        skip_noncompleted: bool,
    ) -> Result<bool> {
        let mut available = true;
        for target in &ticket.targets {
            let MirrorTargetState::Completed {
                candidate_score_micros,
                baseline_score_micros,
                anchor_result,
                candidate_receipt_id,
                baseline_receipt_id,
                grader_receipt_id,
                ..
            } = &target.state
            else {
                if skip_noncompleted {
                    continue;
                }
                return Err(invalid("incomplete_frozen_target"));
            };
            let mut receipts = Vec::new();
            for (id, side, score) in [
                (
                    candidate_receipt_id,
                    MirrorExecutionSide::Candidate,
                    *candidate_score_micros,
                ),
                (
                    baseline_receipt_id,
                    MirrorExecutionSide::Baseline,
                    *baseline_score_micros,
                ),
            ] {
                let receipt: Option<MirrorExecutionReceiptV2> = record_available(
                    snapshot,
                    ns,
                    EXECUTION_RECEIPT_KIND,
                    id,
                    gaps,
                    "original_execution_receipt_missing",
                )?;
                let Some(receipt) = receipt else {
                    available = false;
                    continue;
                };
                let bundle = match side {
                    MirrorExecutionSide::Candidate => &ticket.candidate_bundle_digest,
                    MirrorExecutionSide::Baseline => &ticket.baseline_bundle_digest,
                };
                check(
                    receipt.schema_version == "rsia.execution_receipt.v2"
                        && receipt.receipt_id == *id
                        && receipt.ticket_id == ticket.id
                        && receipt.target_id == target.target_id
                        && receipt.side == side
                        && receipt.bundle_digest == *bundle
                        && receipt.environment_digest == ticket.environment_digest
                        && receipt.executor_actor == ticket.executor_actor
                        && receipt.request_digest
                            == e05_request_digest(
                                ticket,
                                target,
                                side,
                                bundle,
                                &ticket.environment_digest,
                            )?,
                    "available_execution_identity",
                )?;
                let output: Option<MirrorExecutionOutputArtifact> = record_available(
                    snapshot,
                    ns,
                    OUTPUT_KIND,
                    &receipt.output_artifact_id,
                    gaps,
                    "original_execution_output_missing",
                )?;
                if let Some(output) = output {
                    e05_output_artifact(&receipt, &output)?;
                    if let (Some(control), Some(holdout)) = (control, holdout) {
                        let oracle = holdout
                            .oracle_entries
                            .iter()
                            .find(|entry| entry.target_id == target.target_id);
                        if let Some(oracle) = oracle {
                            let actual = owned_grade_fixed_output(
                                &control.fixed_grader,
                                &output.output_utf8,
                                &oracle.expected_answer_json,
                            )?;
                            check(actual == score, "available_original_output_score")?;
                        } else {
                            gaps.insert("original_target_oracle_missing");
                            available = false;
                        }
                    }
                } else {
                    available = false;
                }
                receipts.push(receipt);
            }
            let grader: Option<MirrorIndependentGraderReceiptV2> = record_available(
                snapshot,
                ns,
                GRADER_RECEIPT_KIND,
                grader_receipt_id,
                gaps,
                "original_grader_receipt_missing",
            )?;
            if let Some(grader) = grader {
                check(
                    grader.schema_version == "rsia.independent_grader_receipt.v2"
                        && grader.receipt_id == *grader_receipt_id
                        && grader.ticket_id == ticket.id
                        && grader.target_id == target.target_id
                        && grader.evaluator_actor == ticket.evaluator_actor
                        && grader.candidate_execution_receipt_id == *candidate_receipt_id
                        && grader.baseline_execution_receipt_id == *baseline_receipt_id
                        && grader.candidate_score_micros == *candidate_score_micros
                        && grader.baseline_score_micros == *baseline_score_micros
                        && grader.anchor_result == *anchor_result,
                    "available_grader_ticket_identity",
                )?;
                if let Some(control) = control {
                    check(
                        grader.oracle_digest == control.oracle_digest
                            && grader.grader_digest == control.grader_digest,
                        "available_grader_control_identity",
                    )?;
                }
                for receipt in &receipts {
                    let recorded = match receipt.side {
                        MirrorExecutionSide::Candidate => &grader.candidate_output_digest,
                        MirrorExecutionSide::Baseline => &grader.baseline_output_digest,
                    };
                    check(
                        *recorded == receipt.output_digest,
                        "available_grader_output_identity",
                    )?;
                }
                if receipts.len() == 2 {
                    e05_execution_pair(ticket, &target.target_id, &receipts[0], &receipts[1])?;
                    if let Some(control) = control {
                        let pair = typed_fingerprint(&(
                            &receipts[0].output_digest,
                            &receipts[1].output_digest,
                            &control.oracle_digest,
                            &control.grader_digest,
                            *candidate_score_micros,
                            *baseline_score_micros,
                            *anchor_result,
                        ))?;
                        check(
                            grader.scored_output_pair_digest == pair,
                            "available_scored_output_pair_digest",
                        )?;
                    }
                }
            } else {
                available = false;
            }
        }
        Ok(available)
    }

    #[cfg(test)]
    mod controls {
        use super::*;

        fn root() -> fv::RootBudgetSnapshotV2 {
            fv::RootBudgetSnapshotV2 {
                root_budget_id: "root".into(),
                billing_scope: "scope".into(),
                authorizing_namespace: "n".into(),
                allowed_namespaces: vec!["n".into()],
                currency: "USD".into(),
                pricing_version: "price".into(),
                payment_subject: "subject".into(),
                authorization_receipt_digest: hash(b"receipt"),
                per_call_cap_micros: 10,
                total_limit_micros: 100,
                spent_micros: 10,
                reserved_micros: 2,
                stopped: false,
                stop_reason: None,
                stop_committed_at: None,
                created_at: 1,
            }
        }
        fn exposure() -> fv::ExposureProjectionV2 {
            fv::ExposureProjectionV2 {
                schema_version: evo_core::holdout::EXPOSURE_LEDGER_SCHEMA.into(),
                research_family_id: "family".into(),
                alpha_plan_digest: hash(b"alpha"),
                query_limit: 2,
                entry: evo_core::holdout::ExposureEntry {
                    ticket_id: "ticket".into(),
                    manifest_digest: hash(b"manifest"),
                    candidate_pair_digest: hash(b"pair"),
                    epoch_id: "epoch".into(),
                    slice_id: "slice".into(),
                    task_cluster_ids: vec!["cluster".into()],
                    state: evo_core::holdout::ExposureState::FeedbackUsed {
                        used_at_unix_ms: 2_000,
                    },
                    monetary_state: MonetaryReservationState::DispatchedCostPending,
                },
            }
        }

        #[test]
        fn r39_late_finance_and_root_close_preserve_original_but_payment_identity_does_not() {
            let original = root();
            let mut latest = original.clone();
            latest.spent_micros = 30;
            latest.reserved_micros = 0;
            latest.stopped = true;
            latest.stop_reason = Some("later_root_close".into());
            latest.stop_committed_at = Some(3);
            current_root_overlay(&latest, &original).unwrap();
            assert_eq!(original.spent_micros, 10);
            latest.payment_subject = "other".into();
            assert!(current_root_overlay(&latest, &original).is_err());
        }

        #[test]
        fn r39_exposure_original_entry_keeps_feedback_while_monetary_overlay_can_finalize() {
            let original = exposure();
            let mut latest = original.clone();
            latest.entry.monetary_state = MonetaryReservationState::Finalized;
            current_exposure_overlay(&latest, &original).unwrap();
            assert!(matches!(
                original.entry.monetary_state,
                MonetaryReservationState::DispatchedCostPending
            ));
            // Shared ledger entries for other tickets do not enter either
            // projection. Changing THIS ticket's immutable manifest must fail.
            latest.entry.manifest_digest = hash(b"other-manifest");
            assert!(current_exposure_overlay(&latest, &original).is_err());
        }

        #[test]
        fn r39_first_root_stop_is_not_replaced_by_later_financial_reconciliation() {
            let mut original = root();
            original.stopped = true;
            original.stop_reason = Some("first_stop".into());
            original.stop_committed_at = Some(2);
            let mut latest = original.clone();
            latest.spent_micros = 40;
            latest.reserved_micros = 0;
            current_root_overlay(&latest, &original).unwrap();
            latest.stop_reason = Some("different_stop".into());
            assert!(current_root_overlay(&latest, &original).is_err());
        }

        #[test]
        fn r39_auxiliary_witness_missing_is_delayed_but_wrong_content_is_invalid() {
            let object = OccurrenceObject {
                namespace: "r39-ns".into(),
                kind: "artifact".into(),
                id: "r39-source".into(),
                owner: "r39-admin".into(),
                body: serde_json::json!({"id":"r39-source"}),
                body_value_digest_r26: None,
            };
            let mut snapshot = OccurrenceSnapshot::default();
            let mut gaps = BTreeSet::new();
            assert!(!role_witness_available_r2(&snapshot, &object, "admin", &mut gaps).unwrap());
            assert!(gaps.contains("actual_auxiliary_content_witness_missing"));
            let subject =
                typed_fingerprint(&(&object.namespace, &object.kind, &object.id)).unwrap();
            snapshot.witnesses.insert(
                (
                    object.namespace.clone(),
                    "admin".into(),
                    format!("formal.admin.content.v1.{subject}"),
                ),
                (hash(b"wrong-content"), "admin".into()),
            );
            let error = role_witness_available_r2(&snapshot, &object, "admin", &mut gaps);
            assert!(matches!(error,Err(Error::Conflict(code))
                if code=="formal_invalid_frozen_read:r39:actual_auxiliary_content_witness_mismatch"));
            // Prior missing evidence cannot hide this newly available known contradiction.
            assert!(gaps.contains("actual_auxiliary_content_witness_missing"));
        }

        #[test]
        fn r39_auxiliary_absent_action_is_a_source_gap_not_a_clock_error() {
            let snapshot = OccurrenceSnapshot::default();
            let mut gaps = BTreeSet::new();
            let result = auxiliary_object_r2(
                &snapshot,
                "r39-ns",
                "artifact",
                "principal-action-missing",
                Some("rsia.observed_principal_action.v1"),
                Some("evaluator"),
                &mut gaps,
            )
            .unwrap();
            assert!(result.is_none());
            assert!(gaps.contains("actual_auxiliary_authority_object_missing"));
        }
    }
}
