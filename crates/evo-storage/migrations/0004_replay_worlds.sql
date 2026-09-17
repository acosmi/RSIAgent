-- Replay worlds. Number 0003 is left vacant for the missing increment's 0003_v3_assets.sql.
CREATE TABLE replay_worlds (
 namespace TEXT NOT NULL,
 id TEXT NOT NULL,
 sealed INTEGER NOT NULL CHECK(sealed IN (0,1)),
 manifest TEXT NOT NULL CHECK(json_valid(manifest)),
 PRIMARY KEY(namespace,id)
);
