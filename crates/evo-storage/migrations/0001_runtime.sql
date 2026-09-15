-- Logical entities share a typed document table. All reads and writes bind namespace.
-- SQLite remains the authority; FTS is a disposable index. Network calls never hold a transaction.
CREATE TABLE objects (
 namespace TEXT NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('run','feedback','event','candidate','release','pointer','receipt','dataset','evaluation','budget','reservation','job','improvement','artifact','tombstone')),
 id TEXT NOT NULL,
 owner TEXT NOT NULL,
 body TEXT NOT NULL CHECK(json_valid(body)),
 revision INTEGER NOT NULL DEFAULT 1 CHECK(revision > 0),
 PRIMARY KEY(namespace,kind,id),
 CHECK(json_extract(body,'$.id') = id)
);
CREATE INDEX objects_owner ON objects(namespace,kind,owner);
CREATE INDEX jobs_state ON objects(namespace,json_extract(body,'$.state')) WHERE kind='job';
CREATE TABLE idempotency (
 namespace TEXT NOT NULL, actor TEXT NOT NULL, operation TEXT NOT NULL,
 request_key TEXT NOT NULL, payload_hash TEXT NOT NULL, subject_id TEXT NOT NULL,
 response TEXT NOT NULL CHECK(json_valid(response)), redacted INTEGER NOT NULL DEFAULT 0 CHECK(redacted IN (0,1)),
 PRIMARY KEY(namespace,actor,operation,request_key)
);
CREATE INDEX idempotency_subject ON idempotency(namespace,subject_id);
CREATE TABLE audit (
 seq INTEGER PRIMARY KEY AUTOINCREMENT,
 namespace TEXT NOT NULL,
 payload TEXT NOT NULL CHECK(json_valid(payload)),
 previous_hash TEXT NOT NULL,
 digest TEXT NOT NULL
);
CREATE INDEX audit_namespace ON audit(namespace,seq);
CREATE VIRTUAL TABLE skill_fts USING fts5(namespace UNINDEXED, candidate_id UNINDEXED, tokens, tokenize='unicode61');
