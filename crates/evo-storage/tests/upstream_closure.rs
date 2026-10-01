//! AG-044 (E07/E08/§11): `Session::upstream_closure` walks the dependency edges
//! forward, from a dependent to what it depends on, and hands a revocation gate
//! the runs and artifacts of that closure with what it judges them by. Real SQLite
//! store, no model, no provider, zero monetary cost.
//!
//! Before this the session could read the edges only backwards (`dependents`, the
//! direction the revocation cleanup walks), so a gate at the point where a request
//! names its dependencies could judge those dependencies and nothing above them:
//! a live artifact whose upstream run was revoked, but not cleaned yet, passed.
//!
//! The walk is bounded like the other derivations of E16.5: a closure over the
//! limit is refused with a `Conflict`, never cut short and handed over as if it
//! were everything there is.

use evo_core::{Context, Error, Role};
use evo_storage::{Store, UpstreamNode};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

const REDACTED: &str = "rsia.redacted.v1";

fn ctx(namespace: &str) -> Context {
    Context::new(namespace, "admin", Role::Admin).unwrap()
}

async fn open() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("closure.sqlite3"))
        .await
        .unwrap();
    (dir, store)
}

/// Edges `(src kind, src id, dst kind, dst id)`.
async fn edges(store: &Store, ctx: &Context, edges: &[(&str, &str, &str, &str)]) {
    let mut session = store.session().await.unwrap();
    for (src_kind, src_id, dst_kind, dst_id) in edges {
        session
            .put_edge(ctx, src_kind, src_id, dst_kind, dst_id)
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
}

async fn put(store: &Store, ctx: &Context, kind: &str, id: &str, body: Value) {
    let mut session = store.session().await.unwrap();
    session.put(ctx, kind, id, "admin", &body).await.unwrap();
    session.commit().await.unwrap();
}

async fn walk(
    store: &Store,
    ctx: &Context,
    roots: &[(&str, &str)],
    limit: usize,
) -> Result<Vec<UpstreamNode>, Error> {
    let mut session = store.session().await.unwrap();
    let nodes = session.upstream_closure(ctx, roots, limit).await;
    session.commit().await.unwrap();
    nodes
}

fn keys(nodes: &[UpstreamNode]) -> Vec<(String, String)> {
    nodes
        .iter()
        .map(|node| (node.kind.clone(), node.id.clone()))
        .collect()
}

fn k(kind: &str, id: &str) -> (String, String) {
    (kind.to_string(), id.to_string())
}

fn conflict(result: Result<Vec<UpstreamNode>, Error>, what: &str) -> String {
    match result {
        Err(Error::Conflict(message)) => message,
        other => panic!("{what}: expected a Conflict, got {other:?}"),
    }
}

fn live(id: &str) -> Value {
    json!({"id": id, "schema_version": "fixture.live.v1"})
}

#[tokio::test]
async fn a_chain_is_walked_from_its_root_to_what_it_depends_on() {
    let (_dir, store) = open().await;
    let admin = ctx("n");
    edges(
        &store,
        &admin,
        &[
            ("artifact", "a", "artifact", "b"),
            ("artifact", "b", "artifact", "c"),
        ],
    )
    .await;
    let all = [k("artifact", "a"), k("artifact", "b"), k("artifact", "c")];
    assert_eq!(
        keys(
            &walk(&store, &admin, &[("artifact", "a")], 10)
                .await
                .unwrap()
        ),
        all
    );
    // Only forward: what depends on a node (`a` on `b`) is not part of its closure.
    assert_eq!(
        keys(
            &walk(&store, &admin, &[("artifact", "b")], 10)
                .await
                .unwrap()
        ),
        all[1..]
    );
    assert_eq!(
        keys(
            &walk(&store, &admin, &[("artifact", "c")], 10)
                .await
                .unwrap()
        ),
        all[2..]
    );
}

#[tokio::test]
async fn a_cycle_ends_the_walk_and_lists_each_node_once() {
    let (_dir, store) = open().await;
    let admin = ctx("n");
    edges(
        &store,
        &admin,
        &[
            ("artifact", "a", "artifact", "b"),
            ("artifact", "b", "artifact", "c"),
            ("artifact", "c", "artifact", "a"),
            // A node that depends on itself.
            ("artifact", "c", "artifact", "c"),
        ],
    )
    .await;
    let all = [k("artifact", "a"), k("artifact", "b"), k("artifact", "c")];
    for root in ["a", "b", "c"] {
        assert_eq!(
            keys(
                &walk(&store, &admin, &[("artifact", root)], 3)
                    .await
                    .unwrap()
            ),
            all,
            "from {root}"
        );
    }
    let message = conflict(
        walk(&store, &admin, &[("artifact", "a")], 2).await,
        "a cycle of three nodes under a limit of two",
    );
    assert!(message.contains('2'), "{message}");
}

#[tokio::test]
async fn a_diamond_lists_the_shared_node_once_and_counts_it_once() {
    let (_dir, store) = open().await;
    let admin = ctx("n");
    edges(
        &store,
        &admin,
        &[
            ("artifact", "a", "artifact", "b"),
            ("artifact", "a", "artifact", "c"),
            ("artifact", "b", "run", "d"),
            ("artifact", "c", "run", "d"),
        ],
    )
    .await;
    // Four nodes by four edges: the limit is on the nodes.
    let nodes = walk(&store, &admin, &[("artifact", "a")], 4).await.unwrap();
    assert_eq!(
        keys(&nodes),
        [
            k("artifact", "a"),
            k("artifact", "b"),
            k("artifact", "c"),
            k("run", "d")
        ]
    );
    conflict(
        walk(&store, &admin, &[("artifact", "a")], 3).await,
        "a diamond of four nodes under a limit of three",
    );
}

#[tokio::test]
async fn roots_are_deduplicated_and_counted_together() {
    let (_dir, store) = open().await;
    let admin = ctx("n");
    edges(
        &store,
        &admin,
        &[
            ("artifact", "a", "artifact", "b"),
            ("artifact", "x1", "artifact", "x2"),
            ("artifact", "x2", "artifact", "x3"),
            ("artifact", "y1", "artifact", "y2"),
            ("artifact", "y2", "artifact", "y3"),
        ],
    )
    .await;
    // The same root twice, and a root that is also reached from another root.
    let nodes = walk(
        &store,
        &admin,
        &[("artifact", "a"), ("artifact", "a"), ("artifact", "b")],
        2,
    )
    .await
    .unwrap();
    assert_eq!(keys(&nodes), [k("artifact", "a"), k("artifact", "b")]);
    conflict(
        walk(&store, &admin, &[("artifact", "a"), ("artifact", "a")], 1).await,
        "two nodes under a limit of one",
    );

    // Two disjoint chains of three: the limit counts both.
    let roots = [("artifact", "x1"), ("artifact", "y1")];
    assert_eq!(walk(&store, &admin, &roots, 6).await.unwrap().len(), 6);
    conflict(
        walk(&store, &admin, &roots, 5).await,
        "six nodes under a limit of five",
    );
    // Each of them alone fits a limit that both together do not.
    for root in [("artifact", "x1"), ("artifact", "y1")] {
        assert_eq!(walk(&store, &admin, &[root], 5).await.unwrap().len(), 3);
    }
}

#[tokio::test]
async fn every_kind_is_walked_and_counted_but_only_runs_and_artifacts_are_listed() {
    let (_dir, store) = open().await;
    let admin = ctx("n");
    edges(
        &store,
        &admin,
        &[
            ("artifact", "a", "release", "rel"),
            ("release", "rel", "candidate", "cand"),
            ("candidate", "cand", "feedback", "fb"),
            ("candidate", "cand", "run", "r"),
            ("feedback", "fb", "blob", "0123abcd"),
        ],
    )
    .await;
    // The run behind a release and a candidate is found; the middle nodes are not
    // listed.
    let nodes = walk(&store, &admin, &[("artifact", "a")], 6).await.unwrap();
    assert_eq!(keys(&nodes), [k("artifact", "a"), k("run", "r")]);
    // ... but they count: six nodes (a, rel, cand, fb, blob, r) do not fit five.
    conflict(
        walk(&store, &admin, &[("artifact", "a")], 5).await,
        "six nodes of five kinds under a limit of five",
    );
    // A root of another kind is a root all the same.
    let nodes = walk(&store, &admin, &[("release", "rel")], 5)
        .await
        .unwrap();
    assert_eq!(keys(&nodes), [k("run", "r")]);
}

#[tokio::test]
async fn a_node_nothing_is_stored_for_is_still_listed() {
    let (_dir, store) = open().await;
    let admin = ctx("n");
    // The cleanup deletes a revoked run and leaves the edge to it; a dependency
    // can also name an id that was never written.
    edges(&store, &admin, &[("artifact", "a", "run", "gone")]).await;
    let nodes = walk(
        &store,
        &admin,
        &[("artifact", "a"), ("artifact", "never-written")],
        10,
    )
    .await
    .unwrap();
    assert_eq!(
        nodes,
        [
            UpstreamNode {
                kind: "artifact".into(),
                id: "a".into(),
                tombstone: None,
                redacted: false
            },
            UpstreamNode {
                kind: "artifact".into(),
                id: "never-written".into(),
                tombstone: None,
                redacted: false
            },
            UpstreamNode {
                kind: "run".into(),
                id: "gone".into(),
                tombstone: None,
                redacted: false
            },
        ]
    );
}

#[tokio::test]
async fn the_tombstone_of_an_id_comes_with_the_run_and_the_artifact_that_share_it() {
    let (_dir, store) = open().await;
    let admin = ctx("n");
    edges(
        &store,
        &admin,
        &[
            ("artifact", "root", "run", "shared"),
            ("artifact", "root", "artifact", "shared"),
            ("artifact", "root", "run", "plain"),
            ("artifact", "root", "run", "odd"),
        ],
    )
    .await;
    // Keyed by the id alone: one tombstone for a run and an artifact of that id.
    let shared = json!({
        "id": "shared", "schema_version": "rsia.revoke_tombstone.v1", "source_kind": "run",
        "reason": "test", "watermark_seq": 1, "watermark_digest": "a".repeat(64), "created_at": 1
    });
    put(&store, &admin, "tombstone", "shared", shared.clone()).await;
    // The body is handed over as stored, whether or not it is one `begin_revoke`
    // writes: judging it is the caller's.
    put(&store, &admin, "tombstone", "odd", json!({"id": "odd"})).await;
    let nodes = walk(&store, &admin, &[("artifact", "root")], 10)
        .await
        .unwrap();
    let tombstones: Vec<_> = nodes
        .iter()
        .map(|node| (node.kind.as_str(), node.id.as_str(), node.tombstone.clone()))
        .collect();
    assert_eq!(
        tombstones,
        [
            ("artifact", "root", None),
            ("artifact", "shared", Some(shared.clone())),
            ("run", "odd", Some(json!({"id": "odd"}))),
            ("run", "plain", None),
            ("run", "shared", Some(shared)),
        ]
    );
}

#[tokio::test]
async fn a_redacted_artifact_is_marked_and_a_run_never_is() {
    let (_dir, store) = open().await;
    let admin = ctx("n");
    edges(
        &store,
        &admin,
        &[
            ("artifact", "root", "artifact", "live"),
            ("artifact", "root", "artifact", "redacted"),
            ("artifact", "root", "artifact", "no-schema"),
            ("artifact", "root", "artifact", "odd-schema"),
            ("artifact", "root", "run", "run-redacted-looking"),
            ("artifact", "root", "release", "rel-redacted"),
        ],
    )
    .await;
    put(&store, &admin, "artifact", "live", live("live")).await;
    put(
        &store,
        &admin,
        "artifact",
        "redacted",
        json!({"id": "redacted", "schema_version": REDACTED}),
    )
    .await;
    put(
        &store,
        &admin,
        "artifact",
        "no-schema",
        json!({"id": "no-schema"}),
    )
    .await;
    put(
        &store,
        &admin,
        "artifact",
        "odd-schema",
        json!({"id": "odd-schema", "schema_version": 5}),
    )
    .await;
    // The cleanup deletes a run, never redacts it, and a run body can be large:
    // it is not read, whatever it says.
    put(
        &store,
        &admin,
        "run",
        "run-redacted-looking",
        json!({"id": "run-redacted-looking", "schema_version": REDACTED}),
    )
    .await;
    put(
        &store,
        &admin,
        "release",
        "rel-redacted",
        json!({"id": "rel-redacted", "schema_version": REDACTED}),
    )
    .await;
    let nodes = walk(&store, &admin, &[("artifact", "root")], 20)
        .await
        .unwrap();
    let marked: Vec<_> = nodes
        .iter()
        .map(|node| (node.kind.as_str(), node.id.as_str(), node.redacted))
        .collect();
    assert_eq!(
        marked,
        [
            ("artifact", "live", false),
            ("artifact", "no-schema", false),
            ("artifact", "odd-schema", false),
            ("artifact", "redacted", true),
            ("artifact", "root", false),
            ("run", "run-redacted-looking", false),
        ]
    );
}

#[tokio::test]
async fn namespaces_do_not_cross() {
    let (_dir, store) = open().await;
    let (n, other) = (ctx("n"), ctx("other"));
    edges(
        &store,
        &n,
        &[
            ("artifact", "a", "artifact", "b"),
            ("artifact", "b", "run", "r-n"),
        ],
    )
    .await;
    edges(
        &store,
        &other,
        &[
            ("artifact", "a", "artifact", "c"),
            ("artifact", "b", "run", "r-other"),
            ("artifact", "c", "run", "r-c"),
        ],
    )
    .await;
    // Tombstones and bodies are the namespace's own as well: `b` is tombstoned in
    // `other` only, and the artifact `r-n` is live in `n` and redacted in `other`.
    put(
        &store,
        &other,
        "tombstone",
        "b",
        json!({"id": "b", "source_kind": "artifact"}),
    )
    .await;
    put(&store, &n, "artifact", "r-n", live("r-n")).await;
    put(
        &store,
        &other,
        "artifact",
        "r-n",
        json!({"id": "r-n", "schema_version": REDACTED}),
    )
    .await;
    let nodes = walk(&store, &n, &[("artifact", "a")], 10).await.unwrap();
    assert_eq!(
        keys(&nodes),
        [k("artifact", "a"), k("artifact", "b"), k("run", "r-n")]
    );
    assert!(nodes.iter().all(|node| node.tombstone.is_none()));
    let nodes = walk(&store, &other, &[("artifact", "a")], 10)
        .await
        .unwrap();
    assert_eq!(
        keys(&nodes),
        [k("artifact", "a"), k("artifact", "c"), k("run", "r-c")]
    );
    let in_n = walk(&store, &n, &[("artifact", "r-n")], 1).await.unwrap();
    assert!(!in_n[0].redacted);
    let in_other = walk(&store, &other, &[("artifact", "r-n")], 1)
        .await
        .unwrap();
    assert!(in_other[0].redacted);
    let b_other = walk(&store, &other, &[("artifact", "b")], 2).await.unwrap();
    assert_eq!(keys(&b_other), [k("artifact", "b"), k("run", "r-other")]);
    assert!(b_other[0].tombstone.is_some());
}

#[tokio::test]
async fn a_closure_over_the_limit_is_a_conflict_and_never_a_truncated_list() {
    let (_dir, store) = open().await;
    let admin = ctx("n");
    edges(
        &store,
        &admin,
        &[
            ("artifact", "n0", "artifact", "n1"),
            ("artifact", "n1", "artifact", "n2"),
            ("artifact", "n2", "artifact", "n3"),
            ("artifact", "n3", "run", "n4"),
        ],
    )
    .await;
    let root = [("artifact", "n0")];
    // A closure of five nodes: exactly the limit passes, one less does not.
    assert_eq!(walk(&store, &admin, &root, 5).await.unwrap().len(), 5);
    let message = conflict(
        walk(&store, &admin, &root, 4).await,
        "five nodes, limit four",
    );
    assert!(message.contains("exceeds 4 nodes"), "{message}");
    assert_eq!(
        walk(&store, &admin, &root, usize::MAX).await.unwrap().len(),
        5
    );

    // A limit of zero holds no root at all.
    conflict(walk(&store, &admin, &root, 0).await, "one root, limit zero");
    assert!(walk(&store, &admin, &[], 0).await.unwrap().is_empty());
    assert!(walk(&store, &admin, &[], 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_root_with_an_invalid_id_is_invalid_input() {
    let (_dir, store) = open().await;
    let admin = ctx("n");
    for id in ["has space", "", "semi;colon"] {
        let result = walk(&store, &admin, &[("artifact", "ok"), ("artifact", id)], 10).await;
        assert!(
            matches!(result, Err(Error::Invalid(_))),
            "{id:?}: {result:?}"
        );
    }
}

#[tokio::test]
async fn more_roots_than_a_statement_can_bind_are_walked() {
    let (_dir, store) = open().await;
    let admin = ctx("n");
    let ids: Vec<String> = (0..40_000).map(|index| format!("root-{index}")).collect();
    let roots: Vec<(&str, &str)> = ids.iter().map(|id| ("artifact", id.as_str())).collect();
    let nodes = walk(&store, &admin, &roots, 50_000).await.unwrap();
    assert_eq!(nodes.len(), 40_000);
    conflict(
        walk(&store, &admin, &roots, 39_999).await,
        "40 000 roots under a limit of 39 999",
    );
}

#[tokio::test]
async fn the_walk_reads_what_its_own_session_wrote_and_nothing_uncommitted_survives() {
    let (_dir, store) = open().await;
    let admin = ctx("n");
    let mut session = store.session().await.unwrap();
    session
        .put_edge(&admin, "artifact", "x", "run", "y")
        .await
        .unwrap();
    let nodes = session
        .upstream_closure(&admin, &[("artifact", "x")], 10)
        .await
        .unwrap();
    assert_eq!(keys(&nodes), [k("artifact", "x"), k("run", "y")]);
    // Dropped without a commit: the edge is gone and so is the walk's view of it.
    drop(session);
    let nodes = walk(&store, &admin, &[("artifact", "x")], 10)
        .await
        .unwrap();
    assert_eq!(keys(&nodes), [k("artifact", "x")]);
}

/// The two things the statement is built on, observed through the public call
/// (see the comment on the statement): the walk stops after `limit + 1` nodes,
/// and each node costs one lookup, not a scan of the namespace's edges. A chain of
/// 200 000 edges sits in the namespace and the walk starts at its head.
///
/// - A walk that builds the whole closure before it applies the limit takes
///   hundreds of milliseconds for a limit of 10.
/// - A walk that scans the edges of the namespace once per node takes a similar
///   time for 11 nodes and minutes for 10 000.
/// - The statement as written takes well under a millisecond for the first, and a
///   few tens of milliseconds for the second.
///
/// The bounds below sit a hundred times above what it takes and far below both
/// failures, and the best of five runs is taken, so a loaded machine does not
/// fail it.
#[tokio::test]
async fn the_walk_stops_at_the_limit_and_looks_each_node_up_by_its_key() {
    const EDGES: i64 = 200_000;
    let (dir, store) = open().await;
    let admin = ctx("n");
    // One statement writes the chain `chain-0 -> chain-1 -> ... -> chain-200000`
    // through a second connection; the store is idle meanwhile.
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite:{}",
        dir.path().join("closure.sqlite3").display()
    ))
    .await
    .unwrap();
    sqlx::query(
        "WITH RECURSIVE n(x) AS (SELECT 0 UNION ALL SELECT x + 1 FROM n WHERE x < ?)
         INSERT INTO dependencies(namespace, src_kind, src_id, dst_kind, dst_id)
         SELECT 'n', 'artifact', 'chain-' || x, 'artifact', 'chain-' || (x + 1) FROM n",
    )
    .bind(EDGES - 1)
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    let mut best = Duration::MAX;
    for _ in 0..5 {
        let started = Instant::now();
        let message = conflict(
            walk(&store, &admin, &[("artifact", "chain-0")], 10).await,
            "a chain of 200 000 nodes under a limit of 10",
        );
        best = best.min(started.elapsed());
        assert!(message.contains("exceeds 10 nodes"), "{message}");
    }
    assert!(
        best < Duration::from_millis(50),
        "a walk cut at 11 nodes took {best:?} at best"
    );

    // From the node 9 999 edges below the end of the chain, the closure is
    // exactly 10 000 nodes: it fits a limit of 10 000, and the node before it
    // does not.
    let fits = format!("chain-{}", EDGES - 9_999);
    let over = format!("chain-{}", EDGES - 10_000);
    let started = Instant::now();
    let nodes = tokio::time::timeout(
        Duration::from_secs(60),
        walk(&store, &admin, &[("artifact", fits.as_str())], 10_000),
    )
    .await
    .expect("a walk of 10 000 nodes did not finish: it scans the edges once per node")
    .unwrap();
    let elapsed = started.elapsed();
    assert_eq!(nodes.len(), 10_000);
    assert!(
        elapsed < Duration::from_secs(5),
        "a walk of 10 000 nodes took {elapsed:?}"
    );
    conflict(
        walk(&store, &admin, &[("artifact", over.as_str())], 10_000).await,
        "10 001 nodes under a limit of 10 000",
    );
    println!("UPSTREAM_CLOSURE limit 10: best {best:?}; 10 000 nodes: {elapsed:?}");
}
