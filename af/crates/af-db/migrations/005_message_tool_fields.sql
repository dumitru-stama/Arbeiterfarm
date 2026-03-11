-- Add tool_call_id and tool_name to messages for thread resumption.
-- Required by OpenAI/Anthropic APIs: tool result messages must reference their tool_call_id.

ALTER TABLE messages ADD COLUMN IF NOT EXISTS tool_call_id TEXT;
ALTER TABLE messages ADD COLUMN IF NOT EXISTS tool_name TEXT;
