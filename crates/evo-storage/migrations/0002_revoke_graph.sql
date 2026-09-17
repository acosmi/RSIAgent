-- Typed dependency edges and a namespace revoke watermark. Next number after 0001.
-- Do not add 0003_exploration_worlds.sql; 0003 was reserved by an unmerged increment package.
CREATE TABLE dependencies (
 namespace TEXT NOT NULL,
 src_kind TEXT NOT NULL,
 src_id TEXT NOT NULL,
 dst_kind TEXT NOT NULL,
 dst_id TEXT NOT NULL,
 PRIMARY KEY(namespace, src_kind, src_id, dst_kind, dst_id)
);
CREATE INDEX dependencies_dst ON dependencies(namespace, dst_kind, dst_id);
CREATE TABLE revoke_watermark (
 namespace TEXT NOT NULL PRIMARY KEY,
 seq INTEGER NOT NULL CHECK(seq >= 0),
 digest TEXT NOT NULL
);
