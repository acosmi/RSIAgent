//! AG-096 (E08; plan §11, §11.5; V017/V018/V075/V096 sub-scope): one historical
//! bound model request that the revocation cleanup cannot read must not hold the
//! cleanup of its whole namespace. Real SQLite store and real local budget APIs;
//! no model, no provider, zero monetary cost.
//!
//! The cleanup finds the budget calls derived from a revoked source by reading the
//! source closure of every stored request: once in `__budget_scan`, and again in
//! the recheck that runs when the queue is empty. It used to read a
//! `rsia.bound_model_request.v1` body through a `deny_unknown_fields` mirror of the
//! whole AG093 wire, re-run `ModelRequest::validate` and recompute the compiler
//! binding fingerprints, and any failure was an `Err`: the step rolled back, the
//! job stayed `pending` or `running` with no `last_error`, and an Admin retry
//! (which only takes a `failed` job) could not reach it.
//!
//! What the cleanup needs from a request is its source closure,
//! `request.source_closure[*].id`; the digests, the input and the compiler binding
//! are checks of the use path. A body whose closure cannot be read is isolated as
//! an unknown scope (a persistent `blocked_unknown_scope` record and a `Failed` job
//! with that diagnostic), its bytes are left as they are, and the rest of the
//! namespace is still cleaned. A body whose closure can be read is cleaned by that
//! closure whether or not the use path would still accept it. Accounting facts
//! never move.

use evo_core::contract::{HostCapabilities, Profile, SkillSnapshot};
use evo_core::evidence::Purpose;
use evo_core::optimization::{
    ModelInputPart, ModelInputRole, ModelRequest, ModelRequestContext, ModelStage,
};
use evo_core::skill_edit::{EvidenceRef, skill_snapshot_digest};
use evo_core::{Context, Role, Strategy, fingerprint, hash};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallReservation, BudgetStage,
    REGISTERED_EXECUTION_REQUEST_SCHEMA, RootBudgetAuthorization, UsageCharge,
};
use evo_storage::lifecycle::{CleanupState, CleanupStatus, LifecycleStore, TypedObjectRef};
use serde::Serialize;
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection, sqlite::SqliteConnectOptions};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const NS: &str = "tenant";
const SOURCE: &str = "run-s";
const OTHER: &str = "run-t";
const BOUND: &str = "rsia.bound_model_request.v1";
const REDACTED: &str = "rsia.redacted.v1";
const UNREADABLE: &str = "unreadable_budget_request:rsia.bound_model_request.v1";
const SECRET: &str = "BOUND-SECRET";
/// More than any scenario below needs: a cleanup that does not finish within it
/// is stuck, not slow.
const MAX_STEPS: usize = 500;

/// Calls by `(billing_scope, call_id)`. The budget scan reads them in that order,
/// so each irregular request sorts before the complete one it is set against.
type CallKey = (&'static str, &'static str);
const BAD: CallKey = ("scope-0-bad", "call-bad");
const NAMES_SOURCE: CallKey = ("scope-0-names-source", "call-names-source");
const NAMES_OTHER: CallKey = ("scope-0-names-other", "call-names-other");
const GOOD: CallKey = ("scope-1-good", "call-good");
const BOTH: CallKey = ("scope-2-both", "call-both");
const ELSEWHERE: CallKey = ("scope-3-elsewhere", "call-elsewhere");

fn admin() -> Context {
    Context::new(NS, "admin", Role::Admin).unwrap()
}

fn worker() -> Context {
    Context::new(NS, "worker", Role::Worker).unwrap()
}

fn host() -> Context {
    Context::new(NS, "broker", Role::Host).unwrap()
}

// ---------------------------------------------------------------------------
// The AG093 wire, built field by field: storage cannot depend on the engine's
// writer types. Field order matters where the wire hashes a struct.
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct CompilerBindingWire {
    schema_version: String,
    namespace: String,
    profile_registration_id: String,
    profile_digest: String,
    parent_digest: String,
    baseline_digest: String,
    parent_skill_digest: String,
    parent_strategy_digest: String,
    search_parent_skill_digest: String,
    search_parent_strategy_digest: String,
    baseline_skill_digest: String,
    baseline_strategy_digest: String,
    environment_digest: String,
    capabilities_digest: String,
    purpose: Purpose,
    application_order: Vec<String>,
    watermark: Option<(i64, String)>,
    live_profile_pointer_digest: Option<String>,
    closure: ContentClosureWire,
    binding_digest: String,
}

#[derive(Serialize)]
struct ContentClosureWire {
    nodes: Vec<Value>,
    edges: Vec<Value>,
    digest: String,
}

