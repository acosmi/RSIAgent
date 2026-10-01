//! AG-035 (E08/§11, V017): a revocation cleanup completes only at a closure
//! fixpoint. Real SQLite store, no model, no provider, zero monetary cost.
//!
//! The cleanup expands the dependency closure lazily: a node's dependents are
//! read through a forward key-set cursor and, once the last page is read, the node
//! is `expanded` and never read again. A dependent that is written after
//! `begin_revoke` can therefore escape in two ways: its key sorts before the cursor
//! of a node that is still being paged, or the node was already expanded. Before
//! this change the job went `Complete` as soon as the queue was empty, with that
//! dependent still holding its content.
//!
//! These tests drive a cleanup one step at a time (`edge_page_limit = 1` unless
//! stated), write the late dependent at the exact point where the old code could
//! not see it, and require it to be redacted when the job reaches `Complete`.
//! Budget calls are a second closure: `__budget_scan` reads `root_budget_calls`
//! once, so a call reserved after the scan keeps its request body unless the final
//! recheck covers it too. The no-late-write cases pin the number of steps and the
//! processed-node count the cleanup took before this change.

use evo_core::{Context, Role, hash};
use evo_storage::Store;
use evo_storage::budget::{
    BudgetArtifact, BudgetCallReservation, BudgetCallState, BudgetStage,
    REGISTERED_EXECUTION_REQUEST_SCHEMA, RootBudgetAuthorization,
};
use evo_storage::lifecycle::{CleanupState, CleanupStatus, LifecycleStore, TypedObjectRef};
use serde_json::{Value, json};
use std::path::Path;

const REDACTED: &str = "rsia.redacted.v1";
const SOURCE: &str = "run-r";
const DATABASE: &str = "rsia.sqlite3";
/// More than any scenario below needs: a cleanup that does not finish within it
/// is stuck, not slow.
const MAX_STEPS: usize = 2_000;

fn admin() -> Context {
    Context::new("n", "admin", Role::Admin).unwrap()
}

fn worker() -> Context {
    Context::new("n", "worker", Role::Worker).unwrap()
}

fn host() -> Context {
    Context::new("n", "broker", Role::Host).unwrap()
}

async fn open(dir: &Path) -> Store {
    Store::open(&dir.join(DATABASE)).await.unwrap()
}

async fn put_run(store: &Store, id: &str) {
    let host = host();
    let mut session = store.session().await.unwrap();
    session
        .put(
            &host,
            "run",
            id,
            host.actor(),
            &json!({"id":id,"schema_version":"rsia.optimization.source.v1","body":"run-body"}),
        )
        .await
        .unwrap();
    session.commit().await.unwrap();
}

/// A stage fact of a kind the cleanup redacts. The payload carries a marker that
/// is unique to the node, so a surviving body is recognizable.
fn fact_body(id: &str) -> Value {
    json!({
        "id": id,
        "schema_version": "rsia.optimization.stage_fact.v1",
        "kind": "terminal_rejected",
        "state": "failed",
        "payload": format!("SECRET-{id}"),
    })
}

/// Stores the fact `id` with one typed edge per `(kind, id)` it depends on.
async fn put_fact(store: &Store, id: &str, depends_on: &[(&str, &str)]) {
    put_body(store, id, &fact_body(id), depends_on).await;
}

