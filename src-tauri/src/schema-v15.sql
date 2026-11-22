CREATE TABLE IF NOT EXISTS outgoing_staged(
    file_id TEXT PRIMARY KEY,
    draft_id TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS outgoing_staged_draft ON outgoing_staged(draft_id);
CREATE TRIGGER IF NOT EXISTS outgoing_file_deleted AFTER DELETE ON files
WHEN old.message_id='outgoing' BEGIN
    DELETE FROM outgoing_staged WHERE file_id=old.id;
    DELETE FROM outgoing_owners WHERE file_id=old.id;
END;
PRAGMA user_version=15;
