//! AG-029 (E16.5, E08; plan §11.5, §11.4): the durable facts of the actions,
//! idempotency bindings and invalidations the v4.2 monitoring, practice,
//! curriculum and management work introduced are protected recovery facts.
//!
//! Before this change a backup that was older than the trusted anchor on any of
//! them was restored without complaint, and a restored directory the anchor had
//! moved past was admitted, so a restore could redo a consolidation generation,
//! reopen a spent K=3 registration, re-schedule a coverage probe, re-run a
//! management job, or forget that an environment change had invalidated a
//! consolidation scope (the drift record). The decision rule is unchanged ("the
//! anchor and the backup agree on every protected fact"); only the set of facts
//! grew.
//!
//! Every case that involves a restore starts from a real `Store::backup` and a
//! real run of `scripts/restore_backup.py`; the gate cases run the real
//! `StartupGate` against the directory the script produced. Fixture bodies are
//! built from the engine's own record types where they are public (claim, run,
//! staging, drift, registration, binding, attempt set, probe job, schedule receipt,
//! management job), wrapped in the envelope `curriculum.rs` persists for the two
//! curriculum records (that type is private), and shaped after the engine's record
//! otherwise (the proposal, whose bundle is abbreviated, and the private input).
//! The restore script and the gate read only `kind`, `id`, the top-level
//! `schema_version` and the body as a whole, so nothing else about a fixture
//! matters to them.
//!
//! Redaction (ruling 4). A record that source revocation has redacted carries
//! `rsia.redacted.v1` and matches no protected schema, so a tombstone takes no
//! part in the comparison. The last three tests record what that does, measured
//! with the real cleanup closure:
//! - a backup that still holds the original while the anchor holds the tombstone
//!   is isolated (the original is a protected fact only the backup has), exactly
//!   as for the stage facts that were protected before this change;
//! - a backup taken after the cleanup restores;
//! - a record created after the backup and redacted before the restore is seen by
//!   neither side and is therefore not detected. That is a known, narrow gap and
//!   it is the same for the schemas that were protected before; the facts that
//!   account for a spend or bind an idempotency key are preserved by the cleanup
//!   and stay visible (the run record case below).

use evo_core::curriculum::{ProbeJobV1, ProbeTerminal, ProposalAttemptOutcome};
use evo_core::strategy::PracticePlan;
use evo_core::{Context, Job, JobState, Role, fingerprint, hash};
use evo_engine::curriculum::{
    ProbeScheduleReceiptV1, curriculum_probe_job_storage_id, probe_schedule_receipt_id,
};
use evo_engine::dispatch::{ManagementJob, ManagementJobState};
use evo_engine::monitoring::{
    CONSOLIDATION_CLAIM_SCHEMA, CONSOLIDATION_PROPOSAL_SCHEMA, CONSOLIDATION_RUN_SCHEMA,
    CONSOLIDATION_SCOPE_SCHEMA, CONSOLIDATION_STAGING_SCHEMA, ConsolidationClaim,
    ConsolidationClaimState, ConsolidationRunOutcome, ConsolidationRunRecord, ConsolidationScope,
    ConsolidationScopeIndex, ConsolidationStagingBinding, ENVIRONMENT_DRIFT_SCHEMA,
    ENVIRONMENT_IDENTITY_SCHEMA, EnvironmentDriftRecord, EnvironmentEvidenceScope,
    EnvironmentIdentityRecord, MonitoringCoordinator, SourceDependency,
};
use evo_engine::optimization::OPTIMIZATION_STAGE_FACT_SCHEMA;
use evo_engine::practice::{
    PRACTICE_ATTEMPT_SET_SCHEMA, PRACTICE_REGISTRATION_BINDING_SCHEMA,
    PRACTICE_REGISTRATION_SCHEMA, PracticeAttemptSetV1, PracticeOutcomeV1,
    PracticeRegistrationBindingV1, PracticeRegistrationV1, practice_attempt_set_storage_id,
    practice_registration_binding_storage_id,
};
use evo_engine::startup_gate::{
    RESTORE_ADMISSION_FILE, RecoveryPosture, StartupGate, StartupGateError,
};
use evo_storage::lifecycle::{
    BackupManifest, BackupWatermarkEntry, CleanupState, LifecycleStore,
    PROTECTED_ACTION_SCHEMA_VERSIONS, PROTECTED_OBJECT_KINDS, PROTECTED_SCHEMA_VERSIONS,
    RevokeTombstone, TypedObjectRef, read_control_plane_facts,
};
use evo_storage::{CAPACITY_MANAGEMENT_JOB_SCHEMA, Store};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

const RESTORE_SCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../scripts/restore_backup.py"
);
const NS: &str = "n";
const SOURCE_RUN: &str = "run-src";
const SCOPE_ID: &str = "consolidation-scope-1";
/// The script's verdict when the backup and the anchor disagree on a protected fact.
const FACTS_CHANGED: &str =
    "consumed query/alpha/dispatch/accounting facts changed; make a fresh backup";
/// The schema the revocation cleanup leaves in place of redacted content.
const REDACTED: &str = "rsia.redacted.v1";
/// `curriculum.rs` keeps this constant private; the literal is the persisted value.
const CURRICULUM_ENVELOPE_SCHEMA: &str = "rsia.curriculum_artifact_envelope.v1";
const PRIVATE_INPUT_SCHEMA: &str = "rsia.management_private_input.v1";

fn d(label: &str) -> String {
    hash(label.as_bytes())
}

fn admin() -> Context {
    Context::new(NS, "admin", Role::Admin).unwrap()
}

// ---------------------------------------------------------------------------
// fixtures: one record per protected class
// ---------------------------------------------------------------------------

/// One stored object plus the typed dependency edges written with it (the
/// revocation cleanup walks them).
#[derive(Clone)]
struct Fact {
    label: &'static str,
    kind: &'static str,
    id: String,
    schema: String,
    body: Value,
    edges: Vec<(&'static str, String)>,
}

impl Fact {
    fn new(label: &'static str, kind: &'static str, id: String, body: &impl Serialize) -> Self {
        let body = serde_json::to_value(body).unwrap();
        let schema = body["schema_version"].as_str().unwrap().to_owned();
        assert_eq!(body["id"], json!(id), "{label}: the body names its own key");
        Self {
            label,
            kind,
            id,
            schema,
            body,
            edges: Vec::new(),
        }
    }

    /// `self -> (dst_kind, dst_id)`: `self` depends on the destination, so it is a
    /// dependent the cleanup of the destination reaches.
    fn depends_on(mut self, dst_kind: &'static str, dst_id: &str) -> Self {
        self.edges.push((dst_kind, dst_id.to_owned()));
        self
    }

