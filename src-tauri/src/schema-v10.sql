-- Compact, covering metadata avoids decoding HTML/body JSON while grouping.
CREATE TABLE IF NOT EXISTS mail_groups(id TEXT NOT NULL UNIQUE,account_id TEXT NOT NULL,folder TEXT NOT NULL,received INTEGER NOT NULL,conversation_id TEXT NOT NULL,unread INTEGER NOT NULL,starred INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS mail_groups_order ON mail_groups(conversation_id,received DESC,id DESC,unread,starred,account_id,folder);
CREATE INDEX IF NOT EXISTS mail_groups_folder_order ON mail_groups(folder,conversation_id,received DESC,id DESC,unread,starred,account_id);
CREATE INDEX IF NOT EXISTS mail_groups_account ON mail_groups(account_id,conversation_id,received DESC,id DESC,unread,starred,folder);
CREATE INDEX IF NOT EXISTS mail_groups_account_folder_order ON mail_groups(account_id,folder,conversation_id,received DESC,id DESC,unread,starred);
CREATE INDEX IF NOT EXISTS mail_groups_starred ON mail_groups(starred,conversation_id,received DESC,id DESC,unread,account_id,folder);
INSERT OR REPLACE INTO mail_groups(rowid,id,account_id,folder,received,conversation_id,unread,starred) SELECT m.rowid,m.id,m.account_id,m.folder,m.received,CASE WHEN coalesce(json_extract(m.payload,'$.conversation_key'),'')!='' THEN json_array(m.account_id,json_extract(m.payload,'$.conversation_key')) ELSE json_array(m.id) END,coalesce(json_extract(m.payload,'$.unread'),0),coalesce(json_extract(m.payload,'$.starred'),0) FROM mail m;
CREATE TRIGGER IF NOT EXISTS mail_groups_insert AFTER INSERT ON mail BEGIN
 INSERT INTO mail_groups(rowid,id,account_id,folder,received,conversation_id,unread,starred) SELECT new.rowid,new.id,new.account_id,new.folder,new.received,CASE WHEN coalesce(json_extract(new.payload,'$.conversation_key'),'')!='' THEN json_array(new.account_id,json_extract(new.payload,'$.conversation_key')) ELSE json_array(new.id) END,coalesce(json_extract(new.payload,'$.unread'),0),coalesce(json_extract(new.payload,'$.starred'),0) ON CONFLICT(id) DO UPDATE SET rowid=excluded.rowid,account_id=excluded.account_id,folder=excluded.folder,received=excluded.received,conversation_id=excluded.conversation_id,unread=excluded.unread,starred=excluded.starred
 WHERE mail_groups.rowid!=excluded.rowid OR mail_groups.account_id!=excluded.account_id OR mail_groups.folder!=excluded.folder OR mail_groups.received!=excluded.received OR mail_groups.conversation_id!=excluded.conversation_id OR mail_groups.unread!=excluded.unread OR mail_groups.starred!=excluded.starred;
END;
CREATE TRIGGER IF NOT EXISTS mail_groups_update AFTER UPDATE ON mail BEGIN
 DELETE FROM mail_groups WHERE id=old.id AND old.id!=new.id;
 INSERT INTO mail_groups(rowid,id,account_id,folder,received,conversation_id,unread,starred) SELECT new.rowid,new.id,new.account_id,new.folder,new.received,CASE WHEN coalesce(json_extract(new.payload,'$.conversation_key'),'')!='' THEN json_array(new.account_id,json_extract(new.payload,'$.conversation_key')) ELSE json_array(new.id) END,coalesce(json_extract(new.payload,'$.unread'),0),coalesce(json_extract(new.payload,'$.starred'),0) ON CONFLICT(id) DO UPDATE SET rowid=excluded.rowid,account_id=excluded.account_id,folder=excluded.folder,received=excluded.received,conversation_id=excluded.conversation_id,unread=excluded.unread,starred=excluded.starred
 WHERE mail_groups.rowid!=excluded.rowid OR mail_groups.account_id!=excluded.account_id OR mail_groups.folder!=excluded.folder OR mail_groups.received!=excluded.received OR mail_groups.conversation_id!=excluded.conversation_id OR mail_groups.unread!=excluded.unread OR mail_groups.starred!=excluded.starred;
END;
CREATE TRIGGER IF NOT EXISTS mail_groups_delete AFTER DELETE ON mail BEGIN DELETE FROM mail_groups WHERE id=old.id; END;
PRAGMA user_version=10;
