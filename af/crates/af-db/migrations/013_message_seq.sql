-- Migration 013: Monotonic message ordering
--
-- Adds a sequence-backed column to the messages table for strict total ordering.
-- Parallel agent inserts get distinct sequence numbers from the global sequence,
-- eliminating ambiguity when created_at timestamps collide.

DO $$ BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'messages' AND column_name = 'seq'
    ) THEN
        CREATE SEQUENCE messages_seq_counter;
        ALTER TABLE messages ADD COLUMN seq BIGINT;

        -- Backfill existing rows in chronological order
        WITH ordered AS (
            SELECT id, ROW_NUMBER() OVER (ORDER BY created_at, id) AS rn
            FROM messages
        )
        UPDATE messages SET seq = ordered.rn
        FROM ordered WHERE messages.id = ordered.id;

        -- Advance sequence past existing values
        PERFORM setval('messages_seq_counter', COALESCE((SELECT MAX(seq) FROM messages), 0));

        -- Set default and NOT NULL after backfill
        ALTER TABLE messages ALTER COLUMN seq SET DEFAULT nextval('messages_seq_counter');
        ALTER TABLE messages ALTER COLUMN seq SET NOT NULL;
    END IF;
END $$;
