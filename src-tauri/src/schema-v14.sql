CREATE TABLE IF NOT EXISTS mutation_base (
    id TEXT PRIMARY KEY,
    state TEXT,
    missing INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS rebuild_seen (
    account_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    id TEXT NOT NULL,
    PRIMARY KEY (account_id, scope, id)
);
CREATE TRIGGER IF NOT EXISTS mutation_base_remove AFTER DELETE ON mail BEGIN
    DELETE FROM mutation_base WHERE id=old.id;
END;

-- Legacy cache fields may already contain optimistic changes. Wait for a
-- provider observation before treating those values as confirmed.
INSERT OR IGNORE INTO mutation_base(id)
SELECT m.id FROM mail m JOIN jobs j ON m.id=json_extract(j.payload,'$.id')
WHERE j.kind='mutation' AND j.status IN ('pending','running','uncertain','failed');
UPDATE jobs SET detail='Synchronize this message before retrying'
WHERE kind='mutation' AND status='failed'
  AND EXISTS (SELECT 1 FROM mutation_base b WHERE b.id=json_extract(jobs.payload,'$.id') AND b.state IS NULL);
-- Legacy partial Outlook enumerations did not record the earlier pages.
DELETE FROM sync_state WHERE key LIKE 'delta:%' AND instr(lower(value),'skiptoken=')>0;
PRAGMA user_version=14;
