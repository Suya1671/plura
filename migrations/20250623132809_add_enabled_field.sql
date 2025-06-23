-- Add migration script here
ALTER TABLE members ADD COLUMN enabled BOOLEAN NOT NULL DEFAULT TRUE;
