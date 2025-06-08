-- Add migration script here
CREATE TABLE system_oauth_process (
    id INTEGER NOT NULL PRIMARY KEY,
    owner_id TEXT UNIQUE NOT NULL,
    name TEXT NOT NULL,
    csrf TEXT NOT NULL
);
