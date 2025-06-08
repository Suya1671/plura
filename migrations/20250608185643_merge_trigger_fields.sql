-- Add migration script here
-- Consolidate prefix and suffix into a single trigger field with a boolean to indicate type
-- First, drop the trigger that references the triggers table
DROP TRIGGER IF EXISTS ensure_system_id_update_members_table;

-- Create a new table with the updated schema
CREATE TABLE triggers_new (
    id INTEGER NOT NULL PRIMARY KEY,
    -- The member that will front
    member_id INTEGER NOT NULL REFERENCES members (id),
    -- The trigger text. This will be the prefix or suffix depending on the is_prefix flag
    trigger_text TEXT NOT NULL,
    -- True if this is a prefix trigger, false if suffix
    is_prefix BOOLEAN NOT NULL,
    system_id INTEGER NOT NULL,
    -- Create unique constraints using the system_id and trigger type
    CONSTRAINT unique_trigger UNIQUE (system_id, trigger_text, is_prefix)
);

-- Migrate existing data from the old table
INSERT INTO
    triggers_new (member_id, trigger_text, is_prefix, system_id)
SELECT
    member_id,
    prefix,
    TRUE,
    system_id
FROM
    triggers
WHERE
    prefix IS NOT NULL;

INSERT INTO
    triggers_new (member_id, trigger_text, is_prefix, system_id)
SELECT
    member_id,
    suffix,
    FALSE,
    system_id
FROM
    triggers
WHERE
    suffix IS NOT NULL;

-- Recreate original state/names
DROP TRIGGER IF EXISTS ensure_system_id;

DROP TRIGGER IF EXISTS ensure_system_id_update;

DROP TABLE triggers;

ALTER TABLE triggers_new
RENAME TO triggers;

CREATE TRIGGER ensure_system_id BEFORE INSERT ON triggers FOR EACH ROW BEGIN
SELECT
    RAISE (
        ABORT,
        'system_id must be the same as the system_id on member'
    )
WHERE
    NEW.system_id != (
        SELECT
            system_id
        FROM
            members
        WHERE
            id = NEW.member_id
    );

END;

CREATE TRIGGER ensure_system_id_update BEFORE
UPDATE ON triggers FOR EACH ROW BEGIN
SELECT
    RAISE (
        ABORT,
        'system_id must be the same as the system_id on member'
    )
WHERE
    NEW.system_id != (
        SELECT
            system_id
        FROM
            members
        WHERE
            id = NEW.member_id
    );

END;

CREATE TRIGGER ensure_system_id_update_members_table BEFORE
UPDATE ON members FOR EACH ROW BEGIN
UPDATE triggers
SET
    system_id = NEW.system_id
WHERE
    member_id = NEW.id;

END;
