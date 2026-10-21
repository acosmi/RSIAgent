//! Data codecs only. No serialized value here grants native/Current authority.
use crate::{fingerprint, identifier};
use serde::{Deserialize, Serialize};
pub const SCHEMA: &str = "rsia.e05.formal_report.v3";
pub const KIND: &str = "formal_evaluation_v3";
pub const ENGINE: &str = "rsia.e05.formal_engine.v3";
pub const INPUT_SCHEMA: &str = "rsia.e05.formal_input.v1";
pub const ENGINE_V4: &str = "rsia.e05.formal_engine.v4";
pub const INPUT_SCHEMA_V2: &str = "rsia.e05.formal_input.v2";
pub const REF_SCHEMA: &str = "rsia.e05.formal_report_ref.v1";
pub const OLD_SCHEMA: &str = "rsia.typed_artifact_envelope.v1";
pub const OLD_KIND: &str = "formal_evaluation_v2";

// Unproduced engine4/input2 contract amendment, pending independent/Claude review.
// Only this exact outer codec may carry native first-stop data; old v1 wire stays exact.
pub const NATIVE_TICKET_ENVELOPE: &str = "rsia.native_evaluation_ticket_envelope.v1";
pub const FIRST_STOP_SCHEMA: &str = "rsia.e05.native_first_stop.v1";
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeFirstStopV2 {
    pub schema_version: String,
    pub namespace: String,
    pub ticket_id: String,
    pub evaluator_actor: String,
    pub first_ticket_digest: String,
    pub member_terminal_digest: String,
    // One nibble/member: two state bits, then original candidate/baseline
    // receipt-presence bits. Other slot fields and completed members are
    // immutable after first stop and still physically retained in the ticket.
    pub member_flags: String,
    pub first_next_sequence: u64,
    pub initially_draining: bool,
    pub initial_list_lengths: [usize; 4],
    pub certificate: crate::sequential::EarlyStopCertificate,
}
impl NativeFirstStopV2 {
    pub fn validate_known(&self) -> CodecResult<()> {
        if self.schema_version != FIRST_STOP_SCHEMA {
            return Err(Fault::UnknownVersion);
        }
        for value in [&self.namespace, &self.ticket_id, &self.evaluator_actor] {
            name(value)?;
        }
        digest(&self.first_ticket_digest)?;
        digest(&self.member_terminal_digest)?;
        if self.first_next_sequence == 0
            || self.member_flags.is_empty()
            || self.member_flags.len() > MAX_NODES
            || !self
                .member_flags
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(Fault::InvalidContent);
        }
        self.certificate
            .validate_shape()
            .map_err(|_| Fault::InvalidContent)?;
        let certificate =
            serde_json::to_value(&self.certificate).map_err(|_| Fault::InvalidContent)?;
        if certificate
            .get("ticket_id")
            .and_then(serde_json::Value::as_str)
            != Some(self.ticket_id.as_str())
            || self.certificate.member_terminal_digest() != self.member_terminal_digest
            || self.certificate.usage_uncertain() != self.initially_draining
            || serde_json::to_vec(&certificate)
                .map_err(|_| Fault::InvalidContent)?
                .len()
                > 16384
        {
            return Err(Fault::InvalidContent);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeTicketEnvelopeV2<T> {
    pub schema_version: String,
    pub id: String,
    pub record_kind: String,
    pub payload: T,
    pub native_first_stop: Option<NativeFirstStopV2>,
}
// This is a strict data codec, NEVER a Current/role/native-write constructor.
pub fn native_ticket_payload<'a>(
    value: &'a serde_json::Value,
    namespace: &str,
    physical_id: &str,
) -> CodecResult<&'a serde_json::Value> {
    let object = value.as_object().ok_or(Fault::InvalidContent)?;
    if object.len() != 5
        || ![
            "schema_version",
            "id",
            "record_kind",
            "payload",
            "native_first_stop",
        ]
        .iter()
        .all(|key| object.contains_key(*key))
    {
        return Err(Fault::InvalidContent);
    }
    if value["schema_version"] != NATIVE_TICKET_ENVELOPE
        || value["id"] != physical_id
        || value["record_kind"] != "evaluation_ticket_v2"
    {
        return Err(Fault::InvalidIdentity);
    }
    let payload = value["payload"].as_object().ok_or(Fault::InvalidContent)?;
    if payload
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
        != Some("rsia.evaluation_ticket.v2")
    {
        return Err(Fault::UnknownVersion);
    }
    let members = payload
        .get("targets")
        .and_then(serde_json::Value::as_array)
        .ok_or(Fault::InvalidContent)?;
    if members.is_empty() || members.len() > MAX_NODES {
        return Err(Fault::Capacity);
    }
    let bytes = serde_json::to_vec(value)
        .map_err(|_| Fault::InvalidContent)?
        .len();
    if bytes > MAX_BYTES {
        return Err(Fault::Capacity);
    }
    if !value["native_first_stop"].is_null() {
        let source: NativeFirstStopV2 = serde_json::from_value(value["native_first_stop"].clone())
            .map_err(|_| Fault::InvalidContent)?;
        source.validate_known()?;
        if source.namespace != namespace
            || payload.get("id").and_then(serde_json::Value::as_str)
                != Some(source.ticket_id.as_str())
            || payload
                .get("evaluator_actor")
                .and_then(serde_json::Value::as_str)
                != Some(source.evaluator_actor.as_str())
            || source.member_flags.len() != members.len()
        {
            return Err(Fault::InvalidIdentity);
        }
    } else {
        // Reserve this codec's bounded first-stop proof BEFORE first issue or
        // any later live-ticket write, so stopping never needs a capacity waiver.
        let reserve = 32768_usize
            .checked_add(members.len().checked_mul(64).ok_or(Fault::Capacity)?)
            .ok_or(Fault::Capacity)?;
        if bytes.checked_add(reserve).ok_or(Fault::Capacity)? > MAX_BYTES {
            return Err(Fault::Capacity);
        }
    }
    Ok(&value["payload"])
}