/// A complete bound request in the shape the broker stores: a valid model request
/// whose source closure is `sources`, bound to a self-consistent compiler input.
/// It passes every check the cleanup used to run on such a body (the strict AG093
/// wire, `ModelRequest::validate`, the binding fingerprints), which
/// `complete_bound_requests_are_read_not_isolated` shows at the baseline. It is
/// not registered compiler authority: the use path would not dispatch it.
fn complete_bound(call: &str, sources: &[&str]) -> Value {
    let search_parent = SkillSnapshot {
        content: "parent skill".into(),
        ..SkillSnapshot::empty()
    };
    let baseline = SkillSnapshot::empty();
    let strategy = Strategy::default();
    let capabilities = HostCapabilities {
        available: BTreeSet::new(),
        granted: BTreeSet::new(),
    };
    let profile = Profile {
        id: "profile-1".into(),
        evolution_enabled: true,
        parent_digest: hash(b"parent"),
        baseline_digest: hash(b"baseline"),
    };
    let request = ModelRequest::build(
        ModelRequestContext {
            request_id: call.into(),
            namespace: NS.into(),
            purpose: Purpose::Development,
            stage: ModelStage::ReflectFailure,
            episode_id: "episode-1".into(),
            step: 0,
            attempt: 0,
            parent_skill_digest: skill_snapshot_digest(&search_parent).unwrap(),
            bundle_digest: hash(b"bundle"),
            source_closure: sources
                .iter()
                .map(|id| EvidenceRef {
                    id: (*id).into(),
                    digest: hash(id.as_bytes()),
                })
                .collect(),
            model_digest: hash(b"model"),
            tools_digest: hash(b"tools"),
            rules_digest: hash(b"rules"),
            sampling_digest: hash(b"sampling"),
            revoke_watermark: 0,
            max_suggestions: 1,
        },
        vec![ModelInputPart {
            role: ModelInputRole::User,
            label: "task".into(),
            content: format!("{SECRET}-{call}"),
        }],
    )
    .unwrap();
    let mut binding = CompilerBindingWire {
        schema_version: "rsia.formal_compiler_input_binding.v2".into(),
        namespace: NS.into(),
        profile_registration_id: "registration-1".into(),
        profile_digest: hash(b"profile"),
        parent_digest: profile.parent_digest.clone(),
        baseline_digest: profile.baseline_digest.clone(),
        parent_skill_digest: hash(b"approved parent skill"),
        parent_strategy_digest: hash(b"approved parent strategy"),
        search_parent_skill_digest: fingerprint(&search_parent).unwrap(),
        search_parent_strategy_digest: fingerprint(&strategy).unwrap(),
        baseline_skill_digest: fingerprint(&baseline).unwrap(),
        baseline_strategy_digest: fingerprint(&strategy).unwrap(),
        environment_digest: hash(b"environment"),
        capabilities_digest: fingerprint(&capabilities).unwrap(),
        purpose: Purpose::Development,
        application_order: Vec::new(),
        watermark: None,
        live_profile_pointer_digest: None,
        closure: ContentClosureWire {
            nodes: Vec::new(),
            edges: Vec::new(),
            digest: fingerprint(&(Vec::<Value>::new(), Vec::<Value>::new())).unwrap(),
        },
        binding_digest: String::new(),
    };
    binding.binding_digest = fingerprint(&binding).unwrap();
    json!({
        "schema_version": BOUND,
        "request": request,
        "compiler_use": {
            "schema_version": "rsia.optimization.compiler_use.v1",
            "request": {
                "profile": profile,
                "search_parent": search_parent,
                "baseline": baseline,
                "search_parent_strategy": strategy,
                "expected_parent_strategy_digest": null,
                "baseline_strategy": strategy,
                "capabilities": capabilities,
                "environment_digest": hash(b"environment"),
                "purpose": Purpose::Development,
                "revoke_watermark": 0,
            },
            "binding": binding,
        },
    })
}

/// `complete_bound` with one change made by `edit`.
fn edited(call: &str, sources: &[&str], edit: impl FnOnce(&mut Value)) -> Value {
    let mut value = complete_bound(call, sources);
    edit(&mut value);
    value
}

fn artifact(body: String) -> BudgetArtifact {
    artifact_of(BOUND, body)
}

fn artifact_of(schema: &str, body: String) -> BudgetArtifact {
    BudgetArtifact {
        schema_version: schema.into(),
        digest: hash(body.as_bytes()),
        body,
    }
}

// ---------------------------------------------------------------------------
// Store, budget and cleanup fixtures
// ---------------------------------------------------------------------------

struct Fixture {
    _dir: tempfile::TempDir,
    path: PathBuf,
    store: Store,
    clock: i64,
}

