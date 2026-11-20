use crate::{
    engine::{self, Draft, Mail, ServerConfig},
    providers::{self, db},
    Store,
};
use rusqlite::params;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tauri::Emitter;
#[derive(serde::Serialize, serde::Deserialize)]
struct UndoFields {
    id: String,
    account_id: String,
    unread: bool,
    starred: bool,
    folder: String,
    labels: Vec<String>,
}
#[derive(Default)]
pub struct Runtime {
    pub gate: tokio::sync::Mutex<()>,
    pub organize_gate: tokio::sync::Mutex<()>,
}
#[tauri::command]
pub fn query_mail(
    store: tauri::State<Store>,
    folder: String,
    account: String,
    search: String,
    offset: i64,
    cursor: Option<engine::MailCursor>,
    filters: Option<engine::MailFilters>,
) -> Result<engine::MailPage, String> {
    if search.len() > 2048 {
        return Err("Search too long".into());
    }
    db(&store, |d| {
        engine::query_filtered_conversations(
            d,
            &folder,
            &account,
            &search,
            filters.as_ref(),
            offset,
            cursor.as_ref(),
        )
    })
}
#[tauri::command]
pub fn get_conversation(store: tauri::State<Store>, id: String) -> Result<Vec<Mail>, String> {
    db(&store, |d| engine::conversation(d, &id))
}
#[tauri::command]
pub async fn sync_account(
    app: tauri::AppHandle,
    store: tauri::State<'_, Store>,
    runtime: tauri::State<'_, Arc<Runtime>>,
    id: String,
) -> Result<usize, String> {
    let _guard = runtime.gate.lock().await;
    db(&store, |db| {
        db.execute(
            "DELETE FROM sync_state WHERE account_id=?1 AND key='folder-catalog'",
            [&id],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    })?;
    let result = sync(&store, &id).await;
    if result.is_ok() {
        crate::notifications::after_sync(&app, &store, &id);
    }
    result
}
pub async fn sync(store: &Store, id: &str) -> Result<usize, String> {
    let (provider, _) = providers::account(store, id)?;
    let result = tokio::time::timeout(Duration::from_secs(300), async {
        match provider.as_str() {
            "google" => providers::sync_google(store, id).await,
            "microsoft" => providers::sync_graph(store, id).await,
            "imap" => providers::sync_imap(store, id).await,
            _ => Err("Unsupported provider".into()),
        }
    })
    .await
    .unwrap_or_else(|_| Err("Sync timed out. It will resume from the saved checkpoint.".into()));
    let status = match &result {
        Ok(n) => format!("Synced {n} messages"),
        Err(e) => e.clone(),
    };
    db(store, |d| engine::checkpoint(d, id, "status", &status))?;
    result
}
#[tauri::command]
pub async fn get_mail(
    store: tauri::State<'_, Store>,
    runtime: tauri::State<'_, Arc<Runtime>>,
    id: String,
) -> Result<Mail, String> {
    get_mail_inner(&store, &runtime, &id).await
}
pub async fn get_mail_inner(store: &Store, runtime: &Runtime, id: &str) -> Result<Mail, String> {
    let cached = db(store, |d| {
        engine::upgrade_cached_body(d, engine::get(d, id)?)
    })?;
    if cached.loaded {
        return Ok(cached);
    }
    let _guard = runtime.gate.lock().await;
    let m = db(store, |d| {
        engine::upgrade_cached_body(d, engine::get(d, id)?)
    })?;
    if m.loaded {
        return Ok(m);
    }
    let raw = tokio::time::timeout(Duration::from_secs(90), providers::raw_message(store, &m))
        .await
        .map_err(|_| "Message download timed out")??;
    db(store, |d| {
        let tx = d.unchecked_transaction().map_err(|e| e.to_string())?;
        let m = engine::parse_raw(&tx, m, &raw)?;
        engine::trim_cache(&tx, 1024 * 1024 * 1024, &m.id)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(m)
    })
}
#[tauri::command]
pub async fn connect_imap(
    store: tauri::State<'_, Store>,
    runtime: tauri::State<'_, Arc<Runtime>>,
    config: ServerConfig,
    password: String,
    smtp_password: String,
) -> Result<crate::auth::Account, String> {
    config.validate()?;
    if password.is_empty()
        || smtp_password.is_empty()
        || password.len() > 4096
        || smtp_password.len() > 4096
    {
        return Err("Enter valid server/app credentials".into());
    }
    for host in [&config.imap_host, &config.smtp_host] {
        let h = host.to_ascii_lowercase();
        if h.ends_with("gmail.com")
            || h.ends_with("googlemail.com")
            || h.ends_with("outlook.com")
            || h.ends_with("office365.com")
        {
            return Err("Use Continue with Google or Microsoft for this provider".into());
        }
    }
    let _guard = runtime.gate.lock().await;
    let id = format!(
        "imap:{}",
        engine::identity(
            &config.imap_host,
            &format!("{}:{}", config.imap_port, config.username)
        )
    );
    // Validate before changing stored account configuration or credentials.
    providers::validate_imap_credentials(&config, &password).await?;
    for (service, value) in [
        ("dev.rustymail.imap", password),
        ("dev.rustymail.smtp", smtp_password),
    ] {
        keyring::Entry::new(service, &id)
            .map_err(|_| "Credential storage unavailable")?
            .set_password(&value)
            .map_err(|_| "Cannot securely save mail credential")?;
    }
    db(&store, |d| {
        let tx = d.unchecked_transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT OR REPLACE INTO accounts VALUES(?1,'imap',?2)",
            params![id, config.email],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT OR REPLACE INTO account_config VALUES(?1,?2)",
            params![
                id,
                serde_json::to_string(&config).map_err(|e| e.to_string())?
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    })?;
    Ok(crate::auth::Account {
        id,
        provider: "imap".into(),
        email: config.email,
    })
}
#[tauri::command]
pub fn account_status(store: tauri::State<Store>, id: String) -> Result<String, String> {
    db(&store, |d| {
        Ok(engine::state(d, &id, "status")?.unwrap_or_else(|| "Not yet synchronized".into()))
    })
}
#[tauri::command]
pub async fn queue_mutation(
    app: tauri::AppHandle,
    store: tauri::State<'_, Store>,
    runtime: tauri::State<'_, Arc<Runtime>>,
    id: String,
    action: String,
) -> Result<String, String> {
    let _organize = runtime.organize_gate.lock().await;
    let job = queue_mutation_inner(&store, &id, &action)?;
    jobs_changed(&app, false);
    Ok(job)
}
pub fn queue_mutation_inner(store: &Store, id: &str, action: &str) -> Result<String, String> {
    if ![
        "inbox", "archive", "trash", "spam", "read", "unread", "star", "unstar",
    ]
    .contains(&action)
    {
        return Err("Invalid mail action".into());
    }
    db(store, |d| {
        let tx = d.unchecked_transaction().map_err(|e| e.to_string())?;
        let message = engine::get(&tx, id)?;
        let job = queue_message_mutation(&tx, message, action)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(job)
    })
}

#[tauri::command]
pub async fn queue_bulk_mutation(
    app: tauri::AppHandle,
    store: tauri::State<'_, Store>,
    runtime: tauri::State<'_, Arc<Runtime>>,
    scope: engine::MailScope,
    row_ids: Vec<String>,
    action: String,
) -> Result<usize, String> {
    let _organize = runtime.organize_gate.lock().await;
    let queued = queue_bulk_mutation_inner(&store, &scope, &row_ids, &action)?;
    if queued > 0 {
        jobs_changed(&app, false);
    }
    Ok(queued)
}

pub fn queue_bulk_mutation_inner(
    store: &Store,
    scope: &engine::MailScope,
    row_ids: &[String],
    action: &str,
) -> Result<usize, String> {
    if !matches!(action, "archive" | "trash" | "read") {
        return Err("Invalid bulk mail action".into());
    }
    if row_ids.is_empty()
        || row_ids.len() > 100
        || row_ids.iter().any(|id| id.is_empty() || id.len() > 512)
    {
        return Err("Select between 1 and 100 conversations.".into());
    }
    let mut row_ids = row_ids.to_vec();
    row_ids.sort_unstable();
    row_ids.dedup();
    db(store, |d| {
        let tx = d.unchecked_transaction().map_err(|e| e.to_string())?;
        let ids = engine::bulk_mail_ids(&tx, scope, &row_ids)?;
        let mut queued = 0;
        for id in ids {
            let message = engine::get(&tx, &id)?;
            if (action == "read" && message.unread)
                || (action != "read" && message.folder != action)
            {
                queue_message_mutation(&tx, message, action)?;
                queued += 1;
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(queued)
    })
}

fn queue_message_mutation(
    db: &rusqlite::Connection,
    mut m: Mail,
    action: &str,
) -> Result<String, String> {
    let id = m.id.clone();
    let job = hex::encode(rand::random::<[u8; 16]>());
    let original = UndoFields {
        id: m.id.clone(),
        account_id: m.account_id.clone(),
        unread: m.unread,
        starred: m.starred,
        folder: m.folder.clone(),
        labels: m.labels.clone(),
    };
    db.execute(
        "INSERT INTO jobs VALUES(?1,?2,'mutation',?3,'pending',?5,?4,?4)",
        params![
            job,
            m.account_id,
            json!({"id":id,"action":action,"original":original}).to_string(),
            engine::now(),
            format!("Queued {action}")
        ],
    )
    .map_err(|e| e.to_string())?;
    match action {
        "read" => m.unread = false,
        "unread" => m.unread = true,
        "star" => m.starred = true,
        "unstar" => m.starred = false,
        _ => {
            m.folder = action.to_string();
            if m.provider == "google" {
                m.labels
                    .retain(|label| !matches!(label.as_str(), "INBOX" | "TRASH" | "SPAM"));
                match action {
                    "inbox" => m.labels.push("INBOX".into()),
                    "trash" => m.labels.push("TRASH".into()),
                    "spam" => m.labels.push("SPAM".into()),
                    _ => {}
                }
            } else {
                m.labels.clear();
            }
        }
    }
    engine::put(db, &m)?;
    Ok(job)
}

#[tauri::command]
pub fn snooze_mail(store: tauri::State<Store>, id: String, wake: i64) -> Result<(), String> {
    if wake <= engine::now() {
        return Err("Snooze must be in the future".into());
    }
    db(&store, |d| {
        engine::get(d, &id)?;
        d.execute(
            "INSERT OR REPLACE INTO snoozes VALUES(?1,?2)",
            params![id, wake],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}
#[tauri::command]
pub fn next_snooze_wake(store: tauri::State<Store>) -> Result<Option<i64>, String> {
    db(&store, |d| {
        d.query_row(
            "SELECT min(wake) FROM snoozes WHERE wake>?1",
            [engine::now()],
            |r| r.get(0),
        )
        .map_err(|error| error.to_string())
    })
}
#[tauri::command]
pub fn queue_send(
    app: tauri::AppHandle,
    store: tauri::State<Store>,
    draft: Draft,
    draft_id: String,
) -> Result<String, String> {
    let job = queue_send_with_draft(&store, draft, Some(&draft_id))?;
    jobs_changed(&app, true);
    Ok(job)
}

#[cfg(test)]
pub fn queue_send_inner(store: &Store, draft: Draft) -> Result<String, String> {
    queue_send_with_draft(store, draft, None)
}

pub fn queue_send_with_draft(
    store: &Store,
    draft: Draft,
    draft_id: Option<&str>,
) -> Result<String, String> {
    let editable_payload = draft_id
        .map(|_| serde_json::to_string(&draft).map_err(|e| e.to_string()))
        .transpose()?;
    let draft = crate::compose::prepare_send(store, draft)?;
    let id = hex::encode(rand::random::<[u8; 16]>());
    let raw = providers::build_mime(store, &draft, &id)?;
    db(store, |d| {
        let tx = d.unchecked_transaction().map_err(|e| e.to_string())?;
        if let Some(draft_id) = draft_id {
            let changed = tx
                .execute(
                    "UPDATE saved_drafts SET queued_job_id=?1
                 WHERE id=?2 AND account_id=?3 AND payload=?4 AND queued_job_id IS NULL",
                    params![id, draft_id, draft.account_id, editable_payload],
                )
                .map_err(|e| e.to_string())?;
            if changed == 0 {
                return Err(
                    "This draft changed or was already queued. Save it and try again.".into(),
                );
            }
        }
        let bytes: i64 = tx
            .query_row(
                "SELECT coalesce(sum(length(raw)),0) FROM outbox_mime",
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if bytes + raw.len() as i64 > 100 * 1024 * 1024 {
            return Err(
                "Pending outbox exceeds 100 MiB. Finish or cancel queued sends first.".into(),
            );
        }
        tx.execute("INSERT INTO outbox_mime VALUES(?1,?2)", params![id, raw])
            .map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO jobs VALUES(?1,?2,'send',?3,'pending','Waiting for the 10-second cancellation window',?4,?5)",params![id,draft.account_id,serde_json::to_string(&draft).map_err(|e|e.to_string())?,engine::now()+10,engine::now()]).map_err(|e|e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(id)
    })
}
#[tauri::command]
pub fn list_jobs(store: tauri::State<Store>) -> Result<Vec<engine::Job>, String> {
    db(&store, engine::jobs)
}
#[tauri::command]
pub fn list_active_jobs(
    store: tauri::State<Store>,
    offset: i64,
) -> Result<engine::ActiveJobPage, String> {
    db(&store, |d| engine::active_jobs(d, offset))
}

#[tauri::command]
pub fn cancel_job(
    app: tauri::AppHandle,
    store: tauri::State<Store>,
    id: String,
) -> Result<(), String> {
    cancel_job_inner(&store, &id)?;
    jobs_changed(&app, true);
    Ok(())
}
fn mutation_field(action: &str) -> &str {
    match action {
        "read" | "unread" => "unread",
        "star" | "unstar" => "starred",
        _ => "folder",
    }
}
pub fn cancel_job_inner(store: &Store, id: &str) -> Result<(), String> {
    db(store, |d| {
        let tx = d.unchecked_transaction().map_err(|e| e.to_string())?;
        let (rowid, kind, payload): (i64, String, String) = tx
            .query_row(
                "SELECT rowid,kind,payload FROM jobs WHERE id=?1 AND status='pending'",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|_| "Submission has already started; cancellation is no longer possible")?;
        let n=tx.execute("UPDATE jobs SET status='cancelled',detail='Cancelled before submission' WHERE id=?1 AND status='pending'",[&id]).map_err(|e|e.to_string())?;
        if n == 0 {
            return Err(
                "Submission has already started; cancellation is no longer possible".into(),
            );
        }
        if kind == "mutation" {
            let v: serde_json::Value =
                serde_json::from_str(&payload).map_err(|_| "Invalid operation")?;
            let original: UndoFields =
                serde_json::from_value(v["original"].clone()).map_err(|_| "Invalid undo record")?;
            let field = mutation_field(v["action"].as_str().ok_or("Invalid undo action")?);
            let mut q=tx.prepare("SELECT payload,status FROM jobs WHERE kind='mutation' AND json_extract(payload,'$.id')=?1 AND rowid>?2 AND status!='cancelled'").map_err(|e|e.to_string())?;
            let later = q
                .query_map(params![original.id, rowid], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(|e| e.to_string())?;
            let mut superseded = false;
            for next in later {
                let (payload, status) = next.map_err(|e| e.to_string())?;
                let next: serde_json::Value =
                    serde_json::from_str(&payload).map_err(|_| "Invalid later operation")?;
                if mutation_field(next["action"].as_str().ok_or("Invalid later action")?) == field {
                    if status != "done" {
                        return Err("Cancel the newer action on this message first".into());
                    }
                    superseded = true;
                }
            }
            if !superseded {
                if let Ok(mut current) = engine::get(&tx, &original.id) {
                    match field {
                        "unread" => current.unread = original.unread,
                        "starred" => current.starred = original.starred,
                        _ => {
                            current.folder = original.folder;
                            current.labels = original.labels;
                        }
                    }
                    engine::put(&tx, &current)?;
                }
            }
        } else if kind == "send" {
            tx.execute(
                "UPDATE saved_drafts SET queued_job_id=NULL WHERE queued_job_id=?1",
                [&id],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.execute("DELETE FROM outbox_mime WHERE job_id=?1", [&id])
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    })
}
#[tauri::command]
pub fn outgoing_attachments(
    store: tauri::State<Store>,
    account_id: String,
    ids: Vec<String>,
) -> Result<Vec<engine::Attachment>, String> {
    if ids.len() > 20 {
        return Err("Too many attachments".into());
    }
    db(&store, |d| {
        let mut result = Vec::new();
        for id in ids {
            result.push(d.query_row("SELECT f.id,f.name,f.mime,length(f.data) FROM files f JOIN outgoing_owners o ON o.file_id=f.id WHERE f.id=?1 AND f.message_id='outgoing' AND o.account_id=?2",params![id,account_id],|r|Ok(engine::Attachment{id:r.get(0)?,name:r.get(1)?,mime:r.get(2)?,size:r.get::<_,i64>(3)? as usize})).map_err(|_|"Draft attachment is no longer cached for this account; reattach it")?)
        }
        Ok(result)
    })
}
#[tauri::command]
pub fn resolve_send(
    app: tauri::AppHandle,
    store: tauri::State<Store>,
    id: String,
) -> Result<(), String> {
    db(&store, |d| {
        let tx = d.unchecked_transaction().map_err(|e| e.to_string())?;
        let n=tx.execute("UPDATE jobs SET status='reviewed',detail='User reviewed Sent; no automatic retry' WHERE id=?1 AND kind='send' AND status='uncertain'",[&id]).map_err(|e|e.to_string())?;
        if n == 0 {
            return Err("This send does not need review".into());
        }
        tx.execute("DELETE FROM saved_drafts WHERE queued_job_id=?1", [&id])
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM outbox_mime WHERE job_id=?1", [&id])
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    })?;
    jobs_changed(&app, true);
    Ok(())
}
#[tauri::command]
pub fn retry_mutation(
    app: tauri::AppHandle,
    store: tauri::State<Store>,
    id: String,
) -> Result<(), String> {
    db(&store, |d| {
        d.execute("UPDATE jobs SET status='pending',detail='',due=?2 WHERE id=?1 AND kind='mutation' AND status='failed'",params![id,engine::now()]).map_err(|e|e.to_string())
    }).map(|changed| {
        if changed > 0 {
            jobs_changed(&app, false);
        }
    })
}
#[tauri::command]
pub async fn pick_attachments(
    store: tauri::State<'_, Store>,
    account_id: String,
) -> Result<Vec<engine::Attachment>, String> {
    providers::account(&store, &account_id)?;
    let files = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Attach files (18 MiB total)")
            .pick_files()
    })
    .await
    .map_err(|_| "File dialog unavailable")?;
    let Some(paths) = files else {
        return Ok(Vec::new());
    };
    if paths.len() > 20 {
        return Err("Choose at most 20 attachments".into());
    }
    let mut result = Vec::new();
    let mut total = 0;
    for path in paths {
        let size = std::fs::metadata(&path)
            .map_err(|_| "Cannot read attachment")?
            .len() as usize;
        if total + size > 18 * 1024 * 1024 {
            return Err("Attachments exceed 18 MiB".into());
        }
        use std::io::Read;
        let mut data = Vec::new();
        std::fs::File::open(&path)
            .map_err(|_| "Cannot open attachment")?
            .take((18 * 1024 * 1024 - total + 1) as u64)
            .read_to_end(&mut data)
            .map_err(|_| "Cannot read attachment")?;
        total += data.len();
        if total > 18 * 1024 * 1024 {
            return Err("Attachment changed or exceeds 18 MiB".into());
        }
        let id = hex::encode(rand::random::<[u8; 16]>());
        let name = engine::safe_filename(
            path.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("attachment.bin"),
        );
        db(&store, |d| {
            let tx = d.unchecked_transaction().map_err(|e| e.to_string())?;
            tx.execute(
                "INSERT INTO files VALUES(?1,'outgoing',?2,'application/octet-stream',?3,?4)",
                params![id, name, data, engine::now()],
            )
            .map_err(|e| e.to_string())?;
            tx.execute(
                "INSERT INTO outgoing_owners VALUES(?1,?2)",
                params![id, account_id],
            )
            .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            Ok(())
        })?;
        result.push(engine::Attachment {
            id,
            name,
            size: data.len(),
            mime: "application/octet-stream".into(),
        })
    }
    Ok(result)
}
#[tauri::command]
pub async fn save_attachment(
    store: tauri::State<'_, Store>,
    message_id: String,
    id: String,
) -> Result<bool, String> {
    let (name, data): (String, Vec<u8>) = db(&store, |d| {
        d.query_row(
            "SELECT name,data FROM files WHERE id=?1 AND message_id=?2",
            params![id, message_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|_| "Attachment is not cached. Open its message first.".into())
    })?;
    let name = engine::safe_filename(&name);
    let path = tokio::task::spawn_blocking(move || {
        rfd::FileDialog::new()
            .set_title("Save attachment")
            .set_file_name(name)
            .save_file()
    })
    .await
    .map_err(|_| "File dialog unavailable")?;
    if let Some(path) = path {
        tokio::task::spawn_blocking(move || std::fs::write(path, data))
            .await
            .map_err(|_| "File save task stopped")?
            .map_err(|_| "Could not save attachment")?;
        return Ok(true);
    }
    Ok(false)
}
fn jobs_changed(app: &tauri::AppHandle, drafts_changed: bool) {
    let _ = app.emit("jobs-changed", drafts_changed);
}

#[cfg(test)]
pub async fn process_jobs(store: &Store) -> Result<bool, String> {
    process_job(store, |_| {}).await
}

async fn process_job(store: &Store, on_change: impl Fn(bool)) -> Result<bool, String> {
    let next: Option<(String, String, String)> = db(store, |d| {
        use rusqlite::OptionalExtension;
        d.query_row("SELECT id,kind,payload FROM jobs WHERE status='pending' AND due<=?1 ORDER BY rowid LIMIT 1",[engine::now()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|e.to_string())
    })?;
    let Some((id, kind, payload)) = next else {
        return Ok(false);
    };
    let changed = db(store, |d| {
        d.execute("UPDATE jobs SET status='running',detail='Submitting to provider' WHERE id=?1 AND status='pending'",[&id]).map_err(|e|e.to_string())
    })?;
    if changed == 0 {
        return Ok(false);
    }
    on_change(false);
    let mut safe_to_restore = false;
    let result = if kind == "send" {
        match serde_json::from_str::<Draft>(&payload) {
            Ok(draft) => match queued_mime(store, &id) {
                Ok(raw) => providers::send(store, &id, &draft, raw).await,
                Err(error) => {
                    safe_to_restore = true;
                    Err(error)
                }
            },
            Err(_) => {
                safe_to_restore = true;
                Err("Invalid queued draft".into())
            }
        }
    } else {
        match serde_json::from_str::<serde_json::Value>(&payload) {
            Ok(v) => match db(store, |d| engine::get(d, v["id"].as_str().unwrap_or(""))) {
                Ok(m) => providers::mutate(store, &m, v["action"].as_str().unwrap_or("")).await,
                Err(e) => Err(e),
            },
            Err(_) => Err("Invalid operation".into()),
        }
    };
    let (status, detail) = match result {
        Ok(()) => ("done", "Provider accepted the operation".to_string()),
        Err(e) if kind == "send" && !safe_to_restore => (
            "uncertain",
            format!("{e}. Check Sent before composing another send; automatic retry is disabled."),
        ),
        Err(e) => ("failed", e),
    };
    db(store, |d| {
        let tx = d.unchecked_transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE jobs SET status=?2,detail=?3 WHERE id=?1",
            params![id, status, detail],
        )
        .map_err(|e| e.to_string())?;
        if safe_to_restore {
            tx.execute(
                "UPDATE saved_drafts SET queued_job_id=NULL WHERE queued_job_id=?1",
                [&id],
            )
            .map_err(|e| e.to_string())?;
        }
        if status == "done" {
            if kind == "send" {
                tx.execute("DELETE FROM saved_drafts WHERE queued_job_id=?1", [&id])
                    .map_err(|e| e.to_string())?;
            }
            tx.execute("DELETE FROM outbox_mime WHERE job_id=?1", [&id])
                .map_err(|e| e.to_string())?;
            tx.execute(
                "DELETE FROM sync_state WHERE key=?1",
                [format!("graph-reply:{id}")],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    })?;
    on_change(safe_to_restore || (kind == "send" && status == "done"));
    Ok(true)
}
pub fn queued_mime(store: &Store, id: &str) -> Result<Vec<u8>, String> {
    db(store, |d| {
        d.query_row("SELECT raw FROM outbox_mime WHERE job_id=?1", [id], |r| {
            r.get(0)
        })
        .map_err(|_| "Queued send has no immutable content snapshot; review and requeue it".into())
    })
}
pub fn start_worker(app: tauri::AppHandle, store: Store, runtime: Arc<Runtime>) {
    tauri::async_runtime::spawn(async move {
        let sync_interval = Duration::from_secs(60);
        let mut next_sync = tokio::time::Instant::now() + sync_interval;
        let mut accounts = std::collections::VecDeque::new();
        loop {
            if accounts.is_empty() {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            if accounts.is_empty() && tokio::time::Instant::now() >= next_sync {
                accounts = db(&store, |d| {
                    let mut q = d
                        .prepare("SELECT id FROM accounts ORDER BY id")
                        .map_err(|e| e.to_string())?;
                    let ids = q
                        .query_map([], |r| r.get::<_, String>(0))
                        .map_err(|e| e.to_string())?
                        .collect::<Result<std::collections::VecDeque<_>, _>>()
                        .map_err(|e| e.to_string())?;
                    Ok(ids)
                })
                .unwrap_or_default();
                next_sync = tokio::time::Instant::now() + sync_interval;
            }
            let account = accounts.pop_front();
            let completing_sync = account.is_some() && accounts.is_empty();
            run_worker_step(
                &store,
                &runtime,
                |drafts| jobs_changed(&app, drafts),
                account.map(|id| {
                    let app = &app;
                    let store = &store;
                    async move {
                        if sync(store, &id).await.is_ok() {
                            crate::notifications::after_sync(app, store, &id);
                        }
                        let _ = app.emit("mail-changed", ());
                    }
                }),
            )
            .await;
            if completing_sync {
                next_sync = tokio::time::Instant::now() + sync_interval;
            }
        }
    });
}

async fn run_worker_step(
    store: &Store,
    runtime: &Runtime,
    on_job_change: impl Fn(bool),
    sync_account: Option<impl std::future::Future<Output = ()>>,
) {
    {
        let _guard = runtime.gate.lock().await;
        let _ = process_job(store, on_job_change).await;
    }
    if let Some(sync_account) = sync_account {
        let _guard = runtime.gate.lock().await;
        sync_account.await;
    }
}

#[cfg(test)]
mod worker_tests {
    use super::*;

    #[tokio::test]
    async fn due_job_runs_between_account_syncs() {
        let database = rusqlite::Connection::open_in_memory().unwrap();
        engine::migrate(&database).unwrap();
        let store = Store(Arc::new(std::sync::Mutex::new(database)));
        let runtime = Runtime::default();
        let trace = std::cell::RefCell::new(Vec::new());
        let on_change = |_| {
            let status = db(&store, |d| {
                d.query_row("SELECT status FROM jobs WHERE id='due'", [], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|e| e.to_string())
            })
            .unwrap();
            trace.borrow_mut().push(status);
        };
        run_worker_step(
            &store,
            &runtime,
            on_change,
            Some(async {
                trace.borrow_mut().push("first sync".into());
                db(&store, |d| {
                    d.execute(
                        "INSERT INTO jobs VALUES('due','first','mutation','{}','pending','',?1,?1)",
                        [engine::now()],
                    )
                    .map_err(|e| e.to_string())?;
                    Ok(())
                })
                .unwrap();
            }),
        )
        .await;
        run_worker_step(
            &store,
            &runtime,
            on_change,
            Some(async {
                trace.borrow_mut().push("second sync".into());
            }),
        )
        .await;
        assert_eq!(
            trace.into_inner(),
            ["first sync", "running", "failed", "second sync"]
        );
    }
}
