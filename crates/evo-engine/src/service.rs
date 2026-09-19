//! Shared host-facing business service for HTTP, MCP, and the CLI.
//!
//! Protocol adapters authenticate callers. This module owns validation,
//! idempotency and Store consumption. A HostService is bound to one trusted
//! host identity and one namespace at startup; request payloads cannot change
//! either value.

use crate::evidence::{StoredTraceAuthority, load_stored_source, store_trace_authority};
use crate::release_store::{
    HostApplicationRecord, PersistentRelease, PrepareRunRequest, ReleaseCandidateRecord,
    ReleaseStore, RunApplicationSnapshot, TrustedHostExecutionEvidence,
};
use evo_core::contract::{AppliedReceipt, CapabilityLevel, HostCapabilities, SystemSnapshot};
use evo_core::{
    Candidate, CandidateState, Context, EntityKind, Error, Feedback, FeedbackRecord, Improvement,
    Inspect, Job, Prepared, Proposal, Receipt, Release, Result, Role, Run, Skill, Validate,
    fingerprint, hash, identifier, now, text,
};
use evo_storage::Store;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;

// E03 owns Store kind=`run` for its trusted authority envelope. Service runs
// therefore live in the existing generic artifact kind and are linked to the
// separately frozen RunApplicationSnapshot.
const SERVICE_RUN_KIND: &str = "artifact";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostPrepareConfig {
    pub profile_id: String,
    pub system_snapshot: SystemSnapshot,
    pub host_surface_id: String,
    pub host_capabilities: HostCapabilities,
    pub evolution_enabled: bool,
    pub capability_level: CapabilityLevel,
}

