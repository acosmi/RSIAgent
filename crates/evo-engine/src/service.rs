//! Shared host-facing business service for HTTP, MCP, and the CLI.
//!
//! Protocol adapters authenticate callers. This module owns validation,
//! idempotency and Store consumption. A HostService is bound to one trusted
//! host identity and one namespace at startup; request payloads cannot change
//! either value.

use crate::capacity::{
    CapacityField, CapacityLimits, V41CapacityUsage, admit_field, unix_now_secs,
};
use crate::evidence::{StoredTraceAuthority, load_stored_source, store_trace_authority};
use crate::release_store::{
    CompletePrepareRunRequest, HostApplicationRecord,
    LoadedRunApplicationSnapshot as RunApplicationSnapshot, PersistentRelease, PrepareRunRequest,
    ReadCandidate, ReleaseStore, TrustedHostExecutionEvidence,
};
use evo_core::contract::{AppliedReceipt, CapabilityLevel};
pub use evo_core::prepare_contract::{CompleteHostPrepareConfig, HostPrepareConfig};
use evo_core::prepare_contract::{CompletePrepareOperation, PrepareOperation};
use evo_core::{
    Candidate, CandidateState, Context, EntityKind, Error, Feedback, FeedbackRecord, Improvement,
    Inspect, Job, Prepared, Proposal, Receipt, Release, Result, Role, Run, Skill, Validate,
    fingerprint, hash, identifier, now,
};
use evo_storage::{Session, Store};
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::Mutex;

// E03 owns Store kind=`run` for its trusted authority envelope. Service runs
// therefore live in the existing generic artifact kind and are linked to the
// separately frozen RunApplicationSnapshot.
const SERVICE_RUN_KIND: &str = "artifact";

#[derive(Clone)]
pub struct HostService {
    store: Store,
    trusted_host: Context,
    application_lock: Arc<Mutex<()>>,
    // In-flight prepare is never persisted: like `application_lock` it is a
    // process-local fact shared by every clone of this service.
    inflight_prepare: Arc<AtomicU64>,
}

/// One admitted in-flight prepare slot. Dropping it releases the slot on every
/// exit path of `HostService::prepare`, including errors.
#[derive(Debug)]
#[must_use = "dropping the guard releases the in-flight prepare slot"]
pub struct InflightPrepareGuard {
    counter: Arc<AtomicU64>,
}

impl Drop for InflightPrepareGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

