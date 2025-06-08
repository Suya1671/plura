-- Add migration script here
-- Adds an index on systems.owner_id (which also means 1 system per owner. Probably fine)
CREATE UNIQUE INDEX systems_owner_index ON systems (owner_id);
