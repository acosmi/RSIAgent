-- Extend typed parent storage without changing historical migration checksums.
-- SQLx applies this rebuild and its migration ledger row in one transaction.
-- Preserve rowid: Session::list and existing cursors depend on its order.
CREATE TABLE objects_formal_parent (
 namespace TEXT NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('run','feedback','event','candidate','release','pointer','receipt','dataset','evaluation','budget','reservation','job','improvement','artifact','tombstone','formal_parent')),
 id TEXT NOT NULL,
 owner TEXT NOT NULL,
 body TEXT NOT NULL CHECK(json_valid(body)),
 revision INTEGER NOT NULL DEFAULT 1 CHECK(revision > 0),
 PRIMARY KEY(namespace,kind,id),
 CHECK(json_extract(body,'$.id') = id)
);
INSERT INTO objects_formal_parent(rowid,namespace,kind,id,owner,body,revision)
 SELECT rowid,namespace,kind,id,owner,body,revision FROM objects;
DROP TABLE objects;
ALTER TABLE objects_formal_parent RENAME TO objects;
CREATE INDEX objects_owner ON objects(namespace,kind,owner);
CREATE INDEX jobs_state ON objects(namespace,json_extract(body,'$.state')) WHERE kind='job';
