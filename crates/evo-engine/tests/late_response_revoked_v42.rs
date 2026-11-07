//! AG-043 (E08/§11, E04) at the engine layer: a model response that arrives after
//! one of its sources was revoked is billed, is not returned as a completed
//! response, and leaves no plaintext in the database.
//!
//! Plan §11 says that when a source is deleted while a model request is in
//! flight, the late output can form no usable candidate while its real cost is
//! still booked, and that the late result of a call that was already dispatched
//! cannot regain validity while its cost is still reconciled. The revocation
//! cleanup only redacts what a call holds when it reads the call, and it never
//! reads a call whose request it has already redacted, so the response of a call
//! that was in flight when the cleanup ran used to be stored in plaintext (the
//! transport body carries the model output) and as usable.
//!
//! These tests drive the production caller of the ledger, the persistent model
//! broker, with a transport whose provider await revokes the source (the revocation
//! lands after the dispatch and before the response, the window of the plan) and,
//! in one variant, runs the whole cleanup before the response arrives. The
//! transport counts its calls (the provider is never real) and the store is the
//! real SQLite one. The blocked semantics the broker already has are reused: the
//! result is `Rejected { CancelledAfterDispatch }` with the dispatch receipt, the
//! same a response that arrives after a group stop or an expired lease gets.
//!
//! "No plaintext" is read two ways: the rows of the budget ledger (`root_budget%`
//! tables, through `read_control_plane_facts`) and the raw bytes of the database
//! file and its write-ahead log, which cover every table and the pages that were
//! written and then overwritten. The control (a response over a live source) shows
//! both scans do see a stored output.
//!
//! Fixtures are copied from `tests/broker.rs` and `tests/budget_revoke_gate_v42.rs`;
//! the originals are untouched.

use async_trait::async_trait;
use evo_core::evidence::Purpose;
use evo_core::optimization::{
    ModelInputPart, ModelInputRole, ModelRequest, ModelRequestContext, ModelStage,
};
use evo_core::skill_edit::EvidenceRef;
use evo_core::{Context, Result, Role, hash};
use evo_engine::broker::{
    BrokerConfig, ModelTransport, PersistentModelBroker, TransportCompletion,
};
use evo_engine::lifecycle::LifecycleCoordinator;
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
use evo_storage::lifecycle::{CleanupState, CleanupStatus, read_control_plane_facts};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const NAMESPACE: &str = "ns-a";
const SCOPE: &str = "scope-1";
/// The run the tests revoke, and one that stays live.
const RUN: &str = "run-r";
const LIVE: &str = "run-live";
const REDACTED_SCHEMA: &str = "rsia.redacted.v1";

/// What the provider does to the source of the request while it is in flight.
#[derive(Clone)]
enum DuringCall {
    Nothing,
    /// Commits the revocation of `RUN`; no cleanup step runs.
    Revoke(Store),
    /// Commits the revocation of `RUN` and runs its cleanup to `Complete`.
    RevokeAndClean(Store),
}

impl std::fmt::Debug for DuringCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Nothing => "Nothing",
            Self::Revoke(_) => "Revoke",
            Self::RevokeAndClean(_) => "RevokeAndClean",
        })
    }
}

#[derive(Clone)]
struct LateTransport {
    calls: Arc<AtomicUsize>,
    during_call: DuringCall,
    /// The model the provider reports; `None` is the one the request froze.
    actual_model: Option<String>,
}

impl LateTransport {
    fn new(during_call: DuringCall) -> (Self, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Self {
                calls: calls.clone(),
                during_call,
                actual_model: None,
            },
            calls,
        )
    }
}

/// The output of the provider for `request_id`: plaintext the revocation must not
/// leave in the database, and unique per request.
fn output_of(request_id: &str) -> String {
    format!("MODEL-OUTPUT-MARKER-{request_id}-5b3e90")
}

#[async_trait]
impl ModelTransport for LateTransport {
    fn provenance(&self) -> Option<ModelExecutionProvenance> {
        Some(ModelExecutionProvenance::Fixture)
    }

