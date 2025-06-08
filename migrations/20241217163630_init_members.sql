-- Add migration script here
CREATE TABLE members (
    id INTEGER NOT NULL PRIMARY KEY,
    -- shown in extended info
    full_name TEXT NOT NULL,
    -- shown on messages
    display_name TEXT NOT NULL,
    -- shown on messages
    profile_picture_url TEXT,
    -- shown in extended info
    title TEXT,
    -- shown in extended info
    pronouns TEXT,
    -- shown in extended info
    name_pronunciation TEXT,
    -- shown in extended info
    name_recording_url TEXT,
    system_id INTEGER NOT NULL,
    -- unix timestamp
    created_at INTEGER DEFAULT CURRENT_TIMESTAMP NOT NULL,
    FOREIGN KEY (system_id) REFERENCES systems (id)
);
