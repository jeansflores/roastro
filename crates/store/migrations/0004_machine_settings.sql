-- Where the machine is and what it is, as decided in "Machine setup and
-- connection lifecycle" (#34): set up in the app instead of on the command
-- line. A Demo machine (the built-in simulator) keeps its roasts apart. The
-- machine in use is an app setting.

ALTER TABLE machine ADD COLUMN address TEXT NOT NULL DEFAULT '';
ALTER TABLE machine ADD COLUMN model   TEXT NOT NULL DEFAULT '';
ALTER TABLE machine ADD COLUMN demo    INTEGER NOT NULL DEFAULT 0 CHECK (demo IN (0, 1));

CREATE TABLE app_setting (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;
