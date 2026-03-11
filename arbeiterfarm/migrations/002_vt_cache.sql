CREATE TABLE IF NOT EXISTS re.vt_cache (
    sha256      TEXT PRIMARY KEY,
    response    JSONB NOT NULL,
    positives   INT,
    total       INT,
    fetched_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_af_re_vt_cache_fetched ON re.vt_cache(fetched_at);