pub const FORMAL_AUTHORITY_V3: &str = "rsia.issued_formal_authority.v3";
pub const PERSISTENT_RELEASE_V2: &str = "rsia.persistent_release.v2";
pub const VERIFIED_FORMAL_VIEW_V3: &str = "rsia.verified_formal_report_view.v3";
// Explicit envelopes do not reinterpret or rewrite any v2 authority/v1 release.
// These are data wires only, never a native permission or after-close capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormalAuthorityV3<A> {
    pub schema_version: String,
    pub id: String,
    pub report_ref: ReportRef,
    pub ticket_id: String,
    pub input_digest: String,
    pub authority_digest: String,
    pub authority: A,
}
pub fn formal_authority_id_v3(reference: &ReportRef, input_digest: &str) -> CodecResult<String> {
    reference.validate_wire()?;
    digest(input_digest)?;
    Ok(format!(
        "formal-authority-v3-{}",
        crate::fingerprint(&(FORMAL_AUTHORITY_V3, reference, input_digest))
            .map_err(|_| Fault::InvalidContent)?
    ))
}
impl<A: Serialize> FormalAuthorityV3<A> {
    pub fn validate_known(&self) -> CodecResult<()> {
        if self.schema_version != FORMAL_AUTHORITY_V3 {
            return Err(Fault::UnknownVersion);
        }
        self.report_ref.validate_wire()?;
        name(&self.ticket_id)?;
        digest(&self.input_digest)?;
        digest(&self.authority_digest)?;
        let actual = crate::fingerprint(&self.authority).map_err(|_| Fault::InvalidContent)?;
        if self.id != formal_authority_id_v3(&self.report_ref, &self.input_digest)?
            || self.ticket_id != self.report_ref.ticket_id
            || actual != self.authority_digest
        {
            return Err(Fault::InvalidIdentity);
        }
        let data = serde_json::to_value(&self.authority).map_err(|_| Fault::InvalidContent)?;
        if data["schema_version"] != "rsia.issued_formal_authority.v2"
            || data["id"] != self.id
            || data["report_id"] != self.ticket_id
            || data["ticket_digest"] != self.report_ref.ticket_digest
            || data["raw_report_digest"] != self.report_ref.payload_digest
            || data["root_seal_digest"] != self.input_digest
        {
            return Err(Fault::InvalidIdentity);
        }
        if serde_json::to_vec(self)
            .map_err(|_| Fault::InvalidContent)?
            .len()
            > MAX_BYTES
        {
            return Err(Fault::Capacity);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistentReleaseV2<R> {
    pub schema_version: String,
    pub id: String,
    pub report_ref: ReportRef,
    pub ticket_id: String,
    pub authority_id: String,
    pub approval_input_digest: String,
    pub release_digest: String,
    pub release: R,
}
pub fn persistent_release_id_v2(
    reference: &ReportRef,
    authority_id: &str,
    approval_input_digest: &str,
) -> CodecResult<String> {
    reference.validate_wire()?;
    name(authority_id)?;
    digest(approval_input_digest)?;
    Ok(format!(
        "release-v2-{}",
        crate::fingerprint(&(
            PERSISTENT_RELEASE_V2,
            reference,
            authority_id,
            approval_input_digest,
        ))
        .map_err(|_| Fault::InvalidContent)?
    ))
}
impl<R: Serialize> PersistentReleaseV2<R> {
    pub fn validate_known(&self) -> CodecResult<()> {
        if self.schema_version != PERSISTENT_RELEASE_V2 {
            return Err(Fault::UnknownVersion);
        }
        self.report_ref.validate_wire()?;
        name(&self.ticket_id)?;
        name(&self.authority_id)?;
        digest(&self.approval_input_digest)?;
        digest(&self.release_digest)?;
        if self.ticket_id != self.report_ref.ticket_id
            || self.id
                != persistent_release_id_v2(
                    &self.report_ref,
                    &self.authority_id,
                    &self.approval_input_digest,
                )?
            || self.release_digest
                != crate::fingerprint(&self.release).map_err(|_| Fault::InvalidContent)?
        {
            return Err(Fault::InvalidIdentity);
        }
        let data = serde_json::to_value(&self.release).map_err(|_| Fault::InvalidContent)?;
        if data["schema_version"] != "rsia.persistent_release.v1"
            || data["id"] != self.id
            || data["report_id"] != self.ticket_id
        {
            return Err(Fault::InvalidIdentity);
        }
        if serde_json::to_vec(self)
            .map_err(|_| Fault::InvalidContent)?
            .len()
            > MAX_BYTES
        {
            return Err(Fault::Capacity);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormalReportViewV3<V> {
    pub schema_version: String,
    pub report_ref: AnyReportRef,
    pub ticket_id: String,
    pub read_state: String,
    pub source_legacy: bool,
    pub view: V,
}
impl<V: Serialize> FormalReportViewV3<V> {
    pub fn validate_known(&self) -> CodecResult<()> {
        if self.schema_version != VERIFIED_FORMAL_VIEW_V3 {
            return Err(Fault::UnknownVersion);
        }
        name(&self.ticket_id)?;
        let id = match &self.report_ref {
            AnyReportRef::ImplicitLegacyV2(reference) => {
                reference.validate_wire()?;
                &reference.ticket_id
            }
            AnyReportRef::VersionedV3(reference) => {
                reference.validate_wire()?;
                &reference.ticket_id
            }
        };
        if id != &self.ticket_id
            || !matches!(self.read_state.as_str(), "current_verified" | "legacy")
            || self.source_legacy != (self.read_state == "legacy")
        {
            return Err(Fault::InvalidIdentity);
        }
        let historical = match &self.report_ref {
            AnyReportRef::ImplicitLegacyV2(_) => true,
            AnyReportRef::VersionedV3(reference) => matches!(
                version(
                    SCHEMA,
                    KIND,
                    Some(&reference.engine_version),
                    Some(&reference.input_schema_version)
                )?
                .policy,
                ReadPolicy::HistoricalNoEvaluator
            ),
        };
        if historical != self.source_legacy {
            return Err(Fault::InvalidIdentity);
        }
        // A wire can describe a state but cannot prove it. Only a private
        // same-native captured factory may populate current_verified.
        Ok(())
    }
}

/// Actual registered grader observation. Values received from a caller, wire,
/// Context or PID cannot construct this identity.
#[derive(Debug, Clone)]
pub struct NativeObservedProcessR49 {
    pid: u32,
    uid: u32,
    executable_digest: String,
    workspace: String,
}
impl NativeObservedProcessR49 {
    pub fn pid(&self) -> u32 {
        self.pid
    }
    pub fn uid(&self) -> u32 {
        self.uid
    }
    pub fn executable_digest(&self) -> &str {
        &self.executable_digest
    }
    pub fn workspace(&self) -> &str {
        &self.workspace
    }
}
pub fn observe_registered_process_r49(
    uid: u32,
    code: &str,
    workspace: &str,
) -> crate::Result<NativeObservedProcessR49> {
    use std::os::unix::fs::MetadataExt;
    let path = std::fs::canonicalize(workspace).map_err(|_| crate::Error::Forbidden)?;
    let cwd = std::env::current_dir().map_err(|_| crate::Error::Internal)?;
    let metadata = std::fs::metadata(&path).map_err(|_| crate::Error::Forbidden)?;
    if cwd != path || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(crate::Error::Forbidden);
    }
    let observed = std::process::Command::new("/usr/bin/id")
        .arg("-u")
        .env_clear()
        .output()
        .map_err(|_| crate::Error::Internal)?;
    if !observed.status.success() {
        return Err(crate::Error::Forbidden);
    }
    let observed_uid: u32 = std::str::from_utf8(&observed.stdout)
        .map_err(|_| crate::Error::Internal)?
        .trim()
        .parse()
        .map_err(|_| crate::Error::Internal)?;
    let executable = std::env::current_exe().map_err(|_| crate::Error::Internal)?;
    let bytes = std::fs::read(executable).map_err(|_| crate::Error::Internal)?;
    let actual_digest = crate::hash(&bytes);
    if observed_uid != uid || actual_digest != code {
        return Err(crate::Error::Forbidden);
    }
    Ok(NativeObservedProcessR49 {
        pid: std::process::id(),
        uid,
        executable_digest: actual_digest,
        workspace: path.to_string_lossy().into_owned(),
    })
}

pub const MAX_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_NODES: usize = 10_000;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Fault {
    UnknownVersion,
    InvalidIdentity,
    InvalidContent,
    Capacity,
    SourceInsufficient,
}
pub type CodecResult<T> = std::result::Result<T, Fault>;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadPolicy {
    HistoricalNoEvaluator,
    NativeVerificationRequired,
}
pub struct Version {
    pub schema: &'static str,
    pub kind: &'static str,
    pub engine: Option<&'static str>,
    pub input: Option<&'static str>,
    pub introduced: &'static str,
    pub policy: ReadPolicy,
}
pub static VERSIONS: &[Version] = &[
    Version {
        schema: OLD_SCHEMA,
        kind: OLD_KIND,
        engine: None,
        input: None,
        introduced: "implicit historical V2; real d52c4365 records; historical evaluator unavailable",
        policy: ReadPolicy::HistoricalNoEvaluator,
    },
    Version {
        schema: SCHEMA,
        kind: KIND,
        engine: Some(ENGINE),
        input: Some(INPUT_SCHEMA),
        introduced: "v4.3 frozen formal input / native-asof semantic revision; input-v1 lacks original exposure/root/group inputs; complete historical evaluator not retained",
        policy: ReadPolicy::HistoricalNoEvaluator,
    },
    Version {
        schema: SCHEMA,
        kind: KIND,
        engine: Some(ENGINE_V4),
        input: Some(INPUT_SCHEMA_V2),
        introduced: "input-v2 freezes the ticket exposure, original root/group and full R19 call snapshots; native verification remains required",
        policy: ReadPolicy::NativeVerificationRequired,
    },
];
pub fn version(
    schema: &str,
    kind: &str,
    engine: Option<&str>,
    input: Option<&str>,
) -> CodecResult<&'static Version> {
    VERSIONS
        .iter()
        .find(|v| v.schema == schema && v.kind == kind && v.engine == engine && v.input == input)
        .ok_or(Fault::UnknownVersion)
}
fn name(v: &str) -> CodecResult<()> {
    identifier(v).map_err(|_| Fault::InvalidIdentity)
}
fn digest(v: &str) -> CodecResult<()> {
    if v.len() == 64
        && v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(Fault::InvalidContent)
    }
}
fn fp<T: Serialize>(v: &T) -> CodecResult<String> {
    fingerprint(v).map_err(|_| Fault::InvalidContent)
}
fn bounded<T: Serialize>(v: &T) -> CodecResult<()> {
    if serde_json::to_vec(v)
        .map_err(|_| Fault::InvalidContent)?
        .len()
        <= MAX_BYTES
    {
        Ok(())
    } else {
        Err(Fault::Capacity)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Complete,
    EarlyStop,
    Invalidate,
    Reconcile,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fact {
    pub namespace: String,
    pub kind: String,
    pub id: String,
    pub owner: String,
    pub schema_version: String,
    pub record_kind: Option<String>,
    pub logical_id: String,
    pub raw_row_digest: String,
    pub raw_body_digest: String,
    pub typed_record_digest: String,
    pub outgoing: Vec<(String, String)>,
    pub outgoing_digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call<C> {
    pub namespace: String,
    pub billing_scope: String,
    pub call_id: String,
    pub snapshot_digest: String,
    pub snapshot: C,
}
/// T: complete original typed ticket. C: complete R19 FrozenCall, never a paid overlay.
/// The private producer must collect all qualified input facts in one actual capture.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenInput<T, C> {
    pub schema_version: String,
    pub namespace: String,
    pub ticket_id: String,
    pub ticket_digest: String,
    pub ticket_snapshot: T,
    pub operation: Operation,
    pub intent_digest: String,
    pub occurred_at_unix_ms: i64,
    pub facts: Vec<Fact>,
    pub calls: Vec<Call<C>>,
    pub watermark: Option<(i64, String)>,
}
impl<T: Serialize, C: Serialize> FrozenInput<T, C> {
    pub fn validate_wire(&self) -> CodecResult<()> {
        if self.schema_version != INPUT_SCHEMA {
            return Err(Fault::UnknownVersion);
        }
        name(&self.namespace)?;
        name(&self.ticket_id)?;
        digest(&self.ticket_digest)?;
        digest(&self.intent_digest)?;
        if self.occurred_at_unix_ms < 0 || fp(&self.ticket_snapshot)? != self.ticket_digest {
            return Err(Fault::InvalidContent);
        }
        if self.facts.is_empty() || self.facts.len() > MAX_NODES || self.calls.len() > MAX_NODES {
            return Err(Fault::Capacity);
        }
        if let Some((seq, d)) = &self.watermark {
            if *seq < 0 {
                return Err(Fault::InvalidContent);
            }
            digest(d)?;
        }
        let mut previous = None;
        for node in &self.facts {
            if node.namespace != self.namespace {
                return Err(Fault::InvalidIdentity);
            }
            for v in [
                &node.namespace,
                &node.kind,
                &node.id,
                &node.owner,
                &node.schema_version,
                &node.logical_id,
            ] {
                name(v)?;
            }
            if let Some(kind) = &node.record_kind {
                name(kind)?;
            }
            for d in [
                &node.raw_row_digest,
                &node.raw_body_digest,
                &node.typed_record_digest,
                &node.outgoing_digest,
            ] {
                digest(d)?;
            }
            let key = (&node.namespace, &node.kind, &node.id);
            if previous.is_some_and(|p| p >= key) {
                return Err(Fault::InvalidIdentity);
            }
            previous = Some(key);
            if node.outgoing.len() > MAX_NODES {
                return Err(Fault::Capacity);
            }
            let mut prior = None;
            for edge in &node.outgoing {
                name(&edge.0)?;
                name(&edge.1)?;
                if prior.is_some_and(|p| p >= edge) {
                    return Err(Fault::InvalidIdentity);
                }
                prior = Some(edge);
            }
            if fp(&node.outgoing)? != node.outgoing_digest {
                return Err(Fault::InvalidContent);
            }
        }
        let mut previous = None;
        for call in &self.calls {
            if call.namespace != self.namespace {
                return Err(Fault::InvalidIdentity);
            }
            for v in [&call.namespace, &call.billing_scope, &call.call_id] {
                name(v)?;
            }
            digest(&call.snapshot_digest)?;
            let key = (&call.namespace, &call.billing_scope, &call.call_id);
            if previous.is_some_and(|p| p >= key) {
                return Err(Fault::InvalidIdentity);
            }
            previous = Some(key);
            if fp(&call.snapshot)? != call.snapshot_digest {
                return Err(Fault::InvalidContent);
            }
        }
        bounded(self)
    }
}
pub fn address(
    namespace: &str,
    engine: &str,
    input_schema: &str,
    input_digest: &str,
) -> CodecResult<String> {
    name(namespace)?;
    digest(input_digest)?;
    version(SCHEMA, KIND, Some(engine), Some(input_schema))?;
    Ok(format!(
        "e05-report-{}",
        fp(&(SCHEMA, namespace, engine, input_schema, input_digest))?
    ))
}
pub fn old_lookup_address(ticket_id: &str) -> CodecResult<String> {
    name(ticket_id)?;
    Ok(format!("e05-{}", fp(&(OLD_KIND, ticket_id))?))
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportRef {
    pub schema_version: String,
    pub namespace: String,
    pub owner: String,
    pub physical_id: String,
    pub engine_version: String,
    pub input_schema_version: String,
    pub input_digest: String,
    pub payload_digest: String,
    pub carrier_digest: String,
    pub ticket_id: String,
    pub ticket_digest: String,
}
impl ReportRef {
    pub fn validate_wire(&self) -> CodecResult<()> {
        if self.schema_version != REF_SCHEMA {
            return Err(Fault::UnknownVersion);
        }
        for v in [
            &self.namespace,
            &self.owner,
            &self.physical_id,
            &self.ticket_id,
        ] {
            name(v)?;
        }
        for d in [
            &self.input_digest,
            &self.payload_digest,
            &self.carrier_digest,
            &self.ticket_digest,
        ] {
            digest(d)?;
        }
        if self.physical_id
            != address(
                &self.namespace,
                &self.engine_version,
                &self.input_schema_version,
                &self.input_digest,
            )?
            || self.physical_id == self.ticket_id
        {
            return Err(Fault::InvalidIdentity);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyRef {
    pub namespace: String,
    pub owner: String,
    pub physical_id: String,
    pub ticket_id: String,
    pub payload_digest: String,
}
impl LegacyRef {
    pub fn validate_wire(&self) -> CodecResult<()> {
        for v in [
            &self.namespace,
            &self.owner,
            &self.physical_id,
            &self.ticket_id,
        ] {
            name(v)?;
        }
        digest(&self.payload_digest)?;
        if self.physical_id != old_lookup_address(&self.ticket_id)? {
            return Err(Fault::InvalidIdentity);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "protocol",
    content = "reference",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum AnyReportRef {
    ImplicitLegacyV2(LegacyRef),
    VersionedV3(ReportRef),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Carrier<T, C, P> {
    pub schema_version: String,
    pub record_kind: String,
    pub id: String,
    pub engine_version: String,
    pub input_schema_version: String,
    pub input_digest: String,
    pub payload_digest: String,
    pub input: FrozenInput<T, C>,
    pub payload: P,
}
impl<T: Serialize, C: Serialize, P: Serialize> Carrier<T, C, P> {
    pub fn from_frozen_input(input: FrozenInput<T, C>, payload: P) -> CodecResult<Self> {
        input.validate_wire()?;
        let input_digest = fp(&input)?;
        let record = Self {
            schema_version: SCHEMA.into(),
            record_kind: KIND.into(),
            id: address(&input.namespace, ENGINE, INPUT_SCHEMA, &input_digest)?,
            engine_version: ENGINE.into(),
            input_schema_version: INPUT_SCHEMA.into(),
            input_digest,
            payload_digest: fp(&payload)?,
            input,
            payload,
        };
        bounded(&record)?;
        Ok(record)
    }
    pub fn validate_wire(&self, namespace: &str, id: &str) -> CodecResult<()> {
        if self.engine_version != ENGINE || self.input_schema_version != INPUT_SCHEMA {
            return Err(Fault::UnknownVersion);
        }
        version(
            &self.schema_version,
            &self.record_kind,
            Some(&self.engine_version),
            Some(&self.input_schema_version),
        )?;
        self.input.validate_wire()?;
        if self.input.namespace != namespace
            || self.id != id
            || self.id == self.input.ticket_id
            || address(
                namespace,
                &self.engine_version,
                &self.input_schema_version,
                &self.input_digest,
            )? != self.id
        {
            return Err(Fault::InvalidIdentity);
        }
        if self.input_digest != fp(&self.input)? || self.payload_digest != fp(&self.payload)? {
            return Err(Fault::InvalidContent);
        }
        bounded(self)
    }
    pub fn report_ref(&self, actual_owner: &str) -> CodecResult<ReportRef> {
        self.validate_wire(&self.input.namespace, &self.id)?;
        name(actual_owner)?;
        let reference = ReportRef {
            schema_version: REF_SCHEMA.into(),
            namespace: self.input.namespace.clone(),
            owner: actual_owner.into(),
            physical_id: self.id.clone(),
            engine_version: self.engine_version.clone(),
            input_schema_version: self.input_schema_version.clone(),
            input_digest: self.input_digest.clone(),
            payload_digest: self.payload_digest.clone(),
            carrier_digest: fp(self)?,
            ticket_id: self.input.ticket_id.clone(),
            ticket_digest: self.input.ticket_digest.clone(),
        };
        reference.validate_wire()?;
        Ok(reference)
    }
}
#[derive(Debug, Serialize)]
pub struct LineageAddresses {
    pub namespace: String,
    pub input_digest: String,
    pub known_versioned: Vec<(String, String, String)>,
    /// Lookup candidate only; original-input reconstruction is needed to assert ancestry.
    pub implicit_v2_lookup_candidate: String,
}
pub fn lineage_addresses(
    namespace: &str,
    ticket_id: &str,
    input_digest: &str,
) -> CodecResult<LineageAddresses> {
    name(namespace)?;
    digest(input_digest)?;
    let mut known_versioned = Vec::new();
    for row in VERSIONS {
        if let (Some(engine), Some(input)) = (row.engine, row.input) {
            known_versioned.push((
                engine.into(),
                input.into(),
                address(namespace, engine, input, input_digest)?,
            ));
        }
    }
    Ok(LineageAddresses {
        namespace: namespace.into(),
        input_digest: input_digest.into(),
        known_versioned,
        implicit_v2_lookup_candidate: old_lookup_address(ticket_id)?,
    })
}
/// Read-only diagnosis, never a new-use gate.
#[derive(Debug, Serialize)]
pub struct HistoricalDerivedView<T> {
    pub record: T,
    pub source_legacy: bool,
    pub reasons: Vec<String>,
}

// Fixed readonly material transport. It contains no Session, connection, SQL or authority.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyFormalMaterial {
    pub report_ref: LegacyRef,
    pub raw_body: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeCompleteContent<P> {
    pub report_ref: ReportRef,
    pub formal: P,
}

// Input-v2 is a separate codec. These types carry data, never native/as-of authority.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OriginalValueV2<V> {
    pub typed_digest: String,
    pub snapshot: V,
}
impl<V: Serialize> OriginalValueV2<V> {
    pub fn from_snapshot(snapshot: V) -> CodecResult<Self> {
        Ok(Self {
            typed_digest: fp(&snapshot)?,
            snapshot,
        })
    }
    pub fn validate_wire(&self) -> CodecResult<()> {
        digest(&self.typed_digest)?;
        if fp(&self.snapshot)? != self.typed_digest {
            return Err(Fault::InvalidContent);
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExposureProjectionV2 {
    pub schema_version: String,
    pub research_family_id: String,
    pub alpha_plan_digest: String,
    pub query_limit: u32,
    pub entry: crate::holdout::ExposureEntry,
}
impl ExposureProjectionV2 {
    pub fn validate_wire(&self, ticket_id: &str, occurred_at_ms: i64) -> CodecResult<()> {
        use crate::holdout::{ExposureState as E, MonetaryReservationState as M};
        if self.schema_version != crate::holdout::EXPOSURE_LEDGER_SCHEMA {
            return Err(Fault::UnknownVersion);
        }
        name(&self.research_family_id)?;
        digest(&self.alpha_plan_digest)?;
        if self.query_limit == 0
            || self.entry.task_cluster_ids.is_empty()
            || self.entry.task_cluster_ids.len() > MAX_NODES
        {
            return Err(Fault::Capacity);
        }
        let entry = &self.entry;
        for v in [&entry.ticket_id, &entry.epoch_id, &entry.slice_id] {
            name(v)?;
        }
        if entry.ticket_id != ticket_id {
            return Err(Fault::InvalidIdentity);
        }
        digest(&entry.manifest_digest)?;
        digest(&entry.candidate_pair_digest)?;
        let mut clusters = std::collections::BTreeSet::new();
        for id in &entry.task_cluster_ids {
            name(id)?;
            if !clusters.insert(id) {
                return Err(Fault::InvalidIdentity);
            }
        }
        match (&entry.state, entry.monetary_state) {
            (E::Reserved, M::HeldUndispatched | M::ReleasedUndispatched)
            | (
                E::Dispatched { .. } | E::FeedbackUsed { .. },
                M::DispatchedCostPending | M::Finalized,
            ) => {}
            _ => return Err(Fault::InvalidContent),
        }
        let at = match &entry.state {
            E::Reserved => None,
            E::Dispatched {
                dispatched_at_unix_ms,
            } => Some(*dispatched_at_unix_ms),
            E::FeedbackUsed { used_at_unix_ms } => Some(*used_at_unix_ms),
        };
        if at.is_some_and(|at| at < 0 || at > occurred_at_ms) {
            return Err(Fault::InvalidContent);
        }
        Ok(())
    }
}
// Exact 16-field RootBudgetRecord projection; no schema or paid-overlay field is added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootBudgetSnapshotV2 {
    pub root_budget_id: String,
    pub billing_scope: String,
    pub authorizing_namespace: String,
    pub allowed_namespaces: Vec<String>,
    pub currency: String,
    pub pricing_version: String,
    pub payment_subject: String,
    pub authorization_receipt_digest: String,
    pub per_call_cap_micros: i64,
    pub total_limit_micros: i64,
    pub spent_micros: i64,
    pub reserved_micros: i64,
    pub stopped: bool,
    pub stop_reason: Option<String>,
    pub stop_committed_at: Option<i64>,
    pub created_at: i64,
}
fn frozen_seconds_v2(seconds: i64, event_ms: i64) -> CodecResult<()> {
    if seconds < 0 || seconds.checked_mul(1000).is_none_or(|ms| ms > event_ms) {
        return Err(Fault::InvalidContent);
    }
    Ok(())
}
fn frozen_stop_v2(stopped: bool, reason: &Option<String>, at: Option<i64>) -> CodecResult<()> {
    if (stopped && (reason.is_none() || at.is_none()))
        || (!stopped && (reason.is_some() || at.is_some()))
    {
        return Err(Fault::InvalidContent);
    }
    Ok(())
}
impl RootBudgetSnapshotV2 {
    pub fn validate_wire(&self, event_ms: i64) -> CodecResult<()> {
        for v in [
            &self.root_budget_id,
            &self.billing_scope,
            &self.authorizing_namespace,
            &self.currency,
            &self.pricing_version,
            &self.payment_subject,
        ] {
            name(v)?;
        }
        digest(&self.authorization_receipt_digest)?;
        if self.allowed_namespaces.is_empty() || self.allowed_namespaces.len() > 32 {
            return Err(Fault::Capacity);
        }
        let mut prior = None;
        for namespace in &self.allowed_namespaces {
            name(namespace)?;
            if prior.is_some_and(|p| p >= namespace) {
                return Err(Fault::InvalidIdentity);
            }
            prior = Some(namespace);
        }
        if !self
            .allowed_namespaces
            .contains(&self.authorizing_namespace)
        {
            return Err(Fault::InvalidIdentity);
        }
        if self.per_call_cap_micros <= 0
            || self.total_limit_micros < self.per_call_cap_micros
            || self.spent_micros < 0
            || self.reserved_micros < 0
        {
            return Err(Fault::InvalidContent);
        }
        // A stopped root, over-budget accounting and later reconciliation remain legal.
        frozen_stop_v2(self.stopped, &self.stop_reason, self.stop_committed_at)?;
        frozen_seconds_v2(self.created_at, event_ms)?;
        if let Some(at) = self.stop_committed_at {
            frozen_seconds_v2(at, event_ms)?;
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchGroupSnapshotV2 {
    pub billing_scope: String,
    pub dispatch_group_id: String,
    pub owner_namespace: String,
    pub stopped: bool,
    pub stop_reason: Option<String>,
    pub stop_committed_at: Option<i64>,
    pub created_at: i64,
}
impl DispatchGroupSnapshotV2 {
    pub fn validate_wire(&self, event_ms: i64) -> CodecResult<()> {
        for v in [
            &self.billing_scope,
            &self.dispatch_group_id,
            &self.owner_namespace,
        ] {
            name(v)?;
        }
        frozen_stop_v2(self.stopped, &self.stop_reason, self.stop_committed_at)?;
        frozen_seconds_v2(self.created_at, event_ms)?;
        if let Some(at) = self.stop_committed_at {
            frozen_seconds_v2(at, event_ms)?;
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrozenResourceScopeV2 {
    TicketExecutionOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeMutableProjectionKindV2 {
    TicketSnapshot,
    TicketExposure,
}
/// A native writer authenticates a frozen value projection, not retained bytes
/// of the whole shared ledger. Full raw/typed hashes remain original-source
/// provenance only; this DTO is never an authority or a substitute old body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeMutableProjectionV2 {
    pub kind: String,
    pub id: String,
    pub projection: NativeMutableProjectionKindV2,
    pub projection_digest: String,
    pub original_row_digest: String,
    pub original_body_digest: String,
    pub original_typed_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeExposureCaptureV2 {
    pub schema_version: String,
    pub namespace: String,
    pub physical_id: String,
    pub original_owner: String,
    pub ledger_schema_version: String,
    pub raw_row_digest: String,
    pub raw_body_digest: String,
    pub typed_ledger_digest: String,
    pub projection_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenInputV2<T, C> {
    pub schema_version: String,
    pub namespace: String,
    pub ticket_id: String,
    pub ticket_digest: String,
    pub ticket_snapshot: T,
    pub operation: Operation,
    pub intent_digest: String,
    pub occurred_at_unix_ms: i64,
    pub facts: Vec<Fact>,
    pub calls: Vec<Call<C>>,
    pub watermark: Option<(i64, String)>,
    pub resource_scope: FrozenResourceScopeV2,
    pub mutable_fact_projections: Vec<NativeMutableProjectionV2>,
    pub native_exposure_capture: Option<NativeExposureCaptureV2>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_first_stop: Option<OriginalValueV2<NativeFirstStopV2>>,
    pub exposure: Option<OriginalValueV2<ExposureProjectionV2>>,
    pub root_budget: Option<OriginalValueV2<RootBudgetSnapshotV2>>,
    pub dispatch_group: Option<OriginalValueV2<DispatchGroupSnapshotV2>>,
}
// C is the concrete original R19 FrozenCall (all 29 values + 3 artifact refs).
// A storage decoder must run its known-descriptor checks before collecting missing originals.
// This wire method cannot replace that concrete validator or mint a native permit.
impl<T: Serialize, C: Serialize> FrozenInputV2<T, C> {
    pub fn validate_known_wire(&self) -> CodecResult<()> {
        if self.schema_version != INPUT_SCHEMA_V2 {
            return Err(Fault::UnknownVersion);
        }
        name(&self.namespace)?;
        name(&self.ticket_id)?;
        digest(&self.ticket_digest)?;
        digest(&self.intent_digest)?;
        if self.occurred_at_unix_ms < 0 || fp(&self.ticket_snapshot)? != self.ticket_digest {
            return Err(Fault::InvalidContent);
        }
        if self.facts.is_empty() || self.facts.len() > MAX_NODES || self.calls.len() > MAX_NODES {
            return Err(Fault::Capacity);
        }
        if let Some((seq, d)) = &self.watermark {
            if *seq < 0 {
                return Err(Fault::InvalidContent);
            }
            digest(d)?;
        }
        let mut previous = None;
        for node in &self.facts {
            if node.namespace != self.namespace {
                return Err(Fault::InvalidIdentity);
            }
            for v in [
                &node.namespace,
                &node.kind,
                &node.id,
                &node.owner,
                &node.schema_version,
                &node.logical_id,
            ] {
                name(v)?;
            }
            if let Some(kind) = &node.record_kind {
                name(kind)?;
            }
            for d in [
                &node.raw_row_digest,
                &node.raw_body_digest,
                &node.typed_record_digest,
                &node.outgoing_digest,
            ] {
                digest(d)?;
            }
            let key = (&node.namespace, &node.kind, &node.id);
            if previous.is_some_and(|p| p >= key) {
                return Err(Fault::InvalidIdentity);
            }
            previous = Some(key);
            if node.outgoing.len() > MAX_NODES {
                return Err(Fault::Capacity);
            }
            let mut prior = None;
            for edge in &node.outgoing {
                name(&edge.0)?;
                name(&edge.1)?;
                if prior.is_some_and(|p| p >= edge) {
                    return Err(Fault::InvalidIdentity);
                }
                prior = Some(edge);
            }
            if fp(&node.outgoing)? != node.outgoing_digest {
                return Err(Fault::InvalidContent);
            }
        }
        let mut previous = None;
        for call in &self.calls {
            if call.namespace != self.namespace {
                return Err(Fault::InvalidIdentity);
            }
            for v in [&call.namespace, &call.billing_scope, &call.call_id] {
                name(v)?;
            }
            digest(&call.snapshot_digest)?;
            let key = (&call.namespace, &call.billing_scope, &call.call_id);
            if previous.is_some_and(|p| p >= key) {
                return Err(Fault::InvalidIdentity);
            }
            previous = Some(key);
            if fp(&call.snapshot)? != call.snapshot_digest {
                return Err(Fault::InvalidContent);
            }
        }
        if let Some(exposure) = &self.exposure {
            exposure.validate_wire()?;
            exposure
                .snapshot
                .validate_wire(&self.ticket_id, self.occurred_at_unix_ms)?;
        }
        if let Some(root) = &self.root_budget {
            root.validate_wire()?;
            root.snapshot.validate_wire(self.occurred_at_unix_ms)?;
            if !root.snapshot.allowed_namespaces.contains(&self.namespace) {
                return Err(Fault::InvalidIdentity);
            }
        }
        if let Some(group) = &self.dispatch_group {
            group.validate_wire()?;
            group.snapshot.validate_wire(self.occurred_at_unix_ms)?;
            if group.snapshot.owner_namespace != self.namespace
                || group.snapshot.dispatch_group_id != self.ticket_id
            {
                return Err(Fault::InvalidIdentity);
            }
        }
        if let (Some(root), Some(group)) = (&self.root_budget, &self.dispatch_group) {
            if root.snapshot.billing_scope != group.snapshot.billing_scope {
                return Err(Fault::InvalidIdentity);
            }
        }
        for call in &self.calls {
            if self
                .root_budget
                .as_ref()
                .is_some_and(|root| root.snapshot.billing_scope != call.billing_scope)
                || self
                    .dispatch_group
                    .as_ref()
                    .is_some_and(|group| group.snapshot.billing_scope != call.billing_scope)
            {
                return Err(Fault::InvalidIdentity);
            }
        }
        // Validate every knowable descriptor before any optional source gap.
        if self.mutable_fact_projections.len() > MAX_NODES {
            return Err(Fault::Capacity);
        }
        let mut previous = None;
        for source in &self.mutable_fact_projections {
            let key = (&source.kind, &source.id);
            name(&source.kind)?;
            name(&source.id)?;
            if previous.is_some_and(|old| old >= key) {
                return Err(Fault::InvalidIdentity);
            }
            previous = Some(key);
            for value in [
                &source.projection_digest,
                &source.original_row_digest,
                &source.original_body_digest,
                &source.original_typed_digest,
            ] {
                digest(value)?;
            }
            let fact = self
                .facts
                .iter()
                .find(|fact| fact.kind == source.kind && fact.id == source.id)
                .ok_or(Fault::InvalidIdentity)?;
            if source.original_row_digest != fact.raw_row_digest
                || source.original_body_digest != fact.raw_body_digest
                || source.original_typed_digest != fact.typed_record_digest
            {
                return Err(Fault::InvalidContent);
            }
            let expected = match source.projection {
                NativeMutableProjectionKindV2::TicketSnapshot => Some(fp(&self.ticket_snapshot)?),
                NativeMutableProjectionKindV2::TicketExposure => self
                    .exposure
                    .as_ref()
                    .map(|value| fp(&value.snapshot))
                    .transpose()?,
            };
            if expected.is_some_and(|expected| expected != source.projection_digest) {
                return Err(Fault::InvalidContent);
            }
        }
        if let Some(source) = &self.native_exposure_capture {
            if source.schema_version != "rsia.e05.native_exposure_capture.v1"
                || source.namespace != self.namespace
                || source.ledger_schema_version != "rsia.exposure_ledger.v1"
            {
                return Err(Fault::InvalidIdentity);
            }
            for value in [&source.physical_id, &source.original_owner] {
                name(value)?;
            }
            for value in [
                &source.raw_row_digest,
                &source.raw_body_digest,
                &source.typed_ledger_digest,
                &source.projection_digest,
            ] {
                digest(value)?;
            }
            if let Some(exposure) = &self.exposure
                && source.projection_digest != fp(&exposure.snapshot)?
            {
                return Err(Fault::InvalidContent);
            }
        }
        if let Some(source) = &self.native_first_stop {
            source.snapshot.validate_known()?;
            if source.typed_digest != fp(&source.snapshot)?
                || source.snapshot.namespace != self.namespace
                || source.snapshot.ticket_id != self.ticket_id
            {
                return Err(Fault::InvalidIdentity);
            }
        }
        bounded(self)
    }
    fn require_sources(&self) -> CodecResult<()> {
        if matches!(self.operation, Operation::EarlyStop | Operation::Reconcile)
            && self.native_first_stop.is_none()
        {
            return Err(Fault::SourceInsufficient);
        }
        if self.exposure.is_none() || self.root_budget.is_none() || self.dispatch_group.is_none() {
            return Err(Fault::SourceInsufficient);
        }
        Ok(())
    }
    pub fn validate_wire(&self) -> CodecResult<()> {
        self.validate_known_wire()?;
        self.require_sources()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CarrierV4<T, C, P> {
    pub schema_version: String,
    pub record_kind: String,
    pub id: String,
    pub engine_version: String,
    pub input_schema_version: String,
    pub input_digest: String,
    pub payload_digest: String,
    pub input: FrozenInputV2<T, C>,
    pub payload: P,
}
impl<T: Serialize, C: Serialize, P: Serialize> CarrierV4<T, C, P> {
    pub fn from_frozen_input(input: FrozenInputV2<T, C>, payload: P) -> CodecResult<Self> {
        input.validate_known_wire()?;
        let input_digest = fp(&input)?;
        let record = Self {
            schema_version: SCHEMA.into(),
            record_kind: KIND.into(),
            id: address(&input.namespace, ENGINE_V4, INPUT_SCHEMA_V2, &input_digest)?,
            engine_version: ENGINE_V4.into(),
            input_schema_version: INPUT_SCHEMA_V2.into(),
            input_digest,
            payload_digest: fp(&payload)?,
            input,
            payload,
        };
        bounded(&record)?;
        record.input.require_sources()?;
        Ok(record)
    }
    pub fn validate_known_wire(&self, namespace: &str, id: &str) -> CodecResult<()> {
        if self.schema_version != SCHEMA
            || self.record_kind != KIND
            || self.engine_version != ENGINE_V4
            || self.input_schema_version != INPUT_SCHEMA_V2
        {
            return Err(Fault::UnknownVersion);
        }
        version(
            &self.schema_version,
            &self.record_kind,
            Some(&self.engine_version),
            Some(&self.input_schema_version),
        )?;
        self.input.validate_known_wire()?;
        if self.input.namespace != namespace
            || self.id != id
            || self.id == self.input.ticket_id
            || address(namespace, ENGINE_V4, INPUT_SCHEMA_V2, &self.input_digest)? != self.id
        {
            return Err(Fault::InvalidIdentity);
        }
        if self.input_digest != fp(&self.input)? || self.payload_digest != fp(&self.payload)? {
            return Err(Fault::InvalidContent);
        }
        bounded(self)
    }
    pub fn validate_wire(&self, namespace: &str, id: &str) -> CodecResult<()> {
        self.validate_known_wire(namespace, id)?;
        self.input.require_sources()
    }
    pub fn report_ref(&self, actual_owner: &str) -> CodecResult<ReportRef> {
        self.validate_wire(&self.input.namespace, &self.id)?;
        name(actual_owner)?;
        let reference = ReportRef {
            schema_version: REF_SCHEMA.into(),
            namespace: self.input.namespace.clone(),
            owner: actual_owner.into(),
            physical_id: self.id.clone(),
            engine_version: self.engine_version.clone(),
            input_schema_version: self.input_schema_version.clone(),
            input_digest: self.input_digest.clone(),
            payload_digest: self.payload_digest.clone(),
            carrier_digest: fp(self)?,
            ticket_id: self.input.ticket_id.clone(),
            ticket_digest: self.input.ticket_digest.clone(),
        };
        reference.validate_wire()?;
        Ok(reference)
    }
}

#[cfg(test)]
mod codec_tests_r36 {
    use super::*;
    use crate::holdout::{ExposureEntry, ExposureState, MonetaryReservationState};
    use serde_json::{Value, json};

    type NewCarrier = CarrierV4<Value, Value, Value>;

    // Codec-only fixtures: these values never stand in for a native producer or old record.
    fn frozen_input() -> FrozenInputV2<Value, Value> {
        let ticket = json!({"ticket_id": "ticket", "candidate": "candidate"});
        let outgoing: Vec<(String, String)> = vec![];
        let call = json!({"request": "original"});
        let root = RootBudgetSnapshotV2 {
            root_budget_id: "root".into(),
            billing_scope: "billing".into(),
            authorizing_namespace: "namespace".into(),
            allowed_namespaces: vec!["namespace".into()],
            currency: "USD".into(),
            pricing_version: "pricing".into(),
            payment_subject: "subject".into(),
            authorization_receipt_digest: "a".repeat(64),
            per_call_cap_micros: 1,
            total_limit_micros: 10,
            spent_micros: 20,
            reserved_micros: 2,
            stopped: true,
            stop_reason: Some("first_stop".into()),
            stop_committed_at: Some(2),
            created_at: 1,
        };
        let group = DispatchGroupSnapshotV2 {
            billing_scope: "billing".into(),
            dispatch_group_id: "ticket".into(),
            owner_namespace: "namespace".into(),
            stopped: true,
            stop_reason: Some("first_group_stop".into()),
            stop_committed_at: Some(2),
            created_at: 1,
        };
        let exposure = ExposureProjectionV2 {
            schema_version: crate::holdout::EXPOSURE_LEDGER_SCHEMA.into(),
            research_family_id: "family".into(),
            alpha_plan_digest: "a".repeat(64),
            query_limit: 1,
            entry: ExposureEntry {
                ticket_id: "ticket".into(),
                manifest_digest: "b".repeat(64),
                candidate_pair_digest: "c".repeat(64),
                epoch_id: "epoch".into(),
                slice_id: "slice".into(),
                task_cluster_ids: vec!["cluster".into()],
                state: ExposureState::FeedbackUsed {
                    used_at_unix_ms: 5_000,
                },
                monetary_state: MonetaryReservationState::Finalized,
            },
        };
        FrozenInputV2 {
            schema_version: INPUT_SCHEMA_V2.into(),
            namespace: "namespace".into(),
            ticket_id: "ticket".into(),
            ticket_digest: fp(&ticket).unwrap(),
            ticket_snapshot: ticket,
            operation: Operation::Complete,
            native_first_stop: None,
            intent_digest: fp(&"intent").unwrap(),
            occurred_at_unix_ms: 10_000,
            facts: vec![Fact {
                namespace: "namespace".into(),
                kind: "fixture".into(),
                id: "fact".into(),
                owner: "evaluator".into(),
                schema_version: "rsia.fixture.v1".into(),
                record_kind: Some("fixture".into()),
                logical_id: "fact".into(),
                raw_row_digest: "a".repeat(64),
                raw_body_digest: "b".repeat(64),
                typed_record_digest: "c".repeat(64),
                outgoing_digest: fp(&outgoing).unwrap(),
                outgoing,
            }],
            calls: vec![Call {
                namespace: "namespace".into(),
                billing_scope: "billing".into(),
                call_id: "call".into(),
                snapshot_digest: fp(&call).unwrap(),
                snapshot: call,
            }],
            watermark: Some((1, "a".repeat(64))),
            resource_scope: FrozenResourceScopeV2::TicketExecutionOnly,
            mutable_fact_projections: Vec::new(),
            native_exposure_capture: None,
            exposure: Some(OriginalValueV2::from_snapshot(exposure).unwrap()),
            root_budget: Some(OriginalValueV2::from_snapshot(root).unwrap()),
            dispatch_group: Some(OriginalValueV2::from_snapshot(group).unwrap()),
        }
    }

    fn carrier() -> NewCarrier {
        CarrierV4::from_frozen_input(frozen_input(), json!({"decision": "complete"})).unwrap()
    }

    fn reseal_input(record: &mut NewCarrier) {
        record.input_digest = fp(&record.input).unwrap();
        record.id = address(
            &record.input.namespace,
            ENGINE_V4,
            INPUT_SCHEMA_V2,
            &record.input_digest,
        )
        .unwrap();
    }

    #[test]
    fn versions_reject_mixed_wire_tuples_and_derive_distinct_addresses() {
        let new = carrier();
        let v = new.input.clone();
        let old = Carrier::from_frozen_input(
            FrozenInput {
                schema_version: INPUT_SCHEMA.into(),
                namespace: v.namespace,
                ticket_id: v.ticket_id,
                ticket_digest: v.ticket_digest,
                ticket_snapshot: v.ticket_snapshot,
                operation: v.operation,
                intent_digest: v.intent_digest,
                occurred_at_unix_ms: v.occurred_at_unix_ms,
                facts: v.facts,
                calls: v.calls,
                watermark: v.watermark,
            },
            new.payload.clone(),
        )
        .unwrap();
        old.validate_wire("namespace", &old.id).unwrap();
        new.validate_wire("namespace", &new.id).unwrap();
        assert_ne!(old.id, new.id);
        assert_ne!(
            address("namespace", ENGINE, INPUT_SCHEMA, &new.input_digest).unwrap(),
            new.id,
        );
        let old_bytes = serde_json::to_vec(&old).unwrap();
        let mut mixed_old = old.clone();
        mixed_old.engine_version = ENGINE_V4.into();
        mixed_old.input_schema_version = INPUT_SCHEMA_V2.into();
        assert_eq!(
            mixed_old.validate_wire("namespace", &mixed_old.id),
            Err(Fault::UnknownVersion)
        );
        let mut mixed_new = new.clone();
        mixed_new.engine_version = ENGINE.into();
        mixed_new.input_schema_version = INPUT_SCHEMA.into();
        assert_eq!(
            mixed_new.validate_wire("namespace", &mixed_new.id),
            Err(Fault::UnknownVersion)
        );
        mixed_new = new.clone();
        mixed_new.input.schema_version = INPUT_SCHEMA.into();
        assert_eq!(
            mixed_new.validate_wire("namespace", &mixed_new.id),
            Err(Fault::UnknownVersion)
        );
        assert!(matches!(
            version(SCHEMA, KIND, Some("unknown_engine"), Some(INPUT_SCHEMA_V2)),
            Err(Fault::UnknownVersion)
        ));
        assert_eq!(serde_json::to_vec(&old).unwrap(), old_bytes);
    }

    #[test]
    fn absent_original_does_not_mask_known_payload_identity_or_snapshot_faults() {
        let mut absent = carrier();
        absent.input.exposure = None;
        reseal_input(&mut absent);
        assert_eq!(
            absent.validate_wire("namespace", &absent.id),
            Err(Fault::SourceInsufficient)
        );

        let mut bad_payload = absent.clone();
        bad_payload.payload = json!({"decision": "forged"});
        assert_eq!(
            bad_payload.validate_wire("namespace", &bad_payload.id),
            Err(Fault::InvalidContent)
        );

        let mut bad_group = absent.clone();
        let group = &mut bad_group.input.dispatch_group.as_mut().unwrap().snapshot;
        group.owner_namespace = "other_namespace".into();
        let group_digest = fp(group).unwrap();
        bad_group
            .input
            .dispatch_group
            .as_mut()
            .unwrap()
            .typed_digest = group_digest;
        reseal_input(&mut bad_group);
        assert_eq!(
            bad_group.validate_wire("namespace", &bad_group.id),
            Err(Fault::InvalidIdentity)
        );

        let mut bad_call = absent.clone();
        bad_call.input.calls[0].snapshot = json!({"request": "substituted"});
        reseal_input(&mut bad_call);
        assert_eq!(
            bad_call.validate_wire("namespace", &bad_call.id),
            Err(Fault::InvalidContent)
        );

        let mut bad_projection = absent;
        bad_projection
            .input
            .root_budget
            .as_mut()
            .unwrap()
            .snapshot
            .currency = "changed".into();
        reseal_input(&mut bad_projection);
        assert_eq!(
            bad_projection.validate_wire("namespace", &bad_projection.id),
            Err(Fault::InvalidContent)
        );
    }

    #[test]
    fn closed_over_budget_original_is_readable_without_replacing_first_stop() {
        let record = carrier();
        let original = serde_json::to_vec(&record).unwrap();
        record.validate_wire("namespace", &record.id).unwrap();
        let root = &record.input.root_budget.as_ref().unwrap().snapshot;
        assert!(root.stopped && root.spent_micros > root.total_limit_micros);
        assert_eq!(root.stop_reason.as_deref(), Some("first_stop"));
        assert_eq!(root.stop_committed_at, Some(2));
        assert_eq!(serde_json::to_vec(&record).unwrap(), original);

        let mut contradictory = record.clone();
        let root = &mut contradictory.input.root_budget.as_mut().unwrap().snapshot;
        root.stopped = false;
        let digest = fp(root).unwrap();
        contradictory
            .input
            .root_budget
            .as_mut()
            .unwrap()
            .typed_digest = digest;
        reseal_input(&mut contradictory);
        assert_eq!(
            contradictory.validate_wire("namespace", &contradictory.id),
            Err(Fault::InvalidContent)
        );
    }

    #[test]
    fn byte_and_node_caps_remain_fail_closed() {
        let input = frozen_input();
        assert!(matches!(
            CarrierV4::from_frozen_input(input.clone(), "a".repeat(MAX_BYTES)),
            Err(Fault::Capacity)
        ));
        let mut too_many = input;
        too_many.facts = vec![too_many.facts[0].clone(); MAX_NODES + 1];
        too_many.exposure = None;
        assert_eq!(too_many.validate_wire(), Err(Fault::Capacity));
    }
}
