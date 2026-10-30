CREATE TABLE watches (
    id          TEXT PRIMARY KEY,
    url         TEXT NOT NULL,
    title       TEXT,
    paused      INTEGER NOT NULL DEFAULT 0,
    tags_json   TEXT NOT NULL DEFAULT '[]',
    config_json TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE watch_state (
    watch_id        TEXT PRIMARY KEY REFERENCES watches(id) ON DELETE CASCADE,
    next_check_at   INTEGER,
    last_checked_at INTEGER,
    last_changed_at INTEGER,
    last_error      TEXT,
    error_count     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_watch_state_next ON watch_state(next_check_at);

CREATE TABLE snapshots (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    watch_id TEXT NOT NULL REFERENCES watches(id) ON DELETE CASCADE,
    taken_at INTEGER NOT NULL,
    hash     BLOB NOT NULL,
    path     TEXT NOT NULL,
    size     INTEGER NOT NULL
);
CREATE INDEX idx_snapshots_watch_time ON snapshots(watch_id, taken_at);

CREATE TABLE tags (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    config_json TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE settings (
    key        TEXT PRIMARY KEY,
    value_json TEXT NOT NULL
);

CREATE TABLE events (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    watch_id TEXT,
    kind     TEXT NOT NULL,
    at       INTEGER NOT NULL,
    detail   TEXT
);
