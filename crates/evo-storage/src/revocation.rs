//! One raw, kind-exact revocation judgement for content consumers. No state is
//! written here: namespace isolation is derived from the conflicting source rows.

use crate::lifecycle::{
    REVOKE_TOMBSTONE_SCHEMA, RevokeTombstone, TypedObjectRef, parse_strict_json,
    registered_import_source, valid_native_trace_authority,
};
use crate::{REDACTED_SCHEMA, Session, internal, upstream_rows_in_tx};
use evo_core::evidence::RevocableSourceKind;
use evo_core::{Context, Error, Result, identifier, text};
use sqlx::{Row, Sqlite, Transaction};

pub const MAX_UPSTREAM_CLOSURE_NODES: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevocationCause {
    Revoked,
    Unreadable,
    Redacted,
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

/// A tombstone written for this node's own kind revokes it; a valid tombstone of
/// the other kind belongs to another object; an invalid one fails closed.
fn judge(
    kind: String,
    id: String,
    tombstone: Option<String>,
    redacted: bool,
) -> Option<RevocationFinding> {
    let cause = match tombstone.map(|body| strict_tombstone(&body, &id)) {
        Some(Some(tombstone)) if tombstone.source_kind == kind => Some(RevocationCause::Revoked),
        Some(None) => Some(RevocationCause::Unreadable),
        _ => redacted.then_some(RevocationCause::Redacted),
    };
    cause.map(|cause| RevocationFinding { kind, id, cause })
}

/// One registered-source definition for both physical kinds: a run carrying the
/// complete trusted-host trace authority, or a strict E16 import_source envelope.
pub(crate) async fn registered_source_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    namespace: &str,
    kind: RevocableSourceKind,
    id: &str,
) -> Result<bool> {
    let body: Option<String> =
        sqlx::query_scalar("SELECT body FROM objects WHERE namespace=? AND kind=? AND id=?")
            .bind(namespace)
            .bind(kind.physical_kind())
            .bind(id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(internal)?;
    let Some(value) = body.and_then(|body| parse_strict_json(&body).ok()) else {
        return Ok(false);
    };
    let node = TypedObjectRef {
        kind: kind.physical_kind().into(),
        id: id.into(),
    };
    Ok(match kind {
        RevocableSourceKind::TrustedRun => valid_native_trace_authority(&node, &value),
        RevocableSourceKind::ImportSource => registered_import_source(namespace, &node, &value),
    })
}

/// The id of a stored pair of registered sources (a trusted run and a strict
/// import_source) that share one tombstone key. Only such a pair isolates the
/// namespace; an ordinary artifact or an unqualified run never does. The import
/// side is read first, so a run body is read only for a real import_source id.
pub(crate) async fn namespace_collision_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    namespace: &str,
) -> Result<Option<String>> {
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
        if registered_source_in_tx(tx, namespace, RevocableSourceKind::ImportSource, &id).await?
            && registered_source_in_tx(tx, namespace, RevocableSourceKind::TrustedRun, &id).await?
        {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

fn namespace_conflict(namespace: &str, id: &str) -> Error {
    Error::Conflict(format!(
        "revocation source id collision isolates namespace {namespace}: {id}"
    ))
}

/// Complete-closure status. `typed_e16` is the stricter E16 snapshot contract:
/// every node must exist and run-body redaction counts. Direct gates read only
/// artifact redaction, as `Session::upstream_closure` always has.
pub(crate) async fn status_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    namespace: &str,
    roots: &[(&str, &str)],
    typed_e16: bool,
) -> Result<RevocationStatus> {
    if namespace_collision_in_tx(tx, namespace).await?.is_some() {
        return Ok(RevocationStatus::Unverifiable);
    }
    let Some(nodes) =
        upstream_rows_in_tx(tx, namespace, roots, MAX_UPSTREAM_CLOSURE_NODES, typed_e16).await?
    else {
        return Ok(RevocationStatus::Unverifiable);
    };
    for node in nodes {
        if typed_e16 && !node.exists {
            return Ok(RevocationStatus::Unverifiable);
        }
        if judge(node.kind, node.id, node.tombstone, node.redacted).is_some() {
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
        Ok(
            status_in_tx(&mut self.tx, ctx.namespace(), &seeds, false).await?
                == RevocationStatus::Live,
        )
    }

    /// Registered revocation identity, not permission to consume its content.
    #[doc(hidden)]
    pub async fn registered_revocable_source(
        &mut self,
        ctx: &Context,
        kind: RevocableSourceKind,
        id: &str,
    ) -> Result<bool> {
        identifier(id)?;
        registered_source_in_tx(&mut self.tx, ctx.namespace(), kind, id).await
    }

    /// A conflict isolates new content in this namespace without a new persisted
    /// quarantine format. Accounting and the D4 close path do not call this gate.
    #[doc(hidden)]
    pub async fn ensure_revocation_namespace_live(&mut self, ctx: &Context) -> Result<()> {
        match namespace_collision_in_tx(&mut self.tx, ctx.namespace()).await? {
            Some(id) => Err(namespace_conflict(ctx.namespace(), &id)),
            None => Ok(()),
        }
    }

    /// The node itself: a tombstone of its own kind (run or artifact), or a
    /// redacted body for any kind but a run. A run body is never read here.
    #[doc(hidden)]
    pub async fn revocation_dependency(
        &mut self,
        ctx: &Context,
        kind: &str,
        id: &str,
    ) -> Result<Option<RevocationFinding>> {
        identifier(id)?;
        let row = sqlx::query(
            "SELECT t.body AS tombstone,
                    COALESCE(json_extract(o.body,'$.schema_version')=?4,0) AS redacted
             FROM (SELECT 1) LEFT JOIN objects t
               ON ?1 IN ('run','artifact') AND t.namespace=?2 AND t.kind='tombstone' AND t.id=?3
             LEFT JOIN objects o ON ?1<>'run' AND o.namespace=?2 AND o.kind=?1 AND o.id=?3",
        )
        .bind(kind)
        .bind(ctx.namespace())
        .bind(id)
        .bind(REDACTED_SCHEMA)
        .fetch_one(&mut *self.tx)
        .await
        .map_err(internal)?;
        Ok(judge(
            kind.into(),
            id.into(),
            row.try_get("tombstone").map_err(internal)?,
            row.try_get::<i64, _>("redacted").map_err(internal)? != 0,
        ))
    }

    /// Complete raw closure; intermediate kinds count and cycles/diamonds dedupe.
    /// An isolated namespace is its own refusal, not a finding about a node.
    #[doc(hidden)]
    pub async fn revocation_upstream(
        &mut self,
        ctx: &Context,
        roots: &[(String, String)],
        limit: usize,
    ) -> Result<Option<RevocationFinding>> {
        self.ensure_revocation_namespace_live(ctx).await?;
        let mut seeds = Vec::with_capacity(roots.len());
        for (kind, id) in roots {
            identifier(id)?;
            seeds.push((kind.as_str(), id.as_str()));
        }
        let nodes = upstream_rows_in_tx(&mut self.tx, ctx.namespace(), &seeds, limit, false)
            .await?
            .ok_or_else(|| Error::Conflict(format!(
                "upstream dependency closure exceeds {limit} nodes; it is refused, not truncated"
            )))?;
        Ok(nodes
            .into_iter()
            .find_map(|node| judge(node.kind, node.id, node.tombstone, node.redacted)))
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
