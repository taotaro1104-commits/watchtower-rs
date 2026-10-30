-- 通知の送信待ちキュー。target は資格情報を含む通知 URL (watches.config_json と同じ扱い)。
CREATE TABLE outbox (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    watch_id        TEXT REFERENCES watches(id) ON DELETE CASCADE,
    target          TEXT NOT NULL,
    title           TEXT NOT NULL,
    body            TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'failed')),
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL,
    last_error      TEXT,
    created_at      INTEGER NOT NULL
);
CREATE INDEX idx_outbox_due ON outbox(status, next_attempt_at);
