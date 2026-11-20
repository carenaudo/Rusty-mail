CREATE TABLE IF NOT EXISTS cache_size (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    bytes INTEGER NOT NULL CHECK (bytes >= 0)
);
INSERT INTO cache_size VALUES (
    1,
    (SELECT coalesce(sum(length(raw)), 0) FROM mail_raw)
    + (SELECT coalesce(sum(length(data)), 0) FROM files WHERE message_id <> 'outgoing')
) ON CONFLICT(singleton) DO UPDATE SET bytes=excluded.bytes;
CREATE INDEX IF NOT EXISTS mail_raw_eviction_order ON mail_raw(accessed, id);
CREATE INDEX IF NOT EXISTS files_by_message ON files(message_id);

CREATE TRIGGER IF NOT EXISTS cache_raw_insert AFTER INSERT ON mail_raw BEGIN
    UPDATE cache_size SET bytes = bytes + length(new.raw) WHERE singleton = 1;
END;
CREATE TRIGGER IF NOT EXISTS cache_raw_update AFTER UPDATE OF raw ON mail_raw BEGIN
    UPDATE cache_size SET bytes = bytes + length(new.raw) - length(old.raw) WHERE singleton = 1;
END;
CREATE TRIGGER IF NOT EXISTS cache_raw_delete AFTER DELETE ON mail_raw BEGIN
    UPDATE cache_size SET bytes = bytes - length(old.raw) WHERE singleton = 1;
END;

CREATE TRIGGER IF NOT EXISTS cache_file_insert AFTER INSERT ON files WHEN new.message_id <> 'outgoing' BEGIN
    UPDATE cache_size SET bytes = bytes + length(new.data) WHERE singleton = 1;
END;
CREATE TRIGGER IF NOT EXISTS cache_file_update AFTER UPDATE OF data, message_id ON files
WHEN new.message_id <> 'outgoing' OR old.message_id <> 'outgoing' BEGIN
    UPDATE cache_size SET bytes = bytes
      + CASE WHEN new.message_id <> 'outgoing' THEN length(new.data) ELSE 0 END
      - CASE WHEN old.message_id <> 'outgoing' THEN length(old.data) ELSE 0 END
    WHERE singleton = 1;
END;
CREATE TRIGGER IF NOT EXISTS cache_file_delete AFTER DELETE ON files WHEN old.message_id <> 'outgoing' BEGIN
    UPDATE cache_size SET bytes = bytes - length(old.data) WHERE singleton = 1;
END;

PRAGMA user_version=13;
