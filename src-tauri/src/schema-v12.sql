CREATE TABLE IF NOT EXISTS saved_drafts(
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL,
    payload TEXT NOT NULL,
    updated INTEGER NOT NULL,
    queued_job_id TEXT UNIQUE
);
CREATE INDEX IF NOT EXISTS saved_drafts_recent ON saved_drafts(updated DESC,id DESC) WHERE queued_job_id IS NULL;
CREATE INDEX IF NOT EXISTS saved_drafts_account_recent ON saved_drafts(account_id,updated DESC,id DESC) WHERE queued_job_id IS NULL;
INSERT INTO saved_drafts(id,account_id,payload,updated)
SELECT lower(hex(randomblob(16))),account_id,payload,unixepoch()*1000 FROM live_drafts;
DELETE FROM live_drafts;
PRAGMA user_version=12;
