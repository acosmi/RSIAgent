use evo_core::{Context, Error, Role};
use evo_storage::Store;
use evo_storage::dependency_read::{DirectEdge, DirectEdgeCursor};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn ctx(namespace: &str) -> Context {
    Context::new(namespace, "admin", Role::Admin).unwrap()
}
fn edge(kind: &str, id: &str) -> DirectEdge {
    DirectEdge {
        dst_kind: kind.into(),
        dst_id: id.into(),
    }
}

fn snapshot(path: &Path) -> Value {
    let script = "import json,sqlite3,sys\nc=sqlite3.connect('file:'+sys.argv[1]+'?mode=ro',uri=True)\nnames=sorted(r[0] for r in c.execute(\"SELECT name FROM sqlite_master WHERE type='table'\"))\ndef rows(n):\n data=[[{'blob':x.hex()} if isinstance(x,bytes) else x for x in r] for r in c.execute('SELECT * FROM '+chr(34)+n.replace(chr(34),chr(34)*2)+chr(34))]\n return sorted(data,key=lambda r:json.dumps(r,sort_keys=True))\nprint(json.dumps({n:rows(n) for n in names}))";
    let output = Command::new("python3")
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
    serde_json::from_slice(&output.stdout).unwrap()
}

#[tokio::test]
async fn ordered_pages_count_lookahead_and_exact_full_final_page_without_writes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pages.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let ctx = ctx("pages");
    let mut session = store.session().await.unwrap();
    for i in (0..512).rev() {
        session
            .put_edge(
                &ctx,
                "artifact",
                "subject",
                if i % 2 == 0 { "blob" } else { "artifact" },
                &format!("id-{i:04}"),
            )
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    let before = snapshot(&path);
    let mut session = store.session().await.unwrap();
    let first = session
        .direct_edges_page(&ctx, "artifact", "subject", None, 256)
        .await
        .unwrap();
    assert_eq!(first.edges.len(), 256);
    assert_eq!(first.rows_read, 257);
    assert!(!first.exhausted);
    assert!(first.edges.iter().all(|edge| edge.dst_kind == "artifact"));
    let second = session
        .direct_edges_page(&ctx, "artifact", "subject", first.next_cursor.as_ref(), 256)
        .await
        .unwrap();
    assert_eq!(second.edges.len(), 256);
    assert_eq!(second.rows_read, 256);
    assert!(second.exhausted);
    assert!(second.next_cursor.is_none());
    assert!(second.edges.iter().all(|edge| edge.dst_kind == "blob"));
    assert!(first.edges.last().unwrap() < second.edges.first().unwrap());
    let empty = session
        .direct_edges_page(&ctx, "artifact", "empty", None, 1)
        .await
        .unwrap();
    assert!(empty.exhausted && empty.edges.is_empty() && empty.rows_read == 0);
    let one = session
        .direct_edges_page(&ctx, "artifact", "subject", None, 1)
        .await
        .unwrap();
    assert_eq!(one.edges, vec![edge("artifact", "id-0001")]);
    assert_eq!(one.rows_read, 2);
    assert!(!one.exhausted);
    session.commit().await.unwrap();
    assert_eq!(snapshot(&path), before);
}

#[tokio::test]
async fn namespace_source_and_typed_destination_are_exact_and_cursors_bound() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("binding.sqlite3"))
        .await
        .unwrap();
    let a = ctx("a");
    let b = ctx("b");
    let mut session = store.session().await.unwrap();
    for (context, src_kind, src, kind, dst) in [
        (&a, "artifact", "same", "artifact", "digest"),
        (&a, "artifact", "same", "blob", "digest"),
        (&b, "artifact", "same", "run", "foreign"),
        (&a, "run", "same", "blob", "other-kind"),
        (&a, "artifact", "other", "blob", "other-source"),
    ] {
        session
            .put_edge(context, src_kind, src, kind, dst)
            .await
            .unwrap();
    }
    session.commit().await.unwrap();
    let mut session = store.session().await.unwrap();
    let first = session
        .direct_edges_page(&a, "artifact", "same", None, 1)
        .await
        .unwrap();
    assert_eq!(first.edges, vec![edge("artifact", "digest")]);
    let cursor = first.next_cursor.unwrap();
    assert!(matches!(
        session
            .direct_edges_page(&b, "artifact", "same", Some(&cursor), 1)
            .await,
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        session
            .direct_edges_page(&a, "artifact", "other", Some(&cursor), 1)
            .await,
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        session
            .direct_edges_page(&a, "run", "same", Some(&cursor), 1)
            .await,
        Err(Error::Invalid(_))
    ));
    let last = session
        .direct_edges_page(&a, "artifact", "same", Some(&cursor), 1)
        .await
        .unwrap();
    assert_eq!(last.edges, vec![edge("blob", "digest")]);
    assert!(last.exhausted && last.next_cursor.is_none());
    assert_eq!(
        session
            .direct_edges_page(&b, "artifact", "same", None, 256)
            .await
            .unwrap()
            .edges,
        vec![edge("run", "foreign")]
    );
    session.commit().await.unwrap();
}

