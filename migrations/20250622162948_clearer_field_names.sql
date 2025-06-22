-- Add migration script here
ALTER TABLE systems
RENAME COLUMN trigger_changes_active_member TO auto_switch_on_trigger;

ALTER TABLE systems
RENAME COLUMN active_member_id TO currently_fronting_member_id;
