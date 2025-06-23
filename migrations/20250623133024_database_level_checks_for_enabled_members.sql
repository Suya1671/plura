-- Add migration script here
CREATE TRIGGER check_active_member_is_enabled
BEFORE UPDATE OF currently_fronting_member_id ON systems
FOR EACH ROW
BEGIN
    SELECT
        RAISE(ABORT, 'Cannot update currently_fronting_member_id to a disabled member')
    WHERE EXISTS (
        SELECT 1 FROM members WHERE id = NEW.currently_fronting_member_id AND enabled = FALSE
    );
END;
