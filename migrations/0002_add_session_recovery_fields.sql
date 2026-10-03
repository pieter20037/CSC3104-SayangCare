ALTER TABLE sessions
    ADD COLUMN IF NOT EXISTS caller_display_name TEXT,
    ADD COLUMN IF NOT EXISTS latest_sentiment JSONB;