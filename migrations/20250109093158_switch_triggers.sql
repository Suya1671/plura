-- Add migration script here
-- The current fronting member. If null, no member we have is fronting and the raw account messages are used
ALTER TABLE systems
ADD COLUMN active_member_id INTEGER REFERENCES members (id);

-- If a trigger is used, should the active member be changed to the new member?
ALTER TABLE systems
ADD COLUMN trigger_changes_active_member BOOLEAN DEFAULT FALSE NOT NULL;

-- note: prefix and suffix can both happen on the same trigger. It means either will trigger the switch
CREATE TABLE triggers (
    id INTEGER NOT NULL PRIMARY KEY,
    -- The member that will front
    member_id INTEGER NOT NULL REFERENCES members (id),
    -- The prefix that will trigger this member
    prefix TEXT,
    -- The suffix that will trigger this member
    suffix TEXT,
    system_id INTEGER NOT NULL,
    -- Create unique constraints using the system_id from the member table
    CONSTRAINT unique_prefix UNIQUE (system_id, prefix),
    CONSTRAINT unique_suffix UNIQUE (system_id, suffix)
);

-- ensure system id is the same as the system id on member
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
