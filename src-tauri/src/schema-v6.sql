-- Both sort keys descend so tuple comparison can seek directly to a page anchor.
DROP INDEX mail_order;
DROP INDEX mail_folder_order;
DROP INDEX mail_account_folder_order;
DROP INDEX mail_account;
CREATE INDEX mail_order ON mail(received DESC,id DESC);
CREATE INDEX mail_folder_order ON mail(folder,received DESC,id DESC);
CREATE INDEX mail_account_folder_order ON mail(account_id,folder,received DESC,id DESC);
CREATE INDEX mail_account ON mail(account_id,received DESC,id DESC);
PRAGMA user_version=6;
