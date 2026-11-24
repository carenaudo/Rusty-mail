//! Mail-domain storage and transport-independent invariants. No webview APIs.
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_RAW: usize = 25 * 1024 * 1024;
pub const MAX_ATTACHMENTS: usize = 256;
pub const MAX_HTML: usize = 1_000_000;
pub const SCHEMA_VERSION: i64 = 15;
pub const BODY_FORMAT_VERSION: u8 = 2;

#[cfg(test)]
pub fn measured_utc() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("current UTC time is representable as RFC 3339")
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub fn identity(account: &str, remote: &str) -> String {
    hex::encode(Sha256::digest(format!("{account}\0{remote}")))
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub size: usize,
    pub mime: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Mail {
    pub id: String,
    pub account_id: String,
    pub remote_id: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub thread_id: String,
    /// Derived locally; includes the grouping method and provider namespace.
    #[serde(default)]
    pub conversation_key: String,
    /// Summary fields are computed for the current mailbox/search result.
    #[serde(default)]
    pub conversation_count: usize,
    #[serde(default)]
    pub message_id: String,
    #[serde(default)]
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    #[serde(default)]
    pub reply_to: Vec<String>,
    #[serde(default)]
    pub references: Vec<String>,
    #[serde(default)]
    pub recipients_truncated: bool,
    pub account: String,
    pub sender: String,
    pub address: String,
    pub subject: String,
    pub body: String,
    /// Untrusted MIME HTML; render only through the isolated, sanitized reader.
    #[serde(default)]
    pub html_body: String,
    #[serde(default)]
    pub body_format_version: u8,
    pub category: String,
    pub unread: bool,
    pub starred: bool,
    pub folder: String,
    pub date: String,
    pub received: i64,
    pub loaded: bool,
    pub attachments: Vec<Attachment>,
    pub labels: Vec<String>,
    #[serde(default)]
    pub headers_shortened: bool,
}
fn shorten(text: &mut String, limit: usize) -> bool {
    if text.len() <= limit {
        return false;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    // Replace instead of truncating in place: do not retain a hostile header's
    // original multi-megabyte allocation in every displayed list row.
    *text = text[..end].to_owned();
    true
}
pub fn normalized_subject(subject: &str) -> String {
    let mut subject = subject
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    loop {
        let Some((prefix, rest)) = subject.split_once(':') else {
            break;
        };
        let prefix = prefix.trim();
        let numbered_reply = prefix
            .strip_prefix("re[")
            .and_then(|s| s.strip_suffix(']'))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        if matches!(prefix, "re" | "fw" | "fwd") || numbered_reply {
            subject = rest.trim_start().to_string();
        } else {
            break;
        }
    }
    if matches!(subject.as_str(), "(no subject)" | "no subject") {
        subject.clear();
    }
    subject
}

fn bounded_metadata(mail: &mut Mail) -> Result<(), String> {
    if mail.html_body.len() > MAX_HTML {
        mail.html_body.clear();
    }
    // Opaque IDs must never be shortened: truncation could merge unrelated threads.
    if !matches!(mail.provider.as_str(), "google" | "microsoft")
        || mail.thread_id.len() > 8192
        || mail.thread_id.trim().is_empty()
        || mail.thread_id.chars().any(char::is_control)
    {
        mail.thread_id.clear();
    }
    if mail.id.len() > 512
        || mail.account_id.len() > 512
        || mail.remote_id.len() > 8192
        || mail.folder.len() > 2048
        || mail.labels.len() > 256
        || mail.labels.iter().any(|s| s.len() > 2048)
        || mail.attachments.len() > MAX_ATTACHMENTS
    {
        return Err("Message metadata exceeds supported limits".into());
    }
    for (text, limit) in [
        (&mut mail.subject, 4096),
        (&mut mail.sender, 1024),
        (&mut mail.address, 1024),
        (&mut mail.account, 512),
        (&mut mail.date, 128),
        (&mut mail.category, 64),
        (&mut mail.message_id, 1024),
    ] {
        mail.headers_shortened |= shorten(text, limit);
    }
    mail.conversation_key = if !mail.thread_id.is_empty() {
        format!("native:{}", identity(&mail.provider, &mail.thread_id))
    } else if mail.provider == "imap" && !mail.headers_shortened {
        let subject = normalized_subject(&mail.subject);
        if subject.is_empty() {
            String::new()
        } else {
            format!("subject:{}", identity("imap", &subject))
        }
    } else {
        String::new()
    };
    for values in [&mut mail.to, &mut mail.cc, &mut mail.reply_to] {
        if values.len() > 100 {
            values.truncate(100);
            mail.recipients_truncated = true;
        }
        values.retain(|s| s.len() <= 1024 && !s.chars().any(char::is_control));
    }
    mail.references = crate::compose::reference_ids(mail.references.iter().map(String::as_str));
    for file in &mut mail.attachments {
        if file.id.len() > 1024 {
            return Err("Invalid attachment identity".into());
        }
        file.name = safe_filename(&file.name);
        shorten(&mut file.mime, 256);
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    pub email: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub username: String,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_username: String,
    /// SMTP supports required STARTTLS or implicit TLS. IMAP initially requires implicit TLS.
    pub smtp_starttls: bool,
    pub ca_pem: Option<String>,
}
impl ServerConfig {
    pub fn validate(&self) -> Result<(), String> {
        for host in [&self.imap_host, &self.smtp_host] {
            if host.is_empty()
                || host.len() > 253
                || host
                    .chars()
                    .any(|c| c.is_whitespace() || "/\\@:\r\n".contains(c))
            {
                return Err("Enter server hostnames without ports or URL schemes".into());
            }
        }
        if self.imap_port == 0
            || self.smtp_port == 0
            || self.username.is_empty()
            || self.smtp_username.is_empty()
        {
            return Err("Server ports and usernames are required".into());
        }
        self.email
            .parse::<lettre::Address>()
            .map_err(|_| "Invalid email address")?;
        if self.ca_pem.as_ref().is_some_and(|p| p.len() > 65536) {
            return Err("CA certificate is too large".into());
        }
        Ok(())
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Draft {
    pub account_id: String,
    pub to: String,
    pub subject: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub signature: String,
    #[serde(default)]
    pub attachments: Vec<String>,
    #[serde(default)]
    pub reply_message_id: Option<String>,
    #[serde(default)]
    pub cc: String,
    #[serde(default)]
    pub bcc: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub source_message_id: Option<String>,
    #[serde(default)]
    pub reply_references: Vec<String>,
    #[serde(default)]
    pub provider_thread_id: Option<String>,
    #[serde(default)]
    pub provider_reply_id: Option<String>,
}
#[derive(Serialize)]
pub struct MailPage {
    pub messages: Vec<Mail>,
    pub total: i64,
    pub offset: i64,
    pub next_cursor: Option<MailCursor>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MailCursor {
    pub received: i64,
    pub id: String,
}
#[derive(Default, Deserialize)]
#[serde(default)]
pub struct MailFilters {
    pub unread: bool,
    pub sender: String,
    pub received_from: Option<i64>,
    pub received_before: Option<i64>,
}

impl MailFilters {
    fn is_active(&self) -> bool {
        self.unread
            || !self.sender.trim().is_empty()
            || self.received_from.is_some()
            || self.received_before.is_some()
    }
}

#[derive(Deserialize)]
pub struct MailScope {
    pub folder: String,
    pub account: String,
    pub search: String,
    pub filters: Option<MailFilters>,
}

#[derive(Serialize)]
pub struct Job {
    pub id: String,
    pub account_id: String,
    pub kind: String,
    pub status: String,
    pub detail: String,
    pub due: i64,
}

// Savepoints compose with caller transactions: a failed index/membership write must
// never leave the authoritative row and its derived data disagreeing.
fn atomic<T>(db: &Connection, work: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    db.execute_batch("SAVEPOINT mail_engine_write")
        .map_err(|e| e.to_string())?;
    match work() {
        Ok(value) => {
            db.execute_batch("RELEASE mail_engine_write")
                .map_err(|e| e.to_string())?;
            Ok(value)
        }
        Err(error) => {
            db.execute_batch("ROLLBACK TO mail_engine_write; RELEASE mail_engine_write")
                .map_err(|e| format!("Storage rollback failed: {e}"))?;
            Err(error)
        }
    }
}

pub fn migrate(db: &Connection) -> Result<(), String> {
    db.create_scalar_function(
        "mail_sender_fold",
        1,
        rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| Ok(ctx.get::<String>(0)?.to_lowercase()),
    )
    .map_err(|e| e.to_string())?;
    let version: i64 = db
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if version > SCHEMA_VERSION {
        return Err("Database belongs to a newer Rusty Mail version".into());
    }
    atomic(db, || {
        db.execute_batch("CREATE TABLE IF NOT EXISTS mail(
 id TEXT PRIMARY KEY,account_id TEXT NOT NULL,remote_id TEXT NOT NULL,folder TEXT NOT NULL,received INTEGER NOT NULL,payload TEXT NOT NULL,
 UNIQUE(account_id,remote_id));
 CREATE INDEX IF NOT EXISTS mail_order ON mail(received DESC,id);
 CREATE INDEX IF NOT EXISTS mail_account ON mail(account_id,received DESC);
 CREATE INDEX IF NOT EXISTS mail_account_folder_order ON mail(account_id,folder,received DESC,id);
 CREATE INDEX IF NOT EXISTS mail_folder_order ON mail(folder,received DESC,id);
 CREATE VIRTUAL TABLE IF NOT EXISTS mail_search USING fts5(id UNINDEXED,account_id,sender,subject,body,tokenize='unicode61');
 CREATE TABLE IF NOT EXISTS memberships(mail_id TEXT NOT NULL,mailbox TEXT NOT NULL,PRIMARY KEY(mail_id,mailbox));
 CREATE INDEX IF NOT EXISTS membership_box ON memberships(mailbox,mail_id);
 CREATE TABLE IF NOT EXISTS mailboxes(account_id TEXT NOT NULL,id TEXT NOT NULL,name TEXT NOT NULL,special TEXT NOT NULL,PRIMARY KEY(account_id,id));
 CREATE TABLE IF NOT EXISTS mail_raw(id TEXT PRIMARY KEY,raw BLOB NOT NULL,accessed INTEGER NOT NULL);
 CREATE TABLE IF NOT EXISTS files(id TEXT PRIMARY KEY,message_id TEXT NOT NULL,name TEXT NOT NULL,mime TEXT NOT NULL,data BLOB NOT NULL,created INTEGER NOT NULL);
 CREATE TABLE IF NOT EXISTS account_config(id TEXT PRIMARY KEY,config TEXT NOT NULL);
 CREATE TABLE IF NOT EXISTS sync_state(account_id TEXT NOT NULL,key TEXT NOT NULL,value TEXT NOT NULL,PRIMARY KEY(account_id,key));
 CREATE TABLE IF NOT EXISTS jobs(id TEXT PRIMARY KEY,account_id TEXT NOT NULL,kind TEXT NOT NULL,payload TEXT NOT NULL,status TEXT NOT NULL,detail TEXT NOT NULL DEFAULT '',due INTEGER NOT NULL,created INTEGER NOT NULL);
 CREATE INDEX IF NOT EXISTS jobs_due ON jobs(status,due);
 CREATE INDEX IF NOT EXISTS jobs_status ON jobs(status);
 CREATE INDEX IF NOT EXISTS jobs_message ON jobs(json_extract(payload,'$.id')) WHERE kind='mutation';
 CREATE TABLE IF NOT EXISTS ai_consent(account_id TEXT NOT NULL,endpoint TEXT NOT NULL,PRIMARY KEY(account_id,endpoint));
 CREATE TABLE IF NOT EXISTS live_drafts(account_id TEXT PRIMARY KEY,payload TEXT NOT NULL);
 CREATE TABLE IF NOT EXISTS snoozes(id TEXT PRIMARY KEY,wake INTEGER NOT NULL);
 UPDATE jobs SET status='uncertain',detail='App stopped during submission. Check Sent before choosing to send again.' WHERE kind='send' AND status='running';") .map_err(|e|e.to_string())?;
        if version < 3 {
            db.execute_batch("DROP TABLE mail_search; CREATE VIRTUAL TABLE mail_search USING fts5(id UNINDEXED,account_id,sender,subject,body,tokenize='unicode61'); INSERT INTO mail_search(rowid,id,account_id,sender,subject,body) SELECT rowid,id,account_id,json_extract(payload,'$.sender'),json_extract(payload,'$.subject'),json_extract(payload,'$.body') FROM mail; PRAGMA user_version=3;").map_err(|e|e.to_string())?;
        }
        if version < 4 {
            db.execute_batch(include_str!("schema-v4.sql"))
                .map_err(|e| e.to_string())?;
        }
        if version < 5 {
            db.execute_batch("DROP TABLE mail_search; CREATE VIRTUAL TABLE mail_search USING fts5(id UNINDEXED,account_id,folder,sender,subject,body,tokenize='unicode61'); DROP INDEX IF EXISTS mail_folder; DROP INDEX mail_account; CREATE INDEX mail_account ON mail(account_id,received DESC,id);").map_err(|e|e.to_string())?;
            let mut q = db
                .prepare("SELECT rowid,id,account_id,folder,payload FROM mail")
                .map_err(|e| e.to_string())?;
            let rows = q
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                })
                .map_err(|e| e.to_string())?;
            let mut insert=db.prepare("INSERT INTO mail_search(rowid,id,account_id,folder,sender,subject,body) VALUES(?1,?2,?3,?4,?5,?6,?7)").map_err(|e|e.to_string())?;
            for row in rows {
                let (rowid, id, account, folder, payload) = row.map_err(|e| e.to_string())?;
                let m: Mail = serde_json::from_str(&payload)
                    .map_err(|_| "Invalid cached message during index upgrade")?;
                insert
                    .execute(params![
                        rowid,
                        id,
                        identity(&account, ""),
                        identity(&folder, ""),
                        format!("{} {} {}", m.sender, m.address, m.account),
                        m.subject,
                        m.body
                    ])
                    .map_err(|e| e.to_string())?;
            }
            db.pragma_update(None, "user_version", 5)
                .map_err(|e| e.to_string())?;
        }
        if version < 6 {
            db.execute_batch(include_str!("schema-v6.sql"))
                .map_err(|e| e.to_string())?;
        }
        if version < 7 {
            db.execute_batch("CREATE INDEX IF NOT EXISTS mail_thread ON mail(account_id,json_extract(payload,'$.provider'),json_extract(payload,'$.thread_id'),received,id);
                DELETE FROM sync_state WHERE key IN ('history','page','bootstrap_history') OR key LIKE 'delta:%';
                PRAGMA user_version=7;").map_err(|e| e.to_string())?;
        }
        if version < 8 {
            let has_accounts: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='accounts' AND type='table')", [], |r| r.get(0)).map_err(|e| e.to_string())?;
            // Existing IMAP payloads predate the provider field. Recover it only
            // from the authoritative account record; unknown accounts stay separate.
            let source = if has_accounts {
                "SELECT m.id,m.payload,coalesce(a.provider,'') FROM mail m LEFT JOIN accounts a ON a.id=m.account_id"
            } else {
                "SELECT id,payload,'' FROM mail"
            };
            let mut q = db.prepare(source).map_err(|e| e.to_string())?;
            let rows = q
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .map_err(|e| e.to_string())?;
            for row in rows {
                let (id, payload, provider) = row.map_err(|e| e.to_string())?;
                let mut m: Mail = serde_json::from_str(&payload)
                    .map_err(|_| "Invalid cached message during conversation upgrade")?;
                if m.provider.is_empty() && provider == "imap" {
                    m.provider = provider;
                }
                bounded_metadata(&mut m)?;
                db.execute(
                    "UPDATE mail SET payload=?1 WHERE id=?2",
                    params![serde_json::to_string(&m).map_err(|e| e.to_string())?, id],
                )
                .map_err(|e| e.to_string())?;
            }
            db.execute_batch("CREATE INDEX IF NOT EXISTS mail_conversation ON mail(account_id,json_extract(payload,'$.conversation_key'),received,id); PRAGMA user_version=8;").map_err(|e|e.to_string())?;
        }
        if version < 9 {
            db.execute_batch("CREATE TABLE IF NOT EXISTS account_aliases(account_id TEXT NOT NULL,address TEXT NOT NULL,PRIMARY KEY(account_id,address)); PRAGMA user_version=9;").map_err(|e|e.to_string())?;
        }
        if version < 10 {
            db.execute_batch(include_str!("schema-v10.sql"))
                .map_err(|e| e.to_string())?;
        }
        if version < 11 {
            db.execute_batch(include_str!("schema-v11.sql"))
                .map_err(|e| e.to_string())?;
        }
        if version < 12 {
            db.execute_batch(include_str!("schema-v12.sql"))
                .map_err(|e| e.to_string())?;
        }
        if version < 13 {
            db.execute_batch(include_str!("schema-v13.sql"))
                .map_err(|e| e.to_string())?;
        }
        if version < 14 {
            db.execute_batch(include_str!("schema-v14.sql"))
                .map_err(|e| e.to_string())?;
        }
        if version < 15 {
            db.execute_batch(include_str!("schema-v15.sql"))
                .map_err(|e| e.to_string())?;
        }
        crate::reconcile::recover_running(db)?;
        Ok(())
    })
}
pub fn state(db: &Connection, account: &str, key: &str) -> Result<Option<String>, String> {
    db.query_row(
        "SELECT value FROM sync_state WHERE account_id=?1 AND key=?2",
        params![account, key],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}
pub fn checkpoint(db: &Connection, account: &str, key: &str, value: &str) -> Result<(), String> {
    db.execute(
        "INSERT OR REPLACE INTO sync_state VALUES(?1,?2,?3)",
        params![account, key, value],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
pub fn get(db: &Connection, id: &str) -> Result<Mail, String> {
    let payload: String = db
        .query_row("SELECT payload FROM mail WHERE id=?1", [id], |r| r.get(0))
        .map_err(|_| "Message not found")?;
    let mut mail = serde_json::from_str(&payload).map_err(|_| "Invalid cached message")?;
    bounded_metadata(&mut mail)?;
    Ok(mail)
}
pub fn put(db: &Connection, m: &Mail) -> Result<(), String> {
    if m.id.is_empty() || m.account_id.is_empty() || m.remote_id.is_empty() {
        return Err("Message identity must include an account and remote ID".into());
    }
    atomic(db, || put_inner(db, m))
}
fn put_inner(db: &Connection, m: &Mail) -> Result<(), String> {
    let mut m = m.clone();
    bounded_metadata(&mut m)?;
    if !m.loaded {
        if let Ok(old) = get(db, &m.id) {
            if old.loaded
                && db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM mail_raw WHERE id=?1)",
                        [&m.id],
                        |r| r.get::<_, bool>(0),
                    )
                    .map_err(|e| e.to_string())?
            {
                m.body = old.body;
                m.html_body = old.html_body;
                m.body_format_version = old.body_format_version;
                m.to = old.to;
                m.cc = old.cc;
                m.reply_to = old.reply_to;
                m.references = old.references;
                m.recipients_truncated = old.recipients_truncated;
                m.loaded = true;
                m.attachments = old.attachments;
                if m.message_id.is_empty() {
                    m.message_id = old.message_id;
                }
            }
        }
    }
    let changed=db.execute("INSERT INTO mail(id,account_id,remote_id,folder,received,payload) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(id) DO UPDATE SET folder=excluded.folder,received=excluded.received,payload=excluded.payload WHERE mail.account_id=excluded.account_id AND mail.remote_id=excluded.remote_id",params![m.id,m.account_id,m.remote_id,m.folder,m.received,serde_json::to_string(&m).map_err(|e|e.to_string())?]).map_err(|e|e.to_string())?;
    if changed != 1 {
        return Err("A cached message cannot change account or remote identity".into());
    }
    db.execute("DELETE FROM memberships WHERE mail_id=?1", [&m.id])
        .map_err(|e| e.to_string())?;
    for label in &m.labels {
        db.execute(
            "INSERT OR IGNORE INTO memberships VALUES(?1,?2)",
            params![m.id, label],
        )
        .map_err(|e| e.to_string())?;
    }
    let rowid: i64 = db
        .query_row("SELECT rowid FROM mail WHERE id=?1", [&m.id], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    db.execute("DELETE FROM mail_search WHERE rowid=?1", [rowid])
        .map_err(|e| e.to_string())?;
    db.execute(
        "INSERT INTO mail_search(rowid,id,account_id,folder,sender,subject,body) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            rowid,
            m.id,
            identity(&m.account_id,""),
            identity(&m.folder,""),
            format!("{} {} {}", m.sender, m.address, m.account),
            m.subject,
            m.body
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
pub fn remove(db: &Connection, id: &str) -> Result<(), String> {
    atomic(db, || remove_inner(db, id))
}
fn remove_inner(db: &Connection, id: &str) -> Result<(), String> {
    db.execute(
        "DELETE FROM mail_search WHERE rowid IN (SELECT rowid FROM mail WHERE id=?1)",
        [id],
    )
    .map_err(|e| e.to_string())?;
    for sql in [
        "DELETE FROM memberships WHERE mail_id=?1",
        "DELETE FROM mail_raw WHERE id=?1",
        "DELETE FROM files WHERE message_id=?1",
        "DELETE FROM snoozes WHERE id=?1",
        "DELETE FROM mail WHERE id=?1",
    ] {
        db.execute(sql, [id]).map_err(|e| e.to_string())?;
    }
    Ok(())
}
#[cfg(test)]
pub fn query(
    db: &Connection,
    folder: &str,
    account: &str,
    search: &str,
    offset: i64,
) -> Result<MailPage, String> {
    query_after(db, folder, account, search, offset, None)
}
pub fn query_after(
    db: &Connection,
    folder: &str,
    account: &str,
    search: &str,
    offset: i64,
    cursor: Option<&MailCursor>,
) -> Result<MailPage, String> {
    query_mode(
        db,
        folder,
        account,
        search,
        &MailFilters::default(),
        offset,
        cursor,
        false,
    )
}

#[cfg(test)]
pub fn query_conversations(
    db: &Connection,
    folder: &str,
    account: &str,
    search: &str,
    offset: i64,
    cursor: Option<&MailCursor>,
) -> Result<MailPage, String> {
    query_mode(
        db,
        folder,
        account,
        search,
        &MailFilters::default(),
        offset,
        cursor,
        true,
    )
}

pub fn query_filtered_conversations(
    db: &Connection,
    folder: &str,
    account: &str,
    search: &str,
    filters: Option<&MailFilters>,
    offset: i64,
    cursor: Option<&MailCursor>,
) -> Result<MailPage, String> {
    query_mode(
        db,
        folder,
        account,
        search,
        filters.unwrap_or(&MailFilters::default()),
        offset,
        cursor,
        true,
    )
}

// SQLite returns the bare id and received columns from the row selected by MAX.
// The key orders all signed timestamps, then the full UTF-8 bytes of the id.
const NEWEST_KEY: &str =
    "(CASE WHEN m.received<0 THEN '0' ELSE '1' END)||printf('%016llx',m.received)||hex(m.id)";

fn query_inbox_summary(
    db: &Connection,
    account: &str,
    offset: i64,
    cursor: Option<&MailCursor>,
    snoozes: Option<i64>,
    compact: bool,
) -> Result<MailPage, String> {
    let mut args = Vec::<rusqlite::types::Value>::new();
    let account_condition = if account.is_empty() {
        "1"
    } else {
        args.push(account.to_owned().into());
        "m.account_id=?1"
    };
    let active_snoozes = if let Some(now) = snoozes {
        args.push(now.into());
        let wake_arg = args.len();
        let active = format!(
            "SELECT s.id,m.conversation_id FROM snoozes s CROSS JOIN mail_groups m ON m.id=s.id
             WHERE s.wake>?{wake_arg} AND m.folder='inbox' AND {account_condition}"
        );
        let relevant: bool = db
            .query_row(
                &format!("SELECT EXISTS({active})"),
                rusqlite::params_from_iter(&args),
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if relevant {
            Some(active)
        } else {
            args.pop();
            None
        }
    } else {
        None
    };
    let boundary = cursor_condition(&mut args, cursor)?;
    let skip = if cursor.is_some() { 0 } else { offset };
    args.push(skip.into());
    let skip_arg = args.len();
    let total = format!("SELECT count(*) FROM inbox_summary m WHERE {account_condition}");
    let fields = "m.id,m.received,m.members,m.unread,m.starred";
    let ctes = if let Some(active) = &active_snoozes {
        args.push(skip.saturating_add(101).into());
        let limit_arg = args.len();
        format!(
            "active AS MATERIALIZED ({active}),
             affected AS MATERIALIZED (SELECT DISTINCT conversation_id FROM active),
             visible AS MATERIALIZED (
                 SELECT m.id,m.received,count(*) AS members,sum(m.unread) AS unread,
                        sum(m.starred) AS starred,max({NEWEST_KEY}) AS newest
                 FROM affected a CROSS JOIN mail_groups m ON m.conversation_id=a.conversation_id
                 WHERE m.folder='inbox' AND m.id NOT IN (SELECT id FROM active)
                 GROUP BY m.conversation_id
             ),
             total AS (SELECT ({total}) - (SELECT count(*) FROM affected)
                               + (SELECT count(*) FROM visible) AS n),
             base AS MATERIALIZED (
                 SELECT {fields} FROM inbox_summary m
                 WHERE {account_condition} AND {boundary}
                       AND m.conversation_id NOT IN (SELECT conversation_id FROM affected)
                 ORDER BY m.received DESC,m.id DESC LIMIT ?{limit_arg}
             ),
             page AS MATERIALIZED (
                 SELECT * FROM (SELECT * FROM base UNION ALL SELECT {fields} FROM visible m WHERE {boundary})
                 ORDER BY received DESC,id DESC LIMIT 101 OFFSET ?{skip_arg}
             )"
        )
    } else {
        format!(
            "total(n) AS ({total}),
             page AS MATERIALIZED (
                 SELECT {fields} FROM inbox_summary m WHERE {account_condition} AND {boundary}
                 ORDER BY m.received DESC,m.id DESC LIMIT 101 OFFSET ?{skip_arg}
             )"
        )
    };
    read_conversation_page(
        db,
        &ctes,
        &args,
        offset,
        compact || active_snoozes.is_some(),
    )
}

fn cursor_condition(
    args: &mut Vec<rusqlite::types::Value>,
    cursor: Option<&MailCursor>,
) -> Result<String, String> {
    let Some(cursor) = cursor else {
        return Ok("1".into());
    };
    if cursor.id.len() > 512 {
        return Err("Invalid mailbox cursor".into());
    }
    args.push(cursor.received.into());
    args.push(cursor.id.clone().into());
    Ok(format!(
        "(m.received,m.id) < (?{},?{})",
        args.len() - 1,
        args.len()
    ))
}

fn read_conversation_page(
    db: &Connection,
    ctes: &str,
    args: &[rusqlite::types::Value],
    offset: i64,
    compact: bool,
) -> Result<MailPage, String> {
    // Drive the join from the total so an empty page still reports the exact count.
    let sql = format!(
        "WITH {ctes}
         SELECT total.n, CASE WHEN page.id IS NOT NULL THEN
          json_set(json_remove((SELECT payload FROM mail WHERE id=page.id),
              '$.html_body','$.to','$.cc','$.reply_to','$.references'),
              '$.conversation_count',members,
              '$.unread',json(CASE WHEN unread>0 THEN 'true' ELSE 'false' END),
              '$.starred',json(CASE WHEN starred>0 THEN 'true' ELSE 'false' END))
         END
         FROM total LEFT JOIN page ON true ORDER BY page.received DESC,page.id DESC"
    );
    let mut query = db.prepare(&sql).map_err(|e| e.to_string())?;
    let mut rows = query
        .query(rusqlite::params_from_iter(args))
        .map_err(|e| e.to_string())?;
    let mut total = 0;
    let mut messages = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        total = row.get(0).map_err(|e| e.to_string())?;
        let Some(payload) = row.get::<_, Option<String>>(1).map_err(|e| e.to_string())? else {
            continue;
        };
        let mut message: Mail = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
        if compact {
            message.labels.clear();
            message.attachments.clear();
        }
        bounded_metadata(&mut message)?;
        message.body = message.body.chars().take(240).collect();
        messages.push(message);
    }
    Ok(finish_page(messages, total, offset))
}

fn finish_page(mut messages: Vec<Mail>, total: i64, offset: i64) -> MailPage {
    let more = messages.len() > 100;
    messages.truncate(100);
    let next_cursor = if more {
        messages.last().map(|message| MailCursor {
            received: message.received,
            id: message.id.clone(),
        })
    } else {
        None
    };
    MailPage {
        messages,
        total,
        offset,
        next_cursor,
    }
}

fn mail_predicates(
    db: &Connection,
    folder: &str,
    account: &str,
    search: &str,
    filters: &MailFilters,
    conversations: bool,
) -> Result<MailPredicates, String> {
    if search.len() > 2048 || account.len() > 512 || folder.len() > 2048 {
        return Err("Mailbox query exceeds limits".into());
    }
    let sender = filters.sender.trim();
    if sender.len() > 512 || sender.chars().any(char::is_control) {
        return Err("Invalid sender address filter".into());
    }
    if matches!((filters.received_from, filters.received_before), (Some(from), Some(before)) if from >= before)
    {
        return Err("Received from must be on or before Received through".into());
    }
    let query_now = now();
    let mut terms = Vec::new();
    for term in search.split_whitespace().take(20) {
        terms.push(format!("\"{}\"", term.replace('"', "\"\"")))
    }
    let term = if terms.is_empty() {
        String::new()
    } else {
        format!("{{sender subject body}} : ({})", terms.join(" AND "))
    };
    // An ASCII address has the same searchable tokens in the sender column as
    // in the original address. Use the exact comparison to reject token aliases
    // and matches in the display name or account address.
    let term = if sender.is_ascii() && sender.bytes().any(|b| b.is_ascii_alphanumeric()) {
        let address = format!("sender : \"{}\"", sender.replace('"', "\"\""));
        if term.is_empty() {
            address
        } else {
            format!("({term}) AND ({address})")
        }
    } else {
        term
    };
    let term = if !term.is_empty() && !account.is_empty() {
        format!("account_id:\"{}\" AND ({term})", identity(account, ""))
    } else {
        term
    };
    let simple_folder = matches!(
        folder,
        "all" | "inbox" | "archive" | "trash" | "spam" | "sent" | "drafts"
    );
    let term = if !term.is_empty() && simple_folder && folder != "all" {
        format!("folder:\"{}\" AND ({term})", identity(folder, ""))
    } else {
        term
    };
    // Emit only active predicates: optional-filter ORs force full scans and decode every payload.
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
    let mut clauses = Vec::new();
    if !account.is_empty() {
        args.push(account.to_string().into());
        clauses.push(format!("m.account_id=?{}", args.len()));
    }
    let searching = !term.is_empty();
    let table = if conversations { "mail_groups" } else { "mail" };
    let from = if searching {
        format!("mail_search CROSS JOIN {table} m ON m.rowid=mail_search.rowid")
    } else {
        format!("{table} m")
    };
    let match_position = clauses.len();
    let mut search_arg = 0;
    if searching {
        args.push(term.clone().into());
        search_arg = args.len();
        clauses.push(format!("mail_search MATCH ?{}", args.len()));
    }
    let membership = folder.strip_prefix("mailbox:");
    if membership.is_some() && account.is_empty() {
        return Err("Select an account to browse its folders or labels".into());
    }
    match folder {
        "all" => {}
        "starred" => clauses.push(
            if conversations {
                "m.starred=1"
            } else {
                "json_extract(m.payload,'$.starred')=1"
            }
            .into(),
        ),
        "snoozed" => {
            args.push(query_now.into());
            clauses.push(format!(
                "m.id IN (SELECT id FROM snoozes WHERE wake>?{})",
                args.len()
            ));
        }
        "inbox" | "archive" | "trash" | "spam" | "sent" | "drafts" => {
            args.push(folder.to_string().into());
            clauses.push(format!("m.folder=?{}", args.len()));
        }
        _ => {
            args.push(membership.unwrap_or(folder).to_string().into());
            clauses.push(format!(
                "m.id IN (SELECT mail_id FROM memberships WHERE mailbox=?{})",
                args.len()
            ));
        }
    }
    let snoozed: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM snoozes WHERE wake>?1)",
            [query_now],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if folder != "snoozed" && snoozed {
        args.push(query_now.into());
        clauses.push(format!(
            "m.id NOT IN (SELECT id FROM snoozes WHERE wake>?{})",
            args.len()
        ));
    }
    if filters.unread {
        clauses.push(
            if conversations {
                "m.unread=1"
            } else {
                "json_extract(m.payload,'$.unread')=1"
            }
            .into(),
        );
    }
    if !sender.is_empty() {
        args.push(sender.to_lowercase().into());
        let payload = if conversations {
            "(SELECT payload FROM mail WHERE rowid=m.rowid)"
        } else {
            "m.payload"
        };
        clauses.push(format!(
            "mail_sender_fold(coalesce(json_extract({payload},'$.address'),''))=?{}",
            args.len()
        ));
    }
    if let Some(from) = filters.received_from {
        args.push(from.into());
        clauses.push(format!("m.received>=?{}", args.len()));
    }
    if let Some(before) = filters.received_before {
        args.push(before.into());
        clauses.push(format!("m.received<?{}", args.len()));
    }
    Ok(MailPredicates {
        table,
        from,
        args,
        clauses,
        term,
        match_position,
        search_arg,
        simple_folder,
        snoozed,
        query_now,
    })
}

struct MailPredicates {
    table: &'static str,
    from: String,
    args: Vec<rusqlite::types::Value>,
    clauses: Vec<String>,
    term: String,
    match_position: usize,
    search_arg: usize,
    simple_folder: bool,
    snoozed: bool,
    query_now: i64,
}

fn where_clause(clauses: &[String]) -> String {
    if clauses.is_empty() {
        "1".into()
    } else {
        clauses.join(" AND ")
    }
}

fn query_mode(
    db: &Connection,
    folder: &str,
    account: &str,
    search: &str,
    filters: &MailFilters,
    offset: i64,
    cursor: Option<&MailCursor>,
    conversations: bool,
) -> Result<MailPage, String> {
    let offset = offset.max(0);
    let MailPredicates {
        table,
        mut from,
        mut args,
        mut clauses,
        term,
        match_position,
        search_arg,
        simple_folder,
        snoozed,
        query_now,
    } = mail_predicates(db, folder, account, search, filters, conversations)?;
    let searching = !term.is_empty();
    let filtered = filters.is_active();
    let inbox_snoozes = snoozed.then_some(query_now);
    if conversations && folder == "inbox" && !searching && !filtered {
        return query_inbox_summary(db, account, offset, cursor, inbox_snoozes, false);
    }
    let condition = where_clause(&clauses);
    let indexed_search = searching && simple_folder && !filtered;
    let mut total = if indexed_search {
        db.query_row(
            "SELECT count(*) FROM mail_search WHERE mail_search MATCH ?1",
            [&term],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?
    } else if conversations {
        0 // The grouped result provides its own exact total.
    } else if !searching && simple_folder && !filtered {
        let filtered_folder = if folder == "all" { "" } else { folder };
        let scope = i64::from(!account.is_empty()) + 2 * i64::from(folder != "all");
        db.query_row("SELECT coalesce((SELECT total FROM mail_counts WHERE scope=?1 AND account_id=?2 AND folder=?3),0)",params![scope,account,filtered_folder],|r|r.get(0)).map_err(|e|e.to_string())?
    } else {
        db.query_row(
            &format!("SELECT count(*) FROM {from} WHERE {condition}"),
            rusqlite::params_from_iter(args.iter()),
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?
    };
    let indexed_matches = total;
    // Subtract only active snoozes using their small wake index. A single snooze
    // must not force a full mailbox recount or disable index-only FTS counts.
    if !conversations && snoozed && simple_folder && !filtered {
        let mut excluded_args: Vec<rusqlite::types::Value> = vec![query_now.into()];
        let mut excluded = vec!["s.wake>?1".to_string()];
        if !account.is_empty() {
            excluded_args.push(account.to_string().into());
            excluded.push(format!("m.account_id=?{}", excluded_args.len()));
        }
        if folder != "all" {
            excluded_args.push(folder.to_string().into());
            excluded.push(format!("m.folder=?{}", excluded_args.len()));
        }
        if searching {
            excluded_args.push(term.clone().into());
            excluded.push(format!(
                "EXISTS(SELECT 1 FROM mail_search WHERE rowid=m.rowid AND mail_search MATCH ?{})",
                excluded_args.len()
            ));
        }
        let hidden: i64 = db
            .query_row(
                &format!(
                    "SELECT count(*) FROM snoozes s CROSS JOIN mail m ON m.id=s.id WHERE {}",
                    excluded.join(" AND ")
                ),
                rusqlite::params_from_iter(excluded_args.iter()),
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        total -= hidden;
    }
    // Dense conversation searches use the grouping index with a shared FTS
    // rowid set. Sparse searches start at FTS to visit only matching rows.
    let density_count = if conversations {
        indexed_matches
    } else {
        total
    };
    let inbox_coverage = conversations && folder == "inbox" && density_count > 0;
    if indexed_search && (inbox_coverage || density_count > 1000) {
        let scope = i64::from(!account.is_empty()) + 2 * i64::from(folder != "all");
        let scoped_total:i64=db.query_row("SELECT coalesce((SELECT total FROM mail_counts WHERE scope=?1 AND account_id=?2 AND folder=?3),0)",params![scope,account,if folder=="all"{""}else{folder}],|r|r.get(0)).map_err(|e|e.to_string())?;
        if inbox_coverage && density_count == scoped_total {
            // Every inbox message matches. The summary already contains these groups.
            return query_inbox_summary(db, account, offset, cursor, inbox_snoozes, true);
        }
        if density_count > 1000 && density_count > scoped_total / 2 {
            let index = match (!account.is_empty(), folder != "all") {
                (true, true) => "mail_account_folder_order",
                (true, false) => "mail_account",
                (false, true) => "mail_folder_order",
                (false, false) => "mail_order",
            };
            let index = if conversations {
                index.replacen("mail_", "mail_groups_", 1)
            } else {
                index.to_string()
            };
            from = format!("{table} m INDEXED BY {index}");
            clauses[match_position] = if density_count == scoped_total {
                format!("?{search_arg} IS NOT NULL")
            } else if conversations {
                format!("m.rowid IN (SELECT rowid FROM mail_search WHERE mail_search MATCH ?{search_arg})")
            } else {
                format!("EXISTS(SELECT 1 FROM mail_search WHERE rowid=m.rowid AND mail_search MATCH ?{search_arg})")
            };
        }
    }
    if conversations {
        let condition = where_clause(&clauses);
        let boundary = cursor_condition(&mut args, cursor)?;
        args.push(if cursor.is_some() { 0 } else { offset }.into());
        let ctes = format!(
            "hits AS MATERIALIZED (
                SELECT m.id,m.received,count(*) AS members,sum(m.unread) AS unread,
                       sum(m.starred) AS starred,max({NEWEST_KEY}) AS newest
                FROM {from} WHERE {condition} GROUP BY m.conversation_id
             ),
             total(n) AS (SELECT count(*) FROM hits),
             page AS MATERIALIZED (
                SELECT id,received,members,unread,starred FROM hits m WHERE {boundary}
                ORDER BY received DESC,id DESC LIMIT 101 OFFSET ?{}
             )",
            args.len()
        );
        return read_conversation_page(db, &ctes, &args, offset, true);
    }
    clauses.push(cursor_condition(&mut args, cursor)?);
    let condition = where_clause(&clauses);
    args.push(if cursor.is_some() { 0 } else { offset }.into());
    let sql = format!(
        "SELECT json_remove(payload,'$.html_body','$.to','$.cc','$.reply_to','$.references')
         FROM mail NOT INDEXED WHERE rowid IN (
             SELECT m.rowid FROM {from} WHERE {condition}
             ORDER BY m.received DESC,m.id DESC LIMIT 101 OFFSET ?{}
         ) ORDER BY received DESC,id DESC",
        args.len()
    );
    let mut q = db.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = q
        .query_map(rusqlite::params_from_iter(args.iter()), |r| {
            r.get::<_, String>(0)
        })
        .map_err(|e| e.to_string())?;
    let mut messages = Vec::new();
    for row in rows {
        let mut m: Mail =
            serde_json::from_str(&row.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        m.labels.clear();
        m.labels.shrink_to_fit();
        m.attachments.clear();
        m.attachments.shrink_to_fit();
        bounded_metadata(&mut m)?;
        m.body = m.body.chars().take(240).collect();
        messages.push(m)
    }
    Ok(finish_page(messages, total, offset))
}
pub fn bulk_mail_ids(
    db: &Connection,
    scope: &MailScope,
    row_ids: &[String],
) -> Result<Vec<String>, String> {
    const MAX_MESSAGES: usize = 500;
    let stale = "The selection changed. Refresh and select the conversations again.";
    let MailPredicates {
        mut args,
        mut clauses,
        search_arg,
        match_position,
        ..
    } = mail_predicates(
        db,
        &scope.folder,
        &scope.account,
        &scope.search,
        scope.filters.as_ref().unwrap_or(&MailFilters::default()),
        true,
    )?;
    // A bulk request has at most one page of selected conversations.
    // Look up their members first, then probe the FTS index for each member.
    if search_arg != 0 {
        clauses[match_position] = format!(
            "EXISTS(SELECT 1 FROM mail_search WHERE rowid=m.rowid AND mail_search MATCH ?{search_arg})"
        );
    }
    args.push(
        serde_json::to_string(row_ids)
            .map_err(|e| e.to_string())?
            .into(),
    );
    clauses.push(format!("m.conversation_id IN (SELECT conversation_id FROM mail_groups WHERE id IN (SELECT value FROM json_each(?{})))", args.len()));
    let condition = where_clause(&clauses);
    // The first matching member of each conversation is the same row the
    // mailbox displays. Check it before mutating any member.
    let sql = format!(
        "SELECT m.id,m.conversation_id FROM mail_groups m WHERE {condition}
        ORDER BY m.received DESC,m.id DESC LIMIT {}",
        MAX_MESSAGES + 1
    );
    let mut query = db.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = query
        .query_map(rusqlite::params_from_iter(args.iter()), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;
    let mut ids = Vec::new();
    let mut conversations = std::collections::HashSet::new();
    let mut current_rows = std::collections::HashSet::new();
    for row in rows {
        let (id, conversation) = row.map_err(|e| e.to_string())?;
        if conversations.insert(conversation) {
            current_rows.insert(id.clone());
        }
        ids.push(id);
    }
    if ids.len() > MAX_MESSAGES {
        return Err(
            "Selection matches more than 500 messages. Narrow the selection or filters.".into(),
        );
    }
    if current_rows.len() != row_ids.len() || row_ids.iter().any(|id| !current_rows.contains(id)) {
        return Err(stale.into());
    }
    Ok(ids)
}

pub fn conversation(db: &Connection, id: &str) -> Result<Vec<Mail>, String> {
    let m = get(db, id)?;
    if m.conversation_key.is_empty() {
        let mut m = m;
        m.html_body.clear();
        m.to.clear();
        m.cc.clear();
        m.reply_to.clear();
        m.references.clear();
        m.body = m.body.chars().take(240).collect();
        return Ok(vec![m]);
    }
    let mut q = db.prepare("SELECT json_remove(payload,'$.html_body','$.to','$.cc','$.reply_to','$.references') FROM mail WHERE account_id=?1 AND json_extract(payload,'$.conversation_key')=?2 ORDER BY received,id").map_err(|e|e.to_string())?;
    let rows = q
        .query_map(params![m.account_id, m.conversation_key], |r| {
            r.get::<_, String>(0)
        })
        .map_err(|e| e.to_string())?;
    rows.map(|row| {
        let mut member: Mail =
            serde_json::from_str(&row.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        bounded_metadata(&mut member)?;
        // Bodies are fetched individually when a message is expanded.
        member.body = member.body.chars().take(240).collect();
        Ok(member)
    })
    .collect()
}

/// Upgrade existing downloaded bodies locally, without forcing a provider fetch.
pub fn upgrade_cached_body(db: &Connection, mut mail: Mail) -> Result<Mail, String> {
    if mail.loaded && mail.body_format_version < BODY_FORMAT_VERSION {
        let raw: Option<Vec<u8>> = db
            .query_row("SELECT raw FROM mail_raw WHERE id=?1", [&mail.id], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        if let Some(raw) = raw {
            return parse_raw(db, mail, &raw);
        }
        mail.body_format_version = BODY_FORMAT_VERSION;
        put(db, &mail)?;
    }
    Ok(mail)
}

pub fn decode_url64(s: &str) -> Result<Vec<u8>, String> {
    URL_SAFE_NO_PAD
        .decode(s.trim_end_matches('='))
        .or_else(|_| STANDARD.decode(s))
        .map_err(|_| "Invalid mail encoding".into())
}
pub fn parse_raw(db: &Connection, mail: Mail, raw: &[u8]) -> Result<Mail, String> {
    atomic(db, || parse_raw_inner(db, mail, raw))
}
fn parse_raw_inner(db: &Connection, mut mail: Mail, raw: &[u8]) -> Result<Mail, String> {
    if raw.len() > MAX_RAW {
        return Err("Message exceeds 25 MiB download limit".into());
    }
    let parsed = mail_parser::MessageParser::default()
        .parse(raw)
        .ok_or("Invalid MIME message")?;
    let addresses = |list: Option<mail_parser::AddressList<'_>>| -> (Vec<String>, bool) {
        let values: Vec<_> = list
            .map(|l| {
                l.mailboxes()
                    .filter_map(|a| {
                        let address = a.address()?.parse::<lettre::Address>().ok()?;
                        let name = a
                            .name()
                            .filter(|n| n.len() <= 128 && !n.chars().any(char::is_control))
                            .map(str::to_string);
                        Some(lettre::message::Mailbox::new(name, address).to_string())
                    })
                    .take(101)
                    .collect()
            })
            .unwrap_or_default();
        let truncated = values.len() > 100;
        (values.into_iter().take(100).collect(), truncated)
    };
    let (to, to_truncated) = addresses(parsed.to());
    let (cc, cc_truncated) = addresses(parsed.cc());
    let (reply_to, reply_truncated) = addresses(parsed.reply_to());
    mail.to = to;
    mail.cc = cc;
    mail.reply_to = reply_to;
    mail.recipients_truncated = to_truncated || cc_truncated || reply_truncated;
    mail.references = parsed
        .references()
        .map(|refs| crate::compose::reference_ids(refs.iter()))
        .unwrap_or_default();
    mail.subject = parsed.subject().unwrap_or("(No subject)").to_string();
    mail.message_id = parsed
        .message_id()
        .map(|id| format!("<{}>", id.trim_matches(['<', '>'])))
        .unwrap_or_default();
    if let Some(from) = parsed.from().and_then(|a| a.first()) {
        mail.address = from.address().unwrap_or("").to_string();
        mail.sender = from.name().unwrap_or(&mail.address).to_string()
    }
    mail.body = parsed
        .body_text(0)
        .map(|s| s.into_owned())
        .unwrap_or_else(|| "This message has no readable text body.".into());
    mail.body = mail.body.chars().take(1_000_000).collect();
    mail.html_body = parsed
        .html_body()
        .find(|part| matches!(part.kind(), mail_parser::PartKind::Html))
        .and_then(|part| part.text())
        .filter(|s| s.len() <= MAX_HTML)
        .map(|s| s.into_owned())
        .unwrap_or_default();
    mail.body_format_version = BODY_FORMAT_VERSION;
    mail.loaded = true;
    mail.attachments.clear();
    db.execute("DELETE FROM files WHERE message_id=?1", [&mail.id])
        .map_err(|e| e.to_string())?;
    for (i, part) in parsed.attachments().enumerate() {
        if i >= MAX_ATTACHMENTS {
            return Err("Message exceeds the 256-attachment download limit".into());
        }
        let data = part.decoded();
        let id = format!("{}:{i}", mail.id);
        let name = safe_filename(part.attachment_name().unwrap_or("attachment.bin"));
        let mut mime = part
            .content_type()
            .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("octet-stream")))
            .unwrap_or_else(|| "application/octet-stream".into());
        shorten(&mut mime, 256);
        db.execute(
            "INSERT INTO files VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(id) DO UPDATE SET message_id=excluded.message_id,name=excluded.name,mime=excluded.mime,data=excluded.data,created=excluded.created",
            params![id, mail.id, name, mime, data.as_ref(), now()],
        )
        .map_err(|e| e.to_string())?;
        mail.attachments.push(Attachment {
            id,
            name,
            size: data.len(),
            mime,
        });
    }
    db.execute(
        "INSERT INTO mail_raw VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET raw=excluded.raw,accessed=excluded.accessed",
        params![mail.id, raw, now()],
    )
    .map_err(|e| e.to_string())?;
    bounded_metadata(&mut mail)?;
    put(db, &mail)?;
    Ok(mail)
}
pub fn safe_filename(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_control()
                || matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')
                || "<>:\"/\\|?*".contains(c)
            {
                '_'
            } else {
                c
            }
        })
        .take(160)
        .collect();
    let clean = clean.trim_matches(['.', ' ']);
    let stem = clean
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_ascii_uppercase();
    let reserved = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || (stem.len() == 4
        && (stem.starts_with("COM") || stem.starts_with("LPT"))
        && matches!(stem.as_bytes()[3], b'1'..=b'9'));
    if clean.is_empty() {
        "attachment.bin".into()
    } else if reserved {
        format!("_{clean}")
    } else {
        clean.to_string()
    }
}
#[derive(Serialize)]
pub struct ActiveJob {
    #[serde(flatten)]
    job: Job,
    subject: Option<String>,
}

#[derive(Serialize)]
pub struct ActiveJobPage {
    jobs: Vec<ActiveJob>,
    next_offset: Option<i64>,
}

pub fn active_jobs(db: &Connection, offset: i64) -> Result<ActiveJobPage, String> {
    const PAGE_SIZE: i64 = 20;
    let offset = offset.max(0);
    let mut query = db
        .prepare(
            "SELECT id,account_id,kind,status,detail,due,
           CASE kind WHEN 'mutation' THEN (SELECT json_extract(mail.payload,'$.subject')
             FROM mail WHERE mail.id=json_extract(jobs.payload,'$.id'))
           WHEN 'send' THEN json_extract(jobs.payload,'$.subject') END
         FROM jobs WHERE status IN('pending','running','uncertain','failed')
         ORDER BY rowid DESC LIMIT ?1 OFFSET ?2",
        )
        .map_err(|e| e.to_string())?;
    let mut jobs = query
        .query_map(params![PAGE_SIZE + 1, offset], |row| {
            Ok(ActiveJob {
                job: job_row(row)?,
                subject: row.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let next_offset = (jobs.len() > PAGE_SIZE as usize).then(|| offset + PAGE_SIZE);
    jobs.truncate(PAGE_SIZE as usize);
    Ok(ActiveJobPage { jobs, next_offset })
}

fn job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    Ok(Job {
        id: row.get(0)?,
        account_id: row.get(1)?,
        kind: row.get(2)?,
        status: row.get(3)?,
        detail: row.get(4)?,
        due: row.get(5)?,
    })
}

pub fn jobs(db: &Connection) -> Result<Vec<Job>, String> {
    let fetch = |sql: &str, args: &[&dyn rusqlite::ToSql]| -> Result<Vec<Job>, String> {
        let mut q = db.prepare(sql).map_err(|e| e.to_string())?;
        let rows = q.query_map(args, job_row).map_err(|e| e.to_string())?;
        rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
    };
    let mut result=fetch("SELECT id,account_id,kind,status,detail,due FROM jobs WHERE status IN('pending','running','uncertain','failed') ORDER BY rowid DESC LIMIT 100",&[])?;
    if result.len() < 100 {
        result.extend(fetch("SELECT id,account_id,kind,status,detail,due FROM jobs WHERE created>?1 AND status NOT IN('pending','running','uncertain','failed') ORDER BY created DESC,rowid DESC LIMIT ?2",&[&(now()-7*86400),&((100-result.len()) as i64)])?);
    }
    Ok(result)
}

/// Evict downloaded bodies and files, retaining searchable metadata and all outgoing files.
pub fn trim_cache(db: &Connection, limit: i64, keep: &str) -> Result<usize, String> {
    let mut evicted = 0;
    loop {
        let size: i64 = db
            .query_row("SELECT bytes FROM cache_size WHERE singleton=1", [], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())?;
        if size <= limit {
            break;
        }
        let id: Option<String> = db
            .query_row(
                "SELECT id FROM mail_raw WHERE id!=?1 ORDER BY accessed,id LIMIT 1",
                [keep],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(id) = id else { break };
        db.execute("DELETE FROM mail_raw WHERE id=?1", [&id])
            .map_err(|e| e.to_string())?;
        db.execute("DELETE FROM files WHERE message_id=?1", [&id])
            .map_err(|e| e.to_string())?;
        if let Ok(mut m) = get(db, &id) {
            m.body = m.body.chars().take(240).collect();
            m.loaded = false;
            m.html_body.clear();
            m.attachments.clear();
            put(db, &m)?;
        }
        evicted += 1;
    }
    Ok(evicted)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn db() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        migrate(&db).unwrap();
        db
    }
    #[test]
    fn identities_do_not_cross_accounts() {
        assert_ne!(identity("a", "42"), identity("b", "42"));
        assert_ne!(identity("a", "42"), identity("a", "43"))
    }
    #[test]
    fn search_is_literal_and_scoped() {
        let db = db();
        for a in ["a", "b"] {
            let m = Mail {
                id: identity(a, "42"),
                account_id: a.into(),
                remote_id: "42".into(),
                subject: "Quarterly report".into(),
                folder: "inbox".into(),
                ..Default::default()
            };
            put(&db, &m).unwrap()
        }
        assert_eq!(query(&db, "inbox", "a", "Quarterly", 0).unwrap().total, 1);
        assert_eq!(query(&db, "inbox", "", "\" OR *", 0).unwrap().total, 0);
        assert_eq!(query(&db, "inbox", "", "Quarterly", 0).unwrap().total, 2)
    }
    #[test]
    fn unknown_sends_never_resume_automatically() {
        let db = db();
        db.execute(
            "INSERT INTO jobs VALUES('s','a','send','{}','running','',0,0)",
            [],
        )
        .unwrap();
        migrate(&db).unwrap();
        let status: String = db
            .query_row("SELECT status FROM jobs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "uncertain")
    }
    #[test]
    fn malformed_mime_and_attachment_names() {
        let db = db();
        assert!(parse_raw(&db, Mail::default(), b"\0\0").is_err());
        assert!(!safe_filename("../../evil.exe").contains('/'));
        let raw=b"From: A <a@example.com>\r\nSubject: hello\r\nMIME-Version: 1.0\r\nContent-Type: text/plain\r\n\r\nHello";
        assert_eq!(
            parse_raw(
                &db,
                Mail {
                    id: "x".into(),
                    account_id: "a".into(),
                    remote_id: "1".into(),
                    ..Default::default()
                },
                raw
            )
            .unwrap()
            .body,
            "Hello"
        )
    }
}
