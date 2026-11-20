-- Counters are derived atomically by SQLite, including provider deletes and moves.
CREATE TABLE mail_counts(
 scope INTEGER NOT NULL, account_id TEXT NOT NULL, folder TEXT NOT NULL,
 total INTEGER NOT NULL CHECK(total>=0), PRIMARY KEY(scope,account_id,folder)
) WITHOUT ROWID;
INSERT INTO mail_counts SELECT 0,'','',count(*) FROM mail;
INSERT INTO mail_counts SELECT 1,account_id,'',count(*) FROM mail GROUP BY account_id;
INSERT INTO mail_counts SELECT 2,'',folder,count(*) FROM mail GROUP BY folder;
INSERT INTO mail_counts SELECT 3,account_id,folder,count(*) FROM mail GROUP BY account_id,folder;
CREATE TRIGGER mail_counts_insert AFTER INSERT ON mail BEGIN
 INSERT INTO mail_counts VALUES(0,'','',1),(1,new.account_id,'',1),(2,'',new.folder,1),(3,new.account_id,new.folder,1)
 ON CONFLICT(scope,account_id,folder) DO UPDATE SET total=total+1;
END;
CREATE TRIGGER mail_counts_delete AFTER DELETE ON mail BEGIN
 UPDATE mail_counts SET total=total-1 WHERE
 (scope=0 AND account_id='' AND folder='') OR
 (scope=1 AND account_id=old.account_id AND folder='') OR
 (scope=2 AND account_id='' AND folder=old.folder) OR
 (scope=3 AND account_id=old.account_id AND folder=old.folder);
END;
CREATE TRIGGER mail_counts_move AFTER UPDATE OF account_id,folder ON mail
WHEN old.account_id!=new.account_id OR old.folder!=new.folder BEGIN
 UPDATE mail_counts SET total=total-1 WHERE
 (scope=0 AND account_id='' AND folder='') OR
 (scope=1 AND account_id=old.account_id AND folder='') OR
 (scope=2 AND account_id='' AND folder=old.folder) OR
 (scope=3 AND account_id=old.account_id AND folder=old.folder);
 INSERT INTO mail_counts VALUES(0,'','',1),(1,new.account_id,'',1),(2,'',new.folder,1),(3,new.account_id,new.folder,1)
 ON CONFLICT(scope,account_id,folder) DO UPDATE SET total=total+1;
END;
CREATE INDEX snoozes_wake ON snoozes(wake,id);
CREATE INDEX jobs_created ON jobs(created DESC,id);
CREATE TABLE outgoing_owners(file_id TEXT PRIMARY KEY,account_id TEXT NOT NULL);
-- Only recover legacy attachment ownership when references agree on one account.
INSERT INTO outgoing_owners
 SELECT file_id,min(account_id) FROM (
  SELECT j.value AS file_id,d.account_id FROM live_drafts d,json_each(d.payload,'$.attachments') j
  UNION ALL
  SELECT j.value,k.account_id FROM jobs k,json_each(k.payload,'$.attachments') j WHERE k.kind='send'
 ) WHERE file_id IN (SELECT id FROM files WHERE message_id='outgoing')
 GROUP BY file_id HAVING count(DISTINCT account_id)=1;
CREATE TABLE outbox_mime(job_id TEXT PRIMARY KEY,raw BLOB NOT NULL);
-- Old sends lack an immutable MIME snapshot. Never silently rebuild and submit them.
UPDATE jobs SET status='failed',detail='This older queued send needs review and requeue after upgrading.'
 WHERE kind='send' AND status='pending';
PRAGMA user_version=4;