async fn put_body(store: &Store, id: &str, body: &Value, depends_on: &[(&str, &str)]) {
    let worker = worker();
    let mut session = store.session().await.unwrap();
    session
        .put(&worker, "artifact", id, worker.actor(), body)
        .await
        .unwrap();
    for (kind, destination) in depends_on {
        session
            .put_edge(&worker, "artifact", id, kind, destination)
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
}

async fn body(store: &Store, kind: &str, id: &str) -> Value {
    let mut session = store.session().await.unwrap();
    let value = session.need(&admin(), kind, id).await.unwrap();
    session.commit().await.unwrap();
    value
}

async fn is_redacted(store: &Store, id: &str) -> bool {
    body(store, "artifact", id).await["schema_version"] == REDACTED
}

/// The stored body of `id` must be a tombstone that kept none of its content.
async fn assert_redacted(store: &Store, id: &str) {
    let value = body(store, "artifact", id).await;
    assert_eq!(
        value["schema_version"], REDACTED,
        "{id} still holds its content after the cleanup: {value}"
    );
    assert!(
        !value.to_string().contains(&format!("SECRET-{id}")),
        "{id} kept its payload: {value}"
    );
}

async fn begin(store: &Store, kind: &str, id: &str) -> CleanupStatus {
    LifecycleStore::begin_revoke(
        &admin(),
        store,
        TypedObjectRef {
            kind: kind.into(),
            id: id.into(),
        },
        "source revoked",
        1,
    )
    .await
    .unwrap()
}

/// One `cleanup_step`; `clock` is the monotonic `now` of the test.
async fn step(store: &Store, job_id: &str, limit: usize, clock: &mut i64) -> CleanupStatus {
    *clock += 1;
    LifecycleStore::cleanup_step(&admin(), store, job_id, limit, *clock)
        .await
        .unwrap()
}

fn finished(status: &CleanupStatus) -> bool {
    matches!(status.state, CleanupState::Complete | CleanupState::Failed)
}

/// `(state, processed_nodes, pending_nodes)` after each step.
type Trajectory = Vec<(CleanupState, u64, u64)>;

/// Steps until the job is `Complete` or `Failed`; returns the final status and
/// what every step reported.
async fn drive(
    store: &Store,
    mut status: CleanupStatus,
    limit: usize,
    clock: &mut i64,
) -> (CleanupStatus, Trajectory) {
    let mut trajectory = Vec::new();
    for _ in 0..MAX_STEPS {
        if finished(&status) {
            return (status, trajectory);
        }
        status = step(store, &status.job_id, limit, clock).await;
        trajectory.push((status.state, status.processed_nodes, status.pending_nodes));
    }
    panic!("the cleanup did not finish within {MAX_STEPS} steps: {status:?}");
}

/// Steps until `reached(&status)` holds; the job must not finish first.
async fn step_until(
    store: &Store,
    mut status: CleanupStatus,
    limit: usize,
    clock: &mut i64,
    what: &str,
    reached: impl Fn(&CleanupStatus) -> bool,
) -> CleanupStatus {
    for _ in 0..MAX_STEPS {
        if reached(&status) {
            return status;
        }
        assert!(
            !finished(&status),
            "the job finished before {what}: {status:?}"
        );
        status = step(store, &status.job_id, limit, clock).await;
    }
    panic!("never reached {what}: {status:?}");
}

/// Steps until the content of `id` is redacted, i.e. the node has been expanded.
async fn step_until_redacted(
    store: &Store,
    mut status: CleanupStatus,
    id: &str,
    limit: usize,
    clock: &mut i64,
) -> CleanupStatus {
    for _ in 0..MAX_STEPS {
        if is_redacted(store, id).await {
            return status;
        }
        assert!(
            !finished(&status),
            "the job finished before {id} was redacted: {status:?}"
        );
        status = step(store, &status.job_id, limit, clock).await;
    }
    panic!("{id} was never redacted: {status:?}");
}

/// The graph most scenarios start from: `run-r` <- `p` <- `c1`.
async fn chain(store: &Store) {
    put_run(store, SOURCE).await;
    put_fact(store, "p", &[("run", SOURCE)]).await;
    put_fact(store, "c1", &[("artifact", "p")]).await;
}

// ---------------------------------------------------------------------------
// Dependency edges
// ---------------------------------------------------------------------------

/// Deterministic reproduction of the cursor escape. `p` depends on the run and
/// has the dependents `c-b`, `c-d`, `c-f`. With one edge per page, after the first
/// page of `p` its cursor is at `c-b`; `c-a` sorts before it and can never be
/// returned by that cursor. Once `p` is expanded (and redacted) it is never read
/// again; `c-late` arrives after that.
#[tokio::test]
async fn a_dependent_behind_the_page_cursor_or_added_after_expansion_is_cleaned_before_complete() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    put_run(&store, SOURCE).await;
    put_fact(&store, "p", &[("run", SOURCE)]).await;
    for dependent in ["c-b", "c-d", "c-f"] {
        put_fact(&store, dependent, &[("artifact", "p")]).await;
    }
    let mut clock = 10;
    let status = begin(&store, "run", SOURCE).await;

    // The budget scan, the world scan and the run are expanded (3 processed), `p`
    // and `c-b` are queued: the cursor of `p` is at `c-b`.
    let status = step_until(
        &store,
        status,
        1,
        &mut clock,
        "the first page of p",
        |status| status.processed_nodes >= 3 && status.pending_nodes >= 2,
    )
    .await;
    assert!(
        !is_redacted(&store, "p").await,
        "p is still being paged: {status:?}"
    );
    put_fact(&store, "c-a", &[("artifact", "p")]).await;

    let status = step_until_redacted(&store, status, "p", 1, &mut clock).await;
    assert_eq!(status.state, CleanupState::Running, "{status:?}");
    put_fact(&store, "c-late", &[("artifact", "p")]).await;

    let (status, _) = drive(&store, status, 1, &mut clock).await;
    for id in ["p", "c-a", "c-b", "c-d", "c-f", "c-late"] {
        assert_redacted(&store, id).await;
    }
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert_eq!(status.pending_nodes, 0);
    assert_eq!(status.last_error, None);
    // the budget scan, the world scan, the run, `p`, its three dependents and the
    // two late ones
    assert_eq!(status.processed_nodes, 9, "{status:?}");
}