impl HostService {
    pub fn new(store: Store, trusted_host: Context) -> Result<Self> {
        trusted_host.require(&[Role::Host])?;
        Ok(Self {
            store,
            trusted_host,
            application_lock: Arc::new(Mutex::new(())),
            inflight_prepare: Arc::new(AtomicU64::new(0)),
        })
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Pure identity gate, also used before HTTP review DTO parsing.
    pub fn authorize_review(&self, caller: &Context) -> Result<()> {
        if caller.require(&[Role::Admin]).is_err()
            || caller.namespace() != self.trusted_host.namespace()
        {
            return Err(Error::NotFound);
        }
        Ok(())
    }

    /// Review existing facts as the Admin of this gateway's trusted namespace.
    pub async fn review(
        &self,
        caller: &Context,
        request: crate::review::ReviewRequest,
    ) -> Result<crate::review::ReviewResponse> {
        self.authorize_review(caller)?;
        crate::review::review_stored(caller, &self.store, request).await
    }

    /// Prepares currently in flight in this process.
    pub fn inflight_prepare(&self) -> u64 {
        self.inflight_prepare.load(Ordering::Acquire)
    }

    /// Take one in-flight prepare slot, or refuse with the typed MVP capacity
    /// conflict once `MAX_PREPARE` prepares are already in flight. Tests use
    /// this to saturate the slots; `prepare` takes one for its whole duration.
    pub fn begin_inflight_prepare(&self) -> Result<InflightPrepareGuard> {
        let limits = CapacityLimits::default();
        let mut current = self.inflight_prepare.load(Ordering::Acquire);
        loop {
            admit_field(CapacityField::InflightPrepare, current, &limits)?;
            match self.inflight_prepare.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Ok(InflightPrepareGuard {
                        counter: Arc::clone(&self.inflight_prepare),
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }

    /// E16.5 / plan §3.3.1: a prepare derives a new run. Refuse it, inside the
    /// caller's session, once any MVP counter (runs, events, skills, other
    /// in-flight prepares) is at its limit. Counters are instance-wide (F24):
    /// a cap reached in any namespace refuses new runs in every namespace.
    /// Nothing is truncated; the caller is told which counter to scale or
    /// clean up.
    async fn admit_new_run(&self, session: &mut Session) -> Result<()> {
        let others_in_flight = self.inflight_prepare().saturating_sub(1);
        let usage: V41CapacityUsage = session.capacity_usage_v41(unix_now_secs()?).await?.into();
        let usage = usage.with_inflight_prepare(others_in_flight);
        let limits = CapacityLimits::default();
        for field in CapacityField::MVP_DERIVE {
            admit_field(field, field.usage(&usage), &limits)?;
        }
        Ok(())
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
        Box::pin(self.prepare_inner(caller, request, PrepareConfiguration::legacy(config))).await
    }
    pub async fn prepare_complete(
        &self,
        caller: &Context,
        request: evo_core::Prepare,
        config: &CompleteHostPrepareConfig,
    ) -> Result<Prepared> {
        Box::pin(self.prepare_inner(caller, request, PrepareConfiguration::complete(config))).await
    }
    async fn prepare_inner(
        &self,
        caller: &Context,
        request: evo_core::Prepare,
        config: PrepareConfiguration<'_>,
    ) -> Result<Prepared> {
        self.authorize_model_caller(caller)?;
        request.validate()?;
        config.validate()?;
        ensure_unique(&request.capabilities, "duplicate requested capability")?;
        if !config.host_capabilities.allows(&request.capabilities) {
            return Err(Error::Forbidden);
        }
        let operation = config.operation(&request);
        // Held until this function returns on any path.
        let _inflight = self.begin_inflight_prepare()?;
        {
            let mut session = self.store.session().await?;
            if let Some(existing) = session
                .cached::<Prepared, _>(caller, "service.prepare", &request.request_key, &operation)
                .await?
            {
                let live = ReleaseStore::validate_run_snapshot_live_in_session(
                    &self.trusted_host,
                    &mut session,
                    &existing.run.id,
                )
                .await?;
                config.matches(&live)?;
                let run: Run = session
                    .need(caller, SERVICE_RUN_KIND, &existing.run.id)
                    .await?;
                caller.owns(&run.owner)?;
                if fingerprint(&run)? != fingerprint(&existing.run)? {
                    return Err(Error::Conflict("cached service run changed".into()));
                }
                ReleaseStore::validate_run_capability_for_new_use_in_session(
                    &self.trusted_host,
                    &mut session,
                    &live,
                )
                .await?;
                session.commit().await?;
                return Ok(existing);
            }
            // Idempotent replays above are not new derivations. A miss is:
            // refuse here, before the run snapshot is frozen, so a refused
            // prepare writes nothing at all.
            self.admit_new_run(&mut session).await?;
            session.commit().await?;
        }

        let run_id = stable_id(
            "run",
            &[
                caller.namespace(),
                caller.actor(),
                config.run_component(),
                &request.request_key,
            ],
        );
        let snapshot = match config.original {
            PrepareConfigOriginal::Complete(c) => {
                ReleaseStore::prepare_complete_run(
                    &self.trusted_host,
                    &self.store,
                    CompletePrepareRunRequest {
                        run_id: run_id.clone(),
                        profile_id: config.profile_id.clone(),
                        system_snapshot: c.system_snapshot.clone(),
                        host_surface_id: config.host_surface_id.clone(),
                        host_capabilities: config.host_capabilities.clone(),
                        task_input_digest: hash(request.goal.as_bytes()),
                        evolution_enabled: config.evolution_enabled,
                        capability_level: config.capability_level,
                    },
                )
                .await?
            }
            PrepareConfigOriginal::Legacy(c) => {
                ReleaseStore::prepare_run(
                    &self.trusted_host,
                    &self.store,
                    PrepareRunRequest {
                        run_id: run_id.clone(),
                        profile_id: config.profile_id.clone(),
                        system_snapshot: c.system_snapshot.clone(),
                        host_surface_id: config.host_surface_id.clone(),
                        host_capabilities: config.host_capabilities.clone(),
                        task_input_digest: hash(request.goal.as_bytes()),
                        evolution_enabled: config.evolution_enabled,
                        capability_level: config.capability_level,
                    },
                )
                .await?
            }
        };
        let run = Run {
            id: run_id.clone(),
            owner: caller.actor().into(),
            goal: request.goal.clone(),
            snapshot_id: snapshot.id.clone(),
            improver_version: "unavailable".into(),
            capabilities: request.capabilities.clone(),
            status: "prepared".into(),
            source: "trusted_host_snapshot".into(),
            created_at: now()?,
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
            let live = ReleaseStore::validate_run_snapshot_live_in_session(
                &self.trusted_host,
                &mut session,
                &existing.run.id,
            )
            .await?;
            config.matches(&live)?;
            let run: Run = session
                .need(caller, SERVICE_RUN_KIND, &existing.run.id)
                .await?;
            caller.owns(&run.owner)?;
            if fingerprint(&run)? != fingerprint(&existing.run)? {
                return Err(Error::Conflict("cached service run changed".into()));
            }
            ReleaseStore::validate_run_capability_for_new_use_in_session(
                &self.trusted_host,
                &mut session,
                &live,
            )
            .await?;
            session.commit().await?;
            return Ok(existing);
        }
        let live = ReleaseStore::validate_run_snapshot_live_in_session(
            &self.trusted_host,
            &mut session,
            &run_id,
        )
        .await?;
        config.matches(&live)?;
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
            // Service runs live in the artifact kind, so `Session::put`'s run
            // bound does not see them: re-admit in the session that inserts
            // the run, which is the authoritative count-then-insert.
            self.admit_new_run(&mut session).await?;
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
            let run: Run = session
                .need(caller, SERVICE_RUN_KIND, &request.run_id)
                .await?;
            caller.owns(&run.owner)?;
            if request.kind == evo_core::ArtifactKind::Skill {
                reject_tool_only_skill_evidence(
                    &mut session,
                    &self.trusted_host,
                    &run.id,
                    &request.evidence_refs,
                )
                .await?;
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
                if request.kind == evo_core::ArtifactKind::Skill {
                    reject_tool_only_skill_evidence(
                        &mut session,
                        &self.trusted_host,
                        &run.id,
                        std::slice::from_ref(evidence_id),
                    )
                    .await?;
                }
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
            created_at: now()?,
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
                    let mut view = json!({
                        "kind": "run",
                        "id": run.id,
                        "owner": run.owner,
                        "snapshot_id": run.snapshot_id,
                        "status": run.status,
                        "source": run.source,
                        "created_at": run.created_at
                    });
                    let diagnostic_id =
                        crate::monitoring::tool_only_diagnostic_id(caller.namespace(), &run.id)?;
                    if let Some(diagnostic) =
                        crate::monitoring::load_tool_only_diagnostic_in_session(
                            &mut session,
                            &self.trusted_host,
                            &diagnostic_id,
                            &run.id,
                        )
                        .await?
                    {
                        view["diagnostic"] = json!({
                            "id": diagnostic.id,
                            "route": diagnostic.route,
                            "consumer": diagnostic.consumer,
                            "state": diagnostic.state,
                            "failure_kind": diagnostic.failure_kind,
                        });
                    }
                    view
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
        let actual_request_digest =
            snapshot.applied_request_digest(&evidence.actual_request_material)?;
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
                return ReleaseStore::record_applied_request(
                    &self.trusted_host,
                    &self.store,
                    run_id,
                    evidence,
                )
                .await;
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
        let candidate: ReadCandidate = session
            .need(&self.trusted_host, "artifact", &release.candidate_id)
            .await?;
        candidate.current()?;
        crate::release_store::validate_read_candidate_in_session(
            &mut session,
            &self.trusted_host,
            &candidate,
        )
        .await?;
        session.commit().await?;
        project_current_skill_material(
            &candidate.id,
            candidate.current()?.bundle.clone(),
            snapshot.bundle_digest.as_deref(),
            &snapshot.projection.instructions,
        )
    }
}

// References locate stored facts; neither a prefix nor self-reported feedback
// can establish this cause. Both the initial and cached Skill paths use this gate.
async fn reject_tool_only_skill_evidence(
    session: &mut Session,
    host: &Context,
    run_id: &str,
    evidence_refs: &[String],
) -> Result<()> {
    for id in evidence_refs {
        if crate::monitoring::load_tool_only_diagnostic_in_session(session, host, id, run_id)
            .await?
            .is_some()
        {
            return Err(Error::Conflict("host_capability_tool_only".into()));
        }
        if *id == crate::monitoring::tool_only_diagnostic_id(host.namespace(), run_id)? {
            return Err(Error::NotFound);
        }
    }
    Ok(())
}

// Content projection only. The caller must first reload and validate the real
// release, candidate and original compiler inputs in its Store Session.
fn project_current_skill_material(
    candidate_id: &str,
    bundle: evo_core::contract::ResolvedBundleV3,
    expected_digest: Option<&str>,
    instructions: &[String],
) -> Result<Vec<Skill>> {
    crate::compiler::ReadBundle::from_current(bundle.clone())?.validate_identity()?;
    if expected_digest != Some(bundle.digest.as_str()) {
        return Err(Error::Conflict(
            "prepared projection bundle identity changed".into(),
        ));
    }
    let expected = if bundle.skill.content.is_empty() && bundle.skill.applicability.is_empty() {
        Vec::new()
    } else {
        vec![bundle.skill.content.clone()]
    };
    if instructions != expected {
        return Err(Error::Conflict(
            "prepared projection differs from the active bundle".into(),
        ));
    }
    if expected.is_empty() {
        return Ok(Vec::new());
    }
    Ok(vec![Skill {
        id: candidate_id.into(),
        digest: hash(bundle.skill.content.as_bytes()),
        content: bundle.skill.content,
        applicability: bundle.skill.applicability,
        counterexample: bundle.skill.counterexample,
    }])
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

#[cfg(test)]
mod current_projection_controls {
    use super::*;
    #[test]
    fn current_bundle_projection_keeps_complete_v3_skill_material() {
        let (bundle, _legacy) =
            crate::compiler::current_carrier_content_controls::content_bundles();
        let wire = serde_json::to_vec(&bundle).unwrap();
        let projected = project_current_skill_material(
            "content-candidate",
            bundle.clone(),
            Some(&bundle.digest),
            std::slice::from_ref(&bundle.skill.content),
        )
        .unwrap();
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].id, "content-candidate");
        assert_eq!(projected[0].digest, hash(bundle.skill.content.as_bytes()));
        assert_eq!(projected[0].content, bundle.skill.content);
        assert_eq!(projected[0].applicability, bundle.skill.applicability);
        assert_eq!(projected[0].counterexample, bundle.skill.counterexample);
        assert_eq!(serde_json::to_vec(&bundle).unwrap(), wire);
        assert!(
            matches!(project_current_skill_material("content-candidate", bundle.clone(), Some(&hash(b"another bundle")), std::slice::from_ref(&bundle.skill.content)),
            Err(Error::Conflict(reason)) if reason == "prepared projection bundle identity changed")
        );
        assert!(
            matches!(project_current_skill_material("content-candidate", bundle.clone(), Some(&bundle.digest), &["changed projection".into()]),
            Err(Error::Conflict(reason)) if reason == "prepared projection differs from the active bundle")
        );
        let mut tampered = bundle.clone();
        tampered.skill.counterexample.push_str(" changed");
        assert!(
            project_current_skill_material(
                "content-candidate",
                tampered,
                Some(&bundle.digest),
                std::slice::from_ref(&bundle.skill.content)
            )
            .is_err()
        );
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum PrepareOperationOriginal<'a> {
    Legacy(PrepareOperation<'a>),
    Complete(CompletePrepareOperation<'a>),
}
enum PrepareConfigOriginal<'a> {
    Legacy(&'a HostPrepareConfig),
    Complete(&'a CompleteHostPrepareConfig),
}
struct PrepareConfiguration<'a> {
    original: PrepareConfigOriginal<'a>,
    view: PrepareConfigurationView,
}
struct PrepareConfigurationView {
    profile_id: String,
    system_snapshot: evo_core::contract::LoadedSystemSnapshot,
    host_surface_id: String,
    host_capabilities: evo_core::contract::HostCapabilities,
    evolution_enabled: bool,
    capability_level: CapabilityLevel,
}
impl<'a> PrepareConfiguration<'a> {
    fn legacy(c: &'a HostPrepareConfig) -> Self {
        Self {
            original: PrepareConfigOriginal::Legacy(c),
            view: PrepareConfigurationView {
                profile_id: c.profile_id.clone(),
                system_snapshot: c.system_snapshot.clone().into(),
                host_surface_id: c.host_surface_id.clone(),
                host_capabilities: c.host_capabilities.clone(),
                evolution_enabled: c.evolution_enabled,
                capability_level: c.capability_level,
            },
        }
    }
    fn complete(c: &'a CompleteHostPrepareConfig) -> Self {
        Self {
            original: PrepareConfigOriginal::Complete(c),
            view: PrepareConfigurationView {
                profile_id: c.profile_id.clone(),
                system_snapshot: c.system_snapshot.clone().into(),
                host_surface_id: c.host_surface_id.clone(),
                host_capabilities: c.host_capabilities.clone(),
                evolution_enabled: c.evolution_enabled,
                capability_level: c.capability_level,
            },
        }
    }
    fn validate(&self) -> Result<()> {
        match self.original {
            PrepareConfigOriginal::Legacy(c) => c.validate(),
            PrepareConfigOriginal::Complete(c) => c.validate(),
        }
    }
    fn operation<'b>(&'b self, r: &'b evo_core::Prepare) -> PrepareOperationOriginal<'b> {
        match self.original {
            PrepareConfigOriginal::Legacy(c) => {
                PrepareOperationOriginal::Legacy(PrepareOperation {
                    request: r,
                    config: c,
                })
            }
            PrepareConfigOriginal::Complete(c) => {
                PrepareOperationOriginal::Complete(CompletePrepareOperation {
                    schema_version: "rsia.prepare_operation.v3",
                    request: r,
                    config: c,
                })
            }
        }
    }
    fn run_component(&self) -> &'static str {
        match self.original {
            PrepareConfigOriginal::Legacy(_) => "prepare",
            PrepareConfigOriginal::Complete(_) => "prepare.v3",
        }
    }
    fn matches(&self, run: &RunApplicationSnapshot) -> Result<()> {
        if run.profile_id != self.profile_id
            || run.host_surface_id != self.host_surface_id
            || fingerprint(&run.system_snapshot)? != fingerprint(&self.system_snapshot)?
            || run.host_capabilities_digest != fingerprint(&self.host_capabilities)?
            || run.evolution_enabled != self.evolution_enabled
            || run.capability_level != self.capability_level
            || (self.system_snapshot.complete().is_some() != run.current_v3().is_some())
        {
            return Err(Error::Conflict("configured live run source differs".into()));
        }
        Ok(())
    }
}
impl std::ops::Deref for PrepareConfiguration<'_> {
    type Target = PrepareConfigurationView;
    fn deref(&self) -> &Self::Target {
        &self.view
    }
}

#[cfg(test)]
mod tool_only_service_controls {
    use super::*;
    use crate::release_store::{AppliedRequestMaterial, RunApplicationSnapshotV2};
    use evo_core::contract::{HostCapabilities, RunProjection, SystemSnapshot};
    use evo_core::{ArtifactKind, Outcome};

    struct Component {
        _dir: tempfile::TempDir,
        service: HostService,
        host: Context,
        agent: Context,
        run: Run,
        snapshot: RunApplicationSnapshot,
        application: HostApplicationRecord,
    }
    impl Component {
        async fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::open(&dir.path().join("service-component.sqlite3"))
                .await
                .unwrap();
            let host = Context::new("service-component", "actual-host-writer", Role::Host).unwrap();
            let agent = Context::new("service-component", "run-owner-agent", Role::Agent).unwrap();
            let system = SystemSnapshot {
                schema_version: "rsia.system_snapshot.v2".into(),
                profile_id: "component-profile".into(),
                host_id: "software-host".into(),
                host_version: "1".into(),
                model_id: "disabled".into(),
                tools: vec!["fs_read".into()],
                mandatory_context_digest: hash(b"component mandatory"),
            };
            let caps = HostCapabilities {
                available: Default::default(),
                granted: Default::default(),
            };
            let material = AppliedRequestMaterial {
                mandatory_context_digest: system.mandatory_context_digest.clone(),
                task_input_digest: hash(b"component task"),
                instructions: vec!["given frozen legacy component offering".into()],
                tools: system.tools.clone(),
            };
            let request_digest = fingerprint(&material).unwrap();
            let bundle_digest = hash(b"given legacy component bundle");
            let original = RunApplicationSnapshotV2 {
                id: "run-snapshot-component-run".into(),
                schema_version: "rsia.run_application.v2".into(),
                run_id: "component-run".into(),
                profile_id: system.profile_id.clone(),
                pointer_epoch: 0,
                release_id: None,
                bundle_digest: Some(bundle_digest.clone()),
                environment_digest: system.digest().unwrap(),
                system_snapshot: system,
                host_surface_id: "component-surface".into(),
                host_surface_digest: hash(b"given component surface"),
                host_capabilities_digest: fingerprint(&caps).unwrap(),
                host_capabilities: caps,
                projection: RunProjection {
                    bundle_digest: bundle_digest.clone(),
                    instructions: material.instructions.clone(),
                    tools: material.tools.clone(),
                },
                request_material: material,
                request_digest: request_digest.clone(),
                capability_level: CapabilityLevel::ToolOnly,
                evolution_enabled: true,
            };
            let snapshot: RunApplicationSnapshot =
                serde_json::from_value(serde_json::to_value(original).unwrap()).unwrap();
            assert!(
                snapshot.current_v3().is_none(),
                "component premises must stay legacy; no Active/P authority is seeded"
            );
            let application = HostApplicationRecord {
                id: "given-component-application".into(),
                schema_version: "rsia.host_application.v1".into(),
                run_id: snapshot.run_id.clone(),
                release_id: None,
                actual_request_digest: request_digest.clone(),
                execution_receipt_id: None,
                receipt: Some(AppliedReceipt {
                    offered: vec!["component-skill".into()],
                    attached: vec![],
                    used: vec![],
                    verified_benefit: vec![],
                    bundle_digest,
                    request_digest,
                    capability_level: CapabilityLevel::ToolOnly,
                    truncated: false,
                    attested_by: host.actor().into(),
                }),
            };
            let run = Run {
                id: snapshot.run_id.clone(),
                owner: agent.actor().into(),
                goal: "component given facts".into(),
                snapshot_id: snapshot.id.clone(),
                improver_version: "unavailable".into(),
                capabilities: vec![],
                status: "prepared".into(),
                source: "software_component_precondition".into(),
                created_at: now().unwrap(),
            };
            let mut session = store.session().await.unwrap();
            session
                .put(&host, "artifact", &snapshot.id, host.actor(), &snapshot)
                .await
                .unwrap();
            crate::formal_authority::attest_content(
                &mut session,
                &host,
                "artifact",
                &snapshot.id,
                &snapshot,
            )
            .await
            .unwrap();
            session
                .put(
                    &host,
                    "receipt",
                    &application.id,
                    host.actor(),
                    &application,
                )
                .await
                .unwrap();
            crate::formal_authority::attest_content(
                &mut session,
                &host,
                "receipt",
                &application.id,
                &application,
            )
            .await
            .unwrap();
            session
                .put(&agent, SERVICE_RUN_KIND, &run.id, agent.actor(), &run)
                .await
                .unwrap();
            session
                .put_edge(&agent, SERVICE_RUN_KIND, &run.id, "artifact", &snapshot.id)
                .await
                .unwrap();
            session.commit().await.unwrap();
            let service = HostService::new(store, host.clone()).unwrap();
            Self {
                _dir: dir,
                service,
                host,
                agent,
                run,
                snapshot,
                application,
            }
        }
        async fn record(&self) -> String {
            let mut session = self.service.store.session().await.unwrap();
            crate::monitoring::record_tool_only_diagnostic_in_session(
                &mut session,
                &self.host,
                &self.snapshot,
                &self.application,
            )
            .await
            .unwrap();
            session.commit().await.unwrap();
            crate::monitoring::tool_only_diagnostic_id(self.host.namespace(), &self.run.id).unwrap()
        }
    }

    #[tokio::test]
    async fn diagnostic_reference_blocks_new_and_cached_skill_proposals() {
        let fixture = Component::new().await;
        let id = fixture.record().await;
        let request = Proposal {
            request_key: "component-proposal".into(),
            run_id: fixture.run.id.clone(),
            kind: ArtifactKind::Skill,
            parent_snapshot: fixture.run.snapshot_id.clone(),
            hypothesis: "component hypothesis".into(),
            applicability: "component scope".into(),
            counterexample: "a different environment".into(),
            content: "component proposed content".into(),
            evidence_refs: vec![id],
            required_capabilities: vec![],
            dependencies: vec![],
        };
        let mut session = fixture.service.store.session().await.unwrap();
        // Exercise the shared production gate at the new-write point, with
        // source premises stated above rather than a fabricated Current prepare.
        assert!(
            matches!(reject_tool_only_skill_evidence(&mut session, &fixture.host, &fixture.run.id, &request.evidence_refs).await,
            Err(Error::Conflict(reason)) if reason == "host_capability_tool_only")
        );
        assert_eq!(
            session
                .namespace_object_count(&fixture.agent, "candidate")
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            session
                .namespace_object_count(&fixture.agent, "capability_request")
                .await
                .unwrap(),
            0
        );
        // Given an old proposed/cache record, the identical shared gate is also
        // required before returning it; this is a cache-consumer component test.
        let cached = Candidate {
            id: "given-cached-candidate".into(),
            owner: fixture.agent.actor().into(),
            digest: fingerprint(&request).unwrap(),
            proposal: request.clone(),
            state: CandidateState::Proposed,
            improver_version: "unavailable".into(),
            evaluation_id: None,
            approved_by: None,
            created_at: now().unwrap(),
        };
        session
            .put(
                &fixture.agent,
                "candidate",
                &cached.id,
                fixture.agent.actor(),
                &cached,
            )
            .await
            .unwrap();
        session
            .cache(
                &fixture.agent,
                "service.propose",
                &request.request_key,
                &request,
                &cached.id,
                &cached,
            )
            .await
            .unwrap();
        let replay: Candidate = session
            .cached(
                &fixture.agent,
                "service.propose",
                &request.request_key,
                &request,
            )
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(reject_tool_only_skill_evidence(&mut session, &fixture.host, &replay.proposal.run_id, &replay.proposal.evidence_refs).await,
            Err(Error::Conflict(reason)) if reason == "host_capability_tool_only")
        );
        let mut changed = request.clone();
        changed.content.push_str(" changed");
        assert!(matches!(
            session
                .cached::<Candidate, _>(
                    &fixture.agent,
                    "service.propose",
                    &request.request_key,
                    &changed
                )
                .await,
            Err(Error::Conflict(_))
        ));
        assert_eq!(
            session
                .namespace_object_count(&fixture.agent, "candidate")
                .await
                .unwrap(),
            1
        );
        session.commit().await.unwrap();
    }

    #[tokio::test]
    async fn inspect_tool_only_diagnostic_is_owned_and_retained() {
        let fixture = Component::new().await;
        let inspect = Inspect {
            kind: EntityKind::Run,
            id: fixture.run.id.clone(),
        };
        let original = fixture
            .service
            .inspect(&fixture.agent, inspect.clone())
            .await
            .unwrap();
        assert_eq!(
            original,
            json!({"kind":"run","id":fixture.run.id,"owner":fixture.run.owner,"snapshot_id":fixture.run.snapshot_id,
            "status":fixture.run.status,"source":fixture.run.source,"created_at":fixture.run.created_at})
        );
        let id = fixture.record().await;
        let mut expected = original;
        expected["diagnostic"] = json!({"id":id,"route":"environment","consumer":"inspect","state":"retained","failure_kind":"host_capability_tool_only"});
        assert_eq!(
            fixture
                .service
                .inspect(&fixture.agent, inspect.clone())
                .await
                .unwrap(),
            expected
        );
        assert_eq!(
            fixture
                .service
                .inspect(&fixture.host, inspect.clone())
                .await
                .unwrap(),
            expected
        );
        let other = Context::new(fixture.host.namespace(), "another-agent", Role::Agent).unwrap();
        assert!(matches!(
            fixture.service.inspect(&other, inspect.clone()).await,
            Err(Error::NotFound)
        ));
        let foreign =
            Context::new("foreign-namespace", fixture.agent.actor(), Role::Agent).unwrap();
        assert!(matches!(
            fixture.service.inspect(&foreign, inspect.clone()).await,
            Err(Error::Forbidden)
        ));
        assert_eq!(expected["diagnostic"].as_object().unwrap().len(), 5);
        let wire = fixture
            .service
            .inspect(&fixture.agent, inspect)
            .await
            .unwrap()
            .to_string();
        assert!(!wire.contains(fixture.host.actor()));
        assert!(!wire.contains("snapshot_digest"));
        assert!(!wire.contains("application_id"));
        assert!(!wire.contains("given frozen"));
        let mut session = fixture.service.store.session().await.unwrap();
        let stored: Value = session.need(&fixture.host, "artifact", &id).await.unwrap();
        assert_eq!(stored["state"], "retained");
        session.commit().await.unwrap();
    }

    #[tokio::test]
    async fn normal_feedback_not_selected_and_unknown_keep_original_proposal_rules() {
        let fixture = Component::new().await;
        let mut session = fixture.service.store.session().await.unwrap();
        for case in ["not_selected", "unknown", "truncated"] {
            let mut snapshot = serde_json::to_value(&fixture.snapshot).unwrap();
            let mut application = fixture.application.clone();
            match case {
                "not_selected" => {
                    snapshot["bundle_digest"] = Value::Null;
                    application.receipt = None;
                }
                "unknown" => application.receipt.as_mut().unwrap().offered.clear(),
                "truncated" => application.receipt.as_mut().unwrap().truncated = true,
                _ => unreachable!(),
            }
            let snapshot: RunApplicationSnapshot = serde_json::from_value(snapshot).unwrap();
            crate::monitoring::record_tool_only_diagnostic_in_session(
                &mut session,
                &fixture.host,
                &snapshot,
                &application,
            )
            .await
            .unwrap();
            let id = crate::monitoring::tool_only_diagnostic_id(
                fixture.host.namespace(),
                &snapshot.run_id,
            )
            .unwrap();
            assert!(
                crate::monitoring::load_tool_only_diagnostic_in_session(
                    &mut session,
                    &fixture.host,
                    &id,
                    &snapshot.run_id,
                )
                .await
                .unwrap()
                .is_none(),
                "{case} must not produce a diagnostic"
            );
            assert!(
                matches!(
                    reject_tool_only_skill_evidence(
                        &mut session,
                        &fixture.host,
                        &snapshot.run_id,
                        std::slice::from_ref(&id),
                    )
                    .await,
                    Err(Error::NotFound)
                ),
                "{case} keeps the missing diagnostic reference error"
            );
        }
        // Self-reported wording alone cannot mint a fixed diagnostic or block a
        // normal feedback reference; it retains the original feedback contract.
        let request = Feedback {
            request_key: "ordinary-feedback".into(),
            run_id: fixture.run.id.clone(),
            outcome: Outcome::Failure,
            details: "host_capability_tool_only".into(),
            failure_class: Default::default(),
        };
        let feedback = FeedbackRecord {
            id: "ordinary-feedback".into(),
            owner: fixture.agent.actor().into(),
            request,
            verification: "self_reported_unverified".into(),
        };
        session
            .put(
                &fixture.agent,
                "feedback",
                &feedback.id,
                fixture.agent.actor(),
                &feedback,
            )
            .await
            .unwrap();
        reject_tool_only_skill_evidence(
            &mut session,
            &fixture.host,
            &fixture.run.id,
            std::slice::from_ref(&feedback.id),
        )
        .await
        .unwrap();
        assert_eq!(
            session
                .namespace_object_count(&fixture.agent, "candidate")
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            session
                .namespace_object_count(&fixture.agent, "capability_request")
                .await
                .unwrap(),
            0
        );
        session.commit().await.unwrap();
    }
}