impl Fixture {
    /// A store holding the run that is revoked below.
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rsia.sqlite3");
        let store = Store::open(&path).await.unwrap();
        let host = host();
        let mut session = store.session().await.unwrap();
        session
            .put(
                &host,
                "run",
                SOURCE,
                host.actor(),
                &json!({"id":SOURCE,"schema_version":"rsia.optimization.source.v1","body":"run-body"}),
            )
            .await
            .unwrap();
        session.commit().await.unwrap();
        Self {
            _dir: dir,
            path,
            store,
            clock: 10,
        }
    }

    async fn reopen(&mut self) {
        self.store.close().await;
        self.store = Store::open(&self.path).await.unwrap();
    }

    async fn begin(&self) -> String {
        LifecycleStore::begin_revoke(
            &admin(),
            &self.store,
            TypedObjectRef {
                kind: "run".into(),
                id: SOURCE.into(),
            },
            "source revoked",
            5,
        )
        .await
        .unwrap()
        .job_id
    }

    async fn status(&self, job: &str) -> CleanupStatus {
        LifecycleStore::cleanup_status(&admin(), &self.store, job)
            .await
            .unwrap()
    }

    /// One `cleanup_step` of one edge page. An `Err` rolls the whole step back;
    /// the panic shows that a second step fails the same way and that the job did
    /// not move, which is what a namespace held by one row looks like.
    async fn step(&mut self, ctx: &Context, job: &str) -> CleanupStatus {
        self.clock += 1;
        let before = self.status(job).await;
        match LifecycleStore::cleanup_step(ctx, &self.store, job, 1, self.clock).await {
            Ok(status) => status,
            Err(error) => {
                self.clock += 1;
                let again = LifecycleStore::cleanup_step(ctx, &self.store, job, 1, self.clock)
                    .await
                    .err();
                let after = self.status(job).await;
                panic!(
                    "cleanup_step rolled back with {error:?}, and again with {again:?}; \
                     the job did not move: before {before:?}, after {after:?}"
                );
            }
        }
    }

    /// Steps as the Worker until nothing is left to expand. A Worker never starts
    /// a retry round, so a `Failed` job is drained, not rescanned.
    async fn drain(&mut self, job: &str) -> CleanupStatus {
        let mut status = self.status(job).await;
        for _ in 0..MAX_STEPS {
            match status.state {
                CleanupState::Complete => return status,
                CleanupState::Failed if status.pending_nodes == 0 => return status,
                _ => status = self.step(&worker(), job).await,
            }
        }
        panic!("the cleanup did not finish within {MAX_STEPS} steps: {status:?}");
    }
}

async fn authorize(store: &Store, scope: &str) {
    store
        .authorize_root_budget(
            &admin(),
            &RootBudgetAuthorization {
                root_budget_id: format!("root-{scope}"),
                billing_scope: scope.into(),
                allowed_namespaces: vec![NS.into()],
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                payment_subject: "payer".into(),
                authorization_receipt_digest: hash(b"authorization"),
                per_call_cap_micros: 100,
                total_limit_micros: 1_000,
                created_at: 1,
            },
        )
        .await
        .unwrap();
}

/// Reserves the call through the real budget API with no source list, so only the
/// cleanup's reading of the request body can tie it to a run.
async fn reserve(
    store: &Store,
    (scope, call): CallKey,
    request: BudgetArtifact,
) -> BudgetCallFence {
    authorize(store, scope).await;
    let reserved = store
        .reserve_budget_call(
            &host(),
            &BudgetCallReservation {
                billing_scope: scope.into(),
                call_id: call.into(),
                dispatch_group_id: format!("group-{scope}"),
                stage: BudgetStage::Reflection,
                actual_input_digest: hash(format!("input-{scope}-{call}").as_bytes()),
                request_artifact: Some(request),
                max_cost_micros: 20,
                lease_token: format!("lease-{scope}"),
                lease_until: 100,
                now: 1,
            },
        )
        .await
        .unwrap();
    BudgetCallFence {
        billing_scope: scope.into(),
        call_id: call.into(),
        actual_input_digest: reserved.actual_input_digest,
        lease_token: reserved.lease_token,
        lease_epoch: reserved.lease_epoch,
        now: 2,
    }
}

/// A call that was dispatched and billed: real accounting the cleanup must keep.
async fn settle(store: &Store, call: CallKey, request: BudgetArtifact) {
    let fence = reserve(store, call, request).await;
    store.begin_budget_dispatch(&host(), &fence).await.unwrap();
    store
        .finalize_budget_call(
            &host(),
            &BudgetCallFence { now: 3, ..fence },
            &UsageCharge {
                amount_micros: 7,
                currency: "USD".into(),
                pricing_version: "pricing-v1".into(),
                provider_request_id: format!("provider-{}", call.0),
                usage_record_id: format!("usage-{}", call.0),
                output_digest: hash(b"actual fixture output"),
            },
        )
        .await
        .unwrap();
}

async fn connection(path: &Path) -> SqliteConnection {
    SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path))
        .await
        .unwrap()
}

/// Test-only damage: stores `body` as the call's request bytes, past the JSON
/// CHECK (as a damaged file or an old bypass would leave them). No cleanup state
/// and no accounting column is written.
async fn damage_request(path: &Path, (scope, call): CallKey, body: &str) {
    let mut c = connection(path).await;
    sqlx::query("PRAGMA ignore_check_constraints=ON")
        .execute(&mut c)
        .await
        .unwrap();
    let updated = sqlx::query(
        "UPDATE root_budget_calls SET request_artifact_body=?,request_artifact_digest=?
         WHERE namespace=? AND billing_scope=? AND call_id=?",
    )
    .bind(body)
    .bind(hash(body.as_bytes()))
    .bind(NS)
    .bind(scope)
    .bind(call)
    .execute(&mut c)
    .await
    .unwrap();
    assert_eq!(updated.rows_affected(), 1);
    c.close().await.unwrap();
}

