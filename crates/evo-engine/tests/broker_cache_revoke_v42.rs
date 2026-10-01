//! AG-073: a completed fixture call stays charged, but its cached body cannot be
//! read after any request source or upstream source is revoked (§11/E08).
//! These tests use the real broker, SQLite ledger, and begin_revoke. Hand-written
//! dependency edges and corrupt records are explicitly gate fixtures; they do
//! not claim to authenticate evidence or demonstrate physical content removal.

use async_trait::async_trait;
use evo_core::evidence::Purpose;
use evo_core::optimization::{
    ModelInputPart, ModelInputRole, ModelRequest, ModelRequestContext, ModelStage,
};
use evo_core::skill_edit::EvidenceRef;
use evo_core::{Context, Error, Result, Role, fingerprint, hash};
use evo_engine::broker::{
    BrokerConfig, ModelTransport, PersistentModelBroker, TransportCompletion,
};
use evo_engine::model::{
    ModelExecutionProvenance, ModelExecutionReceipt, ModelPort, ModelRejectionKind, ModelResponse,
    RejectedDispatch,
};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallFence, BudgetCallRecord, BudgetCallReservation, BudgetCallState,
    BudgetExecutionProvenance, BudgetStage, ModelCallSettlementEvidence, RootBudgetAuthorization,
    UsageCharge,
};
use evo_storage::lifecycle::{CleanupState, LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const NS: &str = "ns-a";
const SCOPE: &str = "scope-1";
const A: &str = "run-a";
const B: &str = "run-b";
const U: &str = "run-upstream";
const UNAVAILABLE: &str = "model_response_source_unavailable";

fn context(namespace: &str, role: Role) -> Context {
    Context::new(namespace, "trusted-broker", role).unwrap()
}

fn request(id: &str, sources: &[&str]) -> ModelRequest {
    ModelRequest::build(
        ModelRequestContext {
            request_id: id.into(),
            namespace: NS.into(),
            purpose: Purpose::Development,
            stage: ModelStage::ReflectFailure,
            episode_id: "episode-1".into(),
            step: 1,
            attempt: 1,
            parent_skill_digest: hash(b"parent-v1"),
            bundle_digest: hash(b"bundle-v1"),
            source_closure: sources
                .iter()
                .map(|id| EvidenceRef {
                    id: (*id).into(),
                    digest: hash(id.as_bytes()),
                })
                .collect(),
            model_digest: hash(b"model-v1"),
            tools_digest: hash(b"tools-v1"),
            rules_digest: hash(b"rules-v1"),
            sampling_digest: hash(b"sampling-v1"),
            revoke_watermark: 0,
            max_suggestions: 4,
        },
        vec![ModelInputPart {
            role: ModelInputRole::User,
            label: "task".into(),
            content: "analyze the fixture development evidence".into(),
        }],
    )
    .unwrap()
}

fn config() -> BrokerConfig {
    BrokerConfig {
        billing_scope: SCOPE.into(),
        actor: "trusted-broker".into(),
        currency: "USD".into(),
        pricing_version: "price-v1".into(),
        max_cost_micros: 20,
        lease_seconds: 60,
    }
}

#[derive(Clone)]
struct FixtureTransport {
    calls: Arc<AtomicUsize>,
    fail: bool,
}

#[async_trait]
impl ModelTransport for FixtureTransport {
    fn provenance(&self) -> Option<ModelExecutionProvenance> {
        Some(ModelExecutionProvenance::Fixture)
    }

    async fn execute(
        &self,
        request: &ModelRequest,
        _dispatch_id: &str,
    ) -> Result<TransportCompletion> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(Error::Internal);
        }
        Ok(TransportCompletion {
            response_id: format!("response-{}", request.request_id),
            provider_request_id: format!("provider-{}", request.request_id),
            usage_record_id: format!("usage-{}", request.request_id),
            actual_model_digest: request.model_digest.clone(),
            output: format!("FIXTURE-CACHED-CONTENT-{}", request.request_id),
            actual_cost_micros: 10,
            currency: "USD".into(),
            pricing_version: "price-v1".into(),
        })
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    path: PathBuf,
    store: Store,
    transport: FixtureTransport,
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rsia.sqlite3");
        let store = Store::open(&path).await.unwrap();
        store
            .authorize_root_budget(
                &context(NS, Role::Admin),
                &RootBudgetAuthorization {
                    root_budget_id: "root-1".into(),
                    billing_scope: SCOPE.into(),
                    allowed_namespaces: vec![NS.into()],
                    currency: "USD".into(),
                    pricing_version: "price-v1".into(),
                    payment_subject: "payer-1".into(),
                    authorization_receipt_digest: hash(b"admin-authorization"),
                    per_call_cap_micros: 20,
                    total_limit_micros: 100,
                    created_at: 1,
                },
            )
            .await
            .unwrap();
        for id in [A, B, U] {
            put_run(&store, NS, id).await;
        }
        Self {
            _directory: directory,
            path,
            store,
            transport: FixtureTransport {
                calls: Arc::new(AtomicUsize::new(0)),
                fail: false,
            },
        }
    }

    fn broker(&self) -> PersistentModelBroker<FixtureTransport> {
        PersistentModelBroker::with_clock(
            self.store.clone(),
            self.transport.clone(),
            config(),
            Arc::new(|| 20),
        )
        .unwrap()
    }

    async fn call(&self, request: &ModelRequest) -> BudgetCallRecord {
        self.store
            .budget_call(&context(NS, Role::Host), SCOPE, &request.request_id)
            .await
            .unwrap()
            .unwrap()
    }

    async fn assert_accounting(&self, reserved: i64, spent: i64) {
        let root = self
            .store
            .root_budget(&context(NS, Role::Admin), SCOPE)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((root.reserved_micros, root.spent_micros), (reserved, spent));
        assert_eq!(self.transport.calls.load(Ordering::SeqCst), 1);
    }

    async fn complete(&self, request: &ModelRequest) -> ModelResponse {
        let broker = self.broker();
        let response = broker.dispatch(request.clone()).await.unwrap();
        assert!(matches!(response, ModelResponse::Completed { .. }));
        response.validate_against(request).unwrap();
        broker
            .verify_receipt(request, receipt(&response))
            .await
            .unwrap();
        self.assert_accounting(0, 10).await;
        assert_usable(&self.call(request).await, true);
        response
    }

    async fn refuse_unchanged(&self, request: &ModelRequest) {
        let before = sqlite_snapshot(&self.path);
        for _ in 0..2 {
            assert_unavailable(self.broker().dispatch(request.clone()).await);
        }
        assert_eq!(sqlite_snapshot(&self.path), before);
        self.assert_accounting(0, 10).await;
    }

    async fn reopen(&mut self) {
        self.store.close().await;
        self.store = Store::open(&self.path).await.unwrap();
    }
}