    async fn execute(
        &self,
        request: &ModelRequest,
        _dispatch_id: &str,
    ) -> Result<TransportCompletion> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let admin = Context::new(NAMESPACE, "admin", Role::Admin)?;
        match &self.during_call {
            DuringCall::Nothing => {}
            DuringCall::Revoke(store) => {
                LifecycleCoordinator::revoke_source(&admin, store, RUN, "privacy", 12).await?;
            }
            DuringCall::RevokeAndClean(store) => {
                let mut status =
                    LifecycleCoordinator::revoke_source(&admin, store, RUN, "privacy", 12).await?;
                for now in 13..400 {
                    if status.state == CleanupState::Complete {
                        break;
                    }
                    status = LifecycleCoordinator::continue_cleanup(
                        &admin,
                        store,
                        &status.job_id,
                        8,
                        now,
                    )
                    .await?;
                }
                assert_eq!(status.state, CleanupState::Complete, "{status:?}");
            }
        }
        Ok(TransportCompletion {
            response_id: format!("response-{}", request.request_id),
            provider_request_id: format!("provider-{}", request.request_id),
            usage_record_id: format!("usage-{}", request.request_id),
            actual_model_digest: self
                .actual_model
                .clone()
                .unwrap_or_else(|| request.model_digest.clone()),
            output: output_of(&request.request_id),
            actual_cost_micros: 10,
            currency: "USD".into(),
            pricing_version: "price-v1".into(),
        })
    }
}