/// The stored request of a call: `(schema, digest, body)`, byte for byte.
async fn raw_request(path: &Path, (scope, call): CallKey) -> (String, String, String) {
    let mut c = connection(path).await;
    let row = sqlx::query(
        "SELECT request_artifact_schema,request_artifact_digest,request_artifact_body
         FROM root_budget_calls WHERE namespace=? AND billing_scope=? AND call_id=?",
    )
    .bind(NS)
    .bind(scope)
    .bind(call)
    .fetch_one(&mut c)
    .await
    .unwrap();
    let raw = (
        row.get("request_artifact_schema"),
        row.get("request_artifact_digest"),
        row.get("request_artifact_body"),
    );
    c.close().await.unwrap();
    raw
}

async fn assert_request_redacted(path: &Path, call: CallKey) {
    let (schema, _, body) = raw_request(path, call).await;
    assert_eq!(schema, REDACTED, "{call:?} kept its request: {body}");
    assert!(!body.contains(SECRET), "{call:?} kept its content: {body}");
}

/// Every accounting fact: the billing columns of each call, the root budgets,
/// the dispatch groups and the budget events other than content redaction.
async fn accounting(path: &Path) -> Value {
    let mut c = connection(path).await;
    let calls: String = sqlx::query_scalar(
        "SELECT json_group_array(json_object('scope',billing_scope,'call',call_id,'state',state,
           'reserved',reserved_micros,'charged',actual_cost_micros,'currency',actual_currency,
           'dispatch',dispatch_id,'provider',provider_request_id,'usage',usage_record_id,
           'output',output_digest,'epoch',lease_epoch))
         FROM (SELECT * FROM root_budget_calls ORDER BY billing_scope,call_id)",
    )
    .fetch_one(&mut c)
    .await
    .unwrap();
    let roots: String = sqlx::query_scalar(
        "SELECT json_group_array(json_object('scope',billing_scope,'spent',spent_micros,
           'reserved',reserved_micros,'stopped',stopped))
         FROM (SELECT * FROM root_budgets ORDER BY billing_scope)",
    )
    .fetch_one(&mut c)
    .await
    .unwrap();
    let groups: String = sqlx::query_scalar(
        "SELECT json_group_array(json_object('scope',billing_scope,'group',dispatch_group_id,
           'stopped',stopped,'reason',stop_reason))
         FROM (SELECT * FROM root_budget_dispatch_groups ORDER BY billing_scope,dispatch_group_id)",
    )
    .fetch_one(&mut c)
    .await
    .unwrap();
    let events: String = sqlx::query_scalar(
        "SELECT json_group_array(json_object('seq',seq,'scope',billing_scope,'call',call_id,
           'kind',event_kind,'at',event_at,'details',details))
         FROM (SELECT * FROM root_budget_events WHERE event_kind<>'content_redacted' ORDER BY seq)",
    )
    .fetch_one(&mut c)
    .await
    .unwrap();
    c.close().await.unwrap();
    json!({
        "calls": serde_json::from_str::<Value>(&calls).unwrap(),
        "roots": serde_json::from_str::<Value>(&roots).unwrap(),
        "groups": serde_json::from_str::<Value>(&groups).unwrap(),
        "events": serde_json::from_str::<Value>(&events).unwrap(),
    })
}

async fn content_redactions(path: &Path, (scope, call): CallKey) -> i64 {
    let mut c = connection(path).await;
    let count = sqlx::query_scalar(
        "SELECT COUNT(*) FROM root_budget_events
         WHERE billing_scope=? AND call_id=? AND event_kind='content_redacted'",
    )
    .bind(scope)
    .bind(call)
    .fetch_one(&mut c)
    .await
    .unwrap();
    c.close().await.unwrap();
    count
}

/// The cleanup events of `kind` of the job, oldest first, read back from disk.
async fn cleanup_events(path: &Path, job: &str, kind: &str) -> Vec<Value> {
    let mut c = connection(path).await;
    let encoded: String = sqlx::query_scalar(
        "SELECT json_group_array(json_object('kind',node_kind,'id',node_id,'details',json(details)))
         FROM (SELECT * FROM revoke_cleanup_events
               WHERE namespace=? AND job_id=? AND event_kind=? ORDER BY seq)",
    )
    .bind(NS)
    .bind(job)
    .bind(kind)
    .fetch_one(&mut c)
    .await
    .unwrap();
    c.close().await.unwrap();
    serde_json::from_str::<Vec<Value>>(&encoded).unwrap()
}

fn isolation_of(call: CallKey) -> Value {
    json!({"kind":"reservation","id":call.1,"details":{"error":UNREADABLE}})
}

async fn source_exists(path: &Path) -> bool {
    let mut c = connection(path).await;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM objects WHERE namespace=? AND kind='run' AND id=?",
    )
    .bind(NS)
    .bind(SOURCE)
    .fetch_one(&mut c)
    .await
    .unwrap();
    c.close().await.unwrap();
    count != 0
}

// ---------------------------------------------------------------------------
// Scenarios
// ---------------------------------------------------------------------------