/// A recheck queues at most `edge_page_limit` late dependents per step, and the
/// job stays `Running` while it still finds some. A writer that keeps adding a
/// dependent only delays `Complete`; it never gets one past it.
#[tokio::test]
async fn late_dependents_are_queued_one_page_per_step_and_a_continuing_writer_only_delays_complete()
{
    // A burst of five late dependents, two edges per page.
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    chain(&store).await;
    let mut clock = 10;
    let status = begin(&store, "run", SOURCE).await;
    let status = step_until_redacted(&store, status, "p", 2, &mut clock).await;
    assert_eq!(status.pending_nodes, 1, "only c1 is left: {status:?}");
    for index in 1..=5 {
        put_fact(&store, &format!("d{index}"), &[("artifact", "p")]).await;
    }
    // The step expands `c1`, finds the queue empty and queues two of the five.
    let queued = step(&store, &status.job_id, 2, &mut clock).await;
    let (status, _) = drive(&store, queued.clone(), 2, &mut clock).await;
    for id in ["c1", "d1", "d2", "d3", "d4", "d5"] {
        assert_redacted(&store, id).await;
    }
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert_eq!(
        (queued.state, queued.pending_nodes),
        (CleanupState::Running, 2),
        "the recheck must queue exactly one page of late dependents"
    );
    // the scans, the run, `p`, `c1` and the five late dependents
    assert_eq!(status.processed_nodes, 10, "{status:?}");

    // A writer that adds one more dependent each time the queue is down to its
    // last node.
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    chain(&store).await;
    let mut clock = 10;
    let mut status = begin(&store, "run", SOURCE).await;
    status = step_until_redacted(&store, status, "p", 1, &mut clock).await;
    let mut added = 0;
    for _ in 0..MAX_STEPS {
        if status.pending_nodes == 1 && added < 5 {
            added += 1;
            put_fact(&store, &format!("w{added}"), &[("artifact", "p")]).await;
        }
        if finished(&status) {
            break;
        }
        status = step(&store, &status.job_id, 1, &mut clock).await;
    }
    for id in ["c1", "w1", "w2", "w3", "w4", "w5"] {
        assert_redacted(&store, id).await;
    }
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert_eq!(
        added, 5,
        "the job completed while the writer was still adding"
    );
    assert_eq!(status.processed_nodes, 10, "{status:?}");
}

