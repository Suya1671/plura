-- Add migration script here
CREATE TABLE message_logs (
    id INTEGER NOT NULL PRIMARY KEY,
    member_id INTEGER NOT NULL REFERENCES members (id),
    message_id TEXT UNIQUE NOT NULL
) STRICT;