/// The request of the irregular call: bytes the budget API accepts at the
/// reservation, or bytes only a damaged database holds. Both are written over a
/// complete request after it settled (`assert_isolated`).
enum Stored {
    Accepted(Value),
    AcceptedText(String),
    Damaged(String),
}

/// (a) a complete bound request naming the revoked run, (b) a bound request of
/// the same run whose source closure cannot be read, (c) the revocation of the
/// run. (b) sorts first, so the scan meets it before (a).
///
/// (b) is isolated by the step that meets it: a persistent unknown-scope record,
/// the job `Failed` with that diagnostic, its bytes untouched. The job still goes
/// on, across a restart, past (b): (a) is redacted and the run's own content is
/// deleted. No accounting fact moves.
async fn assert_isolated(case: &str, bad: Stored) {
    let mut f = Fixture::new().await;
    // AG-107 fixture (independent review: plan §7.2.1 L831 rechecks a queued call
    // before dispatch, §11.3 L1112 isolates what is missing, and the AG-107 card):
    // the dispatch now refuses a call reserved without a source list whose request
    // closure cannot be read (`budget_sources_unverifiable`), so accepted irregular
    // bytes can no longer be dispatched and billed through the API. Like damaged
    // bytes, they are written over the settled complete request with the
    // test-only `damage_request`, so the cleanup still meets a billed call whose
    // request it cannot read. Replacement coverage for the refused dispatch:
    // `budget_ref_missing_fail_closed_v42.rs` (T2).
    let text = match bad {
        Stored::Accepted(value) => value.to_string(),
        Stored::AcceptedText(text) | Stored::Damaged(text) => text,
    };
    let complete = complete_bound(BAD.1, &[SOURCE]).to_string();
    settle(&f.store, BAD, artifact(complete)).await;
    damage_request(&f.path, BAD, &text).await;
    settle(
        &f.store,
        GOOD,
        artifact(complete_bound(GOOD.1, &[SOURCE]).to_string()),
    )
    .await;
    let bad_request = raw_request(&f.path, BAD).await;
    let money = accounting(&f.path).await;
    let job = f.begin().await;

    let first = f.step(&worker(), &job).await;
    assert_eq!(first.state, CleanupState::Failed, "{case}: {first:?}");
    assert_eq!(first.last_error.as_deref(), Some(UNREADABLE), "{case}");
    assert!(first.pending_nodes > 0, "{case}: {first:?}");
    f.reopen().await;
    let done = f.drain(&job).await;
    assert_eq!(done.state, CleanupState::Failed, "{case}: {done:?}");
    assert_eq!(done.pending_nodes, 0, "{case}: {done:?}");
    assert_eq!(done.last_error.as_deref(), Some(UNREADABLE), "{case}");
    assert!(
        done.processed_nodes > first.processed_nodes,
        "{case}: {done:?}"
    );

    assert_eq!(
        cleanup_events(&f.path, &job, "blocked_unknown_scope").await,
        vec![isolation_of(BAD)],
        "{case}"
    );
    assert_eq!(raw_request(&f.path, BAD).await, bad_request, "{case}");
    assert_eq!(content_redactions(&f.path, BAD).await, 0, "{case}");
    assert_request_redacted(&f.path, GOOD).await;
    assert!(!source_exists(&f.path).await, "{case}");
    assert_eq!(accounting(&f.path).await, money, "{case}");
}

/// (b) and (b') carry the same irregularity, which a use-path check rejects but
/// which leaves the source closure readable; (b) names the revoked run and (b')
/// another run only. (a) is complete and names the revoked run. Nothing is
/// isolated: (b) and (a) are redacted, (b') is left as it is, the job completes
/// and no accounting fact moves.
async fn assert_read_by_closure(case: &str, irregular: impl Fn(&str, &[&str]) -> Value) {
    let mut f = Fixture::new().await;
    settle(
        &f.store,
        NAMES_SOURCE,
        artifact(irregular(NAMES_SOURCE.1, &[SOURCE]).to_string()),
    )
    .await;
    settle(
        &f.store,
        NAMES_OTHER,
        artifact(irregular(NAMES_OTHER.1, &[OTHER]).to_string()),
    )
    .await;
    settle(
        &f.store,
        GOOD,
        artifact(complete_bound(GOOD.1, &[SOURCE]).to_string()),
    )
    .await;
    let other_request = raw_request(&f.path, NAMES_OTHER).await;
    let money = accounting(&f.path).await;
    let job = f.begin().await;

    let done = f.drain(&job).await;
    assert_eq!(done.state, CleanupState::Complete, "{case}: {done:?}");
    assert_eq!(done.last_error, None, "{case}");
    assert!(
        cleanup_events(&f.path, &job, "blocked_unknown_scope")
            .await
            .is_empty(),
        "{case}"
    );
    assert_request_redacted(&f.path, NAMES_SOURCE).await;
    assert_request_redacted(&f.path, GOOD).await;
    assert_eq!(
        raw_request(&f.path, NAMES_OTHER).await,
        other_request,
        "{case}"
    );
    assert_eq!(content_redactions(&f.path, NAMES_OTHER).await, 0, "{case}");
    assert!(!source_exists(&f.path).await, "{case}");
    assert_eq!(accounting(&f.path).await, money, "{case}");
}

