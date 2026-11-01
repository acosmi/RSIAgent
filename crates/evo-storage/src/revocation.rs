//! One raw, kind-exact revocation judgement for content consumers. No state is
//! written here: namespace isolation is derived from the conflicting source rows.

use crate::lifecycle::{REVOKE_TOMBSTONE_SCHEMA, RevokeTombstone, parse_strict_json};
use crate::{RawUpstreamNode, Session, internal, upstream_rows_in_tx};
use evo_core::evidence::RevocableSourceKind;
use evo_core::{Context, Error, Result, identifier, text};
use sqlx::{Row, Sqlite, Transaction};

pub const MAX_UPSTREAM_CLOSURE_NODES: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevocationCause {
    Revoked,
    Unreadable,
    Redacted,
    NamespaceConflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevocationFinding {
    pub kind: String,
    pub id: String,
    pub cause: RevocationCause,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RevocationStatus {
    Live,
    Blocked,
    Unverifiable,
}

pub(crate) fn strict_tombstone(body: &str, id: &str) -> Option<RevokeTombstone> {
    let value = parse_strict_json(body).ok()?;
    let tombstone: RevokeTombstone = serde_json::from_value(value).ok()?;
    valid_tombstone(&tombstone, id).then_some(tombstone)
}

pub(crate) fn valid_tombstone(tombstone: &RevokeTombstone, id: &str) -> bool {
    tombstone.schema_version == REVOKE_TOMBSTONE_SCHEMA
        && tombstone.id == id
        && RevocableSourceKind::from_physical_kind(&tombstone.source_kind).is_some()
        && tombstone.watermark_seq > 0
        && valid_digest(&tombstone.watermark_digest)
        && tombstone.created_at >= 0
        && text(&tombstone.reason, "reason", 512).is_ok()
}

fn valid_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn judge_node(node: RawUpstreamNode) -> Option<RevocationFinding> {
    let cause = if let Some(body) = node.tombstone {
        match strict_tombstone(&body, &node.id) {
            Some(tombstone) if tombstone.source_kind == node.kind => Some(RevocationCause::Revoked),
            Some(_) => None,
            None => Some(RevocationCause::Unreadable),
        }
    } else {
        None
    };
    cause
        .or(node.redacted.then_some(RevocationCause::Redacted))
        .map(|cause| RevocationFinding {
            kind: node.kind,
            id: node.id,
            cause,
        })
}

pub(crate) async fn qualified_source_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    namespace: &str,
    kind: RevocableSourceKind,
    id: &str,
) -> Result<bool> {
    let row = sqlx::query("SELECT owner,body FROM objects WHERE namespace=? AND kind=? AND id=?")
        .bind(namespace)
        .bind(kind.physical_kind())
        .bind(id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(internal)?;
    let Some(row) = row else { return Ok(false) };
    let body: String = row.try_get("body").map_err(internal)?;
    let owner: String = row.try_get("owner").map_err(internal)?;
    let Ok(value) = parse_strict_json(&body) else {
        return Ok(false);
    };
    Ok(crate::lifecycle::qualified_revocable_source(
        namespace, &owner, kind, id, &value,
    ))
}

async fn namespace_collision_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    namespace: &str,
) -> Result<Option<RevocationFinding>> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT r.id FROM objects r JOIN objects a
           ON a.namespace=r.namespace AND a.kind='artifact' AND a.id=r.id
         WHERE r.namespace=? AND r.kind='run' ORDER BY r.id",
    )
    .bind(namespace)
    .fetch_all(&mut **tx)
    .await
    .map_err(internal)?;
    for id in ids {
        if qualified_source_in_tx(tx, namespace, RevocableSourceKind::TrustedRun, &id).await?
            && qualified_source_in_tx(tx, namespace, RevocableSourceKind::ImportSource, &id).await?
        {
            return Ok(Some(RevocationFinding {
                kind: "namespace".into(),
                id,
                cause: RevocationCause::NamespaceConflict,
            }));
        }
    }
    Ok(None)
}