fn model_request(id: &str, sources: &[&str]) -> ModelRequest {
    ModelRequest::build(
        ModelRequestContext {
            request_id: id.into(),
            namespace: NAMESPACE.into(),
            purpose: Purpose::Development,
            stage: ModelStage::ReflectFailure,
            episode_id: "episode-1".into(),
            step: 1,
            attempt: 1,
            parent_skill_digest: hash(b"parent-v1"),
            bundle_digest: hash(b"bundle-v1"),
            source_closure: sources
                .iter()
                .map(|source| EvidenceRef {
                    id: (*source).into(),
                    digest: hash(source.as_bytes()),
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
            content: "analyze the development evidence".into(),
        }],
    )
    .unwrap()
}

fn broker_config() -> BrokerConfig {
    BrokerConfig {
        billing_scope: SCOPE.into(),
        actor: "trusted-broker".into(),
        currency: "USD".into(),
        pricing_version: "price-v1".into(),
        max_cost_micros: 20,
        lease_seconds: 60,
    }
}

fn admin() -> Context {
    Context::new(NAMESPACE, "admin", Role::Admin).unwrap()
}

fn host() -> Context {
    Context::new(NAMESPACE, "trusted-broker", Role::Host).unwrap()
}

/// A store with the root authorized and the runs `RUN` and `LIVE` stored.
async fn broker_store() -> (tempfile::TempDir, PathBuf, Store) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rsia.sqlite3");
    let store = Store::open(&path).await.unwrap();
    store
        .authorize_root_budget(
            &admin(),
            &RootBudgetAuthorization {
                root_budget_id: "root-1".into(),
                billing_scope: SCOPE.into(),
                allowed_namespaces: vec![NAMESPACE.into()],
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
    let host = host();
    let mut session = store.session().await.unwrap();
    for id in [RUN, LIVE] {
        session
            .put(
                &host,
                "run",
                id,
                host.actor(),
                &serde_json::json!({
                    "id": id,
                    "schema_version": "rsia.optimization.source.v1",
                    "body": "run-body",
                }),
            )
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    (dir, path, store)
}

fn broker_over(store: &Store, transport: LateTransport) -> PersistentModelBroker<LateTransport> {
    PersistentModelBroker::with_clock(store.clone(), transport, broker_config(), Arc::new(|| 10))
        .unwrap()
}

async fn call_of(store: &Store, call_id: &str) -> BudgetCallRecord {
    store
        .budget_call(&host(), SCOPE, call_id)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("{call_id} has no budget row"))
}

async fn micros(store: &Store) -> (i64, i64) {
    let root = store.root_budget(&admin(), SCOPE).await.unwrap().unwrap();
    (root.reserved_micros, root.spent_micros)
}

/// Steps the cleanup job of `status` until it is `Complete`.
async fn complete_cleanup(store: &Store, mut status: CleanupStatus) {
    for now in 21..400 {
        if status.state == CleanupState::Complete {
            return;
        }
        status = LifecycleCoordinator::continue_cleanup(&admin(), store, &status.job_id, 8, now)
            .await
            .unwrap();
    }
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
}

/// The job of the revocation of `RUN`, whatever the transport did: `begin_revoke`
/// is idempotent, so this is the status of the job the provider await started.
async fn revocation_job(store: &Store) -> CleanupStatus {
    LifecycleCoordinator::revoke_source(&admin(), store, RUN, "privacy", 12)
        .await
        .unwrap()
}

/// Where `needle` can be read: in a row of a budget ledger table, or in the raw
/// bytes of the database file or its write-ahead log (every table, and the pages
/// that were written and then overwritten). Empty when it is nowhere.
async fn found_in(path: &Path, needle: &str) -> Vec<String> {
    let mut found = Vec::new();
    let facts = read_control_plane_facts(path).await.unwrap();
    for (table, rows) in &facts.root_budget_tables {
        let hits = rows.iter().filter(|row| row.contains(needle)).count();
        if hits > 0 {
            found.push(format!("table {table}: {hits} row(s)"));
        }
    }
    for (suffix, label) in [("", "file: database"), ("-wal", "file: write-ahead log")] {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        if let Ok(bytes) = std::fs::read(PathBuf::from(name))
            && bytes
                .windows(needle.len())
                .any(|window| window == needle.as_bytes())
        {
            found.push(label.to_string());
        }
    }
    found
}

/// What is wrong with the broker's answer and the books of `request_id` after its
/// response arrived over a revoked source: empty when the answer is the blocked
/// response the broker already gives for a dispatch that became ineligible, the row
/// says unusable, the transport is redacted, and the output is nowhere.
async fn defects_of(
    path: &Path,
    store: &Store,
    request_id: &str,
    response: &ModelResponse,
) -> Vec<String> {
    let mut defects = Vec::new();
    match response {
        ModelResponse::Rejected {
            kind: ModelRejectionKind::CancelledAfterDispatch,
            dispatch: RejectedDispatch::Dispatched { .. },
            ..
        } => {}
        other => defects.push(format!(
            "the broker answered {other:?}, not a blocked response of a dispatched call"
        )),
    }
    let call = call_of(store, request_id).await;
    if call.response_usable != Some(false) {
        defects.push(format!("response_usable is {:?}", call.response_usable));
    }
    if call.response_block_reason.as_deref() != Some("source_revoked") {
        defects.push(format!(
            "response_block_reason is {:?}, not source_revoked",
            call.response_block_reason
        ));
    }
    match &call.transport_artifact {
        Some(transport) if transport.schema_version == REDACTED_SCHEMA => {}
        other => defects.push(format!(
            "the transport body is stored as {:?}, not redacted",
            other.as_ref().map(|artifact| &artifact.schema_version)
        )),
    }
    let found = found_in(path, &output_of(request_id)).await;
    if !found.is_empty() {
        defects.push(format!("the model output is readable in {found:?}"));
    }
    defects
}

/// A blocked answer of a dispatched call, and the receipt it carries.
fn blocked_receipt(response: &ModelResponse, what: &str) -> ModelExecutionReceipt {
    match response {
        ModelResponse::Rejected {
            kind: ModelRejectionKind::CancelledAfterDispatch,
            dispatch: RejectedDispatch::Dispatched { receipt },
            ..
        } => receipt.clone(),
        other => panic!("{what}: expected a blocked response of a dispatched call, got {other:?}"),
    }
}

/// The accounting facts of the settled call `request_id`: the real cost, the
/// provider's ids and the dispatch, which a revocation never reverses.
async fn assert_booked(store: &Store, request_id: &str, what: &str) {
    let call = call_of(store, request_id).await;
    assert_eq!(call.state, BudgetCallState::Finalized, "{what}");
    assert_eq!(call.actual_cost_micros, Some(10), "{what}");
    assert_eq!(call.actual_currency.as_deref(), Some("USD"), "{what}");
    assert_eq!(
        call.actual_pricing_version.as_deref(),
        Some("price-v1"),
        "{what}"
    );
    assert_eq!(
        call.provider_request_id.as_deref(),
        Some(format!("provider-{request_id}").as_str()),
        "{what}"
    );
    assert_eq!(
        call.usage_record_id.as_deref(),
        Some(format!("usage-{request_id}").as_str()),
        "{what}"
    );
    assert_eq!(
        call.output_digest,
        Some(hash(output_of(request_id).as_bytes())),
        "{what}"
    );
    assert!(call.dispatch_id.is_some(), "{what}");
    assert!(call.execution_closed, "{what}");
    assert_eq!(micros(store).await, (0, 10), "{what}");
}

// ---------------------------------------------------------------------------
// The late response of a revoked source
// ---------------------------------------------------------------------------

/// The revocation lands while the provider request is in flight: the response is
/// billed, the broker does not return it as completed, the row says unusable
/// (`source_revoked`), and the model output is nowhere in the database. The source
/// is the whole closure or one of two, and the cleanup has not started, or it has
/// run to `Complete` before the response arrives (the call's request is redacted by
/// then, so the cleanup never reads the call again: the case that used to store the
/// output in plaintext).
#[tokio::test]
async fn a_response_that_arrives_after_its_source_was_revoked_is_billed_but_not_completed() {
    for clean_before_the_response in [false, true] {
        for sources in [&[RUN][..], &[LIVE, RUN][..]] {
            let (_dir, path, store) = broker_store().await;
            let during_call = if clean_before_the_response {
                DuringCall::RevokeAndClean(store.clone())
            } else {
                DuringCall::Revoke(store.clone())
            };
            let what = format!("{during_call:?} over {sources:?}");
            let (transport, calls) = LateTransport::new(during_call);
            let broker = broker_over(&store, transport);
            let request = model_request("request-1", sources);

            let response = broker.dispatch(request.clone()).await.unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 1, "{what}");
            if clean_before_the_response {
                // The cleanup redacted the request of the call in flight, so it will
                // never read the call again: only the settlement can stop the output.
                let request = call_of(&store, "request-1").await.request_artifact;
                assert_eq!(request.unwrap().schema_version, REDACTED_SCHEMA, "{what}");
            }
            let defects = defects_of(&path, &store, "request-1", &response).await;
            assert!(
                defects.is_empty(),
                "{what}: the late response was not blocked and redacted:\n{defects:#?}"
            );
            // The cost is on the books and the receipt still names them.
            assert_booked(&store, "request-1", &what).await;
            let receipt = blocked_receipt(&response, &what);
            assert_eq!(receipt.call_id, "request-1", "{what}");
            broker.verify_receipt(&request, &receipt).await.unwrap();

            // A repeat of the request is answered from the books: it neither sends
            // again nor completes, and it does not un-spend.
            let again = broker.dispatch(request.clone()).await.unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 1, "{what}: sent again");
            blocked_receipt(&again, &format!("{what}, retried"));
            assert_booked(&store, "request-1", &format!("{what}, retried")).await;

            // The cleanup, run to the end, changes none of that.
            complete_cleanup(&store, revocation_job(&store).await).await;
            let after = broker.dispatch(request.clone()).await.unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 1, "{what}: sent again");
            blocked_receipt(&after, &format!("{what}, after the cleanup"));
            assert_booked(&store, "request-1", &format!("{what}, after the cleanup")).await;
            let call = call_of(&store, "request-1").await;
            assert_eq!(call.response_usable, Some(false), "{what}");
            assert_eq!(
                call.request_artifact.unwrap().schema_version,
                REDACTED_SCHEMA,
                "{what}"
            );
            assert!(
                found_in(&path, &output_of("request-1")).await.is_empty(),
                "{what}, after the cleanup: the model output is readable"
            );
        }
    }
}