#[tokio::test]
async fn an_unparsable_bound_request_is_isolated_and_the_namespace_is_still_cleaned() {
    let complete = complete_bound(BAD.1, &[SOURCE]).to_string();
    // Cut in the middle of the body: no writer produces it, the JSON CHECK refuses it.
    assert_isolated(
        "truncated JSON",
        Stored::Damaged(complete[..complete.len() / 2].into()),
    )
    .await;
    // A second `request` key: `serde_json::Value` would keep the last one, which
    // names another run only and hides the revoked one in the first.
    let hidden = complete_bound(BAD.1, &[OTHER])["request"].to_string();
    let request = complete_bound(BAD.1, &[SOURCE])["request"].to_string();
    let compiler_use = complete_bound(BAD.1, &[SOURCE])["compiler_use"].to_string();
    assert_isolated(
        "duplicate request key",
        Stored::AcceptedText(format!(
            r#"{{"compiler_use":{compiler_use},"request":{request},"request":{hidden},"schema_version":"{BOUND}"}}"#
        )),
    )
    .await;
}

#[tokio::test]
async fn a_bound_request_missing_its_closure_or_wire_version_is_isolated() {
    let without = |field: &'static str| {
        edited(BAD.1, &[SOURCE], |value| {
            value.as_object_mut().unwrap().remove(field);
        })
    };
    assert_isolated("no request", Stored::Accepted(without("request"))).await;
    assert_isolated(
        "no source closure",
        Stored::Accepted(edited(BAD.1, &[SOURCE], |value| {
            value["request"]
                .as_object_mut()
                .unwrap()
                .remove("source_closure");
        })),
    )
    .await;
    assert_isolated(
        "no wire version",
        Stored::Accepted(without("schema_version")),
    )
    .await;
    assert_isolated(
        "another wire version",
        Stored::Accepted(edited(BAD.1, &[SOURCE], |value| {
            value["schema_version"] = json!("rsia.bound_model_request.v2");
        })),
    )
    .await;
}

#[tokio::test]
async fn a_bound_request_with_a_mistyped_closure_is_isolated() {
    assert_isolated(
        "closure is not a list",
        Stored::Accepted(edited(BAD.1, &[SOURCE], |value| {
            value["request"]["source_closure"] = json!(SOURCE);
        })),
    )
    .await;
    // The positional (list) form of the request is not the wire either.
    assert_isolated(
        "request is a list",
        Stored::Accepted(edited(BAD.1, &[SOURCE], |value| {
            value["request"] = json!([[{"id": SOURCE, "digest": hash(SOURCE.as_bytes())}]]);
        })),
    )
    .await;
    assert_isolated(
        "closure entry is not an object",
        Stored::Accepted(edited(BAD.1, &[SOURCE], |value| {
            value["request"]["source_closure"] = json!([SOURCE]);
        })),
    )
    .await;
    assert_isolated(
        "source id is a number",
        Stored::Accepted(edited(BAD.1, &[SOURCE], |value| {
            value["request"]["source_closure"][0]["id"] = json!(7);
        })),
    )
    .await;
    // One entry names the revoked run, the other is not an identifier: the
    // closure is not one the budget index could hold, so none of it is trusted.
    assert_isolated(
        "source id is not an identifier",
        Stored::Accepted(edited(BAD.1, &[SOURCE, OTHER], |value| {
            value["request"]["source_closure"][1]["id"] = json!("run t!");
        })),
    )
    .await;
}

#[tokio::test]
async fn a_bound_request_with_unknown_fields_is_cleaned_by_its_closure() {
    assert_read_by_closure("unknown top-level field", |call, sources| {
        edited(call, sources, |value| {
            value["unexpected"] = json!(true);
        })
    })
    .await;
    assert_read_by_closure("unknown field in a closure entry", |call, sources| {
        edited(call, sources, |value| {
            value["request"]["source_closure"][0]["note"] = json!("unexpected");
        })
    })
    .await;
    assert_read_by_closure("unknown field in the compiler binding", |call, sources| {
        edited(call, sources, |value| {
            value["compiler_use"]["binding"]["unexpected"] = json!(true);
        })
    })
    .await;
}

#[tokio::test]
async fn a_bound_request_with_mismatched_fingerprints_is_cleaned_by_its_closure() {
    assert_read_by_closure("compiler binding digest", |call, sources| {
        edited(call, sources, |value| {
            value["compiler_use"]["binding"]["binding_digest"] = json!(hash(b"tampered"));
        })
    })
    .await;
    assert_read_by_closure("capabilities fingerprint", |call, sources| {
        edited(call, sources, |value| {
            value["compiler_use"]["binding"]["capabilities_digest"] =
                json!(hash(b"other capabilities"));
        })
    })
    .await;
    assert_read_by_closure("model request input digest", |call, sources| {
        edited(call, sources, |value| {
            value["request"]["input_digest"] = json!(hash(b"other input"));
        })
    })
    .await;
}

