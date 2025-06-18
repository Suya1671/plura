-- Add migration script here
CREATE TABLE aliases (
    id INTEGER NOT NULL PRIMARY KEY,
    system_id INTEGER NOT NULL REFERENCES systems (id),
    member_id INTEGER NOT NULL REFERENCES members (id),
    alias TEXT NOT NULL,
    -- A system cannot have multiple aliases that are the same
    UNIQUE (system_id, alias),
    -- A member cannot have multiple aliases that are the same
    UNIQUE (member_id, alias)
) STRICT;
