-- Thread target artifact: which sample a thread is analyzing.
-- Set when creating a thread from the "Analyze" button.
-- NULL for general-purpose threads (chat, thinking without a specific target).
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'threads' AND column_name = 'target_artifact_id'
    ) THEN
        ALTER TABLE threads ADD COLUMN target_artifact_id UUID REFERENCES artifacts(id) ON DELETE SET NULL;
    END IF;
END $$;
