use crate::{
    engine::{self, Draft, Mail, ServerConfig},
    outgoing,
    providers::{self, db},
    reconcile, Store,
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
    let result = db(&store, |db| {
        db.execute(
            "DELETE FROM sync_state WHERE account_id=?1 AND key='folder-catalog'",
            [&id],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    });
    let result = match result {
        Ok(()) => sync(&store, &id).await,
        Err(error) => Err(error),
    };
    if result.is_ok() {
        crate::notifications::after_sync(&app, &store, &id);
    }
    let _ = app.emit("sync-status-changed", ());
    result
}
async fn sync(store: &Store, id: &str) -> Result<usize, String> {
    let result = tokio::time::timeout(Duration::from_secs(300), async {
        let (provider, _) = providers::account(store, id)?;
        match provider.as_str() {
            "google" => providers::sync_google(store, id).await,
            "microsoft" => providers::sync_graph(store, id).await,
            "imap" => providers::sync_imap(store, id).await,
            _ => Err("Unsupported provider".into()),
        }
    })
    .await
    .unwrap_or_else(|_| Err("Sync timed out. It will resume from the saved checkpoint.".into()));
    db(store, |d| record_sync_status(d, id, &result))?;
    result
}

fn record_sync_status(
    db: &rusqlite::Connection,
    id: &str,
    result: &Result<usize, String>,
) -> Result<(), String> {
    let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
    let status = match &result {
        Ok(n) => {
            engine::checkpoint(&tx, id, "sync-success", &engine::now().to_string())?;
            format!("Synced {n} messages")
        }
        Err(e) => e.clone(),
    };
    engine::checkpoint(&tx, id, "status", &status)?;
    engine::checkpoint(
        &tx,
        id,
        "sync-error",
        result.as_ref().err().map(String::as_str).unwrap_or(""),
    )?;
    tx.commit().map_err(|e| e.to_string())
}

#[derive(serde::Serialize)]
pub struct SyncStatus {
    account_id: String,
    last_success: Option<i64>,
    error: Option<String>,
}

fn sync_statuses(db: &rusqlite::Connection) -> Result<Vec<SyncStatus>, String> {
    let mut query = db
        .prepare(
            "SELECT a.id,CAST(s.value AS INTEGER),nullif(coalesce(e.value,
           CASE WHEN legacy.value NOT LIKE 'Synced % messages' THEN legacy.value END),'')
         FROM accounts a
         LEFT JOIN sync_state s ON s.account_id=a.id AND s.key='sync-success'
         LEFT JOIN sync_state e ON e.account_id=a.id AND e.key='sync-error'
         LEFT JOIN sync_state legacy ON legacy.account_id=a.id AND legacy.key='status'",
        )
        .map_err(|e| e.to_string())?;
    let rows = query
        .query_map([], |row| {
            Ok(SyncStatus {
                account_id: row.get(0)?,
                last_success: row.get(1)?,
                error: row.get(2)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_sync_statuses(store: tauri::State<Store>) -> Result<Vec<SyncStatus>, String> {
    db(&store, sync_statuses)
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
    reconcile::remember(db, &m)?;
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
    retire_failed(db, &m.account_id, &id, action)?;
    reconcile::apply(&mut m, action);
    engine::put(db, &m)?;
    Ok(job)
}

#[tauri::command]
pub fn snooze_mail(store: tauri::State<Store>, id: String, wake: i64) -> Result<(), String> {
    db(&store, |d| store_snooze(d, &id, wake))
}

fn store_snooze(db: &rusqlite::Connection, id: &str, wake: i64) -> Result<(), String> {
    if wake <= engine::now() {
        return Err("Snooze must be in the future".into());
    }
    let changed = db
        .execute(
            "INSERT INTO snoozes(id,wake) SELECT id,?2 FROM mail WHERE id=?1
         ON CONFLICT(id) DO UPDATE SET wake=excluded.wake",
            params![id, wake],
        )
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        return Err("Message not found".into());
    }
    Ok(())
}

#[tauri::command]
pub fn message_snooze_wake(store: tauri::State<Store>, id: String) -> Result<Option<i64>, String> {
    db(&store, |d| active_snooze(d, &id))
}

fn active_snooze(db: &rusqlite::Connection, id: &str) -> Result<Option<i64>, String> {
    use rusqlite::OptionalExtension;
    db.query_row(
        "SELECT wake FROM snoozes WHERE id=?1 AND wake>?2",
        params![id, engine::now()],
        |row| row.get(0),
    )
    .optional()
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn unsnooze_mail(store: tauri::State<Store>, id: String) -> Result<(), String> {
    db(&store, |d| clear_snooze(d, &id))
}

fn clear_snooze(db: &rusqlite::Connection, id: &str) -> Result<(), String> {
    db.execute("DELETE FROM snoozes WHERE id=?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
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
    let _ = app.emit("mail-changed", ());
    Ok(())
}
fn retire_failed(
    d: &rusqlite::Connection,
    account: &str,
    id: &str,
    action: &str,
) -> Result<(), String> {
    d.execute(
        "UPDATE jobs SET status='superseded',detail='A newer action replaced this one'
         WHERE account_id=?1 AND kind='mutation' AND status='failed' AND json_extract(payload,'$.id')=?2
           AND (CASE json_extract(payload,'$.action')
             WHEN 'read' THEN 'unread' WHEN 'unread' THEN 'unread'
             WHEN 'star' THEN 'starred' WHEN 'unstar' THEN 'starred' ELSE 'folder' END)=?3",
        params![account,id,reconcile::field(action)],
    ).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(serde::Deserialize)]
struct QueuedMutation {
    id: String,
    action: String,
}

fn retire_if_superseded(
    db: &rusqlite::Connection,
    job: &str,
    operation: &QueuedMutation,
) -> Result<(), String> {
    db.execute(
        "UPDATE jobs AS j SET status='superseded', detail='A newer action replaced this one'
         WHERE j.id=?1 AND EXISTS (
           SELECT 1 FROM jobs n WHERE n.kind='mutation' AND n.account_id=j.account_id
             AND n.rowid>j.rowid AND n.status NOT IN ('cancelled','superseded','reviewed')
             AND json_extract(n.payload,'$.id')=?2
             AND (CASE json_extract(n.payload,'$.action')
               WHEN 'read' THEN 'unread' WHEN 'unread' THEN 'unread'
               WHEN 'star' THEN 'starred' WHEN 'unstar' THEN 'starred' ELSE 'folder' END)=?3)",
        params![job, operation.id, reconcile::field(&operation.action)],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
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
            let field = reconcile::field(v["action"].as_str().ok_or("Invalid undo action")?);
            let mut q=tx.prepare("SELECT payload,status FROM jobs WHERE kind='mutation' AND json_extract(payload,'$.id')=?1 AND rowid>?2 AND status NOT IN ('cancelled','superseded','reviewed')").map_err(|e|e.to_string())?;
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
                if reconcile::field(next["action"].as_str().ok_or("Invalid later action")?) == field
                {
                    if status != "done" {
                        return Err("Cancel the newer action on this message first".into());
                    }
                    superseded = true;
                }
            }
            if !superseded && !reconcile::can_retry(&tx, &original.id)? {
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
            reconcile::reproject(&tx, &original.id)?;
        } else if kind == "send" {
            tx.execute(
                "UPDATE saved_drafts SET queued_job_id=NULL WHERE queued_job_id=?1",
                [&id],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.execute("DELETE FROM outbox_mime WHERE job_id=?1", [&id])
            .map_err(|e| e.to_string())?;
        if kind == "send" {
            outgoing::reclaim_files(&tx, &outgoing::payload_files(&payload))?;
        }
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
        let payload: String = tx
            .query_row("SELECT payload FROM jobs WHERE id=?1", [&id], |row| {
                row.get(0)
            })
            .map_err(|e| e.to_string())?;
        outgoing::reclaim_files(&tx, &outgoing::payload_files(&payload))?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    })?;
    jobs_changed(&app, true);
    Ok(())
}
#[tauri::command]
pub async fn retry_mutation(
    app: tauri::AppHandle,
    store: tauri::State<'_, Store>,
    runtime: tauri::State<'_, Arc<Runtime>>,
    id: String,
) -> Result<(), String> {
    let needs_refresh = db(&store, |d| {
        let message = retry_candidate(d, &id)?;
        Ok(!reconcile::can_retry(d, &message)?)
    })?;
    if needs_refresh {
        let _guard = runtime.gate.lock().await;
        let message = db(&store, |d| {
            reconcile::review_target(d, &retry_candidate(d, &id)?)
        })?;
        let observation = timed_mutation_observation(&store, &message).await?;
        db(&store, |d| {
            let tx = d.unchecked_transaction().map_err(|e| e.to_string())?;
            apply_mutation_observation(&tx, &message.id, observation)?;
            tx.commit().map_err(|e| e.to_string())
        })?;
        let _ = app.emit("mail-changed", ());
    }
    retry_mutation_inner(&store, &id)?;
    jobs_changed(&app, false);
    let _ = app.emit("mail-changed", ());
    Ok(())
}

fn retry_mutation_inner(store: &Store, id: &str) -> Result<(), String> {
    db(store, |d| {
        let tx = d.unchecked_transaction().map_err(|e| e.to_string())?;
        let message = retry_candidate(&tx, id)?;
        if !reconcile::can_retry(&tx, &message)? {
            return Err("The provider could not confirm this message for retry".into());
        }
        tx.execute(
            "UPDATE jobs SET status='pending',detail='',due=?2 WHERE id=?1",
            params![id, engine::now()],
        )
        .map_err(|e| e.to_string())?;
        reconcile::reproject(&tx, &message)?;
        tx.commit().map_err(|e| e.to_string())
    })
}

fn retry_candidate(db: &rusqlite::Connection, id: &str) -> Result<String, String> {
    let (order, account, message, action): (i64, String, String, String) = db
        .query_row(
            "SELECT rowid,account_id,json_extract(payload,'$.id'),json_extract(payload,'$.action')
             FROM jobs WHERE id=?1 AND kind='mutation' AND status='failed'",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .map_err(|_| "This action is no longer available to retry")?;
    let newer: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM jobs
             WHERE kind='mutation' AND account_id=?1 AND json_extract(payload,'$.id')=?2
               AND rowid>?3 AND status NOT IN ('cancelled','superseded','reviewed')
               AND (CASE json_extract(payload,'$.action')
                 WHEN 'read' THEN 'unread' WHEN 'unread' THEN 'unread'
                 WHEN 'star' THEN 'starred' WHEN 'unstar' THEN 'starred' ELSE 'folder' END)=?4)",
            params![account, message, order, reconcile::field(&action)],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if newer {
        return Err("A newer action replaced this one".into());
    }
    Ok(message)
}

async fn timed_mutation_observation(store: &Store, message: &Mail) -> Result<Option<Mail>, String> {
    tokio::time::timeout(
        Duration::from_secs(45),
        providers::refresh_mutation_target(store, message),
    )
    .await
    .map_err(|_| "Mailbox action review timed out")?
}

fn apply_mutation_observation(
    db: &rusqlite::Connection,
    id: &str,
    observation: Option<Mail>,
) -> Result<(), String> {
    match observation {
        Some(observed) => reconcile::put_remote(db, &observed),
        None => reconcile::remove_remote(db, id, None),
    }
}

#[tauri::command]
pub async fn review_mutation(
    app: tauri::AppHandle,
    store: tauri::State<'_, Store>,
    runtime: tauri::State<'_, Arc<Runtime>>,
    id: String,
) -> Result<(), String> {
    let _guard = runtime.gate.lock().await;
    let message = db(&store, |d| {
        let message: String = d
            .query_row(
                "SELECT json_extract(payload,'$.id') FROM jobs
                     WHERE id=?1 AND kind='mutation' AND status='uncertain'",
                [&id],
                |row| row.get(0),
            )
            .map_err(|_| "This action does not need review".to_string())?;
        reconcile::review_target(d, &message)
    })?;
    let observed = timed_mutation_observation(&store, &message).await?;
    db(&store, |d| {
        let tx = d.unchecked_transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE jobs SET status='reviewed',detail='User reviewed the mailbox action'
                    WHERE id=?1 AND status='uncertain'",
            [&id],
        )
        .map_err(|e| e.to_string())?;
        apply_mutation_observation(&tx, &message.id, observed)?;
        reconcile::reproject(&tx, &message.id)?;
        tx.commit().map_err(|e| e.to_string())
    })?;
    jobs_changed(&app, false);
    let _ = app.emit("mail-changed", ());
    Ok(())
}
#[tauri::command]
pub async fn pick_attachments(
    store: tauri::State<'_, Store>,
    account_id: String,
    draft_id: String,
) -> Result<Vec<engine::Attachment>, String> {
    outgoing::validate_draft_id(&draft_id)?;
    providers::account(&store, &account_id)?;
    let picked = tokio::task::spawn_blocking(|| {
        let paths = rfd::FileDialog::new()
            .set_title("Attach files (18 MiB total)")
            .pick_files()
            .unwrap_or_default();
        read_attachment_files(paths)
    })
    .await
    .map_err(|_| "File dialog unavailable")??;
    if picked.is_empty() {
        return Ok(Vec::new());
    }
    db(&store, |d| {
        let tx = d.unchecked_transaction().map_err(|e| e.to_string())?;
        let files = picked
            .into_iter()
            .map(|(name, data)| {
                outgoing::store_file(
                    &tx,
                    &account_id,
                    Some(&draft_id),
                    name,
                    "application/octet-stream".into(),
                    data,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(files)
    })
}

fn read_attachment_files(paths: Vec<std::path::PathBuf>) -> Result<Vec<(String, Vec<u8>)>, String> {
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
        let name = engine::safe_filename(
            path.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("attachment.bin"),
        );
        result.push((name, data));
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
    process_job(store, |_, _| {}).await
}

async fn process_job(store: &Store, on_change: impl Fn(bool, bool)) -> Result<bool, String> {
    let next: Option<(String, String, String)> = db(store, |d| {
        use rusqlite::OptionalExtension;
        d.query_row("SELECT j.id,j.kind,j.payload FROM jobs j WHERE j.status='pending' AND j.due<=?1
                     AND (j.kind!='mutation' OR NOT EXISTS (SELECT 1 FROM jobs u
                       WHERE u.account_id=j.account_id AND u.kind='mutation' AND u.status='uncertain'
                         AND json_extract(u.payload,'$.id')=json_extract(j.payload,'$.id')))
                     ORDER BY j.rowid LIMIT 1",[engine::now()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|e|e.to_string())
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
    on_change(false, false);
    let mut safe_to_restore = false;
    let mut uncertain = false;
    let mut mutation = None;
    let mut gmail_labels = None;
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
        match serde_json::from_str::<QueuedMutation>(&payload) {
            Ok(operation) => {
                let target = db(store, |d| reconcile::mutation_target(d, &operation.id));
                let outcome = match target {
                    Ok(mail) => providers::mutate(store, &mail, &operation.action)
                        .await
                        .map(|labels| gmail_labels = labels)
                        .map_err(|failure| {
                            uncertain = failure.uncertain;
                            failure.detail
                        }),
                    Err(error) => Err(error),
                };
                mutation = Some(operation);
                outcome
            }
            Err(_) => Err("Invalid operation".into()),
        }
    };
    let (status, detail) = match result {
        Ok(()) => ("done", "Provider accepted the operation".to_string()),
        Err(e) if kind == "send" && !safe_to_restore => (
            "uncertain",
            format!("{e}. Check Sent before composing another send; automatic retry is disabled."),
        ),
        Err(e) if uncertain => ("uncertain", e),
        Err(e) => ("failed", e),
    };
    db(store, |d| {
        let tx = d.unchecked_transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "UPDATE jobs SET status=?2,detail=?3 WHERE id=?1",
            params![id, status, detail],
        )
        .map_err(|e| e.to_string())?;
        if let Some(operation) = &mutation {
            if status == "done" {
                reconcile::confirm_action(&tx, &operation.id, &operation.action, gmail_labels)?;
            } else if status == "failed" {
                retire_if_superseded(&tx, &id, operation)?;
            }
            reconcile::reproject(&tx, &operation.id)?;
        }
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
        if kind == "send" && (status == "done" || safe_to_restore) {
            outgoing::reclaim_files(&tx, &outgoing::payload_files(&payload))?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    })?;
    on_change(
        safe_to_restore || (kind == "send" && status == "done"),
        mutation.is_some(),
    );
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
                |drafts, mail| {
                    jobs_changed(&app, drafts);
                    if mail {
                        let _ = app.emit("mail-changed", ());
                    }
                },
                account.map(|id| {
                    let app = &app;
                    let store = &store;
                    async move {
                        if sync(store, &id).await.is_ok() {
                            crate::notifications::after_sync(app, store, &id);
                        }
                        let _ = app.emit("sync-status-changed", ());
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
    on_job_change: impl Fn(bool, bool),
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

    #[test]
    fn snooze_can_be_rescheduled_and_ended_without_changing_the_mailbox() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        engine::migrate(&db).unwrap();
        let mail = Mail {
            id: "snoozed-message".into(),
            remote_id: "remote".into(),
            account_id: "account".into(),
            folder: "inbox".into(),
            ..Default::default()
        };
        engine::put(&db, &mail).unwrap();
        let now = engine::now();
        assert!(store_snooze(&db, "missing", now + 60).is_err());
        assert!(store_snooze(&db, &mail.id, now).is_err());
        store_snooze(&db, &mail.id, now + 3600).unwrap();
        store_snooze(&db, &mail.id, now + 7200).unwrap();
        assert_eq!(active_snooze(&db, &mail.id).unwrap(), Some(now + 7200));
        assert_eq!(
            engine::query_conversations(&db, "inbox", "", "", 0, None)
                .unwrap()
                .total,
            0
        );
        assert_eq!(
            engine::query_conversations(&db, "snoozed", "", "", 0, None)
                .unwrap()
                .total,
            1
        );
        clear_snooze(&db, &mail.id).unwrap();
        clear_snooze(&db, &mail.id).unwrap();
        assert_eq!(active_snooze(&db, &mail.id).unwrap(), None);
        assert_eq!(
            engine::query_conversations(&db, "inbox", "", "", 0, None)
                .unwrap()
                .total,
            1
        );
        assert_eq!(
            engine::query_conversations(&db, "snoozed", "", "", 0, None)
                .unwrap()
                .total,
            0
        );
        assert_eq!(engine::get(&db, &mail.id).unwrap().folder, "inbox");
    }

    #[test]
    fn account_sync_failure_keeps_the_previous_success_and_recovery_clears_the_error() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        engine::migrate(&db).unwrap();
        db.execute_batch(
            "CREATE TABLE accounts(id TEXT PRIMARY KEY);
            INSERT INTO accounts VALUES('a'),('b');",
        )
        .unwrap();
        engine::checkpoint(&db, "a", "sync-success", "42").unwrap();
        record_sync_status(&db, "a", &Err("Synthetic provider offline".into())).unwrap();
        record_sync_status(&db, "b", &Ok(1)).unwrap();
        let states = sync_statuses(&db).unwrap();
        let first = states
            .iter()
            .find(|status| status.account_id == "a")
            .unwrap();
        assert_eq!(first.last_success, Some(42));
        assert_eq!(first.error.as_deref(), Some("Synthetic provider offline"));
        let second = states
            .iter()
            .find(|status| status.account_id == "b")
            .unwrap();
        assert!(second.last_success.is_some() && second.error.is_none());
        record_sync_status(&db, "a", &Ok(0)).unwrap();
        let first = sync_statuses(&db)
            .unwrap()
            .into_iter()
            .find(|status| status.account_id == "a")
            .unwrap();
        assert!(first.last_success.unwrap() > 42);
        assert!(first.error.is_none());
    }

    #[test]
    fn failed_actions_revert_only_their_field_and_cannot_retry_after_newer_intent() {
        let database = rusqlite::Connection::open_in_memory().unwrap();
        engine::migrate(&database).unwrap();
        let store = Store(Arc::new(std::sync::Mutex::new(database)));
        let message = Mail {
            id: "message".into(),
            account_id: "account".into(),
            remote_id: "remote".into(),
            folder: "inbox".into(),
            unread: true,
            ..Default::default()
        };
        db(&store, |d| engine::put(d, &message)).unwrap();
        let read = queue_mutation_inner(&store, "message", "read").unwrap();
        let star = queue_mutation_inner(&store, "message", "star").unwrap();
        db(&store, |d| {
            d.execute("UPDATE jobs SET status='failed' WHERE id=?1", [&read])
                .unwrap();
            reconcile::reproject(d, "message")?;
            let mail = engine::get(d, "message")?;
            assert!(mail.unread && mail.starred);
            Ok(())
        })
        .unwrap();
        retry_mutation_inner(&store, &read).unwrap();
        db(&store, |d| {
            let mail = engine::get(d, "message")?;
            assert!(!mail.unread && mail.starred);
            d.execute("UPDATE jobs SET status='failed' WHERE id=?1", [&read])
                .unwrap();
            reconcile::reproject(d, "message")
        })
        .unwrap();
        queue_mutation_inner(&store, "message", "unread").unwrap();
        assert!(retry_mutation_inner(&store, &read).is_err());
        db(&store, |d| {
            d.execute("UPDATE jobs SET status='running' WHERE id=?1", [&star])
                .unwrap();
            engine::migrate(d)?;
            let mail = engine::get(d, "message")?;
            assert!(mail.unread && !mail.starred);
            let status: String = d
                .query_row("SELECT status FROM jobs WHERE id=?1", [&star], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(status, "uncertain");
            Ok(())
        })
        .unwrap();
    }

    #[tokio::test]
    async fn due_job_runs_between_account_syncs() {
        let database = rusqlite::Connection::open_in_memory().unwrap();
        engine::migrate(&database).unwrap();
        let store = Store(Arc::new(std::sync::Mutex::new(database)));
        let runtime = Runtime::default();
        let trace = std::cell::RefCell::new(Vec::new());
        let on_change = |_, _| {
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
