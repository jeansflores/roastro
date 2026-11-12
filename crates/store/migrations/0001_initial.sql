-- The roast record, as decided in "What a Roast record holds" (#7) and
-- "Contract between the backend and the web UI" (#8). Instants are epoch
-- milliseconds (ADR 0001); weights are whole grams; ids are UUIDv7 text.

CREATE TABLE machine (
    id   TEXT PRIMARY KEY,
    name TEXT NOT NULL
) STRICT;

CREATE TABLE coffee (
    id       TEXT PRIMARY KEY,
    name     TEXT NOT NULL,
    notes    TEXT NOT NULL DEFAULT '',
    archived INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1))
) STRICT;

CREATE TABLE roast (
    id               TEXT PRIMARY KEY,
    machine_id       TEXT NOT NULL REFERENCES machine (id),
    number           INTEGER NOT NULL,
    state            TEXT NOT NULL CHECK (state IN ('recording', 'done', 'incomplete')),
    discarded_at     INTEGER,
    coffee_id        TEXT REFERENCES coffee (id),
    roasted_by       TEXT NOT NULL DEFAULT '',
    green_weight_g   INTEGER CHECK (green_weight_g > 0),
    roasted_weight_g INTEGER CHECK (roasted_weight_g > 0),
    reference_id     TEXT REFERENCES roast (id),
    UNIQUE (machine_id, number)
) STRICT;

-- Every register-map point at one instant, as a JSON object by point name.
CREATE TABLE sample (
    roast_id TEXT NOT NULL REFERENCES roast (id),
    at       INTEGER NOT NULL,
    vals     TEXT NOT NULL,
    PRIMARY KEY (roast_id, at)
) STRICT, WITHOUT ROWID;

CREATE TABLE roast_event (
    roast_id    TEXT NOT NULL REFERENCES roast (id),
    kind        TEXT NOT NULL CHECK (kind IN ('charge', 'turning_point', 'dry_end', 'first_crack', 'second_crack', 'drop')),
    at          INTEGER NOT NULL,
    source      TEXT NOT NULL CHECK (source IN ('auto', 'manual', 'computed')),
    original_at INTEGER,
    PRIMARY KEY (roast_id, kind)
) STRICT, WITHOUT ROWID;

CREATE TABLE roast_note (
    id       TEXT PRIMARY KEY,
    roast_id TEXT NOT NULL REFERENCES roast (id),
    at       INTEGER NOT NULL,
    text     TEXT NOT NULL
) STRICT;
CREATE INDEX roast_note_by_roast ON roast_note (roast_id, at);

-- What the operator prepares before Charge: one row, kept across restarts.
CREATE TABLE next_roast (
    id             INTEGER PRIMARY KEY CHECK (id = 1),
    coffee_id      TEXT REFERENCES coffee (id),
    green_weight_g INTEGER CHECK (green_weight_g > 0),
    roasted_by     TEXT NOT NULL DEFAULT '',
    reference_id   TEXT REFERENCES roast (id)
) STRICT;
INSERT INTO next_roast (id) VALUES (1);