    /// The key the gate names in its diagnostics.
    fn key(&self) -> String {
        format!("{NS}/{}/{}", self.kind, self.id)
    }
}

fn claim_id() -> String {
    format!(
        "consolidation-claim-{}",
        &fingerprint(&(SCOPE_ID, 1u64)).unwrap()[..32]
    )
}

fn proposal_id() -> String {
    format!("consolidation-proposal-{}", claim_id())
}

fn claim_fact(state: ConsolidationClaimState) -> Fact {
    Fact::new(
        "consolidation claim",
        "artifact",
        claim_id(),
        &ConsolidationClaim {
            id: claim_id(),
            schema_version: CONSOLIDATION_CLAIM_SCHEMA.into(),
            scope_id: SCOPE_ID.into(),
            generation: 1,
            cycle_ids: vec!["cycle-1".into(), "cycle-2".into()],
            sources: vec![SourceDependency {
                id: SOURCE_RUN.into(),
                content_digest: d("source"),
            }],
            revoke_watermark: 1,
            state,
            execution_input_digest: None,
        },
    )
}

fn run_id() -> String {
    format!("consolidation-run-{}", claim_id())
}

fn run_fact() -> Fact {
    Fact::new(
        "consolidation run record",
        "artifact",
        run_id(),
        &ConsolidationRunRecord {
            id: run_id(),
            schema_version: CONSOLIDATION_RUN_SCHEMA.into(),
            claim_id: claim_id(),
            input_digest: Some(d("input")),
            outcome: ConsolidationRunOutcome::Candidate,
            reason: "a candidate was produced".into(),
            budget_call_ids: vec!["call-1".into(), "call-2".into()],
            proposal_id: Some(proposal_id()),
        },
    )
}

fn proposal_fact() -> Fact {
    // The candidate bundle is abbreviated to its digest; see the module comment.
    Fact::new(
        "consolidation proposal",
        "artifact",
        proposal_id(),
        &json!({
            "id": proposal_id(),
            "schema_version": CONSOLIDATION_PROPOSAL_SCHEMA,
            "claim_id": claim_id(),
            "scope_id": SCOPE_ID,
            "cycle_ids": ["cycle-1", "cycle-2"],
            "pairs_digest": d("pairs"),
            "parent_skill_digest": d("parent-skill"),
            "parent_bundle_digest": d("parent-bundle"),
            "candidate_skill_digest": d("candidate-skill"),
            "candidate_bundle": {"digest": d("candidate-bundle")},
            "candidate_bundle_digest": d("candidate-bundle"),
            "selection_digest": d("selection"),
            "terminal_fact_id": "terminal-fact-1",
            "sources": [{"id": SOURCE_RUN, "content_digest": d("source")}],
            "provenance": "program_fixture",
            "environment_digest": d("environment"),
            "revoke_watermark": 1,
        }),
    )
}

fn staging_fact() -> Fact {
    let id = format!("consolidation-staging-{}", proposal_id());
    Fact::new(
        "consolidation staging binding",
        "artifact",
        id.clone(),
        &ConsolidationStagingBinding {
            id,
            schema_version: CONSOLIDATION_STAGING_SCHEMA.into(),
            proposal_id: proposal_id(),
            candidate_id: "candidate-1".into(),
        },
    )
}

fn drift_id() -> String {
    format!(
        "environment-drift-{}",
        &fingerprint(&("environment-1", "environment-2", SCOPE_ID)).unwrap()[..32]
    )
}

/// The record `record_environment_drift` writes when an environment change
/// invalidates a consolidation scope.
fn drift_fact() -> Fact {
    Fact::new(
        "environment drift record",
        "artifact",
        drift_id(),
        &EnvironmentDriftRecord {
            id: drift_id(),
            schema_version: ENVIRONMENT_DRIFT_SCHEMA.into(),
            previous_environment_id: "environment-1".into(),
            current_environment_id: "environment-2".into(),
            previous_identity_digest: d("identity-previous"),
            current_identity_digest: d("identity-current"),
            invalidated_scope_id: SCOPE_ID.into(),
            reason: "the model identity changed".into(),
        },
    )
}

fn plan_k3() -> PracticePlan {
    PracticePlan::new(
        "cluster-1",
        vec!["task-1".into()],
        3,
        Some(d("authorization")),
    )
    .unwrap()
}

fn registration() -> PracticeRegistrationV1 {
    PracticeRegistrationV1::for_plan(&plan_k3(), "admin", 1).unwrap()
}

fn registration_fact() -> Fact {
    let registration = registration();
    Fact::new(
        "practice registration",
        "artifact",
        registration.id.clone(),
        &registration,
    )
}

fn binding_fact() -> Fact {
    let registration_id = registration().id;
    let id = practice_registration_binding_storage_id(&registration_id).unwrap();
    Fact::new(
        "practice registration binding",
        "artifact",
        id.clone(),
        &PracticeRegistrationBindingV1 {
            schema_version: PRACTICE_REGISTRATION_BINDING_SCHEMA.into(),
            id,
            registration_id,
            set_id: "practice-set-1".into(),
        },
    )
}

fn attempt_set_fact() -> Fact {
    let id = practice_attempt_set_storage_id("practice-set-1").unwrap();
    Fact::new(
        "practice attempt set",
        "artifact",
        id.clone(),
        &PracticeAttemptSetV1 {
            schema_version: PRACTICE_ATTEMPT_SET_SCHEMA.into(),
            id,
            set_id: "practice-set-1".into(),
            namespace: NS.into(),
            control_id: "dev-control".into(),
            episode_id: "episode-1".into(),
            step: 1,
            bundle_digest: d("bundle"),
            environment_digest: d("environment"),
            revoke_watermark: 1,
            request_digest: d("request"),
            plan: plan_k3(),
            registration_id: Some(registration().id),
            attempts: vec![],
            outcome: PracticeOutcomeV1::Incomplete {
                reason: "the root budget ran out".into(),
            },
            independent_clusters: 0,
            counts_as_independent_samples: false,
            created_at: 2,
        },
    )
}

fn curriculum_storage_id(record_kind: &str, id: &str) -> String {
    format!("e12-{}", fingerprint(&(record_kind, id)).unwrap())
}

/// A coverage probe job in the envelope the curriculum coordinator writes.
/// `advanced` is the same job after one valid attempt ended it.
fn probe_job_fact(advanced: bool) -> Fact {
    let id = curriculum_probe_job_storage_id("probe-1").unwrap();
    let job = ProbeJobV1 {
        schema_version: "rsia.coverage_probe_job.v1".into(),
        id: "probe-1".into(),
        profile_id: "profile-1".into(),
        state_id: "state-1".into(),
        state_digest: d("state"),
        trigger_digest: d("trigger"),
        root_budget_limit_micros: 1_000,
        curriculum_share_limit_micros: 100,
        effective_monetary_limit_micros: 0,
        provider_dispatch_count: 0,
        attempts: if advanced {
            vec![ProposalAttemptOutcome::Valid]
        } else {
            vec![]
        },
        terminal: advanced.then_some(ProbeTerminal::GeneratedValid),
    };
    Fact::new(
        "curriculum envelope (coverage probe job)",
        "artifact",
        id.clone(),
        &json!({
            "schema_version": CURRICULUM_ENVELOPE_SCHEMA,
            "id": id,
            "record_kind": "coverage_probe_job_v1",
            "payload": job,
        }),
    )
}

fn schedule_receipt_fact() -> Fact {
    let receipt = ProbeScheduleReceiptV1 {
        schema_version: ProbeScheduleReceiptV1::SCHEMA.into(),
        id: probe_schedule_receipt_id("request-key-1").unwrap(),
        idempotency_key: "request-key-1".into(),
        profile_id: "profile-1".into(),
        state_id: "state-1".into(),
        root_budget_limit_micros: 1_000,
        probe_job_id: "probe-1".into(),
        trigger_digest: d("trigger"),
        revoke_watermark: 1,
    };
    let id = curriculum_storage_id("probe_schedule_receipt_v1", &receipt.id);
    Fact::new(
        "curriculum envelope (probe schedule receipt)",
        "artifact",
        id.clone(),
        &json!({
            "schema_version": CURRICULUM_ENVELOPE_SCHEMA,
            "id": id,
            "record_kind": "probe_schedule_receipt_v1",
            "payload": receipt,
        }),
    )
}

fn job_fact(state: ManagementJobState) -> Fact {
    let failed = state == ManagementJobState::Failed;
    Fact::new(
        "management job",
        "job",
        "management-job-1".into(),
        &ManagementJob {
            id: "management-job-1".into(),
            schema_version: CAPACITY_MANAGEMENT_JOB_SCHEMA.into(),
            operation: "curriculum.step".into(),
            request_key: "request-key-1".into(),
            payload_digest: d("payload"),
            owner_actor: "admin".into(),
            owner_role: Role::Admin,
            state,
            step: if failed { "failed" } else { "accepted" }.into(),
            private_input_ref: "management-input-1".into(),
            result: None,
            error_code: failed.then(|| "conflict".to_owned()),
            cancel_requested: false,
            lease_token: None,
            lease_until: 0,
            generation: u64::from(failed),
            diagnostics: vec![],
            created_at: 1,
        },
    )
}

/// The request payload of a management job: deliberately not a protected fact.
fn private_input_fact() -> Fact {
    Fact::new(
        "management private input",
        "artifact",
        "management-input-1".into(),
        &json!({
            "id": "management-input-1",
            "schema_version": PRIVATE_INPUT_SCHEMA,
            "operation": "curriculum.step",
            "payload_digest": d("payload"),
            "payload": {"profile_id": "profile-1", "state_id": "state-1"},
        }),
    )
}

/// A pre-management job (`evo_core::Job`): it has no `schema_version` and lives in
/// the same object kind as the management jobs.
fn legacy_job_fact() -> Fact {
    let job = Job {
        id: "legacy-job".into(),
        owner: "admin".into(),
        run_id: "run".into(),
        state: JobState::Queued,
        lease_token: None,
        lease_until: 0,
        deadline: 10,
        attempts: 0,
        improver_version: "v1".into(),
        task_snapshot: "snapshot".into(),
        result_ids: vec![],
        error_code: None,
    };
    Fact {
        label: "legacy job",
        kind: "job",
        id: "legacy-job".into(),
        schema: String::new(),
        body: serde_json::to_value(job).unwrap(),
        edges: vec![],
    }
}

fn plain_artifact_fact() -> Fact {
    Fact {
        label: "unrelated artifact",
        kind: "artifact",
        id: "plain-1".into(),
        schema: "some.other.v1".into(),
        body: json!({"id":"plain-1","schema_version":"some.other.v1"}),
        edges: vec![],
    }
}

/// An object of a schema that was protected before this change.
fn stage_fact() -> Fact {
    Fact::new(
        "optimization stage fact (protected before this change)",
        "artifact",
        "stage-fact-1".into(),
        &json!({
            "schema_version": OPTIMIZATION_STAGE_FACT_SCHEMA,
            "id": "stage-fact-1",
            "artifact_id": "stage-fact-1",
            "namespace": NS,
            "kind": "request_prepared",
            "payload": {},
        }),
    )
}

/// One fact of every class this change protects (ten schemas, eleven facts: the
/// curriculum envelope twice, for its two record kinds that carry scheduling
/// idempotency and quota facts).
fn every_class() -> Vec<Fact> {
    vec![
        claim_fact(ConsolidationClaimState::Claimed),
        run_fact(),
        staging_fact(),
        proposal_fact(),
        drift_fact(),
        registration_fact(),
        binding_fact(),
        attempt_set_fact(),
        probe_job_fact(false),
        schedule_receipt_fact(),
        job_fact(ManagementJobState::Queued),
    ]
}

/// Facts that are mutated in place by their owner: the same key, a later body.
fn advanced_pairs() -> Vec<(Fact, Fact)> {
    vec![
        (
            claim_fact(ConsolidationClaimState::Claimed),
            claim_fact(ConsolidationClaimState::CompletedNoChange),
        ),
        (probe_job_fact(false), probe_job_fact(true)),
        (
            job_fact(ManagementJobState::Queued),
            job_fact(ManagementJobState::Failed),
        ),
    ]
}

/// An environment identity as the monitoring coordinator stores it; `variant`
/// makes two of them differ in identity and environment digest.
fn environment_record(id: &str, variant: &str) -> EnvironmentIdentityRecord {
    EnvironmentIdentityRecord {
        id: id.into(),
        schema_version: ENVIRONMENT_IDENTITY_SCHEMA.into(),
        identity_digest: d(&format!("identity-{variant}")),
        namespace: NS.into(),
        evidence_scope: EnvironmentEvidenceScope::ProgramFixture,
        profile_id: "profile-1".into(),
        release_id: None,
        bundle_digest: None,
        environment_digest: d(&format!("environment-{variant}")),
        model_identity: format!("model-{variant}"),
        ordered_tools: vec!["tool-a".into()],
        host_id: "host-1".into(),
        host_version: "1".into(),
        host_surface_digest: d("surface"),
        host_capabilities_digest: d("capabilities"),
        mandatory_context_digest: d("context"),
        development_manifest_digest: d("manifest"),
        grader_digest: d("grader"),
        generation_strategy_digest: d("strategy"),
        revoke_watermark: 1,
    }
}

/// A live consolidation scope over `environment-1`, in the shape and under the id
/// `close_development_cycle` gives it (the scope index is deliberately not a
/// protected schema).
fn scope_index_fact() -> Fact {
    let previous = environment_record("environment-1", "previous");
    let scope = ConsolidationScope {
        namespace: NS.into(),
        skill_id: "skill-1".into(),
        profile_id: "profile-1".into(),
        development_manifest_digest: d("manifest"),
        environment_id: previous.id.clone(),
        environment_identity_digest: previous.identity_digest.clone(),
        environment_digest: previous.environment_digest.clone(),
        grader_digest: d("grader"),
        generation_strategy_digest: d("strategy"),
        parent_skill_digest: d("parent-skill"),
        parent_bundle_digest: d("parent-bundle"),
        billing_scope: "scope-a".into(),
        root_budget_id: "root-a".into(),
        revoke_watermark: 1,
    };
    let id = format!(
        "consolidation-scope-{}",
        &fingerprint(&scope).unwrap()[..32]
    );
    Fact::new(
        "consolidation scope index",
        "artifact",
        id.clone(),
        &ConsolidationScopeIndex {
            id,
            schema_version: CONSOLIDATION_SCOPE_SCHEMA.into(),
            scope,
            cycle_ids: vec![],
            report_fact_ids: vec![],
            claim_ids: BTreeMap::new(),
            invalidated_by: None,
        },
    )
}

/// What `record_environment_drift` reads: the two environment identities and the
/// scope over the first one. None of them is a protected schema.
fn drift_prerequisites() -> Vec<Fact> {
    vec![
        Fact::new(
            "previous environment",
            "artifact",
            "environment-1".into(),
            &environment_record("environment-1", "previous"),
        ),
        Fact::new(
            "current environment",
            "artifact",
            "environment-2".into(),
            &environment_record("environment-2", "current"),
        ),
        scope_index_fact(),
    ]
}

// ---------------------------------------------------------------------------
// harness: a live database (the anchor), a real backup, the real script
// ---------------------------------------------------------------------------

/// Relative path -> content hash (files), target (symlinks) or "dir".
fn snapshot(root: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().display().to_string();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.file_type().is_symlink() {
                out.insert(
                    relative,
                    format!("symlink:{}", std::fs::read_link(&path).unwrap().display()),
                );
            } else if meta.is_dir() {
                out.insert(relative, "dir".into());
                walk(root, &path, out);
            } else {
                out.insert(relative, hash(&std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

async fn put_facts(store: &Store, facts: &[Fact]) {
    let admin = admin();
    let mut session = store.session().await.unwrap();
    for fact in facts {
        session
            .put(&admin, fact.kind, &fact.id, admin.actor(), &fact.body)
            .await
            .unwrap();
        for (dst_kind, dst_id) in &fact.edges {
            session
                .put_edge(&admin, fact.kind, &fact.id, dst_kind, dst_id)
                .await
                .unwrap();
        }
    }
    session.commit().await.unwrap();
}

async fn read_object(database: &Path, kind: &str, id: &str) -> Option<Value> {
    let store = Store::open(database).await.unwrap();
    let mut session = store.session().await.unwrap();
    let value = session.get::<Value>(&admin(), kind, id).await.unwrap();
    session.commit().await.unwrap();
    store.close().await;
    value
}

/// A revocation of `source` on the anchor, as the delta must report it.
struct Revocation {
    source: String,
    seq: i64,
    digest: String,
    tombstone_digest: String,
    reason: String,
    created_at: i64,
}

/// The script's outcome.
struct Restored {
    dest: PathBuf,
    root: PathBuf,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Restored {
    fn output(&self) -> String {
        format!("stdout={:?} stderr={:?}", self.stdout, self.stderr)
    }

    fn assert_isolated(&self, context: &str) {
        assert_eq!(
            self.code,
            Some(1),
            "{context}: the restore was not isolated: {}",
            self.output()
        );
        assert!(
            self.stderr.starts_with("ISOLATE ")
                && self.stderr.contains(FACTS_CHANGED)
                && self.stderr.trim_end().ends_with("; not mounting"),
            "{context}: {}",
            self.output()
        );
        assert!(!self.stdout.contains("RESTORE_OK"), "{context}");
        assert!(
            !self.dest.exists(),
            "{context}: an isolated restore produced a destination"
        );
        let dest_name = self
            .dest
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        for entry in std::fs::read_dir(&self.root).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            assert!(
                !name.starts_with(&format!(".{dest_name}.restore-")),
                "{context}: a staging directory was left behind: {name}"
            );
        }
    }

    fn assert_restored(&self, context: &str, events: usize) {
        assert_eq!(
            self.code,
            Some(0),
            "{context}: the restore was refused: {}",
            self.output()
        );
        assert!(
            self.stdout.contains(&format!("RESTORE_OK events={events}")),
            "{context}: {}",
            self.output()
        );
        assert!(self.dest.join("restore.json").is_file(), "{context}");
        assert!(self.dest.join("rsia.sqlite3").is_file(), "{context}");
    }

    fn data(&self) -> PathBuf {
        self.dest.join("rsia.sqlite3")
    }
}

struct Scenario {
    _guard: tempfile::TempDir,
    root: PathBuf,
    /// The live database: the trusted anchor.
    live: PathBuf,
    backup: PathBuf,
    base: Option<BackupWatermarkEntry>,
    manifest_bytes: Vec<u8>,
}

impl Scenario {
    /// A live database with namespace `n` at revoke watermark 1, the source run
    /// every dependent below hangs from, and `facts`. No backup yet.
    async fn begin(facts: &[Fact]) -> Self {
        let guard = tempfile::tempdir().unwrap();
        // the script refuses symlinks in any supplied path (`/var` on macOS)
        let root = guard.path().canonicalize().unwrap();
        let live = root.join("rsia.sqlite3");
        let store = Store::open(&live).await.unwrap();
        let admin = admin();
        let host = Context::new(NS, "host", Role::Host).unwrap();
        let mut session = store.session().await.unwrap();
        session
            .bump_watermark(&admin, &hash(b"initial-watermark"))
            .await
            .unwrap();
        session
            .put(
                &host,
                "run",
                SOURCE_RUN,
                host.actor(),
                &json!({"id":SOURCE_RUN,"schema_version":"rsia.optimization.source.v1","body":"source"}),
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
        put_facts(&store, facts).await;
        store.close().await;
        Self {
            _guard: guard,
            backup: root.join("backup"),
            root,
            live,
            base: None,
            manifest_bytes: Vec::new(),
        }
    }

    /// Take the real backup of the live database as it is now.
    async fn take_backup(&mut self) {
        let store = Store::open(&self.live).await.unwrap();
        store.backup(&self.backup).await.unwrap();
        store.close().await;
        self.manifest_bytes = std::fs::read(self.backup.join("backup-manifest.json")).unwrap();
        let manifest: BackupManifest = serde_json::from_slice(&self.manifest_bytes).unwrap();
        assert_eq!(manifest.watermarks.len(), 1);
        self.base = Some(manifest.watermarks[0].clone());
    }

    async fn with_backup(facts: &[Fact]) -> Self {
        let mut scenario = Self::begin(facts).await;
        scenario.take_backup().await;
        scenario
    }

    /// Facts the anchor holds and the backup does not (or holds in an older form).
    async fn anchor_writes(&self, facts: &[Fact]) {
        let store = Store::open(&self.live).await.unwrap();
        put_facts(&store, facts).await;
        store.close().await;
    }

    /// Revoke `source` on the anchor and run its cleanup closure to the end.
    async fn revoke_and_clean(&self, source: &str) -> Revocation {
        let store = Store::open(&self.live).await.unwrap();
        let admin = admin();
        let mut status = LifecycleStore::begin_revoke(
            &admin,
            &store,
            TypedObjectRef {
                kind: "run".into(),
                id: source.into(),
            },
            "revoked after the backup",
            9,
        )
        .await
        .unwrap();
        for _ in 0..64 {
            if matches!(status.state, CleanupState::Complete | CleanupState::Failed) {
                break;
            }
            status = LifecycleStore::cleanup_step(&admin, &store, &status.job_id, 100, 10)
                .await
                .unwrap();
        }
        assert_eq!(
            status.state,
            CleanupState::Complete,
            "the cleanup closure did not finish: {status:?}"
        );
        let mut session = store.session().await.unwrap();
        let tombstone: RevokeTombstone = session.need(&admin, "tombstone", source).await.unwrap();
        session.commit().await.unwrap();
        store.close().await;
        Revocation {
            source: source.into(),
            seq: i64::try_from(tombstone.watermark_seq).unwrap(),
            digest: tombstone.watermark_digest.clone(),
            tombstone_digest: hash(&serde_json::to_vec(&tombstone).unwrap()),
            reason: tombstone.reason.clone(),
            created_at: tombstone.created_at,
        }
    }

    /// The revoke delta that takes the backup's watermark to the anchor's.
    fn delta(&self, name: &str, revocations: &[Revocation]) -> PathBuf {
        let base = self.base.as_ref().expect("a backup was taken");
        assert!(revocations.len() <= 1, "one revocation per scenario");
        let events: Vec<Value> = revocations
            .iter()
            .map(|revocation| {
                json!({
                    "seq": revocation.seq,
                    "previous_digest": base.digest,
                    "digest": revocation.digest,
                    "source_kind": "run",
                    "source_id": revocation.source,
                    "tombstone_digest": revocation.tombstone_digest,
                    "reason": revocation.reason,
                    "created_at": revocation.created_at,
                })
            })
            .collect();
        let (latest_seq, latest_digest) = match revocations.first() {
            Some(revocation) => (revocation.seq, revocation.digest.clone()),
            None => (base.seq, base.digest.clone()),
        };
        let path = self.root.join(name);
        std::fs::write(
            &path,
            serde_json::to_vec(&json!({
                "schema_version": "rsia.revoke_delta.v1",
                "base_manifest_sha256": hash(&self.manifest_bytes),
                "namespaces": [{
                    "namespace": base.namespace,
                    "base_seq": base.seq,
                    "base_digest": base.digest,
                    "events": events,
                    "latest_seq": latest_seq,
                    "latest_digest": latest_digest,
                }],
            }))
            .unwrap(),
        )
        .unwrap();
        path
    }

    /// Run the real script against the backup, with the live database as anchor.
    fn restore(&self, name: &str, delta: &Path) -> Restored {
        let dest = self.root.join(name);
        let output = Command::new("python3")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .arg(RESTORE_SCRIPT)
            .arg("--backup")
            .arg(&self.backup)
            .arg("--dest")
            .arg(&dest)
            .arg("--revoke-delta")
            .arg(delta)
            .arg("--trusted-revocations-db")
            .arg(&self.live)
            .output()
            .unwrap();
        Restored {
            dest,
            root: self.root.clone(),
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// `restore` with a delta that reports no revocation.
    fn restore_without_events(&self, name: &str) -> Restored {
        let delta = self.delta(&format!("{name}-delta.json"), &[]);
        self.restore(name, &delta)
    }
}

/// The gate must refuse the directory with `recovery_quarantine` and write nothing.
async fn quarantine_reason(data: &Path, anchor: &Path, directory: &Path) -> String {
    let before = snapshot(directory);
    let error = StartupGate::new(data, Some(anchor))
        .evaluate()
        .await
        .unwrap_err();
    let StartupGateError::Quarantine(reason) = &error else {
        panic!("expected a quarantine, got {error}");
    };
    let text = error.to_string();
    assert!(
        text.starts_with("recovery_quarantine: ") && text.ends_with("; not mounting"),
        "{text}"
    );
    assert_eq!(
        snapshot(directory),
        before,
        "a refused startup changed the data directory ({text})"
    );
    reason.clone()
}

// ---------------------------------------------------------------------------
// 1. the two lists
// ---------------------------------------------------------------------------

fn python_tuple(source: &str, marker: &str) -> Vec<String> {
    assert_eq!(
        source.matches(marker).count(),
        1,
        "{marker:?} must appear exactly once in protected_facts"
    );
    let rest = &source[source.find(marker).unwrap() + marker.len()..];
    let body = &rest[..rest.find(')').expect("closing parenthesis")];
    let mut out = Vec::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c == '"' {
            out.push(chars.by_ref().take_while(|c| *c != '"').collect());
        }
    }
    out
}

/// The schemas the engine persists for this round's action and idempotency facts,
/// taken from the engine's own constants where it exports them.
fn engine_action_schemas() -> BTreeSet<String> {
    [
        CONSOLIDATION_CLAIM_SCHEMA,
        CONSOLIDATION_RUN_SCHEMA,
        CONSOLIDATION_STAGING_SCHEMA,
        CONSOLIDATION_PROPOSAL_SCHEMA,
        ENVIRONMENT_DRIFT_SCHEMA,
        PRACTICE_REGISTRATION_SCHEMA,
        PRACTICE_REGISTRATION_BINDING_SCHEMA,
        PRACTICE_ATTEMPT_SET_SCHEMA,
        CURRICULUM_ENVELOPE_SCHEMA,
        CAPACITY_MANAGEMENT_JOB_SCHEMA,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[test]
fn the_action_schemas_are_listed_once_in_rust_and_in_the_restore_script() {
    let expected = engine_action_schemas();
    assert_eq!(expected.len(), 10, "ten distinct schemas");
    let listed: BTreeSet<String> = PROTECTED_ACTION_SCHEMA_VERSIONS
        .iter()
        .map(|schema| (*schema).to_owned())
        .collect();
    assert_eq!(
        PROTECTED_ACTION_SCHEMA_VERSIONS.len(),
        listed.len(),
        "a schema is listed twice"
    );
    assert_eq!(listed, expected, "the Rust list is the engine's schemas");

    // the two Rust groups do not overlap and the private input is in neither
    for schema in PROTECTED_SCHEMA_VERSIONS {
        assert!(!listed.contains(schema), "{schema} is in both groups");
    }
    assert!(!listed.contains(PRIVATE_INPUT_SCHEMA));
    assert!(!PROTECTED_SCHEMA_VERSIONS.contains(&PRIVATE_INPUT_SCHEMA));
    // the mutable scope index is compared through its claims and the drift record
    assert!(!listed.contains(CONSOLIDATION_SCOPE_SCHEMA));
    assert!(!PROTECTED_SCHEMA_VERSIONS.contains(&CONSOLIDATION_SCOPE_SCHEMA));
    // a redaction tombstone matches no protected schema (ruling 4)
    assert!(!listed.contains(REDACTED));
    assert!(!PROTECTED_SCHEMA_VERSIONS.contains(&REDACTED));
    // "job" is matched by schema only: the kind list is unchanged
    assert_eq!(
        PROTECTED_OBJECT_KINDS,
        ["budget", "reservation", "evaluation", "receipt"]
    );

    // and the script holds every one of them, and nothing the Rust side lacks
    let script = std::fs::read_to_string(RESTORE_SCRIPT).unwrap();
    let protected = &script[script.find("def protected_facts").unwrap()..];
    let protected = &protected[..protected.find("def verify_delta").unwrap()];
    let in_script: BTreeSet<String> = python_tuple(protected, "schema in (").into_iter().collect();
    for schema in &expected {
        assert!(
            in_script.contains(schema),
            "{schema} is missing in the script"
        );
    }
    assert!(!in_script.contains(PRIVATE_INPUT_SCHEMA));
    assert!(!in_script.contains(CONSOLIDATION_SCOPE_SCHEMA));
    let mut both: BTreeSet<String> = listed.clone();
    both.extend(PROTECTED_SCHEMA_VERSIONS.iter().map(|s| (*s).to_owned()));
    assert_eq!(in_script, both, "the script and Rust lists drifted");

    // every fixture carries a listed schema and sits under the kind the engine uses
    for fact in every_class() {
        assert!(
            listed.contains(&fact.schema),
            "{}: {}",
            fact.label,
            fact.schema
        );
        assert_eq!(fact.body["schema_version"], json!(fact.schema));
        assert_eq!(
            fact.kind,
            if fact.schema == CAPACITY_MANAGEMENT_JOB_SCHEMA {
                "job"
            } else {
                "artifact"
            }
        );
    }
    let classes: BTreeSet<String> = every_class().into_iter().map(|fact| fact.schema).collect();
    assert_eq!(classes, expected, "every class has a fixture");
}

/// `protected_facts` of the real script over `database`, as sorted `ns/kind/id`.
fn script_selection(database: &Path) -> Vec<String> {
    let program = r#"
import importlib.util, json, sqlite3, sys
spec = importlib.util.spec_from_file_location("restore_backup", sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
con = sqlite3.connect("file:" + sys.argv[2] + "?mode=ro", uri=True)
keys = sorted("/".join(fact[:3]) for fact in module.protected_facts(con, {"n"}) if len(fact) == 4)
print(json.dumps(keys))
"#;
    let output = Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg("-c")
        .arg(program)
        .arg(RESTORE_SCRIPT)
        .arg(database)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "protected_facts failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[tokio::test]
async fn the_script_and_the_reader_select_the_same_objects_in_a_database_holding_every_class() {
    let scenario = Scenario::begin(&[]).await;
    let mut facts = every_class();
    let protected_keys: BTreeSet<String> = facts.iter().map(Fact::key).collect();
    facts.extend([
        // not protected: the request payload, the mutable scope index, a
        // pre-management job (no schema), a job of a schema this code does not
        // know, a redaction tombstone and an unrelated artifact
        private_input_fact(),
        scope_index_fact(),
        legacy_job_fact(),
        Fact {
            label: "job of an unknown schema",
            kind: "job",
            id: "damaged-job".into(),
            schema: "rsia.management_job.v999".into(),
            body: json!({"id":"damaged-job","schema_version":"rsia.management_job.v999"}),
            edges: vec![],
        },
        Fact {
            label: "redaction tombstone of a claim",
            kind: "artifact",
            id: "redacted-claim".into(),
            schema: REDACTED.into(),
            body: json!({"id":"redacted-claim","schema_version":REDACTED,"state":"source_revoked","original_kind":"artifact","original_schema":CONSOLIDATION_CLAIM_SCHEMA,"original_digest":d("claim"),"metadata":{}}),
            edges: vec![],
        },
        plain_artifact_fact(),
        // protected before this change, by schema and by kind
        stage_fact(),
        Fact {
            label: "reservation",
            kind: "reservation",
            id: "res-1".into(),
            schema: String::new(),
            body: json!({"id":"res-1","state":"reserved"}),
            edges: vec![],
        },
    ]);
    let mut all_keys = protected_keys.clone();
    all_keys.insert(format!("{NS}/artifact/stage-fact-1"));
    all_keys.insert(format!("{NS}/reservation/res-1"));

    let store = Store::open(&scenario.live).await.unwrap();
    put_facts(&store, &facts).await;
    // a namespace with no revoke watermark is out of scope for both implementations
    let other = Context::new("m", "admin", Role::Admin).unwrap();
    let claim = claim_fact(ConsolidationClaimState::Claimed);
    let mut session = store.session().await.unwrap();
    session
        .put(&other, claim.kind, &claim.id, other.actor(), &claim.body)
        .await
        .unwrap();
    session.commit().await.unwrap();
    store.close().await;

    let from_rust: BTreeSet<String> = read_control_plane_facts(&scenario.live)
        .await
        .unwrap()
        .protected_objects
        .keys()
        .map(|(namespace, kind, id)| format!("{namespace}/{kind}/{id}"))
        .collect();
    let from_script: BTreeSet<String> = script_selection(&scenario.live).into_iter().collect();
    assert_eq!(from_rust, all_keys, "the Rust reader's selection");
    assert_eq!(from_script, all_keys, "the script's selection");
    // the eleven fixtures are all in it, under both kinds
    assert!(protected_keys.iter().all(|key| from_rust.contains(key)));
    assert!(from_rust.iter().any(|key| key.starts_with("n/job/")));
    assert!(!from_rust.iter().any(|key| key.contains("legacy-job")));
}

// ---------------------------------------------------------------------------
// 2. the script isolates a backup that is behind the anchor
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_backup_older_than_the_anchor_on_any_action_fact_is_isolated() {
    for fact in every_class() {
        let scenario = Scenario::with_backup(&[]).await;
        scenario.anchor_writes(std::slice::from_ref(&fact)).await;
        let restored = scenario.restore_without_events("restored");
        restored.assert_isolated(fact.label);
    }
}

#[tokio::test]
async fn an_action_fact_that_advanced_on_the_anchor_isolates_the_backup() {
    for (before, after) in advanced_pairs() {
        assert_eq!((&before.kind, &before.id), (&after.kind, &after.id));
        assert_ne!(before.body, after.body);
        let scenario = Scenario::with_backup(std::slice::from_ref(&before)).await;
        // control: while the anchor still equals the backup the restore goes through
        scenario
            .restore_without_events("unchanged")
            .assert_restored(before.label, 0);
        scenario.anchor_writes(std::slice::from_ref(&after)).await;
        let restored = scenario.restore_without_events("advanced");
        restored.assert_isolated(before.label);
    }
}

#[tokio::test]
async fn facts_outside_the_list_do_not_isolate_a_backup() {
    let scenario = Scenario::with_backup(&[]).await;
    scenario
        .anchor_writes(&[
            // the request payload is not an action or an account (ruling 1)
            private_input_fact(),
            // kind `job` is matched by schema, not as a whole kind
            legacy_job_fact(),
            plain_artifact_fact(),
        ])
        .await;
    scenario
        .restore_without_events("restored")
        .assert_restored("private input, legacy job, unrelated artifact", 0);
}

// ---------------------------------------------------------------------------
// 3. the gate quarantines a restored directory the anchor has moved past
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_restored_directory_the_anchor_has_moved_past_is_quarantined_by_the_gate() {
    for fact in every_class() {
        let scenario = Scenario::with_backup(&[]).await;
        let restored = scenario.restore_without_events("restored");
        restored.assert_restored(fact.label, 0);

        // While the anchor still equals the restore the directory verifies ...
        let verified = StartupGate::new(&restored.data(), Some(&scenario.live))
            .evaluate()
            .await
            .unwrap_or_else(|error| panic!("{}: {error}", fact.label));
        assert_eq!(verified.posture(), RecoveryPosture::RestoredVerified);
        // ... and stops verifying the moment the anchor consumes one of these facts.
        scenario.anchor_writes(std::slice::from_ref(&fact)).await;
        let reason = quarantine_reason(&restored.data(), &scenario.live, &restored.dest).await;
        assert!(
            reason.contains("consumed accounting facts differ"),
            "{}: {reason}",
            fact.label
        );
        assert!(
            reason.contains(&format!("only in the trusted anchor: {}", fact.key())),
            "{}: {reason}",
            fact.label
        );
        assert!(
            reason.contains("re-run restore_backup.py from a fresh backup"),
            "{}: {reason}",
            fact.label
        );
    }
}

#[tokio::test]
async fn an_action_fact_the_anchor_advanced_after_the_restore_quarantines_the_directory() {
    for (before, after) in advanced_pairs() {
        let scenario = Scenario::with_backup(std::slice::from_ref(&before)).await;
        let restored = scenario.restore_without_events("restored");
        restored.assert_restored(before.label, 0);
        scenario.anchor_writes(std::slice::from_ref(&after)).await;
        let reason = quarantine_reason(&restored.data(), &scenario.live, &restored.dest).await;
        assert!(
            reason.contains("consumed accounting facts differ")
                && reason.contains(&format!("changed: {}", before.key())),
            "{}: {reason}",
            before.label
        );
    }
}

// ---------------------------------------------------------------------------
// 4. positive control: equal facts restore and the gate admits
// ---------------------------------------------------------------------------

#[tokio::test]
async fn equal_action_facts_restore_and_the_gate_admits_the_directory() {
    let facts = every_class();
    let scenario = Scenario::with_backup(&facts).await;
    let restored = scenario.restore_without_events("restored");
    restored.assert_restored("every class, equal on both sides", 0);

    // the control is not vacuous: both sides really hold the facts as protected
    let expected: BTreeSet<(String, String, String)> = facts
        .iter()
        .map(|fact| (NS.to_owned(), fact.kind.to_owned(), fact.id.clone()))
        .collect();
    for (name, database) in [
        ("backup", scenario.backup.join("rsia.sqlite3")),
        ("anchor", scenario.live.clone()),
        ("restored", restored.data()),
    ] {
        let protected: BTreeSet<_> = read_control_plane_facts(&database)
            .await
            .unwrap()
            .protected_objects
            .keys()
            .cloned()
            .collect();
        assert_eq!(protected, expected, "{name}");
    }

    // first start: verified against the anchor, then admitted and recorded
    let decision = StartupGate::new(&restored.data(), Some(&scenario.live))
        .evaluate()
        .await
        .unwrap();
    assert_eq!(decision.posture(), RecoveryPosture::RestoredVerified);
    Store::open(&restored.data()).await.unwrap().close().await;
    assert!(!restored.dest.join(RESTORE_ADMISSION_FILE).exists());
    decision.admit().unwrap();
    assert!(restored.dest.join(RESTORE_ADMISSION_FILE).is_file());

    // the next start needs no anchor
    let again = StartupGate::new(&restored.data(), None)
        .evaluate()
        .await
        .unwrap();
    assert_eq!(again.posture(), RecoveryPosture::AdmittedPreviously);
}

// ---------------------------------------------------------------------------
// 5. redaction tombstones (ruling 4), measured with the real cleanup closure
// ---------------------------------------------------------------------------

/// The anchor holds a redaction tombstone where the backup holds the original.
async fn assert_redacted(database: &Path, fact: &Fact) {
    let stored = read_object(database, fact.kind, &fact.id)
        .await
        .unwrap_or_else(|| panic!("{}: the object is gone", fact.label));
    assert_eq!(stored["schema_version"], REDACTED, "{}", fact.label);
    assert_eq!(stored["state"], "source_revoked", "{}", fact.label);
    assert_eq!(
        stored["original_schema"],
        json!(fact.schema),
        "{}",
        fact.label
    );
}

fn protected_keys_of(facts: &evo_storage::lifecycle::ControlPlaneFacts) -> BTreeSet<String> {
    facts
        .protected_objects
        .keys()
        .map(|(namespace, kind, id)| format!("{namespace}/{kind}/{id}"))
        .collect()
}

#[tokio::test]
async fn a_claim_redacted_on_the_anchor_after_the_backup_isolates_the_restore() {
    // The fact under test, and a control of a schema that was protected before.
    for fact in [
        claim_fact(ConsolidationClaimState::Claimed).depends_on("run", SOURCE_RUN),
        stage_fact().depends_on("run", SOURCE_RUN),
    ] {
        let scenario = Scenario::with_backup(std::slice::from_ref(&fact)).await;
        let revocation = scenario.revoke_and_clean(SOURCE_RUN).await;
        assert_redacted(&scenario.live, &fact).await;

        // The mechanism: the original is a protected fact of the backup and the
        // tombstone on the anchor is not one, so the two sets differ.
        let backup_keys = protected_keys_of(
            &read_control_plane_facts(&scenario.backup.join("rsia.sqlite3"))
                .await
                .unwrap(),
        );
        let anchor_keys =
            protected_keys_of(&read_control_plane_facts(&scenario.live).await.unwrap());
        assert!(backup_keys.contains(&fact.key()), "{}", fact.label);
        assert!(!anchor_keys.contains(&fact.key()), "{}", fact.label);

        let delta = scenario.delta("delta.json", &[revocation]);
        scenario
            .restore("restored", &delta)
            .assert_isolated(fact.label);
    }
}

#[tokio::test]
async fn a_backup_taken_after_the_redaction_restores_and_the_gate_verifies_it() {
    let claim = claim_fact(ConsolidationClaimState::Claimed).depends_on("run", SOURCE_RUN);
    let mut scenario = Scenario::begin(std::slice::from_ref(&claim)).await;
    scenario.revoke_and_clean(SOURCE_RUN).await;
    assert_redacted(&scenario.live, &claim).await;
    // the operator's answer to the isolation above: a fresh backup of the anchor
    scenario.take_backup().await;
    let restored = scenario.restore_without_events("restored");
    restored.assert_restored("backup after the redaction", 0);
    let decision = StartupGate::new(&restored.data(), Some(&scenario.live))
        .evaluate()
        .await
        .unwrap();
    assert_eq!(decision.posture(), RecoveryPosture::RestoredVerified);
}

#[tokio::test]
async fn a_fact_created_and_redacted_after_the_backup_is_invisible_to_the_comparison() {
    // CHARACTERIZATION of a known, narrow gap; it is not an endorsement. A claim
    // the anchor took after the backup and then redacted (its source was revoked)
    // is a tombstone on the anchor and absent from the backup, so neither side
    // has a protected fact to disagree on, and the restore goes through. The stage
    // fact, protected since before this change, behaves the same way.
    //
    // What keeps it from being a way to redo the action (by reading
    // `claim_consolidation`, not exercised here): the restore replays the
    // revocation, so the namespace watermark is past the one the scope froze and a
    // new claim for that scope is refused. What the claim led to is accounted for
    // by facts the cleanup preserves: money in the root budget tables (compared
    // since AG-018) and the terminal run record, which the second half of this
    // test shows keeps the action visible.
    let claim = claim_fact(ConsolidationClaimState::Claimed).depends_on("run", SOURCE_RUN);
    for fact in [claim.clone(), stage_fact().depends_on("run", SOURCE_RUN)] {
        let scenario = Scenario::with_backup(&[]).await;
        scenario.anchor_writes(std::slice::from_ref(&fact)).await;
        let revocation = scenario.revoke_and_clean(SOURCE_RUN).await;
        assert_redacted(&scenario.live, &fact).await;
        let delta = scenario.delta("delta.json", &[revocation]);
        let restored = scenario.restore("restored", &delta);
        restored.assert_restored(fact.label, 1);
        let decision = StartupGate::new(&restored.data(), Some(&scenario.live))
            .evaluate()
            .await
            .unwrap_or_else(|error| panic!("{}: {error}", fact.label));
        assert_eq!(decision.posture(), RecoveryPosture::RestoredVerified);
        assert!(
            read_object(&restored.data(), fact.kind, &fact.id)
                .await
                .is_none(),
            "{}: the restored database does not hold the record",
            fact.label
        );
    }

    // The companion facts are what keeps the action visible. The terminal run
    // record is preserved by the cleanup (it is an account of what happened), so
    // if the claim led to one, the anchor holds a protected fact the backup lacks.
    let run = run_fact().depends_on("artifact", &claim.id);
    let scenario = Scenario::with_backup(&[]).await;
    scenario.anchor_writes(&[claim.clone(), run.clone()]).await;
    let revocation = scenario.revoke_and_clean(SOURCE_RUN).await;
    assert_redacted(&scenario.live, &claim).await;
    let kept = read_object(&scenario.live, run.kind, &run.id)
        .await
        .unwrap();
    assert_eq!(kept, run.body, "the cleanup preserved the run record");
    let delta = scenario.delta("delta.json", &[revocation]);
    scenario
        .restore("restored", &delta)
        .assert_isolated("claim redacted, run record preserved");
}

// ---------------------------------------------------------------------------
// 6. environment drift, recorded with the engine's own function
// ---------------------------------------------------------------------------

/// The anchor records an environment drift: one transaction invalidates the scope
/// (`invalidated_by` on the scope index) and writes the drift record.
async fn record_drift_on(database: &Path) -> EnvironmentDriftRecord {
    let store = Store::open(database).await.unwrap();
    let drift = MonitoringCoordinator::record_environment_drift(
        &admin(),
        &store,
        "environment-1",
        "environment-2",
        &scope_index_fact().id,
        "the model identity changed",
    )
    .await
    .unwrap();
    store.close().await;
    drift
}

#[tokio::test]
async fn an_environment_drift_recorded_after_the_backup_isolates_the_restore() {
    // The backup holds a live scope. The anchor then learns that its environment
    // drifted, which invalidates the scope. Restoring the old backup would forget
    // that, and consolidation could go on in an environment known to have changed.
    let scope = scope_index_fact();
    let scenario = Scenario::with_backup(&drift_prerequisites()).await;
    scenario
        .restore_without_events("unchanged")
        .assert_restored("before the drift", 0);

    let drift = record_drift_on(&scenario.live).await;
    assert_eq!(drift.schema_version, ENVIRONMENT_DRIFT_SCHEMA);
    assert_eq!(drift.invalidated_scope_id, scope.id);
    let stored = read_object(&scenario.live, "artifact", &drift.id)
        .await
        .unwrap();
    assert_eq!(stored, serde_json::to_value(&drift).unwrap());
    let index = read_object(&scenario.live, "artifact", &scope.id)
        .await
        .unwrap();
    assert_eq!(index["invalidated_by"], json!(drift.id));

    scenario
        .restore_without_events("restored")
        .assert_isolated("environment drift recorded after the backup");

    // control: a backup taken after the drift holds it, and restores
    let mut later = Scenario::begin(&drift_prerequisites()).await;
    record_drift_on(&later.live).await;
    later.take_backup().await;
    later
        .restore_without_events("restored")
        .assert_restored("backup taken after the drift", 0);
}

#[tokio::test]
async fn an_environment_drift_recorded_after_the_restore_quarantines_the_directory() {
    let scenario = Scenario::with_backup(&drift_prerequisites()).await;
    let restored = scenario.restore_without_events("restored");
    restored.assert_restored("before the drift", 0);
    let verified = StartupGate::new(&restored.data(), Some(&scenario.live))
        .evaluate()
        .await
        .unwrap();
    assert_eq!(verified.posture(), RecoveryPosture::RestoredVerified);

    let drift = record_drift_on(&scenario.live).await;
    let reason = quarantine_reason(&restored.data(), &scenario.live, &restored.dest).await;
    assert!(
        reason.contains("consumed accounting facts differ"),
        "{reason}"
    );
    assert!(
        reason.contains(&format!(
            "only in the trusted anchor: {NS}/artifact/{}",
            drift.id
        )),
        "{reason}"
    );
    // The scope index changed on the anchor too, but it is not a compared fact:
    // the drift record is the one that is named.
    assert!(!reason.contains("consolidation-scope-"), "{reason}");
}