impl HostPrepareConfig {
    pub fn validate(&self) -> Result<()> {
        identifier(&self.profile_id)?;
        identifier(&self.host_surface_id)?;
        if self.system_snapshot.profile_id != self.profile_id {
            return Err(Error::Conflict("system snapshot profile mismatch".into()));
        }
        if self.system_snapshot.schema_version != "rsia.system_snapshot.v2" {
            return Err(Error::Invalid("unsupported system snapshot schema".into()));
        }
        for value in [
            &self.system_snapshot.profile_id,
            &self.system_snapshot.host_id,
            &self.system_snapshot.model_id,
        ] {
            identifier(value)?;
        }
        text(&self.system_snapshot.host_version, "host_version", 128)?;
        validate_digest(
            &self.system_snapshot.mandatory_context_digest,
            "mandatory_context_digest",
        )?;
        if self.system_snapshot.tools.len() > 64 {
            return Err(Error::Invalid("too many host tools".into()));
        }
        ensure_unique(&self.system_snapshot.tools, "duplicate host tool")?;
        for tool in &self.system_snapshot.tools {
            identifier(tool)?;
        }
        for capability in self
            .host_capabilities
            .available
            .iter()
            .chain(&self.host_capabilities.granted)
        {
            identifier(capability)?;
        }
        if !self
            .host_capabilities
            .granted
            .is_subset(&self.host_capabilities.available)
        {
            return Err(Error::Conflict(
                "granted capabilities must be available".into(),
            ));
        }
        self.system_snapshot.digest()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
struct PrepareOperation<'a> {
    request: &'a evo_core::Prepare,
    config: &'a HostPrepareConfig,
}

#[derive(Clone)]
pub struct HostService {
    store: Store,
    trusted_host: Context,
    application_lock: Arc<Mutex<()>>,
}

impl HostService {
    pub fn new(store: Store, trusted_host: Context) -> Result<Self> {
        trusted_host.require(&[Role::Host])?;
        Ok(Self {
            store,
            trusted_host,
            application_lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn trusted_namespace(&self) -> &str {
        self.trusted_host.namespace()
    }

    fn authorize_model_caller(&self, caller: &Context) -> Result<()> {
        caller.require(&[Role::Agent])?;
        if caller.namespace() != self.trusted_host.namespace() {
            return Err(Error::Forbidden);
        }
        Ok(())
    }

    fn authorize_host_caller(&self, caller: &Context) -> Result<()> {
        caller.require(&[Role::Host])?;
        if caller.namespace() != self.trusted_host.namespace()
            || caller.actor() != self.trusted_host.actor()
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }

    pub async fn prepare(
        &self,
        caller: &Context,
        request: evo_core::Prepare,
        config: &HostPrepareConfig,
    ) -> Result<Prepared> {
        self.authorize_model_caller(caller)?;
        request.validate()?;
        config.validate()?;
        ensure_unique(&request.capabilities, "duplicate requested capability")?;
        if !config.host_capabilities.allows(&request.capabilities) {
            return Err(Error::Forbidden);
        }
        let operation = PrepareOperation {
            request: &request,
            config,
        };
        {
            let mut session = self.store.session().await?;
            if let Some(existing) = session
                .cached::<Prepared, _>(caller, "service.prepare", &request.request_key, &operation)
                .await?
            {
                ReleaseStore::validate_run_snapshot_live_in_session(
                    &self.trusted_host,
                    &mut session,
                    &existing.run.id,
                )
                .await?;
                let run: Run = session
                    .need(caller, SERVICE_RUN_KIND, &existing.run.id)
                    .await?;
                caller.owns(&run.owner)?;
                if fingerprint(&run)? != fingerprint(&existing.run)? {
                    return Err(Error::Conflict("cached service run changed".into()));
                }
                session.commit().await?;
                return Ok(existing);
            }
            session.commit().await?;
        }

        let run_id = stable_id(
            "run",
            &[
                caller.namespace(),
                caller.actor(),
                "prepare",
                &request.request_key,
            ],
        );
        let snapshot = ReleaseStore::prepare_run(
            &self.trusted_host,
            &self.store,
            PrepareRunRequest {
                run_id: run_id.clone(),
                profile_id: config.profile_id.clone(),
                system_snapshot: config.system_snapshot.clone(),
                host_surface_id: config.host_surface_id.clone(),
                host_capabilities: config.host_capabilities.clone(),
                task_input_digest: hash(request.goal.as_bytes()),
                evolution_enabled: config.evolution_enabled,
                capability_level: config.capability_level,
            },
        )
        .await?;
        let run = Run {
            id: run_id.clone(),
            owner: caller.actor().into(),
            goal: request.goal.clone(),
            snapshot_id: snapshot.id.clone(),
            improver_version: "unavailable".into(),
            capabilities: request.capabilities.clone(),
            status: "prepared".into(),
            source: "trusted_host_snapshot".into(),
            created_at: now(),
        };
        let skills = self.projection_skills(&snapshot).await?;
        let mut limitations = vec!["model_transport_disabled".into()];
        if snapshot.release_id.is_none() {
            limitations.push("no_active_release_zero_skill_baseline".into());
        }
        if snapshot.capability_level == CapabilityLevel::ToolOnly {
            limitations.push("tool_only_cannot_claim_attached_or_used".into());
        }
        let prepared = Prepared {
            run,
            skills,
            limitations,
        };

        let mut session = self.store.session().await?;
        if let Some(existing) = session
            .cached::<Prepared, _>(caller, "service.prepare", &request.request_key, &operation)
            .await?
        {
            ReleaseStore::validate_run_snapshot_live_in_session(
                &self.trusted_host,
                &mut session,
                &existing.run.id,
            )
            .await?;
            let run: Run = session
                .need(caller, SERVICE_RUN_KIND, &existing.run.id)
                .await?;
            caller.owns(&run.owner)?;
            if fingerprint(&run)? != fingerprint(&existing.run)? {
                return Err(Error::Conflict("cached service run changed".into()));
            }
            session.commit().await?;
            return Ok(existing);
        }
        ReleaseStore::validate_run_snapshot_live_in_session(
            &self.trusted_host,
            &mut session,
            &run_id,
        )
        .await?;
        if let Some(existing) = session
            .get::<Run>(caller, SERVICE_RUN_KIND, &run_id)
            .await?
        {
            if fingerprint(&existing)? != fingerprint(&prepared.run)? {
                return Err(Error::Conflict(
                    "service run id reused with different content".into(),
                ));
            }
        } else {
            session
                .put(
                    caller,
                    SERVICE_RUN_KIND,
                    &run_id,
                    caller.actor(),
                    &prepared.run,
                )
                .await?;
            session
                .put_edge(caller, SERVICE_RUN_KIND, &run_id, "artifact", &snapshot.id)
                .await?;
        }
        session
            .cache(
                caller,
                "service.prepare",
                &request.request_key,
                &operation,
                &run_id,
                &prepared,
            )
            .await?;
        session.audit(caller, "service.prepare", &run_id).await?;
        session.commit().await?;
        Ok(prepared)
    }

    pub async fn feedback(&self, caller: &Context, request: Feedback) -> Result<FeedbackRecord> {
        self.authorize_model_caller(caller)?;
        request.validate()?;
        let mut session = self.store.session().await?;
        ReleaseStore::validate_run_snapshot_live_in_session(
            &self.trusted_host,
            &mut session,
            &request.run_id,
        )
        .await?;
        if let Some(existing) = session
            .cached::<FeedbackRecord, _>(caller, "service.feedback", &request.request_key, &request)
            .await?
        {
            let current: FeedbackRecord = session.need(caller, "feedback", &existing.id).await?;
            caller.owns(&current.owner)?;
            if fingerprint(&current)? != fingerprint(&existing)? {
                return Err(Error::Conflict("cached feedback changed".into()));
            }
            session.commit().await?;
            return Ok(existing);
        }
        let run: Run = session
            .need(caller, SERVICE_RUN_KIND, &request.run_id)
            .await?;
        caller.owns(&run.owner)?;
        let id = stable_id(
            "feedback",
            &[caller.namespace(), caller.actor(), &request.request_key],
        );
        let record = FeedbackRecord {
            id: id.clone(),
            owner: caller.actor().into(),
            request: request.clone(),
            verification: "self_reported_unverified".into(),
        };
        session
            .put(caller, "feedback", &id, caller.actor(), &record)
            .await?;
        session
            .put_edge(caller, "feedback", &id, SERVICE_RUN_KIND, &run.id)
            .await?;
        session
            .cache(
                caller,
                "service.feedback",
                &request.request_key,
                &request,
                &id,
                &record,
            )
            .await?;
        session.audit(caller, "service.feedback", &id).await?;
        session.commit().await?;
        Ok(record)
    }

    pub async fn propose(&self, caller: &Context, request: Proposal) -> Result<Candidate> {
        self.authorize_model_caller(caller)?;
        request.validate()?;
        let mut session = self.store.session().await?;
        ReleaseStore::validate_run_snapshot_live_in_session(
            &self.trusted_host,
            &mut session,
            &request.run_id,
        )
        .await?;
        if let Some(existing) = session
            .cached::<Candidate, _>(caller, "service.propose", &request.request_key, &request)
            .await?
        {
            let current: Candidate = session.need(caller, "candidate", &existing.id).await?;
            caller.owns(&current.owner)?;
            if matches!(
                current.state,
                CandidateState::Rejected | CandidateState::Retired | CandidateState::Revoked
            ) {
                return Err(Error::Forbidden);
            }
            if fingerprint(&current)? != fingerprint(&existing)? {
                return Err(Error::Conflict("cached candidate changed".into()));
            }
            session.commit().await?;
            return Ok(existing);
        }
        let run: Run = session
            .need(caller, SERVICE_RUN_KIND, &request.run_id)
            .await?;
        caller.owns(&run.owner)?;
        if request.parent_snapshot != run.snapshot_id {
            return Err(Error::Conflict(
                "proposal parent does not match frozen run snapshot".into(),
            ));
        }
        if !request
            .required_capabilities
            .iter()
            .all(|cap| run.capabilities.contains(cap))
        {
            return Err(Error::Forbidden);
        }
        ensure_unique(
            &request.required_capabilities,
            "duplicate required capability",
        )?;
        ensure_unique(&request.evidence_refs, "duplicate evidence reference")?;
        ensure_unique(&request.dependencies, "duplicate dependency")?;
        for evidence_id in &request.evidence_refs {
            let feedback = session
                .get::<FeedbackRecord>(caller, "feedback", evidence_id)
                .await?;
            let Some(feedback) = feedback else {
                return Err(Error::NotFound);
            };
            caller.owns(&feedback.owner)?;
            if feedback.request.run_id != run.id {
                return Err(Error::Conflict(
                    "evidence reference belongs to a different run".into(),
                ));
            }
        }
        for dependency_id in &request.dependencies {
            let dependency: Candidate = session.need(caller, "candidate", dependency_id).await?;
            caller.owns(&dependency.owner)?;
            if matches!(
                dependency.state,
                CandidateState::Rejected | CandidateState::Retired | CandidateState::Revoked
            ) {
                return Err(Error::Forbidden);
            }
        }
        let id = stable_id(
            "candidate",
            &[caller.namespace(), caller.actor(), &request.request_key],
        );
        let candidate = Candidate {
            id: id.clone(),
            owner: caller.actor().into(),
            digest: fingerprint(&request)?,
            proposal: request.clone(),
            state: CandidateState::Proposed,
            improver_version: "unavailable".into(),
            evaluation_id: None,
            approved_by: None,
            created_at: now(),
        };
        session
            .put(caller, "candidate", &id, caller.actor(), &candidate)
            .await?;
        session
            .put_edge(caller, "candidate", &id, SERVICE_RUN_KIND, &run.id)
            .await?;
        for evidence_id in &request.evidence_refs {
            session
                .put_edge(caller, "candidate", &id, "feedback", evidence_id)
                .await?;
        }
        for dependency_id in &request.dependencies {
            session
                .put_edge(caller, "candidate", &id, "candidate", dependency_id)
                .await?;
        }
        session
            .cache(
                caller,
                "service.propose",
                &request.request_key,
                &request,
                &id,
                &candidate,
            )
            .await?;
        session.audit(caller, "service.propose", &id).await?;
        session.commit().await?;
        Ok(candidate)
    }

    pub async fn inspect(&self, caller: &Context, request: Inspect) -> Result<Value> {
        self.authorize_inspector(caller)?;
        identifier(&request.id)?;
        let mut session = self.store.session().await?;
        let value = match request.kind {
            EntityKind::Run => {
                if let Some(raw) = session
                    .get::<Value>(caller, SERVICE_RUN_KIND, &request.id)
                    .await?
                {
                    let run: Run = serde_json::from_value(raw).map_err(|_| Error::NotFound)?;
                    caller.owns(&run.owner)?;
                    json!({
                        "kind": "run",
                        "id": run.id,
                        "owner": run.owner,
                        "snapshot_id": run.snapshot_id,
                        "status": run.status,
                        "source": run.source,
                        "created_at": run.created_at
                    })
                } else {
                    self.inspect_trace_authority(caller, &mut session, &request.id)
                        .await?
                }
            }
            EntityKind::Feedback => {
                owned_value::<FeedbackRecord>(caller, &mut session, "feedback", &request.id).await?
            }
            EntityKind::Candidate => {
                owned_value::<Candidate>(caller, &mut session, "candidate", &request.id).await?
            }
            EntityKind::Job => owned_value::<Job>(caller, &mut session, "job", &request.id).await?,
            EntityKind::Release => {
                owned_value::<Release>(caller, &mut session, "release", &request.id).await?
            }
            EntityKind::Receipt => {
                let raw: Value = session.need(caller, "receipt", &request.id).await?;
                if raw.get("schema_version").is_some() {
                    if caller.role() == Role::Agent {
                        return Err(Error::NotFound);
                    }
                    serde_json::from_value::<HostApplicationRecord>(raw.clone())
                        .map_err(|_| Error::NotFound)?;
                    raw
                } else {
                    owned_value_from::<Receipt>(caller, raw)?
                }
            }
            EntityKind::Improvement => {
                owned_value::<Improvement>(caller, &mut session, "improvement", &request.id).await?
            }
        };
        session.commit().await?;
        Ok(value)
    }

    fn authorize_inspector(&self, caller: &Context) -> Result<()> {
        caller.require(&[Role::Agent, Role::Host, Role::Admin])?;
        if caller.namespace() != self.trusted_host.namespace() {
            return Err(Error::Forbidden);
        }
        Ok(())
    }

    async fn inspect_trace_authority(
        &self,
        caller: &Context,
        session: &mut evo_storage::Session,
        id: &str,
    ) -> Result<Value> {
        if caller.role() == Role::Agent {
            return Err(Error::NotFound);
        }
        let raw: Value = session.need(caller, "run", id).await?;
        if raw.get("schema_version").and_then(Value::as_str) != Some("rsia.optimization.source.v1")
        {
            return Ok(json!({
                "kind": "run",
                "id": id,
                "authority": "unverified_legacy",
                "usable_as_trusted_source": false
            }));
        }
        let authority = load_stored_source(session, caller, id).await?;
        Ok(json!({
            "kind": "run",
            "id": authority.record.id,
            "authority": "trusted_host",
            "usable_as_trusted_source": true,
            "parent_family": authority.record.parent_family,
            "task_origin": authority.record.task_origin,
            "execution_attestation": authority.record.execution_attestation,
            "purpose": authority.record.purpose,
            "source_digest": authority.trace.source_digest,
            "outcome": authority.trace.outcome
        }))
    }

    pub async fn read_run_snapshot(
        &self,
        caller: &Context,
        run_id: &str,
    ) -> Result<RunApplicationSnapshot> {
        self.authorize_host_caller(caller)?;
        ReleaseStore::validate_run_snapshot_live(&self.trusted_host, &self.store, run_id).await
    }

    pub async fn record_trace(
        &self,
        caller: &Context,
        authority: &StoredTraceAuthority,
    ) -> Result<()> {
        self.authorize_host_caller(caller)?;
        store_trace_authority(&self.store, &self.trusted_host, authority).await
    }

    pub async fn record_application(
        &self,
        caller: &Context,
        run_id: &str,
        evidence: TrustedHostExecutionEvidence,
    ) -> Result<HostApplicationRecord> {
        self.authorize_host_caller(caller)?;
        let _guard = self.application_lock.lock().await;
        let snapshot =
            ReleaseStore::validate_run_snapshot_live(&self.trusted_host, &self.store, run_id)
                .await?;
        let actual_request_digest = fingerprint(&evidence.actual_request_material)?;
        if actual_request_digest != snapshot.request_digest
            || evidence.actual_request_material != snapshot.request_material
            || evidence.environment_digest != snapshot.environment_digest
            || evidence.host_surface_digest != snapshot.host_surface_digest
            || evidence.host_capabilities_digest != snapshot.host_capabilities_digest
        {
            return Err(Error::Conflict(
                "actual applied request differs from frozen run".into(),
            ));
        }
        let expected_receipt = match &snapshot.bundle_digest {
            Some(bundle_digest) => {
                let receipt = AppliedReceipt {
                    offered: evidence.offered.clone(),
                    attached: evidence.attached.clone(),
                    used: evidence.used.clone(),
                    verified_benefit: Vec::new(),
                    bundle_digest: bundle_digest.clone(),
                    request_digest: actual_request_digest.clone(),
                    capability_level: snapshot.capability_level,
                    truncated: evidence.truncated,
                    attested_by: self.trusted_host.actor().into(),
                };
                receipt.validate()?;
                Some(receipt)
            }
            None => {
                if !evidence.offered.is_empty()
                    || !evidence.attached.is_empty()
                    || !evidence.used.is_empty()
                    || evidence.execution_receipt_id.is_some()
                {
                    return Err(Error::Invalid(
                        "zero-skill baseline cannot claim application".into(),
                    ));
                }
                None
            }
        };
        let id = format!("host-application-{run_id}");
        let mut session = self.store.session().await?;
        let current_snapshot = ReleaseStore::validate_run_snapshot_live_in_session(
            &self.trusted_host,
            &mut session,
            run_id,
        )
        .await?;
        if fingerprint(&current_snapshot)? != fingerprint(&snapshot)? {
            return Err(Error::Conflict("frozen run snapshot changed".into()));
        }
        if let Some(existing) = session
            .get::<HostApplicationRecord>(&self.trusted_host, "receipt", &id)
            .await?
        {
            let same = existing.actual_request_digest == actual_request_digest
                && existing.execution_receipt_id == evidence.execution_receipt_id
                && fingerprint(&existing.receipt)? == fingerprint(&expected_receipt)?;
            session.commit().await?;
            if same {
                return Ok(existing);
            }
            return Err(Error::Conflict(
                "run application already recorded with different evidence".into(),
            ));
        }
        session.commit().await?;
        ReleaseStore::record_applied_request(&self.trusted_host, &self.store, run_id, evidence)
            .await
    }

    async fn projection_skills(&self, snapshot: &RunApplicationSnapshot) -> Result<Vec<Skill>> {
        let Some(release_id) = &snapshot.release_id else {
            if !snapshot.projection.instructions.is_empty() || snapshot.bundle_digest.is_some() {
                return Err(Error::Conflict(
                    "zero-skill snapshot contains an active projection".into(),
                ));
            }
            return Ok(Vec::new());
        };
        let mut session = self.store.session().await?;
        let current_snapshot = ReleaseStore::validate_run_snapshot_live_in_session(
            &self.trusted_host,
            &mut session,
            &snapshot.run_id,
        )
        .await?;
        if fingerprint(&current_snapshot)? != fingerprint(snapshot)? {
            return Err(Error::Conflict("frozen run snapshot changed".into()));
        }
        let release: PersistentRelease = session
            .need(&self.trusted_host, "release", release_id)
            .await?;
        let candidate: ReleaseCandidateRecord = session
            .need(&self.trusted_host, "artifact", &release.candidate_id)
            .await?;
        session.commit().await?;
        if snapshot.bundle_digest.as_deref() != Some(candidate.bundle_digest.as_str())
            || candidate.bundle_digest != candidate.bundle.digest
        {
            return Err(Error::Conflict(
                "prepared projection bundle identity changed".into(),
            ));
        }
        let expected = if candidate.bundle.skill.content.is_empty()
            && candidate.bundle.skill.applicability.is_empty()
        {
            Vec::new()
        } else {
            vec![candidate.bundle.skill.content.clone()]
        };
        if snapshot.projection.instructions != expected {
            return Err(Error::Conflict(
                "prepared projection differs from the active bundle".into(),
            ));
        }
        if expected.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![Skill {
            id: candidate.id,
            digest: hash(candidate.bundle.skill.content.as_bytes()),
            content: candidate.bundle.skill.content,
            applicability: candidate.bundle.skill.applicability,
            counterexample: candidate.bundle.skill.counterexample,
        }])
    }
}

async fn owned_value<T>(
    caller: &Context,
    session: &mut evo_storage::Session,
    kind: &str,
    id: &str,
) -> Result<Value>
where
    T: serde::de::DeserializeOwned + Serialize + OwnedRecord,
{
    let raw: Value = session.need(caller, kind, id).await?;
    owned_value_from::<T>(caller, raw)
}

fn owned_value_from<T>(caller: &Context, raw: Value) -> Result<Value>
where
    T: serde::de::DeserializeOwned + Serialize + OwnedRecord,
{
    if caller.role() == Role::Agent {
        let owner = raw
            .get("owner")
            .and_then(Value::as_str)
            .ok_or(Error::NotFound)?;
        caller.owns(owner)?;
    }
    let record: T = serde_json::from_value(raw).map_err(|_| Error::NotFound)?;
    caller.owns(record.owner())?;
    serde_json::to_value(record).map_err(|_| Error::Internal)
}

trait OwnedRecord {
    fn owner(&self) -> &str;
}

impl OwnedRecord for FeedbackRecord {
    fn owner(&self) -> &str {
        &self.owner
    }
}
impl OwnedRecord for Candidate {
    fn owner(&self) -> &str {
        &self.owner
    }
}

impl OwnedRecord for Job {
    fn owner(&self) -> &str {
        &self.owner
    }
}
impl OwnedRecord for Release {
    fn owner(&self) -> &str {
        &self.owner
    }
}
impl OwnedRecord for Receipt {
    fn owner(&self) -> &str {
        &self.owner
    }
}
impl OwnedRecord for Improvement {
    fn owner(&self) -> &str {
        &self.owner
    }
}

fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let digest = hash(parts.join("\0").as_bytes());
    format!("{prefix}-{}", &digest[..24])
}

fn ensure_unique(values: &[String], message: &str) -> Result<()> {
    let unique = values.iter().collect::<std::collections::BTreeSet<_>>();
    if unique.len() != values.len() {
        return Err(Error::Conflict(message.into()));
    }
    Ok(())
}

fn validate_digest(value: &str, name: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(Error::Invalid(format!(
            "{name}: expected lowercase sha256 digest"
        )));
    }
    Ok(())
}