#[tokio::test]
async fn invalid_bounds_identifiers_roles_and_cursor_fields_fail_without_writes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("invalid.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let ctx = ctx("limits");
    let before = snapshot(&path);
    let mut session = store.session().await.unwrap();
    for limit in [0, 257, usize::MAX] {
        assert!(matches!(
            session
                .direct_edges_page(&ctx, "artifact", "source", None, limit)
                .await,
            Err(Error::Invalid(_))
        ));
    }
    for (kind, id) in [
        ("bad kind", "source"),
        ("artifact", ""),
        ("artifact", "bad/id"),
        ("artifact", "\0"),
    ] {
        assert!(matches!(
            session.direct_edges_page(&ctx, kind, id, None, 1).await,
            Err(Error::Invalid(_))
        ));
    }
    for role in [Role::Agent, Role::Host, Role::Evaluator, Role::Worker] {
        let other = Context::new("limits", "admin", role).unwrap();
        assert!(matches!(
            session
                .direct_edges_page(&other, "artifact", "source", None, 1)
                .await,
            Err(Error::Forbidden)
        ));
    }
    let forged: DirectEdgeCursor = serde_json::from_value(serde_json::json!({"namespace":"limits","src_kind":"artifact","src_id":"source","after":{"dst_kind":"blob","dst_id":"bad/id"}})).unwrap();
    assert!(matches!(
        session
            .direct_edges_page(&ctx, "artifact", "source", Some(&forged), 1)
            .await,
        Err(Error::Invalid(_))
    ));
    for limit in [1, 256] {
        assert!(
            session
                .direct_edges_page(&ctx, "artifact", "source", None, limit)
                .await
                .unwrap()
                .exhausted
        );
    }
    session.commit().await.unwrap();
    assert_eq!(before, snapshot(&path));
}

#[tokio::test]
async fn same_session_keeps_snapshot_cross_session_cursor_is_only_position() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("snapshot.sqlite3");
    let store = Store::open(&path).await.unwrap();
    let writer = Store::open(&path).await.unwrap();
    let ctx = ctx("snapshot");
    let mut initial = writer.session().await.unwrap();
    for id in ["a", "c"] {
        initial
            .put_edge(&ctx, "artifact", "source", "blob", id)
            .await
            .unwrap();
    }
    initial.commit().await.unwrap();
    let mut read = store.session().await.unwrap();
    let first = read
        .direct_edges_page(&ctx, "artifact", "source", None, 1)
        .await
        .unwrap();
    let cursor = first.next_cursor.unwrap();
    let mut write = writer.session().await.unwrap();
    write
        .put_edge(&ctx, "artifact", "source", "blob", "b")
        .await
        .unwrap();
    write.commit().await.unwrap();
    let next = read
        .direct_edges_page(&ctx, "artifact", "source", Some(&cursor), 256)
        .await
        .unwrap();
    assert_eq!(next.edges, vec![edge("blob", "c")]);
    read.commit().await.unwrap();
    let mut later = store.session().await.unwrap();
    assert_eq!(
        later
            .direct_edges_page(&ctx, "artifact", "source", Some(&cursor), 256)
            .await
            .unwrap()
            .edges,
        vec![edge("blob", "b"), edge("blob", "c")]
    );
    later.commit().await.unwrap();
}