/// The reason the broker already blocks a response for, a model other than the one
/// the request froze, still decides the answer (`ProviderRejected`) and the reason
/// on the row; the revocation adds only that the transport body is redacted and the
/// output is not stored.
#[tokio::test]
async fn a_wrong_model_response_after_the_revoke_stays_rejected_and_leaves_no_plaintext() {
    let (_dir, path, store) = broker_store().await;
    let (mut transport, calls) = LateTransport::new(DuringCall::RevokeAndClean(store.clone()));
    transport.actual_model = Some(hash(b"another-model"));
    let broker = broker_over(&store, transport);
    let request = model_request("request-1", &[RUN]);

    let response = broker.dispatch(request.clone()).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        matches!(
            response,
            ModelResponse::Rejected {
                kind: ModelRejectionKind::ProviderRejected,
                dispatch: RejectedDispatch::Dispatched { .. },
                ..
            }
        ),
        "{response:?}"
    );
    let call = call_of(&store, "request-1").await;
    assert_eq!(call.response_usable, Some(false));
    assert_eq!(
        call.response_block_reason.as_deref(),
        Some("actual_model_digest_mismatch")
    );
    assert_eq!(
        call.transport_artifact.unwrap().schema_version,
        REDACTED_SCHEMA
    );
    assert!(
        found_in(&path, &output_of("request-1")).await.is_empty(),
        "the model output is readable"
    );
    assert_booked(&store, "request-1", "wrong model").await;
}

