//! Transactional typed documents, idempotency, audit and disposable FTS5 indexes.
//! This crate never connects to a model or executes candidate content.
pub mod budget;
pub mod lifecycle;

use evo_core::{Context, Error, Result, fingerprint, hash, identifier, now, search_tokens};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sqlx::{
    ConnectOptions, Row, Sqlite, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
    root: PathBuf,
    blob_lock: Arc<RwLock<()>>,
}
pub struct Session {
    tx: Transaction<'static, Sqlite>,
}
fn internal(e: impl std::fmt::Display) -> Error {
    tracing::error!(error=%e,"database operation failed");
    Error::Internal
}
impl Store {
    pub async fn open(path: &Path) -> Result<Self> {
        let root = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        tokio::fs::create_dir_all(&root).await.map_err(internal)?;
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .busy_timeout(Duration::from_secs(5))
            .disable_statement_logging();
        // One connection serializes state transitions, including read/modify/write budget transactions.
        // The CLI also takes an OS file lock: separate daemons must not share a data directory.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .map_err(internal)?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(internal)?;
        Ok(Self {
            pool,
            root,
            blob_lock: Arc::new(RwLock::new(())),
        })
    }
    pub async fn session(&self) -> Result<Session> {
        Ok(Session {
            tx: self.pool.begin().await.map_err(internal)?,
        })
    }
    pub async fn close(&self) {
        self.pool.close().await;
    }
    pub async fn integrity(&self) -> Result<String> {
        sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&self.pool)
            .await
            .map_err(internal)
    }
    pub async fn backup(&self, destination: &Path) -> Result<()> {
        crate::lifecycle::backup_consistent(self, destination).await
    }
    /// Caller must register the returned digest and owner through a trusted host operation.
    pub async fn put_blob(&self, ctx: &Context, bytes: &[u8]) -> Result<String> {
        ctx.require(&[evo_core::Role::Host, evo_core::Role::Admin])?;
        if bytes.is_empty() || bytes.len() > 1024 * 1024 {
            return Err(Error::Invalid("blob must be 1..=1048576 bytes".into()));
        }
        let _guard = self.blob_lock.write().await;
        let digest = hash(bytes);
        let dir = self
            .root
            .join("blobs")
            .join(hash(ctx.namespace().as_bytes()));
        tokio::fs::create_dir_all(&dir).await.map_err(internal)?;
        let temp = dir.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        tokio::fs::write(&temp, bytes).await.map_err(internal)?;
        tokio::fs::rename(&temp, dir.join(&digest))
            .await
            .map_err(internal)?;
        Ok(digest)
    }
    pub async fn delete_blob(&self, ctx: &Context, digest: &str) -> Result<()> {
        ctx.require(&[evo_core::Role::Admin])?;
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::Invalid("invalid content digest".into()));
        }
        let _guard = self.blob_lock.write().await;
        let path = self
            .root
            .join("blobs")
            .join(hash(ctx.namespace().as_bytes()))
            .join(digest);
        match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(internal(e)),
        }
    }
    pub async fn verify_audit(&self, ctx: &Context) -> Result<usize> {
        ctx.require(&[evo_core::Role::Admin, evo_core::Role::Evaluator])?;
        let rows = sqlx::query(
            "SELECT payload,previous_hash,digest FROM audit WHERE namespace=? ORDER BY seq",
        )
        .bind(ctx.namespace())
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        let mut previous = String::new();
        for row in &rows {
            let payload: String = row.try_get("payload").map_err(internal)?;
            let prev: String = row.try_get("previous_hash").map_err(internal)?;
            let digest: String = row.try_get("digest").map_err(internal)?;
            if prev != previous || digest != hash(format!("{prev}\n{payload}").as_bytes()) {
                return Err(Error::Conflict("audit chain mismatch".into()));
            }
            previous = digest;
        }
        Ok(rows.len())
    }
}
impl Session {
    pub async fn get<T: DeserializeOwned>(
        &mut self,
        ctx: &Context,
        kind: &str,
        id: &str,
    ) -> Result<Option<T>> {
        identifier(id)?;
        let body: Option<String> =
            sqlx::query_scalar("SELECT body FROM objects WHERE namespace=? AND kind=? AND id=?")
                .bind(ctx.namespace())
                .bind(kind)
                .bind(id)
                .fetch_optional(&mut *self.tx)
                .await
                .map_err(internal)?;
        body.map(|b| serde_json::from_str(&b).map_err(internal))
            .transpose()
    }
    pub async fn need<T: DeserializeOwned>(
        &mut self,
        ctx: &Context,
        kind: &str,
        id: &str,
    ) -> Result<T> {
        self.get(ctx, kind, id).await?.ok_or(Error::NotFound)
    }
    pub async fn list<T: DeserializeOwned>(&mut self, ctx: &Context, kind: &str) -> Result<Vec<T>> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT body FROM objects WHERE namespace=? AND kind=? ORDER BY rowid LIMIT 10001",
        )
        .bind(ctx.namespace())
        .bind(kind)
        .fetch_all(&mut *self.tx)
        .await
        .map_err(internal)?;
        if rows.len() > 10000 {
            return Err(Error::Conflict("scope collection exceeds bounded scan; archive or add paginated query before continuing".into()));
        }
        rows.into_iter()
            .map(|r| serde_json::from_str(&r).map_err(internal))
            .collect()
    }
    pub async fn put<T: Serialize>(
        &mut self,
        ctx: &Context,
        kind: &str,
        id: &str,
        owner: &str,
        body: &T,
    ) -> Result<()> {
        identifier(id)?;
        identifier(owner)?;
        let body = serde_json::to_string(body).map_err(internal)?;
        if body.len() > 4 * 1024 * 1024 {
            return Err(Error::Invalid("object exceeds 4 MiB".into()));
        }
        sqlx::query("INSERT INTO objects(namespace,kind,id,owner,body) VALUES(?,?,?,?,?) ON CONFLICT(namespace,kind,id) DO UPDATE SET owner=excluded.owner,body=excluded.body,revision=objects.revision+1")
   .bind(ctx.namespace()).bind(kind).bind(id).bind(owner).bind(body).execute(&mut *self.tx).await.map_err(internal)?;
        Ok(())
    }
    pub async fn delete(&mut self, ctx: &Context, kind: &str, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM objects WHERE namespace=? AND kind=? AND id=?")
            .bind(ctx.namespace())
            .bind(kind)
            .bind(id)
            .execute(&mut *self.tx)
            .await
            .map_err(internal)?;
        Ok(())
    }
    pub async fn cached<T: DeserializeOwned, A: Serialize>(
        &mut self,
        ctx: &Context,
        op: &str,
        key: &str,
        args: &A,
    ) -> Result<Option<T>> {
        identifier(key)?;
        let row=sqlx::query("SELECT payload_hash,response,redacted FROM idempotency WHERE namespace=? AND actor=? AND operation=? AND request_key=?")
   .bind(ctx.namespace()).bind(ctx.actor()).bind(op).bind(key).fetch_optional(&mut *self.tx).await.map_err(internal)?;
        let Some(row) = row else { return Ok(None) };
        let expected: String = row.try_get("payload_hash").map_err(internal)?;
        if expected != fingerprint(args)? {
            return Err(Error::Conflict(
                "idempotency key reused with different content".into(),
            ));
        }
        if row.try_get::<i64, _>("redacted").map_err(internal)? != 0 {
            return Err(Error::Conflict(
                "subject was deleted; request cannot be replayed".into(),
            ));
        }
        let body: String = row.try_get("response").map_err(internal)?;
        Ok(Some(serde_json::from_str(&body).map_err(internal)?))
    }
    pub async fn cache<A: Serialize, T: Serialize>(
        &mut self,
        ctx: &Context,
        op: &str,
        key: &str,
        args: &A,
        subject: &str,
        result: &T,
    ) -> Result<()> {
        sqlx::query("INSERT INTO idempotency(namespace,actor,operation,request_key,payload_hash,subject_id,response) VALUES(?,?,?,?,?,?,?)")
   .bind(ctx.namespace()).bind(ctx.actor()).bind(op).bind(key).bind(fingerprint(args)?).bind(subject).bind(serde_json::to_string(result).map_err(internal)?)
   .execute(&mut *self.tx).await.map_err(internal)?;
        Ok(())
    }
    pub async fn redact_cache(&mut self, ctx: &Context, subject: &str) -> Result<()> {
        sqlx::query(
            "UPDATE idempotency SET response='null',redacted=1 WHERE namespace=? AND subject_id=?",
        )
        .bind(ctx.namespace())
        .bind(subject)
        .execute(&mut *self.tx)
        .await
        .map_err(internal)?;
        Ok(())
    }
    pub async fn audit(&mut self, ctx: &Context, operation: &str, entity: &str) -> Result<()> {
        let prev: Option<String> = sqlx::query_scalar(
            "SELECT digest FROM audit WHERE namespace=? ORDER BY seq DESC LIMIT 1",
        )
        .bind(ctx.namespace())
        .fetch_optional(&mut *self.tx)
        .await
        .map_err(internal)?;
        let prev = prev.unwrap_or_default();
        let payload=json!({"actor":ctx.actor(),"role":ctx.role(),"operation":operation,"entity":entity,"timestamp":now()}).to_string();
        let digest = hash(format!("{prev}\n{payload}").as_bytes());
        sqlx::query("INSERT INTO audit(namespace,payload,previous_hash,digest) VALUES(?,?,?,?)")
            .bind(ctx.namespace())
            .bind(payload)
            .bind(prev)
            .bind(digest)
            .execute(&mut *self.tx)
            .await
            .map_err(internal)?;
        Ok(())
    }
    pub async fn replace_search_index(
        &mut self,
        ctx: &Context,
        entries: &[(String, String)],
    ) -> Result<()> {
        sqlx::query("DELETE FROM skill_fts WHERE namespace=?")
            .bind(ctx.namespace())
            .execute(&mut *self.tx)
            .await
            .map_err(internal)?;
        for (id, content) in entries {
            let tokens = search_tokens(content).join(" ");
            sqlx::query("INSERT INTO skill_fts(namespace,candidate_id,tokens) VALUES(?,?,?)")
                .bind(ctx.namespace())
                .bind(id)
                .bind(tokens)
                .execute(&mut *self.tx)
                .await
                .map_err(internal)?;
        }
        Ok(())
    }
    pub async fn search(&mut self, ctx: &Context, query: &str) -> Result<Vec<String>> {
        let tokens = search_tokens(query);
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let expression = tokens
            .iter()
            .take(32)
            .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");
        sqlx::query_scalar("SELECT candidate_id FROM skill_fts WHERE skill_fts MATCH ? AND namespace=? ORDER BY bm25(skill_fts),candidate_id LIMIT 64").bind(expression).bind(ctx.namespace()).fetch_all(&mut *self.tx).await.map_err(internal)
    }
    pub async fn put_edge(
        &mut self,
        ctx: &Context,
        src_kind: &str,
        src_id: &str,
        dst_kind: &str,
        dst_id: &str,
    ) -> Result<()> {
        identifier(src_id)?;
        identifier(dst_id)?;
        sqlx::query("INSERT OR IGNORE INTO dependencies(namespace,src_kind,src_id,dst_kind,dst_id) VALUES(?,?,?,?,?)")
            .bind(ctx.namespace())
            .bind(src_kind)
            .bind(src_id)
            .bind(dst_kind)
            .bind(dst_id)
            .execute(&mut *self.tx)
            .await
            .map_err(internal)?;
        Ok(())
    }

    pub async fn dependents(
        &mut self,
        ctx: &Context,
        dst_kind: &str,
        dst_id: &str,
    ) -> Result<Vec<(String, String)>> {
        identifier(dst_id)?;
        let rows = sqlx::query("SELECT src_kind,src_id FROM dependencies WHERE namespace=? AND dst_kind=? AND dst_id=?")
            .bind(ctx.namespace())
            .bind(dst_kind)
            .bind(dst_id)
            .fetch_all(&mut *self.tx)
            .await
            .map_err(internal)?;
        let mut out = Vec::new();
        for row in rows {
            out.push((
                row.try_get("src_kind").map_err(internal)?,
                row.try_get("src_id").map_err(internal)?,
            ));
        }
        Ok(out)
    }

    pub async fn bump_watermark(&mut self, ctx: &Context, digest: &str) -> Result<i64> {
        identifier(digest)?;
        let prev: i64 = sqlx::query_scalar("SELECT seq FROM revoke_watermark WHERE namespace=?")
            .bind(ctx.namespace())
            .fetch_optional(&mut *self.tx)
            .await
            .map_err(internal)?
            .unwrap_or(0);
        let seq = prev + 1;
        sqlx::query("INSERT INTO revoke_watermark(namespace,seq,digest) VALUES(?,?,?) ON CONFLICT(namespace) DO UPDATE SET seq=excluded.seq,digest=excluded.digest")
            .bind(ctx.namespace())
            .bind(seq)
            .bind(digest)
            .execute(&mut *self.tx)
            .await
            .map_err(internal)?;
        Ok(seq)
    }

    pub async fn watermark(&mut self, ctx: &Context) -> Result<Option<(i64, String)>> {
        let row = sqlx::query("SELECT seq,digest FROM revoke_watermark WHERE namespace=?")
            .bind(ctx.namespace())
            .fetch_optional(&mut *self.tx)
            .await
            .map_err(internal)?;
        match row {
            Some(r) => Ok(Some((
                r.try_get("seq").map_err(internal)?,
                r.try_get("digest").map_err(internal)?,
            ))),
            None => Ok(None),
        }
    }

    pub async fn put_world(
        &mut self,
        ctx: &Context,
        id: &str,
        sealed: bool,
        manifest: &Value,
    ) -> Result<()> {
        identifier(id)?;
        sqlx::query("INSERT INTO replay_worlds(namespace,id,sealed,manifest) VALUES(?,?,?,?) ON CONFLICT(namespace,id) DO UPDATE SET sealed=excluded.sealed,manifest=excluded.manifest")
            .bind(ctx.namespace())
            .bind(id)
            .bind(i64::from(sealed))
            .bind(manifest.to_string())
            .execute(&mut *self.tx)
            .await
            .map_err(internal)?;
        Ok(())
    }

    pub async fn get_world(&mut self, ctx: &Context, id: &str) -> Result<Option<(bool, Value)>> {
        identifier(id)?;
        let row =
            sqlx::query("SELECT sealed,manifest FROM replay_worlds WHERE namespace=? AND id=?")
                .bind(ctx.namespace())
                .bind(id)
                .fetch_optional(&mut *self.tx)
                .await
                .map_err(internal)?;
        match row {
            Some(r) => {
                let sealed: i64 = r.try_get("sealed").map_err(internal)?;
                let manifest: String = r.try_get("manifest").map_err(internal)?;
                Ok(Some((
                    sealed != 0,
                    serde_json::from_str(&manifest).map_err(internal)?,
                )))
            }
            None => Ok(None),
        }
    }

    pub async fn commit(self) -> Result<()> {
        self.tx.commit().await.map_err(internal)
    }
    pub async fn raw_object(&mut self, ctx: &Context, kind: &str, id: &str) -> Result<Value> {
        self.need(ctx, kind, id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use evo_core::Role;
    async fn db() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(&d.path().join("db.sqlite3")).await.unwrap();
        (d, s)
    }
    #[tokio::test]
    async fn rollback_is_atomic() {
        let (_d, s) = db().await;
        let c = Context::new("n", "a", Role::Admin).unwrap();
        {
            let mut t = s.session().await.unwrap();
            t.put(&c, "run", "r", "a", &json!({"id":"r"}))
                .await
                .unwrap();
        }
        let mut t = s.session().await.unwrap();
        assert!(t.get::<Value>(&c, "run", "r").await.unwrap().is_none());
    }
    #[tokio::test]
    async fn namespaces_never_cross() {
        let (_d, s) = db().await;
        let a = Context::new("a", "u", Role::Admin).unwrap();
        let b = Context::new("b", "u", Role::Admin).unwrap();
        let mut t = s.session().await.unwrap();
        t.put(&a, "run", "r", "u", &json!({"id":"r"}))
            .await
            .unwrap();
        t.commit().await.unwrap();
        let mut t = s.session().await.unwrap();
        assert!(t.get::<Value>(&b, "run", "r").await.unwrap().is_none());
    }
    #[tokio::test]
    async fn caches_detect_conflict_and_redaction() {
        let (_d, s) = db().await;
        let c = Context::new("n", "a", Role::Admin).unwrap();
        let mut t = s.session().await.unwrap();
        t.cache(&c, "op", "k", &1, "r", &2).await.unwrap();
        assert_eq!(
            t.cached::<i32, _>(&c, "op", "k", &1).await.unwrap(),
            Some(2)
        );
        assert!(t.cached::<i32, _>(&c, "op", "k", &3).await.is_err());
        t.redact_cache(&c, "r").await.unwrap();
        assert!(t.cached::<i32, _>(&c, "op", "k", &1).await.is_err());
    }
    #[tokio::test]
    async fn fts_chinese_and_injection() {
        let (_d, s) = db().await;
        let c = Context::new("n", "a", Role::Admin).unwrap();
        let mut t = s.session().await.unwrap();
        t.replace_search_index(&c, &[("x".into(), "检查配置路径 config_file".into())])
            .await
            .unwrap();
        assert_eq!(t.search(&c, "配置").await.unwrap(), vec!["x"]);
        assert!(t.search(&c, "\" OR * ; DROP TABLE objects").await.is_ok());
    }
    #[tokio::test]
    async fn audit_and_backup_survive_restart() {
        let (d, s) = db().await;
        let c = Context::new("n", "a", Role::Admin).unwrap();
        let mut t = s.session().await.unwrap();
        t.audit(&c, "test", "x").await.unwrap();
        t.bump_watermark(&c, &hash(b"initial-watermark"))
            .await
            .unwrap();
        t.commit().await.unwrap();
        assert_eq!(s.verify_audit(&c).await.unwrap(), 1);
        let backup = d.path().join("backup");
        s.backup(&backup).await.unwrap();
        let restored = Store::open(&backup.join("rsia.sqlite3")).await.unwrap();
        assert_eq!(restored.verify_audit(&c).await.unwrap(), 1);
        assert_eq!(restored.integrity().await.unwrap(), "ok");
    }
    #[tokio::test]
    async fn revoke_edge_blocks_dependents_and_watermark_is_required() {
        let (_d, s) = db().await;
        let c = Context::new("n", "a", Role::Admin).unwrap();
        let mut t = s.session().await.unwrap();
        t.put_edge(&c, "job", "j1", "run", "run2").await.unwrap();
        t.commit().await.unwrap();
        let mut t = s.session().await.unwrap();
        let deps = t.dependents(&c, "run", "run2").await.unwrap();
        assert_eq!(deps, vec![("job".into(), "j1".into())]);
        let seq = t.bump_watermark(&c, "wm1").await.unwrap();
        assert_eq!(seq, 1);
        t.commit().await.unwrap();
        let mut t = s.session().await.unwrap();
        assert!(t.watermark(&c).await.unwrap().is_some());
        t.commit().await.unwrap();
        let mut other = s.session().await.unwrap();
        let b = Context::new("other", "a", Role::Admin).unwrap();
        assert!(other.watermark(&b).await.unwrap().is_none());
    }
    #[tokio::test]
    async fn sealed_world_roundtrip() {
        let (_d, s) = db().await;
        let c = Context::new("n", "a", Role::Admin).unwrap();
        let mut t = s.session().await.unwrap();
        t.put_world(&c, "w1", true, &json!({"k":"v"}))
            .await
            .unwrap();
        t.commit().await.unwrap();
        let mut t = s.session().await.unwrap();
        let got = t.get_world(&c, "w1").await.unwrap().unwrap();
        assert!(got.0);
        assert_eq!(got.1["k"], "v");
    }
}
