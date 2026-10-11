//! Read-only, namespace/source-bound direct dependency pages. Multiple pages in
//! one Session share its transaction snapshot. A cursor reused in another
//! Session supplies a keyset position, not a snapshot or an authorization grant.

use crate::{Session, internal};
use evo_core::{Context, Error, Result, Role, identifier};
use serde::{Deserialize, Serialize};
use sqlx::Row;

pub const MAX_DIRECT_EDGE_PAGE: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectEdge {
    pub dst_kind: String,
    pub dst_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectEdgeCursor {
    namespace: String,
    src_kind: String,
    src_id: String,
    after: DirectEdge,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectEdgePage {
    pub edges: Vec<DirectEdge>,
    pub next_cursor: Option<DirectEdgeCursor>,
    pub exhausted: bool,
    /// Actual SQL rows fetched, including the optional lookahead row. A row
    /// used for lookahead may be fetched again on the next page and counts twice.
    pub rows_read: usize,
}

impl Session {
    /// Exact direct outgoing rows, ordered by `(dst_kind,dst_id)`. `limit` is
    /// 1..=256; SQL fetches at most `limit+1` rows. No recursive graph read or
    /// object read is performed. Unknown but syntactically valid kinds are kept
    /// exact: this interface does not conflate a blob with an artifact.
    pub async fn direct_edges_page(
        &mut self,
        ctx: &Context,
        src_kind: &str,
        src_id: &str,
        cursor: Option<&DirectEdgeCursor>,
        limit: usize,
    ) -> Result<DirectEdgePage> {
        ctx.require(&[Role::Admin])?;
        identifier(src_kind)?;
        identifier(src_id)?;
        let fetch_limit = limit
            .checked_add(1)
            .filter(|_| (1..=MAX_DIRECT_EDGE_PAGE).contains(&limit))
            .and_then(|value| i64::try_from(value).ok())
            .ok_or_else(|| Error::Invalid("invalid direct edge page limit".into()))?;
        if let Some(cursor) = cursor {
            identifier(&cursor.namespace)?;
            identifier(&cursor.src_kind)?;
            identifier(&cursor.src_id)?;
            identifier(&cursor.after.dst_kind)?;
            identifier(&cursor.after.dst_id)?;
            if cursor.namespace != ctx.namespace()
                || cursor.src_kind != src_kind
                || cursor.src_id != src_id
            {
                return Err(Error::Invalid("direct edge cursor binding mismatch".into()));
            }
        }
        let rows = if let Some(cursor) = cursor {
            sqlx::query("SELECT dst_kind,dst_id FROM dependencies WHERE namespace=? AND src_kind=? AND src_id=? AND (dst_kind,dst_id)>(?,?) ORDER BY dst_kind,dst_id LIMIT ?")
                .bind(ctx.namespace())
                .bind(src_kind)
                .bind(src_id)
                .bind(&cursor.after.dst_kind)
                .bind(&cursor.after.dst_id)
                .bind(fetch_limit)
                .fetch_all(&mut *self.tx)
                .await
                .map_err(internal)?
        } else {
            sqlx::query("SELECT dst_kind,dst_id FROM dependencies WHERE namespace=? AND src_kind=? AND src_id=? ORDER BY dst_kind,dst_id LIMIT ?")
                .bind(ctx.namespace())
                .bind(src_kind)
                .bind(src_id)
                .bind(fetch_limit)
                .fetch_all(&mut *self.tx)
                .await
                .map_err(internal)?
        };
        let rows_read = rows.len();
        let exhausted = rows_read <= limit;
        let mut edges = Vec::with_capacity(rows_read.min(limit));
        for row in rows.into_iter().take(limit) {
            edges.push(DirectEdge {
                dst_kind: row.try_get("dst_kind").map_err(internal)?,
                dst_id: row.try_get("dst_id").map_err(internal)?,
            });
        }
        let next_cursor = if exhausted {
            None
        } else {
            Some(DirectEdgeCursor {
                namespace: ctx.namespace().into(),
                src_kind: src_kind.into(),
                src_id: src_id.into(),
                // A non-exhausted page necessarily contains at least one row.
                after: edges[edges.len() - 1].clone(),
            })
        };
        Ok(DirectEdgePage {
            edges,
            next_cursor,
            exhausted,
            rows_read,
        })
    }
}