pub(crate) async fn dependencies_live_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    namespace: &str,
    roots: &[(&str, &str)],
    require_existing: bool,
) -> Result<bool> {
    Ok(status_in_tx(tx, namespace, roots, require_existing).await? == RevocationStatus::Live)
}

pub(crate) async fn status_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    namespace: &str,
    roots: &[(&str, &str)],
    require_existing: bool,
) -> Result<RevocationStatus> {
    if namespace_collision_in_tx(tx, namespace).await?.is_some() {
        return Ok(RevocationStatus::Unverifiable);
    }
    let Some(nodes) =
        upstream_rows_in_tx(tx, namespace, roots, MAX_UPSTREAM_CLOSURE_NODES, true).await?
    else {
        return Ok(RevocationStatus::Unverifiable);
    };
    for node in nodes {
        if require_existing && !node.exists {
            return Ok(RevocationStatus::Unverifiable);
        }
        if judge_node(node).is_some() {
            return Ok(RevocationStatus::Blocked);
        }
    }
    Ok(RevocationStatus::Live)
}

impl Session {
    /// Complete closure status for a compensation decision. An incomplete walk
    /// is a blocked source decision, never permission to release a reservation.
    #[doc(hidden)]
    pub async fn revocation_sources_live(
        &mut self,
        ctx: &Context,
        roots: &[(String, String)],
    ) -> Result<bool> {
        let mut seeds = Vec::with_capacity(roots.len());
        for (kind, id) in roots {
            identifier(id)?;
            seeds.push((kind.as_str(), id.as_str()));
        }
        dependencies_live_in_tx(&mut self.tx, ctx.namespace(), &seeds, false).await
    }

    /// Qualification is read from complete physical bytes, never a schema label.
    #[doc(hidden)]
    pub async fn qualified_revocable_source(
        &mut self,
        ctx: &Context,
        kind: RevocableSourceKind,
        id: &str,
    ) -> Result<bool> {
        identifier(id)?;
        qualified_source_in_tx(&mut self.tx, ctx.namespace(), kind, id).await
    }

    /// A conflict isolates new content in this namespace without a new persisted
    /// quarantine format. Accounting and the D4 close path do not call this gate.
    #[doc(hidden)]
    pub async fn ensure_revocation_namespace_live(&mut self, ctx: &Context) -> Result<()> {
        if let Some(conflict) = namespace_collision_in_tx(&mut self.tx, ctx.namespace()).await? {
            return Err(Error::Conflict(format!(
                "revocation source id collision isolates namespace {}: {}",
                ctx.namespace(),
                conflict.id
            )));
        }
        Ok(())
    }

    /// Direct check used before a consumer's existing digest/permission checks.
    #[doc(hidden)]
    pub async fn revocation_dependency(
        &mut self,
        ctx: &Context,
        kind: &str,
        id: &str,
    ) -> Result<Option<RevocationFinding>> {
        identifier(id)?;
        if let Some(conflict) = namespace_collision_in_tx(&mut self.tx, ctx.namespace()).await? {
            return Ok(Some(conflict));
        }
        let row = sqlx::query(
            "SELECT t.body AS tombstone,
                    COALESCE(json_extract(o.body,'$.schema_version')='rsia.redacted.v1',0) AS redacted,
                    o.id IS NOT NULL AS present
             FROM (SELECT 1) LEFT JOIN objects t
               ON ?1 IN ('run','artifact') AND t.namespace=?2 AND t.kind='tombstone' AND t.id=?3
             LEFT JOIN objects o ON o.namespace=?2 AND o.kind=?1 AND o.id=?3",
        )
        .bind(kind)
        .bind(ctx.namespace())
        .bind(id)
        .fetch_one(&mut *self.tx)
        .await
        .map_err(internal)?;
        Ok(judge_node(RawUpstreamNode {
            kind: kind.into(),
            id: id.into(),
            tombstone: row.try_get("tombstone").map_err(internal)?,
            redacted: row.try_get::<i64, _>("redacted").map_err(internal)? != 0,
            exists: row.try_get::<i64, _>("present").map_err(internal)? != 0,
        }))
    }