/// The broker answers a call that was settled but never closed (the process died
/// between the settlement and the close of its execution) from the books, without
/// calling the provider. A late response that was settled over a revoked source is
/// recovered as the blocked response it was stored as, never as a completed one.
/// The settlement is made through the ledger directly, in the shapes the broker
/// stores, so that the recovery path is what the broker runs.
#[tokio::test]
async fn a_settled_late_response_recovered_after_a_restart_is_still_blocked() {
    for clean_before_the_response in [false, true] {
        let (_dir, path, store) = broker_store().await;
        let request = model_request("request-1", &[RUN]);
        let what = format!("cleaned before the response: {clean_before_the_response}");
        // Exactly the reservation the broker would have made for this request.
        let reserved = store
            .reserve_budget_call_with_sources(
                &host(),
                &BudgetCallReservation {
                    billing_scope: SCOPE.into(),
                    call_id: request.request_id.clone(),
                    dispatch_group_id: request.episode_id.clone(),
                    stage: BudgetStage::Reflection,
                    actual_input_digest: request.cache_key_digest.clone(),
                    request_artifact: Some(
                        BudgetArtifact::from_serializable(
                            "rsia.model_request.artifact.v1",
                            &request,
                        )
                        .unwrap(),
                    ),
                    max_cost_micros: 20,
                    lease_token: "lease-recovery".into(),
                    lease_until: 70,
                    now: 10,
                },
                &[RUN.to_string()],
            )
            .await
            .unwrap();
        let fence = BudgetCallFence {
            billing_scope: SCOPE.into(),
            call_id: request.request_id.clone(),
            actual_input_digest: request.cache_key_digest.clone(),
            lease_token: reserved.lease_token.clone(),
            lease_epoch: reserved.lease_epoch,
            now: 10,
        };
        let dispatched = store
            .begin_budget_dispatch(&host(), &fence)
            .await
            .unwrap()
            .call;
        let status = revocation_job(&store).await;
        if clean_before_the_response {
            complete_cleanup(&store, status).await;
        }

        // The response of the provider, settled in the shapes the broker stores.
        let completion = TransportCompletion {
            response_id: "response-request-1".into(),
            provider_request_id: "provider-request-1".into(),
            usage_record_id: "usage-request-1".into(),
            actual_model_digest: request.model_digest.clone(),
            output: output_of("request-1"),
            actual_cost_micros: 10,
            currency: "USD".into(),
            pricing_version: "price-v1".into(),
        };
        let output_digest = hash(completion.output.as_bytes());
        let receipt = ModelExecutionReceipt {
            call_id: request.request_id.clone(),
            dispatch_id: dispatched.dispatch_id.clone().unwrap(),
            root_budget_id: "root-1".into(),
            provider_request_id: completion.provider_request_id.clone(),
            usage_record_id: completion.usage_record_id.clone(),
            provenance: ModelExecutionProvenance::Fixture,
        };
        let completed = ModelResponse::Completed {
            request_id: request.request_id.clone(),
            response_id: completion.response_id.clone(),
            actual_model_digest: completion.actual_model_digest.clone(),
            input_digest: request.input_digest.clone(),
            output: completion.output.clone(),
            output_digest: output_digest.clone(),
            execution_receipt: receipt.clone(),
        };
        let blocked = ModelResponse::Rejected {
            request_id: request.request_id.clone(),
            kind: ModelRejectionKind::CancelledAfterDispatch,
            reason: "dispatch became ineligible after provider execution".into(),
            dispatch: RejectedDispatch::Dispatched { receipt },
        };
        let settlement = store
            .settle_model_budget_call(
                &host(),
                &BudgetCallFence { now: 20, ..fence },
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
                        &blocked,
                    )
                    .unwrap(),
                    forced_block_reason: None,
                },
            )
            .await
            .unwrap();
        assert!(!settlement.call.execution_closed, "{what}");
        assert!(!settlement.response_usable, "{what}");
        assert_eq!(
            settlement.response_block_reason.as_deref(),
            Some("source_revoked"),
            "{what}"
        );

        // The restart: the broker finds the settled call and answers from the books.
        let (transport, calls) = LateTransport::new(DuringCall::Nothing);
        let broker = broker_over(&store, transport);
        let response = broker.dispatch(request.clone()).await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "{what}: the provider was called"
        );
        let receipt = blocked_receipt(&response, &what);
        broker.verify_receipt(&request, &receipt).await.unwrap();
        let defects = defects_of(&path, &store, "request-1", &response).await;
        assert!(defects.is_empty(), "{what}:\n{defects:#?}");
        assert_booked(&store, "request-1", &what).await;
    }
}