/// A late dependent of a schema the cleanup does not classify is not "cleaned":
/// the job fails closed (`blocked_unknown_scope`) instead of reporting `Complete`.
#[tokio::test]
async fn an_unclassified_late_dependent_fails_the_job_instead_of_completing_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    chain(&store).await;
    let mut clock = 10;
    let status = begin(&store, "run", SOURCE).await;
    let status = step_until_redacted(&store, status, "p", 1, &mut clock).await;
    put_body(
        &store,
        "late-unknown",
        &json!({"id":"late-unknown","schema_version":"rsia.unknown.v9","payload":"SECRET-late-unknown"}),
        &[("artifact", "p")],
    )
    .await;

    let (status, trajectory) = drive(&store, status, 1, &mut clock).await;
    assert!(
        trajectory
            .iter()
            .all(|(state, _, _)| *state != CleanupState::Complete),
        "the job reported Complete with an unclassified dependent: {trajectory:?}"
    );
    assert_eq!(status.state, CleanupState::Failed, "{status:?}");
    let error = status.last_error.clone().unwrap_or_default();
    assert!(
        error.contains("blocked_unknown_scope:artifact:rsia.unknown.v9"),
        "{status:?}"
    );
    // Worker keeps Failed. An explicit Admin retry must revisit the complete
    // frontier before it can conclude; the unknown node still fails that round.
    let unknown_before = body(&store, "artifact", "late-unknown").await;
    clock += 1;
    let worker_status = LifecycleStore::cleanup_step(&worker(), &store, &status.job_id, 1, clock)
        .await
        .unwrap();
    assert_eq!(
        worker_status.state,
        CleanupState::Failed,
        "{worker_status:?}"
    );
    assert_eq!(worker_status.last_error, status.last_error);
    let started = step(&store, &status.job_id, 1, &mut clock).await;
    assert_eq!(started.state, CleanupState::Running, "{started:?}");
    assert_eq!(started.last_error, status.last_error);
    let (again, retry_trajectory) = drive(&store, started, 1, &mut clock).await;
    assert!(
        retry_trajectory
            .iter()
            .all(|(state, _, _)| *state != CleanupState::Complete),
        "retry reported Complete with an unclassified dependent: {retry_trajectory:?}"
    );
    assert_eq!(again.state, CleanupState::Failed, "{again:?}");
    assert_eq!(
        again.last_error.as_deref(),
        Some("blocked_unknown_scope:artifact:rsia.unknown.v9:")
    );
    assert_eq!(again.job_id, status.job_id);
    assert_eq!(again.source, status.source);
    assert_eq!(again.watermark_seq, status.watermark_seq);
    assert_eq!(again.watermark_digest, status.watermark_digest);
    assert_eq!(
        body(&store, "artifact", "late-unknown").await,
        unknown_before
    );
}

/// The recheck reads only the tables, so a restart changes nothing: a dependent
/// written after the store was reopened is found, and so is one the recheck had
/// queued before a second restart.
#[tokio::test]
async fn a_late_dependent_is_cleaned_across_restarts_including_one_queued_before_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    chain(&store).await;
    let mut clock = 10;
    let status = begin(&store, "run", SOURCE).await;
    let status = step_until_redacted(&store, status, "p", 1, &mut clock).await;
    let job_id = status.job_id.clone();

    // First restart; the late dependent is written to the reopened store.
    store.close().await;
    drop(store);
    let store = open(dir.path()).await;
    let reopened = LifecycleStore::cleanup_status(&admin(), &store, &job_id)
        .await
        .unwrap();
    put_fact(&store, "after-restart", &[("artifact", "p")]).await;

    // `c1` is the last node: this step empties the queue and the recheck queues
    // `after-restart`.
    let queued = step(&store, &job_id, 1, &mut clock).await;

    // Second restart with the late node still waiting in the persisted queue.
    store.close().await;
    drop(store);
    let store = open(dir.path()).await;
    let waiting = LifecycleStore::cleanup_status(&admin(), &store, &job_id)
        .await
        .unwrap();
    let (status, _) = drive(&store, waiting.clone(), 1, &mut clock).await;

    for id in ["c1", "after-restart"] {
        assert_redacted(&store, id).await;
    }
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert_eq!(reopened.state, CleanupState::Running, "{reopened:?}");
    assert_eq!(
        (queued.state, queued.pending_nodes),
        (CleanupState::Running, 1),
        "the late node must be queued, not skipped"
    );
    assert_eq!(waiting.pending_nodes, 1, "{waiting:?}");
}

