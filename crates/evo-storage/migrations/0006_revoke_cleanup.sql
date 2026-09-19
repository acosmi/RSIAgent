-- Resumable cleanup progress derived from the existing tombstone, watermark and typed edges.
CREATE TABLE revoke_cleanup_jobs (
 namespace TEXT NOT NULL,
 job_id TEXT NOT NULL,
 source_kind TEXT NOT NULL,
 source_id TEXT NOT NULL,
 watermark_seq INTEGER NOT NULL CHECK(watermark_seq > 0),
 watermark_digest TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('pending','running','complete','failed')),
 processed_nodes INTEGER NOT NULL DEFAULT 0 CHECK(processed_nodes >= 0),
 last_error TEXT,
 created_at INTEGER NOT NULL CHECK(created_at >= 0),
 updated_at INTEGER NOT NULL CHECK(updated_at >= 0),
 PRIMARY KEY(namespace,job_id),
 UNIQUE(namespace,source_kind,source_id,watermark_seq)
);

CREATE TABLE revoke_cleanup_frontier (
 seq INTEGER PRIMARY KEY AUTOINCREMENT,
 namespace TEXT NOT NULL,
 job_id TEXT NOT NULL,
 node_kind TEXT NOT NULL,
 node_id TEXT NOT NULL,
 cursor_src_kind TEXT,
 cursor_src_id TEXT,
 expanded INTEGER NOT NULL DEFAULT 0 CHECK(expanded IN (0,1)),
 FOREIGN KEY(namespace,job_id)
   REFERENCES revoke_cleanup_jobs(namespace,job_id) ON DELETE CASCADE,
 UNIQUE(namespace,job_id,node_kind,node_id),
 CHECK((cursor_src_kind IS NULL AND cursor_src_id IS NULL)
    OR (cursor_src_kind IS NOT NULL AND cursor_src_id IS NOT NULL))
);
CREATE INDEX revoke_cleanup_frontier_next
 ON revoke_cleanup_frontier(namespace,job_id,expanded,seq);

CREATE TABLE revoke_cleanup_events (
 seq INTEGER PRIMARY KEY AUTOINCREMENT,
 namespace TEXT NOT NULL,
 job_id TEXT NOT NULL,
 node_kind TEXT,
 node_id TEXT,
 event_kind TEXT NOT NULL,
 event_at INTEGER NOT NULL CHECK(event_at >= 0),
 details TEXT NOT NULL CHECK(json_valid(details)),
 FOREIGN KEY(namespace,job_id)
   REFERENCES revoke_cleanup_jobs(namespace,job_id) ON DELETE CASCADE
);
CREATE INDEX revoke_cleanup_events_job
 ON revoke_cleanup_events(namespace,job_id,seq);