// ---------------------------------------------------------------------------
// Controls: nothing changes while the source is live
// ---------------------------------------------------------------------------

/// A response over a source that is not revoked is completed, usable, and stored
/// as sent (the output is on the books). The same holds when another run was
/// revoked, and its cleanup completed, while the request was in flight. These are
/// the controls of the scans above: they do see a stored output.
#[tokio::test]
async fn a_response_over_a_live_source_is_still_completed_and_stored_as_sent() {
    for revoke_other_run in [false, true] {
        let (_dir, path, store) = broker_store().await;
        let during_call = if revoke_other_run {
            // `RUN` is revoked and cleaned while the request over `LIVE` is in flight.
            DuringCall::RevokeAndClean(store.clone())
        } else {
            DuringCall::Nothing
        };
        let (transport, calls) = LateTransport::new(during_call);
        let broker = broker_over(&store, transport);
        let request = model_request("request-live", &[LIVE]);
        let what = format!("another run revoked: {revoke_other_run}");

        let response = broker.dispatch(request.clone()).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "{what}");
        let receipt = match &response {
            ModelResponse::Completed {
                output,
                execution_receipt,
                ..
            } => {
                assert_eq!(output, &output_of("request-live"), "{what}");
                execution_receipt.clone()
            }
            other => panic!("{what}: a live source must complete: {other:?}"),
        };
        broker.verify_receipt(&request, &receipt).await.unwrap();
        let call = call_of(&store, "request-live").await;
        assert_eq!(call.response_usable, Some(true), "{what}");
        assert_eq!(call.response_block_reason, None, "{what}");
        assert_eq!(
            call.transport_artifact.unwrap().schema_version,
            "rsia.model_transport.artifact.v1",
            "{what}"
        );
        assert_booked(&store, "request-live", &what).await;
        let found = found_in(&path, &output_of("request-live")).await;
        assert!(
            found
                .iter()
                .any(|place| place.starts_with("table root_budget_calls")),
            "{what}: the scan does not see a live output: {found:?}"
        );
        assert!(
            found.iter().any(|place| place.starts_with("file")),
            "{what}: the scan does not see a live output in the files: {found:?}"
        );

        // A repeat is answered from the books, completed again.
        let again = broker.dispatch(request).await.unwrap();
        assert!(matches!(again, ModelResponse::Completed { .. }), "{what}");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "{what}");
    }
}
