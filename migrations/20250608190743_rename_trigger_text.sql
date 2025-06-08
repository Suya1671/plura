-- Add migration script here
ALTER TABLE triggers
RENAME COLUMN trigger_text TO text;