    /// Complete raw closure; intermediate kinds count and cycles/diamonds dedupe.
    #[doc(hidden)]
    pub async fn revocation_upstream(
        &mut self,
        ctx: &Context,
        roots: &[(String, String)],
        limit: usize,
    ) -> Result<Option<RevocationFinding>> {
        if let Some(conflict) = namespace_collision_in_tx(&mut self.tx, ctx.namespace()).await? {
            return Ok(Some(conflict));
        }
        let mut seeds = Vec::with_capacity(roots.len());
        for (kind, id) in roots {
            identifier(id)?;
            seeds.push((kind.as_str(), id.as_str()));
        }
        let nodes = upstream_rows_in_tx(&mut self.tx, ctx.namespace(), &seeds, limit, true)
            .await?
            .ok_or_else(|| Error::Conflict(format!(
                "upstream dependency closure exceeds {limit} nodes; it is refused, not truncated"
            )))?;
        Ok(nodes.into_iter().find_map(judge_node))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;
    use crate::lifecycle::{LifecycleStore, TypedObjectRef};
    use evo_core::{Role, hash};
    use serde_json::{Value, json};

    fn tombstone(id: &str, kind: &str) -> Value {
        json!({"id":id,"schema_version":REVOKE_TOMBSTONE_SCHEMA,"source_kind":kind,
            "reason":"fixture","watermark_seq":1,"watermark_digest":hash(b"watermark"),"created_at":1})
    }

    #[test]
    fn q03_strict_tombstone_checks_all_identity_fields() {
        let good = tombstone("source", "run");
        assert!(strict_tombstone(&good.to_string(), "source").is_some());
        for (field, invalid) in [
            ("schema_version", json!("other")),
            ("id", json!("other")),
            ("source_kind", json!("unknown")),
            ("watermark_seq", json!(0)),
            ("watermark_digest", json!("invalid")),
            ("created_at", json!(-1)),
            ("reason", json!("x".repeat(513))),
        ] {
            let mut body = good.clone();
            body[field] = invalid;
            assert!(
                strict_tombstone(&body.to_string(), "source").is_none(),
                "{field}"
            );
        }
        let mut extra = good;
        extra["extra"] = json!(true);
        assert!(strict_tombstone(&extra.to_string(), "source").is_none());
    }

    #[test]
    fn q03_strict_tombstone_rejects_duplicate_decoded_keys() {
        let body = tombstone("source", "run").to_string();
        let duplicate = body.replace(
            "\"source_kind\":\"run\"",
            "\"source_kind\":\"run\",\"source\\u005fkind\":\"artifact\"",
        );
        assert_ne!(body, duplicate);
        assert!(strict_tombstone(&duplicate, "source").is_none());
        assert!(strict_tombstone("{unreadable", "source").is_none());
    }

    #[tokio::test]
    async fn q03_other_kind_tombstone_keeps_upstream_check() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("q03.sqlite3")).await.unwrap();
        let ctx = Context::new("n", "admin", Role::Admin).unwrap();
        let other = Context::new("other", "admin", Role::Admin).unwrap();
        let mut session = store.session().await.unwrap();
        session
            .put(
                &ctx,
                "artifact",
                "ordinary",
                ctx.actor(),
                &json!({"id":"ordinary"}),
            )
            .await
            .unwrap();
        session
            .put(&ctx, "run", "source", ctx.actor(), &json!({"id":"source"}))
            .await
            .unwrap();
        session
            .put(
                &ctx,
                "tombstone",
                "ordinary",
                ctx.actor(),
                &tombstone("ordinary", "run"),
            )
            .await
            .unwrap();
        session
            .put(
                &other,
                "tombstone",
                "source",
                other.actor(),
                &tombstone("source", "run"),
            )
            .await
            .unwrap();
        session
            .put_edge(&ctx, "artifact", "ordinary", "candidate", "middle")
            .await
            .unwrap();
        session
            .put_edge(&ctx, "candidate", "middle", "run", "source")
            .await
            .unwrap();
        let roots = vec![("artifact".to_string(), "ordinary".to_string())];
        assert!(session.revocation_sources_live(&ctx, &roots).await.unwrap());
        session.commit().await.unwrap();
        LifecycleStore::begin_revoke(
            &ctx,
            &store,
            TypedObjectRef {
                kind: "run".into(),
                id: "source".into(),
            },
            "fixture",
            2,
        )
        .await
        .unwrap();
        let mut session = store.session().await.unwrap();
        assert!(
            session
                .revocation_dependency(&ctx, "artifact", "ordinary")
                .await
                .unwrap()
                .is_none()
        );
        let finding = session
            .revocation_upstream(&ctx, &roots, MAX_UPSTREAM_CLOSURE_NODES)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (finding.kind.as_str(), finding.id.as_str(), finding.cause),
            ("run", "source", RevocationCause::Revoked)
        );
    }

    #[tokio::test]
    async fn q03_upstream_walk_counts_intermediate_kinds_and_deduplicates_diamonds() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("q03.sqlite3")).await.unwrap();
        let ctx = Context::new("n", "admin", Role::Admin).unwrap();
        let mut session = store.session().await.unwrap();
        for (from_kind, from, to_kind, to) in [
            ("artifact", "root", "candidate", "left"),
            ("artifact", "root", "release", "right"),
            ("candidate", "left", "run", "source"),
            ("release", "right", "run", "source"),
            ("run", "source", "artifact", "root"),
        ] {
            session
                .put_edge(&ctx, from_kind, from, to_kind, to)
                .await
                .unwrap();
        }
        let roots = vec![("artifact".to_string(), "root".to_string())];
        assert!(
            session
                .revocation_upstream(&ctx, &roots, 4)
                .await
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            session.revocation_upstream(&ctx, &roots, 3).await,
            Err(Error::Conflict(_))
        ));
        session
            .put(
                &ctx,
                "tombstone",
                "source",
                ctx.actor(),
                &tombstone("source", "run"),
            )
            .await
            .unwrap();
        assert_eq!(
            session
                .revocation_upstream(&ctx, &roots, 4)
                .await
                .unwrap()
                .unwrap()
                .id,
            "source"
        );
        session
            .put(
                &ctx,
                "candidate",
                "redacted-direct",
                ctx.actor(),
                &json!({"schema_version":"rsia.redacted.v1"}),
            )
            .await
            .unwrap();
        assert_eq!(
            session
                .revocation_dependency(&ctx, "candidate", "redacted-direct")
                .await
                .unwrap()
                .unwrap()
                .cause,
            RevocationCause::Redacted
        );
    }

    #[tokio::test]
    async fn q03_unreadable_or_overflowing_closure_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("q03.sqlite3")).await.unwrap();
        let ctx = Context::new("n", "admin", Role::Admin).unwrap();
        let mut session = store.session().await.unwrap();
        session
            .put(&ctx, "tombstone", "root", ctx.actor(), &json!({}))
            .await
            .unwrap();
        assert!(
            !session
                .revocation_sources_live(&ctx, &[("run".into(), "root".into())])
                .await
                .unwrap()
        );
        session.delete(&ctx, "tombstone", "root").await.unwrap();
        sqlx::query(
            "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i+1 FROM n WHERE i<10000)
            INSERT INTO dependencies(namespace,src_kind,src_id,dst_kind,dst_id)
            SELECT 'n','candidate','node-'||i,'candidate','node-'||(i+1) FROM n",
        )
        .execute(&mut *session.tx)
        .await
        .unwrap();
        let roots = [("candidate".to_string(), "node-0".to_string())];
        assert!(!session.revocation_sources_live(&ctx, &roots).await.unwrap());
        assert!(matches!(
            session
                .revocation_upstream(&ctx, &roots, MAX_UPSTREAM_CLOSURE_NODES)
                .await,
            Err(Error::Conflict(_))
        ));
    }
}