fn receipt(response: &ModelResponse) -> &ModelExecutionReceipt {
    match response {
        ModelResponse::Completed {
            execution_receipt, ..
        } => execution_receipt,
        ModelResponse::Rejected {
            dispatch: RejectedDispatch::Dispatched { receipt },
            ..
        } => receipt,
        _ => panic!("fixture response has no dispatched receipt"),
    }
}

fn assert_usable(call: &BudgetCallRecord, closed: bool) {
    assert_eq!(call.state, BudgetCallState::Finalized);
    assert_eq!(call.response_usable, Some(true));
    assert_eq!(call.execution_closed, closed);
    assert_eq!(
        call.response_artifact.as_ref().unwrap().schema_version,
        "rsia.model_response.artifact.v1"
    );
    assert!(call.dispatch_id.is_some());
    assert!(call.provider_request_id.is_some());
    assert!(call.usage_record_id.is_some());
}

fn assert_unavailable(result: Result<ModelResponse>) {
    match result {
        Err(Error::Conflict(code)) => assert_eq!(code, UNAVAILABLE),
        other => panic!("expected Conflict({UNAVAILABLE}), got {other:?}"),
    }
}

fn assert_same_response(actual: &ModelResponse, expected: &ModelResponse) {
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

/// Exact rows of every table, including ledger/usage/events, references, edges,
/// tombstones, cleanup queues, audit, idempotency and FTS shadow tables. Read-only
/// Python SQLite is already used by engine tests; no new dependency is needed.
fn sqlite_snapshot(path: &Path) -> Value {
    let script = r#"import json,sqlite3,sys
from pathlib import Path
con=sqlite3.connect(Path(sys.argv[1]).resolve().as_uri()+'?mode=ro',uri=True)
con.execute('PRAGMA query_only=ON')
con.execute('BEGIN')
def quoted(name):
    return '"'+name.replace('"','""')+'"'
result={}
for (name,) in con.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name").fetchall():
    columns=[row[1] for row in con.execute('PRAGMA table_info('+quoted(name)+')')]
    sql='SELECT '+','.join('quote('+quoted(column)+')' for column in columns)+' FROM '+quoted(name)
    result[name]={'columns':columns,'rows':sorted([list(row) for row in con.execute(sql)])}
con.rollback()
con.close()
print(json.dumps(result,sort_keys=True))
"#;
    let output = Command::new("python3")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .arg("-c")
        .arg(script)
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let snapshot: Value = serde_json::from_slice(&output.stdout).unwrap();
    for table in [
        "objects",
        "dependencies",
        "audit",
        "root_budget_calls",
        "root_budget_events",
        "revoke_cleanup_jobs",
    ] {
        assert!(snapshot.get(table).is_some(), "snapshot omitted {table}");
    }
    snapshot
}

async fn raw_put(store: &Store, namespace: &str, kind: &str, id: &str, body: &Value) {
    let admin = context(namespace, Role::Admin);
    let mut session = store.session().await.unwrap();
    session
        .put(&admin, kind, id, admin.actor(), body)
        .await
        .unwrap();
    session.commit().await.unwrap();
}

async fn put_run(store: &Store, namespace: &str, id: &str) {
    raw_put(
        store,
        namespace,
        "run",
        id,
        &json!({
            "id":id, "schema_version":"rsia.optimization.source.v1", "body":"fixture-run-body"
        }),
    )
    .await;
}

async fn begin(store: &Store, namespace: &str, kind: &str, id: &str) -> String {
    let status = LifecycleStore::begin_revoke(
        &context(namespace, Role::Admin),
        store,
        TypedObjectRef {
            kind: kind.into(),
            id: id.into(),
        },
        "fixture source revoked",
        30,
    )
    .await
    .unwrap();
    assert_eq!(status.state, CleanupState::Pending);
    assert_eq!(status.processed_nodes, 0);
    status.job_id
}

/// Gate fixture: typed edges created explicitly rather than claiming that a
/// separate evidence producer generated/authenticated this dependency graph.
async fn gate_edges(store: &Store, namespace: &str, edges: &[(&str, &str, &str, &str)]) {
    let mut session = store.session().await.unwrap();
    for (src_kind, src_id, dst_kind, dst_id) in edges {
        session
            .put_edge(
                &context(namespace, Role::Admin),
                src_kind,
                src_id,
                dst_kind,
                dst_id,
            )
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
}

#[tokio::test]
async fn completed_cache_refuses_each_direct_source_before_cleanup_without_rebilling() {
    for revoked in [A, B] {
        let fixture = Fixture::new().await;
        let request = request("direct", &[A, B]);
        let response = fixture.complete(&request).await;
        begin(&fixture.store, NS, "run", revoked).await;
        let call = fixture.call(&request).await;
        assert_usable(&call, true);
        eprintln!(
            "direct {revoked}: response_usable=true, response_artifact=rsia.model_response.artifact.v1, execution_closed=true"
        );
        fixture.refuse_unchanged(&request).await;
        // A refusal to reuse content does not rewrite or invalidate history.
        fixture
            .broker()
            .verify_receipt(&request, receipt(&response))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn completed_cache_refuses_nonprimary_upstream_after_store_reopen() {
    let mut fixture = Fixture::new().await;
    gate_edges(&fixture.store, NS, &[("run", B, "run", U)]).await;
    let request = request("nonprimary-upstream", &[A, B]);
    let response = fixture.complete(&request).await;
    begin(&fixture.store, NS, "run", U).await;
    assert_usable(&fixture.call(&request).await, true);
    eprintln!(
        "nonprimary upstream: response_usable=true, response_artifact=rsia.model_response.artifact.v1, execution_closed=true"
    );
    fixture.refuse_unchanged(&request).await;
    let before = sqlite_snapshot(&fixture.path);
    fixture.reopen().await;
    assert_eq!(sqlite_snapshot(&fixture.path), before);
    fixture.refuse_unchanged(&request).await;
    fixture
        .broker()
        .verify_receipt(&request, receipt(&response))
        .await
        .unwrap();
}

#[tokio::test]
async fn completed_cache_refuses_third_layer_ancestor_through_cycle_and_other_kinds() {
    let fixture = Fixture::new().await;
    gate_edges(
        &fixture.store,
        NS,
        &[
            ("run", B, "artifact", "middle-1"),
            ("artifact", "middle-1", "candidate", "bridge"),
            ("candidate", "bridge", "artifact", "middle-2"),
            ("artifact", "middle-2", "run", U),
            ("artifact", "middle-2", "run", B),
        ],
    )
    .await;
    let request = request("third-layer-cycle", &[A, B]);
    fixture.complete(&request).await;
    begin(&fixture.store, NS, "run", U).await;
    fixture.refuse_unchanged(&request).await;
}

#[tokio::test]
async fn unrelated_watermark_increase_does_not_refuse_live_cached_response() {
    let fixture = Fixture::new().await;
    let request = request("unrelated-watermark", &[A, B]);
    let response = fixture.complete(&request).await;
    begin(&fixture.store, NS, "run", U).await;
    let before = sqlite_snapshot(&fixture.path);
    assert!(before["revoke_watermark"]["rows"].as_array().unwrap().len() == 1);
    assert_eq!(request.revoke_watermark, 0);
    assert_same_response(
        &fixture.broker().dispatch(request).await.unwrap(),
        &response,
    );
    assert_eq!(sqlite_snapshot(&fixture.path), before);
    fixture.assert_accounting(0, 10).await;
}

#[tokio::test]
async fn another_namespace_tombstone_and_edges_do_not_refuse_live_cache() {
    let fixture = Fixture::new().await;
    let request = request("namespace-isolation", &[A, B]);
    let response = fixture.complete(&request).await;
    put_run(&fixture.store, "ns-other", B).await;
    put_run(&fixture.store, "ns-other", U).await;
    begin(&fixture.store, "ns-other", "run", B).await;
    begin(&fixture.store, "ns-other", "run", U).await;
    gate_edges(&fixture.store, "ns-other", &[("run", B, "run", U)]).await;
    // An edge in NS has no NS tombstone to match; the other namespace is private.
    gate_edges(&fixture.store, NS, &[("run", B, "run", U)]).await;
    let before = sqlite_snapshot(&fixture.path);
    assert_same_response(
        &fixture.broker().dispatch(request).await.unwrap(),
        &response,
    );
    assert_eq!(sqlite_snapshot(&fixture.path), before);
    fixture.assert_accounting(0, 10).await;
}

#[tokio::test]
async fn same_id_artifact_revocation_does_not_refuse_live_run_cache() {
    let fixture = Fixture::new().await;
    let request = request("kind-isolation", &[A, B]);
    let response = fixture.complete(&request).await;
    // Strict E16 import_source shape accepted by the actual begin_revoke API.
    raw_put(
        &fixture.store,
        NS,
        "artifact",
        B,
        &json!({
            "schema_version":"rsia.e16.import_source.v1", "id":B, "namespace":NS,
            "owner_actor":"trusted-broker", "request_key":"artifact-source-b",
            "input_digest":hash(b"input-b"), "created_at":1, "updated_at":1,
            "source_refs":[], "revoke_watermark":0,
            "payload":{"raw_blob_digest":null,"status":"prepared"},
        }),
    )
    .await;
    begin(&fixture.store, NS, "artifact", B).await;
    let before = sqlite_snapshot(&fixture.path);
    assert_same_response(
        &fixture.broker().dispatch(request).await.unwrap(),
        &response,
    );
    assert_eq!(sqlite_snapshot(&fixture.path), before);
    fixture.assert_accounting(0, 10).await;
}

#[tokio::test]
async fn live_cycle_and_unstored_source_keep_existing_cache_behavior() {
    for sources in [vec![A, B], vec!["unstored-run"]] {
        let fixture = Fixture::new().await;
        gate_edges(
            &fixture.store,
            NS,
            &[
                ("run", B, "artifact", "cycle-artifact"),
                ("artifact", "cycle-artifact", "candidate", "cycle-candidate"),
                ("candidate", "cycle-candidate", "run", B),
            ],
        )
        .await;
        let request = request("live-or-unstored", &sources);
        let response = fixture.complete(&request).await;
        let before = sqlite_snapshot(&fixture.path);
        assert_same_response(
            &fixture.broker().dispatch(request).await.unwrap(),
            &response,
        );
        assert_eq!(sqlite_snapshot(&fixture.path), before);
        fixture.assert_accounting(0, 10).await;
    }
}

#[tokio::test]
async fn malformed_direct_and_upstream_tombstones_fail_closed() {
    for upstream in [false, true] {
        for unknown_kind in [false, true] {
            let fixture = Fixture::new().await;
            gate_edges(&fixture.store, NS, &[("run", B, "run", U)]).await;
            let request = request("malformed-tombstone", &[A, B]);
            fixture.complete(&request).await;
            let id = if upstream { U } else { B };
            // Gate fixtures: begin_revoke never writes either malformed shape.
            let body = if unknown_kind {
                json!({
                    "id":id,"schema_version":"rsia.revoke_tombstone.v1",
                    "source_kind":"candidate","reason":"fixture-corrupt-kind",
                    "watermark_seq":1,"watermark_digest":hash(b"fixture-watermark"),"created_at":30
                })
            } else {
                json!({"id":id,"source_kind":"run"})
            };
            raw_put(&fixture.store, NS, "tombstone", id, &body).await;
            fixture.refuse_unchanged(&request).await;
        }
    }
}

#[tokio::test]
async fn redacted_upstream_artifact_refuses_cached_body() {
    let fixture = Fixture::new().await;
    gate_edges(
        &fixture.store,
        NS,
        &[("run", B, "artifact", "redacted-ancestor")],
    )
    .await;
    let request = request("redacted-ancestor", &[A, B]);
    fixture.complete(&request).await;
    // Gate fixture of a body the cleanup leaves; no false claim of cleanup here.
    raw_put(
        &fixture.store,
        NS,
        "artifact",
        "redacted-ancestor",
        &json!({
            "id":"redacted-ancestor","schema_version":"rsia.redacted.v1"
        }),
    )
    .await;
    fixture.refuse_unchanged(&request).await;
}

#[tokio::test]
async fn upstream_closure_at_bound_is_usable_but_over_bound_is_refused() {
    let fixture = Fixture::new().await;
    let request = request("closure-bound", &[A, B]);
    let response = fixture.complete(&request).await;
    // Gate fixture: 9,998 distinct ancestors plus the two request roots = 10,000.
    let mut session = fixture.store.session().await.unwrap();
    for index in 0..9_998 {
        session
            .put_edge(
                &context(NS, Role::Admin),
                "run",
                B,
                "artifact",
                &format!("bound-{index}"),
            )
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    assert_same_response(
        &fixture.broker().dispatch(request.clone()).await.unwrap(),
        &response,
    );
    gate_edges(&fixture.store, NS, &[("run", B, "artifact", "bound-extra")]).await;
    let before = sqlite_snapshot(&fixture.path);
    match fixture.broker().dispatch(request).await {
        Err(Error::Conflict(reason)) => assert_eq!(
            reason,
            "upstream dependency closure exceeds 10000 nodes; it is refused, not truncated"
        ),
        other => panic!("expected bounded-closure refusal, got {other:?}"),
    }
    assert_eq!(sqlite_snapshot(&fixture.path), before);
    fixture.assert_accounting(0, 10).await;
}

#[derive(Clone, Copy)]
enum RefFixture {
    Missing,
    Empty,
    Corrupt,
}

async fn edit_reference(fixture: &Fixture, request: &ModelRequest, state: RefFixture) {
    let digest = fingerprint(&("rsia.budget_call_ref.v1", NS, SCOPE, &request.request_id)).unwrap();
    let id = format!("budget-ref-{}", &digest[..32]);
    let ctx = context(NS, Role::Admin);
    let mut session = fixture.store.session().await.unwrap();
    let mut body: Value = session.need(&ctx, "artifact", &id).await.unwrap();
    assert_eq!(body["source_ids"], json!([A, B]));
    // Gate fixtures for lost, empty and undecodable persistent references. The
    // authenticated request still contains both sources; existing edges remain.
    match state {
        RefFixture::Missing => session.delete(&ctx, "artifact", &id).await.unwrap(),
        RefFixture::Empty | RefFixture::Corrupt => {
            body["source_ids"] = match state {
                RefFixture::Empty => json!([]),
                RefFixture::Corrupt => json!("not-a-source-list"),
                RefFixture::Missing => unreachable!(),
            };
            session
                .put(&ctx, "artifact", &id, ctx.actor(), &body)
                .await
                .unwrap();
        }
    }
    session.commit().await.unwrap();
}

async fn reference_does_not_shrink_gate(state: RefFixture) {
    let fixture = Fixture::new().await;
    let request = request("reference-fixture", &[A, B]);
    let response = fixture.complete(&request).await;
    edit_reference(&fixture, &request, state).await;
    let before_live = sqlite_snapshot(&fixture.path);
    assert_same_response(
        &fixture.broker().dispatch(request.clone()).await.unwrap(),
        &response,
    );
    assert_eq!(sqlite_snapshot(&fixture.path), before_live);
    begin(&fixture.store, NS, "run", B).await;
    fixture.refuse_unchanged(&request).await;
    fixture
        .broker()
        .verify_receipt(&request, receipt(&response))
        .await
        .unwrap();
}

#[tokio::test]
async fn missing_budget_reference_does_not_shrink_the_request_gate() {
    reference_does_not_shrink_gate(RefFixture::Missing).await;
}

#[tokio::test]
async fn empty_budget_reference_does_not_shrink_the_request_gate() {
    reference_does_not_shrink_gate(RefFixture::Empty).await;
}

#[tokio::test]
async fn corrupt_budget_reference_does_not_shrink_the_request_gate() {
    reference_does_not_shrink_gate(RefFixture::Corrupt).await;
}

#[tokio::test]
async fn deleting_nonprimary_request_source_cannot_reuse_the_completed_call() {
    let fixture = Fixture::new().await;
    let original = request("request-binding", &[A, B]);
    let response = fixture.complete(&original).await;
    begin(&fixture.store, NS, "run", B).await;
    let before = sqlite_snapshot(&fixture.path);
    let mut stale_digest = original.clone();
    stale_digest.source_closure.pop();
    for changed in [stale_digest, request("request-binding", &[A])] {
        match fixture.broker().dispatch(changed.clone()).await.unwrap() {
            ModelResponse::Rejected {
                kind,
                reason,
                dispatch,
                ..
            } => {
                assert_eq!(kind, ModelRejectionKind::InvalidRequest);
                assert_eq!(dispatch, RejectedDispatch::NotDispatched);
                let expected = if changed.cache_key_digest == original.cache_key_digest {
                    Error::Conflict("model request digest mismatch".into()).to_string()
                } else {
                    "broker_pre_dispatch_v1.call_id_reused".into()
                };
                assert_eq!(reason, expected);
            }
            other => panic!("changed source closure reused the cache: {other:?}"),
        }
    }
    assert_eq!(sqlite_snapshot(&fixture.path), before);
    assert_eq!(
        fixture.call(&original).await.dispatch_id.as_deref(),
        Some(receipt(&response).dispatch_id.as_str())
    );
    fixture
        .broker()
        .verify_receipt(&original, receipt(&response))
        .await
        .unwrap();
    fixture.assert_accounting(0, 10).await;
}

/// Real reserve -> begin dispatch -> fixture transport -> atomic settlement.
/// No close follows, reproducing recovery after settlement without changing an
/// execution flag by SQL. `with_sources=false` is the existing legal legacy API.
async fn settle_open(
    fixture: &Fixture,
    request: &ModelRequest,
    with_sources: bool,
    blocked: bool,
) -> ModelResponse {
    let ctx = context(NS, Role::Host);
    let reservation = BudgetCallReservation {
        billing_scope: SCOPE.into(),
        call_id: request.request_id.clone(),
        dispatch_group_id: request.episode_id.clone(),
        stage: BudgetStage::Reflection,
        actual_input_digest: request.cache_key_digest.clone(),
        request_artifact: Some(
            BudgetArtifact::from_serializable("rsia.model_request.artifact.v1", request).unwrap(),
        ),
        max_cost_micros: 20,
        lease_token: "recovery-lease".into(),
        lease_until: 100,
        now: 10,
    };
    let reserved = if with_sources {
        fixture
            .store
            .reserve_budget_call_with_sources(
                &ctx,
                &reservation,
                &request
                    .source_closure
                    .iter()
                    .map(|source| source.id.clone())
                    .collect::<Vec<_>>(),
            )
            .await
            .unwrap()
    } else {
        fixture
            .store
            .reserve_budget_call(&ctx, &reservation)
            .await
            .unwrap()
    };
    let fence = BudgetCallFence {
        billing_scope: SCOPE.into(),
        call_id: request.request_id.clone(),
        actual_input_digest: request.cache_key_digest.clone(),
        lease_token: reserved.lease_token,
        lease_epoch: reserved.lease_epoch,
        now: 10,
    };
    let dispatched = fixture
        .store
        .begin_budget_dispatch(&ctx, &fence)
        .await
        .unwrap();
    assert!(dispatched.new_dispatch);
    let dispatch_id = dispatched.call.dispatch_id.unwrap();
    let completion = fixture
        .transport
        .execute(request, &dispatch_id)
        .await
        .unwrap();
    let receipt = ModelExecutionReceipt {
        call_id: request.request_id.clone(),
        dispatch_id,
        root_budget_id: "root-1".into(),
        provider_request_id: completion.provider_request_id.clone(),
        usage_record_id: completion.usage_record_id.clone(),
        provenance: ModelExecutionProvenance::Fixture,
    };
    let output_digest = hash(completion.output.as_bytes());
    let completed = ModelResponse::Completed {
        request_id: request.request_id.clone(),
        response_id: completion.response_id.clone(),
        actual_model_digest: completion.actual_model_digest.clone(),
        input_digest: request.input_digest.clone(),
        output: completion.output.clone(),
        output_digest: output_digest.clone(),
        execution_receipt: receipt.clone(),
    };
    let rejected = ModelResponse::Rejected {
        request_id: request.request_id.clone(),
        kind: ModelRejectionKind::CancelledAfterDispatch,
        reason: "fixture persisted dispatched block".into(),
        dispatch: RejectedDispatch::Dispatched { receipt },
    };
    let settlement = fixture
        .store
        .settle_model_budget_call(
            &ctx,
            &fence,
            &UsageCharge {
                amount_micros: 10,
                currency: "USD".into(),
                pricing_version: "price-v1".into(),
                provider_request_id: completion.provider_request_id.clone(),
                usage_record_id: completion.usage_record_id.clone(),
                output_digest,
            },
            &ModelCallSettlementEvidence {
                provenance: BudgetExecutionProvenance::Fixture,
                actual_model_digest: Some(completion.actual_model_digest.clone()),
                transport_artifact: BudgetArtifact::from_serializable(
                    "rsia.model_transport.artifact.v1",
                    &completion,
                )
                .unwrap(),
                usable_response: Some(
                    BudgetArtifact::from_serializable(
                        "rsia.model_response.artifact.v1",
                        &completed,
                    )
                    .unwrap(),
                ),
                blocked_response: BudgetArtifact::from_serializable(
                    "rsia.model_response.artifact.v1",
                    &rejected,
                )
                .unwrap(),
                forced_block_reason: blocked.then(|| "fixture_block".into()),
            },
        )
        .await
        .unwrap();
    assert!(!settlement.call.execution_closed);
    fixture.assert_accounting(0, 10).await;
    if blocked {
        rejected
    } else {
        assert_usable(&settlement.call, false);
        completed
    }
}

#[tokio::test]
async fn revoked_settled_open_execution_refuses_without_closing_or_any_table_write() {
    let mut fixture = Fixture::new().await;
    let request = request("revoked-open-execution", &[A, B]);
    settle_open(&fixture, &request, true, false).await;
    begin(&fixture.store, NS, "run", B).await;
    assert_usable(&fixture.call(&request).await, false);
    fixture.refuse_unchanged(&request).await;
    fixture.reopen().await;
    fixture.refuse_unchanged(&request).await;
    assert_usable(&fixture.call(&request).await, false);
}

#[tokio::test]
async fn live_settled_open_execution_closes_and_replays_without_new_dispatch() {
    for with_sources in [true, false] {
        let mut fixture = Fixture::new().await;
        let request = request("live-open-execution", &[A, B]);
        let expected = settle_open(&fixture, &request, with_sources, false).await;
        let before = sqlite_snapshot(&fixture.path);
        fixture.reopen().await;
        assert_eq!(sqlite_snapshot(&fixture.path), before);
        let broker = fixture.broker();
        let actual = broker.dispatch(request.clone()).await.unwrap();
        assert_same_response(&actual, &expected);
        assert_usable(&fixture.call(&request).await, true);
        broker
            .verify_receipt(&request, receipt(&actual))
            .await
            .unwrap();
        fixture.assert_accounting(0, 10).await;
        let after = sqlite_snapshot(&fixture.path);
        assert_eq!(
            after["objects"], before["objects"],
            "recovery must not repair/create a budget reference"
        );
        assert_eq!(after["dependencies"], before["dependencies"]);
        assert_same_response(&broker.dispatch(request).await.unwrap(), &expected);
        assert_eq!(sqlite_snapshot(&fixture.path), after);
    }
}

#[tokio::test]
async fn revoked_legacy_settlement_without_reference_still_uses_request_sources() {
    let fixture = Fixture::new().await;
    let request = request("legacy-revoked-open", &[A, B]);
    settle_open(&fixture, &request, false, false).await;
    begin(&fixture.store, NS, "run", B).await;
    fixture.refuse_unchanged(&request).await;
    assert_usable(&fixture.call(&request).await, false);
}

#[tokio::test]
async fn persisted_dispatched_rejection_recovers_after_revoke_with_original_receipt() {
    let fixture = Fixture::new().await;
    let request = request("rejected-open-execution", &[A, B]);
    let expected = settle_open(&fixture, &request, true, true).await;
    begin(&fixture.store, NS, "run", B).await;
    let broker = fixture.broker();
    let actual = broker.dispatch(request.clone()).await.unwrap();
    assert_same_response(&actual, &expected);
    let call = fixture.call(&request).await;
    assert_eq!(call.response_usable, Some(false));
    assert!(call.execution_closed);
    broker
        .verify_receipt(&request, receipt(&actual))
        .await
        .unwrap();
    let before = sqlite_snapshot(&fixture.path);
    assert_same_response(&broker.dispatch(request).await.unwrap(), &expected);
    assert_eq!(sqlite_snapshot(&fixture.path), before);
    fixture.assert_accounting(0, 10).await;
}

#[tokio::test]
async fn uncertain_dispatched_retry_after_revoke_keeps_unknown_charge_and_no_replay() {
    let mut fixture = Fixture::new().await;
    fixture.transport.fail = true;
    let request = request("uncertain-after-revoke", &[A, B]);
    let expected = fixture.broker().dispatch(request.clone()).await.unwrap();
    assert!(matches!(expected, ModelResponse::Uncertain { .. }));
    begin(&fixture.store, NS, "run", B).await;
    let before = sqlite_snapshot(&fixture.path);
    fixture.reopen().await;
    assert_same_response(
        &fixture.broker().dispatch(request.clone()).await.unwrap(),
        &expected,
    );
    assert_eq!(sqlite_snapshot(&fixture.path), before);
    let call = fixture.call(&request).await;
    assert_eq!(call.state, BudgetCallState::Uncertain);
    assert!(call.dispatch_id.is_some());
    assert!(call.response_artifact.is_none());
    assert!(!call.execution_closed);
    fixture.assert_accounting(20, 0).await;
}

#[tokio::test]
async fn actual_cleanup_redacted_cache_keeps_existing_dispatched_rejection() {
    let fixture = Fixture::new().await;
    let request = request("redacted-cache", &[A, B]);
    let completed = fixture.complete(&request).await;
    let job_id = begin(&fixture.store, NS, "run", B).await;
    let mut finished = false;
    for now in 31..200 {
        let status = LifecycleStore::cleanup_step(
            &context(NS, Role::Admin),
            &fixture.store,
            &job_id,
            8,
            now,
        )
        .await
        .unwrap();
        if status.state == CleanupState::Complete {
            finished = true;
            break;
        }
        assert_ne!(status.state, CleanupState::Failed);
    }
    assert!(finished);
    let call = fixture.call(&request).await;
    assert_eq!(
        call.response_artifact.as_ref().unwrap().schema_version,
        "rsia.redacted.v1"
    );
    let before = sqlite_snapshot(&fixture.path);
    let broker = fixture.broker();
    let actual = broker.dispatch(request.clone()).await.unwrap();
    match &actual {
        ModelResponse::Rejected { kind, dispatch, .. } => {
            assert_eq!(*kind, ModelRejectionKind::CancelledAfterDispatch);
            assert_eq!(
                *dispatch,
                RejectedDispatch::Dispatched {
                    receipt: receipt(&completed).clone()
                }
            );
        }
        other => panic!("redacted cache changed its recovery response: {other:?}"),
    }
    broker
        .verify_receipt(&request, receipt(&completed))
        .await
        .unwrap();
    assert_same_response(&broker.dispatch(request).await.unwrap(), &actual);
    assert_eq!(sqlite_snapshot(&fixture.path), before);
    fixture.assert_accounting(0, 10).await;
}
