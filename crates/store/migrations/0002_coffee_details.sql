-- Where a coffee comes from, as decided in "Roast history and curve
-- comparison" (#10): its lot, origin, process and supplier. Free text, empty
-- when unknown.

ALTER TABLE coffee ADD COLUMN lot      TEXT NOT NULL DEFAULT '';
ALTER TABLE coffee ADD COLUMN origin   TEXT NOT NULL DEFAULT '';
ALTER TABLE coffee ADD COLUMN process  TEXT NOT NULL DEFAULT '';
ALTER TABLE coffee ADD COLUMN supplier TEXT NOT NULL DEFAULT '';
