-- Add migration script here
CREATE TABLE systems (
    id INTEGER NOT NULL PRIMARY KEY,
    owner_id TEXT UNIQUE NOT NULL,
    name TEXT NOT NULL,
    slack_oauth_token TEXT NOT NULL,
    -- unix timestamp
    created_at INTEGER DEFAULT CURRENT_TIMESTAMP NOT NULL
);
