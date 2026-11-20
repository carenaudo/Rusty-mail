DROP TRIGGER IF EXISTS inbox_summary_insert;
DROP TRIGGER IF EXISTS inbox_summary_update;
DROP TRIGGER IF EXISTS inbox_summary_delete;
DROP TABLE IF EXISTS inbox_summary;
CREATE TABLE inbox_summary(conversation_id TEXT PRIMARY KEY,account_id TEXT NOT NULL,id TEXT NOT NULL,received INTEGER NOT NULL,members INTEGER NOT NULL,unread INTEGER NOT NULL,starred INTEGER NOT NULL,newest TEXT NOT NULL);
CREATE INDEX inbox_summary_order ON inbox_summary(received DESC,id DESC);
CREATE INDEX inbox_summary_account_order ON inbox_summary(account_id,received DESC,id DESC);
INSERT INTO inbox_summary SELECT conversation_id,account_id,id,received,count(*),sum(unread),sum(starred),max((CASE WHEN received<0 THEN '0' ELSE '1' END)||printf('%016llx',received)||hex(id)) FROM mail_groups WHERE folder='inbox' GROUP BY conversation_id;
CREATE TRIGGER inbox_summary_insert AFTER INSERT ON mail_groups WHEN new.folder='inbox' BEGIN
 DELETE FROM inbox_summary WHERE conversation_id=new.conversation_id;
 INSERT INTO inbox_summary SELECT conversation_id,account_id,id,received,count(*),sum(unread),sum(starred),max((CASE WHEN received<0 THEN '0' ELSE '1' END)||printf('%016llx',received)||hex(id)) FROM mail_groups WHERE folder='inbox' AND conversation_id=new.conversation_id GROUP BY conversation_id;
END;
CREATE TRIGGER inbox_summary_delete AFTER DELETE ON mail_groups WHEN old.folder='inbox' BEGIN
 DELETE FROM inbox_summary WHERE conversation_id=old.conversation_id;
 INSERT INTO inbox_summary SELECT conversation_id,account_id,id,received,count(*),sum(unread),sum(starred),max((CASE WHEN received<0 THEN '0' ELSE '1' END)||printf('%016llx',received)||hex(id)) FROM mail_groups WHERE folder='inbox' AND conversation_id=old.conversation_id GROUP BY conversation_id;
END;
CREATE TRIGGER inbox_summary_update AFTER UPDATE ON mail_groups WHEN old.folder='inbox' OR new.folder='inbox' BEGIN
 DELETE FROM inbox_summary WHERE conversation_id=old.conversation_id;
 INSERT INTO inbox_summary SELECT conversation_id,account_id,id,received,count(*),sum(unread),sum(starred),max((CASE WHEN received<0 THEN '0' ELSE '1' END)||printf('%016llx',received)||hex(id)) FROM mail_groups WHERE folder='inbox' AND conversation_id=old.conversation_id GROUP BY conversation_id;
 DELETE FROM inbox_summary WHERE conversation_id=new.conversation_id;
 INSERT INTO inbox_summary SELECT conversation_id,account_id,id,received,count(*),sum(unread),sum(starred),max((CASE WHEN received<0 THEN '0' ELSE '1' END)||printf('%016llx',received)||hex(id)) FROM mail_groups WHERE folder='inbox' AND conversation_id=new.conversation_id GROUP BY conversation_id;
END;
PRAGMA user_version=11;
