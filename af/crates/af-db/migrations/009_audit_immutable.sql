-- Migration 009: Make audit_log append-only via trigger
-- Prevents UPDATE and DELETE on audit_log rows.

CREATE OR REPLACE FUNCTION prevent_audit_mutation() RETURNS TRIGGER AS $$
BEGIN
    RAISE EXCEPTION 'audit_log is append-only: % not allowed', TG_OP;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_trigger WHERE tgname = 'audit_immutable') THEN
        CREATE TRIGGER audit_immutable
            BEFORE UPDATE OR DELETE ON audit_log
            FOR EACH ROW EXECUTE FUNCTION prevent_audit_mutation();
    END IF;
END;
$$;
