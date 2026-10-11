//! Bounded, read-only diagnostics of one E16 subject's declared content closure.
//! The report describes a single Session transaction snapshot. It is neither a
//! read/permission gate nor durable evidence, and does not repair or discover a
//! namespace. Blob bytes, user files, original aggregate semantics, model safety
//! and non-E16 learning-chain objects are outside this inspection. Watermark
//! comparison is diagnostic only; it does not replace existing tombstone/live
//! read gates or promise validity at the time the report is returned.
//!
//! The SQL point-read of a JSON body retains the existing storage body-size cost;
//! row budgets are not absolute CPU, memory, byte or wall-clock bounds. SQL and
//! lower-level JSON errors propagate unchanged. Only successfully read JSON is
//! classified here, without returning its content or parser error text.

use crate::import::{
    IMPORT_REGISTRATION_SCHEMA, IMPORT_RESULT_SCHEMA, IMPORT_SOURCE_SCHEMA, ImportEnvelope,
    ImportRegistrationRequest, ImportResultRecord, ImportResultState, ImportSourceReadStatus,
    ImportSourceRecord, ImportSourceSpec, ImportTypedRef, SOURCE_SELECTION_SCHEMA,
    SourceSelectionRecord,
};
use evo_core::evidence::{
    ExecutionAttestation, MAX_DISCOVERED_FILES, MAX_TOTAL_READ_BYTES, TaskOrigin,
};
use evo_core::{Context, Error, Result, Role, fingerprint, hash, identifier};
use evo_storage::dependency_read::{DirectEdge, MAX_DIRECT_EDGE_PAGE};
use evo_storage::{Session, Store};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const E16_EDGE_INSPECTION_SCHEMA: &str = "rsia.e16.edge_inspection.v1";
pub const MAX_INSPECTION_OBJECTS: usize = 202;
pub const MAX_INSPECTION_EDGE_ROWS: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectionBudget {
    pub max_objects: usize,
    pub max_edge_rows: usize,
    pub page_size: usize,
}

impl Default for InspectionBudget {
    fn default() -> Self {
        Self {
            max_objects: MAX_INSPECTION_OBJECTS,
            max_edge_rows: MAX_INSPECTION_EDGE_ROWS,
            page_size: MAX_DIRECT_EDGE_PAGE,
        }
    }
}