/// A frontier is the closure of one job: a node another job has queued is still
/// a late dependent for this one. `n` is queued by the job of `run-1` and not
/// expanded; the job of `run-2` must queue `n` itself when `n` gains an edge to
/// a node it has already expanded.
#[tokio::test]
async fn the_recheck_uses_the_frontier_of_its_own_job() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    put_run(&store, "run-1").await;
    put_run(&store, "run-2").await;
    put_fact(&store, "n", &[("run", "run-1")]).await;
    put_fact(&store, "m", &[("run", "run-2")]).await;
    put_fact(&store, "m2", &[("run", "run-2")]).await;
    let mut clock = 10;

    // Job 1: `n` is queued, not expanded.
    let first = begin(&store, "run", "run-1").await;
    let first = step_until(
        &store,
        first,
        1,
        &mut clock,
        "n queued in job 1",
        |status| status.processed_nodes >= 3 && status.pending_nodes == 1,
    )
    .await;
    assert!(!is_redacted(&store, "n").await);

    // Job 2: `m` is expanded, `m2` is still queued.
    let second = begin(&store, "run", "run-2").await;
    let second = step_until_redacted(&store, second, "m", 1, &mut clock).await;
    assert_eq!(second.pending_nodes, 1, "only m2 is left: {second:?}");
    put_fact(&store, "n", &[("run", "run-1"), ("artifact", "m")]).await;

    let (second, _) = drive(&store, second, 1, &mut clock).await;
    assert_redacted(&store, "n").await;
    assert_eq!(second.state, CleanupState::Complete, "{second:?}");
    // the scans, the run, `m`, `m2` and the late `n`
    assert_eq!(second.processed_nodes, 6, "{second:?}");
    // job 1 is not affected and still completes
    let (first, _) = drive(&store, first, 1, &mut clock).await;
    assert_eq!(first.state, CleanupState::Complete, "{first:?}");
}

// ---------------------------------------------------------------------------
// Budget calls
// ---------------------------------------------------------------------------

