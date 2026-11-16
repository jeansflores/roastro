-- When the machine stopped answering after having answered, as decided in
-- "Machine setup and connection lifecycle" (#34): the log of link drops.
-- A drop still going on has no end.

CREATE TABLE link_drop (
    id         INTEGER PRIMARY KEY,
    machine_id TEXT NOT NULL REFERENCES machine (id),
    started_at INTEGER NOT NULL,
    ended_at   INTEGER,
    error      TEXT NOT NULL,
    roast_id   TEXT REFERENCES roast (id)
) STRICT;
CREATE INDEX link_drop_by_start ON link_drop (machine_id, started_at);