/// The other request schemas the cleanup reads go through the same reader: a
/// model request artifact or a registered execution whose closure cannot be read
/// is isolated the same way, under its own schema's diagnostic, instead of
/// rolling the step back, and is left as it is.
#[tokio::test]
async fn unreadable_requests_of_the_other_known_schemas_are_isolated_the_same_way() {
    const MODEL: CallKey = ("scope-0-model", "call-model");
    const REGISTERED: CallKey = ("scope-0-registered", "call-registered");
    let mut f = Fixture::new().await;
    // AG-107 fixture (independent review: plan §7.2.1 L831 rechecks a queued call
    // before dispatch, §11.3 L1112 isolates what is missing, and the AG-107 card):
    // the dispatch now refuses a call reserved without a source list whose request
    // closure cannot be read, so each call is settled with a readable request of
    // its schema that names no source, and the irregular body is then written over
    // it with the test-only `damage_request`, so the cleanup still meets billed
    // calls whose requests it cannot read. Replacement coverage for the refused
    // dispatch: `budget_ref_missing_fail_closed_v42.rs` (T2).
    settle(
        &f.store,
        MODEL,
        artifact_of(
            "rsia.model_request.artifact.v1",
            json!({"source_closure": [], "input": format!("{SECRET}-model")}).to_string(),
        ),
    )
    .await;
    damage_request(
        &f.path,
        MODEL,
        &json!({"source_closure": SOURCE, "input": format!("{SECRET}-model")}).to_string(),
    )
    .await;
    settle(
        &f.store,
        REGISTERED,
        artifact_of(
            REGISTERED_EXECUTION_REQUEST_SCHEMA,
            json!({"source_ids": [], "input": format!("{SECRET}-registered")}).to_string(),
        ),
    )
    .await;
    damage_request(
        &f.path,
        REGISTERED,
        &json!({"source_ids": [7], "input": format!("{SECRET}-registered")}).to_string(),
    )
    .await;
    settle(
        &f.store,
        GOOD,
        artifact(complete_bound(GOOD.1, &[SOURCE]).to_string()),
    )
    .await;
    let model = raw_request(&f.path, MODEL).await;
    let registered = raw_request(&f.path, REGISTERED).await;
    let money = accounting(&f.path).await;
    let job = f.begin().await;

    let done = f.drain(&job).await;
    assert_eq!(done.state, CleanupState::Failed, "{done:?}");
    assert_eq!(done.pending_nodes, 0, "{done:?}");
    assert_eq!(
        done.last_error.as_deref(),
        Some("unreadable_budget_request:rsia.registered_execution_request.v1")
    );
    assert_eq!(
        cleanup_events(&f.path, &job, "blocked_unknown_scope").await,
        vec![
            json!({"kind":"reservation","id":MODEL.1,
                "details":{"error":"unreadable_budget_request:rsia.model_request.artifact.v1"}}),
            json!({"kind":"reservation","id":REGISTERED.1,
                "details":{"error":"unreadable_budget_request:rsia.registered_execution_request.v1"}}),
        ]
    );
    assert_eq!(raw_request(&f.path, MODEL).await, model);
    assert_eq!(raw_request(&f.path, REGISTERED).await, registered);
    assert_eq!(content_redactions(&f.path, MODEL).await, 0);
    assert_eq!(content_redactions(&f.path, REGISTERED).await, 0);
    assert_request_redacted(&f.path, GOOD).await;
    assert!(!source_exists(&f.path).await);
    assert_eq!(accounting(&f.path).await, money);
}

/// Positive control (it passes at the baseline as well): complete, readable
/// bound requests are never taken for an unknown scope. One naming the revoked
/// run alone and one naming it with another run are redacted, one naming another
/// run only is left byte for byte, and the job completes.
#[tokio::test]
async fn complete_bound_requests_are_read_not_isolated() {
    let mut f = Fixture::new().await;
    settle(
        &f.store,
        GOOD,
        artifact(complete_bound(GOOD.1, &[SOURCE]).to_string()),
    )
    .await;
    settle(
        &f.store,
        BOTH,
        artifact(complete_bound(BOTH.1, &[OTHER, SOURCE]).to_string()),
    )
    .await;
    settle(
        &f.store,
        ELSEWHERE,
        artifact(complete_bound(ELSEWHERE.1, &[OTHER]).to_string()),
    )
    .await;
    let elsewhere = raw_request(&f.path, ELSEWHERE).await;
    let money = accounting(&f.path).await;
    let job = f.begin().await;

    let done = f.drain(&job).await;
    assert_eq!(done.state, CleanupState::Complete, "{done:?}");
    assert_eq!(done.last_error, None);
    assert!(
        cleanup_events(&f.path, &job, "blocked_unknown_scope")
            .await
            .is_empty()
    );
    assert_request_redacted(&f.path, GOOD).await;
    assert_request_redacted(&f.path, BOTH).await;
    assert_eq!(raw_request(&f.path, ELSEWHERE).await, elsewhere);
    assert_eq!(content_redactions(&f.path, ELSEWHERE).await, 0);
    assert!(!source_exists(&f.path).await);
    assert_eq!(accounting(&f.path).await, money);
}