async fn authorize_budget(store: &Store) {
    store
        .authorize_root_budget(
            &admin(),
            &RootBudgetAuthorization {
                root_budget_id: "root-1".into(),
                billing_scope: "scope-1".into(),
                allowed_namespaces: vec!["n".into()],
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

fn reservation(call_id: &str, request: BudgetArtifact) -> BudgetCallReservation {
    BudgetCallReservation {
        billing_scope: "scope-1".into(),
        call_id: call_id.into(),
        dispatch_group_id: "group-1".into(),
        stage: BudgetStage::Reflection,
        actual_input_digest: hash(format!("input-{call_id}").as_bytes()),
        request_artifact: Some(request),
        max_cost_micros: 20,
        lease_token: format!("lease-{call_id}"),
        lease_until: 100,
        now: 1,
    }
}

/// A model request whose source closure is `sources`.
fn model_request(call_id: &str, sources: &[&str]) -> BudgetArtifact {
    let closure: Vec<Value> = sources
        .iter()
        .map(|id| json!({"id":id,"digest":hash(id.as_bytes())}))
        .collect();
    BudgetArtifact::from_serializable(
        "rsia.model_request.artifact.v1",
        &json!({
            "request_id": call_id,
            "source_closure": closure,
            "input": [{"content": format!("BUDGET-SECRET-{call_id}")}],
        }),
    )
    .unwrap()
}

/// A registered in-process execution whose source ids are `sources`.
fn registered_request(call_id: &str, sources: &[&str]) -> BudgetArtifact {
    BudgetArtifact::from_serializable(
        REGISTERED_EXECUTION_REQUEST_SCHEMA,
        &json!({
            "source_ids": sources,
            "input": format!("BUDGET-SECRET-{call_id}"),
        }),
    )
    .unwrap()
}

async fn request_of(store: &Store, call_id: &str) -> BudgetArtifact {
    let call = store
        .budget_call(&host(), "scope-1", call_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(call.state, BudgetCallState::Reserved, "{call_id}: {call:?}");
    call.request_artifact.expect("a request artifact")
}

/// `__budget_scan` reads `root_budget_calls` once. A call reserved after it has
/// passed names the revoked run in its request, so its request body must be
/// redacted before the job reports `Complete`; its reservation (accounting) must
/// stay. A call that does not name the run is left alone.
#[tokio::test]
async fn a_budget_call_reserved_after_the_budget_scan_is_redacted_before_complete() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    put_run(&store, SOURCE).await;
    authorize_budget(&store).await;
    let mut clock = 10;
    let status = begin(&store, "run", SOURCE).await;
    // The first step expands `__budget_scan`; there is no call yet.
    let status = step(&store, &status.job_id, 1, &mut clock).await;
    assert_eq!(status.processed_nodes, 1, "{status:?}");

    // Without a source list (no budget-ref edge to the run: only the scan could
    // find them): a model request whose closure is the revoked run alone, one that
    // names it with another run, a registered execution, and one that names another
    // run only. A late reservation with a source list is refused (AG-038), so that
    // case can no longer be written here.
    store
        .reserve_budget_call(
            &host(),
            &reservation("call-sole", model_request("call-sole", &[SOURCE])),
        )
        .await
        .unwrap();
    store
        .reserve_budget_call(
            &host(),
            &reservation(
                "call-bare",
                model_request("call-bare", &[SOURCE, "run-other"]),
            ),
        )
        .await
        .unwrap();
    store
        .reserve_budget_call(
            &host(),
            &reservation(
                "call-registered",
                registered_request("call-registered", &[SOURCE]),
            ),
        )
        .await
        .unwrap();
    store
        .reserve_budget_call(
            &host(),
            &reservation("call-other", model_request("call-other", &["run-other"])),
        )
        .await
        .unwrap();

    let (status, _) = drive(&store, status, 1, &mut clock).await;

    for call_id in ["call-sole", "call-bare", "call-registered"] {
        let request = request_of(&store, call_id).await;
        assert_eq!(
            request.schema_version, REDACTED,
            "{call_id} kept its request after the cleanup: {request:?}"
        );
        assert!(
            !request.body.contains("BUDGET-SECRET"),
            "{call_id} kept its content: {request:?}"
        );
    }
    let other = request_of(&store, "call-other").await;
    assert_eq!(other.schema_version, "rsia.model_request.artifact.v1");
    assert!(other.body.contains("BUDGET-SECRET-call-other"));
    let root = store
        .root_budget(&admin(), "scope-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!((root.reserved_micros, root.spent_micros), (80, 0));
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert_eq!(status.pending_nodes, 0);
    assert_eq!(status.last_error, None);

    // The reservations of the three late calls are historical accounting facts
    // that depend on the run, exactly as the scan leaves an earlier one.
    let mut session = store.session().await.unwrap();
    let dependents = session.dependents(&admin(), "run", SOURCE).await.unwrap();
    session.commit().await.unwrap();
    assert_eq!(dependents.len(), 3, "{dependents:?}");
}

/// The same fail-closed rule as the scan: a late call whose request is of a
/// schema the cleanup cannot read cannot be shown to be free of the source.
#[tokio::test]
async fn an_unclassified_late_budget_request_fails_the_job_instead_of_completing_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    put_run(&store, SOURCE).await;
    authorize_budget(&store).await;
    let mut clock = 10;
    let status = begin(&store, "run", SOURCE).await;
    let status = step(&store, &status.job_id, 1, &mut clock).await;
    store
        .reserve_budget_call(
            &host(),
            &reservation(
                "call-unknown",
                BudgetArtifact::from_serializable(
                    "rsia.unknown_request.v9",
                    &json!({"input":"BUDGET-SECRET-call-unknown"}),
                )
                .unwrap(),
            ),
        )
        .await
        .unwrap();

    let (status, trajectory) = drive(&store, status, 1, &mut clock).await;
    assert!(
        trajectory
            .iter()
            .all(|(state, _, _)| *state != CleanupState::Complete),
        "the job reported Complete with an unreadable request: {trajectory:?}"
    );
    assert_eq!(status.state, CleanupState::Failed, "{status:?}");
    let error = status.last_error.clone().unwrap_or_default();
    assert!(
        error.contains("unknown_budget_request_schema:rsia.unknown_request.v9"),
        "{status:?}"
    );
}

// ---------------------------------------------------------------------------
// No late write: the steps the cleanup took before this change
// ---------------------------------------------------------------------------

/// A graph that exercises paging, a diamond, a cycle, a preserved node and the
/// budget scan, with every write done before `begin_revoke`.
async fn static_graph(store: &Store) {
    put_run(store, SOURCE).await;
    put_run(store, "run-other").await;
    authorize_budget(store).await;
    put_fact(store, "p1", &[("run", SOURCE)]).await;
    put_fact(store, "p2", &[("run", SOURCE)]).await;
    // a cycle
    put_fact(store, "q1", &[("artifact", "p1"), ("artifact", "q2")]).await;
    put_fact(store, "q2", &[("artifact", "q1")]).await;
    // a diamond
    put_fact(store, "x", &[("artifact", "p1"), ("artifact", "p2")]).await;
    // a wide fan-in
    for index in 1..=7 {
        put_fact(store, &format!("f{index}"), &[("artifact", "p2")]).await;
    }
    // a preserved node behind the fan-in
    let worker = worker();
    let mut session = store.session().await.unwrap();
    session
        .put(
            &worker,
            "evaluation",
            "eval-1",
            worker.actor(),
            &json!({"id":"eval-1","score":7,"state":"failed"}),
        )
        .await
        .unwrap();
    session
        .put_edge(&worker, "evaluation", "eval-1", "artifact", "f1")
        .await
        .unwrap();
    session.commit().await.unwrap();
    // one call the scan redacts, one it leaves
    store
        .reserve_budget_call_with_sources(
            &host(),
            &reservation("call-m", model_request("call-m", &[SOURCE])),
            &[SOURCE.to_string()],
        )
        .await
        .unwrap();
    store
        .reserve_budget_call(
            &host(),
            &reservation("call-n", model_request("call-n", &["run-other"])),
        )
        .await
        .unwrap();
}

/// `(edge_page_limit, steps to Complete, processed_nodes)` of [`static_graph`] as
/// measured on the code before AG-035 (commit 04e6309). A recheck of a closed
/// graph finds nothing and completes in the step that empties the queue, so none
/// of the three may change.
const STATIC_GRAPH_BEFORE: [(usize, usize, u64); 4] =
    [(1, 28, 17), (3, 7, 17), (8, 3, 17), (256, 1, 17)];

#[tokio::test]
async fn a_graph_without_late_writes_takes_the_steps_it_took_before() {
    let mut measured = Vec::new();
    for (limit, _, _) in STATIC_GRAPH_BEFORE {
        let dir = tempfile::tempdir().unwrap();
        let store = open(dir.path()).await;
        static_graph(&store).await;
        let mut clock = 10;
        let status = begin(&store, "run", SOURCE).await;
        let (status, trajectory) = drive(&store, status, limit, &mut clock).await;
        assert_eq!(
            status.state,
            CleanupState::Complete,
            "limit {limit}: {status:?}"
        );
        assert_eq!(status.last_error, None, "limit {limit}");
        for id in ["p1", "p2", "q1", "q2", "x", "f1", "f7"] {
            assert_redacted(&store, id).await;
        }
        measured.push((limit, trajectory.len(), status.processed_nodes));
    }
    assert_eq!(measured, STATIC_GRAPH_BEFORE.to_vec());
}

/// The scenario of `tests/lifecycle.rs::more_than_ten_thousand_edges_are_keyset_
/// paginated_without_truncation`, with its step count and processed-node count
/// pinned: `(steps to Complete, processed_nodes)` before AG-035.
const FAN_IN_BEFORE: (usize, u64) = (79, 10_004);

#[tokio::test]
async fn ten_thousand_fan_in_edges_take_the_steps_they_took_before() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path()).await;
    put_run(&store, "run-many").await;
    let worker = worker();
    let mut session = store.session().await.unwrap();
    for index in 0..10_001 {
        session
            .put_edge(
                &worker,
                "artifact",
                &format!("node-{index:05}"),
                "run",
                "run-many",
            )
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    let mut clock = 10;
    let status = begin(&store, "run", "run-many").await;
    let (status, trajectory) = drive(&store, status, 256, &mut clock).await;
    assert_eq!(status.state, CleanupState::Complete, "{status:?}");
    assert_eq!((trajectory.len(), status.processed_nodes), FAN_IN_BEFORE);
}