impl InspectionBudget {
    fn validate(self) -> Result<()> {
        if self.max_objects > MAX_INSPECTION_OBJECTS
            || self.max_edge_rows > MAX_INSPECTION_EDGE_ROWS
            || !(1..=MAX_DIRECT_EDGE_PAGE).contains(&self.page_size)
        {
            return Err(Error::Invalid("invalid E16 inspection budget".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InspectionStatus {
    Complete,
    Anomalous,
    Unknown,
    HistoryOnly,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum E16Wire {
    SourceSelection,
    ImportSource,
    ImportResult,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectScope {
    DeclaredContentAndEdges,
    /// Only the source's selection backlink is checked; other sources belonging
    /// to that selection are not added to a source-only inspection.
    SelectionBacklinkAuthority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueClass {
    Anomaly,
    Unknown,
    History,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueCode {
    SubjectMissing,
    MissingAuthorityObject,
    UnknownSchema,
    MalformedRecord,
    ConflictingReferences,
    InputDigestMismatch,
    IdentityMismatch,
    AuthorityKindMismatch,
    AuthorityRelationMismatch,
    ImmutableAuthorityDigestMismatch,
    PendingOrMismatchedAuthority,
    AuthorityNotCheckable,
    HistoricalDeclarationsOnly,
    RedactedAuthority,
    MissingDeclaredEdge,
    UnexpectedExistingEdge,
    MalformedExistingEdge,
    ObjectBudgetExhausted,
    EdgeBudgetExhausted,
    DeclarationScopeExceeded,
    WatermarkUnavailable,
    WatermarkDigestNotCheckable,
    WatermarkChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectionIssue {
    pub class: IssueClass,
    pub code: IssueCode,
    /// Only a syntactically valid opaque typed identity; never a parser error,
    /// owner, path, content string, observed-digest reference or inferred object.
    pub target: Option<DirectEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectedObject {
    pub id: String,
    pub database_kind: String,
    pub wire: E16Wire,
    pub scope: ObjectScope,
    pub status: InspectionStatus,
    pub typed_digest: Option<String>,
    pub declarations_checked: bool,
    pub declared_edges: Vec<DirectEdge>,
    pub direct_edges_exhausted: bool,
    pub edge_rows_read: usize,
    pub edge_rows_returned: usize,
    pub authorities_checked: bool,
    pub issues: Vec<InspectionIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectionWatermark {
    pub sequence: u64,
    /// Storage accepts arbitrary identifier markers here. Only a SHA-256 value
    /// is returned as a digest; other markers are withheld and remain unknown.
    pub digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct E16EdgeInspection {
    pub schema_version: String,
    pub subject_id: String,
    pub status: InspectionStatus,
    pub budget: InspectionBudget,
    pub object_reads: usize,
    pub edge_rows_read: usize,
    pub namespace_watermark: Option<InspectionWatermark>,
    /// Always not_checked: even Ready declarations do not cause a file/blob read.
    pub physical_blob_check: String,
    pub snapshot_scope: String,
    /// Objects are ordered by database kind/id; issues and declarations are
    /// ordered independently of JSON object key order and SQL insertion order.
    pub objects: Vec<InspectedObject>,
}

enum Record {
    Selection(SourceSelectionRecord),
    Source(ImportSourceRecord),
    Result(Box<ImportResultRecord>),
}

struct Node {
    report: InspectedObject,
    record: Option<Record>,
    refs: Vec<ImportTypedRef>,
    history: bool,
    read: bool,
}

impl Node {
    fn new(id: &str, scope: ObjectScope) -> Self {
        Self {
            report: InspectedObject {
                id: id.into(),
                database_kind: "artifact".into(),
                wire: E16Wire::Unknown,
                scope,
                status: InspectionStatus::Unknown,
                typed_digest: None,
                declarations_checked: false,
                declared_edges: vec![],
                direct_edges_exhausted: false,
                edge_rows_read: 0,
                edge_rows_returned: 0,
                authorities_checked: false,
                issues: vec![],
            },
            record: None,
            refs: vec![],
            history: false,
            read: false,
        }
    }

    fn issue(&mut self, class: IssueClass, code: IssueCode, target: Option<DirectEdge>) {
        self.report.issues.push(InspectionIssue {
            class,
            code,
            target,
        });
    }
}

struct Inspection<'a> {
    session: Session,
    ctx: &'a Context,
    budget: InspectionBudget,
    object_reads: usize,
    edge_rows_read: usize,
    watermark: Option<InspectionWatermark>,
    nodes: BTreeMap<String, Node>,
}

/// Inspect only this subject and its declared E16 authorities. Admin identity
/// checks apply even to authorities; a foreign owner/namespace aborts with
/// Forbidden, without returning an object report. Unknown versions remain
/// unknown; a malformed known declaration never supplies a guessed edge set.
pub async fn inspect_e16_content_edges(
    ctx: &Context,
    store: &Store,
    subject_id: &str,
    budget: InspectionBudget,
) -> Result<E16EdgeInspection> {
    ctx.require(&[Role::Admin])?;
    identifier(subject_id)?;
    budget.validate()?;
    let mut session = store.session().await?;
    let watermark = session
        .watermark(ctx)
        .await?
        .and_then(|(sequence, marker)| {
            u64::try_from(sequence)
                .ok()
                .map(|sequence| InspectionWatermark {
                    sequence,
                    digest: digest(&marker).then_some(marker),
                })
        });
    let mut state = Inspection {
        session,
        ctx,
        budget,
        object_reads: 0,
        edge_rows_read: 0,
        watermark,
        nodes: BTreeMap::new(),
    };
    state
        .point(subject_id, ObjectScope::DeclaredContentAndEdges, true)
        .await?;
    {
        let node = &state.nodes[subject_id];
        // Only declared artifact authorities are added; unexpected edge targets
        // and blob identities never become point reads or traversal roots.
        let mut full = Vec::new();
        let mut backlink = None;
        match &node.record {
            Some(Record::Selection(selection)) if node.report.declarations_checked => {
                full.extend(selection.payload.import_source_ids.iter().cloned());
            }
            Some(Record::Result(result)) if node.report.declarations_checked => {
                full.extend(
                    result
                        .source_refs
                        .iter()
                        .map(|reference| reference.id.clone()),
                );
            }
            Some(Record::Source(source)) if node.report.declarations_checked => {
                backlink = Some(source.payload.selection_id.clone());
            }
            _ if node.history && node.report.declarations_checked => {
                full.extend(
                    node.refs
                        .iter()
                        .filter(|reference| reference.kind != "blob")
                        .map(|reference| reference.id.clone()),
                );
            }
            _ => {}
        }
        for target in full {
            state
                .point(&target, ObjectScope::DeclaredContentAndEdges, false)
                .await?;
        }
        if let Some(target) = backlink {
            state
                .point(&target, ObjectScope::SelectionBacklinkAuthority, false)
                .await?;
        }
    }
    state.check_relations()?;
    let ids: Vec<_> = state.nodes.keys().cloned().collect();
    // Inspect the subject first so its edge result can be complete even when
    // later authority edges exhaust the shared budget. Output remains sorted.
    let ordered = std::iter::once(subject_id.to_string())
        .chain(ids.into_iter().filter(|id| id != subject_id));
    for id in ordered {
        state.check_edges(&id).await?;
    }
    let mut objects = Vec::with_capacity(state.nodes.len());
    for mut node in state.nodes.into_values() {
        node.report.issues.sort();
        node.report.issues.dedup();
        node.report.status = status(&node.report.issues);
        objects.push(node.report);
    }
    let overall = status(
        &objects
            .iter()
            .flat_map(|object| object.issues.iter().cloned())
            .collect::<Vec<_>>(),
    );
    state.session.commit().await?;
    Ok(E16EdgeInspection {
        schema_version: E16_EDGE_INSPECTION_SCHEMA.into(),
        subject_id: subject_id.into(),
        status: overall,
        budget,
        object_reads: state.object_reads,
        edge_rows_read: state.edge_rows_read,
        namespace_watermark: state.watermark,
        physical_blob_check: "not_checked".into(),
        snapshot_scope: "single_transaction_e16_declared_content_only".into(),
        objects,
    })
}

fn status(issues: &[InspectionIssue]) -> InspectionStatus {
    if issues
        .iter()
        .any(|issue| issue.class == IssueClass::Partial)
    {
        InspectionStatus::Partial
    } else if issues
        .iter()
        .any(|issue| issue.class == IssueClass::Anomaly)
    {
        InspectionStatus::Anomalous
    } else if issues
        .iter()
        .any(|issue| issue.class == IssueClass::Unknown)
    {
        InspectionStatus::Unknown
    } else if issues
        .iter()
        .any(|issue| issue.class == IssueClass::History)
    {
        InspectionStatus::HistoryOnly
    } else {
        InspectionStatus::Complete
    }
}

impl Inspection<'_> {
    async fn point(&mut self, id: &str, scope: ObjectScope, subject: bool) -> Result<()> {
        if let Some(node) = self.nodes.get_mut(id) {
            if scope == ObjectScope::DeclaredContentAndEdges {
                node.report.scope = scope;
            }
            return Ok(());
        }
        let mut node = Node::new(id, scope);
        if self.object_reads == self.budget.max_objects {
            node.issue(IssueClass::Partial, IssueCode::ObjectBudgetExhausted, None);
        } else {
            self.object_reads += 1;
            node.read = true;
            if let Some(value) = self.session.get::<Value>(self.ctx, "artifact", id).await? {
                decode(self.ctx, &mut node, value)?;
            } else {
                node.issue(
                    IssueClass::Anomaly,
                    if subject {
                        IssueCode::SubjectMissing
                    } else {
                        IssueCode::MissingAuthorityObject
                    },
                    None,
                );
            }
        }
        self.nodes.insert(id.into(), node);
        Ok(())
    }

    fn check_relations(&mut self) -> Result<()> {
        let mut issues = Vec::new();
        for (id, node) in &self.nodes {
            if !node.report.declarations_checked {
                continue;
            }
            let mut checked = true;
            if let Some(record) = &node.record {
                let recorded = match record {
                    Record::Selection(value) => value.revoke_watermark,
                    Record::Source(value) => value.revoke_watermark,
                    Record::Result(value) => value.revoke_watermark,
                };
                match &self.watermark {
                    None => issues.push((
                        id.clone(),
                        IssueClass::Unknown,
                        IssueCode::WatermarkUnavailable,
                        None,
                    )),
                    Some(watermark) if watermark.sequence != recorded => issues.push((
                        id.clone(),
                        IssueClass::Unknown,
                        IssueCode::WatermarkChanged,
                        None,
                    )),
                    _ => {}
                }
                if self
                    .watermark
                    .as_ref()
                    .is_some_and(|watermark| watermark.digest.is_none())
                {
                    issues.push((
                        id.clone(),
                        IssueClass::Unknown,
                        IssueCode::WatermarkDigestNotCheckable,
                        None,
                    ));
                }
            }
            let required: Vec<_> = match &node.record {
                _ if node.report.scope == ObjectScope::SelectionBacklinkAuthority => vec![],
                Some(Record::Source(source)) => vec![ImportTypedRef {
                    kind: "source_selection".into(),
                    id: source.payload.selection_id.clone(),
                    content_digest: String::new(),
                }],
                _ => node
                    .refs
                    .iter()
                    .filter(|reference| reference.kind != "blob")
                    .cloned()
                    .collect(),
            };
            for reference in required {
                let target = edge("artifact", &reference.id);
                let Some(authority) = self.nodes.get(&reference.id) else {
                    checked = false;
                    issues.push((
                        id.clone(),
                        IssueClass::Unknown,
                        IssueCode::AuthorityNotCheckable,
                        Some(target),
                    ));
                    continue;
                };
                if !authority.read {
                    checked = false;
                    issues.push((
                        id.clone(),
                        IssueClass::Partial,
                        IssueCode::ObjectBudgetExhausted,
                        Some(target),
                    ));
                } else if authority.history {
                    checked = false;
                    issues.push((
                        id.clone(),
                        IssueClass::History,
                        IssueCode::RedactedAuthority,
                        Some(target),
                    ));
                } else if let Some(record) = &authority.record {
                    let kind_ok = matches!(
                        (reference.kind.as_str(), record),
                        ("source_selection", Record::Selection(_))
                            | ("import_source", Record::Source(_))
                    );
                    if !kind_ok {
                        issues.push((
                            id.clone(),
                            IssueClass::Anomaly,
                            IssueCode::AuthorityKindMismatch,
                            Some(target),
                        ));
                    } else if !authority.report.declarations_checked {
                        checked = false;
                        issues.push((
                            id.clone(),
                            IssueClass::Unknown,
                            IssueCode::AuthorityNotCheckable,
                            Some(target),
                        ));
                    } else {
                        if !node.history
                            && !reference.content_digest.is_empty()
                            && authority.report.typed_digest.as_ref()
                                != Some(&reference.content_digest)
                        {
                            let transition = matches!(node.record, Some(Record::Selection(_)));
                            issues.push((
                                id.clone(),
                                if transition {
                                    IssueClass::Unknown
                                } else {
                                    IssueClass::Anomaly
                                },
                                if transition {
                                    IssueCode::PendingOrMismatchedAuthority
                                } else {
                                    IssueCode::ImmutableAuthorityDigestMismatch
                                },
                                Some(target.clone()),
                            ));
                        }
                        if !relation_matches(node, authority) {
                            issues.push((
                                id.clone(),
                                IssueClass::Anomaly,
                                IssueCode::AuthorityRelationMismatch,
                                Some(target),
                            ));
                        }
                        if let (Some(Record::Source(source)), Record::Selection(selection)) =
                            (&node.record, record)
                            && selection
                                .source_refs
                                .iter()
                                .find(|reference| reference.id == source.id)
                                .is_some_and(|reference| {
                                    Some(&reference.content_digest)
                                        != node.report.typed_digest.as_ref()
                                })
                        {
                            issues.push((
                                id.clone(),
                                IssueClass::Unknown,
                                IssueCode::PendingOrMismatchedAuthority,
                                Some(edge("artifact", &selection.id)),
                            ));
                        }
                    }
                } else {
                    checked = false;
                    let missing = authority.report.issues.iter().any(|issue| {
                        matches!(
                            issue.code,
                            IssueCode::SubjectMissing | IssueCode::MissingAuthorityObject
                        )
                    });
                    issues.push((
                        id.clone(),
                        if missing {
                            IssueClass::Anomaly
                        } else {
                            IssueClass::Unknown
                        },
                        if missing {
                            IssueCode::MissingAuthorityObject
                        } else {
                            IssueCode::AuthorityNotCheckable
                        },
                        Some(target),
                    ));
                }
            }
            if let Some(Record::Result(result)) = &node.record
                && let Some(Node {
                    record: Some(Record::Selection(selection)),
                    report,
                    ..
                }) = self.nodes.get(&result.payload.selection_id)
                && report.declarations_checked
                && (result.source_refs.len() != selection.source_refs.len() + 1
                    || result.source_refs[1..]
                        .iter()
                        .map(|reference| &reference.id)
                        .ne(selection.payload.import_source_ids.iter()))
            {
                issues.push((
                    id.clone(),
                    IssueClass::Anomaly,
                    IssueCode::ConflictingReferences,
                    None,
                ));
            }
            if let Some(Record::Selection(selection)) = &node.record
                && node.report.scope == ObjectScope::DeclaredContentAndEdges
            {
                let sources: Option<Vec<_>> = selection
                    .payload
                    .import_source_ids
                    .iter()
                    .map(|id| match self.nodes.get(id) {
                        Some(Node {
                            record: Some(Record::Source(source)),
                            report,
                            ..
                        }) if report.declarations_checked => Some(source),
                        _ => None,
                    })
                    .collect();
                if let Some(sources) = sources {
                    let request = ImportRegistrationRequest {
                        schema_version: IMPORT_REGISTRATION_SCHEMA.into(),
                        request_key: selection.request_key.clone(),
                        roots: selection.payload.roots.clone(),
                        purpose: selection.payload.purpose,
                        allow_model_excerpts: selection.payload.allow_model_excerpts,
                        outbound_authorized: selection.payload.outbound_authorized,
                        retention_scope: selection.payload.retention_scope,
                        sources: sources.iter().map(|source| source_spec(source)).collect(),
                    };
                    if fingerprint(&request)? != selection.input_digest {
                        issues.push((
                            id.clone(),
                            IssueClass::Anomaly,
                            IssueCode::InputDigestMismatch,
                            None,
                        ));
                    }
                }
            }
            // The backlink-only selection is not a separate closure root: its
            // remaining sources are explicitly outside a source-only scope.
            if node.report.scope == ObjectScope::SelectionBacklinkAuthority {
                checked = true;
            }
            if !checked {
                issues.push((
                    id.clone(),
                    IssueClass::Unknown,
                    IssueCode::AuthorityNotCheckable,
                    None,
                ));
            }
        }
        for (id, class, code, target) in issues {
            let node = self.nodes.get_mut(&id).expect("known node");
            if matches!(
                code,
                IssueCode::InputDigestMismatch | IssueCode::ConflictingReferences
            ) {
                node.report.declarations_checked = false;
                node.report.declared_edges.clear();
            }
            node.issue(class, code, target);
        }
        for node in self.nodes.values_mut() {
            node.report.authorities_checked = node.report.declarations_checked
                && !node.report.issues.iter().any(|issue| {
                    matches!(
                        issue.code,
                        IssueCode::ObjectBudgetExhausted
                            | IssueCode::AuthorityNotCheckable
                            | IssueCode::MissingAuthorityObject
                            | IssueCode::RedactedAuthority
                    )
                });
        }
        Ok(())
    }

    async fn check_edges(&mut self, id: &str) -> Result<()> {
        let node = &self.nodes[id];
        if !node.report.declarations_checked
            || node.report.scope == ObjectScope::SelectionBacklinkAuthority
        {
            return Ok(());
        }
        let expected: BTreeSet<_> = node.report.declared_edges.iter().cloned().collect();
        let mut seen = BTreeSet::new();
        let mut cursor = None;
        loop {
            let remaining = self.budget.max_edge_rows - self.edge_rows_read;
            // Reserve the possible lookahead row before issuing SQL. With one
            // row left, an empty table cannot safely be proven exhausted.
            if remaining < 2 {
                self.nodes.get_mut(id).expect("known node").issue(
                    IssueClass::Partial,
                    IssueCode::EdgeBudgetExhausted,
                    None,
                );
                break;
            }
            let limit = self.budget.page_size.min(remaining - 1);
            let page = self
                .session
                .direct_edges_page(self.ctx, "artifact", id, cursor.as_ref(), limit)
                .await?;
            self.edge_rows_read += page.rows_read;
            let node = self.nodes.get_mut(id).expect("known node");
            node.report.edge_rows_read += page.rows_read;
            node.report.edge_rows_returned += page.edges.len();
            for observed in page.edges {
                if identifier(&observed.dst_kind).is_err() || identifier(&observed.dst_id).is_err()
                {
                    node.issue(IssueClass::Anomaly, IssueCode::MalformedExistingEdge, None);
                } else if !expected.contains(&observed) {
                    node.issue(
                        IssueClass::Unknown,
                        IssueCode::UnexpectedExistingEdge,
                        Some(observed.clone()),
                    );
                }
                seen.insert(observed);
            }
            if page.exhausted {
                node.report.direct_edges_exhausted = true;
                for missing in expected.difference(&seen) {
                    node.issue(
                        IssueClass::Anomaly,
                        IssueCode::MissingDeclaredEdge,
                        Some(missing.clone()),
                    );
                }
                break;
            }
            cursor = page.next_cursor;
        }
        Ok(())
    }
}

fn edge(kind: &str, id: &str) -> DirectEdge {
    DirectEdge {
        dst_kind: kind.into(),
        dst_id: id.into(),
    }
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn refs_valid(refs: &[ImportTypedRef]) -> bool {
    let mut seen = BTreeSet::new();
    refs.iter().all(|reference| {
        identifier(&reference.id).is_ok()
            && digest(&reference.content_digest)
            && matches!(
                reference.kind.as_str(),
                "source_selection" | "import_source" | "blob"
            )
            && seen.insert((reference.kind.as_str(), reference.id.as_str()))
    })
}

fn envelope_valid<T>(record: &ImportEnvelope<T>, id: &str) -> bool {
    record.id == id
        && identifier(&record.id).is_ok()
        && identifier(&record.owner_actor).is_ok()
        && identifier(&record.request_key).is_ok()
        && digest(&record.input_digest)
        && record.created_at > 0
        && record.updated_at >= record.created_at
}

fn check_identity(ctx: &Context, id: &str, value: &Value, redacted: bool) -> Result<bool> {
    let envelope = if redacted { &value["metadata"] } else { value };
    // Check present identities even for an unknown wire or malformed typed
    // record. Missing/non-string identity fields are classified below, never
    // accepted as a known authorized envelope.
    for (field, expected) in [("namespace", ctx.namespace()), ("owner_actor", ctx.actor())] {
        if let Some(actual) = envelope.get(field).and_then(Value::as_str)
            && actual != expected
        {
            return Err(Error::Forbidden);
        }
    }
    Ok(value.get("id").and_then(Value::as_str) == Some(id)
        && envelope.get("namespace").and_then(Value::as_str) == Some(ctx.namespace())
        && envelope.get("owner_actor").and_then(Value::as_str) == Some(ctx.actor()))
}

fn decode(ctx: &Context, node: &mut Node, value: Value) -> Result<()> {
    let redacted = value["schema_version"].as_str() == Some("rsia.redacted.v1");
    let identity = check_identity(ctx, &node.report.id, &value, redacted)?;
    if !identity {
        node.issue(IssueClass::Anomaly, IssueCode::IdentityMismatch, None);
        return Ok(());
    }
    if redacted {
        return decode_history(node, &value);
    }
    let record = match value["schema_version"].as_str() {
        Some(SOURCE_SELECTION_SCHEMA) => {
            node.report.wire = E16Wire::SourceSelection;
            serde_json::from_value::<SourceSelectionRecord>(value).map(Record::Selection)
        }
        Some(IMPORT_SOURCE_SCHEMA) => {
            node.report.wire = E16Wire::ImportSource;
            serde_json::from_value::<ImportSourceRecord>(value).map(Record::Source)
        }
        Some(IMPORT_RESULT_SCHEMA) => {
            node.report.wire = E16Wire::ImportResult;
            serde_json::from_value::<ImportResultRecord>(value)
                .map(|record| Record::Result(Box::new(record)))
        }
        _ => {
            node.issue(IssueClass::Unknown, IssueCode::UnknownSchema, None);
            return Ok(());
        }
    };
    let Ok(record) = record else {
        node.issue(IssueClass::Anomaly, IssueCode::MalformedRecord, None);
        return Ok(());
    };
    let (valid, refs, typed_digest) = match &record {
        Record::Selection(selection) => (
            envelope_valid(selection, &node.report.id),
            &selection.source_refs,
            fingerprint(selection)?,
        ),
        Record::Source(source) => (
            envelope_valid(source, &node.report.id),
            &source.source_refs,
            fingerprint(source)?,
        ),
        Record::Result(result) => (
            envelope_valid(result, &node.report.id),
            &result.source_refs,
            fingerprint(result)?,
        ),
    };
    node.report.typed_digest = Some(typed_digest);
    if !valid {
        node.issue(IssueClass::Anomaly, IssueCode::MalformedRecord, None);
    } else if !refs_valid(refs) {
        node.issue(IssueClass::Anomaly, IssueCode::ConflictingReferences, None);
    } else {
        let structural = match &record {
            Record::Selection(selection) => {
                !selection.source_refs.is_empty()
                    && selection
                        .source_refs
                        .iter()
                        .all(|reference| reference.kind == "import_source")
                    && selection
                        .source_refs
                        .iter()
                        .map(|reference| &reference.id)
                        .eq(selection.payload.import_source_ids.iter())
                    && selection.id == format!("e16sel-{}", &selection.input_digest[..24])
            }
            Record::Source(source) => source_valid(source),
            Record::Result(result) => result_valid(result),
        };
        if !structural {
            node.issue(IssueClass::Anomaly, IssueCode::ConflictingReferences, None);
        } else if refs.len()
            > MAX_DISCOVERED_FILES + usize::from(matches!(record, Record::Result(_)))
        {
            node.issue(
                IssueClass::Partial,
                IssueCode::DeclarationScopeExceeded,
                None,
            );
        } else {
            let input_matches = match &record {
                Record::Result(result) => fingerprint(&result.source_refs)? == result.input_digest,
                Record::Source(source) => {
                    fingerprint(&(source_spec(source), source.payload.raw_digest.as_str()))?
                        == source.input_digest
                }
                Record::Selection(_) => true, // Request needs the source authorities.
            };
            if !input_matches {
                node.issue(IssueClass::Anomaly, IssueCode::InputDigestMismatch, None);
            } else {
                node.refs = refs.clone();
                node.report.declared_edges = refs
                    .iter()
                    .map(|reference| {
                        edge(
                            if reference.kind == "blob" {
                                "blob"
                            } else {
                                "artifact"
                            },
                            &reference.id,
                        )
                    })
                    .collect();
                node.report.declared_edges.sort();
                node.report.declarations_checked = true;
            }
        }
    }
    node.record = Some(record);
    Ok(())
}

fn source_spec(source: &ImportSourceRecord) -> ImportSourceSpec {
    ImportSourceSpec {
        source_id: source.payload.logical_source_id.clone(),
        path: source.payload.authorized_path.clone(),
        reader: source.payload.reader,
        expected_digest: source.payload.raw_digest.clone(),
    }
}

fn source_valid(source: &ImportSourceRecord) -> bool {
    let payload = &source.payload;
    if identifier(&payload.selection_id).is_err()
        || identifier(&payload.logical_source_id).is_err()
        || payload.task_origin != TaskOrigin::ImportedHistory
        || payload.execution_attestation != ExecutionAttestation::UnverifiedImport
        || !digest(&payload.raw_digest)
        || !digest(&payload.raw_blob_digest)
        || payload
            .observed_digest
            .as_ref()
            .is_some_and(|value| !digest(value))
        || payload.raw_blob_digest != payload.raw_digest
        || source.source_refs.len() != 1
        || source.source_refs[0].kind != "blob"
        || source.source_refs[0].id != payload.raw_blob_digest
        || source.source_refs[0].content_digest != payload.raw_digest
        || payload.reader_version != payload.reader.as_label()
        || source.id
            != format!(
                "e16src-{}",
                &hash(
                    format!("{}\n{}", payload.selection_id, payload.logical_source_id).as_bytes()
                )[..24]
            )
    {
        return false;
    }
    match payload.status {
        ImportSourceReadStatus::Prepared => {
            !payload.blob_published && payload.observed_digest.is_none() && payload.byte_len == 0
        }
        ImportSourceReadStatus::Ready => {
            payload.blob_published
                && payload.observed_digest.as_ref() == Some(&payload.raw_digest)
                && (1..=MAX_TOTAL_READ_BYTES as u64).contains(&payload.byte_len)
        }
        ImportSourceReadStatus::SourceChanged => {
            !payload.blob_published && payload.observed_digest.as_ref() != Some(&payload.raw_digest)
        }
        ImportSourceReadStatus::ZeroRecords => {
            !payload.blob_published
                && payload.byte_len == 0
                && payload.observed_digest.as_deref() == Some(hash(b"").as_str())
        }
        _ => !payload.blob_published,
    }
}

fn result_valid(result: &ImportResultRecord) -> bool {
    identifier(&result.payload.selection_id).is_ok()
        && result.id
            == format!(
                "e16res-{}",
                &hash(result.payload.selection_id.as_bytes())[..24]
            )
        && result.source_refs.len() >= 2
        && result.source_refs[0].kind == "source_selection"
        && result.source_refs[0].id == result.payload.selection_id
        && result.source_refs[1..]
            .iter()
            .all(|reference| reference.kind == "import_source")
        && result.payload.generation_status == "blocked_external_generation_conditions_unavailable"
        && ((result.payload.state == ImportResultState::Failed)
            == result.payload.evidence_set.is_none())
        && result.payload.aggregate_summary.total_events == result.payload.event_records.len()
        && result.payload.evidence_set.as_ref().is_none_or(|set| {
            set.members.iter().all(|member| {
                member.task_origin == TaskOrigin::ImportedHistory
                    && member.execution_attestation == ExecutionAttestation::UnverifiedImport
            })
        })
}

fn relation_matches(node: &Node, authority: &Node) -> bool {
    match (&node.record, &authority.record) {
        (Some(Record::Selection(selection)), Some(Record::Source(source)))
        | (Some(Record::Source(source)), Some(Record::Selection(selection))) => {
            source.payload.selection_id == selection.id
                && selection.payload.import_source_ids.contains(&source.id)
                && source.request_key == selection.request_key
                && source.revoke_watermark == selection.revoke_watermark
                && source.payload.purpose == selection.payload.purpose
        }
        (Some(Record::Result(result)), Some(Record::Selection(selection))) => {
            result.payload.selection_id == selection.id
                && result.request_key == selection.request_key
                && result.revoke_watermark == selection.revoke_watermark
        }
        (Some(Record::Result(result)), Some(Record::Source(source))) => {
            source.payload.selection_id == result.payload.selection_id
                && result.request_key == source.request_key
                && result.revoke_watermark == source.revoke_watermark
        }
        // Historical declarations no longer have an original payload to compare.
        (None, _) if node.history => true,
        _ => false,
    }
}

fn decode_history(node: &mut Node, value: &Value) -> Result<()> {
    node.history = true;
    node.issue(
        IssueClass::History,
        IssueCode::HistoricalDeclarationsOnly,
        None,
    );
    let (wire, expected_kind) = match value["original_schema"].as_str() {
        Some(SOURCE_SELECTION_SCHEMA) => (E16Wire::SourceSelection, "source_selection"),
        Some(IMPORT_SOURCE_SCHEMA) => (E16Wire::ImportSource, "import_source"),
        Some(IMPORT_RESULT_SCHEMA) => (E16Wire::ImportResult, "import_result"),
        _ => {
            node.issue(IssueClass::Unknown, IssueCode::UnknownSchema, None);
            return Ok(());
        }
    };
    node.report.wire = wire;
    let metadata = &value["metadata"];
    if !metadata["input_digest"].as_str().is_some_and(digest)
        || !metadata["request_key"]
            .as_str()
            .is_some_and(|key| identifier(key).is_ok())
        || !metadata["created_at"].as_i64().is_some_and(|created| {
            created > 0
                && metadata["updated_at"]
                    .as_i64()
                    .is_some_and(|updated| updated >= created)
        })
        || !value["original_digest"].as_str().is_some_and(digest)
        || metadata["revoke_watermark"].as_u64().is_none()
        || value["state"].as_str() != Some("source_revoked")
    {
        node.issue(IssueClass::Anomaly, IssueCode::MalformedRecord, None);
        return Ok(());
    }
    let Ok(refs) =
        serde_json::from_value::<Vec<ImportTypedRef>>(value["metadata"]["source_refs"].clone())
    else {
        node.issue(IssueClass::Anomaly, IssueCode::MalformedRecord, None);
        return Ok(());
    };
    let structural = refs_valid(&refs)
        && match wire {
            E16Wire::SourceSelection => {
                !refs.is_empty()
                    && refs
                        .iter()
                        .all(|reference| reference.kind == "import_source")
            }
            E16Wire::ImportSource => {
                refs.len() == 1 && refs[0].kind == "blob" && refs[0].id == refs[0].content_digest
            }
            E16Wire::ImportResult => {
                refs.len() >= 2
                    && refs[0].kind == "source_selection"
                    && refs[1..]
                        .iter()
                        .all(|reference| reference.kind == "import_source")
            }
            E16Wire::Unknown => false,
        };
    if value["original_kind"].as_str() != Some(expected_kind) || !structural {
        node.issue(IssueClass::Anomaly, IssueCode::ConflictingReferences, None);
    } else if refs.len() > MAX_DISCOVERED_FILES + usize::from(wire == E16Wire::ImportResult) {
        node.issue(
            IssueClass::Partial,
            IssueCode::DeclarationScopeExceeded,
            None,
        );
    } else if wire == E16Wire::ImportResult
        && fingerprint(&refs)? != metadata["input_digest"].as_str().unwrap_or_default()
    {
        node.issue(IssueClass::Anomaly, IssueCode::InputDigestMismatch, None);
    } else {
        node.report.declarations_checked = true;
        node.report.declared_edges = refs
            .iter()
            .map(|reference| {
                edge(
                    if reference.kind == "blob" {
                        "blob"
                    } else {
                        "artifact"
                    },
                    &reference.id,
                )
            })
            .collect();
        node.report.declared_edges.sort();
        node.refs = refs;
    }
    Ok(())
}