/// The recheck that runs once the queue is empty reads the requests reserved
/// after `__budget_scan` passed, with the same reader. A late unreadable bound
/// request is isolated there too, instead of rolling back every recheck, and the
/// late complete request beside it is still redacted.
#[tokio::test]
async fn the_closure_recheck_isolates_a_late_unreadable_bound_request() {
    let mut f = Fixture::new().await;
    let job = f.begin().await;
    let scanned = f.step(&worker(), &job).await;
    assert_eq!(scanned.processed_nodes, 1, "{scanned:?}");
    assert_eq!(scanned.state, CleanupState::Running);
    // Reserved after the scan passed. A source list naming the revoked run would
    // be refused, so they have none: only the recheck can find them.
    reserve(
        &f.store,
        BAD,
        artifact(
            edited(BAD.1, &[SOURCE], |value| {
                value["request"]
                    .as_object_mut()
                    .unwrap()
                    .remove("source_closure");
            })
            .to_string(),
        ),
    )
    .await;
    reserve(
        &f.store,
        GOOD,
        artifact(complete_bound(GOOD.1, &[SOURCE]).to_string()),
    )
    .await;
    let bad_request = raw_request(&f.path, BAD).await;
    let money = accounting(&f.path).await;

    let done = f.drain(&job).await;
    assert_eq!(done.state, CleanupState::Failed, "{done:?}");
    assert_eq!(done.pending_nodes, 0, "{done:?}");
    assert_eq!(done.last_error.as_deref(), Some(UNREADABLE));
    assert_eq!(
        cleanup_events(&f.path, &job, "blocked_unknown_scope").await,
        vec![isolation_of(BAD)]
    );
    assert_eq!(raw_request(&f.path, BAD).await, bad_request);
    assert_eq!(content_redactions(&f.path, BAD).await, 0);
    assert_request_redacted(&f.path, GOOD).await;
    assert!(!source_exists(&f.path).await);
    assert_eq!(accounting(&f.path).await, money);
}

/// An isolated request is never trusted later: a Worker step does not retry the
/// failed job, and every explicit Admin retry rescans the request and isolates it
/// again while it stays unreadable. Once the bytes are repaired (a test-only
/// fixture write), the next Admin retry cleans it and the job completes, with
/// the earlier isolation records kept as history.
#[tokio::test]
async fn an_isolated_bound_request_is_rescanned_by_every_admin_retry_until_repaired() {
    let mut f = Fixture::new().await;
    let complete = complete_bound(BAD.1, &[SOURCE]).to_string();
    settle(&f.store, BAD, artifact(complete.clone())).await;
    damage_request(&f.path, BAD, &complete[..complete.len() / 2]).await;
    settle(
        &f.store,
        GOOD,
        artifact(complete_bound(GOOD.1, &[SOURCE]).to_string()),
    )
    .await;
    let money = accounting(&f.path).await;
    let job = f.begin().await;

    let failed = f.drain(&job).await;
    assert_eq!(failed.state, CleanupState::Failed, "{failed:?}");
    assert_eq!(failed.last_error.as_deref(), Some(UNREADABLE));
    let idle = f.step(&worker(), &job).await;
    assert_eq!(idle.state, CleanupState::Failed);
    assert_eq!(idle.processed_nodes, failed.processed_nodes);
    assert!(
        cleanup_events(&f.path, &job, "cleanup_retry_started")
            .await
            .is_empty()
    );

    let retried = f.step(&admin(), &job).await;
    assert_eq!(retried.state, CleanupState::Failed, "{retried:?}");
    assert_eq!(retried.last_error.as_deref(), Some(UNREADABLE));
    let retried = f.drain(&job).await;
    assert_eq!(retried.state, CleanupState::Failed, "{retried:?}");
    assert_eq!(
        cleanup_events(&f.path, &job, "cleanup_retry_started")
            .await
            .len(),
        1
    );
    assert_eq!(
        cleanup_events(&f.path, &job, "blocked_unknown_scope").await,
        vec![isolation_of(BAD), isolation_of(BAD)]
    );

    let mut c = connection(&f.path).await;
    sqlx::query(
        "UPDATE root_budget_calls SET request_artifact_body=?,request_artifact_digest=?
         WHERE namespace=? AND billing_scope=? AND call_id=?",
    )
    .bind(&complete)
    .bind(hash(complete.as_bytes()))
    .bind(NS)
    .bind(BAD.0)
    .bind(BAD.1)
    .execute(&mut c)
    .await
    .unwrap();
    c.close().await.unwrap();
    let restarted = f.step(&admin(), &job).await;
    assert_eq!(restarted.state, CleanupState::Running, "{restarted:?}");
    let done = f.drain(&job).await;
    assert_eq!(done.state, CleanupState::Complete, "{done:?}");
    assert_eq!(done.last_error, None);
    assert_request_redacted(&f.path, BAD).await;
    assert_request_redacted(&f.path, GOOD).await;
    assert_eq!(
        cleanup_events(&f.path, &job, "blocked_unknown_scope")
            .await
            .len(),
        2
    );
    assert_eq!(accounting(&f.path).await, money);
}
